// The frame budget on a platform that cannot read the machine's load: it is recorded, never judged. Node's loadavg
// is always [0, 0, 0] on Windows, so "not loud" there says nothing, and a budget judged by it fails a slow runner that
// is fine (t-43414: the thousand-page settle gap, 185–212 ms on Windows, against a 128 ms wall). Same rule as lane.sh's
// wait_for_calm: an unknown load is said, never judged calm.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as machineLoad from "./machine-load.mjs";

test("a platform that cannot read the load does not judge a frame budget", () => {
  assert.equal(machineLoad.loadReadable("win32"), false);
  assert.equal(machineLoad.loadReadable("darwin"), true);
  assert.equal(machineLoad.loadReadable("linux"), true);
  // A gap over the wall on Windows is unjudged, not a failure: it holds.
  assert.equal(machineLoad.frameBudgetHolds(166, 128, "win32"), true);
});

test("the load note says an unreadable platform is unjudged", () => {
  assert.match(machineLoad.loadNote("win32"), /unjudged/);
});

test("a readable platform still judges a frame budget that is over its wall on a quiet machine", () => {
  // Under the wall holds on any platform; the loud case depends on the machine, so only the quiet side is pinned here.
  assert.equal(machineLoad.frameBudgetHolds(60, 128, "darwin"), true);
});
