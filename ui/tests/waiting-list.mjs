// 나를 기다림 — the sidebar's top list (t-26595): what it shows, where it sits, and
// what a click does.
//
// Rust decides the rows (`waiting_on_me`, the notify rules). This suite stands in
// for that answer and checks the window: the list stays hidden while nothing waits,
// it sits above the shortcut rows at the top of the column, a block and a question
// come before a finish, and a click opens the pane and releases its finish mark
// (`clear_finish_mark`).
//
// The painting cost prints when `WAITING_LIST_REPORT=1`: the median time to paint
// 200 rows, in milliseconds. The low-spec profile runs this under `taskpolicy -b`.
const REPORT_ROWS = 200;
const REPORT_RUNS = 9;

export async function testWaitingList(browser, origin, standBackend, ok) {
  const page = await browser.newPage();
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__WAITING__ = { rows: [], asks: 0, cleared: [] };
    window.__TAURI__.core.invoke = (name, args) => {
      const state = window.__WAITING__;
      if (name === "waiting_on_me") {
        state.asks += 1;
        return Promise.resolve(state.rows.map((row) => ({ ...row })));
      }
      if (name === "clear_finish_mark") {
        state.cleared.push(args?.pane);
        state.rows = state.rows.filter((row) => !(row.pane === args?.pane && row.waiting === "finished"));
        return Promise.resolve(true);
      }
      return base(name, args);
    };
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);

  const setRows = (rows) =>
    page.evaluate(async (rows) => {
      window.__WAITING__.rows = rows;
      await refreshWaitingList();
    }, rows);

  // Nothing waits: the list is not drawn at all.
  await setRows([]);
  const hiddenWhenEmpty = await page.evaluate(() => document.getElementById("waiting-list").hidden);
  ok("the waiting list stays hidden while nothing waits", hiddenWhenEmpty === true, `hidden=${hiddenWhenEmpty}`);

  // Three panes wait, in the order Rust sends them: a block, a question, a finish.
  const rows = [
    { pane: "term:5", agent: "zo", worktree: "/work/acme/wt-c", waiting: "blocked", since_ms: 1 },
    { pane: "term:3", agent: "codex", worktree: "/work/acme/wt-b", waiting: "question", since_ms: 2 },
    { pane: "term:7", agent: "claude", worktree: "/work/acme/wt-a", waiting: "finished", since_ms: 3 },
  ];
  await setRows(rows);
  const shown = await page.evaluate(() => {
    const box = document.getElementById("waiting-list");
    const buttons = [...box.querySelectorAll(".waiting-row")];
    return {
      visible: !box.hidden,
      kinds: buttons.map((button) => button.dataset.waiting),
      kindWords: buttons.map((button) => button.querySelector(".waiting-row-kind").textContent),
      places: buttons.map((button) => button.querySelector(".waiting-row-place").textContent),
    };
  });
  ok("the list shows once a pane waits", shown.visible === true, JSON.stringify(shown));
  ok(
    "a block and a question come before a finish, in the order Rust sent",
    JSON.stringify(shown.kinds) === JSON.stringify(["blocked", "question", "finished"]),
    JSON.stringify(shown.kinds),
  );
  ok(
    "each row names its kind in words (Korean is the source)",
    JSON.stringify(shown.kindWords) === JSON.stringify(["막힘", "질문", "끝남"]),
    JSON.stringify(shown.kindWords),
  );
  ok(
    "each row names its worktree's own folder",
    JSON.stringify(shown.places) === JSON.stringify(["wt-c", "wt-b", "wt-a"]),
    JSON.stringify(shown.places),
  );

  // The list sits at the very top of the column, above the shortcut rows.
  const placement = await page.evaluate(() => {
    const list = document.getElementById("waiting-list");
    const nav = list.parentElement?.querySelector(":scope > nav.nav-rows");
    return {
      inColumn: list.parentElement?.classList.contains("threads") === true,
      aboveNav: Boolean(nav) && Boolean(list.compareDocumentPosition(nav) & Node.DOCUMENT_POSITION_FOLLOWING),
      firstChild: list.parentElement?.firstElementChild?.id ?? null,
    };
  });
  ok(
    "the list is the first thing in the column, above the shortcut rows",
    placement.inColumn && placement.aboveNav && placement.firstChild === "waiting-list",
    JSON.stringify(placement),
  );

  // A click opens the pane and releases its finish mark, then the list asks again.
  const before = await page.evaluate(() => window.__WAITING__.asks);
  await page.evaluate(() => document.querySelector('.waiting-row[data-waiting="finished"]').click());
  await page.waitForFunction((from) => window.__WAITING__.asks > from, before, { timeout: 5000 });
  const cleared = await page.evaluate(() => window.__WAITING__.cleared);
  ok("a click on a finished row releases that pane's finish mark", JSON.stringify(cleared) === JSON.stringify(["term:7"]), JSON.stringify(cleared));
  const afterClick = await page.evaluate(() => [...document.querySelectorAll("#waiting-list .waiting-row")].map((button) => button.dataset.waiting));
  ok(
    "after the release the finished row is gone and the blocks stay",
    JSON.stringify(afterClick) === JSON.stringify(["blocked", "question"]),
    JSON.stringify(afterClick),
  );

  if (process.env.WAITING_LIST_REPORT === "1") {
    const bulk = Array.from({ length: REPORT_ROWS }, (_, index) => ({
      pane: `term:${index + 100}`,
      agent: index % 2 ? "claude" : "codex",
      worktree: `/work/acme/wt-${index % 9}`,
      waiting: ["blocked", "question", "finished"][index % 3],
      since_ms: index,
    }));
    const timings = await page.evaluate(({ bulk, runCount }) => {
      waitingRows = bulk;
      const runs = [];
      for (let run = 0; run < runCount; run += 1) {
        const started = performance.now();
        paintWaitingList();
        runs.push(performance.now() - started);
      }
      waitingRows = [];
      return runs.sort((left, right) => left - right);
    }, { bulk, runCount: REPORT_RUNS });
    const median = timings[Math.floor(timings.length / 2)];
    console.log(
      `WAITING_LIST_REPORT rows=${REPORT_ROWS} runs=${REPORT_RUNS} paint_ms_median=${median.toFixed(2)} paint_ms_min=${timings[0].toFixed(2)} paint_ms_max=${timings[timings.length - 1].toFixed(2)}`,
    );
  }

  await page.close();
}
