/* The inbox page, photographed (t-67483) — from the window harness with the page's
 * synthetic example (acme, example.com, /Users/dev).
 *
 *   node ui/tests/inbox-shots.mjs <folder>
 *
 * 1440 px dark with the first question open, 768 px light (the preview folds under the
 * list), 480 px dark, and the 넘김 tab. A picture of the window is the only proof that a
 * letter is where the eye is. */
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium, createWindowServer, standBackend } from "./window-boot.mjs";
import { inboxFixture } from "./inbox.mjs";

const folder = resolve(process.argv[2] ?? "output/inbox");
await mkdir(folder, { recursive: true });
const { origin } = await createWindowServer();
const browser = await chromium.launch();

async function shoot(name, { width, height, theme, tab = "all", unreadOnly = false, pick = null }) {
  const page = await browser.newPage({ viewport: { width, height } });
  await standBackend(page);
  await page.addInitScript(() => {
    const base = window.__TAURI__.core.invoke;
    window.__INBOX__ = { snapshot: null };
    window.__TAURI__.core.invoke = (command, args) =>
      command === "board_inbox" ? Promise.resolve(window.__INBOX__.snapshot) : base(command, args);
  });
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  await page.evaluate((theme) => {
    document.documentElement.dataset.theme = theme;
  }, theme);
  await page.evaluate(async (snapshot) => {
    window.__INBOX__.snapshot = snapshot;
    await refreshInbox();
    document.getElementById("nav-inbox").click();
  }, inboxFixture());
  await page.waitForSelector(".inbox-view:not([hidden]) .inbox-row");
  await page.evaluate(({ tab, unreadOnly, pick }) => {
    const view = document.querySelector(".inbox-view:not([hidden])");
    view.querySelector(`.inbox-tab[data-inbox-tab="${tab}"]`).click();
    if (pick) view.querySelector(`.inbox-row[data-id="${pick}"]`).click();
    if (unreadOnly) view.querySelector(".inbox-unread-switch").click();
  }, { tab, unreadOnly, pick });
  await page.screenshot({ path: `${folder}/${name}.png` });
  await page.close();
}

await shoot("inbox-1440-dark", { width: 1440, height: 900, theme: "dark" });
await shoot("inbox-768-light", { width: 768, height: 900, theme: "light" });
await shoot("inbox-480-dark", { width: 480, height: 900, theme: "dark" });
await shoot("inbox-handoff-1440-dark", { width: 1440, height: 900, theme: "dark", tab: "handoff", pick: "handoff:run-a/t-2033" });
await shoot("inbox-blocked-1440-light", { width: 1440, height: 900, theme: "light", tab: "blocked", pick: "pane:term:5@1" });
await browser.close();
console.log(`photographed into ${folder}`);
process.exit(0);
