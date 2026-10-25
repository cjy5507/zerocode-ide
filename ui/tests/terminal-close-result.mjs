import { openWindowTestPage } from "./window-boot.mjs";

const frame = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));

/* A scene's wait with a deadline: one that never ends fails its scene instead
 * of holding the suite (t-75153). */
const within = (promise, what, ms = 10_000) => Promise.race([
  promise,
  new Promise((_, refuse) => setTimeout(() => refuse(new Error(`${what} did not end within ${ms} ms`)), ms)),
]);

async function scene(browser, origin, split) {
  const opened = await openWindowTestPage(browser, origin);
  await opened.page.evaluate(async (split) => {
    confirmCloseRunning = false;
    confirmClosePinnedTab = false;
    window.__NEXT_TERM__ = Math.max(window.__NEXT_TERM__ ?? 0, 91_000);
    const first = await openTermTab({ placement: "tab", door: "terminal" });
    const tab = tabOfTerm(first);
    if (split) await splitActivePane("row");
    window.__CLOSE_SCENE__ = { tab: tab.id, first, going: activePaneOf(tab), calls: [] };
    window.__CLOSE_VIEW__ = viewOfTerm(window.__CLOSE_SCENE__.going);
  }, split);
  return opened;
}

const inspect = (page) => page.evaluate(() => {
  const scene = window.__CLOSE_SCENE__;
  const tab = tabs.find((one) => one.id === scene.tab);
  return {
    exists: !!tab,
    leaves: tab ? paneLeaves(tab.layout) : [],
    view: !!viewOfTerm(scene.going),
    sameView: viewOfTerm(scene.going) === window.__CLOSE_VIEW__,
    firstView: !!viewOfTerm(scene.first),
    calls: [...scene.calls],
    going: scene.going,
    first: scene.first,
    notice: document.querySelector(".toast[data-kind=halt]")?.textContent ?? "",
  };
});

export async function testTerminalCloseResult(browser, origin, ok) {
  for (const road of ["tab", "pane"]) {
    for (const kind of ["journal_unavailable", "journal_failed", "signal_failed", "unconfirmed", "replaced", "ipc"]) {
      const { page, faults } = await scene(browser, origin, road === "pane");
      try {
        await page.evaluate(({ road, kind }) => {
          window.__ANSWER__.close_term = ({ term }) => {
            window.__CLOSE_SCENE__.calls.push(term);
            if (kind === "ipc") throw new Error("synthetic IPC refusal");
            throw { kind };
          };
          const held = window.__CLOSE_SCENE__;
          if (road === "tab") closeTab(held.tab);
          else void closePaneLeaf(tabs.find((one) => one.id === held.tab), held.going);
        }, { road, kind });
        await frame(page);
        const failed = await inspect(page);
        ok(`failed ${road} close keeps its live screen (${kind})`,
          failed.exists && failed.leaves.includes(failed.going) && failed.sameView,
          JSON.stringify(failed));
        ok(`failed ${road} close says why and can be retried (${kind})`,
          failed.notice.length > 0 && !failed.notice.includes("[object Object]"), JSON.stringify(failed));
        if (failed.exists && failed.leaves.includes(failed.going)) {
          await page.evaluate((road) => {
            window.__ANSWER__.close_term = ({ term }) => (window.__CLOSE_SCENE__.calls.push(term), null);
            const held = window.__CLOSE_SCENE__;
            if (road === "tab") closeTab(held.tab);
            else void closePaneLeaf(tabs.find((one) => one.id === held.tab), held.going);
          }, road);
          await frame(page);
          const retried = await inspect(page);
          ok(`confirmed retry removes only the closed ${road}`,
            !retried.view && !retried.leaves.includes(retried.going) &&
              (road === "tab" ? !retried.exists : retried.firstView), JSON.stringify(retried));
        }
        ok(`close scene has no unhandled script error (${road}, ${kind})`, faults.length === 0, faults.join("\n"));
      } finally { await page.close(); }
    }
  }

  const { page, faults } = await scene(browser, origin, true);
  try {
    await page.evaluate(() => {
      window.__ANSWER__.close_term = ({ term }) => {
        window.__CLOSE_SCENE__.calls.push(term);
        if (term === window.__CLOSE_SCENE__.going) throw { kind: "signal_failed" };
        return null;
      };
      closeTab(window.__CLOSE_SCENE__.tab);
    });
    await frame(page);
    const partial = await inspect(page);
    ok("a partly closed tab keeps only its failed pane and its screen",
      partial.exists && partial.sameView && !partial.firstView &&
        partial.leaves.length === 1 && partial.leaves[0] === partial.going,
      JSON.stringify(partial));
    ok("partial close has no unhandled script error", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }

  const pending = await scene(browser, origin, false);
  try {
    await pending.page.evaluate(() => {
      window.__ANSWER__.close_term = ({ term }) => {
        window.__CLOSE_SCENE__.calls.push(term);
        return new Promise((resolve) => { window.__CLOSE_REPLY__ = resolve; });
      };
      closeTab(window.__CLOSE_SCENE__.tab);
      closeTab(window.__CLOSE_SCENE__.tab);
    });
    await frame(pending.page);
    const before = await inspect(pending.page);
    ok("an unanswered close keeps the screen and coalesces repeat clicks",
      before.exists && before.sameView && before.calls.length === 1, JSON.stringify(before));
    await pending.page.evaluate(() => window.__CLOSE_REPLY__(null));
    await frame(pending.page);
    const after = await inspect(pending.page);
    ok("the screen leaves when its close receipt arrives", !after.exists && !after.view, JSON.stringify(after));
    const lateExit = await pending.page.evaluate(() => {
      const term = window.__CLOSE_SCENE__.going;
      for (const hear of window.__LISTENERS__["term:exited"] ?? []) hear({ payload: { term } });
      return exitedUnheld.has(term);
    });
    ok("a late exit does not retain a closed terminal as an unmounted restore", !lateExit);
  } finally { await pending.page.close(); }

  const race = await scene(browser, origin, false);
  try {
    await race.page.evaluate(() => {
      window.__ANSWER__.close_term = () => new Promise((resolve, reject) => { window.__CLOSE_REJECT__ = reject; });
      closeTab(window.__CLOSE_SCENE__.tab);
    });
    await frame(race.page);
    await race.page.evaluate(() => {
      const term = window.__CLOSE_SCENE__.going;
      for (const hear of window.__LISTENERS__["term:exited"] ?? []) hear({ payload: { term } });
      window.__CLOSE_REJECT__({ kind: "journal_failed" });
    });
    await frame(race.page);
    const gone = await inspect(race.page);
    ok("an observed native exit wins over a late failed close reply",
      !gone.exists && !gone.view && gone.notice === "", JSON.stringify(gone));
  } finally { await race.page.close(); }

  const removal = await scene(browser, origin, true);
  try {
    const kept = await removal.page.evaluate(async () => {
      const path = activeWorktreePath;
      const tree = stageTree();
      stageTrees.set(path, tree);
      window.__ANSWER__.close_term = () => { throw { kind: "journal_unavailable" }; };
      const closed = await dropWorktreeSurfaces(path);
      return { closed, treeKept: stageTrees.get(path) === tree };
    });
    const screen = await inspect(removal.page);
    ok("worktree surface cleanup keeps failed shells and their stage",
      kept.closed === false && kept.treeKept && screen.sameView && screen.firstView,
      JSON.stringify({ kept, screen }));
  } finally { await removal.page.close(); }

  const project = await scene(browser, origin, false);
  try {
    const kept = await project.page.evaluate(async () => {
      const home = activeProjectPath;
      projects = [...projects, { ...projects[0], path: "/tmp/terminal-close-other", name: "other", worktrees: [] }];
      let removed = 0;
      window.__ANSWER__.remove_project = () => { removed += 1; return null; };
      window.__ANSWER__.close_term = () => { throw { kind: "signal_failed" }; };
      const previousAsk = askConfirm;
      askConfirm = async () => true;
      try { await removeProjectFromList(home); }
      finally { askConfirm = previousAsk; }
      return { removed, stillListed: projects.some((one) => one.path === home), sameProject: activeProjectPath === home };
    });
    const screen = await inspect(project.page);
    ok("project removal keeps its catalog entry and retry screen when a shell cannot close",
      kept.removed === 0 && kept.stillListed && kept.sameProject && screen.sameView,
      JSON.stringify({ kept, screen }));
  } finally { await project.page.close(); }

  const mirror = await scene(browser, origin, true);
  try {
    const opened = await mirror.page.evaluate(async () => {
      const scene = window.__CLOSE_SCENE__;
      mirrorPanes.set(`${scene.first}:codex`, scene.going);
      let opened = 0;
      window.__ANSWER__.open_mirror_term = () => { opened += 1; return ++window.__NEXT_TERM__; };
      window.__ANSWER__.close_mirror_term = () => { throw { kind: "unconfirmed" }; };
      await openMirrorPane(scene.first, "codex", "/tmp/terminal-close-synthetic.ansi");
      return opened;
    });
    const screen = await inspect(mirror.page);
    ok("a refused mirror replacement keeps the old screen and opens no extra process",
      opened === 0 && screen.sameView && screen.leaves.length === 2, JSON.stringify(screen));
  } finally { await mirror.page.close(); }

  const stored = await scene(browser, origin, false);
  try {
    const result = await stored.page.evaluate(async () => {
      const tab = openStoredTab({ id: 811, root: { type: "leaf" }, agents: {}, running: {} }, activeWorktreePath, "restore");
      let calls = 0;
      window.__ANSWER__.close_term = () => { calls += 1; return null; };
      closeTab(tab.id);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return { closed: !tabs.includes(tab), calls };
    });
    ok("a stored tab with no process closes without a native termination request", result.closed && result.calls === 0, JSON.stringify(result));
  } finally { await stored.page.close(); }

  const wakingScene = await scene(browser, origin, false);
  try {
    await wakingScene.page.evaluate(() => {
      const tab = openStoredTab({ id: 812, root: { type: "leaf" }, agents: {}, running: {} }, activeWorktreePath, "restore");
      window.__WAKING_TAB__ = tab;
      window.__WAKING_CLOSE_CALLS__ = [];
      window.__ANSWER__.close_term = ({ term }) => { window.__WAKING_CLOSE_CALLS__.push(term); throw { kind: "unconfirmed" }; };
      waking.set(tab.id, new Promise((resolve) => {
        window.__FINISH_CLOSE_WAKE__ = () => {
          const term = ++window.__NEXT_TERM__;
          tab.term = term;
          tab.layout = { type: "leaf", term };
          tab.activePane = term;
          delete tab.asleep;
          setActiveTab(tab.id);
          renderPanes(tab);
          waking.delete(tab.id);
          resolve();
        };
      }));
      closeTab(tab.id);
    });
    await frame(wakingScene.page);
    const waiting = await wakingScene.page.evaluate(() => ({ kept: tabs.includes(window.__WAKING_TAB__), calls: window.__WAKING_CLOSE_CALLS__.length }));
    ok("closing during a stored wake waits for its process identity", waiting.kept && waiting.calls === 0, JSON.stringify(waiting));
    await wakingScene.page.evaluate(() => window.__FINISH_CLOSE_WAKE__());
    await frame(wakingScene.page);
    const result = await wakingScene.page.evaluate(() => {
      const tab = window.__WAKING_TAB__;
      return { kept: tabs.includes(tab), screen: !!viewOfTerm(tab.term), calls: window.__WAKING_CLOSE_CALLS__ };
    });
    ok("a newly woken process still keeps its screen when termination is unconfirmed", result.kept && result.screen && result.calls.length === 1, JSON.stringify(result));
  } finally { await wakingScene.page.close(); }

  const late = await scene(browser, origin, true);
  try {
    await late.page.evaluate(() => {
      const home = activeProjectPath;
      const owned = activeWorktreePath;
      const other = "/tmp/terminal-close-other";
      projects = [...projects, { ...projects[0], path: other, name: "other", worktrees: [{ ...projects[0].worktrees[0], path: other, active: true }] }];
      window.__LATE_OWNER__ = { home, owned };
      activeProjectPath = other;
      activeWorktreePath = other;
      const mirror = ++window.__NEXT_TERM__;
      window.__LATE_TERM__ = mirror;
      window.__LATE_REMOVE_CALLS__ = 0;
      window.__LATE_CLOSE_CALLS__ = [];
      window.__ANSWER__.open_mirror_term = () => new Promise((resolve) => { window.__LATE_MIRROR_REPLY__ = () => resolve(mirror); });
      // The mirror is closed by the project's removal, through close_term.
      window.__ANSWER__.close_term = ({ term }) => {
        window.__LATE_CLOSE_CALLS__.push(term);
        if (term === mirror) throw { kind: "unconfirmed" };
        return null;
      };
      window.__ANSWER__.remove_project = () => {
        window.__LATE_REMOVE_CALLS__ += 1;
        return new Promise((resolve) => { window.__LATE_REMOVE_REPLY__ = () => { window.__LATE_REMOVED__ = true; resolve(null); }; });
      };
      window.__ANSWER__.project_catalog = () => window.__LATE_REMOVED__ ? projects.filter((one) => one.path !== home) : projects;
      window.__LATE_MIRROR_JOB__ = openMirrorPane(window.__CLOSE_SCENE__.first, "codex", "/tmp/terminal-close-late.ansi");
    });
    await frame(late.page);
    await late.page.evaluate(async () => {
      await closeTermAndView(window.__CLOSE_SCENE__.first, "test-parent-left");
      const previousAsk = askConfirm;
      askConfirm = async () => true;
      window.__LATE_REMOVE_JOB__ = removeProjectFromList(window.__LATE_OWNER__.home).finally(() => { askConfirm = previousAsk; });
    });
    await frame(late.page);
    const premature = await late.page.evaluate(() => window.__LATE_REMOVE_CALLS__);
    ok("project removal waits for a mirror whose native creation reply is still pending", premature === 0, String(premature));
    await late.page.evaluate(async () => {
      window.__LATE_MIRROR_REPLY__();
      await window.__LATE_MIRROR_JOB__;
      window.__LATE_REMOVE_REPLY__?.();
      await window.__LATE_REMOVE_JOB__;
    });
    const result = await late.page.evaluate(() => {
      const { home, owned } = window.__LATE_OWNER__;
      const tab = tabOfTerm(window.__LATE_TERM__);
      return {
        listed: projects.some((one) => one.path === home),
        removed: window.__LATE_REMOVE_CALLS__,
        retryTab: tab?.worktree === owned,
        closes: window.__LATE_CLOSE_CALLS__.filter((term) => term === window.__LATE_TERM__).length,
      };
    });
    ok("a late mirror close refusal preserves its project catalog and retry tab", result.listed && result.removed === 0 && result.retryTab && result.closes === 1, JSON.stringify(result));
  } finally { await late.page.close(); }

  /* The second wall, as the backend keeps it: `close_term` ends a shell only
   * on the person's hand (t-74975); a mirror pane — a `tail -F` the window
   * opened over a run's log — closes by `close_mirror_term` with no hand
   * (t-75153, R7b). */
  const wall = () => {
    window.__WALL__ = { closed: [], refused: [], mirrors: [] };
    window.__ANSWER__.close_term = ({ term }, options) => {
      const hand = options?.headers?.["zerocode-hand"] ?? null;
      if (hand !== "person") {
        window.__WALL__.refused.push([term, hand]);
        throw "close_term is the person's to run, and this call does not say the person made it";
      }
      window.__WALL__.closed.push(term);
      return null;
    };
    window.__ANSWER__.close_mirror_term = ({ term }) => {
      window.__WALL__.mirrors.push(term);
      return null;
    };
  };

  // A run's mirror replaced on the backend's word, no person's event in sight.
  const replaced = await scene(browser, origin, true);
  try {
    const result = await within(replaced.page.evaluate(async (wall) => {
      eval(`(${wall})`)();
      const scene = window.__CLOSE_SCENE__;
      mirrorPanes.set(`${scene.first}:codex`, scene.going);
      let opened = 0;
      window.__ANSWER__.open_mirror_term = () => { opened += 1; return ++window.__NEXT_TERM__; };
      await openMirrorPane(scene.first, "codex", "/tmp/terminal-close-replaced.ansi");
      const tab = tabs.find((one) => one.id === scene.tab);
      return { opened, wall: window.__WALL__, oldGone: !paneLeaves(tab.layout).includes(scene.going) };
    }, wall.toString()), "scene step 1");
    ok("a mirror replaced with no person's event closes the old mirror and opens the new one",
      result.opened === 1 && result.oldGone && result.wall.refused.length === 0
        && result.wall.mirrors.length === 1 && result.wall.closed.length === 0,
      JSON.stringify(result));
  } finally { await replaced.page.close(); }

  // The parent leaves while its mirror is still opening: the mirror goes with it.
  const orphan = await scene(browser, origin, false);
  try {
    const result = await within(orphan.page.evaluate(async (wall) => {
      eval(`(${wall})`)();
      const scene = window.__CLOSE_SCENE__;
      const mirror = ++window.__NEXT_TERM__;
      let reply;
      window.__ANSWER__.open_mirror_term = () => new Promise((done) => { reply = () => done(mirror); });
      const opening = openMirrorPane(scene.first, "codex", "/tmp/terminal-close-orphan.ansi");
      await new Promise((done) => setTimeout(done, 0));
      // The person closes the parent while the mirror is still being born.
      await closeTermAndView(scene.first, "test-parent-left", "person");
      reply();
      await opening;
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return { wall: window.__WALL__, mirror, left: !!tabOfTerm(mirror) };
    }, wall.toString()), "scene step 2");
    ok("a mirror whose parent left closes with no person's event",
      result.wall.mirrors.includes(result.mirror) && !result.left && result.wall.refused.length === 0,
      JSON.stringify(result));
  } finally { await orphan.page.close(); }

  // 「다른 탭 닫기」 over a document and a terminal: the document is let go of
  // first (an await), and the terminal still closes as the person's (R7b).
  const run = await scene(browser, origin, false);
  try {
    await within(run.page.evaluate(async (wall) => {
      eval(`(${wall})`)();
      window.__ANSWER__.read_text_file = () => ({ text: "# run", version: "1:1" });
      window.__ANSWER__.file_version = () => "1:1";
      await openFile("/tmp/terminal-close-run.md");
      const button = document.createElement("button");
      button.id = "close-run-probe";
      button.type = "button";
      button.textContent = "Close run";
      button.style.cssText = "position:fixed;left:8px;bottom:8px;z-index:99999";
      document.body.append(button);
      const term = tabs.find((one) => one.id === window.__CLOSE_SCENE__.tab);
      const doc = tabs.find((one) => one.id === "file:/tmp/terminal-close-run.md");
      button.addEventListener("click", () => void closeTabRun([doc, term]));
    }, wall.toString()), "scene step 3");
    await run.page.click("#close-run-probe");
    await run.page.waitForFunction(() => window.__WALL__.closed.length + window.__WALL__.refused.length > 0, null, { timeout: 5000 });
    const result = await within(run.page.evaluate(() => ({
      wall: window.__WALL__,
      termOpen: tabs.some((one) => one.id === window.__CLOSE_SCENE__.tab),
    })), "scene step 4");
    ok("closing a run of tabs ends its terminal as the person after letting go of a document",
      result.wall.closed.length === 1 && result.wall.refused.length === 0 && !result.termOpen, JSON.stringify(result));
  } finally { await run.page.close(); }

  // ⌘W held down: the key's auto-repeat is no new press, so one close only.
  const held = await scene(browser, origin, false);
  try {
    const modifier = await within(held.page.evaluate(async (wall) => {
      eval(`(${wall})`)();
      await openTermTab({ placement: "tab", door: "terminal" });
      // The harness ships ⌘W unbound; the person's default chord, back.
      delete keybindingOverrides["tab.close"];
      rebuildBound();
      return usesCommandModifier ? "Meta" : "Control";
    }, wall.toString()), "scene step 5");
    const before = await within(held.page.evaluate(() => tabs.filter((one) => one.kind === "term").length), "scene step 6");
    await held.page.keyboard.down(modifier);
    await held.page.keyboard.down("w");
    await held.page.keyboard.down("w");
    await held.page.keyboard.down("w");
    await held.page.keyboard.up("w");
    await held.page.keyboard.up(modifier);
    await frame(held.page);
    const result = await within(held.page.evaluate((before) => ({
      before,
      after: tabs.filter((one) => one.kind === "term").length,
      wall: window.__WALL__,
    }), before), "scene step 7");
    ok("a held close chord closes one terminal, not one per auto-repeat",
      result.before >= 2 && result.after === result.before - 1 && result.wall.closed.length === 1, JSON.stringify(result));
  } finally { await held.page.close(); }

  // Enter or Space held on a focused destructive button: the first keydown
  // presses it as the person; the repeats press nothing more (R7b).
  const pressed = await scene(browser, origin, false);
  try {
    await within(pressed.page.evaluate(() => {
      window.__PRESSES__ = [];
      window.__ANSWER__.remove_project = (args, options) => {
        window.__PRESSES__.push(options?.headers?.["zerocode-hand"] ?? null);
        return null;
      };
      // Keys reach a focused control only where something owns the keyboard
      // (a dialog, a settings pane); elsewhere they go to the terminal.
      const owner = document.createElement("div");
      owner.dataset.keyboardOwner = "";
      owner.style.cssText = "position:fixed;left:8px;bottom:8px;z-index:99999";
      const button = document.createElement("button");
      button.id = "held-danger";
      button.type = "button";
      button.textContent = "Remove probe";
      owner.append(button);
      document.body.append(owner);
      button.addEventListener("click", () => void invoke("remove_project", { path: "/tmp/held-probe" }, { hand: handNow() }));
    }), "scene step 8");
    const hold = async (key) => {
      await pressed.page.focus("#held-danger");
      await pressed.page.keyboard.down(key);
      await pressed.page.keyboard.down(key);
      await pressed.page.keyboard.down(key);
      await pressed.page.keyboard.up(key);
      await frame(pressed.page);
      return pressed.page.evaluate(() => window.__PRESSES__.splice(0));
    };
    const enter = await hold("Enter");
    const space = await hold(" ");
    ok("Enter held on a focused button presses it once, as the person", enter.join() === "person", JSON.stringify(enter));
    ok("Space held on a focused button presses it once, as the person", space.join() === "person", JSON.stringify(space));
  } finally { await pressed.page.close(); }
}
