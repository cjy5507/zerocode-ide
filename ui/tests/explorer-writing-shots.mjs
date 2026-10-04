/* The file tree while an agent writes, photographed (t-31715) — what the person
 * sees, in both themes and under reduced motion, taken from the window harness
 * with fake hooks and a fake scoped git answer.
 *
 *   node ui/tests/explorer-writing-shots.mjs <folder>
 *
 * Two pictures for each look: the tree mid-write (a file shimmering in an open
 * folder, a shut folder shimmering for the file inside it) and the same tree a
 * moment after the writes end (+N -N on the row, the letter and the folder's
 * roll-up). A sheen moves, and a picture does not: it is held still at one
 * instant of its sweep, so the picture is the same every run. Nothing here
 * reads the tree's markup beyond the rows it photographs — it fires what the
 * backend fires (`hook:activity`) and answers what the tree asks
 * (`scm_numstat`). */
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";

const folder = resolve(process.argv[2] ?? "output/explorer-writing");
await mkdir(folder, { recursive: true });

/* How far into its sweep the sheen is held, in ms: a little under half of the
 * sweep (`--tree-writing-sweep`, 1100 ms), where it lies across the row. */
const SHEEN_HELD_AT_MS = 440;

/* How long after the writes end the second picture waits, in ms: the window the
 * numstat is asked in (150) and the time the answer takes to be drawn, with a
 * margin. */
const LANDED_AFTER_MS = 1200;

/* The checkout the pictures stand in. */
const LISTINGS = {
  "": [
    { name: "docs", is_dir: true },
    { name: "src", is_dir: true },
    { name: "tests", is_dir: true },
    { name: "Cargo.toml", is_dir: false },
    { name: "README.md", is_dir: false },
  ],
  src: [
    { name: "lib.rs", is_dir: false },
    { name: "main.rs", is_dir: false },
    { name: "parser.rs", is_dir: false },
  ],
  docs: [{ name: "guide.md", is_dir: false }],
  tests: [{ name: "smoke.rs", is_dir: false }],
};

/* What the scoped question answers for the two files the agent writes: one
 * edited with counts, one new (git has no counts for a file it has not seen).
 * The whole-repository status says the same once the writes have ended — the
 * page re-reads it itself 1.5 s after the last activity, and a status that
 * disagreed with the scoped answer would take the numbers off the rows. */
const ANSWER = {
  "src/parser.rs": { code: " M", added: 42, removed: 7 },
  "tests/smoke.rs": { code: "??", added: null, removed: null },
};

const { files, origin } = await createWindowServer();
const browser = await launchWindowBrowser();
const shots = [];
const faultsSeen = [];

async function look(theme, motion) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.emulateMedia({ reducedMotion: motion });
    await page.evaluate(async ({ listed, answer, one }) => {
      document.documentElement.dataset.theme = one;
      window.__ANSWER__.list_dir = ({ path }) => listed[path] ?? [];
      const entryOf = (path) => ({ path, staged: false, changed: true, conflict: null, origin: null, ...answer[path] });
      window.__ANSWER__.scm_numstat = ({ paths }) => paths.filter((path) => answer[path]).map(entryOf);
      window.__ANSWER__.scm_status = () => ({ changed: window.__LANDED__ ? Object.keys(answer).map(entryOf) : [], ignored: [] });
      revealActivity("files", "name");
      fileSearch.blur();
      resetTreeSelection();
      await loadTree(fileTree, "");
      // The folders stay as they are: the tree does not walk to the file, so the
      // shut folder is seen wearing the state itself.
      document.getElementById("tree-follow").click();
      await fileTree.querySelector('.tree-row[data-tree-path="src"]')._treeUnfold(true);
    }, { listed: LISTINGS, answer: ANSWER, one: theme });
    const fire = (activities) => page.evaluate(({ batch }) => {
      const root = activeWorktreePath;
      for (const handler of window.__LISTENERS__["hook:activity"] ?? []) {
        handler({ payload: { pane: "term:1", activities: batch.map((one, index) => ({ seq: index + 1, activity: { verb: one.verb, target: `${root}/${one.path}`, phase: one.phase, call: one.call } })) } });
      }
    }, { batch: activities });
    const settle = () => page.waitForTimeout(250);
    const name = `${theme}${motion === "reduce" ? "-reduced" : ""}`;
    const shoot = async (picture) => {
      const path = resolve(folder, `${name}-${picture}.png`);
      await page.locator("#activity-files").screenshot({ path });
      shots.push(path);
    };

    await fire([
      { verb: "edit", path: "src/parser.rs", phase: "started", call: "call-1" },
      { verb: "write", path: "tests/smoke.rs", phase: "started", call: "call-2" },
    ]);
    await settle();
    await page.evaluate((at) => {
      for (const one of document.getAnimations()) {
        if (one.animationName !== "tree-writing-sweep") continue;
        one.pause();
        one.currentTime = at;
      }
    }, SHEEN_HELD_AT_MS);
    await shoot("1-writing");

    await page.evaluate(() => { window.__LANDED__ = true; });
    await fire([
      { verb: "edit", path: "src/parser.rs", phase: "finished", call: "call-1" },
      { verb: "write", path: "tests/smoke.rs", phase: "finished", call: "call-2" },
    ]);
    await page.waitForTimeout(LANDED_AFTER_MS);
    await shoot("2-landed");
  } finally {
    faultsSeen.push(...faults);
    await page.close();
  }
}

for (const theme of ["dark", "light"]) {
  await look(theme, "no-preference");
  await look(theme, "reduce");
}

await browser.close();
files.close();
console.log(JSON.stringify({ shots, faults: faultsSeen }, null, 2));
process.exit(faultsSeen.length === 0 ? 0 : 1);
