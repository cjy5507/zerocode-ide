import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parse } from "yaml";
import { requirePackageVerification } from "./package-verification.mjs";

test("both package paths wait for every workspace they bundle", async () => {
  const { jobs } = parse(await readFile(new URL("../.github/workflows/verify.yml", import.meta.url), "utf8"));
  const predecessors = (name, seen = new Set()) => {
    for (const parent of [jobs[name].needs ?? []].flat()) {
      if (seen.has(parent)) continue;
      seen.add(parent);
      predecessors(parent, seen);
    }
    return seen;
  };
  for (const name of ["package-smoke", "release-package"]) {
    const waited = predecessors(name);
    assert(waited.has("verify"), `${name} must wait for the root workspace`);
    assert(waited.has("zo-ide-verify"), `${name} must wait for the bundled zo workspace`);
    assert(waited.has("changes"), `${name} must know whether zo verification was deliberately skipped`);
    assert(waited.has("quality-baseline"), `${name} must wait for independently judged workflow completion`);
  }
  assert.equal(jobs["quality-baseline"]["continue-on-error"] ?? false, false);
});

const passed = { root: "success", changes: "success", zo: "success", quality: "success", required: "true", event: "push" };

test("package policy permits passed gates and deliberate unchanged PR skips", () => {
  assert.doesNotThrow(() => requirePackageVerification(passed));
  assert.doesNotThrow(() => requirePackageVerification({ ...passed, event: "pull_request", required: "false", zo: "skipped" }));
});

test("failed, cancelled, missing and accidental skips cannot authorize a package", () => {
  for (const status of ["failure", "cancelled", "skipped", "pending", undefined]) {
    assert.throws(() => requirePackageVerification({ ...passed, root: status }));
    assert.throws(() => requirePackageVerification({ ...passed, changes: status }));
    assert.throws(() => requirePackageVerification({ ...passed, zo: status }));
    assert.throws(() => requirePackageVerification({ ...passed, quality: status }));
  }
  for (const event of ["push", undefined]) {
    assert.throws(() => requirePackageVerification({ ...passed, event, required: "false", zo: "skipped" }));
  }
  for (const required of [undefined, "", "unknown"]) {
    assert.throws(() => requirePackageVerification({ ...passed, required }));
  }
  assert.throws(() => requirePackageVerification({ ...passed, event: "pull_request", required: "false", zo: "failure" }));
});

test("the package verifier consumes every dependency result in the workflow", async () => {
  const { jobs } = parse(await readFile(new URL("../.github/workflows/verify.yml", import.meta.url), "utf8"));
  const gate = jobs["package-verification"];
  const step = gate.steps.find(step => step.run === "node scripts/package-verification.mjs");
  assert(step, "the workflow must execute the tested package policy");
  assert.deepEqual(step.env, {
    ROOT_RESULT: "${{ needs.verify.result }}",
    CHANGES_RESULT: "${{ needs.changes.result }}",
    ZO_RESULT: "${{ needs['zo-ide-verify'].result }}",
    QUALITY_RESULT: "${{ needs['quality-baseline'].result }}",
    ZO_REQUIRED: "${{ needs.changes.outputs.zo_ide }}",
    EVENT_NAME: "${{ github.event_name }}",
  });
});
