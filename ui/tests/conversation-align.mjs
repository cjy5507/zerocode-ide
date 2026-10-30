/* The conversation's answers as they were written (t-85975, 10-30): a table that starts on the line
 * right under a sentence is a table, and in a narrow page it scrolls inside its own box instead of
 * pushing the page sideways. Read off the laid-out page, in both treatments.
 *
 *   node ui/tests/conversation-align.mjs
 *   CONVERSATION_ALIGN_SHOTS=<folder> node ui/tests/conversation-align.mjs   keeps a picture per width and treatment */
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { openConversation } from "./conversation-parity.mjs";
import { mockupTurns } from "./conversation-redesign.mjs";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

const WIDTHS = Object.freeze([390, 1000, 1440]);
/* The column steps at 1700px (`--reading-column-wide-*`): 390 is the narrow page, 1000 and 1440 the first steps. */
const COLUMN_WIDTHS = Object.freeze([390, 1000, 1440, 1800]);

/* The answer the person's screenshot showed: a sentence, then the table on the very next line. */
const TABLE_ANSWER = [
  "여기까지 채웠습니다(4/6 단계).",
  "| 칸 | 값 |",
  "|---|---|",
  "| 회사명 | 제로코드 주식회사 |",
  `| 주소 | ${"서울특별시 강남구 테헤란로 아주 긴 주소 ".repeat(3)} |`,
  "| 상세 주소 | 5층 |",
  "",
  "다음 단계로 넘어갑니다.",
].join("\n");

/* An answer with every shape the column holds: a paragraph, bold, a list, a quote, a code block and a table. */
const SHAPES_ANSWER = [
  "먼저 **굵은 글**이 들어간 문단입니다.",
  "",
  "- 목록 첫째",
  "- 목록 둘째",
  "",
  "> 인용 한 줄",
  "",
  "```js",
  "const column = 1;",
  "```",
  "",
  "| 칸 | 값 |",
  "|---|---|",
  "| 가 | 나 |",
].join("\n");

/* The left edge each kind of row paints its first mark at, read off the laid-out page: a pill's dot and a
 * step's dot sit where the answer's words start, a code block's and a table's box starts there too, and the
 * person's bubble ends on the column's right edge. Everything is measured against the answer paragraph. */
const READ_COLUMN = `(() => {
  const root = document.querySelector("#worker-view .helper-turns");
  const px = (value) => Number.parseFloat(value) || 0;
  const said = [...root.querySelectorAll(".helper-turn.is-assistant .helper-said")].find((one) => one.textContent.includes("굵은 글"));
  const left = (node) => (node ? Math.round(node.getBoundingClientRect().left * 10) / 10 : null);
  const markLeft = (node, dot) => {
    if (!node) return null;
    const style = getComputedStyle(node);
    return Math.round((node.getBoundingClientRect().left + px(style.paddingLeft) + (dot ? px(getComputedStyle(dot).marginLeft) : 0)) * 10) / 10;
  };
  const cap = root.querySelector(".helper-group-cap");
  const step = root.querySelector(".helper-turn.is-tool > .helper-step-line");
  const bubble = root.querySelector(".helper-turn.is-user > .helper-said");
  const actions = root.querySelector(".helper-turn.is-assistant .helper-actions");
  /* The ring turns, so its own box is not where it stands: the line's padding edge is. */
  const status = root.querySelector(".helper-status") ?? document.querySelector("#worker-view .helper-status");
  return {
    prose: left(said?.querySelector(":scope > p")),
    list: left(said?.querySelector("ul, ol")),
    quote: left(said?.querySelector("blockquote")),
    code: left(said?.querySelector("pre")),
    table: left(said?.querySelector("table")),
    pillDot: markLeft(cap),
    stepDot: markLeft(step, step?.querySelector(".helper-step-dot")),
    actions: left(actions?.querySelector(".icon")),
    status: status && !status.hidden ? markLeft(status) : null,
    bubbleRight: bubble ? Math.round(bubble.getBoundingClientRect().right * 10) / 10 : null,
    columnRight: said ? Math.round(said.getBoundingClientRect().right * 10) / 10 : null,
    pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    elements: root.querySelectorAll("*").length,
  };
})()`;

export async function testConversationColumn(browser, origin, ok, { shots = process.env.CONVERSATION_ALIGN_SHOTS ?? "" } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    const turns = mockupTurns(Date.now() - 60_000);
    const last = turns.pop();
    turns.push({ role: "assistant", text: SHAPES_ANSWER, at_ms: last.at_ms - 900 }, last);
    await openConversation(page, turns, { status: "working" });
    if (shots) mkdirSync(shots, { recursive: true });
    /* The pills' steps stand under them only once opened. */
    await page.evaluate(async () => {
      for (const cap of document.querySelectorAll("#worker-view .helper-group-cap")) {
        if (cap.getAttribute("aria-expanded") !== "true") cap.click();
      }
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, theme);
      const seen = {};
      for (const width of COLUMN_WIDTHS) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate(async () => {
          for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
        });
        seen[width] = await page.evaluate(READ_COLUMN);
        if (shots) await page.screenshot({ path: join(shots, `column-${theme}-${width}.png`) });
      }
      for (const width of COLUMN_WIDTHS) {
        const one = seen[width];
        const marks = ["list", "quote", "code", "table", "pillDot", "stepDot", "actions", "status"].filter((key) => one[key] !== null);
        const off = marks.filter((key) => Math.abs(one[key] - one.prose) > 1.5);
        ok(`${theme} ${width}px: every row starts its marks where the answer's words start (${marks.join(", ")})`,
          one.prose !== null && marks.length >= 7 && off.length === 0, JSON.stringify(one));
        ok(`${theme} ${width}px: the person's bubble ends on the column's right edge and the page does not scroll sideways`,
          one.bubbleRight !== null && Math.abs(one.bubbleRight - one.columnRight) <= 1.5 && one.pageOverflow <= 0, JSON.stringify(one));
      }
    }
    ok("the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testConversationAlign(browser, origin, ok, { shots = process.env.CONVERSATION_ALIGN_SHOTS ?? "" } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    const turns = mockupTurns(Date.now() - 60_000);
    const last = turns.pop();
    turns.push({ role: "assistant", text: TABLE_ANSWER, at_ms: last.at_ms - 900 }, last);
    await openConversation(page, turns);
    if (shots) mkdirSync(shots, { recursive: true });
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, theme);
      const seen = {};
      for (const width of WIDTHS) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate(async () => {
          for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
        });
        seen[width] = await page.evaluate(() => {
          const said = [...document.querySelectorAll("#worker-view .helper-said")].find((one) => one.textContent.includes("4/6"));
          const table = said?.querySelector("table.md-table") ?? null;
          const scroller = table;
          return {
            table: table !== null,
            rows: table?.rows.length ?? 0,
            sentence: said?.querySelector(":scope > p.md-para")?.textContent ?? "",
            pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
            scrollerOverflowX: scroller ? getComputedStyle(scroller).overflowX : "",
            scrollerInside: scroller ? scroller.getBoundingClientRect().right <= said.getBoundingClientRect().right + 1 : false,
          };
        });
        if (shots) await page.screenshot({ path: join(shots, `align-${theme}-${width}.png`) });
      }
      ok(`${theme}: a table on the line under a sentence is drawn as a table — four rows, the sentence alone above it`,
        WIDTHS.every((width) => seen[width].table && seen[width].rows === 4 && seen[width].sentence === "여기까지 채웠습니다(4/6 단계)."),
        JSON.stringify(seen));
      ok(`${theme}: a wide table is its own scroll box and never pushes the page sideways`,
        WIDTHS.every((width) => seen[width].pageOverflow <= 0 && seen[width].scrollerInside && seen[width].scrollerOverflowX === "auto"),
        JSON.stringify(seen));
    }
    ok("the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    const ok = (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    };
    await testConversationAlign(browser, origin, ok);
    await testConversationColumn(browser, origin, ok);
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
