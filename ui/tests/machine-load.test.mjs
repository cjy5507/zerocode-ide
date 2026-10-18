// The frame budget on a platform that cannot read the machine's load: it is recorded, never judged. Node's loadavg is
// always [0, 0, 0] on Windows, so "not loud" there says nothing, and a budget judged by it fails a slow runner that is
// fine (t-43414: the thousand-page settle gap, 185–228 ms on Windows, against a 128 ms wall). Same rule as lane.sh's
// wait_for_calm: an unknown load is said, never judged calm.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as machineLoad from "./machine-load.mjs";

test("a frame over its wall on a platform that cannot read the load holds, unjudged", () => {
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "win32", load: 0, env: {} }), true);
});

test("the load note names an unreadable platform in its own words", () => {
  assert.match(machineLoad.loadNote({ platform: "win32", load: 0, env: {} }), /load unreadable on win32/);
});

test("the rule reads a platform's own load: a readable one judges a frame over its wall by the machine's load", () => {
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "darwin", load: 0, env: {} }), false);
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "darwin", load: 1e3, env: {} }), true);
  assert.equal(machineLoad.frameBudgetHolds(60, 128, { platform: "darwin", load: 0, env: {} }), true);
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { platform: "win32", load: 1e3, env: {} }), true);
});

test("the default platform is the machine's own, not the page's report", () => {
  // TEST_PLATFORM is the platform a page reports (ZO_TEST_PLATFORM); the load is the OS's, so the default is process.platform.
  assert.equal(machineLoad.loadReadable(), process.platform !== "win32");
  assert.equal(machineLoad.loadReadable("win32"), false);
  assert.equal(machineLoad.loadReadable("darwin"), true);
});

test("a platform that cannot read the load is loud, so its budget is unjudgeable; a readable quiet one is not", () => {
  assert.equal(machineLoad.machineIsLoudNow({ platform: "win32", load: 0 }), true);
  assert.equal(machineLoad.machineIsLoudNow({ platform: "darwin", load: 0 }), false);
});

test("a budget with no platform passed is judged on this machine's own platform", () => {
  assert.equal(machineLoad.frameBudgetHolds(166, 128, { load: 0, env: {} }), process.platform === "win32");
});

const HOSTED = { GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "github-hosted" };

test("a GitHub-hosted runner records an absolute frame budget and never judges it, whatever the load", () => {
  assert.equal(machineLoad.frameBudgetHolds(206, 128, { platform: "darwin", load: 0, env: HOSTED }), true);
  assert.equal(machineLoad.frameBudgetHolds(206, 128, { platform: "darwin", load: 1e3, env: HOSTED }), true);
  assert.match(machineLoad.loadNote({ platform: "darwin", load: 0, env: HOSTED }),
    /^load \d+\.\d\/\d+ \(hosted runner — budget unjudged\)$/);
});

test("a self-hosted runner, or one with no CI environment, still judges an absolute frame budget", () => {
  assert.equal(machineLoad.frameBudgetHolds(206, 128, { platform: "darwin", load: 0, env: {} }), false);
  assert.equal(machineLoad.frameBudgetHolds(206, 128, { platform: "darwin", load: 0,
    env: { GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" } }), false);
});

test("the hosted rule is for absolute budgets only: the ratio rule still judges on a hosted runner", () => {
  assert.equal(machineLoad.machineIsLoudNow({ platform: "darwin", load: 0, env: HOSTED }), false);
});

test("the load note this run saw is logged for the record", () => {
  console.log(`load note at this run: ${machineLoad.loadNote()} on ${process.platform}`);
});

// A frame judged against two references measured in the same run (t-63638). The machine moves all three together:
// the paths frame at 1020 nodes read 84 ms and 121.8 ms on the same code nine minutes apart (10-18), at loads 6.3
// and 9.6 of 12. Only the frame's own cost crosses the midpoint.
const QUIET = { platform: "darwin", load: 0, env: {} };

test("a frame judged against two references of its own run holds below their midpoint and fails at it", () => {
  assert.equal(machineLoad.referenceMidpoint(80, 104), 92);
  assert.equal(machineLoad.referencesHold(84, 80, 104, QUIET), true);
  assert.equal(machineLoad.referencesHold(92, 80, 104, QUIET), false);
  assert.equal(machineLoad.referencesHold(104, 80, 104, QUIET), false);
});

test("a machine that slows the page slows the frame and both references, so neither verdict moves", () => {
  for (const slow of [1.4, 2.5]) {
    assert.equal(machineLoad.referencesHold(84 * slow, 80 * slow, 104 * slow, QUIET), true);
    assert.equal(machineLoad.referencesHold(104 * slow, 80 * slow, 104 * slow, QUIET), false);
  }
});

test("the reference verdict is recorded where the load cannot judge", () => {
  assert.equal(machineLoad.referencesHold(150, 80, 104, { platform: "darwin", load: 1e3, env: {} }), true);
  assert.equal(machineLoad.referencesHold(150, 80, 104, { platform: "win32", load: 0, env: {} }), true);
});

// A GitHub-hosted runner records the reference verdict as it records an absolute budget, until CI runs show the
// margins (run-11955): the frame and its references are separate readings, and a shared VM's steal time can land on
// one of them.
test("a GitHub-hosted runner records a crossed midpoint and says so; the same numbers fail everywhere else", () => {
  const crossed = [150, 80, 104];
  assert.equal(machineLoad.referencesHold(...crossed, { platform: "darwin", load: 0, env: HOSTED }), true);
  assert.equal(machineLoad.referencesHold(...crossed, { platform: "darwin", load: 0, env: {} }), false);
  assert.equal(machineLoad.referencesHold(...crossed, { platform: "darwin", load: 0,
    env: { GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" } }), false);
  assert.equal(machineLoad.referencesNote(...crossed, { env: HOSTED }),
    "margin -58ms, crossed (hosted runner — recorded)");
  assert.equal(machineLoad.referencesNote(84, 80, 104, { env: HOSTED }), "margin 8ms, held (hosted runner — recorded)");
  assert.equal(machineLoad.referencesNote(...crossed, { env: {} }), "margin -58ms");
});

test("the power note reads the source and the power mode in pmset's own words, and says what it cannot read", () => {
  const ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t80%; charged; 0:00 remaining present: true\n";
  const battery = "Now drawing from 'Battery Power'\n";
  const note = (settings, source) => machineLoad.powerNote({ platform: "darwin", settings, battery: source });
  assert.equal(note(" powermode            1\n", ac), "power AC, powermode 1");
  assert.equal(note(" lowpowermode         0\n", battery), "power battery, lowpowermode 0");
  assert.equal(note(" highpowermode        1\n powermode            2\n", ac), "power AC, powermode 2");
  assert.equal(note("", ""), "power unknown, powermode unknown");
  assert.equal(machineLoad.powerNote({ platform: "linux" }), "power unreadable on linux");
});

test("the memory note reads the kernel's pressure level and the swap in use", () => {
  const swap = "total = 6144.00M  used = 4750.00M  free = 1394.00M  (encrypted)\n";
  const note = (level, text = swap) => machineLoad.memoryNote({ platform: "darwin", level, swap: text });
  assert.equal(note("1\n"), "memory normal, swap 4750/6144 MB");
  assert.equal(note("2\n"), "memory warn, swap 4750/6144 MB");
  assert.equal(note("4\n"), "memory critical, swap 4750/6144 MB");
  assert.equal(note("", ""), "memory unknown, swap unknown");
  assert.equal(machineLoad.memoryNote({ platform: "win32" }), "memory unreadable on win32");
});

test("the power and memory notes this run saw are logged for the record", () => {
  console.log(`power and memory at this run: ${machineLoad.powerNote()}; ${machineLoad.memoryNote()}`
    + ` on ${process.platform}`);
});
