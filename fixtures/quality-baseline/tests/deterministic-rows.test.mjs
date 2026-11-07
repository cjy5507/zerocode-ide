import assert from "node:assert/strict";
import test from "node:test";
import { requireDeterministicRows, table } from "../runner.mjs";

const opts = { task: null, repeat: 2 };
const rows = () => Object.keys(table.Tasks).flatMap(task => [1, 2].map(repeat => ({ task, repeat, verified: true, interventions: 0 })));

test("deterministic acceptance requires every task/repeat, in any order", () => {
  assert.doesNotThrow(() => requireDeterministicRows(rows().reverse(), opts));
  assert.doesNotThrow(() => requireDeterministicRows([{ task: "Q5", repeat: 1, verified: true, interventions: 0 }], { task: "Q5", repeat: 1 }));
});

test("an empty, truncated, duplicated or unexpected cohort is not success", () => {
  assert.throws(() => requireDeterministicRows([], opts));
  assert.throws(() => requireDeterministicRows(rows().slice(1), opts));
  for (const replacement of [rows()[1], { ...rows()[0], task: "unexpected" }, { ...rows()[0], repeat: 0 }]) {
    const cohort = rows(); cohort[0] = replacement;
    assert.throws(() => requireDeterministicRows(cohort, opts));
  }
});

test("a complete cohort still fails on failed judgment or human input", () => {
  for (const change of [{ verified: false }, { verified: "true" }, { interventions: 1 }]) {
    const cohort = rows(); cohort[0] = { ...cohort[0], ...change };
    assert.throws(() => requireDeterministicRows(cohort, opts));
  }
});
