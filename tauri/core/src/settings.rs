//! Persisted user settings. YAML format preserved so the existing
//! `settings.yml` from the Ruby app can be read directly (including the
//! `:checked_playlist_ids` list — that's the whole point of the migration:
//! existing playlist selections carry over to the new app on first launch).
//!
//! On first launch the app looks for a legacy `settings.yml` in the current
//! working directory or alongside the legacy project and migrates it once into
//! the OS-appropriate config directory. After that the OS config dir is
//! authoritative.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Mirror of the Ruby `Settings` hash. Field names use `serde(rename)` with the
/// leading colon Ruby serialises as a symbol so the existing `settings.yml`
/// loads unchanged. New fields (e.g. `device_token`) are added without
/// breaking the legacy file because all fields are optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    // The `alias = ":foo"` lets us still read the colon-prefixed keys
    // from the legacy Ruby settings.yml; new writes use the clean name.
    // This is critical: without it the JSON sent to JS has the colon-
    // prefixed key and any frontend write goes into a phantom field.
    #[serde(rename = "library_path", alias = ":library_path", default,
            skip_serializing_if = "Option::is_none")]
    pub library_path: Option<String>,

    /// Legacy FTP fields. Still read on migration so users transitioning from
    /// the Ruby app see their old config; they are unused by the new protocol.
    #[serde(rename = "ftp_username", alias = ":ftp_username", default,
            skip_serializing_if = "Option::is_none")]
    pub ftp_username: Option<String>,
    #[serde(rename = "ftp_password", alias = ":ftp_password", default,
            skip_serializing_if = "Option::is_none")]
    pub ftp_password: Option<String>,
    #[serde(rename = "ftp_ip", alias = ":ftp_ip", default,
            skip_serializing_if = "Option::is_none")]
    pub ftp_ip: Option<String>,
    #[serde(rename = "ftp_port", alias = ":ftp_port", default,
            skip_serializing_if = "Option::is_none")]
    pub ftp_port: Option<String>,

    /// The device-side music root. Legacy FTP setups stored e.g. `/sdcard/Music/`.
    #[serde(rename = "ftp_path", alias = ":ftp_path", default,
            skip_serializing_if = "Option::is_none")]
    pub ftp_path: Option<String>,

    /// The critical field for migration: which playlists the user has chosen
    /// to sync. The Ruby code matches against either playlist ID or name, so
    /// we preserve string values verbatim.
    #[serde(rename = "checked_playlist_ids", alias = ":checked_playlist_ids", default)]
    pub checked_playlist_ids: Vec<String>,

    /// New field: paired device token. Set during pairing, used as the auth
    /// secret in the HELLO message of the new protocol.
    #[serde(rename = "device_token", default, skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,

    /// Friendly name of the paired phone (e.g. "Pixel 7"). Set during
    /// pairing; shown in the UI's "Paired with…" banner.
    #[serde(rename = "paired_device_name", default, skip_serializing_if = "Option::is_none")]
    pub paired_device_name: Option<String>,

    /// Parallel record of every playlist the user has ever selected, with
    /// its human-readable name and a pending action. Survives a playlist
    /// disappearing from the library: if the playlist reappears we still
    /// want it checked. If the user "Forgets" a missing playlist, both
    /// this list AND `checked_playlist_ids` lose the entry.
    #[serde(rename = "remembered_playlists", default)]
    pub remembered_playlists: Vec<RememberedPlaylist>,

    /// Whether the next sync should delete tracks from the phone that are
    /// known iTunes tracks but are no longer in any currently-checked
    /// playlist. Persists between sessions so the user only sets it once.
    #[serde(rename = "delete_unused_songs", default)]
    pub delete_unused_songs: bool,

    /// Per-playlist "clean up this playlist's tracks on next sync" flags.
    /// Only meaningful for currently-UNCHECKED playlists — clicking the
    /// Clean-up checkbox on a row queues those tracks for deletion at
    /// next sync. Persists so the queue survives a reload.
    #[serde(rename = "cleanup_playlist_ids", default)]
    pub cleanup_playlist_ids: Vec<String>,

    /// Device names ("Pixel 7", "Galaxy S24") the user has explicitly
    /// rejected during pairing. mDNS hits matching any of these are
    /// silently skipped on discovery. Cleared only by the user via the
    /// UI (no auto-pruning).
    #[serde(rename = "ignored_devices", default)]
    pub ignored_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RememberedPlaylist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub action: PlaylistAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistAction {
    /// Do nothing when this playlist is checked but missing from library.
    /// The user can still see it in the "missing" section of the UI.
    #[default]
    Ignore,
    /// On next sync, send FILE_DELETE for `<name>.m3u` to the phone.
    Delete,
}

impl Settings {
    /// Default config-file location: OS-appropriate per `directories`. On
    /// macOS this is `~/Library/Application Support/musicsync/settings.yml`;
    /// on Linux `~/.config/musicsync/settings.yml`.
    pub fn default_config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "musicsync")
            .map(|d| d.config_dir().join("settings.yml"))
    }

    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).context("failed to parse settings YAML")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading settings file {}", path.display()))?;
        Self::from_yaml_str(&text)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// First-launch migration: if the OS config path doesn't have a
    /// `settings.yml` yet, look for the legacy one in any of `legacy_paths`
    /// (typically `./settings.yml` and the user's old project directory).
    /// If found, load + write to the new location. Returns the loaded settings
    /// either way, or `Default::default()` if nothing exists yet.
    pub fn load_with_migration(
        new_path: &Path,
        legacy_paths: &[PathBuf],
    ) -> Result<Self> {
        if new_path.exists() {
            return Self::load(new_path);
        }
        for legacy in legacy_paths {
            if legacy.exists() {
                let s = Self::load(legacy)?;
                s.save(new_path)?;
                tracing::info!(
                    "migrated settings from {} to {}",
                    legacy.display(),
                    new_path.display()
                );
                return Ok(s);
            }
        }
        Ok(Self::default())
    }

    pub fn is_playlist_checked(&self, id: &str, name: &str) -> bool {
        self.checked_playlist_ids
            .iter()
            .any(|v| v == id || v == name)
    }

    pub fn set_checked_playlist_ids(&mut self, ids: Vec<String>) {
        self.checked_playlist_ids = ids;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    const RUBY_FIXTURE: &str = "---\n\
:library_path: Library.xml\n\
:ftp_username: pc\n\
:ftp_ip: 192.168.0.56\n\
:ftp_port: '6714'\n\
:ftp_password: '951808'\n\
:ftp_path: \"/sdcard/Music/\"\n\
:checked_playlist_ids:\n\
- '106'\n\
- '118'\n\
- '187'\n";

    #[test]
    fn parses_existing_ruby_settings_yml() {
        let s = Settings::from_yaml_str(RUBY_FIXTURE).expect("parse");
        assert_eq!(s.library_path.as_deref(), Some("Library.xml"));
        assert_eq!(s.ftp_username.as_deref(), Some("pc"));
        assert_eq!(s.ftp_ip.as_deref(), Some("192.168.0.56"));
        assert_eq!(s.ftp_port.as_deref(), Some("6714"));
        assert_eq!(s.ftp_path.as_deref(), Some("/sdcard/Music/"));
        assert_eq!(
            s.checked_playlist_ids,
            vec!["106".to_string(), "118".to_string(), "187".to_string()]
        );
    }

    #[test]
    fn migration_copies_legacy_into_new_path() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("legacy/settings.yml");
        let new = tmp.path().join("new/settings.yml");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, RUBY_FIXTURE).unwrap();

        let s = Settings::load_with_migration(&new, &[legacy.clone()]).expect("migrate");
        assert_eq!(s.checked_playlist_ids.len(), 3);
        assert!(new.exists(), "migration should write to new path");

        // Round-trip: reading the new path matches the original.
        let s2 = Settings::load(&new).unwrap();
        assert_eq!(s, s2);
    }

    #[test]
    fn migration_no_legacy_returns_default() {
        let tmp = TempDir::new().unwrap();
        let new = tmp.path().join("settings.yml");
        let s = Settings::load_with_migration(&new, &[]).expect("default");
        assert_eq!(s, Settings::default());
        assert!(!new.exists());
    }

    #[test]
    fn migration_prefers_new_when_both_exist() {
        let tmp = TempDir::new().unwrap();
        let new = tmp.path().join("settings.yml");
        let legacy = tmp.path().join("legacy.yml");

        let mut new_settings = Settings::default();
        new_settings.checked_playlist_ids = vec!["new1".into()];
        new_settings.save(&new).unwrap();
        std::fs::write(&legacy, RUBY_FIXTURE).unwrap();

        let s = Settings::load_with_migration(&new, &[legacy]).unwrap();
        assert_eq!(s.checked_playlist_ids, vec!["new1".to_string()]);
    }

    #[test]
    fn is_playlist_checked_matches_id_or_name() {
        let mut s = Settings::default();
        s.checked_playlist_ids = vec!["106".into(), "My Playlist".into()];
        assert!(s.is_playlist_checked("106", "Anything"));
        assert!(s.is_playlist_checked("999", "My Playlist"));
        assert!(!s.is_playlist_checked("999", "Other"));
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("s.yml");
        let mut s = Settings::default();
        s.library_path = Some("L.xml".into());
        s.checked_playlist_ids = vec!["a".into(), "b".into()];
        s.device_token = Some("tok".into());
        s.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(s, loaded);
    }
}
