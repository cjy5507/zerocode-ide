import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testArtifactPreferences(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async () => {
      window.__PREFERENCES__ = [];
      window.__PREFERENCE_SAVES__ = [];
      window.__PREFERENCE_REVOKES__ = [];
      window.__FEEDBACK_RECORDS__ = [];
      const snapshot = () => ({ projectKey: "a".repeat(64), entries: window.__PREFERENCES__,
        supportedAgent: "zo", maxTextBytes: 1024, maxPerScope: 8 });
      window.__ANSWER__.artifact_preferences = snapshot;
      window.__ANSWER__.artifact_feedback_record = ({ feedback }) => {
        window.__FEEDBACK_RECORDS__.push(feedback);
        return { count: 1, version: feedback.version };
      };
      window.__ANSWER__.artifact_preference_save = (request) => {
        window.__PREFERENCE_SAVES__.push(request);
        window.__PREFERENCES__ = [{ id: "b".repeat(64), text: request.text,
          scope: request.scope === "personal" ? { kind: "personal" } : { kind: "project", key: request.expectedProject },
          origin: { artifactId: request.feedback.id, version: request.feedback.version, sha256: "c".repeat(64),
            feedbackKey: "d".repeat(64), followedLink: false }, createdMs: 10, updatedMs: 10 }];
        return snapshot();
      };
      window.__ANSWER__.artifact_preference_revoke = ({ id }) => {
        window.__PREFERENCE_REVOKES__.push(id);
        window.__PREFERENCES__ = window.__PREFERENCES__.filter((entry) => entry.id !== id);
        return snapshot();
      };
      await recordArtifactFeedback({ facts: { id: "page-example" }, version: 2, pageUrl: null },
        [{ selector: ".heading", comment: "Make this heading shorter" }], { term: 19, agent: "codex" });
    });
    ok("delivered feedback remains one-off until the person chooses to remember it",
      await page.evaluate(() => window.__FEEDBACK_RECORDS__.length === 1 && window.__PREFERENCE_SAVES__.length === 0));
    await page.getByRole("button", { name: "선호로 기억", exact: true }).click();
    const panel = page.getByRole("dialog", { name: "선호 관리" });
    await panel.waitFor({ state: "visible" });
    const input = panel.locator('input[name="preference"]');
    const save = panel.getByRole("button", { name: "선호로 저장", exact: true });
    ok("persistent wording is a separate explicit choice, with project scope as the safe default",
      await input.inputValue() === "" && await save.isDisabled()
        && await panel.locator('select[name="scope"]').inputValue() === "project");
    ok("the preference editor covers native browser panes and states its supported consumer",
      await page.evaluate(() => browserCovered()) && (await panel.textContent()).includes("zo의 다음 요청"));
    await input.fill("Prefer concise headings");
    await save.click();
    await panel.locator("article").waitFor();
    const saved = await page.evaluate(() => window.__PREFERENCE_SAVES__[0]);
    ok("saving binds the explicit text to the delivered feedback, version and selected project",
      saved.text === "Prefer concise headings" && saved.expectedProject === "a".repeat(64)
        && saved.feedback.version === 2 && saved.feedback.items[0].comment === "Make this heading shorter"
        && saved.feedback.recipient.agent === "codex", JSON.stringify(saved));
    await panel.getByRole("button", { name: "철회", exact: true }).click();
    await page.waitForFunction(() => window.__PREFERENCE_REVOKES__.length === 1);
    ok("revocation removes the active preference without deleting the original feedback",
      await panel.locator("article").count() === 0
        && await page.evaluate(() => window.__FEEDBACK_RECORDS__.length === 1));
    await input.fill("<img src=x onerror=alert(1)>");
    await panel.locator('select[name="scope"]').selectOption("personal");
    await save.click();
    await panel.locator("article").waitFor();
    ok("personal scope is explicit and preference text never becomes executable markup",
      await page.evaluate(() => window.__PREFERENCE_SAVES__.at(-1).scope === "personal")
        && await panel.locator("article img").count() === 0
        && (await panel.locator("article").textContent()).includes("<img src=x onerror=alert(1)>"));
    await page.evaluate(() => window.__FAIL__.add("artifact_preference_save"));
    await input.fill("Keep this unsaved wording");
    await save.click();
    await page.waitForFunction(() => document.querySelector(".artifact-preferences-status")?.textContent.includes("refused"));
    ok("a failed save keeps the draft and does not resend annotations or claim success",
      await input.inputValue() === "Keep this unsaved wording"
        && await page.evaluate(() => window.__FEEDBACK_RECORDS__.length === 1 && window.__PREFERENCE_SAVES__.length === 2));
    await page.evaluate(() => { activeWorktreePath = "/repos/another"; });
    await panel.getByRole("button", { name: "철회", exact: true }).click();
    ok("a changed project refuses an old editor action before IPC",
      await page.evaluate(() => window.__PREFERENCE_REVOKES__.length === 1)
        && (await panel.textContent()).includes("작업공간이 바뀌었습니다"));
    await page.setViewportSize({ width: 360, height: 800 });
    ok("the editor fits narrow windows", await panel.evaluate((node) => node.scrollWidth <= node.clientWidth));
    if (process.env.PREFERENCE_SCREENSHOT) await page.screenshot({ path: process.env.PREFERENCE_SCREENSHOT });
    await panel.getByRole("button", { name: "닫기", exact: true }).click();
    await page.evaluate(() => window.__FAIL__.add("artifact_preferences"));
    await page.evaluate(() => { void openArtifactPreferences(); });
    await page.waitForFunction(() => document.querySelector(".artifact-preferences-status")?.textContent.includes("refused"));
    ok("an unreadable store is not presented as an empty preference list",
      !(await panel.textContent()).includes("저장된 선호가 없습니다"));
    await panel.getByRole("button", { name: "닫기", exact: true }).click();
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.evaluate(async () => {
      window.__ANNOTATION_PASTES__ = [];
      window.__ANSWER__.open_browser_pane = () => "browser-preference-batch";
      window.__ANSWER__.browser_zoom = () => null;
      window.__ANSWER__.artifact_versions = () => [{ n: 1,
        path: "/tmp/zerocode-window-test/artifacts/pages/batch/v1/index.html", sha256: "e".repeat(64) }];
      window.__ANSWER__.term_paste = (request) => { window.__ANNOTATION_PASTES__.push(request); };
      const maker = await openTermTab({ placement: "tab" });
      paneAgents.set(maker, "codex");
      window.__ANSWER__.agent_terms = () => [[maker, "codex"]];
      await openArtifactPage({ id: "batch", kind: "page", title: "Batch", version: 1,
        created_ms: Date.now(), modified_ms: Date.now(), bytes: 10,
        path: "/tmp/zerocode-window-test/artifacts/pages/batch/index.html",
        origin: { pane: `term-${maker}`, agent: "codex" } });
      const tab = tabs.find((entry) => entry.kind === "browser" && entry.artifact?.id === "batch");
      const changed = { intent: "change", selector: ".heading", comment: "Original heading feedback", tag: "h1" };
      browserAnnotations.set(tab.label, [changed,
        { intent: "change", selector: ".footer", comment: "Original footer feedback", tag: "footer" }]);
      setActiveTab(tab.id);
      await deliverAnnotations(docHost(tab.pane, "browser"), tab);
      changed.comment = "Edited while choosing recipient";
      browserAnnotations.get(tab.label).push(
        { intent: "change", selector: ".body", comment: "Added while choosing recipient", tag: "main" });
    });
    await page.locator("#note-pop .note-pop-row").first().click();
    await page.waitForFunction(() => window.__FEEDBACK_RECORDS__.length === 2);
    const batch = await page.evaluate(() => {
      const tab = tabs.find((entry) => entry.kind === "browser" && entry.artifact?.id === "batch");
      return { recorded: window.__FEEDBACK_RECORDS__.at(-1), remaining: browserAnnotations.get(tab.label),
        pasted: window.__ANNOTATION_PASTES__.at(-1)?.text };
    });
    ok("feedback records the delivered snapshot, not annotations changed during recipient selection",
      batch.recorded.items.map((entry) => entry.comment).join("|") === "Original heading feedback|Original footer feedback"
        && batch.pasted?.includes("Original heading feedback") && !batch.pasted?.includes("Edited while choosing recipient"),
      JSON.stringify(batch));
    ok("only delivered annotations leave the page; later additions and edits remain",
      batch.remaining?.map((entry) => entry.comment).join("|") === "Edited while choosing recipient|Added while choosing recipient",
      JSON.stringify(batch.remaining));
    ok("preference controls raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testArtifactPreferences(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
