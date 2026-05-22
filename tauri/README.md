# MusicSync (Tauri desktop)

Rust + Tauri rewrite of the Ruby app. Talks to the Android companion app
over WebSocket on the LAN (see `../PROTOCOL.md`).

## Layout

```
tauri/
├── Cargo.toml              workspace root
├── core/                   pure-logic crate (no GUI deps; `cargo test`-able)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── xml_helpers.rs  ports app/lib/xml_helpers.rb
│   │   ├── settings.rs     reads + migrates legacy settings.yml
│   │   ├── library.rs      Library.xml parser (quick-xml SAX)
│   │   ├── track.rs        device_path generation
│   │   ├── playlist.rs     .m3u serialisation
│   │   ├── matching.rs     size-based device matching
│   │   └── protocol.rs     wire types (mirrored by android Protocol.kt)
│   └── tests/              fixture-based integration tests
├── src-tauri/              Tauri binary
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs         tauri::Builder + commands
│       └── sync.rs         WebSocket client + sync orchestrator
└── src/                    frontend (vanilla HTML/JS + Bootstrap)
    ├── index.html
    ├── main.js             uses invoke() + listen() (no polling)
    └── styles.css
```

## Build prerequisites

* Rust stable (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
* On Linux: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
    build-essential pkg-config`
* On macOS: Xcode command-line tools (`xcode-select --install`)
* On Windows: Visual Studio Build Tools + WebView2 (preinstalled on Win11)
* Tauri CLI (once): `cargo install tauri-cli --version '^2'`

## Running

```bash
cd tauri
cargo tauri dev               # development window
cargo tauri build             # release bundle (.app / .dmg / .deb / .msi)
```

The core crate's tests run without any GUI dependency:

```bash
cargo test -p musicsync-core
```

## Settings migration

On first launch the app looks for the legacy `../settings.yml` (the Ruby
app's config) and copies it to the OS config dir:

* macOS: `~/Library/Application Support/musicsync/settings.yml`
* Linux: `~/.config/musicsync/settings.yml`
* Windows: `%APPDATA%\musicsync\settings.yml`

The migration carries over `:checked_playlist_ids` verbatim, so playlist
selections from the Ruby app are preserved. Tested by
`settings::tests::migration_copies_legacy_into_new_path`.

## Path compatibility

`track::device_path_for_location` is a faithful port of Ruby's
`Track#initialize`. Files already on the device from the Ruby/iSyncr era
will be recognised by size match against unchanged paths — see the unit
tests in `core/src/track.rs` for the exact behaviour covered.
