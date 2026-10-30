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
    await testConversationAlign(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
