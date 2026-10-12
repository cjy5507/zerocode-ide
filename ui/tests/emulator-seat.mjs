import { openWindowTestPage } from "./window-boot.mjs";

/* 에이전트가 연 에뮬레이터 거울은 부른 판의 체크아웃에 선다 (t-6379).
 *
 * 09-23 14:14·14:34·14:49, 워커 워크트리에서 부른 `zerocode-emulator open`의
 * 거울 탭이 세 번 다 사람이 보던 다른 프로젝트(dl)의 스테이지에 섰다. 문이
 * 부른 판을 싣지 않았고, 창은 「지금 초점 판 옆」에 세웠다. 이제 문이 판
 * 키를 싣고 이벤트가 그 판의 term을 들고 오므로, 창은
 *   1. 사람이 다른 체크아웃을 보고 있으면 — 부른 판의 그룹에 탭으로만 세우고
 *      활성 체크아웃·활성 탭·초점 그룹·스테이지 나눔을 그대로 둔다;
 *   2. 사람이 그 체크아웃을 보고 있어도 — 부른 판의 그룹에 비활성 탭으로
 *      추가하고 활성 탭·초점·나눔을 유지한다;
 *   3. 팔레트에서 연 것은 지금처럼 초점 옆;
 *   4. 부른 판을 모르면 현재 그룹에 비활성 탭으로 추가하고 한 줄 말한다.
 * 그리고 다른 체크아웃에 선 탭은 사람이 보는 화면에 제 이름·자막을 쓰지 않고,
 * 이미 스트림이 있는 기기를 다시 열어도 화면을 넘기지 않는다.
 *
 * 전부 가짜 백엔드 안에서 잰다 — 기기도 스트림도 없다. */

const AWAY = "/tmp/zerocode-window-test/wt-emulator-seat";

export async function testEmulatorSeat(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      window.__SEAT__ = { starts: [], channels: [] };
      const fleet = [
        { udid: "U1", name: "iPhone 17", booted: true },
        { udid: "U2", name: "t-6379 probe", booted: false },
      ];
      window.__ANSWER__.mobile_emulators = () => fleet;
      window.__ANSWER__.android_emulators = () => [];
      window.__ANSWER__.start_emulator_stream = (args) => {
        window.__SEAT__.starts.push(args?.udid ?? null);
        window.__SEAT__.channels.push(args?.onFrame ?? null);
        const device = fleet.find((one) => one.udid === args?.udid) ?? fleet[0];
        const shared = window.__SEAT__.shared?.[device.udid];
        return shared
          ? { stream: shared, udid: device.udid, name: device.name, reused: true }
          : { stream: `S${window.__SEAT__.starts.length}`, udid: device.udid, name: device.name };
      };
      window.__ANSWER__.stop_emulator_stream = () => null;
    });
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    const clear = () => page.evaluate(() => {
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const term of [...termViews.keys()]) dropTermView(term);
      for (const group of stageGroups()) collapseStageGroup(group);
      for (const note of document.querySelectorAll(".toasts .toast")) note.remove();
      window.__SEAT__.shared = {};
      renderTabs();
      updateStage();
    });
    const fire = (payload) => page.evaluate((said) => {
      for (const handler of window.__LISTENERS__["emulator:agent-open"] ?? []) handler({ payload: said });
    }, payload);
    const unseatedWords = await page.evaluate(() => t(
      "emulator.agentOpenUnseated",
      "에이전트가 에뮬레이터를 열었지만 요청한 터미널을 알 수 없어, 현재 그룹에 비활성 탭으로 추가했습니다.",
    ));
    const toasts = () => page.evaluate(() =>
      [...document.querySelectorAll(".toasts .toast")].map((one) => one.textContent.trim()));

    /* ---- 1. 다른 체크아웃의 워커가 부른다 ------------------------------ */
    await clear();
    const standing = await page.evaluate(async (away) => {
      const leader = await openTermTab({ placement: "tab" });
      const worker = 13790;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: leader, term: worker, worktree: away, agent: "claude", resumed: "fresh" } });
      }
      return { leader, worker };
    }, AWAY);
    await settle();
    const before = await page.evaluate(() => ({
      worktree: activeWorktreePath,
      tab: activeTabId,
      focused: focusedPane,
      tree: JSON.stringify(stageTree()),
      starts: window.__SEAT__.starts.length,
    }));
    await fire({ platform: "ios", device: "U2", term: standing.worker });
    await settle();
    const away = await page.evaluate(({ worker, before: was }) => {
      const mirror = tabs.find((one) => one.kind === "emulator" && one.deviceId === "U2");
      const caller = tabOfTerm(worker);
      return {
        seatedIn: mirror?.worktree ?? null,
        group: mirror?.pane ?? null,
        callerIn: caller?.worktree ?? null,
        callerGroup: caller?.pane ?? null,
        worktreeKept: activeWorktreePath === was.worktree,
        tabKept: activeTabId === was.tab,
        focusKept: focusedPane === was.focused,
        stageKept: JSON.stringify(stageTree()) === was.tree,
        asked: window.__SEAT__.starts.length - was.starts,
        onStrip: mirror ? Boolean(document.querySelector(`[data-tab="${CSS.escape(mirror.id)}"]`)) : null,
      };
    }, { worker: standing.worker, before });
    ok(
      "an agent's mirror is seated in the asking pane's group in its own checkout, and the person's screen stays",
      away.seatedIn === AWAY && away.callerIn === AWAY && away.group === away.callerGroup
        && away.worktreeKept && away.tabKept && away.focusKept && away.stageKept
        && away.asked === 1 && away.onStrip === false,
      JSON.stringify(away),
    );

    /* ---- 2. 사람이 보고 있는 체크아웃의 판이 부른다 --------------------- */
    await clear();
    const home = await page.evaluate(async () => {
      const leader = await openTermTab({ placement: "tab" });
      const leaderGroup = tabOfTerm(leader).pane;
      // 사람은 옆 그룹의 보드를 보고 있다 — 초점은 부른 판이 아니다.
      const beside = standBesideFocused();
      openTab({ id: "board", kind: "board", pane: beside });
      return {
        leader, leaderGroup, focused: focusedPane, tab: activeTabId,
        tree: JSON.stringify(stageTree()), worktree: activeWorktreePath,
      };
    });
    await settle();
    await fire({ platform: "ios", device: "U1", term: home.leader });
    await settle();
    const here = await page.evaluate((was) => {
      const mirror = tabs.find((one) => one.kind === "emulator" && one.deviceId === "U1");
      return {
        seatedIn: mirror?.worktree ?? null,
        home: activeWorktreePath,
        group: mirror?.pane ?? null,
        tabKept: activeTabId === was.tab,
        focusKept: focusedPane === was.focused,
        stageKept: JSON.stringify(stageTree()) === was.tree,
        worktreeKept: activeWorktreePath === was.worktree,
      };
    }, home);
    ok(
      "an agent's mirror in the checkout on screen keeps the active tab, focus group and split layout",
      here.seatedIn === here.home && here.group === home.leaderGroup
        && home.focused !== home.leaderGroup && here.tabKept && here.focusKept
        && here.stageKept && here.worktreeKept,
      JSON.stringify({ home, here }),
    );

    /* ---- 3. 팔레트는 지금처럼 초점 옆 ---------------------------------- */
    await clear();
    const palette = await page.evaluate(async () => {
      await openTermTab({ placement: "tab" });
      const focused = focusedPane;
      await openEmulatorTab("ios", "U1");
      await new Promise((done) => setTimeout(done, 120));
      const mirror = tabs.find((one) => one.kind === "emulator");
      return {
        took: activeTabId === mirror?.id,
        group: mirror?.pane ?? null,
        focused,
        seatedIn: mirror?.worktree ?? null,
        home: activeWorktreePath,
      };
    });
    const paletteToasts = await toasts();
    ok(
      "the palette's mirror still stands beside the focus and takes the screen, saying nothing",
      palette.took && palette.group !== palette.focused && palette.seatedIn === palette.home
        && !paletteToasts.includes(unseatedWords),
      JSON.stringify({ palette, paletteToasts }),
    );

    /* ---- 4. 부른 판을 모르면 비활성 탭 + 한 줄 -------------------------- */
    await clear();
    await page.evaluate(() => openTermTab({ placement: "tab" }));
    await settle();
    const namelessBefore = await page.evaluate(() => ({
      tab: activeTabId, focused: focusedPane, tree: JSON.stringify(stageTree()),
    }));
    await fire({ platform: "ios", device: "U1" });
    await settle();
    const nameless = await page.evaluate((was) => ({
      mirrors: tabs.filter((one) => one.kind === "emulator" && one.worktree === activeWorktreePath).length,
      tabKept: activeTabId === was.tab, focusKept: focusedPane === was.focused,
      stageKept: JSON.stringify(stageTree()) === was.tree,
      group: tabs.find((one) => one.kind === "emulator")?.pane,
    }), namelessBefore);
    const namelessToasts = await toasts();
    await clear();
    await page.evaluate(() => openTermTab({ placement: "tab" }));
    await settle();
    const strangerBefore = await page.evaluate(() => ({
      tab: activeTabId, focused: focusedPane, tree: JSON.stringify(stageTree()),
    }));
    await fire({ platform: "ios", device: "U1", term: 987654 });
    await settle();
    const stranger = await page.evaluate((was) => ({
      mirrors: tabs.filter((one) => one.kind === "emulator" && one.worktree === activeWorktreePath).length,
      tabKept: activeTabId === was.tab, focusKept: focusedPane === was.focused,
      stageKept: JSON.stringify(stageTree()) === was.tree,
      group: tabs.find((one) => one.kind === "emulator")?.pane,
    }), strangerBefore);
    const strangerToasts = await toasts();
    ok(
      "an unknown caller adds an inactive mirror without changing the tab, focus group or split layout",
      nameless.mirrors === 1 && namelessToasts.includes(unseatedWords)
        && nameless.tabKept && nameless.focusKept && nameless.stageKept
        && nameless.group === namelessBefore.focused
        && stranger.mirrors === 1 && strangerToasts.includes(unseatedWords)
        && stranger.tabKept && stranger.focusKept && stranger.stageKept
        && stranger.group === strangerBefore.focused,
      JSON.stringify({ nameless, namelessToasts, stranger, strangerToasts, unseatedWords }),
    );

    /* ---- 5. 이미 스트림이 있는 기기를 다른 체크아웃에서 다시 열어도 ------ */
    await clear();
    const reused = await page.evaluate(async (awayPath) => {
      const leader = await openTermTab({ placement: "tab" });
      await openEmulatorTab("ios", "U1");
      await new Promise((done) => setTimeout(done, 120));
      const person = tabs.find((one) => one.kind === "emulator");
      window.__SEAT__.shared = { U1: person?.stream };
      // 사람은 다시 제 터미널로 돌아왔다.
      setActiveTab(tabOfTerm(leader).id);
      const worker = 13791;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: leader, term: worker, worktree: awayPath, agent: "claude", resumed: "fresh" } });
      }
      await new Promise((done) => setTimeout(done, 120));
      return { worker, tab: activeTabId, worktree: activeWorktreePath, person: person?.id ?? null };
    }, AWAY);
    await fire({ platform: "ios", device: "U1", term: reused.worker });
    await settle();
    const rehomed = await page.evaluate((was) => ({
      tabKept: activeTabId === was.tab,
      worktreeKept: activeWorktreePath === was.worktree,
      mirrors: tabs.filter((one) => one.kind === "emulator").map((one) => one.id),
    }), reused);
    ok(
      "re-opening a device that already streams, from another checkout, does not hand the person's screen over",
      rehomed.tabKept && rehomed.worktreeKept
        && rehomed.mirrors.length === 1 && rehomed.mirrors[0] === reused.person,
      JSON.stringify({ reused, rehomed }),
    );

    /* 같은 체크아웃의 재사용도 기존 탭과 입력 수신자를 유지한다. */
    await clear();
    const sameCheckout = await page.evaluate(async () => {
      const leader = await openTermTab({ placement: "tab" });
      await openEmulatorTab("ios", "U1");
      const person = tabs.find((one) => one.kind === "emulator");
      window.__SEAT__.shared = { U1: person?.stream };
      window.__SEAT__.oldChannel = person?.binaryDoor?.channel;
      setActiveTab(tabOfTerm(leader).id);
      const field = document.createElement("input");
      field.id = "emulator-seat-typing";
      document.body.appendChild(field);
      field.focus();
      return {
        leader, person: person?.id, stream: person?.stream,
        tab: activeTabId, focused: focusedPane, tree: JSON.stringify(stageTree()),
      };
    });
    await fire({ platform: "ios", device: "U1", term: sameCheckout.leader });
    await settle();
    const sameReuse = await page.evaluate((was) => {
      const person = tabs.find((one) => one.id === was.person);
      const latest = window.__SEAT__.channels.at(-1);
      const answer = {
        tabKept: activeTabId === was.tab, focusKept: focusedPane === was.focused,
        stageKept: JSON.stringify(stageTree()) === was.tree,
        receiverKept: document.activeElement?.id === "emulator-seat-typing",
        mirrors: tabs.filter((one) => one.kind === "emulator").map((one) => one.id),
        streamKept: person?.stream === was.stream,
        frameChannelKept: latest != null && person?.binaryDoor?.channel === latest && !latest.cleaned,
        oldChannelReleased: window.__SEAT__.oldChannel?.cleaned === true,
      };
      document.getElementById("emulator-seat-typing")?.remove();
      return answer;
    }, sameCheckout);
    ok(
      "same-checkout reuse keeps the person's tab, typing field and layout, and retains the live frame channel",
      sameReuse.tabKept && sameReuse.focusKept && sameReuse.stageKept && sameReuse.receiverKept
        && sameReuse.mirrors.length === 1 && sameReuse.mirrors[0] === sameCheckout.person
        && sameReuse.streamKept && sameReuse.frameChannelKept && sameReuse.oldChannelReleased,
      JSON.stringify({ sameCheckout, sameReuse }),
    );

    /* ---- 6. 다른 체크아웃에 선 탭은 사람의 화면에 쓰지 않는다 ------------ */
    await clear();
    const watched = await page.evaluate(async (awayPath) => {
      await openEmulatorTab("ios", "U1");
      await new Promise((done) => setTimeout(done, 120));
      const person = tabs.find((one) => one.kind === "emulator");
      for (const hear of window.__LISTENERS__["emulator:frame"] ?? []) {
        hear({ payload: { stream: person.stream, bytes: btoa("person-frame") } });
      }
      const worker = 13792;
      // 워커의 판은 모든 체크아웃에 있는 그룹 0에 앉는다 — 사람의 거울이 지금
      // 서 있는 바로 그 그룹의 번호다.
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: 0, term: worker, worktree: awayPath, agent: "claude", resumed: "fresh" } });
      }
      await new Promise((done) => setTimeout(done, 120));
      const host = groupOf(person.pane).emulatorView;
      return {
        worker,
        person: person.id,
        personGroup: person.pane,
        workerGroup: tabOfTerm(worker)?.pane ?? null,
        name: host?.querySelector(".emulator-name")?.textContent ?? null,
        note: host?.querySelector(".emulator-note")?.textContent ?? null,
      };
    }, AWAY);
    await fire({ platform: "ios", device: "U2", term: watched.worker });
    await page.evaluate(() => new Promise((done) => setTimeout(done, 1_300)));
    const untouched = await page.evaluate((was) => {
      const person = tabs.find((one) => one.id === was.person);
      const host = groupOf(person.pane).emulatorView;
      return {
        bound: host?._emulatorTab === person,
        name: host?.querySelector(".emulator-name")?.textContent ?? null,
        note: host?.querySelector(".emulator-note")?.textContent ?? null,
        tab: activeTabId === was.person,
      };
    }, watched);
    ok(
      "a mirror seated in another checkout paints nothing on the device the person is watching",
      watched.workerGroup === watched.personGroup && untouched.bound && untouched.tab
        && untouched.name === watched.name && untouched.note === watched.note,
      JSON.stringify({ watched, untouched }),
    );

    ok("the emulator seat page raised nothing", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
