// The frame budget on a platform that cannot read the machine's load: it is recorded, never judged. Node's loadavg is
// always [0, 0, 0] on Windows, so "not loud" there says nothing, and a budget judged by it fails a slow runner that is
// fine (t-43414: the thousand-page settle gap, 185–228 ms on Windows, against a 128 ms wall). Same rule as lane.sh's
// wait_for_calm: an unknown load is said, never judged calm.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as machineLoad from "./machine-load.mjs";

test("a frame over its wall on a platform that cannot read the load holds, unjudged", () => {
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "win32", load: 0 }), true);
});

test("the load note names an unreadable platform in its own words", () => {
  assert.match(machineLoad.loadNote({ platform: "win32", load: 0 }), /load unreadable on win32/);
});

test("the rule reads a platform's own load: a readable one judges a frame over its wall by the machine's load", () => {
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "darwin", load: 0 }), false);
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "darwin", load: 1e3 }), true);
  assert.equal(machineLoad.frameBudgetHolds(60, 128, { platform: "darwin", load: 0 }), true);
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "win32", load: 1e3 }), true);
});

test("the default platform is the machine's own, not the page's report", () => {
  // TEST_PLATFORM is the platform a page reports (ZO_TEST_PLATFORM); the load is the OS's, so the default is process.platform.
  assert.equal(machineLoad.loadReadable(), process.platform !== "win32");
  assert.equal(machineLoad.loadReadable("win32"), false);
  assert.equal(machineLoad.loadReadable("darwin"), true);
});

test("the load note this run saw is logged for the record", () => {
  console.log(`load note at this run: ${machineLoad.loadNote()} on ${process.platform}`);
});
