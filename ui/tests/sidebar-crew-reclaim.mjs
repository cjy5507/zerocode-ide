import { openWindowTestPage } from "./window-boot.mjs";
import { ownedChildrenViolations } from "./sidebar-aria.mjs";

/* The familiar-frame sidebar's crew and reclaim words, read off the drawn rows (t-95957, draft v4).
 *
 * The check of 2026-11-05 (t-95926) found that nothing under ui/tests read `.wt-agent-reclaim`, `.is-crew` or a
 * worker's second line, so a ledger change that only set `release_held` left 「회수 중」 standing and no test saw it.
 * The scene is a synthetic ledger built from the window's own roads (`hook:agent`, `ledger:changed` over
 * `ledger_agents` and `coordinator_seats`, `project_catalog`), with the sidebar at the 313px the draft is drawn at:
 *   ① a coordinator's crew line says 「워커 N · 상태 · t-…」, and a worker's second line its task and its coordinator;
 *   ② a refused reclaim the ledger reports by `release_held` alone turns 「회수 중」 into 보류 — on the 「완료 N개」 line
 *      and on the coordinator's crew line;
 *   ③ the 「완료 N개」 line breaks its finished panes down — 「회수 중 2 · 보류 1」 — and, opened, each row says its own
 *      reclaim word and the reason in its tip;
 *   ④ the 「완료 N개」 line is one button: its chevron is drawn for the eye only, and the row says it is expanded;
 *   ⑤ a 「완료 N개」 that folds only helpers (no pane ever stood) says nothing about a reclaim;
 *   ⑥ a resting row's model moves to its tooltip, so a coordinator's name reads at 313px;
 *   ⑦ the new words read in the four other catalogues;
 *   ⑧ a coordinator's row that folds child panes is one button too (r2): axe's nested-interactive finds nothing in the card
 *      list, the chevron is drawn for the eye, a press on it folds from the row's own press, and Left and Right are the
 *      keyboard's way to the same fold;
 *   ⑨ a helper's reading hand is a button beside its row, not one inside it (r3): axe finds nothing in the card list, at
 *      work or opened in the 「완료 N개」 line; the hand stays the next tab stop after its row, is silent at rest and shows
 *      at the row's right end when the row is pointed at, a press on it opens the helper's page once and goes to no pane,
 *      and the arrows step on from it. */

const RUN = { run: "run-crew", name: "크루 런", generation: 1 };
const LEAD = 9901;
const WORKER = 9902;
const FINISHED = [9903, 9904, 9905];
const RESTING = 9906;
const SOLO = 9907;
const PATHS = ["/r/main", "/r/w-a", "/r/solo"];
const HELD_REASON = "입력칸에 사람 글자가 있어 판을 닫지 않음";

/* The scene, on a page that has booted: also what a capture of these rows stands on (t-95957). */
export async function standCrewReclaimScene(page) {
  await page.setViewportSize({ width: 1280, height: 1400 });
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async ({ run, lead, worker, finished, resting, solo, paths }) => {
    setLocale("ko", { persist: false });
    setEveryProjectClosed(false);
    setPanelWidth("sidebar", 313);
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    window.__TELL__ = tell;
    const rowOf = (over) => ({
      run: run.run, worker: "", agent: "claude", state: "working", ledger: "active", hearing: "pending",
      hearing_at: 1, checkout: "", task: "", task_id: "", reported: false, failed: false, settled: false,
      session: null, at: 1, review: { verified: false, merged: false, deployed: false, written: false, author: null },
      review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null,
      kept: null, taken_over: false, ...over,
    });
    const seat = (term, path, { agent = "claude", state = "working", model = "claude-opus-5-5", parent } = {}) => {
      tabs.push({ kind: "term", worktree: path, id: `tab-${term}`, layout: { type: "leaf", term } });
      paneAgents.set(term, agent);
      if (parent !== undefined) paneParents.set(term, parent);
      tell("hook:agent", { term, state: "working", agent, session: `s-${term}`, model });
      if (state !== "working") tell("hook:agent", { term, state, agent, session: `s-${term}`, model });
    };
    window.__LEDGER__ = [
      rowOf({ term: worker, checkout: "/r/w-a", worker: "w-a", task: "워커 과업", task_id: "t-9902" }),
      ...finished.map((term, at) => rowOf({
        term, checkout: "/r/main", worker: `w-f${at}`, task: `끝난 과업 ${at}`, task_id: `t-990${3 + at}`,
        state: "done", ledger: "reclaimable", reported: true, progress: { stage: "merged" },
        // Landed work: nothing is left for the coordinator, so the pane folds into 「완료 N개」.
        review: { verified: true, merged: true, deployed: false, written: true, author: "coordinator" },
      })),
    ];
    window.__SEATS__ = [{ run: run.run, name: run.name, seat: true, term: lead, held: true, generation: run.generation, vacated_ms: null }];
    window.__ANSWER__.coordinator_seats = () => window.__SEATS__;
    panePrompts.set(lead, "검증까지 끝나면 main commit push");
    seat(lead, "/r/main", { state: "done" });
    seat(worker, "/r/w-a");
    for (const term of finished) seat(term, "/r/main", { state: "done", parent: lead });
    seat(resting, "/r/main", { agent: "codex", state: "done", model: "gpt-6-astra" });
    panePrompts.set(resting, "쉬는 코덱스");
    // A pane whose only finished children were helpers inside it: no pane of theirs ever stood.
    seat(solo, "/r/solo", { state: "done" });
    paneSubagents.set(solo, [{ id: "h-done", name: "explore", state: "done" }]);
    window.__ANSWER__.project_catalog = () => [{
      name: "r", path: "/r",
      external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
      worktrees: paths.map((path, at) => ({
        path, branch: path.slice(3), is_main: path === "/r/main", active: path === "/r/main", is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
      })),
    }];
    window.__ANSWER__.set_sidebar_view = () => null;
    setSidebarView({ groupBy: "repo", layout: "branches" });
    await refreshWorktrees();
    window.__LEDGER_MOVED__ = async (want = () => true) => {
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
      for (let tries = 0; tries < 80 && !want(); tries += 1) await new Promise((done) => setTimeout(done, 25));
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 150));
      await window.__PAINTED__();
    };
    window.__MOVE__ = async (term, over) => {
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.term === term ? { ...row, ...over } : row);
      await window.__LEDGER_MOVED__(() => {
        const fact = paneLedger.get(term);
        return fact && Object.entries(over).every(([key, value]) => (fact[key] ?? "") === (value ?? ""));
      });
    };
    await window.__LEDGER_MOVED__(() => paneLedger.size >= 4 && document.querySelector(".wt-agent.is-crew") !== null);
    const card = (path) => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
    window.__HISTORY__ = (path) => {
      const node = card(path)?.querySelector(".wt-agent.is-history");
      if (!node) return null;
      const fold = node.querySelector(":scope > .wt-agent-fold");
      return {
        name: node.querySelector(".wt-agent-name")?.textContent ?? "",
        parts: [...node.querySelectorAll(".wt-agent-reclaim")].map((one) => ({
          word: one.textContent, tone: [...one.classList].find((name) => name.startsWith("is-"))?.slice(3) ?? "",
        })),
        aria: node.getAttribute("aria-label") ?? "",
        tip: node.dataset.tip ?? "",
        expanded: node.getAttribute("aria-expanded"),
        inner: node.querySelectorAll('[role="button"], [tabindex]').length,
        foldHidden: fold?.getAttribute("aria-hidden") ?? null,
        foldExpanded: fold?.getAttribute("aria-expanded") ?? null,
      };
    };
    window.__CREW__ = (path) => {
      const node = card(path)?.querySelector(".wt-agent.is-crew");
      if (!node) return null;
      const mood = node.querySelector(".wt-agent-crew-mood");
      return {
        name: node.querySelector(".wt-agent-name")?.textContent ?? "",
        mood: mood?.textContent ?? "",
        tone: mood ? [...mood.classList].find((name) => name.startsWith("is-"))?.slice(3) ?? "" : "",
        tasks: node.querySelector(".wt-agent-crew-tasks")?.textContent ?? "",
        inner: node.querySelectorAll('[role="button"], [tabindex]').length,
      };
    };
    window.__ROW__ = (term) => {
      const node = document.querySelector(`.wt-agent[data-term="${term}"]:not(.is-history):not(.is-crew)`);
      if (!node) return null;
      const name = node.querySelector(".wt-agent-name");
      return {
        name: name?.textContent ?? "",
        nameWidth: name ? Math.round(name.getBoundingClientRect().width) : 0,
        clipped: name ? name.scrollWidth > name.clientWidth + 1 : false,
        model: node.querySelector(".wt-agent-model")?.textContent ?? null,
        status: node.querySelector(":scope > .wt-agent-state")?.textContent ?? "",
        statusInk: (() => {
          const state = node.querySelector(":scope > .wt-agent-state");
          return state ? getComputedStyle(state).color : "";
        })(),
        task: node.querySelector(".wt-agent-meta .wt-agent-task")?.textContent ?? "",
        lead: node.querySelector(".wt-agent-meta .wt-agent-lead")?.textContent ?? "",
        tip: node.dataset.tip ?? "",
        classes: node.className,
      };
    };
    window.__INKS__ = () => {
      const probe = document.createElement("span");
      document.body.appendChild(probe);
      probe.style.color = "var(--signal-wait-ink)";
      const wait = getComputedStyle(probe).color;
      probe.remove();
      return { wait };
    };
  }, { run: RUN, lead: LEAD, worker: WORKER, finished: FINISHED, resting: RESTING, solo: SOLO, paths: PATHS });
}

export const CREW_SCENE = Object.freeze({ lead: LEAD, worker: WORKER, finished: FINISHED, resting: RESTING, solo: SOLO, heldReason: HELD_REASON });

/* The row of a pane that folds its children: not the 「완료 N개」 line nor the crew line under it, which carry the same term. */
const foldingRow = (path, term) =>
  `.wt-agents[data-worktree-path="${path}"] .wt-agent[data-term="${term}"]:not(.is-history):not(.is-crew)`;

/* What a folding row says about itself, read off the drawn node. */
const foldFacts = (page, path, term) => page.evaluate(({ path, term }) => {
  const node = document.querySelector(`.wt-agents[data-worktree-path="${path}"] .wt-agent[data-term="${term}"]:not(.is-history):not(.is-crew)`);
  if (!node) return null;
  const chevron = node.querySelector(":scope > .wt-agent-fold");
  const model = worktreeAgentRows(path).find((row) => row.term === term && !row.sub && !row.history && !row.crew);
  return {
    tag: node.tagName,
    expanded: node.getAttribute("aria-expanded"),
    aria: node.getAttribute("aria-label") ?? "",
    summary: node.classList.contains("is-folded-summary"),
    folded: agentFolded.has(term),
    kids: model?.kids ?? 0,
    disclosure: model ? (model.kidsShown
      ? t("board.kidsHide", "자식 에이전트 {{count}}개 숨기기", { count: model.kids })
      : t("board.kidsShow", "자식 에이전트 {{count}}개 표시", { count: model.kids })) : "",
    chevronHidden: chevron?.getAttribute("aria-hidden") ?? null,
    chevronRole: chevron?.getAttribute("role") ?? null,
    chevronTab: chevron?.getAttribute("tabindex") ?? null,
    chevronExpanded: chevron?.getAttribute("aria-expanded") ?? null,
    chevronLabel: chevron?.getAttribute("aria-label") ?? null,
    inner: node.querySelectorAll('[role="button"], [tabindex]').length,
    focused: document.activeElement?.dataset?.term === String(term) && document.activeElement?.getAttribute("aria-expanded") !== null,
  };
}, { path, term });

/* What a helper's reading hand says about itself, read off the drawn nodes. The hand and its row are found either way the
 * hand may stand — inside the row's button, or beside it in the helper's line — so the same read judges both. */
const SOLO_CARD = '.wt-agents[data-worktree-path="/r/solo"]';
const RUNNING = "h-run";
const handFacts = (page, path, sub) => page.evaluate(({ path, sub }) => {
  const host = document.querySelector(`.wt-agents[data-worktree-path="${path}"]`);
  const row = host?.querySelector(`.wt-agent[data-sub="${sub}"]`) ?? null;
  const hand = row?.querySelector(".wt-agent-peek") ?? row?.parentElement?.querySelector(":scope > .wt-agent-peek") ?? null;
  if (!row || !hand) return null;
  const box = (node) => { const one = node.getBoundingClientRect(); return { left: one.left, right: one.right, top: one.top, bottom: one.bottom, width: one.width }; };
  const stops = [...host.querySelectorAll('button, [role="button"], [tabindex]:not([tabindex="-1"])')];
  const style = getComputedStyle(hand);
  return {
    tag: hand.tagName,
    role: hand.getAttribute("role"),
    label: hand.getAttribute("aria-label") ?? "",
    inRowButton: hand.parentElement?.closest('button, [role="button"]') !== null,
    nextStop: stops[stops.indexOf(row) + 1] === hand,
    opacity: style.opacity,
    events: style.pointerEvents,
    hand: box(hand),
    row: box(row),
    wash: getComputedStyle(row).backgroundColor,
  };
}, { path, sub });

export async function testSidebarCrewReclaim({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await standCrewReclaimScene(page);

    const words = await page.evaluate(() => ({
      going: t("agent.reclaim.going", "회수 중"),
      held: t("agent.crew.held", "회수 보류"),
      lead: t("agent.crew.lead", "코디 {{name}}", { name: "검증까지 끝나면 main commit push" }),
      crew: t("agent.crew.count", "워커 {{count}}", { count: 1 }),
      working: bucketWord("working"),
      history3: t("board.historyShow", "완료 {{count}}개 펼치기", { count: 3 }),
      history1: t("board.historyShow", "완료 {{count}}개 펼치기", { count: 1 }),
    }));

    /* ① — the crew line and the worker's second line. */
    const first = await page.evaluate(({ worker }) => ({ crew: window.__CREW__("/r/main"), worker: window.__ROW__(worker) }), { worker: WORKER });
    ok(
      "a coordinator's crew line reads 「워커 1 · 작업 중 · t-9902」 under its row, as one button with nothing pressable inside",
      first.crew?.name === words.crew && first.crew?.mood === words.working && first.crew?.tone === "work" &&
        first.crew?.tasks === "t-9902" && first.crew?.inner === 0,
      JSON.stringify(first.crew),
    );
    ok(
      "a worker at work says its task and its coordinator on its second line — 「t-9902 · 코디 …」 — and not its model",
      first.worker?.task === "t-9902" && first.worker?.lead === words.lead && first.worker?.model === null,
      JSON.stringify(first.worker),
    );

    /* ② and ③ — a reason alone turns 회수 중 into 보류, and the line breaks the panes down. */
    const going = await page.evaluate(() => window.__HISTORY__("/r/main"));
    ok(
      "three finished panes the window is closing read 「완료 3개 · 회수 중」 — one word when every pane says the same",
      going?.parts.length === 1 && going.parts[0].word === words.going && going.parts[0].tone === "busy",
      JSON.stringify(going),
    );
    const heldOnly = await page.evaluate(async ({ term, reason }) => {
      await window.__MOVE__(term, { release_held: reason });
      return { line: window.__HISTORY__("/r/main"), fact: paneLedger.get(term)?.release_held ?? null };
    }, { term: FINISHED[1], reason: HELD_REASON });
    ok(
      "a refused reclaim the ledger reports by release_held alone (its state still reclaimable) is drawn: the 「완료 N개」 line says 보류 in the caution's tone",
      heldOnly.fact === HELD_REASON && heldOnly.line?.parts.some((part) => part.tone === "held") &&
        heldOnly.line.tip.includes(HELD_REASON),
      JSON.stringify(heldOnly),
    );
    const broken = await page.evaluate(async ({ term }) => {
      await window.__MOVE__(term, { ledger: "release_unknown" });
      return {
        line: window.__HISTORY__("/r/main"),
        expected: [
          t("agent.reclaim.goingCount", "회수 중 {{count}}", { count: 1 }),
          t("agent.reclaim.heldCount", "보류 {{count}}", { count: 2 }),
        ],
      };
    }, { term: FINISHED[2] });
    ok(
      "panes in two reclaim states break the line down as the draft draws it — 「완료 3개 · 회수 중 1 · 보류 2」 — the 보류 part alone in the caution's tone, and the button's name says it too",
      JSON.stringify(broken.line?.parts.map((part) => part.word)) === JSON.stringify(broken.expected) &&
        broken.line.parts[0].tone === "busy" && broken.line.parts[1].tone === "held" &&
        broken.line.aria.includes(broken.expected[0]) && broken.line.aria.includes(broken.expected[1]),
      JSON.stringify(broken),
    );

    /* ④ — one button. */
    ok(
      "the 「완료 N개」 line is one button: nothing inside it is pressable, its chevron is aria-hidden, and the row itself carries aria-expanded",
      broken.line?.inner === 0 && broken.line.foldHidden === "true" && broken.line.foldExpanded === null &&
        broken.line.expanded === "false",
      JSON.stringify(broken.line),
    );

    /* ③ — opened, each row says its own reclaim. */
    const opened = await page.evaluate(async ({ finished, reason }) => {
      document.querySelector('.wt-agents[data-worktree-path="/r/main"] .wt-agent.is-history')?.click();
      await window.__PAINTED__();
      const rows = finished.map((term) => window.__ROW__(term));
      return {
        line: window.__HISTORY__("/r/main"), rows,
        going: t("agent.reclaim.going", "회수 중"),
        heldRow: t("agent.reclaim.heldRow", "보류 · {{reason}}", { reason }),
        heldBare: t("agent.reclaim.heldBare", "보류"),
        inks: window.__INKS__(),
      };
    }, { finished: FINISHED, reason: HELD_REASON });
    ok(
      "opened, each finished pane says its own reclaim at the right — 회수 중, 「보류 · <the reason>」 in the caution's ink, and a bare 보류 where the window could not confirm — with the reason in its tip and the name still read beside it, and the line says it is expanded",
      opened.line?.expanded === "true" &&
        opened.rows[0]?.status === opened.going &&
        opened.rows[1]?.status === opened.heldRow && opened.rows[1].tip.includes(HELD_REASON) && opened.rows[1].nameWidth >= 40 &&
        opened.rows[1].statusInk === opened.inks.wait &&
        opened.rows[2]?.status === opened.heldBare && opened.rows[2].statusInk === opened.inks.wait,
      JSON.stringify(opened),
    );
    await page.evaluate(async () => {
      document.querySelector('.wt-agents[data-worktree-path="/r/main"] .wt-agent.is-history')?.click();
      await window.__PAINTED__();
    });

    /* ② on the crew line — the worker's pane ends, then its reclaim is refused by a reason alone. */
    const crew = await page.evaluate(async ({ worker, reason }) => {
      window.__TELL__("hook:agent", { term: worker, state: "done", agent: "claude", session: `s-${worker}`, model: "claude-opus-5-5" });
      await window.__MOVE__(worker, { ledger: "reclaimable", state: "done" });
      const ending = window.__CREW__("/r/main");
      await window.__MOVE__(worker, { release_held: reason });
      const held = window.__CREW__("/r/main");
      return { ending, held };
    }, { worker: WORKER, reason: HELD_REASON });
    ok(
      "the coordinator's crew line says 회수 중 while the worker's finished pane is being closed, and 회수 보류 once the ledger gives a reason alone",
      crew.ending?.mood === words.going && crew.ending?.tone === "busy" &&
        crew.held?.mood === words.held && crew.held?.tone === "held",
      JSON.stringify(crew),
    );

    /* ⑤ — helpers only. */
    const solo = await page.evaluate(() => window.__HISTORY__("/r/solo"));
    ok(
      "a 「완료 N개」 that folds only finished helpers says no reclaim word — no pane ever stood to close",
      solo !== null && solo.name.includes("1") && solo.parts.length === 0 && !solo.aria.includes("회수") && !solo.tip.includes("회수"),
      JSON.stringify(solo),
    );

    /* ⑥ — a resting row's name reads at 313px. */
    const rest = await page.evaluate(({ lead, resting }) => ({
      lead: window.__ROW__(lead), resting: window.__ROW__(resting),
      width: getComputedStyle(document.documentElement).getPropertyValue("--sidebar-width").trim(),
    }), { lead: LEAD, resting: RESTING });
    ok(
      "a resting row wears no model chip — the model rides its tooltip — so at a 313px sidebar a coordinator's name is read, not cut to one letter",
      rest.width === "313px" &&
        rest.lead?.model === null && rest.lead.tip.includes("claude-opus-5-5") && rest.lead.nameWidth >= 120 &&
        rest.resting?.model === null && rest.resting.tip.includes("gpt-6-astra") && !rest.resting.clipped && !rest.lead.clipped,
      JSON.stringify(rest),
    );

    /* ⑦ — the words in every catalogue. */
    const missing = await page.evaluate(() => {
      const keys = ["agent.reclaim.goingCount", "agent.reclaim.heldCount", "agent.reclaim.doneCount", "agent.reclaim.openCount",
        "agent.reclaim.heldRow", "agent.reclaim.heldBare"];
      return ["en", "ja", "zh", "es"].flatMap((code) => keys.filter((key) => !CATALOG[code]?.[key]).map((key) => `${code}:${key}`));
    });
    ok("the reclaim breakdown's words read in English, Japanese, Chinese and Spanish", missing.length === 0, JSON.stringify(missing));

    /* ⑧ — a coordinator's row that folds child panes is one button too (r2). The scene has two such rows: the
     * coordinator's own (it holds three finished panes and a worker) and a pane with one finished helper. */
    const nested = await ownedChildrenViolations(page, ".wt-agents", ["nested-interactive"]);
    ok(
      "no row of the card list nests a pressable in its button — axe's nested-interactive finds nothing, the rows that fold child panes included",
      nested.length === 0,
      JSON.stringify(nested),
    );
    const standing = { lead: await foldFacts(page, "/r/main", LEAD), solo: await foldFacts(page, "/r/solo", SOLO) };
    ok(
      "a row that folds child panes is one button: its chevron is aria-hidden with no role, tab stop or label of its own, nothing pressable stands inside the row, and the row carries aria-expanded and the chevron's words in its name",
      Object.values(standing).every((one) => one !== null && one.tag === "BUTTON" && one.kids > 0 && one.expanded === "true" &&
        one.chevronHidden === "true" && one.chevronRole === null && one.chevronTab === null && one.chevronExpanded === null &&
        one.chevronLabel === null && one.inner === 0 && one.aria.endsWith(one.disclosure)),
      JSON.stringify(standing),
    );
    // The press goes where the stage would take it: stood in for, so only the call is read.
    await page.evaluate(() => {
      window.__WENT__ = [];
      window.__REAL_FOCUS__ = focusAgentPane;
      window.focusAgentPane = (...args) => { window.__WENT__.push(args[2]); return Promise.resolve(); };
    });
    const chevron = `${foldingRow("/r/main", LEAD)} > .wt-agent-fold`;
    await page.locator(chevron).click();
    const shut = {
      facts: await foldFacts(page, "/r/main", LEAD),
      went: await page.evaluate(() => [...window.__WENT__]),
      nested: await ownedChildrenViolations(page, ".wt-agents", ["nested-interactive"]),
    };
    ok(
      "a press on the chevron folds the lineage from the row's own press — the row shuts and stands as the folded summary, goes to no pane, and the folded summary nests nothing either",
      shut.facts?.folded === true && shut.facts.expanded === "false" && shut.facts.summary && shut.facts.chevronHidden === "true" &&
        shut.facts.inner === 0 && shut.facts.aria.endsWith(shut.facts.disclosure) && shut.went.length === 0 && shut.nested.length === 0,
      JSON.stringify(shut),
    );
    await page.locator(chevron).click();
    await page.locator(`${foldingRow("/r/main", LEAD)} .wt-agent-name`).click();
    const pressed = { facts: await foldFacts(page, "/r/main", LEAD), went: await page.evaluate(() => [...window.__WENT__]) };
    ok(
      "a press on the row anywhere but the chevron goes to its pane and folds nothing",
      pressed.facts?.folded === false && pressed.facts.expanded === "true" && pressed.went.length === 1 && pressed.went[0] === LEAD,
      JSON.stringify(pressed),
    );
    await page.evaluate(() => { window.focusAgentPane = window.__REAL_FOCUS__; });
    // The chevron takes no tab stop, so the tree's keys are the keyboard's way to the same fold.
    await page.focus(foldingRow("/r/main", LEAD));
    await page.keyboard.press("ArrowLeft");
    const left = await foldFacts(page, "/r/main", LEAD);
    await page.keyboard.press("ArrowLeft");
    const leftAgain = await foldFacts(page, "/r/main", LEAD);
    await page.keyboard.press("ArrowRight");
    const right = await foldFacts(page, "/r/main", LEAD);
    ok(
      "Left shuts and Right opens a row that folds child panes — a key that does nothing on a row already so — and the focus stays on the row after the card is drawn again",
      left?.folded === true && left.expanded === "false" && left.focused &&
        leftAgain?.folded === true && leftAgain.focused &&
        right?.folded === false && right.expanded === "true" && right.focused,
      JSON.stringify({ left, leftAgain, right }),
    );

    /* ⑨ — a helper at work wears a reading hand, and the hand is a button beside the row, not one inside it (r3). The
     * scene's pane SOLO holds one finished helper; a second one, at work, stands under it. */
    const peekWord = await page.evaluate(() => t("session.subagentOpenTranscript", "헬퍼 대화 보기"));
    await page.evaluate(async ({ term }) => {
      window.__TELL__("hook:subagent", { term, rows: [
        { id: "h-done", name: "explore", state: "done" },
        { id: "h-run", name: "price-research", state: "running", tool_calls: 4 },
      ] });
      await window.__PAINTED__();
    }, { term: SOLO });
    const helperRow = `${SOLO_CARD} .wt-agent[data-sub="${RUNNING}"]`;
    const working = await ownedChildrenViolations(page, ".wt-agents", ["nested-interactive"]);
    const atWork = await handFacts(page, "/r/solo", RUNNING);
    ok(
      "a helper at work wears its reading hand as a button beside its row: axe's nested-interactive finds nothing in the card list, the hand is no button inside a button, it is the next tab stop after its row, and it carries the helper's page words",
      working.length === 0 && atWork !== null && atWork.tag === "BUTTON" && atWork.role === null && !atWork.inRowButton &&
        atWork.nextStop && atWork.label === peekWord,
      JSON.stringify({ working, atWork }),
    );
    await page.hover(helperRow);
    await page.waitForTimeout(300);
    const shown = await handFacts(page, "/r/solo", RUNNING);
    await page.hover(`${SOLO_CARD} .wt-agent-peek`);
    const onHand = await handFacts(page, "/r/solo", RUNNING);
    ok(
      "the hand is silent at rest — no ink, no width, no touch — and shows at the row's right end, inside the row, when the row is pointed at; the row keeps its wash while the pointer is on the hand",
      atWork?.opacity === "0" && atWork.events === "none" && atWork.hand.width === 0 &&
        shown?.opacity === "1" && shown.events === "auto" && Math.round(shown.hand.width) === 16 &&
        Math.abs(shown.row.right - shown.hand.right - 4) <= 1 && shown.hand.top >= shown.row.top && shown.hand.bottom <= shown.row.bottom &&
        shown.wash !== atWork.wash && onHand?.wash === shown.wash,
      JSON.stringify({ atWork, shown, onHand }),
    );
    // Where a press goes: the call is read, the stage is not moved. The row opens the helper's page too (it has no pane of
    // its own), so a press on the hand that also reached the row would be two calls.
    await page.evaluate(() => {
      window.__OPENED__ = [];
      window.__WENT__ = [];
      window.__REAL_OPEN__ = openHelperPage;
      window.__REAL_FOCUS__ = focusAgentPane;
      window.openHelperPage = (row, sub) => { window.__OPENED__.push(sub.id); return Promise.resolve(); };
      window.focusAgentPane = (...args) => { window.__WENT__.push(args[2]); return Promise.resolve(); };
    });
    const said = () => page.evaluate(() => ({ opened: [...window.__OPENED__], went: [...window.__WENT__] }));
    await page.locator(`${SOLO_CARD} .wt-agent-peek`).click();
    const byHand = await said();
    await page.focus(helperRow);
    await page.keyboard.press("Tab");
    const tabbed = await page.evaluate(() => document.activeElement?.classList.contains("wt-agent-peek") ?? false);
    await page.keyboard.press("Enter");
    const byKey = await said();
    await page.keyboard.press("ArrowUp");
    const stepped = await page.evaluate(() => ({
      row: document.activeElement?.classList.contains("wt-agent") ?? false, term: document.activeElement?.dataset?.term ?? null,
    }));
    await page.evaluate(() => { window.openHelperPage = window.__REAL_OPEN__; window.focusAgentPane = window.__REAL_FOCUS__; });
    ok(
      "a press on the hand opens the helper's page once and goes to no pane; Tab from the row reaches the hand, Enter on it opens the page again, and the arrow keys step on to the row above",
      byHand.opened.length === 1 && byHand.opened[0] === RUNNING && byHand.went.length === 0 &&
        tabbed && byKey.opened.length === 2 && byKey.went.length === 0 && stepped.row && stepped.term === String(SOLO),
      JSON.stringify({ byHand, tabbed, byKey, stepped }),
    );
    // The finished helper in the opened 「완료 N개」 line wears the same hand.
    await page.evaluate(async () => {
      document.querySelector('.wt-agents[data-worktree-path="/r/solo"] .wt-agent.is-history')?.click();
      await window.__PAINTED__();
    });
    const openedLine = await ownedChildrenViolations(page, ".wt-agents", ["nested-interactive"]);
    const hands = await page.evaluate(() => [...document.querySelectorAll('.wt-agents[data-worktree-path="/r/solo"] .wt-agent-peek')]
      .map((hand) => ({ tag: hand.tagName, inRowButton: hand.parentElement?.closest('button, [role="button"]') !== null })));
    ok(
      "opened, the 「완료 N개」 line stands its finished helper with the same hand, and the card list still nests nothing",
      openedLine.length === 0 && hands.length === 2 && hands.every((hand) => hand.tag === "BUTTON" && !hand.inRowButton),
      JSON.stringify({ openedLine, hands }),
    );
  } finally {
    await page.close();
  }
}
