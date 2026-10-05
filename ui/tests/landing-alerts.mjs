import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The ledger's alerts about late landings, as the window words them (t-34501 stage 2, t-22105).
 *
 * The backend decides what is late and writes `landing_stalled` to the coordinator and `branch_drifted`
 * to a live worker; what stands here is the window's half, read off a laid-out page:
 *   - a verified task the coordinator marked as having no code to land reads 「완료 — 착지할 것 없음」,
 *     which is not 병합 and not 검증 대기 — and a worker's word for it, or a merge beside it, changes
 *     nothing the board already says;
 *   - the desk draws that as a stage of its own, outside the flow, and a `landing_stalled` letter says
 *     which of the five things is late and for how long, in the words each reason has;
 *   - the sidebar says how long the work has waited — 「검증 대기 3시간」 — and says nothing at all, never
 *     zero, for a row whose ledger kept no time to count from; a verification nobody merged says so
 *     in the tooltip, in the same terms. */

const HOUR = 3600_000;

/* The rows of the sidebar's scene, counted back from the moment the suite RUNS: the window suite imports
 * every suite at its start and reaches this one minutes later, so a clock read at import made
 * 「reported a moment ago」 seven minutes old in the whole-suite run. */
const sceneAt = (NOW) => [
  { path: "/r/rev", settled: { reported: true, task: "waits for a review", review_since_ms: NOW - 3 * HOUR } },
  { path: "/r/revold", settled: { reported: true, task: "an older row, no time kept" } },
  { path: "/r/revnow", settled: { reported: true, task: "reported a moment ago", review_since_ms: NOW - 20_000 } },
  { path: "/r/ver", settled: { reported: true, task: "verified, never merged",
    review: { verified: true, merged: false, written: true, author: "coordinator" }, review_since_ms: NOW - 2 * HOUR } },
  { path: "/r/verold", settled: { reported: true, task: "verified, time not kept",
    review: { verified: true, merged: false, written: true, author: "coordinator" } } },
  { path: "/r/merged", settled: { reported: true, task: "merged",
    review: { verified: true, merged: true, written: true, author: "coordinator" } } },
  { path: "/r/nothing", settled: { reported: true, task: "research",
    review: { verified: true, nothing_to_land: true, written: true, author: "coordinator" } } },
];

export async function testLandingAlerts({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); setLocale("ko", { persist: false }); });

    /* ---- the board's own words ---- */
    const board = await page.evaluate(() => {
      const row = (review, over = {}) => ({ reported: true, failed: false, review: { verified: false, merged: false, deployed: false, ...review }, ...over });
      const stage = (id) => DESK_STAGES.find((one) => one.id === id) ?? null;
      return {
        nothing: ledgerReviewWord(row({ verified: true, nothing_to_land: true })),
        mergedBeside: ledgerReviewWord(row({ verified: true, merged: true, nothing_to_land: true })),
        claimed: ledgerReviewWord(row({ claimed_nothing_to_land: true })),
        unverified: ledgerReviewWord(row({ nothing_to_land: false })),
        phase: ledgerReviewPhaseOf(row({ verified: true, nothing_to_land: true })),
        liveStage: AGENT_GRAPH_LIVE_STAGES.find((one) => one.stage === "nothing-to-land")?.flag ?? null,
        deskStage: stage("nothing_to_land"),
        over: DESK_OVER_STAGES.includes("nothing_to_land"),
        flow: DESK_STAGES.filter((one) => one.flow).map((one) => one.id),
      };
    });
    ok(
      "a verified task marked as having no code to land reads 완료 — 착지할 것 없음: not 병합 (a merge beside the mark wins), not 검증 대기 (a worker's claim leaves it waiting), counted as vouched; the board's live stages know it and the desk draws it outside the flow",
      board.nothing === "완료 — 착지할 것 없음" && board.mergedBeside === "병합됨" &&
        board.claimed === "검증 대기" && board.unverified === "검증 대기" && board.phase === "vouched" &&
        board.liveStage === "nothing_to_land" && board.deskStage?.word === "완료 — 착지할 것 없음" &&
        board.deskStage?.flow === false && board.over === true && !board.flow.includes("nothing_to_land"),
      JSON.stringify(board),
    );

    /* ---- the desk's letter ---- */
    const letters = await page.evaluate(() => {
      const now = Date.now();
      const said = (reason, over = {}) => deskLetterDetail({ kind: "landing_stalled", reason, since_ms: now - 3 * 3600_000, ...over }, now);
      return {
        mail: DESK_MAIL.landing_stalled ?? null,
        reasons: Object.fromEntries(["no_review", "no_merge", "merged_unrecorded", "cleanable", "ledger_merged_git_not"].map((one) => [one, said(one)])),
        unknown: said("something_new"),
        unaged: said("no_review", { since_ms: null }),
      };
    });
    const heard = Object.values(letters.reasons);
    ok(
      "a landing_stalled letter says which of the five things is late in words of its own and for how long, a reason the window does not know stands in the ledger's own word, and a letter with no time says no time",
      letters.mail?.state === "needs-attention" && letters.mail.word === "착지 점검" &&
        new Set(heard).size === 5 && heard.every((one) => one.includes("3시간째")) &&
        letters.reasons.no_review.includes("검증") && letters.reasons.no_merge.includes("병합 기록") &&
        letters.reasons.merged_unrecorded.includes("git") && letters.reasons.cleanable.includes("정리") &&
        letters.reasons.ledger_merged_git_not.includes("git") &&
        letters.unknown.startsWith("something_new") && !letters.unaged.includes("째"),
      JSON.stringify(letters),
    );

    /* ---- the sidebar's time ---- */
    const SCENE = sceneAt(Date.now());
    await page.evaluate(async (scene) => {
      const asked = (over = {}) => ({ verified: false, merged: false, deployed: false, written: false, ...over });
      const ledger = [];
      for (const one of scene) {
        ledger.push({
          run: "run-1", worker: `w-${one.path.slice(3)}`, agent: "claude", state: "working",
          ledger: "active", hearing: "pending", hearing_at: 1, checkout: one.path,
          task: one.settled.task, task_id: `t-${one.path.slice(3)}`, reported: false, failed: false,
          session: null, at: 1, ...one.settled, settled: true, review: asked(one.settled.review),
        });
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
      for (let tries = 0; tries < 80 && checkoutLedger.size !== scene.length; tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
    }, SCENE);
    const rows = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => {
      const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
      return [path, {
        phase: row?.querySelector(".wt-phase")?.textContent ?? null,
        phaseTip: row?.querySelector(".wt-phase")?.dataset.tip ?? "",
        dotTip: row?.querySelector(".wt-dot")?.dataset.tip ?? "",
      }];
    })), SCENE.map((one) => one.path));
    ok(
      "the sidebar says how long a report has waited — 검증 대기 3시간 — and a verification nobody merged says so in the tooltip; a row whose ledger kept no time, or a report less than a minute old, says no time at all (never zero), and a merged or nothing-to-land row is never called late",
      rows["/r/rev"].phase === "검증 대기 3시간" && rows["/r/rev"].dotTip.includes("3시간") &&
        rows["/r/revold"].phase === "검증 대기" && !/\d/.test(rows["/r/revold"].dotTip) &&
        rows["/r/revnow"].phase === "검증 대기" &&
        rows["/r/ver"].dotTip.includes("병합 기록 없이 2시간") && rows["/r/ver"].phase === "완료" &&
        !rows["/r/verold"].dotTip.includes("병합 기록 없이") &&
        !rows["/r/merged"].dotTip.includes("병합 기록 없이") && !rows["/r/nothing"].dotTip.includes("병합 기록 없이"),
      JSON.stringify(rows),
    );

    // Pictures of the waiting rows for the stage report, when asked for (LANDING_ALERTS_CAPTURE=dir).
    const capture = process.env.LANDING_ALERTS_CAPTURE ?? null;
    if (capture) {
      await mkdir(capture, { recursive: true });
      for (const [name, width] of [["", null], ["-narrow", "180px"]]) {
        for (const theme of ["dark", "light"]) {
          await page.evaluate(async ([which, wide]) => {
            document.documentElement.dataset.theme = which;
            if (wide) document.documentElement.style.setProperty("--sidebar-width", wide);
            else document.documentElement.style.removeProperty("--sidebar-width");
            await window.__PAINTED__();
          }, [theme, width]);
          await page.locator(".threads").screenshot({ path: resolve(capture, `waiting-${theme}${name}.png`), animations: "disabled" });
        }
      }
    }
  } finally {
    await page.close();
  }
}
