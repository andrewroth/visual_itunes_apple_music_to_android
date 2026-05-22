//! Parser for the Apple Music / iTunes `Library.xml` plist format.
//!
//! The plist XML layout we care about (roughly):
//!
//! ```xml
//! <plist>
//!   <dict>
//!     <key>Music Folder</key>     <string>file://localhost/.../</string>
//!     <key>Tracks</key>
//!     <dict>
//!       <key>123</key>            <!-- track id -->
//!       <dict>
//!         <key>Track ID</key>     <integer>123</integer>
//!         <key>Name</key>         <string>...</string>
//!         <key>Size</key>         <integer>...</integer>
//!         <key>Location</key>     <string>file://...</string>   <!-- optional -->
//!         ...other keys we ignore...
//!       </dict>
//!       <key>124</key>            <dict>...</dict>
//!       ...
//!     </dict>
//!     <key>Playlists</key>
//!     <array>
//!       <dict>
//!         <key>Name</key>         <string>...</string>
//!         <key>Playlist ID</key>  <integer>...</integer>
//!         <key>Playlist Items</key>
//!         <array>
//!           <dict><key>Track ID</key><integer>123</integer></dict>
//!           ...
//!         </array>
//!       </dict>
//!       ...
//!     </array>
//!   </dict>
//! </plist>
//! ```
//!
//! The parser walks the tree in a single SAX pass, keeping a small explicit
//! stack of "what dict am I currently in." This avoids loading the whole DOM
//! and handles 5000-track libraries in well under a second.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

use crate::playlist::Playlist;
use crate::settings::Settings;
use crate::track::Track;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub music_folder: String,
    pub tracks: HashMap<String, Track>,
    pub playlists: Vec<Playlist>,
}

impl Library {
    pub fn parse_xml(xml: &str, device_music_root: &str, settings: &Settings) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        // Find the outer plist/dict and parse it.
        // The outer dict has Music Folder, Tracks, Playlists, etc. as keys.
        // We walk it manually keeping a "current key" and dispatching on it.

        let mut music_folder: Option<String> = None;
        let mut tracks: HashMap<String, Track> = HashMap::new();
        let mut playlists: Vec<Playlist> = Vec::new();

        // Depth tracking: we want to operate on the *outer* dict only.
        // Depth 0 before any element, 1 inside <plist>, 2 inside <plist><dict>.
        let mut depth: i32 = 0;
        let mut current_key: Option<String> = None;

        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => {
                    let name_bytes = e.name();
                    let name = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("").to_string();
                    match name.as_str() {
                        "plist" => { depth += 1; }
                        "dict" => {
                            depth += 1;
                            // At depth 3 inside the outer dict, we're entering a value-dict for the current_key.
                            if depth == 3 {
                                match current_key.as_deref() {
                                    Some("Tracks") => {
                                        parse_tracks_dict(&mut reader, &mut tracks, music_folder.as_deref().unwrap_or(""), device_music_root)?;
                                        depth -= 1; // parse_tracks_dict consumed the matching </dict>
                                        current_key = None;
                                    }
                                    _ => { /* unknown top-level dict value — skip via depth tracking */ }
                                }
                            }
                        }
                        "array" => {
                            if depth == 2 && current_key.as_deref() == Some("Playlists") {
                                parse_playlists_array(&mut reader, &mut playlists, &tracks, settings)?;
                                current_key = None;
                            }
                        }
                        "key" => {
                            if depth == 2 {
                                current_key = Some(read_text(&mut reader, "key")?);
                            } else {
                                // skip
                                skip_to_end(&mut reader, b"key")?;
                            }
                        }
                        "string" => {
                            if depth == 2 {
                                let val = read_text(&mut reader, "string")?;
                                if current_key.as_deref() == Some("Music Folder") {
                                    music_folder = Some(val);
                                }
                                current_key = None;
                            } else {
                                skip_to_end(&mut reader, b"string")?;
                            }
                        }
                        _ => {
                            // Other scalar value at depth 2: clear key and skip.
                            if depth == 2 {
                                current_key = None;
                            }
                            skip_to_end(&mut reader, name.as_bytes())?;
                        }
                    }
                }
                Event::End(e) => {
                    let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                    if name == "dict" || name == "plist" {
                        depth -= 1;
                    }
                    if depth < 0 { break; }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }

        let music_folder = music_folder
            .ok_or_else(|| anyhow!("Library.xml missing Music Folder key"))?;

        Ok(Library { music_folder, tracks, playlists })
    }

    pub fn parse_file(path: &std::path::Path, device_music_root: &str, settings: &Settings) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse_xml(&text, device_music_root, settings)
    }
}

/// Read the text content of the current open element (e.g. `<key>foo</key>`).
/// Consumes events through the matching end tag.
fn read_text(reader: &mut Reader<&[u8]>, tag: &str) -> Result<String> {
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Text(t) => out.push_str(&t.unescape()?),
            Event::CData(t) => out.push_str(std::str::from_utf8(t.as_ref())?),
            Event::End(e) if e.name().as_ref() == tag.as_bytes() => return Ok(out),
            Event::Eof => return Err(anyhow!("unexpected EOF reading <{tag}>")),
            _ => {}
        }
        buf.clear();
    }
}

/// Skip over the rest of an element including any nested children, stopping
/// after the matching closing tag.
fn skip_to_end(reader: &mut Reader<&[u8]>, tag: &[u8]) -> Result<()> {
    // It might already be self-closing or empty — read events until we see
    // the matching End at depth 0.
    let mut depth = 1;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) if e.name().as_ref() == tag => depth += 1,
            Event::End(e) if e.name().as_ref() == tag => {
                depth -= 1;
                if depth == 0 { return Ok(()); }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

/// Parse the value-dict for the "Tracks" key. We're called *after* the opening
/// `<dict>` has been read; we consume until and including the matching `</dict>`.
///
/// Layout inside:
///   <key>track_id</key>
///   <dict>...track fields...</dict>
///   ... repeated ...
fn parse_tracks_dict(
    reader: &mut Reader<&[u8]>,
    tracks: &mut HashMap<String, Track>,
    music_folder: &str,
    device_music_root: &str,
) -> Result<()> {
    let mut buf = Vec::new();
    let mut current_key: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                match name.as_str() {
                    "key" => {
                        current_key = Some(read_text(reader, "key")?);
                    }
                    "dict" => {
                        // This is a track's dict; current_key holds the track id string.
                        let track_id = current_key.take().unwrap_or_default();
                        let track_fields = parse_track_fields(reader)?;
                        if let Some(location) = track_fields.location {
                            // Skip tracks without Location (cloud-only/etc.) — matches Ruby.
                            let track = Track::from_xml_fields(
                                track_id.clone(),
                                track_fields.name.unwrap_or_default(),
                                track_fields.artist.unwrap_or_default(),
                                track_fields.size.unwrap_or(0),
                                &location,
                                music_folder,
                                device_music_root,
                            );
                            tracks.insert(track_id, track);
                        }
                    }
                    other => skip_to_end(reader, other.as_bytes())?,
                }
            }
            Event::End(e) if e.name().as_ref() == b"dict" => return Ok(()),
            Event::Eof => return Err(anyhow!("unexpected EOF in Tracks dict")),
            _ => {}
        }
        buf.clear();
    }
}

#[derive(Default)]
struct TrackFields {
    name: Option<String>,
    artist: Option<String>,
    size: Option<u64>,
    location: Option<String>,
}

/// Parse the inside of a single track's `<dict>`, called after its opening
/// tag has been read. Consumes through the matching `</dict>`.
fn parse_track_fields(reader: &mut Reader<&[u8]>) -> Result<TrackFields> {
    let mut fields = TrackFields::default();
    let mut current_key: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                match name.as_str() {
                    "key" => current_key = Some(read_text(reader, "key")?),
                    "string" => {
                        let v = read_text(reader, "string")?;
                        match current_key.as_deref() {
                            Some("Name") => fields.name = Some(v),
                            Some("Artist") => fields.artist = Some(v),
                            Some("Location") => fields.location = Some(v),
                            _ => {}
                        }
                        current_key = None;
                    }
                    "integer" => {
                        let v = read_text(reader, "integer")?;
                        if current_key.as_deref() == Some("Size") {
                            fields.size = v.parse::<u64>().ok();
                        }
                        current_key = None;
                    }
                    "date" | "data" => {
                        skip_to_end(reader, name.as_bytes())?;
                        current_key = None;
                    }
                    other => {
                        skip_to_end(reader, other.as_bytes())?;
                        current_key = None;
                    }
                }
            }
            Event::Empty(e) => {
                // Self-closing element like <true/>, <false/>.
                let _ = e;
                current_key = None;
            }
            Event::End(e) if e.name().as_ref() == b"dict" => return Ok(fields),
            Event::Eof => return Err(anyhow!("unexpected EOF in track dict")),
            _ => {}
        }
        buf.clear();
    }
}

/// Parse the `<array>` for the "Playlists" key. Called after the opening
/// `<array>` is read.
fn parse_playlists_array(
    reader: &mut Reader<&[u8]>,
    playlists: &mut Vec<Playlist>,
    tracks: &HashMap<String, Track>,
    settings: &Settings,
) -> Result<()> {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) if e.name().as_ref() == b"dict" => {
                let pl = parse_one_playlist(reader, tracks)?;
                if let Some(mut pl) = pl {
                    pl.checked = settings.is_playlist_checked(&pl.playlist_id, &pl.name);
                    playlists.push(pl);
                }
            }
            Event::End(e) if e.name().as_ref() == b"array" => return Ok(()),
            Event::Eof => return Err(anyhow!("unexpected EOF in Playlists array")),
            _ => {}
        }
        buf.clear();
    }
}

fn parse_one_playlist(
    reader: &mut Reader<&[u8]>,
    tracks: &HashMap<String, Track>,
) -> Result<Option<Playlist>> {
    let mut name: Option<String> = None;
    let mut playlist_id: Option<String> = None;
    let mut track_ids: Option<Vec<String>> = None;
    let mut current_key: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let n = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                match n.as_str() {
                    "key" => current_key = Some(read_text(reader, "key")?),
                    "string" => {
                        let v = read_text(reader, "string")?;
                        if current_key.as_deref() == Some("Name") { name = Some(v); }
                        current_key = None;
                    }
                    "integer" => {
                        let v = read_text(reader, "integer")?;
                        if current_key.as_deref() == Some("Playlist ID") {
                            playlist_id = Some(v);
                        }
                        current_key = None;
                    }
                    "array" => {
                        if current_key.as_deref() == Some("Playlist Items") {
                            track_ids = Some(parse_playlist_items(reader)?);
                        } else {
                            skip_to_end(reader, b"array")?;
                        }
                        current_key = None;
                    }
                    other => {
                        skip_to_end(reader, other.as_bytes())?;
                        current_key = None;
                    }
                }
            }
            Event::Empty(_) => { current_key = None; }
            Event::End(e) if e.name().as_ref() == b"dict" => break,
            Event::Eof => return Err(anyhow!("unexpected EOF in playlist dict")),
            _ => {}
        }
        buf.clear();
    }

    let Some(name) = name else { return Ok(None); };
    let Some(playlist_id) = playlist_id else { return Ok(None); };
    let Some(mut track_ids) = track_ids else {
        // Playlists with no Playlist Items array (smart playlists, etc.)
        // are skipped — matches the Ruby behaviour.
        return Ok(None);
    };

    // Drop track IDs that don't have a corresponding entry in the Tracks dict
    // (e.g. movies, removed media). Matches Ruby `track_ids.select!`.
    track_ids.retain(|id| tracks.contains_key(id));

    Ok(Some(Playlist { name, playlist_id, track_ids, checked: false }))
}

/// Parse the array of Playlist Items, each of which is
/// `<dict><key>Track ID</key><integer>N</integer></dict>`. Called after the
/// opening `<array>` is read.
fn parse_playlist_items(reader: &mut Reader<&[u8]>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) if e.name().as_ref() == b"dict" => {
                // Read until </dict>, collecting a Track ID integer.
                let mut current_key: Option<String> = None;
                let mut buf2 = Vec::new();
                loop {
                    match reader.read_event_into(&mut buf2)? {
                        Event::Start(e2) => {
                            let n = std::str::from_utf8(e2.name().as_ref()).unwrap_or("").to_string();
                            match n.as_str() {
                                "key" => current_key = Some(read_text(reader, "key")?),
                                "integer" => {
                                    let v = read_text(reader, "integer")?;
                                    if current_key.as_deref() == Some("Track ID") {
                                        out.push(v);
                                    }
                                    current_key = None;
                                }
                                other => { skip_to_end(reader, other.as_bytes())?; current_key = None; }
                            }
                        }
                        Event::End(e2) if e2.name().as_ref() == b"dict" => break,
                        Event::Eof => return Err(anyhow!("EOF in playlist item dict")),
                        _ => {}
                    }
                    buf2.clear();
                }
            }
            Event::End(e) if e.name().as_ref() == b"array" => return Ok(out),
            Event::Eof => return Err(anyhow!("EOF in playlist items array")),
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const MINIMAL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Music Folder</key><string>file://localhost/Users/u/Music/iTunes/iTunes%20Media/</string>
    <key>Tracks</key>
    <dict>
        <key>10</key>
        <dict>
            <key>Track ID</key><integer>10</integer>
            <key>Name</key><string>Song A</string>
            <key>Size</key><integer>1000</integer>
            <key>Location</key><string>file://localhost/Users/u/Music/iTunes/iTunes%20Media/Music/Artist/Album/A.mp3</string>
        </dict>
        <key>11</key>
        <dict>
            <key>Track ID</key><integer>11</integer>
            <key>Name</key><string>Song B</string>
            <key>Size</key><integer>2000</integer>
            <key>Location</key><string>file://localhost/Users/u/Music/iTunes/iTunes%20Media/Music/Artist/Album/B.mp3</string>
        </dict>
        <key>12</key>
        <dict>
            <key>Track ID</key><integer>12</integer>
            <key>Name</key><string>Cloud Only</string>
            <key>Size</key><integer>3000</integer>
        </dict>
    </dict>
    <key>Playlists</key>
    <array>
        <dict>
            <key>Name</key><string>My Playlist</string>
            <key>Playlist ID</key><integer>100</integer>
            <key>Playlist Items</key>
            <array>
                <dict><key>Track ID</key><integer>10</integer></dict>
                <dict><key>Track ID</key><integer>11</integer></dict>
                <dict><key>Track ID</key><integer>12</integer></dict>
            </array>
        </dict>
        <dict>
            <key>Name</key><string>Smart Playlist</string>
            <key>Playlist ID</key><integer>200</integer>
        </dict>
    </array>
</dict>
</plist>"#;

    #[test]
    fn parses_minimal_library() {
        let s = Settings::default();
        let lib = Library::parse_xml(MINIMAL_XML, "/sdcard/Music/", &s).expect("parse");
        assert_eq!(lib.music_folder, "file://localhost/Users/u/Music/iTunes/iTunes%20Media/");
        // Track 12 had no Location — skipped.
        assert_eq!(lib.tracks.len(), 2);
        assert!(lib.tracks.contains_key("10"));
        assert!(lib.tracks.contains_key("11"));
        assert_eq!(lib.tracks["10"].name, "Song A");
        assert_eq!(lib.tracks["10"].size, 1000);
        assert_eq!(lib.tracks["10"].device_path, "/sdcard/Music/Artist/Album/A.mp3");
    }

    #[test]
    fn parses_playlists_filtering_missing_tracks_and_smart_playlists() {
        let s = Settings::default();
        let lib = Library::parse_xml(MINIMAL_XML, "/sdcard/Music/", &s).expect("parse");
        // Only "My Playlist" should appear — "Smart Playlist" had no items.
        assert_eq!(lib.playlists.len(), 1);
        let p = &lib.playlists[0];
        assert_eq!(p.name, "My Playlist");
        assert_eq!(p.playlist_id, "100");
        // Track 12 referenced in items but absent from Tracks dict — filtered.
        assert_eq!(p.track_ids, vec!["10".to_string(), "11".to_string()]);
        assert!(!p.checked);
    }

    #[test]
    fn checked_state_picks_up_from_settings_by_id() {
        let mut s = Settings::default();
        s.checked_playlist_ids = vec!["100".into()];
        let lib = Library::parse_xml(MINIMAL_XML, "/sdcard/Music/", &s).expect("parse");
        assert!(lib.playlists[0].checked);
    }

    #[test]
    fn checked_state_picks_up_from_settings_by_name() {
        let mut s = Settings::default();
        s.checked_playlist_ids = vec!["My Playlist".into()];
        let lib = Library::parse_xml(MINIMAL_XML, "/sdcard/Music/", &s).expect("parse");
        assert!(lib.playlists[0].checked, "name-based match should set checked");
    }
}
