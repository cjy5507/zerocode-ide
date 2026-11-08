import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function workspaceBoardFixture() {
  const names = ["zerocode", "acme", "lotto", "ReguProof"];
  const branches = ["main", "feature/agent-board", "feature/query-performance", "release/customer-staging", "fix/login-session", "feature/admin-console"];
  const catalog = names.map((name, project) => ({ name, path: `/projects/${name}`,
    worktrees: branches.slice(0, project === 0 ? 6 : 5).map((branch, i) => ({
      path: `/projects/${name}/${i}`, branch, base: i ? "main" : null,
      is_main: i === 0, is_folder: false, active: project === 0 && i === 0,
      ownership: "zerocode-managed", external_hidden: false, last_activity_ms: Date.now() - i * 60_000,
      host: name === "acme" && i === 0 ? "acme@edge-ssh" : null,
    })),
  }));
  window.__ANSWER__.project_catalog = () => catalog;
  // zo's accuracy report for the active project: 7/9 first-try, 1/3 catch,
  // weakest = verify. The strip must render these as percentages, not raw.
  window.__ANSWER__.orchestration_accuracy = () => ({
    totalDecisions: 9, firstTrySuccess: 7 / 9, reworkRate: 1 / 7, verifyCatchRate: 1 / 3,
    modelOnlyVerifyRate: 1, foldRate: 2 / 9, weakestDecision: "verify",
  });
  activeProjectPath = catalog[0].path;
  await refreshWorktrees();
  workspaceBoardMode = "list";
  workspaceBoardFit = true;
  workspaceBoardQuery = "";
  workspaceBoardFoldedProjects.clear();
  workspaceBoardSettings = { statuses: DEFAULT_WORKSPACE_BOARD_STATUSES.map((status) => ({ ...status })),
    cards: { "/projects/zerocode/0": { status: "in-progress", pinned: true } }, column_width: 308 };
  window.__ANSWER__.patch_workspace_board_items = () => ({ workspace_board: structuredClone(workspaceBoardSettings) });
  window.__REVIEWS__ = [{ worktree: "/projects/zerocode/1", state: "open", number: 82 },
    { worktree: "/projects/acme/0", state: "merged", number: 31 }];
  for (const [path, state] of [["/projects/zerocode/0", "working"], ["/projects/acme/0", "needs-attention"]]) {
    const term = await openTermTab({ placement: "tab" });
    const tab = tabOfTerm(term);
    tab.worktree = path;
    paneAgents.set(term, "codex");
    hookStates.set(term, state);
    paneActivities.set(`term:${term}`, [{ at: Date.now(), activity: { verb: "edit", target: "ui/shell-workspace.js", phase: "started" } }]);
  }
  setWorkspaceBoardOpen(true);
}

export async function testWorkspaceBoard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(workspaceBoardFixture);
    await page.waitForSelector(".workspace-board-project-group");
    const read = await page.evaluate(() => {
      const board = document.getElementById("workspace-board");
      const first = board.querySelector(".workspace-board-card");
      paintWorkspaceBoard();
      return { mode: board.dataset.view, groups: board.querySelectorAll(".workspace-board-project-group").length,
        cards: board.querySelectorAll(".workspace-board-card").length,
        pinned: board.querySelector(".workspace-board-project-group").classList.contains("is-pinned"),
        stable: first === board.querySelector(".workspace-board-card"),
        summary: document.getElementById("workspace-board-summary").textContent,
        changed: board.querySelectorAll(".workspace-board-changes").length };
    });
    ok("workspace board defaults to project rows, separates pins, preserves nodes and exposes changes",
      read.mode === "list" && read.groups === 5 && read.cards === 21 && read.pinned && read.stable && read.changed === 21, JSON.stringify(read));

    // The board is one of the stage's tabs (2026-11-08): it stands on the
    // strip with its name, its door is pressed, and nothing of its own closes
    // it — the tab's ×, ⌘W and the door do, as for every page.
    const asTab = await page.evaluate(() => {
      const tab = tabs.find((held) => held.id === "workspaces");
      return {
        kind: tab?.kind ?? null, active: activeTabId, label: tab ? tabLabel(tab) : null,
        door: document.getElementById("nav-board").getAttribute("aria-pressed"),
        current: document.getElementById("nav-board").getAttribute("aria-current"),
        ownClose: document.getElementById("workspace-board-close") !== null,
        stored: STAGE_STORED_KINDS.has("workspaces"),
        // A tab takes its own leaf only: a browser in the leaf beside it stays on screen.
        coversBrowsers: browserCovered(),
      };
    });
    ok("the workspace board is a stage tab its door presses and nothing of its own closes",
      asTab.kind === "workspaces" && asTab.active === "workspaces" && Boolean(asTab.label) &&
      asTab.door === "true" && asTab.current === "page" && !asTab.ownClose && asTab.stored &&
      asTab.coversBrowsers === false, JSON.stringify(asTab));

    const hostBadge = await page.locator('.workspace-board-card[data-worktree-path="/projects/acme/0"] .workspace-board-card-host').textContent();
    ok("SSH host badge renders on remote cards", hostBadge === "acme@edge-ssh");

    // zo's rates are folded behind one quiet 「기록」 button at the head's
    // right; folded, never erased — the button opens the same figures.
    await page.waitForSelector("#workspace-board-records:not([hidden])");
    const folded = await page.evaluate(() => ({
      strip: document.getElementById("workspace-board-accuracy").hidden,
      expanded: document.getElementById("workspace-board-records").getAttribute("aria-expanded"),
    }));
    await page.click("#workspace-board-records");
    await page.waitForSelector('#workspace-board-accuracy:not([hidden]) [data-accuracy-field="weakestDecision"]');
    const accuracy = await page.evaluate(() => {
      const strip = document.getElementById("workspace-board-accuracy");
      const figure = (field) => strip.querySelector(`[data-accuracy-field="${field}"] strong`)?.textContent ?? null;
      return {
        hidden: strip.hidden,
        expanded: document.getElementById("workspace-board-records").getAttribute("aria-expanded"),
        firstTry: figure("firstTrySuccess"),
        catchRate: figure("verifyCatchRate"),
        weakest: figure("weakestDecision"),
      };
    });
    ok("board accuracy record opens from its button with zo's report as percentages and the weakest decision",
      folded.strip === true && folded.expanded === "false" &&
      accuracy.hidden === false && accuracy.expanded === "true" && accuracy.firstTry === "78%" &&
      accuracy.catchRate === "33%" && Boolean(accuracy.weakest), JSON.stringify({ folded, accuracy }));
    await page.keyboard.press("Escape");
    const escaped = await page.evaluate(() => ({
      strip: document.getElementById("workspace-board-accuracy").hidden,
      expanded: document.getElementById("workspace-board-records").getAttribute("aria-expanded"),
      active: activeTabId,
    }));
    ok("Escape folds the record and leaves the board's tab standing",
      escaped.strip === true && escaped.expanded === "false" && escaped.active === "workspaces", JSON.stringify(escaped));

    await page.keyboard.press("Meta+k");
    const focusedOnCmdK = await page.evaluate(() => document.activeElement?.id === "workspace-board-query");
    ok("Cmd+K focuses the workspace search query when board is open", focusedOnCmdK);

    const respectsScrim = await page.evaluate(() => {
      const scrim = document.getElementById("qo-scrim");
      const input = document.getElementById("qo-input");
      scrim.hidden = false;
      input.focus();
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true, cancelable: true }));
      const active = document.activeElement?.id;
      scrim.hidden = true;
      return active === "qo-input";
    });
    ok("Cmd+K respects active scrims and does not steal focus", respectsScrim);

    await page.fill("#workspace-board-query", "edge-ssh");
    ok("search matches SSH host field", await page.locator(".workspace-board-card:visible").count() === 1);
    await page.click("#workspace-board-search-clear");

    const prFilterTest = await page.evaluate(async () => {
      workspaceBoardFilterPrState = "open";
      paintWorkspaceBoard();
      const openMatches = [...document.querySelectorAll(".workspace-board-card")]
        .filter((node) => !node.hidden && node.offsetParent !== null)
        .map((node) => node.dataset.worktreePath);
      const filterBadgeVisible = !document.getElementById("workspace-board-filter-count").hidden;

      workspaceBoardFilterPrState = "merged";
      paintWorkspaceBoard();
      const mergedMatches = [...document.querySelectorAll(".workspace-board-card")]
        .filter((node) => !node.hidden && node.offsetParent !== null)
        .map((node) => node.dataset.worktreePath);

      workspaceBoardFilterPrState = null;
      paintWorkspaceBoard();
      return { openMatches, mergedMatches, filterBadgeVisible };
    });
    ok("PR state filter isolates matching reviews and updates filter badge",
      prFilterTest.openMatches.length === 1 && prFilterTest.openMatches[0] === "/projects/zerocode/1" &&
      prFilterTest.mergedMatches.length === 1 && prFilterTest.mergedMatches[0] === "/projects/acme/0" &&
      prFilterTest.filterBadgeVisible, JSON.stringify(prFilterTest));

    const stage = page.locator('.workspace-board-card[data-worktree-path="/projects/zerocode/0"] .workspace-board-card-stage');
    await stage.focus();
    await page.keyboard.press("Enter");
    ok("keyboard activation of a planning stage opens its menu, not the workspace",
      await page.locator("#workspace-board").isVisible() && await page.locator("#sidebar-menu").isVisible());
    await page.evaluate(() => closeSidebarMenu());
    await page.click('[data-workspace-project="/projects/lotto"] .workspace-board-project-head');
    await page.fill("#workspace-board-query", "lotto");
    ok("search reveals matches inside a collapsed project", await page.locator(".workspace-board-card:visible").count() === 5);
    await page.click("#workspace-board-search-clear");
    ok("clearing search restores the person's project fold", await page.locator('[data-workspace-project="/projects/lotto"] .workspace-board-project-cards').isHidden());
    await page.click('[data-workspace-project="/projects/lotto"] .workspace-board-project-head');

    await page.click('[data-workspace-view="kanban"]');
    const lanes = await page.evaluate(() => [...document.querySelectorAll(".workspace-board-lane")].map((lane) => ({
      id: lane.dataset.workspaceStatus,
      empty: lane.dataset.empty,
      name: lane.querySelector(".workspace-board-lane-name")?.textContent ?? null,
      add: lane.querySelector(".workspace-board-lane-add")?.getBoundingClientRect().width ?? 0,
      cards: getComputedStyle(lane.querySelector(".workspace-board-lane-cards")).display,
      height: Math.round(lane.getBoundingClientRect().height),
    })));
    const emptyLanes = lanes.filter((lane) => lane.empty === "true");
    const fullLanes = lanes.filter((lane) => lane.empty === "false");
    ok("kanban keeps every stage as a destination, folds the empty ones and keeps the saved planning assignments",
      lanes.length === 4 && emptyLanes.length === 2 && fullLanes.length === 2 &&
      emptyLanes.every((lane) => lane.add > 0 && lane.cards === "none") &&
      fullLanes.every((lane) => lane.cards !== "none") &&
      await page.locator(".workspace-board-card").count() === 21, JSON.stringify(lanes));
    ok("the born stages speak the window's language",
      lanes.map((lane) => lane.name).join(",") === "할 일,진행 중,검토 중,완료", JSON.stringify(lanes.map((lane) => lane.name)));

    // A card shows the one agent that matters most; the rest stand behind
    // 「+N」, which is a button that opens them inside the card and folds them
    // again — the same open state as the sidebar's fold, by mouse, Tab, Enter
    // and Space, with a name a screen reader speaks (t-12077: it was a dead word).
    const sixAgents = "/projects/zerocode/2";
    const sixCard = `.workspace-board-card[data-worktree-path="${sixAgents}"]`;
    await page.evaluate(async (path) => {
      for (let seat = 0; seat < 6; seat += 1) {
        const term = await openTermTab({ placement: "tab" });
        tabOfTerm(term).worktree = path;
        paneAgents.set(term, "claude");
        hookStates.set(term, "working");
      }
      setWorkspaceBoardOpen(true);
      paintWorkspaceBoard();
    }, sixAgents);
    const agentFold = (selector) => page.evaluate(({ selector, path }) => {
      const card = document.querySelector(selector);
      const more = card?.querySelector(".workspace-board-card-more");
      return {
        rows: card?.querySelectorAll(".workspace-board-card-live .wt-agent").length ?? 0,
        tag: more?.tagName ?? null,
        text: more?.textContent ?? null,
        expanded: more?.getAttribute("aria-expanded") ?? null,
        label: more?.getAttribute("aria-label") ?? null,
        focused: document.activeElement === more,
        open: expandedWorktrees.has(path),
        board: workspaceBoardOpen,
      };
    }, { selector, path: sixAgents });
    const shut = await agentFold(sixCard);
    const more = page.locator(`${sixCard} .workspace-board-card-more`);
    await more.click();
    const clicked = await agentFold(sixCard);
    await more.focus();
    await page.keyboard.press("Enter");
    const entered = await agentFold(sixCard);
    await page.keyboard.press("Space");
    const spaced = await agentFold(sixCard);
    await page.keyboard.press("Space");
    await page.locator(`${sixCard} .workspace-board-card-live .wt-agent`).nth(0).focus();
    await page.keyboard.press("Tab");
    const tabbed = await agentFold(sixCard);
    const english = await page.evaluate(({ selector }) => {
      const was = locale;
      setLocale("en");
      paintWorkspaceBoard();
      const label = document.querySelector(`${selector} .workspace-board-card-more`)?.getAttribute("aria-label");
      const words = t("workspaceBoard.showMoreAgents", "", { count: 5, s: "s" });
      setLocale(was);
      paintWorkspaceBoard();
      return { label, words };
    }, { selector: sixCard });
    ok("a board card's +N is a button that opens the other agents in the card and folds them again",
      shut.rows === 1 && shut.tag === "BUTTON" && shut.text === "+5" && shut.expanded === "false" &&
      shut.label?.includes("5") && !shut.open &&
      clicked.rows === 6 && clicked.expanded === "true" && clicked.open && clicked.board &&
      entered.rows === 1 && entered.expanded === "false" && entered.focused && !entered.open &&
      spaced.rows === 6 && spaced.expanded === "true" && spaced.focused &&
      tabbed.rows === 1 && tabbed.focused && tabbed.expanded === "false" &&
      Boolean(english.words) && english.label === english.words,
      JSON.stringify({ shut, clicked, entered, spaced, tabbed, english }));

    // The record belongs to the head above BOTH views, so a kanban paint asks
    // zo again (2026-09-10: it asked only in list mode — a board opened in kanban
    // never showed it). A different answer proves the paint asked, not that a
    // list-mode answer lingered.
    const kanbanAccuracy = await page.evaluate(async () => {
      window.__ANSWER__.orchestration_accuracy = () => ({
        totalDecisions: 10, firstTrySuccess: 0.9, reworkRate: null, verifyCatchRate: null,
        modelOnlyVerifyRate: null, foldRate: null, weakestDecision: null,
      });
      workspaceBoardAccuracyCache.at = 0;
      paintWorkspaceBoard();
      const strip = document.getElementById("workspace-board-accuracy");
      const figure = () => strip.querySelector('[data-accuracy-field="firstTrySuccess"] strong')?.textContent ?? null;
      for (let i = 0; i < 40 && figure() !== "90%"; i += 1) {
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      return { mode: workspaceBoardMode, records: !document.getElementById("workspace-board-records").hidden, firstTry: figure() };
    });
    ok("a kanban paint refreshes the accuracy record too",
      kanbanAccuracy.mode === "kanban" && kanbanAccuracy.records && kanbanAccuracy.firstTry === "90%", JSON.stringify(kanbanAccuracy));

    // Every agent stir schedules a board paint, and the report is a zo exec per
    // ask: inside the TTL a paint reuses the last answer for the same project,
    // a paint during a drag never asks, and an expired cache asks exactly once more.
    const accuracyAsks = await page.evaluate(async () => {
      let asks = 0;
      const report = { totalDecisions: 10, firstTrySuccess: 0.9, reworkRate: null, verifyCatchRate: null,
        modelOnlyVerifyRate: null, foldRate: null, weakestDecision: null };
      window.__ANSWER__.orchestration_accuracy = () => { asks += 1; return report; };
      paintWorkspaceBoard();
      paintWorkspaceBoard();
      paintWorkspaceBoard();
      const withinTtl = asks;
      workspaceBoardDragging = true;
      workspaceBoardAccuracyCache.at = 0;
      paintWorkspaceBoard();
      const whileDragging = asks;
      workspaceBoardDragging = false;
      paintWorkspaceBoard();
      const afterExpiry = asks;
      await new Promise((resolve) => setTimeout(resolve, 30));
      return { withinTtl, whileDragging, afterExpiry, ttl: WORKSPACE_BOARD_ACCURACY_TTL_MS > 0 };
    });
    ok("board paints inside the TTL reuse the accuracy answer; drags never ask; expiry asks once",
      accuracyAsks.withinTtl === 0 && accuracyAsks.whileDragging === 0 && accuracyAsks.afterExpiry === 1 && accuracyAsks.ttl,
      JSON.stringify(accuracyAsks));

    const pinZoneStates = await page.evaluate(async () => {
      workspaceBoardSettings.cards["/projects/zerocode/0"].pinned = false;
      paintWorkspaceBoard();
      const hiddenZone = document.getElementById("workspace-board").dataset.pinZone;
      workspaceBoardDragging = true;
      paintWorkspaceBoard();
      const dropZone = document.getElementById("workspace-board").dataset.pinZone;
      workspaceBoardMode = "list";
      paintWorkspaceBoard();
      const listDropZone = document.getElementById("workspace-board").dataset.pinZone;
      workspaceBoardMode = "kanban";
      workspaceBoardDragging = false;
      workspaceBoardSettings.cards["/projects/zerocode/0"].pinned = true;
      paintWorkspaceBoard();
      return { hiddenZone, dropZone, listDropZone };
    });
    ok("pin zone is hidden when no cards are pinned and reveals on drag",
      pinZoneStates.hiddenZone === "hidden" && pinZoneStates.dropZone === "drop" && pinZoneStates.listDropZone === "drop", JSON.stringify(pinZoneStates));

    const customFirstStage = await page.evaluate(() => {
      const reversedStatuses = [...DEFAULT_WORKSPACE_BOARD_STATUSES].reverse();
      workspaceBoardSettings.statuses = reversedStatuses;
      const defaultStatus = workspaceBoardDefaultStatus();
      workspaceBoardSettings.statuses = DEFAULT_WORKSPACE_BOARD_STATUSES.map((status) => ({ ...status }));
      return defaultStatus;
    });
    ok("custom statuses order defaults to the user-defined first stage", customFirstStage === "completed");
    const moved = await page.evaluate(async () => {
      await patchWorkspaceBoardItems(["/projects/zerocode/1"], { status: "in-review" });
      return workspaceBoardStatusOf("/projects/zerocode/1");
    });
    ok("planning changes still use the canonical settings patch", moved === "in-review");
    await page.click('[data-workspace-view="list"]');
    ok("switching back to project rows keeps the explicit planning stage",
      await page.locator('[data-worktree-path="/projects/zerocode/1"] .workspace-board-card-stage').textContent() === "검토 중" &&
      await page.locator('[data-worktree-path="/projects/zerocode/1"] .workspace-board-card-stage').getAttribute("data-source") === "manual");

    // A drag from the sidebar never raises the board (Orca #12203): it is a
    // tab, and coming forward would take away the tab the person was reading.
    // A board already in front opens its folded stages for the drag instead.
    const sidebarDrag = await page.evaluate(() => {
      const row = document.querySelector('#worktrees .wt-row[data-worktree-path="/projects/zerocode/1"]');
      const drag = (type, data) => row.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: data }));
      const terminal = tabs.find((held) => held.kind === "term" && held.worktree === activeWorktreePath);
      setActiveTab(terminal.id);
      const behind = new DataTransfer();
      drag("dragstart", behind);
      const away = { active: activeTabId, dragging: workspaceBoardDragging, door: document.getElementById("nav-board").getAttribute("aria-pressed") };
      drag("dragend", behind);
      setWorkspaceBoardOpen(true);
      workspaceBoardMode = "kanban";
      paintWorkspaceBoard();
      const board = document.getElementById("workspace-board");
      const front = new DataTransfer();
      drag("dragstart", front);
      const during = { active: activeTabId, dragging: board.dataset.dragging,
        openLanes: [...board.querySelectorAll('.workspace-board-lane[data-empty="true"] .workspace-board-lane-cards')]
          .every((cards) => getComputedStyle(cards).display !== "none") };
      drag("dragend", front);
      const after = { dragging: board.dataset.dragging };
      workspaceBoardMode = "list";
      paintWorkspaceBoard();
      return { row: row !== null, terminal: terminal?.id ?? null, away, during, after };
    });
    ok("a sidebar drag never raises the board, and a board in front opens its stages for it",
      sidebarDrag.row && sidebarDrag.away.active === sidebarDrag.terminal && sidebarDrag.away.dragging === false &&
      sidebarDrag.away.door === "false" &&
      sidebarDrag.during.active === "workspaces" && sidebarDrag.during.dragging === "true" && sidebarDrag.during.openLanes &&
      sidebarDrag.after.dragging === "false", JSON.stringify(sidebarDrag));

    await mkdir("output/playwright/workspace-board", { recursive: true });
    for (const width of [1920, 1280, 800, 560, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => {
        const board = document.getElementById("workspace-board");
        board.style.position = "fixed"; board.style.inset = "0"; board.style.zIndex = "100";
      });
      for (const mode of ["list", "kanban"]) {
        await page.click(`[data-workspace-view="${mode}"]`);
        await page.mouse.move(5, 5);
        await page.waitForTimeout(180);
        const sizes = await page.evaluate(() => {
          const board = document.getElementById("workspace-board");
          const lanes = document.getElementById("workspace-board-lanes");
          const card = board.querySelector(".workspace-board-card");
          return { board: board.clientWidth, content: board.scrollWidth,
            lanes: lanes.clientWidth, scroll: lanes.scrollWidth, card: card.getBoundingClientRect().width };
        });
        ok(`${mode} fits the workspace surface at ${width}px`, sizes.content <= sizes.board + 1 && sizes.scroll <= sizes.lanes + 1, JSON.stringify(sizes));
        await page.screenshot({ path: `output/playwright/workspace-board/${mode}-${width}.png` });
      }
    }
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.click('[data-workspace-view="list"]');
    // `applyTheme` reads the chosen treatment from `theme`; it takes no argument.
    await page.evaluate(() => { theme = "light"; applyTheme(); });
    await page.screenshot({ path: "output/playwright/workspace-board/list-light.png" });
    await page.evaluate(() => setLocale("en"));
    ok("workspace modes and labels follow the locale", await page.locator('[data-workspace-view="list"]').textContent() === "Projects" &&
      (await page.locator("#workspace-board-summary").textContent()).includes("workspaces"));
    // A row of the workspace already in front asks for its terminal: the
    // board steps back onto the strip the way the 작업 tab does, and waits there.
    const backToTerminal = await page.evaluate(async () => {
      document.querySelector('.workspace-board-card[data-worktree-path="/projects/zerocode/0"]').click();
      for (let i = 0; i < 40 && activeTabId === "workspaces"; i += 1) {
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      const staged = tabs.find((held) => held.id === activeTabId);
      const seen = { kind: staged?.kind ?? null, worktree: staged?.worktree ?? null,
        kept: tabs.some((held) => held.id === "workspaces"),
        door: document.getElementById("nav-board").getAttribute("aria-pressed") };
      setWorkspaceBoardOpen(true);
      return seen;
    });
    ok("a row of the workspace in front brings its terminal back and leaves the board's tab on the strip",
      backToTerminal.kind === "term" && backToTerminal.worktree === "/projects/zerocode/0" &&
      backToTerminal.kept && backToTerminal.door === "false", JSON.stringify(backToTerminal));

    // Whichever road the tab leaves by — here its × — the board forgets what it
    // was narrowed to, and the next opening shows every workspace again.
    const forgets = await page.evaluate(async () => {
      const shown = () => [...document.querySelectorAll("#workspace-board .workspace-board-card")]
        .filter((node) => node.offsetParent !== null).length;
      workspaceBoardQuery = "lotto";
      workspaceBoardFilterPrState = "open";
      paintWorkspaceBoard();
      const narrowed = shown();
      closeTab("workspaces");
      const closed = !tabs.some((held) => held.id === "workspaces");
      setWorkspaceBoardOpen(true);
      await new Promise((resolve) => setTimeout(resolve, 50));
      return { narrowed, closed, query: workspaceBoardQuery, input: document.getElementById("workspace-board-query").value,
        filter: workspaceBoardFilterPrState, chip: workspaceBoardChip, shown: shown() };
    });
    ok("closing the board's tab by its × forgets the search and the filters",
      forgets.narrowed === 0 && forgets.closed && forgets.query === "" && forgets.input === "" &&
      forgets.filter === null && forgets.chip === null && forgets.shown === 21, JSON.stringify(forgets));
    await page.evaluate(() => {
      window.__ANSWER__.set_active_worktree = ({ path }) => {
        window.__WORKSPACE_CHANGE_TARGET__ = path;
        for (const project of window.__ANSWER__.project_catalog()) {
          for (const worktree of project.worktrees) worktree.active = worktree.path === path;
        }
        return "feature/agent-board";
      };
    });
    await page.click('[data-worktree-path="/projects/zerocode/1"] .workspace-board-changes');
    await page.waitForSelector("#activity-scm", { state: "visible" });
    const target = await page.evaluate(() => ({ path: window.__WORKSPACE_CHANGE_TARGET__,
      board: workspaceBoardOpen, activity: activityShowing(), folded: folded.aside,
      kept: tabs.some((held) => held.id === "workspaces") }));
    ok("view changes activates the row's workspace and opens its source control panel",
      target.path === "/projects/zerocode/1" && !target.board && target.activity === "scm" && !target.folded &&
      target.kept, JSON.stringify(target));
    ok("workspace board raised no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testWorkspaceBoard(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
