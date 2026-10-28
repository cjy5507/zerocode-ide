import { openWindowTestPage } from "./window-boot.mjs";

/* 코디 판이 「갑자기 꺼짐」 (t-67936).
 *
 * 2026-10-21 13:2x the person: 「계속 꺼졋어 갑자기」. Two things had ended
 * their coordinators with no word on the screen. At 13:24:35 one tab the 12:37
 * restart had put back (`term:asleep:4`) closed by close-tab with two runs'
 * coordinators in it — the question before a close asks about a foreground
 * process alone, and this person had told it not to ask again — and six
 * workers lost their coordinator. At 05:05 and 12:37 the window restarted by
 * its own restart road, at a gap nobody was watching for.
 *
 * Two scenes, each on its own page and built from the window's own doors and a
 * fake ledger — nobody's pane, conversation or file is read:
 *
 *   close-guard     ① a tab holding two coordinators says so on its tip —
 *                      both seats and its folder (its name no longer leads
 *                      with 「코디 2 ·」, t-78107); a restored tab not
 *                      woken yet says it holds conversations to wake;
 *                   ② the person's × on it asks one line naming the seats —
 *                      with the running switch OFF, as theirs was — and
 *                      「취소」 keeps every pane;
 *                   ③ a click no hand made is refused outright, and said;
 *                   ④ 「탭 닫기」 ends both panes;
 *                   ⑤ a worker's tab asks the same way, a plain shell and a
 *                      tab not woken yet close without a word;
 *                   ⑥ every close leaves one black-box line — road, hand,
 *                      pointer, Computer Use, the seats by id, the outcome —
 *                      and no word a person wrote;
 *                   ⑦ the new words read in four catalogs.
 *   restart-notice  ① a restart door goes by the way out's notice, never the
 *                      restart road itself: 「10초 뒤 1.1.86(으)로 다시
 *                      시작합니다 — 취소」, the person's last key reported;
 *                   ② the beat's next line is said in place, and a key held
 *                      while it stands is said to the backend once a line;
 *                   ③ 「취소」 takes the line down and tells the backend;
 *                   ④ 「지금 다시 시작」 after the census's question also goes
 *                      by the notice;
 *                   ⑤ the window a restart started says which build it came
 *                      back as, once;
 *                   ⑥ the new words read in four catalogs.
 *
 * `ok` details carry the numbers the report quotes: questions asked before a
 * seat closed, panes ended, restarts gone with no notice. */

const SEATS = [
  { run: "run-4275", name: "ZO", generation: 3, term: 9601 },
  { run: "run-11955", name: "옆 런", generation: 2, term: 9602 },
];
const WORKER = { term: 9603, worker: "w-9603", task: "닫기 묻기 과업", task_id: "t-9603" };
const PERSON = "사람이-쓴-글";

export async function testCloseGuard({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForFunction(() => projectsRead);

    /* ---- the scene: two coordinators split in one tab, a worker, a shell, a tab not woken ---- */
    const ids = await page.evaluate(async ({ seats, worker, person }) => {
      setLocale("ko", { persist: false });
      const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
      const A = activeWorktreePath;
      window.__LINES__ = [];
      window.__ANSWER__.log_window_error = (args) => {
        window.__LINES__.push(String(args?.message ?? ""));
        return null;
      };
      window.__ENDED__ = [];
      window.__ANSWER__.close_term = (args) => {
        window.__ENDED__.push(args?.term);
        return null;
      };
      // The person's own switch on 2026-10-21: never ask about running terminals.
      setConfirmCloseRunning(false);
      const agentPane = (term) => {
        mountTermTab(term, { agent: "Claude", worktree: A }, { placement: "tab", focus: false });
        paneAgents.set(term, "claude");
        return tabOfTerm(term);
      };
      const coord = agentPane(seats[0].term);
      tileTermPane(coord, seats[1].term, { agent: "Claude" }, "right", false, seats[0].term);
      paneAgents.set(seats[1].term, "claude");
      // Words a person wrote, where a careless line would pick them up.
      coord.paneTitles = { [seats[0].term]: `${person}-title` };
      panePrompts.set(seats[1].term, `${person}-prompt`);
      const working = agentPane(worker.term);
      mountTermTab(9604, { worktree: A }, { placement: "tab", focus: false });
      const plain = tabOfTerm(9604);
      const asleep = openStoredTab({
        id: 9605,
        root: { type: "leaf" },
        agents: { 0: { agent: "claude", key: "session_id", id: "8f0c1d52-0b7a-4c43-9d3e-5a1f2b3c4d5e" } },
        running: { 0: "claude" },
      }, A, "restore");
      window.__ANSWER__.coordinator_seats = () => seats.map((seat) => ({
        run: seat.run, name: seat.name, seat: true, term: seat.term, held: true,
        generation: seat.generation, vacated_ms: null,
      }));
      window.__LEDGER__ = [{
        run: seats[0].run, worker: worker.worker, agent: "claude", state: "working", ledger: "active",
        hearing: "heard", hearing_at: 1, checkout: A, term: worker.term, task: worker.task,
        task_id: worker.task_id, reported: false, failed: false, settled: false, session: null, at: 1,
        review: { verified: false, merged: false, deployed: false, written: false, author: null },
        review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null,
        handed_in: null, kept: null, taken_over: false,
      }];
      // The ledger moved: what the window does on its own beat, until the seats and the worker have landed.
      for (let tries = 0; tries < 80 && !(coordinatorSeats.length === seats.length && paneLedger.get(worker.term)?.worker); tries += 1) {
        for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
        await settle(25);
      }
      setActiveTab(coord.id);
      renderTabs();
      await window.__PAINTED__();
      await settle();
      return { coord: coord.id, worker: working.id, plain: plain.id, asleep: asleep.id, folder: basename(A) };
    }, { seats: SEATS, worker: WORKER, person: PERSON });

    /* ① — what the strip says of the tabs, before anything closes. */
    const strip = await page.evaluate(({ ids, seats }) => {
      const node = (id) => document.querySelector(`[data-tab="${CSS.escape(id)}"]`);
      const coordinator = t("agent.role.coordinator", "코디");
      return {
        coordLabel: node(ids.coord)?.querySelector(".tab-label")?.textContent ?? null,
        coordLead: `${coordinator} `,
        coordTip: node(ids.coord)?.dataset.tip ?? null,
        holds: t("terminal.holdsCoordinators", "코디 판 {{n}}개({{names}})", {
          n: 2, names: seats.map((seat) => seat.name).join("·"),
        }),
        asleepTip: node(ids.asleep)?.dataset.tip ?? null,
        stored: t("terminal.holdsStored", "다시 열 대화"),
        folder: ids.folder,
        roles: [...paneLeaves(tabs.find((tab) => tab.id === ids.coord)?.layout)].map((term) => paneRole(term)?.kind ?? null),
      };
    }, { ids, seats: SEATS });
    ok(
      "a tab holding two runs' coordinators names both seats and its folder on its tip, and its name on the strip leads with no role word (t-78107); a restored tab not woken yet says it holds conversations to wake (t-67936)",
      strip.roles.every((kind) => kind === "coordinator") &&
        strip.coordLabel !== null && !strip.coordLabel.startsWith(strip.coordLead) &&
        strip.coordTip?.includes(strip.holds) && strip.coordTip.includes(strip.folder) &&
        strip.asleepTip?.includes(strip.stored) && strip.asleepTip.includes(strip.folder),
      JSON.stringify(strip),
    );

    /* ② — the person's own × on the coordinators' tab, the running switch off. A press is
     * made only on what stands, so a window that already lost the tab says so in a check
     * rather than in a timeout. */
    const press = async (selector) => {
      if ((await page.locator(selector).count()) === 0) return false;
      await page.click(selector);
      return true;
    };
    const before = await page.evaluate(() => window.__LINES__.length);
    await press(`[data-tab="${ids.coord}"] .tab-close`);
    await page.waitForTimeout(150);
    const asked = await page.evaluate(({ ids, seats }) => {
      const held = t("terminal.holdsCoordinators", "코디 판 {{n}}개({{names}})", {
        n: 2, names: seats.map((seat) => seat.name).join("·"),
      });
      return {
        asked: !document.getElementById("ask-scrim").hidden,
        title: document.getElementById("ask-title").textContent,
        want: t("terminal.holdsTitle", "{{held}}가 들어 있습니다 — 닫을까요?", { held }),
        confirm: document.getElementById("ask-yes").textContent,
        wantConfirm: t("tab.close", "탭 닫기"),
        remember: !document.querySelector(".ask-remember")?.hidden,
        open: tabs.some((tab) => tab.id === ids.coord),
        ended: [...window.__ENDED__],
      };
    }, { ids, seats: SEATS });
    if (asked.asked) await press("#ask-no");
    await page.waitForTimeout(150);
    const kept = await page.evaluate(({ ids }) => ({
      open: tabs.some((tab) => tab.id === ids.coord),
      ended: [...window.__ENDED__],
      scrimDown: document.getElementById("ask-scrim").hidden,
    }), { ids });
    const keptLines = await page.evaluate((from) => window.__LINES__.slice(from), before);
    ok(
      "the person's × on a tab holding two coordinators asks one line naming both seats before anything ends — with 「도는 터미널에 대해 다시 묻지 않기」 on, as the person's was — and 「취소」 keeps both panes (t-67936: questions before a seat closes 0 → 1)",
      asked.asked && asked.title === asked.want && asked.confirm === asked.wantConfirm && !asked.remember &&
        asked.open && asked.ended.length === 0 &&
        kept.open && kept.ended.length === 0 && kept.scrimDown,
      JSON.stringify({ asked, kept, lines: keptLines }),
    );

    /* ③ — a click no hand made: refused, and said. */
    const synthetic = await page.evaluate(async ({ ids }) => {
      const settle = (ms = 80) => new Promise((done) => setTimeout(done, ms));
      const from = window.__LINES__.length;
      for (const note of document.querySelectorAll(".toast")) note.remove();
      document.querySelector(`[data-tab="${CSS.escape(ids.coord)}"] .tab-close`)?.click();
      await settle();
      const toast = [...document.querySelectorAll(".toast")].map((note) => note.textContent ?? "");
      return {
        asked: !document.getElementById("ask-scrim").hidden,
        open: tabs.some((tab) => tab.id === ids.coord),
        ended: [...window.__ENDED__],
        said: toast.some((words) => words.startsWith(t(
          "terminal.closeRefused", "사람 손이 아닌 입력이 탭을 닫으려 해서 막았습니다 — {{held}}", { held: "" },
        ).trim().replace(/—$/, "").trim())),
        lines: window.__LINES__.slice(from),
      };
    }, { ids });
    ok(
      "a close carried by an event no hand made — a script's click on the × — is refused outright when the tab holds a seat: no question, no pane ended, a toast and a black-box line saying so",
      !synthetic.asked && synthetic.open && synthetic.ended.length === 0 && synthetic.said &&
        synthetic.lines.length === 1 && synthetic.lines[0].includes("synthetic click") &&
        synthetic.lines[0].includes("refused"),
      JSON.stringify(synthetic),
    );

    /* ④ — 「탭 닫기」: both panes end. */
    await press(`[data-tab="${ids.coord}"] .tab-close`);
    await page.waitForTimeout(150);
    const confirmAsked = await page.evaluate(() => !document.getElementById("ask-scrim").hidden);
    if (confirmAsked) await press("#ask-yes");
    await page.waitForTimeout(200);
    const closed = await page.evaluate(({ ids, seats }) => ({
      open: tabs.some((tab) => tab.id === ids.coord),
      ended: [...window.__ENDED__].sort(),
      want: seats.map((seat) => seat.term).sort(),
    }), { ids, seats: SEATS });
    ok(
      "「탭 닫기」 answered, the tab goes and both coordinators' panes end",
      confirmAsked && !closed.open && JSON.stringify(closed.ended) === JSON.stringify(closed.want),
      JSON.stringify({ confirmAsked, ...closed }),
    );

    /* ⑤ — a worker's tab asks the same way; a plain shell and a tab not woken yet close without a word. */
    const others = await page.evaluate(async ({ ids, worker }) => {
      const settle = (ms = 120) => new Promise((done) => setTimeout(done, ms));
      const shown = () => !document.getElementById("ask-scrim").hidden;
      closeTab(ids.worker);
      await settle();
      const workerAsked = shown();
      const workerTitle = document.getElementById("ask-title").textContent;
      const workerWant = t("terminal.holdsTitle", "{{held}}가 들어 있습니다 — 닫을까요?", {
        held: t("terminal.holdsWorkers", "워커 판 {{n}}개({{names}})", { n: 1, names: worker.worker }),
      });
      if (workerAsked) document.getElementById("ask-no").click();
      await settle();
      const workerKept = tabs.some((tab) => tab.id === ids.worker);
      closeTab(ids.plain);
      await settle();
      const plainAsked = shown();
      const plainGone = !tabs.some((tab) => tab.id === ids.plain);
      closeTab(ids.asleep);
      await settle();
      const asleepAsked = shown();
      const asleepGone = !tabs.some((tab) => tab.id === ids.asleep);
      return { workerAsked, workerSays: workerTitle === workerWant, workerKept, plainAsked, plainGone, asleepAsked, asleepGone };
    }, { ids, worker: WORKER });
    ok(
      "a worker's tab asks the same one line naming the worker, while a plain shell and a restored tab not woken yet close without a word",
      others.workerAsked && others.workerSays && others.workerKept &&
        !others.plainAsked && others.plainGone && !others.asleepAsked && others.asleepGone,
      JSON.stringify(others),
    );

    /* ⑥ — the black box: one line a close, its road, its hand, its pointer, Computer Use, the seats by id. */
    const lines = await page.evaluate((from) => window.__LINES__.slice(from).filter((line) => line.startsWith("close tab ")), before);
    const coordLines = lines.filter((line) => line.startsWith(`close tab ${ids.coord} `));
    const read = {
      lines,
      kept: coordLines.some((line) => / by button · person click at \d+,\d+ · cu idle · holds coordinator run-4275, coordinator run-11955 → kept$/.test(line)),
      refused: coordLines.some((line) => line.includes(" by button · synthetic click") && line.endsWith("→ refused: no hand made the event")),
      closedAfter: coordLines.some((line) => / by button · person click at \d+,\d+ · cu idle · .* → close requested after asking$/.test(line)),
      worker: lines.some((line) => line.startsWith(`close tab ${ids.worker} by program`) && line.includes("holds worker w-9603") && line.endsWith("→ kept")),
      plain: lines.some((line) => line.startsWith(`close tab ${ids.plain} `) && line.endsWith("holds nothing → close requested")),
      asleep: lines.some((line) => line.startsWith(`close tab ${ids.asleep} `) && line.endsWith("holds stored conversations → close requested")),
      personWords: lines.some((line) => line.includes(PERSON) || line.includes(SEATS[1].name)),
    };
    ok(
      "every close of a terminal tab leaves one black-box line — the road (button, program), whose hand (person click, synthetic click), the pointer, Computer Use, the seats by run and worker id, the outcome — and no word a person wrote",
      read.kept && read.refused && read.closedAfter && read.worker && read.plain && read.asleep && !read.personWords &&
        coordLines.length === 3,
      JSON.stringify(read),
    );

    /* ⑦ — the new words in every catalogue. */
    const langs = await page.evaluate(() => {
      const KEYS = {
        "terminal.holdsCoordinators": "코디 판 {{n}}개({{names}})",
        "terminal.holdsWorkers": "워커 판 {{n}}개({{names}})",
        "terminal.holdsAgents": "에이전트 판 {{n}}개",
        "terminal.holdsStored": "다시 열 대화",
        "terminal.holdsTitle": "{{held}}가 들어 있습니다 — 닫을까요?",
        "terminal.closeRefused": "사람 손이 아닌 입력이 탭을 닫으려 해서 막았습니다 — {{held}}",
      };
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        found[code] = Object.keys(KEYS).filter((key) => typeof CATALOG[code]?.[key] !== "string");
      }
      setLocale("ko", { persist: false });
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.map((key) => `${code}:${key}`));
    ok(
      "every new close word reads in English, Japanese, Chinese and Spanish, not in the Korean fallback",
      missing.length === 0,
      JSON.stringify({ missing }),
    );
  } finally {
    await page.close().catch(() => {});
  }
}

export async function testRestartNotice({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(() => {
      setLocale("ko", { persist: false });
      window.__ROAD__ = [];
      const heard = (kind, answer = null) => (args) => {
        window.__ROAD__.push([kind, JSON.parse(JSON.stringify(args ?? {}))]);
        return typeof answer === "function" ? answer(args) : answer;
      };
      window.__ANSWER__.relaunch_window = heard("relaunch");
      window.__ANSWER__.leave_hold = heard("hold");
      window.__ANSWER__.leave_cancel = heard("cancel");
      window.__ANSWER__.leave_when_idle = heard("arm", (args) => ({ road: args.road, running: 1, unknown: 0, waitedMin: 0, waitMin: 30 }));
      // The backend's first notice line for a restart; a close went at once.
      window.__ANSWER__.leave_now = heard("now", (args) => (args.road === "restart"
        ? { road: "restart", door: args.door ?? null, seconds: 10, held: false }
        : null));
      window.__IDLE__ = { busy: false, workers: 0, turning: 0, background: 0, unknown: 0, running: 0, gap: true };
      window.__BUSY__ = { busy: true, workers: 2, turning: 1, background: 1, unknown: 0, running: 1, gap: false };
      window.__CENSUS__ = window.__IDLE__;
      window.__ANSWER__.busy_census = (args) => ({
        road: args.road, door: args.door ?? null, busy: window.__CENSUS__, waitMin: 30, answerSec: null,
      });
      // The lane installed 1.1.86 under the running 1.1.85: the build a restart brings.
      releaseNotice = {
        app: {
          change: "version", installed: "d5570f5f5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", installed_version: "1.1.86",
          running: "9be7b2a47aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", running_version: "1.1.85",
        },
        zo: null,
      };
      noticedBuilds.clear();
      for (const note of document.querySelectorAll(".toast")) note.remove();
      raiseUpdateToast();
    });
    const notices = () => page.evaluate(() => [...document.querySelectorAll(".toast")]
      .filter((note) => note.dataset.notice === "exit-notice" && !note.classList.contains("is-closing"))
      .map((note) => note.querySelector(".toast-text")?.textContent ?? ""));
    const road = () => page.evaluate(() => window.__ROAD__.map(([kind, args]) => [kind, args]));
    // A press only on what stands: a window with no notice says so in a check, not a timeout.
    const press = async (selector) => {
      if ((await page.locator(selector).count()) === 0) return false;
      await page.click(selector);
      return true;
    };

    /* ① — a key, then the person's click on 「다시 시작」 with nothing busy. */
    await page.keyboard.press("ArrowDown");
    await press('.toast[data-notice="update"] .toast-action');
    await page.waitForTimeout(150);
    const pressed = {
      notices: await notices(),
      want: await page.evaluate(() => t("exit.noticeTo", "{{seconds}}초 뒤 {{version}}(으)로 다시 시작합니다", { seconds: 10, version: "1.1.86" })),
      road: await road(),
    };
    const went = pressed.road.find(([kind]) => kind === "now")?.[1] ?? null;
    ok(
      "a restart door with nothing busy goes by the way out's notice and never the restart road itself — 「10초 뒤 1.1.86(으)로 다시 시작합니다」 with its 취소, the hand that pressed it named, and the person's last key reported (t-67936: restarts with no notice 0)",
      went?.road === "restart" && went.door === "update-toast" && went.unasked === true && went.by === "person click" &&
        !pressed.road.some(([kind]) => kind === "relaunch") &&
        pressed.notices.length === 1 && pressed.notices[0] === pressed.want &&
        pressed.road.some(([kind, args]) => kind === "hold" && Number.isInteger(args.agoMs) && args.agoMs >= 0),
      JSON.stringify(pressed),
    );

    /* ② — the beat's next line, said in place; a key while it stands, said once a line. */
    const holdsBefore = (await road()).filter(([kind]) => kind === "hold").length;
    await page.evaluate(() => {
      for (const handler of window.__LISTENERS__["exit:notice"] ?? []) {
        handler({ payload: { road: "restart", door: "update-toast", seconds: 4, held: true } });
      }
    });
    await page.waitForTimeout(80);
    const holdsAfterLine = (await road()).filter(([kind]) => kind === "hold").length;
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.waitForTimeout(80);
    const keyed = (await road()).filter(([kind]) => kind === "hold");
    const moved = {
      notices: await notices(),
      want: await page.evaluate(() => `${t("exit.noticeHeld", "입력 중이라 미룹니다")} · ${t("exit.noticeTo", "{{seconds}}초 뒤 {{version}}(으)로 다시 시작합니다", { seconds: 4, version: "1.1.86" })}`),
      lineReports: holdsAfterLine - holdsBefore,
      keyReports: keyed.length - holdsAfterLine,
      lastKey: keyed.at(-1)?.[1] ?? null,
    };
    ok(
      "the beat's next line is said in the same one line — 「입력 중이라 미룹니다 · 4초 뒤 …」 while the person's typing holds it — and two keys while it stands are said to the backend once",
      moved.notices.length === 1 && moved.notices[0] === moved.want &&
        moved.lineReports === 1 && moved.keyReports === 1 && moved.lastKey?.agoMs === 0,
      JSON.stringify(moved),
    );

    /* ③ — 「취소」. */
    await press('.toast[data-notice="exit-notice"] .toast-action');
    await page.waitForTimeout(300);
    const cancelled = { notices: await notices(), road: await road() };
    ok(
      "「취소」 takes the notice down and tells the backend",
      cancelled.notices.length === 0 && cancelled.road.some(([kind]) => kind === "cancel"),
      JSON.stringify(cancelled),
    );

    /* ④ — work in progress: the census's question, then 「지금 다시 시작」 — by the notice too. */
    await page.evaluate(() => {
      window.__CENSUS__ = window.__BUSY__;
      window.__ROAD__ = [];
      void askBeforeRestart("window-material");
    });
    await page.waitForTimeout(150);
    const questioned = await page.evaluate(() => !document.getElementById("ask-scrim").hidden);
    if (questioned) await press("#ask-no");
    await page.waitForTimeout(150);
    const now = { questioned, notices: await notices(), road: await road() };
    const nowWent = now.road.find(([kind]) => kind === "now")?.[1] ?? null;
    ok(
      "「지금 다시 시작」 after the census's question goes by the notice as well, naming the hand that answered — never the restart road itself",
      questioned && nowWent?.road === "restart" && nowWent.door === "window-material" && nowWent.unasked === undefined &&
        typeof nowWent.by === "string" && nowWent.by.startsWith("person ") &&
        !now.road.some(([kind]) => kind === "relaunch") && now.notices.length === 1,
      JSON.stringify(now),
    );
    await page.evaluate(() => {
      for (const note of document.querySelectorAll(".toast")) note.remove();
    });

    /* ④b — t-76434: no worker busy, four coordinators mid-turn (2026-10-26
     * 22:56, `choice unasked`). The door asks, and the question names them —
     * and a fifth this window has not heard from yet (t-76504). */
    await page.evaluate(() => {
      window.__CENSUS__ = { busy: true, workers: 1, turning: 0, background: 0, unknown: 0, conversations: 4, unheardConversations: 1, running: 0, gap: false };
      window.__ROAD__ = [];
      void askBeforeRestart("update-toast");
    });
    await page.waitForTimeout(150);
    const talking = await page.evaluate(() => ({
      questioned: !document.getElementById("ask-scrim").hidden,
      body: document.getElementById("ask-scrim").textContent ?? "",
      want: t("exit.busyConversations", "턴 중인 대화 {{n}}개", { n: 4 }),
      wantUnheard: t("exit.busyUnheardConversations", "상태를 모르는 대화 {{n}}개", { n: 1 }),
      road: window.__ROAD__.map(([kind]) => kind),
    }));
    if (talking.questioned) await press("#ask-cancel").catch(() => false);
    await page.evaluate(() => {
      document.getElementById("ask-scrim").hidden = true;
      for (const note of document.querySelectorAll(".toast")) note.remove();
    });
    ok(
      "a restart over conversations mid-turn and no busy worker asks first, and its question says 「턴 중인 대화 4개」 and 「상태를 모르는 대화 1개」 — it no longer goes unasked (t-76434, t-76504)",
      talking.questioned && talking.body.includes(talking.want) && talking.body.includes(talking.wantUnheard) && !talking.road.includes("now"),
      JSON.stringify(talking),
    );

    /* ⑤ — the window a restart started says which build it came back as, once. */
    const back = await page.evaluate(async () => {
      const said = () => [...document.querySelectorAll(".toast")]
        .filter((note) => note.dataset.notice === "restart-back")
        .map((note) => note.querySelector(".toast-text")?.textContent ?? note.textContent ?? "");
      const answers = [
        { door: "update-toast", fromVersion: "1.1.85", fromCommit: "9be7b2a47", version: "1.1.86", commit: "d5570f5f5" },
        { door: "update-toast", fromVersion: "1.1.86", fromCommit: "9be7b2a47", version: "1.1.86", commit: "d5570f5f5" },
        { door: "window-material", fromVersion: "1.1.86", fromCommit: "d5570f5f5", version: "1.1.86", commit: "d5570f5f5" },
        null,
      ];
      const seen = [];
      for (const answer of answers) {
        for (const note of document.querySelectorAll(".toast")) note.remove();
        window.__ANSWER__.boot_after_restart = () => answer;
        if (typeof sayRestartBack === "function") await sayRestartBack();
        seen.push(said());
      }
      return {
        seen,
        want: [
          [t("exit.backTo", "{{version}}(으)로 다시 시작했습니다 (업데이트)", { version: "1.1.86" })],
          [t("exit.backToBuild", "새 빌드({{sha}})로 다시 시작했습니다 (업데이트)", { sha: "d5570f5" })],
          [t("exit.back", "다시 시작했습니다")],
          [],
        ],
      };
    });
    ok(
      "the window a restart started says once which build it came back as — 「1.1.86(으)로 다시 시작했습니다 (업데이트)」, a new build by its sha, a plain restart plainly — and a start that was no restart says nothing",
      JSON.stringify(back.seen) === JSON.stringify(back.want),
      JSON.stringify(back),
    );

    /* ⑥ — the new words in every catalogue. */
    const langs = await page.evaluate(() => {
      const KEYS = ["exit.noticeTo", "exit.notice", "exit.noticeHeld", "exit.backTo", "exit.backToBuild", "exit.back", "exit.busyConversations", "exit.busyUnheardConversations"];
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        found[code] = KEYS.filter((key) => typeof CATALOG[code]?.[key] !== "string");
      }
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.map((key) => `${code}:${key}`));
    ok(
      "every new restart word reads in English, Japanese, Chinese and Spanish, not in the Korean fallback",
      missing.length === 0,
      JSON.stringify({ missing }),
    );
  } finally {
    await page.close().catch(() => {});
  }
}
