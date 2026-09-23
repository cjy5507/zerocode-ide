import { openWindowTestPage } from "./window-boot.mjs";

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

    /* A helper has no pane of its own. Its navigator row is therefore a door to
     * the parent terminal where its work is running, even when the vendor also
     * left a transcript that can be opened through another surface. */
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
      seen.parentTab = activeTabId === owner.id;
      seen.parentPane = owner.activePane === term;
      seen.noHelperPage = !tabs.some((tab) => tab.id === `helper:${term}:kept`);
      seen.logAsked = logAsked;

      delete window.__ANSWER__.subagent_log;
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });

    ok(
      "a pane helper row focuses the parent terminal instead of opening a helper page",
      helperRowFocus.parentTab &&
        helperRowFocus.parentPane &&
        helperRowFocus.noHelperPage &&
        helperRowFocus.logAsked === 0,
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
        turns: [{ role: "assistant", text: "done here" }, { role: "tool", text: "read_file · /a.rs" }],
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
      // tool row stands plain under the CLI's own name.
      seen.noRunningTail = face?.querySelector(".is-live") === null &&
        face?.querySelector(".helper-status")?.hidden === true &&
        face?.querySelector(".helper-turn.is-tool .helper-tool-name")?.textContent === "read_file";
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
    // the restore asks it before guessing.
    await page.evaluate(() => {
      window.__ANSWER__.set_active_worktree = () => "wt/t-3";
      window.__ASKED_SEAT__ = [];
      window.__ANSWER__.worktree_last_agent = (args) => (
        window.__ASKED_SEAT__.push(args),
        { agent: "codex", sleeping: false }
      );
      window.__LAUNCHED__ = null;
    });
    const seatedBack = await page.evaluate(async () => {
      await activateWorktree("/tmp/zerocode-window-test/wt-t3");
      return { launched: window.__LAUNCHED__?.agent ?? null, asked: window.__ASKED_SEAT__ };
    });
    ok(
      "a workspace the ledger seated a Codex worker in comes back as Codex, not the default agent",
      seatedBack.launched === "codex" &&
        seatedBack.asked.length === 1,
      JSON.stringify(seatedBack),
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
