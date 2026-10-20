//! Delete planning: which files on the phone a sync should remove.
//!
//! Device files are matched to library tracks by size (see `matching`), so
//! deciding what is safe to delete has to be done per device *file*, not per
//! track: two different tracks can share a size, and the phone can hold
//! several copies of one track. The rules:
//!
//! - A file whose size matches no library track is an **orphan**.
//! - Otherwise its candidate tracks are the same-sized library tracks,
//!   narrowed to those with the same filename when any have it (the same
//!   tiebreak `mark_on_device_strict` uses).
//! - If any candidate is in a checked playlist the file is **wanted** and is
//!   never deleted. Only when every candidate is unwanted is it **unwanted**.
//!
//! Both the sync preview and the sync itself go through [`plan_deletes`], so
//! what the user confirms is what gets deleted.

use std::collections::{HashMap, HashSet};

use crate::matching::DeviceFile;
use crate::playlist::Playlist;
use crate::track::Track;

/// What a file on the phone is, relative to the library and the playlists
/// currently being synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass {
    /// Size matches no track in the library.
    Orphan,
    /// Could be a track in a checked playlist. Never deleted.
    Wanted,
    /// Matches only tracks that are in no checked playlist (their IDs).
    Unwanted(Vec<String>),
}

pub fn is_playlist_file(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".m3u")
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Every music file on the phone, classified. Playlist (.m3u) files in the
/// manifest are skipped.
pub struct ClassifiedFiles {
    files: Vec<(String, FileClass)>,
}

impl ClassifiedFiles {
    pub fn new(
        tracks: &HashMap<String, Track>,
        playlists: &[Playlist],
        device_files: &[DeviceFile],
    ) -> Self {
        let wanted: HashSet<&str> = playlists
            .iter()
            .filter(|p| p.checked)
            .flat_map(|p| p.track_ids.iter().map(|s| s.as_str()))
            .collect();
        let mut by_size: HashMap<u64, Vec<&Track>> = HashMap::new();
        for t in tracks.values() {
            by_size.entry(t.size).or_default().push(t);
        }

        let mut files = Vec::with_capacity(device_files.len());
        for f in device_files {
            if is_playlist_file(&f.path) {
                continue;
            }
            let class = match by_size.get(&f.size) {
                None => FileClass::Orphan,
                Some(same_size) => {
                    let named: Vec<&Track> = same_size
                        .iter()
                        .copied()
                        .filter(|t| basename(&t.device_path) == f.basename())
                        .collect();
                    let candidates = if named.is_empty() { same_size } else { &named };
                    if candidates.iter().any(|t| wanted.contains(t.id.as_str())) {
                        FileClass::Wanted
                    } else {
                        let mut ids: Vec<String> =
                            candidates.iter().map(|t| t.id.clone()).collect();
                        ids.sort();
                        FileClass::Unwanted(ids)
                    }
                }
            };
            files.push((f.path.clone(), class));
        }
        ClassifiedFiles { files }
    }

    fn paths_where<F: Fn(&FileClass) -> bool>(&self, pred: F) -> Vec<String> {
        let mut out: Vec<String> = self
            .files
            .iter()
            .filter(|(_, c)| pred(c))
            .map(|(p, _)| p.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Files not in the library at all.
    pub fn orphan_paths(&self) -> Vec<String> {
        self.paths_where(|c| matches!(c, FileClass::Orphan))
    }

    /// Library tracks on the phone that no checked playlist contains.
    pub fn unwanted_paths(&self) -> Vec<String> {
        self.paths_where(|c| matches!(c, FileClass::Unwanted(_)))
    }

    /// The unwanted files that belong to `playlist`: what ticking that
    /// playlist's Delete box removes.
    pub fn playlist_unwanted_paths(&self, playlist: &Playlist) -> Vec<String> {
        let ids: HashSet<&str> = playlist.track_ids.iter().map(|s| s.as_str()).collect();
        self.paths_where(|c| match c {
            FileClass::Unwanted(track_ids) => track_ids.iter().any(|id| ids.contains(id.as_str())),
            _ => false,
        })
    }
}

/// A previously-synced playlist that is no longer in the library.
#[derive(Debug, Clone)]
pub struct MissingPlaylist<'a> {
    pub name: &'a str,
    /// The user picked "Delete from phone on next sync" for it.
    pub delete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteOptions<'a> {
    /// Delete files that aren't in the library at all.
    pub orphans: bool,
    /// Persistent IDs of unchecked playlists whose Delete box is ticked.
    pub cleanup_playlist_ids: HashSet<&'a str>,
    pub missing_playlists: Vec<MissingPlaylist<'a>>,
    /// Delete everything that isn't part of a checked playlist: orphans,
    /// every unwanted library track, and the playlist file of every
    /// unchecked or missing playlist. Playlist files the app has never
    /// synced (e.g. made on the phone) are left alone.
    pub everything_else: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletePlan {
    /// Device paths of music files to delete, sorted.
    pub track_paths: Vec<String>,
    /// Device paths of playlist files to delete, sorted.
    pub playlist_files: Vec<String>,
}

impl DeletePlan {
    pub fn is_empty(&self) -> bool {
        self.track_paths.is_empty() && self.playlist_files.is_empty()
    }
}

/// The device path of `name`'s playlist file, if it's on the phone and no
/// checked playlist of the same name is going to (re)write it.
pub fn deletable_playlist_file<'a>(
    name: &str,
    playlists: &[Playlist],
    device_playlists: &'a HashMap<String, String>,
) -> Option<&'a String> {
    if playlists.iter().any(|p| p.checked && p.name == name) {
        return None;
    }
    device_playlists.get(name)
}

/// `device_playlists` maps a playlist name (file basename without `.m3u`)
/// to that file's path on the phone. Only playlist files actually on the
/// phone end up in the plan.
pub fn plan_deletes(
    classified: &ClassifiedFiles,
    playlists: &[Playlist],
    device_playlists: &HashMap<String, String>,
    options: &DeleteOptions,
) -> DeletePlan {
    let mut tracks: Vec<String> = Vec::new();
    let mut names: Vec<&str> = Vec::new();

    if options.orphans || options.everything_else {
        tracks.extend(classified.orphan_paths());
    }
    if options.everything_else {
        tracks.extend(classified.unwanted_paths());
    }
    for p in playlists.iter().filter(|p| !p.checked) {
        if options.everything_else {
            names.push(&p.name);
        } else if options.cleanup_playlist_ids.contains(p.persistent_id.as_str()) {
            tracks.extend(classified.playlist_unwanted_paths(p));
            names.push(&p.name);
        }
    }
    for m in &options.missing_playlists {
        if m.delete || options.everything_else {
            names.push(m.name);
        }
    }

    tracks.sort();
    tracks.dedup();
    let mut playlist_files: Vec<String> = names
        .into_iter()
        .filter_map(|n| deletable_playlist_file(n, playlists, device_playlists))
        .cloned()
        .collect();
    playlist_files.sort();
    playlist_files.dedup();
    DeletePlan { track_paths: tracks, playlist_files }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn track(id: &str, size: u64, file: &str) -> (String, Track) {
        (
            id.to_string(),
            Track {
                id: id.into(),
                name: format!("track_{id}"),
                artist: String::new(),
                size,
                local_path: String::new(),
                device_path: format!("/sdcard/Music/{file}"),
                on_device: false,
            },
        )
    }

    fn playlist(id: &str, name: &str, checked: bool, track_ids: &[&str]) -> Playlist {
        Playlist {
            name: name.into(),
            playlist_id: id.into(),
            persistent_id: id.into(),
            track_ids: track_ids.iter().map(|s| s.to_string()).collect(),
            checked,
        }
    }

    fn file(path: &str, size: u64) -> DeviceFile {
        DeviceFile { path: path.into(), size }
    }

    fn device_playlists(names: &[&str]) -> HashMap<String, String> {
        names.iter().map(|n| (n.to_string(), format!("{n}.m3u"))).collect()
    }

    /// Library: tracks 1-4. "Keep" (checked) has 1 and 2; "Old" (unchecked)
    /// has 2 and 3; track 4 is in no playlist. Phone has all four plus a
    /// file that isn't in the library.
    struct Fixture {
        tracks: HashMap<String, Track>,
        playlists: Vec<Playlist>,
        files: Vec<DeviceFile>,
    }

    fn fixture() -> Fixture {
        Fixture {
            tracks: HashMap::from([
                track("1", 1000, "A/one.mp3"),
                track("2", 2000, "A/two.mp3"),
                track("3", 3000, "B/three.mp3"),
                track("4", 4000, "C/four.mp3"),
            ]),
            playlists: vec![
                playlist("K", "Keep", true, &["1", "2"]),
                playlist("O", "Old", false, &["2", "3"]),
            ],
            files: vec![
                file("A/one.mp3", 1000),
                file("A/two.mp3", 2000),
                file("B/three.mp3", 3000),
                file("C/four.mp3", 4000),
                file("X/stray.mp3", 9999),
                file("Keep.m3u", 30),
                file("Old.m3u", 30),
            ],
        }
    }

    fn plan(fx: &Fixture, device_pl: &[&str], options: &DeleteOptions) -> DeletePlan {
        let classified = ClassifiedFiles::new(&fx.tracks, &fx.playlists, &fx.files);
        plan_deletes(&classified, &fx.playlists, &device_playlists(device_pl), options)
    }

    #[test]
    fn nothing_is_deleted_by_default() {
        let fx = fixture();
        assert!(plan(&fx, &["Keep", "Old"], &DeleteOptions::default()).is_empty());
    }

    #[test]
    fn classification_skips_playlist_files() {
        let fx = fixture();
        let c = ClassifiedFiles::new(&fx.tracks, &fx.playlists, &fx.files);
        assert_eq!(c.orphan_paths(), vec!["X/stray.mp3"]);
        assert_eq!(c.unwanted_paths(), vec!["B/three.mp3", "C/four.mp3"]);
    }

    #[test]
    fn orphans_option_deletes_only_files_not_in_library() {
        let fx = fixture();
        let p = plan(&fx, &["Keep", "Old"], &DeleteOptions { orphans: true, ..Default::default() });
        assert_eq!(p.track_paths, vec!["X/stray.mp3"]);
        assert!(p.playlist_files.is_empty());
    }

    #[test]
    fn playlist_cleanup_removes_its_unshared_tracks_and_its_playlist_file() {
        let fx = fixture();
        let p = plan(
            &fx,
            &["Keep", "Old"],
            &DeleteOptions { cleanup_playlist_ids: HashSet::from(["O"]), ..Default::default() },
        );
        // Track 2 is also in the checked "Keep" playlist, so it stays.
        assert_eq!(p.track_paths, vec!["B/three.mp3"]);
        assert_eq!(p.playlist_files, vec!["Old.m3u"]);
    }

    #[test]
    fn playlist_cleanup_skips_playlist_file_that_is_not_on_phone() {
        let fx = fixture();
        let p = plan(
            &fx,
            &["Keep"],
            &DeleteOptions { cleanup_playlist_ids: HashSet::from(["O"]), ..Default::default() },
        );
        assert!(p.playlist_files.is_empty());
    }

    #[test]
    fn cleanup_flag_on_a_checked_playlist_does_nothing() {
        let fx = fixture();
        let p = plan(
            &fx,
            &["Keep", "Old"],
            &DeleteOptions { cleanup_playlist_ids: HashSet::from(["K"]), ..Default::default() },
        );
        assert!(p.is_empty());
    }

    #[test]
    fn everything_else_removes_all_but_checked_playlists() {
        let fx = fixture();
        let p = plan(
            &fx,
            &["Keep", "Old", "MadeOnPhone", "Gone"],
            &DeleteOptions {
                everything_else: true,
                missing_playlists: vec![MissingPlaylist { name: "Gone", delete: false }],
                ..Default::default()
            },
        );
        // Includes track 4, which is in the library but in no playlist.
        assert_eq!(p.track_paths, vec!["B/three.mp3", "C/four.mp3", "X/stray.mp3"]);
        // "MadeOnPhone" was never synced by the app, so it's left alone.
        assert_eq!(p.playlist_files, vec!["Gone.m3u", "Old.m3u"]);
    }

    #[test]
    fn missing_playlist_is_deleted_only_when_marked_and_on_phone() {
        let fx = fixture();
        let options = DeleteOptions {
            missing_playlists: vec![
                MissingPlaylist { name: "Gone", delete: true },
                MissingPlaylist { name: "Ignored", delete: false },
                MissingPlaylist { name: "AlreadyDeleted", delete: true },
            ],
            ..Default::default()
        };
        let p = plan(&fx, &["Keep", "Gone", "Ignored"], &options);
        assert_eq!(p.playlist_files, vec!["Gone.m3u"]);
        assert!(p.track_paths.is_empty());
    }

    #[test]
    fn playlist_file_is_kept_when_a_checked_playlist_shares_the_name() {
        let mut fx = fixture();
        fx.playlists.push(playlist("K2", "Old", true, &["1"]));
        let p = plan(
            &fx,
            &["Keep", "Old"],
            &DeleteOptions { everything_else: true, ..Default::default() },
        );
        assert!(p.playlist_files.is_empty());
    }

    #[test]
    fn same_sized_wanted_track_protects_the_file() {
        // Tracks 1 (wanted) and 5 (unwanted) are the same size, and the
        // file's name matches neither, so it might be the wanted one.
        let mut fx = fixture();
        fx.tracks.extend([track("5", 1000, "D/five.mp3")]);
        fx.playlists.push(playlist("P5", "Fives", false, &["5"]));
        fx.files = vec![file("Z/renamed.mp3", 1000)];
        let p = plan(
            &fx,
            &[],
            &DeleteOptions {
                everything_else: true,
                cleanup_playlist_ids: HashSet::from(["P5"]),
                ..Default::default()
            },
        );
        assert!(p.track_paths.is_empty());
    }

    #[test]
    fn filename_tiebreak_separates_same_sized_tracks() {
        // Same sizes, but the filenames say which file is which: the
        // unwanted one goes, the wanted one stays.
        let mut fx = fixture();
        fx.tracks.extend([track("5", 1000, "D/five.mp3")]);
        fx.playlists.push(playlist("P5", "Fives", false, &["5"]));
        fx.files = vec![file("A/one.mp3", 1000), file("D/five.mp3", 1000)];
        let p = plan(
            &fx,
            &[],
            &DeleteOptions { cleanup_playlist_ids: HashSet::from(["P5"]), ..Default::default() },
        );
        assert_eq!(p.track_paths, vec!["D/five.mp3"]);
    }

    #[test]
    fn every_copy_of_an_unwanted_track_is_deleted() {
        let mut fx = fixture();
        fx.files = vec![file("B/three.mp3", 3000), file("Dupes/three copy.mp3", 3000)];
        let p = plan(
            &fx,
            &[],
            &DeleteOptions { cleanup_playlist_ids: HashSet::from(["O"]), ..Default::default() },
        );
        assert_eq!(p.track_paths, vec!["B/three.mp3", "Dupes/three copy.mp3"]);
    }
}
