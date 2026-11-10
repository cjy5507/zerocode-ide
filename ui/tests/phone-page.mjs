// The phone web app (ui/phone/), served as the phone link serves it — the
// app's own files at the root and the window's tokens beside them — with a
// fixture behind its four API paths. It checks what a person sees before and
// after pairing: a card the script hides is gone from the page, not merely
// marked hidden (2026-11-11 WebKit audit: `display: grid` on the cards beat
// the attribute, and a paired phone still showed the pairing card).
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "./window-boot.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ORIGIN = "http://phone.test";
const FILES = {
  "/": ["phone/index.html", "text/html"],
  "/phone.css": ["phone/phone.css", "text/css"],
  "/phone.js": ["phone/phone.js", "text/javascript"],
  "/phone-i18n.js": ["phone/phone-i18n.js", "text/javascript"],
  "/tokens.css": ["tokens.css", "text/css"],
};
const PANES = {
  now: 1_800_000_000_000,
  panes: [
    { name: "zerocode", agent: "claude", task: "t-1", group: "attention", waiting: "ask", sinceMs: 1_799_999_940_000 },
    { name: "site", agent: "codex", group: "working", sinceMs: 1_799_999_000_000 },
  ],
};

async function openPhone(browser, { token }) {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const page = await context.newPage();
  const faults = [];
  page.on("pageerror", (error) => faults.push(String(error)));
  if (token) await page.addInitScript((value) => localStorage.setItem("zerocode.phone.token", value), token);
  await page.route(`${ORIGIN}/**`, async (route) => {
    const { pathname } = new URL(route.request().url());
    const file = FILES[pathname];
    if (file) {
      const body = await readFile(join(UI, file[0]), "utf8");
      return route.fulfill({ status: 200, contentType: `${file[1]}; charset=utf-8`, body });
    }
    const authorized = route.request().headers().authorization === `Bearer ${token}`;
    const json = (status, value) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(value) });
    if (pathname === "/api/status") return authorized ? json(200, { version: "test", name: "desk" }) : json(401, {});
    if (pathname === "/api/panes") return authorized ? json(200, PANES) : json(401, {});
    return json(404, {});
  });
  await page.goto(`${ORIGIN}/`);
  return { context, page, faults };
}

const shown = (page) => page.evaluate(() => Object.fromEntries(
  ["phone-pair", "phone-install", "phone-notify", "phone-list"].map((id) => [id, getComputedStyle(document.getElementById(id)).display !== "none"]),
));

export async function testPhonePage(browser, ok) {
  {
    const { context, page, faults } = await openPhone(browser, { token: null });
    try {
      await page.waitForFunction(() => !document.getElementById("phone-pair").hidden);
      const seen = await shown(page);
      ok("an unpaired phone sees the pairing card alone — the install, notification and list cards are not drawn",
        seen["phone-pair"] && !seen["phone-install"] && !seen["phone-notify"] && !seen["phone-list"], JSON.stringify(seen));
      ok("the unpaired phone page raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await context.close(); }
  }
  {
    const { context, page, faults } = await openPhone(browser, { token: "fixture-token" });
    try {
      await page.waitForSelector("#phone-groups .phone-group");
      const seen = await shown(page);
      ok("a paired phone sees its list and not the pairing card",
        !seen["phone-pair"] && seen["phone-list"] && !seen["phone-install"], JSON.stringify(seen));
      const groups = await page.evaluate(() => {
        const sections = [...document.querySelectorAll("#phone-groups .phone-group")];
        const [first, second] = sections.map((node) => node.getBoundingClientRect());
        return { count: sections.length, gap: second ? second.top - first.bottom : null };
      });
      ok("the list's groups stand apart, not on each other's edge", groups.count === 2 && groups.gap > 0, JSON.stringify(groups));
      ok("the paired phone page raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await context.close(); }
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testPhonePage(browser, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); }
  if (failures) process.exitCode = 1;
}
