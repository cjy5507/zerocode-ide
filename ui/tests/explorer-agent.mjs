/* The file tree wears what the agents do (t-24298).
 *
 * The person looked at a Claude Code mod that draws a file tree pane
 * (claude-code-filetree, MIT) and asked for its behaviour in THIS window's
 * tree, for every agent and not only Claude: the files an agent reads,
 * writes and commits light up, the tree unfolds to the one being touched,
 * git's numbers sit on the rows, and the keyboard walks the tree.
 *
 * Each suite drives the real window through the roads the backend uses —
 * `hook:activity` batches, `scm_status`, `upstream_status`, `list_dir` — and
 * reads the laid-out rows. Nothing here calls a function the feature added:
 * on a window without it, every check fails at its assertion. */
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { openWindowTestPage, PRIMARY_EVENT, WINDOW_MOTION_REST } from "./window-boot.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/* A constant of the tree's module, read from its source the way the harness
 * reads the attach limits: a number written twice is a number that drifts.
 * NaN when the module does not have it, which its check then says. */
const treeConstant = async (name) => {
  const source = await readFile(resolve(UI, "shell-explorer-tree.js"), "utf8");
  return Number(source.match(new RegExp(`const ${name} = ([\\d_]+);`))?.[1]?.replace(/_/g, ""));
};

/* The synthetic checkout every suite here stands in. Three levels, so a file
 * can sit under a folder that is itself under a folder. */
const LISTINGS = {
  "": [
    { name: "docs", is_dir: true },
    { name: "src", is_dir: true },
    { name: "README.md", is_dir: false },
  ],
  src: [
    { name: "deep", is_dir: true },
    { name: "lib.rs", is_dir: false },
    { name: "main.rs", is_dir: false },
  ],
  "src/deep": [{ name: "x.rs", is_dir: false }],
  docs: [{ name: "guide.md", is_dir: false }],
};

/* What every case starts from: the files panel up, the listing above, the
 * tree loaded at its root. Helpers the page-side cases share are hung on
 * `window.__XT__` so each case reads as what it does. */
async function standTree(page, listings = LISTINGS) {
  await page.evaluate(async (listed) => {
    window.__ANSWER__.list_dir = ({ path }) => listed[path] ?? [];
    revealActivity("files", "name");
    fileSearch.blur();
    resetTreeSelection();
    await loadTree(fileTree, "");
    const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
    let seq = 0;
    window.__XT__ = {
      root: activeWorktreePath,
      row: (relative) => fileTree.querySelector(`.tree-row[data-tree-path="${CSS.escape(relative)}"]`),
      fire: (pane, activities) => {
        for (const handler of window.__LISTENERS__["hook:activity"] ?? []) handler({ payload: { pane, activities } });
      },
      // `extra` is what a road adds beside the three fields every agent says —
      // the call's id (`call`), the folder it ran in (`cwd`).
      act: (verb, target, phase = "started", extra = {}) => ({ seq: ++seq, activity: { verb, ...(target === null ? {} : { target }), phase, ...extra } }),
      // Two frames and the tasks between them: a batch is painted on the
      // frame after it lands, and an unfold waits on its listing.
      settle: async (rounds = 4) => {
        for (let at = 0; at < rounds; at += 1) {
          await frame();
          await new Promise((done) => setTimeout(done, 0));
        }
      },
      open: (relative) => window.__XT__.row(relative)?.getAttribute("aria-expanded") === "true",
      touch: (relative) => window.__XT__.row(relative)?.dataset.agentTouch ?? null,
      within: (relative) => window.__XT__.row(relative)?.dataset.agentTouchWithin ?? null,
      marked: () => [...fileTree.querySelectorAll("[data-agent-touch], [data-agent-touch-within]")].map((row) => row.dataset.treePath),
    };
  }, listings);
}

/* ---- slice 1: what the agents touch ------------------------------------- */

export async function testExplorerAgentActivity(browser, origin, ok) {
  const hold = await treeConstant("TREE_TOUCH_HOLD_MS");
  ok("the time a touched file stays lit is one named constant of the tree's module", Number.isFinite(hold) && hold > 0, String(hold));
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async () => {
      const { root, fire, act, settle, open, touch, within, marked } = window.__XT__;
      const out = {};
      const listed = () => window.__COUNTS__.list_dir ?? 0;
      // An edit names its file absolutely — Claude, Cursor and Copilot do.
      // Follow is on by default: the tree unfolds to the file and the file
      // wears the write colour.
      fire("term:1", [act("edit", `${root}/src/deep/x.rs`)]);
      await settle(6);
      out.followed = open("src") && open("src/deep");
      out.written = touch("src/deep/x.rs");
      // A read wears its own colour.
      fire("term:1", [act("read", `${root}/README.md`)]);
      await settle();
      out.read = touch("README.md");
      // Outside the checkout — another project, a path climbing out — is
      // nobody's business here, and costs no listing.
      const before = listed();
      fire("term:1", [act("read", "/tmp/elsewhere/a.rs"), act("read", `${root}/../escape/a.rs`), act("grep", "fn main")]);
      await settle();
      out.outsideIgnored = marked().every((path) => ["src/deep/x.rs", "README.md", "src", "src/deep"].includes(path)) && listed() === before;
      // A write that failed wrote nothing.
      fire("term:1", [act("write", `${root}/docs/guide.md`, "failed")]);
      await settle();
      out.failedUnmarked = touch("docs/guide.md") === null && !open("docs");
      // A relative path is the pane's: resolved from where its process
      // stands (`term:cwd`), inside the pane's own workspace.
      mountTermTab(9871, { worktree: root }, { placement: "tab", focus: false });
      for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9871, cwd: `${root}/src` } });
      fire("term:9871", [act("read", "lib.rs")]);
      // A helper's card names its parent's pane, and is resolved the same way.
      fire("sub:9871:a-1", [act("read", "main.rs")]);
      await settle();
      out.relative = touch("src/lib.rs") === "read" && touch("src/main.rs") === "read";
      // A pane nobody knows has no place to resolve a relative path from.
      fire("term:31337", [act("read", "docs/guide.md")]);
      await settle();
      out.unknownPaneIgnored = touch("docs/guide.md") === null;
      // Follow off: the view stays where the person left it, and the colour
      // still shows — on the collapsed folder that holds the file.
      const toggle = document.getElementById("tree-follow");
      toggle?.click();
      out.toggle = toggle ? { pressed: toggle.getAttribute("aria-pressed"), label: toggle.getAttribute("aria-label") } : null;
      fire("term:1", [act("write", `${root}/docs/guide.md`)]);
      await settle();
      out.stayedClosed = !open("docs");
      out.folderWears = within("docs");
      toggle?.click();
      out.toggleBack = toggle?.getAttribute("aria-pressed") ?? null;
      return out;
    });
    ok("an edit an agent makes unfolds the tree to its file (follow on by default) and the file wears the write colour", seen.followed === true && seen.written === "write", JSON.stringify(seen));
    ok("a file an agent reads wears the read colour", seen.read === "read", JSON.stringify(seen));
    ok("a path outside the active checkout, a path climbing out of it and a search pattern light nothing and cost no listing", seen.outsideIgnored === true, JSON.stringify(seen));
    ok("a tool call that failed lights nothing and unfolds nothing", seen.failedUnmarked === true, JSON.stringify(seen));
    ok("a relative path is resolved from the pane's own working folder, for the pane and its helpers alike; an unknown pane resolves nothing", seen.relative === true && seen.unknownPaneIgnored === true, JSON.stringify(seen));
    ok("with follow off the tree keeps its view and the collapsed folder wears its file's colour", seen.toggle?.pressed === "false" && seen.toggle?.label?.length > 0 && seen.stayedClosed === true && seen.folderWears === "write" && seen.toggleBack === "true", JSON.stringify(seen));

    const colours = await page.evaluate(async () => {
      const { root, fire, act, settle, row } = window.__XT__;
      const prior = document.documentElement.dataset.theme;
      // Both kinds a tool call can say on screen at once; the commit colour
      // is read off its token beside them.
      fire("term:1", [act("read", `${root}/src/lib.rs`), act("write", `${root}/src/main.rs`)]);
      await settle();
      const rgb = (value) => (value.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
      const luminance = (values) => values.map((v) => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [.2126, .7152, .0722][i], 0);
      const ground = (node) => {
        for (let at = node; at; at = at.parentElement) {
          const paint = getComputedStyle(at).backgroundColor;
          if (paint && !/rgba\(.*,\s*0\)$/.test(paint) && paint !== "transparent") return paint;
        }
        return getComputedStyle(document.body).backgroundColor;
      };
      const out = [];
      for (const theme of ["dark", "light"]) {
        document.documentElement.dataset.theme = theme;
        await new Promise((done) => setTimeout(done, 250));
        // A row the tree never reached reads as no colour at all, so a
        // window without the feature fails these checks rather than throwing.
        const ink = (relative) => {
          const name = row(relative)?.querySelector(".tree-name");
          return name ? getComputedStyle(name).color : "";
        };
        const read = ink("src/lib.rs");
        const write = ink("src/main.rs");
        const plain = ink("src/deep");
        const back = luminance(rgb(ground(row("src/main.rs") ?? fileTree)));
        const ratio = (colour) => {
          const pair = [luminance(rgb(colour)), back].sort((a, b) => b - a);
          return (pair[0] + .05) / (pair[1] + .05);
        };
        const tokens = getComputedStyle(document.documentElement);
        out.push({
          theme, read, write, plain, readRatio: ratio(read), writeRatio: ratio(write),
          tokens: ["--tree-touch-read", "--tree-touch-write", "--tree-touch-commit"].map((name) => tokens.getPropertyValue(name).trim()),
        });
      }
      document.documentElement.dataset.theme = prior;
      return out;
    });
    ok("the read, write and commit colours are theme tokens, each different, readable on the panel in both themes", colours.every((one) => one.tokens.every(Boolean) && new Set(one.tokens).size === 3 && one.read !== one.write && one.read !== one.plain && one.write !== one.plain && one.readRatio >= 4.5 && one.writeRatio >= 4.5) && colours[0].tokens.join() !== colours[1].tokens.join(), JSON.stringify(colours));

    const faded = await page.evaluate(async (wait) => {
      const { root, fire, act, settle, marked } = window.__XT__;
      fire("term:1", [act("read", `${root}/README.md`)]);
      await settle();
      const lit = marked().length;
      await new Promise((done) => setTimeout(done, Number.isFinite(wait) ? wait : 0));
      await settle();
      return { lit, left: marked() };
    }, hold + 400);
    ok("the colours fade once the hold has passed", Number.isFinite(hold) && faded.lit > 0 && faded.left.length === 0, JSON.stringify(faded));

    const motion = await page.evaluate(async () => {
      const { root, fire, act, settle, row } = window.__XT__;
      fire("term:1", [act("read", `${root}/README.md`)]);
      await settle();
      const style = getComputedStyle(row("README.md"));
      return { moving: style.transitionDuration };
    });
    await page.emulateMedia({ reducedMotion: "reduce" });
    const still = await page.evaluate(() => getComputedStyle(window.__XT__.row("README.md")).transitionDuration);
    await page.emulateMedia({ reducedMotion: "no-preference" });
    ok("the fade moves, and stands still under reduced motion", motion.moving.split(",").some((part) => parseFloat(part) > 0) && still.split(",").every((part) => parseFloat(part) === 0), JSON.stringify({ ...motion, still }));
    ok("the activity suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* The cost of a busy agent on a big tree: 400 files open on screen, one
 * burst of 200 activity events across them. Counted, not felt — paints,
 * listings and git calls during the burst, and the milliseconds the burst's
 * handlers and its paint took. Printed as `EXPLORER_AGENT_NUMBERS` beside the
 * checks, so a before/after is two log lines. */
export async function testExplorerAgentBurst(browser, origin, ok) {
  const hold = await treeConstant("TREE_TOUCH_HOLD_MS");
  const windowMs = await treeConstant("TREE_NUMSTAT_WINDOW_MS");
  const listings = { "": [] };
  for (let dir = 0; dir < 20; dir += 1) {
    const name = `d${String(dir).padStart(2, "0")}`;
    listings[""].push({ name, is_dir: true });
    listings[name] = [];
    for (let file = 0; file < 20; file += 1) listings[name].push({ name: `f${String(file).padStart(2, "0")}.rs`, is_dir: false });
  }
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page, listings);
    const numbers = await page.evaluate(async () => {
      const { root, fire, act, settle } = window.__XT__;
      for (const row of [...fileTree.querySelectorAll(":scope > .tree-row.is-dir")]) await row._treeUnfold(true);
      await settle();
      const files = fileTree.querySelectorAll(".tree-row.is-file").length;
      const counts = () => ({ ...window.__COUNTS__ });
      const before = counts();
      const paintsBefore = typeof treeTouchPaints === "number" ? treeTouchPaints : NaN;
      let frames = 0;
      let counting = true;
      const tick = () => { if (!counting) return; frames += 1; requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
      const started = performance.now();
      for (let at = 0; at < 200; at += 1) {
        const dir = `d${String(at % 20).padStart(2, "0")}`;
        const file = `f${String(Math.floor(at / 20) * 2 % 20).padStart(2, "0")}.rs`;
        fire("term:1", [act(at % 3 === 0 ? "edit" : "read", `${root}/${dir}/${file}`, "started")]);
      }
      const handled = performance.now() - started;
      await settle(3);
      const settled = performance.now() - started;
      counting = false;
      const after = counts();
      const delta = (command) => (after[command] ?? 0) - (before[command] ?? 0);
      return {
        files,
        handledMs: Math.round(handled * 10) / 10,
        settledMs: Math.round(settled * 10) / 10,
        frames,
        paints: (typeof treeTouchPaints === "number" ? treeTouchPaints : NaN) - paintsBefore,
        listDir: delta("list_dir"),
        git: delta("scm_status") + delta("upstream_status") + delta("git_history"),
        lit: fileTree.querySelectorAll("[data-agent-touch]").length,
      };
    });
    console.log(`EXPLORER_AGENT_NUMBERS ${JSON.stringify(numbers)}`);
    ok("a burst of 200 activity events on a 400-file tree paints at most once a frame, lists nothing already listed and runs no git", numbers.files >= 400 && numbers.lit > 0 && numbers.paints >= 1 && numbers.paints <= numbers.frames && numbers.listDir === 0 && numbers.git === 0, JSON.stringify(numbers));

    /* The same tree, 200 start/end pairs (t-31715): what live progress costs a
     * busy agent. Counted the way a low-spec machine feels it — paints per
     * frame, the frames the page's own work made late (an animation-frame gap
     * past one 60 Hz budget), the handlers' milliseconds — and what it asks of
     * git: one scoped question per window, never a whole-repo status. Printed as
     * `EXPLORER_AGENT_PAIR_NUMBERS`, so a before and an after are two log lines
     * (and `WINDOW_CPU_THROTTLE=4` is the low-spec profile). */
    const pairs = await page.evaluate(async ({ hold, windowMs }) => {
      const { root, fire, act, settle } = window.__XT__;
      const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
      // The first burst's marks fade before this one is measured.
      await sleep(hold + 800);
      await settle(3);
      window.__ANSWER__.scm_numstat = ({ paths }) => paths.map((path) => ({ path, code: " M", staged: false, changed: true, added: 2, removed: 1, conflict: null, origin: null }));
      const counts = () => ({ ...window.__COUNTS__ });
      const before = counts();
      const paintsBefore = typeof treeTouchPaints === "number" ? treeTouchPaints : NaN;
      // A 60 Hz frame that fit its budget lands ~16.7 ms after the last; a gap
      // past this is a frame the page's own work made late.
      const LONG_FRAME_GAP_MS = 20;
      let frames = 0;
      let longFrames = 0;
      let worstGapMs = 0;
      let last = performance.now();
      let counting = true;
      const tick = (stamp) => {
        if (!counting) return;
        frames += 1;
        const gap = stamp - last;
        last = stamp;
        if (gap > LONG_FRAME_GAP_MS) longFrames += 1;
        worstGapMs = Math.max(worstGapMs, gap);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      let longTasks = 0;
      const observer = new PerformanceObserver((list) => { longTasks += list.getEntries().length; });
      try { observer.observe({ type: "longtask" }); } catch { /* an engine with no long-task entries counts none */ }
      const started = performance.now();
      for (let at = 0; at < 200; at += 1) {
        const dir = `d${String(at % 20).padStart(2, "0")}`;
        const file = `f${String(Math.floor(at / 20) * 2 % 20).padStart(2, "0")}.rs`;
        const target = `${root}/${dir}/${file}`;
        fire("term:1", [act("edit", target, "started", { call: `burst${at}` })]);
        fire("term:1", [act("edit", target, "finished", { call: `burst${at}` })]);
      }
      const handled = performance.now() - started;
      await settle(3);
      const settled = performance.now() - started;
      const shown = fileTree.querySelectorAll("[data-agent-writing]").length;
      await sleep(windowMs + 500);
      counting = false;
      observer.disconnect();
      const after = counts();
      const delta = (command) => (after[command] ?? 0) - (before[command] ?? 0);
      const tallied = [...fileTree.querySelectorAll(".tree-row.is-file .tree-tally")].filter((tally) => tally.textContent.includes("+2")).length;
      // Nothing writing and nothing held: no animation runs and no timer of the
      // tree's is armed — the cost of an idle tree is zero.
      await sleep(hold + 1500);
      const idle = {
        animations: fileTree.getAnimations({ subtree: true }).length,
        timers: [typeof treeTouchSweep === "undefined" ? "missing" : treeTouchSweep, typeof treeNumstatTimer === "undefined" ? "missing" : treeNumstatTimer, typeof treeVcsSweep === "undefined" ? "missing" : treeVcsSweep].filter((timer) => timer !== null).length,
      };
      return {
        handledMs: Math.round(handled * 10) / 10,
        settledMs: Math.round(settled * 10) / 10,
        frames,
        longFrames,
        longTasks,
        worstGapMs: Math.round(worstGapMs),
        paints: (typeof treeTouchPaints === "number" ? treeTouchPaints : NaN) - paintsBefore,
        shown,
        tallied,
        numstat: delta("scm_numstat"),
        wholeRepo: delta("scm_status") + delta("upstream_status") + delta("git_history"),
        listDir: delta("list_dir"),
        idle,
      };
    }, { hold, windowMs });
    console.log(`EXPLORER_AGENT_PAIR_NUMBERS ${JSON.stringify(pairs)}`);
    ok("a burst of 200 start/end pairs shows the writing state, paints at most once a frame, asks git one scoped question per window and never a whole-repo status", pairs.shown > 0 && pairs.paints >= 1 && pairs.paints <= pairs.frames && pairs.numstat >= 1 && pairs.numstat <= Math.ceil((pairs.settledMs + windowMs + 500) / windowMs) && pairs.wholeRepo === 0 && pairs.listDir === 0 && pairs.tallied > 0, JSON.stringify(pairs));
    ok("an idle tree runs no animation and holds no timer once the writing has ended and the marks have faded", pairs.idle.animations === 0 && pairs.idle.timers === 0, JSON.stringify(pairs.idle));
    ok("the burst suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 2: git on the rows ---------------------------------------------
 *
 * The numbers `scm_status` already carries (numstat, per file) sit on the
 * tree's rows; a folder rolls up what is beneath it by one stated priority and
 * counts each kind; the head says the branch and where it stands against its
 * upstream; and the badges are re-read while the TREE is on screen, settled
 * on the agents' beat exactly like the source-control panel. */
const STATUS = [
  { path: "README.md", code: "A ", staged: true, changed: false, added: 5, removed: 0, conflict: null, origin: null },
  { path: "docs/guide.md", code: "UU", staged: false, changed: true, added: null, removed: null, conflict: "both-modified", origin: null },
  { path: "src/deep/x.rs", code: "??", staged: false, changed: true, added: null, removed: null, conflict: null, origin: null },
  { path: "src/lib.rs", code: " D", staged: false, changed: true, added: 0, removed: 12, conflict: null, origin: null },
  { path: "src/main.rs", code: " M", staged: false, changed: true, added: 3, removed: 1, conflict: null, origin: null },
];

export async function testExplorerGit(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async (status) => {
      const { row, settle } = window.__XT__;
      window.__ANSWER__.scm_status = () => ({ changed: status, ignored: [] });
      window.__ANSWER__.upstream_status = () => ({ upstream: "origin/main", ahead: 2, behind: 1, behind_commits_are_patch_equivalent: null });
      setActivityItem("files");
      // The moment the tree asks git: the agents' settled beat and the window
      // coming back both land here (the panel's own refresh is the other).
      refreshScmIfShowing();
      await new Promise((done) => setTimeout(done, 80));
      await loadTree(fileTree, "");
      await row("src")._treeUnfold(true);
      await row("src/deep")._treeUnfold(true);
      await row("docs")._treeUnfold(true);
      await settle();
      const tally = (relative) => row(relative)?.querySelector(".tree-tally")?.textContent ?? null;
      const chips = (relative) => [...(row(relative)?.querySelectorAll(".badge .tree-count") ?? [])].map((chip) => chip.textContent);
      const git = (relative) => row(relative)?.querySelector(".badge")?.dataset.git ?? null;
      const head = document.getElementById("tree-head");
      return {
        mainTally: tally("src/main.rs"),
        libTally: tally("src/lib.rs"),
        newTally: tally("src/deep/x.rs"),
        readmeTally: tally("README.md"),
        src: { git: git("src"), chips: chips("src") },
        deep: { git: git("src/deep"), chips: chips("src/deep") },
        docs: { git: git("docs"), chips: chips("docs") },
        conflictFile: git("docs/guide.md"),
        head: head ? { text: head.textContent.replace(/\s+/g, " ").trim(), hidden: head.hidden } : null,
      };
    }, STATUS);
    ok("a modified file says +N -N from the numstat scm_status already carries; a file without counts says nothing", seen.mainTally?.includes("+3") && seen.mainTally?.includes("-1") && seen.libTally === "-12" && !seen.newTally && seen.readmeTally === "+5", JSON.stringify(seen));
    ok("a folder rolls up conflict > deleted > modified > added > untracked and counts each kind beneath it", seen.src.git === "deleted" && JSON.stringify(seen.src.chips) === JSON.stringify(["D1", "M1", "U1"]) && seen.deep.git === "untracked" && JSON.stringify(seen.deep.chips) === JSON.stringify(["U1"]) && seen.docs.git === "conflict" && JSON.stringify(seen.docs.chips) === JSON.stringify(["!1"]) && seen.conflictFile === "conflict", JSON.stringify(seen));
    ok("the tree's head says the branch, its upstream and how far ahead and behind it stands", seen.head?.hidden === false && ["main", "origin/main", "↑2", "↓1"].every((word) => seen.head.text.includes(word)), JSON.stringify(seen.head));

    const refresh = await page.evaluate(async (status) => {
      const { row } = window.__XT__;
      const counts = () => ({ ...window.__COUNTS__ });
      const delta = (before, command) => (window.__COUNTS__[command] ?? 0) - (before[command] ?? 0);
      const moved = status.map((entry) => entry.path === "src/main.rs" ? { ...entry, added: 7, removed: 2 } : entry);
      window.__ANSWER__.scm_status = () => ({ changed: moved, ignored: [] });
      // The files panel is the one up, source control is not: an agent's
      // beat must still reach the tree's badges — settled, once per burst.
      setActivityItem("files");
      const before = counts();
      for (let at = 0; at < 20; at += 1) noteScmMayHaveChanged();
      await new Promise((done) => setTimeout(done, SCM_SETTLE_MS + 400));
      const seen = {
        scm: delta(before, "scm_status"),
        upstream: delta(before, "upstream_status"),
        listed: delta(before, "list_dir"),
        tally: row("src/main.rs")?.querySelector(".tree-tally")?.textContent ?? null,
        stillOpen: row("src")?.getAttribute("aria-expanded"),
      };
      // A folded column is nobody looking: nothing is asked.
      setPanelFolded("aside", true);
      const folded = counts();
      noteScmMayHaveChanged();
      await new Promise((done) => setTimeout(done, SCM_SETTLE_MS + 400));
      seen.foldedAsks = delta(folded, "scm_status");
      setPanelFolded("aside", false);
      return seen;
    }, STATUS);
    ok("while the tree is on screen an agent's burst re-reads git once, settled, and repaints the badges in place without listing the tree again", refresh.scm === 1 && refresh.upstream <= 1 && refresh.listed === 0 && refresh.tally?.includes("+7") && refresh.stillOpen === "true" && refresh.foldedAsks === 0, JSON.stringify(refresh));
    ok("the git suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 3: the person's own words reveal the tree ----------------------
 *
 * An `@path` in a prompt the person sent (the prompt event every hook agent
 * reports) unfolds the tree to that file, follow or not — it is the person's
 * own pointer, not the agent's. The source-control row's menu shares the
 * same reveal. */
export async function testExplorerMentions(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async () => {
      const { root, fire, act, settle, open, row } = window.__XT__;
      const out = {};
      mountTermTab(9872, { worktree: root }, { placement: "tab", focus: false });
      document.getElementById("tree-follow")?.click();
      out.followOff = document.getElementById("tree-follow")?.getAttribute("aria-pressed") ?? null;
      fire("term:9872", [act("prompt", "what changed in @src/deep/x.rs, and in @/etc/hosts or @../outside.rs?", "prompted")]);
      await settle(8);
      out.revealed = open("src") && open("src/deep") && row("src/deep/x.rs")?.classList.contains("is-revealed");
      out.outsideIgnored = !fileTree.querySelector(".tree-row.is-revealed:not([data-tree-path='src/deep/x.rs'])");
      document.getElementById("tree-follow")?.click();
      // The source-control row's menu offers the same reveal as its last row
      // (the original's own order: its last row reveals in its file panel).
      await row("src")._treeUnfold(false);
      window.__ANSWER__.scm_status = () => ({ changed: [{ path: "src/main.rs", code: " M", staged: false, changed: true, added: 1, removed: 0, conflict: null, origin: null }], ignored: [] });
      setActivityItem("scm");
      await refreshScm();
      const changed = [...document.querySelectorAll("#activity-scm .scm-row")].find((one) => one.textContent.includes("main.rs"));
      changed?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 60, clientY: 60 }));
      await settle();
      const items = [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")];
      const offered = items.find((item) => item.textContent.includes(t("sourceControl.revealInTree", "파일 트리에서 보기")));
      out.offered = Boolean(offered) && items.at(-1) === offered;
      if (offered) offered.click();
      else closeSidebarMenu();
      await settle(8);
      out.menuRevealed = !document.getElementById("activity-files").hidden && open("src") && row("src/main.rs")?.classList.contains("is-revealed") && document.activeElement === row("src/main.rs");
      return out;
    });
    ok("an @path in the person's prompt unfolds the tree to that file even with follow off; paths outside the checkout are ignored", seen.followOff === "false" && seen.revealed === true && seen.outsideIgnored === true, JSON.stringify(seen));
    ok("the source-control row's menu ends with the same reveal: the files panel comes up and the row is revealed and focused", seen.offered === true && seen.menuRevealed === true, JSON.stringify(seen));
    ok("the mentions suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 4: the keyboard walks the tree --------------------------------- */
export async function testExplorerKeys(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const roles = await page.evaluate(async (status) => {
      const { row } = window.__XT__;
      window.__ANSWER__.scm_status = () => ({ changed: status, ignored: [] });
      await refreshScm();
      await loadTree(fileTree, "");
      return {
        tree: fileTree.getAttribute("role"),
        owner: fileTree.dataset.keyboardOwner ?? null,
        item: row("src")?.getAttribute("role"),
        level: row("src")?.getAttribute("aria-level"),
        stops: [...fileTree.querySelectorAll(".tree-row")].filter((one) => one.tabIndex === 0).length,
        label: row("README.md")?.getAttribute("aria-label") ?? "",
        // The word in whatever language the page reads in.
        added: t("tree.git.added", "추가됨"),
      };
    }, STATUS);
    ok("the tree is a tree to a screen reader: role tree, treeitems with levels, one tab stop, names that say the git state", roles.tree === "tree" && roles.owner === "true" && roles.item === "treeitem" && roles.level === "1" && roles.stops === 1 && roles.label.includes("README.md") && roles.label.includes(roles.added), JSON.stringify(roles));

    await page.evaluate(() => window.__XT__.row("docs").focus());
    const at = () => page.evaluate(() => document.activeElement?.dataset?.treePath ?? document.activeElement?.id ?? null);
    const steps = {};
    await page.keyboard.press("ArrowDown");
    steps.down = await at();
    await page.keyboard.press("ArrowRight");
    await page.evaluate(() => window.__XT__.settle());
    steps.opened = await page.evaluate(() => window.__XT__.open("src"));
    await page.keyboard.press("ArrowRight");
    steps.child = await at();
    await page.keyboard.press("ArrowDown");
    steps.next = await at();
    steps.selected = await page.evaluate(() => [...selectedTreePaths]);
    await page.keyboard.press("ArrowLeft");
    steps.parent = await at();
    await page.keyboard.press("ArrowLeft");
    steps.closed = await page.evaluate(() => !window.__XT__.open("src"));
    await page.keyboard.press("End");
    steps.end = await at();
    await page.keyboard.press("Home");
    steps.home = await at();
    steps.focusVisible = await page.evaluate(() => {
      const row = document.activeElement;
      const style = getComputedStyle(row);
      return row.matches(":focus-visible") && (style.outlineStyle !== "none" || style.boxShadow !== "none");
    });
    // Enter on a file opens it — the row's own click, through the keyboard.
    await page.keyboard.press("End");
    await page.evaluate(() => {
      window.__XT__.opened = [];
      window.__XT__.keptOpenPath = openPath;
      openPath = (path, options) => { window.__XT__.opened.push([path, options?.preview ?? null]); };
    });
    await page.keyboard.press("Enter");
    steps.entered = await page.evaluate(() => {
      openPath = window.__XT__.keptOpenPath;
      return window.__XT__.opened;
    });
    const opens = await page.evaluate(async () => {
      const { row, root, settle } = window.__XT__;
      const counted = window.__COUNTS__.fs_open_default ?? 0;
      let asked = null;
      window.__ANSWER__.fs_open_default = (args) => { asked = args.path; return null; };
      row("README.md").dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
      await settle();
      return { asked, root, calls: (window.__COUNTS__.fs_open_default ?? 0) - counted };
    });
    steps.keySinkKept = await page.evaluate(() => document.activeElement !== keySink);
    ok("arrow keys walk the visible rows and carry the selection; right opens then enters a folder, left climbs then closes; Home and End reach the ends; Enter opens a file; focus shows", steps.down === "src" && steps.opened === true && steps.child === "src/deep" && steps.next === "src/lib.rs" && JSON.stringify(steps.selected) === JSON.stringify(["src/lib.rs"]) && steps.parent === "src" && steps.closed === true && steps.end === "README.md" && steps.home === "docs" && steps.focusVisible === true && JSON.stringify(steps.entered) === JSON.stringify([["README.md", true]]) && steps.keySinkKept === true, JSON.stringify(steps));
    ok("a double-click opens the file in the system's default app", opens.calls === 1 && opens.asked === `${opens.root}/README.md`, JSON.stringify(opens));
    ok("the keys suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 6: the tree's root — the agent's folder, or one the person pins
 *
 * The tree is the active workspace's; inside it, an agent's pane that stands
 * in a folder (`term:cwd`) has that folder revealed and marked while follow
 * is on. And the person can pin any folder as the tree's root: the tree
 * shows that folder alone until it is unpinned, every reload keeps it, and
 * what happens outside it is not drawn. */
export async function testExplorerRoot(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async () => {
      const { root, fire, act, settle, open, row } = window.__XT__;
      const out = {};
      const cwd = (term, path) => {
        for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term, cwd: path } });
      };
      // An agent's pane in this workspace moves into a folder: the tree
      // unfolds to it and marks it as where the agent stands.
      mountTermTab(9873, { agent: "claude", worktree: root }, { placement: "tab", focus: false });
      paneAgents.set(9873, "claude");
      cwd(9873, `${root}/src/deep`);
      await settle(8);
      out.cwdRevealed = open("src") && row("src/deep")?.dataset.agentCwd === "true";
      cwd(9873, root);
      await settle();
      out.cwdCleared = !fileTree.querySelector("[data-agent-cwd]");
      // Pinning a folder from its own menu.
      row("src").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
      await settle();
      const pin = [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")]
        .find((item) => item.textContent.includes(t("tree.menu.pinRoot", "트리 루트로 고정")));
      out.offered = Boolean(pin);
      if (pin) pin.click();
      else closeSidebarMenu();
      await settle(8);
      const top = () => [...fileTree.querySelectorAll(":scope > .tree-row")].map((one) => one.dataset.treePath);
      out.pinnedTop = top();
      out.levels = row("src/lib.rs")?.getAttribute("aria-level") ?? null;
      const chip = document.getElementById("tree-pin");
      out.chip = chip ? { hidden: chip.hidden, text: chip.textContent.replace(/\s+/g, " ").trim() } : null;
      // Every reload keeps the pin — the many roads that reload the tree
      // all ask for its root.
      await loadTree(fileTree, "");
      out.reloadKept = JSON.stringify(top()) === JSON.stringify(out.pinnedTop);
      // What happens outside the pinned folder is not drawn, and costs no
      // listing; inside it is.
      const listed = window.__COUNTS__.list_dir ?? 0;
      fire("term:1", [act("read", `${root}/docs/guide.md`)]);
      await settle();
      out.outsideQuiet = (window.__COUNTS__.list_dir ?? 0) === listed && !fileTree.querySelector("[data-agent-touch]");
      fire("term:1", [act("edit", `${root}/src/deep/x.rs`)]);
      await settle(6);
      out.insideLit = row("src/deep/x.rs")?.dataset.agentTouch === "write";
      // Unpinned from the head: the workspace's root is back.
      document.getElementById("tree-unpin")?.click();
      await settle(6);
      out.unpinnedTop = top();
      out.chipGone = document.getElementById("tree-pin")?.hidden ?? null;
      return out;
    });
    ok("an agent's pane standing in a folder of this workspace has it revealed and marked, and the mark leaves when it steps back to the root", seen.cwdRevealed === true && seen.cwdCleared === true, JSON.stringify(seen));
    ok("a folder's menu pins it as the tree's root: its rows stand at the top, levels count from it, the head says the pin, and every reload keeps it", seen.offered === true && JSON.stringify(seen.pinnedTop) === JSON.stringify(["src/deep", "src/lib.rs", "src/main.rs"]) && seen.levels === "1" && seen.chip?.hidden === false && seen.chip.text.includes("src") && seen.reloadKept === true, JSON.stringify(seen));
    ok("with a pinned root, work outside it is not drawn and costs no listing, work inside it is; unpinning brings the workspace back", seen.outsideQuiet === true && seen.insideLit === true && JSON.stringify(seen.unpinnedTop) === JSON.stringify(["docs", "src", "README.md"]) && seen.chipGone === true, JSON.stringify(seen));
    ok("the root suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 5: what an agent is doing in git, under the tree ---------------
 *
 * A shell call that runs git or gh carries its operations (`activity.vcs`,
 * read by the backend with the shell's grammar). While it runs, one line
 * under the tree says so in git's own words; it is gone when the call
 * finishes or fails, or the turn ends. A commit that finished lights the
 * files it took in the commit colour once git has been asked again. Only
 * this workspace's panes speak here. */
export async function testExplorerVcs(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async () => {
      const { root, fire, settle, open, row } = window.__XT__;
      const out = {};
      const line = () => {
        const host = document.getElementById("tree-ops");
        return host ? { hidden: host.hidden, text: host.textContent.replace(/\s+/g, " ").trim(), role: host.getAttribute("role") } : null;
      };
      const command = "git add -A && git commit -m 'tree: wear the agents'";
      const vcs = [{ tool: "git", verb: "add" }, { tool: "git", verb: "commit" }];
      let seq = 100;
      const bash = (phase, target, steps = vcs) => ({ seq: ++seq, activity: { verb: "bash", target, phase, vcs: steps } });
      mountTermTab(9874, { agent: "claude", worktree: root }, { placement: "tab", focus: false });
      mountTermTab(9875, { agent: "codex", worktree: "/tmp/zerocode-window-test-elsewhere" }, { placement: "tab", focus: false });
      // The files git holds as changed before the commit.
      window.__ANSWER__.scm_status = () => ({ changed: [
        { path: "README.md", code: " M", staged: false, changed: true, added: 1, removed: 0, conflict: null, origin: null },
        { path: "src/main.rs", code: " M", staged: false, changed: true, added: 2, removed: 1, conflict: null, origin: null },
      ], ignored: [] });
      setActivityItem("files");
      await refreshScm();
      await row("src")._treeUnfold(true);
      fire("term:9874", [bash("started", command)]);
      await settle();
      out.running = line();
      // Another workspace's pane is that workspace's news.
      fire("term:9875", [bash("started", "git push origin HEAD", [{ tool: "git", verb: "push" }])]);
      await settle();
      out.othersQuiet = !(line()?.text ?? "").includes("git push");
      // The commit lands: the call finishes, git is asked again and no longer
      // holds src/main.rs as changed — it was taken into the commit.
      fire("term:9874", [bash("finished", command)]);
      await settle();
      out.cleared = line()?.hidden ?? null;
      window.__ANSWER__.scm_status = () => ({ changed: [
        { path: "README.md", code: " M", staged: false, changed: true, added: 1, removed: 0, conflict: null, origin: null },
      ], ignored: [] });
      await refreshScm();
      await settle();
      out.committed = row("src/main.rs")?.dataset.agentTouch ?? null;
      out.untouched = row("README.md")?.dataset.agentTouch ?? null;
      // A failed operation clears the line too, and so does a turn's end.
      fire("term:9874", [bash("started", "gh pr create --fill", [{ tool: "gh", verb: "pr create" }])]);
      await settle();
      out.pr = line()?.text ?? null;
      fire("term:9874", [bash("failed", "gh pr create --fill", [{ tool: "gh", verb: "pr create" }])]);
      await settle();
      out.failedCleared = line()?.hidden ?? null;
      fire("term:9874", [bash("started", "git pull --rebase", [{ tool: "git", verb: "pull" }])]);
      await settle();
      fire("term:9874", [{ seq: ++seq, activity: { verb: "stop", phase: "stopped" } }]);
      await settle();
      out.stoppedCleared = line()?.hidden ?? null;
      return out;
    });
    ok("a running git operation is one status line under the tree, in git's own words, for this workspace's panes only", seen.running?.hidden === false && seen.running.role === "status" && seen.running.text.includes("git add") && seen.running.text.includes("git commit") && seen.othersQuiet === true, JSON.stringify(seen));
    ok("the line clears when the operation finishes, fails or the turn ends", seen.cleared === true && seen.pr?.includes("gh pr create") && seen.failedCleared === true && seen.stoppedCleared === true, JSON.stringify(seen));
    ok("a finished commit lights the files it took in the commit colour once git has been asked again", seen.committed === "commit" && seen.untouched === null, JSON.stringify(seen));
    ok("the vcs suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 3, second half: the selection is the agent's context ----------
 *
 * Whatever the person has selected in the tree is said to the backend as it
 * moves — once per frame, however fast the arrows go — so the next prompt in
 * this workspace's panes can carry it (the brief the prompt hook takes). */
export async function testExplorerSelection(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    const seen = await page.evaluate(async (primary) => {
      const { root, row, settle } = window.__XT__;
      const said = [];
      window.__ANSWER__.tree_selection = (args) => { said.push(JSON.parse(JSON.stringify(args))); return null; };
      // A plain click also opens the file's preview; this suite is about the
      // selection, so the preview door is held shut while it clicks.
      const keptOpenPath = openPath;
      openPath = () => {};
      const click = (relative, extra = {}) => row(relative).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ...extra }));
      click("README.md");
      await settle();
      const one = said.at(-1) ?? null;
      click("src", primary);
      await settle();
      const two = said.at(-1) ?? null;
      // Ten moves inside one frame are one word to the backend.
      const before = said.length;
      for (let at = 0; at < 10; at += 1) click(at % 2 ? "README.md" : "docs");
      await settle();
      const burst = said.length - before;
      // Letting go of the selection says so too.
      resetTreeSelection();
      await settle();
      const none = said.at(-1) ?? null;
      openPath = keptOpenPath;
      return { root, one, two, burst, none };
    }, PRIMARY_EVENT);
    ok("the tree's selection is said to the backend as it moves, relative to its workspace, once a frame", seen.one?.root === seen.root && JSON.stringify(seen.one?.paths) === JSON.stringify(["README.md"]) && JSON.stringify([...(seen.two?.paths ?? [])].sort()) === JSON.stringify(["README.md", "src"]) && seen.burst === 1 && JSON.stringify(seen.none?.paths) === JSON.stringify([]), JSON.stringify(seen));
    ok("the selection suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 8: what an agent is writing right now (t-31715) --------------------
 *
 * The person tried the tree of 1.1.50 and said its marks were there but nothing
 * in it was *happening*. Live progress, and only that: while a write or edit
 * tool call is open — it has started and not finished — its file's row, and
 * every collapsed folder above it, shimmers; the moment the call ends the
 * file's +N -N lands on the row, from one scoped git question for every file
 * written in a short window (never one per event, never a whole-repo status);
 * and all of it is the same for every agent, because the tree reads one shape —
 * `hook:activity`'s — in which a start and its end are paired by the call's id,
 * or, where an agent gives none, by its verb and target. Hook agents, wire
 * sessions (`wire:<n>`) and zo's main pane all arrive on that shape; this suite
 * drives it the way each arrives.
 *
 * Nothing here calls a function the feature added: on a window without it,
 * every check fails at its assertion. */
export async function testExplorerWriting(browser, origin, ok) {
  const [max, min, windowMs, most] = await Promise.all(["TREE_WRITING_MAX_MS", "TREE_WRITING_MIN_MS", "TREE_NUMSTAT_WINDOW_MS", "TREE_NUMSTAT_PATHS_MAX"].map(treeConstant));
  ok("the writing mark's lost-end bound, its shortest showing, the numstat window and the most paths one question names are named constants of the tree's module", [max, min, windowMs, most].every((one) => Number.isFinite(one) && one > 0) && min < max, JSON.stringify({ max, min, windowMs, most }));
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standTree(page);
    // What the backend's scoped question answers: the asked files, each
    // modified, with the counts below.
    await page.evaluate(() => {
      window.__XT__.asked = [];
      window.__ANSWER__.scm_numstat = (args) => {
        window.__XT__.asked.push([...args.paths]);
        return args.paths.map((path) => ({ path, code: " M", staged: false, changed: true, added: 3, removed: 1, conflict: null, origin: null }));
      };
      window.__XT__.sleep = (ms) => new Promise((done) => setTimeout(done, ms));
      window.__XT__.writing = (relative) => window.__XT__.row(relative)?.dataset.agentWriting ?? null;
      window.__XT__.sweeping = (relative) => {
        const row = window.__XT__.row(relative);
        return row ? row.getAnimations({ subtree: true }).filter((one) => one instanceof CSSAnimation && one.animationName === "tree-writing-sweep" && one.playState === "running").length : 0;
      };
    });

    /* a hook agent in Claude's shape: the call's id on both ends */
    const claude = await page.evaluate(async ({ windowMs, min }) => {
      const { root, fire, act, settle, row, open, sleep, writing, sweeping } = window.__XT__;
      const file = "src/deep/x.rs";
      const asking = () => window.__COUNTS__.scm_numstat ?? 0;
      const out = {};
      fire("term:1", [act("edit", `${root}/${file}`, "started", { call: "toolu_a" })]);
      await settle(6);
      out.writing = writing(file);
      out.followed = open("src") && open("src/deep");
      out.sweeping = sweeping(file);
      const word = t("tree.writing.file", "");
      out.said = word.length > 0 && (row(file)?.getAttribute("aria-label") ?? "").includes(word);
      out.askedBeforeEnd = asking();
      // The end arrives in the same batch the start did: the mark stays for its
      // shortest showing, so an edit that took 20 ms is still seen to happen.
      fire("term:1", [act("edit", `${root}/${file}`, "finished", { call: "toolu_a" })]);
      await settle(2);
      out.heldByMin = writing(file);
      await sleep(windowMs + 500);
      out.asked = window.__XT__.asked.map((paths) => paths.join(","));
      out.tally = row(file)?.querySelector(".tree-tally")?.textContent ?? null;
      out.letter = row(file)?.querySelector(".badge")?.textContent ?? null;
      out.chips = ["src/deep", "src"].map((folder) => [...(row(folder)?.querySelectorAll(".badge .tree-count") ?? [])].map((chip) => chip.textContent).join(""));
      await sleep(min + 500);
      out.cleared = writing(file) === null && sweeping(file) === 0;
      return out;
    }, { windowMs, min });
    ok("a write that has started and not ended shimmers its file's row, in words for a screen reader, and costs git nothing until it ends", claude.writing === "true" && claude.sweeping === 1 && claude.said === true && claude.askedBeforeEnd === 0 && claude.followed === true, JSON.stringify(claude));
    ok("a call's end lands its +N -N on the row, its letter and its folders' roll-ups from one scoped question, and the mark leaves after its shortest showing", claude.heldByMin === "true" && JSON.stringify(claude.asked) === JSON.stringify(["src/deep/x.rs"]) && claude.tally?.includes("+3") && claude.tally?.includes("-1") && claude.letter === "M" && JSON.stringify(claude.chips) === JSON.stringify(["M1", "M1"]) && claude.cleared === true, JSON.stringify(claude));

    /* with follow off the folders stay shut and wear the state themselves */
    const folders = await page.evaluate(async ({ min }) => {
      const { root, fire, act, settle, row, open, sleep } = window.__XT__;
      const toggle = document.getElementById("tree-follow");
      toggle.click();
      fire("term:1", [act("write", `${root}/docs/guide.md`, "started", { call: "toolu_b" })]);
      await settle();
      const folder = row("docs");
      const out = {
        within: folder?.dataset.agentWritingWithin ?? null,
        stayedClosed: !open("docs"),
        sweeping: folder ? folder.getAnimations({ subtree: true }).filter((one) => one instanceof CSSAnimation && one.animationName === "tree-writing-sweep").length : 0,
        said: (() => { const word = t("tree.writing.within", ""); return word.length > 0 && (folder?.getAttribute("aria-label") ?? "").includes(word); })(),
      };
      fire("term:1", [act("write", `${root}/docs/guide.md`, "finished", { call: "toolu_b" })]);
      await sleep(min + 600);
      out.cleared = (row("docs")?.dataset.agentWritingWithin ?? null) === null;
      toggle.click();
      return out;
    }, { min });
    ok("with follow off the collapsed folder above a file being written shimmers for it, says so in words and stays shut; it clears with the call", folders.within === "true" && folders.stayedClosed === true && folders.sweeping === 1 && folders.said === true && folders.cleared === true, JSON.stringify(folders));

    /* an agent that gives no call id: start and end meet by verb and target */
    const plain = await page.evaluate(async ({ min }) => {
      const { root, fire, act, settle, sleep, writing } = window.__XT__;
      const out = {};
      fire("term:2", [act("edit", `${root}/README.md`, "started"), act("write", `${root}/src/lib.rs`, "started")]);
      await settle(6);
      out.both = [writing("README.md"), writing("src/lib.rs")];
      fire("term:2", [act("edit", `${root}/README.md`, "finished")]);
      await sleep(min + 500);
      out.afterOne = [writing("README.md"), writing("src/lib.rs")];
      // An end that names no file closes the pane's oldest open call of that verb.
      fire("term:2", [act("write", null, "finished")]);
      await sleep(min + 500);
      out.afterTwo = writing("src/lib.rs");
      return out;
    }, { min });
    ok("without a call id an end closes the call with its verb and target, and an end naming nothing closes the pane's oldest open call of that verb", JSON.stringify(plain.both) === JSON.stringify(["true", "true"]) && JSON.stringify(plain.afterOne) === JSON.stringify([null, "true"]) && plain.afterTwo === null, JSON.stringify(plain));

    /* a helper's card with no id is a snapshot of what it does now: nothing closes it;
     * the same card with a call id is an event stream like any pane's */
    const helper = await page.evaluate(async ({ min }) => {
      const { root, fire, act, settle, row, sleep, writing } = window.__XT__;
      fire("sub:1:h9", [act("edit", `${root}/src/main.rs`, "started")]);
      await settle(6);
      const out = { touched: row("src/main.rs")?.dataset.agentTouch ?? null, snapshot: writing("src/main.rs") };
      fire("sub:1:h9", [act("edit", `${root}/src/main.rs`, "started", { call: "h9-1" })]);
      await settle(6);
      out.withId = writing("src/main.rs");
      fire("sub:1:h9", [act("edit", `${root}/src/main.rs`, "finished", { call: "h9-1" })]);
      await sleep(min + 500);
      out.closed = writing("src/main.rs") === null;
      return out;
    }, { min });
    ok("a helper's activity with no call id lights its file but never opens a write nothing would close; with a call id its writes are paired like any pane's", helper.touched === "write" && helper.snapshot === null && helper.withId === "true" && helper.closed === true, JSON.stringify(helper));

    /* the turn ending, a failed call, an end that never came, an end whose start was missed */
    const ends = await page.evaluate(async ({ windowMs, min, max }) => {
      const { root, fire, act, settle, sleep, writing } = window.__XT__;
      const asking = () => window.__COUNTS__.scm_numstat ?? 0;
      const out = {};
      await sleep(windowMs + 300);
      let before = asking();
      fire("term:3", [act("edit", `${root}/src/main.rs`, "started", { call: "c3" })]);
      await settle(6);
      out.openedForStop = writing("src/main.rs");
      fire("term:3", [{ seq: 900, activity: { verb: "stop", phase: "stopped" } }]);
      await sleep(min + windowMs + 500);
      out.stopCleared = writing("src/main.rs") === null;
      out.stopAsked = asking() - before;
      before = asking();
      fire("term:3", [act("edit", `${root}/README.md`, "started", { call: "f1" })]);
      await settle(6);
      fire("term:3", [act("edit", `${root}/README.md`, "failed", { call: "f1" })]);
      await sleep(min + windowMs + 500);
      out.failedCleared = writing("README.md") === null;
      out.failedAsked = asking() - before;
      before = asking();
      fire("term:3", [act("edit", `${root}/docs/guide.md`, "finished", { call: "orphan" })]);
      await sleep(windowMs + 500);
      out.orphanAsked = asking() - before;
      out.orphanNamed = window.__XT__.asked.at(-1)?.join(",") ?? null;
      // An end that never comes: the bound closes the call and asks git once.
      before = asking();
      fire("term:3", [act("edit", `${root}/src/lib.rs`, "started", { call: "lost1" })]);
      await settle(6);
      out.openedForBound = writing("src/lib.rs");
      const real = performance.now.bind(performance);
      performance.now = () => real() + max + 1000;
      try { sweepTreeTouches(); } finally { performance.now = real; }
      await settle(3);
      out.boundCleared = writing("src/lib.rs") === null;
      await sleep(windowMs + 500);
      out.boundAsked = asking() - before;
      return out;
    }, { windowMs, min, max });
    ok("the turn's end closes what it left open and asks git once; a failed call clears the mark and asks nothing; an end whose start was missed still asks for its file", ends.openedForStop === "true" && ends.stopCleared === true && ends.stopAsked === 1 && ends.failedCleared === true && ends.failedAsked === 0 && ends.orphanAsked === 1 && ends.orphanNamed === "docs/guide.md", JSON.stringify(ends));
    ok("a call whose end never comes is let go at its bound — the mark clears and git is asked once for what it may have written", ends.openedForBound === "true" && ends.boundCleared === true && ends.boundAsked === 1, JSON.stringify(ends));

    /* a wire session: its own pane key, its folder in `cwd`, its end naming no file */
    const wire = await page.evaluate(async ({ windowMs, min }) => {
      const { root, fire, act, settle, sleep, writing } = window.__XT__;
      const out = {};
      const before = window.__COUNTS__.scm_numstat ?? 0;
      fire("wire:7", [act("edit", "src/lib.rs", "started", { call: "item_1", cwd: root })]);
      await settle(6);
      out.writing = writing("src/lib.rs");
      out.notFiled = typeof paneActivities === "object" && !paneActivities.has("wire:7");
      fire("wire:7", [act("edit", null, "finished", { call: "item_1" })]);
      await sleep(min + windowMs + 500);
      out.ended = writing("src/lib.rs") === null;
      out.asked = (window.__COUNTS__.scm_numstat ?? 0) - before;
      out.named = window.__XT__.asked.at(-1)?.join(",") ?? null;
      return out;
    }, { windowMs, min });
    ok("a wire session's writes follow start to end the same way: its folder is its `cwd`, its end needs no file of its own, and it fills no sidebar card", wire.writing === "true" && wire.notFiled === true && wire.ended === true && wire.asked === 1 && wire.named === "src/lib.rs", JSON.stringify(wire));

    /* a burst: 200 pairs in one tick, 40 files — one question for the window, 40 files in it */
    const burst = await page.evaluate(async ({ windowMs, most }) => {
      const { root, fire, act, settle, sleep } = window.__XT__;
      const out = {};
      await sleep(windowMs + 300);
      const before = { ...window.__COUNTS__ };
      const askedBefore = window.__XT__.asked.length;
      for (let at = 0; at < 200; at += 1) {
        const target = `${root}/src/burst${at % 40}.rs`;
        fire("term:5", [act("edit", target, "started", { call: `b${at}` })]);
        fire("term:5", [act("edit", target, "finished", { call: `b${at}` })]);
      }
      await settle(3);
      await sleep(windowMs * 3 + 600);
      const delta = (command) => (window.__COUNTS__[command] ?? 0) - (before[command] ?? 0);
      const questions = window.__XT__.asked.slice(askedBefore);
      out.questions = questions.length;
      out.files = new Set(questions.flat()).size;
      out.biggest = Math.max(0, ...questions.map((paths) => paths.length));
      out.repeats = questions.flat().length - out.files;
      out.wholeRepo = delta("scm_status") + delta("upstream_status");
      out.listed = delta("list_dir");
      // More files than one question may name wait for the next window, in order.
      const spill = 2 * most + 6;
      const spillBefore = window.__XT__.asked.length;
      for (let at = 0; at < spill; at += 1) fire("term:5", [act("edit", `${root}/src/spill${at}.rs`, "finished", { call: `s${at}` })]);
      await sleep(windowMs * 5 + 600);
      const spilled = window.__XT__.asked.slice(spillBefore);
      out.spillQuestions = spilled.length;
      out.spillFiles = new Set(spilled.flat()).size;
      out.spillBiggest = Math.max(0, ...spilled.map((paths) => paths.length));
      out.spill = spill;
      return out;
    }, { windowMs, most });
    ok("a burst of 200 start/end pairs asks git one scoped question about each file once — never per event, never a whole-repo status, no listing", burst.questions === 1 && burst.files === 40 && burst.repeats === 0 && burst.wholeRepo === 0 && burst.listed === 0, JSON.stringify(burst));
    ok("more files than one question may name are asked in the following windows, none dropped and none named twice", burst.spillBiggest <= most && burst.spillFiles === burst.spill && burst.spillQuestions >= 3, JSON.stringify(burst));

    /* held writes are bounded however many calls start and never end */
    const held = await page.evaluate(() => {
      const { root, fire, act } = window.__XT__;
      for (let at = 0; at < 300; at += 1) fire("term:6", [act("edit", `${root}/src/many${at}.rs`, "started", { call: `many${at}` })]);
      return { writes: typeof treeWrites === "object" ? treeWrites.size : NaN, cap: typeof TREE_TOUCH_CAP === "number" ? TREE_TOUCH_CAP : NaN };
    });
    ok("the writes held open are bounded by the touch cap however many calls start and never end", Number.isFinite(held.writes) && held.writes <= held.cap, JSON.stringify(held));

    /* the sheen is a theme token and the name stays readable under its peak */
    const sheen = await page.evaluate(async () => {
      const prior = document.documentElement.dataset.theme;
      const channels = (value) => (value.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
      const luminance = (values) => values.map((v) => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [.2126, .7152, .0722][i], 0);
      const ratio = (a, b) => { const pair = [luminance(a), luminance(b)].sort((x, y) => y - x); return (pair[0] + .05) / (pair[1] + .05); };
      const resolved = (property, value) => {
        const probe = document.createElement("span");
        probe.style[property] = value;
        document.body.append(probe);
        const read = getComputedStyle(probe)[property];
        probe.remove();
        return channels(read);
      };
      const out = [];
      for (const theme of ["dark", "light"]) {
        document.documentElement.dataset.theme = theme;
        await new Promise((done) => setTimeout(done, 250));
        const tokens = getComputedStyle(document.documentElement);
        const raw = tokens.getPropertyValue("--tree-writing-sheen").trim();
        const share = Number(raw.match(/(\d+(?:\.\d+)?)%/)?.[1]) / 100;
        const ink = resolved("color", tokens.getPropertyValue("--tree-touch-write"));
        const ratios = ["--surface-deck", "--surface-well"].map((name) => {
          const ground = resolved("backgroundColor", tokens.getPropertyValue(name));
          return ratio(ink, ground.map((value, at) => value * (1 - share) + ink[at] * share));
        });
        out.push({ theme, raw, share, ratios: ratios.map((one) => Math.round(one * 100) / 100), sweepMs: parseFloat(tokens.getPropertyValue("--tree-writing-sweep")) });
      }
      document.documentElement.dataset.theme = prior;
      return out;
    });
    ok("the sheen is a theme token — its own in each theme — and the written name stays 4.5:1 readable under its peak on both grounds; one sweep lasts at least the shortest showing", sheen.length === 2 && sheen.every((one) => one.share > 0 && one.share < .5 && one.ratios.every((ratio) => ratio >= 4.5) && one.sweepMs >= min) && sheen[0].raw !== sheen[1].raw, JSON.stringify(sheen));

    /* reduced motion: nothing moves, a static marker stands, the words are the same */
    // Nothing writing from here on, whatever the cases above left open.
    await page.evaluate(() => { if (typeof treeWrites === "object") treeWrites.clear(); });
    await page.emulateMedia({ reducedMotion: "reduce" });
    const still = await page.evaluate(async () => {
      const { root, fire, act, settle, row, writing, sweeping } = window.__XT__;
      fire("term:7", [act("edit", `${root}/README.md`, "started", { call: "rm1" })]);
      await settle(6);
      const row0 = row("README.md");
      const mark = row0 ? getComputedStyle(row0, "::after") : null;
      const word = t("tree.writing.file", "");
      return {
        writing: writing("README.md"),
        sweeping: sweeping("README.md"),
        animationName: mark?.animationName ?? null,
        marker: mark ? { content: mark.content, display: mark.display, height: parseFloat(mark.height) } : null,
        said: word.length > 0 && (row0?.getAttribute("aria-label") ?? "").includes(word),
      };
    });
    await page.emulateMedia({ reducedMotion: WINDOW_MOTION_REST });
    ok("under reduced motion the writing state does not move, a static marker stands in its place, and the words are the same", still.writing === "true" && still.sweeping === 0 && still.animationName === "none" && still.marker?.content !== "none" && still.marker?.display !== "none" && still.marker?.height > 0 && still.said === true, JSON.stringify(still));

    /* the keyboard: the row that holds focus names what is happening to it */
    await page.evaluate(() => window.__XT__.row("README.md").focus());
    // README.md is the tree's last row: up to its neighbour and back, by keyboard.
    await page.keyboard.press("ArrowUp");
    await page.keyboard.press("ArrowDown");
    const focus = await page.evaluate(() => {
      const row = document.activeElement;
      const word = t("tree.writing.file", "");
      return { path: row?.dataset?.treePath ?? null, visible: row?.matches?.(":focus-visible") ?? false, said: word.length > 0 && (row?.getAttribute?.("aria-label") ?? "").includes(word) };
    });
    ok("a focused row that is being written shows its focus ring and says what is happening to it in words", focus.path === "README.md" && focus.visible === true && focus.said === true, JSON.stringify(focus));
    ok("the writing suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* ---- slice 8, memory: a long run grows nothing (t-31715) -----------------------
 *
 * An agent works for hours: thousands of calls, many of whose ends never reach
 * the window. Everything the tree holds for them — the open writes, the files
 * waiting for git, the rows it dressed — is bounded by a cap, so the heap after
 * a long run, with garbage collected, is the heap before it. Measured over CDP
 * (`HeapProfiler.collectGarbage`, `Runtime.getHeapUsage`: exact, where the
 * page's own `performance.memory` is bucketed) and printed as
 * `EXPLORER_AGENT_MEMORY`, so a before and an after are two log lines. */
export async function testExplorerAgentMemory(browser, origin, ok) {
  const listings = { "": [] };
  for (let dir = 0; dir < 20; dir += 1) {
    const name = `d${String(dir).padStart(2, "0")}`;
    listings[""].push({ name, is_dir: true });
    listings[name] = [];
    for (let file = 0; file < 20; file += 1) listings[name].push({ name: `f${String(file).padStart(2, "0")}.rs`, is_dir: false });
  }
  const windowMs = await treeConstant("TREE_NUMSTAT_WINDOW_MS");
  const { page, faults } = await openWindowTestPage(browser, origin);
  const cdp = await page.context().newCDPSession(page);
  const heap = async () => {
    await cdp.send("HeapProfiler.collectGarbage");
    await cdp.send("HeapProfiler.collectGarbage");
    return (await cdp.send("Runtime.getHeapUsage")).usedSize;
  };
  try {
    await standTree(page, listings);
    await page.evaluate(async () => {
      const { settle } = window.__XT__;
      window.__ANSWER__.scm_numstat = ({ paths }) => paths.map((path) => ({ path, code: " M", staged: false, changed: true, added: 2, removed: 1, conflict: null, origin: null }));
      for (const row of [...fileTree.querySelectorAll(":scope > .tree-row.is-dir")]) await row._treeUnfold(true);
      await settle();
    });
    // One round: 200 calls that end, 200 that never do — each with an id nobody
    // has used — over the 400 files, then a breath.
    const round = (rounds, windowMs) => page.evaluate(async ({ rounds, windowMs }) => {
      const { root, fire, act } = window.__XT__;
      const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
      let calls = window.__MEMORY_CALLS__ ?? 0;
      for (let at = 0; at < rounds; at += 1) {
        for (let one = 0; one < 200; one += 1) {
          const dir = `d${String(calls % 20).padStart(2, "0")}`;
          const file = `f${String(Math.floor(calls / 20) % 20).padStart(2, "0")}.rs`;
          const target = `${root}/${dir}/${file}`;
          fire("term:1", [act("edit", target, "started", { call: `mem${calls}` })]);
          if (one % 2 === 0) fire("term:1", [act("edit", target, "finished", { call: `mem${calls}` })]);
          calls += 1;
        }
        await sleep(windowMs + 40);
      }
      window.__MEMORY_CALLS__ = calls;
      return calls;
    }, { rounds, windowMs });
    // Warm: the first rounds build what any long run keeps (rows dressed, maps filled).
    await round(5, windowMs);
    const before = await heap();
    const calls = await round(50, windowMs);
    const after = await heap();
    const held = await page.evaluate(() => ({
      writes: typeof treeWrites === "object" ? treeWrites.size : NaN,
      wanted: typeof treeNumstatWanted === "object" ? treeNumstatWanted.size : NaN,
      dressed: typeof treeDressedRows === "object" ? treeDressedRows.size : NaN,
      touches: treeTouches.size,
      cap: typeof TREE_TOUCH_CAP === "number" ? TREE_TOUCH_CAP : NaN,
      activityRing: [...paneActivities.values()].reduce((sum, list) => sum + list.length, 0),
    }));
    const growthKiB = Math.round((after - before) / 1024);
    const numbers = { calls, beforeKiB: Math.round(before / 1024), afterKiB: Math.round(after / 1024), growthKiB, ...held };
    console.log(`EXPLORER_AGENT_MEMORY ${JSON.stringify(numbers)}`);
    ok("what the tree holds for a long run is bounded by its caps: the open writes, the files waiting for git, the rows dressed and the window's ring", held.writes <= held.cap && held.wanted <= held.cap * 8 && held.dressed <= held.cap * 8 && held.touches <= held.cap && held.activityRing <= 20 * 4, JSON.stringify(numbers));
    ok("fifty rounds of four hundred calls — half of them never ended — leave the heap, garbage collected, where it was (growth under 2 MiB)", Number.isFinite(growthKiB) && growthKiB < 2048 && held.writes <= held.cap, JSON.stringify(numbers));
    ok("the memory suite raised no renderer faults", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
