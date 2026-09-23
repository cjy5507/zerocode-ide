import { openWindowTestPage } from "./window-boot.mjs";

/* 에이전트가 빌린 기기는 그 과업과 함께 꺼진다 (t-6336).
 *
 * 09-23, 다른 세션의 앱 감사가 띄운 시뮬레이터 둘과 에뮬레이터 하나가 세션이
 * 끝난 뒤에도 켜진 채 메모리(압축 18 GiB)를 눌렀다. 백엔드는 이제 문이
 * 에이전트의 판을 위해 부팅한 기기를 「빌린 기기」로 적고, 그 판이 닫히거나
 * worker_done을 보내거나 세션이 끝나면 끈다. 창의 몫은 셋이다:
 *   1. 에이전트의 open만 스트림 시작에 부른 판(`borrower`)을 싣는다 — 팔레트도,
 *      부른 판을 모르는 open도 싣지 않는다(사람의 기기는 빌린 것이 아니다);
 *   2. 빌린 기기가 반납되면 그 기기를 비추던 에이전트의 거울 탭을 걷는다 —
 *      사람의 탭은 그대로;
 *   3. 상태바에 「빌린 기기 n · 마지막 사용」 한 줄을 세우고, 0이면 치운다.
 * 전부 가짜 백엔드 안에서 잰다. */

const AWAY = "/tmp/zerocode-window-test/wt-emulator-loans";

export async function testEmulatorLoans(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const boot = await page.evaluate(() => ({ asked: window.__COUNTS__.emulator_loans ?? 0 }));
    await page.evaluate(() => {
      window.__LOANS__ = { starts: [] };
      const fleet = [
        { udid: "U1", name: "iPhone 17", booted: true },
        { udid: "U2", name: "t-6336 probe", booted: false },
      ];
      window.__ANSWER__.mobile_emulators = () => fleet;
      window.__ANSWER__.android_emulators = () => [];
      window.__ANSWER__.start_emulator_stream = (args) => {
        window.__LOANS__.starts.push({ udid: args?.udid ?? null, borrower: args?.borrower ?? null });
        const device = fleet.find((one) => one.udid === args?.udid) ?? fleet[0];
        return { stream: `L${window.__LOANS__.starts.length}`, udid: device.udid, name: device.name };
      };
      window.__ANSWER__.stop_emulator_stream = () => null;
    });
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    const fire = (name, payload) => page.evaluate(({ name: event, said }) => {
      for (const handler of window.__LISTENERS__[event] ?? []) handler({ payload: said });
    }, { name, said: payload });

    /* ---- 1. 에이전트의 open만 부른 판을 싣는다 -------------------------- */
    const callers = await page.evaluate(async (away) => {
      const leader = await openTermTab({ placement: "tab" });
      const worker = 13890;
      for (const handler of window.__LISTENERS__["term:worker"] ?? []) {
        handler({ payload: { parent: leader, term: worker, worktree: away, agent: "claude", resumed: "fresh" } });
      }
      return { leader, worker };
    }, AWAY);
    await settle();
    await fire("emulator:agent-open", { platform: "ios", device: "U2", term: callers.worker });
    await settle();
    await fire("emulator:agent-open", { platform: "ios", device: "U1", term: callers.leader });
    await settle();
    await fire("emulator:agent-open", { platform: "ios", device: "U1" });
    await settle();
    await page.evaluate(() => openEmulatorTab("ios", "U1"));
    await settle();
    const starts = await page.evaluate(() => window.__LOANS__.starts);
    ok(
      "only an agent's open names the pane it borrows for — the palette and a nameless open do not",
      starts.length === 4
        && starts[0].udid === "U2" && starts[0].borrower === callers.worker
        && starts[1].udid === "U1" && starts[1].borrower === callers.leader
        && starts[2].borrower === null && starts[3].borrower === null,
      JSON.stringify({ callers, starts }),
    );

    /* ---- 2. 반납된 기기의 에이전트 거울만 걷힌다 ------------------------ */
    const before = await page.evaluate(() => tabs
      .filter((one) => one.kind === "emulator")
      .map((one) => ({ id: one.id, device: one.deviceId, borrower: one.borrower ?? null })));
    await fire("emulator:loan-returned", { platform: "ios", device: "U2" });
    await settle();
    const after = await page.evaluate(() => tabs
      .filter((one) => one.kind === "emulator")
      .map((one) => ({ id: one.id, device: one.deviceId, borrower: one.borrower ?? null })));
    const agentsU2 = before.filter((one) => one.device === "U2" && one.borrower !== null);
    ok(
      "a returned device takes its borrower's mirror off the strip, and every other mirror stays",
      agentsU2.length === 1
        && !after.some((one) => one.id === agentsU2[0].id)
        && after.length === before.length - 1,
      JSON.stringify({ before, after }),
    );

    /* ---- 3. 상태바의 한 줄 ---------------------------------------------- */
    const words = await page.evaluate(() => ({
      three: t("emulator.loans", "빌린 기기 {{count}} · 마지막 사용 {{ago}} 전", {
        count: 2,
        ago: agoWord(Date.now() - 3 * 60_000, Date.now()),
      }),
      now: t("emulator.loansNow", "빌린 기기 {{count}} · 방금 사용", { count: 1 }),
    }));
    await fire("emulator:loans", { count: 2, lastUsedMs: Date.now() - 3 * 60_000 });
    await settle();
    const lent = await page.evaluate(() => {
      const chip = document.getElementById("sb-loans");
      return { shown: chip ? !chip.hidden : false, said: chip?.textContent.trim() ?? null };
    });
    await fire("emulator:loans", { count: 1, lastUsedMs: Date.now() });
    await settle();
    const fresh = await page.evaluate(() => document.getElementById("sb-loans")?.textContent.trim() ?? null);
    await fire("emulator:loans", { count: 0, lastUsedMs: null });
    await settle();
    const none = await page.evaluate(() => document.getElementById("sb-loans")?.hidden ?? null);
    ok(
      "the status bar says how many devices agents borrowed and when one was last used, and nothing when none",
      boot.asked >= 1 && lent.shown && lent.said === words.three && fresh === words.now && none === true,
      JSON.stringify({ boot, lent, fresh, none, words }),
    );

    ok("the emulator loans page raised nothing", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
