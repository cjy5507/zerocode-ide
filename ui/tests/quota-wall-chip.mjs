/* 한도 벽 칩 (t-39771) — 사용 한도에 걸려 멈춘 워커를 사람이 판에 들어가 보지 않고
 * 알아채고, 그 자리에서 다시 시도하거나 다른 에이전트로 넘길 수 있는가.
 *
 * 전에는 한도에 선 워커가 사이드바에서 「작업 중」과 같아 보였고, 상황판의 「한도 벽」
 * 낱말이 전부였다 — 무엇을 할 수 있는지는 판에 들어가 봐야 알았다. 여기서 재는 것:
 *   - 사이드바 행의 칩: 재개 시각을 알면 사람의 시각(시:분)으로, 모르면 모른다고;
 *     원장 박자에서 칩이 서기까지 몇 ms인가; 누르면 상황판이 열린다;
 *   - 상황판 워커 줄의 띠: 칩·「지금 다시 시도」·넘길 곳·「다른 에이전트로 넘기기」;
 *     확인 창에서 「예」를 해야만 명령이 불리고, 「아니요」면 아무것도 안 불린다;
 *     두 번 눌러도 명령은 한 번; 원장이 거절할 까닭(행의 `retry_refusal`·
 *     `hand_over_refusal`)과 누른 뒤의 거절은 사람에게 보이는 글로 남는다;
 *   - 벽이 시계로 끝나거나(원장 박자가 없어도) 원장이 칩을 거두면(계정 이동) 칩이 내려간다.
 *
 * 원장의 행은 `ledger_agents`가 보내는 모양 그대로이고(`quota_wall` = 백엔드의
 * `wall_chip_for`), 받아들임·거절의 판단은 러스트 시험의 것이다 — 여기서 재는 것은
 * 그림과 사람의 손이다.
 *
 *   node ui/tests/quota-wall-chip.mjs                   이 스위트만
 *   WINDOW_SUITES='^quota-wall-chip$' node ui/tests/window.mjs */
import { mkdir, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";

const OUT = "output/playwright/quota-wall-chip";
const MINUTE = 60_000;

export async function testQuotaWallChip({ browser, origin, ok, faults: shared }) {
  const { page, faults } = await openWindowTestPage(browser, origin, shared ? { faults: shared } : {});
  const measured = {};
  await mkdir(OUT, { recursive: true });
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });

    /* ---- 사이드바: 세 체크아웃 — 재개 시각을 아는 벽, 모르는 벽, 벽 없음 ---------- */
    const sidebar = await page.evaluate(async (minute) => {
      const now = Date.now();
      const scene = [
        { path: "/q/walled", term: 9101, worker: "w-walled",
          chip: { wall: "m-wall-1", observed_at_ms: now - 5 * minute, resets_at_ms: now + 45 * minute,
            stands_until_ms: now + 48 * minute, declared_alt: null, retry_refusal: null, hand_over_refusal: null } },
        { path: "/q/unknown", term: 9102, worker: "w-unknown",
          chip: { wall: "m-wall-2", observed_at_ms: now - 5 * minute, resets_at_ms: null,
            stands_until_ms: now + 355 * minute, declared_alt: null, retry_refusal: null, hand_over_refusal: null } },
        { path: "/q/plain", term: 9103, worker: "w-plain", chip: null },
      ];
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      for (const one of scene) {
        tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
        paneAgents.set(one.term, "codex");
        tell("hook:agent", { term: one.term, state: "idle", agent: "codex", session: `s-${one.term}` });
      }
      window.__ANSWER__.project_catalog = () => [{
        name: "q", path: "/q",
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: scene.map((one, at) => ({
          path: one.path, branch: one.path.slice(3), is_main: false, active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      await refreshWorktrees();
      await window.__PAINTED__();
      const rowOf = (path) => document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
      const shown = (node) => node !== null && !node.hidden && getComputedStyle(node).display !== "none";
      const before = scene.map((one) => shown(rowOf(one.path)?.querySelector(".wt-wall") ?? null));
      // The beat: the ledger's rows arrive, the way `ledger:changed` brings them.
      window.__LEDGER__ = scene.map((one) => ({
        run: "run-q", worker: one.worker, agent: "codex", state: "working", ledger: "active",
        hearing: "pending", hearing_at: 1, checkout: one.path, task: `${one.worker} task`, task_id: `t-${one.term}`,
        reported: false, failed: false, settled: false, session: null, at: 1, term: one.term,
        review: { verified: false, merged: false, deployed: false, written: false },
        wall: one.chip ? { wall: one.chip.wall, observed_at_ms: one.chip.observed_at_ms,
          resets_at_ms: one.chip.resets_at_ms, reset_waitable: one.chip.resets_at_ms !== null,
          stands_until_ms: one.chip.stands_until_ms } : null,
        quota_wall: one.chip,
      }));
      const started = performance.now();
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 41 });
      const chipOf = () => rowOf("/q/walled")?.querySelector(".wt-wall") ?? null;
      for (let frame = 0; frame < 600 && !shown(chipOf()); frame += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const arrivedMs = Math.round(performance.now() - started);
      await window.__PAINTED__();
      const clock = (ms) => {
        const at = new Date(ms);
        return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
      };
      const read = (path) => {
        const chip = rowOf(path)?.querySelector(".wt-wall") ?? null;
        return { shown: shown(chip), text: chip?.textContent ?? "", tip: chip?.dataset.tip ?? "",
          button: chip?.tagName === "BUTTON", term: chip?.dataset.term ?? "" };
      };
      return {
        before, arrivedMs, expected: `한도 대기 · ${clock(scene[0].chip.resets_at_ms)} 재개 예정`,
        walled: read("/q/walled"), unknown: read("/q/unknown"), plain: read("/q/plain"),
      };
    }, MINUTE);
    measured.sidebarChipMs = sidebar.arrivedMs;
    ok("before the beat no sidebar row wears a quota-wall chip", sidebar.before.every((one) => !one), JSON.stringify(sidebar.before));
    ok("the beat that brings a walled worker raises its sidebar chip within a second",
      sidebar.walled.shown && sidebar.arrivedMs < 1000, JSON.stringify({ ms: sidebar.arrivedMs, row: sidebar.walled }));
    ok("the chip says the reset in the person's own clock: 한도 대기 · HH:MM 재개 예정",
      sidebar.walled.text === sidebar.expected, JSON.stringify({ text: sidebar.walled.text, expected: sidebar.expected }));
    ok("a wall that names no reset says so and fills no time in: 한도 대기 · 재개 시각 모름",
      sidebar.unknown.shown && sidebar.unknown.text === "한도 대기 · 재개 시각 모름", JSON.stringify(sidebar.unknown));
    ok("a worker at no wall wears no chip", !sidebar.plain.shown, JSON.stringify(sidebar.plain));
    ok("the chip is a real button that names the worker and the way to the board",
      sidebar.walled.button && sidebar.walled.tip.includes("w-walled") && sidebar.walled.term === "9101",
      JSON.stringify(sidebar.walled));
    await page.screenshot({ path: `${OUT}/sidebar.png` });

    const english = await page.evaluate(async () => {
      setLocale("en", { persist: false });
      paintWorktreeDots();
      const text = document.querySelector('.wt-row[data-worktree-path="/q/unknown"] .wt-wall')?.textContent ?? "";
      setLocale("ko", { persist: false });
      paintWorktreeDots();
      return text;
    });
    ok("the chip speaks the window's language from the catalog", english === "Usage limit · resume time unknown", english);

    const opened = await page.evaluate(async () => {
      document.querySelector('.wt-row[data-worktree-path="/q/walled"] .wt-wall').click();
      for (let frame = 0; frame < 120 && !(boardTab() && activeTabId === boardTab().id); frame += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      return { board: Boolean(boardTab()) && activeTabId === boardTab()?.id, selected: selectedWorktreePaths.has("/q/walled") };
    });
    ok("pressing the sidebar chip opens the task board, and does not select the row under it",
      opened.board && !opened.selected, JSON.stringify(opened));

    /* ---- 상황판: 워커 줄의 띠와 두 단추 --------------------------------------- */
    const deskNow = Date.now();
    await page.evaluate(coordinatorDeskFixture, { workers: 4, now: deskNow });
    await page.waitForSelector(".task-board-row");
    // Worker w-4 is the desk fixture's walled worker; it wears the ledger's chip now.
    const strip = await page.evaluate(async ({ now, minute }) => {
      agentRows = [
        { id: "codex", name: "Codex", installed: true },
        { id: "claude", name: "Claude Code", installed: true },
        { id: "zo", name: "zo", installed: true },
        { id: "gemini", name: "Gemini", installed: false },
      ];
      window.__CHIP__ = (over = {}) => ({
        wall: "m-wall-4", observed_at_ms: now - 5 * minute, resets_at_ms: now + 45 * minute,
        stands_until_ms: now + 48 * minute, declared_alt: null, retry_refusal: null, hand_over_refusal: null, ...over,
      });
      /* One ledger answer, the way the desk takes it: the rows change, the desk reads them again and
       * paints. Settled when the desk holds THIS chip, no read is in flight and no paint is pending —
       * a strip already standing from the last chip proves nothing about the next. */
      window.__SET_CHIP__ = async (chip) => {
        window.__LEDGER__ = window.__LEDGER__.map((row) => (row.worker === "w-4" ? { ...row, quota_wall: chip } : row));
        const started = performance.now();
        refreshDeskLedger();
        const want = JSON.stringify(chip);
        const held = () => JSON.stringify(deskAgents.find((row) => row.worker === "w-4")?.quota_wall ?? null);
        for (let frame = 0; frame < 300; frame += 1) {
          if (held() === want && !deskLedgerAsking && deskPaintFrame === null) break;
          await new Promise((done) => requestAnimationFrame(done));
        }
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        return Math.round(performance.now() - started);
      };
      window.__READ_STRIP__ = () => {
        const node = document.querySelector('#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall');
        const part = (name) => node?.querySelector(`.board-desk-worker-wall-${name}`) ?? null;
        const shown = (one) => one !== null && !one.hidden && getComputedStyle(one).display !== "none";
        return {
          shown: shown(node),
          chip: part("chip")?.textContent ?? "",
          retry: { text: part("retry")?.textContent ?? "", disabled: part("retry")?.disabled ?? null },
          hand: { text: part("handover")?.textContent ?? "", disabled: part("handover")?.disabled ?? null },
          to: [...(part("to")?.options ?? [])].map((option) => ({ value: option.value, label: option.textContent })),
          toShown: shown(part("to")),
          why: shown(part("why")) ? part("why").textContent : "",
          said: shown(part("said")) ? part("said").textContent : "",
          tone: part("said")?.dataset.tone ?? "",
          inMain: Boolean(node?.closest(".board-desk-worker-main")),
        };
      };
      window.__SENT__ = [];
      window.__ANSWER__.quota_wall_retry = (args) => {
        window.__SENT__.push({ command: "quota_wall_retry", ...args });
        return { receipt: "m-resumed-1", line: "The usage limit may have lifted; try again and continue from where you left off." };
      };
      window.__ANSWER__.quota_wall_handover = (args) => {
        window.__SENT__.push({ command: "quota_wall_handover", ...args });
        return { ended: "dp-4", replacement: "w-9", dispatch: "dp-9", agent: "codex" };
      };
      const ms = await window.__SET_CHIP__(window.__CHIP__());
      // Read after the desk painted its roster: the idle worker's row stands, and wears no strip.
      const plain = document.querySelector('#board-view [data-worker="run-desk/w-2"] .board-desk-worker-wall');
      return { ms, read: window.__READ_STRIP__(), plainHidden: plain ? plain.hidden : null };
    }, { now: deskNow, minute: MINUTE });
    measured.deskStripMs = strip.ms;
    const deskClock = await page.evaluate((ms) => {
      const at = new Date(ms);
      return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
    }, deskNow + 45 * MINUTE);
    ok("the desk's walled worker wears the same chip, its two buttons and where it would go",
      strip.read.shown && strip.read.chip === `한도 대기 · ${deskClock} 재개 예정`
        && strip.read.retry.text === "지금 다시 시도" && strip.read.retry.disabled === false
        && strip.read.hand.text === "다른 에이전트로 넘기기" && strip.read.hand.disabled === false,
      JSON.stringify(strip.read));
    ok("the strip stands beside the row's button, never inside it", strip.read.inMain === false, JSON.stringify(strip.read));
    ok("the agents offered are this machine's installed ones other than the worker's own",
      JSON.stringify(strip.read.to.map((one) => one.value)) === JSON.stringify(["codex", "claude"]), JSON.stringify(strip.read.to));
    ok("a worker at no wall has no strip", strip.plainHidden === true, String(strip.plainHidden));
    ok("the desk's strip arrives within a second of the beat", strip.ms < 1000, String(strip.ms));
    await page.screenshot({ path: `${OUT}/desk.png` });

    /* ---- 확인 창: 「아니요」면 아무것도 안 불리고, 「예」면 한 번 불린다 ------------ */
    const press = async (part, answer) => {
      await page.click(`#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall-${part}`);
      await page.waitForSelector("#ask-yes", { state: "visible" });
      const asked = await page.evaluate(() => ({
        title: document.querySelector("#ask-title, .ask-title")?.textContent ?? "",
        body: document.querySelector("#ask-body")?.textContent ?? "",
      }));
      await page.click(answer === true ? "#ask-yes" : "#ask-no");
      await page.evaluate(async () => {
        for (let frame = 0; frame < 60; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      });
      return asked;
    };
    const denied = await press("retry", false);
    const afterDeny = await page.evaluate(() => ({ sent: window.__SENT__.length, counted: window.__COUNTS__.quota_wall_retry ?? 0 }));
    ok("a press the person answers 아니요 calls nothing", afterDeny.sent === 0 && afterDeny.counted === 0,
      JSON.stringify({ afterDeny, denied }));
    const asked = await press("retry", true);
    const afterRetry = await page.evaluate(() => ({ sent: [...window.__SENT__], read: window.__READ_STRIP__() }));
    ok("the confirm says what a retry does before it is done", asked.body.includes("w-4") && asked.body.includes("한 번"), JSON.stringify(asked));
    ok("a yes calls quota_wall_retry once, for the run and the worker, and says what came of it",
      afterRetry.sent.length === 1 && afterRetry.sent[0].command === "quota_wall_retry"
        && afterRetry.sent[0].run === "run-desk" && afterRetry.sent[0].worker === "w-4"
        && afterRetry.read.said === "이어서 하라는 한 줄을 넣었습니다" && afterRetry.read.tone === "ok",
      JSON.stringify(afterRetry));

    /* ---- 넘기기: 고른 에이전트로, 두 번 눌러도 한 번 ---------------------------- */
    const handed = await page.evaluate(async () => {
      const select = document.querySelector('#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall-to');
      select.value = "claude";
      let release;
      window.__ANSWER__.quota_wall_handover = (args) => {
        window.__SENT__.push({ command: "quota_wall_handover", ...args });
        return new Promise((done) => { release = () => done({ ended: "dp-4", replacement: "w-9", dispatch: "dp-9", agent: "claude" }); });
      };
      document.querySelector('#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall-handover').click();
      for (let frame = 0; frame < 30 && document.querySelector("#ask-yes")?.offsetParent === null; frame += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const body = document.querySelector("#ask-body")?.textContent ?? "";
      document.querySelector("#ask-yes").click();
      for (let frame = 0; frame < 10; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      const sending = window.__READ_STRIP__();
      // The second press, while the first is in flight: the button holds.
      document.querySelector('#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall-handover').click();
      for (let frame = 0; frame < 10; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      const askedAgain = document.querySelector("#ask-yes")?.offsetParent !== null;
      release();
      for (let frame = 0; frame < 30; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      return { body, sending, askedAgain, sent: window.__SENT__.filter((one) => one.command === "quota_wall_handover"),
        done: window.__READ_STRIP__() };
    });
    ok("the handover's confirm names the worker and the agent the person chose",
      handed.body.includes("w-4") && handed.body.includes("Claude Code"), handed.body);
    ok("a handover pressed twice is asked for once: the button holds while the first is in flight",
      handed.sent.length === 1 && handed.sending.hand.disabled === true && handed.askedAgain === false,
      JSON.stringify({ sent: handed.sent, sending: handed.sending, askedAgain: handed.askedAgain }));
    ok("the press carries the run, the worker and the chosen agent, and says who took it over",
      handed.sent[0]?.run === "run-desk" && handed.sent[0]?.worker === "w-4" && handed.sent[0]?.to === "claude"
        && handed.done.said === "w-9에게 넘겼습니다", JSON.stringify(handed));

    /* ---- 거절은 사람이 읽는 글로: 누른 뒤의 거절과 미리 아는 거절 -------------- */
    let refusalsSeen = 0;
    const refused = await page.evaluate(async () => {
      window.__ANSWER__.quota_wall_handover = (args) => {
        window.__SENT__.push({ command: "quota_wall_handover", ...args });
        return Promise.reject("worker w-4 is at work again — it is not handed over");
      };
      document.querySelector('#board-view [data-worker="run-desk/w-4"] .board-desk-worker-wall-handover').click();
      for (let frame = 0; frame < 30 && document.querySelector("#ask-yes")?.offsetParent === null; frame += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      document.querySelector("#ask-yes").click();
      for (let frame = 0; frame < 30; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      return window.__READ_STRIP__();
    });
    const refusedShown = refused.said === "안 됨 — worker w-4 is at work again — it is not handed over" && refused.tone === "refused";
    if (refusedShown) refusalsSeen += 1;
    ok("a press the ledger refused says why, in the ledger's words, beside the buttons", refusedShown, JSON.stringify(refused));

    const known = await page.evaluate(async () => {
      const cases = {};
      cases.retry = (await window.__SET_CHIP__(window.__CHIP__({ retry_refusal: "worker w-4 is waiting on an answer it asked for" })), window.__READ_STRIP__());
      cases.typing = (await window.__SET_CHIP__(window.__CHIP__({ retry_refusal: "a continuation is already being typed",
        hand_over_refusal: "a continuation is being typed at dispatch dp-4 — a handover waits until it settles" })), window.__READ_STRIP__());
      agentRows = [{ id: "zo", name: "zo", installed: true }];
      cases.nobody = (await window.__SET_CHIP__(window.__CHIP__({ wall: "m-wall-5" })), window.__READ_STRIP__());
      cases.declared = (await window.__SET_CHIP__(window.__CHIP__({ wall: "m-wall-6", declared_alt: { agent: "codex", model: "gpt-6-astra" } })), window.__READ_STRIP__());
      return cases;
    });
    const retryRefused = known.retry.retry.disabled === true
      && known.retry.why === "다시 시도 안 됨 — worker w-4 is waiting on an answer it asked for"
      && known.retry.hand.disabled === false;
    if (retryRefused) refusalsSeen += 1;
    ok("a retry the ledger would refuse is held, its reason written under it", retryRefused, JSON.stringify(known.retry));
    const typingRefused = known.typing.retry.disabled === true && known.typing.hand.disabled === true
      && known.typing.why.includes("다시 시도 안 됨 — a continuation is already being typed")
      && known.typing.why.includes("넘기기 안 됨 — a continuation is being typed at dispatch dp-4");
    if (typingRefused) refusalsSeen += 2;
    ok("while a continuation is typed both buttons hold and say why: no second retry, no handover",
      typingRefused, JSON.stringify(known.typing));
    const nobodyRefused = known.nobody.hand.disabled === true && known.nobody.why === "넘길 에이전트가 이 기계에 없음"
      && known.nobody.toShown === false && known.nobody.said === "";
    if (nobodyRefused) refusalsSeen += 1;
    ok("with no other agent installed the handover holds and says so; a new wall drops the last wall's words",
      nobodyRefused, JSON.stringify(known.nobody));
    ok("a declared alternative is offered first, by its whole spelling",
      known.declared.to[0]?.value === "codex:gpt-6-astra" && known.declared.to[0]?.label === "codex:gpt-6-astra (지정된 대안)"
        && known.declared.hand.disabled === false, JSON.stringify(known.declared));
    measured.refusalsSeenInWindow = refusalsSeen;

    /* ---- 칩이 내려가는 두 길: 시계, 그리고 원장이 거둔 칩 --------------------------- */
    const down = await page.evaluate(async () => {
      const soon = Date.now() + 400;
      await window.__SET_CHIP__(window.__CHIP__({ wall: "m-wall-7", stands_until_ms: soon }));
      const standing = window.__READ_STRIP__().shown;
      // No beat, no ledger change: only the clock.
      await new Promise((done) => setTimeout(done, 700));
      for (let frame = 0; frame < 10; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      const afterClock = window.__READ_STRIP__().shown;
      await window.__SET_CHIP__(window.__CHIP__({ wall: "m-wall-8" }));
      const again = window.__READ_STRIP__().shown;
      // The ledger takes the chip down (an account switch moved the worker).
      await window.__SET_CHIP__(null);
      return { standing, afterClock, again, moved: window.__READ_STRIP__().shown };
    });
    ok("a wall's chip goes when its window passes, on the clock and with no beat", down.standing && !down.afterClock, JSON.stringify(down));
    ok("a chip the ledger takes down — the worker moved to another account — leaves the desk", down.again && !down.moved, JSON.stringify(down));

    measured.at = new Date().toISOString();
    await writeFile(`${OUT}/measure.json`, `${JSON.stringify(measured, null, 2)}\n`);
    ok("the quota-wall chip raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testQuotaWallChip({
      browser,
      origin,
      ok: (name, pass, detail = "") => {
        console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
        if (!pass) failures++;
      },
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
