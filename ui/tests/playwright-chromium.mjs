/* Playwright's Chromium for the browser tests that run without the window
 * harness (`browser-ring.mjs`, `browser-door.mjs`): the project's own install
 * first, the global npm root second, and a clear FAIL when neither has it —
 * the gate must not pass by skipping. One copy of the lookup, so the two
 * files cannot drift in how they find the engine. */

import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { join } from "node:path";

async function globalNpmRoot() {
  return new Promise((done) => {
    const npm = spawn("npm", ["root", "-g"], { stdio: ["ignore", "pipe", "ignore"] });
    let out = "";
    npm.stdout.on("data", (chunk) => (out += chunk));
    npm.on("close", () => done(out.trim()));
    npm.on("error", () => done(""));
  });
}

async function playwrightChromium() {
  try {
    const require = createRequire(import.meta.url);
    let entry;
    try {
      entry = require.resolve("playwright");
    } catch {
      const root = await globalNpmRoot();
      if (!root) throw new Error("no global npm root");
      entry = require.resolve(join(root, "playwright"));
    }
    return require(entry).chromium;
  } catch {
    console.error("FAIL  Playwright is unavailable — run `npm ci && npx playwright install chromium`");
    process.exit(1);
  }
}

export const chromium = await playwrightChromium();
