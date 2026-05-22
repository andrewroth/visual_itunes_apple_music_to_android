# Viamta Music Sync

**V**isual **i**Tunes/**A**pple **M**usic **t**o **A**ndroid — sync your Apple
Music library to an Android phone over Wi-Fi.

Two pieces:

- **Desktop app** (`tauri/`) — reads your `Library.xml`, lets you pick which
  playlists to sync.
- **Android companion** (`android/`) — runs a WebSocket server on your LAN
  that the desktop pushes files and playlists to.

The desktop auto-discovers the phone via mDNS; pairing is a one-time
six-digit comparison.

## Install

Grab the latest pre-release from the [Releases page](../../releases) —
installers for Windows/macOS/Linux and an APK for Android.

## Build from source

- Desktop: see `tauri/README.md`
- Android: see `android/README.md`
- Wire protocol: `PROTOCOL.md`
