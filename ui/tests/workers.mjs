import { readFile } from "node:fs/promises";
import { openWindowTestPage, WINDOW_MOTION_REST } from "./window-boot.mjs";

/* Workers and helpers, on a page of their own (t-4017).
 *
 * These scenarios seat workers, split helper panes and read the sidebar's
 * agent rows for the ACTIVE workspace — and a worker seat refreshes the
 * catalog, which re-roots the window to the catalog's active workspace. In
 * the shared flow that made them a state protocol nobody had written down:
 * a new case (`workerFold`) moved the active path and broke three cases
 * 2,300 lines later (v1.3.57); putting the path back broke nine helper-row
 * cases whose card had stood on the moved path (v1.3.58); only moving the
 * case 33,000 lines up went green (f802dba1). Where a case stood decided
 * what it measured.
 *
 * Here they stand on a page that is theirs: the window boots, the projects
 * expand (the footing the shared flow gives every scenario), the roster
 * cases run first on the fresh window's card, the seat cases last — and the
 * page dies with the suite, so nothing they moved (the active workspace,
 * `paneHelpers`, `paneParents`, the tabs) is put back for a neighbour. The
 * checks are the shared flow's, word for word. */
export async function testWorkers({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    // The suite works in expanded projects, as after a person opens them —
    // the same footing the shared window flow stands on.
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(async () => { setEveryProjectClosed(false); await refreshWorktrees(); });

    /* ---- 프로세스 안의 서브에이전트 (1-eo) ----------------------------------
     *
     * claude가 background agent 다섯을 띄웠는데 아무 데도 안 보였다. tmux를 안
     * 부르니 판이 갈리지 않는 것이 맞고(Orca도 팀·tmux만 분할한다), 대신 부모
     * 아래 **행**으로 서 있어야 한다. 훅은 이미 오고 있었다 — `hook_state`가
     * SubagentStart에 대해 일부러 아무 말도 하지 않으므로 전달 루프에서 그대로
     * 떨어지고 있었을 뿐이다. */
    const subRows = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const rows = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      const helpers = (list) => {
        for (const handler of window.__LISTENERS__["hook:subagent"] ?? []) {
          handler({ payload: { term, rows: list } });
        }
      };

      // The parent says something first, the way a real one does. 그림은 프레임에
      // 한 번 모이므로(1-ep) 읽기 전에 그 한 번을 지나 보낸다.
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
        handler({ payload: { term, state: "working", agent: "claude", session: "s-1" } });
      }
      await window.__PAINTED__();
      seen.beforeRows = rows().length;

      // Five workers, one event. Nothing is fetched to draw them — 그림 한 번까지
      // 포함해서. 앞의 훅이 이미 문의 배지를 예약해 두었으므로 자는 그 그림이
      // 지나간 **뒤에** 놓는다.
      window.__COUNTS__ = {};
      helpers(
        ["arch", "shell", "ui", "pty", "core"].map((name) => ({ id: `t-${name}`, name: `@${name}` })),
      );
      await window.__PAINTED__();
      seen.askedNothing = Object.keys(window.__COUNTS__).length;
      // 실패가 났을 때 "하나 물었다"가 아니라 **무엇을** 물었는지 말하게 한다.
      seen.asked = Object.keys(window.__COUNTS__);
      seen.names = rows().map((one) => one.textContent);
      seen.depths = rows().map((one) => one.style.getPropertyValue("--card-depth") || "0");
      // A helper is always running: the parent going quiet must not grey out five
      // workers that are still going.
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
        handler({ payload: { term, state: "done", agent: "claude", session: "s-1" } });
      }
      await window.__PAINTED__();
      seen.parentDone = rows()[0]?.className ?? "";
      seen.stillWorking = rows()
        .slice(1)
        .every((one) => one.className.includes("is-working"));

      // The board hangs them off the pane they run in.
      window.__PANES__ = [{ term, agent: "claude", state: "done", at: 5, resumable: false }];
      const cards = cardsFromPanes(window.__PANES__);
      seen.cardIds = cards.map((one) => one.pane);
      seen.cardParents = cards.map((one) => one.parent);

      // Two stop. The rows go with them — a stop is what takes a row away.
      helpers([{ id: "t-arch", name: "@arch" }, { id: "t-core", name: "@core" }]);
      await window.__PAINTED__();
      seen.afterStop = rows().map((one) => one.dataset.sub ?? "");
      // And the last one stopping leaves the parent standing alone — the pane is
      // still there, it just has nobody under it any more.
      helpers([]);
      await window.__PAINTED__();
      seen.emptied = rows().map((one) => one.dataset.sub ?? "");

      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });

    ok(
      "the helpers an agent runs inside its own process are rows under it, and cost nothing to draw",
      subRows.beforeRows === 1 &&
        subRows.askedNothing === 0 &&
        subRows.names.length === 6 &&
        subRows.names.slice(1).every((said, i) => said.includes(["@arch", "@shell", "@ui", "@pty", "@core"][i])) &&
        // These helpers are direct children of the pane, so each is one
        // generation below the parent and shares its depth.
        JSON.stringify(subRows.depths) === JSON.stringify(["0", "1", "1", "1", "1", "1"]),
      JSON.stringify(subRows),
    );
    ok(
      "a helper is running until it stops, and stopping is what takes its row away",
      subRows.parentDone.includes("is-done") &&
        subRows.stillWorking &&
        // The three that were not stopped: the parent's own row, and the two
        // helpers the new list still names.
        JSON.stringify(subRows.afterStop) === JSON.stringify(["", "t-arch", "t-core"]) &&
        JSON.stringify(subRows.emptied) === JSON.stringify([""]),
      JSON.stringify(subRows),
    );
    ok(
      "and on the board they are children of the pane they run in, not roots beside it",
      subRows.cardIds.length === 6 &&
        subRows.cardIds[0] === `term:${subRows.cardIds[1].split(":")[1]}` &&
        subRows.cardParents.slice(1).every((one) => one === subRows.cardIds[0]) &&
        subRows.cardParents[0] === "",
      JSON.stringify(subRows),
    );

    /* A helper has no pane of its own. Its navigator row used to be a door to
     * the parent terminal where its work is running — the pane already in
     * front of the person, so the press seemed to do nothing ("눌러도 열리지
     * 않는다", t-11827). The row now opens the helper's own conversation when
     * the vendor left one; `sidebar-agents` holds the road without one. */
    const helperRowFocus = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const rows = () => [
        ...(document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`)
          ?.querySelectorAll(".wt-agent") ?? []),
      ];
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-1" });
      tell("hook:subagent", { term, rows: [{ id: "kept", name: "@kept" }] });
      await window.__PAINTED__();

      let logAsked = 0;
      window.__ANSWER__.subagent_log = () => {
        logAsked += 1;
        return {
          found: true,
          next: 120,
          turns: [{ role: "assistant", text: "a transcript exists" }],
        };
      };
      const row = rows().find((one) => one.dataset.sub === "kept");
      openTab({ id: `t2163-pane-decoy:${term}`, kind: "board" });
      row?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.helperPage = activeTabId === `helper:${term}:kept`;
      seen.logAsked = logAsked;

      delete window.__ANSWER__.subagent_log;
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });

    ok(
      "a helper row opens that helper's own conversation page",
      helperRowFocus.helperPage && helperRowFocus.logAsked === 1,
      JSON.stringify(helperRowFocus),
    );

    /* 끝난 헬퍼는 남는다 — 병렬로 다섯을 띄운 사람이 하나씩 끝날 때마다 행과
     * 페이지가 사라지는 것을 보았다. 명부가 `done`으로 말하는 행은 완료로 서고,
     * 그 페이지는 실행 중이라 하지 않으며, 보드 카드도 완료를 입는다. */
    const doneHelpers = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-done" });
      tell("hook:subagent", {
        term,
        rows: [{ id: "a-done", name: "finished-one", state: "done" }, { id: "a-live", name: "live-one", state: "running" }],
      });
      await window.__PAINTED__();
      // 끝난 헬퍼는 기본으로 「완료 1개」한 줄 뒤에 접힌다(t-2614) — 행은 그대로
      // 명부에 있고, 펼치면 제 상태로 선다. 도는 헬퍼는 그 접힘과 무관하다.
      const folded = worktreeAgentRows(owner.worktree);
      seen.doneFolded =
        folded.some((row) => row.history === 1 && !row.historyShown) &&
        !folded.some((row) => row.sub?.id === "a-done");
      agentHistoryShown.add(term);
      const rows = worktreeAgentRows(owner.worktree);
      const doneRow = rows.find((row) => row.sub?.id === "a-done");
      const liveRow = rows.find((row) => row.sub?.id === "a-live");
      seen.bothListed = Boolean(doneRow) && Boolean(liveRow);
      seen.doneState = doneRow ? agentRowState(doneRow) : null;
      seen.liveState = liveRow ? agentRowState(liveRow) : null;
      agentHistoryShown.delete(term);
      // The page of a finished helper opens, and does not call its tail running.
      window.__ANSWER__.subagent_log = () => ({
        found: true,
        next: 2,
        turns: [{ role: "assistant", text: "done here" }, { role: "tool", text: "read_file · /a.rs", tool: { name: "read_file", kind: "read" } }],
      });
      await openHelperPage(
        { term, agent: "zo", worktree: owner.worktree, tab: owner },
        { id: "a-done", name: "finished-one", state: "done" },
      );
      await new Promise((done) => setTimeout(done, 120));
      const pageTab = tabs.find((tab) => tab.id === `helper:${term}:a-done`);
      seen.pageStatus = pageTab?.worker.status ?? null;
      const face = document.querySelector("#worker-view");
      // A finished helper's page has no call out and no status line — the tail
      // tool row stands plain under its kind's word.
      seen.noRunningTail = face?.querySelector(".is-live") === null &&
        face?.querySelector(".helper-status")?.hidden === true &&
        face?.querySelector(".helper-turn.is-tool .helper-step-kind")?.textContent === t("worker.stepRead", "파일 읽기");
      // The live one finishes: its open page follows the roster.
      await openHelperPage(
        { term, agent: "zo", worktree: owner.worktree, tab: owner },
        { id: "a-live", name: "live-one", state: "running" },
      );
      await new Promise((done) => setTimeout(done, 120));
      const liveTab = tabs.find((tab) => tab.id === `helper:${term}:a-live`);
      seen.liveOpenedRunning = liveTab?.worker.status === "running";
      tell("hook:subagent", {
        term,
        rows: [{ id: "a-done", name: "finished-one", state: "done" }, { id: "a-live", name: "live-one", state: "done" }],
      });
      seen.liveTurnedDone = liveTab?.worker.status === "done";
      delete window.__ANSWER__.subagent_log;
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a finished helper keeps its row as done, its page opens without a running tail, and an open page follows the roster to done",
      doneHelpers.doneFolded && doneHelpers.bothListed && doneHelpers.doneState === "done" &&
        doneHelpers.liveState === "working" &&
        doneHelpers.pageStatus === "done" && doneHelpers.noRunningTail &&
        doneHelpers.liveOpenedRunning && doneHelpers.liveTurnedDone,
      JSON.stringify(doneHelpers),
    );

    /* 한 신원, 한 행(t-3024). zo 판 레인의 서브에이전트는 두 길로 온다 — 훅이
     * 세운 헬퍼 행(`hook:subagent`)과 split이 세운 판 행(`term:split`) — 그리고
     * 사용자는 같은 헬퍼를 두 줄로 보았다(09-07 18:40 스크린샷: `code-reviewer·…`
     * 와 `ZO claude-fable-5…`). `term:split`이 실어 온 헬퍼 id가 둘을 잇는다: 판
     * 행이 표면이고 헬퍼의 이름과 전사 손을 이어받으며, 셰브론은 한 쌍을 한 번만
     * 센다. 판이 떠나고 헬퍼가 끝나면 오늘처럼 「완료 1개」로 간다. id 없는
     * split(진짜 팀원 판)은 오늘처럼 두 길 다 선다. */
    const foldedHelper = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const nodes = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      const under = () => worktreeAgentRows(path).filter((row) => row.depth === 1 && !row.history);
      const parentKids = () => worktreeAgentRows(path).find((row) => row.term === term && row.depth === 0)?.kids;
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-fold" });
      // 길 1: 훅이 헬퍼를 세운다.
      tell("hook:subagent", { term, rows: [{ id: "agent-1", name: "code-reviewer·X", state: "running", tool_calls: 3 }] });
      // 길 2: 같은 헬퍼의 판이 갈라지고, 그 판이 제 훅으로 말한다(실제 순서).
      const child = term + 7000;
      tell("term:split", { parent: term, term: child, direction: "vertical", agent: "zo", helper: "agent-1" });
      tell("hook:agent", { term: child, state: "working", agent: "zo", session: "s-child" });
      await window.__PAINTED__();
      const rows = under();
      seen.childCount = rows.length;
      seen.childTerm = rows[0]?.term ?? null;
      seen.childSub = rows[0]?.sub?.id ?? null;
      seen.childName = rows[0] ? agentRowPrimary(rows[0], agentRowState(rows[0])) : null;
      // 판이므로 상태는 제 훅의 것이고, 활동 카드도 제 판의 것.
      seen.childState = rows[0] ? agentRowState(rows[0]) : null;
      seen.childPane = rows[0] ? agentRowPane(rows[0]) : null;
      seen.kids = parentKids();
      // 그림도 한 줄: 판의 term을 달고, 헬퍼의 이름과 전사 손을 들고.
      seen.drawn = nodes().length;
      const node = nodes().find((one) => one.dataset.term === String(child)) ?? null;
      seen.drawnSub = node?.dataset.sub ?? null;
      seen.drawnName = node?.querySelector(".wt-agent-name")?.textContent ?? null;
      seen.drawnHand = node?.querySelector(".wt-agent-peek") !== null;
      seen.drawnUses = node?.querySelector(".wt-agent-uses")?.textContent ?? "";
      // 손은 헬퍼 명부의 주인 — 부모 — 의 페이지를 연다.
      window.__ANSWER__.subagent_log = (args) => ({
        found: args.term === term && args.id === "agent-1",
        next: 1,
        turns: [{ role: "assistant", text: "reviewing" }],
      });
      node?.querySelector(".wt-agent-peek")?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.pageOpened = tabs.some((tab) => tab.id === `helper:${term}:agent-1`);
      seen.noChildPage = !tabs.some((tab) => tab.id === `helper:${child}:agent-1`);
      delete window.__ANSWER__.subagent_log;
      // 판이 떠나고 헬퍼가 끝난다 — 오늘처럼 「완료 1개」, 도는 자식은 없다.
      tell("term:exited", { term: child });
      tell("hook:subagent", { term, rows: [{ id: "agent-1", name: "code-reviewer·X", state: "done" }] });
      await window.__PAINTED__();
      seen.history = worktreeAgentRows(path).find((row) => row.history === 1)?.history ?? 0;
      seen.runningAfter = under().length;
      seen.helperForgotten = !paneHelpers.has(child);
      // id 없는 split은 오늘 그대로 — 진짜 팀원 판 하나 + 무관한 헬퍼 하나 = 두 행.
      tell("hook:subagent", { term, rows: [{ id: "agent-2", name: "@other", state: "running" }] });
      const mate = term + 7001;
      tell("term:split", { parent: term, term: mate, direction: "vertical", agent: "claude" });
      tell("hook:agent", { term: mate, state: "working", agent: "claude", session: "s-mate" });
      await window.__PAINTED__();
      seen.bothRoads = under().map((row) => (row.sub ? `sub:${row.sub.id}` : `term:${row.term}`)).sort();
      seen.bothKids = parentKids();
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term: mate });
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "one identity, one row: a zo pane-lane helper's hook row folds into its pane row, which wears the helper's name and transcript hand, and unfolds into the done history when the pane leaves",
      foldedHelper.childCount === 1 &&
        typeof foldedHelper.childTerm === "number" &&
        foldedHelper.childSub === "agent-1" &&
        foldedHelper.childName === "code-reviewer·X" &&
        foldedHelper.childState === "working" &&
        foldedHelper.childPane === `term:${foldedHelper.childTerm}` &&
        foldedHelper.kids === 1 &&
        foldedHelper.drawn === 2 &&
        foldedHelper.drawnSub === "agent-1" &&
        foldedHelper.drawnName === "code-reviewer·X" &&
        foldedHelper.drawnHand &&
        foldedHelper.drawnUses.includes("3") &&
        foldedHelper.pageOpened && foldedHelper.noChildPage &&
        foldedHelper.history === 1 && foldedHelper.runningAfter === 0 && foldedHelper.helperForgotten &&
        JSON.stringify(foldedHelper.bothRoads) === JSON.stringify([`sub:agent-2`, `term:${foldedHelper.childTerm + 1}`]) &&
        foldedHelper.bothKids === 2,
      JSON.stringify(foldedHelper),
    );

    /* 헬퍼 판이 끝나면 그 행은 사이드바에서 떠나고 부모의 완료 셈만 남는다(t-3098).
     *
     * 사용자 09-07 23:5x 「agent done인데 계속 표시됨」: zo 팀메이트 판이 23:35:18에
     * 떠났는데(창 로그 `term 6 ended`) 「완료 1개」 옆에 헬퍼 행이 남았다. 창의
     * 결함이 아니었다 — 백엔드 명부가 소환 하나를 두 행으로 실어 왔다: zo의
     * SubagentStart/Stop 쌍은 **스폰 호출**을 감싸 배경 Agent가 돌아오는 순간
     * 스폰 행(도구 호출 id)이 「완료」로 은퇴했고, 자식의 진짜 행(agent id,
     * split이 `ZO_AGENT_ID`로 실은 그 id)은 판이 떠난 뒤에도 다음 롤콜까지
     * 「도는 중」이었다. 고침은 명부(백엔드)에: 스폰을 닫는 stop은 행을 지울
     * 뿐 아무것도 끝내지 않고(`close_helper_row`), 헬퍼의 판이 끝나면 그 판이
     * 곧 헬퍼이므로 부모 명부에서 은퇴한다(`forget_term_state`→`publish_owed_
     * rosters`). 이 핀은 창이 그 명부를 받았을 때의 그림을 고정한다: 판이
     * 떠난 뒤 서는 것은 부모와 「완료 1개」뿐이고, 부모의 셰브론과 「완료 1개」를
     * 풀었다 닫아도 헬퍼 행은 되살아나지 않으며, 펼친 이력은 소환 하나에 행
     * 하나 — 헬퍼의 이름과 전사 손을 든 그 한 줄이다. */
    const helperPaneLeft = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const nodes = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      // 사이드바에 선 것 — 부모 행 / 헬퍼 행(sub) / 「완료 N개」 이력 줄.
      const standing = () => nodes().map((one) =>
        one.dataset.history ? `history:${one.dataset.history}`
          : one.dataset.sub ? `sub:${one.dataset.sub}:${one.dataset.term}`
          : `term:${one.dataset.term}`);
      const settle = async () => { paintWorktreeAgents(); await window.__PAINTED__(); };
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-left" });
      // 소환: 명부는 자식을 agent id 하나로 싣고(스폰 행은 이제 서지 않는다),
      // split이 같은 id를 실어 판 행이 접는다.
      tell("hook:subagent", { term, rows: [{ id: "agent-1788790805", name: "stage-art", state: "running", tool_calls: 2 }] });
      const child = term + 7100;
      tell("term:split", { parent: term, term: child, direction: "vertical", agent: "zo", helper: "agent-1788790805" });
      tell("hook:agent", { term: child, state: "working", agent: "zo", session: "s-child-left" });
      await window.__PAINTED__();
      seen.whileRunning = standing();
      // 판이 떠난다: 백엔드가 그 판을 헬퍼로 알아 부모 명부에서 은퇴시키고
      // (`term:exited` 전후 어느 쪽이든 한 펌프 박자 안), 창은 판 종료를 본다.
      tell("hook:subagent", { term, rows: [{ id: "agent-1788790805", name: "stage-art", state: "done", tool_calls: 9 }] });
      tell("term:exited", { term: child });
      await window.__PAINTED__();
      seen.afterLeft = standing();
      seen.helperForgotten = !paneHelpers.has(child) && !paneParents.has(child);
      const parentNode = () => nodes().find((one) => one.dataset.term === String(term) && !one.dataset.sub && !one.dataset.history) ?? null;
      const historyNode = () => nodes().find((one) => one.dataset.history) ?? null;
      // 부모의 셰브론을 접었다 편다 — 헬퍼 행은 되살아나지 않는다.
      parentNode()?.querySelector(".wt-agent-fold")?.click();
      await settle();
      seen.parentFolded = standing();
      parentNode()?.querySelector(".wt-agent-fold")?.click();
      await settle();
      seen.parentUnfolded = standing();
      // 「완료 1개」를 펼친다 — 소환 하나에 행 하나: 헬퍼의 이름과 전사 손.
      historyNode()?.querySelector(".wt-agent-fold")?.click();
      await settle();
      seen.historyOpen = standing();
      const shown = nodes().find((one) => one.dataset.sub) ?? null;
      seen.shownName = shown?.querySelector(".wt-agent-name")?.textContent ?? null;
      seen.shownHand = shown?.querySelector(".wt-agent-peek") !== null;
      seen.shownDone = shown?.classList.contains("is-done") ?? false;
      // 닫는다 — 되살아나지 않는다.
      historyNode()?.querySelector(".wt-agent-fold")?.click();
      await settle();
      seen.historyClosed = standing();
      // 늦게 온 같은 명부(다음 롤콜)는 그림을 바꾸지 않는다.
      tell("hook:subagent", { term, rows: [{ id: "agent-1788790805", name: "stage-art", state: "done", tool_calls: 9 }] });
      await window.__PAINTED__();
      seen.afterRollCall = standing();
      seen.term = term;
      seen.child = child;
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    const helperPaneSaid = (rows) => JSON.stringify(rows);
    const helperPaneParent = `term:${helperPaneLeft.term}`;
    ok(
      "a helper pane that ended leaves the sidebar: only the parent and 「완료 1개」 stand, folding the parent and opening and closing the done history never stands a second row for the one summons, and the opened history is that one helper with its name and transcript hand",
      helperPaneSaid(helperPaneLeft.whileRunning) === helperPaneSaid([helperPaneParent, `sub:agent-1788790805:${helperPaneLeft.child}`]) &&
        helperPaneSaid(helperPaneLeft.afterLeft) === helperPaneSaid([helperPaneParent, "history:1"]) &&
        helperPaneLeft.helperForgotten &&
        helperPaneSaid(helperPaneLeft.parentFolded) === helperPaneSaid([helperPaneParent]) &&
        helperPaneSaid(helperPaneLeft.parentUnfolded) === helperPaneSaid(helperPaneLeft.afterLeft) &&
        helperPaneSaid(helperPaneLeft.historyOpen) === helperPaneSaid([helperPaneParent, `sub:agent-1788790805:${helperPaneLeft.term}`, "history:1"]) &&
        helperPaneLeft.shownName === "stage-art" && helperPaneLeft.shownHand && helperPaneLeft.shownDone &&
        helperPaneSaid(helperPaneLeft.historyClosed) === helperPaneSaid(helperPaneLeft.afterLeft) &&
        helperPaneSaid(helperPaneLeft.afterRollCall) === helperPaneSaid(helperPaneLeft.afterLeft),
      JSON.stringify(helperPaneLeft),
    );

    /* 도는 헬퍼의 행은 이름과 상태만이 아니라 **무엇을 하고 있는지와 몇 번**을
     * 말한다 — Claude Code가 도는 Task 줄에 다는 그 둘("12 tool uses").
     *
     * 활동 줄은 부모 행이 쓰는 그 함수(`activityLine`)를 그대로 쓴다: 헬퍼에게는
     * 제 판이 없으므로 카드 이름만 `sub:<term>:<id>`로 갈라지고, 그 뒤는 같은
     * 길이다. 수는 명부가 나르는 한 필드이고 낱말은 카탈로그 한 열쇠에서 온다 —
     * 0은 낱말이 아니다(아직 아무것도 집지 않은 헬퍼에게 「0 tool uses」를 다는
     * 것은 빈칸을 낱말로 채우는 일이다). */
    const helperDoing = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const helper = () =>
        [...(card()?.querySelectorAll(".wt-agent") ?? [])].find((one) => one.dataset.sub === "a-1")
        ?? null;
      const usesOf = () => helper()?.querySelector(".wt-agent-uses")?.textContent ?? null;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      // 언어만 갈아입힌다 — `refresh`는 워크트리 목록을 다시 받아 오고 `persist`는
      // 설정을 저장하므로, 낱말을 재려고 그 둘을 켜면 이 검사가 옆 검사의 상태를
      // 바꾼다.
      const wear = (code) => setLocale(code, { refresh: false, persist: false });
      const wore = locale;
      wear("en");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-uses" });

      // 아직 아무 도구도 집지 않은 헬퍼: 수는 서지 않는다.
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch", tool_calls: 0 }] });
      await window.__PAINTED__();
      seen.noneYet = usesOf();

      // 셸이 그 헬퍼의 카드로 활동을 보내고, 명부가 같은 헬퍼의 수를 나른다.
      tell("hook:activity", {
        pane: `sub:${term}:a-1`,
        activities: [{ seq: 0, activity: { verb: "read", target: "src/x.rs", phase: "started" } }],
      });
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch", tool_calls: 12 }] });
      await window.__PAINTED__();
      seen.said = helper()?.querySelector(".wt-agent-said")?.textContent ?? "";
      seen.wantSaid = activityLine(`sub:${term}:a-1`);
      seen.uses = usesOf();
      // 잘린 줄의 나머지를 읽을 곳은 툴팁 하나뿐이므로, 거기에는 둘 다 선다.
      seen.tip = helper()?.dataset.tip ?? "";

      // 한 번은 단수로 — 카탈로그가 그 자리를 정한다.
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch", tool_calls: 1 }] });
      await window.__PAINTED__();
      seen.once = usesOf();

      // 네 카탈로그. 낱말이 언어를 따라 바뀌면 그것은 그리는 손에 박힌 문자열이
      // 아니라 카탈로그에서 온 것이다.
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch", tool_calls: 12 }] });
      await window.__PAINTED__();
      seen.words = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        wear(code);
        paintWorktreeAgents();
        seen.words[code] = usesOf();
      }
      wear("en");
      paintWorktreeAgents();

      // 그리고 그 헬퍼의 페이지 머리도 같은 수를 말하고, 명부가 수를 올리면
      // 상태가 그대로여도 따라간다.
      window.__ANSWER__.subagent_log = () => ({
        found: true,
        next: 1,
        turns: [{ role: "assistant", text: "reading" }],
      });
      await openHelperPage(
        { term, agent: "zo", worktree: path, tab: owner },
        { id: "a-1", name: "@arch", state: "running", tool_calls: 12 },
      );
      await new Promise((done) => setTimeout(done, 120));
      const headUses = () => document.querySelector("#worker-view .worker-uses")?.textContent ?? null;
      seen.head = headUses();
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch", tool_calls: 30 }] });
      await new Promise((done) => setTimeout(done, 60));
      seen.headMoved = headUses();

      delete window.__ANSWER__.subagent_log;
      wear(wore);
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a running helper's row says what it is doing and how many tools it has picked up, in every catalogue",
      helperDoing.noneYet === null &&
        // 부모 행이 쓰는 그 함수 그대로다 — 동사는 카탈로그의 낱말이고 대상은
        // 기계의 말이다.
        helperDoing.said === helperDoing.wantSaid &&
        helperDoing.said.endsWith(" · src/x.rs") &&
        helperDoing.uses === "12 tool uses" &&
        helperDoing.once === "1 tool use" &&
        helperDoing.tip.includes("src/x.rs") && helperDoing.tip.includes("12 tool uses") &&
        new Set(Object.values(helperDoing.words)).size === 4 &&
        Object.values(helperDoing.words).every((word) => word?.includes("12")) &&
        helperDoing.head === "12 tool uses" &&
        helperDoing.headMoved === "30 tool uses",
      JSON.stringify(helperDoing),
    );

    /* 그리고 그 행 안의 작은 손이 전사를 연다.
     *
     * t2163은 행 클릭의 뜻을 정했지 전사를 없앤 것이 아니다 — 그 뒤로
     * `openHelperPage`는 부르는 곳이 없어 광고만 남았고, 사이드바에서 헬퍼를
     * 눌러도 그 대화는 어디로도 열리지 않았다("kg-core를 클릭해도 볼수없음").
     * 행은 공간(부모 판), 단추는 읽기 표면. 그리고 열리지 않을 문은 그리지
     * 않는다: 벤더가 전사를 남기지 않았으면 이 손도 행이 가는 곳으로 간다. */
    const helperRowPeek = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const rows = () => [
        ...(document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`)
          ?.querySelectorAll(".wt-agent") ?? []),
      ];
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-1" });
      tell("hook:subagent", {
        term,
        rows: [{ id: "kept", name: "@kept" }, { id: "bare", name: "@bare" }],
      });
      await window.__PAINTED__();

      const rowOf = (id) => rows().find((one) => one.dataset.sub === id) ?? null;
      const peekOf = (id) => rowOf(id)?.querySelector(".wt-agent-peek") ?? null;
      const kept = rowOf("kept");
      const peek = peekOf("kept");
      seen.onHelper = Boolean(peek);
      // 판을 가진 행에는 없다 — 그 행에게는 판 자체가 문이다.
      seen.notOnParent = rows()[0]?.querySelector(".wt-agent-peek") === null;
      const words = t("session.subagentOpenTranscript", "헬퍼 대화 보기");
      seen.named = peek?.dataset.i18nTitle === "session.subagentOpenTranscript" &&
        peek?.dataset.i18nAria === "session.subagentOpenTranscript" &&
        peek?.dataset.tip === words &&
        peek?.getAttribute("aria-label") === words &&
        peek?.hasAttribute("title") === false;
      seen.reachable = peek?.getAttribute("role") === "button" && peek?.tabIndex === 0;
      // 쉴 때는 폭도 잉크도 없어 낱말이 서는 자리를 밀지 않고, 키보드가 행 안에
      // 들어오면 선다. 전이가 끝나기를 기다렸다 잰다 — 움직이는 중의 값은 이
      // 규칙이 아니라 그 순간의 그림이다.
      const settled = async () => new Promise((done) => setTimeout(done, 320));
      const restStyle = peek ? getComputedStyle(peek) : null;
      seen.rest = restStyle &&
        restStyle.opacity === "0" &&
        restStyle.maxWidth === "0px" &&
        restStyle.pointerEvents === "none";
      kept?.focus();
      seen.focused = document.activeElement === kept;
      await settled();
      const litStyle = peek ? getComputedStyle(peek) : null;
      seen.lit = litStyle &&
        litStyle.opacity === "1" &&
        litStyle.maxWidth === "16px" &&
        litStyle.pointerEvents === "auto";

      // 문 — 전사가 있으면 그 페이지가 열리고 활성화된다. 그리고 행의 약속은
      // 눌리지 않는다: 손 하나에 두 곳이 열리면 어느 쪽도 답이 아니다.
      let asked = null;
      window.__ANSWER__.subagent_log = (args) => {
        asked ??= args;
        return {
          found: true,
          next: 120,
          turns: [{ role: "assistant", text: "전사가 있다" }],
        };
      };
      openTab({ id: `t2316-peek-decoy:${term}`, kind: "board" });
      let bubbled = 0;
      const overhear = () => {
        bubbled += 1;
      };
      kept?.addEventListener("click", overhear);
      peek?.click();
      await new Promise((done) => setTimeout(done, 160));
      kept?.removeEventListener("click", overhear);
      seen.ownsClick = bubbled === 0;
      seen.askedFor = asked;
      seen.opened = tabs.some((tab) => tab.id === `helper:${term}:kept`);
      seen.active = activeTabId === `helper:${term}:kept`;

      // 전사가 없는 헬퍼 — 페이지는 서지 않고, 손은 행이 가는 곳으로 간다.
      setActiveTab(`t2316-peek-decoy:${term}`);
      window.__ANSWER__.subagent_log = () => ({ found: false });
      peekOf("bare")?.click();
      await new Promise((done) => setTimeout(done, 160));
      seen.noPageForBare = !tabs.some((tab) => tab.id === `helper:${term}:bare`);
      seen.fellBackToParent = activeTabId === owner.id && owner.activePane === term;

      delete window.__ANSWER__.subagent_log;
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });

    /* 판 없는 zo 세션의 헬퍼 행에도 같은 손이 선다. 이 행에는 제 term이 없으므로
     * (zo는 한 세션을 한 TUI에서 돌린다) 전사를 물을 이름은 그 세션을 안고 있는
     * 판의 term이고, 그런 판이 아직 없으면 창이 아는 유일한 자리는 부모 레인이다
     * — 행이 늘 가던 그곳. */
    const laneRowPeek = await page.evaluate(async () => {
      const seen = {};
      const laneId = "lane-zo-peek";
      const session = "session-zo-peek";
      const host = document.createElement("div");
      host.className = "wt-children";
      document.body.appendChild(host);
      upsertLane({
        id: laneId,
        title: "zo parent",
        state: "streaming",
        session_id: session,
        worktree_id: null,
        agent: "zo",
      });
      const entry = lanes.get(laneId);
      host.appendChild(entry.row);
      const fire = (name, payload) => {
        for (const hear of window.__LISTENERS__[name] ?? []) hear({ payload });
      };
      const started = Math.floor(Date.now() / 1000) - 90;
      fire("session:frame", {
        session,
        frame: {
          type: "subagents",
          running: [{ id: "kg-core", label: "kg-core", model: "z1", started_epoch: started }],
        },
      });
      const child = entry.row.querySelector(":scope > .lane-subagents > .lane-subagent");
      const peek = child?.querySelector(".wt-agent-peek") ?? null;
      const words = t("session.subagentOpenTranscript", "헬퍼 대화 보기");
      seen.onChild = Boolean(peek);
      seen.named = peek?.dataset.tip === words &&
        peek?.getAttribute("aria-label") === words &&
        peek?.getAttribute("role") === "button" &&
        peek?.tabIndex === 0;

      // 판이 없는 동안은 행과 같은 곳으로.
      openTab({ id: "t2316-lane-peek-decoy", kind: "board" });
      let bubbled = 0;
      const overhear = () => {
        bubbled += 1;
      };
      child?.addEventListener("click", overhear);
      peek?.click();
      await new Promise((done) => setTimeout(done, 60));
      child?.removeEventListener("click", overhear);
      seen.ownsClick = bubbled === 0;
      seen.toParentLane = focusedId === laneId && activeTabId === LANE_TAB;

      // 그 세션을 판이 안고 있으면 그 판의 term으로 전사를 묻는다.
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      fire("hook:agent", {
        term,
        state: "working",
        agent: "zo",
        session: { key: "session_id", id: session, transcript_path: null },
        resumable: true,
      });
      await window.__PAINTED__();
      let asked = null;
      window.__ANSWER__.subagent_log = (args) => {
        asked ??= args;
        return { found: true, next: 12, turns: [{ role: "assistant", text: "레인의 전사" }] };
      };
      setActiveTab("t2316-lane-peek-decoy");
      peek?.click();
      await new Promise((done) => setTimeout(done, 160));
      seen.askedFor = asked;
      seen.opened = tabs.some((tab) => tab.id === `helper:${term}:kg-core`);
      seen.active = activeTabId === `helper:${term}:kg-core`;
      seen.named2 = tabs.find((tab) => tab.id === `helper:${term}:kg-core`)?.worker.name === "kg-core";
      seen.ownerHeld = owner !== null;

      delete window.__ANSWER__.subagent_log;
      fire("session:ended", { session, reason: null });
      removeLane(laneId);
      host.remove();
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a pane helper row carries a quiet second hand that opens the vendor's transcript, and stays a door to the parent when there is none",
      helperRowPeek.onHelper &&
        helperRowPeek.notOnParent &&
        helperRowPeek.named &&
        helperRowPeek.reachable &&
        helperRowPeek.rest &&
        helperRowPeek.focused &&
        helperRowPeek.lit &&
        helperRowPeek.ownsClick &&
        helperRowPeek.askedFor?.id === "kept" &&
        helperRowPeek.askedFor?.after === 0 &&
        helperRowPeek.opened &&
        helperRowPeek.active &&
        helperRowPeek.noPageForBare &&
        helperRowPeek.fellBackToParent,
      JSON.stringify(helperRowPeek),
    );
    ok(
      "a lane helper row's hand asks the pane that holds the session, and falls back to the parent lane while no pane does",
      laneRowPeek.onChild &&
        laneRowPeek.named &&
        laneRowPeek.ownsClick &&
        laneRowPeek.toParentLane &&
        laneRowPeek.askedFor?.id === "kg-core" &&
        laneRowPeek.askedFor?.after === 0 &&
        laneRowPeek.opened &&
        laneRowPeek.active &&
        laneRowPeek.named2 &&
        laneRowPeek.ownerHeld,
      JSON.stringify(laneRowPeek),
    );

    /* ---- 도우미의 페이지와 행은 도우미 자신의 모델을 말한다 (t-15625) --------
     *
     * 화면(사용자 스크린샷 09-29 16:3x): 모델을 sonnet으로 시켜 sonnet으로 돌고
     * 있는 도우미의 페이지가 「ZO · opus」를 말했다. 도우미에게는 제 판이 없어
     * 페이지가 부모 판과 함께 열리고, 입력줄의 칩은 부모 판의 모델을 읽었다.
     * 명부의 행이 도우미의 모델을 나르지 않으니 페이지도 사이드바도 도우미의
     * 것을 말할 방법이 없었다.
     *
     * 합성 이름만 쓴다: 부모 판은 model-o로 돌고, 도우미는 `sonnet`을 요청받아
     * model-s로 돌았다. 세 자리 — 페이지의 칩, 사이드바의 행, 에이전트 알약의
     * 메뉴 — 가 같은 말(model-s)을 해야 하고, 요청과 실행이 다르면 툴팁이 둘을
     * 다 말하고, 벤더가 모델을 말하지 않은 도우미는 에이전트 이름만 말한다.
     * 도우미의 칩은 모델을 바꾸자고 하지 않는다 — 도우미는 `/model`을 칠 수
     * 있는 판이 아니다. */
    const helperOwnModel = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      // The rows of THIS pane: a checkout's card holds every pane opened in it.
      const rowsOf = (at) => [
        ...(document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`)
          ?.querySelectorAll(`.wt-agent[data-term="${at}"]`) ?? []),
      ];
      const rows = () => rowsOf(term);
      const rowOf = (id) => rows().find((one) => one.dataset.sub === id) ?? null;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const settle = (ms = 100) => new Promise((done) => setTimeout(done, ms));
      const face = () => document.querySelector("#worker-view");
      // 페이지 머리의 모델 알약. 입력줄의 칩은 부모의 것이라 도우미의 페이지에서 나갔다(t-15683).
      const chip = () => face()?.querySelector(".worker-head .helper-model") ?? null;
      const chipModel = () => chip()?.querySelector(".helper-model-words")?.textContent ?? null;
      const chipName = () => face()?.querySelector(".worker-head .worker-mark")?.getAttribute("aria-label") ?? null;
      const repaintPage = () => paintWorkerView(tabs.find((one) => one.id === activeTabId));
      const asked = { id: "h-s", name: "@reader", state: "running", model: "model-s", requestedModel: "sonnet", effort: "high" };
      const blank = { id: "h-blank", name: "@blank", state: "running" };
      const rosterOf = (first) => [first, blank];
      window.__ANSWER__.subagent_log = () => ({
        found: true,
        next: 1,
        turns: [{ role: "assistant", text: "reading" }],
      });
      const open = async (sub) => {
        await openHelperPage({ term, agent: "claude", worktree: path, tab: owner }, sub);
        await settle();
      };

      // 부모 판은 model-o로 돌고, 도우미 둘이 나가 있다.
      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-own-model", model: "model-o" });
      tell("hook:subagent", { term, rows: rosterOf(asked) });
      await window.__PAINTED__();

      // 사이드바: 부모 행은 제 모델을, 도우미 행은 도우미의 모델을, 말하지 않은
      // 도우미 행은 아무것도 달지 않는다.
      const wornBy = (node) => node?.querySelector(".wt-agent-model") ?? null;
      seen.parentRow = wornBy(rows().find((one) => !one.dataset.sub))?.textContent ?? null;
      seen.helperRow = wornBy(rowOf("h-s"))?.textContent ?? null;
      seen.helperRowTip = wornBy(rowOf("h-s"))?.dataset.tip ?? "";
      seen.blankRow = wornBy(rowOf("h-blank"))?.textContent ?? null;

      // 페이지: 칩이 도우미의 모델을 말하고, 바꾸자고 하지 않고, 툴팁이 둘 다 말한다.
      await open(asked);
      seen.pageModel = chipModel();
      seen.pageName = chipName();
      seen.pageTip = chip()?.dataset.tip ?? "";
      seen.pageAsked = chip()?.querySelector(".helper-model-asked")?.textContent ?? null;
      seen.pageOffersSwitch = chip() === null ? null :
        chip().tagName === "BUTTON" || chip().hasAttribute("aria-haspopup") ||
        chip().querySelector(".worker-composer-chevron") !== null;
      chip()?.click();
      await settle();
      seen.pageOpenedAMenu = document.querySelector(".composer-menu") !== null;
      closeComposerMenu();

      // 에이전트 알약의 메뉴: 같은 말. 알약은 부모의 입력줄에 서므로 그 메뉴를 부모 판의
      // 이름으로 연다 — 도우미의 페이지에는 입력줄이 없다.
      openComposerAgentsMenu(document.body, { term, agent: "claude" });
      await settle();
      const menuRows = [...document.querySelectorAll(".composer-menu-item")];
      const subOf = (name) => menuRows
        .find((one) => one.querySelector(".composer-menu-name")?.textContent === name)
        ?.querySelector(".composer-menu-sub")?.textContent ?? null;
      seen.menuHelper = subOf("@reader");
      seen.menuBlank = subOf("@blank");
      closeComposerMenu();

      // 모델을 말하지 않은 도우미의 페이지: 에이전트 이름만, 부모의 모델은 없다.
      await open(blank);
      seen.blankPageModel = chipModel();
      seen.blankPageName = chipName();
      seen.blankPageTip = chip()?.dataset.tip ?? "";
      seen.blankPillWords = chip()?.textContent ?? null;
      seen.blankPillUnknown = chip()?.classList.contains("is-unknown") === true;

      // 폴백이 돌던 도우미를 다른 모델로 옮기면 열린 페이지가 따라간다 — 요청은 그대로.
      setActiveTab(`helper:${term}:h-s`);
      tell("hook:subagent", { term, rows: rosterOf({ ...asked, model: "model-t" }) });
      await settle();
      seen.movedModel = chipModel();
      seen.movedTip = chip()?.dataset.tip ?? "";
      seen.movedRow = wornBy(rowOf("h-s"))?.textContent ?? null;

      // 명부가 도우미를 놓아도(세션이 넘어갔다) 열린 페이지는 마지막으로 안 모델을 지킨다.
      tell("hook:subagent", { term, rows: [] });
      await settle();
      seen.keptModel = chipModel();

      // 카탈로그가 아는 모델은 카탈로그의 이름으로 — 세 자리가 같은 낱말로.
      window.__ANSWER__.agent_models = () => [{ id: "model-s", display_name: "Model S", provider: "claude" }];
      agentModelLists.clear();
      await agentModelsFor("claude");
      tell("hook:subagent", { term, rows: rosterOf(asked) });
      await settle();
      seen.namedPage = chipModel();
      seen.namedRow = wornBy(rowOf("h-s"))?.textContent ?? null;
      openComposerAgentsMenu(document.body, { term, agent: "claude" });
      await settle();
      seen.namedMenu = [...document.querySelectorAll(".composer-menu-item")]
        .find((one) => one.querySelector(".composer-menu-name")?.textContent === "@reader")
        ?.querySelector(".composer-menu-sub")?.textContent ?? null;
      closeComposerMenu();

      // 네 카탈로그: 요청→실행 문장과 "바꿀 수 없다"는 이유가 언어를 따라 바뀌고,
      // 사이드바 행과 페이지 칩이 같은 문장을 쓴다. 모델 id는 벤더의 말 그대로다.
      const wore = locale;
      seen.tipsByLocale = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { refresh: false, persist: false });
        paintWorktreeAgents();
        paintComposerChipsFor(term);
        repaintPage();
        seen.tipsByLocale[code] = {
          row: wornBy(rowOf("h-s"))?.dataset.tip ?? "",
          chip: chip()?.dataset.tip ?? "",
        };
      }
      setLocale(wore, { refresh: false, persist: false });
      paintWorktreeAgents();
      paintComposerChipsFor(term);
      repaintPage();

      // 거꾸로도 새지 않는다: 도우미의 전사가 말한 모델은 부모 판의 모델이 아니다.
      // 모델을 모르는 부모 판 밑의 도우미 페이지를 폴하면, 전사의 model-s가 그
      // 부모의 표에 적히고 부모의 칩과 행이 도우미의 모델을 제 것인 양 말했다.
      const lonely = await openTermTab({ placement: "tab" });
      const lonelyOwner = tabOfTerm(lonely);
      tell("hook:agent", { term: lonely, state: "working", agent: "claude", session: "s-lonely" });
      tell("hook:subagent", { term: lonely, rows: [{ id: "h-leak", name: "@leak", state: "running" }] });
      await window.__PAINTED__();
      window.__ANSWER__.subagent_log = () => ({
        found: true,
        next: 2,
        turns: [{ role: "assistant", text: "reading" }],
        model: "model-s",
      });
      await openHelperPage(
        { term: lonely, agent: "claude", worktree: lonelyOwner.worktree, tab: lonelyOwner },
        { id: "h-leak", name: "@leak", state: "running" },
      );
      await pollHelperPages();
      await window.__PAINTED__();
      seen.parentModelFromHelperTranscript = paneModels.get(lonely) ?? null;
      // The parent's own row must stand (or "wears none" is said of nothing).
      const lonelyRows = rowsOf(lonely).filter((one) => !one.dataset.sub);
      seen.lonelyParentRows = lonelyRows.length;
      seen.leakedIntoParentRow = lonelyRows.map((one) => wornBy(one)?.textContent ?? null);

      delete window.__ANSWER__.subagent_log;
      delete window.__ANSWER__.agent_models;
      agentModelLists.clear();
      paneModels.delete(lonely);
      tell("hook:subagent", { term: lonely, rows: [] });
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term: lonely });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a helper's sidebar row wears the model the helper runs on, its parent's row keeps the parent's, and a helper whose vendor names none wears none",
      helperOwnModel.parentRow === "model-o" &&
        helperOwnModel.helperRow === "model-s" &&
        helperOwnModel.blankRow === null,
      JSON.stringify(helperOwnModel),
    );
    ok(
      "a helper's page header pill says the helper's model, names the agent to a screen reader, shows what was asked beside it, never says the pane's, and does not offer to switch it",
      helperOwnModel.pageModel === "model-s" &&
        helperOwnModel.pageName === "Claude" &&
        helperOwnModel.pageAsked?.includes("sonnet") === true &&
        helperOwnModel.pageOffersSwitch === false &&
        helperOwnModel.pageOpenedAMenu === false,
      JSON.stringify(helperOwnModel),
    );
    ok(
      "the tip on a helper's header pill and row says what was asked and what ran, and how hard it thinks",
      helperOwnModel.pageTip.includes("sonnet") && helperOwnModel.pageTip.includes("model-s") &&
        helperOwnModel.pageTip.includes("high") &&
        helperOwnModel.helperRowTip.includes("sonnet") && helperOwnModel.helperRowTip.includes("model-s"),
      JSON.stringify(helperOwnModel),
    );
    ok(
      "the agents menu says the helper's model in the same words, and a helper that names none says its state alone",
      helperOwnModel.menuHelper?.includes("model-s") === true &&
        helperOwnModel.menuBlank?.includes("model-o") === false &&
        helperOwnModel.menuBlank?.includes("model-s") === false,
      JSON.stringify(helperOwnModel),
    );
    ok(
      "a helper whose vendor names no model shows the agent's name alone in a muted header pill, with no model and none of its parent's, and the tip says the vendor did not tell",
      helperOwnModel.blankPageModel === null &&
        helperOwnModel.blankPillUnknown === true &&
        helperOwnModel.blankPillWords === "Claude" &&
        helperOwnModel.blankPageName === "Claude" &&
        helperOwnModel.blankPageTip.length > 0 &&
        helperOwnModel.blankPageTip.includes("model-o") === false,
      JSON.stringify(helperOwnModel),
    );
    ok(
      "a helper moved to another model is followed by its open page and its row, the request stays in the tip, and a page outlives its roster row with the last model it knew",
      helperOwnModel.movedModel === "model-t" &&
        helperOwnModel.movedTip.includes("sonnet") && helperOwnModel.movedTip.includes("model-t") &&
        helperOwnModel.movedTip.includes("model-s") === false &&
        helperOwnModel.movedRow === "model-t" &&
        helperOwnModel.keptModel === "model-t",
      JSON.stringify(helperOwnModel),
    );
    ok(
      "a model the catalog knows is worded by the catalog, the same on the page's header pill, the sidebar row and the agents menu",
      helperOwnModel.namedPage === "Model S" &&
        helperOwnModel.namedRow === "Model S" &&
        helperOwnModel.namedMenu?.includes("Model S") === true,
      JSON.stringify(helperOwnModel),
    );
    ok(
      "the tip on a helper's header pill and row is worded by every catalog and says the same on both, with the vendor's ids untouched",
      Object.values(helperOwnModel.tipsByLocale).length === 4 &&
        new Set(Object.values(helperOwnModel.tipsByLocale).map((tip) => tip.row)).size === 4 &&
        new Set(Object.values(helperOwnModel.tipsByLocale).map((tip) => tip.chip)).size === 4 &&
        Object.values(helperOwnModel.tipsByLocale).every((tip) =>
          [tip.row, tip.chip].every((words) => words.includes("sonnet") && words.includes("model-s")) &&
          tip.chip.startsWith(tip.row) && !tip.chip.includes("helper.model") && !tip.chip.includes("composer.helperModel")),
      JSON.stringify(helperOwnModel),
    );
    ok(
      "the model a helper's transcript names is not written down as its parent pane's",
      helperOwnModel.parentModelFromHelperTranscript === null &&
        helperOwnModel.lonelyParentRows >= 1 &&
        helperOwnModel.leakedIntoParentRow.every((worn) => worn === null),
      JSON.stringify(helperOwnModel),
    );

    // 페이지의 머리·띠·카드·푸터 (t-15683) — 아래 함수.
    await testHelperPage(page, ok);

    /* ---- 그 다섯이 각각 무엇을 하고 있는가 (1-ey) ---------------------------
     *
     * 상태는 셋뿐이라, 다섯이 동시에 돌면 화면에는 "작업 중"이 다섯 줄 선다. 도구
     * 이름과 대상은 훅 페이로드에 처음부터 실려 왔고 분류가 끝난 자리에서 버려지고
     * 있었다. 이 슬라이스가 그 길을 놓았고, 여기서 재는 것은 그 길의 끝이다:
     * 돌고 있는 카드 행이 지금 하는 일을 말하는가, 배치로 와도 맞는가, 헬퍼의 일이
     * 부모의 일로 새지 않는가, 그리고 그 모든 것이 왕복 없이 되는가. */
    const doingNow = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const rows = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      const said = () => rows().map((one) => one.querySelector(".wt-agent-said")?.textContent ?? "");
      const names = () => rows().map((one) => one.querySelector(".wt-agent-name")?.textContent ?? "");
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };

      // 상태 낱말은 창 안에서 읽어 둔다 — 밖에서 비교하려면 카탈로그를 두 번
      // 구현하게 되고, 그 둘이 어긋나면 시험이 창이 아니라 자기 사본을 잰다.
      seen.wordWorking = bucketWord("working");
      seen.wordDone = bucketWord("done");
      seen.labelWord = agentName("claude");

      tell("hook:agent", { term, state: "working", agent: "claude", session: "s-1" });
      await window.__PAINTED__();
      // 아무 일도 보고되지 않았을 때 첫 낱말은 상태어, 둘째는 벤더 라벨 —
      // 원본 사다리의 두 바닥 단(primary=stateLabel, secondary=type label).
      seen.beforeAnything = said();
      seen.beforeNames = names();

      // 한 번의 emit이 여러 개를 싣고 온다: 백엔드가 카드마다 100ms에 하나만
      // 보내고 그 사이의 것을 모아 붙이기 때문이다. 창은 이어 붙이고 **마지막**을
      // 그린다.
      // The clock stands while the round trips are counted: a poll tick that lands inside this paint on a slower runner
      // is a round trip of its own, not the window's. The thousand-page test holds the clock the same way (t-2412 rule 2).
      // Two frames first, so a call the previous step left in flight is not counted here.
      await window.__PAINTED__();
      await window.__PAINTED__();
      window.__HOLD_POLLERS__ = true;
      window.__COUNTS__ = {};
      tell("hook:activity", {
        pane: `term:${term}`,
        activities: [
          { seq: 0, activity: { verb: "read", target: "/repo/ui/shell.js", phase: "finished" } },
          { seq: 1, activity: { verb: "bash", target: "cargo test --workspace", phase: "started" } },
        ],
      });
      await window.__PAINTED__();
      seen.asked = Object.keys(window.__COUNTS__).length;
      // The names of the commands that were counted, so a failure says which call came in.
      seen.askedNames = Object.keys(window.__COUNTS__);
      window.__HOLD_POLLERS__ = false;
      window.__RELEASE_POLLERS__();
      seen.working = said();

      // 헬퍼의 도구 호출은 헬퍼의 행에만 닿는다. 헬퍼는 자기 판이 없어서 부모의
      // 판 번호를 달고 오므로, 카드 이름이 갈라 주지 않으면 부모의 줄이 워커의
      // 일을 제 일로 말하게 된다.
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch" }] });
      tell("hook:activity", {
        pane: `sub:${term}:a-1`,
        activities: [{ seq: 0, activity: { verb: "grep", target: "activity_of", phase: "started" } }],
      });
      await window.__PAINTED__();
      seen.withHelper = said();

      // 대상이 없는 것도 있다 — 그때는 동사만 선다.
      tell("hook:activity", {
        pane: `term:${term}`,
        activities: [{ seq: 2, activity: { verb: "mcp__linear__create_issue", phase: "started" } }],
      });
      await window.__PAINTED__();
      seen.verbOnly = said()[0];

      // 멈춘 에이전트의 도구 줄은 걷힌다: 마지막 도구 호출은 이미 지나간 일이고,
      // 끝난 카드 밑에 그것을 남겨 두면 아직 하고 있는 것처럼 읽힌다(원본
      // agent-row-tool-preview.ts의 상태 게이트). 첫 낱말이 상태어가 되고, 둘째는
      // 사다리의 다음 단(벤더 라벨)로 돌아간다.
      tell("hook:agent", { term, state: "done", agent: "claude", session: "s-1" });
      await window.__PAINTED__();
      seen.afterDone = said()[0];
      seen.afterDoneName = names()[0];
      // 헬퍼는 언제나 돌고 있으므로 그 줄은 그대로다.
      seen.helperStill = said()[1];

      // 스무 개에서 끊는다 — 창의 지도는 백엔드 링의 사본이고, 사본이 원본보다
      // 길게 자라는 것은 그냥 새는 것이다.
      tell("hook:activity", {
        pane: `term:${term}`,
        activities: Array.from({ length: 50 }, (_, at) => ({
          seq: 10 + at,
          activity: { verb: "read", target: `/repo/file-${at}.rs`, phase: "finished" },
        })),
      });
      seen.capped = paneActivities.get(`term:${term}`).length;
      seen.newest = paneActivities.get(`term:${term}`).at(-1).activity.target;

      // 그리고 셸이 닫히면 그 카드와 그 밑의 헬퍼 카드가 함께 사라진다.
      dropTermView(term);
      seen.leftBehind = [...paneActivities.keys()].filter(
        (key) => key === `term:${term}` || key.startsWith(`sub:${term}:`),
      ).length;

      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a running agent's row says what it is doing, not only that it is doing something",
      doingNow.beforeNames[0] === doingNow.wordWorking &&
        doingNow.beforeAnything[0] === doingNow.labelWord &&
        // 마지막 것 하나. 배치의 앞선 것들은 이미 지나간 일이다.
        doingNow.working[0].includes("cargo test --workspace") &&
        doingNow.working[0] !== doingNow.wordWorking &&
        // 왕복 없이. 이 소식은 도구 호출마다 오고, 그때마다 백엔드에 묻는 창은
        // 에이전트 넷이 도는 동안 초에 수십 번을 묻는 창이다.
        doingNow.asked === 0 &&
        // 대상이 없으면 동사만, 그리고 이 창이 모르는 도구는 벤더의 이름 그대로.
        doingNow.verbOnly === "mcp__linear__create_issue",
      JSON.stringify(doingNow),
    );
    ok(
      "a helper's work is the helper's row, and an agent that stopped drops the tool line",
      // Guarded reads: a helper row that has not been drawn yet is a FAIL line
      // the lane can judge solo, never an exception that aborts the whole run
      // (2026-09-14 01:2x, a run under the lane's load died here with
      // `withHelper[1]` undefined and reported nothing after it).
      (doingNow.withHelper[1] ?? "").includes("activity_of") &&
        (doingNow.withHelper[0] ?? "").includes("cargo test --workspace") &&
        doingNow.afterDoneName === doingNow.wordDone &&
        doingNow.afterDone === doingNow.labelWord &&
        !doingNow.afterDone.includes("cargo test") &&
        doingNow.helperStill.includes("activity_of"),
      JSON.stringify(doingNow),
    );
    ok(
      "the window's copy of the ring is bounded and goes back with the shell",
      doingNow.capped === 20 &&
        doingNow.newest === "/repo/file-49.rs" &&
        doingNow.leftBehind === 0,
      JSON.stringify(doingNow),
    );

    /* ---- 아무 말 없이 나가 버린 에이전트 (1-fe) -----------------------------
     *
     * 보고된 것: 터미널에서 codex를 돌리다 빠져나와 셸 프롬프트로 돌아왔는데
     * 사이드바는 계속 "Codex 작업 중"이었다. 판의 상태를 지우는 길은 전부
     * **사건**이었다 — `Stop` 훅이거나 터미널이 닫히는 것이거나. 둘 다 오지
     * 않는다: 벤더는 `Stop`을 보내지 않았고, 에이전트가 돌던 셸은 멀쩡히 살아
     * 있으므로 `term:exited`도 없다.
     *
     * 백엔드가 대신 pty의 전경 프로세스 그룹을 본다. 그 판정이 창에 닿는 길은
     * **새 길이 아니다** — 여느 훅 보고와 똑같은 `hook:agent`이고, 상태 낱말만
     * `idle`이다. 여기서 재는 것은 그 낱말 하나가 네 표면을 전부 옳게 만드는가,
     * 그리고 **대화 기록은 남는가**이다. 방금 끄고 나온 사람이야말로 그 대화를
     * 다시 열고 싶은 사람이다. */
    const walkedOut = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const rows = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      // 이 검사에는 레인이 없다 — 재는 것은 훅 쪽 장부 하나가 점을 어떻게
      // 움직이는가이므로, 레인 목록은 비어 있는 채로 넘긴다.
      const dotOf = (path$1) => worktreeDotState(path$1, []);

      seen.wordWorking = bucketWord("working");
      seen.wordIdle = bucketWord("idle");

      // 돌고 있는 codex 하나, 그 밑에 헬퍼 둘, 그리고 이어 열 수 있는 대화.
      tell("hook:agent", { term, state: "working", agent: "codex", session: "c-1", resumable: true });
      tell("hook:subagent", { term, rows: [{ id: "a-1", name: "@arch" }, { id: "a-2", name: "@ui" }] });
      tell("hook:activity", {
        pane: `sub:${term}:a-1`,
        activities: [{ seq: 0, activity: { verb: "grep", target: "hook_state", phase: "started" } }],
      });
      await window.__PAINTED__();
      seen.busyRows = rows().length;
      // 상태어는 이제 첫 낱말(.wt-agent-name)이다 — V3 사다리의 바닥 단.
      seen.busySaid = rows()[0]?.querySelector(".wt-agent-name")?.textContent ?? "";
      seen.busyClass = rows()[0]?.className ?? "";
      seen.busyDot = dotOf(path);
      seen.busyTab = [...document.querySelectorAll(".tab")].some((one) =>
        one.classList.contains("is-streaming"));

      // 그리고 사람이 codex에서 빠져나온다. 훅은 오지 않는다 — 오는 것은 백엔드가
      // 터미널을 보고 내린 판정이고, 그것은 여느 보고와 같은 문으로 들어온다.
      tell("hook:agent", { term, state: "idle", agent: "codex" });
      tell("hook:subagent", { term, rows: [] });
      await window.__PAINTED__();
      seen.leftSaid = rows()[0]?.querySelector(".wt-agent-name")?.textContent ?? "";
      seen.leftClass = rows()[0]?.className ?? "";
      seen.leftRows = rows().length;
      seen.leftDot = dotOf(path);
      seen.leftTab = [...document.querySelectorAll(".tab")].some((one) =>
        one.classList.contains("is-streaming") || one.classList.contains("is-waiting"));
      // 탭 줄이 읽는 접힘도 조용해졌는가 — 돌고 있는 판 하나도 남지 않았다.
      seen.leftFold = hookStateOfTab(owner);

      // 대화는 남는다. 판의 메뉴가 여전히 "이 대화 다시 열기"를 낼 수 있어야 한다.
      seen.stillResumable = resumableSessionOf(owner)?.session ?? null;
      seen.stillKnown = paneSessions.get(term)?.agent ?? null;

      // 그리고 같은 셸에서 다시 에이전트를 돌리면 그대로 살아난다 — `idle`은
      // 판을 끝낸 것이 아니라 지금 비었다고 말한 것뿐이다.
      tell("hook:agent", { term, state: "working", agent: "codex", session: "c-1", resumable: true });
      await window.__PAINTED__();
      seen.againSaid = rows()[0]?.querySelector(".wt-agent-name")?.textContent ?? "";
      seen.againDot = dotOf(path);

      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "an agent that walked out of its shell stops being drawn as working",
      walkedOut.busySaid === walkedOut.wordWorking &&
        walkedOut.busyClass.includes("is-working") &&
        walkedOut.busyDot === "streaming" &&
        walkedOut.busyTab === true &&
        // 그리고 나간 뒤: 낱말도, 점도, 탭의 배지도 전부 조용해진다.
        walkedOut.leftSaid === walkedOut.wordIdle &&
        walkedOut.leftClass.includes("is-idle") &&
        walkedOut.leftDot === "idle" &&
        walkedOut.leftTab === false &&
        walkedOut.leftFold === "idle",
      JSON.stringify(walkedOut),
    );
    ok(
      "its helpers go with it — no stop event is coming for them either",
      walkedOut.busyRows === 3 && walkedOut.leftRows === 1,
      JSON.stringify(walkedOut),
    );
    ok(
      "but the conversation stays, because quitting an agent is when you most want it back",
      walkedOut.stillResumable === "c-1" &&
        walkedOut.stillKnown === "codex" &&
        // 그리고 그 판은 죽은 판이 아니다 — 다시 돌면 다시 돈다.
        walkedOut.againSaid === walkedOut.wordWorking &&
        walkedOut.againDot === "streaming",
      JSON.stringify(walkedOut),
    );

    /* ---- the worker seat ----------------------------------------------------
     *
     * From here the cases seat workers for other checkouts and move the active
     * workspace as they go; nothing after them on this page needs it back. */
    // A checkout the ledger cut for a Codex worker stores no pane layout — the
    // worker's tab is the ledger's — so it came back after a restart as an empty
    // workspace, and an empty workspace opened the DEFAULT agent: a Codex
    // worktree wearing Claude. The ledger still knows who was seated there, and
    // the restore asks it before guessing — for a seat that stands or one that is
    // coming back. A seat the ledger LET GO of is nobody's (t-19779): the visit
    // used to launch the agent the ledger named, fresh, so five finished workers'
    // checkouts each held an empty agent a few minutes after their last turn,
    // whatever the person had chosen to open in a new workspace, and the
    // reclaimer waited on it. The backend no longer answers for such a checkout;
    // the stub gives the answer it used to — the agent that was there, no seat,
    // nothing to wait for — and the window must not launch it: the person's own
    // road opens a first terminal (here the harness's plain one).
    await page.evaluate(() => {
      window.__ANSWER__.set_active_worktree = () => "wt/t-3";
      window.__ASKED_SEAT__ = [];
      window.__ANSWER__.worktree_last_agent = (args) => (
        window.__ASKED_SEAT__.push(args),
        { agent: "codex", sleeping: false }
      );
      window.__LAUNCHED__ = null;
    });
    const letGo = await page.evaluate(async () => {
      const terminalTabs = () => tabs.filter((held) => held.kind === "term").length;
      const before = terminalTabs();
      await activateWorktree("/tmp/zerocode-window-test/wt-t3");
      return {
        launched: window.__LAUNCHED__?.agent ?? null,
        asked: window.__ASKED_SEAT__.length,
        opened: terminalTabs() - before,
      };
    });
    ok(
      "a seat the ledger names but nobody is coming back to is not launched again — the visit takes the person's own road to a first terminal",
      letGo.launched === null && letGo.asked === 1 && letGo.opened >= 1,
      JSON.stringify(letGo),
    );

    // A stronger answer than the last agent's TYPE is its live seat. The window
    // can miss the one `term:worker` event that normally mounts that seat; opening
    // the checkout must attach the term the ledger still names, not launch a
    // second agent beside it.
    await page.evaluate(() => {
      const path = "/tmp/zerocode-window-test/wt-live-seat";
      window.__ASKED_SEAT__ = [];
      window.__ANSWER__.worktree_last_agent = (args) => (
        window.__ASKED_SEAT__.push(args),
        { agent: "codex", sleeping: false, term: 13499 }
      );
      window.__LIVE_SEAT__ = {
        path,
        term: 13499,
        previousPath: activeWorktreePath,
        previousTab: activeTabId,
      };
      // The periodic board view has not learned about this newly seated worker.
      window.__LEDGER__ = [];
      window.__LAUNCHED__ = null;
    });
    const liveSeatRecovered = await page.evaluate(async () => {
      const { path, term } = window.__LIVE_SEAT__;
      // Call the restore's one fallback directly. The harness catalog always
      // marks main active and would race `activateWorktree` back to main; the real
      // backend moves that mark with `set_active_worktree`.
      activeWorktreePath = path;
      const reserved = await openLedgerSeatedAgent();
      const tab = tabOfTerm(term);
      return {
        reserved,
        tab: tab?.id ?? null,
        worktree: tab?.worktree ?? null,
        active: activeTabId,
        launched: window.__LAUNCHED__,
        fellBack: window.__ASKED_SEAT__.length,
      };
    });
    ok(
      "a live ledger seat missed by the tab event is attached instead of launching a second agent",
      liveSeatRecovered.tab === "term:13499" &&
        liveSeatRecovered.worktree === "/tmp/zerocode-window-test/wt-live-seat" &&
        liveSeatRecovered.active === liveSeatRecovered.tab && liveSeatRecovered.reserved &&
        liveSeatRecovered.launched === null && liveSeatRecovered.fellBack === 1,
      JSON.stringify(liveSeatRecovered),
    );
    await page.evaluate(() => {
      const { term, previousPath, previousTab } = window.__LIVE_SEAT__;
      const tab = tabOfTerm(term);
      if (tab) dropTab(tab.id);
      dropTermView(term);
      activeWorktreePath = previousPath;
      if (previousTab && tabs.some((held) => held.id === previousTab)) setActiveTab(previousTab);
      else updateStage();
      window.__LEDGER__ = [];
      delete window.__LIVE_SEAT__;
    });

    // A sleeping row is different: this checkout is reserved for the ledger's
    // replacement pane, so the generic fallback must not race it. The inactive
    // fact becomes the restored pane's one-word birth label, then gives way to the
    // first real hook.
    await page.evaluate(() => {
      window.__ANSWER__.set_active_worktree = () => "wt/t-sleeping";
      window.__ANSWER__.worktree_last_agent = () => ({ agent: "codex", sleeping: true });
      window.__LAUNCHED__ = null;
    });
    const sleepingReservation = await page.evaluate(async () => {
      const path = "/tmp/zerocode-window-test/wt-sleeping";
      await activateWorktree(path);
      activeWorktreePath = path;
      const reserved = await openLedgerSeatedAgent();
      return {
        launched: window.__LAUNCHED__,
        waiting: restoringWorkers.get(path),
        reserved,
      };
    });
    ok(
      "a sleeping worker checkout waits for the ledger instead of opening a generic agent",
      sleepingReservation.launched === null &&
        sleepingReservation.waiting === "codex" &&
        sleepingReservation.reserved,
      JSON.stringify(sleepingReservation),
    );

    // A restored worker used to arrive as a split first, then acquire its ledger
    // seat after the restored conversation was known. The seat must move that one
    // leaf into a tab of its own; adopting the tab would rename the coordinator's
    // whole split as the worker's checkout.
    const restoredSplit = await page.evaluate(async () => {
      const path = "/tmp/zerocode-window-test/wt-restored-split";
      const leader = await openTermTab({ placement: "tab" });
      const host = tabOfTerm(leader);
      const before = {
        id: host.id,
        worktree: host.worktree,
        ledgerManaged: host.ledgerManaged === true,
      };
      const worker = 13500;
      for (const handler of window.__LISTENERS__["term:split"] ?? []) {
        handler({
          payload: { parent: leader, term: worker, direction: "horizontal", agent: "claude" },
        });
      }
      const tiled = paneLeaves(host.layout);
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({
          payload: { parent: leader, term: worker, worktree: path, agent: "claude", resumed: "session" },
        });
      }
      const hostAfter = tabs.find((tab) => tab.id === before.id);
      const own = tabOfTerm(worker);
      const out = {
        tiled,
        hostLeaves: hostAfter ? paneLeaves(hostAfter.layout) : [],
        hostWorktree: hostAfter?.worktree ?? null,
        hostLedgerManaged: hostAfter?.ledgerManaged === true,
        ownId: own?.id ?? null,
        ownLeaves: own ? paneLeaves(own.layout) : [],
        ownWorktree: own?.worktree ?? null,
        ownLedgerManaged: own?.ledgerManaged === true,
        before,
        leader,
        worker,
      };
      if (own) dropTab(own.id);
      if (hostAfter) dropTab(hostAfter.id);
      dropTermView(worker);
      dropTermView(leader);
      return out;
    });
    ok(
      "a restored worker tiled into its coordinator's tab moves into a tab of its own",
      JSON.stringify(restoredSplit.tiled) ===
        JSON.stringify([restoredSplit.leader, restoredSplit.worker]) &&
        JSON.stringify(restoredSplit.hostLeaves) === JSON.stringify([restoredSplit.leader]) &&
        restoredSplit.hostWorktree === restoredSplit.before.worktree &&
        restoredSplit.hostLedgerManaged === restoredSplit.before.ledgerManaged &&
        restoredSplit.ownId !== null &&
        restoredSplit.ownId !== restoredSplit.before.id &&
        JSON.stringify(restoredSplit.ownLeaves) === JSON.stringify([restoredSplit.worker]) &&
        restoredSplit.ownWorktree === "/tmp/zerocode-window-test/wt-restored-split" &&
        restoredSplit.ownLedgerManaged,
      JSON.stringify(restoredSplit),
    );
    const restoredBirth = await page.evaluate(() => {
      const term = 13501;
      const path = "/tmp/zerocode-window-test/wt-sleeping";
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: 101, term, worktree: path, agent: "codex", resumed: "session" } });
      }
      const primary = "Working";
      const born = agentRowSecondary({ term, agent: "codex" }, "working", primary);
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
        handler({ payload: { term, state: "working", agent: "codex" } });
      }
      const fresh = term + 1;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: 101, term: fresh, worktree: path, agent: "codex", resumed: "fresh" } });
      }
      return {
        born,
        fresh: agentRowSecondary({ term: fresh, agent: "codex" }, "working", primary),
        after: agentRowSecondary({ term, agent: "codex" }, "working", primary),
        waiting: restoringWorkers.has(path),
        birthHeld: restoredWorkers.has(term),
        // The words are the catalog's, read in the language this page speaks —
        // the shared flow happened to be speaking English where this case
        // stood, and the English literals were that position, not the rule.
        wantBorn: t("board.restored", "이어서"),
        wantFresh: t("board.restoredFresh", "이어서 (새 대화)"),
      };
    });
    ok(
      "a restored worker says how it resumed only until its first hook",
      restoredBirth.born === restoredBirth.wantBorn &&
        restoredBirth.fresh === restoredBirth.wantFresh &&
        restoredBirth.after !== restoredBirth.wantBorn &&
        !restoredBirth.waiting &&
        !restoredBirth.birthHeld,
      JSON.stringify(restoredBirth),
    );
    /* 같은 접힘이 worker 길에서도 선다(2026-09-13 23:56 「implement#0 눌러도 안
     * 보임」). 워크플로가 띄운 팀원은 제 탭을 가진 worker 표면으로 오는데, 그 자리
     * 이벤트(`term:worker`)는 헬퍼 id를 싣지 않았다 — 명부 행은 부모 시계를 단
     * 헬퍼 행으로 남고 클릭은 부모 판(사람이 이미 보고 있는 판)으로 갔다. 이제
     * `term:worker`도 id를 싣고, 판이 첫 훅을 말하기 전이라도 헬퍼 행의 클릭은 그
     * 헬퍼의 판이 선 탭으로 간다. */
    const workerFold = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const under = () => worktreeAgentRows(path).filter((row) => row.depth === 1 && !row.history);
      const settle = () => new Promise((done) => setTimeout(done, 120));
      const landedOn = (child) => {
        const seat = tabOfTerm(child);
        return seat !== null && activeTabId === seat.id && activePaneOf(seat) === child;
      };
      tell("hook:agent", { term, state: "working", agent: "zo", session: "s-wfold" });
      tell("hook:subagent", { term, rows: [{ id: "agent-9", name: "implement#0", state: "running", tool_calls: 1 }] });
      // The worker road: a tab of its own in the same checkout — and the helper id.
      const child = term + 7100;
      tell("term:worker", { parent: term, term: child, worktree: path, agent: "zo", helper: "agent-9" });
      // The worker seat repaints the cards after an asynchronous refresh, and a
      // tab change only SCHEDULES a paint — so every read of the drawn rows below
      // asks for a paint and waits for it, instead of reading between two.
      const painted = async () => {
        await settle();
        scheduleAgentPaint(["cards"]);
        await window.__PAINTED__();
      };
      await painted();
      seen.helperKnown = paneHelpers.get(child) === "agent-9";
      const own = tabOfTerm(child);
      seen.ownTab = own !== null && own.id !== owner.id;
      // Before the pane has spoken its first hook: whatever row stands for the
      // helper, its click lands on the helper's own tab — not on the parent.
      seen.earlyRows = under().map((row) => (row.sub ? `sub:${row.sub.id}` : `term:${row.term}`)).sort();
      setActiveTab(owner.id);
      await painted();
      // Pressed through the row's own node (`makeAgentRow` wires the click), not
      // through the sidebar card: the worker seat's `refreshWorktrees()` rebuilds
      // the cards from the backend's list, and a fixture workspace has no card
      // there — the wiring under test is the row's, not the card's.
      const earlyRow = under().find((row) => row.sub?.id === "agent-9") ?? null;
      seen.earlyRowDrawn = earlyRow !== null;
      if (earlyRow) makeAgentRow(earlyRow).click();
      await settle();
      seen.earlyLanded = landedOn(child);
      // The pane speaks: one identity, one row, wearing the helper's name and its
      // own term — and the click still lands on its tab.
      tell("hook:agent", { term: child, state: "working", agent: "zo", session: "s-wchild" });
      await painted();
      const rows = under();
      seen.childCount = rows.length;
      seen.childTerm = rows[0]?.term ?? null;
      seen.childSub = rows[0]?.sub?.id ?? null;
      setActiveTab(owner.id);
      await painted();
      const foldedRow = rows.find((row) => row.term === child) ?? null;
      if (foldedRow) makeAgentRow(foldedRow).click();
      await settle();
      seen.landed = landedOn(child);
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term: child });
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a worker-surface helper folds too: its seat carries the helper id, and the helper row's click lands on the helper's own tab even before the pane speaks",
      workerFold.helperKnown &&
        workerFold.ownTab &&
        workerFold.earlyRowDrawn &&
        workerFold.earlyLanded &&
        workerFold.childCount === 1 &&
        typeof workerFold.childTerm === "number" &&
        workerFold.childSub === "agent-9" &&
        workerFold.landed,
      JSON.stringify(workerFold),
    );

    // A person may close a split while leaving the agent alive. If that was the
    // checkout's last visible tab, other workspaces still keep the global `tabs`
    // list non-empty. The empty stage is about THIS checkout, and it must say what
    // is still running there and carry the existing row's reattach action.
    const partedStage = await page.evaluate(async () => {
      const previousPath = activeWorktreePath;
      const previousTab = activeTabId;
      const other = await openTermTab({ placement: "tab" });
      const otherTab = tabOfTerm(other);
      const path = "/tmp/zerocode-window-test/wt-parted";
      const term = 13503;
      activeWorktreePath = path;
      activeTabId = null;
      paneAgents.set(term, "codex");
      hookStates.set(term, "working");
      detachedAgents.set(term, path);
      updateStage();
      const before = {
        hidden: placeholder.hidden,
        words: placeholder.textContent,
        expected: t(
          "stage.detachedAgents",
          "이 워크트리에 에이전트 {{count}}개가 탭 없이 실행 중입니다.",
          { count: 1 },
        ),
        globalTabs: tabs.length,
        workspaceTabs: tabs.filter((tab) => tab.worktree === path).length,
      };
      placeholder.querySelector(`[data-stage-agent="${term}"]`)?.click();
      const mounted = tabOfTerm(term);
      const after = {
        tab: mounted?.id ?? null,
        worktree: mounted?.worktree ?? null,
        active: activeTabId,
        detached: detachedAgents.has(term),
        hidden: placeholder.hidden,
      };

      if (mounted) dropTab(mounted.id);
      dropTermView(term);
      hookStates.delete(term);
      paneAgents.delete(term);
      detachedAgents.delete(term);
      if (otherTab && tabs.includes(otherTab)) dropTab(otherTab.id);
      activeWorktreePath = previousPath;
      if (previousTab && tabs.some((tab) => tab.id === previousTab)) setActiveTab(previousTab);
      else updateStage();
      return { before, after };
    });
    ok(
      "an empty checkout says its detached agent is still running and offers the pane back",
      partedStage.before.hidden === false && partedStage.before.globalTabs > 0 &&
        partedStage.before.workspaceTabs === 0 &&
        partedStage.before.words.includes(partedStage.before.expected) &&
        partedStage.after.tab === "term:13503" &&
        partedStage.after.worktree === "/tmp/zerocode-window-test/wt-parted" &&
        partedStage.after.active === partedStage.after.tab &&
        partedStage.after.detached === false && partedStage.after.hidden === true,
      JSON.stringify(partedStage),
    );

    // A worker's tab takes its stage group at birth, and the group came from
    // `focusedPane` — a leaf of whichever tree was IN FRONT. Seated for another
    // checkout while this one's stage was split, a worker was born into a group
    // the other checkout's tree never had; a split folding while it still held
    // such a tab stranded it the same way. The sidebar listed it (it reads
    // `tabs`), the strip and the stage never could (they read the group), and
    // clicking its row went nowhere (live report 2026-08-30, "zerocode-cli의
    // 워커를 클릭해도 보이지 않음"). Both roads are closed, and a tab stranded by
    // any road is brought home the first time the stage is asked for it.
    const strandedWorker = await page.evaluate(async () => {
      const previousPath = activeWorktreePath;
      const previousTab = activeTabId;
      const previousTree = stageTree();
      const previousFocus = focusedPane;
      // The checkout in front, its stage split, the second leaf focused.
      const home = focusedPane;
      const split = nextGroupId;
      nextGroupId += 1;
      setStageTree(splitStageLeaf(stageTree(), home, split, "horizontal", "second"));
      focusedPane = split;
      const path = "/tmp/zerocode-window-test/wt-stranded";
      // A worker seated for ANOTHER checkout while that split is in front.
      // `tabOfTerm`, not the seat's return: `openTab` files a COPY of the tab it
      // is handed, and the copy is the record the strip and the stage read.
      const bornTerm = 13511;
      seatLedgerManagedTerm(bornTerm, path, "codex");
      const born = tabOfTerm(bornTerm);
      const birth = { pane: born.pane, worktree: born.worktree };
      // And one an older window did strand: sitting in the split leaf.
      const strayTerm = 13512;
      seatLedgerManagedTerm(strayTerm, path, "codex");
      const stray = tabOfTerm(strayTerm);
      stray.pane = split;
      const folded = collapseStageGroup(split);
      const afterFold = { folded, strayPane: stray.pane, live: stageGroups() };
      // Now the person opens that checkout and clicks the worker's row.
      activeWorktreePath = path;
      activeTabId = null;
      updateStage();
      const listed = paneTabs(stageGroups()[0]).map((tab) => tab.id);
      await focusAgentPane(path, born.id, bornTerm, "codex");
      const shown = { active: activeTabId, onStage: activeTabIn(born.pane)?.id ?? null };
      for (const term of [bornTerm, strayTerm]) {
        const tab = tabOfTerm(term);
        if (tab) dropTab(tab.id);
        dropTermView(term);
      }
      activeWorktreePath = previousPath;
      setStageTree(previousTree);
      focusedPane = previousFocus;
      if (previousTab && tabs.some((tab) => tab.id === previousTab)) setActiveTab(previousTab);
      else updateStage();
      return { birth, afterFold, listed, shown, split };
    });
    ok(
      "a worker seated for another checkout is born into that checkout's chassis, not the split in front",
      strandedWorker.birth.pane === 0 &&
        strandedWorker.birth.worktree === "/tmp/zerocode-window-test/wt-stranded",
      JSON.stringify(strandedWorker),
    );
    ok(
      "folding a split brings home the tabs of every checkout that sat in it",
      strandedWorker.afterFold.folded === true &&
        strandedWorker.afterFold.strayPane !== strandedWorker.split &&
        strandedWorker.afterFold.live.includes(strandedWorker.afterFold.strayPane),
      JSON.stringify(strandedWorker),
    );
    ok(
      "a stranded worker tab is listed with its checkout and put on stage when its row is clicked",
      strandedWorker.listed.includes("term:13511") &&
        strandedWorker.listed.includes("term:13512") &&
        strandedWorker.shown.active === "term:13511" &&
        strandedWorker.shown.onStage === "term:13511",
      JSON.stringify(strandedWorker),
    );

    /* ---- the placement seat's label: the person's move, reported once ----
     *
     * A worker the seat answered for (`placed`) is remembered by its term;
     * the person dragging its tab to another group reports the move to the
     * one door with the worker's id, and a second drag of the same tab
     * reports nothing. A worker restored without a seat (a restart's
     * reseat) is nobody's answer, and its drag reports nothing either. */
    const placedMove = await page.evaluate(async () => {
      const path = "/tmp/zerocode-window-test/wt-placed";
      const leader = await openTermTab({ placement: "tab" });
      const reported = [];
      window.__ANSWER__.note_worker_room_change = (args) => {
        reported.push(JSON.parse(JSON.stringify(args)));
        return true;
      };
      window.__ANSWER__.judge_worker_room = () => ({
        outcome: "answered",
        chosen: "tab",
        applied: false,
        offered: ["tab", "split", "background"],
        placed: true,
      });
      const worker = 13520;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({
          payload: {
            parent: leader,
            term: worker,
            worktree: path,
            agent: "codex",
            seat: { run: "run-1", worker: "w-13520", dispatch: "dp-1", task: "t-1", brief: "measure the frame time", briefChars: 21 },
          },
        });
      }
      // The answer arrives off the handler: wait for the door to have been asked.
      const waited = Date.now();
      while (!placedWorkers.has(worker) && Date.now() - waited < 2_000) {
        await new Promise((done) => setTimeout(done, 10));
      }
      const remembered = placedWorkers.get(worker)?.worker ?? null;
      const restored = worker + 1;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: leader, term: restored, worktree: path, agent: "codex", resumed: "session" } });
      }
      const restoredRemembered = placedWorkers.has(restored);
      // The person opens that checkout, so both tabs stand on its stage.
      const previousPath = activeWorktreePath;
      const previousTree = stageTree();
      const previousFocus = focusedPane;
      activeWorktreePath = path;
      activeTabId = null;
      updateStage();
      const drag = (term) => {
        const tab = tabOfTerm(term);
        const other = splitStageBesideFocused("horizontal");
        tabDrag = { id: tab.id, startX: 0, moved: true, target: { group: other, zone: "center" } };
        endTabDrag();
        return tab.pane === other;
      };
      const firstDrag = drag(worker);
      const afterFirst = reported.length;
      const secondDrag = drag(worker);
      const afterSecond = reported.length;
      const restoredDrag = drag(restored);
      const afterRestored = reported.length;
      for (const term of [worker, restored]) {
        const tab = tabOfTerm(term);
        if (tab) dropTab(tab.id);
        dropTermView(term);
      }
      const leaderTab = tabOfTerm(leader);
      if (leaderTab) dropTab(leaderTab.id);
      dropTermView(leader);
      delete window.__ANSWER__.note_worker_room_change;
      delete window.__ANSWER__.judge_worker_room;
      activeWorktreePath = previousPath;
      setStageTree(previousTree);
      focusedPane = previousFocus;
      updateStage();
      return { remembered, restoredRemembered, firstDrag, secondDrag, restoredDrag, afterFirst, afterSecond, afterRestored, reported };
    });
    ok(
      "a placed worker's tab dragged by the person reports the move once, with the worker's id and the room",
      placedMove.remembered === "w-13520" &&
        placedMove.firstDrag === true &&
        placedMove.afterFirst === 1 &&
        placedMove.reported[0]?.worker === "w-13520" &&
        placedMove.reported[0]?.room === "tab" &&
        placedMove.secondDrag === true &&
        placedMove.afterSecond === 1,
      JSON.stringify(placedMove),
    );
    ok(
      "a worker restored without a seat is nobody's answer and its drag reports nothing",
      placedMove.restoredRemembered === false &&
        placedMove.restoredDrag === true &&
        placedMove.afterRestored === 1,
      JSON.stringify(placedMove),
    );

    /* ---- the placement seat's label: a sight, reported once (t-6342) ----
     *
     * A placed worker's pane that stands on the stage, with the window in
     * front, for the dwell the door named reports one sight through its own
     * door — and only one, however many times the stage is drawn again. A
     * placed pane that never reaches the stage reports none: nobody was in
     * front of it, and its label is left unmarked. */
    const placedSight = await page.evaluate(async () => {
      const path = "/tmp/zerocode-window-test/wt-sighted";
      const leader = await openTermTab({ placement: "tab" });
      const sights = [];
      window.__ANSWER__.note_worker_room_seen = (args) => {
        sights.push(JSON.parse(JSON.stringify(args)));
        return true;
      };
      window.__ANSWER__.judge_worker_room = () => ({
        outcome: "answered",
        chosen: "tab",
        applied: false,
        offered: ["tab", "background"],
        placed: true,
        seenAfterMs: 30,
      });
      const hadFocus = document.hasFocus;
      document.hasFocus = () => true;
      const sighted = 13530;
      const unseen = 13531;
      for (const [term, worker] of [[sighted, "w-13530"], [unseen, "w-13531"]]) {
        for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
          handler({
            payload: {
              parent: leader,
              term,
              worktree: path,
              agent: "codex",
              seat: { run: "run-1", worker, dispatch: "dp-1", task: "t-1", brief: "look at the frame time", briefChars: 22 },
            },
          });
        }
      }
      const waited = Date.now();
      while ((!placedWorkers.has(sighted) || !placedWorkers.has(unseen)) && Date.now() - waited < 2_000) {
        await new Promise((done) => setTimeout(done, 10));
      }
      // The person opens that checkout with the first worker's tab in front.
      const previousPath = activeWorktreePath;
      const previousTree = stageTree();
      const previousFocus = focusedPane;
      activeWorktreePath = path;
      activeTabId = tabOfTerm(sighted)?.id ?? null;
      updateStage();
      const onStage = readingTerms().has(sighted) && !readingTerms().has(unseen);
      await new Promise((done) => setTimeout(done, 150));
      const afterDwell = sights.length;
      updateStage();
      await new Promise((done) => setTimeout(done, 150));
      const afterAgain = sights.length;
      document.hasFocus = hadFocus;
      for (const term of [sighted, unseen]) {
        const tab = tabOfTerm(term);
        if (tab) dropTab(tab.id);
        dropTermView(term);
      }
      const leaderTab = tabOfTerm(leader);
      if (leaderTab) dropTab(leaderTab.id);
      dropTermView(leader);
      const forgotten = !placedWorkers.has(sighted) && !placedWorkers.has(unseen);
      delete window.__ANSWER__.note_worker_room_seen;
      delete window.__ANSWER__.judge_worker_room;
      activeWorktreePath = previousPath;
      setStageTree(previousTree);
      focusedPane = previousFocus;
      updateStage();
      return { onStage, afterDwell, afterAgain, forgotten, sights };
    });
    ok(
      "a placed worker's pane on the stage, with the window in front, reports one sight after the dwell",
      placedSight.onStage === true &&
        placedSight.afterDwell === 1 &&
        placedSight.sights[0]?.worker === "w-13530" &&
        placedSight.afterAgain === 1,
      JSON.stringify(placedSight),
    );
    ok(
      "a placed pane that ends is forgotten with its sight clock",
      placedSight.forgotten === true,
      JSON.stringify(placedSight),
    );
  } finally {
    await page.close();
  }
}

/* ---- 도우미의 페이지: 누구의 것이고, 무엇을 시켰고, 사람이 여기서 무엇을 할 수 있는가 (t-15683) ----
 *
 * 화면(09-30): 도우미의 페이지에는 이름 하나뿐이었다. 어느 대화의 도우미인지도, 돌아가는 길도
 * 없었고, 도우미가 받은 지시는 입력 상자를 닮은 말풍선에 문장 중간에서 잘려 서 있었고, 바닥의
 * 입력줄은 「다음 메시지 대기열에 추가」라 했지만 그 말을 누가 받는지 알 수 없었다. 페이지는
 * 이제 셋을 말한다: 이 도우미가 누구의 것인가(부모 대화로 가는 한 번의 누름, 같은 부모의 다른
 * 도우미들의 띠), 무엇을 시켰는가(카드), 사람이 여기서 할 수 있는 일(부모 대화에 말하기 —
 * 도우미 하나만 멈추는 길은 어느 에이전트에도 없어서 그 단추는 서지 않는다).
 *
 * 합성 이름과 합성 명부만 쓴다. */
const HELPER_BRIEFS = {
  // 문장 여섯. 조건 셋이 말로 적혀 있다: 금지, 범위, 분량.
  stated:
    "Review the retry loop in the sync module and report what you find. " +
    "Start with the backoff timing, then check how errors are counted. " +
    "Do not modify any files. Only read code under src/sync. " +
    "Keep the report under 200 words. " +
    "Write the report so a new teammate could follow it without opening the code.",
  // 조건처럼 들리지만 세상의 사실을 말하는 문장뿐이다. 태그가 하나도 서면 안 된다.
  facts:
    "The tests do not pass on main. Only three of them fail. " +
    "Investigate why the retry loop gives up early and report the cause.",
  // 첫 문장 하나가 200자를 넘는다: 문장 중간에서 자를 수밖에 없다.
  runOn:
    "Go through every file under the sync module and compare how each of them handles a failed request and a timed out request and a request that came back with an empty body then write down for each file which of the three cases is handled and which is not so that the person reading it can see the gaps at a glance",
  short: "Look at the retry loop and tell me why it gives up early.",
};
// [지시, 조건으로 인용돼야 할 절, 조건이 아니라 사실을 말하는 절] — 다섯 말로.
const HELPER_BRIEFS_BY_LANGUAGE = {
  ko: ["동기화 모듈의 재시도 루프를 살펴보고 원인을 보고해 주세요. 파일은 수정하지 마세요. 테스트가 통과하지 않는다.", "파일은 수정하지 마세요", "테스트가 통과하지 않는다"],
  en: ["Review the retry loop in the sync module and report the cause. Never touch the lockfile. The build does not finish on main.", "Never touch the lockfile", "The build does not finish on main"],
  ja: ["同期モジュールの再試行ループを調べて原因を報告してください。ファイルは変更しないでください。テストは通らない。", "ファイルは変更しないでください", "テストは通らない"],
  zh: ["请检查同步模块的重试循环并报告原因。不要修改任何文件。测试没有通过。", "不要修改任何文件", "测试没有通过"],
  es: ["Revisa el bucle de reintentos del módulo de sincronización e informa de la causa. No modifiques ningún archivo. Las pruebas no pasan en main.", "No modifiques ningún archivo", "Las pruebas no pasan en main"],
};

async function testHelperPage(page, ok) {
  const agentSource = await readFile(new URL("../../crates/zerocode-core/src/agent.rs", import.meta.url), "utf8");
  const catalogIds = [...agentSource.matchAll(/^ {8}id: "([^"]+)"/gm)].map((one) => one[1]);
  const catalogNames = [...agentSource.matchAll(/^ {8}name: "([^"]+)"/gm)].map((one) => one[1]);
  // The road each catalog row names to stop one helper (`AgentVoice.helper_stop`),
  // read from the voice table; a row the table does not voice has none.
  const voiced = [...agentSource.matchAll(/^ {8}"([^"]+)",\n {8}AgentVoice \{([\s\S]*?)^ {8}\},$/gm)];
  const catalogRoads = Object.fromEntries(voiced.map((one) =>
    [one[1], /^\s*helper_stop: Some\("([^"]+)"\),$/m.exec(one[2])?.[1] ?? null]));
  const shellSource = await readFile(new URL("../shell.js", import.meta.url), "utf8");
  const styleSource = await readFile(new URL("../shell.css", import.meta.url), "utf8");

  await page.evaluate(async () => {
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    const settle = (ms = 100) => new Promise((done) => setTimeout(done, ms));
    const shown = (node) =>
      Boolean(node) && node.getClientRects().length > 0 && getComputedStyle(node).visibility !== "hidden";
    const words = (node) => (node?.textContent ?? "").replace(/\s+/g, " ").trim();
    const logs = {};
    const made = [];
    const parts = () => {
      const host = document.querySelector("#worker-view");
      const find = (selector) => host?.querySelector(selector) ?? null;
      return {
        host,
        head: find(".helper-head"),
        crumb: find(".helper-crumb"),
        sibs: find(".helper-sibs"),
        card: find(".helper-brief"),
        turns: find(".helper-turns"),
        foot: find(".helper-foot"),
      };
    };
    // 도우미의 파일은 줄마다 시각을 찍는다 — 머리의 시계는 그 시각들의 폭이다(t-18702): 시각이
    // 없는 파일의 페이지는 시계를 그리지 않으므로, 시계가 있는 머리를 보는 시험은 찍힌 파일을 쓴다.
    const stampedLog = (brief) => [
      { role: "user", text: brief, at_ms: Date.now() - 41_000 },
      { role: "assistant", text: "reading", at_ms: Date.now() - 1_000 },
    ];
    // 부모 판 하나와 그 명부. 도우미마다 전사가 있어 그 페이지가 열린다.
    const scene = async (roster, { model = "model-o" } = {}) => {
      window.__ANSWER__.subagent_log = ({ id, after }) => {
        const all = logs[id];
        if (!all) return { found: false };
        return { found: true, next: all.length, turns: all.slice(after ?? 0), skipped: false };
      };
      const term = await openTermTab({ placement: "tab" });
      made.push(term);
      const owner = tabOfTerm(term);
      tell("hook:agent", { term, state: "working", agent: "claude", session: `s-${term}`, model });
      tell("hook:subagent", { term, rows: roster });
      for (const row of roster) logs[row.id] ??= stampedLog(`Look into ${row.name}.`);
      await window.__PAINTED__();
      return { term, owner };
    };
    const open = async (at, sub, { agent = "claude", brief } = {}) => {
      if (brief !== undefined || !logs[sub.id]) logs[sub.id] = stampedLog(brief ?? `Look into ${sub.name}.`);
      await openHelperPage({ term: at.term, agent, worktree: at.owner.worktree, tab: at.owner }, sub);
      await settle();
      return parts();
    };
    // A catalog of the test's own making, laid over the harness's rows: which
    // row names a road to stop one helper is the rows' own say (t-16943).
    const harnessAgents = await invoke("list_agents", {});
    const useAgents = async (given) => {
      // The shape every `list_agents` row has, under what the test says of each.
      const rows = given.map((row) => ({
        favicon_domain: "", homepage_url: "", found_as: row.id, unsupported_here: false,
        missing_requirement: null, takes_a_paste: true, ready: "quiet", ...row,
      }));
      window.__ANSWER__.list_agents = () => [...rows, ...harnessAgents.filter((one) => !rows.some((row) => row.id === one.id))];
      await refreshAgents();
    };
    const cleanup = async () => {
      setConversationFocusView(false);
      if (window.__ANSWER__.list_agents) {
        delete window.__ANSWER__.list_agents;
        await refreshAgents();
      }
      for (const term of made.splice(0)) {
        if (paneChatOn(term)) await setPaneChat(term, false);
        tell("hook:subagent", { term, rows: [] });
        tell("term:exited", { term });
      }
      for (const key of Object.keys(logs)) delete logs[key];
      for (const command of ["subagent_log", "pane_log", "term_key", "wire_interrupt", "stop_pane_helper"]) {
        delete window.__ANSWER__[command];
      }
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
    };
    window.__HP__ = { tell, settle, shown, words, logs, parts, scene, open, useAgents, cleanup };
  });

  /* ---- C1: 머리가 부모 대화를 말하고 한 번에 돌아가며, 같은 부모의 도우미들이 한 띠에 선다 ---- */
  const strip = await page.evaluate(async (briefs) => {
    const { tell, settle, shown, words, parts, scene, open, cleanup } = window.__HP__;
    const seen = {};
    const rows = (over = {}) => [
      { id: "h-a", name: "@auditor", state: "running", model: "model-s", requestedModel: "sonnet", effort: "high", tool_calls: 3 },
      { id: "h-b", name: "@mapper", state: "running", model: "model-s" },
      { id: "h-c", name: "@checker", state: "done", model: "model-s" },
      { id: "h-d", name: "@linter", state: "done", model: "model-s" },
    ].map((row) => ({ ...row, ...(over[row.id] ?? {}) }));
    const at = await scene(rows());
    // 남의 부모 밑의 도우미는 이 띠에 서지 않는다.
    await scene([{ id: "h-z", name: "@stranger", state: "running" }]);
    const here = `helper:${at.term}:h-a`;
    const chips = () => [...(parts().sibs?.querySelectorAll(".helper-sib") ?? [])].filter(shown).map((node) => ({
      tag: node.tagName,
      words: words(node),
      current: node.getAttribute("aria-current"),
      expanded: node.getAttribute("aria-expanded"),
    }));
    seen.running = subagentStateWords("running");
    seen.done = subagentStateWords("done");
    let p = await open(at, rows()[0], { brief: briefs.stated });
    seen.parentLabel = tabLabel(at.owner);
    seen.crumbLabel = p.crumb?.getAttribute("aria-label") ?? null;
    seen.crumbButtons = [...(p.crumb?.querySelectorAll("button") ?? [])].map((one) => ({
      words: words(one),
      label: one.getAttribute("aria-label"),
    }));
    seen.crumbHere = words(p.crumb?.querySelector(".helper-crumb-here"));
    seen.oneHead = document.querySelectorAll("#worker-view .worker-head").length;
    seen.name = words(p.head?.querySelector(".worker-name"));
    seen.state = words(p.head?.querySelector(".worker-state"));
    seen.dotWhileRunning = p.head?.querySelector(".helper-state-dot") != null;
    seen.uses = words(p.head?.querySelector(".worker-uses"));
    seen.metaHolds = ["worker-focus", "worker-elapsed", "worker-uses", "worker-state"]
      .every((name) => p.head?.querySelector(`.worker-meta .${name}`) != null);

    // 부모로 가는 길은 한 번의 누름이다.
    p.crumb?.querySelector("button")?.click();
    seen.afterBack = activeTabId === at.owner.id;
    setActiveTab(here);
    await settle(60);
    p = parts();

    seen.sibsLabel = p.sibs?.getAttribute("aria-label") ?? null;
    seen.chips = chips();
    seen.strangerSeen = words(p.sibs).includes("@stranger");
    p.sibs?.querySelector(".helper-sib-fold")?.click();
    await settle(60);
    seen.chipsOpen = chips();
    parts().sibs?.querySelector(".helper-sib-fold")?.click();
    await settle(60);
    seen.chipsClosedAgain = chips();

    // 형제 하나를 한 번 누르면 그 도우미의 페이지로 간다.
    [...(parts().sibs?.querySelectorAll("button.helper-sib") ?? [])]
      .find((one) => words(one).startsWith("@mapper"))?.click();
    await settle(150);
    seen.movedTo = activeTabId;
    seen.expectedMove = `helper:${at.term}:h-b`;
    p = parts();
    seen.movedName = words(p.head?.querySelector(".worker-name"));
    seen.movedChips = chips();

    // 명부가 움직이면 띠가 따라간다.
    setActiveTab(here);
    await settle(60);
    tell("hook:subagent", { term: at.term, rows: rows({ "h-b": { state: "done" } }) });
    await settle(100);
    seen.chipsAfterFinish = chips();
    tell("hook:subagent", { term: at.term, rows: rows({ "h-a": { state: "done" }, "h-b": { state: "done" } }) });
    await settle(100);
    p = parts();
    seen.finishedState = words(p.head?.querySelector(".worker-state"));
    seen.dotWhenDone = p.head?.querySelector(".helper-state-dot") != null;
    seen.finishedChips = chips();
    tell("hook:subagent", { term: at.term, rows: [rows()[0]] });
    await settle(100);
    seen.aloneShown = shown(parts().sibs);

    // 지시하던 부모 대화가 닫혔다: 돌아갈 곳이 없으니 돌아가는 조종도 없다.
    dropTab(at.owner.id);
    setActiveTab(here);
    await settle(100);
    p = parts();
    seen.orphan = {
      crumbButtons: p.crumb?.querySelectorAll("button").length ?? null,
      crumbHere: words(p.crumb?.querySelector(".helper-crumb-here")),
      foot: words(p.foot),
      footButtons: p.foot?.querySelectorAll("button").length ?? null,
    };
    await cleanup();
    return seen;
  }, HELPER_BRIEFS);
  ok(
    "a helper's page names its parent conversation in a crumb, and one press on it goes back to that conversation",
    strip.crumbButtons.length === 1 &&
      strip.crumbButtons[0].words.includes(strip.parentLabel) &&
      strip.crumbButtons[0].label?.includes(strip.parentLabel) === true &&
      strip.crumbLabel?.length > 0 &&
      strip.crumbHere.length > 0 &&
      strip.afterBack === true,
    JSON.stringify(strip),
  );
  ok(
    "a helper's page has one head with its name, its state, its tool count and its clock in one group, and the state's dot shows only while it runs",
    strip.oneHead === 1 &&
      strip.name === "@auditor" &&
      strip.state.length > 0 &&
      strip.state !== strip.finishedState &&
      strip.uses.includes("3") &&
      strip.metaHolds === true &&
      strip.dotWhileRunning === true &&
      strip.dotWhenDone === false,
    JSON.stringify(strip),
  );
  ok(
    "the helpers of one parent stand in one strip: this one marked as current, the running ones as buttons that say their state, the finished ones folded into a count",
    strip.sibsLabel?.length > 0 &&
      strip.chips.length === 3 &&
      strip.chips[0].tag === "SPAN" && strip.chips[0].current === "true" &&
      strip.chips[0].words.startsWith("@auditor") && strip.chips[0].words.includes(strip.running) &&
      strip.chips[1].tag === "BUTTON" && strip.chips[1].words.startsWith("@mapper") &&
      strip.chips[1].words.includes(strip.running) &&
      strip.chips[2].tag === "BUTTON" && strip.chips[2].expanded === "false" &&
      strip.chips[2].words.includes("2") &&
      !strip.chips.some((chip) => /@checker|@linter/.test(chip.words)),
    JSON.stringify(strip),
  );
  ok(
    "the folded count opens and closes on a press and lists each finished helper with its state, and one press on a running sibling moves to that helper's page",
    strip.chipsOpen.length === 5 &&
      strip.chipsOpen.filter((chip) =>
        chip.tag === "BUTTON" && /@checker|@linter/.test(chip.words) && chip.words.includes(strip.done)).length === 2 &&
      strip.chipsOpen.find((chip) => chip.expanded !== null)?.expanded === "true" &&
      strip.chipsClosedAgain.length === 3 &&
      strip.movedTo === strip.expectedMove &&
      strip.movedName === "@mapper" &&
      strip.movedChips.some((chip) => chip.current === "true" && chip.words.startsWith("@mapper")) &&
      strip.movedChips.some((chip) => chip.tag === "BUTTON" && chip.words.startsWith("@auditor")),
    JSON.stringify(strip),
  );
  ok(
    "the strip follows the roster: a sibling that finishes moves into the count, a helper of another parent never appears, and a helper on its own shows no strip",
    strip.chipsAfterFinish.length === 2 &&
      strip.chipsAfterFinish[1].words.includes("3") &&
      !strip.chipsAfterFinish.some((chip) => chip.words.startsWith("@mapper")) &&
      strip.finishedChips.length === 2 &&
      strip.finishedChips[0].current === "true" &&
      strip.finishedChips[0].words.startsWith("@auditor") &&
      strip.finishedChips[1].words.includes("3") &&
      strip.strangerSeen === false &&
      strip.aloneShown === false,
    JSON.stringify(strip),
  );
  ok(
    "a helper whose directing conversation has closed has a crumb with no control to go back to, and a footer that says so and offers nothing",
    strip.orphan.crumbButtons === 0 &&
      strip.orphan.crumbHere.length > 0 &&
      strip.orphan.foot.length > 0 &&
      strip.orphan.footButtons === 0,
    JSON.stringify(strip),
  );

  /* ---- C2: 무엇을 시켰는가는 카드다 ---- */
  const card = await page.evaluate(async ({ briefs, byLanguage }) => {
    const { tell, settle, shown, words, parts, scene, open, logs, cleanup } = window.__HP__;
    const seen = {};
    const cases = {
      ...briefs,
      ...Object.fromEntries(Object.entries(byLanguage).map(([code, [text]]) => [`lang-${code}`, text])),
    };
    const roster = Object.keys(cases).map((key) => ({ id: `h-${key}`, name: `@${key}`, state: "running", model: "model-s" }));
    const at = await scene(roster);
    const read = (p) => {
      const q = (selector) => p.card?.querySelector(selector) ?? null;
      const more = q(".helper-brief-more");
      const full = q(".helper-brief-full");
      return {
        present: p.card !== null,
        said: q(".helper-brief-said")?.textContent ?? null,
        cut: q(".helper-brief-cut")?.textContent ?? null,
        cutSaid: words(q(".helper-brief-text .helper-sr")),
        tags: [...(p.card?.querySelectorAll(".helper-brief-tag") ?? [])].map((node) => node.textContent),
        tagsRow: q(".helper-brief-tags") !== null,
        more: more ? words(more) : null,
        expanded: more?.getAttribute("aria-expanded") ?? null,
        controls: more?.getAttribute("aria-controls") ?? null,
        fullId: full?.id ?? null,
        fullShown: shown(full),
        full: full?.textContent ?? null,
      };
    };
    for (const key of Object.keys(cases)) {
      seen[key] = read(await open(at, roster.find((row) => row.id === `h-${key}`), { brief: cases[key] }));
    }

    // 조건이 말로 적힌 지시의 카드를 자세히 본다.
    let p = await open(at, roster.find((row) => row.id === "h-stated"));
    const first = p.card;
    const more = first?.querySelector(".helper-brief-more") ?? null;
    if (first) {
      const style = getComputedStyle(first);
      seen.box = {
        tag: first.tagName,
        inputs: first.querySelectorAll("textarea, input, select, [contenteditable]").length,
        borders: [style.borderTopWidth, style.borderRightWidth, style.borderBottomWidth, style.borderLeftWidth],
        cursor: style.cursor,
        label: words(first.querySelector(".helper-brief-label")),
        aria: first.getAttribute("aria-label"),
      };
    }
    const briefing = p.host?.querySelector(".helper-turn.is-briefing") ?? null;
    seen.briefingRow = briefing === null ? null : getComputedStyle(briefing).display;
    seen.assistantRowShown = shown(p.host?.querySelector(".helper-turn.is-assistant"));
    if (more) {
      const fullStyle = getComputedStyle(first.querySelector(".helper-brief-full"));
      seen.fullStyle = { white: fullStyle.whiteSpace, overflowY: fullStyle.overflowY, maxHeight: fullStyle.maxHeight };
      more.focus();
      more.click();
      await settle(60);
      seen.opened = read(parts());
      seen.openedTabindex = parts().card?.querySelector(".helper-brief-full")?.getAttribute("tabindex") ?? null;
      seen.moreWordsOpen = words(more);
      more.click();
      await settle(60);
      seen.closedAgain = read(parts());
      more.click();
      await settle(60);
    }
    // 명부가 움직여 페이지가 다시 그려져도 카드는 다시 지어지지 않고 초점도 그대로다.
    tell("hook:subagent", {
      term: at.term,
      rows: roster.map((row) => (row.id === "h-stated" ? { ...row, tool_calls: 9 } : row)),
    });
    await settle(100);
    seen.sameCard = parts().card === first && first !== null;
    seen.sameMore = parts().card?.querySelector(".helper-brief-more") === more && more !== null;
    seen.stillOpen = more?.getAttribute("aria-expanded") === "true";
    seen.focusHeld = more !== null && document.activeElement === more;
    // 턴이 사백 개를 넘어 0번 턴이 밀려나도 카드는 지시를 잃지 않는다.
    logs["h-stated"].push(...Array.from({ length: 450 }, (_, i) => ({ role: "assistant", text: `note ${i}` })));
    await pollHelperPages();
    await settle(150);
    const held = tabs.find((one) => one.id === `helper:${at.term}:h-stated`)?.worker.helper;
    seen.spliced = held !== undefined && held.turns.every((turn) => turn.seq !== 0) && held.turns.length <= 400;
    seen.saidAfterCap = parts().card?.querySelector(".helper-brief-said")?.textContent ?? null;
    seen.fullAfterCap = parts().card?.querySelector(".helper-brief-full")?.textContent ?? null;
    await cleanup();
    return seen;
  }, { briefs: HELPER_BRIEFS, byLanguage: HELPER_BRIEFS_BY_LANGUAGE });
  const stated = HELPER_BRIEFS.stated;
  // The sentences that are not conditions, as many whole ones as fit: the conditions are the tags, so the summary does not say them twice.
  const statedSummary = stated.slice(0, stated.indexOf(" Do not modify"));
  ok(
    "the card shows the first whole sentences that are not conditions, up to a limit, names no cut where it cut at a sentence, and offers the whole instruction with its length in one press",
    card["stated"].present === true &&
      card["stated"].said === statedSummary &&
      statedSummary.endsWith("are counted.") && statedSummary.length <= 200 &&
      card["stated"].cut === null && card["stated"].cutSaid === "" &&
      card["stated"].more?.includes(String(stated.length)) === true &&
      card["stated"].expanded === "false" &&
      card["stated"].fullShown === false &&
      card.opened.expanded === "true" &&
      card.opened.fullShown === true &&
      card.opened.full === stated &&
      card.moreWordsOpen !== card["stated"].more &&
      card.opened.controls === card.opened.fullId &&
      card.closedAgain.expanded === "false" && card.closedAgain.fullShown === false &&
      card.fullStyle?.white === "pre-wrap" && card.fullStyle?.overflowY === "auto" && card.fullStyle?.maxHeight !== "none" &&
      card.openedTabindex === "0",
    JSON.stringify(card),
  );
  ok(
    "an instruction short enough to show whole has no whole-instruction control and no hidden text, and a card that had to cut inside one long sentence says so both in sight and to a screen reader",
    card["short"].said === HELPER_BRIEFS.short && card["short"].more === null && card["short"].fullId === null &&
      card["facts"].more === null &&
      card["runOn"].said?.length <= 200 &&
      HELPER_BRIEFS.runOn.startsWith(card["runOn"].said ?? "\u0000") &&
      HELPER_BRIEFS.runOn[(card["runOn"].said ?? "").length] === " " &&
      card["runOn"].cut === "…" &&
      card["runOn"].cutSaid.length > 0 &&
      card["runOn"].more?.includes(String(HELPER_BRIEFS.runOn.length)) === true,
    JSON.stringify(card),
  );
  const withItsStop = (sentence) => new RegExp(sentence.replace(/[.*+?^$()|[\]\\{}]/g, "\\$&") + "[。.！!？?]?\\s*");
  const tagChecks = Object.fromEntries(Object.entries(HELPER_BRIEFS_BY_LANGUAGE).map(([code, [text, tag, fact]]) => [code, {
    exactly: JSON.stringify(card[`lang-${code}`].tags) === JSON.stringify([tag]),
    noFact: !card[`lang-${code}`].tags.some((one) => one.includes(fact)),
    // The summary is the instruction without its condition's sentence — the fact stays in it, the gap the next sentence had too.
    whole: card[`lang-${code}`].said === text.replace(withItsStop(tag), "").trim(),
  }]));
  const quoted = [...card["stated"].tags, ...card["lang-en"].tags].every((one) =>
    [stated, HELPER_BRIEFS_BY_LANGUAGE.en[0]].some((text) => text.includes(one.replace(/…$/, ""))));
  ok(
    "conditions show as short tags only where the instruction states them, each a word-for-word quote, and a sentence that merely mentions a condition or a fact makes no tag, in five languages",
    JSON.stringify(card["stated"].tags) === JSON.stringify(["Do not modify any files", "Only read code under src/sync", "Keep the report under 200 words"]) &&
      card["facts"].tags.length === 0 && card["facts"].tagsRow === false &&
      card["runOn"].tagsRow === false && card["short"].tagsRow === false &&
      quoted &&
      Object.values(tagChecks).every((one) => one.exactly && one.noFact && one.whole),
    JSON.stringify({ tagChecks, tags: card["stated"].tags, facts: card["facts"] }),
  );
  ok(
    "the card is a labelled region and not an input box, with a side rule and no outline and no text cursor, and the briefing bubble is not drawn a second time under it",
    card.box?.tag === "SECTION" && card.box.inputs === 0 &&
      JSON.stringify(card.box.borders) === JSON.stringify(["0px", "0px", "0px", "3px"]) &&
      card.box.cursor !== "text" && card.box.label.length > 0 && card.box.aria?.length > 0 &&
      card.briefingRow === "none" && card.assistantRowShown === true,
    JSON.stringify(card),
  );
  ok(
    "a repaint from the roster leaves the card, its open state and its focus alone, and the card keeps the instruction after the turn list has dropped its first turn",
    card.sameCard === true && card.sameMore === true && card.stillOpen === true && card.focusHeld === true &&
      card.spliced === true &&
      card.saidAfterCap === statedSummary && card.fullAfterCap === stated,
    JSON.stringify(card),
  );

  /* ---- C3: 입력줄이 서 있던 자리에는 푸터가 선다 ---- */
  const footer = await page.evaluate(async () => {
    const { settle, words, parts, scene, open, cleanup } = window.__HP__;
    const seen = {};
    const roster = [{ id: "h-a", name: "@auditor", state: "running", model: "model-s" }];
    const at = await scene(roster, { model: "model-o" });
    let p = await open(at, roster[0]);
    seen.parentLabel = tabLabel(at.owner);
    seen.lastChildIsFoot = p.foot !== null && p.host?.lastElementChild === p.foot;
    seen.says = words(p.foot?.querySelector(".helper-foot-says"));
    seen.buttons = [...(p.foot?.querySelectorAll("button") ?? [])].map((one) => words(one));
    // A place for the road a catalog row may name one day (t-16031 gives zo one):
    // one group of controls in the footer, its first the speaking button, and a
    // second control laid in beside it sits on the same row, clear of it.
    const group = p.foot?.querySelector(".helper-foot-actions") ?? null;
    const speaking = p.foot?.querySelector(".helper-foot-speak") ?? null;
    seen.place = {
      stands: group !== null && group.parentElement === p.foot,
      holds: group !== null && speaking !== null && speaking.parentElement === group,
      flex: group !== null && getComputedStyle(group).display === "flex",
      beside: false,
    };
    if (group && speaking) {
      const spare = document.createElement("button");
      spare.type = "button";
      spare.className = speaking.className;
      spare.textContent = speaking.textContent;
      group.appendChild(spare);
      const first = speaking.getBoundingClientRect();
      const second = spare.getBoundingClientRect();
      seen.place.beside = Math.abs(first.top - second.top) <= 2 && second.left >= first.right - 1 &&
        group.getBoundingClientRect().right <= p.foot.getBoundingClientRect().right + 1;
      spare.remove();
    }
    seen.composerParts = p.host?.querySelectorAll(
      ".chat-dock, .worker-composer, .worker-composer-box, textarea, .worker-composer-door, button.worker-where, .worker-composer-agent, .worker-composer-mode",
    ).length ?? null;
    seen.parentModelShown = words(p.host).includes("model-o");
    seen.stopWords = /멈추|중지|중단|stop|halt|kill|cancel|停止|中止|detener/i
      .test([p.head, p.sibs, p.card, p.foot].map((node) => words(node)).join(" "));

    // 부모가 터미널일 때: 누르면 부모의 탭이 서고 터미널의 입력에 초점이 간다.
    p.foot?.querySelector(".helper-foot-speak")?.click();
    await settle(250);
    seen.terminalDoor = { tab: activeTabId === at.owner.id, focus: document.activeElement === keySink };

    // 부모가 대화 판일 때: 부모 페이지의 입력 상자에 초점이 간다.
    window.__ANSWER__.pane_log = () => ({
      found: true, next: 2, skipped: false, more: false, folded: false, model: "model-o",
      turns: [{ role: "user", text: "Fix the sync bug." }, { role: "assistant", text: "On it." }],
    });
    await setPaneChat(at.term, true);
    await settle(150);
    p = await open(at, roster[0]);
    p.foot?.querySelector(".helper-foot-speak")?.click();
    await settle(300);
    const focused = document.activeElement;
    seen.chatDoor = {
      tab: activeTabId === at.owner.id,
      box: focused?.matches?.(".worker-composer-box") === true,
      inParentChat: Boolean(focused?.closest?.(`.pane-slot[data-term="${at.term}"] .pane-chat`)),
    };
    await cleanup();
    return seen;
  });
  ok(
    "the footer stands where the composer stood, says which conversation directs the helper, holds one button that speaks to that parent in a group with room beside it for one more, and nothing on the page reads the parent's model or offers a stop",
    footer.lastChildIsFoot === true &&
      footer.says.includes(footer.parentLabel) &&
      footer.buttons.length === 1 && footer.buttons[0].length > 0 &&
      footer.place.stands && footer.place.holds && footer.place.flex && footer.place.beside &&
      footer.composerParts === 0 &&
      footer.parentModelShown === false &&
      footer.stopWords === false,
    JSON.stringify(footer),
  );
  ok(
    "the press on speaking to the parent goes to the parent's page with its input focused: the terminal's input for a terminal, the composer box for a conversation",
    footer.terminalDoor.tab === true && footer.terminalDoor.focus === true &&
      footer.chatDoor.tab === true && footer.chatDoor.box === true && footer.chatDoor.inParentChat === true,
    JSON.stringify(footer),
  );

  // Esc: 도우미의 페이지에서 Esc는 부모의 턴 전체를 끊지 않는다 — 끊는 길은 부모 판의 것이다.
  await page.evaluate(async () => {
    const { settle, parts, scene, open } = window.__HP__;
    const roster = [{ id: "h-a", name: "@auditor", state: "running", model: "model-s" }];
    const at = await scene(roster);
    window.__ESC__ = { sent: [], focusedInPage: false };
    window.__ANSWER__.term_key = (args) => { window.__ESC__.sent.push(["term_key", args]); return null; };
    window.__ANSWER__.wire_interrupt = (args) => { window.__ESC__.sent.push(["wire_interrupt", args]); return null; };
    const p = await open(at, roster[0]);
    p.host?.querySelector("button")?.focus();
    await settle(50);
    window.__ESC__.focusedInPage = Boolean(p.host?.contains(document.activeElement)) && document.activeElement !== p.host;
    window.__ESC__.working = hookStates.get(at.term);
  });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  const esc = await page.evaluate(async () => {
    const seen = { ...window.__ESC__ };
    delete window.__ESC__;
    await window.__HP__.cleanup();
    return seen;
  });
  ok(
    "a plain Esc on a helper's page, with its parent at work, sends no interrupt: it cannot end the parent's whole turn from a page that has no input",
    esc.focusedInPage === true && esc.working === "working" && esc.sent.length === 0,
    JSON.stringify(esc),
  );

  /* ---- C6: 도우미 하나를 멈추는 단추 — 행이 길을 말할 때, 도는 도우미에만 (t-16943) ---- */
  const stopping = await page.evaluate(async () => {
    const { tell, settle, words, parts, scene, open, useAgents, cleanup } = window.__HP__;
    await useAgents([
      { id: "agent-with-road", name: "Agent R", installed: true, helper_stop: "road.to.one.helper" },
      { id: "agent-without", name: "Agent N", installed: true },
    ]);
    const rows = (over = {}) => [
      { id: "h-a", name: "@auditor", state: "running", model: "model-s" },
      { id: "h-b", name: "@mapper", state: "running", model: "model-s" },
      { id: "h-c", name: "@checker", state: "running", model: "model-s" },
      { id: "h-d", name: "@linter", state: "done", model: "model-s" },
      { id: "h-e", name: "@plain", state: "running", model: "model-s" },
    ].map((row) => ({ ...row, ...(over[row.id] ?? {}) }));
    const at = await scene(rows());
    tell("hook:agent", { term: at.term, state: "working", agent: "claude", session: { key: "session_id", id: "sess-parent", transcript_path: null } });
    await settle(30);
    const calls = [];
    let reply = null;
    window.__ANSWER__.stop_pane_helper = (args) => {
      calls.push(JSON.parse(JSON.stringify(args)));
      return new Promise((resolve, reject) => { reply = { resolve, reject }; });
    };
    const stopOf = () => parts().foot?.querySelector(".helper-foot-stop") ?? null;
    const told = () => words(parts().foot?.querySelector(".helper-foot-told"));
    const road = { agent: "agent-with-road" };
    const seen = {};

    let p = await open(at, rows()[0], road);
    seen.group = [...(p.foot?.querySelector(".helper-foot-actions")?.querySelectorAll("button") ?? [])].map((one) => one.className);
    seen.idle = { words: words(stopOf()), disabled: stopOf()?.getAttribute("aria-disabled") ?? null, told: told() };
    seen.running = workerStatusWords({ status: "running" });

    // 누름: 단추는 곧바로 「멈추는 중…」이 되어 더 눌리지 않는다.
    const button = stopOf();
    // No button, nothing to press: every check below reads as failed, not as a crash.
    if (!button) {
      seen.missing = true;
      await cleanup();
      return seen;
    }
    const pressed = performance.now();
    button.click();
    seen.pressToBusyMs = button.getAttribute("aria-disabled") === "true" ? Math.round((performance.now() - pressed) * 1000) / 1000 : null;
    seen.busy = { words: words(button), focusable: button.disabled === false };
    button.click();
    button.click();
    await settle(50);
    seen.callsWhileOut = calls.length;
    seen.args = calls[0] ?? null;
    seen.session = paneSessions.get(at.term)?.session?.id ?? null;

    // 답: 그 말이 한 박자 선다 — 상태는 아직 명부가 정하지 않았다.
    let paintedAt = null;
    const watch = new MutationObserver(() => {
      if (paintedAt === null && words(button) !== seen.busy.words) paintedAt = performance.now();
    });
    watch.observe(button, { childList: true, characterData: true, subtree: true });
    const answered = performance.now();
    reply?.resolve({ status: "stopped", agent_id: "h-a", record_key: "stopped_by_person" });
    await settle(40);
    watch.disconnect();
    seen.replyToWordsMs = paintedAt === null ? null : Math.round((paintedAt - answered) * 1000) / 1000;
    seen.toldWords = words(button);
    seen.stoppedWords = helperStopWords("stopped");
    seen.told = told();
    seen.stateBeforeRoster = words(parts().head?.querySelector(".worker-state"));

    // 명부가 끝났다고 말하면 머리가 따라가고, 박자가 지나면 단추는 걷힌다.
    tell("hook:subagent", { term: at.term, rows: rows({ "h-a": { state: "done" } }) });
    await settle(120);
    seen.stateAfterRoster = words(parts().head?.querySelector(".worker-state"));
    seen.byPerson = helperStopWords("stopped_by_person");
    seen.stopDuringBeat = stopOf() !== null;
    await settle(2600);
    seen.afterBeat = { buttons: parts().foot?.querySelectorAll("button").length ?? null, stop: stopOf() !== null, told: parts().foot?.querySelector(".helper-foot-told") !== null };

    // 끝난 도우미와 길이 없는 행의 도우미에는 단추가 없다.
    p = await open(at, rows()[3], road);
    seen.finishedButtons = p.foot?.querySelectorAll("button").length ?? null;
    p = await open(at, rows()[4], { agent: "agent-without" });
    seen.roadlessButtons = p.foot?.querySelectorAll("button").length ?? null;

    // 답이 오기 전에 페이지를 떠나면 그 답은 버려진다 — 다른 도우미의 페이지에 칠해지지 않는다.
    p = await open(at, rows()[1], road);
    const left = stopOf();
    left?.click();
    p = await open(at, rows()[2], road);
    const other = stopOf();
    reply?.resolve({ status: "failed", agent_id: "h-b", record_key: null, detail: "a-detail-for-h-b" });
    await settle(60);
    seen.leftBehind = {
      leftConnected: left?.isConnected ?? null,
      otherWords: words(other),
      otherDisabled: other?.getAttribute("aria-disabled") ?? null,
      otherTold: told(),
      detailShown: words(parts().host).includes("a-detail-for-h-b") || other?.getAttribute("data-tip") !== null,
      calls: calls.length,
    };
    p = await open(at, rows()[1], road);
    seen.backOnLeft = { words: words(stopOf()), disabled: stopOf()?.getAttribute("aria-disabled") ?? null };

    // 답의 말: 일곱 가지 status, 모르는 낱말, 창이 채널에 닿지 못한 거절.
    const kinds = ["stopped", "closed", "already_finished", "not_found", "not_owned", "unreachable", "failed", "a-word-from-later", "refused"];
    seen.said = {};
    for (const kind of kinds) {
      window.__ANSWER__.stop_pane_helper = (args) => {
        calls.push(JSON.parse(JSON.stringify(args)));
        if (kind === "refused") throw new Error("no channel for this session");
        return { status: kind, agent_id: args.agentId, record_key: null, ...(kind === "failed" && { detail: "worker gone" }) };
      };
      // A fresh page each time: leaving and coming back stands a new button.
      await open(at, rows()[1], road);
      await open(at, rows()[2], road);
      stopOf()?.click();
      await settle(40);
      seen.said[kind] = { words: words(stopOf()), told: told(), tip: stopOf()?.getAttribute("data-tip") ?? null, expected: helperStopWords(kind === "refused" ? "failed" : kind) };
    }
    seen.calls = calls.length;
    await cleanup();
    return seen;
  });
  ok(
    "a running helper whose agent's catalog row names a road has a second button beside speaking to the parent, and a finished helper or a row with no road has none",
    stopping.missing !== true &&
      JSON.stringify(stopping.group) === JSON.stringify(["helper-foot-speak", "helper-foot-stop"]) &&
      stopping.idle.words.length > 0 && stopping.idle.disabled === "false" && stopping.idle.told === "" &&
      stopping.finishedButtons === 1 && stopping.roadlessButtons === 1 &&
      stopping.afterBeat.buttons === 1 && stopping.afterBeat.stop === false && stopping.afterBeat.told === false,
    JSON.stringify(stopping),
  );
  ok(
    "a press asks the backend once with the pane's session, the agent and the helper's id and nothing else, says stopping at once, and takes no second press while the answer is out",
    stopping.missing !== true &&
      stopping.pressToBusyMs !== null && stopping.pressToBusyMs < 16 &&
      stopping.busy.words.length > 0 && stopping.busy.words !== stopping.idle.words && stopping.busy.focusable === true &&
      stopping.callsWhileOut === 1 &&
      JSON.stringify(stopping.args) === JSON.stringify({ session: "sess-parent", agent: "agent-with-road", agentId: "h-a" }) &&
      stopping.session === "sess-parent",
    JSON.stringify({ pressToBusyMs: stopping.pressToBusyMs, busy: stopping.busy, callsWhileOut: stopping.callsWhileOut, args: stopping.args, session: stopping.session }),
  );
  ok(
    "the answer's word stands on the button for a beat and is read out, the head keeps saying running until the roster says the helper ended, and then it says the person stopped it",
    stopping.missing !== true &&
      stopping.replyToWordsMs !== null && stopping.replyToWordsMs < 16 &&
      stopping.toldWords === stopping.stoppedWords && stopping.told.includes(stopping.stoppedWords) &&
      stopping.stateBeforeRoster === stopping.running &&
      stopping.stateAfterRoster === stopping.byPerson && stopping.byPerson !== stopping.stoppedWords &&
      stopping.stopDuringBeat === true,
    JSON.stringify({ replyToWordsMs: stopping.replyToWordsMs, toldWords: stopping.toldWords, told: stopping.told, before: stopping.stateBeforeRoster, after: stopping.stateAfterRoster, byPerson: stopping.byPerson }),
  );
  ok(
    "an answer that comes back after its page was left is dropped: the helper now showing keeps its own idle button, and the left helper's button stands idle when the person returns",
    stopping.missing !== true &&
      stopping.leftBehind.leftConnected === false &&
      stopping.leftBehind.otherWords === stopping.idle.words && stopping.leftBehind.otherDisabled === "false" &&
      stopping.leftBehind.otherTold === "" && stopping.leftBehind.detailShown === false &&
      stopping.leftBehind.calls === 2 &&
      stopping.backOnLeft.words === stopping.idle.words && stopping.backOnLeft.disabled === "false",
    JSON.stringify({ leftBehind: stopping.leftBehind, backOnLeft: stopping.backOnLeft }),
  );
  const saidKinds = ["stopped", "closed", "already_finished", "not_found", "not_owned", "unreachable", "failed"];
  ok(
    "each of the seven answers has its own words, a failure carries its detail in the tip and the read-out, and an unknown word or a refused call reads as not stopped",
    stopping.missing !== true &&
      Object.values(stopping.said).every((one) => one.words === one.expected && one.words.length > 0) &&
      new Set(saidKinds.map((kind) => stopping.said[kind].words)).size === saidKinds.length &&
      stopping.said.failed.tip === "worker gone" && stopping.said.failed.told.includes("worker gone") &&
      stopping.said["a-word-from-later"].words === stopping.said.failed.words &&
      stopping.said.refused.words === stopping.said.failed.words && stopping.said.refused.told.includes("no channel for this session") &&
      saidKinds.filter((kind) => kind !== "failed").every((kind) => stopping.said[kind].tip === null),
    JSON.stringify(stopping.said),
  );

  // Esc는 도우미의 페이지에서 여전히 아무것도 끊지 않는다 — 멈춤 단추에 초점이 있어도. Enter 한 번은 그 한 메서드를 한 번 보낸다.
  await page.evaluate(async () => {
    const { tell, settle, parts, scene, open, useAgents } = window.__HP__;
    await useAgents([{ id: "agent-with-road", name: "Agent R", installed: true, helper_stop: "road.to.one.helper" }]);
    const roster = [{ id: "h-a", name: "@auditor", state: "running", model: "model-s" }, { id: "h-b", name: "@mapper", state: "running" }];
    const at = await scene(roster);
    tell("hook:agent", { term: at.term, state: "working", agent: "claude", session: { key: "session_id", id: "sess-esc", transcript_path: null } });
    window.__STOPKEYS__ = { sent: [] };
    for (const command of ["term_key", "wire_interrupt", "stop_pane_helper"]) {
      window.__ANSWER__[command] = (args) => { window.__STOPKEYS__.sent.push([command, JSON.parse(JSON.stringify(args))]); return command === "stop_pane_helper" ? { status: "stopped", agent_id: args.agentId, record_key: "stopped_by_person" } : null; };
    }
    await open(at, roster[0], { agent: "agent-with-road" });
    await settle(50);
    parts().foot?.querySelector(".helper-foot-stop")?.focus();
    window.__STOPKEYS__.focused = document.activeElement?.classList.contains("helper-foot-stop") === true;
    window.__ORDER__ = [];
  });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  const afterEsc = await page.evaluate(() => ({ sent: [...window.__STOPKEYS__.sent], order: [...window.__ORDER__] }));
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);
  const stopKeys = await page.evaluate(async () => {
    const seen = { ...window.__STOPKEYS__, order: [...window.__ORDER__] };
    delete window.__STOPKEYS__;
    delete window.__ORDER__;
    await window.__HP__.cleanup();
    return seen;
  });
  const ending = /^(?:stop_pane_helper|term_key|term_paste|wire_interrupt|wire_stop|end_terminal_session|end_all_terminal_sessions)$/;
  ok(
    "on a helper's page Esc sends nothing even with the stop button focused, and one Enter on it sends the one stop, with the one helper's id, once — no interrupt of the parent's turn",
    stopKeys.focused === true &&
      afterEsc.sent.length === 0 && !afterEsc.order.some((command) => ending.test(command)) &&
      stopKeys.sent.length === 1 &&
      JSON.stringify(stopKeys.sent[0]) === JSON.stringify(["stop_pane_helper", { session: "sess-esc", agent: "agent-with-road", agentId: "h-a" }]) &&
      stopKeys.order.filter((command) => ending.test(command)).length === 1,
    JSON.stringify({ afterEsc, stopKeys }),
  );

  /* ---- C3 · C4: 카탈로그의 모든 에이전트에서 머리는 하나이고, 어느 에이전트에도 도우미 하나만 멈추는 길이 없다 ---- */
  const everyAgent = await page.evaluate(async ({ ids, rows, brief }) => {
    const { words, parts, scene, open, useAgents, cleanup } = window.__HP__;
    // Every catalog row as the backend lists it, each with its own road or none.
    await useAgents(rows);
    const at = await scene([]);
    const shape = (root) => root === null ? null : [root, ...root.querySelectorAll("*")]
      .map((node) => `${node.tagName.toLowerCase()}.${[...node.classList].sort().join(".")}`).join(" ");
    const seen = { shapes: {}, marks: {}, voice: {}, labels: {}, names: {}, footButtons: {}, footLast: {}, stopLike: {}, composerParts: {}, withRoad: [] };
    for (const id of ids) {
      const sub = { id: `h-${id}`, name: "@one", state: "running", model: "model-s", requestedModel: "sonnet", tool_calls: 2 };
      const p = await open(at, sub, { agent: id, brief });
      // A catalog row that names a road to stop one helper (`helper_stop`) is the one
      // case where a stop button may stand; this page draws none, and says so below.
      if (installedAgents().find((row) => row.id === id)?.helper_stop) seen.withRoad.push(id);
      seen.shapes[id] = ["head", "card", "foot"].map((key) => shape(p[key])).join(" | ");
      const mark = p.head?.querySelector(".worker-mark") ?? null;
      seen.marks[id] = mark?.textContent ?? null;
      seen.labels[id] = mark?.getAttribute("aria-label") ?? null;
      seen.voice[id] = agentVoice(id).glyph;
      seen.names[id] = agentName(id);
      seen.footButtons[id] = p.foot?.querySelectorAll("button").length ?? null;
      seen.footLast[id] = [...(p.foot?.querySelectorAll(".helper-foot-actions > button") ?? [])].at(-1)?.className ?? null;
      seen.stopLike[id] = [...(p.host?.querySelectorAll(".helper-head button, .helper-sibs button, .helper-brief button, .helper-foot button") ?? [])]
        .map((one) => `${words(one)} ${one.getAttribute("aria-label") ?? ""}`)
        .filter((text) => /멈추|중지|중단|stop|halt|kill|cancel|停止|中止|detener/i.test(text)).length;
      seen.composerParts[id] = p.host?.querySelectorAll(".worker-composer, .chat-dock, textarea").length ?? null;
    }
    await cleanup();
    return seen;
  }, {
    ids: [...catalogIds, "not-in-catalog"],
    rows: catalogIds.map((id, at) => ({
      id, name: catalogNames.length === catalogIds.length ? catalogNames[at] : id, installed: true,
      ...(catalogRoads[id] && { helper_stop: catalogRoads[id] }),
    })),
    brief: HELPER_BRIEFS.stated,
  });
  const roadIds = catalogIds.filter((id) => catalogRoads[id]);
  // The stop and its read-out are the one difference a road makes to the page.
  const withoutStop = (one) => one.split(" ").filter((node) => !/^(?:button\.helper-foot-stop|span\.helper-foot-told\.)/.test(node)).join(" ");
  const roadlessShapes = new Set(Object.entries(everyAgent.shapes).filter(([id]) => !roadIds.includes(id)).map(([, one]) => one));
  const roadShapes = new Set(Object.entries(everyAgent.shapes).filter(([id]) => roadIds.includes(id)).map(([, one]) => one));
  const wornOff = Object.keys(everyAgent.marks).filter((id) =>
    everyAgent.marks[id] !== everyAgent.voice[id] || everyAgent.labels[id] !== everyAgent.names[id]);
  ok(
    "the head, the card and the footer of a helper's page are one and the same for every agent in the catalog, element for element, with only the mark and its name read from the catalog and the stop standing where the row names a road",
    catalogIds.length >= 30 &&
      Object.keys(everyAgent.shapes).length === catalogIds.length + 1 &&
      roadlessShapes.size === 1 && roadShapes.size === (roadIds.length > 0 ? 1 : 0) &&
      [...roadShapes].every((one) => withoutStop(one) === [...roadlessShapes][0] && one !== [...roadlessShapes][0]) &&
      [...roadlessShapes][0].includes("helper-head") && [...roadlessShapes][0].includes("helper-brief") &&
      [...roadlessShapes][0].includes("helper-foot") &&
      wornOff.length === 0,
    JSON.stringify({ agents: Object.keys(everyAgent.shapes).length, roadless: roadlessShapes.size, road: roadShapes.size, wornOff, first: [...roadlessShapes][0]?.slice(0, 240) }),
  );
  ok(
    "walking every catalog row: a row that names a road to stop one helper has two buttons in its helper's footer, the second the stop; every other row, and an id the catalog does not know, has the one speaking button, and nothing else on the page reads as a stop or wears a composer",
    voiced.length >= 1 && roadIds.length >= 1 &&
      JSON.stringify([...everyAgent.withRoad].sort()) === JSON.stringify([...roadIds].sort()) &&
      Object.keys(everyAgent.footButtons).length === catalogIds.length + 1 &&
      Object.entries(everyAgent.footButtons).every(([id, count]) => count === (roadIds.includes(id) ? 2 : 1)) &&
      Object.entries(everyAgent.footLast).every(([id, last]) => last === (roadIds.includes(id) ? "helper-foot-stop" : "helper-foot-speak")) &&
      Object.entries(everyAgent.stopLike).every(([id, count]) => count === (roadIds.includes(id) ? 1 : 0)) &&
      Object.values(everyAgent.composerParts).every((count) => count === 0),
    JSON.stringify({ roads: catalogRoads, withRoad: everyAgent.withRoad, footButtons: everyAgent.footButtons, stopLike: everyAgent.stopLike }).slice(0, 900),
  );
  const bodyOf = (name) => {
    const start = shellSource.search(new RegExp(`^(?:async )?function ${name}\\(`, "m"));
    return start < 0 ? "" : shellSource.slice(start, shellSource.indexOf("\n}\n", start) + 3);
  };
  const literalsOf = (body) => [...body.matchAll(/"((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'|`((?:[^`\\]|\\.)*)`/g)]
    .map((one) => one[1] ?? one[2] ?? one[3] ?? "");
  const namesAnAgent = (literal) => [...catalogIds, ...catalogNames].some((word) =>
    literal.toLowerCase() === word.toLowerCase() ||
    (word.length >= 4 && new RegExp(`\\b${word.replace(/[^\w]/g, "\\$&")}\\b`, "i").test(literal)));
  const drawn = [...shellSource.matchAll(/^(?:async )?function ((?:helperPage|updateHelperPage|paintHelperPage|syncHelperPage|helperBrief|helperStop|stopHelperFromPage)\w*)\(/gm)]
    .map((one) => one[1]);
  const offending = drawn.flatMap((name) => literalsOf(bodyOf(name)).filter(namesAnAgent).map((lit) => `${name}: ${lit}`));
  // The method a row names is the core's to read: no string in the window spells one.
  const roadsSpelled = literalsOf(shellSource).filter((lit) => Object.values(catalogRoads).includes(lit));
  const styleOffending = [...styleSource.matchAll(/^[^{}\n]*\.helper-(?:head|crumb|sibs?|brief|foot|model|state)[^{}\n]*\[data-agent[^\n]*$/gm)]
    .map((one) => one[0]);
  ok(
    "no agent's name is written in the code that draws a helper's head, strip, card, footer and stop: no string in those functions and no rule in their styles names one of the catalog's agents, and no string in the window spells a row's stop method",
    drawn.length >= 10 && drawn.every((name) => bodyOf(name).length > 0) &&
      ["syncHelperPageStop", "paintHelperPageStop", "stopHelperFromPage", "helperStopWords"].every((name) => drawn.includes(name)) &&
      offending.length === 0 && styleOffending.length === 0 && roadsSpelled.length === 0,
    JSON.stringify({ drawn, offending, styleOffending, roadsSpelled }),
  );

  /* ---- C5: 한 장면에서 자판, 움직임, 대비, 읽어 주기, 다섯 말을 본다 ---- */
  const localeBrief = `${HELPER_BRIEFS.runOn}. Do not modify any files.`;
  await page.evaluate(async ({ briefs, brief }) => {
    const { settle, words, parts, scene, open, useAgents } = window.__HP__;
    // The row these helpers ran on names a road, so the footer holds the stop too.
    await useAgents([{ id: "agent-with-road", name: "Agent R", installed: true, helper_stop: "road.to.one.helper" }]);
    window.__STOPPED__ = [];
    window.__ANSWER__.stop_pane_helper = (args) => {
      window.__STOPPED__.push(args.agentId);
      return { status: "not_found", agent_id: args.agentId, record_key: null };
    };
    const roster = [
      { id: "h-cut", name: "@cutter", state: "running", model: "model-s", requestedModel: "sonnet", effort: "high", tool_calls: 4 },
      { id: "h-blank", name: "@blank", state: "running" },
      { id: "h-b", name: "@mapper", state: "running", model: "model-s" },
      { id: "h-c", name: "@checker", state: "done", model: "model-s" },
      { id: "h-d", name: "@linter", state: "done", model: "model-s" },
    ];
    const at = await scene(roster);
    await open(at, roster[1], { brief: briefs.short, agent: "agent-with-road" });
    await open(at, roster[0], { brief, agent: "agent-with-road" });
    const here = `helper:${at.term}:h-cut`;
    window.__HPS__ = { at, here, blank: `helper:${at.term}:h-blank`, prior: theme };
    const controls = [
      () => parts().crumb?.querySelector("button") ?? null,
      () => parts().head?.querySelector(".worker-focus") ?? null,
      () => [...(parts().sibs?.querySelectorAll("button.helper-sib") ?? [])].find((one) => words(one).startsWith("@mapper")) ?? null,
      () => parts().sibs?.querySelector(".helper-sib-fold") ?? null,
      () => parts().card?.querySelector(".helper-brief-more") ?? null,
      () => parts().foot?.querySelector(".helper-foot-speak") ?? null,
      () => parts().foot?.querySelector(".helper-foot-stop") ?? null,
    ];
    const where = () => activeTabId === at.owner.id ? "parent"
      : activeTabId === here ? "helper"
      : activeTabId === `helper:${at.term}:h-b` ? "sibling" : activeTabId;
    const effects = [
      where,
      () => String(focusViewOn()),
      where,
      () => parts().sibs?.querySelector(".helper-sib-fold")?.getAttribute("aria-expanded") ?? null,
      () => parts().card?.querySelector(".helper-brief-more")?.getAttribute("aria-expanded") ?? null,
      where,
      () => String(window.__STOPPED__.length),
    ];
    window.__HPK__ = {
      // 페이지를 제자리로 — 이 도우미의 페이지, 접힌 띠, 접힌 카드, 꺼진 집중 보기 — 놓고 그 조종에 초점을 준다.
      async prepare(index) {
        setConversationFocusView(false);
        // A fresh stop button: the page stands anew when it is left and come back to.
        if (index === 6) {
          setActiveTab(window.__HPS__.blank);
          await settle(80);
        }
        setActiveTab(here);
        await settle(80);
        for (const selector of [".helper-sib-fold", ".helper-brief-more"]) {
          const node = parts().host?.querySelector(selector);
          if (node?.getAttribute("aria-expanded") === "true") node.click();
        }
        await settle(60);
        controls[index]()?.focus();
        return controls[index]() !== null && document.activeElement === controls[index]();
      },
      read: (index) => effects[index](),
      which: () => controls.findIndex((pick) => pick() !== null && pick() === document.activeElement),
      focusedWords: () => words(document.activeElement).slice(0, 60),
      ring: () => {
        const style = getComputedStyle(document.activeElement);
        return { outline: `${style.outlineStyle} ${style.outlineWidth}`, shadow: style.boxShadow };
      },
    };
  }, { briefs: HELPER_BRIEFS, brief: localeBrief });

  // 자판: 첫 조종에서 Tab으로 걸으면 여섯이 읽는 차례로 온다.
  const reached = [];
  const rings = [];
  const tabbedTo = [];
  await page.evaluate(() => window.__HPK__.prepare(0));
  for (let step = 0; step < 90 && reached.length < 7; step += 1) {
    const here = await page.evaluate(() => ({
      which: window.__HPK__.which(), words: window.__HPK__.focusedWords(), ring: window.__HPK__.ring(),
    }));
    tabbedTo.push(here.words);
    if (here.which >= 0 && reached.at(-1) !== here.which) {
      reached.push(here.which);
      rings.push(here.ring);
    }
    await page.keyboard.press("Tab");
  }
  await page.evaluate(() => window.__HPK__.prepare(1));
  await page.keyboard.press("Shift+Tab");
  const shiftBack = await page.evaluate(() => window.__HPK__.which());
  // 누름: 조종마다 Enter로 한 번, Space로 한 번 — 둘 다 같은 일을 한다.
  const pressed = [];
  for (let index = 0; index < 7; index += 1) {
    for (const key of ["Enter", "Space"]) {
      const focused = await page.evaluate((one) => window.__HPK__.prepare(one), index);
      const before = await page.evaluate((one) => window.__HPK__.read(one), index);
      await page.keyboard.press(key);
      await page.waitForTimeout(200);
      const after = await page.evaluate((one) => window.__HPK__.read(one), index);
      pressed.push({ index, key, focused, before, after });
    }
  }
  // The stop's effect is one request per press (its count of requests, read after).
  const effectOf = ["parent", "true", "sibling", "true", "true", "parent", null];
  const visibleRing = (ring) =>
    (ring.outline.split(" ")[0] !== "none" && parseFloat(ring.outline.split(" ")[1]) > 0) || ring.shadow !== "none";
  ok(
    "every new control is a real button that Tab reaches in reading order and that Enter and Space both press to the same effect: back to the parent, the focus toggle, a sibling, the folded count, the whole instruction, speaking to the parent, and — after it — the stop, one request per press",
    JSON.stringify(reached) === "[0,1,2,3,4,5,6]" &&
      shiftBack === 0 &&
      pressed.length === 14 &&
      pressed.every((one) => one.focused === true && one.before !== one.after &&
        (effectOf[one.index] === null ? Number(one.after) === Number(one.before) + 1 : one.after === effectOf[one.index])),
    JSON.stringify({ reached, shiftBack, pressed }),
  );
  ok(
    "the helpers folded into the count are out of the tab order until the count is opened, and every control wears a visible focus ring when the keyboard reaches it",
    reached.length === 7 &&
      !tabbedTo.some((words) => /@checker|@linter/.test(words)) &&
      rings.length === 7 && rings.every(visibleRing),
    JSON.stringify({ tabbedTo, rings }),
  );

  // 움직임: 점의 맥박과 조종들의 부드러움은 움직임 줄이기에서 멈춘다.
  const motion = {};
  for (const mode of ["no-preference", "reduce"]) {
    await page.emulateMedia({ reducedMotion: mode });
    motion[mode] = await page.evaluate(async () => {
      const { settle, parts } = window.__HP__;
      setActiveTab(window.__HPS__.here);
      await settle(80);
      const p = parts();
      const seconds = (node) =>
        node ? getComputedStyle(node).transitionDuration.split(",").map((one) => parseFloat(one)) : null;
      const dot = p.head?.querySelector(".helper-state-dot") ?? null;
      return {
        dot: dot ? getComputedStyle(dot).animationName : null,
        crumb: seconds(p.crumb?.querySelector("button")),
        sib: seconds(p.sibs?.querySelector("button.helper-sib")),
        fold: seconds(p.sibs?.querySelector(".helper-sib-fold")),
        more: seconds(p.card?.querySelector(".helper-brief-more")),
        speak: seconds(p.foot?.querySelector(".helper-foot-speak")),
        stop: seconds(p.foot?.querySelector(".helper-foot-stop")),
      };
    });
  }
  await page.emulateMedia({ reducedMotion: WINDOW_MOTION_REST });
  const easing = (state) => [state.crumb, state.sib, state.fold, state.more, state.speak, state.stop];
  ok(
    "the state dot's pulse and the controls' easing move when motion is allowed and stop under reduced motion",
    motion["no-preference"].dot !== null && motion["no-preference"].dot !== "none" &&
      easing(motion["no-preference"]).every((one) => one !== null && Math.max(...one) > 0) &&
      motion["reduce"].dot === "none" &&
      easing(motion["reduce"]).every((one) => one !== null && Math.max(...one) === 0),
    JSON.stringify(motion),
  );

  // 대비: 새 머리, 띠, 카드, 푸터의 글은 두 처리 모두에서 4.5:1 이상이다.
  const required = [
    ".helper-crumb-back", ".helper-crumb-here", ".worker-name", ".helper-model-words", ".helper-model-asked",
    ".helper-model.is-unknown", ".worker-state", ".worker-elapsed", ".worker-uses", ".helper-sib.is-here",
    "button.helper-sib:not(.helper-sib-fold):not(.is-done)", ".helper-sib-fold", ".helper-sib.is-done",
    ".helper-brief-label", ".helper-brief-said", ".helper-brief-cut", ".helper-brief-tag", ".helper-brief-more",
    ".helper-brief-full", ".helper-foot-says", ".helper-foot-speak", ".helper-foot-stop",
  ];
  const contrastIn = async (treatment) => {
    await page.evaluate((code) => setTheme(code), treatment);
    await page.waitForTimeout(400);
    return page.evaluate(async (selectors) => {
      const { settle, shown, parts } = window.__HP__;
      const freeze = document.createElement("style");
      freeze.textContent = "*, *::before, *::after { transition: none !important; animation: none !important; }";
      document.head.append(freeze);
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      const rgba = (css) => {
        ctx.clearRect(0, 0, 1, 1);
        ctx.fillStyle = "#000000";
        ctx.fillStyle = css;
        ctx.fillRect(0, 0, 1, 1);
        const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
        return [r, g, b, a / 255];
      };
      const over = (top, under) => [0, 1, 2].map((i) => top[i] * top[3] + under[i] * (1 - top[3]));
      const light = (rgb) => {
        const [r, g, b] = rgb.map((one) => {
          const c = one / 255;
          return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const ratioOf = (node) => {
        const chain = [];
        for (let at = node; at; at = at.parentElement) chain.unshift(at);
        let ground = [255, 255, 255];
        let opacity = 1;
        for (const one of chain) {
          const style = getComputedStyle(one);
          const back = rgba(style.backgroundColor);
          if (back[3] > 0) ground = over(back, ground);
          opacity *= parseFloat(style.opacity);
        }
        const ink = rgba(getComputedStyle(node).color);
        const seen = over([ink[0], ink[1], ink[2], ink[3] * opacity], ground);
        const a = light(seen);
        const b = light(ground);
        return Math.round(((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)) * 100) / 100;
      };
      const found = {};
      const measure = () => {
        for (const selector of selectors) {
          for (const node of parts().host?.querySelectorAll(selector) ?? []) {
            if (!shown(node)) continue;
            found[selector] = Math.min(found[selector] ?? 99, ratioOf(node));
          }
        }
      };
      const { here, blank } = window.__HPS__;
      setActiveTab(here);
      await settle(80);
      const fold = parts().sibs?.querySelector(".helper-sib-fold");
      if (fold?.getAttribute("aria-expanded") !== "true") fold?.click();
      await settle(60);
      measure();
      const more = parts().card?.querySelector(".helper-brief-more");
      if (more?.getAttribute("aria-expanded") !== "true") more?.click();
      await settle(60);
      measure();
      more?.click();
      fold?.click();
      setActiveTab(blank);
      await settle(80);
      measure();
      freeze.remove();
      return { theme: document.documentElement.dataset.theme ?? null, found };
    }, required);
  };
  const dark = await contrastIn("dark");
  const lightSide = await contrastIn("light");
  await page.evaluate(() => setTheme(window.__HPS__.prior));
  const unread = (side) => required.filter((one) => !(one in side.found) || side.found[one] < 4.5);
  ok(
    "every word the new head, strip, card and footer draw reads at 4.5:1 or better against what it stands on, in the dark treatment and in the light one",
    (dark.theme ?? "dark") === "dark" && lightSide.theme === "light" &&
      unread(dark).length === 0 && unread(lightSide).length === 0,
    JSON.stringify({ dark: unread(dark).map((one) => [one, dark.found[one] ?? null]), light: unread(lightSide).map((one) => [one, lightSide.found[one] ?? null]) }),
  );

  // 읽어 주기: 이름 있는 조종, 이름 있는 영역, 꾸밈은 숨김, 상태는 공손히 알린다.
  const reading = await page.evaluate(async () => {
    const { settle, words, parts } = window.__HP__;
    setActiveTab(window.__HPS__.here);
    await settle(80);
    const p = parts();
    const nameOf = (node) => (node.getAttribute("aria-label") ?? words(node)).trim();
    const buttons = [...(p.host?.querySelectorAll(".helper-head button, .helper-sibs button, .helper-brief button, .helper-foot button") ?? [])];
    const mark = p.head?.querySelector(".worker-mark") ?? null;
    const label = (node) => ({ tag: node?.tagName ?? null, label: node?.getAttribute("aria-label") ?? "" });
    return {
      buttons: buttons.length,
      unnamed: buttons.filter((node) => nameOf(node).length === 0).length,
      allPlain: buttons.every((node) => node.tagName === "BUTTON" && node.type === "button"),
      crumb: label(p.crumb),
      sibs: label(p.sibs),
      card: label(p.card),
      full: label(p.card?.querySelector(".helper-brief-full") ?? null),
      hidden: {
        sep: p.crumb?.querySelector(".helper-crumb-sep")?.getAttribute("aria-hidden") ?? null,
        dot: p.head?.querySelector(".helper-state-dot")?.getAttribute("aria-hidden") ?? null,
        cut: p.card?.querySelector(".helper-brief-cut")?.getAttribute("aria-hidden") ?? null,
      },
      state: p.head?.querySelector(".worker-state")?.getAttribute("role") ?? null,
      mark: { role: mark?.getAttribute("role") ?? null, label: mark?.getAttribute("aria-label") ?? "" },
      current: p.sibs?.querySelector('[aria-current="true"]')?.tagName ?? null,
    };
  });
  ok(
    "a screen reader gets every control by name, the crumb, the strip and the card as named regions, the card's whole text as a region under a name of its own, the decorative marks hidden, the current helper marked, and the helper's state announced politely",
    reading.buttons >= 6 && reading.unnamed === 0 && reading.allPlain === true &&
      reading.crumb.tag === "NAV" && reading.crumb.label.length > 0 &&
      reading.sibs.tag === "NAV" && reading.sibs.label.length > 0 &&
      reading.card.tag === "SECTION" && reading.card.label.length > 0 &&
      reading.full.tag === "DIV" && reading.full.label.length > 0 && reading.full.label !== reading.card.label &&
      reading.hidden.sep === "true" && reading.hidden.dot === "true" && reading.hidden.cut === "true" &&
      reading.state === "status" &&
      reading.mark.role === "img" && reading.mark.label.length > 0 &&
      reading.current !== null,
    JSON.stringify(reading),
  );

  // 다섯 말: 새 말은 다섯 언어로, 그 언어의 글자로, 부모의 이름과 글자 수가 제자리에 들어간다.
  const spoken = await page.evaluate(async () => {
    const { settle, words, parts } = window.__HP__;
    const { at, here } = window.__HPS__;
    const wore = locale;
    const seen = { owned: {}, orphan: {} };
    const q = (node, selector) => node?.querySelector(selector) ?? null;
    const gather = () => {
      const p = parts();
      return {
        parent: tabLabel(at.owner),
        crumbLabel: p.crumb?.getAttribute("aria-label") ?? "",
        crumbHere: words(q(p.crumb, ".helper-crumb-here")),
        back: q(p.crumb, "button")?.getAttribute("aria-label") ?? "",
        sibsLabel: p.sibs?.getAttribute("aria-label") ?? "",
        fold: words(q(p.sibs, ".helper-sib-fold")),
        briefLabel: words(q(p.card, ".helper-brief-label")),
        tagsLabel: q(p.card, ".helper-brief-tags")?.getAttribute("aria-label") ?? "",
        full: q(p.card, ".helper-brief-full")?.getAttribute("aria-label") ?? "",
        more: words(q(p.card, ".helper-brief-more")),
        cutSaid: words(q(p.card, ".helper-brief-text .helper-sr")),
        asked: words(q(p.head, ".helper-model-asked")),
        says: words(q(p.foot, ".helper-foot-says")),
        speak: words(q(p.foot, ".helper-foot-speak")),
        stop: words(q(p.foot, ".helper-foot-stop")),
        // The word a press shows while the answer is out, read off the button itself.
        stopping: (() => {
          const stop = q(p.foot, ".helper-foot-stop");
          stop?.click();
          return words(stop);
        })(),
        stopSaid: ["stopped", "closed", "already_finished", "not_found", "not_owned", "unreachable", "failed", "stopped_by_person"]
          .map((kind) => helperStopWords(kind)),
      };
    };
    const repaint = async (code) => {
      setLocale(code, { refresh: false, persist: false });
      paintWorkerView(tabs.find((one) => one.id === here));
      await settle(80);
    };
    // The stop pressed above may still be saying its answer: a page stood anew reads its own words.
    setActiveTab(window.__HPS__.blank);
    await settle(80);
    setActiveTab(here);
    await settle(80);
    for (const code of ["ko", "en", "ja", "zh", "es"]) {
      await repaint(code);
      seen.owned[code] = gather();
    }
    dropTab(at.owner.id);
    setActiveTab(here);
    await settle(80);
    for (const code of ["ko", "en", "ja", "zh", "es"]) {
      await repaint(code);
      seen.orphan[code] = words(q(parts().foot, ".helper-foot-says"));
    }
    setLocale(wore, { refresh: false, persist: false });
    return seen;
  });
  const codes = ["ko", "en", "ja", "zh", "es"];
  const script = {
    hangul: /[\u3131-\u318E\uAC00-\uD7A3]/,
    kana: /[\u3040-\u30FF]/,
    han: /[\u3400-\u9FFF]/,
  };
  const inItsScript = {
    ko: (one) => script.hangul.test(one),
    en: (one) => /[A-Za-z]/.test(one) && !script.hangul.test(one) && !script.kana.test(one) && !script.han.test(one),
    ja: (one) => (script.kana.test(one) || script.han.test(one)) && !script.hangul.test(one),
    zh: (one) => script.han.test(one) && !script.hangul.test(one) && !script.kana.test(one),
    es: (one) => /[A-Za-z]/.test(one) && !script.hangul.test(one) && !script.kana.test(one) && !script.han.test(one),
  };
  const keys = ["crumbLabel", "crumbHere", "back", "sibsLabel", "fold", "briefLabel", "tagsLabel", "full", "more", "cutSaid", "asked", "says", "speak", "stop", "stopping"];
  // The stop's answers: eight words in each language, none alike within one, each its language's own.
  const stopSaidStray = codes.flatMap((code) => spoken.owned[code].stopSaid
    .filter((one) => one.length === 0 || !inItsScript[code](one) || /\{\{|\}\}|helper\.[a-z]/.test(one))
    .map((one) => `${code}=${one}`));
  const stopSaidAlike = codes.filter((code) => new Set(spoken.owned[code].stopSaid).size !== spoken.owned[code].stopSaid.length);
  const stopSaidShared = spoken.owned.ko.stopSaid.map((_, at) => at)
    .filter((at) => new Set(codes.map((code) => spoken.owned[code].stopSaid[at])).size !== codes.length);
  const stray = [];
  const alike = [];
  for (const key of keys) {
    if (new Set(codes.map((code) => spoken.owned[code][key])).size !== codes.length) alike.push(key);
    for (const code of codes) {
      const one = spoken.owned[code][key];
      if (one.length === 0 || !inItsScript[code](one) || /\{\{|\}\}|helper\.[a-z]/.test(one)) stray.push(`${code}.${key}=${one}`);
    }
  }
  const orphanAlike = new Set(codes.map((code) => spoken.orphan[code])).size !== codes.length;
  const orphanStray = codes.filter((code) => !inItsScript[code](spoken.orphan[code]) || spoken.orphan[code] === spoken.owned[code].says);
  ok(
    "the helper page's new words come in all five languages, each in the letters of its language, with the parent's name, the character count and what was asked in place, and the English is plain",
    alike.length === 0 && stray.length === 0 && !orphanAlike && orphanStray.length === 0 &&
      stopSaidStray.length === 0 && stopSaidAlike.length === 0 && stopSaidShared.length === 0 &&
      codes.every((code) => spoken.owned[code].says.includes(spoken.owned[code].parent)) &&
      codes.every((code) => spoken.owned[code].back.includes(spoken.owned[code].parent)) &&
      codes.every((code) => spoken.owned[code].more.includes(String(localeBrief.length))) &&
      codes.every((code) => spoken.owned[code].asked.includes("sonnet")) &&
      keys.every((key) => !/breadcrumb|sibling|toggle|subagent/i.test(spoken.owned.en[key])),
    JSON.stringify({ alike, stray, orphanAlike, orphanStray, stopSaidStray, stopSaidAlike, stopSaidShared, en: spoken.owned.en, orphan: spoken.orphan }),
  );
  await page.evaluate(async () => {
    await window.__HP__.cleanup();
    delete window.__HPS__;
    delete window.__HPK__;
    delete window.__STOPPED__;
  });

  /* ---- 좁은 창: 머리와 띠와 카드와 푸터가 옆으로 밀리지 않고, 목록도 남는다 ---- */
  await page.setViewportSize({ width: 360, height: 780 });
  const narrow = await page.evaluate(async (brief) => {
    const { settle, shown, parts, scene, open, useAgents, cleanup } = window.__HP__;
    await useAgents([{ id: "agent-with-road", name: "Agent R", installed: true, helper_stop: "road.to.one.helper" }]);
    const long = "@a-helper-with-a-name-that-goes-on-and-on-and-on-past-any-width";
    const roster = [
      { id: "h-a", name: long, state: "running", model: "a-model-with-a-long-name-s", requestedModel: "another-long-model-name", effort: "high", tool_calls: 12 },
      { id: "h-b", name: "@mapper", state: "running" },
      { id: "h-c", name: `${long}-2`, state: "done" },
      { id: "h-d", name: "@linter", state: "done" },
    ];
    const at = await scene(roster);
    let p = await open(at, roster[0], { brief, agent: "agent-with-road" });
    Object.assign(p.host.style, { position: "fixed", inset: "0", zIndex: "100", width: "100vw", height: "100vh" });
    const seen = {};
    const longTitle = "Refactor the retry loop in the sync module and its backoff timing tests";
    const measure = (key) => {
      const q = parts();
      // The room to read is the plain page's; a parent title as long as the one
      // laid in below asks the footer for more lines, and that is measured for
      // overflow only.
      const room = Math.round(q.turns?.getBoundingClientRect().height ?? 0);
      // A long parent title, worded into the crumb and the footer's sentence: the
      // layout is what is measured, so the words are laid in by hand.
      const crumb = q.head?.querySelector(".helper-crumb-parent");
      if (crumb) crumb.textContent = longTitle;
      const says = q.foot?.querySelector(".helper-foot-says");
      if (says) says.textContent = `이 도우미는 “${longTitle}” 대화가 지시합니다.`;
      const view = q.host.getBoundingClientRect();
      const inside = (node) => {
        const box = node.getBoundingClientRect();
        return box.left >= view.left - 1 && box.right <= view.right + 1;
      };
      seen[key] = {
        noSideScroll: q.host.scrollWidth <= q.host.clientWidth + 1,
        blocksInside: [q.head, q.sibs, q.card, q.foot].every((node) => node !== null && inside(node)),
        chipsInside: [...q.host.querySelectorAll(".helper-sib, .helper-brief-tag, .helper-model, .helper-state, .helper-foot-speak, .helper-foot-stop, .helper-crumb-back")]
          .filter(shown).every(inside),
        wide: [...q.host.querySelectorAll("*")]
          .filter((node) => shown(node) && node.getBoundingClientRect().right > view.right + 1)
          .slice(0, 6)
          .map((node) => `${node.tagName.toLowerCase()}.${String(node.className).split(" ")[0]} +${Math.round(node.getBoundingClientRect().right - view.right)}px`),
        scrollWidth: q.host.scrollWidth,
        listHeight: room,
        speakShown: shown(q.foot?.querySelector(".helper-foot-speak")),
        stopShown: shown(q.foot?.querySelector(".helper-foot-stop")),
        buttonsInFoot: [...(q.foot?.querySelectorAll("button") ?? [])].every((node) => {
          const box = node.getBoundingClientRect();
          const foot = q.foot.getBoundingClientRect();
          return box.left >= foot.left - 1 && box.right <= foot.right + 1;
        }),
        saysShown: shown(q.foot?.querySelector(".helper-foot-says")),
      };
    };
    await settle(120);
    measure("shut");
    p.sibs?.querySelector(".helper-sib-fold")?.click();
    p.card?.querySelector(".helper-brief-more")?.click();
    await settle(120);
    measure("open");
    await cleanup();
    return seen;
  }, HELPER_BRIEFS.stated);
  await page.setViewportSize({ width: 1280, height: 860 });
  ok(
    "at 360px wide the page does not scroll sideways with a long name, a long model, the strip and the card open, its footer's sentence and both its buttons show inside it, and the list of turns keeps room to read",
    ["shut", "open"].every((key) =>
      narrow[key].noSideScroll && narrow[key].blocksInside && narrow[key].chipsInside &&
      narrow[key].speakShown && narrow[key].stopShown && narrow[key].buttonsInFoot && narrow[key].saysShown) &&
      narrow.shut.listHeight >= 200 && narrow.open.listHeight >= 100,
    JSON.stringify(narrow),
  );

  /* ---- 나머지 페이지는 그대로다: 판의 대화는 제 입력줄과 제 머리를 지키고, 첫 말도 여전히 보인다 ---- */
  const paneKeeps = await page.evaluate(async () => {
    const { settle, shown, scene, cleanup } = window.__HP__;
    const at = await scene([]);
    window.__ANSWER__.pane_log = () => ({
      found: true, next: 2, skipped: false, more: false, folded: false, model: "model-o",
      turns: [{ role: "user", text: "Please look at the sync bug." }, { role: "assistant", text: "On it." }],
    });
    await setPaneChat(at.term, true);
    setActiveTab(at.owner.id);
    await settle(250);
    const host = document.querySelector(`.pane-slot[data-term="${at.term}"] .pane-chat`);
    const seen = {
      hostFound: host !== null,
      composer: host?.querySelector(".worker-composer-box") != null,
      oldHead: host?.querySelector(".worker-head") != null,
      newChrome: host?.querySelector(".helper-crumb, .helper-sibs, .helper-brief, .helper-foot") != null,
      firstUserShown: shown(host?.querySelector(".helper-turn.is-user")),
    };
    await cleanup();
    return seen;
  });
  ok(
    "a pane's own conversation keeps its composer and its plain head and still shows its first message, so the helper page's crumb, strip, card and footer stand on helper pages alone",
    paneKeeps.hostFound === true && paneKeeps.composer === true && paneKeeps.oldHead === true &&
      paneKeeps.newChrome === false && paneKeeps.firstUserShown === true,
    JSON.stringify(paneKeeps),
  );
}
