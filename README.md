# Viamta Music Sync

**V**isual **i**Tunes/**A**pple **M**usic **t**o **A**ndroid

Visually select playlists to sync to an android device.

Desktop app available for Mac / Windows / Linux. Connects to Android companion app for fast transfer.

Scans music files via companion app and tansfers only what's needed based on selected playlists.

Note - Works with Apple Music only when you use it to manage your own music files, not the streaming service.

## Get it

[Latest release →](../../releases)

### macOS: first launch

The Mac app isn't notarized by Apple, so macOS blocks it the first time you open it.

1. Drag the app to Applications and open it. You'll see a warning that Apple could not verify it. Click Done.
2. Open System Settings → Privacy & Security, scroll down to Security, and click **Open Anyway** next to Viamta Music Sync.
3. Confirm with your password or Touch ID. You only need to do this once per download.

If you instead see "Viamta Music Sync is damaged and can't be opened", don't move it to the Trash. Run this in Terminal, then open the app again:

```
xattr -cr "/Applications/Viamta Music Sync.app"
```

## Instructions

1. Get a Library XML file from iTunes/Apple Music. Either:
   - **Auto-updating (recommended):** turn on library XML sharing, and iTunes/Apple Music keeps the file up to date for you. See [Auto-updating library XML](#auto-updating-library-xml) below.
   - **Manual export:** File → Library → Export Library… You have to re-export every time your playlists change.
2. Run the desktop app and browse for the XML file. The app remembers it, and reloads automatically whenever the file changes.
3. Install and run the android app. Choose music directory.
4. Be on the same network as the desktop app and allow access on the app
   once the desktop connects. If it doesn't auto-detect, you can try a
   manual connect.
5. Choose playlists you want copied to the android device by clicking on
   a playlist row in the desktop app.
6. Hit Sync Now

### Auto-updating library XML

iTunes and Apple Music can keep a copy of your library as XML that updates whenever your library or playlists change. Point the desktop app at it once and you never need to export again. The app only reads this file and never changes your library.

**Apple Music (Mac):**

1. Open Music → Settings → **Files** (on older macOS versions it's under **Advanced**).
2. Tick **Share Library XML with other apps**.
3. In the desktop app, browse to `Library.xml` in the folder that holds your Apple Music library, usually `Music/Music` in your home folder.

**iTunes (Windows, or older Macs):**

1. Open Edit → Preferences → **Advanced** (on a Mac: iTunes → Preferences → **Advanced**).
2. Tick **Share iTunes Library XML with other applications**.
3. In the desktop app, browse to `iTunes Library.xml` in your iTunes folder, usually `Music\iTunes` in your user folder.

## Screenshots

<img width="60%" alt="image" src="https://github.com/user-attachments/assets/09cac717-974f-4d54-b0e2-41e0a22dd0cb" />
<br/>
<img width="60%" alt="image" src="https://github.com/user-attachments/assets/730026c7-8456-43be-b357-05826a732ba2" />
<br/><br/>
<img width="33%"  alt="Screenshot_20260523_000422_Viamta Music Sync" src="https://github.com/user-attachments/assets/f89f0765-ac24-4365-867c-93340b62b00d" />
<br/><br/>
<img width="33%"  alt="Screenshot_20260522_234634_Viamta Music Sync" src="https://github.com/user-attachments/assets/ad7f7a74-e9da-4761-bbb4-dad06a08b4f5" />

