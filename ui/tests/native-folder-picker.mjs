import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { mkdir } from "node:fs/promises";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

export async function testNativeFolderPicker(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      window.__PICK_CALLS__ = [];
      window.__ANSWER__.choose_paths = (args) => {
        window.__PICK_CALLS__.push(args);
        return new Promise((done) => { window.__PICK_FINISH__ = done; });
      };
      window.__PICK_PROMISE__ = openPathBrowser({ start: "/Users/person/Projects" });
    });
    await page.waitForFunction(() => typeof window.__PICK_FINISH__ === "function");
    const pending = await page.evaluate(() => {
      let clicked = false;
      const button = document.createElement("button");
      button.onclick = () => { clicked = true; };
      document.body.append(button); button.click(); button.remove();
      const same = openPathBrowser({ start: "/second" }) === window.__PICK_PROMISE__;
      return { hidden: el("pb-scrim").hidden, clicked, same, calls: window.__PICK_CALLS__, cancel: !!document.querySelector(".folder-panel-cancel") };
    });
    ok("native folder selection opens directly and keeps the app responsive with one pending request",
      pending.hidden && pending.clicked && pending.same && pending.calls.length === 1 && pending.cancel &&
      pending.calls[0].kind === "folder" && pending.calls[0].start === "/Users/person/Projects", JSON.stringify(pending));
    const picked = await page.evaluate(async () => {
      window.__PICK_FINISH__(["/Users/person/My project"]);
      return await window.__PICK_PROMISE__;
    });
    ok("the native macOS-style path is preserved", picked[0] === "/Users/person/My project");

    await page.evaluate(() => { window.__PICK_PROMISE__ = openPathBrowser({ start: "C:\\Users\\person" }); });
    await page.waitForFunction(() => window.__PICK_CALLS__.length === 2);
    const cancelled = await page.evaluate(async () => {
      window.__PICK_FINISH__([]);
      const result = await window.__PICK_PROMISE__;
      return { result, closed: pathBrowser === null, hidden: el("pb-scrim").hidden };
    });
    ok("dismissing the system dialog cancels without opening a second browser", cancelled.result.length === 0 && cancelled.closed && cancelled.hidden);

    await page.evaluate(() => {
      window.__ANSWER__.cancel_folder_panel = () => true;
      window.__PICK_PROMISE__ = openPathBrowser();
    });
    await page.waitForFunction(() => window.__PICK_CALLS__.length === 3);
    const cancellation = await page.evaluate(async () => {
      closePathBrowser([]);
      const same = openPathBrowser() === window.__PICK_PROMISE__;
      window.__PICK_FINISH__(["/too-late"]);
      return { same, result: await window.__PICK_PROMISE__, count: window.__PICK_CALLS__.length };
    });
    ok("cancelled requests retain the slot until their reply and discard a late selection", cancellation.same && cancellation.count === 3 && cancellation.result.length === 0);

    await page.evaluate(() => {
      window.__ANSWER__.choose_paths = () => Promise.reject(new Error("fixture: helper unavailable"));
      window.__PICK_PROMISE__ = openPathBrowser({ start: null });
    });
    await page.waitForFunction(() => !el("pb-scrim").hidden && el("pb-error").textContent.includes("helper unavailable"));
    ok("a failed native launch offers the in-app fallback", await page.locator("#pb-scrim").isVisible());
    const recovered = await page.evaluate(async () => {
      window.__ANSWER__.choose_paths = () => ["C:\\Users\\person\\Projects"];
      await askSystemPanel();
      return await window.__PICK_PROMISE__;
    });
    ok("retry returns the Windows-style path without rewriting it", recovered[0] === "C:\\Users\\person\\Projects");

    const localChoice = await page.evaluate(async () => {
      const answer = openPathBrowser({ mode: "folder", system: false });
      let finish;
      window.__ANSWER__.choose_paths = () => new Promise((done) => { finish = done; });
      const native = askSystemPanel();
      await Promise.resolve();
      closePathBrowser(["/chosen-in-fallback"]);
      finish(["/late-native-answer"]);
      await native;
      return await answer;
    });
    ok("an in-app choice survives cancellation of the still-open native helper", localChoice[0] === "/chosen-in-fallback");

    await page.evaluate(() => {
      setWorktreeForm(true);
      setWorktreeTab("text");
      el("wt-name-text").value = "folder-test";
      paintWorktreeName();
      window.__DIRECTORY_DEFAULT__ = workspaceCreationPrefs.directory;
      window.__ANSWER__.choose_paths = () => new Promise((done) => { window.__DIRECTORY_FINISH__ = done; });
      el("wt-directory-browse").click();
    });
    await page.waitForFunction(() => typeof window.__DIRECTORY_FINISH__ === "function");
    ok("creation waits for the pending destination choice", await page.locator("#wt-create").isDisabled());
    await page.evaluate(() => window.__DIRECTORY_FINISH__("/tmp/chosen-parent".split("\n")));
    await page.waitForFunction(() => el("wt-directory").value === "/tmp/chosen-parent" && !worktreeDirectoryPicking);
    const job = await page.evaluate(async () => {
      askAboutSetup = async () => false;
      askRepoTrust = async () => true;
      const original = startWorktreeCreation;
      let job;
      startWorktreeCreation = (value) => { job = value; };
      await createWorktreeFromSpec();
      startWorktreeCreation = original;
      window.__ANSWER__.create_worktree = (args) => {
        window.__CREATED_AT__ = args;
        throw new Error("fixture: stop after inspecting create arguments");
      };
      pendingCreations.set("folder-fixture", { id: "folder-fixture", job, phase: "preparing", error: null });
      await runWorktreeCreation("folder-fixture");
      pendingCreations.delete("folder-fixture");
      setWorktreeForm(true);
      return { directory: job.directory, sent: window.__CREATED_AT__?.directory,
        defaultUnchanged: workspaceCreationPrefs.directory === window.__DIRECTORY_DEFAULT__,
        next: el("wt-directory").value, default: window.__DIRECTORY_DEFAULT__ };
    });
    ok("the chosen parent travels with this creation and does not overwrite defaults", job.directory === "/tmp/chosen-parent" && job.sent === job.directory && job.defaultUnchanged && job.next === job.default, JSON.stringify(job));
    await mkdir("output/playwright/native-folder-picker", { recursive: true });
    await page.setViewportSize({ width: 1000, height: 1000 });
    await page.screenshot({ path: "output/playwright/native-folder-picker/worktree-folder.png" });
    const stale = await page.evaluate(async () => {
      let finish;
      window.__ANSWER__.choose_paths = () => new Promise((done) => { finish = done; });
      el("wt-directory-browse").click();
      await Promise.resolve();
      const pending = pathBrowser.promise;
      setWorktreeForm(false);
      setWorktreeForm(true);
      finish(["/stale-parent"]);
      await pending;
      await Promise.resolve();
      return el("wt-directory").value;
    });
    ok("a closed form's late picker reply cannot overwrite a reopened form", stale === job.default, stale);
    ok("folder picker flows raise no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try { await testNativeFolderPicker(browser, origin, (name, pass, details = "") => {
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${pass ? "" : `\n${details}`}`);
    if (!pass) failures++;
  }); } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
