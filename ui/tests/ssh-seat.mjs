import { openWindowTestPage } from "./window-boot.mjs";

/* 에이전트가 연 터미널 탭은 요청한 판의 프로젝트에 앉는다 (t-66346).
 *
 * 10-20 15:58, 한 프로젝트(여기서는 「프로젝트 B」)의 에이전트가 제 판에서
 * `zerocode-ssh open`으로 SSH 터미널을 열었는데, 새 탭이 사람이 보던 다른
 * 프로젝트의 무대에 서고 화면까지 그쪽으로 넘어갔다. 문이 부른 판을 싣지
 * 않았고, 창은 「지금 초점 그룹」에 세웠다. 이제 문이 판 키를 싣고 이벤트가
 * 그 판의 term(`from`)을 들고 오므로, 창은
 *   가. 부른 판이 다른 체크아웃이면 — 그 판의 체크아웃·그룹에 탭으로만 세우고
 *      활성 체크아웃·활성 탭·초점 그룹·스테이지 나눔을 그대로 둔다;
 *   나. 부른 판이 사람이 보는 체크아웃이면 — 지금처럼 그 판 곁(그 판의 그룹)에
 *      세우고 키보드를 준다;
 *   다. 부른 판을 모르면 — 현재 그룹에 비활성 탭으로 추가하고 한 줄 말한다;
 *   라. 로컬 셸(`open --local`)도 가.와 같다.
 * 사람이 그 프로젝트로 돌아가면(C) 탭은 그 무대의 그 그룹에, 셸은 그 프로젝트
 * 줄에 선다. 그리고 창은 어디에 세웠는지(체크아웃·그룹)를 백엔드에 말해
 * 에이전트의 답(`seatedIn`)에 실리게 한다(D).
 *
 * 전부 가짜 백엔드 안에서 잰다 — 접속도 셸도 없다. 경로는 모양일 뿐이다. */

const PROJECT_B = "/tmp/zerocode-window-test/project-b";

export async function testSshSeat(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    // 창이 프로젝트 목록을 한 번 읽은 뒤에 시작한다 — 그 목록이 아래 장면의 바탕이다.
    await page.waitForFunction(() => projectsRead);
    // 프로젝트 B를 카탈로그에 더한다 — 사이드바가 그 줄을 그리고, 사람이 그리로
    // 갈 수 있게. 창이 알리는 자리는 받아 적는다.
    const home = await page.evaluate(async (projectB) => {
      const base = JSON.parse(JSON.stringify(projects));
      const b = {
        ...JSON.parse(JSON.stringify(base[0])),
        name: "project-b",
        path: projectB,
        slug: null,
        worktrees: [{
          path: projectB,
          branch: "main",
          is_main: true,
          active: false,
          is_folder: false,
          ownership: "zerocode-managed",
          external_hidden: false,
        }],
      };
      const catalog = [...base.filter((one) => one.path !== projectB), b];
      window.__ANSWER__.project_catalog = () => catalog.map((project) => ({
        ...project,
        worktrees: project.worktrees.map((one) => ({ ...one, active: one.path === activeWorktreePath })),
      }));
      window.__ANSWER__.pane_layouts = () => [];
      window.__ANSWER__.save_pane_layouts = () => null;
      window.__SEATS__ = [];
      window.__ANSWER__.ssh_agent_seated = (args) => {
        window.__SEATS__.push(JSON.parse(JSON.stringify(args ?? {})));
        return true;
      };
      await refreshWorktrees();
      return activeWorktreePath;
    }, PROJECT_B);
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    const clear = () => page.evaluate(() => {
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const term of [...termViews.keys()]) dropTermView(term);
      for (const group of stageGroups()) collapseStageGroup(group);
      for (const note of document.querySelectorAll(".toasts .toast")) note.remove();
      window.__SEATS__ = [];
      renderTabs();
      updateStage();
    });
    const fire = (payload) => page.evaluate((said) => {
      for (const handler of window.__LISTENERS__["ssh:agent-open"] ?? []) handler({ payload: said });
    }, payload);
    const unseatedWords = await page.evaluate(() => t(
      "ssh.agentOpenUnseated",
      "에이전트가 터미널을 열었지만 요청한 터미널을 알 수 없어, 현재 그룹에 비활성 탭으로 추가했습니다.",
    ));
    const toasts = () => page.evaluate(() =>
      [...document.querySelectorAll(".toasts .toast")].map((one) => one.textContent.trim()));
    /* 프로젝트 B의 워커 — 원장이 사람의 터미널 곁에서 B의 체크아웃에 앉힌 판. */
    const workerInB = (term) => page.evaluate(async ({ projectB, term }) => {
      const leader = await openTermTab({ placement: "tab" });
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: leader, term, worktree: projectB, agent: "codex", resumed: "fresh" } });
      }
      return leader;
    }, { projectB: PROJECT_B, term });
    const screen = () => page.evaluate(() => ({
      worktree: activeWorktreePath,
      tab: activeTabId,
      focused: focusedPane,
      tree: JSON.stringify(stageTree()),
    }));
    /* 연 탭이 어디에 섰고 사람의 화면이 어떻게 됐는가. */
    const seatOf = (term, caller, was) => page.evaluate(({ term, caller, was }) => {
      const tab = tabOfTerm(term);
      const asked = caller === null ? null : tabOfTerm(caller);
      return {
        seatedIn: tab?.worktree ?? null,
        group: tab?.pane ?? null,
        callerIn: asked?.worktree ?? null,
        callerGroup: asked?.pane ?? null,
        worktreeKept: activeWorktreePath === was.worktree,
        tabKept: activeTabId === was.tab,
        focusKept: focusedPane === was.focused,
        stageKept: JSON.stringify(stageTree()) === was.tree,
        took: tab ? activeTabId === tab.id : null,
        focusedGroup: focusedPane,
        onStrip: tab ? Boolean(document.querySelector(`[data-tab="${CSS.escape(tab.id)}"]`)) : null,
        // 장면의 수: 사람이 보던 프로젝트의 무대에 선, 에이전트가 연 터미널 탭.
        onPersonsStage: tabs.filter((one) => one.kind === "term" && one.worktree === was.worktree
          && paneLeaves(one.layout).includes(term)).length,
        said: window.__SEATS__.filter((one) => one.term === term),
      };
    }, { term, caller, was });

    /* ---- 가. 프로젝트 B의 판이 SSH 터미널을 연다 --------------------------- */
    await clear();
    const remoteCaller = 16346;
    await workerInB(remoteCaller);
    await settle();
    const beforeRemote = await screen();
    await fire({ term: 16401, label: "build-box", remoteHost: "host-1", from: remoteCaller });
    await settle();
    const remote = await seatOf(16401, remoteCaller, beforeRemote);
    ok(
      "가. an agent's SSH tab is seated in the asking pane's checkout and group, and the person's screen stays",
      remote.seatedIn === PROJECT_B && remote.callerIn === PROJECT_B && remote.group === remote.callerGroup
        && remote.worktreeKept && remote.tabKept && remote.focusKept && remote.stageKept
        && remote.onStrip === false,
      JSON.stringify(remote),
    );
    ok(
      "the stage the person was reading gets no terminal another project's agent opened (scene: 1 → 0)",
      remote.onPersonsStage === 0,
      JSON.stringify({ onPersonsStage: remote.onPersonsStage, seatedIn: remote.seatedIn }),
    );
    ok(
      "D. the window says where it seated the agent's terminal — the checkout and the group — for the agent's answer",
      remote.said.length === 1 && remote.said[0].worktree === PROJECT_B
        && remote.said[0].group === remote.callerGroup,
      JSON.stringify(remote.said),
    );

    /* ---- C. 사람이 프로젝트 B로 돌아가면 ---------------------------------- */
    const back = await page.evaluate(async ({ projectB, home, term, caller }) => {
      const before = {
        countedInB: worktreeTerms(projectB).includes(term),
        countedAtHome: worktreeTerms(home).includes(term),
        liveInB: worktreeHasShell(projectB),
      };
      await activateWorktree(projectB);
      await new Promise((done) => setTimeout(done, 160));
      const tab = tabOfTerm(term);
      const seen = {
        before,
        active: activeWorktreePath,
        onStrip: tab ? Boolean(document.querySelector(`[data-tab="${CSS.escape(tab.id)}"]`)) : false,
        inItsGroup: tab ? paneTabs(tab.pane).some((one) => one.id === tab.id) : false,
        groupOnStage: tab ? stageGroups().includes(tab.pane) : false,
        besideCaller: tab ? tab.pane === tabOfTerm(caller)?.pane : false,
        project: projectOfWorktree(tab?.worktree ?? null)?.name ?? null,
        countedInB: worktreeTerms(projectB).includes(term),
        countedAtHome: worktreeTerms(home).includes(term),
      };
      await activateWorktree(home);
      await new Promise((done) => setTimeout(done, 160));
      seen.backHome = activeWorktreePath === home;
      return seen;
    }, { projectB: PROJECT_B, home, term: 16401, caller: remoteCaller });
    ok(
      "C. back in project B the agent's tab stands on its stage beside the asking pane, and the shell is counted under B",
      back.before.countedInB && !back.before.countedAtHome && back.before.liveInB
        && back.active === PROJECT_B && back.onStrip && back.inItsGroup && back.groupOnStage
        && back.besideCaller && back.project === "project-b"
        && back.countedInB && !back.countedAtHome && back.backHome,
      JSON.stringify(back),
    );

    /* ---- 나. 사람이 보는 체크아웃의 판이 연다 ------------------------------- */
    await clear();
    const here = await page.evaluate(async () => {
      const leader = await openTermTab({ placement: "tab" });
      const leaderGroup = tabOfTerm(leader).pane;
      // 사람은 옆 그룹의 보드를 보고 있다 — 초점은 부른 판이 아니다.
      const beside = standBesideFocused();
      openTab({ id: "board", kind: "board", pane: beside });
      return { leader, leaderGroup, focused: focusedPane, worktree: activeWorktreePath };
    });
    await settle();
    const beforeHere = await screen();
    await fire({ term: 16402, label: "build-box", remoteHost: "host-1", from: here.leader });
    await settle();
    const sameCheckout = await seatOf(16402, here.leader, beforeHere);
    ok(
      "나. an SSH tab asked for from the checkout on screen stands beside the asking pane and takes the keyboard, as before",
      here.focused !== here.leaderGroup
        && sameCheckout.seatedIn === here.worktree && sameCheckout.group === here.leaderGroup
        && sameCheckout.took === true && sameCheckout.focusedGroup === here.leaderGroup
        && sameCheckout.worktreeKept && sameCheckout.onStrip === true
        && sameCheckout.said.length === 1 && sameCheckout.said[0].worktree === here.worktree
        && sameCheckout.said[0].group === here.leaderGroup,
      JSON.stringify({ here, sameCheckout }),
    );

    /* ---- 다. 부른 판을 모르면 비활성 탭 + 한 줄 ------------------------------ */
    await clear();
    await page.evaluate(() => openTermTab({ placement: "tab" }));
    await settle();
    const beforeNameless = await screen();
    await fire({ term: 16403, label: "build-box", remoteHost: "host-1" });
    await settle();
    const nameless = await seatOf(16403, null, beforeNameless);
    const namelessToasts = await toasts();
    await clear();
    await page.evaluate(() => openTermTab({ placement: "tab" }));
    await settle();
    const beforeStranger = await screen();
    await fire({ term: 16404, label: "build-box", remoteHost: "host-1", from: 987654 });
    await settle();
    const stranger = await seatOf(16404, null, beforeStranger);
    const strangerToasts = await toasts();
    ok(
      "다. an SSH tab whose asking pane is unknown is added inactive to the current group, and the window says why",
      nameless.seatedIn === beforeNameless.worktree && nameless.group === beforeNameless.focused
        && nameless.tabKept && nameless.focusKept && nameless.stageKept && nameless.took === false
        && namelessToasts.includes(unseatedWords)
        && stranger.seatedIn === beforeStranger.worktree && stranger.group === beforeStranger.focused
        && stranger.tabKept && stranger.focusKept && stranger.stageKept && stranger.took === false
        && strangerToasts.includes(unseatedWords),
      JSON.stringify({ nameless, namelessToasts, stranger, strangerToasts, unseatedWords }),
    );

    /* ---- 라. 로컬 셸(`open --local`)도 가.와 같다 ---------------------------- */
    await clear();
    const localCaller = 16347;
    await workerInB(localCaller);
    await settle();
    const beforeLocal = await screen();
    // 로컬 길의 페이로드: 호스트 이름이 없다.
    await fire({ term: 16405, label: null, remoteHost: null, from: localCaller });
    await settle();
    const local = await seatOf(16405, localCaller, beforeLocal);
    ok(
      "라. an agent's local shell is seated in the asking pane's checkout and group too, and the person's screen stays",
      local.seatedIn === PROJECT_B && local.group === local.callerGroup
        && local.worktreeKept && local.tabKept && local.focusKept && local.stageKept
        && local.onStrip === false && local.onPersonsStage === 0
        && local.said.length === 1 && local.said[0].worktree === PROJECT_B,
      JSON.stringify(local),
    );

    await clear();
    ok("the ssh seat page raised nothing", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
