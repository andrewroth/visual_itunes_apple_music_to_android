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

1. Export your iTunes/Apple Music library. Library -> Export Library...
       (unfortunately you do have to do this every time playlists are changed in iTunes/Apple Music)
3. Run the desktop app, browse for your saved export
       (if you keep using the same export save location, this step can be skipped)
5. Install and run the android app. Choose music directory.
6. Be on the same network as the desktop app and allow access on the app
   once the desktop connects. If it doesn't auto-detect, you can try a
   manual connect.
7. Choose playlists you want copied to the android device by clicking on
   a playlist row in the desktop app.
8. Hit Sync Now

## Screenshots

<img width="60%" alt="image" src="https://github.com/user-attachments/assets/09cac717-974f-4d54-b0e2-41e0a22dd0cb" />
<br/>
<img width="60%" alt="image" src="https://github.com/user-attachments/assets/730026c7-8456-43be-b357-05826a732ba2" />
<br/><br/>
<img width="33%"  alt="Screenshot_20260523_000422_Viamta Music Sync" src="https://github.com/user-attachments/assets/f89f0765-ac24-4365-867c-93340b62b00d" />
<br/><br/>
<img width="33%"  alt="Screenshot_20260522_234634_Viamta Music Sync" src="https://github.com/user-attachments/assets/ad7f7a74-e9da-4761-bbb4-dad06a08b4f5" />

