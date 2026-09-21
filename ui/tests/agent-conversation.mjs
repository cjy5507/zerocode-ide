import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testAgentConversation(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async () => {
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      paneAgents.set(term, "claude");
      hookStates.set(term, "working");
      window.__CHAT_OWNER__ = owner;
      window.__ANSWER__.read_image_file = () => ({ mime: "image/svg+xml", bytes: 120,
        data: btoa('<svg xmlns="http://www.w3.org/2000/svg" width="180" height="60"><rect width="180" height="60" fill="seagreen"/><text x="12" y="36" fill="white">Verified</text></svg>') });
      window.__ANSWER__.subagent_log = () => ({ found: true, next: 1, turns: [
        { role: "user", text: "실행 결과를 확인하고 보고서를 만들어줘." },
        { role: "tool", text: "Bash · node first.js", tool: { call_id: "a", name: "Bash", input: "node first.js\nprintf done", is_error: false } },
        { role: "tool", text: "Bash · node second.js", tool: { call_id: "b", name: "Bash", input: "node second.js", is_error: false } },
      ] });
      await openHelperPage({ term, tab: owner, worktree: owner.worktree, agent: "claude" }, { id: "conversation-test", name: "검증 도우미", state: "working" });
      window.__CHAT_TAB__ = activeHelperPage();
      window.__ANSWER__.subagent_log = () => ({ found: true, next: 1, turns: [] });
    });
    await page.waitForSelector(".helper-turn.is-tool");
    await page.locator(".helper-turn.is-tool .helper-tool-more > summary").first().click();
    const paired = await page.evaluate(() => {
      const tab = window.__CHAT_TAB__;
      const original = document.querySelector(".helper-turn.is-tool");
      holdHelperTurns(tab.worker.helper, [
        { role: "tool_result", text: "SECOND OUTPUT", tool: { call_id: "b", is_error: false } },
        { role: "tool_result", text: "FIRST OUTPUT\nexit 1", tool: { call_id: "a", is_error: true } },
      ]);
      paintWorkerView(tab);
      const rows = [...document.querySelectorAll(".helper-turn.is-tool")];
      return { count: rows.length, same: rows[0] === original, open: rows[0].querySelector(".helper-tool-more").open,
        command: rows[0].querySelector(".helper-tool-input").textContent,
        results: rows.map((row) => row.querySelector(".helper-tool-result").textContent),
        output: rows[0].querySelector(".helper-tool-output").textContent,
        secondBare: rows[1].querySelector(".helper-tool-more") === null,
        failed: rows.map((row) => row.classList.contains("is-failed")),
        done: rows.map((row) => row.classList.contains("is-done")) };
    });
    ok("interleaved tool results join their call IDs: each dresses its own row — first line under the call, the rest behind the fold that stayed open, the failed one in the halt state",
      paired.count === 2 && paired.same && paired.open && paired.command.includes("printf done") &&
      paired.results.join("|") === "FIRST OUTPUT|SECOND OUTPUT" && paired.output === "FIRST OUTPUT\nexit 1" &&
      paired.secondBare && paired.failed.join() === "true,false" && paired.done.join() === "false,true", JSON.stringify(paired));

    const hidden = await page.evaluate(async () => {
      const tab = window.__CHAT_TAB__;
      const helper = tab.worker.helper;
      let reads = 0;
      let finish;
      window.__ANSWER__.subagent_log = () => {
        reads++;
        return new Promise((resolve) => { finish = resolve; });
      };
      const first = pollHelperPages();
      await pollHelperPages();
      setActiveTab(window.__CHAT_OWNER__.id);
      finish({ found: true, next: 99, turns: [{ role: "assistant", text: "숨은 동안 도착한 결과입니다." }] });
      await first;
      window.__ANSWER__.subagent_log = () => ({ found: true, next: 99, turns: [] });
      setActiveTab(tab.id);
      // The answer landed whole and stands whole on the next paint. What
      // this asks is that it was kept and drawn after the tab came back.
      await new Promise((settle) => setTimeout(settle, 200));
      return { reads, next: helper.next, kept: helper.turns.at(-1).text,
        painted: document.querySelector(".helper-turns").textContent.includes("숨은 동안 도착한 결과") };
    });
    ok("one transcript read stays in flight and an answer arriving after a tab switch is retained",
      hidden.reads === 1 && hidden.next === 99 && hidden.kept.includes("숨은 동안") && hidden.painted, JSON.stringify(hidden));

    await page.evaluate(() => {
      const tab = window.__CHAT_TAB__;
      holdHelperTurns(tab.worker.helper, [{ role: "assistant", text:
        "| 항목 | 결과 |\n| --- | --- |\n| 테스트 | 통과 |\n\n![검증 이미지](chart.png)\n\n<img src=x onerror=window.__CHAT_INJECTED__=true>" }]);
      paintWorkerView(tab);
    });
    await page.waitForSelector('.helper-said img.md-image[src^="data:"]');
    ok("assistant messages render tables and local images without interpreting raw HTML",
      await page.locator(".helper-said table").count() === 1 &&
      await page.evaluate(() => window.__CHAT_INJECTED__ !== true && !document.querySelector(".helper-said [onerror]")));

    await page.fill(".worker-composer-box", "기존 초안");
    const draftBefore = await page.evaluate(() => window.__COUNTS__.term_paste ?? 0);
    // The window's own /artifact, reached through the `/` button's palette,
    // puts its draft under the words already written.
    await page.click(".worker-composer-slash");
    await page.click('.composer-slash-row[data-kind="window"]');
    const draft = await page.locator(".worker-composer-box").inputValue();
    ok("new artifact creates a reviewable draft in this conversation without sending or erasing text",
      draft.startsWith("기존 초안") && draft.includes("HTML") &&
      await page.evaluate(() => window.__COUNTS__.term_paste ?? 0) === draftBefore);

    // The parent is between turns for the sends below: words typed while it
    // works wait in the window's queue (`composer-queue`); these pins ride the
    // road a send takes, not the wait.
    await page.evaluate(() => {
      const term = window.__CHAT_TAB__.worker.term;
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
        handler({ payload: { term, state: "idle", agent: "claude" } });
      }
    });
    const failure = await page.evaluate(async () => {
      let writes = 0;
      let rejectWrite;
      window.__ANSWER__.term_paste = () => {
        writes++;
        return new Promise((resolve, reject) => { rejectWrite = reject; });
      };
      const form = document.querySelector(".worker-composer");
      const box = form.querySelector("textarea");
      const draft = box.value;
      form.requestSubmit();
      form.requestSubmit();
      const retained = box.value === draft;
      rejectWrite(new Error("fixture paste failed"));
      await Promise.resolve(); await Promise.resolve();
      return { writes, retained, value: box.value, draft };
    });
    await page.waitForFunction(() => window.__CHAT_TAB__.worker.sending === false);
    ok("duplicate submission is blocked while sending and a failed paste preserves the draft",
      failure.writes === 1 && failure.retained && failure.value === failure.draft, JSON.stringify(failure));
    await page.evaluate(() => {
      window.__PARTIAL_PASTES__ = 0;
      window.__ANSWER__.term_paste = () => { window.__PARTIAL_PASTES__++; return null; };
      window.__ANSWER__.term_key = () => { throw new Error("fixture key reply lost"); };
      document.querySelector(".worker-composer").requestSubmit();
    });
    await page.waitForFunction(() => window.__CHAT_TAB__.worker.sendUncertain === true);
    const partial = await page.evaluate(() => {
      document.querySelector(".worker-composer").requestSubmit();
      return { pastes: window.__PARTIAL_PASTES__, disabled: document.querySelector(".worker-composer-send").disabled,
        retained: document.querySelector(".worker-composer-box").value !== "" };
    });
    ok("an uncertain Enter reply cannot automatically paste the same draft twice",
      partial.pastes === 1 && partial.disabled && partial.retained, JSON.stringify(partial));
    await page.click(".worker-delivery button");
    await page.evaluate(() => { delete window.__ANSWER__.term_key; });
    const rebuilt = await page.evaluate(() => {
      let finish;
      window.__ANSWER__.term_paste = () => new Promise((done) => { finish = done; });
      const tab = window.__CHAT_TAB__;
      const host = docHost(tab.pane, "worker");
      const previous = host.querySelector(".worker-composer-box");
      host.querySelector(".worker-composer").requestSubmit();
      host.__helperPage = null;
      paintWorkerView(tab);
      const current = host.querySelector(".worker-composer-box");
      const state = { rebuilt: previous !== current, readOnly: current.readOnly,
        disabled: host.querySelector(".worker-composer-send").disabled };
      finish(null);
      return state;
    });
    await page.waitForFunction(() => window.__CHAT_TAB__.worker.sending === false);
    const delivered = await page.locator(".worker-composer-box").inputValue();
    ok("a composer rebuilt during delivery remains locked and clears only after submission succeeds",
      rebuilt.rebuilt && rebuilt.readOnly && rebuilt.disabled && delivered === "", JSON.stringify({ ...rebuilt, delivered }));

    await mkdir("output/playwright/agent-conversation", { recursive: true });
    await page.evaluate(() => {
      delete window.__ANSWER__.term_paste;
      const tab = window.__CHAT_TAB__;
      tab.worker.status = "done";
      tab.worker.endedAt = Date.now();
      paintWorkerView(tab);
    });
    for (const width of [1280, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => {
        const host = docHost(window.__CHAT_TAB__.pane, "worker");
        Object.assign(host.style, { position: "fixed", inset: "0", zIndex: "100", width: "100vw", height: "100vh" });
      });
      const fits = await page.evaluate(() => {
        const host = docHost(window.__CHAT_TAB__.pane, "worker");
        return host.scrollWidth <= host.clientWidth + 1;
      });
      ok(`conversation fits ${width}px`, fits);
      await page.screenshot({ path: `output/playwright/agent-conversation/${width}.png` });
    }
    ok("the conversation raised no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testAgentConversation(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally { await browser.close(); files.close(); }
  if (failed) process.exitCode = 1;
}
