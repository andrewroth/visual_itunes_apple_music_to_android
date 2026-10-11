// Unit tests for the phone-search scheduling in src/scan_cycle.js.
// Run with `npm test` from tauri/ (Node 20+, no dependencies).

import { test, beforeEach, afterEach, mock } from "node:test";
import assert from "node:assert/strict";

import {
  createScanCycle,
  deviceDeadAction,
  LAN_SCAN_INTERVAL_MS,
  SUBNET_SWEEP_INTERVAL_MS,
  SEARCH_TIMEOUT_MS,
} from "../src/scan_cycle.js";

let calls;
let alive;

function makeCycle() {
  return createScanCycle({
    lanScan: () => calls.lan++,
    subnetSweep: () => calls.sweep++,
    tryManual: () => calls.manual++,
    isAlive: () => alive,
    onNoneFound: () => calls.noneFound++,
  });
}

beforeEach(() => {
  calls = { lan: 0, sweep: 0, manual: 0, noneFound: 0 };
  alive = false;
  mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
});

afterEach(() => {
  mock.timers.reset();
});

test("start fires a LAN scan, a subnet sweep and the manual try immediately", () => {
  const cycle = makeCycle();
  cycle.start();
  assert.deepEqual(calls, { lan: 1, sweep: 1, manual: 1, noneFound: 0 });
  assert.equal(cycle.running(), true);
});

test("tryManual: false skips the remembered address but still searches", () => {
  const cycle = makeCycle();
  cycle.start({ tryManual: false });
  assert.equal(calls.manual, 0);
  assert.equal(calls.lan, 1);
  assert.equal(calls.sweep, 1);
});

test("a dead manual address doesn't stop the search: scans and sweeps keep repeating", () => {
  // The bug: trying the remembered address used to stop the cycle
  // straight away, so a phone that had changed IP was never found.
  const cycle = makeCycle();
  cycle.start();
  mock.timers.tick(SUBNET_SWEEP_INTERVAL_MS * 2);
  assert.equal(calls.lan, 1 + (SUBNET_SWEEP_INTERVAL_MS * 2) / LAN_SCAN_INTERVAL_MS);
  assert.equal(calls.sweep, 3);
  assert.equal(calls.manual, 1, "manual address is only tried once per search");
  assert.equal(cycle.running(), true);
});

test("gives up after the timeout and shows none-found when nothing answered", () => {
  const cycle = makeCycle();
  cycle.start();
  mock.timers.tick(SEARCH_TIMEOUT_MS);
  assert.equal(calls.noneFound, 1);
  assert.equal(cycle.running(), false);
  const lanAtTimeout = calls.lan;
  mock.timers.tick(SEARCH_TIMEOUT_MS);
  assert.equal(calls.lan, lanAtTimeout, "no scans after giving up");
});

test("timeout doesn't show none-found if a phone came alive", () => {
  const cycle = makeCycle();
  cycle.start();
  alive = true;
  mock.timers.tick(SEARCH_TIMEOUT_MS);
  assert.equal(calls.noneFound, 0);
});

test("sweep interval stops the cycle once a phone is alive", () => {
  const cycle = makeCycle();
  cycle.start();
  alive = true;
  mock.timers.tick(SUBNET_SWEEP_INTERVAL_MS);
  assert.equal(calls.sweep, 1, "no further sweeps once alive");
  assert.equal(cycle.running(), false);
});

test("stop cancels every timer", () => {
  const cycle = makeCycle();
  cycle.start();
  cycle.stop();
  mock.timers.tick(SEARCH_TIMEOUT_MS * 2);
  assert.deepEqual(calls, { lan: 1, sweep: 1, manual: 1, noneFound: 0 });
  assert.equal(cycle.running(), false);
});

test("restarting doesn't stack duplicate timers", () => {
  const cycle = makeCycle();
  cycle.start();
  cycle.start();
  calls = { lan: 0, sweep: 0, manual: 0, noneFound: 0 };
  mock.timers.tick(SUBNET_SWEEP_INTERVAL_MS);
  assert.equal(calls.lan, SUBNET_SWEEP_INTERVAL_MS / LAN_SCAN_INTERVAL_MS);
  assert.equal(calls.sweep, 1);
  mock.timers.tick(SEARCH_TIMEOUT_MS);
  assert.equal(calls.noneFound, 1);
});

test("deviceDeadAction: losing a live connection rescans and clears tried devices", () => {
  assert.equal(deviceDeadAction({ wasAlive: true, searching: false }), "rescan-clear");
  assert.equal(deviceDeadAction({ wasAlive: true, searching: true }), "rescan-clear");
});

test("deviceDeadAction: an unanswered address mid-search is ignored", () => {
  assert.equal(deviceDeadAction({ wasAlive: false, searching: true }), "ignore");
});

test("deviceDeadAction: an unanswered address with no search running rescans", () => {
  // Previously ignored, which left the app showing a dead address forever.
  assert.equal(deviceDeadAction({ wasAlive: false, searching: false }), "rescan-keep");
});
