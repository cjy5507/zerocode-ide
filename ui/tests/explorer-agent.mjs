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
import { openWindowTestPage, PRIMARY_EVENT } from "./window-boot.mjs";

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
      act: (verb, target, phase = "started") => ({ seq: ++seq, activity: { verb, ...(target === null ? {} : { target }), phase } }),
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
