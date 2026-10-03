import { readFileSync } from "node:fs";
import { openWindowTestPage } from "./window-boot.mjs";

const fixtures = JSON.parse(readFileSync(new URL("../../fixtures/agent-support.json", import.meta.url), "utf8"));

export async function testAgentSupport(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const rendered = await page.evaluate((rows) => rows.map((row) => {
      const element = agentRow({ ...row, name: row.id, installed: false, takes_a_paste: true });
      return { text: element.querySelector(".agent-support")?.textContent,
        hint: element.querySelector(".agent-support")?.dataset.tip,
        commands: element.querySelectorAll(".agent-row-cmd").length };
    }), fixtures);
    ok("the Rust support fixture renders all four capabilities without claiming live status",
      rendered.every((row) => row.text.includes("실행 지원") && row.text.includes("구조화 관찰 지원")
        && row.hint.includes("현재 실행") && row.hint.includes("작업 완료")));
    ok("hook context and direct protocol control remain separate capabilities",
      rendered[0].text.includes("추가 문맥 지원") && rendered[1].text.includes("추가 문맥 미지원")
        && rendered[1].text.includes("직접 제어 지원"));
    ok("support does not duplicate the editable command row", rendered.every((row) => row.commands === 1));
    ok("an older backend without the contract does not invent support", await page.evaluate(() =>
      agentRow({ id: "unknown", name: "Unknown", installed: false, takes_a_paste: true })
        .querySelector(".agent-support") === null));
    ok("support rendering raises no script errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
