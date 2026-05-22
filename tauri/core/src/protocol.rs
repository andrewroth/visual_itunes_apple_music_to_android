//! Wire protocol types shared between the desktop client (Rust) and the
//! Android server (Kotlin).
//!
//! Transport: a single WebSocket connection per sync session. JSON text
//! frames for control messages; binary frames for file payloads. The
//! `kind` discriminator is the JSON message type.
//!
//! Sequence:
//!   client → HELLO {token, protocol_version}
//!   server → HELLO_OK {device_name, music_root, protocol_version}
//!   client → MANIFEST_REQUEST
//!   server → MANIFEST {files, playlists}
//!   client → FILE_PUT {path, size}, then a binary frame of `size` bytes
//!   server → FILE_OK {path} | FILE_ERR {path, message}
//!   client → PLAYLIST_PUT {name, content}
//!   server → PLAYLIST_OK {name} | PLAYLIST_ERR {name, message}
//!   client → FILE_DELETE {path}    (optional)
//!   server → FILE_DELETE_OK {path} | FILE_DELETE_ERR
//!   client → BYE
//!   server → BYE
//!
//! Server may push PROGRESS messages at any time after the session is
//! authenticated. (Currently unused; placeholder for future per-chunk
//! reporting on slow devices.)

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 7800;
pub const MDNS_SERVICE_TYPE: &str = "_musicsync._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientMessage {
    Hello {
        token: String,
        protocol_version: u32,
        /// Same identity fields the desktop announces in PAIR_REQUEST.
        /// Used on the phone for the "approve unknown token?" dialog so
        /// the user sees "andrew@192.168.0.42" rather than "(unknown)".
        #[serde(default)]
        desktop_user: String,
        #[serde(default)]
        desktop_host: String,
    },
    /// First step of bluetooth-style numeric comparison. Sent instead of
    /// HELLO when the desktop has no stored token yet. Phone replies with
    /// PAIR_CHALLENGE and waits for PAIR_CONFIRM plus user tap on the phone.
    ///
    /// `desktop_user` and `desktop_host` are best-effort identifiers shown
    /// on the phone's confirm dialog so the user knows which machine is
    /// asking ("andrew@192.168.0.42" style label). Empty strings are
    /// allowed when the platform doesn't provide them.
    PairRequest {
        protocol_version: u32,
        #[serde(default)]
        desktop_user: String,
        #[serde(default)]
        desktop_host: String,
    },
    /// Sent by the desktop after the user clicks Confirm on the desktop
    /// dialog. The phone resolves the pair (issuing a PAIR_OK with the
    /// long-term token) once it has *also* received a user tap on its side.
    PairConfirm,
    PairCancel,
    ManifestRequest,
    /// Announces an incoming binary frame containing `size` bytes to be
    /// written to `path` (path is relative to the music root). The next
    /// WebSocket binary frame after this message is the payload.
    FilePut { path: String, size: u64 },
    PlaylistPut { name: String, content: String },
    FileDelete { path: String },
    Bye,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerMessage {
    HelloOk {
        device_name: String,
        music_root: String,
        protocol_version: u32,
    },
    /// 6-digit comparison code. Both sides display it; the user verifies
    /// the same digits appear on both screens before tapping Confirm.
    PairChallenge { code: String, device_name: String },
    /// Echo of the desktop's announced identity so the user has feedback
    /// on which machine just connected (for log lines etc.). Optional.
    PairPeerInfo { desktop_user: String, desktop_host: String },
    /// Pairing succeeded. `token` is the persistent secret the desktop
    /// should store and send in HELLO from now on.
    PairOk { token: String, device_name: String, music_root: String },
    /// Pairing aborted (timeout, user cancelled on the phone, or other).
    PairCancelled { reason: String },
    Manifest {
        files: Vec<ManifestFile>,
        playlists: Vec<ManifestPlaylist>,
    },
    FileOk { path: String },
    FileErr { path: String, message: String },
    PlaylistOk { name: String },
    PlaylistErr { name: String, message: String },
    FileDeleteOk { path: String },
    FileDeleteErr { path: String, message: String },
    Progress {
        /// What's currently happening, e.g. "uploading X" or "walking filesystem".
        message: String,
        /// 0.0 .. 1.0 if known.
        fraction: Option<f32>,
    },
    Bye,
    Error { message: String },
}

/// One entry in the file portion of the manifest. Path is relative to the
/// device's music root (no leading `/`), e.g. `Artist/Album/Track.mp3`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    /// Unix epoch seconds.
    pub mtime: i64,
}

/// One entry in the playlist portion of the manifest. `content` is the full
/// .m3u file text — playlists are tiny, inlining them saves a round-trip per
/// playlist and lets the Mac diff in-memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestPlaylist {
    pub name: String,
    pub mtime: i64,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_roundtrips() {
        let m = ClientMessage::Hello {
            token: "secret".into(),
            protocol_version: PROTOCOL_VERSION,
            desktop_user: "andrew".into(),
            desktop_host: "192.168.0.42".into(),
        };
        let j = serde_json::to_string(&m).unwrap();
        assert!(j.contains("\"kind\":\"HELLO\""));
        let back: ClientMessage = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn manifest_serialises_with_inlined_playlist_content() {
        let s = ServerMessage::Manifest {
            files: vec![ManifestFile {
                path: "Artist/Album/A.mp3".into(),
                size: 1000,
                mtime: 1716300000,
            }],
            playlists: vec![ManifestPlaylist {
                name: "Favourites".into(),
                mtime: 1716300000,
                content: "#EXTM3U\nArtist/Album/A.mp3\n".into(),
            }],
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"kind\":\"MANIFEST\""));
        assert!(j.contains("Artist/Album/A.mp3"));
        assert!(j.contains("#EXTM3U"));
        let back: ServerMessage = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn file_put_roundtrips() {
        let m = ClientMessage::FilePut { path: "x/y.mp3".into(), size: 42 };
        let j = serde_json::to_string(&m).unwrap();
        let back: ClientMessage = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn unknown_kind_rejected() {
        let j = r#"{"kind":"NONSENSE"}"#;
        assert!(serde_json::from_str::<ClientMessage>(j).is_err());
    }
}
