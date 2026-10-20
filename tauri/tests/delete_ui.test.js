// Unit tests for the delete wording in src/delete_ui.js.

import { test } from "node:test";
import assert from "node:assert/strict";

import { cleanupLabel, everythingElseLabel, confirmDeleteLines } from "../src/delete_ui.js";

test("cleanupLabel covers tracks, playlist file, both and neither", () => {
  assert.equal(cleanupLabel({ tracks: 3, playlistFile: true }), "Delete playlist + 3 tracks");
  assert.equal(cleanupLabel({ tracks: 1, playlistFile: false }), "Delete 1 track");
  assert.equal(cleanupLabel({ tracks: 0, playlistFile: true }), "Delete playlist");
  assert.equal(cleanupLabel({ tracks: 0, playlistFile: false }), "");
  assert.equal(cleanupLabel({ tracks: null, playlistFile: false }), "");
});

test("everythingElseLabel pluralises and handles nothing to delete", () => {
  assert.equal(everythingElseLabel({ tracks: 12, playlists: 1 }), "Delete 12 tracks + 1 playlist");
  assert.equal(everythingElseLabel({ tracks: 1, playlists: 0 }), "Delete 1 track");
  assert.equal(everythingElseLabel({ tracks: 0, playlists: 2 }), "Delete 2 playlists");
  assert.equal(everythingElseLabel({ tracks: 0, playlists: 0 }), "Nothing to delete");
});

test("confirmDeleteLines lists each playlist and one track total", () => {
  const lines = confirmDeleteLines({
    remove_playlist_items: [{ name: "Old", filename: "Old.m3u" }, { name: "Gone", filename: "Gone.m3u" }],
    delete_songs: 40,
  });
  assert.deepEqual(lines, [
    "Playlist: Old",
    "Playlist: Gone",
    "40 tracks (see Details… for the full list)",
  ]);
});

test("confirmDeleteLines is empty when nothing is deleted or there is no preview", () => {
  assert.deepEqual(confirmDeleteLines({ remove_playlist_items: [], delete_songs: 0 }), []);
  assert.deepEqual(confirmDeleteLines(null), []);
  assert.deepEqual(confirmDeleteLines(undefined), []);
});
