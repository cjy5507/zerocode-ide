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
 *   ⑧ the landing line counts what waits to land and opens that list on the task board;
 *   ⑨ 「가지별 보기」 brings the branch list back, stored, and the list's own switch brings the team back;
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
export async function standTeamScene(page, { layout = null, seated = true, now = Date.now() } = {}) {
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async ({ runs, workers, extras, layout, seated, now }) => {
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
    window.__ANSWER__.project_catalog = () => [{
      name: "r", path: "/r",
      external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
      worktrees: paths.map((path, at) => {
        const landing = landingOf(path);
        return {
          path, branch: path === "/r/main" ? "main" : `wt/${path.slice(3)}`, is_main: path === "/r/main",
          active: path === window.__ACTIVE__, is_folder: false, ownership: "zerocode-managed", external_hidden: false,
          last_activity_ms: at,
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
  }, { runs: TEAM_RUNS, workers: TEAM_WORKERS, extras: TEAM_EXTRA_CHECKOUTS, layout, seated, now });
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
          dot: [...node.querySelector(".team-dot").classList].find((name) => name.startsWith("is-"))?.slice(3) ?? "",
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
      walled: t("board.desk.healthWalled", "한도 벽"),
      asking: t("board.desk.healthAsking", "답 기다림"),
      commit2: t("sidebar.team.step.commitCount", "커밋 {{count}}", { count: 2 }),
      read: t("sidebar.team.step.read", "읽기"),
      verify: t("sidebar.team.step.verify", "검증"),
      land: t("sidebar.team.step.land", "착지"),
      commit1: t("sidebar.team.step.commitCount", "커밋 {{count}}", { count: 1 }),
      ago2: t("sidebar.team.ago", "{{time}} 전", { time: agoWord(now - 2 * 60_000, Date.now()) }),
      checkFailed: t("board.desk.landCheckFailed", "점검 실패 (rc {{rc}})", { rc: 2 }),
      pane5: t("sidebar.team.pane", "판 {{pane}}", { pane: "%5" }),
    }), now);
    const cells = (worker) => read[worker]?.cells.join(",");
    ok(
      "each worker's six steps say what the ledger and git hold: a worker with commits is at 커밋, a blocked one is held where it stands, a hand-in waits at 검증 with its failed check held, verified work waits at 착지 — and a step with no record stays empty",
      cells("w-a1") === "on,cur,,,," && read["w-a1"].step === words.commit2 &&
        cells("w-a2") === "stuck,,,,," && read["w-a2"].step === words.read &&
        cells("w-b1") === "on,stuck,,,," && read["w-b1"].step === words.commit1 &&
        cells("w-b2") === "on,on,stuck,on,cur," && read["w-b2"].step === words.verify &&
        read["w-b2"].stepsTip.includes(words.checkFailed) &&
        cells("w-c1") === "on,on,,on,on,cur" && read["w-c1"].step === words.land,
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
    ok(
      "the line under a worker is the first sentence of its last status — at most thirty characters — and when it was said; a worker that said nothing has no line; the title is the task and the pane is the ledger's",
      read["w-a1"].now === `세 흐름 측정 결과 읽는 중. · ${words.ago2}` &&
        [...read["w-b2"].now.split(" · ")[0]].length === 30 && read["w-b2"].now.split(" · ")[0].endsWith("…") &&
        read["w-b1"].now === "" && read["w-a1"].title === "대화 화면 2단계" && read["w-a1"].pane === words.pane5 &&
        !read["w-a1"].title.includes("wt/"),
      JSON.stringify(read),
    );

    /* The coordinator cards: who, what owes, and the landing line. */
    const cards = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll("#sidebar-team > .team-coordinator")]
      .map((node) => [node.dataset.teamKey, {
        name: node.querySelector(".team-name")?.textContent ?? "",
        run: node.querySelector(".team-run")?.hidden ? "" : node.querySelector(".team-run")?.textContent ?? "",
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
        beats.letter.written === 1 && beats.letter.now === `초록 시험 돌리는 중 · ${beats.justNow}`,
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
      settled.shown?.dot === "waiting" && settled.shown.pane === "" && settled.shown.cells.join(",") === "on,,,on,cur," &&
        settled.landedGone && settled.order.indexOf("w:run-team-c/w-c2") === settled.order.indexOf("c:run-team-c") + 2 &&
        settled.afterGone,
      JSON.stringify(settled),
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
      "「가지별 보기」 brings the branch list back — thirteen rows — and stores the choice; the list's switch brings the team back the same way; a restart's snapshot restores it",
      toggled.branches.layout === "branches" && toggled.branches.written?.layout === "branches" &&
        toggled.branches.written?.groupBy === "repo" && toggled.branches.rows.branches === 13 &&
        toggled.branches.teamHidden && toggled.branches.switchShown && toggled.branches.teamNodes === 0 &&
        toggled.team.layout === "team" && toggled.team.written?.layout === "team" && toggled.team.rows.team === 9 &&
        toggled.restored.layout === "branches" && toggled.restored.rows.branches === 13,
      JSON.stringify(toggled),
    );
    /* The numbers the report quotes: the same scene, read as the branch list and as the team. */
    ok(
      "the numbers: rows a person reads, team against branches, and the sidebar's elements",
      toggled.team.rows.coordinators + toggled.team.rows.workers === 8 && toggled.branches.rows.branches === 13,
      JSON.stringify({ team: toggled.team.rows, branches: toggled.branches.rows }),
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
      quiet.seatAsks === 3 && quiet.reads === 0 && !quiet.stood && quiet.rows.branches === 13 && !quiet.switchShown &&
        quiet.teamNodes === 0,
      JSON.stringify(quiet),
    );
  } finally {
    await idle.page.close().catch(() => {});
  }
}
