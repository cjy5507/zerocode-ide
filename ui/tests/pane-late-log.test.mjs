import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parse } from "acorn";

const shell = readFileSync(new URL("../shell.js", import.meta.url), "utf8");
const tree = parse(shell, { ecmaVersion: "latest" });
const lifecycle = readFileSync(new URL("../shell-pane-lifecycle.js", import.meta.url), "utf8");
const production = (name) => {
  const node = tree.body.find(node => node.type === "FunctionDeclaration" && node.id?.name === name);
  assert(node, `production function ${name} is present`);
  return shell.slice(node.start, node.end);
};

function pageAnswering(id = "pane", { paintError = false } = {}) {
  let reply;
  let reject;
  const requests = [];
  const held = { id, next: 0, epoch: 0, found: null, from: 0, skipped: false, turns: [] };
  const tab = { kind: "chat", worker: { term: 7, helper: held } };
  let active = tab;
  const context = vm.createContext({
    activeHelperPage: () => active,
    invoke: (command, args) => new Promise((resolve, failed) => {
      reply = resolve; reject = failed; requests.push({ command, args });
    }),
    PANE_LOG_ID: "pane", WIRE_LOG_ID: "wire",
    document: { hidden: true },
    paintHelperSurface: () => { if (paintError) throw new Error("paint failed"); }, paintStatusModel: () => {},
    holdHelperAbout: () => false,
    holdHelperTurns: (entry, turns) => entry.turns.push(...turns),
    queueMicrotask,
  });
  vm.runInContext(lifecycle, context);
  vm.runInContext('const paneModels = paneLifecycle.map("paneModels"); const paneUsage = paneLifecycle.map("paneUsage");', context);
  vm.runInContext(production("rememberPaneUsage") + "\n" + production("pollHelperPages"), context);
  const stores = vm.runInContext("({ paneLifecycle, paneModels, paneUsage })", context);
  return {
    held, requests, ...stores,
    poll: () => vm.runInContext("pollHelperPages()", context),
    reply: value => reply(value),
    reject: error => reject(error),
    close: () => { stores.paneLifecycle.dispose(7); active = null; },
  };
}

const answer = { found: true, next: 1, model: "transcript-model", usage: { tokens: 900, window: 1000 }, turns: [] };

test("a pane log arriving after closure cannot recreate pane model or usage", async () => {
  const page = pageAnswering();
  const pending = page.poll();
  page.close();
  page.reply(answer);
  await pending;
  assert.equal(page.paneModels.has(7), false);
  assert.equal(page.paneUsage.has(7), false);
  assert.equal(page.paneLifecycle.pending(), 0);
});

test("an old pane log cannot replace the usage of a new pane at that id", async () => {
  const page = pageAnswering();
  const pending = page.poll();
  page.close();
  page.paneModels.set(7, "new-pane-model");
  page.paneUsage.set(7, { tokens: 10, window: 100 });
  page.reply(answer);
  await pending;
  assert.equal(page.paneModels.get(7), "new-pane-model");
  assert.equal(page.paneUsage.get(7).tokens, 10);
});

test("live pane logs still update their metadata and helper history still arrives", async () => {
  const live = pageAnswering();
  const pending = live.poll();
  live.reply(answer);
  await pending;
  assert.equal(live.paneModels.get(7), "transcript-model");
  assert.equal(live.paneUsage.get(7).tokens, 900);
  const helper = pageAnswering("helper-1");
  const history = helper.poll();
  helper.close();
  helper.reply({ ...answer, turns: [{ text: "retained history" }] });
  await history;
  assert.equal(helper.held.turns[0].text, "retained history");
});

test("a rejected log is quiet, clears polling and permits a later read", async () => {
  const page = pageAnswering();
  const failed = page.poll();
  page.reject(new Error("backend unavailable"));
  await failed;
  assert.equal(page.held.polling, false);
  assert.equal(page.paneLifecycle.pending(), 0);
  const retry = page.poll();
  page.reply(answer);
  await retry;
  assert.equal(page.paneModels.get(7), "transcript-model");
});

test("a receiver failure is observable and still releases its answer fence", async () => {
  const page = pageAnswering("pane", { paintError: true });
  const pending = page.poll();
  page.reply(answer);
  await assert.rejects(pending, /paint failed/);
  assert.equal(page.paneLifecycle.pending(), 0);
});

test("news during a log read retries from the cursor the first answer committed", async () => {
  const page = pageAnswering();
  const pending = page.poll();
  await page.poll();
  page.reply({ ...answer, next: 5 });
  await pending;
  await new Promise(setImmediate);
  assert.equal(page.requests.length, 2);
  assert.equal(page.requests[1].args.after, 5);
  page.reply({ ...answer, next: 5 });
  await new Promise(setImmediate);
  assert.equal(page.paneLifecycle.pending(), 0);
});
