//! WebSocket-based sync client. Connects to the Android companion app,
//! fetches its manifest, diffs against the loaded library, and streams the
//! missing files + changed playlists across.
//!
//! Layout follows the protocol module: text frames for control messages,
//! binary frames for file payloads. Each FILE_PUT control message is
//! immediately followed by exactly one binary frame whose length matches
//! the announced `size`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use musicsync_core::matching::{mark_on_device_strict, tracks_to_upload, DeviceFile};
use musicsync_core::playlist::{m3u_semantically_equal, Playlist};
use musicsync_core::protocol::{
    ClientMessage, ManifestFile, ManifestPlaylist, ServerMessage, PROTOCOL_VERSION,
};
use musicsync_core::track::Track;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::tungstenite::Message;

pub struct SyncReport {
    pub uploaded_tracks: usize,
    pub uploaded_playlists: usize,
    pub already_present: usize,
    pub deleted_files: usize,
    pub errors: Vec<String>,
}

/// Standalone scan: connects, sends HELLO + MANIFEST_REQUEST, returns
/// every file (path, size), every playlist (full manifest entries), and
/// the device music root. Used by the scan_device Tauri command.
///
/// `on_progress` is called for every PROGRESS message the phone pushes
/// while it walks its music folder, so the desktop can mirror the phone's
/// progress bar in real time. Arguments are `(message, fraction)` where
/// fraction is in `[0, 1]` when known.
pub async fn fetch_manifest_full(
    ws_url: &str,
    token: &str,
    on_progress: impl Fn(&str, Option<f32>) + Send + Sync,
) -> Result<(Vec<(String, u64)>, Vec<ManifestPlaylist>, String)> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = ws_stream.split();
    send_msg(
        &mut sink,
        &{
            let (u, h) = crate::pair::desktop_identity();
            ClientMessage::Hello {
                token: token.to_string(),
                protocol_version: PROTOCOL_VERSION,
                desktop_user: u,
                desktop_host: h,
            }
        },
    )
    .await?;
    let hello_ok = recv_msg(&mut stream).await?;
    let music_root = match hello_ok {
        ServerMessage::HelloOk { music_root, .. } => music_root,
        other => return Err(anyhow!("unexpected response to HELLO: {other:?}")),
    };
    send_msg(&mut sink, &ClientMessage::ManifestRequest).await?;
    // The phone may push PROGRESS frames before the final MANIFEST while
    // it walks its music folder. Forward those through the callback and
    // keep reading until the actual manifest arrives.
    let (files, playlists) = loop {
        match recv_msg(&mut stream).await? {
            ServerMessage::Progress { message, fraction } => {
                on_progress(&message, fraction);
                continue;
            }
            ServerMessage::Manifest { files, playlists } => {
                let files = files.into_iter().map(|f| (f.path, f.size)).collect();
                break (files, playlists);
            }
            other => return Err(anyhow!("unexpected response to MANIFEST_REQUEST: {other:?}")),
        }
    };
    let _ = send_msg(&mut sink, &ClientMessage::Bye).await;
    Ok((files, playlists, music_root))
}

/// Run a full sync against the phone at `ws_url` (e.g. `ws://192.168.0.10:7800`).
///
/// `progress` reports incremental status (per-file uploads etc.).
/// `on_scan_started` / `on_scan_complete` are fired around the MANIFEST
/// round-trip; the UI uses them to render a dedicated "exploration / scan"
/// banner separate from the running status line.
pub async fn run_sync(
    ws_url: &str,
    token: &str,
    tracks: &mut HashMap<String, Track>,
    playlists: &[Playlist],
    // Playlist filenames (e.g. ["Old.m3u"]) to delete from the phone's
    // music root before the regular upload flow. Empty for the common case.
    playlists_to_delete: &[String],
    // Device-relative paths of music files to delete (e.g. unused-songs
    // cleanup). Empty unless the user opted in.
    tracks_to_delete: &[String],
    // Set to true by the abort_sync Tauri command. We check between each
    // file boundary and bail out early when it flips. The half-uploaded
    // file is a .tmp.X on the phone (atomic-write contract) so nothing
    // partial appears in the next manifest.
    abort_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: impl Fn(&str, Option<f32>) + Send + Sync,
    on_scan_started: impl Fn() + Send + Sync,
    on_scan_complete: impl Fn(usize, usize) + Send + Sync,
) -> Result<SyncReport> {
    progress("Connecting...", None);
    progress("Authenticating...", None);
    let (mut sink, mut stream, music_root) = connect_and_authenticate(ws_url, token).await?;

    // MANIFEST
    progress("Scanning...", None);
    on_scan_started();
    send_msg(&mut sink, &ClientMessage::ManifestRequest).await?;
    // Forward the phone's PROGRESS frames through the same progress
    // channel so the UI's status line + progress bar update during the
    // scan, then break out when the final MANIFEST arrives.
    let (files, device_playlists) = loop {
        match recv_msg(&mut stream).await? {
            ServerMessage::Progress { message, fraction } => {
                progress(&message, fraction);
                continue;
            }
            ServerMessage::Manifest { files, playlists } => break (files, playlists),
            other => return Err(anyhow!("unexpected response to MANIFEST_REQUEST: {other:?}")),
        }
    };
    on_scan_complete(files.len(), device_playlists.len());
    progress(
        &format!(
            "Scan complete, ready to copy. ({} tracks, {} playlists)",
            files.len(),
            device_playlists.len(),
        ),
        None,
    );

    // Match
    let device_files: Vec<DeviceFile> = files
        .iter()
        .map(|f| DeviceFile { path: f.path.clone(), size: f.size })
        .collect();
    mark_on_device_strict(tracks, &device_files);

    let checked: Vec<&Playlist> = playlists.iter().filter(|p| p.checked).collect();
    let to_upload = tracks_to_upload(&checked, tracks);
    let already_present = tracks.values().filter(|t| t.on_device).count();
    let total_deletes = playlists_to_delete.len() + tracks_to_delete.len();
    let device_by_name: HashMap<&str, &ManifestPlaylist> =
        device_playlists.iter().map(|p| (p.name.as_str(), p)).collect();
    let playlists_to_upload: Vec<(&Playlist, String)> = checked
        .iter()
        .filter_map(|pl| {
            let content = pl.generate_m3u(&music_root, |id| tracks.get(id));
            let want_upload = match device_by_name.get(pl.device_filename().as_str()) {
                Some(existing) => !m3u_semantically_equal(&existing.content, &content),
                None => true,
            };
            if want_upload {
                Some((*pl, content))
            } else {
                None
            }
        })
        .collect();

    let total_ops = total_deletes + to_upload.len() + playlists_to_upload.len();
    let mut completed_ops = 0usize;

    let op_fraction = |completed: usize| -> Option<f32> {
        if total_ops == 0 {
            None
        } else {
            Some((completed as f32 / total_ops as f32).clamp(0.0, 1.0))
        }
    };

    // Delete playlists + any unused-track files. Server returns OK even
    // when the path is absent, so we don't gate on the manifest content.
    let mut deleted_files: usize = 0;
    let mut errors = Vec::new();
    for name in playlists_to_delete {
        let step = completed_ops + 1;
        let pct = ((step as f32 / total_ops.max(1) as f32) * 100.0).round() as u32;
        let msg = format!(
            "[{}/{}, {}%] Deleting playlist {:?}",
            step,
            total_ops.max(1),
            pct,
            name,
        );
        let frac = op_fraction(step);
        progress(&msg, frac);
        let _ = emit_client_progress(&mut sink, &msg, frac).await;
        let _ = send_msg(&mut sink, &ClientMessage::FileDelete { path: name.clone() }).await;
        if let Ok(ServerMessage::FileDeleteOk { .. }) = recv_msg(&mut stream).await {
            deleted_files += 1;
        }
        completed_ops += 1;
    }
    for path in tracks_to_delete {
        let step = completed_ops + 1;
        let pct = ((step as f32 / total_ops.max(1) as f32) * 100.0).round() as u32;
        let msg = format!(
            "[{}/{}, {}%] Deleting unused {:?}",
            step,
            total_ops.max(1),
            pct,
            path,
        );
        let frac = op_fraction(step);
        progress(&msg, frac);
        let _ = emit_client_progress(&mut sink, &msg, frac).await;
        let _ = send_msg(&mut sink, &ClientMessage::FileDelete { path: path.clone() }).await;
        if let Ok(ServerMessage::FileDeleteOk { .. }) = recv_msg(&mut stream).await {
            deleted_files += 1;
        }
        completed_ops += 1;
    }

    // Upload missing tracks. Each file is wrapped in a reconnect-retry
    // loop so a flaky link doesn't tear down a long sync — when an
    // attempt fails (timeout, broken pipe, stream closed), we sleep a
    // moment, reopen the WS + re-HELLO, and re-send THIS track. The
    // phone writes atomically (.tmp.X → rename) so a half-uploaded file
    // doesn't appear in the next manifest; retrying just resends.
    progress("Copying...", None);
    let mut uploaded_tracks = 0usize;
    for track_id in &to_upload {
        if abort_flag.load(std::sync::atomic::Ordering::SeqCst) {
            progress("Aborted by user.", None);
            errors.push("aborted by user".into());
            break;
        }
        let Some(track) = tracks.get(track_id) else { continue; };
        let dest = track.playlist_path(&music_root);
        let step = completed_ops + 1;
        let pct = ((step as f32 / total_ops.max(1) as f32) * 100.0).round() as u32;
        let msg = format!(
            "[{}/{}, {}%] Copying {:?} -> {:?}",
            step, total_ops.max(1), pct, track.local_path, dest,
        );
        let frac = op_fraction(step);
        progress(&msg, frac);
        let _ = emit_client_progress(&mut sink, &msg, frac).await;
        let mut backoff_secs: u64 = 1;
        let mut last_err: Option<String> = None;
        for attempt in 1..=MAX_OP_ATTEMPTS {
            if abort_flag.load(std::sync::atomic::Ordering::SeqCst) { break; }
            match upload_one_file(&mut sink, &mut stream, track, &music_root).await {
                Ok(()) => {
                    uploaded_tracks += 1;
                    last_err = None;
                    // One log-tab line per successful file so the user has a
                    // complete record of what crossed the wire, not just
                    // the in-flight "Copying …" pre-message.
                    progress(
                        &format!("Uploaded {} ({} bytes) -> {:?}", track.name, track.size, dest),
                        frac,
                    );
                    break;
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                    if attempt == MAX_OP_ATTEMPTS { break; }
                    let retry_msg = format!(
                        "{}: attempt {}/{} failed: {e}; reconnecting in {}s",
                        track.name, attempt, MAX_OP_ATTEMPTS, backoff_secs,
                    );
                    progress(&retry_msg, frac);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(15);
                    match connect_and_authenticate(ws_url, token).await {
                        Ok((s, st, _root)) => { sink = s; stream = st; }
                        Err(re) => {
                            // Reconnect itself failed; loop will try again
                            // next iteration with the still-dead handles
                            // (which will surface the error and burn an
                            // attempt). That's fine — the backoff caps
                            // request rate either way.
                            tracing::warn!("reconnect failed mid-sync: {re}");
                        }
                    }
                }
            }
        }
        if let Some(err) = last_err {
            errors.push(format!("{}: {err} (after {} attempts)", track.name, MAX_OP_ATTEMPTS));
        }
        completed_ops += 1;
    }

    // Upload changed playlists — same reconnect-retry shape as tracks.
    let mut uploaded_playlists = 0usize;
    for (pl, content) in &playlists_to_upload {
        let step = completed_ops + 1;
        let pct = ((step as f32 / total_ops.max(1) as f32) * 100.0).round() as u32;
        let msg = format!(
            "[{}/{}, {}%] Writing playlist {}",
            step,
            total_ops.max(1),
            pct,
            pl.name,
        );
        let frac = op_fraction(step);
        progress(&msg, frac);
        let _ = emit_client_progress(&mut sink, &msg, frac).await;
        let mut backoff_secs: u64 = 1;
        let mut last_err: Option<String> = None;
        for attempt in 1..=MAX_OP_ATTEMPTS {
            if abort_flag.load(std::sync::atomic::Ordering::SeqCst) { break; }
            match upload_playlist(&mut sink, &mut stream, pl, content).await {
                Ok(()) => {
                    uploaded_playlists += 1;
                    last_err = None;
                    // One log-tab line per successful playlist write.
                    let line_count = content
                        .lines()
                        .filter(|l| !l.is_empty() && *l != "#EXTM3U")
                        .count();
                    progress(
                        &format!("Wrote playlist {} ({} tracks)", pl.name, line_count),
                        frac,
                    );
                    break;
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                    if attempt == MAX_OP_ATTEMPTS { break; }
                    let retry_msg = format!(
                        "playlist {}: attempt {}/{} failed: {e}; reconnecting in {}s",
                        pl.name, attempt, MAX_OP_ATTEMPTS, backoff_secs,
                    );
                    progress(&retry_msg, frac);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(15);
                    if let Ok((s, st, _root)) = connect_and_authenticate(ws_url, token).await {
                        sink = s;
                        stream = st;
                    }
                }
            }
        }
        if let Some(err) = last_err {
            errors.push(format!("playlist {}: {err} (after {} attempts)", pl.name, MAX_OP_ATTEMPTS));
        }
        completed_ops += 1;
    }

    // BYE
    let _ = emit_client_progress(&mut sink, "Sync complete.", Some(1.0)).await;
    let _ = send_msg(&mut sink, &ClientMessage::Bye).await;
    progress("Sync complete.", Some(1.0));

    Ok(SyncReport {
        uploaded_tracks,
        uploaded_playlists,
        already_present,
        deleted_files,
        errors,
    })
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Open a fresh WS, send HELLO, expect HELLO_OK. Returns sink/stream
/// and the device music root. Used both for the initial connection at
/// the top of [run_sync] and for the resume-on-disconnect path inside
/// the per-file upload loops.
pub(crate) async fn connect_and_authenticate(
    ws_url: &str,
    token: &str,
) -> Result<(WsSink, WsStream, String)> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = ws_stream.split();
    let (u, h) = crate::pair::desktop_identity();
    send_msg(
        &mut sink,
        &ClientMessage::Hello {
            token: token.to_string(),
            protocol_version: PROTOCOL_VERSION,
            desktop_user: u,
            desktop_host: h,
        },
    )
    .await?;
    let music_root = match recv_msg(&mut stream).await? {
        ServerMessage::HelloOk { music_root, protocol_version, .. } => {
            if protocol_version != PROTOCOL_VERSION {
                return Err(anyhow!(
                    "protocol version mismatch: server={} client={}",
                    protocol_version,
                    PROTOCOL_VERSION
                ));
            }
            music_root
        }
        ServerMessage::Error { message } => return Err(anyhow!("phone error: {message}")),
        other => return Err(anyhow!("unexpected response to HELLO: {other:?}")),
    };
    Ok((sink, stream, music_root))
}

/// Per-operation retry budget. Each track / playlist gets this many
/// attempts before we surface the error to the caller. Reconnect
/// happens between attempts so a torn-down WS gets rebuilt automatically.
const MAX_OP_ATTEMPTS: usize = 8;

async fn send_msg(sink: &mut WsSink, msg: &ClientMessage) -> Result<()> {
    let text = serde_json::to_string(msg)?;
    sink.send(Message::Text(text.into())).await?;
    Ok(())
}

async fn emit_client_progress(
    sink: &mut WsSink,
    message: &str,
    fraction: Option<f32>,
) -> Result<()> {
    send_msg(
        sink,
        &ClientMessage::Progress {
            message: message.to_string(),
            fraction,
        },
    )
    .await
}

async fn recv_msg(stream: &mut WsStream) -> Result<ServerMessage> {
    loop {
        let frame = stream
            .next()
            .await
            .ok_or_else(|| anyhow!("connection closed"))??;
        match frame {
            Message::Text(t) => return Ok(serde_json::from_str(&t)?),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Binary(_) => return Err(anyhow!("unexpected binary frame before control")),
            Message::Close(_) => return Err(anyhow!("connection closed by server")),
            Message::Frame(_) => continue,
        }
    }
}

async fn upload_one_file(
    sink: &mut WsSink,
    stream: &mut WsStream,
    track: &Track,
    music_root: &str,
) -> Result<()> {
    let local = Path::new(&track.local_path);
    let mut file = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("opening local source {}", local.display()))?;
    let mut bytes = Vec::with_capacity(track.size as usize);
    file.read_to_end(&mut bytes).await?;
    let relative = track.playlist_path(music_root);
    send_msg(sink, &ClientMessage::FilePut { path: relative.clone(), size: bytes.len() as u64 })
        .await?;
    sink.send(Message::Binary(bytes.into())).await?;
    match recv_msg(stream).await? {
        ServerMessage::FileOk { .. } => Ok(()),
        ServerMessage::FileErr { message, .. } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response to FILE_PUT: {other:?}")),
    }
}

async fn upload_playlist(
    sink: &mut WsSink,
    stream: &mut WsStream,
    playlist: &Playlist,
    content: &str,
) -> Result<()> {
    send_msg(
        sink,
        &ClientMessage::PlaylistPut {
            name: playlist.device_filename(),
            content: content.to_string(),
        },
    )
    .await?;
    match recv_msg(stream).await? {
        ServerMessage::PlaylistOk { .. } => Ok(()),
        ServerMessage::PlaylistErr { message, .. } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response to PLAYLIST_PUT: {other:?}")),
    }
}

#[allow(dead_code)]
fn _force_use(_: &ManifestFile) {}
