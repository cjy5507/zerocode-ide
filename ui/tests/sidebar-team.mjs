import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar's team view (t-56141).
 *
 * Asked 2026-10-15 11:1x from the person's own window: 「코디 별로 밑에 생성된 워커를 표시… 각자 자기 창을
 * 보게… 직관적인 UI」, and at 11:25 「응 그렇게 수정해줘 성능 메모리 최적화 그 화면에 영향받는 모든 곳 체크」.
 * The branch list mixed branches, tasks and workers on one line each, and reaching a worker's pane took a
 * project to open, a card to open and an agent row to press.
 *
 * The scene is a synthetic ledger — three coordinators, five workers, thirteen checkouts — built from the
 * window's own roads (`hook:agent`, `ledger:changed` over `ledger_agents` and `coordinator_seats`,
 * `project_catalog`). Nobody's conversation, screen or settings is read; the page dies with the suite.
 *   ① with a coordinator sitting, the team stands by default and no branch row is built behind it;
 *   ② eight rows — a card for each coordinator, a row for each worker — in the order a person reads them:
 *      a question or a blocker first, then work going, then work waiting, then rest;
 *   ③ each worker's six steps say what the ledger and git hold, and nothing else;
 *   ④ the line under a worker is the first sentence of its last status, at most thirty characters, and when;
 *   ⑤ a press brings that worker's pane to the front, a coordinator's card its pane, ⌘2 the second
 *      coordinator's, and the arrows walk the rows;
 *   ⑥ an account switch moves the pane number and not the row;
 *   ⑦ a quiet beat writes no row, a status letter writes one, and pointing at a row reads its pane's last
 *      three lines once;
 *      a released worker's work that waits on its coordinator keeps its row and landed work leaves — a worker
 *      whose release got no answer too (t-60850), while a worker at work keeps its row on landed work;
 *   ⑧ the landing line counts what waits to land and opens that list on the task board;
 *   ⑨ 「브랜치 보기」 brings the branch list back, stored, and the list's own switch brings the team back;
 *      a restart restores the choice;
 *   ⑩ every new word reads in the four other catalogues;
 *   ⑪ with no coordinator sitting the view reads nothing and the branch list is what it was.
 *
 * `ok` details carry the numbers the report quotes (rows, elements, rows written a beat). */

export const TEAM_RUNS = Object.freeze([
  { run: "run-team-a", name: "묶음 런", generation: 2, term: 9601, path: "/r/main", agent: "claude", model: "claude-opus-5-5",
    mail: 0, news: 2, landing: 1, batched: 0 },
  { run: "run-team-b", name: "판정 런", generation: 1, term: 9602, path: "/r/zo", agent: "zo", model: "claude-opus-5-5",
    mail: 1, news: 0, landing: 1, batched: 0 },
  { run: "run-team-c", name: "모델 런", generation: 1, term: 9603, path: "/r/codex", agent: "codex", model: "gpt-6-astra",
    mail: 0, news: 0, landing: 0, batched: 0 },
]);

/* The workers in the ledger's own order — which is not the order a person reads them in. Minutes are before the
 * scene's clock. */
export const TEAM_WORKERS = Object.freeze([
  { run: "run-team-a", worker: "w-a1", term: 9611, path: "/r/wt-a1", pane: "%5", agent: "claude", hook: "working",
    task: "대화 화면 2단계", task_id: "t-1001", landing: { state: "unlanded", ahead: 2 },
    status: { minutes: 2, line: "세 흐름 측정 결과 읽는 중. 표로 정리해 보고서에 넣는다" },
    ledger: { progress: { stage: "working" } } },
  { run: "run-team-a", worker: "w-a2", term: 9612, path: "/r/wt-a2", pane: "%3", agent: "claude", hook: "done",
    task: "아티팩트 화면 1단계", task_id: "t-1002", landing: { state: "no_commits", ahead: 0 },
    status: { minutes: 9, line: "시험 일감 다시 넣음, 줄 대기" },
    ledger: { progress: { stage: "working" }, wall: { resets_at_ms: null }, wallFor: 3_600_000 } },
  { run: "run-team-b", worker: "w-b2", term: 9622, path: "/r/wt-b2", pane: "%4", agent: "zo", hook: "done",
    task: "카파시 조각 보이기", task_id: "t-2002", landing: { state: "unlanded", ahead: 3 },
    status: { minutes: 20, line: "넘김 끝내고 코디 검증을 기다리는 중이며 다음 차례로 착지 묶음에 들어갈 예정" },
    ledger: { reported: true, handed_in: "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2", progress: { stage: "report_only" },
      land_check: { state: "failed", rc: 2, created_ms: 1 } } },
  { run: "run-team-b", worker: "w-b1", term: 9621, path: "/r/wt-b1", pane: "%2", agent: "zo", hook: "done",
    task: "Jev: 판정이 실제로 일어나게", task_id: "t-2001", landing: { state: "unlanded", ahead: 1 },
    status: null,
    ledger: { asking: true, progress: { stage: "working" } } },
  { run: "run-team-c", worker: "w-c1", term: 9631, path: "/r/wt-c1", pane: "%4", agent: "codex", hook: "done",
    task: "판단 모델 불러오기(URL → 설치)", task_id: "t-3001", landing: { state: "unlanded", ahead: 4 },
    status: { minutes: 1, line: "초록 고침 쓰는 중" },
    ledger: { reported: true, handed_in: "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1",
      review: { verified: true, merged: false, deployed: false, written: true, author: "coordinator" },
      progress: { stage: "verified" } } },
]);

/* The checkouts a branch list shows beside them: a red cut beside a worker's own, and finished work nobody
 * cleaned yet — thirteen rows in all. */
const TEAM_EXTRA_CHECKOUTS = Object.freeze(["/r/wt-a1-red", "/r/old-1", "/r/old-2", "/r/old-3", "/r/old-4"]);

/* Stands the scene in `page`. `layout` pins a view through the window's own writer (`null`: whatever the window
 * opens on); `seated: false` stands the same panes with no coordinator in any seat. Every helper it leaves on the
 * page waits for the window's own beat to have painted. Used by `sidebar-team-perf.mjs` against a base tree too,
 * so it touches nothing the team view added. */
export async function standTeamScene(page, { layout = null, seated = true, now = Date.now(), runs = TEAM_RUNS, workers = TEAM_WORKERS,
  extras = TEAM_EXTRA_CHECKOUTS, project = "r", taskIds = false } = {}) {
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async ({ runs, workers, extras, layout, seated, now, project, taskIds }) => {
    if (typeof setLocale === "function") setLocale("ko", { persist: false });
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    window.__TELL__ = tell;
    /* The team view's own state, read by name — a window without it (the base tree) answers `fallback`, so a check
     * there fails on what it asserts instead of throwing. */
    window.__TEAM__ = (name, fallback = null) => {
      try {
        return (0, eval)(name);
      } catch {
        return fallback;
      }
    };
    window.__ROWS__ = () => {
      const shown = (node) => node && !node.hidden && node.getClientRects().length > 0;
      return {
        team: [...document.querySelectorAll("#sidebar-team > .team-row")].filter(shown).length,
        coordinators: [...document.querySelectorAll("#sidebar-team > .team-coordinator")].filter(shown).length,
        workers: [...document.querySelectorAll("#sidebar-team > .team-worker")].filter(shown).length,
        branches: [...document.querySelectorAll("#worktrees .wt-row")].filter(shown).length,
        agents: [...document.querySelectorAll("#worktrees .wt-agent")].filter(shown).length,
        elements: document.querySelectorAll("#sidebar-scroll *").length,
      };
    };
    window.__ACTIVE__ = "/r/main";
    window.__ANSWER__.set_active_worktree = ({ path }) => {
      window.__ACTIVE__ = path;
      return path.split("/").pop();
    };
    window.__VIEW_WRITES__ = [];
    window.__ANSWER__.set_sidebar_view = (args) => {
      window.__VIEW_WRITES__.push(args);
      return null;
    };
    window.__ANSWER__.term_snapshot = ({ term }) => ({
      rows: [{ text: "" }, { text: `${term} 첫째 줄` }, { text: `${term} 둘째 줄` }, { text: `${term} 셋째 줄` },
        { text: `${term} 마지막 줄` }, { text: "   " }],
    });
    const paths = ["/r/main", ...runs.filter((one) => one.path !== "/r/main").map((one) => one.path),
      ...workers.map((one) => one.path), ...extras];
    const landingOf = (path) => workers.find((one) => one.path === path)?.landing ?? null;
    // `taskIds`: the catalog names each checkout's task the way the backend does — by the id its folder carries, so a
    // copy cut beside a worker's own (`<folder>-verify`) names the same task.
    const taskOf = (path) => (taskIds
      ? workers.find((one) => path === one.path || path.startsWith(`${one.path}-`))?.task_id : undefined);
    window.__ANSWER__.project_catalog = () => [{
      name: project, path: "/r",
      external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
      worktrees: paths.map((path, at) => {
        const landing = landingOf(path);
        return {
          path, branch: path === "/r/main" ? "main" : `wt/${path.slice(3)}`, is_main: path === "/r/main",
          active: path === window.__ACTIVE__, is_folder: false, ownership: "zerocode-managed", external_hidden: false,
          last_activity_ms: at,
          ...(taskOf(path) ? { task_id: taskOf(path) } : {}),
          ...(landing ? { landing: { compare_ref: "origin/main", dirty: false, ignored: false, detached: false,
            far_behind: false, ...landing } } : {}),
        };
      }),
    }];
    const panes = [
      ...runs.map((one) => ({ term: one.term, path: one.path, agent: one.agent, model: one.model, hook: "working" })),
      ...workers.map((one) => ({ term: one.term, path: one.path, agent: one.agent, model: "claude-sonnet-5-5", hook: one.hook })),
    ];
    for (const pane of panes) {
      tabs.push({ kind: "term", worktree: pane.path, id: `tab-${pane.term}`, layout: { type: "leaf", term: pane.term } });
      paneAgents.set(pane.term, pane.agent);
      tell("hook:agent", { term: pane.term, state: pane.hook, agent: pane.agent, session: `s-${pane.term}`, model: pane.model });
    }
    window.__ROW_OF__ = (over) => ({
      run: "", worker: "", agent: "claude", state: "working", ledger: "active", hearing: "pending", hearing_at: 1,
      checkout: "", task: "", task_id: "", reported: false, failed: false, settled: false, session: null, at: 1,
      review: { verified: false, merged: false, deployed: false, written: false, author: null }, review_since_ms: null,
      closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null, kept: null, taken_over: false,
      pane: "", stalled: [], ...over,
    });
    window.__LEDGER__ = workers.map((one) => {
      const { wallFor, wall, ...ledger } = one.ledger;
      return window.__ROW_OF__({
        run: one.run, worker: one.worker, agent: one.agent, term: one.term, checkout: one.path, task: one.task,
        task_id: one.task_id, pane: one.pane, model: "claude-sonnet-5-5",
        ...ledger,
        ...(one.root ?? {}),
        ...(wall ? { wall: { ...wall, stands_until_ms: now + wallFor } } : {}),
        ...(one.status ? { last_status: { message_id: `m-${one.worker}-1`, created_ms: now - one.status.minutes * 60_000,
          line: one.status.line } } : {}),
      });
    });
    window.__SEATS__ = seated
      ? runs.map((one) => ({ run: one.run, name: one.name, seat: true, term: one.term, held: true,
        generation: one.generation, vacated_ms: null, mail: one.mail, news: one.news, landing: one.landing,
        batched: one.batched }))
      : [];
    window.__ANSWER__.coordinator_seats = () => window.__SEATS__;
    setEveryProjectClosed(false);
    if (layout) setSidebarView({ groupBy: "repo", layout });
    await refreshWorktrees();
    /* The ledger moved: what the window does on its own beat. Waits for the navigator's read to land, for the team's
     * read when one stands, and for the paint after them. */
    window.__TEAM_SETTLED__ = async () => {
      const frame = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const team = () => typeof sidebarTeamData === "undefined"
        || (!sidebarTeamAsking && sidebarTeamFrame === 0 && (!sidebarTeamStood || sidebarTeamData.read));
      for (let tries = 0; tries < 120 && !(paneLedger.size >= window.__LEDGER__.filter((row) => Number.isInteger(row.term)).length && team()); tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      await frame();
      for (let tries = 0; tries < 60 && !team(); tries += 1) await new Promise((done) => setTimeout(done, 20));
      await frame();
    };
    window.__LEDGER_MOVED__ = async () => {
      tell("ledger:changed", Date.now());
      await window.__TEAM_SETTLED__();
    };
    await window.__LEDGER_MOVED__();
    // The first stand reads once more after the seats arrived; one more beat settles what that read painted.
    await window.__LEDGER_MOVED__();
  }, { runs, workers, extras, layout, seated, now, project, taskIds });
}

/* What a person reads in the sidebar, by kind of row — read in the page (`window.__ROWS__`). */
export async function readSidebarRows(page) {
  return page.evaluate(() => window.__ROWS__());
}

export async function testSidebarTeam({ browser, origin, ok, faults }) {
  const now = Date.now();
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await standTeamScene(page, { now });
    await page.evaluate(() => {
      window.__WORKER_ROW__ = (worker) => document.querySelector(`#sidebar-team [data-team-key$="/${CSS.escape(worker)}"]`);
      window.__READ_WORKER__ = (worker) => {
        const node = window.__WORKER_ROW__(worker);
        if (!node) return null;
        const text = (selector) => {
          const part = node.querySelector(selector);
          return part && !part.hidden ? part.textContent : "";
        };
        return {
          key: node.dataset.teamKey,
          title: text(".team-title"),
          chip: text(".team-chip"),
          question: node.querySelector(".team-chip")?.classList.contains("is-question") ?? false,
          pane: text(".team-pane"),
          now: text(".team-now"),
          nowIdle: node.querySelector(".team-now")?.classList.contains("is-idle") ?? false,
          nowTip: node.querySelector(".team-now")?.dataset.tip ?? "",
          dot: [...node.querySelector(".team-dot").classList].find((name) => name.startsWith("is-"))?.slice(3) ?? "",
          spinning: /\bwt-spin-\d+\b/.test(node.querySelector(".team-dot").className),
          moving: node.classList.contains("is-working"),
          cells: [...node.querySelectorAll(".team-strip .team-step")].map((cell) =>
            [...cell.classList].find((name) => name.startsWith("is-"))?.slice(3) ?? ""),
          step: text(".team-step-word"),
          stepsTip: node.querySelector(".team-strip")?.dataset.tip ?? "",
          tip: node.dataset.tip ?? "",
          aria: node.getAttribute("aria-label") ?? "",
        };
      };
    });

    /* ① — the team stands by default, and the branch list is not built behind it. */
    const stood = await page.evaluate(() => ({
      layout: window.__TEAM__("sidebarLayout"),
      stood: window.__TEAM__("sidebarTeamStood", false),
      teamShown: document.getElementById("sidebar-team")?.hidden === false,
      listHidden: document.getElementById("worktrees").hidden,
      branchNodes: document.querySelectorAll("#worktrees *").length,
      switchShown: document.querySelector(".team-switch")?.hidden === false,
      rows: window.__ROWS__(),
    }));
    ok(
      "with coordinators sitting, the sidebar opens on the team: the team stands, the branch list is hidden and holds no row, and the list's own switch is not offered",
      stood.layout === "team" && stood.stood && stood.teamShown && stood.listHidden && stood.branchNodes === 0 && !stood.switchShown,
      JSON.stringify(stood),
    );

    /* ② — eight rows, in a person's order. */
    const order = await page.evaluate(() => [...document.querySelectorAll("#sidebar-team > .team-row")]
      .map((node) => node.dataset.teamKey));
    ok(
      "eight rows — three coordinator cards and five worker rows — and under each coordinator a question or a blocker first, then work going, then work waiting",
      stood.rows.coordinators === 3 && stood.rows.workers === 5 && JSON.stringify(order) === JSON.stringify([
        "c:run-team-a", "w:run-team-a/w-a2", "w:run-team-a/w-a1",
        "c:run-team-b", "w:run-team-b/w-b1", "w:run-team-b/w-b2",
        "c:run-team-c", "w:run-team-c/w-c1", "landing",
      ]),
      JSON.stringify({ order, rows: stood.rows }),
    );

    /* ③ — six steps from the ledger's and git's records; ④ — the line under the title. */
    const read = await page.evaluate((workers) => Object.fromEntries(workers.map((one) => [one.worker, window.__READ_WORKER__(one.worker)])),
      TEAM_WORKERS);
    const words = await page.evaluate((now) => ({
      walled: t("board.desk.healthWalled", "한도 도달"),
      asking: t("board.desk.healthAsking", "답 기다림"),
      commit2: t("sidebar.team.step.commitCount", "커밋 {{count}}", { count: 2 }),
      read: t("sidebar.team.step.read", "읽기"),
      verify: t("sidebar.team.step.verify", "검증"),
      land: t("sidebar.team.step.land", "착지"),
      commit1: t("sidebar.team.step.commitCount", "커밋 {{count}}", { count: 1 }),
      ago2: t("sidebar.team.ago", "{{time}} 전", { time: agoWord(now - 2 * 60_000, Date.now()) }),
      checkFailed: t("board.desk.landCheckFailed", "점검 실패 (rc {{rc}})", { rc: 2 }),
      pane5: t("sidebar.team.pane", "판 {{pane}}", { pane: "%5" }),
      nowNone: t("sidebar.team.nowNone", "아직 보낸 말 없음"),
    }), now);
    const cells = (worker) => read[worker]?.cells.join(",");
    ok(
      "each worker's six steps say what the ledger and git hold: a worker with commits is at 커밋, a blocked one is held where it stands, a hand-in waits at 검증 with its failed check held, verified work waits at 착지 — and a step with no record stays empty",
      cells("w-a1") === "on,cur,,,," && read["w-a1"].step === words.commit2 &&
        cells("w-a2") === "stuck,,,,," && read["w-a2"].step === words.read &&
        cells("w-b1") === "on,stuck,,,," && read["w-b1"].step === words.commit1 &&
        cells("w-b2") === "on,on,stuck,on,cur," && read["w-b2"].step === words.verify &&
        read["w-b2"].stepsTip.includes(words.checkFailed) &&
        cells("w-c1") === "on,on,past,on,on,cur" && read["w-c1"].step === words.land,
      JSON.stringify(read),
    );
    ok(
      "a row's dot and its one chip say its state: working, blocked by its quota wall, asking (the chip in the question colour), waiting",
      read["w-a1"].dot === "working" && read["w-a1"].chip === "" &&
        read["w-a2"].dot === "blocked" && read["w-a2"].chip === words.walled &&
        read["w-b1"].dot === "asking" && read["w-b1"].chip === words.asking && read["w-b1"].question &&
        read["w-b2"].dot === "waiting" && read["w-c1"].dot === "waiting",
      JSON.stringify(read),
    );
    /* A working row moves — its dot's ring carries a spin angle within a beat and the row wears the flowing
     * border; a blocked or waiting row stands still. */
    const moving = await page.evaluate(async () => {
      await new Promise((resolve) => setTimeout(resolve, 160));
      return { a1: window.__READ_WORKER__("w-a1"), a2: window.__READ_WORKER__("w-a2"), c1: window.__READ_WORKER__("w-c1") };
    });
    ok(
      "a working row moves — its dot carries a spin angle and the row wears the flowing border — and a blocked or waiting row stands still",
      moving.a1.spinning && moving.a1.moving && !moving.a2.spinning && !moving.a2.moving && !moving.c1.spinning && !moving.c1.moving,
      JSON.stringify(moving),
    );
    ok(
      "the line under a worker is the first sentence of its last status — at most thirty characters — and the clock is the line's tip, not its words; a worker that said nothing keeps the line, dimmed, saying so; the title is the task and the pane is the ledger's",
      read["w-a1"].now === "세 흐름 측정 결과 읽는 중." && read["w-a1"].nowTip.endsWith(words.ago2) &&
        [...read["w-b2"].now].length === 30 && read["w-b2"].now.endsWith("…") &&
        read["w-b1"].now === words.nowNone && read["w-b1"].nowIdle && !read["w-a1"].nowIdle &&
        read["w-a1"].title === "대화 화면 2단계" && read["w-a1"].pane === words.pane5 &&
        !read["w-a1"].title.includes("wt/"),
      JSON.stringify(read),
    );

    /* The coordinator cards: who, what owes, and the landing line. */
    const cards = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll("#sidebar-team > .team-coordinator")]
      .map((node) => [node.dataset.teamKey, {
        name: node.querySelector(".team-name")?.textContent ?? "",
        run: node.querySelector(".team-run")?.hidden ? "" : node.querySelector(".team-run")?.textContent ?? "",
        project: node.querySelector(".team-project")?.hidden ? "" : node.querySelector(".team-project")?.textContent ?? "",
        questions: node.querySelector(".team-badge.is-question")?.hidden ? "" : node.querySelector(".team-badge.is-question")?.textContent ?? "",
        letters: node.querySelector(".team-badge.is-letters")?.hidden ? "" : node.querySelector(".team-badge.is-letters")?.textContent ?? "",
        face: node.querySelector(".team-face .agent-ico") !== null,
        tip: node.dataset.tip ?? "",
      }])));
    const cardWords = await page.evaluate(() => ({
      coordinator: t("agent.role.coordinator", "코디"),
      question1: t("sidebar.team.questions", "질문 {{count}}", { count: 1 }),
      letters2: t("sidebar.team.letters", "편지 {{count}}", { count: 2 }),
    }));
    ok(
      "a coordinator card names the coordinator and its run, wears its face, and badges what it owes: a question to answer, news to read",
      cards["c:run-team-a"]?.name.startsWith(cardWords.coordinator) && cards["c:run-team-a"].run === "묶음 런" &&
        cards["c:run-team-a"].letters === cardWords.letters2 && cards["c:run-team-a"].questions === "" &&
        cards["c:run-team-b"]?.questions === cardWords.question1 && cards["c:run-team-b"].letters === "" &&
        Object.values(cards).every((one) => one.face) && cards["c:run-team-a"].tip.includes("묶음 런"),
      JSON.stringify(cards),
    );
    ok(
      "a coordinator card names the project it works in — the scene's three seats all sit in r",
      Object.values(cards).every((one) => one.project === "r"),
      JSON.stringify(cards),
    );
    const landing = await page.evaluate(() => {
      const node = document.querySelector("#sidebar-team > .team-landing");
      return {
        text: [...(node?.querySelectorAll(".team-landing-count, .team-landing-step") ?? [])].map((one) => one.textContent).join(" → "),
        expected: [t("sidebar.team.landing", "착지 대기 {{count}}", { count: 2 }), t("sidebar.team.batch", "묶음"),
          t("sidebar.team.release", "다음 릴리즈")].join(" → "),
      };
    });
    ok(
      "the landing line counts the tasks every coordinator's run holds verified and not merged, then the batch, then the next release",
      landing.text === landing.expected,
      JSON.stringify(landing),
    );

    /* ⑤ — one press to a worker's pane. The press goes down the window's one road to a pane (`focusAgentPane`,
     * whose stage moves the agent rows' suites already hold) with the worker's own checkout, tab and terminal —
     * observed here, on the panes this scene only pretends to hold. */
    await page.evaluate(() => {
      window.__FOCUSED__ = [];
      window.__REAL_FOCUS__ = window.focusAgentPane;
      window.focusAgentPane = (...args) => {
        window.__FOCUSED__.push(args);
        return Promise.resolve();
      };
    });
    const pressed = await page.evaluate(() => {
      window.__WORKER_ROW__("w-a1")?.click();
      document.querySelector('#sidebar-team [data-team-key="c:run-team-c"]')?.click();
      return window.__FOCUSED__.splice(0);
    });
    ok(
      "one press on a worker's row brings that worker's pane forward, in its own checkout; a coordinator's card brings its own",
      JSON.stringify(pressed) === JSON.stringify([["/r/wt-a1", "tab-9611", 9611, "claude"], ["/r/codex", "tab-9603", 9603, "codex"]]),
      JSON.stringify({ presses: 1, pressed }),
    );

    /* ⑤ — the keyboard: arrows walk, Enter presses, ⌘2 reaches the second coordinator. */
    const keys = await page.evaluate(async () => {
      const first = document.querySelector('#sidebar-team [data-team-key="c:run-team-a"]');
      if (!first) {
        window.focusAgentPane = window.__REAL_FOCUS__;
        return {};
      }
      first.focus();
      const press = (target, init) => target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
      press(document.activeElement, { key: "ArrowDown" });
      const afterDown = document.activeElement?.dataset.teamKey ?? null;
      press(document.activeElement, { key: "ArrowDown" });
      const afterTwo = document.activeElement?.dataset.teamKey ?? null;
      press(document.activeElement, { key: "ArrowUp" });
      const afterUp = document.activeElement?.dataset.teamKey ?? null;
      const stops = [...document.querySelectorAll("#sidebar-team > .team-row")].filter((node) => node.tabIndex === 0).length;
      // A key pressed on a row stays the row's: the window's floor would hand it to the staged terminal.
      const standing = document.activeElement;
      press(standing, { key: "Enter" });
      const kept = document.activeElement === standing && document.activeElement !== keySink;
      // A button's Enter is its click.
      document.activeElement.click();
      const entered = window.__FOCUSED__.splice(0).map((args) => args[1]);
      const row = window.__WORKER_ROW__("w-b2");
      row?.focus();
      const chord = usesCommandModifier ? { key: "2", code: "Digit2", metaKey: true } : { key: "2", code: "Digit2", ctrlKey: true };
      const held = row ? !press(row, chord) : false;
      const reached = window.__FOCUSED__.splice(0).map((args) => args[1]);
      // A digit past the last coordinator reaches nothing — not a workspace of the branch list behind the team.
      const before = activeWorktreePath;
      const five = usesCommandModifier ? { key: "5", code: "Digit5", metaKey: true } : { key: "5", code: "Digit5", ctrlKey: true };
      const heldFive = row ? !press(row, five) : false;
      await new Promise((done) => setTimeout(done, 50));
      const fifth = { held: heldFive, reached: window.__FOCUSED__.splice(0).length, moved: activeWorktreePath !== before };
      window.focusAgentPane = window.__REAL_FOCUS__;
      return { afterDown, afterTwo, afterUp, stops, kept, entered, held, reached, fifth };
    });
    ok(
      "↓ and ↑ walk the rows with one tab stop among them, a key pressed on a row stays the row's, ⏎ presses it, ⌘2 takes the chord to the second coordinator's pane and ⌘5 reaches nothing behind the team",
      keys.afterDown === "w:run-team-a/w-a2" && keys.afterTwo === "w:run-team-a/w-a1" && keys.afterUp === "w:run-team-a/w-a2" &&
        keys.stops === 1 && keys.kept && JSON.stringify(keys.entered) === JSON.stringify(["tab-9612"]) && keys.held &&
        JSON.stringify(keys.reached) === JSON.stringify(["tab-9602"]) &&
        keys.fifth?.held === true && keys.fifth.reached === 0 && !keys.fifth.moved,
      JSON.stringify(keys),
    );

    /* ⑥ and ⑦ — what a beat writes. */
    const beats = await page.evaluate(async () => {
      const node = window.__WORKER_ROW__("w-a2");
      const dressed = () => window.__TEAM__("sidebarTeamCounts", { dressed: 0 }).dressed;
      let from = dressed();
      await window.__LEDGER_MOVED__();
      const quiet = dressed() - from;
      // An account switch: the worker keeps its id, its pane moves (%3 → %7).
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-a2" ? { ...row, pane: "%7" } : row);
      from = dressed();
      await window.__LEDGER_MOVED__();
      const moved = { written: dressed() - from, same: node !== null && window.__WORKER_ROW__("w-a2") === node,
        pane: window.__READ_WORKER__("w-a2")?.pane ?? null, expected: t("sidebar.team.pane", "판 {{pane}}", { pane: "%7" }) };
      // A status letter: one worker's line moves.
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-a1"
        ? { ...row, last_status: { message_id: "m-w-a1-2", created_ms: Date.now(), line: "초록 시험 돌리는 중" } } : row);
      from = dressed();
      const started = performance.now();
      await window.__LEDGER_MOVED__();
      const letter = { written: dressed() - from, now: window.__READ_WORKER__("w-a1")?.now ?? null, ms: performance.now() - started };
      return { quiet, moved, letter, justNow: t("board.justNow", "방금") };
    });
    ok(
      "a quiet beat writes no row; an account switch moves the pane number on the same row and writes only it; a status letter writes only its worker's row",
      beats.quiet === 0 && beats.moved.written === 1 && beats.moved.same && beats.moved.pane === beats.moved.expected &&
        beats.letter.written === 1 && beats.letter.now === "초록 시험 돌리는 중",
      JSON.stringify(beats),
    );

    /* ⑦ — pointing at a row reads its pane's last three lines, once. */
    const peek = await page.evaluate(async () => {
      const node = window.__WORKER_ROW__("w-c1");
      const asks = () => window.__COUNTS__.term_snapshot ?? 0;
      const from = asks();
      if (!node) return { tip: "", asked: 0 };
      node.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      for (let tries = 0; tries < 50 && !(node.dataset.tip ?? "").includes("마지막 줄"); tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      node.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      await new Promise((done) => setTimeout(done, 50));
      return { tip: node.dataset.tip ?? "", asked: asks() - from };
    });
    const peekLines = peek.tip.split("\n").filter((line) => line.startsWith("9631 "));
    ok(
      "pointing at a worker's row puts its pane's last three lines under the row's own tip, read once for two pointings",
      JSON.stringify(peekLines) === JSON.stringify(["9631 둘째 줄", "9631 셋째 줄", "9631 마지막 줄"]) && peek.asked === 1 &&
        peek.tip.includes("w-c1"),
      JSON.stringify(peek),
    );

    /* ⑦b — a released worker's work that still waits on its coordinator keeps its row; landed work leaves. */
    const settled = await page.evaluate(async () => {
      const left = (over) => window.__ROW_OF__({ run: "run-team-c", agent: "codex", settled: true, reported: true,
        state: "waiting", ledger: "released", pane: "%9", model: "claude-sonnet-5-5", ...over });
      window.__LEDGER__ = [...window.__LEDGER__,
        left({ worker: "w-c2", checkout: "/r/old-1", task: "넘긴 일", task_id: "t-3002",
          handed_in: "c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2", progress: { stage: "report_only" } }),
        left({ worker: "w-c3", checkout: "/r/old-2", task: "착지한 일", task_id: "t-3003",
          handed_in: "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3",
          review: { verified: true, merged: true, deployed: false, written: true, author: "coordinator" },
          progress: { stage: "merged" } })];
      await window.__LEDGER_MOVED__();
      const order = [...document.querySelectorAll("#sidebar-team > .team-row")].map((node) => node.dataset.teamKey);
      const read = { shown: window.__READ_WORKER__("w-c2"), landedGone: window.__WORKER_ROW__("w-c3") === null, order };
      window.__LEDGER__ = window.__LEDGER__.filter((row) => row.worker !== "w-c2" && row.worker !== "w-c3");
      await window.__LEDGER_MOVED__();
      read.afterGone = window.__WORKER_ROW__("w-c2") === null;
      return read;
    });
    ok(
      "a released worker's handed-in work keeps its row under its coordinator — waiting at 검증, with no pane to name — while work that landed leaves the team",
      settled.shown?.dot === "waiting" && settled.shown.pane === "" && settled.shown.cells.join(",") === "on,past,past,on,cur," &&
        settled.landedGone && settled.order.indexOf("w:run-team-c/w-c2") === settled.order.indexOf("c:run-team-c") + 2 &&
        settled.afterGone,
      JSON.stringify(settled),
    );

    /* ⑦c — a worker the ledger could not confirm released (t-60850; the person, 10-16 13:1x: 「코덱스가 돌고 있는데
     * 착지라 표시되는 버그」). Only `released` settles a worker in the ledger's eyes; one whose release got no answer
     * stood under a running coordinator on landed work, every step green at 착지, as if it were going. Landed work
     * leaves the team unless its worker is at work — its pane's hook working, or, with no hook to read, the ledger
     * saying it lives. */
    const unconfirmed = await page.evaluate(async () => {
      const onLanded = (over) => window.__ROW_OF__({ run: "run-team-c", agent: "codex", settled: false, reported: true,
        pane: "%8", model: "gpt-6-astra", handed_in: "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4",
        review: { verified: true, merged: true, deployed: false, written: true, author: "coordinator" },
        progress: { stage: "merged" }, ...over });
      // The worker at work here: seated in a pane whose hook says working.
      const term = 9634;
      tabs.push({ kind: "term", worktree: "/r/old-4", id: `tab-${term}`, layout: { type: "leaf", term } });
      paneAgents.set(term, "codex");
      window.__TELL__("hook:agent", { term, state: "working", agent: "codex", session: `s-${term}`, model: "gpt-6-astra" });
      window.__LEDGER__ = [...window.__LEDGER__,
        // The release was asked for and the answer never came: no pane to hear, the ledger's word `release_unknown`.
        onLanded({ worker: "w-c4", checkout: "/r/old-3", task: "착지한 일, 놓지 못한 워커", task_id: "t-3004",
          state: "waiting", ledger: "release_unknown" }),
        onLanded({ worker: "w-c5", checkout: "/r/old-4", task: "착지한 일, 일하는 워커", task_id: "t-3005", term,
          state: "working", ledger: "active" }),
        // Seated in another window: no hook to read here, and the ledger alone says it lives.
        onLanded({ worker: "w-c6", checkout: "/r/old-5", task: "착지한 일, 다른 창의 워커", task_id: "t-3006",
          state: "working", ledger: "active" }),
      ];
      await window.__LEDGER_MOVED__();
      const order = [...document.querySelectorAll("#sidebar-team > .team-row")].map((node) => node.dataset.teamKey);
      const read = { unknownGone: window.__WORKER_ROW__("w-c4") === null, rows: window.__ROWS__(), order,
        here: window.__READ_WORKER__("w-c5"), elsewhere: window.__READ_WORKER__("w-c6") };
      window.__LEDGER__ = window.__LEDGER__.filter((row) => !["w-c4", "w-c5", "w-c6"].includes(row.worker));
      tabs.splice(tabs.findIndex((tab) => tab.id === `tab-${term}`), 1);
      paneAgents.delete(term);
      await window.__LEDGER_MOVED__();
      read.after = window.__ROWS__();
      return read;
    });
    ok(
      "a worker whose release got no answer leaves the team with its landed work — not a row of six green steps under a running coordinator — and the other rows stand as they were",
      unconfirmed.unknownGone && !unconfirmed.order.includes("w:run-team-c/w-c4") && unconfirmed.rows.workers === 7 &&
        unconfirmed.after.workers === 5,
      JSON.stringify(unconfirmed),
    );
    ok(
      "a worker at work keeps its row on landed work — its pane's hook working here, or the ledger alone saying it lives elsewhere — drawn working with every step at 착지, standing where the ledger's order puts it (after the older row, not lifted over it)",
      unconfirmed.here?.dot === "working" && unconfirmed.here.cells.join(",") === "on,past,past,on,on,on" &&
        unconfirmed.here.step === words.land && unconfirmed.elsewhere?.dot === "working" &&
        unconfirmed.elsewhere.cells.join(",") === "on,past,past,on,on,on" &&
        unconfirmed.order.indexOf("w:run-team-c/w-c5") > unconfirmed.order.indexOf("w:run-team-c/w-c1") &&
        unconfirmed.order.indexOf("w:run-team-c/w-c6") > unconfirmed.order.indexOf("w:run-team-c/w-c5"),
      JSON.stringify(unconfirmed),
    );

    /* ⑧ — the landing line opens what waits to land. */
    const landed = await page.evaluate(async () => {
      document.querySelector("#sidebar-team > .team-landing")?.click();
      for (let tries = 0; tries < 50 && !boardTab(); tries += 1) await new Promise((done) => setTimeout(done, 20));
      return { board: Boolean(boardTab()), stage: deskChoice.stage, mode: agentBoardMode };
    });
    ok(
      "the landing line opens the task board on the tasks that wait to land",
      landed.board && landed.stage === "verified" && landed.mode === "tasks",
      JSON.stringify(landed),
    );

    /* ⑨ — the branch list one press away, stored; and back. */
    const toggled = await page.evaluate(async () => {
      const rows = window.__ROWS__;
      document.querySelector("#sidebar-team .team-toggle")?.click();
      for (let tries = 0; tries < 100 && document.querySelectorAll("#worktrees .wt-row").length === 0; tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      await window.__PAINTED__();
      const branches = { layout: window.__TEAM__("sidebarLayout"), written: window.__VIEW_WRITES__.at(-1) ?? null, rows: rows(),
        teamHidden: document.getElementById("sidebar-team")?.hidden === true,
        switchShown: document.querySelector(".team-switch")?.hidden === false, teamNodes: document.querySelectorAll("#sidebar-team *").length };
      document.querySelector(".team-switch")?.click();
      for (let tries = 0; tries < 100 && !window.__TEAM__("sidebarTeamStood", false); tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      await window.__TEAM_SETTLED__();
      const team = { layout: window.__TEAM__("sidebarLayout"), written: window.__VIEW_WRITES__.at(-1) ?? null, rows: rows() };
      // A restart: the stored view comes back through the settings snapshot.
      applySettingsSnapshot({ sidebar_view: { group_by: "repo", sort_by: "default", project_order: "default", layout: "branches" } });
      for (let tries = 0; tries < 100 && document.querySelectorAll("#worktrees .wt-row").length === 0; tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      const restored = { layout: window.__TEAM__("sidebarLayout"), rows: rows() };
      applySettingsSnapshot({ sidebar_view: { group_by: "repo", sort_by: "default", project_order: "default", layout: "team" } });
      await window.__TEAM_SETTLED__();
      return { branches, team, restored };
    });
    ok(
      "「브랜치 보기」 brings the branch list back — twelve rows, the worker's red copy folded under its row — and stores the choice; the list's switch brings the team back the same way; a restart's snapshot restores it",
      toggled.branches.layout === "branches" && toggled.branches.written?.layout === "branches" &&
        toggled.branches.written?.groupBy === "repo" && toggled.branches.rows.branches === 12 &&
        toggled.branches.teamHidden && toggled.branches.switchShown && toggled.branches.teamNodes === 0 &&
        toggled.team.layout === "team" && toggled.team.written?.layout === "team" && toggled.team.rows.team === 9 &&
        toggled.restored.layout === "branches" && toggled.restored.rows.branches === 12,
      JSON.stringify(toggled),
    );
    /* The numbers the report quotes: the same scene, read as the branch list and as the team. */
    ok(
      "the numbers: rows a person reads, team against branches, and the sidebar's elements",
      toggled.team.rows.coordinators + toggled.team.rows.workers === 8 && toggled.branches.rows.branches === 12,
      JSON.stringify({ team: toggled.team.rows, branches: toggled.branches.rows }),
    );

    /* ⑪ — t-69010: a worker whose turn failed on an error its CLI named is held, and its row says the failure where
     * its last status stood: 「턴 실패 — <the error>」. Before this a zo turn that died on a provider silence reached
     * nobody as anything but quiet. The failure leaves the row once the ledger's row no longer carries it. */
    const failedTurn = await page.evaluate(async (now) => {
      setSidebarView({ layout: "team" });
      for (let tries = 0; tries < 100 && !window.__TEAM__("sidebarTeamStood", false); tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      const error = "api failed after 3 attempts: stream_startup_no_progress";
      const moved = async (over) => {
        window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-a1" ? { ...row, ...over } : row);
        await window.__LEDGER_MOVED__();
      };
      await moved({ turn_failed: { error, at_ms: now - 60_000 } });
      const failed = window.__READ_WORKER__("w-a1");
      await moved({ turn_failed: undefined });
      const back = window.__READ_WORKER__("w-a1");
      return {
        failed, back, error,
        word: t("sidebar.team.turnFailed", "턴 실패"),
        line: t("sidebar.team.turnFailedLine", "턴 실패 — {{error}}", { error }),
      };
    }, now);
    ok(
      "a worker whose turn failed is held — 턴 실패 on its chip, its row's sentence is the failure and the error's own first words — and the row goes back to its own words once the ledger stops carrying the failure",
      failedTurn.failed?.dot === "blocked" && failedTurn.failed?.chip === failedTurn.word &&
        failedTurn.failed?.now.startsWith(failedTurn.line.slice(0, 20)) &&
        failedTurn.back?.chip !== failedTurn.word && !failedTurn.back?.now.includes(failedTurn.word),
      JSON.stringify(failedTurn),
    );

    /* ⑩ — the new words in every catalogue. */
    const langs = await page.evaluate(() => {
      const keys = Object.keys(CATALOG.en).filter((key) => key.startsWith("sidebar.team."));
      const found = { count: keys.length };
      for (const code of ["en", "ja", "zh", "es"]) {
        found[code] = keys.filter((key) => !Object.hasOwn(CATALOG[code], key) || CATALOG[code][key] === "");
      }
      return found;
    });
    ok(
      "every word of the team view reads in English, Japanese, Chinese and Spanish",
      langs.count >= 30 && ["en", "ja", "zh", "es"].every((code) => langs[code].length === 0),
      JSON.stringify(langs),
    );
  } finally {
    await page.close().catch(() => {});
  }

  /* ⑪ — nobody sitting: nothing is read for the team, and the branch list is the list. */
  const idle = await openWindowTestPage(browser, origin, { faults });
  try {
    await idle.page.setViewportSize({ width: 1280, height: 1600 });
    await standTeamScene(idle.page, { now, seated: false });
    const quiet = await idle.page.evaluate(async () => {
      const rows = window.__ROWS__;
      const seats = () => window.__COUNTS__.coordinator_seats ?? 0;
      const reads = () => window.__TEAM__("sidebarTeamCounts", { reads: null }).reads;
      const from = { seats: seats(), reads: reads() };
      for (let beat = 0; beat < 3; beat += 1) await window.__LEDGER_MOVED__();
      return {
        seatAsks: seats() - from.seats, reads: reads(), stood: window.__TEAM__("sidebarTeamStood", false), rows: rows(),
        switchShown: document.querySelector(".team-switch")?.hidden === false,
        teamNodes: document.querySelectorAll("#sidebar-team *").length,
      };
    });
    ok(
      "with no coordinator sitting the team reads nothing — the seats are asked once a beat, by the navigator as before — and the branch list stands as it was",
      quiet.seatAsks === 3 && quiet.reads === 0 && !quiet.stood && quiet.rows.branches === 12 && !quiet.switchShown &&
        quiet.teamNodes === 0,
      JSON.stringify(quiet),
    );
  } finally {
    await idle.page.close().catch(() => {});
  }
}

/* Workers on one piece of work, as one row (t-60498; the person, 10-16 10:4x: 「같은 업무인데 여러 행·내용 바뀌어 헷갈림」,
 * and 10-25 20:5x: 「추천안대로 진행해」 over the sketch p-94e57b887379e021).
 *
 * One coordinator, six workers: four on tasks that share a root task («대화 화면 개선»), one on a task that is its own
 * root, one with no root at all.
 *   ① workers whose tasks share a root stand under one head row with a count and a one-line tally, closed to begin
 *      with; a worker alone on its root, and one with none, stay plain rows;
 *   ② opened, the family's rows keep the ledger's order whatever their states do; a question lifts the family
 *      among the rows — the head shows 「질문 N」 — and never a row inside it;
 *   ③ the line under a worker is always there, holds one height before and after the worker speaks, and a minute
 *      passing writes no word and no row; a sentence that changed is lit for 1.6 s;
 *   ④ eight beats of a ledger, a minute apart, counted: the words that changed, the rows that moved, the rows that
 *      changed height — against the same eight beats of the sketch's base commit (b428c18bc): 59 words, 9 moves,
 *      2 height changes;
 *   ⑤ every new word reads in the four other catalogues. */
export const FAMILY_RUN = Object.freeze({ run: "run-fam", name: "묶음 런", generation: 1, term: 9601, path: "/r/main", agent: "claude",
  model: "claude-opus-5-5", mail: 0, news: 0, landing: 0, batched: 0 });
const FAMILY_ROOT = Object.freeze({ root_id: "t-9100", root_title: "대화 화면 개선" });
export const FAMILY_WORKERS = Object.freeze([
  { run: "run-fam", worker: "w-f1", term: 9611, path: "/r/wt-f1", pane: "%5", agent: "claude", hook: "working",
    task: "입력창 고치기", task_id: "t-9101", root: FAMILY_ROOT, landing: { state: "unlanded", ahead: 2 },
    status: { minutes: 2, line: "입력창 높이를 재는 중. 표로 정리한다" }, ledger: { progress: { stage: "working" } } },
  { run: "run-fam", worker: "w-f2", term: 9612, path: "/r/wt-f2", pane: "%6", agent: "claude", hook: "working",
    task: "목록 줄 맞추기", task_id: "t-9102", root: FAMILY_ROOT, landing: { state: "unlanded", ahead: 1 },
    status: null, ledger: { progress: { stage: "working" } } },
  { run: "run-fam", worker: "w-f3", term: 9613, path: "/r/wt-f3", pane: "%7", agent: "claude", hook: "done",
    task: "문구 정리", task_id: "t-9103", root: FAMILY_ROOT, landing: { state: "unlanded", ahead: 3 },
    status: { minutes: 6, line: "넘김 끝내고 코디 검증을 기다리는 중" },
    ledger: { reported: true, handed_in: "f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3", progress: { stage: "report_only" } } },
  { run: "run-fam", worker: "w-f4", term: 9614, path: "/r/wt-f4", pane: "%8", agent: "claude", hook: "working",
    task: "색 고치기", task_id: "t-9104", root: FAMILY_ROOT, landing: { state: "unlanded", ahead: 1 },
    status: null, ledger: { progress: { stage: "working" } } },
  { run: "run-fam", worker: "w-g1", term: 9621, path: "/r/wt-g1", pane: "%9", agent: "claude", hook: "working",
    task: "알림 소리 고르기", task_id: "t-9200", root: { root_id: "t-9200", root_title: "알림 소리 고르기" },
    landing: { state: "unlanded", ahead: 1 },
    status: { minutes: 3, line: "소리 후보 셋을 비교하는 중" }, ledger: { progress: { stage: "working" } } },
  { run: "run-fam", worker: "w-h1", term: 9622, path: "/r/wt-h1", pane: "%4", agent: "claude", hook: "working",
    task: "설정 문구 고치기", task_id: "t-9300", landing: { state: "unlanded", ahead: 1 },
    status: null, ledger: { progress: { stage: "working" } } },
]);
const FAMILY_COPIES = Object.freeze(["/r/wt-f1-verify", "/r/wt-f1-windows", "/r/wt-g1-red", "/r/old-1"]);

/* One worker's state moved, through the window's own roads: its pane's hook and the ledger's row. */
const FAMILY_MOVES = `
  window.__MOVE__ = (worker, state) => {
    const row = window.__LEDGER__.find((one) => one.worker === worker);
    row.asking = state === "asking";
    row.reported = state === "waiting";
    row.handed_in = state === "waiting" ? "ab".repeat(20) : null;
    row.progress = { stage: state === "waiting" ? "report_only" : "working" };
    const hook = state === "working" ? "working" : "done";
    for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
      handler({ payload: { term: row.term, state: hook, agent: row.agent, session: "s-" + row.term, model: "claude-sonnet-5-5" } });
    }
  };
  window.__TEAM_KEYS__ = () => [...document.querySelectorAll("#sidebar-team > .team-row")].map((node) => node.dataset.teamKey);
`;

/* Eight beats of a ledger, a minute apart — what a person's eye would have to follow in the team view: the rows whose
 * words changed, the rows that moved, the rows that changed height. `open` stands the family open first. The same
 * script reads the base commit's window (no family rows there), so the before and the after are one measurement
 * (`sidebar-team-perf.mjs`). */
const FAMILY_BEATS = [
  `move("w-f4", "asking"); say("w-f1", "표를 정리해 보고서에 넣는 중. 거의 끝");`,
  `say("w-f2", "목록 줄 높이를 맞추는 중");`,
  `move("w-f3", "working"); say("w-f3", "검증 지적 두 건을 고치는 중");`,
  `say("w-f1", "시험을 돌리는 중. 3분쯤 걸린다");`,
  `move("w-f4", "working"); say("w-f4", "답을 받아 색을 바꾸는 중");`,
  `say("w-f2", "목록 시험 통과. 넘길 준비"); say("w-h1", "문구 후보를 고르는 중");`,
  `move("w-f3", "waiting"); say("w-f3", "고쳐서 다시 넘김");`,
  `say("w-f1", "입력창 단계 끝. 넘김을 쓰는 중");`,
];

export async function playTeamBeats(page, { open = false } = {}) {
  await page.evaluate(FAMILY_MOVES);
  await page.evaluate(async (open) => {
    for (const node of document.querySelectorAll("#sidebar-team .team-group")) {
      if ((node.getAttribute("aria-expanded") === "true") !== open) node.click();
    }
    await window.__TEAM_SETTLED__();
    const real = Date.now.bind(Date);
    window.__SKEW__ = 0;
    Date.now = () => real() + window.__SKEW__;
    window.__SNAP__ = () => [...document.querySelectorAll("#sidebar-team > .team-row")]
      .filter((node) => !node.hidden && node.getClientRects().length > 0)
      .map((node) => ({
        key: node.dataset.teamKey,
        text: [...node.querySelectorAll("*")].filter((part) => part.children.length === 0 && !part.hidden && part.textContent.trim() !== "")
          .map((part) => part.textContent.trim()).join("|"),
        height: Math.round(node.getBoundingClientRect().height * 10) / 10,
      }));
  }, open);
  const total = { words: 0, clock: 0, moved: 0, tall: 0, rows: 0, dressed: 0, built: 0, beats: [] };
  let before = await page.evaluate(() => window.__SNAP__());
  total.rows = before.length;
  const counts = () => page.evaluate(() => window.__TEAM__("sidebarTeamCounts", { dressed: 0, built: 0 }));
  const from = await counts();
  for (let at = 0; at < FAMILY_BEATS.length; at += 1) {
    await page.evaluate(async (body) => {
      window.__SKEW__ += 60_000;
      const now = Date.now();
      const say = (worker, line) => {
        const row = window.__LEDGER__.find((one) => one.worker === worker);
        row.last_status = { message_id: `m-${worker}-${now}`, created_ms: now, line };
      };
      const move = window.__MOVE__;
      // eslint-disable-next-line no-new-func
      new Function("say", "move", body)(say, move);
      await window.__LEDGER_MOVED__();
      // The window's own minute beat.
      if (typeof paintSidebarTeam === "function") paintSidebarTeam();
    }, FAMILY_BEATS[at]);
    await page.waitForTimeout(80);
    const after = await page.evaluate(() => window.__SNAP__());
    const was = new Map(before.map((row, index) => [row.key, { ...row, index }]));
    const beat = { words: 0, clock: 0, moved: 0, tall: 0 };
    after.forEach((row, index) => {
      const old = was.get(row.key);
      if (!old) return;
      if (old.text !== row.text) {
        beat.words += 1;
        const bare = (text) => text.replace(/\d+\s*(?:분|초|시간) 전|방금/g, "");
        if (bare(old.text) === bare(row.text)) beat.clock += 1;
      }
      if (old.index !== index) beat.moved += 1;
      if (old.height !== row.height) beat.tall += 1;
    });
    for (const name of ["words", "clock", "moved", "tall"]) total[name] += beat[name];
    total.beats.push(beat);
    before = after;
  }
  const to = await counts();
  total.dressed = to.dressed - from.dressed;
  total.built = to.built - from.built;
  return total;
}

export async function testSidebarTeamFamilies({ browser, origin, ok, faults }) {
  const now = Date.now();
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await standTeamScene(page, { now, runs: [FAMILY_RUN], workers: FAMILY_WORKERS, extras: FAMILY_COPIES });
    await page.evaluate(FAMILY_MOVES);
    const words = await page.evaluate(() => ({
      count4: t("sidebar.team.groupCount", "워커 {{count}}", { count: 4 }),
      ask1: t("sidebar.team.questions", "질문 {{count}}", { count: 1 }),
      working: t("sidebar.team.stateWorking", "일하는 중"),
      waiting: t("sidebar.team.stateWaiting", "기다림"),
      nowNone: t("sidebar.team.nowNone", "아직 보낸 말 없음"),
    }));
    const head = () => page.evaluate(() => {
      const node = document.querySelector('#sidebar-team [data-team-key="g:run-fam/t-9100"]');
      if (!node) return null;
      const text = (selector) => {
        const part = node.querySelector(selector);
        return part && !part.hidden ? part.textContent : "";
      };
      return { title: text(".team-title"), count: text(".team-pane"), tally: text(".team-now"), chip: text(".team-chip"),
        expanded: node.getAttribute("aria-expanded"), tip: node.dataset.tip ?? "" };
    });

    /* ① — one head row, closed. */
    const closed = await page.evaluate(() => ({ order: window.__TEAM_KEYS__(), inside: document.querySelectorAll("#sidebar-team .team-worker.is-child").length,
      f1: document.querySelector('#sidebar-team [data-team-key="w:run-fam/w-f1"]') !== null }));
    const closedHead = await head();
    ok(
      "workers whose tasks share a root stand under one head row with a count and a one-line tally, closed to begin with; a worker alone on its root and one with no root stay plain rows",
      JSON.stringify(closed.order) === JSON.stringify(["c:run-fam", "g:run-fam/t-9100", "w:run-fam/w-g1", "w:run-fam/w-h1", "landing"]) &&
        closedHead?.title === "대화 화면 개선" && closedHead.count === words.count4 &&
        closedHead.tally === `${words.working} 3 · ${words.waiting} 1` && closedHead.chip === "" && closedHead.expanded === "false" &&
        closed.inside === 0 && !closed.f1,
      JSON.stringify({ closed, closedHead }),
    );

    /* ② — opened: the ledger's order, whatever the states do. */
    const opened = await page.evaluate(async () => {
      document.querySelector('#sidebar-team [data-team-key="g:run-fam/t-9100"]')?.click();
      await window.__TEAM_SETTLED__();
      const out = { open: window.__TEAM_KEYS__() };
      window.__MOVE__("w-f1", "waiting");
      window.__MOVE__("w-f3", "working");
      await window.__LEDGER_MOVED__();
      out.swapped = window.__TEAM_KEYS__();
      window.__MOVE__("w-h1", "asking");
      await window.__LEDGER_MOVED__();
      out.lone = window.__TEAM_KEYS__();
      window.__MOVE__("w-f2", "asking");
      await window.__LEDGER_MOVED__();
      out.family = window.__TEAM_KEYS__();
      out.chip = document.querySelector('#sidebar-team [data-team-key="g:run-fam/t-9100"] .team-chip')?.textContent ?? "";
      window.__MOVE__("w-f2", "working");
      window.__MOVE__("w-h1", "working");
      window.__MOVE__("w-f1", "working");
      window.__MOVE__("w-f3", "waiting");
      await window.__LEDGER_MOVED__();
      out.settled = window.__TEAM_KEYS__();
      return out;
    });
    const standing = ["c:run-fam", "g:run-fam/t-9100", "w:run-fam/w-f1", "w:run-fam/w-f2", "w:run-fam/w-f3", "w:run-fam/w-f4",
      "w:run-fam/w-g1", "w:run-fam/w-h1", "landing"];
    ok(
      "inside an opened family the rows keep the ledger's order — a worker going to rest or to work moves nothing — and a question lifts the family among the rows, or a lone row, and never a row inside the family",
      JSON.stringify(opened.open) === JSON.stringify(standing) && JSON.stringify(opened.swapped) === JSON.stringify(standing) &&
        JSON.stringify(opened.lone) === JSON.stringify(["c:run-fam", "w:run-fam/w-h1", "g:run-fam/t-9100", "w:run-fam/w-f1",
          "w:run-fam/w-f2", "w:run-fam/w-f3", "w:run-fam/w-f4", "w:run-fam/w-g1", "landing"]) &&
        JSON.stringify(opened.family) === JSON.stringify(["c:run-fam", "g:run-fam/t-9100", "w:run-fam/w-f1", "w:run-fam/w-f2",
          "w:run-fam/w-f3", "w:run-fam/w-f4", "w:run-fam/w-h1", "w:run-fam/w-g1", "landing"]) && opened.chip === words.ask1 &&
        JSON.stringify(opened.settled) === JSON.stringify(standing),
      JSON.stringify(opened),
    );

    /* ②b — the keyboard: the head is a row the arrows walk, and a press on it opens or folds the family with the focus
     * staying on the head. */
    const keys = await page.evaluate(async () => {
      const key = (target, init) => target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
      const headNode = () => document.querySelector('#sidebar-team [data-team-key="g:run-fam/t-9100"]');
      headNode().focus();
      key(document.activeElement, { key: "ArrowDown" });
      const down = document.activeElement?.dataset.teamKey ?? null;
      key(document.activeElement, { key: "ArrowUp" });
      const up = document.activeElement?.dataset.teamKey ?? null;
      headNode().click();
      await window.__TEAM_SETTLED__();
      const folded = { expanded: headNode().getAttribute("aria-expanded"), focus: document.activeElement === headNode(),
        rows: window.__TEAM_KEYS__().length };
      headNode().click();
      await window.__TEAM_SETTLED__();
      return { down, up, folded, reopened: headNode().getAttribute("aria-expanded"), rows: window.__TEAM_KEYS__().length,
        stops: [...document.querySelectorAll("#sidebar-team > .team-row")].filter((node) => node.tabIndex === 0).length };
    });
    ok(
      "↓ from the head walks into the family's first row and ↑ comes back; a press folds the family and opens it again with the focus left on the head, and one row holds the tab stop throughout",
      keys.down === "w:run-fam/w-f1" && keys.up === "g:run-fam/t-9100" && keys.folded.expanded === "false" && keys.folded.focus &&
        keys.folded.rows === 5 && keys.reopened === "true" && keys.rows === 9 && keys.stops === 1,
      JSON.stringify(keys),
    );

    /* ③ — the line under a worker. */
    const line = await page.evaluate(async () => {
      const node = document.querySelector('#sidebar-team [data-team-key="w:run-fam/w-h1"]');
      const read = () => ({ height: node.getBoundingClientRect().height, now: node.querySelector(".team-now").textContent,
        shown: !node.querySelector(".team-now").hidden && node.querySelector(".team-now").getClientRects().length > 0,
        idle: node.querySelector(".team-now").classList.contains("is-idle") });
      const before = read();
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-h1"
        ? { ...row, last_status: { message_id: "m-h1-1", created_ms: Date.now(), line: "문구를 바꾸는 중. 곧 끝난다" } } : row);
      const spokeAt = performance.now();
      await window.__LEDGER_MOVED__();
      const spoke = read();
      const fresh = node.querySelector(".team-now").classList.contains("is-fresh");
      // The light goes out on its own; the clock below is read after it, so it is the only thing that could write.
      while (document.querySelector("#sidebar-team .team-now.is-fresh") && performance.now() - spokeAt < 8000) {
        await new Promise((done) => setTimeout(done, 25));
      }
      const lit = { stayedMs: performance.now() - spokeAt, gone: document.querySelector("#sidebar-team .team-now.is-fresh") === null };
      // Minutes pass with nothing said: the rows' clock beat only.
      const real = Date.now.bind(Date);
      window.__SKEW__ = 0;
      Date.now = () => real() + window.__SKEW__;
      const dressed = () => window.__TEAM__("sidebarTeamCounts", { dressed: 0 }).dressed;
      const words = () => [...document.querySelectorAll("#sidebar-team .team-now")].map((part) => part.textContent).join("|");
      const from = { dressed: dressed(), words: words() };
      let changed = 0;
      const watch = new MutationObserver((records) => { changed += records.length; });
      // What a person reads: words, tips, labels and what is shown. A working dot's ring turns on its own beat
      // (`spinWorktreeDots`) and is not a word the clock wrote.
      watch.observe(document.getElementById("sidebar-team"), { subtree: true, childList: true, characterData: true, attributes: true,
        attributeFilter: ["data-tip", "aria-label", "aria-expanded", "hidden", "data-said-ms"] });
      for (let minute = 0; minute < 8; minute += 1) {
        window.__SKEW__ += 60_000;
        paintSidebarTeam();
      }
      await new Promise((done) => setTimeout(done, 30));
      watch.disconnect();
      Date.now = real;
      // The tip tells how long ago, when pointed at.
      const sayer = document.querySelector('#sidebar-team [data-team-key="w:run-fam/w-f1"] .team-now');
      sayer.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      return { before, spoke, fresh, lit, clock: { dressed: dressed() - from.dressed, same: words() === from.words, mutations: changed },
        tip: sayer.dataset.tip };
    });
    ok(
      "the line under a worker holds one height before and after it speaks — a worker that said nothing keeps the line, dimmed, saying so — and the sentence that changed is lit",
      line.before.shown && line.before.idle && line.before.now === words.nowNone && line.spoke.shown && !line.spoke.idle &&
        line.spoke.now === "문구를 바꾸는 중." && line.spoke.height === line.before.height && line.fresh,
      JSON.stringify(line),
    );
    ok(
      "a minute passing writes no word and no row — eight minutes of the clock beat change nothing in the team — and the line's tip says how long ago when pointed at",
      line.clock.dressed === 0 && line.clock.same && line.clock.mutations === 0 &&
        line.tip.includes("입력창 높이를 재는 중.") && /(분|시간) 전|방금/.test(line.tip),
      JSON.stringify(line),
    );
    ok("the light on a changed sentence stays about 1.6 s and then goes out on its own", line.lit.gone && line.lit.stayedMs >= 1500,
      JSON.stringify(line.lit));

    /* ④ — eight beats, closed and open. The sketch's base commit measured this on the same beats. */
    const closedBeats = await playTeamBeats(page, { open: false });
    ok(
      "eight beats of the ledger, a minute apart, move no row and change no row's height — closed or open — and the clock writes no word",
      closedBeats.moved === 0 && closedBeats.tall === 0 && closedBeats.clock === 0,
      JSON.stringify(closedBeats),
    );
    const openBeats = await playTeamBeats(page, { open: true });
    ok(
      "opened, the same eight beats change only the sentences that were said and the head's tally — no row moves, no row grows, no clock writes a word",
      openBeats.moved === 0 && openBeats.tall === 0 && openBeats.clock === 0,
      JSON.stringify({ closed: closedBeats, open: openBeats }),
    );

    /* ⑤ — the new words. */
    const langs = await page.evaluate(() => {
      const KEYS = {
        "sidebar.team.groupCount": "워커 {{count}}",
        "sidebar.team.groupTip": "같은 일에서 갈라진 과업 {{count}}개 — 누르면 펼치거나 접습니다",
        "sidebar.team.nowNone": "아직 보낸 말 없음",
      };
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { persist: false });
        found[code] = Object.entries(KEYS).map(([key, fallback]) => ({ key, fallback: t(key, fallback) === fallback }));
      }
      setLocale("ko", { persist: false });
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.filter((one) => one.fallback).map((one) => `${code}:${one.key}`));
    ok("every new word of the family rows reads in English, Japanese, Chinese and Spanish, not in the Korean fallback", missing.length === 0,
      JSON.stringify({ missing }));
  } finally {
    await page.close().catch(() => {});
  }
}
