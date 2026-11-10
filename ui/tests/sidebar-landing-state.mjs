import { mkdir, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { openWindowTestPage } from "./window-boot.mjs";
import { drawHangulAsWindows } from "./windows-hangul.mjs";

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
 *   - the git panel's head says the same words from the same function;
 *   - a landed checkout that still holds files git ignores says so on the chip
 *     in the words the evidence panel uses (t-34315) — right after 반영됨, before
 *     the offer to clean up, because the ellipsis cuts the end of a chip at the
 *     sidebar's default width — and only a landed one: nothing about ignored
 *     files is said of work main does not have;
 *   - an unlanded row also says how far behind the compare ref it runs and what a merge would
 *     conflict on (t-34501): a clash is the second word of the chip in its own colour, a branch
 *     `far_behind` says 「main보다 N 뒤」 after it, the tooltip always has the commit count, the files
 *     and the next thing to do, and a conflict nobody looked at is said as not read — never as none;
 *   - work nobody has committed yet (t-45314, AT-②-1 ⑧): a row with unsaved changes also says how far
 *     behind the compare ref it runs, and its chip says 「main과 겹침 N」 when the compare ref changed some
 *     of the same files since the two split — by file, so never called a conflict, in word or colour; the
 *     tooltip names the files, and an overlap nobody read is said as not read, never as none. Those rows
 *     are the backend's own serialization (`sidebar-landing-overlap.json`, which `worktree_landing.rs`
 *     compares its answer with). */

const REF = "origin/main";
const NOW = Date.now();
const LANDED_IN = { sha: "0123456789abcdef0123456789abcdef01234567", time_ms: NOW - 3 * 3600_000 };
const CLEAN = { total: 0, files: [] };
const base = (state, over = {}) => ({
  state, detached: false, ahead: 0, dirty: false, dirty_checked_ms: NOW - 1000, compare_ref: REF, ref_updated_ms: NOW - 2 * 86_400_000, ...over,
});
/* AT-②-1 (t-45314): checkouts B, C and D as the backend tells the window of them, with the two clock
 * stamps it also sends put back. */
const OVERLAP = JSON.parse(await readFile(
  resolve(dirname(fileURLToPath(import.meta.url)), "sidebar-landing-overlap.json"), "utf8",
)).rows;
const stamped = (landing) => ({ ...landing, dirty_checked_ms: NOW - 1000, ref_updated_ms: NOW - 2 * 86_400_000 });
const UNSAVED = ["/r/overlap-b", "/r/overlap-c", "/r/overlap-d", "/r/overlap-unread"];

const SCENE = [
  { path: "/r/landed", landing: base("landed", { landed_in: LANDED_IN }) },
  { path: "/r/busy", term: 9101, hook: "working", landing: base("landed") },
  { path: "/r/ahead", landing: base("unlanded", { ahead: 13 }) },
  { path: "/r/nocommit", landing: base("no_commits") },
  { path: "/r/dirty", landing: base("landed", { dirty: true }) },
  { path: "/r/lm", settled: { reported: true, task: "ledger says merged",
    review: { verified: true, merged: true, written: true, author: "coordinator" }, progress: { stage: "merged" } },
  landing: base("unlanded", { ahead: 2 }) },
  { path: "/r/gl", term: 9102, hook: "done", task: "reported only", ledger: { reported: true, progress: { stage: "report_only" } },
    landing: base("landed") },
  { path: "/r/vouched", settled: { reported: true, task: "merged and in git",
    review: { verified: true, merged: true, written: true, author: "coordinator" }, progress: { stage: "merged" } },
  landing: base("landed", { landed_in: LANDED_IN }) },
  { path: "/r/noref", landing: base("no_ref", { compare_ref: "origin/gone", ref_updated_ms: null }) },
  { path: "/r/det", landing: base("landed", { detached: true }) },
  { path: "/r/unk", landing: base("unknown") },
  { path: "/r/fail", landing: { state: "failed", detached: false, ahead: 0, dirty: false } },
  { path: "/r/pend", landing: { state: "pending", detached: false, ahead: 0, dirty: false } },
  { path: "/r/ignored", landing: base("landed", { ignored: true }) },
  { path: "/r/busyignored", term: 9103, hook: "working", landing: base("landed", { ignored: true }) },
  { path: "/r/dirtyignored", landing: base("landed", { dirty: true, ignored: true }) },
  { path: "/r/aheadignored", landing: base("unlanded", { ahead: 2, ignored: true }) },
  // How far behind the compare ref a branch runs and what a merge would clash on (t-34501).
  { path: "/r/near", landing: base("unlanded", { ahead: 1, behind: 3, far_behind: false, conflict: CLEAN }) },
  { path: "/r/clash", landing: base("unlanded", { ahead: 3, behind: 5, far_behind: false,
    conflict: { total: 2, files: ["src/a.rs", "ui/b.js"] } }) },
  { path: "/r/far", landing: base("unlanded", { ahead: 4, behind: 70, far_behind: true, conflict: CLEAN }) },
  { path: "/r/farclash", landing: base("unlanded", { ahead: 17, behind: 71, far_behind: true,
    conflict: { total: 7, files: ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"] } }) },
  { path: "/r/clashdirty", landing: base("unlanded", { ahead: 2, behind: 4, far_behind: false, dirty: true,
    conflict: { total: 1, files: ["x.js"] } }) },
  { path: "/r/unseen", landing: base("unlanded", { ahead: 2, far_behind: false }) },
  // Work nobody has committed yet, beside a main that moved one commit (AT-②-1, t-45314).
  { path: "/r/overlap-b", landing: stamped(OVERLAP.B) },
  { path: "/r/overlap-c", landing: stamped(OVERLAP.C) },
  { path: "/r/overlap-d", landing: stamped(OVERLAP.D) },
  // Unsaved changes whose overlap git could not read: the backend sends no `overlap` at all.
  { path: "/r/overlap-unread", landing: base("no_commits", { dirty: true, behind: 2 }) },
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
  "/r/ignored": { word: "반영됨 · 무시된 파일 남음 · 정리 가능", tone: "landed", cleanable: true },
  "/r/busyignored": { word: "반영됨 · 무시된 파일 남음", tone: "landed", cleanable: false },
  "/r/dirtyignored": { word: "반영됨 · 저장 안 한 변경 · 무시된 파일 남음", tone: "landed", cleanable: false },
  "/r/aheadignored": { word: "미반영 2", tone: "ahead", cleanable: false },
  "/r/near": { word: "미반영 1", tone: "ahead", cleanable: false },
  "/r/clash": { word: "미반영 3 · 충돌 2", tone: "conflict", cleanable: false },
  "/r/far": { word: "미반영 4 · main보다 70 뒤", tone: "ahead", cleanable: false },
  "/r/farclash": { word: "미반영 17 · 충돌 7 · main보다 71 뒤", tone: "conflict", cleanable: false },
  "/r/clashdirty": { word: "미반영 2 · 충돌 1 · 저장 안 한 변경", tone: "conflict", cleanable: false },
  "/r/unseen": { word: "미반영 2", tone: "ahead", cleanable: false },
};

export async function testSidebarLandingState({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  // The chips are Korean, and Windows draws Hangul 16% wider than macOS: the sidebar must hold them at that width.
  await drawHangulAsWindows(page);
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
      // The stamp answer this suite starts with: every block that replaces it puts this one back (review t-103290 N7).
      window.__STAMP_AT_START__ = window.__ANSWER__.worktree_landing_stamp;
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
          color: chip ? getComputedStyle(chip).color : null,
          tip: shown ? chip.dataset.tip ?? "" : null,
          cleanable: chip?.dataset.cleanable === "1",
          // The ellipsis cuts what does not fit (`textContent` is the whole word either way), so the
          // room the chip has is read beside it: a measurement, not a check (SIDEBAR_LANDING_WIDTHS).
          scroll: chip ? chip.scrollWidth : null,
          client: chip ? chip.clientWidth : null,
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

    if (process.env.SIDEBAR_LANDING_WIDTHS) {
      console.log("LANDING_CHIP_WIDTHS " + JSON.stringify(Object.fromEntries(Object.entries(chips)
        .filter(([, one]) => one.word)
        .map(([path, one]) => [path, { word: one.word, scroll: one.scroll, client: one.client, cut: one.scroll > one.client }]))));
    }

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
      "files git ignores are said on the chip of a landed checkout — right after 반영됨 and before the offer to clean up, so the cut the ellipsis makes at the default width takes the offer and not the fact — in the words the evidence panel uses, with a tooltip line that says they go with the folder, and never on work main does not have",
      chips["/r/ignored"].word === "반영됨 · 무시된 파일 남음 · 정리 가능" && chips["/r/ignored"].tip.includes("git이 무시하는 파일이 남아 있습니다") &&
        chips["/r/ignored"].tip.includes("폴더를 지우면 함께 사라집니다") &&
        chips["/r/landed"].word === "반영됨 · 정리 가능" && !chips["/r/landed"].tip.includes("무시하는") &&
        !chips["/r/aheadignored"].word.includes("무시된") && !chips["/r/aheadignored"].tip.includes("무시하는"),
      JSON.stringify({ ignored: chips["/r/ignored"], landed: chips["/r/landed"], ahead: chips["/r/aheadignored"] }),
    );

    ok(
      "an unlanded row's tooltip always says how many commits it runs behind the compare ref, which files a merge would conflict on (with the count of the ones left out) and the next thing to do; a conflict nobody looked at is said as not read, and work that is in main says none of it",
      chips["/r/near"].tip.includes(`${REF}보다 3커밋 뒤처져 있습니다`) &&
        chips["/r/near"].tip.includes(`${REF}에 합쳐 보아도 충돌하는 파일이 없습니다`) &&
        chips["/r/near"].tip.includes("다음 할 일: git fetch 뒤") && chips["/r/near"].tip.includes("게이트를 다시 돌리세요") &&
        chips["/r/clash"].tip.includes(`${REF}에 합치면 2개 파일이 충돌합니다: src/a.rs, ui/b.js`) &&
        !chips["/r/clash"].tip.includes(" 외 ") &&
        chips["/r/farclash"].tip.includes(`${REF}에 합치면 7개 파일이 충돌합니다: a.rs, b.rs, c.rs, d.rs, e.rs 외 2개`) &&
        chips["/r/farclash"].tip.includes(`${REF}보다 71커밋 뒤처져 있습니다`) &&
        chips["/r/unseen"].tip.includes("충돌하는지는 읽지 못했습니다") && !chips["/r/unseen"].tip.includes("충돌하는 파일이 없습니다") &&
        !chips["/r/unseen"].tip.includes("뒤처져") &&
        !chips["/r/landed"].tip.includes("뒤처져") && !chips["/r/landed"].tip.includes("충돌") && !chips["/r/landed"].tip.includes("다음 할 일"),
      JSON.stringify(Object.fromEntries(["/r/near", "/r/clash", "/r/farclash", "/r/unseen", "/r/landed"].map((path) => [path, chips[path].tip]))),
    );

    ok(
      "a clash wears its own colour — the halt ink the 확인 필요 chip uses, and not the wait ink 미반영 wears — and a branch that is far behind but merges cleanly keeps the wait ink, so the colour is for the conflict alone",
      chips["/r/clash"].color === chips["/r/lm"].color && chips["/r/clash"].color !== chips["/r/ahead"].color &&
        chips["/r/farclash"].color === chips["/r/lm"].color && chips["/r/far"].color === chips["/r/ahead"].color,
      JSON.stringify({ clash: chips["/r/clash"].color, check: chips["/r/lm"].color, ahead: chips["/r/ahead"].color, far: chips["/r/far"].color }),
    );

    const heads = await page.evaluate((paths) => Object.fromEntries(paths.map((path) => [path, worktreeLandingSayFor(path)?.head ?? null])),
      UNSAVED);
    const [b, c, d, unread] = UNSAVED.map((path) => chips[path]);
    ok(
      "AT-②-1 ⑧ (t-45314): laid out from the backend's own rows, B — an edit nobody committed, of a file main changed too — says main과 겹침 1 · 커밋 없음 · 저장 안 한 변경, the overlap first (t-55424), and never calls it a conflict, in word or colour; its tooltip names src/login.rs, says it runs 1 commit behind origin/main and that the overlap is by file; its head word stays 커밋 없음; C and D say nothing of an overlap",
      b.word === "main과 겹침 1 · 커밋 없음 · 저장 안 한 변경" && !b.word.includes("충돌") &&
        b.tone === "none" && b.color === chips["/r/nocommit"].color &&
        b.tip.includes("src/login.rs") && b.tip.includes(`${REF}보다 1커밋 뒤처져`) && b.tip.includes("파일 단위 겹침") &&
        !b.tip.includes("충돌") && heads["/r/overlap-b"] === "커밋 없음" &&
        c.word === "커밋 없음 · 저장 안 한 변경" && d.word === "커밋 없음" && !c.word.includes("겹침") && !d.word.includes("겹침"),
      JSON.stringify({ b, c, d, heads }),
    );

    // Read where the person reads it: the sidebar at its default width, the words' own ink against the chip's
    // box. The ellipsis is drawn over the end of the box, so a word that shows ends before it. A chip's news is
    // its words up to its last clash or overlap with its count, and with neither its first word; what follows
    // may fall under the ellipsis, because the tooltip says it whole (t-34501).
    const room = await page.evaluate((paths) => {
      const read = (path, words) => {
        const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
        const chip = row.querySelector(".wt-landing");
        const title = row.querySelector(".wt-title");
        const text = chip.firstChild;
        const style = getComputedStyle(chip);
        const px = (name) => parseFloat(style.getPropertyValue(name));
        const span = (from, to) => {
          const range = document.createRange();
          range.setStart(text, from);
          range.setEnd(text, to);
          return range.getBoundingClientRect();
        };
        const pen = document.createElement("canvas").getContext("2d");
        pen.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        const box = chip.getBoundingClientRect();
        const cut = chip.scrollWidth > chip.clientWidth;
        const end = box.right - px("padding-right") - px("border-right-width") - (cut ? pen.measureText("…").width : 0);
        const at = words ? text.data.indexOf(words) : -1;
        const inner = box.width - px("padding-left") - px("padding-right") - px("border-left-width") - px("border-right-width");
        return {
          word: text.data,
          shows: at >= 0 && span(at, at + words.length).right <= end + 0.5,
          cut,
          chip: box.width,
          slack: inner - span(0, text.data.length).width,
          chipUnderName: box.top >= title.getBoundingClientRect().bottom - 1,
          nameCut: title.scrollWidth > title.clientWidth,
        };
      };
      const news = (word) => {
        const parts = word.split(" · ");
        const last = parts.findLastIndex((part) => /(^충돌|겹침) \d+$/.test(part));
        return parts.slice(0, last + 1 || 1).join(" · ");
      };
      const every = {};
      for (const path of paths) {
        const chip = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"] .wt-landing`);
        if (!chip || getComputedStyle(chip).display === "none" || chip.textContent === "") continue;
        const said = news(chip.textContent);
        const one = read(path, said);
        every[path] = { news: said, shows: one.shows, cut: one.cut };
      }
      return {
        sidebar: getComputedStyle(document.documentElement).getPropertyValue("--sidebar-width").trim(),
        b: read("/r/overlap-b", "main과 겹침 1"),
        d: read("/r/overlap-d", null),
        unread: read("/r/overlap-unread", null),
        every,
      };
    }, SCENE.map((one) => one.path));
    const worded = Object.values(chips).filter((one) => one.word).length;
    ok(
      "at the sidebar's default 280 px every chip says its news whole, before any ellipsis — B's main과 겹침 1, said first (t-55424), 미반영 N · 충돌 N, 반영됨, 커밋 없음, even on the row whose long name holds its chip to half the line — and B's name is not cut for it",
      room.sidebar === "280px" && room.b.shows && !room.b.nameCut &&
        Object.keys(room.every).length === worded && Object.values(room.every).every((one) => one.shows),
      JSON.stringify({ sidebar: room.sidebar, b: room.b, worded, every: room.every }),
    );
    ok(
      "a chip is never wider than its words — D's 커밋 없음 has no empty room inside it — and a long name keeps its whole line, the chip standing on the line under it (t-78107, 시안 5: the name first)",
      Math.abs(room.d.slack) <= 1 && !room.unread.nameCut && room.unread.chipUnderName,
      JSON.stringify({ d: room.d, unread: room.unread }),
    );

    ok(
      "an overlap that was read and is empty is said as none in the tooltip, beside how far behind the row runs; one git could not read is said as not read — never as none, and never on the chip; a checkout with nothing unsaved says neither",
      c.tip.includes(`${REF}보다 1커밋 뒤처져`) && c.tip.includes("바뀐 파일은 없습니다") && !c.tip.includes("읽지 못했습니다") &&
        unread.word === "커밋 없음 · 저장 안 한 변경" && unread.tip.includes("겹치는지는 읽지 못했습니다") &&
        !unread.tip.includes("바뀐 파일은 없습니다") && unread.tip.includes(`${REF}보다 2커밋 뒤처져`) &&
        !d.tip.includes("겹치") && !d.tip.includes("바뀐 파일") && !d.tip.includes("뒤처져"),
      JSON.stringify({ c: c.tip, unread: unread.tip, d: d.tip }),
    );

    const catalogs = await page.evaluate(() => {
      const keys = ["worktree.landOverlap", "worktree.landTipOverlap", "worktree.landTipOverlapMore",
        "worktree.landTipOverlapScope", "worktree.landTipOverlapNone", "worktree.landTipOverlapUnknown"];
      const holes = (text) => [...text.matchAll(/\{\{(\w+)\}\}/g)].map((one) => one[1]).sort().join(",");
      const missing = [];
      const uneven = [];
      for (const key of keys) {
        const shapes = new Set();
        for (const code of ["ko", "en", "ja", "zh", "es"]) {
          const said = CATALOG[code]?.[key];
          if (typeof said !== "string" || said.trim() === "") missing.push(`${code}:${key}`);
          else shapes.add(holes(said));
        }
        if (shapes.size > 1) uneven.push(key);
      }
      return { missing, uneven };
    });
    ok(
      "the words for unsaved work against the compare ref stand in all five catalogs — ko, en, ja, zh and es — with the same placeholders in each",
      catalogs.missing.length === 0 && catalogs.uneven.length === 0,
      JSON.stringify(catalogs),
    );

    // The time is written by the page's engine (`toLocaleString()` in `worktreeLandingSay`), so it is read in the
    // page: Node formats in its own locale (en-US here), and WebKit takes the system's (ko-KR here) where Chromium
    // headless takes en-US — compared with Node's, the same tooltip failed on WebKit only (t-103178).
    const landedTime = await page.evaluate((ms) => new Date(ms).toLocaleString(), LANDED_IN.time_ms);
    ok(
      "the tooltip names the ref compared with and how long ago it last moved, the main commit that took the work (nine characters) and its time, and says so for a detached HEAD, a missing ref and an unknown",
      chips["/r/landed"].tip.includes(`비교: ${REF}`) && chips["/r/landed"].tip.includes("마지막 갱신 2일 전") &&
        chips["/r/landed"].tip.includes("012345678 ·") && !chips["/r/landed"].tip.includes("0123456789") &&
        chips["/r/landed"].tip.includes(landedTime) &&
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
      const out = { ahead: read("/r/ahead"), plain: read("/r/plain"), landed: read("/r/landed"), clash: read("/r/farclash") };
      out.rowTip = window.__CHIP__("/r/ahead").tip;
      out.clashRow = window.__CHIP__("/r/farclash");
      return out;
    });
    ok(
      "the git panel's head says the same words from the same function — 미반영 13 with the row's tooltip — and nothing at all where git said nothing; the current checkout is never offered for clean-up",
      !panel.ahead.hidden && panel.ahead.word === "미반영 13" && panel.ahead.tip === panel.rowTip &&
        panel.plain.hidden === true && !panel.landed.hidden && panel.landed.word === "반영됨" &&
        panel.clash.word === "미반영 17 · 충돌 7 · main보다 71 뒤" && panel.clash.tip === panel.clashRow.tip,
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
      const stampWas = window.__ANSWER__.worktree_landing_stamp;
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
      window.__ANSWER__.worktree_landing_stamp = stampWas;
      return { first, filled, afterNotice, baseline, unchanged, tooSoon, moved: asked.catalog };
    });
    ok(
      "a row git has not been asked about goes out as 확인 중 and is filled when the backend says so; the stamp poll re-reads only when the stamp moved and no oftener than its period",
      live.first === "확인 중" && live.filled === "미반영 4" && live.afterNotice >= 2 &&
        live.baseline === live.afterNotice && live.unchanged === live.baseline &&
        live.tooSoon === live.unchanged && live.moved === live.unchanged + 1,
      JSON.stringify(live),
    );

    const moved = await page.evaluate(async () => {
      // Only `behind` and the conflict change — ahead, state and the ref stand — and the chip says so.
      const was = window.__ANSWER__.project_catalog;
      window.__ANSWER__.project_catalog = () => {
        const [project] = was();
        return [{ ...project, worktrees: project.worktrees.map((one) => one.path !== "/r/clash" ? one : {
          ...one, landing: { ...one.landing, behind: 9, conflict: { total: 3, files: ["src/a.rs", "ui/b.js", "c.md"] } },
        }) }];
      };
      await refreshWorktrees();
      await window.__PAINTED__();
      return window.__CHIP__("/r/clash");
    });
    ok(
      "a change of only the commits behind or of the conflicting files repaints the chip and its tooltip — landing is painted in place, not rebuilt",
      moved.word === "미반영 3 · 충돌 3" && moved.tip.includes("9커밋 뒤처져") && moved.tip.includes("3개 파일이 충돌합니다: src/a.rs, ui/b.js, c.md"),
      JSON.stringify(moved),
    );

    /* ---- landing alone is painted in place; a rebuild keeps the rings turning (t-102869) ----
     *
     * The person, 2026-11-08: 「네비바의 판이 한번씩 깜박거려」. Landing used to be part of the list's shape,
     * so every five-second stamp poll and every `worktree:landing` notice that moved one number rebuilt
     * the whole sidebar (1fcfd6c52 took it out). What stands here keeps it out: each landing field moved
     * alone, through both paths, leaves the same nodes and the same drawn shape, and only the chip's words
     * move. A branch renamed is the control — it must rebuild, or this check could not see a rebuild. And
     * the rebuild that remains must not send a turning ring back to the top: the new nodes' turns stand at
     * the angle the old ones stood at, and a working dot carries its step from the moment it is built. */
    const steady = await page.evaluate(async () => {
      const PATH = "/r/clash";
      const was = window.__ANSWER__.project_catalog;
      let move = (one) => one;
      let asked = 0;
      window.__ANSWER__.project_catalog = () => {
        asked += 1;
        const [project] = was();
        return [{ ...project, worktrees: project.worktrees.map((one) => (one.path === PATH ? move(one) : one)) }];
      };
      const rowNode = () => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(PATH)}"]`)?.closest(".wt-node") ?? null;
      const said = () => {
        const chip = window.__CHIP__(PATH);
        return `${chip.word}|${chip.tip}`;
      };
      const removed = { count: 0 };
      const watch = new MutationObserver((records) => {
        for (const record of records) {
          for (const gone of record.removedNodes) {
            if (gone.nodeType !== 1) continue;
            if (gone.matches(".wt-node, .proj")) removed.count += 1;
            removed.count += gone.querySelectorAll(".wt-node, .proj").length;
          }
        }
      });
      watch.observe(worktreeList, { childList: true, subtree: true });
      let stamp = 0;
      // Put back at the end: the suites after this one ask the stamp too (review t-102958 #7).
      const stampWas = window.__ANSWER__.worktree_landing_stamp;
      window.__ANSWER__.worktree_landing_stamp = () => `s${stamp}`;
      const viaPoll = async () => {
        landingStamp = `s${stamp}`;
        stamp += 1;
        landingLookedAt = 0;
        await lookForLandingChange();
        await window.__PAINTED__();
      };
      const viaNotice = async () => {
        const before = asked;
        for (const listener of window.__LISTENERS__["worktree:landing"] ?? []) listener({ payload: null });
        for (let tries = 0; tries < 80 && asked === before; tries += 1) await new Promise((done) => setTimeout(done, 25));
        // The notice's refresh is not awaited by the listener: give its last await a turn, then a frame.
        for (let tries = 0; tries < 4; tries += 1) await new Promise((done) => setTimeout(done, 25));
        await window.__PAINTED__();
      };
      const landed = { sha: "fedcba9876543210fedcba9876543210fedcba98", time_ms: Date.now() - 3600_000 };
      // Each step moves one more landing field than the step before, so every step changes what the chip says.
      const steps = [
        ["ahead", (landing) => ({ ...landing, ahead: 6 })],
        ["dirty", (landing) => ({ ...landing, dirty: true })],
        ["behind", (landing) => ({ ...landing, behind: 12 })],
        ["far_behind", (landing) => ({ ...landing, far_behind: true, behind: 80 })],
        ["conflict", (landing) => ({ ...landing, conflict: { total: 4, files: ["src/a.rs", "ui/b.js", "c.md", "d.md"] } })],
        ["overlap", (landing) => ({ ...landing, conflict: { total: 0, files: [] }, overlap: { total: 1, files: ["ui/b.js"] } })],
        ["reclaim", (landing) => landing, { kind: "locked", detail: "held by a test" }],
        ["state", () => ({ state: "landed", detached: false, ahead: 0, dirty: false, compare_ref: "origin/main",
          ref_updated_ms: Date.now() - 86_400_000, dirty_checked_ms: Date.now(), landed_in: landed })],
      ];
      const node = rowNode();
      const drawn = worktreeListDrawn;
      const seen = [];
      let landing = was()[0].worktrees.find((one) => one.path === PATH).landing;
      let reclaim;
      for (const [at, [field, change, held]] of steps.entries()) {
        landing = change(landing);
        if (held) reclaim = held;
        const now = { landing, reclaim };
        move = (one) => ({ ...one, landing: now.landing, ...(now.reclaim ? { reclaim: now.reclaim } : {}) });
        const before = said();
        const path = at % 2 === 0 ? "poll" : "notice";
        if (path === "poll") await viaPoll(); else await viaNotice();
        // The other path too, with nothing new: it must not rebuild either.
        if (path === "poll") await viaNotice(); else await viaPoll();
        seen.push({ field, path, same: rowNode() === node && node.isConnected, drawn: worktreeListDrawn === drawn,
          removed: removed.count, moved: said() !== before, word: window.__CHIP__(PATH).word });
      }

      // The control, and the turns across it: a branch renamed is a new shape, so the list is rebuilt.
      const TURNS = new Set(["wt-activity-turn", "wt-agent-spin"]);
      const turns = () => worktreeList.getAnimations({ subtree: true }).filter((one) => TURNS.has(one.animationName));
      // Where a turn stands against the document's clock: the same number before and after means the ring did not hop.
      const offset = (one) => {
        const period = one.effect.getComputedTiming().duration;
        const at = Math.round((((document.timeline.currentTime - one.currentTime) % period) + period) % period);
        return at === period ? 0 : at;
      };
      const turnSaid = (one) => ({ name: one.animationName, at: offset(one) });
      const turnsBefore = turns().map(turnSaid);
      move = (one) => ({ ...one, branch: "clash-renamed", landing, reclaim });
      await refreshWorktrees();
      // Read before any frame: what the rebuild left is what the first frame after it draws.
      const busyDot = document.querySelector('.wt-row[data-worktree-path="/r/busy"] .wt-dot');
      const dotClass = busyDot?.className ?? "";
      const turnsAfter = turns().map(turnSaid);
      const control = { same: rowNode() === node, drawn: worktreeListDrawn === drawn, removed: removed.count };
      watch.disconnect();

      // The card's own ring (`wt-activity-turn`) stands only where no working agent row is shown, and every working
      // row in this scene shows its agents: with the agents left off the card, the working cards wear the ring
      // instead — the same rebuild, measured on the card's ring (review t-102958 #4).
      // Turned off through the setting's own painter, the road the 표시 menu takes.
      const agentsShown = worktreeCardProperties.has("agents");
      worktreeCardProperties.delete("agents");
      paintWorktreeCardProperties();
      // Read before any frame: the setting pins the rings it uncovers as it changes them (review t-103290 N3), so
      // they already stand on the document's clock. Unpinned, a ring just uncovered has no start time yet.
      const uncovered = turns().filter((one) => one.animationName === "wt-activity-turn")
        .map((one) => one.startTime);
      await window.__PAINTED__();
      // The rebuild is measured against rings that stand on the document's clock, as any ring a person watches turn
      // does. The setting pins them; this wait stays as the net under it — the window's `animationstart` listener
      // pins a ring a frame after it starts — because a ring read before its pin stands where it was born and the
      // rebuild that pins it reads as a hop (v1.1.140's third lane, 2026-11-11: before 1570, after 0). The wait is
      // for the pin, not for a time.
      for (let frames = 0; frames < 30 && turns().some((one) => one.startTime !== 0); frames += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const cardBefore = turns().map(turnSaid);
      move = (one) => ({ ...one, branch: "clash-renamed-again", landing, reclaim });
      await refreshWorktrees();
      const cardAfter = turns().map(turnSaid);
      // A row that starts working on a node that stands starts its card's ring there: it is read before any frame
      // too, so it must already stand where the other rings stand, not at the top (review t-102958 #2).
      const startedPath = "/r/gl";
      const startedAt = turns().filter((one) => one.animationName === "wt-activity-turn").length;
      const startedNode = document.querySelector(`.wt-row[data-worktree-path="${startedPath}"]`)?.closest(".wt-node") ?? null;
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      tell("hook:agent", { term: 9102, state: "working", agent: "claude", session: "s-9102" });
      paintWorktreeDots();
      const startedRing = startedNode?.getAnimations({ subtree: true })
        .filter((one) => one.animationName === "wt-activity-turn").map(turnSaid) ?? [];
      const started = {
        same: startedNode !== null && startedNode.isConnected,
        before: startedAt,
        ring: startedRing,
        all: turns().filter((one) => one.animationName === "wt-activity-turn").map(turnSaid),
      };
      tell("hook:agent", { term: 9102, state: "done", agent: "claude", session: "s-9102" });
      if (agentsShown) worktreeCardProperties.add("agents");
      paintWorktreeCardProperties();

      window.__ANSWER__.project_catalog = was;
      window.__ANSWER__.worktree_landing_stamp = stampWas;
      await refreshWorktrees();
      await window.__PAINTED__();
      return { seen, control, dotClass, turnsBefore, turnsAfter, uncovered, cardBefore, cardAfter, started,
        stampPutBack: window.__ANSWER__.worktree_landing_stamp === window.__STAMP_AT_START__ };
    });
    for (const one of steady.seen) {
      ok(
        `a change of only the landing's ${one.field} (${one.path} first, then the other path) repaints the chip in place: the same row node, the same drawn shape, no node taken out`,
        one.same && one.drawn && one.removed === 0 && one.moved,
        JSON.stringify(one),
      );
    }
    ok(
      "the control: a branch renamed is a new shape, and the check above sees the rebuild it makes",
      !steady.control.same && !steady.control.drawn && steady.control.removed > 0,
      JSON.stringify(steady.control),
    );
    const offsets = (turns) => turns.map((one) => one.at);
    const named = (turns, name) => turns.some((one) => one.name === name);
    ok(
      "a rebuild does not send a turning ring back to the top: every turn in the list stands where it stood against the document's clock, and a working dot wears its step from the moment it is built",
      steady.turnsBefore.length > 0 && steady.turnsAfter.length > 0 &&
        named(steady.turnsAfter, "wt-agent-spin") &&
        offsets(steady.turnsAfter).every((at) => offsets(steady.turnsBefore).includes(at)) &&
        /\bwt-spin-\d+\b/.test(steady.dotClass),
      JSON.stringify({ before: steady.turnsBefore, after: steady.turnsAfter, dot: steady.dotClass }),
    );
    ok(
      "turning the agents off the cards pins the card rings it uncovers before the frame that first draws them",
      steady.uncovered.length > 0 && steady.uncovered.every((at) => at === 0),
      JSON.stringify({ uncovered: steady.uncovered }),
    );
    ok(
      "the card's own turning ring (wt-activity-turn) is among the turns measured, and a rebuild leaves it where it stood against the document's clock",
      named(steady.cardBefore, "wt-activity-turn") && named(steady.cardAfter, "wt-activity-turn") &&
        offsets(steady.cardAfter).every((at) => offsets(steady.cardBefore).includes(at)),
      JSON.stringify({ before: steady.cardBefore, after: steady.cardAfter }),
    );
    ok(
      "a card ring that starts on a row that stands is drawn from its first frame where the other rings stand, not from the top",
      steady.started.same && steady.started.ring.length > 0 && steady.started.all.length > steady.started.before &&
        offsets(steady.started.ring).every((at) => offsets(steady.cardAfter).includes(at)),
      JSON.stringify(steady.started),
    );
    ok(
      "the suite puts back the stamp answer it started with, so a suite after it asks the stamp it expects",
      steady.stampPutBack,
      JSON.stringify({ stampPutBack: steady.stampPutBack }),
    );

    /* ---- a working row that goes quiet uncovers its card's ring, pinned before the frame (review t-103290 N1) ----
     *
     * The card's ring stands only where no working agent row is shown. A session still streaming in the checkout
     * keeps the dot working while the one agent row there goes quiet: the quiet row is a new node, the working row
     * goes once the new one is laid out, and only then does the stylesheet uncover the card's ring. Read before any
     * frame, that ring must already stand on the document's clock; pinned while the working row still stood, there
     * was no ring yet to pin and it drew one frame at the top. */
    const quieted = await page.evaluate(async () => {
      const PATH = "/r/busy";
      const TERM = 9101;
      const LANE = "lane-landing-n1";
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const card = () => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(PATH)}"]`)?.closest(".wt-node") ?? null;
      const said = () => {
        const node = card();
        return {
          dot: node?.querySelector(".wt-row .wt-dot")?.dataset.state ?? "",
          working: node?.querySelectorAll(".wt-agent.is-working").length ?? 0,
          rings: node?.getAnimations({ subtree: true })
            .filter((one) => one.animationName === "wt-activity-turn").map((one) => one.startTime) ?? [],
        };
      };
      upsertLane({ id: LANE, title: "busy lane", state: "streaming", session_id: null, worktree_id: PATH, agent: "claude" });
      paintWorktreeDots();
      paintWorktreeAgents();
      await window.__PAINTED__();
      const before = said();
      const row = card()?.querySelector(".wt-agent.is-working") ?? null;
      tell("hook:agent", { term: TERM, state: "done", agent: "claude", session: `s-${TERM}` });
      flushAgentPaint();
      // Read before any frame: what the card painter left is what the first frame after it draws.
      const after = { ...said(), rowGone: row !== null && !row.isConnected };
      removeLane(LANE);
      tell("hook:agent", { term: TERM, state: "working", agent: "claude", session: `s-${TERM}` });
      flushAgentPaint();
      await window.__PAINTED__();
      return { before, after, putBack: said().working > 0 && !lanes.has(LANE) };
    });
    ok(
      "a working agent row that goes quiet while its checkout still works uncovers the card's ring, which stands on the document's clock before the frame that first draws it",
      quieted.before.dot === "working" && quieted.before.working > 0 && quieted.before.rings.length === 0 &&
        quieted.after.dot === "working" && quieted.after.working === 0 && quieted.after.rowGone &&
        quieted.after.rings.length > 0 && quieted.after.rings.every((at) => at === 0) && quieted.putBack,
      JSON.stringify(quieted),
    );

    // The artifact card and drawer wear the state's own word only (t-36910 `head`): the clash and the
    // distance are said in the sidebar chip and in the tooltip, and a clash is told by colour there.
    const card = await page.evaluate(() => {
      const wear = (path) => {
        const chip = document.createElement("span");
        chip.className = "artifact-card-landing";
        document.body.append(chip);
        dressArtifactLanding(chip, worktreeLandingSayFor(path));
        const seen = {
          text: chip.textContent,
          tone: chip.dataset.landing ?? null,
          tip: chip.dataset.tip ?? "",
          color: getComputedStyle(chip).color,
          border: getComputedStyle(chip).borderTopColor,
        };
        chip.remove();
        return seen;
      };
      return { clash: wear("/r/farclash"), check: wear("/r/unk"), quiet: wear("/r/ahead"), sidebar: window.__CHIP__("/r/farclash") };
    });
    ok(
      "an unlanded row with a clash and a long way behind reads 미반영 N alone on an artifact card, tells the clash by data-landing and by the 확인 필요 colour, keeps the clash and the distance in its tooltip, and wears the whole sentence in the sidebar",
      card.clash.text === "미반영 17" && card.clash.tone === "conflict" &&
        card.clash.color === card.check.color && card.clash.border === card.check.border &&
        card.clash.color !== card.quiet.color &&
        card.clash.tip.includes("7개 파일이 충돌합니다") && card.clash.tip.includes("71커밋 뒤처져") &&
        card.sidebar.word === "미반영 17 · 충돌 7 · main보다 71 뒤",
      JSON.stringify(card),
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
      // The rows of uncommitted work close up (AT-②-1, t-45314): the four rows together, then B with its
      // tooltip open where the window puts it. The tooltip opens only once the list has scrolled and painted:
      // a scroll closes it, and the scroll's event lands after `scrollIntoView` returns. The list's scroll is
      // put back afterwards, so the whole-list pictures are taken as before.
      const clipAround = (paths, tip) => page.evaluate(async ([paths, tip]) => {
        const nodes = paths.map((path) => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`));
        if (tip) {
          showTip(nodes[0].querySelector(".wt-landing"));
          await window.__PAINTED__();
          nodes.push(document.querySelector(".tooltip"));
        }
        const boxes = nodes.map((node) => node.getBoundingClientRect());
        const x = Math.max(0, Math.min(...boxes.map((box) => box.left)) - 8);
        const y = Math.max(0, Math.min(...boxes.map((box) => box.top)) - 8);
        return {
          x, y,
          width: Math.min(window.innerWidth, Math.max(...boxes.map((box) => box.right)) + 8) - x,
          height: Math.min(window.innerHeight, Math.max(...boxes.map((box) => box.bottom)) + 8) - y,
        };
      }, [paths, tip]);
      const closeUp = async (suffix, tip) => {
        await page.evaluate(async (first) => {
          const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(first)}"]`);
          window.__SCROLLED__ = [];
          for (let node = row.parentElement; node; node = node.parentElement) window.__SCROLLED__.push([node, node.scrollTop]);
          row.scrollIntoView({ block: "start" });
          await window.__PAINTED__();
        }, UNSAVED[0]);
        await page.screenshot({ path: resolve(capture, `landing-overlap-${suffix}.png`), clip: await clipAround(UNSAVED, false), animations: "disabled" });
        if (tip) {
          await page.screenshot({ path: resolve(capture, `landing-overlap-tip-${suffix}.png`), clip: await clipAround(UNSAVED.slice(0, 1), true), animations: "disabled" });
        }
        await page.evaluate(async () => {
          hideTip();
          for (const [node, top] of window.__SCROLLED__) node.scrollTop = top;
          await window.__PAINTED__();
        });
      };
      // The default width; the narrowest the sidebar allows (panel limits: 180 px), where the ellipsis cuts
      // the chip; and the widest it allows (560 px), the most room a chip gets. The tooltip is the same there.
      for (const [width, suffix, tip] of [[null, "", true], ["180px", "-narrow", true], ["560px", "-wide", false]]) {
        for (const theme of ["dark", "light"]) {
          await page.evaluate(async ([which, wide]) => {
            document.documentElement.dataset.theme = which;
            if (wide) document.documentElement.style.setProperty("--sidebar-width", wide);
            await window.__PAINTED__();
          }, [theme, width]);
          await page.locator(".threads").screenshot({ path: resolve(capture, `landing-${theme}${suffix}.png`), animations: "disabled" });
          await closeUp(`${theme}${suffix}`, tip);
        }
      }
    }
  } finally {
    await page.close();
  }
}
