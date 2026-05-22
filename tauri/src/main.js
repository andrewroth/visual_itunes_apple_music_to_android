// Frontend for the MusicSync Tauri app.
//
// Uses Tauri's invoke/listen API for *all* backend communication — no
// polling, no XHR/fetch. Each user gesture maps to an invoke; the backend
// emits "progress" events that this script listens for and renders.

// Build-version tag. Bump when debugging so you can tell at a glance which
// version is actually running (caches, stale loads, etc.).
const MUSICSYNC_BUILD = "0.2.0-pre-2026-05-22";
console.log("MusicSync frontend loaded:", MUSICSYNC_BUILD);

import { invoke } from "https://cdn.jsdelivr.net/npm/@tauri-apps/api@2/core/+esm";
import { listen } from "https://cdn.jsdelivr.net/npm/@tauri-apps/api@2/event/+esm";

// Native file-picker dialog via the Tauri dialog plugin's own command.
// No external JS module needed — we just call the Rust command directly.
async function openDialog(options) {
  return await invoke("plugin:dialog|open", { options });
}

const $ = (id) => document.getElementById(id);

const SETTINGS_FIELDS = ["library_path", "ws_url", "ftp_path"];

// Last-loaded settings, kept around so other components (e.g. the sync
// confirm modal) can consult delete_unused_songs without an extra round-trip.
let lastSettings = {};

async function loadSettings() {
  const settings = await invoke("load_settings");
  lastSettings = settings;
  // Defensive lookups — a missing element from an HTML edit shouldn't
  // abort the entire handler and break unrelated buttons (Browse, etc.).
  const setVal = (id, v) => { const el = $(id); if (el) el.value = v; };
  const setChecked = (id, v) => { const el = $(id); if (el) el.checked = v; };
  setVal("library_path", settings.library_path || "");
  setVal("ftp_path", settings.ftp_path || "");
  setVal("ws_url", settings.ws_url || $("ws_url")?.value || "");
  // delete_unused_songs is rendered via the orphan row in the playlist
  // table — see renderPlaylists.
  renderPairedBanner(settings);
  renderForgetPairingBtn();
  if ($("ws_url_display")?.style.display !== "none" && !window._wsUrl) {
    showSearchingState();
  }
  // If the user hasn't picked a library yet, show the "Export Library"
  // instructions in the status banner so the first-launch path is obvious.
  if (!settings.library_path) {
    renderLibraryBanner(null);
  }
}

async function saveSettings() {
  const current = await invoke("load_settings");
  current.library_path = $("library_path").value || null;
  current.ftp_path = $("ftp_path").value || null;
  window._wsUrl = $("ws_url").value || "";
  await invoke("save_settings", { new: current });
}

// Persistent banner was duplicative with the address row's green ●
// indicator + the Approvals dialog. Always hide it.
function renderPairedBanner(_settings) {
  const banner = $("paired_banner");
  if (banner) banner.style.display = "none";
}

function formatTime(ms) {
  return new Date(ms).toLocaleTimeString();
}

function verboseLog(line) {
  if (!lastSettings.verbose_logging) return;
  appendLog(line);
}

// Scan lifecycle messages — route through the status line, no separate
// banner above the playlist table.
listen("scan_started", () => {
  setStatusMessage("info", "Scanning...");
});
// Mirror of the phone's own scan progress bar, pushed over the WS as
// PROGRESS frames while ManifestBuilder walks the music folder. Drives
// the status line + bottom progress bar so the desktop reflects the
// same percentage the phone shows.
listen("scan_progress", (e) => {
  const { message, fraction } = e.payload;
  verboseLog(
    `scan_progress event received: message=${JSON.stringify(message)} ` +
    `fraction=${typeof fraction === "number" ? fraction : "null"}`
  );
  setStatusMessage("info", message);
  if (typeof fraction === "number") {
    const pct = Math.round(fraction * 100);
    verboseLog(`scan_progress updating bar width to ${pct}%`);
    $("progress").style.width = `${pct}%`;
    updateEta(fraction);
  } else {
    verboseLog("scan_progress had no numeric fraction; leaving bar width unchanged");
  }
});
listen("scan_complete", (e) => {
  const { files, playlists } = e.payload;
  setStatusMessage(
    "success",
    `Scan complete, ready to copy. (${files} tracks, ${playlists} playlists)`,
  );
  // Re-anchor the ETA so the copy phase isn't extrapolated from the
  // (much faster) scan rate.
  resetEta();
});

// Update the right-hand status banner with a typed message. Use this
// helper for *all* user-visible status text so messages always land in
// the same blue/yellow/red strip next to the Sync button rather than
// somewhere offscreen. Reverts back to the library summary on Idle.
function setStatusMessage(level, text) {
  const s = $("status");
  // Reset the Bootstrap colour class.
  s.className = "alert py-1 px-2 mb-0 flex-grow-1 ms-1";
  const cls = ({
    info: "alert-info",
    warning: "alert-warning",
    error: "alert-danger",
    success: "alert-success",
  })[level] || "alert-info";
  s.classList.add(cls);
  s.textContent = text;
}

// The #status div serves as both the library summary (blue alert when
// idle) and the live status line (overwritten during transient events).
function renderLibraryBanner(library) {
  const s = $("status");
  if (!library) {
    // No library yet — show a brief "how do I get the Library.xml" hint
    // so a first-time user knows what to do with the Browse button.
    if (!$("library_path").value) {
      s.innerHTML =
        `<strong>No library selected.</strong> ` +
        `In the Music app (formerly iTunes), choose <em>File → Library → Export Library…</em> ` +
        `and save the resulting XML somewhere. Then click <strong>Browse…</strong> above.`;
    } else {
      s.textContent = "Library not loaded yet.";
    }
    return;
  }
  const exportedPart = library.library_mtime_ms
    ? ` - Library XML was exported ${escapeHtml(new Date(library.library_mtime_ms).toLocaleString())}`
    : "";
  s.innerHTML =
    `${library.track_count} tracks, ${library.playlists.length} playlists${exportedPart}`;
}

// Remember the most recently loaded library so transient sync messages can
// be replaced with the summary again afterwards.
let lastLibraryView = null;

function renderMissingPlaylists(library) {
  const section = $("missing_playlists_section");
  const body = $("missing_playlists_body");
  const missing = (library && library.missing_playlists) || [];
  if (missing.length === 0) {
    section.style.display = "none";
    body.innerHTML = "";
    return;
  }
  section.style.display = "";
  body.innerHTML = "";
  for (const m of missing) {
    const tr = document.createElement("tr");
    tr.innerHTML =
      `<td>${escapeHtml(m.name)}</td>` +
      `<td>` +
      `<select class="form-select form-select-sm action-select" data-id="${escapeHtml(m.id)}">` +
      `<option value="ignore"${m.action === "ignore" ? " selected" : ""}>Ignore</option>` +
      `<option value="delete"${m.action === "delete" ? " selected" : ""}>Delete from phone on next sync</option>` +
      `</select>` +
      `</td>` +
      `<td><button class="btn btn-sm btn-outline-danger forget-btn" data-id="${escapeHtml(m.id)}" title="Forget removes this playlist from the remembered list. If a playlist with the same ID reappears in your Library.xml later, it will NOT auto-check.">Forget</button></td>`;
    body.appendChild(tr);
  }
  // Wire up handlers for the freshly-rendered rows.
  for (const sel of body.querySelectorAll(".action-select")) {
    sel.addEventListener("change", async (e) => {
      const updated = await invoke("set_playlist_action", {
        playlistId: e.target.dataset.id,
        action: e.target.value,
      });
      renderPlaylists(updated);
      showSyncPreview();
    });
  }
  for (const btn of body.querySelectorAll(".forget-btn")) {
    btn.addEventListener("click", async () => {
      const id = btn.dataset.id;
      const row = btn.closest("tr");
      const name = row?.querySelector("td:first-child")?.textContent || id;
      const ok = window.confirm(
        `Forget "${name}"?\n\n` +
        `This removes the playlist from Viamta Music Sync's remembered list. ` +
        `If a playlist with the same ID later reappears in your Library.xml, ` +
        `it will NOT be automatically re-checked.\n\n` +
        `This does not touch the phone or any music files.`
      );
      if (!ok) return;
      const updated = await invoke("forget_playlist", { playlistId: id });
      renderPlaylists(updated);
      showSyncPreview();
    });
  }
}

function renderPlaylists(library) {
  lastLibraryView = library;
  renderLibraryBanner(library);
  renderMissingPlaylists(library);
  const tbody = $("playlists").querySelector("tbody");
  tbody.innerHTML = "";
  for (const p of library.playlists) {
    const tr = document.createElement("tr");
    tr.setAttribute("data-id", p.playlist_id);
    if (p.checked) tr.classList.add("table-secondary");
    const deviceCount =
      typeof p.device_tracks_count === "number" ? p.device_tracks_count : "";
    let toCopy = "";
    if (typeof p.tracks_to_copy === "number") {
      toCopy =
        p.tracks_to_copy === 0
          ? `<span class="text-success">0</span>`
          : `${p.tracks_to_copy}`;
    }
    // Per-row Delete column. Only meaningful for UNCHECKED playlists
    // that have device tracks no longer covered by any checked playlist
    // (cleanup_count > 0). For checked rows, blank.
    let deleteCell = "";
    if (!p.checked && typeof p.cleanup_count === "number" && p.cleanup_count > 0) {
      const checked = p.cleanup_checked ? "checked" : "";
      deleteCell =
        `<label class="d-inline-flex align-items-center gap-1 m-0" ` +
        `title="Delete tracks already on the phone that aren't in any playlist being synced.">` +
        `<input type="checkbox" class="form-check-input cleanup-checkbox m-0" ` +
        `data-playlist-id="${escapeHtml(p.playlist_id)}" ${checked}>` +
        `<span>Delete ${p.cleanup_count} Track${p.cleanup_count === 1 ? "" : "s"}</span>` +
        `</label>`;
    }
    tr.innerHTML =
      `<td>${p.checked ? "✓" : ""}</td>` +
      `<td>${escapeHtml(p.name)}</td>` +
      `<td>${p.track_count}</td>` +
      `<td>${deviceCount}</td>` +
      `<td>${deleteCell}</td>` +
      `<td>${toCopy}</td>`;
    // Row click toggles the playlist's Copy state, BUT not when the
    // click came from inside the Delete checkbox (it owns its own click).
    tr.addEventListener("click", async (e) => {
      if (e.target.closest(".cleanup-checkbox, label")) return;
      const updated = await invoke("toggle_playlist", {
        playlistId: p.playlist_id,
      });
      renderPlaylists(updated);
      showSyncPreview();
    });
    tbody.appendChild(tr);
  }

  // Orphan row: files on the phone whose size doesn't match any track
  // in the Music Library (iTunes/Apple Music). Single italic label
  // spanning Copy+Name+#Tracks, then a "Delete N Tracks" checkbox in
  // the Device # Tracks column. Shown only when there's actually
  // something to delete.
  if (typeof library.orphan_count === "number" && library.orphan_count > 0) {
    const tr = document.createElement("tr");
    tr.classList.add("table-warning");
    const checked = lastSettings.delete_unused_songs ? "checked" : "";
    tr.innerHTML =
      `<td colspan="3"><i>Tracks not in your Music Library</i></td>` +
      `<td>` +
      `<label class="d-inline-flex align-items-center gap-1 m-0" ` +
      `title="Tracks on the phone whose size doesn't match anything in your Music Library. ` +
      `Usually leftovers from a previous sync after the library shrunk, or manually-added files.">` +
      `<input type="checkbox" class="form-check-input m-0" id="orphan_delete_checkbox" ${checked}>` +
      `<span>Delete ${library.orphan_count} Track${library.orphan_count === 1 ? "" : "s"}</span>` +
      `</label>` +
      `</td>` +
      `<td></td>` +
      `<td></td>`;
    tbody.appendChild(tr);
  }

  // Attach handlers AFTER the tbody is populated so query finds nodes.
  for (const cb of tbody.querySelectorAll(".cleanup-checkbox")) {
    cb.addEventListener("click", async (e) => {
      e.stopPropagation();
      const id = cb.getAttribute("data-playlist-id");
      const updated = await invoke("toggle_cleanup_playlist", { playlistId: id });
      renderPlaylists(updated);
      showSyncPreview();
    });
  }
  const orphanCb = $("orphan_delete_checkbox");
  if (orphanCb) {
    orphanCb.addEventListener("click", async (e) => {
      e.stopPropagation();
      const updated = await invoke("set_delete_unused_songs", {
        value: orphanCb.checked,
      });
      lastSettings.delete_unused_songs = orphanCb.checked;
      renderPlaylists(updated);
      showSyncPreview();
    });
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[c]);
}

function appendLog(line) {
  const div = document.createElement("div");
  div.textContent = `[${new Date().toLocaleTimeString()}] ${line}`;
  $("log").appendChild(div);
  $("log").scrollTop = $("log").scrollHeight;
}

async function loadLibrary() {
  $("status").textContent = "Loading library…";
  try {
    const view = await invoke("load_library");
    renderPlaylists(view);
    const exportStr = view.library_mtime_ms
      ? new Date(view.library_mtime_ms).toLocaleString()
      : "unknown time";
    appendLog(
      `Library updated — ${view.track_count} tracks, ` +
      `${view.playlists.length} playlists, exported ${exportStr}`
    );
  } catch (e) {
    $("status").textContent = `Error: ${e}`;
    appendLog(`Error loading library: ${e}`);
  }
}

// List of planned deletes — built from missing playlists with action=delete
// plus (if checkbox) unused songs. Used for the confirm modal.
function plannedDeletes() {
  const out = { playlists: [], songs: [] };
  const missing = (lastLibraryView && lastLibraryView.missing_playlists) || [];
  for (const m of missing) {
    if (m.action === "delete") out.playlists.push(m.name);
  }
  if (
    lastSettings.delete_unused_songs &&
    lastLibraryView &&
    typeof lastLibraryView.orphan_count === "number" &&
    lastLibraryView.orphan_count > 0
  ) {
    out.songs.push(
      `${lastLibraryView.orphan_count} track${
        lastLibraryView.orphan_count === 1 ? "" : "s"
      } not in your Music Library`,
    );
  }
  // Per-playlist cleanups, summed per playlist for the confirm modal.
  const pls = (lastLibraryView && lastLibraryView.playlists) || [];
  for (const p of pls) {
    if (p.cleanup_checked && p.cleanup_count > 0) {
      out.songs.push(
        `${p.cleanup_count} track${p.cleanup_count === 1 ? "" : "s"} from "${p.name}"`,
      );
    }
  }
  return out;
}

let confirmSyncModalInstance = null;
function showConfirmSyncModal(planned) {
  if (!confirmSyncModalInstance) {
    confirmSyncModalInstance = new bootstrap.Modal($("confirm_sync_modal"));
  }
  const list = $("confirm_sync_list");
  list.innerHTML = "";
  for (const p of planned.playlists) {
    const li = document.createElement("li");
    li.textContent = `Playlist file: ${p}.m3u`;
    list.appendChild(li);
  }
  for (const s of planned.songs) {
    const li = document.createElement("li");
    li.textContent = s;
    list.appendChild(li);
  }
  return new Promise((resolve) => {
    const onConfirm = () => { cleanup(); resolve(true); };
    const onCancel = () => { cleanup(); resolve(false); };
    const cleanup = () => {
      $("confirm_sync_confirm").removeEventListener("click", onConfirm);
      $("confirm_sync_cancel").removeEventListener("click", onCancel);
      confirmSyncModalInstance.hide();
    };
    $("confirm_sync_confirm").addEventListener("click", onConfirm);
    $("confirm_sync_cancel").addEventListener("click", onCancel);
    confirmSyncModalInstance.show();
  });
}

async function runSync() {
  const wsUrl = $("ws_url").value || window._wsUrl;
  if (!wsUrl) {
    setStatusMessage("warning", "Enter the phone address first");
    return;
  }
  $("sync").setAttribute("disabled", "true");
  $("progress").style.width = "0%";
  setStatusMessage("info", "Loading library...");
  try {
    await loadLibrary();

    const planned = plannedDeletes();
    if (planned.playlists.length > 0 || planned.songs.length > 0) {
      const ok = await showConfirmSyncModal(planned);
      if (!ok) {
        setStatusMessage("warning", "Sync cancelled.");
        return;
      }
    }

    setStatusMessage("info", "Starting sync...");
    const result = await invoke("run_sync", { wsUrl });
    appendLog(result);
    setStatusMessage("success", result);
    // Refresh the scan so the "unused" count reflects the freshly-cleaned
    // state. Errors are silent — sync already succeeded.
    // Post-sync: rescan the phone so per-playlist counts reflect the new
    // Post-sync refresh.
    setStatusMessage("info", "Refreshing post-sync state...");
    try { await scanDevice(); } catch (e) { appendLog(`post-sync scan failed: ${e}`); }
    try { await loadLibrary(); } catch (e) { appendLog(`post-sync library reload failed: ${e}`); }
    renderLibraryBanner(lastLibraryView);
    // Briefly show the sync result, then fall back to the preview of
    // whatever's left to do (likely "nothing — everything is up to date.").
    setStatusMessage("success", result);
    setTimeout(() => showSyncPreview(), 4000);
  } catch (e) {
    setStatusMessage("error", `Sync error: ${e}`);
    appendLog(`Sync error: ${e}`);
  } finally {
    // Sync just exercised a live connection, so it's safe to re-enable.
    // (If the connection went bad during sync, the next scanDevice() will
    // disable it again.)
    setSyncEnabled(true);
  }
}

// Remembered scan result so Sync can summarise planned deletes without
// re-asking the backend.
let lastScan = null;

function renderCleanupSection() {
  // The old standalone cleanup section is gone; orphan + per-playlist
  // delete UI is rendered inline in the playlist table now. Kept as a
  // no-op so existing call sites don't crash.
}

// Posted into the status banner whenever the user has a complete picture
// (manifest in hand) so they always see "what will happen if I tap Sync
// right now." Updates on scan completion and any user toggle that
// affects the counts. Returns null if we don't have enough data yet.
function syncPreviewText() {
  const view = lastLibraryView;
  if (!view || !view.preview) return null;
  const { new_playlists, new_songs, remove_playlists, delete_songs } = view.preview;
  if (
    new_playlists === 0 &&
    new_songs === 0 &&
    remove_playlists === 0 &&
    delete_songs === 0
  ) {
    return "Sync would do nothing — everything is up to date.";
  }
  const pl = (n, s) => `${n} ${s}${n === 1 ? "" : "s"}`;
  return (
    "Syncing will add " + pl(new_playlists, "new playlist") +
    ", " + pl(new_songs, "new song") +
    ", remove " + pl(remove_playlists, "playlist") +
    ", and delete " + pl(delete_songs, "song") +
    "."
  );
}

function showSyncPreview() {
  const text = syncPreviewText();
  if (!text) return;
  const s = $("status");
  const view = lastLibraryView;
  const missing =
    view && view.preview && view.preview.missing_files
      ? view.preview.missing_files.length
      : 0;
  const cls = missing > 0 ? "alert-danger" : "alert-info";
  s.className = `alert ${cls} py-1 px-2 mb-0 flex-grow-1 ms-1`;
  let html = escapeHtml(text);
  if (missing > 0) {
    html +=
      ` <strong>⚠ ${missing} file${missing === 1 ? "" : "s"} missing on disk!</strong>`;
  }
  html += ` <a href="#" id="status_details_link" class="ms-1">Details…</a>`;
  s.innerHTML = html;
  const link = $("status_details_link");
  if (link) {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      openSyncDetails();
    });
  }
}

let syncDetailsModal = null;
function openSyncDetails() {
  const view = lastLibraryView;
  if (!view || !view.preview) return;
  const tbody = $("sync_details_body");
  tbody.innerHTML = "";

  // Running row counter — restarts at 1 for each operation group so the
  // user can read counts off directly per section.
  // Two trailing icon columns: 💻 with the desktop path, 📱 with the
  // phone path. Each cell is empty if that side has no path for this
  // operation kind (e.g. orphan deletes have no desktop side).
  function iconCell(emoji, path) {
    if (!path) return `<td></td>`;
    return `<td title="${escapeHtml(path)}">${emoji}</td>`;
  }
  function songRows(opLabel, items, rowClass, kind /* "add" | "delete" */) {
    items.forEach((s, i) => {
      const tr = document.createElement("tr");
      tr.className = rowClass;
      // For "add" rows: local path is in s.path, device path in s.device_path.
      // For "delete" rows: s.path IS the device path; no desktop side.
      const localPath = kind === "add" ? (s.path || "") : "";
      const devicePath = kind === "add" ? (s.device_path || "") : (s.path || "");
      tr.innerHTML =
        `<td class="text-end text-muted">${i + 1}</td>` +
        `<td style="white-space: nowrap">${escapeHtml(opLabel)}</td>` +
        `<td>${escapeHtml(s.artist || "")}</td>` +
        `<td>${escapeHtml(s.name || "")}</td>` +
        iconCell("💻", localPath) +
        iconCell("📱", devicePath);
      tbody.appendChild(tr);
    });
  }
  function playlistRows(opLabel, items, rowClass) {
    // .m3u filenames live on the phone — show in the 📱 column only.
    items.forEach((p, i) => {
      const tr = document.createElement("tr");
      tr.className = rowClass;
      tr.innerHTML =
        `<td class="text-end text-muted">${i + 1}</td>` +
        `<td style="white-space: nowrap">${escapeHtml(opLabel)}</td>` +
        `<td></td>` +
        `<td>${escapeHtml(p.name || "")}</td>` +
        `<td></td>` +
        iconCell("📱", p.filename || "");
      tbody.appendChild(tr);
    });
  }

  const p = view.preview;
  if (p.missing_files && p.missing_files.length > 0) {
    p.missing_files.forEach((s, i) => {
      const tr = document.createElement("tr");
      tr.className = "table-danger";
      tr.innerHTML =
        `<td class="text-end text-muted">${i + 1}</td>` +
        `<td style="white-space: nowrap"><strong>⚠ Missing file</strong></td>` +
        `<td>${escapeHtml(s.artist || "")}</td>` +
        `<td>${escapeHtml(s.name || "")}</td>` +
        iconCell("💻", s.path || "") +
        iconCell("📱", s.device_path || "");
      tbody.appendChild(tr);
    });
  }
  songRows("Add song", p.new_song_items || [], "table-success", "add");
  playlistRows("Add playlist", p.new_playlist_items || [], "table-info");
  songRows("Delete song", p.delete_song_items || [], "table-danger", "delete");
  playlistRows("Remove playlist", p.remove_playlist_items || [], "table-warning");

  if (tbody.children.length === 0) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td colspan="6" class="text-muted">Nothing to sync — everything is up to date.</td>`;
    tbody.appendChild(tr);
  }

  if (!syncDetailsModal) {
    syncDetailsModal = new bootstrap.Modal($("sync_details_modal"));
  }
  syncDetailsModal.show();
}

async function scanDevice() {
  const wsUrl = $("ws_url").value || window._wsUrl;
  if (!wsUrl) return;
  if (!lastSettings.device_token) return; // not paired yet
  try {
    setStatusMessage("info", "Scanning...");
    appendLog("Scanning phone for current file list…");
    resetEta();
    const result = await invoke("scan_device", { wsUrl });
    lastScan = result;
    appendLog(
      `Scan complete: ${result.files} files on phone, ${result.unused} unused`
    );
    renderCleanupSection();
    setSyncEnabled(true);
    // Refresh the library view (so orphan_count + cleanup_count come
    // through) and then post the preview line into the status banner.
    try { await loadLibrary(); } catch (_) { /* logged elsewhere */ }
    showSyncPreview();
  } catch (e) {
    setStatusMessage("error", `Scan failed: ${e}`);
    appendLog(`Scan failed: ${e}`);
    setSyncEnabled(false);
  }
}

// Sync lifecycle: backend emits sync_started/sync_ended around each
// run_sync. The Stop button is only useful (and only visible) in between.
// Reset the "Aborting…" pending state on every fresh sync so it doesn't
// stick around if the user starts a new sync after a prior abort.
listen("sync_started", () => {
  $("sync").style.display = "none";
  const stop = $("stop_sync");
  stop.style.display = "";
  stop.removeAttribute("disabled");
  stop.textContent = "Stop sync";
  resetEta();
});
listen("sync_ended", () => {
  const stop = $("stop_sync");
  stop.style.display = "none";
  stop.removeAttribute("disabled");
  stop.textContent = "Stop sync";
  $("sync").style.display = "";
  hideEta();
});

// ----- ETA -----
// Timestamp (ms) of the first non-zero progress fraction we see in the
// current sync. Used as the anchor for the linear extrapolation in
// updateEta(). Reset on sync_started so a new run computes fresh.
let etaStartMs = null;
let etaStartFraction = null;

function resetEta() {
  etaStartMs = null;
  etaStartFraction = null;
  const el = $("progress_eta");
  if (el) {
    el.textContent = "";
    el.style.display = "none";
  }
}

function hideEta() {
  const el = $("progress_eta");
  if (el) el.style.display = "none";
}

// Format milliseconds as HH:MM, capped at 99:59 for sanity. Returns
// "<1m" for anything under one minute so the chip doesn't say 00:00.
function formatHhMm(ms) {
  if (!isFinite(ms) || ms < 0) return "";
  const totalMinutes = Math.round(ms / 60000);
  if (totalMinutes < 1) return "<1m";
  const hh = Math.min(99, Math.floor(totalMinutes / 60));
  const mm = totalMinutes % 60;
  return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}`;
}

// Linear extrapolation: assume progress rate from the first non-zero
// fraction we saw is representative. Cheap and reasonable for the
// dominant cost (file uploads on a steady WiFi link).
function updateEta(fraction) {
  const el = $("progress_eta");
  if (!el) return;
  if (typeof fraction !== "number" || fraction <= 0 || fraction >= 1) {
    el.style.display = "none";
    return;
  }
  const now = Date.now();
  if (etaStartMs == null) {
    etaStartMs = now;
    etaStartFraction = fraction;
    return; // need at least one delta to extrapolate
  }
  const elapsed = now - etaStartMs;
  const progressed = fraction - etaStartFraction;
  if (progressed <= 0 || elapsed <= 0) return;
  const remainingFraction = 1 - fraction;
  const eta = elapsed * (remainingFraction / progressed);
  const label = formatHhMm(eta);
  if (!label) {
    el.style.display = "none";
    return;
  }
  el.textContent = `~${label} left`;
  el.style.display = "";
}

async function stopSync() {
  const ok = window.confirm(
    "Stop sync now?\n\n" +
    "Files already copied stay on the phone. The next sync will check " +
    "what's still missing and pick up where this one left off."
  );
  if (!ok) return;
  // Immediate visual feedback. The backend's abort_flag is only checked
  // between file boundaries, so there's an inherent delay before
  // sync_ended fires — we don't want the user clicking Stop repeatedly
  // wondering if it registered.
  const stop = $("stop_sync");
  stop.setAttribute("disabled", "true");
  stop.textContent = "Aborting…";
  try {
    await invoke("abort_sync");
    appendLog("Stop requested — current sync will exit at the next file boundary.");
  } catch (e) {
    appendLog(`Abort failed: ${e}`);
    // Roll back the visual state so the user can retry.
    stop.removeAttribute("disabled");
    stop.textContent = "Stop sync";
  }
}

// Backend-pushed progress events. This is the "Tauri way" — the backend
// emits, the frontend listens, no polling.
listen("progress", (e) => {
  const { message, fraction } = e.payload;
  verboseLog(
    `progress event received: message=${JSON.stringify(message)} ` +
    `fraction=${typeof fraction === "number" ? fraction : "null"}`
  );
  // Pick a colour from the message content. Most progress is info; the
  // explicit successes / errors / aborts get matching styles.
  let level = "info";
  if (/error|fail|abort/i.test(message)) level = "error";
  else if (/complete|done/i.test(message)) level = "success";
  setStatusMessage(level, message);
  if (typeof fraction === "number") {
    const pct = Math.round(fraction * 100);
    verboseLog(`progress updating bar width to ${pct}%`);
    $("progress").style.width = `${pct}%`;
    updateEta(fraction);
  } else {
    verboseLog("progress had no numeric fraction; leaving bar width unchanged");
  }
  appendLog(message);
});

// ----- Pairing flow -----

let pairModalInstance = null;

function openPairModal(code, deviceName) {
  $("pair_code").textContent = code;
  $("pair_device_name").textContent = deviceName || "your phone";
  if (!pairModalInstance) {
    pairModalInstance = new bootstrap.Modal($("pair_modal"));
  }
  pairModalInstance.show();
}

function closePairModal() {
  if (pairModalInstance) pairModalInstance.hide();
}

// Backend emits this exactly once during a pair session, once the phone
// has acknowledged the request. That's the cue to display the code.
listen("pair_challenge", (e) => {
  const { code, device_name } = e.payload;
  appendLog(`Pair challenge: ${code} from ${device_name}`);
  window._currentPairDeviceName = device_name;
  openPairModal(code, device_name);
});

async function startPair() {
  const wsUrl = $("ws_url").value || window._wsUrl;
  if (!wsUrl) {
    setStatusMessage("warning", "Enter the phone address first");
    return;
  }
  setStatusMessage("info", `Contacting ${wsUrl}…`);
  appendLog(`Attempting pair with ${wsUrl}…`);
  try {
    const result = await invoke("start_pairing", { wsUrl });
    closePairModal();
    setStatusMessage("success", `Paired with ${result.device_name}`);
    appendLog(`Paired with ${result.device_name} (root ${result.music_root})`);
    await loadSettings();
    scanDevice();
  } catch (e) {
    closePairModal();
    setStatusMessage("error", `Pair failed: ${e}`);
    appendLog(`Pair failed: ${e}`);
  }
}

async function confirmPair() {
  setStatusMessage("info", "Waiting for phone confirmation…");
  try {
    await invoke("pair_confirm");
  } catch (e) {
    setStatusMessage("warning", `Confirm error: ${e}`);
  }
}

async function cancelPair() {
  try {
    await invoke("pair_cancel");
  } catch (_) {
    /* no pending pair is fine */
  }
  closePairModal();
  setStatusMessage("info", "Pair cancelled.");
}


let managePairingsModal = null;

function openManagePairings() {
  // Desktop is still single-paired for now, so the list shows one row
  // when paired, empty otherwise. Future multi-pair refactor would loop
  // over stored pairings here.
  const ul = $("manage_pairings_list");
  ul.innerHTML = "";
  if (lastSettings.device_token && lastSettings.paired_device_name) {
    const li = document.createElement("li");
    li.className = "d-flex align-items-center py-2";
    li.innerHTML =
      `<div class="flex-grow-1">` +
      `<strong>${escapeHtml(lastSettings.paired_device_name)}</strong>` +
      (lastSettings.ftp_path
        ? ` <span class="text-muted small">music root ${escapeHtml(lastSettings.ftp_path)}</span>`
        : "") +
      `</div>`;
    const x = document.createElement("button");
    x.className = "btn btn-sm btn-outline-danger";
    x.textContent = "✕";
    x.title = "Forget this pairing";
    x.addEventListener("click", async () => {
      if (!window.confirm(
        `Forget pairing with ${lastSettings.paired_device_name}?\n\n` +
        "You'll need to pair again next time you want to sync to this phone."
      )) return;
      await forgetPairing();
      if (managePairingsModal) managePairingsModal.hide();
    });
    li.appendChild(x);
    ul.appendChild(li);
  } else {
    const li = document.createElement("li");
    li.className = "text-muted";
    li.textContent = "No phones paired.";
    ul.appendChild(li);
  }
  if (!managePairingsModal) {
    managePairingsModal = new bootstrap.Modal($("manage_pairings_modal"));
  }
  managePairingsModal.show();
}

async function forgetPairing() {
  // Confirm step is in the caller (openManagePairings X click). This
  // version just does the work.
  await invoke("forget_pairing");
  await invoke("stop_heartbeat").catch(() => {});
  lastSettings.device_token = null;
  lastSettings.paired_device_name = null;
  lastSettings.paired_device_id = null;
  window._wsUrl = "";
  _triedDevices.clear();
  _heartbeatAlive = false;
  renderForgetPairingBtn();
  renderPairedBanner(lastSettings);
  setSyncEnabled(false);
  showSearchingState();
  appendLog("Pairing forgotten. Searching for a new device…");
}

// ----- mDNS discovery -----
//
// On launch we kick off a 10-second browse for `_musicsync._tcp`. If a
// phone responds, we populate the ws_url field automatically and the user
// just sees a green "Found <device>" hint. If nothing responds, the field
// flips to an empty input so the user can type the address manually. The
// "Enter manually" button forces the manual state at any time.

// Phone field has three display states: searching / found / manual.
// Visibility of the "Forget pairing" button is independent — it shows
// whenever there is a stored pairing (regardless of search state).

// Repeating-broadcast state. While the searching banner is up we fire a
// UDP probe every 2 seconds. After 30 seconds with no discovery_found
// event, swap the banner to "No MusicSync Apps Found" with a Rescan
// button so the user knows nothing was found and can retry on demand.
let _scanIntervalId = null;
let _scanTimeoutId = null;

function stopScanCycle() {
  if (_scanIntervalId != null) {
    clearInterval(_scanIntervalId);
    _scanIntervalId = null;
  }
  if (_scanTimeoutId != null) {
    clearTimeout(_scanTimeoutId);
    _scanTimeoutId = null;
  }
}

function startScanCycle() {
  stopScanCycle();
  // Initial scan immediately, then every 2 seconds.
  invoke("start_lan_scan").catch(() => {});
  _scanIntervalId = setInterval(() => {
    invoke("start_lan_scan").catch(() => {});
  }, 2000);
  _scanTimeoutId = setTimeout(() => {
    // 30 seconds elapsed with no hit. Stop probing and show the
    // "nothing found" state with a Rescan button.
    stopScanCycle();
    showNoneFoundState();
  }, 30_000);
}

function showSearchingState() {
  $("ws_url_display").style.display = "";
  $("ws_url_display").innerHTML =
    `<span class="spinner-border spinner-border-sm me-1"></span> ` +
    `Scanning for Viamta Music Sync App`;
  $("ws_url").style.display = "none";
  $("ws_url_edit").textContent = "Enter manually";
  renderForgetPairingBtn();
  startScanCycle();
}

function showNoneFoundState() {
  $("ws_url_display").style.display = "";
  $("ws_url_display").innerHTML =
    `<span class="text-muted">No Viamta Music Sync Apps Found.</span> ` +
    `<button type="button" id="rescan_btn" class="btn btn-sm btn-link p-0 ms-1">Rescan</button>`;
  $("ws_url").style.display = "none";
  $("ws_url_edit").textContent = "Enter manually";
  $("rescan_btn")?.addEventListener("click", () => {
    appendLog("Manual rescan");
    showSearchingState();
  });
}

// Re-render just the colored ● indicator + device label without
// touching anything else. Called by the alive/yellow listeners.
function renderFoundDot(colorClass /* "success" | "warning" */) {
  const deviceName = window._foundDeviceName || "";
  const wsUrl = window._wsUrl || "";
  $("ws_url_display").innerHTML =
    `<span class="text-${colorClass}">●</span> ` +
    `<strong>${escapeHtml(deviceName)}</strong> ` +
    `<span class="text-muted small">${escapeHtml(wsUrl)}</span>`;
}

function showFoundState(deviceName, wsUrl) {
  $("ws_url").value = wsUrl;
  $("ws_url_display").style.display = "";
  $("ws_url_display").innerHTML =
    `<span class="text-success">●</span> ` +
    `<strong>${escapeHtml(deviceName)}</strong> ` +
    `<span class="text-muted small">${escapeHtml(wsUrl)}</span>`;
  $("ws_url").style.display = "none";
  // With an address in hand, the right-side button switches from "Enter
  // manually" to "Scan" — clicking it abandons the current address and
  // re-enters the searching state.
  $("ws_url_edit").textContent = "Scan";
  window._wsUrl = wsUrl;
  window._foundDeviceName = deviceName;
  stopScanCycle();
  invoke("start_heartbeat", { wsUrl }).catch(() => {});
  renderForgetPairingBtn();
}

// Toggle the input-group between "showing the discovered/found address"
// and "letting the user type one in." During manual entry the Enter
// manually button is hidden and Save/Cancel take its place; Forget
// pairing also hides because it's unrelated to typing.
function showManualState() {
  $("ws_url_display").style.display = "none";
  $("ws_url").style.display = "";
  $("ws_url_edit").style.display = "none";
  $("ws_url_save").style.display = "";
  $("ws_url_cancel").style.display = "";
  $("forget_pairing_btn").style.display = "none";
  // Pausing auto-rescan while the user is typing keeps the searching
  // text from popping back in unexpectedly.
  stopScanCycle();
  $("ws_url").focus();
}

// Restore the display + Enter manually button. Re-honors the
// Forget-pairing visibility rules based on current token state.
function exitManualState() {
  $("ws_url").style.display = "none";
  $("ws_url_save").style.display = "none";
  $("ws_url_cancel").style.display = "none";
  $("ws_url_edit").style.display = "";
  $("ws_url_display").style.display = "";
  renderForgetPairingBtn();
}

async function saveManualUrl() {
  const wsUrl = $("ws_url").value.trim();
  if (!wsUrl) return;
  exitManualState();
  // Synthesise a discovery hit so the same auto-pair / scan code path
  // runs as if mDNS had just reported this device.
  _triedDevices.delete(wsUrl);
  appendLog(`Manual address entered: ${wsUrl}`);
  showFoundState("manual entry", wsUrl);
  if (lastSettings.device_token) {
    try { await scanDevice(); } catch (_) { /* logged by scanDevice */ }
  } else {
    startPair();
  }
}

function cancelManualUrl() {
  exitManualState();
  // Whatever state we were in before the user clicked Enter manually:
  // if we already had a discovered address, restore it; otherwise back
  // to the searching banner.
  if (window._wsUrl && window._foundDeviceName) {
    showFoundState(window._foundDeviceName, window._wsUrl);
  } else {
    showSearchingState();
  }
}

function renderForgetPairingBtn() {
  $("forget_pairing_btn").style.display =
    lastSettings.device_token ? "" : "none";
}

// Sync stays disabled until a successful scan has happened against the
// currently-active phone. Call this whenever scan succeeds / fails / forget.
function setSyncEnabled(ok) {
  const btn = $("sync");
  if (ok) {
    btn.removeAttribute("disabled");
    // Remove the "why is this disabled" hover tip while it's actually
    // clickable — hovering over a usable button shouldn't lecture you.
    btn.removeAttribute("title");
  } else {
    btn.setAttribute("disabled", "true");
    const tip = btn.getAttribute("data-disabled-title");
    if (tip) btn.setAttribute("title", tip);
  }
}

// Discovery dispatcher. The behavior depends on pair state:
//  - Paired (token + remembered name): only act on devices whose name
//    matches paired_device_name. Set address silently, then scan.
//  - Unpaired (no token): take the first device that isn't in the
//    ignored list and auto-initiate pairing.
// Track devices we've already tried this session so we don't spam
// connect attempts on every duplicate broadcast reply.
const _triedDevices = new Set();

// Heartbeat-driven connection indicator. While the heartbeat is alive we
// keep the green dot + device name. When the phone goes away (Wi-Fi drop,
// app killed) we fall back to the searching state and let mDNS / UDP
// discovery pick it up again. The previously-tried device IS removed from
// the dedup set so a fresh discovery_found can re-trigger the connect path.
let _heartbeatAlive = false;

listen("device_alive", () => {
  _heartbeatAlive = true;
  if (window._foundDeviceName && window._wsUrl) {
    renderFoundDot("success");
  }
});

// 4-9 seconds since the last Pong from the phone. Show the device with
// a yellow dot to signal "still connected on paper but not responsive."
// On the next Pong this flips back to green; if it stays this way for
// long enough, the backend emits device_dead and we drop to searching.
listen("device_yellow", () => {
  if (window._foundDeviceName && window._wsUrl) {
    renderFoundDot("warning");
  }
});

// Stored token no longer valid → ask the user with the same generic
// "found a MusicSync App, approve?" wording. No long explanation.
let _tokenRejectPromptOpen = false;
listen("heartbeat_token_rejected", async () => {
  _heartbeatAlive = false;
  setSyncEnabled(false);
  showSearchingState();
  if (_tokenRejectPromptOpen) return;
  _tokenRejectPromptOpen = true;
  try {
    const name = window._foundDeviceName || lastSettings.paired_device_name || "(unknown)";
    const url = window._wsUrl || "(unknown address)";
    const ok = window.confirm(`Found a Viamta Music Sync App. Approve?\n\n${name} (${url})`);
    if (ok) {
      await forgetPairing();
      appendLog("Re-pairing — waiting for next discovery hit…");
    }
  } finally {
    _tokenRejectPromptOpen = false;
  }
});

listen("device_dead", () => {
  if (_heartbeatAlive) {
    _heartbeatAlive = false;
    appendLog(`Lost connection to ${window._foundDeviceName || "phone"}. Re-scanning…`);
    // Forget who we were talking to so the searching banner doesn't keep
    // the stale name, and clear the discovery dedup set so the next
    // discovery_found event re-triggers the connect path.
    _triedDevices.clear();
    window._wsUrl = "";
    // (keep window._foundDeviceName for the "Lost connection to X" log)
    setSyncEnabled(false);
    showSearchingState();
    // Kick a fresh LAN broadcast so we don't wait for the next mDNS tick.
    invoke("start_lan_scan").catch(() => {});
  }
});

// The phone pushes DEVICE_RENAMED over the persistent heartbeat WS
// whenever the user changes its display name. The backend updates
// settings on disk and then emits this so the UI redraws the "paired
// with X" banner in place — no scan, no reconnect.
listen("paired_device_renamed", async (e) => {
  const { device_id, device_name } = e.payload;
  lastSettings.paired_device_name = device_name;
  if (device_id) lastSettings.paired_device_id = device_id;
  if (window._foundDeviceName) window._foundDeviceName = device_name;
  // Re-render whichever indicator is currently visible.
  if (window._wsUrl && window._foundDeviceName) {
    renderFoundDot(_heartbeatAlive ? "success" : "warning");
  }
  renderPairedBanner(lastSettings);
  appendLog(`Phone renamed to ${device_name}`);
});

listen("discovery_found", async (e) => {
  const { ws_url, device_name, device_id } = e.payload;
  // Dedup by device_id when the companion advertises one — that's the
  // stable identity, so a rename (which changes the mDNS instance name
  // and re-fires discovery_found) hits the same key and is ignored.
  // Fall back to ws_url for older companions without device_id.
  const dedupKey = device_id || ws_url;
  if (_triedDevices.has(dedupKey)) return;
  _triedDevices.add(dedupKey);
  appendLog(`Discovered ${device_name} at ${ws_url}`);

  if (lastSettings.device_token) {
    // We have a token. If we know our paired phone's device_id, only act
    // on the matching device — that's the durable identity. Otherwise
    // fall back to trying any hit (legacy pairings without a stored id).
    if (
      device_id &&
      lastSettings.paired_device_id &&
      device_id !== lastSettings.paired_device_id
    ) {
      appendLog(`Ignoring ${device_name}: not the paired phone`);
      return;
    }
    showFoundState(device_name, ws_url);
    try {
      await scanDevice();
      appendLog(`Connected to ${device_name}.`);
    } catch (err) {
      const msg = String(err);
      const looksLikeAuth = /bad token|auth|violated/i.test(msg);
      if (looksLikeAuth) {
        appendLog(
          `Stored pairing rejected by ${device_name}. ` +
          `If this is the phone you meant to use, tap "Forget pairing" ` +
          `here (or on the phone) and then reconnect.`
        );
      } else {
        appendLog(`Couldn't reach ${device_name}: ${err}`);
      }
    }
    return;
  }
  // Unpaired path: auto-initiate pair on the first non-ignored hit.
  showFoundState(device_name, ws_url);
  appendLog(`No stored pairing — auto-pairing with ${device_name}`);
  startPair();
});
// No discovery_idle handler — the backend browses forever. The user
// switches to manual mode via the "Enter manually" button at any time.

window.addEventListener("DOMContentLoaded", async () => {
  await loadSettings();
  // Auto-reload whenever the library_path field changes — handles the
  // "switched to a different export" case.
  $("library_path").addEventListener("change", async () => {
    await saveSettings();
    await loadLibrary();
  });
  // Auto-reload when the backend detects the Library.xml on disk changed.
  // The user clicks of playlist toggles are written to settings on each
  // click, and the reload reads them back from settings, so reloads do not
  // clobber the user's pending selections.
  listen("library_changed", async () => {
    appendLog("Library.xml changed on disk — auto-reloading");
    await loadLibrary();
  });
  $("sync").addEventListener("click", runSync);
  $("stop_sync").addEventListener("click", stopSync);
  $("pair_confirm_btn").addEventListener("click", confirmPair);
  $("pair_cancel_btn").addEventListener("click", cancelPair);
  // Single button, two modes: "Enter manually" (in searching/none-found
  // state) opens the type-an-address input; "Scan" (in found state)
  // disconnects + falls back to searching. Branch on current label.
  $("ws_url_edit").addEventListener("click", async () => {
    if ($("ws_url_edit").textContent.trim() === "Scan") {
      // The button only displays "Scan" while a device is selected
      // (found state). Confirm regardless of the heartbeat flag — the
      // JS state can briefly disagree with the actual server side,
      // and there's no harm in confirming.
      const ok = window.confirm(
        "This will break the current connection and scan for Viamta Music Sync Apps. Continue?"
      );
      if (!ok) return;
      // Disconnect the persistent presence WS and drop back to search.
      try { await invoke("stop_heartbeat"); } catch (_) { /* fine */ }
      _heartbeatAlive = false;
      _triedDevices.clear();
      window._wsUrl = "";
      window._foundDeviceName = "";
      setSyncEnabled(false);
      showSearchingState();
      appendLog("Disconnected — scanning for Viamta Music Sync Apps");
    } else {
      showManualState();
    }
  });
  $("ws_url_save").addEventListener("click", saveManualUrl);
  $("ws_url_cancel").addEventListener("click", cancelManualUrl);
  // Enter key in the field = Save (saves a click).
  $("ws_url").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); saveManualUrl(); }
    if (e.key === "Escape") { e.preventDefault(); cancelManualUrl(); }
  });
  $("forget_pairing_btn").addEventListener("click", openManagePairings);
  $("manage_pairings_close").addEventListener("click", () => {
    if (managePairingsModal) managePairingsModal.hide();
  });
  // (The old #delete_unused_songs checkbox lived in a separate panel
  // that's been replaced by the in-table orphan row. Handler attached
  // inline in renderPlaylists() now.)

  // Verbose-logging checkbox in the About tab.
  const vlog = $("verbose_logging");
  if (vlog) {
    vlog.checked = !!lastSettings.verbose_logging;
    vlog.addEventListener("change", async (e) => {
      await invoke("set_verbose_logging", { value: e.target.checked });
      lastSettings.verbose_logging = e.target.checked;
      appendLog(
        e.target.checked
          ? "Verbose logging ON — per-track matching detail will appear here and in musicsync-YYYY-MM-DD.log"
          : "Verbose logging OFF"
      );
    });
  }
  // "Click here to export to a file" — dumps the in-memory Log tab
  // contents to a user-chosen file via a native save dialog.
  const exportBtn = $("export_log");
  if (exportBtn) {
    exportBtn.addEventListener("click", async (e) => {
      e.preventDefault();
      try {
        const stamp = new Date().toISOString().replace(/[:.]/g, "-");
        const path = await invoke("plugin:dialog|save", {
          options: {
            defaultPath: `musicsync-log-${stamp}.txt`,
            filters: [
              { name: "Text", extensions: ["txt", "log"] },
              { name: "All files", extensions: ["*"] },
            ],
          },
        });
        if (!path) return; // user cancelled
        const contents = $("log").innerText;
        await invoke("write_text_file", { path, contents });
        appendLog(`Log exported to ${path}`);
      } catch (err) {
        appendLog(`Export failed: ${err}`);
      }
    });
  }

  // Backend-pushed log lines (verbose tracing dumps).
  listen("log_line", (e) => appendLog(e.payload));
  const msg1 = `MusicSync frontend ${MUSICSYNC_BUILD} ready`;
  console.log(msg1);
  appendLog(msg1);

  const browseBtn = $("library_path_browse");
  if (!browseBtn) {
    const m = "WARN: Browse button element not found in DOM";
    console.warn(m);
    appendLog(m);
  } else {
    console.log("Browse button bound");
    appendLog("Browse button bound");
  }
  $("library_path_browse").addEventListener("click", async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        filters: [
          { name: "Library XML", extensions: ["xml"] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (!selected) return; // user cancelled — no log noise
      $("library_path").value = selected;
      await saveSettings();
      await loadLibrary();
    } catch (e) {
      // Real errors still go to the log so we can see if something went
      // wrong (CDN, plugin permissions, etc.).
      appendLog(`Browse failed: ${e}`);
      console.error("Browse failed", e);
    }
  });
  // Save settings on any field change (no polling — single event per edit).
  for (const id of SETTINGS_FIELDS) {
    const el = $(id);
    if (el) el.addEventListener("change", saveSettings);
  }
  // Kick off mDNS browse on launch. Initialise UI to the right state
  // based on whether we already have a stored pairing.
  renderForgetPairingBtn();
  showSearchingState();
  setSyncEnabled(false);
  try { await invoke("start_discovery"); } catch (e) { appendLog(`Discovery failed: ${e}`); }
  // Fast-path probe of recent_devices ws_urls. Has to run AFTER
  // listen("discovery_found", …) is wired up above — otherwise the
  // probe's synthetic event arrives during webview boot and gets
  // dropped, leaving the address bar stuck on the "Scanning…" → "No
  // … Apps Found" path even while sync proceeds in the background.
  try { await invoke("start_recent_probe"); } catch (e) { appendLog(`Recent-probe failed: ${e}`); }
  // showSearchingState() (called above) already kicked off the
  // every-2-seconds UDP broadcast cycle, so no need to fire one here.
  // Attempt initial load if a library path is set.
  if ($("library_path").value) await loadLibrary();
});
