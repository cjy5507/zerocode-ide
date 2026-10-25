import { openWindowTestPage } from "./window-boot.mjs";

/* A coordinator's sub-agents that finished, under it in the branch list (t-66539).
 *
 * Asked 2026-10-20 21:3x from the person's own window, over a picture of the branch list: 「왜 애는 서브에이전트가
 * 안보여?」. A zo coordinator of a zo delegation run stood alone on its project's card, while the Claude coordinator on
 * the next card wore a worker row and a 「완료 1개」 fold under it. A zo delegation worker works in its coordinator's own
 * project folder; it is released when it hands its work in, its pane goes, and the work it left — still waiting on the
 * coordinator's review — is a ledger row the card said on its line and drew nowhere under the coordinator: a checkout's
 * finished work is drawn only on a card with no live row (`paintWorktreeAgents`), and the coordinator's own pane is one.
 *
 * The scene is a synthetic ledger built from the window's own roads (`hook:agent`, `hook:subagent`, `ledger:changed`
 * over `ledger_agents` and `coordinator_seats`, `project_catalog`): three projects, three coordinators. The ledger sends
 * one finished-work row a checkout, the newest, as the backend does (`settled_workers`). Nobody's conversation, screen
 * or settings is read; the page dies with the suite.
 *   ① the work a released zo worker handed in, still waiting on its zo delegation coordinator, stands under the
 *      coordinator's row: its task, 「넘김 끝 · 검토 대기」 and why it is late, 검증 대기, 워커, no pane;
 *   ② pressing it opens that task's artifacts, with the report the ledger kept chosen;
 *   ③ work that had nothing to land or landed folds into 「완료 N개」 under the coordinator, and opening the fold stands it;
 *   ④ one road: a Claude coordinator over a Claude worker's work in its own folder draws the same rows;
 *   ⑤ the numbers: the rows under the zo delegation coordinator before and after its work arrives, the Claude
 *      coordinator's rows unchanged by it, and the team view, which listed the work all along.
 *
 * `ok` details carry the numbers the report quotes. */

/* The runs, each coordinator in its own project's main checkout. */
const LEAD = { run: "run-lead", name: "묶음 런", generation: 3, term: 9601, path: "/r/main", agent: "claude", model: "claude-opus-5-5" };
const DELEGATION = { run: "run-delegation", name: "zo delegation", generation: 2, term: 9701, path: "/p/main", agent: "zo", model: "gpt-6-astra" };
const SAME = { run: "run-same", name: "같은 폴더 런", generation: 1, term: 9801, path: "/q/main", agent: "claude", model: "claude-opus-5-5" };

/* The Claude coordinator's side, as the picture shows it: a worker at work in the coordinator's own checkout, and a
 * helper that finished. */
const LEAD_WORKER = { term: 9611, worker: "w-r1", task: "대화 화면 2단계", task_id: "t-1001" };
const LEAD_HELPER = { id: "h-explore", name: "Explore", state: "done", born_listed: false };

/* The finished work a checkout holds, as the ledger sends it: released, seatless, its attempt closed. */
const SETTLED = Object.freeze({
  delegation: { run: DELEGATION.run, worker: "w-z6", agent: "zo", checkout: DELEGATION.path, task: "넘긴 조사", task_id: "t-7006",
    pane: "%4", model: "gpt-6-astra", handed_in: "m-7007", minutes: 240, stalled: ["no_review"] },
  same: { run: SAME.run, worker: "w-q2", agent: "claude", checkout: SAME.path, task: "같은 폴더의 넘긴 일", task_id: "t-8002",
    pane: "%3", model: "claude-sonnet-5-5", handed_in: "q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2q2", minutes: 90, stalled: [] },
});

const PROJECTS = [
  { name: "r", path: "/r", main: LEAD.path },
  { name: "p", path: "/p", main: DELEGATION.path },
  { name: "q", path: "/q", main: SAME.path },
];

export async function testSidebarTeamSettled({ browser, origin, ok, faults }) {
  const now = Date.now();
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });

    /* ---- the scene, at first without the finished work: what the card drew before it arrived ---- */
    await page.evaluate(async ({ runs, projects, worker, helper, settled, now }) => {
      setLocale("ko", { persist: false });
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      window.__ROW_OF__ = (over) => ({
        run: "", worker: "", agent: "claude", state: "working", ledger: "active", hearing: "pending", hearing_at: 1,
        checkout: "", task: "", task_id: "", reported: false, failed: false, settled: false, session: null, at: 1,
        review: { verified: false, merged: false, deployed: false, written: false, author: null }, review_since_ms: null,
        closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null, kept: null, taken_over: false,
        pane: "", stalled: [], ...over,
      });
      /* A finished-work row the way `ledger_agents` sends it (t-10993): the worker released, no seat. `over` moves
       * where the work stands. */
      window.__SETTLED__ = (one, over = {}) => {
        const { minutes, ...said } = one;
        return window.__ROW_OF__({
          ...said, state: "waiting", ledger: "released", hearing: "gone", reported: true, settled: true,
          at: now - minutes * 60_000, progress: { stage: "report_only" }, ...over,
        });
      };
      for (const one of runs) {
        tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
        paneAgents.set(one.term, one.agent);
        tell("hook:agent", { term: one.term, state: "working", agent: one.agent, session: `s-${one.term}`, model: one.model });
      }
      // The Claude coordinator's worker, summoned into the coordinator's own checkout, and its finished helper.
      tabs.push({ kind: "term", worktree: runs[0].path, id: `tab-${worker.term}`, layout: { type: "leaf", term: worker.term } });
      paneAgents.set(worker.term, "claude");
      paneParents.set(worker.term, runs[0].term);
      tell("hook:agent", { term: worker.term, state: "working", agent: "claude", session: `s-${worker.term}`, model: "claude-sonnet-5-5" });
      tell("hook:subagent", { term: runs[0].term, rows: [helper] });
      window.__LEDGER__ = [window.__ROW_OF__({ run: runs[0].run, worker: worker.worker, term: worker.term, checkout: runs[0].path,
        task: worker.task, task_id: worker.task_id, pane: "%2", model: "claude-sonnet-5-5" })];
      window.__SEATS__ = runs.map((one) => ({ run: one.run, name: one.name, seat: true, term: one.term, held: true,
        generation: one.generation, vacated_ms: null, mail: 0, news: 0, landing: 0, batched: 0 }));
      window.__ANSWER__.coordinator_seats = () => window.__SEATS__;
      window.__ANSWER__.project_catalog = () => projects.map((project, at) => ({
        name: project.name, path: project.path,
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: [{ path: project.main, branch: "main", is_main: true, active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at }],
      }));
      window.__ANSWER__.set_sidebar_view = () => null;
      // The scene is the branch list's: with a coordinator sitting, the team view would stand in its place.
      setSidebarView({ groupBy: "repo", layout: "branches" });
      await refreshWorktrees();
      // The ledger moved: what the window does on its own beat. Waits for `want`, then for the paint after it.
      window.__LEDGER_MOVED__ = async (want = () => true) => {
        for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
        for (let tries = 0; tries < 80 && !want(); tries += 1) await new Promise((done) => setTimeout(done, 25));
        await window.__PAINTED__();
        await new Promise((done) => setTimeout(done, 120));
        await window.__PAINTED__();
      };
      await window.__LEDGER_MOVED__(() => paneLedger.size >= 1 && coordinatorSeats.length === runs.length);
      /* What a person reads of one card's agent rows, in order. */
      window.__AGENT_ROWS__ = (path) => {
        const host = document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
        if (!host || host.hidden) return [];
        return [...host.children].filter((node) => node.classList.contains("wt-agent") && node.getClientRects().length > 0)
          .map((node) => {
            const text = (selector) => node.querySelector(selector)?.textContent ?? "";
            const role = node.querySelector(":scope > .wt-agent-role");
            return {
              term: node.dataset.term ?? "",
              work: node.dataset.work ?? "",
              history: node.dataset.history ?? "",
              depth: Number(node.dataset.depth ?? 0),
              name: text(".wt-agent-name"),
              said: text(".wt-agent-said"),
              status: text(".wt-agent-state"),
              role: role?.textContent ?? "",
              roleTip: role?.dataset.tip ?? "",
              review: node.classList.contains("is-review"),
              done: node.classList.contains("is-done"),
              model: text(".wt-agent-model"),
              fold: node.querySelector(":scope > .wt-agent-fold:not(.is-quiet)")?.getAttribute("aria-label") ?? "",
              tip: node.dataset.tip ?? "",
              aria: node.getAttribute("aria-label") ?? "",
              tag: node.tagName,
            };
          });
      };
      window.__WORDS__ = () => ({
        handedIn: t("worktree.stage.handedIn", "넘김 끝 · 검토 대기"),
        noReview: t("worktree.stall.noReview", "검증 늦음"),
        nothing: t("worktree.stage.nothing", "착지할 것 없음"),
        landed: t("worktree.stage.landed", "착지됨"),
        awaiting: t("board.awaitingReview", "검증 대기"),
        nothingWord: t("board.nothingToLand", "완료 — 착지할 것 없음"),
        merged: t("board.merged", "병합됨"),
        worker: t("agent.role.worker", "워커"),
        kidsOne: t("board.kidsHide", "자식 에이전트 {{count}}개 숨기기", { count: 1 }),
        history1: t("board.historyCount", "완료 {{count}}개", { count: 1 }),
        pane4: t("sidebar.team.pane", "판 {{pane}}", { pane: "%4" }),
      });
    }, { runs: [LEAD, DELEGATION, SAME], projects: PROJECTS, worker: LEAD_WORKER, helper: LEAD_HELPER, settled: SETTLED, now });

    const words = await page.evaluate(() => window.__WORDS__());
    const before = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => [path, window.__AGENT_ROWS__(path)])),
      [LEAD.path, DELEGATION.path, SAME.path]);

    /* ---- the finished work arrives: the newest a checkout holds, as the backend sends it ---- */
    const after = await page.evaluate(async ({ settled, paths }) => {
      window.__LEDGER__ = [...window.__LEDGER__, window.__SETTLED__(settled.delegation), window.__SETTLED__(settled.same)];
      await window.__LEDGER_MOVED__(() => checkoutLedger.size >= 2);
      return Object.fromEntries(paths.map((path) => [path, window.__AGENT_ROWS__(path)]));
    }, { settled: SETTLED, paths: [LEAD.path, DELEGATION.path, SAME.path] });
    const under = (rows, term) => rows.filter((row) => row.depth > 0 && row.term !== String(term));
    const zoRows = after[DELEGATION.path] ?? [];
    const work = zoRows.find((row) => row.work === SETTLED.delegation.worker) ?? null;

    /* ① — the handed-in work stands under its coordinator. */
    ok(
      "a released zo worker's handed-in work stands under its zo delegation coordinator in the branch list — one row right after the coordinator's, one level in, with its task for a name",
      zoRows.length === 2 && zoRows[0]?.term === String(DELEGATION.term) && zoRows[1]?.work === SETTLED.delegation.worker &&
        work?.depth === 1 && work?.name === SETTLED.delegation.task,
      JSON.stringify({ before: before[DELEGATION.path], after: zoRows }),
    );
    ok(
      "the row says where the work stands as the card's own line says it — 「넘김 끝 · 검토 대기」 and why it is late — 검증 대기 at the right, the hourglass on its dot, 워커 on its chip, and names no pane",
      work !== null && work.said === `${words.handedIn} · ${words.noReview}` && work.status === words.awaiting && work.review &&
        work.role === words.worker && work.roleTip.includes(SETTLED.delegation.worker) &&
        !`${work.tip}\n${work.aria}`.includes(SETTLED.delegation.pane) && !work.aria.includes(words.pane4),
      JSON.stringify(work),
    );
    ok(
      "the coordinator's row counts the work among its children and folds it away with them",
      zoRows[0]?.fold === words.kidsOne,
      JSON.stringify(zoRows[0] ?? null),
    );

    /* ② — the row opens what the work handed in. */
    const opened = await page.evaluate(async ({ settled, path }) => {
      const kept = { state: "kept", kept: 2, left_out: 0, bytes: 2048, report: "artifact-report-7006", reasons: [], masked: 0 };
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === settled.worker ? { ...row, kept } : row);
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.kept?.report === kept.report);
      const host = document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const node = host?.querySelector(`.wt-agent[data-work="${CSS.escape(settled.worker)}"]`) ?? null;
      const front = activeTabId;
      node?.click();
      for (let tries = 0; tries < 40 && artifactsTab() === null; tries += 1) await new Promise((done) => setTimeout(done, 25));
      const read = {
        pressable: node?.tagName === "BUTTON",
        artifacts: artifactsTab() !== null,
        filter: artifactFilter.origin ?? null,
        chosen: artifactSelectedId ?? null,
        movedPane: activeTabId !== front && tabs.find((tab) => tab.id === activeTabId)?.kind === "term",
      };
      const opened = artifactsTab();
      if (opened) closeTab(opened.id);
      await window.__PAINTED__();
      return read;
    }, { settled: SETTLED.delegation, path: DELEGATION.path });
    ok(
      "pressing the work's row opens that task's artifacts with the report the ledger kept chosen — the desk's own door — and brings no pane forward",
      opened.pressable && opened.artifacts && opened.filter?.field === "task" && opened.filter?.value === SETTLED.delegation.task_id &&
        opened.chosen === "artifact-report-7006" && !opened.movedPane,
      JSON.stringify(opened),
    );
    const told = await page.evaluate(() => ({
      tip: t("agent.settledWorkTip", "판이 닫힌 워커가 넘긴 일 — 누르면 이 과업의 보고서와 증거를 아티팩트에서 엽니다"),
      missing: ["en", "ja", "zh", "es"].filter((code) => !CATALOG[code]?.["agent.settledWorkTip"]),
    }));
    ok(
      "the row's tip says where its press goes — it has no pane — and the word reads in English, Japanese, Chinese and Spanish",
      work !== null && work.tip.endsWith(`\n${told.tip}`) && told.missing.length === 0,
      JSON.stringify({ tip: work?.tip ?? null, missing: told.missing }),
    );

    /* ③ — finished work folds into 완료 N개. */
    const folded = await page.evaluate(async ({ settled, path }) => {
      const moved = (over) => {
        window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === settled.worker ? { ...row, ...over } : row);
      };
      const coordinator = { verified: false, merged: false, deployed: false, written: true, author: "coordinator" };
      moved({ review: { ...coordinator, nothing_to_land: true }, progress: { stage: "nothing_to_land" }, stalled: [] });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "nothing_to_land");
      const shut = window.__AGENT_ROWS__(path);
      document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"] .wt-agent[data-history]`)?.click();
      await window.__PAINTED__();
      const open = window.__AGENT_ROWS__(path);
      document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"] .wt-agent[data-history]`)?.click();
      await window.__PAINTED__();
      moved({ review: { ...coordinator, verified: true, merged: true }, progress: { stage: "merged" } });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "merged");
      const merged = window.__AGENT_ROWS__(path);
      // Back to the work that waits, for the steps after this one.
      moved({ review: { verified: false, merged: false, deployed: false, written: false, author: null },
        progress: { stage: "report_only" }, stalled: ["no_review"] });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "report_only");
      return { shut, open, merged };
    }, { settled: SETTLED.delegation, path: DELEGATION.path });
    const historyOf = (rows) => rows.find((row) => row.history !== "") ?? null;
    ok(
      "work that had nothing to land folds into 「완료 1개」 under its coordinator — no row of its own stands — and opening the fold stands it, saying 착지할 것 없음",
      folded.shut.length === 2 && historyOf(folded.shut)?.name === words.history1 && historyOf(folded.shut)?.depth === 1 &&
        !folded.shut.some((row) => row.work !== "") &&
        folded.open.some((row) => row.work === SETTLED.delegation.worker && row.said === words.nothing &&
          row.status === words.nothingWord && row.done && !row.review),
      JSON.stringify(folded),
    );
    ok(
      "landed work folds the same way, into 「완료 1개」",
      folded.merged.length === 2 && historyOf(folded.merged)?.name === words.history1 &&
        !folded.merged.some((row) => row.work !== ""),
      JSON.stringify(folded.merged),
    );

    /* ④ — one road for every run and every agent. */
    const sameRows = after[SAME.path] ?? [];
    const sameWork = sameRows.find((row) => row.work === SETTLED.same.worker) ?? null;
    ok(
      "a Claude coordinator over a Claude worker's handed-in work in its own folder draws the same rows — one road, whatever the run or the agent",
      sameRows.length === 2 && sameRows[0]?.term === String(SAME.term) && sameWork?.depth === 1 &&
        sameWork?.name === SETTLED.same.task && sameWork?.said === words.handedIn && sameWork?.status === words.awaiting &&
        sameWork?.role === words.worker && sameWork?.review,
      JSON.stringify(sameRows),
    );

    /* ⑥ — t-73910: an attempt that ended with no result seen is 중단, never 실패. On 2026-10-25 two released workers
     * (w-73948, w-73950) stood painted 「실패」 while the ledger held neither as failed (`succeeded: null`). */
    const ended = await page.evaluate(async ({ settled, path }) => {
      const moved = (over) => {
        window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === settled.worker ? { ...row, ...over } : row);
      };
      const read = () => {
        const host = document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
        const node = host?.querySelector(`.wt-agent[data-work="${CSS.escape(settled.worker)}"]`) ?? null;
        return {
          status: node?.querySelector(".wt-agent-state")?.textContent ?? null,
          said: node?.querySelector(".wt-agent-said")?.textContent ?? null,
          dotTip: node?.querySelector(".wt-agent-dot")?.dataset.tip ?? null,
          text: node?.textContent ?? null,
        };
      };
      moved({ handed_in: null, reported: true, failed: false, stopped: true, stalled: [], progress: { stage: "stopped" } });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "stopped");
      const stopped = read();
      // The team view keeps the row while it waits on its coordinator, and its strip says 중단 at 넘김.
      setSidebarView({ layout: "team" });
      for (let tries = 0; tries < 100 && !sidebarTeamStood; tries += 1) await new Promise((done) => setTimeout(done, 20));
      const key = `w:${settled.run}/${settled.worker}`;
      await window.__LEDGER_MOVED__(() => document.querySelector(`#sidebar-team [data-team-key="${CSS.escape(key)}"]`) !== null);
      const row = document.querySelector(`#sidebar-team [data-team-key="${CSS.escape(key)}"]`);
      const team = {
        stands: row !== null,
        strip: row?.querySelector(".team-strip")?.dataset.tip ?? null,
        held: [...(row?.querySelectorAll(".team-step") ?? [])].map((cell) => cell.className),
      };
      setSidebarView({ layout: "branches" });
      await window.__PAINTED__();
      moved({ stopped: false, failed: true, progress: { stage: "failed" } });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "failed");
      const failed = read();
      // A stage this window has no word for — a newer ledger's — draws no word and breaks nothing.
      moved({ failed: false, progress: { stage: "a_stage_from_a_newer_ledger" } });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "a_stage_from_a_newer_ledger");
      const unknown = read();
      moved({ handed_in: settled.handed_in, stalled: ["no_review"], progress: { stage: "report_only" } });
      await window.__LEDGER_MOVED__(() => checkoutLedger.get(path)?.progress?.stage === "report_only");
      return {
        stopped, failed, unknown, team,
        words: {
          stopped: t("board.stageStopped", "중단"),
          failed: t("board.desk.stageFailed", "실패"),
          stoppedTip: t(AGENT_STOPPED_TIP.key, AGENT_STOPPED_TIP.word),
          failedTip: t(AGENT_FAILED_TIP.key, AGENT_FAILED_TIP.word),
        },
        missing: ["en", "ja", "zh", "es"].filter((code) =>
          !CATALOG[code]?.["board.stageStopped"] || !CATALOG[code]?.["worktree.attemptStoppedTip"]),
      };
    }, { settled: SETTLED.delegation, path: DELEGATION.path });
    ok(
      "a released worker's attempt that ended with no result seen reads 중단 on its row and its dot — the word 실패 appears nowhere on it — and one the worker reported failed still reads 실패",
      ended.stopped.status === ended.words.stopped && ended.stopped.said === ended.words.stopped &&
        ended.stopped.dotTip === ended.words.stoppedTip && !ended.stopped.text?.includes(ended.words.failed) &&
        ended.failed.status === ended.words.failed && ended.failed.dotTip === ended.words.failedTip &&
        ended.words.stoppedTip !== ended.words.failedTip && ended.missing.length === 0,
      JSON.stringify(ended),
    );
    ok(
      "the team view keeps the stopped worker's row under its coordinator, its strip's tip says 중단 and never 실패, and 넘김 is held",
      ended.team.stands && ended.team.strip?.includes(ended.words.stopped) && !ended.team.strip?.includes(ended.words.failed) &&
        ended.team.held[3]?.includes("is-stuck"),
      JSON.stringify(ended.team),
    );
    ok(
      "a stage word this window does not know breaks nothing and is said as no word — never 실패, 중단 or a stage it is not: the row says nothing or does not stand, and the page raises no error",
      (ended.unknown.status === null || ended.unknown.status === "") &&
        ![ended.words.failed, ended.words.stopped].some((word) => ended.unknown.text?.includes(word)) && faults.length === 0,
      JSON.stringify({ unknown: ended.unknown, faults }),
    );

    /* ⑤ — the numbers, and the team view. */
    const lead = { before: before[LEAD.path] ?? [], after: after[LEAD.path] ?? [] };
    ok(
      "the numbers: under the zo delegation coordinator 0 sub-agent rows before its work arrived and 1 after; the Claude coordinator's rows — its worker and 「완료 1개」 — are the same rows before and after",
      under(before[DELEGATION.path] ?? [], DELEGATION.term).length === 0 && under(zoRows, DELEGATION.term).length === 1 &&
        JSON.stringify(lead.before) === JSON.stringify(lead.after) && lead.after.length === 3 &&
        lead.after.some((row) => row.term === String(LEAD_WORKER.term)) && lead.after.some((row) => row.history !== ""),
      JSON.stringify({
        delegation: { before: under(before[DELEGATION.path] ?? [], DELEGATION.term).length, after: under(zoRows, DELEGATION.term).length },
        lead: { before: lead.before.length, after: lead.after.length },
      }),
    );
    const team = await page.evaluate(async (run) => {
      setSidebarView({ layout: "team" });
      for (let tries = 0; tries < 100 && !sidebarTeamStood; tries += 1) await new Promise((done) => setTimeout(done, 20));
      await window.__LEDGER_MOVED__(() => document.querySelector(`#sidebar-team [data-team-key="c:${run}"]`) !== null);
      const keys = [...document.querySelectorAll("#sidebar-team > .team-row")].map((node) => node.dataset.teamKey);
      setSidebarView({ layout: "branches" });
      await window.__PAINTED__();
      return keys;
    }, DELEGATION.run);
    const card = team.indexOf(`c:${DELEGATION.run}`);
    ok(
      "the team view listed the same work under the same coordinator all along — the picture was the branch list's",
      card >= 0 && team[card + 1] === `w:${DELEGATION.run}/${SETTLED.delegation.worker}`,
      JSON.stringify(team),
    );
  } finally {
    await page.close().catch(() => {});
  }
}
