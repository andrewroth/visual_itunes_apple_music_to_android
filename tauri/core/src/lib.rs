//! musicsync-core: platform-independent domain logic for syncing an
//! iTunes/Apple Music library to an Android companion app.
//!
//! All testable logic (XML parsing, settings persistence, device-path
//! generation, .m3u format, size-based matching) lives here so it can be
//! exercised with `cargo test` without needing the Tauri GUI.

pub mod xml_helpers;
pub mod settings;
pub mod library;
pub mod track;
pub mod playlist;
pub mod matching;
pub mod protocol;
