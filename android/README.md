# MusicSync Companion (Android)

Native Android app that exposes a WebSocket server on the LAN, letting the
MusicSync desktop client push music files and playlists to this device's
music folder. See `../PROTOCOL.md` for the wire protocol.

## Status

Source-only scaffold. Build it by:

1. Install Android Studio (Hedgehog or newer).
2. Open the `android/` directory as a project.
3. Let Gradle sync (downloads Android SDK platform 35, build tools, AGP 8.6).
4. Connect a phone via ADB or start an emulator, then Run.

Unit tests run on the JVM (no emulator needed):

```
cd android
./gradlew test
```

The wrapper isn't committed (would be ~50 KB of jars); the first `gradle`
invocation from Android Studio will generate it. From the command line you
can also run `gradle wrapper` if you have Gradle installed system-wide.

## Architecture

| File | Responsibility |
|------|----------------|
| `Protocol.kt` | Sealed classes matching the Rust wire types in `tauri/core/src/protocol.rs` |
| `ManifestBuilder.kt` | Walks the music folder, builds the `MANIFEST` response |
| `AtomicFileWriter.kt` | Write-to-temp-then-rename for resumable uploads |
| `TokenStore.kt` | Generates/verifies the pairing token, persisted in EncryptedSharedPreferences |
| `SyncServer.kt` | Ktor WebSocket server implementing the protocol state machine |
| `SyncService.kt` | Foreground service that hosts the server |
| `MainActivity.kt` | Compose UI: start/stop, show token, show event log |

## Permissions

The app declares legacy-storage permissions so it can write to `/sdcard/Music`
matching the existing layout used by iSyncr / the Ruby app. On Android 11+
the user may need to grant "All files access" the first time. If denied,
the server falls back to the app-scoped external files dir.
