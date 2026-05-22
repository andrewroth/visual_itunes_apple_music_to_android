//! Track domain type and path-generation logic.
//!
//! `device_path_for_location` is the load-bearing function — it has to produce
//! the *exact same string* the Ruby `Track#initialize` does, byte-for-byte.
//! If it diverges, existing files on the phone won't be recognised by size
//! match (because the recorded `device_location` won't line up with what the
//! manifest reports), and the app will try to re-upload everything. Unit
//! tests pin down the behaviour with cases drawn from the Ruby source.

use serde::{Deserialize, Serialize};

use crate::xml_helpers::{strip_url_file_path_starting, unescape_xml};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    /// Numeric Track ID from Library.xml (string-typed because it's used as a
    /// hash key throughout).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    pub size: u64,
    /// Local filesystem path to the source file (e.g. on the Mac).
    pub local_path: String,
    /// Device-side absolute path under the music root, e.g.
    /// `/sdcard/Music/Artist/Album/01 Track.mp3`. Matches Ruby's
    /// `device_location`.
    pub device_path: String,
    /// True once the matching algorithm has determined the device has a file
    /// with this size (and filename, if disambiguating).
    #[serde(default)]
    pub on_device: bool,
}

impl Track {
    /// Build a Track from raw Library.xml field values.
    /// `music_folder` is the iTunes "Music Folder" key (a `file://` URL); it
    /// is used to compute the path of `location` relative to the music root.
    /// `device_music_root` is the base path on the device (e.g.
    /// `/sdcard/Music/`).
    pub fn from_xml_fields(
        id: String,
        name: String,
        artist: String,
        size: u64,
        location: &str,
        music_folder: &str,
        device_music_root: &str,
    ) -> Self {
        let device_path = device_path_for_location(location, music_folder, device_music_root);
        let local_path = strip_url_file_path_starting(&unescape_xml(location));
        Self {
            id,
            name,
            artist,
            size,
            local_path,
            device_path,
            on_device: false,
        }
    }

    /// The path written into a .m3u file. Matches Ruby's `playlist_path`:
    /// device_path with the base music root prefix stripped, and any leading
    /// `/` removed.
    pub fn playlist_path(&self, device_music_root: &str) -> String {
        if let Some(rest) = self.device_path.strip_prefix(device_music_root) {
            return rest.trim_start_matches('/').to_string();
        }
        self.device_path.trim_start_matches('/').to_string()
    }
}

/// Pure function: given the raw iTunes Location URL, the iTunes Music Folder
/// URL, and the device's music root path, produce the device-side absolute
/// path for this track.
///
/// Behaviour reproduced from `app/models/track.rb`:
/// 1. If the music_folder appears as a substring of the location, take the
///    portion of the location after it.
/// 2. Otherwise, fall back to the last three path components of the location.
/// 3. Unescape XML/percent-encoded entities.
/// 4. Strip a leading `Music/` or `/Music/` (legacy iSyncr layout).
/// 5. Strip a leading `mp3/` or `/mp3/` (another legacy layout).
/// 6. Prepend the device music root.
pub fn device_path_for_location(
    location: &str,
    music_folder: &str,
    device_music_root: &str,
) -> String {
    let relative_raw = if let Some(idx) = location.find(music_folder) {
        location[idx + music_folder.len()..].to_string()
    } else {
        // last 3 path components, joined with '/'. Empty result if the
        // location has fewer than 3 components.
        let parts: Vec<&str> = location.split('/').collect();
        let n = parts.len();
        let take = n.min(3);
        parts[n - take..].join("/")
    };

    let mut device_relative = unescape_xml(&relative_raw);

    // Strip the legacy iSyncr "Music/" or "mp3/" sub-prefix if present, with
    // an optional leading slash.
    device_relative = strip_leading_dir(&device_relative, "Music");
    device_relative = strip_leading_dir(&device_relative, "mp3");

    join_path(device_music_root, &device_relative)
}

fn strip_leading_dir(s: &str, dir: &str) -> String {
    let pat_no_slash = format!("{dir}/");
    let pat_slash = format!("/{dir}/");
    if let Some(rest) = s.strip_prefix(&pat_slash) {
        return rest.to_string();
    }
    if let Some(rest) = s.strip_prefix(&pat_no_slash) {
        return rest.to_string();
    }
    s.to_string()
}

fn join_path(base: &str, rest: &str) -> String {
    let base_trimmed = base.trim_end_matches('/');
    let rest_trimmed = rest.trim_start_matches('/');
    format!("{base_trimmed}/{rest_trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // Music folder URLs as iTunes records them.
    const MUSIC_FOLDER: &str = "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/";
    const DEVICE_ROOT: &str = "/sdcard/Music/";

    #[test]
    fn device_path_strips_music_folder_prefix() {
        let location =
            "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/Music/Artist/Album/01%20Track.mp3";
        let path = device_path_for_location(location, MUSIC_FOLDER, DEVICE_ROOT);
        // Music/ subfolder should also be stripped by step 4.
        assert_eq!(path, "/sdcard/Music/Artist/Album/01 Track.mp3");
    }

    #[test]
    fn device_path_strips_leading_mp3_dir() {
        let location =
            "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/mp3/Artist/Album/Track.mp3";
        let path = device_path_for_location(location, MUSIC_FOLDER, DEVICE_ROOT);
        assert_eq!(path, "/sdcard/Music/Artist/Album/Track.mp3");
    }

    #[test]
    fn device_path_falls_back_to_last_three_components() {
        // Location not under the iTunes media folder at all — fall through to
        // last-three behaviour.
        let location = "file:///some/random/place/Band/Album/Song.mp3";
        let path = device_path_for_location(location, MUSIC_FOLDER, DEVICE_ROOT);
        assert_eq!(path, "/sdcard/Music/Band/Album/Song.mp3");
    }

    #[test]
    fn device_path_unescapes_percent_utf8() {
        // Artist name with combining diaeresis.
        let location =
            "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/Music/Crumba%CC%88cher/Album/Song.mp3";
        let path = device_path_for_location(location, MUSIC_FOLDER, DEVICE_ROOT);
        // Decomposed "a" + combining diaeresis (matches Ruby's byte-level
        // decode of %CC%88 = U+0308).
        assert_eq!(path, "/sdcard/Music/Crumba\u{0308}cher/Album/Song.mp3");
    }

    #[test]
    fn device_path_handles_device_root_without_trailing_slash() {
        let location =
            "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/Music/A/B/C.mp3";
        let path = device_path_for_location(location, MUSIC_FOLDER, "/sdcard/Music");
        assert_eq!(path, "/sdcard/Music/A/B/C.mp3");
    }

    #[test]
    fn playlist_path_strips_root_and_leading_slash() {
        let t = Track {
            id: "1".into(),
            name: "x".into(),
            size: 0,
            artist: "".into(),
            local_path: "".into(),
            device_path: "/sdcard/Music/Artist/Album/Track.mp3".into(),
            on_device: false,
        };
        assert_eq!(
            t.playlist_path("/sdcard/Music/"),
            "Artist/Album/Track.mp3"
        );
        // Also works when root has no trailing slash.
        assert_eq!(
            t.playlist_path("/sdcard/Music"),
            "Artist/Album/Track.mp3"
        );
    }

    #[test]
    fn from_xml_fields_populates_both_paths() {
        let t = Track::from_xml_fields(
            "42".into(),
            "Test".into(),
            "Some Artist".into(),
            12345,
            "file://localhost/Users/andrew/Music/iTunes/iTunes%20Media/Music/A/B/C.mp3",
            MUSIC_FOLDER,
            DEVICE_ROOT,
        );
        assert_eq!(t.id, "42");
        assert_eq!(t.artist, "Some Artist");
        assert_eq!(t.size, 12345);
        assert_eq!(t.device_path, "/sdcard/Music/A/B/C.mp3");
        assert_eq!(t.local_path, "/Users/andrew/Music/iTunes/iTunes Media/Music/A/B/C.mp3");
        assert!(!t.on_device);
    }
}
