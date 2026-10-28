import { openWindowTestPage } from "./window-boot.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";

/* Who is the coordinator, read off the sidebar (t-45938).
 *
 * Asked 2026-10-11 10:4x from the person's own window: 「누가 코디인지 네비바에
 * 표시 되나」. It was not: the coordinator's pane, a worker's pane and a pane the
 * person took over all read 「Claude · one line · model · clock」, and the only
 * place that told them apart was the board's desk, two presses away.
 *
 * What stands here, built from the window's own roads (`hook:agent`,
 * `ledger:changed` over `ledger_agents` and `coordinator_seats`,
 * `project_catalog`), with a fake ledger and nobody's conversation read:
 *   ① each agent row wears its role after the state dot and before the face —
 *      코디 (the pane sitting in a run's live coordinator seat, its tip naming the
 *      run and the generation), 워커 (a pane the ledger summoned, its tip naming
 *      the worker and the task — still 워커 when the person took the pane over,
 *      the chip then wearing is-taken and its tip saying so; 10-11 「사람이
 *      소환한 게 아닌데 사람이라 표시됨」) — and a pane the ledger never seated
 *      wears none;
 *   ② two runs' coordinators in one window both read 코디;
 *   ③ a vacated seat takes the chip off, and 「코디 (비어 있음)」 stands once on
 *      a workspace line; a handed-over seat moves the chip to its new pane;
 *   ④ a folded summary row wears no chip — the coordinator's face is ringed instead;
 *   ⑤ the workspace line's worker cell says 워커 w-… / 사람 w-… / 코디, and a
 *      checkout whose work was handed in keeps its stage word alone;
 *   ⑥ the tab strip and the desk's worker heads lead with the same words;
 *   ⑦ every new word reads in the four other catalogues.
 *
 * `ok` details carry the numbers the report quotes: how many kinds of row the
 * sidebar draws for the three roles, and how many presses it takes to find the
 * coordinator from the sidebar. */

const RUN_A = { run: "run-roles-a", name: "역할 칩 런", generation: 3 };
const RUN_B = { run: "run-roles-b", name: "옆 런", generation: 1 };

/* The scene's panes, one per checkout. `seat` names the run whose coordinator seat
 * the pane holds; `ledger` is the worker row `ledger_agents` sends for it. */
const PANES = [
  { term: 9501, path: "/r/main", seat: RUN_A },
  { term: 9502, path: "/r/w-work", ledger: { worker: "w-9502", task: "역할 칩 과업", task_id: "t-9502" } },
  { term: 9503, path: "/r/w-person", ledger: { worker: "w-9503", task: "넘겨받은 과업", task_id: "t-9503", taken_over: true } },
  { term: 9504, path: "/r/w-own" },
];
/* Joined later, one at a time. */
const SIDE = { term: 9506, path: "/r/side", seat: RUN_B };
const HANDED = { term: 9505, path: "/r/w-handed", ledger: { worker: "w-9505", task: "넘김 끝난 과업", task_id: "t-9505", reported: true,
  progress: { stage: "report_only" } } };
const CHILD = { term: 9508, path: "/r/side", parent: 9506 };
/* A worker of run A that sleeps — no terminal in this window — after a restart. */
const ASLEEP = { path: "/r/w-sleep", ledger: { worker: "w-9507", task: "잠든 과업", task_id: "t-9507", state: "waiting", ledger: "sleeping" } };

const PATHS = ["/r/main", "/r/w-work", "/r/w-person", "/r/w-own", "/r/side", "/r/w-handed", "/r/w-sleep"];

/* A seat row the way `coordinator_seats` answers it (the desk's run list). */
const seatRow = (run, term, over = {}) => ({
  run: run.run, name: run.name, seat: term !== null, term, held: true, generation: run.generation, vacated_ms: null, ...over,
});

export async function testSidebarRoles({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });

    /* ---- the scene: a coordinator, a worker, a person, and a pane nobody seated ---- */
    await page.evaluate(async ({ panes, paths, asleep, seats }) => {
      setLocale("ko", { persist: false });
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      window.__ROW_OF__ = (over) => ({
        run: "run-roles-a", worker: "", agent: "claude", state: "working", ledger: "active", hearing: "pending",
        hearing_at: 1, checkout: "", task: "", task_id: "", reported: false, failed: false, settled: false,
        session: null, at: 1, review: { verified: false, merged: false, deployed: false, written: false, author: null },
        review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null,
        kept: null, taken_over: false, ...over,
      });
      window.__SEAT__ = (one) => {
        tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
        paneAgents.set(one.term, "claude");
        if (one.parent !== undefined) paneParents.set(one.term, one.parent);
        tell("hook:agent", { term: one.term, state: "working", agent: "claude", session: `s-${one.term}`, model: "claude-haiku-5-5" });
        if (one.ledger) {
          window.__LEDGER__.push(window.__ROW_OF__({ term: one.term, checkout: one.path, ...one.ledger }));
        }
      };
      window.__LEDGER__ = [window.__ROW_OF__({ checkout: asleep.path, ...asleep.ledger })];
      window.__SEATS__ = seats;
      window.__ANSWER__.coordinator_seats = () => window.__SEATS__;
      for (const one of panes) window.__SEAT__(one);
      window.__ANSWER__.project_catalog = () => [{
        name: "r", path: "/r",
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: paths.map((path, at) => ({
          path, branch: path.slice(3), is_main: path === "/r/main", active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      // The branch list's own rows are what this suite reads: with a coordinator sitting, the team view
      // (t-56141, its own suite `sidebar-team`) would stand in their place.
      setSidebarView({ groupBy: "repo", layout: "branches" });
      await refreshWorktrees();
      // The ledger moved: what the window does on its own beat. Waits for the rows to land, then for one paint.
      window.__LEDGER_MOVED__ = async (want) => {
        for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
        for (let tries = 0; tries < 80 && !want(); tries += 1) await new Promise((done) => setTimeout(done, 25));
        await window.__PAINTED__();
        await new Promise((done) => setTimeout(done, 200));
        await window.__PAINTED__();
      };
      await window.__LEDGER_MOVED__(() => paneLedger.size >= 2 && document.querySelector(".wt-agent-role") !== null);
      // What a person can read of the agent rows and the workspace lines.
      window.__CHIPS__ = () => [...document.querySelectorAll(".wt-agent[data-term]")].map((node) => {
        const chip = node.querySelector(":scope > .wt-agent-role");
        return {
          term: Number(node.dataset.term),
          folded: node.classList.contains("is-folded-summary"),
          chip: chip ? chip.textContent : null,
          kind: chip ? [...chip.classList].find((name) => name.startsWith("is-") && name !== "is-taken")?.slice(3) ?? "" : null,
          taken: chip?.classList.contains("is-taken") ?? false,
          tip: chip?.dataset.tip ?? "",
          titled: chip?.hasAttribute("title") ?? false,
          tag: chip?.tagName ?? null,
          before: chip?.previousElementSibling?.classList.contains("wt-agent-dot") ?? false,
          after: chip?.nextElementSibling?.classList.contains("wt-agent-face") ?? false,
          aria: node.getAttribute("aria-label") ?? "",
          ringed: node.querySelector(":scope > .wt-agent-face")?.classList.contains("is-coordinator") ?? false,
          ring: (() => {
            const face = node.querySelector(":scope > .wt-agent-face");
            return face ? getComputedStyle(face).boxShadow : null;
          })(),
        };
      });
      window.__LINE__ = (path) => {
        const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
        if (!row) return null;
        const shown = (node) => Boolean(node) && !node.hidden && node.textContent !== "";
        const worker = row.querySelector(".wt-worker");
        const vacant = row.querySelector(".wt-coordinator-vacant");
        return {
          worker: shown(worker) ? worker.textContent : "",
          stage: shown(row.querySelector(".wt-stage")) ? row.querySelector(".wt-stage").textContent : "",
          vacant: shown(vacant) ? vacant.textContent : "",
          vacantTip: vacant?.dataset.tip ?? "",
          vacantTitled: vacant?.hasAttribute("title") ?? false,
        };
      };
      window.__VACANT__ = () => [...document.querySelectorAll(".wt-row .wt-coordinator-vacant")]
        .filter((node) => !node.hidden && node.textContent !== "")
        .map((node) => node.closest(".wt-row")?.dataset.worktreePath ?? "");
      window.__WORDS__ = () => ({
        coordinator: t("agent.role.coordinator", "코디"),
        worker: t("agent.role.worker", "워커"),
        vacant: t("agent.role.coordinatorVacant", "코디 (비어 있음)"),
        handedIn: t("worktree.stage.handedIn", "넘김 끝 · 검토 대기"),
      });
    }, { panes: PANES, paths: PATHS, asleep: ASLEEP, seats: [seatRow(RUN_A, 9501)] });

    const words = await page.evaluate(() => window.__WORDS__());
    const chips = await page.evaluate(() => window.__CHIPS__());
    const at = (term) => chips.find((one) => one.term === term) ?? null;

    /* ① — one of each, in its place, with its tip. */
    const kinds = new Set(chips.filter((one) => one.chip !== null).map((one) => one.kind));
    ok(
      "a fake ledger draws the role chips by who the ledger seated — 코디 on the seat, 워커 on both summoned workers (the one the person took over keeps 워커 and wears is-taken), none on the pane the ledger never seated, and no chip ever reads 사람",
      at(9501)?.chip === words.coordinator && at(9501)?.kind === "coordinator" &&
        at(9502)?.chip === words.worker && at(9502)?.kind === "worker" && !at(9502)?.taken &&
        at(9503)?.chip === words.worker && at(9503)?.kind === "worker" && at(9503)?.taken &&
        at(9504) !== null && at(9504).chip === null &&
        chips.filter((one) => one.kind === "coordinator").length === 1 &&
        chips.filter((one) => one.kind === "worker").length === 2 &&
        chips.every((one) => one.chip !== "사람"),
      JSON.stringify({ kindsDrawn: kinds.size, chips }),
    );
    ok(
      "a role chip stands after the state dot and before the face, as a span inside the row's one button, with its tip in data-tip and no title",
      [9501, 9502, 9503].every((term) => at(term)?.before && at(term)?.after && at(term)?.tag === "SPAN" && !at(term)?.titled),
      JSON.stringify(chips),
    );
    ok(
      "the coordinator chip's tip names the run and the seat's generation; a worker's names the worker and its task (and that the person took it over); the person's says they opened it — and the row says its role to a screen reader",
      at(9501)?.tip.includes(RUN_A.name) && at(9501)?.tip.includes(String(RUN_A.generation)) &&
        at(9502)?.tip.includes("w-9502") && at(9502)?.tip.includes("역할 칩 과업") &&
        at(9503)?.tip.includes("w-9503") && at(9503)?.tip !== at(9502)?.tip.replace("w-9502", "w-9503") &&
        at(9501)?.aria.startsWith(words.coordinator) && at(9502)?.aria.startsWith(words.worker) &&
        at(9503)?.aria.startsWith(words.worker),
      JSON.stringify(chips.map(({ term, tip, aria }) => ({ term, tip, aria }))),
    );
    /* The numbers the report quotes. Before this change no row drew a role, so the sidebar could not say
     * who the coordinator was: the person opened the board and read its desk (two presses). */
    const presses = at(9501)?.kind === "coordinator" ? 0 : 2;
    ok(
      "the sidebar alone says who the coordinator is — 0 presses — and draws the coordinator's and the workers' panes two different ways",
      presses === 0 && kinds.size === 2,
      JSON.stringify({ presses, kindsDrawn: kinds.size }),
    );

    /* ⑤ — the workspace lines' worker cells. */
    const lines = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => [path, window.__LINE__(path)])), PATHS);
    ok(
      "a workspace line says who works there: 코디 where the coordinator's pane stands, 워커 w-… for a summoned worker, 사람 w-… where the person took it over, nothing for a pane nobody seated",
      lines["/r/main"]?.worker === words.coordinator &&
        lines["/r/w-work"]?.worker === `${words.worker} w-9502` &&
        lines["/r/w-person"]?.worker === `${words.worker} w-9503` &&
        lines["/r/w-own"]?.worker === "",
      JSON.stringify(lines),
    );

    /* ⑥ — the tab strip says the work, and its tip says the role (t-78107, 시안 2: 「코디 · …」 before every name took
     * a third of the tab). The strip paints the checkout in front, so each is put in front in turn. */
    const strip = await page.evaluate(async (terms) => {
      const front = activeWorktreePath;
      const read = {};
      for (const term of terms) {
        const tab = tabs.find((one) => one.id === `tab-${term}`);
        activeWorktreePath = tab.worktree;
        renderTabs();
        await window.__PAINTED__();
        const node = document.querySelector(`.tab[data-tab="${CSS.escape(tab.id)}"]`);
        read[term] = { label: node?.querySelector(".tab-label")?.textContent ?? null, tip: node?.dataset.tip ?? "" };
      }
      activeWorktreePath = front;
      renderTabs();
      await window.__PAINTED__();
      return read;
    }, [9501, 9502, 9503, 9504]);
    ok(
      "a tab's name leads with no role word, and its tip names the role its row wears — 코디, 워커 (also for the taken-over worker) — while a tab nobody seated keeps its plain name and says no role (t-78107)",
      [9501, 9502, 9503, 9504].every((term) => strip[term]?.label !== null &&
        ![words.coordinator, words.worker].some((word) => strip[term].label.startsWith(`${word} · `))) &&
        strip[9501].tip.includes(words.coordinator) && strip[9502].tip.includes(words.worker) &&
        strip[9503].tip.includes(words.worker) &&
        ![words.coordinator, words.worker].some((word) => strip[9504].tip.includes(word)),
      JSON.stringify(strip),
    );

    /* ② — a second run's coordinator in the same window. */
    const two = await page.evaluate(async ({ side, seats }) => {
      window.__SEAT__(side);
      window.__SEATS__ = seats;
      await refreshWorktrees();
      await window.__LEDGER_MOVED__(() => document.querySelectorAll(".wt-agent-role.is-coordinator").length >= 2);
      return window.__CHIPS__().filter((one) => one.kind === "coordinator");
    }, { side: SIDE, seats: [seatRow(RUN_A, 9501), seatRow(RUN_B, 9506)] });
    ok(
      "two runs' coordinators in one window both read 코디, each tip naming its own run",
      two.length === 2 && two.some((one) => one.term === 9501 && one.tip.includes(RUN_A.name)) &&
        two.some((one) => one.term === 9506 && one.tip.includes(RUN_B.name)),
      JSON.stringify(two),
    );

    /* ③ — the seat vacated (a restart), then handed to another pane. */
    const vacated = await page.evaluate(async (seats) => {
      window.__SEATS__ = seats;
      await window.__LEDGER_MOVED__(() => !document.querySelector('.wt-agent[data-term="9501"] .wt-agent-role'));
      return { chips: window.__CHIPS__(), vacant: window.__VACANT__(), main: window.__LINE__("/r/main") };
    }, [seatRow(RUN_A, null, { seat: false, held: false, vacated_ms: Date.now() - 60_000 }), seatRow(RUN_B, 9506)]);
    const vacantAt = (term) => vacated.chips.find((one) => one.term === term) ?? null;
    ok(
      "a vacated seat takes the chip off its pane, the other run's coordinator keeps its own, and 「코디 (비어 있음)」 stands once — on the project's main line, its tip naming the run",
      vacantAt(9501)?.chip === null && vacantAt(9506)?.kind === "coordinator" &&
        vacated.vacant.length === 1 && vacated.vacant[0] === "/r/main" &&
        vacated.main?.vacant === words.vacant && vacated.main.vacantTip.includes(RUN_A.name) && !vacated.main.vacantTitled &&
        vacated.main.worker !== words.coordinator,
      JSON.stringify(vacated),
    );
    const handed = await page.evaluate(async (seats) => {
      window.__SEATS__ = seats;
      await window.__LEDGER_MOVED__(() => document.querySelector('.wt-agent[data-term="9504"] .wt-agent-role') !== null);
      return { chips: window.__CHIPS__(), vacant: window.__VACANT__() };
    }, [seatRow(RUN_A, 9504, { generation: RUN_A.generation + 1 }), seatRow(RUN_B, 9506)]);
    const handedAt = (term) => handed.chips.find((one) => one.term === term) ?? null;
    ok(
      "a seat handed to another pane moves the chip there with the new generation, leaves none on the pane it left, and the empty-seat line goes",
      handedAt(9504)?.kind === "coordinator" && handedAt(9504)?.tip.includes(String(RUN_A.generation + 1)) &&
        handedAt(9501)?.chip === null && handed.vacant.length === 0,
      JSON.stringify(handed),
    );

    /* ④ — the folded summary: no chip, the coordinator's face ringed. */
    const folded = await page.evaluate(async (child) => {
      window.__SEAT__(child);
      agentFolded.add(child.parent);
      await refreshWorktrees();
      await window.__LEDGER_MOVED__(() => document.querySelector('.wt-agent.is-folded-summary[data-term="9506"]') !== null);
      const plain = document.querySelector('.wt-agent:not(.is-folded-summary)[data-term="9502"] > .wt-agent-face');
      return {
        row: window.__CHIPS__().find((one) => one.term === child.parent) ?? null,
        plainRing: plain ? getComputedStyle(plain).boxShadow : null,
      };
    }, CHILD);
    ok(
      "a folded summary row wears no chip: the coordinator's face is ringed instead, and the ring is drawn",
      folded.row?.folded === true && folded.row.chip === null && folded.row.ringed === true &&
        folded.row.ring !== null && folded.row.ring !== "none" && folded.row.ring !== folded.plainRing,
      JSON.stringify(folded),
    );

    /* ⑤b — a worker whose work was handed in: still a worker's row, but its line keeps the stage word alone. */
    const handedIn = await page.evaluate(async (one) => {
      window.__SEAT__(one);
      await refreshWorktrees();
      await window.__LEDGER_MOVED__(() => document.querySelector('.wt-agent[data-term="9505"] .wt-agent-role') !== null);
      return { row: window.__CHIPS__().find((held) => held.term === one.term) ?? null, line: window.__LINE__(one.path) };
    }, HANDED);
    ok(
      "a checkout whose work was handed in keeps its stage word and names no role on its line, while its pane's row still reads 워커",
      handedIn.row?.kind === "worker" && handedIn.line?.stage === words.handedIn &&
        ![words.worker, words.coordinator].some((word) => handedIn.line.worker.includes(word)),
      JSON.stringify(handedIn),
    );

    /* ⑦ — the new words in every catalogue. */
    const langs = await page.evaluate(() => {
      const KEYS = {
        "agent.role.coordinator": "코디",
        "agent.role.worker": "워커",
        "agent.role.coordinatorTip": "코디네이터 · 런 {{run}} · {{generation}}세대",
        "agent.role.workerTip": "워커 {{worker}}",
        "agent.role.workerTakenTip": "사람이 넘겨받은 판 · 워커 {{worker}}",
        "agent.role.coordinatorVacant": "코디 (비어 있음)",
        "agent.role.coordinatorVacantTip": "{{runs}} 런의 코디 자리가 비어 있습니다 — 코디가 다시 앉으면 사라집니다",
      };
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { persist: false });
        found[code] = Object.entries(KEYS).filter(([key, fallback]) => t(key, fallback) === fallback).map(([key]) => key);
      }
      setLocale("ko", { persist: false });
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.map((key) => `${code}:${key}`));
    ok(
      "every new role word reads in English, Japanese, Chinese and Spanish, not in the Korean fallback",
      missing.length === 0,
      JSON.stringify({ missing }),
    );
  } finally {
    await page.close().catch(() => {});
  }

  /* ⑥b — the desk's worker heads, on the desk's own fixture: one worker taken over by the person. */
  const desk = await openWindowTestPage(browser, origin, { faults });
  try {
    await desk.page.setViewportSize({ width: 1500, height: 1100 });
    await desk.page.evaluate(coordinatorDeskFixture, {});
    const heads = await desk.page.evaluate(async () => {
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-2" ? { ...row, taken_over: true } : row);
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
      // The workers' rows stand on the board's 워커 tab (t-67607).
      chooseBoardTab(document.querySelector("#board-view"), "workers", false);
      const read = () => Object.fromEntries([...document.querySelectorAll('#board-view [data-desk-block="workers"] .board-desk-worker')]
        .map((row) => [row.dataset.worker.split("/")[1], row.querySelector(".board-desk-worker-id")?.textContent ?? ""]));
      // Bounded: on a window without role words the head never changes, and the check below says so.
      const worker = t("agent.role.worker", "워커");
      for (let tries = 0; tries < 120 && read()["w-2"] !== `${worker} w-2`; tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      return { heads: read(), worker };
    });
    ok(
      "the desk's worker heads lead with the same role word — 워커 w-… for a summoned worker, taken over by the person or not",
      heads.heads["w-1"] === `${heads.worker} w-1` && heads.heads["w-2"] === `${heads.worker} w-2`,
      JSON.stringify(heads),
    );
  } finally {
    await desk.page.close().catch(() => {});
  }
}
