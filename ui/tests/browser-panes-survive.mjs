import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* 사람의 판은 사람이 닫을 때만 닫힌다 (t-5453).
 *
 * 2026-09-20 16:13:03·05·09, 명령 하나 없이 브라우저 판 셋이 3초 간격으로
 * 닫혔고 복원 기록(`browser.open_tabs`)이 3 → 2 → 1 → 0으로 따라 비었다 —
 * 재시작해도 돌아올 것이 없었다. 같은 순간 창 자신의 탭(작업 상황판·지식
 * 그래프)도 함께 사라졌고, 살아남은 것은 터미널 탭뿐이었다.
 *
 * 범인은 Escape였다. 그 길은 「무엇이든 듣고 있지 않으면 앞의 탭을 닫는다」
 * (`keyboardTarget() === null`)였고, 그 질문에 예라고 답하는 것은 터미널과
 * 레인을 뺀 전부다 — 로그인된 브라우저 판도, 보드도, 그래프도. 그리고 브라우저
 * 탭이 닫힐 때마다 `dropTab`이 복원 기록을 다시 썼다.
 *
 * 그래서 이 수트는 두 계약을 잰다:
 *   1. Escape는 「읽는 문서」만 닫는다 — 살아 있는 표면은 제 닫기 문으로만 간다.
 *   2. 저장된 `open_tabs`는 사람이 닫은 탭만 잃는다 — 창이 뒤치다꺼리로
 *      걷어간 탭(디스크에서 사라진 체크아웃 따위)은 기록에 그대로 남는다.
 *
 * 그리고 사건의 이웃들 — 다른 판의 설정 쓰기, 되풀이되는 `emulator open`(같은
 * 스트림이 다른 판으로 re-home 되는 길) — 이 판도 기록도 건드리지 못한다는
 * 것을 함께 못 박는다. 재현은 전부 이 가짜 백엔드 안에서만 한다: 설치본 창에
 * 대고 이걸 돌리면 사람의 판을 진짜로 닫는다. */

/* 사건이 남긴 주소 그대로 — 증거
 * `/Users/dev/zerocode/archives/20260920-e2e-v1.1.3/06-window-after-android-open.png`. */
const PERSON_TABS = Object.freeze([
  "https://198.51.100.135/schemes/payments/edit",
  "http://192.0.2.104:8080/actuator/prometheus",
  "https://example.com/r1-walk-fixture",
]);

/* 이 창이 그리는 살아 있는 표면들 — 파일·차이·노트북처럼 「읽는」 것이 아니라
 * 그 안에서 뭔가가 돌고 있는 탭. Escape는 이 중 하나도 닫지 못한다. */
const LIVE_SURFACES = Object.freeze(
  ["browser", "emulator", "board", "knowledge", "vault", "skills", "artifacts", "changes", "worker", "term", "lane"],
);

export async function testBrowserPanesSurvive(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async (urls) => {
      /* 백엔드가 한 말을 전부 적는 장부. 판 닫기와 기록 쓰기는 이 시험의 두
         측정값이므로 세지 말고 남긴다 — 무엇이 닫혔는지가 실패의 내용이다. */
      window.__PANES__ = { closed: [], saved: [], placed: 0, streams: 0 };
      let minted = 0;
      window.__ANSWER__.open_browser_pane = () => `browser-${(minted += 1)}`;
      window.__ANSWER__.browser_place = () => (window.__PANES__.placed += 1, null);
      window.__ANSWER__.browser_navigate = () => null;
      window.__ANSWER__.browser_zoom = () => null;
      window.__ANSWER__.close_browser_pane = (args) => (window.__PANES__.closed.push(args.label), null);
      window.__ANSWER__.set_browser_open_tabs = (args) => {
        const tabs = args.tabs ?? [];
        window.__PANES__.saved.push(tabs.map((one) => one.url));
        browserPrefs = { ...browserPrefs, open_tabs: tabs };
        return { browser: { ...browserPrefs, open_tabs: tabs } };
      };
      /* 되풀이되는 `zerocode-emulator open`이 실제로 받는 답: 같은 스트림이
         「다른 판이 쓰던 것」으로 되돌아온다(창 로그의 `reused by another
         pane; frames re-homed`). */
      window.__ANSWER__.mobile_emulators = () => [{ udid: "U1", name: "Sai Parallel QA 26.5", booted: true }];
      window.__ANSWER__.android_emulators = () => [];
      window.__ANSWER__.start_emulator_stream = () => {
        window.__PANES__.streams += 1;
        return {
          stream: "S1",
          udid: "U1",
          name: "Sai Parallel QA 26.5",
          reused: window.__PANES__.streams > 1,
        };
      };
      window.__ANSWER__.stop_emulator_stream = () => null;
      for (const url of urls) await openBrowserTab(url);
      openBoard();
    }, PERSON_TABS);
    await page.waitForFunction(
      (count) => tabs.filter((one) => one.kind === "browser").length === count
        && tabs.some((one) => one.id === "board"),
      PERSON_TABS.length,
    );

    /* ---- 1. Escape는 사람의 판을 닫지 못한다 ---------------------------- */
    const stood = await page.evaluate(() => ({
      panes: tabs.filter((one) => one.kind === "browser").map((one) => one.label),
      stored: (browserPrefs.open_tabs ?? []).map((one) => one.url),
    }));
    ok("the person's three panes stand, and the restore record names all three",
      stood.panes.length === 3 && stood.stored.length === 3, JSON.stringify(stood));

    /* 사건이 걸린 만큼 — 판 셋과 보드와 그 다음 하나. 한 번에 하나씩 눌러야
       탭이 앞으로 나온 뒤에 그 다음 키가 간다(진짜 사람의 박자). */
    for (let press = 0; press < 5; press += 1) {
      await page.evaluate(() => {
        setActiveTab(tabs.find((one) => one.kind === "browser")?.id ?? activeTabId);
      });
      await page.keyboard.press("Escape");
      await page.waitForTimeout(60);
    }
    const afterEscape = await page.evaluate(() => ({
      panes: tabs.filter((one) => one.kind === "browser").map((one) => one.label),
      board: tabs.some((one) => one.id === "board"),
      closed: [...window.__PANES__.closed],
      stored: (browserPrefs.open_tabs ?? []).map((one) => one.url),
    }));
    ok("Escape closes no browser pane, keeps the board, and writes nothing to the restore record",
      afterEscape.panes.length === 3 && afterEscape.board
        && afterEscape.closed.length === 0 && afterEscape.stored.length === 3,
      JSON.stringify(afterEscape));

    /* ---- 2. …그러나 Escape가 문서를 닫는 일은 그대로다 ------------------ */
    const document_ = await page.evaluate(async () => {
      window.__ANSWER__.read_text_file = () => ({ text: "read me", version: 1 });
      await openPath("/tmp/zerocode-window-test/README.md");
      return { opened: currentTab()?.kind ?? null, id: currentTab()?.id ?? null };
    });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(80);
    const documentGone = await page.evaluate((id) => ({
      gone: !tabs.some((one) => one.id === id),
      panes: tabs.filter((one) => one.kind === "browser").length,
    }), document_.id);
    ok("Escape still closes the document it was written for, and only it",
      document_.opened === "file" && documentGone.gone && documentGone.panes === 3,
      JSON.stringify({ document_, documentGone }));

    /* ---- 3. 살아 있는 표면은 표에 없다 ---------------------------------- */
    /* 표가 아예 없으면 그것이 답이다 — 던지면 나머지 케이스가 안 돌고, 빨강
       한 판은 아직 못 본 것까지 다 말해야 한다. */
    const table = await page.evaluate((live) => {
      const held = typeof ESCAPE_CLOSES === "undefined" ? null : ESCAPE_CLOSES;
      if (held === null) return { closes: null, live };
      return { closes: [...held], live: live.filter((kind) => held.has(kind)) };
    }, LIVE_SURFACES);
    ok("the Escape table holds only read surfaces — no live one is in it",
      table.closes !== null && table.live.length === 0 && table.closes.includes("file"),
      JSON.stringify(table));

    /* ---- 4. 설정 쓰기·emulator open 되풀이 ------------------------------ */
    const storm = await page.evaluate(async () => {
      const before = (browserPrefs.open_tabs ?? []).map((one) => one.url);
      const writes = [];
      for (let round = 0; round < 4; round += 1) {
        /* 이 판이 설정을 쓴다 — 사건의 분에 preferences.json이 두 번 써졌다. */
        writes.push(await commitSetting(
          "browser.restore_tabs", "set_browser_restore_tabs", { enabled: true },
        ).then(() => true, () => false));
        /* 그리고 다른 판이 쓴 것이 이 창에 도착한다. */
        for (const handler of window.__LISTENERS__["settings:changed"] ?? []) {
          handler({ payload: { revision: 3470 + round, keys: ["browser"] } });
        }
        /* 그 사이에 `zerocode-emulator open`이 또 한 번. */
        for (const handler of window.__LISTENERS__["emulator:agent-open"] ?? []) {
          handler({ payload: { platform: "ios" } });
        }
        await new Promise((done) => setTimeout(done, 140));
      }
      return {
        before,
        writes,
        streams: window.__PANES__.streams,
        panes: tabs.filter((one) => one.kind === "browser").map((one) => one.label),
        closed: [...window.__PANES__.closed],
        stored: (browserPrefs.open_tabs ?? []).map((one) => one.url),
      };
    });
    ok("settings writes and repeated `emulator open` re-homings leave every pane and the whole record standing",
      storm.streams >= 4 && storm.panes.length === 3 && storm.closed.length === 0
        && storm.stored.join("|") === storm.before.join("|"),
      JSON.stringify(storm));

    /* ---- 5. 사람이 닫으면 기록도 잊는다 --------------------------------- */
    const byHand = await page.evaluate(async () => {
      /* 앞선 케이스가 이미 다 닫아 버렸으면 닫을 것이 없다 — 그 자체가 답이고,
         던지는 대신 그렇게 말한다(아래 케이스도 돌아야 한다). */
      const going = tabs.find((one) => one.kind === "browser");
      if (going === undefined) return { label: null, closed: [...window.__PANES__.closed], stored: [] };
      closeTab(going.id);
      await new Promise((done) => setTimeout(done, 120));
      return {
        label: going.label,
        closed: [...window.__PANES__.closed],
        stored: (browserPrefs.open_tabs ?? []).map((one) => one.url),
      };
    });
    ok("a close the person asked for takes the pane down AND forgets its address",
      byHand.label !== null && byHand.closed.length === 1 && byHand.closed[0] === byHand.label
        && byHand.stored.length === 2 && !byHand.stored.includes(byHand.label),
      JSON.stringify(byHand));

    /* ---- 6. 창이 걷어간 탭은 기록에 남는다 ------------------------------ */
    const swept = await page.evaluate(async () => {
      const before = (browserPrefs.open_tabs ?? []).map((one) => one.url);
      const wasClosed = window.__PANES__.closed.length;
      /* 체크아웃이 디스크에서 사라졌다 — 그 자리의 표면은 전부 걷힌다. 사람이
         닫은 것이 아니므로 다음 시작에 그 주소는 돌아와야 한다. */
      dropWorktreeSurfaces(activeWorktreePath);
      await new Promise((done) => setTimeout(done, 120));
      return {
        before,
        panes: tabs.filter((one) => one.kind === "browser").length,
        newlyClosed: window.__PANES__.closed.length - wasClosed,
        stored: (browserPrefs.open_tabs ?? []).map((one) => one.url),
      };
    });
    ok("a checkout swept off the disk takes its panes down but leaves the restore record standing",
      swept.panes === 0 && swept.newlyClosed === 2
        && swept.stored.join("|") === swept.before.join("|") && swept.stored.length === 2,
      JSON.stringify(swept));

    ok("the pane survival suite raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testBrowserPanesSurvive(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
      if (!pass) failed += 1;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
