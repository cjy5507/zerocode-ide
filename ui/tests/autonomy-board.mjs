import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testAutonomyBoard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const shot = resolve("output/playwright/goal-board");
  await mkdir(shot, { recursive: true });
  try {
    await page.evaluate(() => {
      const term = 6142;
      const path = projects[0].worktrees[0].path;
      mountTermTab(term, { agent: "zo", worktree: path }, { focus: false });
      paneSessions.set(term, { agent: "zo", session: { id: "goal-board" }, resumable: true });
      hookStates.set(term, "done");
      panePrompts.set(term, "목표와 반복 실행의 상태를 일관되게 표시하기");
      window.__PANES__ = [{ term, agent: "zo", state: "done", at: Date.now(),
        you: "목표와 반복 실행의 상태를 일관되게 표시하기",
        state_started_at: Date.now(), heading: "목표와 반복 실행", worktree: path }];
      window.__COLUMNS__ = [];
      // Rust owns grouping; its matching lifecycle cases live in board::tests.
      window.__ANSWER__.board_columns = ({ cards }) => ["attention", "working", "done", "idle"].map((bucket) => ({
        bucket, cards: cards.filter((card) => {
          const retired = ["released", "release_pending", "release_unknown", "sleeping"].includes(card.ledger);
          const running = card.autonomy?.goal?.phase === "running" || card.autonomy?.loops?.some((one) => one.phase === "running");
          const paused = card.autonomy?.goal?.phase === "paused" || card.autonomy?.loops?.some((one) => one.phase === "paused");
          const actual = retired ? "done" : card.state === "needs-attention" ? "attention"
            : running && ["working", "done"].includes(card.state) ? "working"
              : paused && card.state === "done" ? "idle"
                : card.state === "done" && !card.unseen ? "idle" : card.state;
          return actual === bucket;
        }),
      }));
      window.__GOAL_EVENT__ = (autonomy, session = "goal-board") => {
        for (const listener of window.__LISTENERS__["pane:autonomy"] ?? []) {
          listener({ payload: { term, session, autonomy, model: "gpt-6-astra" } });
        }
      };
      window.__GOAL__ = { phase: "running", next_at: Date.now() + 600_000,
        gates_passed: 1, gates_total: 2, action_turns: 3, stalled_turns: 0 };
      window.__LOOP__ = { id: "loop-1", phase: "running", trigger: "every 600s",
        next_at: Date.now() + 600_000, runs: 1, max_runs: 8, quiet: 0 };
      agentBoardMode = "tasks";
      openBoard();
    });
    await page.waitForSelector('.task-board-row');
    await page.evaluate(() => window.__GOAL_EVENT__({ goal: window.__GOAL__, loops: [] }));
    await page.waitForFunction(() => document.querySelector('.task-board-row.is-working .task-board-activity')?.textContent.includes("다음 실행 대기"));
    ok("metadata alone repaints a finished turn as scheduled work", true);
    const workspace = await page.evaluate(() => {
      const path = tabOfTerm(6142).worktree;
      return { hook: hookStates.get(6142), dot: worktreeDotState(path, []),
        row: agentRowState(worktreeAgentRows(path).find((one) => one.term === 6142)),
        stage: workspaceBoardStage(path) };
    });
    ok("workspace and agent row agree while the hook remains Done",
      workspace.hook === "done" && workspace.dot === "streaming" && workspace.row === "working" &&
      workspace.stage.id === "in-progress" && workspace.stage.automatic, JSON.stringify(workspace));
    await page.evaluate(() => window.__GOAL_EVENT__({ goal: { ...window.__GOAL__, phase: "saved", next_at: null },
      loops: [window.__LOOP__, { ...window.__LOOP__, id: "loop-2" }] }));
    await page.waitForFunction(() => document.querySelector('.task-board-row.is-working .task-board-activity')?.textContent.includes("loop-2"));
    const mixed = await page.locator('.task-board-row.is-working .task-board-activity').innerText();
    ok("a saved goal hides neither running loop", mixed.startsWith("루프 loop-1") && mixed.includes("loop-2") && mixed.includes("저장됨"), mixed);
    await page.setViewportSize({ width: 1440, height: 960 });
    await page.screenshot({ path: resolve(shot, "tasks-wide.png") });
    await page.setViewportSize({ width: 360, height: 800 });
    await page.evaluate(() => {
      const view = docHost(boardTab().pane, "board");
      window.__GOAL_VIEW_STYLE__ = view.getAttribute("style") ?? "";
      Object.assign(view.style, { position: "fixed", inset: "0", width: "360px", height: "800px", zIndex: "100" });
    });
    await page.screenshot({ path: resolve(shot, "tasks-narrow.png") });
    ok("multiple loops do not overflow a narrow task row", await page.evaluate(() => {
      const row = document.querySelector('.task-board-row');
      return row.scrollWidth <= row.clientWidth + 1 && row.getBoundingClientRect().right <= 361;
    }));
    await page.evaluate(() => docHost(boardTab().pane, "board").setAttribute("style", window.__GOAL_VIEW_STYLE__));
    await page.setViewportSize({ width: 1440, height: 960 });
    await page.evaluate(() => {
      window.__PAUSED__ = { goal: { ...window.__GOAL__, phase: "paused", next_at: null,
        pause_reason: "자동 진행이 꺼져 있습니다. <script>잘못된 실행 없음</script>" }, loops: [] };
      window.__PAUSE_STATE__ = (state, reason) => {
        hookStates.set(6142, state);
        window.__PANES__[0].state = state;
        window.__PANES__[0].state_started_at = Date.now();
        window.__GOAL_EVENT__({ ...window.__PAUSED__, goal: { ...window.__PAUSED__.goal, pause_reason: reason } });
      };
      window.__GOAL_EVENT__(window.__PAUSED__);
    });
    await page.waitForSelector('.task-board-row.is-paused');
    await page.waitForSelector('.wt-agent[data-term="6142"].is-paused');
    ok("a paused goal shows its reason as text without claiming completion", (await page.locator('.task-board-activity').innerText()).includes("자동 진행이 꺼져 있습니다. <script>"));
    const paused = await page.evaluate(() => {
      const path = tabOfTerm(6142).worktree;
      const dot = document.querySelector('.wt-dot.is-paused');
      const row = document.querySelector('.wt-agent[data-term="6142"]');
      const mark = row.querySelector('.wt-agent-dot');
      const view = docHost(boardTab().pane, "board");
      const tasks = taskBoardModels.get(view);
      const model = agentGraphModels.get(view);
      const entry = model.agents.find((one) => one.card.pane === "term:6142");
      return { dot: worktreeDotState(path, []), row: agentRowState(worktreeAgentRows(path).find((one) => one.term === 6142)),
        glyph: getComputedStyle(dot, '::before').content, dotBorder: getComputedStyle(dot, '::before').borderLeftWidth,
        markBorder: getComputedStyle(mark).borderLeftWidth, tip: dot.dataset.tip,
        state: entry.state, hidden: model.entities.get(entry.key).hiddenByIdle,
        attention: tasks.counts.get("attention"), paused: tasks.counts.get("paused"),
        section: document.querySelector('.task-board-row.is-paused').closest('[data-task-section]').dataset.taskSection,
        stage: workspaceBoardStage(path).id,
        mixed: foldWorktreeState("empty", ["paused", "working"], true),
        question: foldWorktreeState("empty", ["paused", "working", "needs-attention"], true) };
    });
    ok("paused work has its own visible section, never a question or completed history",
      paused.state === "paused" && !paused.hidden && paused.section === "paused" && paused.paused === 1 && paused.attention === 0 && paused.stage === "in-progress", JSON.stringify(paused));
    ok("workspace and agent rows draw pause bars rather than the question mark",
      paused.dot === "paused" && paused.row === "paused" && paused.glyph !== '"?"' &&
      paused.dotBorder === "2px" && paused.markBorder === "2px" && paused.tip === "일시 정지", JSON.stringify(paused));
    ok("real work outranks a pause and a real question outranks both",
      paused.mixed === "streaming" && paused.question === "waiting", JSON.stringify(paused));
    await page.screenshot({ path: resolve(shot, "paused.png"), animations: "disabled" });
    await page.click('[data-board-mode="graph"]');
    // 관계 탭은 행성계로 열린다(t-9444) — 이 검사는 카드 그림의 상태 줄을 읽는다.
    await page.click('[data-relations-view="cards"]');
    await page.waitForSelector('.agent-graph-node.is-agent.is-paused .agent-graph-node-status');
    ok("the relations graph keeps the paused goal visible with its actual status label", (await page.locator('.agent-graph-node.is-agent.is-paused .agent-graph-node-status').innerText()).startsWith("일시 정지"));
    await page.screenshot({ path: resolve(shot, "paused-graph.png"), animations: "disabled" });
    await page.click('[data-board-mode="tasks"]');
    await page.evaluate(() => window.__PAUSE_STATE__("working", "목표 자동 진행만 일시 정지"));
    await page.waitForSelector('.task-board-row.is-working');
    await page.waitForSelector('.wt-agent[data-term="6142"].is-working');
    ok("a manually running turn is working even while its goal remains paused", await page.evaluate(() => {
      const path = tabOfTerm(6142).worktree;
      return worktreeDotState(path, []) === "streaming" && document.querySelector('.wt-dot.is-working') &&
        document.querySelector('.task-board-state .is-working') && !document.querySelector('.task-board-row.is-attention');
    }));
    await page.evaluate(() => window.__PAUSE_STATE__("needs-attention", "실제 입력 요청도 발생함"));
    await page.waitForSelector('.task-board-row.is-attention');
    await page.waitForSelector('.wt-agent[data-term="6142"].is-waiting');
    ok("a real question retains its question icon despite paused autonomy", await page.evaluate(() =>
      getComputedStyle(document.querySelector('.wt-dot.is-permission'), '::before').content === '"?"' &&
      document.querySelector('.wt-agent[data-term="6142"] use')?.getAttribute('href') === "#i-msg-ask" &&
      document.querySelector('.task-board-state .is-needs-attention')));
    await page.evaluate(() => {
      hookStates.set(6142, "done");
      window.__PANES__[0].state = "done";
      window.__PANES__[0].state_started_at = Date.now();
      window.__GOAL_EVENT__({ goal: { ...window.__GOAL__, phase: "saved", next_at: null },
        loops: [{ ...window.__LOOP__, phase: "paused", next_at: null, reason: "사용자가 반복 실행을 멈춤" }] });
    });
    await page.waitForSelector('.task-board-row.is-paused');
    ok("a paused loop also remains visible without asking a question", (await page.locator('.task-board-activity').innerText()).includes("사용자가 반복 실행을 멈춤") &&
      await page.locator('.task-board-row.is-attention').count() === 0);
    await page.evaluate(() => window.__GOAL_EVENT__({ goal: { ...window.__GOAL__, phase: "completed", next_at: null },
      loops: [{ ...window.__LOOP__, phase: "stopped", next_at: null, reason: "사용자가 중단함" }] }));
    await page.waitForFunction(() => document.querySelector('.task-board-activity')?.textContent.includes("사용자가 중단함"));
    const ended = await page.locator('.task-board-activity').innerText();
    ok("goal achievement and a stopped loop retain distinct end states", ended.includes("달성") && ended.includes("중단됨") && !ended.includes("검증 완료"), ended);
    await page.evaluate(() => window.__GOAL_EVENT__(null));
    await page.waitForFunction(() => !document.querySelector('.task-board-activity')?.textContent.includes("달성"));
    ok("an explicit clear removes the goal without a lifecycle hook", await page.evaluate(() => paneAutonomyValue(6142) === null && hookStates.get(6142) === "done"));
    const boundaries = await page.evaluate(() => {
      const running = { goal: window.__GOAL__, loops: [] };
      window.__GOAL_EVENT__(running);
      const path = tabOfTerm(6142).worktree;
      workspaceBoardSettings.cards[path] = { status: "todo", pinned: false };
      const manual = workspaceBoardStage(path);
      hookStates.set(6142, "needs-attention");
      window.__GOAL_EVENT__({ ...running, loops: [window.__LOOP__] });
      const question = autonomousPaneState(6142, hookStates.get(6142));
      hookStates.set(6142, "idle");
      const gone = autonomousPaneState(6142, "idle");
      hookStates.set(6142, "done");
      paneLedger.set(6142, { ledger: "released" });
      const released = autonomousPaneState(6142, "working");
      paneLedger.delete(6142);
      paneSessions.set(6142, { session: { id: "replacement" } });
      window.__GOAL_EVENT__(running);
      const foreign = paneAutonomyValue(6142);
      paneAutonomy.delete(6142);
      reconcilePaneAgentSnapshot([{ ...window.__PANES__[0], autonomy: running }]);
      const hydrated = paneAutonomyValue(6142);
      window.__GOAL_EVENT__(null, "replacement");
      reconcilePaneAgentSnapshot([{ ...window.__PANES__[0], autonomy: running }]);
      const stale = paneAutonomyValue(6142);
      return { manual, question, gone, released, foreign, hydrated, stale };
    });
    ok("manual planning, questions, exited panes and retired workers outrank stale goals",
      !boundaries.manual.automatic && boundaries.manual.id === "todo" && boundaries.question === "needs-attention" &&
      boundaries.gone === "idle" && boundaries.released === "done", JSON.stringify(boundaries));
    ok("session switch rejects the old sender; hydration cannot undo a live clear",
      boundaries.foreign === null && boundaries.hydrated?.goal?.phase === "running" && boundaries.stale === null, JSON.stringify(boundaries));
    await page.evaluate(() => {
      window.__GOAL_EVENT__({ goal: window.__GOAL__, loops: [window.__LOOP__] }, "replacement");
      setWorkspaceBoardOpen(true);
    });
    await page.waitForSelector('.workspace-board-card');
    await page.waitForFunction(() => document.querySelector('.workspace-board-card-live')?.textContent.includes('다음 실행 대기'));
    await page.evaluate(() => window.__GOAL_EVENT__({ goal: { ...window.__GOAL__, gates_passed: 2 }, loops: [window.__LOOP__] }, "replacement"));
    await page.waitForFunction(() => document.querySelector('.workspace-board-card-live')?.textContent.includes('게이트 2/2'));
    ok("workspace metadata-only updates repaint without overriding its manual stage", await page.locator('.workspace-board-card-stage[data-source="manual"]').count() === 1);
    await page.waitForFunction(() => getComputedStyle(document.querySelector('#workspace-board')).opacity === '1');
    await page.screenshot({ path: resolve(shot, "workspace-manual.png"), animations: "disabled" });
    await page.evaluate(() => {
      setWorkspaceBoardOpen(false);
      secondBrainVault = "/vault";
      window.__VAULT__ = { pages: 4, linksPer: 1, ghosts: 0, tags: ["reference"] };
      paintKnowledgeEntry();
      window.__ANSWER__.second_brain_seat_recalls = () => [{ page: "wiki/Page-0001.md", title: "처음 참고한 지식" }];
      openBoard();
    });
    await page.locator('.task-board-row').click();
    await page.waitForSelector('.task-board-recall[data-knowledge-page="wiki/Page-0001.md"]');
    await page.evaluate(() => {
      window.__ANSWER__.second_brain_seat_recalls = () => [{ page: "wiki/Page-0002.md", title: "새로 참고한 지식" }];
      for (const listener of window.__LISTENERS__["second-brain:changed"] ?? []) listener({ payload: {} });
    });
    await page.waitForSelector('.task-board-recall[data-knowledge-page="wiki/Page-0002.md"]');
    ok("a knowledge change refreshes actual recalls without reopening the inspector",
      await page.locator('.task-board-recall[data-knowledge-page="wiki/Page-0001.md"]').count() === 0);
    await page.locator('.task-board-recall[data-knowledge-page="wiki/Page-0002.md"]').click();
    await page.waitForFunction(() => knowledgeSelectedKey === "wiki/Page-0002.md");
    ok("the recall opens its exact knowledge node rather than a title search", await page.evaluate(() => knowledgeQuery === ""));
    await page.locator('.knowledge-node.is-selected').waitFor({ state: 'visible' });
    await page.screenshot({ path: resolve(shot, "knowledge-recall.png"), animations: "disabled" });
    ok("autonomy rendering raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testAutonomyBoard(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
