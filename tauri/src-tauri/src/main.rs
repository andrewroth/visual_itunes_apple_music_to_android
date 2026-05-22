#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri desktop entry point.
//!
//! All actual logic — library loading, sync, settings — is in
//! `musicsync-core` and `sync.rs`. This file just wires those into Tauri
//! `#[tauri::command]` functions that the frontend can call via `invoke`,
//! plus an `emit` channel for backend-pushed events (progress, log lines).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use musicsync_core::library::Library;
use musicsync_core::settings::{PlaylistAction, RememberedPlaylist, Settings};
use std::io::Write as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

mod discovery;
mod pair;
mod sync;

/// Long-lived in-memory state. Wrapped in a Mutex because Tauri commands can
/// be invoked concurrently from the frontend, though in practice the UI
/// serialises user actions.
#[derive(Default)]
struct AppState {
    settings_path: PathBuf,
    settings: Settings,
    library: Option<Library>,
    /// mtime of the Library.xml when we last parsed it. Carried in the
    /// LibraryView from subsequent commands so the "exported …" banner
    /// keeps its timestamp across playlist toggles / forgets.
    library_mtime_ms: Option<u64>,
    /// Sender side of the in-flight pair handshake's user-confirmation
    /// channel. `start_pairing` puts a sender here when it suspends waiting
    /// for the user; `pair_confirm` / `pair_cancel` take it back out and
    /// signal the result. `None` means there is no pair in progress.
    pair_confirm_tx: Option<oneshot::Sender<bool>>,
    /// Cached result of the most recent scan_device call. Holds the
    /// device-side files plus computed unused-track ids, so the Sync
    /// command can reuse them without re-fetching the manifest.
    last_scan: Option<LastScan>,
    /// Set to true when the user clicks "Stop sync". The run_sync task
    /// polls this between file uploads and exits early. Reset to false at
    /// the start of every run_sync.
    abort_sync: Arc<AtomicBool>,
    /// In-flight heartbeat task. Each call to start_heartbeat aborts the
    /// previous one so we never have two heartbeats fighting over the UI.
    heartbeat_task: Option<tauri::async_runtime::JoinHandle<()>>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct LastScan {
    // Note: kept field name `unused_device_paths` for backwards compat
    // with on-disk snapshot, but it now contains *orphan-only* paths:
    // device files whose size doesn't match ANY iTunes track. Per-
    // playlist cleanups are computed fresh at sync time.
    /// Manifest paths the phone reported, in scan order.
    device_files: Vec<(String, u64)>,
    /// Tracks that are on the device but not in any currently-checked
    /// playlist. Device-side paths (relative-ish, what FILE_DELETE expects).
    unused_device_paths: Vec<String>,
    /// Map of on-device playlist basename (e.g. "Favourites") → number of
    /// path entries inside its .m3u. Used to populate the "Device # Tracks"
    /// column matching the Ruby app's table.
    device_playlist_line_counts: std::collections::HashMap<String, usize>,
    timestamp_ms: u64,
    music_root: String,
}

#[derive(Serialize, Clone)]
struct PlaylistView {
    playlist_id: String,
    name: String,
    /// Track count from iTunes Library.xml.
    track_count: usize,
    /// Number of paths in the on-device .m3u (what the phone thinks this
    /// playlist contains). `None` until a scan has been done, or if no
    /// matching .m3u exists on the device yet.
    device_tracks_count: Option<usize>,
    /// How many of this playlist's tracks would actually be uploaded if
    /// Sync ran right now (i.e. not size-matched as on-device).
    /// `None` until a scan has been done.
    tracks_to_copy: Option<usize>,
    checked: bool,
    /// For UNCHECKED playlists only: count of this playlist's tracks
    /// that exist on the phone but aren't in any currently-checked
    /// playlist (so syncing wouldn't put them back). `None` until a
    /// scan has been done, or for checked playlists.
    cleanup_count: Option<usize>,
    /// True iff the user has ticked the per-row Clean-up checkbox for
    /// this playlist. Only meaningful for unchecked playlists.
    cleanup_checked: bool,
}

#[derive(Serialize, Clone)]
struct LibraryView {
    track_count: usize,
    /// Files on the phone whose size doesn't match any iTunes track —
    /// these aren't owned by any playlist. The orphan row in the table
    /// surfaces this count + checkbox. `None` until a scan has happened.
    orphan_count: Option<usize>,
    /// Pre-computed preview of what `run_sync` would actually do right
    /// now. None until a scan has happened. Properly dedupes across
    /// playlists (a track in two checked playlists counts once).
    preview: Option<SyncPreview>,
    playlists: Vec<PlaylistView>,
    music_folder: String,
    /// Local path of the Library.xml file that was parsed.
    library_path: String,
    /// Last-modified time of the Library.xml on disk, Unix epoch ms. Lets
    /// the UI show "your export is from 14:23:11" so the user knows whether
    /// a re-export has been picked up.
    library_mtime_ms: Option<u64>,
    /// Previously-checked playlists that don't appear in the current
    /// library. The UI shows these in a "no longer in library" section with
    /// Forget / Ignore / Delete controls.
    missing_playlists: Vec<MissingPlaylistView>,
}

#[derive(Serialize, Clone, Default)]
struct PreviewSong {
    name: String,
    artist: String,
    /// For adds: local source path on the desktop. For deletes: device
    /// path that will be sent in FILE_DELETE. Shown in the 💻/📱 icon
    /// hover for that side.
    path: String,
    /// Device-side path (under the phone's music root). Populated for
    /// adds; for deletes the `path` above already IS the device path,
    /// so this stays empty.
    #[serde(default)]
    device_path: String,
}

#[derive(Serialize, Clone, Default)]
struct PreviewPlaylist {
    name: String,
    /// The .m3u filename written to the phone (e.g. "Favourites.m3u").
    filename: String,
}

#[derive(Serialize, Clone, Default)]
struct SyncPreview {
    /// Counts (used for the one-line preview).
    new_playlists: usize,
    new_songs: usize,
    remove_playlists: usize,
    delete_songs: usize,
    /// Itemised entries for the Details… dialog.
    new_playlist_items: Vec<PreviewPlaylist>,
    new_song_items: Vec<PreviewSong>,
    remove_playlist_items: Vec<PreviewPlaylist>,
    delete_song_items: Vec<PreviewSong>,
    /// Songs the plan wants to upload but whose `local_path` couldn't be
    /// read on the desktop (file moved/renamed/deleted in the Music
    /// folder since the Library.xml export). UI flags the status banner
    /// red and shows these at the top of Details. NOT included in
    /// new_song_items (we wouldn't be able to upload them anyway).
    missing_files: Vec<PreviewSong>,
}

#[derive(Serialize, Clone)]
struct MissingPlaylistView {
    id: String,
    name: String,
    /// "ignore" | "delete" — must match the snake_case serde repr.
    action: String,
}

#[tauri::command]
fn load_settings(state: State<Mutex<AppState>>) -> Result<Settings, String> {
    let s = state.lock().unwrap();
    Ok(s.settings.clone())
}

#[tauri::command]
fn save_settings(
    new: Settings,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    let mut s = state.lock().unwrap();
    new.save(&s.settings_path).map_err(|e| e.to_string())?;
    s.settings = new;
    Ok(())
}

#[tauri::command]
fn load_library(app: AppHandle, state: State<Mutex<AppState>>) -> Result<LibraryView, String> {
    let mut guard = state.lock().unwrap();
    let library_path = guard
        .settings
        .library_path
        .clone()
        .ok_or_else(|| "library_path not set in settings".to_string())?;
    let device_root = guard
        .settings
        .ftp_path
        .clone()
        .unwrap_or_else(|| "/sdcard/Music/".into());

    let lib_path = std::path::PathBuf::from(&library_path);
    let mut lib = Library::parse_file(&lib_path, &device_root, &guard.settings)
        .map_err(|e| e.to_string())?;
    let verbose = guard.settings.verbose_logging;

    // Re-apply the most recent device scan so on_device flags survive a
    // library reload. Without this, anything that triggers load_library
    // after scan_device (e.g. the post-scan refresh in main.js) wipes the
    // match results and the UI reports every track as needing to copy.
    if let Some(scan) = guard.last_scan.as_ref() {
        use musicsync_core::matching::{mark_on_device_strict, DeviceFile};
        let dfs: Vec<DeviceFile> = scan
            .device_files
            .iter()
            .map(|(p, s)| DeviceFile { path: p.clone(), size: *s })
            .collect();
        mark_on_device_strict(&mut lib.tracks, &dfs);
    }

    // Read mtime so the UI can show when the Library.xml was last exported.
    let mtime_ms = std::fs::metadata(&lib_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);

    log_library_inventory(&app, verbose, &library_path, &lib);

    let (playlists, missing) = build_views_with_scan(&lib, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(&lib, &guard.settings, s));
    let view = library_view(&lib, library_path, mtime_ms, playlists, missing, guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()), preview);
    guard.library = Some(lib);
    guard.library_mtime_ms = mtime_ms;
    Ok(view)
}

#[tauri::command]
fn toggle_playlist(
    playlist_id: String,
    state: State<Mutex<AppState>>,
) -> Result<LibraryView, String> {
    let mut guard = state.lock().unwrap();
    let library_path_str = guard.settings.library_path.clone().unwrap_or_default();

    // Phase 1: mutate the library and gather snapshots we'll need later,
    // releasing the mutable borrow before we touch other fields of guard.
    let (toggled_name, currently_checked, library_ids, checked_ids_in_library): (
        Option<String>,
        bool,
        std::collections::HashSet<String>,
        Vec<String>,
    );
    {
        let lib = guard
            .library
            .as_mut()
            .ok_or_else(|| "library not loaded".to_string())?;
        let mut name: Option<String> = None;
        for p in &mut lib.playlists {
            if p.playlist_id == playlist_id {
                p.checked = !p.checked;
                name = Some(p.name.clone());
            }
        }
        toggled_name = name;
        currently_checked = lib
            .playlists
            .iter()
            .find(|p| p.playlist_id == playlist_id)
            .map(|p| p.checked)
            .unwrap_or(false);
        library_ids = lib.playlists.iter().map(|p| p.playlist_id.clone()).collect();
        checked_ids_in_library = lib
            .playlists
            .iter()
            .filter(|p| p.checked)
            .map(|p| p.playlist_id.clone())
            .collect();
    }

    // Phase 2: reconcile settings. Carry over IDs for playlists NOT in the
    // current library (the "missing" ones) so they re-check automatically
    // if the playlist reappears; then layer on the currently-checked IDs.
    let mut new_checked: Vec<String> = guard
        .settings
        .checked_playlist_ids
        .iter()
        .filter(|id| !library_ids.contains(*id))
        .cloned()
        .collect();
    new_checked.extend(checked_ids_in_library);
    guard.settings.checked_playlist_ids = new_checked;

    // Add a remembered-playlist record on first check; ignore (don't add)
    // when toggling off. Forget is the explicit way to remove.
    if let (Some(name), true) = (toggled_name, currently_checked) {
        let already = guard
            .settings
            .remembered_playlists
            .iter()
            .any(|r| r.id == playlist_id);
        if !already {
            guard.settings.remembered_playlists.push(RememberedPlaylist {
                id: playlist_id.clone(),
                name,
                action: PlaylistAction::Ignore,
            });
        }
    }

    // Phase 3: build view + persist. By this point the mutable borrow on
    // lib is gone, so we can read both library and settings together.
    let mtime_ms = guard.library_mtime_ms;
    let lib_ref = guard.library.as_ref().unwrap();
    let (playlists_snapshot, missing) = build_views_with_scan(lib_ref, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(lib_ref, &guard.settings, s));
    let view = library_view(lib_ref, library_path_str, mtime_ms, playlists_snapshot, missing, guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()), preview);

    let settings_path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    drop(guard);
    to_save.save(&settings_path).map_err(|e| e.to_string())?;
    Ok(view)
}

#[tauri::command]
fn forget_playlist(
    playlist_id: String,
    state: State<Mutex<AppState>>,
) -> Result<LibraryView, String> {
    let mut guard = state.lock().unwrap();
    guard.settings.checked_playlist_ids.retain(|id| id != &playlist_id);
    guard.settings.remembered_playlists.retain(|r| r.id != playlist_id);
    let settings_path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    let library_path_str = guard.settings.library_path.clone().unwrap_or_default();
    to_save.save(&settings_path).map_err(|e| e.to_string())?;

    let mtime_ms = guard.library_mtime_ms;
    let lib = guard
        .library
        .as_ref()
        .ok_or_else(|| "library not loaded".to_string())?;
    let (playlists_snapshot, missing) = build_views_with_scan(lib, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(lib, &guard.settings, s));
    Ok(library_view(lib, library_path_str, mtime_ms, playlists_snapshot, missing, guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()), preview))
}

#[tauri::command]
fn set_playlist_action(
    playlist_id: String,
    action: String,
    state: State<Mutex<AppState>>,
) -> Result<LibraryView, String> {
    let action = match action.as_str() {
        "delete" => PlaylistAction::Delete,
        _ => PlaylistAction::Ignore,
    };
    let mut guard = state.lock().unwrap();
    for r in guard.settings.remembered_playlists.iter_mut() {
        if r.id == playlist_id {
            r.action = action;
        }
    }
    let settings_path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    let library_path_str = guard.settings.library_path.clone().unwrap_or_default();
    to_save.save(&settings_path).map_err(|e| e.to_string())?;

    let mtime_ms = guard.library_mtime_ms;
    let lib = guard
        .library
        .as_ref()
        .ok_or_else(|| "library not loaded".to_string())?;
    let (playlists_snapshot, missing) = build_views_with_scan(lib, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(lib, &guard.settings, s));
    Ok(library_view(lib, library_path_str, mtime_ms, playlists_snapshot, missing, guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()), preview))
}

/// Compute the playlist and missing-playlist view-models from current state.
/// Takes an optional last-scan reference so the per-playlist "Device #
/// Tracks" / "Tracks To Copy" columns reflect actual device contents.
fn build_views_with_scan(
    lib: &Library,
    settings: &Settings,
    last_scan: Option<&LastScan>,
) -> (Vec<PlaylistView>, Vec<MissingPlaylistView>) {
    let have_scan = last_scan.is_some();

    // Set of track IDs that ARE in some currently-checked playlist —
    // anything outside this set is a deletion candidate for cleanup.
    let checked_track_ids: std::collections::HashSet<&str> = lib
        .playlists
        .iter()
        .filter(|p| p.checked)
        .flat_map(|p| p.track_ids.iter().map(|s| s.as_str()))
        .collect();

    // Sizes on device for quick "is this track actually on the phone?" lookup.
    let device_sizes: std::collections::HashSet<u64> = last_scan
        .map(|s| s.device_files.iter().map(|(_, sz)| *sz).collect())
        .unwrap_or_default();

    let cleanup_flagged: std::collections::HashSet<&str> = settings
        .cleanup_playlist_ids
        .iter()
        .map(|s| s.as_str())
        .collect();

    let playlists = lib
        .playlists
        .iter()
        .map(|p| {
            let to_copy = if have_scan {
                let mut n = 0usize;
                for tid in &p.track_ids {
                    if let Some(t) = lib.tracks.get(tid) {
                        if !t.on_device { n += 1; }
                    }
                }
                Some(n)
            } else { None };

            let device_count = last_scan
                .and_then(|s| s.device_playlist_line_counts.get(&p.name).copied());

            // Cleanup count: only meaningful for unchecked playlists.
            // Count this playlist's tracks that are on the device AND
            // not in any other checked playlist.
            let cleanup_count = if have_scan && !p.checked {
                let mut n = 0usize;
                for tid in &p.track_ids {
                    if checked_track_ids.contains(tid.as_str()) { continue; }
                    if let Some(t) = lib.tracks.get(tid) {
                        if device_sizes.contains(&t.size) { n += 1; }
                    }
                }
                Some(n)
            } else { None };

            PlaylistView {
                playlist_id: p.playlist_id.clone(),
                name: p.name.clone(),
                track_count: p.track_ids.len(),
                device_tracks_count: device_count,
                tracks_to_copy: to_copy,
                checked: p.checked,
                cleanup_count,
                cleanup_checked: cleanup_flagged.contains(p.playlist_id.as_str()),
            }
        })
        .collect();

    let library_ids: std::collections::HashSet<&str> =
        lib.playlists.iter().map(|p| p.playlist_id.as_str()).collect();
    let missing = settings
        .remembered_playlists
        .iter()
        .filter(|r| !library_ids.contains(r.id.as_str()))
        .map(|r| MissingPlaylistView {
            id: r.id.clone(),
            name: r.name.clone(),
            action: match r.action {
                PlaylistAction::Ignore => "ignore".into(),
                PlaylistAction::Delete => "delete".into(),
            },
        })
        .collect();

    (playlists, missing)
}

#[tauri::command]
async fn run_sync(
    ws_url: String,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<String, String> {
    // Reset the abort flag at the start of each sync so a stale signal
    // from a previously-cancelled run can't immediately stop this one.
    let abort_flag = {
        let guard = state.lock().unwrap();
        guard.abort_sync.store(false, Ordering::SeqCst);
        guard.abort_sync.clone()
    };
    let _ = app.emit("sync_started", ());

    // Take an owned snapshot of the data we need, dropping the lock before
    // awaiting (Mutex isn't Send across await points).
    let (mut tracks_clone, playlists_clone, token, playlists_to_delete, tracks_to_delete) = {
        let guard = state.lock().unwrap();
        let lib = guard
            .library
            .as_ref()
            .ok_or_else(|| "library not loaded".to_string())?;
        // Delete-list = remembered playlists with action=Delete. We use
        // `<name>.m3u` as the on-device filename, matching how Playlist
        // generation in core/src/playlist.rs writes it.
        let pl_to_delete: Vec<String> = guard
            .settings
            .remembered_playlists
            .iter()
            .filter(|r| matches!(r.action, PlaylistAction::Delete))
            .map(|r| format!("{}.m3u", r.name))
            .collect();
        // Unused-track delete-list comes from the last_scan cache and is
        // only honored when the user has explicitly opted in.
        let mut tr_to_delete: Vec<String> = if guard.settings.delete_unused_songs {
            guard
                .last_scan
                .as_ref()
                .map(|s| s.unused_device_paths.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // Per-playlist cleanup: for each playlist whose Delete checkbox
        // is ticked, queue its tracks that are on the phone but not in
        // any currently-checked playlist.
        let cleanup_ids: std::collections::HashSet<&str> = guard
            .settings
            .cleanup_playlist_ids
            .iter()
            .map(|s| s.as_str())
            .collect();
        let checked_track_ids: std::collections::HashSet<&str> = lib
            .playlists
            .iter()
            .filter(|p| p.checked)
            .flat_map(|p| p.track_ids.iter().map(|s| s.as_str()))
            .collect();
        let device_path_by_size: std::collections::HashMap<u64, String> = guard
            .last_scan
            .as_ref()
            .map(|s| {
                let mut m = std::collections::HashMap::new();
                for (p, sz) in &s.device_files {
                    if p.to_ascii_lowercase().ends_with(".m3u") { continue; }
                    m.entry(*sz).or_insert_with(|| p.clone());
                }
                m
            })
            .unwrap_or_default();
        for p in &lib.playlists {
            if !cleanup_ids.contains(p.playlist_id.as_str()) { continue; }
            for tid in &p.track_ids {
                if checked_track_ids.contains(tid.as_str()) { continue; }
                if let Some(t) = lib.tracks.get(tid) {
                    if let Some(path) = device_path_by_size.get(&t.size) {
                        tr_to_delete.push(path.clone());
                    }
                }
            }
        }
        tr_to_delete.sort();
        tr_to_delete.dedup();
        (
            lib.tracks.clone(),
            lib.playlists.clone(),
            guard.settings.device_token.clone().unwrap_or_default(),
            pl_to_delete,
            tr_to_delete,
        )
    };

    let progress_app = app.clone();
    let scan_started_app = app.clone();
    let scan_complete_app = app.clone();
    let started = std::time::Instant::now();
    let report = sync::run_sync(
        &ws_url,
        &token,
        &mut tracks_clone,
        &playlists_clone,
        &playlists_to_delete,
        &tracks_to_delete,
        abort_flag.clone(),
        move |msg, fraction| {
            let _ = progress_app.emit("progress", ProgressEvent {
                message: msg.to_string(),
                fraction,
            });
        },
        move || {
            let _ = scan_started_app.emit("scan_started", ());
        },
        move |files, playlists| {
            let timestamp_ms = now_ms();
            let _ = scan_complete_app.emit(
                "scan_complete",
                ScanCompleteEvent { files, playlists, timestamp_ms },
            );
        },
    )
    .await
    .map_err(|e| {
        let _ = app.emit("sync_ended", ());
        e.to_string()
    })?;
    let _ = app.emit("sync_ended", ());

    // Write the updated on_device flags back into app state.
    {
        let mut guard = state.lock().unwrap();
        if let Some(lib) = guard.library.as_mut() {
            for (id, t) in tracks_clone {
                if let Some(existing) = lib.tracks.get_mut(&id) {
                    existing.on_device = t.on_device;
                }
            }
        }
    }

    let elapsed = started.elapsed();
    Ok(format_sync_summary(&report, elapsed))
}

fn format_sync_summary(report: &sync::SyncReport, elapsed: std::time::Duration) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "Sync complete in {}.",
        format_duration(elapsed),
    ));

    if report.uploaded_tracks > 0 {
        parts.push(format!(
            "Copied {} track{}",
            report.uploaded_tracks,
            if report.uploaded_tracks == 1 { "" } else { "s" },
        ));
    }
    if report.uploaded_playlists > 0 {
        parts.push(format!(
            "wrote {} playlist{}",
            report.uploaded_playlists,
            if report.uploaded_playlists == 1 { "" } else { "s" },
        ));
    }
    if report.deleted_files > 0 {
        parts.push(format!(
            "deleted {} file{}",
            report.deleted_files,
            if report.deleted_files == 1 { "" } else { "s" },
        ));
    }
    if report.uploaded_tracks == 0
        && report.uploaded_playlists == 0
        && report.deleted_files == 0
    {
        parts.push(format!(
            "nothing to do — {} track{} already on phone",
            report.already_present,
            if report.already_present == 1 { "" } else { "s" },
        ));
    } else {
        parts.push(format!("{} already present", report.already_present));
    }

    let mut summary = parts.join(" · ");
    summary.push('.');
    if !report.errors.is_empty() {
        summary.push_str(&format!(
            " {} error{}: {}",
            report.errors.len(),
            if report.errors.len() == 1 { "" } else { "s" },
            report.errors.join("; "),
        ));
    }
    summary
}

fn format_duration(d: std::time::Duration) -> String {
    let total_sec = d.as_secs();
    if total_sec < 1 {
        return format!("{} ms", d.as_millis());
    }
    if total_sec < 60 {
        return format!("{}s", total_sec);
    }
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

#[derive(Serialize, Clone)]
struct ProgressEvent {
    message: String,
    fraction: Option<f32>,
}

#[derive(Serialize, Clone)]
struct ScanCompleteEvent {
    files: usize,
    playlists: usize,
    /// Unix epoch milliseconds. JS formats with the user's locale.
    timestamp_ms: u64,
}

#[derive(Serialize, Clone)]
struct PairChallengeEvent {
    code: String,
    device_name: String,
}

/// Emitted when the phone pushes DEVICE_RENAMED over the heartbeat
/// connection. The frontend updates the "paired with X" banner in place
/// — no rescan or reconnect.
#[derive(Serialize, Clone)]
struct PairedDeviceRenamedEvent {
    device_id: String,
    device_name: String,
}

#[derive(Serialize, Clone)]
struct PairResultEvent {
    device_id: String,
    device_name: String,
    music_root: String,
}

/// Starts the bluetooth-style numeric comparison. Returns when pairing
/// either succeeds (token saved to settings) or fails. During the wait,
/// emits `pair_challenge` so the frontend can show the comparison dialog.
#[tauri::command]
async fn start_pairing(
    ws_url: String,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<PairResultEvent, String> {
    let (tx, rx) = oneshot::channel::<bool>();
    {
        let mut guard = state.lock().unwrap();
        // If a previous pair was still pending (shouldn't happen but defend
        // anyway), cancel it so we don't leak the sender.
        if let Some(old) = guard.pair_confirm_tx.take() {
            let _ = old.send(false);
        }
        guard.pair_confirm_tx = Some(tx);
    }

    let outcome_app = app.clone();
    let outcome = pair::run_pairing(
        &ws_url,
        move |code, device_name| {
            let _ = outcome_app.emit(
                "pair_challenge",
                PairChallengeEvent {
                    code: code.to_string(),
                    device_name: device_name.to_string(),
                },
            );
        },
        async move { rx.await.unwrap_or(false) },
    )
    .await
    .map_err(|e| e.to_string());

    // Always clear the slot so a future re-attempt starts clean.
    {
        let mut guard = state.lock().unwrap();
        guard.pair_confirm_tx = None;
    }

    let outcome = outcome?;
    let settings_path = {
        let mut guard = state.lock().unwrap();
        guard.settings.device_token = Some(outcome.token.clone());
        guard.settings.ftp_path = Some(outcome.music_root.clone());
        guard.settings.paired_device_name = Some(outcome.device_name.clone());
        if !outcome.device_id.is_empty() {
            guard.settings.paired_device_id = Some(outcome.device_id.clone());
        }
        guard.settings_path.clone()
    };
    let snapshot = {
        let guard = state.lock().unwrap();
        guard.settings.clone()
    };
    snapshot.save(&settings_path).map_err(|e| e.to_string())?;

    Ok(PairResultEvent {
        device_id: outcome.device_id,
        device_name: outcome.device_name,
        music_root: outcome.music_root,
    })
}

/// Called from the frontend's pair-confirm modal. Resolves the suspended
/// `start_pairing` task; either `true` (confirm) or `false` (cancel).
#[tauri::command]
fn pair_confirm(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    let tx = guard
        .pair_confirm_tx
        .take()
        .ok_or_else(|| "no pairing in progress".to_string())?;
    let _ = tx.send(true);
    Ok(())
}

#[derive(Serialize, Clone)]
struct ScanResultEvent {
    files: usize,
    playlists: usize,
    unused: usize,
    timestamp_ms: u64,
}

/// Connect to the paired phone, fetch its manifest, compute which on-device
/// tracks aren't in any currently-checked playlist, and cache the result
/// for the next Sync. Doesn't upload or delete anything; safe to run
/// whenever we have a phone address + token.
#[tauri::command]
async fn scan_device(
    ws_url: String,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<ScanResultEvent, String> {
    use musicsync_core::matching::{mark_on_device_strict, DeviceFile};
    use std::collections::HashSet;

    let (mut tracks_clone, _playlists_clone, token) = {
        let guard = state.lock().unwrap();
        let lib = guard
            .library
            .as_ref()
            .ok_or_else(|| "library not loaded".to_string())?;
        (
            lib.tracks.clone(),
            lib.playlists.clone(),
            guard.settings.device_token.clone().unwrap_or_default(),
        )
    };

    let progress_app = app.clone();
    let (device_files, device_playlists, music_root) =
        sync::fetch_manifest_full(&ws_url, &token, move |msg, fraction| {
            vlog_dyn(
                &progress_app,
                format!(
                    "scan_progress recv from phone: message={msg:?} fraction={fraction:?}"
                ),
            );
            let _ = progress_app.emit(
                "scan_progress",
                ProgressEvent { message: msg.to_string(), fraction },
            );
            vlog_dyn(
                &progress_app,
                format!(
                    "scan_progress emitted to frontend: message={msg:?} fraction={fraction:?}"
                ),
            );
        })
        .await
        .map_err(|e| e.to_string())?;

    // Compute count of non-empty / non-header lines for each on-device
    // playlist, keyed by basename without the .m3u extension. Matches
    // Ruby's device_tracks_count semantics.
    let mut device_playlist_line_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for p in &device_playlists {
        let n = p
            .content
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && t != "#EXTM3U" && !t.starts_with('#')
            })
            .count();
        let basename = p
            .name
            .rsplit('/')
            .next()
            .unwrap_or(&p.name)
            .strip_suffix(".m3u")
            .unwrap_or(&p.name)
            .to_string();
        device_playlist_line_counts.insert(basename, n);
    }

    // Mark on_device status. Then compute orphans: device files whose
    // size doesn't match ANY iTunes track in the library. These are
    // strictly "we have no idea what these are" files — never owned by
    // any playlist. The per-playlist Delete checkboxes handle the
    // separate case of "this playlist's tracks no longer want them."
    let dfs: Vec<DeviceFile> = device_files
        .iter()
        .map(|(p, s)| DeviceFile { path: p.clone(), size: *s })
        .collect();

    // Verbose dump BEFORE matching so the log has a full picture of
    // what's on the phone vs what iTunes knows about. The user can grep
    // for a specific filename and see both sides.
    let verbose = {
        let guard = state.lock().unwrap();
        guard.settings.verbose_logging
    };
    if verbose {
        vlog(&app, true, format!(
            "=== scan_device start: {} iTunes tracks, {} device files",
            tracks_clone.len(), device_files.len(),
        ));
        // Histogram of device-file sizes to spot duplicates.
        let mut size_counts: std::collections::HashMap<u64, usize> =
            std::collections::HashMap::new();
        for (_, sz) in &device_files {
            *size_counts.entry(*sz).or_insert(0) += 1;
        }
        vlog(&app, true, format!(
            "device file size distribution: {} unique sizes, {} duplicates",
            size_counts.len(),
            size_counts.values().filter(|&&v| v > 1).count(),
        ));
        // Dump every device file. Verbose mode is opt-in and meant for
        // diagnosing real issues — truncating defeats the point.
        for (i, (p, sz)) in device_files.iter().enumerate() {
            vlog(&app, true, format!("  device[{i}] size={sz} path={p:?}"));
        }
        log_device_manifest_playlists(&app, true, &device_playlists);
    }
    mark_on_device_strict(&mut tracks_clone, &dfs);

    if verbose {
        // Per-track outcome. Verbose mode dumps EVERY track on both
        // sides — no sampling — so the log lets you grep any filename
        // and see exactly what happened to it.
        let device_sizes: std::collections::HashSet<u64> =
            device_files.iter().map(|(_, s)| *s).collect();
        let mut matched = 0usize;
        let mut unmatched = 0usize;
        for t in tracks_clone.values() {
            if t.on_device {
                matched += 1;
                vlog(&app, true, format!(
                    "MATCHED  track id={} size={} name={:?} -> some device file with size {}",
                    t.id, t.size, t.name, t.size,
                ));
            } else {
                unmatched += 1;
                let nearby: Vec<u64> = device_sizes
                    .iter()
                    .filter(|s| (**s as i64 - t.size as i64).abs() < 1024)
                    .copied()
                    .take(5)
                    .collect();
                vlog(&app, true, format!(
                    "MISSING  track id={} size={} name={:?} local={:?} \
                     | nearby device sizes (±1KB): {:?}",
                    t.id, t.size, t.name, t.local_path, nearby,
                ));
            }
        }
        vlog(&app, true, format!(
            "=== matching summary: {} matched, {} unmatched (out of {})",
            matched, unmatched, tracks_clone.len(),
        ));
    }

    let library_sizes: HashSet<u64> = tracks_clone.values().map(|t| t.size).collect();
    let mut unused_paths: Vec<String> = Vec::new();
    for (path, size) in &device_files {
        if path.to_ascii_lowercase().ends_with(".m3u") { continue; }
        if !library_sizes.contains(size) {
            unused_paths.push(path.clone());
        }
    }
    unused_paths.sort();
    unused_paths.dedup();

    let timestamp_ms = now_ms();
    let result = ScanResultEvent {
        files: device_files.len(),
        playlists: 0, // playlists count is part of manifest but we don't surface here
        unused: unused_paths.len(),
        timestamp_ms,
    };

    // If the phone now reports a music_root that differs from what we
    // baked into Track.device_path values (settings.ftp_path), rebase
    // every track in place so subsequent uploads send relative paths
    // that match the phone's current root. Persist the new ftp_path so
    // future scan/sync runs and a fresh app start see the same value.
    let mut settings_to_save: Option<(Settings, std::path::PathBuf)> = None;
    {
        let mut guard = state.lock().unwrap();
        let old_root = guard.settings.ftp_path.clone().unwrap_or_default();
        let root_changed = !old_root.is_empty() && old_root != music_root;
        if root_changed {
            if let Some(lib) = guard.library.as_mut() {
                for t in lib.tracks.values_mut() {
                    t.rebase_device_path(&old_root, &music_root);
                }
            }
            for t in tracks_clone.values_mut() {
                t.rebase_device_path(&old_root, &music_root);
            }
            guard.settings.ftp_path = Some(music_root.clone());
            settings_to_save = Some((guard.settings.clone(), guard.settings_path.clone()));
            vlog(&app, verbose, format!(
                "music_root changed: was {old_root:?}, now {music_root:?} — rebased {} tracks",
                tracks_clone.len(),
            ));
        }

        guard.last_scan = Some(LastScan {
            device_files,
            unused_device_paths: unused_paths,
            device_playlist_line_counts,
            timestamp_ms,
            music_root,
        });
        // Propagate on_device flags into the cached library so the next
        // build_views() reports accurate per-playlist completion counts.
        if let Some(lib) = guard.library.as_mut() {
            for (id, t) in &tracks_clone {
                if let Some(existing) = lib.tracks.get_mut(id) {
                    existing.on_device = t.on_device;
                }
            }
        }
    }
    if let Some((settings, path)) = settings_to_save {
        let _ = settings.save(&path);
    }

    Ok(result)
}

/// Write a verbose-debug line to both the Log tab (via the `log_line`
/// event) and a dated file `musicsync-YYYY-MM-DD.log` in the working
/// directory. No-op when `verbose_logging` is off.
fn vlog(app: &AppHandle, verbose: bool, line: impl AsRef<str>) {
    if !verbose { return; }
    let s = line.as_ref();
    tracing::info!("[vlog] {s}");
    // Emit to frontend Log tab.
    let _ = app.emit("log_line", s.to_string());
    // Append to dated file. Best-effort; never panic if disk is full.
    let date = chrono_today_yyyy_mm_dd();
    let path = format!("musicsync-{date}.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) {
        let _ = writeln!(f, "{} {}", chrono_now_hms(), s);
    }
}

/// Same as [vlog] but reads the `verbose_logging` flag from app state on
/// each call. Used by long-lived background tasks (e.g. the heartbeat)
/// where the user may toggle the setting while the task is running.
fn vlog_dyn(app: &AppHandle, line: impl AsRef<str>) {
    let verbose = app
        .try_state::<Mutex<AppState>>()
        .map(|s| s.lock().map(|g| g.settings.verbose_logging).unwrap_or(false))
        .unwrap_or(false);
    vlog(app, verbose, line);
}

fn log_library_inventory(
    app: &AppHandle,
    verbose: bool,
    library_path: &str,
    lib: &Library,
) {
    if !verbose { return; }
    vlog(
        app,
        true,
        format!(
            "=== library load start: path={library_path:?} tracks={} playlists={}",
            lib.tracks.len(),
            lib.playlists.len(),
        ),
    );

    let mut track_ids: Vec<&String> = lib.tracks.keys().collect();
    track_ids.sort();
    for id in track_ids {
        let t = &lib.tracks[id];
        vlog(
            app,
            true,
            format!(
                "LIB_TRACK id={} size={} name={:?} artist={:?} local={:?} device={:?} on_device={}",
                t.id,
                t.size,
                t.name,
                t.artist,
                t.local_path,
                t.device_path,
                t.on_device,
            ),
        );
    }

    let mut playlists: Vec<&musicsync_core::playlist::Playlist> = lib.playlists.iter().collect();
    playlists.sort_by(|a, b| a.name.cmp(&b.name).then(a.playlist_id.cmp(&b.playlist_id)));
    for pl in playlists {
        vlog(
            app,
            true,
            format!(
                "LIB_PLAYLIST id={} name={:?} checked={} tracks={}",
                pl.playlist_id,
                pl.name,
                pl.checked,
                pl.track_ids.len(),
            ),
        );
        for (idx, track_id) in pl.track_ids.iter().enumerate() {
            match lib.tracks.get(track_id) {
                Some(t) => vlog(
                    app,
                    true,
                    format!(
                        "  LIB_PLAYLIST_TRACK playlist_id={} index={} track_id={} name={:?} artist={:?} size={} local={:?} device={:?}",
                        pl.playlist_id,
                        idx,
                        t.id,
                        t.name,
                        t.artist,
                        t.size,
                        t.local_path,
                        t.device_path,
                    ),
                ),
                None => vlog(
                    app,
                    true,
                    format!(
                        "  LIB_PLAYLIST_TRACK playlist_id={} index={} track_id={} MISSING_FROM_LIBRARY",
                        pl.playlist_id,
                        idx,
                        track_id,
                    ),
                ),
            }
        }
    }
    vlog(app, true, "=== library load end");
}

fn log_device_manifest_playlists(
    app: &AppHandle,
    verbose: bool,
    device_playlists: &[musicsync_core::protocol::ManifestPlaylist],
) {
    if !verbose { return; }
    vlog(
        app,
        true,
        format!(
            "=== device playlist manifest start: {} playlist files",
            device_playlists.len(),
        ),
    );
    let mut playlists: Vec<&musicsync_core::protocol::ManifestPlaylist> =
        device_playlists.iter().collect();
    playlists.sort_by(|a, b| a.name.cmp(&b.name));
    for p in playlists {
        vlog(
            app,
            true,
            format!(
                "DEVICE_PLAYLIST name={:?} mtime={} bytes={} content_start",
                p.name,
                p.mtime,
                p.content.len(),
            ),
        );
        for (idx, line) in p.content.lines().enumerate() {
            vlog(
                app,
                true,
                format!("  DEVICE_PLAYLIST_LINE name={:?} line={} text={:?}", p.name, idx, line),
            );
        }
        if p.content.is_empty() {
            vlog(app, true, format!("  DEVICE_PLAYLIST_LINE name={:?} EMPTY", p.name));
        }
        vlog(app, true, format!("DEVICE_PLAYLIST name={:?} content_end", p.name));
    }
    vlog(app, true, "=== device playlist manifest end");
}

fn chrono_today_yyyy_mm_dd() -> String {
    // Lightweight YYYY-MM-DD without pulling in chrono just for this.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn chrono_now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = (secs % 86_400) as u32;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Civil date from Unix epoch days. Algorithm from Howard Hinnant's
/// "date" library; produces (year, month, day).
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[tauri::command]
fn set_verbose_logging(
    value: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    guard.settings.verbose_logging = value;
    let path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    drop(guard);
    to_save.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_delete_unused_songs(
    value: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<LibraryView, String> {
    let mut guard = state.lock().unwrap();
    guard.settings.delete_unused_songs = value;
    let path = guard.settings_path.clone();
    let settings_clone = guard.settings.clone();
    let mtime_ms = guard.library_mtime_ms;
    drop(guard);
    settings_clone.save(&path).map_err(|e| e.to_string())?;

    let guard = state.lock().unwrap();
    let lib = guard
        .library
        .as_ref()
        .ok_or_else(|| "library not loaded".to_string())?;
    let (playlists, missing) =
        build_views_with_scan(lib, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(lib, &guard.settings, s));
    Ok(LibraryView {
        track_count: lib.tracks.len(),
        orphan_count: guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()),
        preview,
        playlists,
        music_folder: lib.music_folder.clone(),
        library_path: guard.settings.library_path.clone().unwrap_or_default(),
        library_mtime_ms: mtime_ms,
        missing_playlists: missing,
    })
}

/// Toggle the per-row "clean up this playlist's tracks" flag. Only
/// meaningful for unchecked playlists; the UI hides the checkbox on
/// checked rows so toggling here is safe but pointless for those.
#[tauri::command]
fn toggle_cleanup_playlist(
    playlist_id: String,
    state: State<'_, Mutex<AppState>>,
) -> Result<LibraryView, String> {
    let mut guard = state.lock().unwrap();
    let pos = guard
        .settings
        .cleanup_playlist_ids
        .iter()
        .position(|x| *x == playlist_id);
    if let Some(i) = pos {
        guard.settings.cleanup_playlist_ids.remove(i);
    } else {
        guard.settings.cleanup_playlist_ids.push(playlist_id);
    }
    let path = guard.settings_path.clone();
    let settings_clone = guard.settings.clone();
    let mtime_ms = guard.library_mtime_ms;
    drop(guard);
    settings_clone.save(&path).map_err(|e| e.to_string())?;

    // Re-borrow to build the view.
    let guard = state.lock().unwrap();
    let lib = guard.library.as_ref().ok_or_else(|| "library not loaded".to_string())?;
    let (playlists, missing) =
        build_views_with_scan(lib, &guard.settings, guard.last_scan.as_ref());
    let preview = guard.last_scan.as_ref().map(|s| compute_preview(lib, &guard.settings, s));
    Ok(LibraryView {
        track_count: lib.tracks.len(),
        orphan_count: guard.last_scan.as_ref().map(|s| s.unused_device_paths.len()),
        preview,
        playlists,
        music_folder: lib.music_folder.clone(),
        library_path: guard.settings.library_path.clone().unwrap_or_default(),
        library_mtime_ms: mtime_ms,
        missing_playlists: missing,
    })
}

/// Start an open-ended mDNS browse on the LAN for `_musicsync._tcp.local.`
/// services. The backend emits `discovery_found` for each result; the task
/// runs for the lifetime of the app (no timeout). Manual-entry override on
/// the frontend is purely a UI swap, doesn't stop this.
#[tauri::command]
fn start_discovery(app: AppHandle) -> Result<(), String> {
    discovery::start_browse(app);
    Ok(())
}

/// Fallback TCP scan of the local /24 subnet. Used when mDNS isn't
/// finding the phone (router blocks multicast, etc.). Connects to
/// port 7800 on every candidate address with a 250ms timeout; any
/// host that accepts is emitted as `discovery_found`.
#[tauri::command]
fn start_lan_scan(app: AppHandle) -> Result<(), String> {
    discovery::start_lan_scan(app);
    Ok(())
}

/// Open and hold a persistent authenticated WebSocket connection to the
/// phone for the lifetime of the app session. Phone counts us as a real
/// connected client (its chip stays green). Tokio-tungstenite handles
/// PING/PONG keepalive frames in the background; on any error we emit
/// `device_dead` and the frontend re-enters the searching state.
///
/// Distinct from the ephemeral connections opened by scan_device /
/// run_sync — those still come and go on their own. This connection is
/// the "we're here" presence indicator.
#[tauri::command]
fn start_heartbeat(
    ws_url: String,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let token = {
        let guard = state.lock().unwrap();
        guard.settings.device_token.clone().unwrap_or_default()
    };
    if token.is_empty() {
        // No point opening a presence connection without a token —
        // the phone would reject HELLO and we'd just churn.
        tracing::info!("heartbeat skipped: no token yet (unpaired)");
        return Ok(());
    }

    {
        let mut guard = state.lock().unwrap();
        if let Some(h) = guard.heartbeat_task.take() {
            h.abort();
        }
    }

    let app_handle = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        use futures_util::{SinkExt, StreamExt};
        use musicsync_core::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
        use tokio_tungstenite::tungstenite::Message;

        let mut last_emitted_alive: Option<bool> = None;
        let mut backoff_secs: u64 = 1;

        loop {
            // Try to open the WS.
            vlog_dyn(&app_handle, format!("heartbeat: connecting to {ws_url}"));
            let ws = match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((w, _)) => w,
                Err(e) => {
                    if last_emitted_alive != Some(false) {
                        let _ = app_handle.emit("device_dead", ());
                        last_emitted_alive = Some(false);
                    }
                    vlog_dyn(
                        &app_handle,
                        format!("heartbeat: connect failed: {e}; retry in {backoff_secs}s"),
                    );
                    tracing::debug!("heartbeat connect failed: {e}; retry in {backoff_secs}s");
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(15);
                    continue;
                }
            };
            backoff_secs = 1;
            vlog_dyn(&app_handle, "heartbeat: WS connected, sending HELLO");
            let (mut sink, mut stream) = ws.split();

            // HELLO + expect HELLO_OK.
            let (u, h) = pair::desktop_identity();
            let hello = ClientMessage::Hello {
                token: token.clone(),
                protocol_version: PROTOCOL_VERSION,
                desktop_user: u,
                desktop_host: h,
            };
            let txt = match serde_json::to_string(&hello) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if sink.send(Message::Text(txt.into())).await.is_err() {
                continue;
            }
            // Read frames until we see HELLO_OK or fail.
            let mut authed = false;
            let mut token_rejected = false;
            while let Some(frame) = stream.next().await {
                match frame {
                    Ok(Message::Text(t)) => {
                        match serde_json::from_str::<ServerMessage>(&t) {
                            Ok(ServerMessage::HelloOk { device_id, device_name, .. }) => {
                                // Stamp this device into recent_devices so a
                                // future launch can fall back to its ws_url
                                // if discovery doesn't find it via mDNS/UDP.
                                record_recent_device(
                                    &app_handle,
                                    &device_id,
                                    &device_name,
                                    &ws_url,
                                );
                                authed = true;
                                break;
                            }
                            Ok(ServerMessage::Error { message }) => {
                                tracing::info!("heartbeat HELLO rejected: {message}");
                                // "bad token" is permanent — the phone
                                // doesn't recognise our token. Retrying
                                // won't help; user has to re-pair.
                                if message.to_lowercase().contains("token") ||
                                   message.to_lowercase().contains("auth") {
                                    token_rejected = true;
                                }
                                break;
                            }
                            _ => continue,
                        }
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                }
            }
            if token_rejected {
                let _ = app_handle.emit("heartbeat_token_rejected", ());
                tracing::info!("heartbeat stopping — token rejected; user must re-pair");
                return; // exit the task entirely
            }
            if !authed {
                if last_emitted_alive != Some(false) {
                    let _ = app_handle.emit("device_dead", ());
                    last_emitted_alive = Some(false);
                }
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(15);
                continue;
            }

            if last_emitted_alive != Some(true) {
                let _ = app_handle.emit("device_alive", ());
                last_emitted_alive = Some(true);
            }

            // Active ping/pong loop. We send a WS PING every 5 seconds
            // and track the most recent Pong. State machine:
            //   alive   — got a pong within the last 15 seconds
            //   yellow  — 15-30 seconds since the last pong
            //   dead    — 30+ seconds → close and reconnect
            //
            // Relaxed intervals so a heavy ongoing transfer (which can
            // saturate the WiFi link and delay control frames on the
            // separate heartbeat WS) doesn't get killed mid-flight.
            // The trade-off is slower "phone closed" detection — up to
            // ~30s before the green dot drops — but transfers don't
            // get torn down by a chatty health check.
            let mut last_pong = tokio::time::Instant::now();
            let mut ping_tick = tokio::time::interval(std::time::Duration::from_secs(5));
            ping_tick.tick().await; // immediate first tick consumed
            let mut emitted_yellow = false;
            // Reason the inner loop exited, surfaced in verbose logs so
            // we can tell apart "phone went away" (pong stale) from "WS
            // got a real Close" or "send failed" cases.
            let exit_reason: &str;
            'inner: loop {
                tokio::select! {
                    _ = ping_tick.tick() => {
                        let since_before_send = last_pong.elapsed();
                        match sink.send(Message::Ping(Vec::<u8>::new().into())).await {
                            Ok(_) => {
                                vlog_dyn(
                                    &app_handle,
                                    format!(
                                        "heartbeat: → PING (last pong {:.1}s ago)",
                                        since_before_send.as_secs_f32(),
                                    ),
                                );
                            }
                            Err(e) => {
                                vlog_dyn(
                                    &app_handle,
                                    format!("heartbeat: PING send failed: {e}"),
                                );
                                exit_reason = "ping send failed";
                                break 'inner;
                            }
                        }
                        let since = last_pong.elapsed();
                        if since > std::time::Duration::from_secs(30) {
                            vlog_dyn(
                                &app_handle,
                                format!(
                                    "heartbeat: no pong for {:.1}s — declaring dead",
                                    since.as_secs_f32(),
                                ),
                            );
                            exit_reason = "pong timeout (>30s)";
                            break 'inner;
                        } else if since > std::time::Duration::from_secs(15) {
                            if !emitted_yellow {
                                vlog_dyn(
                                    &app_handle,
                                    format!(
                                        "heartbeat: no pong for {:.1}s — yellow",
                                        since.as_secs_f32(),
                                    ),
                                );
                                let _ = app_handle.emit("device_yellow", ());
                                emitted_yellow = true;
                            }
                        }
                    }
                    frame = stream.next() => {
                        let Some(frame) = frame else {
                            vlog_dyn(&app_handle, "heartbeat: stream ended (None)");
                            exit_reason = "stream ended";
                            break 'inner;
                        };
                        match frame {
                            Ok(Message::Pong(_)) => {
                                let gap = last_pong.elapsed();
                                last_pong = tokio::time::Instant::now();
                                vlog_dyn(
                                    &app_handle,
                                    format!(
                                        "heartbeat: ← PONG (gap {:.1}s)",
                                        gap.as_secs_f32(),
                                    ),
                                );
                                if emitted_yellow {
                                    let _ = app_handle.emit("device_alive", ());
                                    emitted_yellow = false;
                                }
                            }
                            Ok(Message::Ping(payload)) => {
                                vlog_dyn(
                                    &app_handle,
                                    "heartbeat: ← PING from phone, replying PONG",
                                );
                                let _ = sink.send(Message::Pong(payload)).await;
                            }
                            Ok(Message::Text(t)) => {
                                // The heartbeat WS is also the live channel for
                                // server-pushed notifications. Currently only
                                // DEVICE_RENAMED — update the persisted display
                                // label and let the UI redraw in place.
                                if let Ok(ServerMessage::DeviceRenamed { device_id, device_name }) =
                                    serde_json::from_str::<ServerMessage>(&t)
                                {
                                    vlog_dyn(
                                        &app_handle,
                                        format!(
                                            "heartbeat: ← DEVICE_RENAMED id={device_id} name={device_name}",
                                        ),
                                    );
                                    handle_device_renamed(&app_handle, device_id, device_name);
                                }
                            }
                            Ok(Message::Binary(_)) => continue,
                            Ok(Message::Close(cf)) => {
                                vlog_dyn(
                                    &app_handle,
                                    format!("heartbeat: ← Close frame: {cf:?}"),
                                );
                                exit_reason = "close frame";
                                break 'inner;
                            }
                            Err(e) => {
                                vlog_dyn(
                                    &app_handle,
                                    format!("heartbeat: stream error: {e}"),
                                );
                                exit_reason = "stream error";
                                break 'inner;
                            }
                            _ => continue,
                        }
                    }
                }
            }

            vlog_dyn(
                &app_handle,
                format!("heartbeat: inner loop exited ({exit_reason}); reconnecting in 1s"),
            );

            if last_emitted_alive != Some(false) {
                let _ = app_handle.emit("device_dead", ());
                last_emitted_alive = Some(false);
            }
            // Connection dropped; loop to reconnect.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    let mut guard = state.lock().unwrap();
    guard.heartbeat_task = Some(handle);
    Ok(())
}

/// Fallback discovery path: take the stored `recent_devices` snapshot
/// and probe each `ws_url` in parallel, emitting `discovery_found` for
/// any that answer HELLO_OK with a matching `device_id`. Runs alongside
/// mDNS/UDP discovery on startup so we never have to wait for the
/// network's broadcast story to be reliable — the worst case is "the
/// phone moved to a new IP" in which case the probe fails and the
/// regular discovery picks it up later.
///
/// Invoked from the frontend AFTER its `listen("discovery_found", …)`
/// handler is registered — running it from Tauri's `setup()` callback
/// would race the WebView and the event would be dropped before the
/// listener exists.
#[tauri::command]
fn start_recent_probe(app: AppHandle) -> Result<(), String> {
    start_recent_devices_probe(app);
    Ok(())
}

fn start_recent_devices_probe(app: AppHandle) {
    let snapshot: Vec<musicsync_core::settings::RecentDevice> = {
        let state = app.state::<Mutex<AppState>>();
        let guard = state.lock().unwrap();
        if guard.settings.device_token.is_none() {
            return;
        }
        guard.settings.recent_devices.clone()
    };
    if snapshot.is_empty() {
        return;
    }
    for entry in snapshot {
        let app_for_probe = app.clone();
        tauri::async_runtime::spawn(async move {
            probe_recent_device(app_for_probe, entry).await;
        });
    }
}

async fn probe_recent_device(
    app: AppHandle,
    entry: musicsync_core::settings::RecentDevice,
) {
    use futures_util::{SinkExt, StreamExt};
    use musicsync_core::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
    use tokio_tungstenite::tungstenite::Message;

    let token = {
        let state = app.state::<Mutex<AppState>>();
        let guard = state.lock().unwrap();
        guard.settings.device_token.clone().unwrap_or_default()
    };
    if token.is_empty() {
        return;
    }
    // Bail out fast if the host isn't reachable — we don't want a 3s
    // TCP SYN timeout × 10 entries blocking other startup work.
    let connect = tokio_tungstenite::connect_async(&entry.ws_url);
    let ws = match tokio::time::timeout(std::time::Duration::from_secs(3), connect).await {
        Ok(Ok((w, _))) => w,
        Ok(Err(e)) => {
            tracing::debug!("recent probe {} failed: {e}", entry.ws_url);
            return;
        }
        Err(_) => {
            tracing::debug!("recent probe {} timed out", entry.ws_url);
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();
    let (u, h) = pair::desktop_identity();
    let hello = ClientMessage::Hello {
        token,
        protocol_version: PROTOCOL_VERSION,
        desktop_user: u,
        desktop_host: h,
    };
    let txt = match serde_json::to_string(&hello) {
        Ok(t) => t,
        Err(_) => return,
    };
    if sink.send(Message::Text(txt.into())).await.is_err() {
        return;
    }
    // Wait for HELLO_OK with a short overall budget. Bail on anything
    // else — Errors, Close, timeout — without surfacing the failure
    // (the regular discovery flow handles user-facing logging).
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(frame) = stream.next().await {
            match frame {
                Ok(Message::Text(t)) => {
                    if let Ok(ServerMessage::HelloOk { device_id, device_name, .. }) =
                        serde_json::from_str::<ServerMessage>(&t)
                    {
                        return Some((device_id, device_name));
                    } else {
                        return None; // unexpected reply (Error etc.)
                    }
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                _ => return None,
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    // Best-effort close; we don't need the result.
    let _ = sink.send(Message::Close(None)).await;

    let Some((device_id, device_name)) = outcome else { return };
    // Only fire if the device_id still matches the stored entry. A
    // different device on the same ws_url should NOT be auto-treated as
    // the paired phone — regular discovery / pairing handles that case.
    if !entry.device_id.is_empty() && device_id != entry.device_id {
        tracing::info!(
            "recent probe {}: device_id changed ({} -> {}), ignoring",
            entry.ws_url,
            entry.device_id,
            device_id,
        );
        return;
    }
    // Synthesise a discovery_found event so the frontend's existing
    // dispatcher handles the rest exactly as it would for an mDNS hit.
    let host = entry
        .ws_url
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
    let port = entry
        .ws_url
        .rsplit(':')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(musicsync_core::protocol::DEFAULT_PORT);
    let _ = app.emit(
        "discovery_found",
        discovery::DiscoveryFoundEvent {
            ws_url: entry.ws_url.clone(),
            device_id,
            device_name,
            host,
            port,
        },
    );
}

/// Update the `recent_devices` list with this device: move/insert at
/// the head (last-seen first) and trim to 10 entries. Persists settings.
/// `device_id` is the key — empty `device_id` (legacy companion) is a
/// no-op because there's nothing stable to dedup on.
fn record_recent_device(
    app: &AppHandle,
    device_id: &str,
    device_name: &str,
    ws_url: &str,
) {
    if device_id.is_empty() {
        return;
    }
    let state = app.state::<Mutex<AppState>>();
    let path = {
        let mut guard = state.lock().unwrap();
        guard.settings.recent_devices.retain(|d| d.device_id != device_id);
        guard.settings.recent_devices.insert(
            0,
            musicsync_core::settings::RecentDevice {
                device_id: device_id.to_string(),
                device_name: device_name.to_string(),
                ws_url: ws_url.to_string(),
                last_seen_ms: now_ms(),
            },
        );
        guard.settings.recent_devices.truncate(10);
        guard.settings_path.clone()
    };
    let snapshot = {
        let guard = state.lock().unwrap();
        guard.settings.clone()
    };
    if let Err(e) = snapshot.save(&path) {
        tracing::warn!("failed to persist recent_devices: {e}");
    }
}

/// Apply an incoming DEVICE_RENAMED notification: persist the new display
/// label against the matching paired phone (matched by `device_id`) and
/// emit `paired_device_renamed` so the UI can update its banner without
/// triggering a rescan or reconnect.
fn handle_device_renamed(app: &AppHandle, device_id: String, device_name: String) {
    if device_id.is_empty() || device_name.is_empty() {
        return;
    }
    let state = app.state::<Mutex<AppState>>();
    let path = {
        let mut guard = state.lock().unwrap();
        // Only accept the rename if it matches our currently-paired device.
        // (Backwards-compat: if we don't have a stored device_id yet, adopt
        // the one we just learned about so subsequent renames are matched.)
        match guard.settings.paired_device_id.clone() {
            Some(existing) if existing == device_id => {}
            Some(_) => return, // mismatched — ignore
            None => {
                guard.settings.paired_device_id = Some(device_id.clone());
            }
        }
        if guard.settings.paired_device_name.as_deref() == Some(device_name.as_str()) {
            return; // no change; don't bother re-saving
        }
        guard.settings.paired_device_name = Some(device_name.clone());
        guard.settings_path.clone()
    };
    let snapshot = {
        let guard = state.lock().unwrap();
        guard.settings.clone()
    };
    if let Err(e) = snapshot.save(&path) {
        tracing::warn!("failed to persist renamed device label: {e}");
    }
    let _ = app.emit(
        "paired_device_renamed",
        PairedDeviceRenamedEvent { device_id, device_name },
    );
}

/// If a `legacy_settings.yml` is sitting in the working directory on
/// startup, merge its `checked_playlist_ids` into `settings` and rename
/// it to `legacy_settings.yml.imported` so subsequent launches don't
/// reprocess it. Other fields in the legacy file (pairing token, library
/// path, etc.) are deliberately ignored — only the playlist selections
/// carry over. Best-effort: any I/O or parse error is logged but never
/// fatal.
fn maybe_import_legacy_settings_yml(settings: &mut Settings) {
    let path = std::path::PathBuf::from("legacy_settings.yml");
    if !path.exists() {
        return;
    }
    let legacy = match Settings::load(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("legacy_settings.yml present but failed to parse: {e}");
            return;
        }
    };
    let existing: std::collections::HashSet<String> =
        settings.checked_playlist_ids.iter().cloned().collect();
    let mut imported = 0usize;
    for id in legacy.checked_playlist_ids {
        if !existing.contains(&id) {
            settings.checked_playlist_ids.push(id);
            imported += 1;
        }
    }
    let dest = std::path::PathBuf::from("legacy_settings.yml.imported");
    let rename_result = std::fs::rename(&path, &dest);
    tracing::info!(
        "legacy_settings.yml: imported {imported} new playlist id(s); \
         rename to {} -> {:?}",
        dest.display(),
        rename_result,
    );
}

/// Open the process working directory (where the dated `musicsync-*.log`
/// files are written) in the OS file browser. Returned string is the
/// absolute path so the frontend can surface it in a tooltip / error.
#[tauri::command]
fn reveal_working_dir() -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let path_str = cwd.to_string_lossy().to_string();
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![path_str.as_str()]);
    #[cfg(target_os = "windows")]
    let cmd = ("explorer", vec![path_str.as_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = ("xdg-open", vec![path_str.as_str()]);
    std::process::Command::new(cmd.0)
        .args(&cmd.1)
        .spawn()
        .map_err(|e| format!("failed to open file browser: {e}"))?;
    Ok(path_str)
}

/// Write the given text to the chosen path. Used by the About-tab "export
/// log" link to dump the in-memory Log tab contents to disk.
#[tauri::command]
fn write_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_heartbeat(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    if let Some(h) = guard.heartbeat_task.take() {
        h.abort();
    }
    Ok(())
}

/// Set the abort flag so the running sync task exits at its next file
/// boundary. Does NOT close the WebSocket directly — graceful abort.
#[tauri::command]
fn abort_sync(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let guard = state.lock().unwrap();
    guard.abort_sync.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn forget_pairing(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    guard.settings.device_token = None;
    guard.settings.paired_device_name = None;
    guard.settings.paired_device_id = None;
    let path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    drop(guard);
    to_save.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_ignored_device(
    device_name: String,
    device_id: Option<String>,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    if !guard.settings.ignored_devices.iter().any(|n| n == &device_name) {
        guard.settings.ignored_devices.push(device_name);
    }
    if let Some(id) = device_id.filter(|s| !s.is_empty()) {
        if !guard.settings.ignored_device_ids.iter().any(|v| v == &id) {
            guard.settings.ignored_device_ids.push(id);
        }
    }
    let path = guard.settings_path.clone();
    let to_save = guard.settings.clone();
    drop(guard);
    to_save.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn pair_cancel(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    if let Some(tx) = guard.pair_confirm_tx.take() {
        let _ = tx.send(false);
    }
    Ok(())
}

/// Current wall-clock time as Unix epoch milliseconds. The frontend uses
/// `new Date(ms).toLocaleTimeString()` so the user always sees local time
/// regardless of the host's TZ configuration.
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn library_view(
    lib: &Library,
    library_path: String,
    library_mtime_ms: Option<u64>,
    playlists: Vec<PlaylistView>,
    missing_playlists: Vec<MissingPlaylistView>,
    orphan_count: Option<usize>,
    preview: Option<SyncPreview>,
) -> LibraryView {
    LibraryView {
        track_count: lib.tracks.len(),
        orphan_count,
        preview,
        playlists,
        music_folder: lib.music_folder.clone(),
        library_path,
        library_mtime_ms,
        missing_playlists,
    }
}

/// Compute what `run_sync` would actually do right now. Uses the same
/// dedupe logic (`tracks_to_upload`) so the count matches the real sync.
/// Also collects names/paths for the Details dialog.
fn compute_preview(
    lib: &Library,
    settings: &Settings,
    last_scan: &LastScan,
) -> SyncPreview {
    use musicsync_core::matching::tracks_to_upload;
    let checked_refs: Vec<&musicsync_core::playlist::Playlist> =
        lib.playlists.iter().filter(|p| p.checked).collect();
    let to_upload_ids = tracks_to_upload(&checked_refs, &lib.tracks);
    let mut new_song_items: Vec<PreviewSong> = Vec::with_capacity(to_upload_ids.len());
    let mut missing_files: Vec<PreviewSong> = Vec::new();
    for id in &to_upload_ids {
        let Some(t) = lib.tracks.get(id) else { continue; };
        let entry = PreviewSong {
            name: t.name.clone(),
            artist: t.artist.clone(),
            path: t.local_path.clone(),
            device_path: t.device_path.clone(),
        };
        let exists = !t.local_path.is_empty()
            && std::fs::metadata(&t.local_path).is_ok();
        if exists {
            new_song_items.push(entry);
        } else {
            // Log the exact reason so the user can compare against what
            // actually exists on disk.
            let why = if t.local_path.is_empty() {
                "empty local_path (Library.xml had no Location)".to_string()
            } else {
                match std::fs::metadata(&t.local_path) {
                    Ok(_) => "stat ok but flagged missing??".into(),
                    Err(e) => format!("{}", e),
                }
            };
            tracing::info!("missing-file: {:?} | reason: {}", t.local_path, why);
            missing_files.push(entry);
        }
    }

    let new_playlist_items: Vec<PreviewPlaylist> = lib
        .playlists
        .iter()
        .filter(|p| p.checked)
        .filter(|p| !last_scan.device_playlist_line_counts.contains_key(&p.name))
        .map(|p| PreviewPlaylist {
            name: p.name.clone(),
            filename: format!("{}.m3u", p.name),
        })
        .collect();

    let library_ids: std::collections::HashSet<&str> =
        lib.playlists.iter().map(|p| p.playlist_id.as_str()).collect();
    let remove_playlist_items: Vec<PreviewPlaylist> = settings
        .remembered_playlists
        .iter()
        .filter(|r| !library_ids.contains(r.id.as_str()))
        .filter(|r| matches!(r.action, PlaylistAction::Delete))
        .map(|r| PreviewPlaylist {
            name: r.name.clone(),
            filename: format!("{}.m3u", r.name),
        })
        .collect();

    let cleanup_ids: std::collections::HashSet<&str> = settings
        .cleanup_playlist_ids
        .iter()
        .map(|s| s.as_str())
        .collect();
    let checked_track_ids: std::collections::HashSet<&str> = lib
        .playlists
        .iter()
        .filter(|p| p.checked)
        .flat_map(|p| p.track_ids.iter().map(|s| s.as_str()))
        .collect();
    let mut device_path_by_size: std::collections::HashMap<u64, String> =
        std::collections::HashMap::new();
    for (p, sz) in &last_scan.device_files {
        if p.to_ascii_lowercase().ends_with(".m3u") { continue; }
        device_path_by_size.entry(*sz).or_insert_with(|| p.clone());
    }
    let mut to_delete: std::collections::HashSet<String> = std::collections::HashSet::new();
    if settings.delete_unused_songs {
        for p in &last_scan.unused_device_paths {
            to_delete.insert(p.clone());
        }
    }
    for p in &lib.playlists {
        if !cleanup_ids.contains(p.playlist_id.as_str()) { continue; }
        for tid in &p.track_ids {
            if checked_track_ids.contains(tid.as_str()) { continue; }
            if let Some(t) = lib.tracks.get(tid) {
                if let Some(path) = device_path_by_size.get(&t.size) {
                    to_delete.insert(path.clone());
                }
            }
        }
    }
    let mut paths: Vec<String> = to_delete.into_iter().collect();
    paths.sort();

    // For each path being deleted, look up its iTunes track (by matching
    // size against device_files) so we can display name + artist if we
    // know them. Orphan files (no iTunes match) get blank name/artist
    // and just the path.
    let track_by_size: std::collections::HashMap<u64, &musicsync_core::track::Track> = lib
        .tracks
        .values()
        .map(|t| (t.size, t))
        .collect();
    let path_size: std::collections::HashMap<&str, u64> = last_scan
        .device_files
        .iter()
        .map(|(p, s)| (p.as_str(), *s))
        .collect();
    let delete_song_items: Vec<PreviewSong> = paths
        .iter()
        .map(|p| {
            let sz = path_size.get(p.as_str()).copied();
            let t = sz.and_then(|s| track_by_size.get(&s));
            PreviewSong {
                name: t.map(|t| t.name.clone()).unwrap_or_default(),
                artist: t.map(|t| t.artist.clone()).unwrap_or_default(),
                path: p.clone(),
                device_path: String::new(),
            }
        })
        .collect();

    SyncPreview {
        new_playlists: new_playlist_items.len(),
        new_songs: new_song_items.len(),
        remove_playlists: remove_playlist_items.len(),
        delete_songs: delete_song_items.len(),
        new_playlist_items,
        new_song_items,
        remove_playlist_items,
        delete_song_items,
        missing_files,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn main() {
    tracing_subscriber::fmt::init();

    let settings_path = Settings::default_config_path()
        .expect("could not locate config dir");
    let legacy_paths = vec![
        PathBuf::from("../settings.yml"),
        PathBuf::from("./settings.yml"),
    ];
    let mut settings = Settings::load_with_migration(&settings_path, &legacy_paths)
        .unwrap_or_default();
    // One-shot migration: if the user dropped a `legacy_settings.yml`
    // in CWD (e.g. copied from an old Ruby install), merge its
    // checked_playlist_ids and rename the file so we don't redo it.
    maybe_import_legacy_settings_yml(&mut settings);
    // Persist if the merge added anything (or did nothing — cheap).
    let _ = settings.save(&settings_path);

    let state = AppState {
        settings_path,
        settings,
        library: None,
        library_mtime_ms: None,
        pair_confirm_tx: None,
        last_scan: None,
        abort_sync: Arc::new(AtomicBool::new(false)),
        heartbeat_task: None,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(state))
        .setup(|app| {
            // Background poll: watch the configured library_path for mtime
            // changes every 10 seconds and emit `library_changed`. The
            // frontend reloads on that event. No-op if no library_path set.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut last: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    let path_str = {
                        let state = handle.state::<Mutex<AppState>>();
                        let guard = state.lock().unwrap();
                        guard.settings.library_path.clone()
                    };
                    let Some(path_str) = path_str else { continue };
                    let path = std::path::PathBuf::from(&path_str);
                    let Ok(meta) = std::fs::metadata(&path) else { continue };
                    let Ok(mtime) = meta.modified() else { continue };
                    let changed = match &last {
                        Some((p, m)) => p != &path || *m != mtime,
                        None => false,
                    };
                    if changed {
                        let _ = handle.emit("library_changed", ());
                    }
                    last = Some((path, mtime));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            load_library,
            toggle_playlist,
            run_sync,
            start_pairing,
            pair_confirm,
            pair_cancel,
            start_discovery,
            forget_playlist,
            set_playlist_action,
            scan_device,
            set_delete_unused_songs,
            set_verbose_logging,
            toggle_cleanup_playlist,
            forget_pairing,
            add_ignored_device,
            abort_sync,
            start_lan_scan,
            start_heartbeat,
            stop_heartbeat,
            reveal_working_dir,
            write_text_file,
            start_recent_probe,
        ])
        .run(tauri::generate_context!())
        .expect("error running MusicSync");
}
