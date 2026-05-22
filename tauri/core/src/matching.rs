//! Size-based device-to-library matching. Mirrors
//! `Library#match_device_tracks` in the Ruby code: group library tracks by
//! size, then for each device entry of matching size mark the corresponding
//! tracks as `on_device = true`. The Ruby version marks every track sharing
//! that size, which can over-mark when multiple library tracks happen to be
//! the same size; we do the same to preserve behaviour, and provide a
//! filename-tiebreaker variant that's stricter when the user wants fewer
//! false positives.

use std::collections::HashMap;

use crate::track::Track;

/// One file entry as reported by the device's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFile {
    /// Absolute path on the device (or relative to its music root — only the
    /// trailing basename matters for tiebreaking).
    pub path: String,
    pub size: u64,
}

impl DeviceFile {
    pub fn basename(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Group tracks by size for O(1) lookup. Returns a map from size to list of
/// track IDs at that size.
pub fn tracks_by_size(tracks: &HashMap<String, Track>) -> HashMap<u64, Vec<String>> {
    let mut by_size: HashMap<u64, Vec<String>> = HashMap::new();
    for (id, track) in tracks {
        by_size.entry(track.size).or_default().push(id.clone());
    }
    by_size
}

/// Update `tracks` in place: clear `on_device` for all, then for every
/// device file of matching size, set on_device=true on every same-sized
/// library track (loose match, matching Ruby behaviour).
pub fn mark_on_device_loose(tracks: &mut HashMap<String, Track>, device_files: &[DeviceFile]) {
    for t in tracks.values_mut() {
        t.on_device = false;
    }
    let by_size = tracks_by_size(tracks);
    for f in device_files {
        if let Some(ids) = by_size.get(&f.size) {
            for id in ids {
                if let Some(t) = tracks.get_mut(id) {
                    t.on_device = true;
                }
            }
        }
    }
}

/// Stricter variant: when multiple library tracks share a size, only mark the
/// one whose filename also matches the device file's basename. If no name
/// match exists, fall back to loose-marking all of them (same as Ruby's
/// fallback path).
pub fn mark_on_device_strict(tracks: &mut HashMap<String, Track>, device_files: &[DeviceFile]) {
    for t in tracks.values_mut() {
        t.on_device = false;
    }
    let by_size = tracks_by_size(tracks);

    for f in device_files {
        let Some(ids) = by_size.get(&f.size) else {
            continue;
        };
        let device_basename = f.basename();

        // Try filename match first.
        let mut name_matched = false;
        for id in ids {
            if let Some(t) = tracks.get_mut(id) {
                if track_basename(&t.device_path) == device_basename {
                    t.on_device = true;
                    name_matched = true;
                }
            }
        }
        // Fall back to marking every same-sized track when no filename match
        // was possible.
        if !name_matched {
            for id in ids {
                if let Some(t) = tracks.get_mut(id) {
                    t.on_device = true;
                }
            }
        }
    }
}

fn track_basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Given a set of playlists, compute the unique list of track IDs that need
/// to be uploaded: union of all checked playlists' track_ids minus tracks
/// already on the device. Order preserved by first appearance, matching what
/// `copy_to_device` in the Ruby code produces via `flatten.uniq`.
pub fn tracks_to_upload(
    checked_playlists: &[&crate::playlist::Playlist],
    tracks: &HashMap<String, Track>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for pl in checked_playlists {
        for id in &pl.track_ids {
            if seen.insert(id.clone()) {
                if let Some(t) = tracks.get(id) {
                    if !t.on_device {
                        out.push(id.clone());
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_track(id: &str, size: u64, device_path: &str) -> Track {
        Track {
            id: id.into(),
            name: format!("track_{id}"),
            size,
            artist: "".into(),
            local_path: "".into(),
            device_path: device_path.into(),
            on_device: false,
        }
    }

    #[test]
    fn loose_marks_all_same_sized_tracks() {
        let mut tracks = HashMap::new();
        tracks.insert("1".into(), make_track("1", 1000, "/sdcard/Music/A.mp3"));
        tracks.insert("2".into(), make_track("2", 1000, "/sdcard/Music/B.mp3"));
        tracks.insert("3".into(), make_track("3", 2000, "/sdcard/Music/C.mp3"));
        let device = vec![DeviceFile { path: "/sdcard/Music/A.mp3".into(), size: 1000 }];
        mark_on_device_loose(&mut tracks, &device);
        assert!(tracks["1"].on_device);
        assert!(tracks["2"].on_device, "loose mode marks both same-sized tracks");
        assert!(!tracks["3"].on_device);
    }

    #[test]
    fn strict_uses_filename_to_disambiguate() {
        let mut tracks = HashMap::new();
        tracks.insert("1".into(), make_track("1", 1000, "/sdcard/Music/A.mp3"));
        tracks.insert("2".into(), make_track("2", 1000, "/sdcard/Music/B.mp3"));
        let device = vec![DeviceFile { path: "/sdcard/Music/A.mp3".into(), size: 1000 }];
        mark_on_device_strict(&mut tracks, &device);
        assert!(tracks["1"].on_device);
        assert!(!tracks["2"].on_device, "strict mode disambiguates by filename");
    }

    #[test]
    fn strict_falls_back_to_loose_when_no_filename_match() {
        let mut tracks = HashMap::new();
        tracks.insert("1".into(), make_track("1", 1000, "/sdcard/Music/A.mp3"));
        tracks.insert("2".into(), make_track("2", 1000, "/sdcard/Music/B.mp3"));
        // Device has a same-sized file with a name matching neither track.
        let device = vec![DeviceFile { path: "/sdcard/Music/Other.mp3".into(), size: 1000 }];
        mark_on_device_strict(&mut tracks, &device);
        assert!(tracks["1"].on_device);
        assert!(tracks["2"].on_device);
    }

    #[test]
    fn marks_clear_on_re_run() {
        let mut tracks = HashMap::new();
        tracks.insert("1".into(), make_track("1", 1000, "/sdcard/Music/A.mp3"));
        tracks.get_mut("1").unwrap().on_device = true;
        mark_on_device_loose(&mut tracks, &[]);
        assert!(!tracks["1"].on_device, "empty device should clear marks");
    }

    #[test]
    fn tracks_to_upload_dedups_and_filters_on_device() {
        let mut tracks = HashMap::new();
        tracks.insert("1".into(), make_track("1", 1, "/sdcard/Music/A.mp3"));
        tracks.insert("2".into(), make_track("2", 2, "/sdcard/Music/B.mp3"));
        tracks.insert("3".into(), make_track("3", 3, "/sdcard/Music/C.mp3"));
        tracks.get_mut("2").unwrap().on_device = true;

        let p1 = crate::playlist::Playlist {
            name: "P1".into(),
            playlist_id: "p1".into(),
            persistent_id: "PID-P1".into(),
            track_ids: vec!["1".into(), "2".into()],
            checked: true,
        };
        let p2 = crate::playlist::Playlist {
            name: "P2".into(),
            playlist_id: "p2".into(),
            persistent_id: "PID-P2".into(),
            track_ids: vec!["2".into(), "3".into(), "1".into()],
            checked: true,
        };
        let upload = tracks_to_upload(&[&p1, &p2], &tracks);
        // 1 first (from P1), 3 next (first new in P2). 2 filtered (on device).
        assert_eq!(upload, vec!["1".to_string(), "3".to_string()]);
    }
}
