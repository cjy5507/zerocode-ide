/* The conversation's weight, held to what the person passed on 2026-11-10 (window artifact
 * p-1c1bbf75726ff227, 「성능·메모리」): a long conversation opens on its last turns and the ones above them
 * stand in steps after the first frame, with the reader's place kept; the folded 400-turn page holds no more
 * elements than its budget. Counts and places, never times — a time is the measuring tool's to read
 * (`conversation-perf.mjs --scenarios --folded`), on a quiet machine.
 *
 *   WINDOW_SUITES='conversation-budget' node ui/tests/window.mjs */
import { conversationFixture, openFixtureConversation } from "./conversation-perf.mjs";
import { foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* The folded 400-turn page's elements: 694 before the conversation of 2026-11-10, 730 the bound it was passed
 * with (the answer's card of what its turn changed is the difference). */
const LIST_ELEMENTS = 730;

/* What the list holds and where it stands. */
function readOpen() {
  const list = document.querySelector("#worker-view .helper-turns");
  const rows = [...list.querySelectorAll(":scope > [data-turn]")];
  return {
    rows: rows.length,
    turns: rows.reduce((sum, row) => sum + (row.__members?.length ?? 1), 0),
    held: list.__run.helper.turns.length,
    floor: list.__openFloor ?? null,
    stepping: list.__openFloor !== null && list.__openFloor !== undefined,
    gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight),
    screens: Math.round((list.scrollHeight / Math.max(1, list.clientHeight)) * 10) / 10,
    elements: list.getElementsByTagName("*").length,
    firstRole: rows[0]?.__turn?.role ?? rows[0]?.__members?.[0]?.role ?? null,
  };
}

export async function testConversationBudget(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    // The page as the product opens it: folded before the conversation stands, so the open builds folded rows.
    await foldConversations(page);
    // Every paint of the list, with what it had drawn when the paint returned — counted off the rows, with
    // no read of the page's layout inside the paint: a read there lays the list out before its own frame and
    // changes which rows the shelf takes for near. Where the list stands is read once the paint's frame and
    // the one after it are drawn — the frame the list's own follow settles in (`scrollHelperToBottom`).
    await page.evaluate(() => {
      window.__PAINTS__ = [];
      window.__GAPS__ = [];
      const real = syncHelperTurns;
      syncHelperTurns = (list, run) => {
        real(list, run);
        if (run.helper.turns.length === 0) return;
        const rows = [...list.querySelectorAll(":scope > [data-turn]")];
        const turns = rows.reduce((sum, row) => sum + (row.__members?.length ?? 1), 0);
        window.__PAINTS__.push({ turns, floor: list.__openFloor ?? null });
        requestAnimationFrame(() => requestAnimationFrame(() => {
          // A list with no place yet has no foot to be away from.
          if (list.__follow?.placed !== true || list.clientHeight === 0) return;
          window.__GAPS__.push({ turns, gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight) });
        }));
      };
    });
    await openFixtureConversation(page, conversationFixture());
    await page.waitForFunction(() => {
      const floor = document.querySelector("#worker-view .helper-turns").__openFloor;
      return floor === null;
    });
    // What the page settles to: the frames after the last step, and the shelf's own moments for the rows far
    // from view — it lets a body go some frames after the row left its reach, so the weight is waited for,
    // not read at a fixed moment.
    await page.evaluate(async () => { for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__(); });
    await page.waitForFunction((cap) => document.querySelector("#worker-view .helper-turns").getElementsByTagName("*").length <= cap,
      LIST_ELEMENTS, { timeout: 8000 }).catch(() => {});
    const whole = await page.evaluate(readOpen);
    const paints = await page.evaluate(() => window.__PAINTS__);
    const gaps = await page.evaluate(() => window.__GAPS__);
    // The paints that drew turns: the open's first, then one for each step up.
    const grew = paints.filter((paint, at) => at === 0 || paint.turns > paints[at - 1].turns);
    ok("budget: a 400-turn conversation opens on its last turns — back to a person's words, two screens and more of them — and its first paint waits for no other row",
      grew[0].turns >= 80 && grew[0].turns < 200 && grew[0].floor !== null && whole.rows > 0,
      JSON.stringify({ rows: whole.rows, first: grew[0] }));
    ok("budget: the turns above them stand in steps after it, every one of the 400 in the end, and a reader at the foot is at the foot after each step",
      grew.length >= 3 && grew.at(-1).turns === 400 && whole.turns === 400 && whole.held === 400 && !whole.stepping &&
        gaps.length >= 2 && gaps.every((one) => one.gap <= 1) && whole.gap <= 1,
      JSON.stringify({ steps: grew.map((paint) => paint.turns), gaps, whole }));
    ok(`budget: the folded 400-turn page holds no more than ${LIST_ELEMENTS} elements in its list`,
      whole.elements <= LIST_ELEMENTS, JSON.stringify({ elements: whole.elements, rows: whole.rows }));
    ok("budget: the open raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  await testConversationOpenPlace(browser, origin, ok);
}

/* A reader who went up before the steps finished keeps the row they read; a find stands every turn at once;
 * the toggle draws the page anew from its last turns; a short conversation stands whole. */
async function testConversationOpenPlace(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    await openFixtureConversation(page, conversationFixture({ blocks: 3 }));
    const short = await page.evaluate(readOpen);
    ok("budget: a conversation of some sixty turns stands whole at once — there is nothing to step up to",
      short.floor === null && short.held < 120 && short.turns === short.held, JSON.stringify(short));
    await page.close();
  } finally {
    if (!page.isClosed()) await page.close();
  }
  const second = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(second.page);
    // Open with the steps held, so the page stands on its last turns for as long as the check needs; then go up
    // the list and let the rest stand at once.
    const kept = await second.page.evaluate(async ({ history }) => {
      const realSteps = standRestOfOpen;
      standRestOfOpen = () => {};
      window.__RELEASE_STEPS__ = () => { standRestOfOpen = realSteps; };
      window.__ANSWER__.wire_start = (args) => ({ id: 32, agent: args.agent, protocol: "claude-stream", version: "2.1.278", model: null, session: null });
      window.__ANSWER__.wire_log = () => ({ found: true, skipped: false, next: 0, turns: [], status: "idle", asks: [], live: [], agent: "claude",
        protocol: "claude-stream", models: [], modes: [], commands: [], version: "2.1.278" });
      window.__ANSWER__.wire_stop = () => null;
      window.__ANSWER__.wire_image = () => "";
      await openWirePage("claude", "/tmp/zerocode-window-test", { history });
      const list = document.querySelector("#worker-view .helper-turns");
      // The list takes its place at its first box; a reader cannot go up before that.
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      const before = { stepping: list.__openFloor !== null && list.__openFloor !== undefined, rows: list.querySelectorAll(":scope > [data-turn]").length };
      // The reader goes up: a wheel, then where the list went.
      list.dispatchEvent(new WheelEvent("wheel", { deltaY: -600, bubbles: true }));
      list.scrollTop = Math.max(0, list.scrollTop - 600);
      list.dispatchEvent(new Event("scroll"));
      const top = list.getBoundingClientRect().top;
      const read = [...list.querySelectorAll(":scope > [data-turn]")].find((row) => row.getBoundingClientRect().bottom > top);
      const offset = Math.round(read.getBoundingClientRect().top - top);
      completeChatOpen(list);
      window.__RELEASE_STEPS__();
      // The frame in which the person's long words above are cut (`clipPersonRows`), and one more.
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      const rows = [...list.querySelectorAll(":scope > [data-turn]")];
      return {
        before, floor: list.__openFloor ?? null,
        turns: rows.reduce((sum, row) => sum + (row.__members?.length ?? 1), 0),
        moved: Math.round(read.getBoundingClientRect().top - top) - offset, stillThere: read.isConnected,
        away: list.__follow.away,
      };
    }, { history: conversationFixture() });
    ok("budget: a reader who went up before the steps finished keeps the row they read where it stood when the rest stands above it — also once the long words above are cut to their clip",
      kept.before.stepping && kept.floor === null && kept.turns === 400 && kept.stillThere && Math.abs(kept.moved) <= 2 && kept.away,
      JSON.stringify(kept));

    // The toggle draws the page anew from its last turns; a find stands every turn at once.
    const anew = await second.page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      const drawn = () => [...list.querySelectorAll(":scope > [data-turn]")].reduce((sum, row) => sum + (row.__members?.length ?? 1), 0);
      setChatViewMode("all");
      const toggled = { stepping: list.__openFloor !== null && list.__openFloor !== undefined, turns: drawn() };
      const first = list.__run.helper.turns[0];
      const spot = chatFindSpot(list, list.__run, first);
      const found = { floor: list.__openFloor ?? null, turns: drawn(), spot: Boolean(spot) };
      setChatViewMode("normal");
      return { toggled, found };
    });
    ok("budget: 「전체」 draws the page anew from its last turns, and a find for the first turn stands all 400 at once",
      anew.toggled.stepping && anew.toggled.turns >= 80 && anew.toggled.turns < 400 && anew.found.floor === null && anew.found.turns === 400 && anew.found.spot,
      JSON.stringify(anew));
    ok("budget: the places raised no page errors", faults.length === 0 && second.faults.length === 0, [...faults, ...second.faults].join("\n"));
  } finally {
    await second.page.close();
  }
}
