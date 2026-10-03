import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar and the git panel say whether a checkout's work is in main, from
 * git (t-22104).
 *
 * Reported 2026-10-03 00:0x from the person's own window: 「네비바에 실제로
 * 반영이 된건지 안된건지 사용자는 알수없다」. The row's words came from the
 * ledger alone (검증 대기 / 완료), and 완료 was one word for verified, merged
 * and deployed — none of them a look at git. The backend now sends
 * `landing` with every checkout row (`worktree_landing.rs`); what stands here
 * is the window's half, read off a laid-out page:
 *   - every kind of landing wears its own word on a second chip beside the
 *     ledger's, and 커밋 없음 is never 반영됨;
 *   - the ledger and git disagreeing is 확인 필요 with both facts in the
 *     tooltip, in both directions, and the ledger's own dot keeps its word;
 *   - 정리 가능 stands only on a landed checkout with no session and nothing
 *     unsaved, and the chip opens the clean-up review that already exists;
 *   - the tooltip names the compare ref and when it last moved;
 *   - the git panel's head says the same words from the same function. */

const REF = "origin/main";
const NOW = Date.now();
const LANDED_IN = { sha: "0123456789abcdef0123456789abcdef01234567", time_ms: NOW - 3 * 3600_000 };
const base = (state, over = {}) => ({
  state, detached: false, ahead: 0, dirty: false, dirty_checked_ms: NOW - 1000, compare_ref: REF, ref_updated_ms: NOW - 2 * 86_400_000, ...over,
});

const SCENE = [
  { path: "/r/landed", landing: base("landed", { landed_in: LANDED_IN }) },
  { path: "/r/busy", term: 9101, hook: "working", landing: base("landed") },
  { path: "/r/ahead", landing: base("unlanded", { ahead: 13 }) },
  { path: "/r/nocommit", landing: base("no_commits") },
  { path: "/r/dirty", landing: base("landed", { dirty: true }) },
  { path: "/r/lm", settled: { reported: true, task: "ledger says merged",
    review: { verified: true, merged: true, written: true, author: "coordinator" } },
  landing: base("unlanded", { ahead: 2 }) },
  { path: "/r/gl", term: 9102, hook: "done", task: "reported only", ledger: { reported: true },
    landing: base("landed") },
  { path: "/r/vouched", settled: { reported: true, task: "merged and in git",
    review: { verified: true, merged: true, written: true, author: "coordinator" } },
  landing: base("landed", { landed_in: LANDED_IN }) },
  { path: "/r/noref", landing: base("no_ref", { compare_ref: "origin/gone", ref_updated_ms: null }) },
  { path: "/r/det", landing: base("landed", { detached: true }) },
  { path: "/r/unk", landing: base("unknown") },
  { path: "/r/fail", landing: { state: "failed", detached: false, ahead: 0, dirty: false } },
  { path: "/r/pend", landing: { state: "pending", detached: false, ahead: 0, dirty: false } },
  { path: "/r/plain" },
];

const WANT = {
  "/r/landed": { word: "반영됨 · 정리 가능", tone: "landed", cleanable: true },
  "/r/busy": { word: "반영됨", tone: "landed", cleanable: false },
  "/r/ahead": { word: "미반영 13", tone: "ahead", cleanable: false },
  "/r/nocommit": { word: "커밋 없음", tone: "none", cleanable: false },
  "/r/dirty": { word: "반영됨 · 저장 안 한 변경", tone: "landed", cleanable: false },
  "/r/lm": { word: "확인 필요", tone: "check", cleanable: false },
  "/r/gl": { word: "확인 필요", tone: "check", cleanable: false },
  "/r/vouched": { word: "반영됨 · 정리 가능", tone: "landed", cleanable: true },
  "/r/noref": { word: "비교 기준 없음", tone: "none", cleanable: false },
  "/r/det": { word: "반영됨 · 정리 가능", tone: "landed", cleanable: true },
  "/r/unk": { word: "확인 필요", tone: "check", cleanable: false },
  "/r/pend": { word: "확인 중", tone: "pending", cleanable: false },
  "/r/fail": { word: "확인 실패", tone: "check", cleanable: false },
};

export async function testSidebarLandingState({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  const capture = process.env.SIDEBAR_LANDING_CAPTURE ?? null;
  if (capture) await mkdir(capture, { recursive: true });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });
    await page.evaluate(async (scene) => {
      setLocale("ko", { persist: false });
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const asked = (over = {}) => ({ verified: false, merged: false, deployed: false, written: false, ...over });
      const ledger = [];
      for (const one of scene) {
        if (one.term) {
          tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
          paneAgents.set(one.term, "claude");
          tell("hook:agent", { term: one.term, state: one.hook, agent: "claude", session: `s-${one.term}` });
        }
        const row = (over) => ({
          run: "run-1", worker: `w-${one.path.slice(3)}`, agent: "claude", state: "working",
          ledger: "active", hearing: "pending", hearing_at: 1, checkout: one.path,
          task: over.task ?? "", task_id: `t-${one.path.slice(3)}`, reported: false, failed: false,
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
          ...(one.landing ? { landing: one.landing } : {}),
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 9 });
      const seated = scene.filter((one) => one.ledger).length;
      const settled = scene.filter((one) => one.settled).length;
      for (let tries = 0; tries < 80 && !(paneLedger.size === seated && checkoutLedger.size === settled); tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
      window.__CHIP__ = (path) => {
        const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
        const chip = row?.querySelector(".wt-landing");
        const shown = chip && getComputedStyle(chip).display !== "none" && chip.textContent !== "";
        return {
          row: Boolean(row),
          word: shown ? chip.textContent : null,
          tone: chip?.dataset.landing ?? null,
          tip: shown ? chip.dataset.tip ?? "" : null,
          cleanable: chip?.dataset.cleanable === "1",
          dotTip: row?.querySelector(".wt-dot")?.dataset.tip ?? "",
          phaseWord: row?.querySelector(".wt-phase")?.textContent ?? "",
          height: row ? row.getBoundingClientRect().height : 0,
        };
      };
    }, process.env.SIDEBAR_LANDING_BEFORE
      // The picture of what a backend that says nothing about git draws: the
      // same scene without the one field this change adds.
      ? SCENE.map(({ landing, ...one }) => one)
      : SCENE);
    if (process.env.SIDEBAR_LANDING_BEFORE) {
      for (const theme of ["dark", "light"]) {
        await page.evaluate(async (which) => { document.documentElement.dataset.theme = which; await window.__PAINTED__(); }, theme);
        await page.locator(".threads").screenshot({ path: resolve(capture, `landing-${theme}.png`), animations: "disabled" });
      }
      return;
    }

    const chips = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => [path, window.__CHIP__(path)])),
      SCENE.map((one) => one.path));

    ok(
      "each kind of landing wears its own word on a second chip beside the ledger's: 반영됨, 미반영 N, 커밋 없음, 저장 안 한 변경, 비교 기준 없음 — and 커밋 없음 is never 반영됨",
      Object.entries(WANT).every(([path, want]) => chips[path].word === want.word && chips[path].tone === want.tone &&
        chips[path].cleanable === want.cleanable) && chips["/r/plain"].word === null &&
        chips["/r/nocommit"].word !== "반영됨",
      JSON.stringify(chips),
    );

    ok(
      "the ledger and git disagreeing is 확인 필요 with BOTH facts in the tooltip — the ledger's merged against git's two missing commits, and git's landing against a report nobody recorded as merged — and the ledger's own dot keeps its word",
      chips["/r/lm"].tip.includes("원장에는 병합됨") && chips["/r/lm"].tip.includes(`${REF}에 없는 커밋 2개`) &&
        chips["/r/gl"].tip.includes("원장에는 병합 기록이 없습니다") && chips["/r/gl"].tip.includes(REF) &&
        chips["/r/gl"].phaseWord === "검증 대기" && chips["/r/lm"].phaseWord === "완료" &&
        chips["/r/gl"].dotTip.includes("검증 대기") && !chips["/r/gl"].dotTip.includes("반영"),
      JSON.stringify({ lm: chips["/r/lm"], gl: chips["/r/gl"] }),
    );

    ok(
      "the tooltip names the ref compared with and how long ago it last moved, the main commit that took the work (nine characters) and its time, and says so for a detached HEAD, a missing ref and an unknown",
      chips["/r/landed"].tip.includes(`비교: ${REF}`) && chips["/r/landed"].tip.includes("마지막 갱신 2일 전") &&
        chips["/r/landed"].tip.includes("012345678 ·") && !chips["/r/landed"].tip.includes("0123456789") &&
        chips["/r/landed"].tip.includes(new Date(LANDED_IN.time_ms).toLocaleString()) &&
        chips["/r/det"].tip.includes("분리된 HEAD") &&
        chips["/r/noref"].tip.includes("origin/gone") && !chips["/r/noref"].tip.includes("origin/gone를") && chips["/r/noref"].tip.includes("시각을 알 수 없음") &&
        chips["/r/unk"].tip.includes("가를 수 없습니다") &&
        chips["/r/ahead"].tip.includes(`${REF}에 없는 커밋이 13개`) &&
        chips["/r/dirty"].tip.includes("커밋하지 않은 변경") &&
        // When the unsaved mark was read is said in every tooltip git spoke in.
        Object.entries(chips).filter(([path, one]) => one.tip && !["/r/pend", "/r/fail"].includes(path))
          .every(([, one]) => one.tip.includes("저장 안 한 변경은") && one.tip.includes("에 확인한 값입니다")) &&
        chips["/r/pend"].tip.includes("확인하는 중") && chips["/r/fail"].tip.includes("다시 시도합니다"),
      JSON.stringify(Object.fromEntries(Object.entries(chips).map(([path, one]) => [path, one.tip]))),
    );

    const door = await page.evaluate(async () => {
      let opened = 0;
      const real = openCleanup;
      // eslint-disable-next-line no-global-assign
      openCleanup = () => { opened += 1; };
      const row = (path) => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
      row("/r/busy").querySelector(".wt-landing").click();
      row("/r/ahead").querySelector(".wt-landing").click();
      const closed = opened;
      row("/r/landed").querySelector(".wt-landing").click();
      const after = opened;
      // eslint-disable-next-line no-global-assign
      openCleanup = real;
      return { closed, after };
    });
    ok(
      "정리 가능 is a door to the review that already exists and nothing else: the chip opens it on a landed checkout with no session, and a row with a session or unmerged work opens nothing",
      door.closed === 0 && door.after === 1,
      JSON.stringify(door),
    );

    const panel = await page.evaluate(async () => {
      const read = (path) => {
        activeWorktreePath = path;
        paintScmLanding();
        const chip = document.getElementById("scm-compare-landing");
        return { hidden: chip.hidden, word: chip.textContent, tip: chip.dataset.tip ?? "" };
      };
      const out = { ahead: read("/r/ahead"), plain: read("/r/plain"), landed: read("/r/landed") };
      out.rowTip = window.__CHIP__("/r/ahead").tip;
      return out;
    });
    ok(
      "the git panel's head says the same words from the same function — 미반영 13 with the row's tooltip — and nothing at all where git said nothing; the current checkout is never offered for clean-up",
      !panel.ahead.hidden && panel.ahead.word === "미반영 13" && panel.ahead.tip === panel.rowTip &&
        panel.plain.hidden === true && !panel.landed.hidden && panel.landed.word === "반영됨",
      JSON.stringify(panel),
    );

    /* ---- behind the list: 확인 중 filled by the backend's notice; the stamp ---- */
    const live = await page.evaluate(async () => {
      const asked = { catalog: 0 };
      let answer = "pending";
      const wait = async (test) => {
        for (let tries = 0; tries < 80 && !test(); tries += 1) await new Promise((done) => setTimeout(done, 25));
        await window.__PAINTED__();
      };
      const row = () => ({
        path: "/r/live", branch: "live", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, last_activity_ms: 99,
        landing: answer === "pending"
          ? { state: "pending", detached: false, ahead: 0, dirty: false }
          : { state: "unlanded", detached: false, ahead: 4, dirty: false, compare_ref: "origin/main",
            ref_updated_ms: Date.now(), dirty_checked_ms: Date.now() },
      });
      const was = window.__ANSWER__.project_catalog;
      window.__ANSWER__.project_catalog = () => {
        asked.catalog += 1;
        const [project] = was();
        return [{ ...project, worktrees: [...project.worktrees.filter((one) => one.path !== "/r/live"), row()] }];
      };
      await refreshWorktrees();
      await window.__PAINTED__();
      const word = () => window.__CHIP__("/r/live").word;
      const first = word();
      answer = "filled";
      for (const listener of window.__LISTENERS__["worktree:landing"] ?? []) listener({ payload: null });
      await wait(() => word() === "미반영 4");
      const filled = word();
      const afterNotice = asked.catalog;
      // The poll: the first look only takes the baseline, a look at the same stamp
      // re-reads nothing, and a stamp that moved re-reads once.
      let stamp = "a";
      window.__ANSWER__.worktree_landing_stamp = () => stamp;
      landingStamp = null;
      landingLookedAt = 0;
      await lookForLandingChange();
      const baseline = asked.catalog;
      landingLookedAt = 0;
      await lookForLandingChange();
      const unchanged = asked.catalog;
      landingLookedAt = Date.now();
      stamp = "b";
      await lookForLandingChange();
      const tooSoon = asked.catalog;
      landingLookedAt = 0;
      await lookForLandingChange();
      return { first, filled, afterNotice, baseline, unchanged, tooSoon, moved: asked.catalog };
    });
    ok(
      "a row git has not been asked about goes out as 확인 중 and is filled when the backend says so; the stamp poll re-reads only when the stamp moved and no oftener than its period",
      live.first === "확인 중" && live.filled === "미반영 4" && live.afterNotice >= 2 &&
        live.baseline === live.afterNotice && live.unchanged === live.baseline &&
        live.tooSoon === live.unchanged && live.moved === live.unchanged + 1,
      JSON.stringify(live),
    );

    const heights = await page.evaluate(() => {
      const rows = [...document.querySelectorAll(".wt-row")].map((row) => row.getBoundingClientRect().height);
      return { min: Math.min(...rows), max: Math.max(...rows), plain: window.__CHIP__("/r/plain").height };
    });
    ok(
      "the chip does not grow the row: a row with the longest chip is as tall as one with none",
      heights.max - heights.min < 1 && Math.abs(chips["/r/dirty"].height - heights.plain) < 1,
      JSON.stringify({ heights, dirty: chips["/r/dirty"].height }),
    );

    if (capture) {
      for (const theme of ["dark", "light"]) {
        await page.evaluate(async (which) => { document.documentElement.dataset.theme = which; await window.__PAINTED__(); }, theme);
        await page.locator(".threads").screenshot({ path: resolve(capture, `landing-${theme}.png`), animations: "disabled" });
      }
    }
  } finally {
    await page.close();
  }
}
