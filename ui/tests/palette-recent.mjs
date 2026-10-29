// 입력 팔레트 「어디로 갈까요」 — 최근 5개와 이동 (t-67485, 시안 p-4571e5542ead6526).
//
// ⌘J는 빈 칸일 때 맨 위에 최근 연 과업·판·아티팩트 5개(가장 최근 순, 1~5 숫자 키)를, 그 아래
// 「이동」 화면 바로가기를, 그 아래 워크트리를 보인다. 글자를 치면 세 묶음이 함께 걸러진다.
// ⌘P는 파일 찾기 그대로다. 합성 데이터만 쓴다(acme, /Users/dev).
//
// 그리는 시간은 `PALETTE_RECENT_REPORT=1`일 때 9번 그려 중앙값을 밀리초로 낸다.
import { PRIMARY_EVENT } from "./test-platform.mjs";

const TREES = ["main", "wt/t-2041/pay-copy", "wt/t-2037/order-list", "wt/t-2033/login-error", "wt/t-2025/home-empty"].map((branch, at) => ({
  path: at === 0 ? "/Users/dev/acme-web" : `/Users/dev/acme-web-${branch.split("/")[1]}`,
  branch, is_main: at === 0, is_folder: false,
}));
const PLACES = [
  { kind: "task", id: "agent:term:2041", label: "t-2041 결제 화면 문구" },
  { kind: "pane", id: "term-2043", label: "w-2043 · claude · acme-web", worktree: "/Users/dev/acme-web-t-2041" },
  { kind: "artifact", id: "a-2051", label: "결제 화면 시안 B", path: "/Users/dev/acme-web/.zerocode/artifacts/b.html" },
  { kind: "task", id: "agent:term:2037", label: "t-2037 주문 목록 속도" },
  { kind: "artifact", id: "a-2052", label: "주문 목록 속도 보고서", path: "/Users/dev/acme-web/.zerocode/artifacts/r.html" },
];
const REPORT_RUNS = 9;

export async function testPaletteRecent(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.addInitScript((trees) => {
    const base = window.__TAURI__.core.invoke;
    // The backend's catalog and its list are one git listing, and the finder lists only the paths the
    // catalog knows (t-82875): the project these worktrees belong to stands in the catalog beside the
    // harness's own.
    const acme = {
      name: "acme-web", path: trees[0].path, slug: null, worktrees: trees.map((tree) => ({ ...tree, active: false })),
      external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
    };
    window.__TAURI__.core.invoke = (name, args) => {
      if (name === "list_worktrees") return Promise.resolve(trees);
      if (name === "project_catalog") return Promise.resolve(base(name, args)).then((catalog) => [...(catalog ?? []), acme]);
      return base(name, args);
    };
  }, TREES);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  await page.evaluate(() => { el("nav-tasks").disabled = false; });

  const known = await page.evaluate(() => typeof rememberRecentPlace === "function" && typeof readRecentPlaces === "function");
  ok("the palette keeps a list of recent places", known === true, `known=${known}`);
  if (!known) { await page.close(); return; }

  const seed = (places) => page.evaluate(async (places) => {
    for (const place of places) rememberRecentPlace(place);
  }, places);
  const rows = () => page.evaluate(() => [...document.querySelectorAll("#qo-results > *")].map((node) => ({
    group: node.classList.contains("qo-group"),
    text: node.querySelector?.(".qo-label")?.textContent ?? node.textContent,
    key: node.querySelector?.(".qo-key")?.textContent ?? null,
  })));
  const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 400)));
  const chord = () => page.evaluate((primary) => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", ...primary, bubbles: true, cancelable: true }));
  }, PRIMARY_EVENT);

  // 가장 오래된 것부터 넣으면 마지막으로 넣은 것이 맨 위다.
  await seed([...PLACES].reverse());
  await chord();
  await settle();
  const opened = await rows();
  const groups = opened.filter((row) => row.group).map((row) => row.text);
  ok("the palette is titled 「어디로 갈까요」 and opens on three groups", groups.join("|") === "최근|이동|워크트리", JSON.stringify(groups));
  const recent = opened.slice(1, 6);
  ok("the five recent places come first, the newest on top, with keys 1 to 5",
    recent.map((row) => row.text).join("|") === PLACES.map((place) => place.label).join("|")
      && recent.map((row) => row.key).join("") === "12345", JSON.stringify(recent));
  const go = opened.slice(opened.findIndex((row) => row.group && row.text === "이동") + 1).filter((row) => !row.group).slice(0, 7).map((row) => row.text);
  ok("the Go group names the doors the sidebar already has", go.join("|") === "작업 상황판|받은 편지함|아티팩트|자동화|보관함|스킬|설정" || go.join("|") === "작업 상황판|받은 편지함|아티팩트|보관함|스킬|설정", JSON.stringify(go));

  // 「받은 편지함」 줄은 받은 편지함 화면을 연다(t-67483의 화면, 사이드바 문과 같은 길).
  // Each step waits for the page's own state, not for a guessed time: the palette is open on its
  // first list, a typed query has replaced the rows it started from (a fresh `paletteRows`), and
  // Enter has closed the palette and made the inbox tab.
  await page.evaluate(() => { closePalette(); togglePalette("worktree"); });
  await page.waitForFunction(() => paletteMode === "worktree" && document.querySelector("#qo-results .qo-row") !== null);
  await page.evaluate(() => {
    window.__PALETTE_ROWS_BEFORE__ = paletteRows;
    const input = document.getElementById("qo-input");
    input.value = "받은 편지함";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await page.waitForFunction(() => paletteRows !== window.__PALETTE_ROWS_BEFORE__ && document.querySelectorAll("#qo-results .qo-row").length === paletteRows.length);
  const shown = await page.evaluate(() => [...document.querySelectorAll("#qo-results .qo-row .qo-label")].map((node) => node.textContent));
  await page.evaluate(() => {
    document.getElementById("qo-input").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  });
  await page.waitForFunction(() => paletteMode === null && tabs.some((tab) => tab.kind === "inbox")).catch(() => null);
  const inbox = await page.evaluate(() => ({ shown: null, closed: paletteMode === null, opened: tabs.some((tab) => tab.kind === "inbox") }));
  inbox.shown = shown;
  ok("the Go group's 받은 편지함 opens the inbox page and closes the palette",
    inbox.shown.join("|") === "받은 편지함" && inbox.closed === true && inbox.opened === true, JSON.stringify(inbox));

  // 키 이벤트 두 개: ⌘J, 숫자 하나. 첫째 최근(과업)을 상황판에서 다시 고른다.
  const keys = await page.evaluate(async (primary) => {
    const seen = [];
    const count = (event) => seen.push(event.key);
    const revealed = [];
    const original = window.revealTaskBoardMember;
    window.revealTaskBoardMember = async (key) => { revealed.push(key); return true; };
    closePalette();
    window.addEventListener("keydown", count, true);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", ...primary, bubbles: true, cancelable: true }));
    await new Promise((done) => setTimeout(done, 400));
    document.getElementById("qo-input").dispatchEvent(new KeyboardEvent("keydown", { key: "1", bubbles: true, cancelable: true }));
    await new Promise((done) => setTimeout(done, 100));
    window.removeEventListener("keydown", count, true);
    window.revealTaskBoardMember = original;
    return { seen, revealed, closed: paletteMode === null };
  }, PRIMARY_EVENT);
  ok("reopening a recent place takes two key events and closes the palette",
    keys.seen.length === 2 && keys.closed === true && keys.revealed.join() === "agent:term:2041", JSON.stringify(keys));

  // 숫자는 입력칸이 빌 때만 동작한다.
  const digit = await page.evaluate(async () => {
    closePalette();
    togglePalette("worktree");
    await new Promise((done) => setTimeout(done, 400));
    const input = document.getElementById("qo-input");
    input.value = "t-";
    const typed = new KeyboardEvent("keydown", { key: "2", bubbles: true, cancelable: true });
    input.dispatchEvent(typed);
    return { prevented: typed.defaultPrevented, stillOpen: !document.getElementById("qo-scrim").hidden };
  });
  ok("a digit opens a recent place only while the field is empty", digit.prevented === false && digit.stillOpen === true, JSON.stringify(digit));

  // 글자를 치면 세 묶음이 함께 걸러진다.
  await page.evaluate(() => { document.getElementById("qo-input").value = "2041"; document.getElementById("qo-input").dispatchEvent(new Event("input", { bubbles: true })); });
  await settle();
  const filtered = await rows();
  const kept = filtered.filter((row) => !row.group).map((row) => row.text);
  ok("typing filters recent places and worktrees together",
    kept.includes("t-2041 결제 화면 문구") && kept.includes("wt/t-2041/pay-copy") && !kept.includes("t-2037 주문 목록 속도"), JSON.stringify(kept));

  // 최근은 5개다: 여섯째를 열면 가장 오래된 것이 빠진다.
  const capped = await page.evaluate(() => {
    for (let n = 1; n <= 6; n += 1) rememberRecentPlace({ kind: "task", id: `agent:term:${n}`, label: `t-${n} 합성` });
    return readRecentPlaces().map((place) => place.id);
  });
  ok("the recent list holds five and drops the oldest", capped.length === 5 && capped[0] === "agent:term:6" && !capped.includes("agent:term:1"), JSON.stringify(capped));

  // ⌘P는 파일 찾기 그대로다: 빈 칸이면 아무것도 보이지 않는다.
  const file = await page.evaluate(async () => {
    closePalette();
    togglePalette("file");
    await new Promise((done) => setTimeout(done, 400));
    return { groups: document.querySelectorAll("#qo-results .qo-group").length, rows: document.querySelectorAll("#qo-results .qo-row").length, title: document.getElementById("qo-title").textContent };
  });
  ok("the file finder stays as it was: no groups, no rows before a word", file.groups === 0 && file.rows === 0, JSON.stringify(file));

  if (process.env.PALETTE_RECENT_REPORT) {
    // What the groups cost on top of the worktree list: the finder's whole answer and drawing
    // (`runPaletteQuery`), nine times with five recent places and nine times with none.
    const paint = (places) => page.evaluate(async ({ places, runs }) => {
      localStorage.removeItem(RECENT_PLACES_KEY);
      for (const place of [...places].reverse()) rememberRecentPlace(place);
      closePalette();
      togglePalette("worktree");
      await new Promise((done) => setTimeout(done, 400));
      const out = [];
      for (let n = 0; n < runs; n += 1) {
        const started = performance.now();
        await runPaletteQuery();
        out.push(performance.now() - started);
      }
      return out.sort((a, b) => a - b)[Math.floor(runs / 2)];
    }, { places, runs: REPORT_RUNS });
    const bare = await paint([]);
    const grouped = await paint(PLACES);
    const delta = grouped - bare;
    console.log(`MEASURE palette_recent_paint_ms bare=${bare.toFixed(2)} grouped=${grouped.toFixed(2)} added=${delta.toFixed(2)}`);
    ok("the groups add at most 2.1 ms to the worktree list's paint", delta <= 2.1, `added=${delta.toFixed(2)} ms`);
  }
  await page.close();
}
