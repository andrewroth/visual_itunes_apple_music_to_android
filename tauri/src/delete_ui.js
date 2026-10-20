// Wording for the delete controls and the pre-sync confirmation, split
// out of main.js so it can be unit-tested under Node (see
// tauri/tests/delete_ui.test.js). No DOM or Tauri imports here.

function plural(n, word) {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

// Label for an unchecked playlist's Delete box. `tracks` is how many of
// its files on the phone no synced playlist wants; `playlistFile` is
// whether its .m3u is on the phone. Returns "" when there's nothing to
// delete, i.e. no box should be shown.
export function cleanupLabel({ tracks, playlistFile }) {
  const n = tracks || 0;
  if (n > 0 && playlistFile) return `Delete playlist + ${plural(n, "track")}`;
  if (n > 0) return `Delete ${plural(n, "track")}`;
  if (playlistFile) return "Delete playlist";
  return "";
}

// Label for the "everything not in a synced playlist" box.
export function everythingElseLabel({ tracks, playlists }) {
  const parts = [];
  if (tracks > 0) parts.push(plural(tracks, "track"));
  if (playlists > 0) parts.push(plural(playlists, "playlist"));
  return parts.length ? `Delete ${parts.join(" + ")}` : "Nothing to delete";
}

// Lines for the "Confirm deletions" dialog, taken from the backend's
// sync preview so they are exactly what the sync will delete. Empty
// when the sync deletes nothing (no dialog needed).
export function confirmDeleteLines(preview) {
  if (!preview) return [];
  const lines = [];
  for (const p of preview.remove_playlist_items || []) {
    lines.push(`Playlist: ${p.name}`);
  }
  const songs = preview.delete_songs || 0;
  if (songs > 0) {
    lines.push(`${plural(songs, "track")} (see Details… for the full list)`);
  }
  return lines;
}
