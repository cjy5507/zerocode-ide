import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar tells 작업 중, 검증 대기 and 완료 apart (t-18902).
 *
 * Reported 2026-09-30 23:4x from the person's own window: "네비바에 완료 작업중
 * 검증대기 표시를, 작업중일 때는 현재를 유지하고 구분이 되게 검증대기와 완료
 * 표시를 명확하게 네비바에 표시" — and again, "상태별 구분이 되면 좋겠어". A
 * worker that filed its `worker_done` and a checkout the coordinator verified,
 * merged or deployed both read 「완료」 in the sidebar, because `done` was one
 * thing there: an agent's turn ended. The ledger has always known the two
 * apart (`ledgerReviewWord`, the board's own words), and the row's tooltip
 * said the confusion out loud — 「완료 또는 대기 중」, two states in one
 * sentence.
 *
 * What stands here, all read off a laid-out page:
 *   - the 상태별 grouping has a 「검증 대기」 lane between 권한 필요 and 완료,
 *     and 완료 holds only what the ledger vouched for (or an agent with no
 *     ledger task at all, which keeps the rule it always had);
 *   - every row wears a mark and a word that differ in SHAPE and in WORDS, not
 *     in colour alone, and the tooltips stop sharing one sentence;
 *   - the agent rows hung under a card say the same thing;
 *   - a row does not hop between lanes under the pointer: the mark follows the
 *     ledger at once and the lane follows it after the frozen reading settles;
 *   - the five languages, both treatments, a narrow sidebar and reduced motion.
 *
 * Every fixture is built from the window's own roads — `hook:agent`,
 * `ledger:changed` over `ledger_agents` — and the page dies with the suite. */

/* The scene. Fourteen checkouts in one repository, each one a case the person can
 * meet: the ledger's rows are the shape `ledger_agents` sends — a seated worker
 * has a `term`, a released one whose work still stands has `settled` and the
 * checkout it left it in. */
const SCENE = [
  { path: "/r/ask", term: 8101, hook: "needs-attention" },
  { path: "/r/rep", term: 8102, hook: "done", task: "wire the beat", ledger: { reported: true } },
  { path: "/r/claim", term: 8103, hook: "done", task: "claim only",
    ledger: { reported: true, review: { claimed_merged: true, author: "worker" } } },
  { path: "/r/left", term: 8109, hook: "idle", task: "process left after reporting",
    ledger: { reported: true } },
  { path: "/r/ver", term: 8104, hook: "done", task: "verified work",
    ledger: { reported: true, review: { verified: true, written: true, author: "coordinator" } } },
  { path: "/r/own", term: 8105, hook: "done" },
  { path: "/r/work", term: 8106, hook: "working", task: "still going", ledger: { reported: false } },
  { path: "/r/gone", settled: { reported: true, task: "released, unreviewed" } },
  { path: "/r/landed", settled: { reported: true, task: "released, merged",
    review: { verified: true, merged: true, written: true, author: "coordinator" } } },
  { path: "/r/failed", term: 8107, hook: "done", task: "failed attempt",
    ledger: { reported: true, failed: true } },
  { path: "/r/quiet", term: 8108, hook: "done", task: "turn ended, nothing filed",
    ledger: { reported: false } },
  // A failed attempt whose task a sweep compacted away: no title, no task id —
  // the failed dispatch is all the ledger has left to say there was one.
  { path: "/r/compact", term: 8110, hook: "done", noTask: true, ledger: { reported: true, failed: true } },
  { path: "/r/lost", settled: { reported: true, failed: true, task: "released, failed" } },
  { path: "/r/idle" },
  // Done by hand after attempts that handed nothing in (t-19328): no review
  // record can ever be written for it, the ledger says so (`unreviewable`), and
  // it waits on nobody — not 검증 대기, and not a verified 완료 either.
  { path: "/r/norec", settled: { reported: true, task: "released, done by hand",
    review: { unreviewable: true } } },
  { path: "/r/norecfail", settled: { reported: true, failed: true, task: "released, done by hand after a failure",
    review: { unreviewable: true } } },
  { path: "/r/norecseat", term: 8111, hook: "done", task: "seated, done by hand",
    ledger: { reported: true, review: { unreviewable: true } } },
];

const WANT_GROUPS = {
  permission: ["/r/ask"],
  review: ["/r/rep", "/r/claim", "/r/left", "/r/gone"],
  done: ["/r/ver", "/r/own", "/r/landed"],
  working: ["/r/work"],
  active: ["/r/failed", "/r/quiet", "/r/compact"],
  inactive: ["/r/lost", "/r/idle", "/r/norec", "/r/norecfail", "/r/norecseat"],
};

/* What each row's dot should read, by the indicator the window draws. 완료 is
 * the ledger's word (verified, merged, deployed) or the turn of an agent the
 * ledger never gave a task; work the ledger holds that was never handed in, or
 * whose attempt failed, is neither, and the row does not say it is done — it
 * is a session standing there, which is all that is known. */
const WANT_INDICATOR = {
  "/r/ask": "permission", "/r/rep": "review", "/r/claim": "review", "/r/left": "review",
  "/r/ver": "done", "/r/own": "done", "/r/work": "working", "/r/gone": "review", "/r/landed": "done",
  "/r/failed": "active", "/r/quiet": "active", "/r/compact": "active", "/r/lost": "inactive",
  "/r/idle": "inactive", "/r/norec": "inactive", "/r/norecfail": "inactive", "/r/norecseat": "inactive",
};

/* The words that stood on one tooltip before (`worktree.stateIdle`), one per
 * catalog. A tooltip that says either of these says two states at once. */
const CONFLATED = ["완료 또는 대기 중", "Done or waiting", "完了または待機中", "已完成或等待中", "Terminado o en espera"];

const LOCALES = ["ko", "en", "ja", "zh", "es"];
const TALL = 2400;

/* The board's review stages (`AGENT_GRAPH_LIVE_STAGES`) and where the sidebar
 * places each: waiting for the coordinator, vouched by the coordinator, or
 * neither — the rule the sidebar always had. The list is closed on purpose: a
 * stage the board learns tomorrow makes this suite red instead of falling
 * through the sidebar unplaced. */
const WANT_PHASE = {
  reported: "review", "claimed-verified": "review", "claimed-merged": "review", "claimed-deployed": "review",
  verified: "vouched", merged: "vouched", deployed: "vouched", failed: "", closed: "closed",
  unreviewable: "unreviewable",
};

export async function testSidebarReviewState({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  const capture = process.env.SIDEBAR_REVIEW_CAPTURE ?? null;
  if (capture) await mkdir(capture, { recursive: true });
  try {
    // Tall on purpose: the sidebar builds only the rows a lane shows in view
    // (`renderSidebarWindow`), and every row of the scene has to stand.
    await page.setViewportSize({ width: 1280, height: TALL });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });

    /* ---- the scene, laid out ---------------------------------------------- */
    await page.evaluate(async (scene) => {
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const asked = (over = {}) => ({ verified: false, merged: false, deployed: false, written: false, ...over });
      const ledger = [];
      for (const one of scene) {
        if (one.term) {
          tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`,
            layout: { type: "leaf", term: one.term } });
          paneAgents.set(one.term, "claude");
          tell("hook:agent", { term: one.term, state: one.hook, agent: "claude", session: `s-${one.term}` });
        }
        const row = (over) => ({
          run: "run-1", worker: `w-${one.path.slice(3)}`, agent: "claude", state: "working",
          ledger: "active", hearing: "pending", hearing_at: 1, checkout: one.path,
          task: one.noTask ? "" : over.task ?? "", task_id: one.noTask ? "" : `t-${one.path.slice(3)}`,
          reported: false, failed: false,
          settled: false, session: null, at: 1, ...over, review: asked(over.review),
        });
        if (one.ledger) ledger.push(row({ ...one.ledger, term: one.term, task: one.task ?? "" }));
        if (one.settled) ledger.push(row({ ...one.settled, settled: true, term: undefined }));
      }
      window.__LEDGER__ = ledger;
      window.__ANSWER__.project_catalog = () => [{
        name: "r", path: "/r",
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: scene.map((one, at) => ({
          path: one.path, branch: one.path.slice(3), is_main: false, active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 9 });
      const seated = scene.filter((one) => one.ledger).length;
      const settled = scene.filter((one) => one.settled).length;
      for (let tries = 0; tries < 80 && !(paneLedger.size === seated && checkoutLedger.size === settled); tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      await refreshWorktrees();
      setSidebarView({ groupBy: "state" });
      await new Promise((done) => setTimeout(done, 120));
      // The settle is what takes the reading in a running window; here it is
      // taken by hand, once, so the scene stands before the first check.
      freezeWorkspaceActivity();
      await refreshWorktrees();
      await window.__PAINTED__();
      // The page-side hands the later steps share.
      window.__READ__ = {
        groups: () => [...document.querySelectorAll(".proj[data-state-group]")].map((node) => ({
          id: node.dataset.stateGroup,
          head: node.querySelector(":scope > .proj-row .proj-name")?.textContent ?? "",
          count: node.querySelector(":scope > .proj-row .state-group-count")?.textContent ?? "",
          mark: node.querySelector(":scope > .proj-row .wt-dot")?.className ?? "",
          rows: [...node.querySelectorAll(".wt-row")].map((row) => row.dataset.worktreePath),
        })),
        row: (path) => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`),
        shape: (dot) => {
          const pseudo = getComputedStyle(dot, "::before");
          return [pseudo.content, pseudo.width, pseudo.height, pseudo.borderTopWidth,
            pseudo.borderRightWidth, pseudo.borderBottomWidth, pseudo.borderLeftWidth,
            pseudo.borderRadius, pseudo.clipPath, pseudo.backgroundImage].join("|");
        },
        mark: (path) => {
          const row = window.__READ__.row(path);
          const dot = row.querySelector(".wt-dot");
          const chip = row.querySelector(".wt-phase");
          const shown = chip !== null && !chip.hidden && getComputedStyle(chip).display !== "none";
          return {
            path,
            state: dot.dataset.state ?? null,
            klass: dot.className,
            tip: dot.dataset.tip ?? "",
            aria: row.getAttribute("aria-label") ?? "",
            chip: shown ? chip.textContent : null,
            chipPhase: chip?.dataset.phase ?? null,
            chipTip: shown ? chip.dataset.tip ?? "" : null,
            shape: window.__READ__.shape(dot),
          };
        },
      };
    }, SCENE);

    /* ---- 1. the lanes ----------------------------------------------------- */
    const lanes = await page.evaluate(() => window.__READ__.groups());
    const want = await page.evaluate(() => ({
      review: t("board.awaitingReview", "검증 대기"),
      done: t("sidebar.stateDone", "완료"),
      working: t("sidebar.stateWorking", "작업 중"),
      needsYou: t("sidebar.stateNeedsYou", "권한 필요"),
      idle: t("sidebar.stateIdle", "비활성"),
    }));
    const membership = Object.fromEntries(lanes.map((lane) => [lane.id, lane.rows]));
    ok(
      "the 상태별 lanes stand 권한 필요, 검증 대기, 완료, 작업 중 — a worker that reported waits for review, and 완료 holds only what the ledger vouched for or an agent with no ledger task: a failed attempt and a turn that ended before its report are not done",
      JSON.stringify(lanes.map((lane) => lane.id)) === JSON.stringify(["permission", "review", "done", "working", "active", "inactive"]) &&
        JSON.stringify(membership) === JSON.stringify(WANT_GROUPS) &&
        lanes.find((lane) => lane.id === "review")?.head === want.review &&
        lanes.find((lane) => lane.id === "done")?.head === want.done &&
        lanes.find((lane) => lane.id === "review")?.count === "4",
      JSON.stringify({ lanes, want }),
    );

    /* The number: the 검증 대기 lane counts only work a coordinator can still
     * review. The same ledger as an older window's rows carried it — no
     * `unreviewable` anywhere — puts six checkouts there, exactly as it always
     * did; with the ledger's own fact the two reported ones whose task was done
     * by hand with nothing handed in leave it (the third was a failed attempt,
     * which the old reading called 실패, not waiting), and nothing that can be
     * reviewed does. */
    const counted = await page.evaluate(async () => {
      const count = () => window.__READ__.groups().find((lane) => lane.id === "review")?.rows.length ?? 0;
      const follow = async (want, payload) => {
        for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload });
        for (let tries = 0; tries < 80 && !(want()); tries += 1) await new Promise((done) => setTimeout(done, 25));
        freezeWorkspaceActivity();
        await refreshWorktrees();
        await window.__PAINTED__();
      };
      const after = count();
      const held = window.__LEDGER__;
      window.__LEDGER__ = held.map((row) => {
        const { unreviewable, ...review } = row.review;
        return { ...row, review };
      });
      await follow(() => paneLedger.get(8111)?.review?.unreviewable === undefined
        && checkoutLedger.get("/r/norec")?.review?.unreviewable === undefined, 30);
      const before = count();
      window.__LEDGER__ = held;
      await follow(() => paneLedger.get(8111)?.review?.unreviewable === true
        && checkoutLedger.get("/r/norec")?.review?.unreviewable === true, 31);
      return { before, after, restored: count() };
    });
    ok(
      "the 검증 대기 lane counts only what a coordinator can still review: the same ledger without the fact (an older ledger's rows) puts six checkouts there, with it four — the two reported ones whose task was done by hand with nothing handed in leave it",
      counted.before === 6 && counted.after === 4 && counted.restored === 4,
      JSON.stringify(counted),
    );

    /* ---- 2. every row, in shape and in words ------------------------------- */
    const marks = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => [path, window.__READ__.mark(path)])),
      SCENE.map((one) => one.path));
    const byShape = (paths) => new Set(paths.map((path) => marks[path].shape));
    const differentInShape = byShape(["/r/rep", "/r/ver", "/r/work", "/r/ask"]).size === 4 &&
      marks["/r/rep"].shape !== marks["/r/idle"].shape && marks["/r/ver"].shape !== marks["/r/idle"].shape;
    const sameKindSameShape = marks["/r/rep"].shape === marks["/r/claim"].shape &&
      marks["/r/rep"].shape === marks["/r/gone"].shape &&
      marks["/r/ver"].shape === marks["/r/own"].shape && marks["/r/ver"].shape === marks["/r/landed"].shape;
    ok(
      "each row's dot reads by the ledger — 검증 대기 and 완료 are different marks in shape, not in colour alone, and a claim, a released worker's work and a reported one all wait alike",
      Object.entries(WANT_INDICATOR).every(([path, indicator]) => marks[path].state === indicator) &&
        differentInShape && sameKindSameShape,
      JSON.stringify(Object.fromEntries(Object.entries(marks).map(([path, one]) => [path, { state: one.state, shape: one.shape }]))),
    );

    const tips = Object.fromEntries(Object.entries(marks).map(([path, one]) => [path, one.tip]));
    const distinctTips = new Set([tips["/r/rep"], tips["/r/ver"], tips["/r/own"], tips["/r/work"], tips["/r/ask"]]);
    ok(
      "each row says its state in words a person can read — 검증 대기 and 완료 wear a word on the row itself, working keeps the ring alone, and no tooltip is the old 「완료 또는 대기 중」",
      marks["/r/rep"].chip === want.review && marks["/r/claim"].chip === want.review &&
        marks["/r/left"].chip === want.review && marks["/r/compact"].chip === null &&
        marks["/r/gone"].chip === want.review && marks["/r/ver"].chip === want.done &&
        marks["/r/own"].chip === want.done && marks["/r/landed"].chip === want.done &&
        marks["/r/work"].chip === null && marks["/r/ask"].chip === null && marks["/r/idle"].chip === null &&
        marks["/r/failed"].chip === null && marks["/r/quiet"].chip === null &&
        // The word carries the whole sentence as its own tooltip.
        ["/r/rep", "/r/claim", "/r/gone", "/r/ver", "/r/own", "/r/landed"]
          .every((path) => marks[path].chipTip === marks[path].tip && marks[path].chipTip !== "") &&
        marks["/r/failed"].tip === marks["/r/quiet"].tip && marks["/r/compact"].tip === marks["/r/failed"].tip &&
        !marks["/r/failed"].tip.includes(want.done) &&
        distinctTips.size === 5 &&
        tips["/r/rep"].includes(want.review) && tips["/r/ver"].includes(want.done) && tips["/r/own"].includes(want.done) &&
        !Object.values(tips).some((tip) => CONFLATED.includes(tip)) &&
        Object.values(marks).filter((one) => one.state !== "working" && one.state !== "inactive")
          .every((one) => one.aria.endsWith(one.tip)),
      JSON.stringify({ tips, chips: Object.fromEntries(Object.entries(marks).map(([path, one]) => [path, one.chip])), want }),
    );

    /* The working row is what it was. */
    const working = await page.evaluate(() => {
      const dot = window.__READ__.row("/r/work").querySelector(".wt-dot");
      const pseudo = getComputedStyle(dot, "::before");
      return { klass: dot.className.replace(/ wt-spin-\d+/, ""), tip: dot.dataset.tip,
        width: pseudo.width, border: pseudo.borderTopWidth,
        want: t("worktree.stateStreaming", "작업 중") };
    });
    ok(
      "a working row is exactly what it was: the ring, its old tooltip, no word beside the name",
      working.klass === "wt-dot is-working" && working.tip === working.want &&
        working.width === "10px" && ["1px", "1.5px"].includes(working.border),
      JSON.stringify(working),
    );

    /* ---- 3. the agent rows under a card say the same ---------------------- */
    const agents = await page.evaluate(async () => {
      paintWorktreeAgents();
      await window.__PAINTED__();
      const read = (path) => {
        const host = document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
        const node = host?.querySelector(".wt-agent");
        if (!node) return null;
        const said = node.querySelector(".wt-agent-state")?.textContent ?? node.querySelector(".wt-agent-said")?.textContent ?? "";
        return {
          klass: node.className,
          glyph: node.querySelector(".wt-agent-dot use")?.getAttribute("href") ?? null,
          dotTip: node.querySelector(".wt-agent-dot")?.dataset.tip ?? "",
          said,
        };
      };
      return Object.fromEntries(["/r/rep", "/r/claim", "/r/left", "/r/ver", "/r/own", "/r/gone", "/r/landed", "/r/failed", "/r/quiet", "/r/compact", "/r/lost",
        "/r/norec", "/r/norecfail", "/r/norecseat"].map((path) => [path, read(path)]));
    });
    // The failed attempt's line, as the window words it — "" where the window has none.
    const failedTip = await page.evaluate(() =>
      typeof AGENT_FAILED_TIP === "undefined" ? "" : t(AGENT_FAILED_TIP.key, AGENT_FAILED_TIP.word));
    const wantWords = await page.evaluate(() => ({
      awaiting: t("board.awaitingReview", "검증 대기"), claimed: t("board.claimedMerged", "병합됐다 함"),
      verified: t("board.verified", "검증됨"), merged: t("board.merged", "병합됨"), done: t("board.done", "완료"),
      failed: t("board.desk.stageFailed", "실패"), idle: t("board.idle", "대기 중"),
      noRecord: t("board.noReviewRecord", "완료 — 검토 기록 없음"),
    }));
    ok(
      "the agent rows hung under a card wear the same distinction: an hourglass and 검증 대기 while the coordinator has not vouched, a check and the ledger's word once it has, a plain 완료 for an agent the ledger never seated, and a plain dot at rest — never a check — for a turn that ended before its report or a failed attempt",
      agents["/r/rep"]?.glyph === "#i-hourglass" && agents["/r/rep"].klass.includes("is-review") &&
        agents["/r/rep"].said === wantWords.awaiting &&
        agents["/r/claim"]?.glyph === "#i-hourglass" && agents["/r/claim"].said === wantWords.claimed &&
        agents["/r/ver"]?.glyph === "#i-circle-check" && agents["/r/ver"].klass.includes("is-verified") &&
        !agents["/r/ver"].klass.includes("is-review") && agents["/r/ver"].said === wantWords.verified &&
        agents["/r/own"]?.glyph === "#i-circle-check" && !agents["/r/own"].klass.includes("is-review") &&
        !agents["/r/own"].klass.includes("is-verified") && agents["/r/own"].said === wantWords.done &&
        agents["/r/gone"]?.glyph === "#i-hourglass" && agents["/r/gone"].klass.includes("is-review") &&
        agents["/r/gone"].said === wantWords.awaiting &&
        agents["/r/landed"]?.glyph === "#i-circle-check" && agents["/r/landed"].klass.includes("is-verified") &&
        agents["/r/landed"].said === wantWords.merged &&
        agents["/r/failed"]?.glyph === null && agents["/r/failed"].klass.includes("is-failed") &&
        agents["/r/failed"].said === wantWords.failed &&
        // A process that left with no Stop folds as done on the card, so its row
        // says the same: an hourglass, not a plain dot under a card's hourglass.
        agents["/r/left"]?.glyph === "#i-hourglass" && agents["/r/left"].klass.includes("is-review") &&
        agents["/r/left"].said === wantWords.awaiting &&
        // A failure whose task was compacted away is still a failure, on the row
        // and on the card.
        agents["/r/compact"]?.glyph === null && agents["/r/compact"].klass.includes("is-failed") &&
        agents["/r/compact"].said === wantWords.failed && agents["/r/compact"].dotTip === failedTip &&
        agents["/r/quiet"]?.glyph === null && agents["/r/quiet"].klass.includes("is-unsettled") &&
        agents["/r/quiet"].said === wantWords.idle &&
        // A failed attempt's dot says why, as far as the ledger knows — on a
        // live row and on the work a released worker left. Nothing else does.
        agents["/r/lost"]?.said === wantWords.failed && agents["/r/lost"].glyph === null &&
        failedTip !== "" && agents["/r/failed"].dotTip === failedTip && agents["/r/lost"].dotTip === failedTip &&
        ["/r/rep", "/r/claim", "/r/left", "/r/ver", "/r/own", "/r/gone", "/r/landed", "/r/quiet"]
          .every((path) => agents[path].dotTip === ""),
      JSON.stringify({ agents, wantWords, failedTip }),
    );
    // Work nothing can review says so in its own word — on a seated row whose turn
    // ended and on the work a released worker left, even after a failed attempt —
    // with a quiet check (nothing is hourglassed for a coordinator, and nothing
    // vouches for it either) and no failure tip.
    ok(
      "work a coordinator can never review wears its own word on the agent rows — 완료 — 검토 기록 없음, a quiet check, never the hourglass and 검증 대기, never green and 검증됨, and not 실패 after a failed attempt",
      ["/r/norec", "/r/norecfail", "/r/norecseat"].every((path) =>
        agents[path]?.said === wantWords.noRecord && agents[path].said !== wantWords.awaiting &&
        agents[path].glyph === "#i-circle-check" && !agents[path].klass.includes("is-review") &&
        !agents[path].klass.includes("is-verified") && !agents[path].klass.includes("is-failed") &&
        !agents[path].klass.includes("is-unsettled") && agents[path].dotTip === ""),
      JSON.stringify({ agents: [agents["/r/norec"], agents["/r/norecfail"], agents["/r/norecseat"]], wantWords }),
    );

    /* The other two places that word a workspace's state say it the same way: a
     * folded summary row's tooltip, and the workspace board's card. */
    const others = await page.evaluate(() => {
      const first = (path) => worktreeAgentRows(path).find((row) => !row.sub);
      const folded = (path) => foldedSummaryWords(first(path), agentRowState(first(path)), null).tip;
      const card = (path) => {
        const worktree = projects.flatMap((one) => one.worktrees).find((one) => one.path === path);
        return workspaceBoardCardNode(worktree, 0, false)
          .querySelector(".workspace-board-card-live strong")?.textContent ?? null;
      };
      // A released worker's work has no agent row: the card's repaint signature
      // has to see the ledger move on its own.
      const gone = projects.flatMap((one) => one.worktrees).find((one) => one.path === "/r/gone");
      const signed = workspaceBoardCardPaintSignature(gone);
      const held = checkoutLedger.get("/r/gone");
      checkoutLedger.set("/r/gone", { ...held, review: { ...held.review, verified: true } });
      const signedAfter = workspaceBoardCardPaintSignature(gone);
      checkoutLedger.set("/r/gone", held);
      return {
        cardRepaints: signed !== signedAfter,
        foldedReview: folded("/r/rep"), foldedOwn: folded("/r/own"),
        cardReview: card("/r/rep"), cardOwn: card("/r/own"), cardVouched: card("/r/landed"),
        want: { awaiting: t("board.awaitingReview", "검증 대기"), done: t("board.done", "완료"),
          review: t("worktree.stateReview", "검증 대기"), idle: worktreeStateLabel("idle") },
      };
    });
    ok(
      "a folded summary and the workspace board's card word a workspace's state as the sidebar does: 검증 대기 is never called 완료 there, and an agent the ledger never seated keeps its turn's word",
      others.foldedReview.includes(others.want.awaiting) && !others.foldedReview.includes(others.want.done) &&
        others.foldedOwn.includes(others.want.done) &&
        others.cardReview !== null && others.cardReview.startsWith(others.want.awaiting) &&
        others.cardOwn === others.want.idle && others.cardVouched !== others.want.idle &&
        others.cardRepaints,
      JSON.stringify(others),
    );

    /* ---- 4. no hop under the pointer -------------------------------------- */
    const hop = await page.evaluate(async () => {
      const seen = {};
      const lane = (path) => window.__READ__.groups().find((one) => one.rows.includes(path))?.id ?? null;
      const glyph = () => document.querySelector('.wt-agents[data-worktree-path="/r/rep"] .wt-agent-dot use')?.getAttribute("href") ?? null;
      seen.before = { lane: lane("/r/rep"), state: window.__READ__.mark("/r/rep").state, glyph: glyph() };
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-rep"
        ? { ...row, review: { ...row.review, verified: true, written: true, author: "coordinator" } } : row);
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 10 });
      for (let tries = 0; tries < 80 && !paneLedger.get(8102)?.review?.verified; tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      await window.__PAINTED__();
      await window.__PAINTED__();
      // The ledger has moved. The dot follows at once; the row stays where it
      // stands until the reading settles.
      seen.soon = { lane: lane("/r/rep"), state: window.__READ__.mark("/r/rep").state,
        chip: window.__READ__.mark("/r/rep").chip, glyph: glyph() };
      const started = performance.now();
      for (let tries = 0; tries < 200 && lane("/r/rep") !== "done"; tries += 1) {
        await new Promise((done) => setTimeout(done, 50));
      }
      seen.after = { lane: lane("/r/rep"), state: window.__READ__.mark("/r/rep").state,
        waitedMs: Math.round(performance.now() - started) };
      seen.settleMs = WORKSPACE_ACTIVITY_SETTLE_MS;
      return seen;
    });
    ok(
      "a row does not hop between lanes under the pointer: when the coordinator verifies, the dot turns to 완료 at once and the row keeps its lane until the frozen reading settles, then moves to 완료",
      hop.before.lane === "review" && hop.before.state === "review" && hop.before.glyph === "#i-hourglass" &&
        hop.soon.lane === "review" && hop.soon.state === "done" && hop.soon.chip === want.done &&
        hop.soon.glyph === "#i-circle-check" &&
        hop.after.lane === "done" && hop.after.state === "done" &&
        hop.after.waitedMs >= 1_000 && hop.after.waitedMs <= hop.settleMs + 4_000,
      JSON.stringify(hop),
    );

    /* A failed attempt's dot says why for as long as it is one. The ledger moving
     * between "not handed in yet" and "failed" changes the row's word in place,
     * and the line on the dot has to move with it — both ways. */
    const turning = await page.evaluate(async () => {
      const row = () => document.querySelector('.wt-agents[data-worktree-path="/r/quiet"] .wt-agent');
      const read = () => ({
        klass: row()?.className ?? "",
        tip: row()?.querySelector(".wt-agent-dot")?.dataset.tip ?? "",
        said: row()?.querySelector(".wt-agent-state")?.textContent ?? "",
      });
      const settle = async (until) => {
        for (let tries = 0; tries < 80 && !until(); tries += 1) await new Promise((done) => setTimeout(done, 25));
        await window.__PAINTED__();
        await window.__PAINTED__();
      };
      const held = window.__LEDGER__;
      const before = read();
      window.__LEDGER__ = held.map((one) => one.worker === "w-quiet" ? { ...one, reported: true, failed: true } : one);
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 11 });
      await settle(() => paneLedger.get(8108)?.failed === true);
      const failed = read();
      window.__LEDGER__ = held;
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 12 });
      await settle(() => paneLedger.get(8108)?.failed === false);
      const back = read();
      return { before, failed, back,
        want: typeof AGENT_FAILED_TIP === "undefined" ? "" : t(AGENT_FAILED_TIP.key, AGENT_FAILED_TIP.word),
        failedWord: t("board.desk.stageFailed", "실패"), idleWord: t("board.idle", "대기 중") };
    });
    ok(
      "a failed attempt's dot follows the ledger: it says why the moment the attempt fails and stops saying it when the ledger takes the failure back",
      turning.want !== "" &&
        turning.before.klass.includes("is-unsettled") && turning.before.tip === "" &&
        turning.before.said === turning.idleWord &&
        turning.failed.klass.includes("is-failed") && turning.failed.tip === turning.want &&
        turning.failed.said === turning.failedWord &&
        turning.back.klass.includes("is-unsettled") && turning.back.tip === "" &&
        turning.back.said === turning.idleWord,
      JSON.stringify(turning),
    );

    /* ---- 5. five languages ------------------------------------------------ */
    const languages = {};
    for (const code of LOCALES) {
      languages[code] = await page.evaluate(async (locale) => {
        setLocale(locale, { persist: false });
        await new Promise((done) => setTimeout(done, 200));
        await window.__PAINTED__();
        const words = (path) => window.__READ__.mark(path);
        const dotTip = (path) => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"] .wt-agent-dot`)?.dataset.tip ?? "";
        return {
          failedTip: { want: typeof AGENT_FAILED_TIP === "undefined" ? "" : t(AGENT_FAILED_TIP.key, AGENT_FAILED_TIP.word),
            live: dotTip("/r/failed"), settled: dotTip("/r/lost") },
          review: t("board.awaitingReview", "검증 대기"), done: t("sidebar.stateDone", "완료"),
          noRecord: t("board.noReviewRecord", "완료 — 검토 기록 없음"), verified: t("board.verified", "검증됨"),
          seatedNoRecord: document.querySelector('.wt-agents[data-worktree-path="/r/norecseat"] .wt-agent-state')?.textContent ?? "",
          heads: window.__READ__.groups().map((lane) => lane.head),
          gone: words("/r/gone"), landed: words("/r/landed"), own: words("/r/own"),
          shapes: [words("/r/gone").shape, words("/r/landed").shape],
        };
      }, code);
    }
    await page.evaluate(() => setLocale("ko", { persist: false }));
    ok(
      "the five languages each say 완료 — 검토 기록 없음 in a word of their own: not 검증 대기, not the verified word, not Korean, and the seated row follows the language",
      LOCALES.every((code) => {
        const one = languages[code];
        return one.noRecord !== one.review && one.noRecord !== one.verified && one.noRecord !== one.done &&
          one.seatedNoRecord === one.noRecord &&
          (code === "ko" || one.noRecord !== languages.ko.noRecord);
      }),
      JSON.stringify(Object.fromEntries(LOCALES.map((code) => [code, [languages[code].noRecord, languages[code].seatedNoRecord]]))),
    );
    const conflatedTip = (one) => CONFLATED.includes(one.gone.tip) || CONFLATED.includes(one.landed.tip) || CONFLATED.includes(one.own.tip);
    ok(
      "the five languages each say 검증 대기 and 완료 on the lane heads, on the row and in the tooltip — the words differ from each other, and none falls back to Korean",
      LOCALES.every((code) => {
        const one = languages[code];
        return one.review !== one.done &&
          // The failed attempt's tooltip follows the language on a row that was
          // rebuilt (the live one) and on one that was not (the released worker's).
          one.failedTip.want !== "" && one.failedTip.live === one.failedTip.want &&
          one.failedTip.settled === one.failedTip.want &&
          (code === "ko" || one.failedTip.want !== languages.ko.failedTip.want) &&
          one.heads.includes(one.review) && one.heads.includes(one.done) &&
          one.gone.chip === one.review && one.landed.chip === one.done && one.own.chip === one.done &&
          one.gone.tip.includes(one.review) && one.landed.tip.includes(one.done) &&
          one.gone.tip !== one.landed.tip && one.landed.tip !== one.own.tip &&
          !conflatedTip(one) &&
          (code === "ko" || (one.review !== languages.ko.review && one.done !== languages.ko.done &&
            one.gone.tip !== languages.ko.gone.tip && one.landed.tip !== languages.ko.landed.tip &&
            one.own.tip !== languages.ko.own.tip));
      }),
      JSON.stringify(languages),
    );

    /* ---- 6. contrast, both treatments, every state a row can be in ---------- */
    // A row's wash and its ink ease over 200 ms (`.wt-row`'s transition), and a
    // computed colour read mid-ease is a colour that is not on screen yet — so
    // the measure is taken with the easing off. The reduced-motion check below
    // reads the page as it ships.
    await page.addStyleTag({ content: "*, *::before, *::after { transition: none !important; }", });
    const contrast = {};
    for (const theme of ["dark", "light"]) {
      contrast[theme] = {};
      await page.evaluate(async (which) => {
        document.documentElement.dataset.theme = which;
        await window.__PAINTED__();
      }, theme);
      for (const state of ["rest", "hover", "active", "selected"]) {
       contrast[theme][state] = {};
       // One row at a time, so a hovered row is the row under the pointer: the
       // word on 완료 is measured hovered too, not read off a row at rest.
       for (const only of ["/r/claim", "/r/ver"]) {
        if (state === "hover") await page.hover(`.wt-row[data-worktree-path="${only}"]`);
        else await page.mouse.move(2, 2);
        await page.waitForTimeout(60);
        Object.assign(contrast[theme][state], await page.evaluate(({ mode, paths }) => {
          const parse = (css) => {
            const parts = css.match(/-?\d*\.?\d+(?:e-?\d+)?/gi)?.map(Number) ?? [];
            let [r = 0, g = 0, b = 0, a = 1] = parts;
            if (css.startsWith("color(")) [r, g, b] = [r * 255, g * 255, b * 255];
            return { r, g, b, a: parts.length > 3 ? a : 1 };
          };
          const over = (top, under) => ({
            r: top.r * top.a + under.r * (1 - top.a), g: top.g * top.a + under.g * (1 - top.a),
            b: top.b * top.a + under.b * (1 - top.a), a: 1,
          });
          const linear = (channel) => {
            const value = channel / 255;
            return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
          };
          const luminance = ({ r, g, b }) => 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
          const ratio = (front, back) => {
            const [high, low] = [luminance(front), luminance(back)].sort((a, b) => b - a);
            return (high + 0.05) / (low + 0.05);
          };
          const ground = (node) => {
            const layers = [];
            for (let at = node; at; at = at.parentElement) {
              const fill = parse(getComputedStyle(at).backgroundColor);
              if (fill.a > 0) layers.push(fill);
            }
            return layers.reduceRight((under, layer) => over(layer, under), { r: 255, g: 255, b: 255, a: 1 });
          };
          const seen = {};
          for (const path of paths) {
            const row = window.__READ__.row(path);
            const node = row.closest(".wt-node");
            if (mode === "active" || mode === "selected") {
              for (const one of document.querySelectorAll(".wt-row.is-active, .wt-node.is-active, .wt-row.is-selected, .wt-node.is-selected")) {
                one.classList.remove("is-active", "is-selected");
              }
              row.classList.add(`is-${mode}`);
              node.classList.add(`is-${mode}`);
            }
            const chip = row.querySelector(".wt-phase");
            const dot = row.querySelector(".wt-dot");
            const pseudo = getComputedStyle(dot, "::before");
            const markBack = ground(dot);
            const mark = over(parse(pseudo.color), markBack);
            // A row with no word has no contrast to read: it measures as 0, so
            // the check says so instead of the page throwing.
            let text = 0;
            if (chip) {
              const back = ground(chip);
              text = Number(ratio(over(parse(getComputedStyle(chip).color), back), back).toFixed(2));
            }
            seen[path] = { text, mark: Number(ratio(mark, markBack).toFixed(2)) };
            row.classList.remove("is-active", "is-selected");
            node.classList.remove("is-active", "is-selected");
          }
          return seen;
        }, { mode: state, paths: [only] }));
       }
      }
    }
    const floor = (measured, key) => Math.min(...Object.values(measured).flatMap((byState) =>
      Object.values(byState).flatMap((byPath) => Object.values(byPath).map((one) => one[key]))));
    await page.evaluate(() => {
      document.documentElement.dataset.theme = "dark";
      // The measure's own style leaves with the measure.
      for (const style of [...document.querySelectorAll("style")]) {
        if (style.textContent.startsWith("*, *::before, *::after { transition: none")) style.remove();
      }
    });
    ok(
      "the word and the mark on 검증 대기 and 완료 rows read at 4.5:1 or better on every state a row can be in, in both treatments",
      floor(contrast, "text") >= 4.5 && floor(contrast, "mark") >= 4.5,
      JSON.stringify({ textFloor: floor(contrast, "text"), markFloor: floor(contrast, "mark"), contrast }),
    );

    /* A beat that changes nothing writes nothing: the dot and the word are
     * corrected in place on every turn of every agent, and an attribute written
     * to the value it already holds still costs the accessibility tree a look.
     * The working ring's own turning (a class the JS spinner writes per step) is
     * the one thing that moves on a still list, and it is set aside. */
    const beat = await page.evaluate(async () => {
      await window.__PAINTED__();
      const records = [];
      const watch = new MutationObserver((list) => records.push(...list));
      watch.observe(worktreeList, { subtree: true, attributes: true, childList: true, characterData: true });
      paintWorktreeDots();
      paintWorktreeDots();
      paintWorktreeAgents();
      await window.__PAINTED__();
      watch.takeRecords().forEach((one) => records.push(one));
      watch.disconnect();
      const moved = records.filter((one) => !(one.type === "attributes" && one.attributeName === "class" &&
        one.target.classList.contains("is-working")));
      return { count: moved.length,
        sample: moved.slice(0, 4).map((one) => `${one.type}:${one.attributeName ?? ""}@${one.target.className ?? one.target.nodeName}`) };
    });
    ok(
      "a beat that changes nothing writes nothing: repainting the dots and the agent rows twice over a still list leaves no mutation on the rows, the marks or the words",
      beat.count === 0,
      JSON.stringify(beat),
    );

    /* The 잠자는 워크스페이스 숨기기 sweep is for workspaces with nothing going on.
     * A released worker's checkout has no pane in it, so it folds to empty and
     * would go with them — but its work that waits for a coordinator is what the
     * 검증 대기 lane is for, and it is the one thing the sweep must not take. */
    const sweep = await page.evaluate(async () => {
      window.__ANSWER__.set_hide_sleeping_workspaces = () => null;
      window.__ANSWER__.set_keep_default_branch_awake = () => null;
      setHideSleepingWorkspaces(true);
      await refreshWorktrees();
      await window.__PAINTED__();
      const kept = [...document.querySelectorAll(".wt-row")].map((row) => row.dataset.worktreePath);
      setHideSleepingWorkspaces(false);
      await refreshWorktrees();
      await window.__PAINTED__();
      return { kept, all: [...document.querySelectorAll(".wt-row")].length };
    });
    ok(
      "the sleeping sweep spares the work that waits for a coordinator: a released worker's unreviewed checkout stays on the list while a bare one, a released worker's failed one, a merged one and one nothing can review go",
      sweep.kept.includes("/r/gone") && !sweep.kept.includes("/r/idle") && !sweep.kept.includes("/r/lost") &&
        !sweep.kept.includes("/r/landed") && sweep.kept.includes("/r/rep") && sweep.all === 17 &&
        // Work nothing can review waits on nobody: the sweep takes it like a failed one.
        !sweep.kept.includes("/r/norec") && !sweep.kept.includes("/r/norecfail"),
      JSON.stringify(sweep),
    );

    /* ---- 7. narrow: the sidebar at its narrowest and a 360 px window ------- */
    // The rows that wear a word are the rows this change touches, so they are
    // the rows measured: a bare checkout's 「에이전트 창 없음」 chip is wider than
    // the narrowest sidebar, and always was — that is not this suite's subject.
    const fits = {};
    for (const [name, apply] of [
      ["narrowest", async () => { await page.setViewportSize({ width: 1280, height: TALL }); await page.evaluate(() => setPanelWidth("sidebar", 180)); }],
      ["phone", async () => { await page.setViewportSize({ width: 360, height: TALL }); }],
      ["default", async () => { await page.setViewportSize({ width: 1280, height: TALL }); await page.evaluate(() => setPanelWidth("sidebar", 280)); }],
    ]) {
      await apply();
      for (const code of ["es", "ko"]) {
        fits[`${name}-${code}`] = await page.evaluate(async (locale) => {
          setLocale(locale, { persist: false });
          await new Promise((done) => setTimeout(done, 200));
          await window.__PAINTED__();
          const wearing = [...document.querySelectorAll(".wt-row")].filter((row) => {
            const chip = row.querySelector(".wt-phase");
            return chip !== null && !chip.hidden && getComputedStyle(chip).display !== "none";
          });
          const scroller = document.getElementById("sidebar-scroll");
          const edge = scroller.getBoundingClientRect().right;
          const seen = wearing.map((row) => {
            const chip = row.querySelector(".wt-phase");
            const title = row.querySelector(".wt-title");
            const line = row.querySelector(".wt-topline");
            const box = row.getBoundingClientRect();
            const word = chip.getBoundingClientRect();
            return {
              path: row.dataset.worktreePath,
              rowOverflow: row.scrollWidth - row.clientWidth,
              lineOverflow: line.scrollWidth - line.clientWidth,
              wordOutside: word.right > box.right + 0.5 || word.left < box.left - 0.5,
              rowPastList: box.right > edge + 0.5,
              // A title stays readable: at least 24 px of it — two or three
              // characters and the ellipsis — or all of it when it is shorter.
              // The line is 67 px wide in the narrowest sidebar, and the word
              // takes what it needs of it, no more than half.
              titleShort: title.clientWidth < Math.min(title.scrollWidth, 24) - 1,
            };
          });
          // One row's parts, so a failure says WHERE the width went.
          const parts = (row) => row === undefined ? null : Object.fromEntries(
            [".wt-twist", ".wt-dot", ".wt-titlebox", ".wt-topline", ".wt-title", ".wt-phase"]
              .map((one) => [one.slice(4), Math.round(row.querySelector(one)?.getBoundingClientRect().width ?? -1)]));
          return {
            rowWidth: wearing.length ? Math.round(wearing[0].getBoundingClientRect().width) : 0,
            sample: parts(wearing.find((row) => row.dataset.worktreePath === "/r/claim")),
            wearing: wearing.length,
            bad: seen.filter((one) => one.rowOverflow > 1 || one.lineOverflow > 1 || one.wordOutside ||
              one.rowPastList || one.titleShort),
            pageScroll: document.documentElement.scrollWidth - document.documentElement.clientWidth,
          };
        }, code);
      }
    }
    await page.evaluate(() => setLocale("ko", { persist: false }));
    ok(
      "a row wearing its word fits the sidebar at its narrowest (180 px) and in a 360 px window, in Spanish where the word is longest: nothing overflows, the word stays inside the row, and the title keeps room to read",
      Object.values(fits).every((one) => one.wearing >= 2 && one.bad.length === 0 && one.pageScroll <= 0),
      JSON.stringify(fits),
    );

    /* The row is as tall with the word as without: nothing jumps when a row
     * turns from working to waiting for review to done. */
    const height = await page.evaluate(async () => {
      const at = (path) => window.__READ__.row(path).getBoundingClientRect().height;
      const chip = window.__READ__.row("/r/claim").querySelector(".wt-phase");
      const withWord = at("/r/claim");
      const held = chip?.style.display ?? "";
      if (chip) chip.style.display = "none";
      const withoutWord = at("/r/claim");
      if (chip) chip.style.display = held;
      return { withWord, withoutWord, working: at("/r/work"), verified: at("/r/ver"), own: at("/r/own"), hasChip: chip !== null };
    });
    ok(
      "the word does not change the row's height: a row is as tall with it as without, and as tall working as done",
      height.hasChip && Math.abs(height.withWord - height.withoutWord) < 0.5 &&
        Math.abs(height.working - height.verified) < 0.5 && Math.abs(height.own - height.verified) < 0.5,
      JSON.stringify(height),
    );

    /* ---- 8. reduced motion ------------------------------------------------ */
    await page.emulateMedia({ reducedMotion: "reduce" });
    const motion = await page.evaluate(async () => {
      await window.__PAINTED__();
      const still = (node, pseudo) => {
        const style = getComputedStyle(node, pseudo);
        return style.animationName === "none" &&
          style.transitionDuration.split(",").every((one) => parseFloat(one) === 0);
      };
      const seen = {};
      for (const path of ["/r/claim", "/r/ver"]) {
        const row = window.__READ__.row(path);
        const chip = row.querySelector(".wt-phase");
        seen[path] = { mark: still(row.querySelector(".wt-dot"), "::before"), chip: chip !== null && still(chip) };
      }
      for (const lane of document.querySelectorAll(".proj-row.is-state-group .wt-dot")) {
        seen[`lane:${lane.className}`] = { mark: still(lane, "::before") };
      }
      return seen;
    });
    await page.emulateMedia({ reducedMotion: "no-preference" });
    ok(
      "with reduced motion asked for, the 검증 대기 and 완료 marks and words draw still: no animation and no transition on the mark, the word or the lane heads",
      Object.values(motion).every((one) => Object.values(one).every(Boolean)) &&
        motion["/r/claim"] !== undefined && motion["/r/ver"] !== undefined,
      JSON.stringify(motion),
    );

    /* ---- 9. the seam is read, not copied ---------------------------------- */
    const seam = await page.evaluate(() => {
      if (typeof worktreeReviewPhase !== "function" || typeof ledgerReviewPhaseOf !== "function") return { missing: true };
      const facts = (review, over = {}) => ({ reported: true, failed: false, review, ...over });
      // Every stage the board can name is either placed on one of the two words
      // or left on the rule the sidebar always had. A stage the board learns
      // tomorrow lands here red instead of falling through silently.
      const stages = AGENT_GRAPH_LIVE_STAGES.map((one) => one.stage);
      const placed = Object.fromEntries(AGENT_GRAPH_LIVE_STAGES.map((one) => {
        const row = one.flag === "closed"
          ? { reported: true, closed: { kind: "outdated", why: "x" }, review: null }
          : one.flag === "reported" || one.flag === "failed"
            ? { reported: true, [one.flag]: true, review: null }
            : { reported: true, review: { [one.flag]: true } };
        return [one.stage, ledgerReviewPhaseOf(row)];
      }));
      return { stages, placed, none: ledgerReviewPhaseOf({ reported: false, review: null }),
        claimNotVouched: ledgerReviewPhaseOf(facts({ claimed_verified: true, verified: false })),
        // The attempt failed and the task was done by hand: the task's reading wins.
        unreviewableAfterFailure: ledgerReviewPhaseOf(facts({ unreviewable: true }, { failed: true })),
        // Between closed and vouched: something a coordinator vouched for is not
        // hidden by it, and it never hides what still waits.
        ranked: [WORKTREE_PHASE_RANK.closed, WORKTREE_PHASE_RANK.unreviewable, WORKTREE_PHASE_RANK.vouched,
          WORKTREE_PHASE_RANK.unsettled, WORKTREE_PHASE_RANK.review] };
    });
    ok(
      "the sidebar reads the board's own review stages: a worker's report or claim waits for review, a coordinator's verify, merge or deploy is 완료, and a failed or unreported one keeps the old rule — every stage the board can name is placed",
      seam.missing !== true &&
        JSON.stringify([...seam.stages].sort()) === JSON.stringify(Object.keys(WANT_PHASE).sort()) &&
        Object.entries(WANT_PHASE).every(([stage, phase]) => seam.placed[stage] === phase) &&
        seam.none === "" && seam.claimNotVouched === "review" &&
        seam.unreviewableAfterFailure === "unreviewable" &&
        seam.ranked.every((rank, at) => Number.isInteger(rank) && (at === 0 || rank > seam.ranked[at - 1])),
      JSON.stringify(seam),
    );

    /* ---- the pair of pictures --------------------------------------------- */
    if (capture) {
      const shoot = async (name) => {
        for (const theme of ["dark", "light"]) {
          await page.evaluate(async (which) => { document.documentElement.dataset.theme = which; await window.__PAINTED__(); }, theme);
          await page.locator(".threads").screenshot({ path: resolve(capture, `${name}-${theme}.png`), animations: "disabled" });
        }
      };
      await page.evaluate(async () => {
        setLocale("ko", { persist: false });
        setSidebarView({ groupBy: "state" });
        await new Promise((done) => setTimeout(done, 150));
        freezeWorkspaceActivity();
        await refreshWorktrees();
        paintWorktreeAgents();
        await window.__PAINTED__();
      });
      await shoot("state");
      await page.evaluate(async () => {
        setSidebarView({ groupBy: "repo" });
        await new Promise((done) => setTimeout(done, 150));
        await refreshWorktrees();
        paintWorktreeAgents();
        await window.__PAINTED__();
      });
      await shoot("repo");
    }
  } finally {
    await page.close();
  }
}
