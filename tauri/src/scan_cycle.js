// Phone-search scheduling, split out of main.js so it can be unit-tested
// under Node (see tauri/tests/scan_cycle.test.js). No DOM or Tauri
// imports here: everything the cycle touches is passed in.

export const LAN_SCAN_INTERVAL_MS = 2000;
export const SUBNET_SWEEP_INTERVAL_MS = 10_000;
export const SEARCH_TIMEOUT_MS = 30_000;

// While searching we fire a UDP probe every 2 seconds and a subnet
// sweep every 10 seconds, and try the remembered manual address once.
// All three run in parallel; whichever reaches the phone first wins.
// The cycle only stops when the phone is confirmed alive (stop() from
// markDeviceAlive) or after 30 seconds, when onNoneFound runs if
// nothing has answered.
//
// deps:
//   lanScan()      fire one UDP/mDNS probe
//   subnetSweep()  fire one bounded TCP sweep of the local subnet
//   tryManual()    attempt the remembered manual address (may no-op)
//   isAlive()      true once the heartbeat has had a pong from a phone
//   onNoneFound()  render the "nothing found, Rescan" state
//   timers         optional { setTimeout, setInterval, clearTimeout, clearInterval }
export function createScanCycle(deps) {
  const t = deps.timers || globalThis;
  let lanId = null;
  let sweepId = null;
  let timeoutId = null;

  function stop() {
    if (lanId != null) { t.clearInterval(lanId); lanId = null; }
    if (sweepId != null) { t.clearInterval(sweepId); sweepId = null; }
    if (timeoutId != null) { t.clearTimeout(timeoutId); timeoutId = null; }
  }

  function start({ tryManual = true } = {}) {
    stop();
    deps.lanScan();
    lanId = t.setInterval(() => deps.lanScan(), LAN_SCAN_INTERVAL_MS);
    // A remembered address that no longer answers must not end the
    // search, so the sweep keeps going until a phone is actually alive.
    deps.subnetSweep();
    sweepId = t.setInterval(() => {
      if (deps.isAlive()) { stop(); return; }
      deps.subnetSweep();
    }, SUBNET_SWEEP_INTERVAL_MS);
    if (tryManual) deps.tryManual();
    timeoutId = t.setTimeout(() => {
      timeoutId = null;
      stop();
      if (!deps.isAlive()) deps.onNoneFound();
    }, SEARCH_TIMEOUT_MS);
  }

  function running() {
    return lanId != null;
  }

  return { start, stop, running };
}

// What to do when the heartbeat reports the current address dead.
//  - Was alive: real connection loss. Search again and forget which
//    devices were already tried, so rediscovering the same phone at a
//    new IP reconnects.
//  - Never alive and still searching: the address being tried didn't
//    answer; the running search carries on, nothing to do.
//  - Never alive and not searching: we were sitting on an address that
//    never answered. Search again, but keep the tried set so the same
//    dead address isn't retried in a loop.
export function deviceDeadAction({ wasAlive, searching }) {
  if (wasAlive) return "rescan-clear";
  if (searching) return "ignore";
  return "rescan-keep";
}
