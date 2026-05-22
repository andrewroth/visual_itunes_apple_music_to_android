//! Playlist domain type and .m3u serialisation.
//!
//! Ruby's `Playlist#generate` writes:
//!     #EXTM3U\n
//!     <track1.playlist_path>\n
//!     <track2.playlist_path>\n
//!     ...
//!
//! `File#puts` in Ruby appends "\n" (a single LF), not CRLF, and uses the
//! default UTF-8 encoding. To keep the output byte-for-byte compatible with
//! existing .m3u files on the device — so the diff doesn't flag them as
//! "changed" cosmetically — we mirror that exactly here.

use serde::{Deserialize, Serialize};

use crate::track::Track;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub playlist_id: String,
    pub track_ids: Vec<String>,
    #[serde(default)]
    pub checked: bool,
}

impl Playlist {
    /// Generate the .m3u file contents. `device_music_root` is the device's
    /// base path; entries are written as paths relative to it (no leading `/`).
    /// `lookup` resolves a track ID to its Track. Missing tracks are silently
    /// skipped (same as Ruby — it iterates @track_ids and calls
    /// `library.tracks[track_id]`, which is nil for missing entries, but the
    /// load_playlists step already filters those out).
    pub fn generate_m3u<'a, F>(&self, device_music_root: &str, mut lookup: F) -> String
    where
        F: FnMut(&str) -> Option<&'a Track>,
    {
        let mut out = String::with_capacity(64 + self.track_ids.len() * 80);
        out.push_str("#EXTM3U\n");
        for track_id in &self.track_ids {
            if let Some(track) = lookup(track_id) {
                out.push_str(&track.playlist_path(device_music_root));
                out.push('\n');
            }
        }
        out
    }

    /// The on-device file name for the playlist (`<name>.m3u`).
    pub fn device_filename(&self) -> String {
        format!("{}.m3u", self.name)
    }
}

/// Compare two .m3u contents semantically: identical track lists ignoring
/// cosmetic differences (line-ending style, trailing whitespace, blank
/// trailing lines). Used by the sync diff so we don't re-upload a playlist
/// just because line endings differ between iSyncr-generated and our
/// generated copies.
pub fn m3u_semantically_equal(a: &str, b: &str) -> bool {
    fn normalise(s: &str) -> Vec<String> {
        s.lines()
            .map(|l| l.trim_end_matches('\r').trim_end().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }
    normalise(a) == normalise(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_track(id: &str, device_path: &str) -> Track {
        Track {
            id: id.into(),
            name: format!("track_{id}"),
            artist: "".into(),
            size: 0,
            local_path: "".into(),
            device_path: device_path.into(),
            on_device: false,
        }
    }

    #[test]
    fn m3u_format_matches_ruby_output() {
        // The Ruby Playlist#generate output for a playlist with two tracks at
        // paths Artist/Album/A.mp3 and Artist/Album/B.mp3 is exactly:
        // #EXTM3U\nArtist/Album/A.mp3\nArtist/Album/B.mp3\n
        let p = Playlist {
            name: "Test".into(),
            playlist_id: "1".into(),
            track_ids: vec!["10".into(), "11".into()],
            checked: true,
        };
        let t1 = make_track("10", "/sdcard/Music/Artist/Album/A.mp3");
        let t2 = make_track("11", "/sdcard/Music/Artist/Album/B.mp3");
        let lookup = |id: &str| -> Option<&Track> {
            match id {
                "10" => Some(&t1),
                "11" => Some(&t2),
                _ => None,
            }
        };
        let out = p.generate_m3u("/sdcard/Music/", lookup);
        assert_eq!(out, "#EXTM3U\nArtist/Album/A.mp3\nArtist/Album/B.mp3\n");
    }

    #[test]
    fn m3u_empty_playlist_is_just_header() {
        let p = Playlist {
            name: "Empty".into(),
            playlist_id: "1".into(),
            track_ids: vec![],
            checked: false,
        };
        assert_eq!(p.generate_m3u("/sdcard/Music/", |_| None), "#EXTM3U\n");
    }

    #[test]
    fn m3u_skips_missing_tracks() {
        let p = Playlist {
            name: "P".into(),
            playlist_id: "1".into(),
            track_ids: vec!["10".into(), "missing".into(), "11".into()],
            checked: false,
        };
        let t1 = make_track("10", "/sdcard/Music/A.mp3");
        let t2 = make_track("11", "/sdcard/Music/B.mp3");
        let lookup = |id: &str| -> Option<&Track> {
            match id {
                "10" => Some(&t1),
                "11" => Some(&t2),
                _ => None,
            }
        };
        assert_eq!(p.generate_m3u("/sdcard/Music/", lookup), "#EXTM3U\nA.mp3\nB.mp3\n");
    }

    #[test]
    fn semantic_equality_ignores_line_endings() {
        let lf = "#EXTM3U\nA.mp3\nB.mp3\n";
        let crlf = "#EXTM3U\r\nA.mp3\r\nB.mp3\r\n";
        assert!(m3u_semantically_equal(lf, crlf));
    }

    #[test]
    fn semantic_equality_ignores_trailing_blank_lines() {
        let a = "#EXTM3U\nA.mp3\nB.mp3\n";
        let b = "#EXTM3U\nA.mp3\nB.mp3\n\n\n";
        assert!(m3u_semantically_equal(a, b));
    }

    #[test]
    fn semantic_equality_detects_real_differences() {
        let a = "#EXTM3U\nA.mp3\nB.mp3\n";
        let b = "#EXTM3U\nA.mp3\nC.mp3\n";
        assert!(!m3u_semantically_equal(a, b));
    }

    #[test]
    fn device_filename_appends_m3u() {
        let p = Playlist {
            name: "Favourites".into(),
            playlist_id: "1".into(),
            track_ids: vec![],
            checked: false,
        };
        assert_eq!(p.device_filename(), "Favourites.m3u");
    }
}
