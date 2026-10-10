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

test("the load alone says loud: a zero reading (what Windows reads) is not loud, so the path-time excuse is not granted there", () => {
  assert.equal(machineLoad.loadIsHigh(0), false);
  assert.equal(machineLoad.loadIsHigh(1e3), true);
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
