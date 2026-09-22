/* Gather the pages a browser-read measurement asks about (t-6041).
 *
 * Opens each address in Chromium and runs THE window's own block cutter —
 * `BROWSER_READ_BLOCKS_BODY` and its helpers, read out of
 * `crates/zerocode-shell/src/cmd/browser.rs` the way `ui/tests/browser-door.mjs`
 * reads them, at the landmark list of `crates/zerocode-core/src/jev.rs` — so
 * the blocks in the seed are the blocks the window would have cut, not a
 * copy of the rule. Nothing is judged here: the seed is what a read SAW, and
 * `cargo test … the_pages_that_were_gathered -- --ignored` asks the endpoint
 * about it through the seat's own question builder.
 *
 *   NODE_PATH=<a node_modules with playwright> \
 *   node tools/browser-read-replay/gather.mjs --out /tmp/browser-read/seed.json [--urls urls.txt]
 *
 * `--urls` is one address per line (`#` comments allowed); without it the ten
 * public pages named in README.md are read. A page that does not load in its
 * budget is written with `error` and no blocks, never silently dropped. */

import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const DOOR = await readFile(resolve(ROOT, "crates/zerocode-shell/src/cmd/browser.rs"), "utf8");
const JEV = await readFile(resolve(ROOT, "crates/zerocode-core/src/jev.rs"), "utf8");
const HELPERS = DOOR.match(/const BROWSER_AUTOMATION_HELPERS: &str = r#"([\s\S]*?)"#;/)[1];
const READ_BLOCKS_BODY = DOOR.match(/const BROWSER_READ_BLOCKS_BODY: &str = r#"([\s\S]*?)"#;/)[1];
const BLOCK_ROOTS = [...JEV.match(/pub const BROWSER_READ_BLOCK_ROOTS: \[&str; \d+\] = \[([\s\S]*?)\];/)[1]
  .matchAll(/"([^"]+)"/g)].map((hit) => hit[1]);
const named = (name) => Number(JEV.match(new RegExp(`const ${name}: usize = ([0-9_]+);`))?.[1]?.replace(/_/g, ""));
/* The read's own caps, read from the door and the table. */
const TEXT_CAP = Number(DOOR.match(/const BROWSER_READ_CAP: usize = ([0-9_]+);/)[1].replace(/_/g, ""));
const TITLE_CAP = Number(DOOR.match(/const BROWSER_TITLE_CAP: usize = ([0-9_]+);/)[1].replace(/_/g, ""));
const URL_CAP = Number(DOOR.match(/const BROWSER_URL_CAP: usize = ([0-9_]+);/)[1].replace(/_/g, ""));
const ANSWER_CAP = Number(DOOR.match(/const BROWSER_CALLBACK_CAP: usize = ([0-9_]+);/)[1].replace(/_/g, ""));

const script = (request, body) =>
  `(() => {\n${HELPERS}\nconst request = ${JSON.stringify(request)};\ntry {\n${body}\n} catch (_) { return zcFail("evaluation_failed"); }\n})()`;

/* A page that answered with a bot wall instead of itself: an HTTP status
 * past 399, or a title one of the common walls puts up. Flagged and still
 * gathered, so the measurement can show its totals with and without them —
 * a wall's own words are the page, and folding them says nothing (t-6155
 * F6). */
const WALL_TITLES = [/just a moment/i, /attention required/i, /access denied/i, /are you (a )?human/i, /verify you are/i, /security check/i];
const isWall = (status, title) => status >= 400 || WALL_TITLES.some((wall) => wall.test(title ?? ""));

const args = process.argv.slice(2);
const flag = (name) => { const at = args.indexOf(name); return at >= 0 ? args[at + 1] : null; };
const out = flag("--out");
if (!out) { console.error("gather: --out <seed.json> is required"); process.exit(2); }
const urlsFile = flag("--urls");
const urls = urlsFile
  ? (await readFile(urlsFile, "utf8")).split("\n").map((line) => line.trim()).filter((line) => line && !line.startsWith("#"))
  : (await readFile(resolve(dirname(fileURLToPath(import.meta.url)), "pages.txt"), "utf8"))
      .split("\n").map((line) => line.trim()).filter((line) => line && !line.startsWith("#"));

// The same Chromium the window's browser harnesses launch, found the way
// they find it (a checkout's node_modules, else the global npm root).
const { chromium } = await import(resolve(ROOT, "ui/tests/playwright-chromium.mjs"));
const browser = await chromium.launch();
const pages = [];
for (const url of urls) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const began = Date.now();
  try {
    const response = await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30000 });
    const status = response?.status() ?? 0;
    await page.waitForTimeout(1500);
    const answer = JSON.parse(await page.evaluate(script({
      blockRoots: BLOCK_ROOTS.join(","), textCap: TEXT_CAP, titleCap: TITLE_CAP, urlCap: URL_CAP, answerCap: ANSWER_CAP,
    }, READ_BLOCKS_BODY)));
    if (!answer.ok) throw new Error(`the cutter refused: ${answer.code}`);
    const wall = isWall(status, answer.value.title);
    pages.push({ url, loadMs: Date.now() - began, status, wall, ...answer.value });
    console.error(`gathered ${url}: ${answer.value.blocks.length} blocks, ${answer.value.text.length} chars, status ${status}${wall ? " (bot wall)" : ""}`);
  } catch (error) {
    pages.push({ url, loadMs: Date.now() - began, error: String(error?.message ?? error), title: "", text: "", blocks: [] });
    console.error(`failed ${url}: ${error?.message ?? error}`);
  } finally {
    await page.close();
  }
}
await browser.close();
await mkdir(dirname(out), { recursive: true });
await writeFile(out, JSON.stringify({
  gatheredAt: new Date().toISOString(),
  blockRoots: BLOCK_ROOTS,
  caps: { text: TEXT_CAP, title: TITLE_CAP, blockCap: named("BROWSER_READ_BLOCK_CAP"), shardTarget: named("BROWSER_READ_SHARD_TARGET") },
  pages,
}, null, 1));
console.log(`${pages.filter((page) => !page.error).length}/${pages.length} pages → ${out}`);
