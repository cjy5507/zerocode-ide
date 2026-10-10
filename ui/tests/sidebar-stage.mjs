import { openWindowTestPage } from "./window-boot.mjs";

/* A sidebar row raises its worker's terminal on the stage (t-44057).
 *
 * On 2026-10-10 a person saw a working agent in the sidebar whose terminal was
 * not on the stage: the stage kept showing a plain shell, and clicking the row
 * changed nothing. Four paths can leave a living worker's pane where the stage
 * does not look for it. Each one is a scene here, and each scene opens a window
 * of its own.
 *
 *   a. The row was drawn under the checkout the worker stood in, and the ledger
 *      has since seated the worker in another checkout. The card has not
 *      repainted yet, so the row still names the old checkout. The click must
 *      follow the terminal, not the row.
 *   b. A restart. The ledger reseats the worker into a checkout whose pane file
 *      still holds a stored shell from before the restart. A visit to that
 *      checkout must raise the living worker, not the stored shell.
 *   c. A visit to the checkout already in front, after the stage lost its
 *      terminal. The ledger knows the living worker there. The visit must attach
 *      that worker and must not open a blank shell beside it.
 *
 * Every path is synthetic. Nothing here is a real project or a real pane. */

const CHECKOUT_B = "/tmp/t44057/wt-b";
const CHECKOUT_C = "/tmp/t44057/wt-c";
const WORKER = 9101;

/* Lets the window's own timers and frames run. */
const settle = (page, ms) =>
  page.evaluate((ms) => new Promise((done) => setTimeout(done, ms)), ms);

/* Adds checkout B to the catalog, so the sidebar draws its card, and answers the
 * pane file of B with `stored`. The ledger's answer for B is `lastSeat`. */
const addCheckoutB = (page, { stored = [], lastSeat } = {}) =>
  page.evaluate(({ CHECKOUT_B, stored, lastSeat }) => {
    const base = JSON.parse(JSON.stringify(projects));
    base[0].worktrees = base[0].worktrees.filter((one) => one.path !== CHECKOUT_B);
    base[0].worktrees.push({
      path: CHECKOUT_B,
      branch: "wt/t-44057/b",
      is_main: false,
      active: false,
      is_folder: false,
      ownership: "zerocode-managed",
      external_hidden: false,
    });
    window.__ANSWER__.project_catalog = () => base.map((project) => ({
      ...project,
      worktrees: project.worktrees.map((one) => ({ ...one, active: one.path === activeWorktreePath })),
    }));
    window.__ANSWER__.pane_layouts = ({ worktree }) =>
      (worktree === CHECKOUT_B ? JSON.parse(JSON.stringify(stored)) : []);
    window.__ANSWER__.save_pane_layouts = () => null;
    if (lastSeat !== undefined) {
      window.__ANSWER__.worktree_last_agent = ({ worktree }) =>
        (worktree === CHECKOUT_B ? lastSeat : null);
    }
  }, { CHECKOUT_B, stored, lastSeat });

/* The state a person sees: is the worker's terminal on the stage, and is it the
 * tab in front. */
const stageOf = (term) => ({
  active: activeWorktreePath,
  onStage: [...readingTerms()].includes(term),
  inFront: activeTabId === tabOfTerm(term)?.id,
  tabWorktree: tabOfTerm(term)?.worktree ?? null,
});

/* A scene's receipt: what a check printed, as one line. */
const receipt = (value) => JSON.stringify(value);

export async function testSidebarStage(browser, origin, ok) {
  // a. The row was drawn under A; the ledger has since seated the worker in B.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page);
    const scene = await page.evaluate(async ({ CHECKOUT_B, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const A = activeWorktreePath;
      const shell = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: shell, term: WORKER, worktree: A, agent: "codex" } });
      }
      await settle(250);
      // The row the person sees, under the checkout in front.
      const row = document.querySelector(`.wt-agents[data-worktree-path="${A}"] .wt-agent[data-term="${WORKER}"]`);
      const drawnUnderA = row !== null;
      // The ledger seats the same worker in its own checkout. The card has not repainted yet.
      seatLedgerManagedTerm(WORKER, CHECKOUT_B, "codex");
      // The click lands on the row still showing A.
      const pressed = performance.now();
      row?.click();
      let elapsed = null;
      while (performance.now() - pressed < 3000) {
        if (stage(WORKER).onStage) {
          elapsed = Math.round(performance.now() - pressed);
          break;
        }
        await new Promise((done) => requestAnimationFrame(() => done()));
      }
      await settle(100);
      return { drawnUnderA, ms: elapsed, ...stage(WORKER) };
    }, { CHECKOUT_B, WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a sidebar row whose worker the ledger moved to another checkout raises that worker's terminal on the stage",
      scene.drawnUnderA && scene.onStage && scene.inFront && scene.active === CHECKOUT_B && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
  }

  // a2. The worker moves again while the click's activation of B is still in flight: the ledger
  // seats it in C before B has finished coming to the front. The click must end where the terminal
  // stands when the activation returns, not where it stood when the click began (run-11955 m-44638).
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page);
    const scene = await page.evaluate(async ({ CHECKOUT_B, CHECKOUT_C, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const A = activeWorktreePath;
      // C beside B, through the same catalog answer the sidebar reads.
      const catalog = window.__ANSWER__.project_catalog;
      window.__ANSWER__.project_catalog = () => catalog().map((project) => ({
        ...project,
        worktrees: [...project.worktrees.filter((one) => one.path !== CHECKOUT_C), {
          path: CHECKOUT_C, branch: "wt/t-44057/c", is_main: false, active: activeWorktreePath === CHECKOUT_C,
          is_folder: false, ownership: "zerocode-managed", external_hidden: false,
        }],
      }));
      const layouts = window.__ANSWER__.pane_layouts;
      window.__ANSWER__.pane_layouts = (ask) => (ask.worktree === CHECKOUT_C ? [] : layouts(ask));
      await refreshWorktrees();
      const shell = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: shell, term: WORKER, worktree: A, agent: "codex" } });
      }
      await settle(250);
      const row = document.querySelector(`.wt-agents[data-worktree-path="${A}"] .wt-agent[data-term="${WORKER}"]`);
      seatLedgerManagedTerm(WORKER, CHECKOUT_B, "codex");
      // The first activation (A → B) is held open; the worker moves to C meanwhile.
      const paneFile = window.__ANSWER__.pane_layouts;
      let movedDuring = false;
      window.__ANSWER__.pane_layouts = async (ask) => {
        if (ask.worktree === CHECKOUT_B && !movedDuring) {
          movedDuring = true;
          seatLedgerManagedTerm(WORKER, CHECKOUT_C, "codex");
          await settle(50);
        }
        return paneFile(ask);
      };
      const pressed = performance.now();
      row?.click();
      let elapsed = null;
      while (performance.now() - pressed < 3000) {
        const now = stage(WORKER);
        if (now.onStage && now.active === CHECKOUT_C) {
          elapsed = Math.round(performance.now() - pressed);
          break;
        }
        await new Promise((done) => requestAnimationFrame(() => done()));
      }
      await settle(100);
      return { movedDuring, ms: elapsed, ...stage(WORKER) };
    }, { CHECKOUT_B, CHECKOUT_C, WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a worker the ledger moves again while the click's activation is in flight ends on the stage of the checkout it stands in when the activation returns",
      scene.movedDuring && scene.onStage && scene.inFront && scene.active === CHECKOUT_C &&
        scene.tabWorktree === CHECKOUT_C && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
  }

  // a3. A worker that keeps moving: the ledger re-seats it during every activation. After the bounded
  // hops the click must not pick a tab whose checkout is not the one in front — it stops, selecting
  // nothing, rather than showing the wrong stage as if it were the worker's (run-11955 m-44652).
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page);
    const scene = await page.evaluate(async ({ CHECKOUT_B, CHECKOUT_C, CHECKOUT_D, CHECKOUT_E, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const A = activeWorktreePath;
      const extra = [CHECKOUT_C, CHECKOUT_D, CHECKOUT_E];
      const catalog = window.__ANSWER__.project_catalog;
      window.__ANSWER__.project_catalog = () => catalog().map((project) => ({
        ...project,
        worktrees: [...project.worktrees.filter((one) => !extra.includes(one.path)), ...extra.map((path, at) => ({
          path, branch: `wt/t-44057/${"cde"[at]}`, is_main: false, active: activeWorktreePath === path,
          is_folder: false, ownership: "zerocode-managed", external_hidden: false,
        }))],
      }));
      const layouts = window.__ANSWER__.pane_layouts;
      window.__ANSWER__.pane_layouts = (ask) => (extra.includes(ask.worktree) ? [] : layouts(ask));
      await refreshWorktrees();
      const shell = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: shell, term: WORKER, worktree: A, agent: "codex" } });
      }
      await settle(250);
      const row = document.querySelector(`.wt-agents[data-worktree-path="${A}"] .wt-agent[data-term="${WORKER}"]`);
      seatLedgerManagedTerm(WORKER, CHECKOUT_B, "codex");
      // Every activation the click awaits finds the worker gone one checkout further: B → C → D → E.
      const paneFile = window.__ANSWER__.pane_layouts;
      const next = { [CHECKOUT_B]: CHECKOUT_C, [CHECKOUT_C]: CHECKOUT_D, [CHECKOUT_D]: CHECKOUT_E };
      let moves = 0;
      window.__ANSWER__.pane_layouts = async (ask) => {
        if (next[ask.worktree]) {
          moves += 1;
          seatLedgerManagedTerm(WORKER, next[ask.worktree], "codex");
          await settle(30);
        }
        return paneFile(ask);
      };
      const activeBefore = activeTabId;
      row?.click();
      await settle(1500);
      const now = stage(WORKER);
      return { moves, ...now, activeTabId, activeBefore, selectedWrong: now.inFront && now.active !== now.tabWorktree };
    }, { CHECKOUT_B, CHECKOUT_C, CHECKOUT_D: "/tmp/t44057/wt-d", CHECKOUT_E: "/tmp/t44057/wt-e", WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a worker that keeps moving while the click follows it is not shown as in front once the bounded hops run out — the click stops rather than selecting a tab of a checkout that is not on the stage",
      scene.moves >= 3 && scene.selectedWrong === false && scene.inFront === false && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
  }

  // a4. No row click at all: a plain visit of D (what a sidebar card or a project switch does) while the
  // ledger moves D's worker to E during the visit's own reads. The visit's restore chose D's living agent
  // before it awaited the stored layouts; it must ask again afterwards and not put a tab of E in front
  // (run-11955 m-44711).
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page);
    const scene = await page.evaluate(async ({ CHECKOUT_B, CHECKOUT_C, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const A = activeWorktreePath;
      const catalog = window.__ANSWER__.project_catalog;
      window.__ANSWER__.project_catalog = () => catalog().map((project) => ({
        ...project,
        worktrees: [...project.worktrees.filter((one) => one.path !== CHECKOUT_C), {
          path: CHECKOUT_C, branch: "wt/t-44057/c", is_main: false, active: activeWorktreePath === CHECKOUT_C,
          is_folder: false, ownership: "zerocode-managed", external_hidden: false,
        }],
      }));
      const layouts = window.__ANSWER__.pane_layouts;
      window.__ANSWER__.pane_layouts = (ask) => (ask.worktree === CHECKOUT_C ? [] : layouts(ask));
      await refreshWorktrees();
      const shell = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: shell, term: WORKER, worktree: A, agent: "codex" } });
      }
      await settle(250);
      // The worker stands in B. The visit of B reads B's pane file; while that read is open, the ledger
      // moves the worker to C.
      seatLedgerManagedTerm(WORKER, CHECKOUT_B, "codex");
      const paneFile = window.__ANSWER__.pane_layouts;
      let movedDuring = false;
      window.__ANSWER__.pane_layouts = async (ask) => {
        if (ask.worktree === CHECKOUT_B && !movedDuring) {
          movedDuring = true;
          seatLedgerManagedTerm(WORKER, CHECKOUT_C, "codex");
          await settle(50);
        }
        return paneFile(ask);
      };
      await activateWorktree(CHECKOUT_B);
      await settle(150);
      const now = stage(WORKER);
      const front = tabs.find((held) => held.id === activeTabId) ?? null;
      return { movedDuring, ...now, frontWorktree: front?.worktree ?? null, frontIsWorker: front?.id === tabOfTerm(WORKER)?.id };
    }, { CHECKOUT_B, CHECKOUT_C, WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a plain visit whose worker the ledger moves away during the visit's own reads does not put that worker's tab in front — the restore asks again which tab still stands in the checkout",
      scene.movedDuring && scene.active === CHECKOUT_B && scene.frontIsWorker === false &&
        (scene.frontWorktree === null || scene.frontWorktree === CHECKOUT_B) && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
  }

  // b. After a restart: a stored shell in B, and the ledger has reseated the worker into B.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page, { stored: [{ id: 1, root: { type: "leaf" } }] });
    const scene = await page.evaluate(async ({ CHECKOUT_B, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const shell = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: shell, term: WORKER, worktree: CHECKOUT_B, agent: "codex" } });
      }
      await settle(250);
      // The person visits B from its workspace row.
      await activateWorktree(CHECKOUT_B);
      await settle(400);
      const visit = stage(WORKER);
      // Living agent panes in B, and how many of them the stage shows.
      const living = [...paneAgents.keys()].filter((term) => tabOfTerm(term)?.worktree === CHECKOUT_B);
      const shown = living.filter((term) => [...readingTerms()].includes(term));
      // Then the row, as the person would click it.
      document.querySelector(`.wt-agent[data-term="${WORKER}"]`)?.click();
      await settle(300);
      return { visit, living: living.length, shown: shown.length, afterRow: stage(WORKER) };
    }, { CHECKOUT_B, WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a visit to a checkout whose living ledger worker sits beside a stored shell raises the worker, not the shell",
      scene.visit.onStage && scene.visit.inFront && scene.living === 1 && scene.shown === 1 && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
    ok(
      "a row click after a restart seat raises the reseated worker's terminal",
      scene.afterRow.onStage && scene.afterRow.inFront && faults.length === 0,
      receipt({ afterRow: scene.afterRow, faults: faults.length }),
    );
  }

  // c. The checkout is in front, the stage lost its terminal, and the ledger names the living worker.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    await addCheckoutB(page);
    await page.evaluate(async (CHECKOUT_B) => { await activateWorktree(CHECKOUT_B); }, CHECKOUT_B);
    await settle(page, 200);
    // The ledger now knows a living worker in B, which has no tab yet.
    await addCheckoutB(page, { lastSeat: { agent: "codex", sleeping: false, term: WORKER } });
    await page.evaluate((CHECKOUT_B) => {
      activeTabId = null;
      for (const held of tabs.filter((one) => one.worktree === CHECKOUT_B)) dropTab(held.id);
    }, CHECKOUT_B);
    await settle(page, 100);
    const scene = await page.evaluate(async ({ CHECKOUT_B, WORKER, stageOfSource }) => {
      const stage = eval(`(${stageOfSource})`);
      const settle = (ms) => new Promise((done) => setTimeout(done, ms));
      const before = window.__NEXT_TERM__ ?? 0;
      // Ten visits to the checkout in front.
      for (let visit = 0; visit < 10; visit += 1) await activateWorktree(CHECKOUT_B);
      await settle(300);
      return { blankShells: (window.__NEXT_TERM__ ?? 0) - before, ...stage(WORKER) };
    }, { CHECKOUT_B, WORKER, stageOfSource: stageOf.toString() });
    ok(
      "a visit to the checkout in front attaches the ledger's living worker and opens no blank shell",
      scene.onStage && scene.inFront && scene.blankShells === 0 && faults.length === 0,
      receipt({ ...scene, faults: faults.length }),
    );
  }
}
