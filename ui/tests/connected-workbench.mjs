import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { taskBoardFixture } from "./task-board.mjs";
import { workspaceBoardFixture } from "./workspace-board.mjs";

export async function testConnectedWorkbench(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(taskBoardFixture);
    await page.waitForSelector(".task-board-row");
    const scoped = await page.evaluate(() => {
      const card = { pane: "term:1", heading: "Same title", project: "/repos/one", worktree: "main", state: "working", parent: "" };
      const cards = [card, { ...card, pane: "term:2", parent: "term:1" },
        { ...card, pane: "term:3", project: "/repos/two" }];
      const places = new Map([["term:1", { checkout: "/repos/one" }],
        ["term:2", { checkout: "/repos/one-child" }], ["term:3", { checkout: "/repos/two" }]]);
      const model = agentGraphModel([{ bucket: "working", cards }], places, new Map(), Date.now(), null, []);
      const groups = taskBoardModel(model, null, Date.now(), "/repos/one-child").groups;
      return { groups: groups.length, members: groups[0]?.members.map((entry) => entry.card.pane),
        absent: taskBoardModel(model, null, Date.now(), "/repos/missing").groups.length };
    });
    ok("workspace scope uses exact checkout identity and preserves cross-checkout task lineage",
      scoped.groups === 1 && scoped.members.join() === "term:1,term:2" && scoped.absent === 0, JSON.stringify(scoped));

    const taskClone = await page.evaluate(() => {
      const original = docHost(boardTab().pane, "board");
      const places = new Map(window.__COLUMNS__.flatMap((column) => column.cards)
        .map((card) => [card.pane, { checkout: card.project }]));
      const model = agentGraphModel(window.__COLUMNS__, places, new Map(), Date.now(), null, []);
      paintAgentGraph(original, model);
      selectTaskBoardMember(original, "agent:term:103");
      const group = nextGroupId++;
      const holder = groupOf(group);
      document.body.append(holder.el);
      const clone = docHost(group, "board");
      paintAgentGraph(clone, agentGraphModels.get(original));
      clone.querySelector('[data-task-key="agent:term:103"] .task-board-action').click();
      const selected = agentGraphSelectedKey;
      clone.querySelector(".workbench-related-artifacts").click();
      const origin = artifactFilter.origin;
      setAgentGraphInspectorOpen(original, false);
      holder.el.remove(); groups.delete(group);
      return { selected, origin };
    });
    ok("a cloned task board owns working row actions and opens results with the exact checkout filter",
      taskClone.selected === "agent:term:103" && taskClone.origin?.field === "worktree" &&
      taskClone.origin.value === "/repos/zerocode", JSON.stringify(taskClone));
    await page.evaluate(taskBoardFixture);
    await page.waitForSelector(".task-board-row");

    await page.click('[data-task-filter="attention"]');
    await page.click(".task-board-action");
    await page.fill(".board-ask-field", "Keep this answer");
    await page.click('#board-view .workbench-nav-link[data-workbench-view="workspaces"]');
    await page.click('#workspace-board .workbench-nav-link[data-workbench-view="tasks"]');
    await page.waitForSelector(".board-ask-field");
    ok("surface navigation preserves task selection, attention filter and unsent answer",
      await page.locator(".board-ask-field").inputValue() === "Keep this answer" &&
      await page.locator('[data-task-filter="attention"]').getAttribute("aria-pressed") === "true");

    await page.evaluate(workspaceBoardFixture);
    const stages = await page.evaluate(async () => {
      const path = "/projects/zerocode/0";
      const original = structuredClone(workspaceBoardSettings.cards[path]);
      workspaceBoardSettings.cards[path].status = null;
      const automatic = workspaceBoardStage(path);
      workspaceBoardSettings.cards[path].status = "completed";
      const manual = workspaceBoardStage(path);
      await patchWorkspaceBoardItems([path], { automatic: true });
      const reset = workspaceBoardStage(path);
      workspaceBoardSettings.cards[path] = original;
      paintWorkspaceBoard();
      return { automatic, manual, reset };
    });
    ok("live work chooses an unfinished automatic stage while explicit manual stages persist",
      stages.automatic.automatic && stages.automatic.id === "in-progress" &&
      !stages.manual.automatic && stages.manual.id === "completed" &&
      stages.reset.automatic && stages.reset.id === "in-progress", JSON.stringify(stages));
    const accessibleCard = page.locator('.workspace-board-card[data-worktree-path="/projects/zerocode/1"]');
    const accessibleTree = await accessibleCard.ariaSnapshot();
    ok("workspace rows expose independent controls in the accessibility tree",
      accessibleTree.includes('row') && accessibleTree.includes('gridcell') &&
      accessibleTree.includes('button "이 작업 공간의 작업"') && accessibleTree.includes('button "변경 보기"'), accessibleTree);
    await accessibleCard.locator(".workspace-board-card-stage").click();
    ok("automatic stage menus mark activity rather than a manual default",
      await page.getByRole("menuitem", { name: "✓ 실제 활동에 따라 자동 표시", exact: true }).count() === 1 &&
      await page.getByRole("menuitem", { name: "✓ Todo", exact: true }).count() === 0);
    await page.keyboard.press("Escape");
    await accessibleCard.focus();
    await page.keyboard.press("Space");
    ok("accessible workspace rows retain keyboard selection",
      await accessibleCard.getAttribute("aria-selected") === "true");
    await page.fill("#workspace-board-query", "query-performance");
    await page.click('#workspace-board .workbench-nav-link[data-workbench-view="tasks"]');
    await page.click('#board-view .workbench-nav-link[data-workbench-view="workspaces"]');
    ok("workspace query survives a cross-surface round trip",
      await page.locator("#workspace-board-query").inputValue() === "query-performance");
    await page.fill("#workspace-board-query", "");
    await page.click('.workspace-board-card[data-worktree-path="/projects/zerocode/1"] .workspace-board-tasks');
    await page.waitForSelector("#board-view .workbench-context:not([hidden])");
    const context = await page.evaluate(() => ({ path: workbenchScopes.tasks?.path,
      rows: document.querySelectorAll("#board-view .task-board-row").length,
      title: document.querySelector("#board-view .workbench-context-label").dataset.tip }));
    ok("workspace task link scopes even an empty checkout without borrowing same-name agents",
      context.path === "/projects/zerocode/1" && context.title === context.path && context.rows === 0, JSON.stringify(context));
    await page.click("#board-view .workbench-context-clear");
    await page.waitForSelector("#board-view .task-board-row");
    ok("scope can be cleared to recover the full task inventory", await page.locator("#board-view .task-board-row").count() > 0);

    await page.evaluate(async () => {
      const pending = new Promise((resolve) => { window.__finishAccuracy = resolve; });
      window.__ANSWER__.orchestration_accuracy = () => pending;
      workspaceBoardAccuracyCache.at = 0;
      window.__accuracyPending = refreshWorkspaceBoardAccuracy("/projects/zerocode");
      await refreshWorkspaceBoardAccuracy(null);
      window.__finishAccuracy({ totalDecisions: 1, firstTrySuccess: 1 });
      await window.__accuracyPending;
    });
    ok("clearing the project invalidates an in-flight accuracy answer", await page.locator("#workspace-board-accuracy").isHidden());

    await page.evaluate(() => { secondBrainVault = "/vault"; paintKnowledgeEntry(); setWorkspaceBoardOpen(true); });
    await page.click("#nav-knowledge");
    ok("sidebar knowledge navigation closes a covering workspace board",
      await page.locator("#workspace-board").isHidden());
    const reopened = await page.evaluate(() => {
      const held = tabs.find((tab) => tab.kind === "knowledge");
      held.worktree = "/another-checkout";
      held.pane = nextGroupId + 1;
      focusedPane = stageGroups()[0];
      openKnowledgeGraph();
      return { pane: held.pane, visible: stageGroups().includes(held.pane),
        checkout: held.worktree === activeWorktreePath, active: currentTab()?.kind };
    });
    ok("reopening a global knowledge tab uses a leaf in the current checkout",
      reopened.visible && reopened.checkout && reopened.active === "knowledge", JSON.stringify(reopened));

    await page.evaluate(() => {
      window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 2, tags: ["reference"] };
      secondBrainVault = "/vault";
      knowledgeQuery = "";
      paintKnowledgeEntry();
      openWorkbenchView("knowledge");
    });
    await page.waitForSelector(".knowledge-node");
    await page.evaluate(() => {
      const view = docHost(knowledgeTab().pane, "knowledge");
      const model = knowledgeLayouts.get(view).model;
      const key = model.keys.find((_, index) => model.kinds[index] !== "ghost");
      selectKnowledgeNode(view, key);
    });
    await page.click(".knowledge-inspector-request");
    await page.waitForSelector("#wt-spec:visible");
    const request = await page.evaluate(() => ({ value: wtSpec.value, asked: window.__GRAPH_PAGE_ASKED__,
      project: worktreeFormProject, started: window.__STARTED__?.length ?? 0 }));
    ok("knowledge opens the existing editable request composer with a backend-resolved source",
      request.value.includes(`/vault/${request.asked.id}`) && request.asked.path === "/vault" &&
      request.project === "/projects/zerocode", JSON.stringify(request));
    await page.evaluate(() => setWorktreeForm(false));
    const stale = await page.evaluate(async () => {
      const view = docHost(knowledgeTab().pane, "knowledge");
      const model = knowledgeLayouts.get(view).model;
      let finish;
      window.__ANSWER__.second_brain_page = () => new Promise((resolve) => { finish = resolve; });
      const pending = requestFromKnowledge(view, knowledgeSelectedKey);
      await new Promise((resolve) => setTimeout(resolve, 0));
      selectKnowledgeNode(view, model.keys.find((key) => key !== knowledgeSelectedKey));
      finish("/vault/old.md");
      await pending;
      return document.getElementById("wt-spec").closest(".scrim").hidden;
    });
    ok("late source resolution never opens a composer for a different selected page", stale);

    const guards = await page.evaluate(async () => {
      const view = docHost(knowledgeTab().pane, "knowledge");
      const model = knowledgeLayouts.get(view).model;
      selectKnowledgeNode(view, model.keys.find((_, i) => model.kinds[i] !== "ghost"));
      let finish;
      window.__ANSWER__.second_brain_page = () => new Promise((resolve) => { finish = resolve; });
      const begin = async () => {
        const pending = requestFromKnowledge(view, knowledgeSelectedKey);
        await new Promise((resolve) => setTimeout(resolve, 0));
        return { pending };
      };
      const first = await begin();
      openWorktreeFormForProject("/projects/zerocode");
      wtSpec.value = "A newer request";
      finish("/vault/older.md");
      await first.pending;
      const preserved = wtSpec.value === "A newer request";
      setWorktreeForm(false);
      const second = await begin();
      openWorktreeFormForProject("/projects/zerocode");
      setWorktreeForm(false);
      finish("/vault/older.md");
      await second.pending;
      const stayedClosed = wtNewScrim.hidden;
      const third = await begin();
      openWorkbenchView("workspaces");
      finish("/vault/older.md");
      await third.pending;
      const noModalOverBoard = wtNewScrim.hidden;
      openWorkbenchView("knowledge");
      const unscoped = { workspace: { path: "\u0000unscoped-workspace", scoped: false }, card: { heading: "Unscoped" } };
      const related = workbenchRelatedActions(unscoped);
      return { preserved, stayedClosed, noModalOverBoard,
        noFalseDoors: !related.querySelector(".workbench-related-workspace, .workbench-related-artifacts") };
    });
    ok("late knowledge responses preserve newer drafts, closed forms and chosen destinations",
      guards.preserved && guards.stayedClosed && guards.noModalOverBoard, JSON.stringify(guards));
    ok("unscoped agents never receive invented workspace or artifact links", guards.noFalseDoors);

    const cloned = await page.evaluate(async () => {
      window.__ANSWER__.second_brain_page = (args) => `${args.path}/${args.id}`;
      const group = nextGroupId++;
      const holder = groupOf(group);
      document.body.append(holder.el);
      const source = docHost(knowledgeTab().pane, "knowledge");
      source.querySelector(".knowledge-inspector-request").disabled = true;
      const clone = docHost(group, "knowledge");
      source.querySelector(".knowledge-inspector-request").disabled = false;
      await paintKnowledgeView({ ...knowledgeTab(), pane: group });
      clone.hidden = false;
      const model = knowledgeLayouts.get(clone).model;
      selectKnowledgeNode(clone, model.keys.find((_, i) => model.kinds[i] !== "ghost"));
      clone.querySelector(".knowledge-inspector-request").click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const opened = !wtNewScrim.hidden && wtSpec.value.includes("/vault/");
      setWorktreeForm(false);
      holder.el.remove();
      groups.delete(group);
      return { different: source !== clone, copiedFlag: clone.dataset.knowledgeWired, opened };
    });
    ok("a cloned knowledge leaf wires its own request button even when the source was already wired",
      cloned.different && cloned.copiedFlag === "yes" && cloned.opened, JSON.stringify(cloned));

    const scopedReport = await page.evaluate(async () => {
      let asked;
      window.__ANSWER__.orchestration_accuracy = ({ path }) => { asked = path; return { totalDecisions: 3 }; };
      const path = "/projects/acme/0";
      const project = projects.find((one) => one.path === "/projects/acme");
      const held = project.worktrees[0].external_hidden;
      project.worktrees[0].external_hidden = true;
      workspaceBoardQuery = "an old search";
      openWorkbenchWorkspace(path, "Remote main");
      await new Promise((resolve) => setTimeout(resolve, 0));
      const result = { asked, input: el("workspace-board-query").value,
        paths: workspaceBoardWorktrees().map((one) => one.path) };
      project.worktrees[0].external_hidden = held;
      return result;
    });
    ok("a direct workspace link reveals its filtered checkout and uses its own project report",
      scopedReport.asked === "/projects/acme" && scopedReport.input === "" &&
      scopedReport.paths.join() === "/projects/acme/0", JSON.stringify(scopedReport));

    // 2026-09-13 「네비바에 안 보여서」: a worktree this window did not make is hidden
    // — unless an agent is running there in one of this window's panes.
    const liveHidden = await page.evaluate(() => {
      // A worktree no pane of this window is working in — the fixtures seat
      // agents in some, and those are exactly the rows the exception keeps.
      const seat = projects.flatMap((project) => project.worktrees.map((worktree) => ({ project, worktree })))
        .find(({ worktree }) => worktreeAgentRows(worktree.path).length === 0);
      if (!seat) return { noQuietWorktree: true };
      const { project, worktree } = seat;
      const held = worktree.external_hidden;
      worktree.external_hidden = true;
      const hiddenAlone = !shownWorktrees(project).some((one) => one.path === worktree.path);
      const term = 4711;
      mountTermTab(term, { agent: "zo", worktree: worktree.path }, { focus: false });
      paneAgents.set(term, "zo");
      const shownWithAgent = shownWorktrees(project).some((one) => one.path === worktree.path);
      paneAgents.delete(term);
      dropTab(tabOfTerm(term)?.id);
      const hiddenAgain = !shownWorktrees(project).some((one) => one.path === worktree.path);
      worktree.external_hidden = held;
      return { hiddenAlone, shownWithAgent, hiddenAgain };
    });
    ok("a hidden external worktree stays listed while an agent runs in one of this window's panes",
      liveHidden.hiddenAlone && liveHidden.shownWithAgent && liveHidden.hiddenAgain, JSON.stringify(liveHidden));

    // 회상 간선(2026-09-13): 작업 상황판 ↔ 지식 그래프. 훅의 회상 추적이 적은
    // 「이 판이 이 페이지를 봤다」가 양쪽에서 문이 된다 — 제목 검색과 다른 줄.
    await page.evaluate(taskBoardFixture);
    await page.waitForSelector(".task-board-row");
    const recalled = await page.evaluate(async () => {
      const out = {};
      const now = Date.now();
      secondBrainVault = "/vault";
      window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 2, tags: ["reference"], live: {
        now_ms: now, recalled: [{ id: "wiki/Page-0001.md", at_ms: now - 1000, count: 2 }],
        bus: [
          { at_ms: now - 1000, kind: "recalled", page: "wiki/Page-0001.md", note: "term-103" },
          { at_ms: now - 5000, kind: "recalled", page: "wiki/Page-0001.md", note: "term-999" },
        ],
        merge: [], limits: { activity_window_minutes: 30, today_hours: 24, bus_rows_max: 40 } } };
      mountTermTab(103, { agent: "codex", worktree: activeWorktreePath }, { focus: false });
      const asked = [];
      window.__ANSWER__.second_brain_seat_recalls = (args) => {
        asked.push(args);
        return [{ page: "wiki/Page-0001.md", title: "첫 페이지", at_ms: now - 1000, count: 2 }];
      };
      taskBoardRecallsHeld.clear();
      knowledgeAskedAt = 0;
      const board = document.querySelector("#board-view");
      selectTaskBoardMember(board, "agent:term:103");
      await new Promise((resolve) => setTimeout(resolve, 60));
      const block = board.querySelector(".task-board-recalls");
      const recall = board.querySelector(".task-board-recall");
      out.asked = asked[0] ?? null;
      out.listed = Boolean(recall) && recall.textContent === "첫 페이지" && Boolean(block) && !block.hidden;
      recall?.click();
      return out;
    });
    await page.waitForSelector(".knowledge-node.is-selected");
    const revealed = await page.evaluate(async () => {
      await new Promise((resolve) => setTimeout(resolve, 100));
      const view = docHost(knowledgeTab().pane, "knowledge");
      const card = view.querySelector(".knowledge-card");
      const seats = [...card.querySelectorAll(".knowledge-recalled-by .knowledge-seat")];
      const bus = [...view.querySelectorAll(".knowledge-bus-row .knowledge-seat")];
      return {
        kind: currentTab()?.kind, selected: knowledgeSelectedKey, query: knowledgeQuery,
        cardShown: !card.hidden, seats: seats.map((s) => [s.textContent, s.disabled, s.dataset.knowledgeSeat]),
        busSeats: bus.map((s) => [s.disabled, s.dataset.knowledgeSeat]),
      };
    });
    ok("a task's recalled knowledge is asked for its seat and opens the graph on that page, cleared of search",
      recalled.listed && recalled.asked?.pane === "term-103" && revealed.kind === "knowledge" &&
      revealed.selected === "wiki/Page-0001.md" && revealed.query === "" && revealed.cardShown,
      JSON.stringify({ recalled, revealed }));
    ok("the page card and the bus name the panes that saw the page: an open pane is a door, a closed one only a name",
      revealed.seats.length === 2 && revealed.seats[0][1] === false && revealed.seats[0][2] === "103" &&
      revealed.seats[1][1] === true && revealed.busSeats.some(([disabled, term]) => !disabled && term === "103"),
      JSON.stringify(revealed));
    await page.click(".knowledge-recalled-by .knowledge-seat:not([disabled])");
    await page.waitForFunction(() => currentTab()?.kind === "board" && agentGraphSelectedKey === "agent:term:103");
    ok("a seat door lands on that pane's card on the task board", true);
    await page.evaluate(() => {
      dropTab(tabOfTerm(103)?.id);
      delete window.__ANSWER__.second_brain_seat_recalls;
      taskBoardRecallsHeld.clear();
    });

    ok("connected workbench raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testConnectedWorkbench(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
