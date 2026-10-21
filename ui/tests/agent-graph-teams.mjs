/* t-68475 · 관계 탭 방향 E2 — 코디별 줄기 (사람 10-21 「e2로 가자」, 시안 p-2259b7961aa24a46 v1의 ③ E2).
 *
 * 카드 그림은 코디마다 한 열이고, 그 줄기에 워커와 헬퍼가 급한 순서로 매달린다. 여기서 재는 것은
 * 그 그림의 약속들이다:
 *
 *   - 코디마다 한 열, 열의 머리는 코디의 카드(제목 · 실행 · 모델 / 상태 줄 / 팀 줄)다.
 *   - 줄기의 차례는 당신 차례 → 멈춤 → 도는 중 → 차례 기다림 → 끝남, 헬퍼는 제 워커 밑에 들여쓴다.
 *   - 카드 크기 셋: L(질문 전문 · 답하기 · 질문 전문), M(지금 줄 · 까닭 · 로그 보기), S(두 줄).
 *   - 헬퍼 줄기에 끝난 카드가 둘 이상이면 「끝남 N — 제목들 … 펼치기」 한 줄로 묶이고(시안의 「끝난
 *     헬퍼는 한 줄로 묶습니다」), 편 것은 다시 그려도 남는다.
 *   - 머리의 「당신 차례 N」을 누르면 그 카드로 가고, 거듭 누르면 다음 카드로 간다.
 *   - 쉬는 코디의 자식은 그 코디의 열에 선다(방향 D의 결함 둘째), 칩은 카드 제 워크스페이스를 말한다
 *     (결함 첫째), 컨텍스트 수는 60%부터만 서고 4.5:1 위에서 읽힌다(결함 셋째).
 *   - 주인이 떠난 워커는 제 실행의 열(자리 빔)에, 자리가 넘어간 실행의 워커는 새 코디의 열에 선다.
 *   - 760px 밑의 그림에서는 열이 한 열로 쌓인다. 카드 보기에는 배율 단추가 없다.
 *   - 상태 낱말·칩·컨텍스트 글은 다크와 라이트 모두 4.5:1 위다. 상시 애니메이션은 없다.
 *
 * 장면은 시안의 예시 그대로다(코디 둘, 그 아래 일곱) — 원장의 길(`__PANES__`·`__LEDGER__`·
 * `__COLUMNS__`·코디 자리)로만 들어간다.
 *
 *   WINDOW_SUITES=agent-graph-teams node ui/tests/window.mjs
 */
import { openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";

/* 시안의 장면. 페이지 안에서 평가된다. `seats`는 원장의 코디 자리(`coordinator_seats`)가 하는 말 그대로다. */
export function teamsFixture({ seats = null, extra = [] } = {}) {
  const ROOT = "/repos/zerocode";
  const WT_A = "/repos/zerocode-wt/t-66788";
  const WT_B = "/repos/zerocode-wt/t-67607";
  const now = Date.now();
  projects = [{ name: "zerocode", path: ROOT, worktrees: [
    { path: ROOT, branch: "main", base: "", is_main: true },
    { path: WT_A, branch: "wt/t-66788/cu-observe", base: "main", is_main: false },
    { path: WT_B, branch: "wt/t-67607/board-geist", base: "main", is_main: false },
  ] }];
  const pane = (term, agent, state, over = {}) => ({
    term, agent, state, at: now - 60_000, state_started_at: now - 60_000, resumable: false, ...over,
  });
  window.__PANES__ = [
    pane(3, "claude", "working"), pane(4, "zo", "idle"), pane(6, "claude", "working", { parent: 3 }),
    pane(7, "claude", "needs-attention", { parent: 3 }), pane(8, "zo", "done", { parent: 4 }),
    pane(9, "zo", "needs-attention", { parent: 4 }), pane(20, "claude", "idle"),
  ];
  const row = (over) => ({
    run: "run-4275", worker: "", agent: "claude", state: "working", ledger: "active", hearing: "hook",
    hearing_at: now, checkout: ROOT, task: "", task_id: "", reported: false, review: null, term: null,
    at: now - 60_000, ...over,
  });
  window.__LEDGER__ = [
    row({ worker: "w-7", term: 7, checkout: WT_A, task: "반복 관측 줄이기", task_id: "t-66788", state: "waiting" }),
    row({ worker: "w-6", term: 6, checkout: WT_B, task: "작업 상황판 B 입히기", task_id: "t-67607" }),
    // 주인이 떠난 실행의 워커 — 판이 이 창에 없다(`worker:` 카드).
    row({ run: "run-9", worker: "w-orphan", term: null, checkout: WT_A, task: "주인 떠난 워커", task_id: "t-9" }),
  ];
  const card = (pane, heading, state, over = {}) => ({
    pane, heading, state, agent: "claude", project: ROOT, worktree: "main", task: "", you: "", said: "",
    ask: "", parent: "", ledger: "", unseen: false, changed_at: now - 60_000, at: now - 60_000, ...over,
  });
  window.__COLUMNS__ = [
    { bucket: "attention", cards: [
      card("term:7", "반복 관측 줄이기", "needs-attention", { parent: "term:3", task: "반복 관측 줄이기",
        ask: "같은 화면을 세 번 본 뒤 멈출까요, 다섯 번까지 볼까요? 세 번이면 시험 시간이 40% 줄고, 다섯 번이면 놓치는 변화가 없습니다.",
        at: now - 180_000 }),
      card("term:9", "Jev 판정 다시 돌리기", "needs-attention", { agent: "zo", parent: "term:4",
        ask: "cargo test -p zerocode-jev를 실행해도 될까요? 3분쯤 걸립니다.", at: now - 60_000 }),
    ] },
    { bucket: "working", cards: [
      card("term:3", "코디 — 창과 아티팩트", "working", { at: now - 5_000 }),
      card("term:6", "작업 상황판 B 입히기", "working", { parent: "term:3", task: "작업 상황판 B 입히기",
        said: "토큰 표를 값으로 옮기는 중입니다", at: now - 12_000 }),
      card("sub:7:b1", "헬퍼 B 시험 돌리기", "failed", { parent: "term:7", said: "시험 하나가 실패해 멈췄습니다", at: 0, changed_at: 0 }),
      card("worker:w-orphan", "주인 떠난 워커", "working", { task: "주인 떠난 워커" }),
    ] },
    { bucket: "done", cards: [
      card("term:8", "Jev 라벨 21건 세기", "done", { agent: "zo", parent: "term:4", said: "21건 중 비교된 판단은 0건입니다",
        unseen: true, at: now - 240_000 }),
      card("sub:6:a1", "토큰 대비 재기", "done", { parent: "term:6", said: "글자 대비가 모두 4.5:1을 넘습니다", at: 0, changed_at: 0 }),
      card("sub:6:a2", "게이트 이름 찾기", "done", { parent: "term:6", said: "바꿀 게이트 세 곳을 찾았습니다", at: 0, changed_at: 0 }),
    ] },
    { bucket: "idle", cards: [
      card("term:4", "코디 — zo와 Jev", "idle", { agent: "zo", at: now - 540_000 }),
      card("term:20", "손으로 연 판", "idle", { said: "끝났습니다" }),
    ] },
  ];
  for (const one of extra) window.__COLUMNS__.find((column) => column.bucket === one.bucket).cards.push(card(...one.card));
  window.__OVERLAYS__ = { latest: null, mail: [], dependencies: [], merge: [] };
  paneActivities.set("term:6", [{ at: now - 12_000, activity: { verb: "edit", target: "ui/tokens.css", phase: "started" } }]);
  paneActivities.set("term:3", [{ at: now - 5_000, activity: { verb: "read", target: "reports/t-67561/report.md", phase: "finished" } }]);
  /* 컨텍스트: term:7은 74%(서고), term:9는 33%(서지 않는다). */
  for (const [term, tokens] of [[7, 74_000], [9, 33_000]]) {
    paneSessions.set(term, { ...(paneSessions.get(term) ?? {}), session: { id: `s-${term}` } });
    sessionUsage.set(`s-${term}`, { tokens, window: 100_000 });
  }
  coordinatorSeats = (seats ?? [
    { run: "run-4275", name: "창과 아티팩트", term: 3, held: true },
    { run: "run-11955", name: "zo와 Jev", term: 4, held: true },
    { run: "run-9", name: "", term: null, held: false },
  ]).map((seat) => ({ generation: 1, vacatedMs: null, ...seat }));
  window.__ANSWER__.coordinator_seats = () => coordinatorSeats.map((seat) => ({ ...seat, vacated_ms: seat.vacatedMs }));
  if (typeof setAgentOrbitChoice === "function") setAgentOrbitChoice(document.getElementById("board-view"), "cards");
  agentBoardMode = "graph";
  agentGraphSelectedKey = null;
  agentGraphTraceKey = null;
  agentGraphScopeKey = "";
  agentGraphOverlayMode = "none";
  agentGraphFollowing = false;
  agentGraphFoldsOpen.clear();
  agentGraphAsksOpen.clear();
  agentGraphLogsOpen.clear();
  boardQuery = "";
  boardBroken = false;
  if (typeof activeTabId !== "undefined" && activeTabId === "board") dropTab("board");
  openBoard();
}

/* 그림이 선 그대로를 읽는다 — 열·머리·줄기·카드. A page function. */
function readTeams() {
  const view = document.querySelector("#board-view");
  const host = view.querySelector(".agent-graph-nodes");
  const card = (node) => ({
    key: node.dataset.graphKey,
    signal: node.dataset.signal,
    size: ["l", "m", "s"].find((one) => node.classList.contains(`is-size-${one}`)) ?? (node.classList.contains("is-head") ? "head" : ""),
    title: node.querySelector(".agent-graph-node-title")?.textContent ?? "",
    word: node.querySelector(".agent-graph-state-word")?.textContent ?? "",
    chip: node.querySelector(".agent-graph-node-branch:not([hidden])")?.textContent ?? "",
    level: node.getAttribute("aria-level"),
  });
  const spine = (list) => [...(list?.children ?? [])].map((limb) => {
    if (limb.classList.contains("is-fold")) {
      const fold = limb.querySelector(":scope > .agent-graph-fold");
      return { fold: fold.dataset.graphFold, open: fold.getAttribute("aria-expanded") === "true",
        count: fold.querySelector(".agent-graph-fold-count")?.textContent ?? "",
        names: fold.querySelector(".agent-graph-fold-names")?.textContent ?? "",
        members: spine(limb.querySelector(":scope > ol")) };
    }
    const node = limb.querySelector(":scope > .agent-graph-node");
    return { ...card(node), joint: limb.querySelector(":scope > .agent-graph-joint > .agent-graph-signal")?.className ?? "",
      kids: spine(limb.querySelector(":scope > .agent-graph-spine")) };
  });
  return [...host.querySelectorAll(":scope > .agent-graph-team")].map((team) => {
    const head = team.querySelector(":scope > .agent-graph-node.is-head, :scope > .agent-graph-team-head");
    return {
      kind: [...team.classList].find((one) => ["is-lead", "is-run", "is-own"].includes(one)) ?? "",
      head: head.classList.contains("agent-graph-node") ? card(head) : {
        title: head.querySelector(".agent-graph-node-title")?.textContent ?? "",
        runs: head.querySelector(".agent-graph-node-model")?.textContent ?? "",
        word: head.querySelector(".agent-graph-state-word")?.textContent ?? "",
      },
      runs: head.querySelector(".agent-graph-node-model")?.textContent ?? "",
      now: head.querySelector(".agent-graph-card-now")?.textContent ?? "",
      sum: [...head.querySelectorAll(".agent-graph-team-sum li")].map((li) => li.textContent.replace(/\s+/g, " ").trim()),
      spine: spine(team.querySelector(":scope > .agent-graph-spine")),
    };
  });
}

/* 다크·라이트의 글자 대비 — 카드가 실제로 칠한 색으로. A page function. */
function readContrast() {
  const view = document.querySelector("#board-view");
  const channel = (value) => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
  const rgb = (color) => {
    const parts = (color.match(/[\d.]+/g) ?? [0, 0, 0]).map(Number);
    return color.startsWith("color(srgb") ? parts.slice(0, 3).map((one) => one * 255) : parts.slice(0, 3);
  };
  const luminance = (color) => { const [r, g, b] = rgb(color); return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b); };
  const ratio = (ink, ground) => { const a = luminance(ink); const b = luminance(ground); return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05); };
  /* 바탕은 그 글자 위로 처음 칠해진 불투명한 바탕이다. */
  const groundOf = (node) => {
    for (let at = node; at; at = at.parentElement) {
      const color = getComputedStyle(at).backgroundColor;
      const alpha = color.startsWith("color(srgb") ? Number((color.match(/\/\s*([\d.]+)/) ?? [0, 1])[1]) : Number((color.match(/rgba?\([^)]*,\s*([\d.]+)\)/) ?? [0, 1])[1]);
      if (color !== "rgba(0, 0, 0, 0)" && color !== "transparent" && alpha > 0.99) return color;
    }
    return getComputedStyle(document.body).backgroundColor;
  };
  const read = (selector) => [...view.querySelectorAll(selector)]
    .filter((node) => node.checkVisibility() && node.textContent.trim())
    .map((node) => Math.round(ratio(getComputedStyle(node).color, groundOf(node)) * 100) / 100);
  return {
    theme: document.documentElement.dataset.theme ?? "dark",
    words: read(".agent-graph-nodes .agent-graph-state-word"),
    chips: read(".agent-graph-nodes .agent-graph-node-branch-name"),
    context: read(".agent-graph-nodes .agent-graph-node-context-word"),
    ages: read(".agent-graph-nodes .agent-graph-card-age"),
    said: read(".agent-graph-nodes .agent-graph-card-said, .agent-graph-nodes .agent-graph-node-message"),
    titles: read(".agent-graph-nodes .agent-graph-node-title"),
    chipsHead: read(".agent-graph-summary .agent-graph-stat:not(:disabled) .agent-graph-stat-count"),
  };
}

export async function testAgentGraphTeams(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.evaluate(teamsFixture);
    await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
    });

    const teams = await page.evaluate(readTeams);
    const [lead, zo, vacant, own] = teams;
    ok("E2 stands one column per coordinator, the most urgent first, then the vacant run and the person's own panes",
      teams.length === 4 && lead.kind === "is-lead" && zo.kind === "is-lead" && vacant.kind === "is-run" && own.kind === "is-own"
        && lead.head.key === "agent:term:3" && lead.head.title === "코디 — 창과 아티팩트" && lead.runs.includes("run-4275")
        && zo.head.key === "agent:term:4" && zo.head.title === "코디 — zo와 Jev",
      JSON.stringify(teams));

    ok("a coordinator's head says its state, what it does, and its team line",
      lead.head.word === "도는 중" && lead.now.includes("report.md") && lead.sum.join("|") === "팀 5|당신 차례 1|멈춤 1"
        && zo.head.word === "차례 기다림" && zo.now === "Jev 판정 다시 돌리기의 넘김을 기다립니다" && zo.sum.join("|") === "팀 2|당신 차례 1",
      JSON.stringify({ lead: [lead.head, lead.now, lead.sum], zo: [zo.head, zo.now, zo.sum] }));

    const leadSpine = lead.spine;
    ok("the stem hangs its cards in urgency order: your turn, halted, running, waiting its turn, finished",
      leadSpine.length === 2 && leadSpine[0].key === "agent:term:7" && leadSpine[0].signal === "you"
        && leadSpine[1].key === "agent:term:6" && leadSpine[1].signal === "run"
        && zo.spine.map((one) => one.key).join() === "agent:term:9,agent:term:8"
        && zo.spine[1].signal === "done",
      JSON.stringify({ leadSpine, zo: zo.spine }));

    ok("three card sizes: your turn is L, running and halted M, the rest S — and each joint wears its card's shape",
      leadSpine[0].size === "l" && leadSpine[1].size === "m" && leadSpine[0].kids[0]?.size === "m"
        && zo.spine[1].size === "s" && leadSpine[0].joint.includes("is-you") && leadSpine[1].joint.includes("is-run")
        && leadSpine[0].kids[0]?.joint.includes("is-halt") && zo.spine[1].joint.includes("is-done"),
      JSON.stringify(leadSpine));

    ok("a helper hangs under its own worker on an indented stem",
      leadSpine[0].kids.length === 1 && leadSpine[0].kids[0].key === "agent:sub:7:b1" && leadSpine[0].kids[0].level === "3"
        && leadSpine[0].level === "2" && lead.head.level === "1"
        && await page.evaluate(() => {
          const box = (key) => document.querySelector(`.agent-graph-node[data-graph-key="${key}"]`).getBoundingClientRect();
          return box("agent:sub:7:b1").left - box("agent:term:7").left >= 20;
        }),
      JSON.stringify(leadSpine[0]));

    const lCard = await page.evaluate(() => {
      const node = document.querySelector('.agent-graph-node[data-graph-key="agent:term:7"]');
      const ask = node.querySelector(".agent-graph-ask");
      const reply = node.querySelector(".agent-graph-reply");
      return { ask: ask?.textContent ?? "", clamp: ask ? getComputedStyle(ask).webkitLineClamp : "",
        reply: reply?.textContent ?? "", toggle: node.querySelector(".agent-graph-ask-toggle")?.textContent ?? "",
        context: node.querySelector(".agent-graph-node-context:not([hidden]) .agent-graph-node-context-word")?.textContent ?? "",
        age: node.querySelector(".agent-graph-card-age")?.textContent ?? "" };
    });
    ok("the L card carries the whole question, 「답하기 term:N」 and 「질문 전문」, and its context from 60%",
      lCard.ask.startsWith("같은 화면을 세 번") && lCard.clamp === "3" && lCard.reply.includes("답하기") && lCard.reply.includes("term:7")
        && lCard.toggle === "질문 전문" && lCard.context === "74%" && lCard.age === "3분째 기다림",
      JSON.stringify(lCard));

    const below = await page.evaluate(() => document.querySelector('.agent-graph-node[data-graph-key="agent:term:9"] .agent-graph-node-context')?.hidden);
    ok("a context under 60% stands no ring and no number", below === true, String(below));

    const mCard = await page.evaluate(() => {
      const node = document.querySelector('.agent-graph-node[data-graph-key="agent:term:6"]');
      return { verb: node.querySelector(".agent-card-activity-verb")?.textContent ?? "",
        target: node.querySelector(".agent-card-activity-target")?.textContent ?? "",
        why: node.querySelector(".agent-graph-node-message")?.textContent ?? "",
        chip: node.querySelector(".agent-graph-node-branch:not([hidden])")?.textContent ?? "" };
    });
    ok("the M card says what it touches in one line and why in another, with its own workspace chip",
      mCard.target.includes("tokens.css") && mCard.why === "토큰 표를 값으로 옮기는 중입니다" && mCard.chip === "wt/t-67607/board-geist",
      JSON.stringify(mCard));

    ok("every chip names the card's own workspace and a helper in its worker's workspace wears none (direction D's shifted labels)",
      leadSpine[0].chip === "wt/t-66788/cu-observe" && leadSpine[1].chip === "wt/t-67607/board-geist"
        && leadSpine[0].kids[0].chip === "",
      JSON.stringify(leadSpine.map((one) => [one.key, one.chip, one.kids.map((kid) => [kid.key, kid.chip])])));

    const fold = leadSpine[1].kids[0];
    ok("two finished helpers fold into one 「끝남 N — 제목들 … 펼치기」 line",
      leadSpine[1].kids.length === 1 && fold?.fold === "fold:agent:term:6" && fold.count === "끝남 2" && !fold.open
        && fold.names === "토큰 대비 재기 · 게이트 이름 찾기" && fold.members.length === 0,
      JSON.stringify(leadSpine[1]));

    const opened = await page.evaluate(async () => {
      document.querySelector('.agent-graph-fold[data-graph-fold="fold:agent:term:6"]').click();
      await window.__BOARD_SETTLED__();
      const after = [...document.querySelectorAll('.agent-graph-fold[data-graph-fold="fold:agent:term:6"] + ol > li > .agent-graph-node')]
        .map((node) => node.dataset.graphKey);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const kept = document.querySelector('.agent-graph-fold[data-graph-fold="fold:agent:term:6"]').getAttribute("aria-expanded");
      const act = document.querySelector('.agent-graph-fold[data-graph-fold="fold:agent:term:6"] .agent-graph-fold-act').textContent;
      document.querySelector('.agent-graph-fold[data-graph-fold="fold:agent:term:6"]').click();
      await window.__BOARD_SETTLED__();
      return { after, kept, act, closed: document.querySelectorAll('.agent-graph-node[data-graph-key^="agent:sub:6:"]').length };
    });
    ok("펼치기 opens the folded cards in their own stem, keeps them open across a repaint, and 접기 closes them",
      opened.after.join() === "agent:sub:6:a1,agent:sub:6:a2" && opened.kept === "true" && opened.act === "접기" && opened.closed === 0,
      JSON.stringify(opened));

    const chips = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const counts = Object.fromEntries([...view.querySelectorAll("[data-graph-signal]")]
        .map((button) => [button.dataset.graphSignal, button.querySelector("[data-graph-count]").textContent]));
      const words = [...view.querySelectorAll("[data-graph-signal] > span")].map((one) => one.textContent);
      // 그림의 기본 선택은 첫 당신 차례 카드다 — 그 위에서 누르면 다음 카드로 가므로, 다른 카드에서 시작한다.
      selectAgentGraphEntity(view, "agent:term:6", { focus: false });
      await window.__BOARD_SETTLED__();
      const you = view.querySelector('[data-graph-signal="you"]');
      you.click();
      await window.__BOARD_SETTLED__();
      const first = { selected: agentGraphSelectedKey, focused: document.activeElement?.dataset.graphKey ?? null };
      you.click();
      await window.__BOARD_SETTLED__();
      const second = agentGraphSelectedKey;
      view.querySelector('[data-graph-signal="done"]').click();
      await window.__BOARD_SETTLED__();
      const done = agentGraphSelectedKey;
      return { counts, words, first, second, done };
    });
    ok("the head's five state chips count the picture, and 「당신 차례 N」 goes to that card — again to the next",
      chips.counts.you === "2" && chips.counts.halt === "1" && chips.counts.run === "3" && chips.counts.wait === "1" && chips.counts.done === "3"
        && chips.words.join() === "당신 차례,멈춤,도는 중,차례 기다림,끝남"
        && chips.first.selected === "agent:term:7" && chips.first.focused === "agent:term:7" && chips.second === "agent:term:9"
        && ["agent:term:8", "agent:sub:6:a1", "agent:sub:6:a2"].includes(chips.done),
      JSON.stringify(chips));

    ok("a resting coordinator's column stands with its children under it (direction D's floating children)",
      zo.head.key === "agent:term:4" && zo.spine.length === 2 && zo.spine.every((one) => one.level === "2"),
      JSON.stringify(zo));

    ok("a worker whose coordinator left stands in its run's column, headed by the empty seat",
      vacant.head.title === "코디 — run-9" && vacant.head.word === "코디 자리가 비어 있습니다"
        && vacant.spine.map((one) => one.key).join() === "agent:worker:w-orphan"
        && own.head.title === "내 판 1" && own.spine.map((one) => one.key).join() === "agent:term:20",
      JSON.stringify({ vacant, own }));

    const takeover = await page.evaluate(async () => {
      // run-4275의 자리가 term:20으로 넘어갔다 — term:3은 열려 있지만 더는 자리에 앉아 있지 않다.
      coordinatorSeats = coordinatorSeats.map((seat) => seat.run === "run-4275" ? { ...seat, term: 20 } : seat);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      const view = document.querySelector("#board-view");
      const teamOf = (key) => view.querySelector(`.agent-graph-node[data-graph-key="${key}"]`)?.closest(".agent-graph-team");
      const headOf = (key) => teamOf(key)?.querySelector(":scope > .agent-graph-node.is-head, :scope > .agent-graph-team-head");
      const seen = {
        worker7: headOf("agent:term:7")?.dataset.graphKey ?? null,
        worker6: headOf("agent:term:6")?.dataset.graphKey ?? null,
        oldLead: teamOf("agent:term:3")?.className ?? "",
        heads: [...view.querySelectorAll(".agent-graph-node.is-head")].map((node) => node.dataset.graphKey),
      };
      coordinatorSeats = coordinatorSeats.map((seat) => seat.run === "run-4275" ? { ...seat, term: 3 } : seat);
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      return seen;
    });
    ok("when a run's seat moves to a new pane, its workers stand under the new coordinator and the old pane leads no empty column",
      takeover.worker7 === "agent:term:20" && takeover.worker6 === "agent:term:20" && takeover.oldLead.includes("is-own")
        && !takeover.heads.includes("agent:term:3"),
      JSON.stringify(takeover));

    const trace = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const node = (key) => view.querySelector(`.agent-graph-node[data-graph-key="${key}"]`);
      // term:6의 끝난 헬퍼 둘이 펼쳐진 판 — 계보는 아래로 끝까지 밝다.
      agentGraphFoldsOpen.add("fold:agent:term:6");
      await paintBoardView(undefined, { force: true });
      await window.__BOARD_SETTLED__();
      node("agent:term:6").click();
      await window.__BOARD_SETTLED__();
      const lit = [...view.querySelectorAll(".agent-graph-node.is-traced")].map((one) => one.dataset.graphKey).sort();
      const tracing = view.querySelector(".agent-graph-nodes").classList.contains("is-tracing");
      // 물러남은 전이다 — 다 지난 뒤의 값을 읽는다.
      await new Promise((done) => setTimeout(done, 400));
      const dim = getComputedStyle(node("agent:term:9")).opacity;
      view.querySelector(".agent-graph-layout").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await window.__BOARD_SETTLED__();
      return { lit, tracing, dim, after: view.querySelector(".agent-graph-nodes").classList.contains("is-tracing"),
        selected: agentGraphSelectedKey };
    });
    ok("a pressed card keeps only its lineage lit, and Escape gives the whole picture back",
      trace.tracing && trace.lit.join() === "agent:sub:6:a1,agent:sub:6:a2,agent:term:3,agent:term:6"
        && Math.abs(Number(trace.dim) - 0.45) < 0.01 && !trace.after
        && trace.selected === "agent:term:6",
      JSON.stringify(trace));

    const keys = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      selectAgentGraphEntity(view, "agent:term:7", { focus: true });
      await window.__BOARD_SETTLED__();
      const press = async (key) => {
        document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
        await window.__BOARD_SETTLED__();
        return document.activeElement?.dataset.graphKey ?? document.activeElement?.dataset.graphFold ?? null;
      };
      return { right: await press("ArrowRight"), left: await press("ArrowLeft"), up: await press("ArrowLeft"),
        down: await press("ArrowDown"), home: await press("Home"),
        tabStops: [...view.querySelectorAll('.agent-graph-nodes [tabindex="0"]')].length };
    });
    ok("the columns are a tree a keyboard walks: right into a helper, left to the worker and its coordinator, down the stem",
      keys.right === "agent:sub:7:b1" && keys.left === "agent:term:7" && keys.up === "agent:term:3" && keys.down === "agent:term:7"
        && keys.home === "agent:term:3" && keys.tabStops === 1,
      JSON.stringify(keys));

    const mail = await page.evaluate(async () => {
      window.__OVERLAYS__ = { latest: null, mail: [{ from: "term:7", to: "term:9", count: 2, unread: 1, at: Date.now() }],
        dependencies: [], merge: [] };
      await paintBoardView(undefined, { force: true });
      agentGraphSetOverlay(document.querySelector("#board-view"), "mail");
      await window.__BOARD_SETTLED__();
      const view = document.querySelector("#board-view");
      const line = view.querySelector(".agent-graph-edges .agent-graph-edge.is-mail");
      const from = view.querySelector('.agent-graph-node[data-graph-key="agent:term:7"]').getBoundingClientRect();
      const to = view.querySelector('.agent-graph-node[data-graph-key="agent:term:9"]').getBoundingClientRect();
      const seen = { d: line?.getAttribute("d") ?? "", label: line?.parentElement.querySelector("text")?.textContent ?? "",
        across: to.left > from.right };
      agentGraphSetOverlay(view, "mail");
      await window.__BOARD_SETTLED__();
      seen.after = view.querySelectorAll(".agent-graph-edges .agent-graph-edge").length;
      return seen;
    });
    ok("the mail overlay draws its line across the columns, and none stands when the overlay is off",
      // 두 끝이 같은 높이면 곧은 한 획(`H`), 다르면 곡선(`C`)이다.
      mail.across && /^M [\d.]+ [\d.]+ (C|H) /.test(mail.d) && mail.label.startsWith("메일 · 2") && mail.after === 0,
      JSON.stringify(mail));

    const still = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 400));
      const view = document.querySelector("#board-view");
      const running = view.getAnimations({ subtree: true }).filter((one) => one.effect?.getTiming().iterations === Infinity);
      const zoom = getComputedStyle(view.querySelector(".agent-graph-zoom")).display;
      const before = agentGraphZoom;
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "1", shiftKey: true, bubbles: true }));
      /* 그리고 상태 칩 다섯은 두 보기 모두의 머리에 선다 — D의 상태 단추가 그랬듯이. 입체에서는 점과
       * 숫자만 남고 낱말은 단추의 이름으로 남아, 칩 줄이 머리를 두 줄로 늘리지 않는다(입체의 무대가 D
       * 때의 높이를 지킨다). 입체를 띄우지 않고 그 보기의 표지만 잠깐 입혀 규칙을 읽는다. */
      const summary = view.querySelector(".agent-graph-summary");
      const word = summary.querySelector(".agent-graph-stat > span");
      const chipsInCards = getComputedStyle(summary).display;
      const wordInCards = Math.round(word.getBoundingClientRect().width);
      view.classList.add("is-orbit");
      const chipsInOrbit = getComputedStyle(summary).display;
      const wordInOrbit = Math.round(word.getBoundingClientRect().width);
      const nameInOrbit = word.textContent;
      view.classList.remove("is-orbit");
      return { infinite: running.length, zoom, zoomKept: agentGraphZoom === before,
        chipsInCards, chipsInOrbit, wordInCards, wordInOrbit, nameInOrbit };
    });
    ok("the card picture runs no endless animation and stands no zoom buttons — zoom is the orbit's, and the five chips stand in both views, the orbit's as mark and count",
      still.infinite === 0 && still.zoom === "none" && still.zoomKept
        && still.chipsInCards !== "none" && still.chipsInOrbit !== "none"
        && still.wordInCards > 1 && still.wordInOrbit <= 1 && still.nameInOrbit !== "", JSON.stringify(still));

    const stacked = async (width) => {
      await page.setViewportSize({ width, height: 1100 });
      return page.evaluate(async () => {
        await new Promise((done) => setTimeout(done, 300));
        await window.__BOARD_SETTLED__();
        const view = document.querySelector("#board-view");
        const scroll = view.querySelector(".agent-graph-scroll");
        const lefts = new Set([...view.querySelectorAll(".agent-graph-team")].map((team) => Math.round(team.getBoundingClientRect().left)));
        return { graph: scroll.clientWidth, columns: lefts.size, sideways: scroll.scrollWidth > scroll.clientWidth + 1 };
      });
    };
    const wide = await stacked(1440);
    const narrow = await stacked(720);
    ok("below a 760px picture the columns stack into one, and nothing scrolls sideways at either width",
      wide.columns >= 2 && narrow.graph < 760 && narrow.columns === 1 && !wide.sideways && !narrow.sideways,
      JSON.stringify({ wide, narrow }));
    await page.setViewportSize({ width: 1440, height: 1100 });

    const dark = await page.evaluate(async () => { await window.__BOARD_SETTLED__(); return null; }).then(() => page.evaluate(readContrast));
    await page.evaluate(async () => { theme = "light"; applyTheme(); await new Promise((done) => setTimeout(done, 400)); });
    const light = await page.evaluate(readContrast);
    await page.evaluate(() => { theme = "dark"; applyTheme(); });
    const atLeast = (list) => list.length > 0 && list.every((one) => one >= 4.5);
    ok("state words, chips, context numbers, ages, lines and titles all clear 4.5:1 in dark and light",
      dark.theme === "dark" && light.theme === "light"
        && [dark, light].every((one) => atLeast(one.words) && atLeast(one.chips) && atLeast(one.context)
          && atLeast(one.ages) && atLeast(one.said) && atLeast(one.titles) && atLeast(one.chipsHead)),
      JSON.stringify({ dark, light }));

    await page.emulateMedia({ reducedMotion: "reduce" });
    const reduced = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const card = view.querySelector(".agent-graph-node.is-size-l");
      return { card: getComputedStyle(card).transitionDuration, animation: getComputedStyle(card).animationName,
        fold: getComputedStyle(view.querySelector(".agent-graph-fold")).transitionDuration };
    });
    ok("reduced motion stills the cards and the fold line", reduced.animation === "none"
      && reduced.card.split(",").every((one) => Number.parseFloat(one) === 0)
      && reduced.fold.split(",").every((one) => Number.parseFloat(one) === 0), JSON.stringify(reduced));

    ok("E2 raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
