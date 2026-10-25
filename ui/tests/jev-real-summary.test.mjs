import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { realSummary } from "./jev-real-summary.mjs";

/* The dashboard's evidence step (t-73988). It used to run whatever zo sat in
 * the home it ran in and read that home's `~/.zo`, so the gate passed on the
 * machine whose data it read and timed out in a clean home. These hold it to
 * the promise: the same answer in any home, and an installed zo is run only
 * when a person names one. Every zo here is a script written into a
 * throwaway home; none is the person's. */

const SEATS = JSON.stringify({ seats: [{ id: "recall", mode: "shadow", found: null }] });

/* A home with a `zo` at the installed place that leaves a mark when it runs,
 * and a second one a test can name. */
function homeWithZo(body) {
  const home = mkdtempSync(join(tmpdir(), "jev-real-summary-"));
  const bin = join(home, ".local", "bin");
  mkdirSync(bin, { recursive: true });
  const marks = join(home, "ran");
  const script = (path) => {
    writeFileSync(path, `#!/bin/sh\necho "$0" >> "${marks}"\n${body}\n`);
    chmodSync(path, 0o755);
  };
  script(join(bin, "zo"));
  script(join(home, "named-zo"));
  return { home, installed: join(bin, "zo"), named: join(home, "named-zo"), marks };
}

test("an installed zo in the home is neither run nor read when no zo is named", () => {
  const { home, marks } = homeWithZo(`echo '${SEATS}'`);
  try {
    assert.equal(realSummary({ env: {}, home, root: home }), null);
    assert.equal(existsSync(marks), false, "the installed zo ran");
  } finally { rmSync(home, { recursive: true, force: true }); }
});

test("a zo a person names is the one that answers", () => {
  const { home, named, installed, marks } = homeWithZo(`echo '${SEATS}'`);
  try {
    const got = realSummary({ env: { JEV_ZO_BIN: named }, home, root: home });
    assert.equal(got?.zo, named);
    assert.deepEqual(got.seats.map((seat) => seat.id), ["recall"]);
    assert.equal(existsSync(marks) && readFileSync(marks, "utf8").includes(installed), false);
  } finally { rmSync(home, { recursive: true, force: true }); }
});

test("a named zo that never answers leaves the fixture in under the gate's own limit", () => {
  const { home, named } = homeWithZo("sleep 120");
  try {
    const began = Date.now();
    assert.equal(realSummary({ env: { JEV_ZO_BIN: named }, home, root: home }), null);
    assert.ok(Date.now() - began < 15_000, `waited ${Date.now() - began} ms`);
  } finally { rmSync(home, { recursive: true, force: true }); }
});
