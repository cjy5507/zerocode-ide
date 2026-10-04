/* The long-form bench's fake desk (t-37883): the page in a headless Chromium,
 * answered the way the window answers `zerocode-computer` — so an agent's
 * real Computer road (zo's tool, its looks, its batches) runs against the
 * fixture without the person's pointer, keyboard or screen.
 *
 * The real pointer may not be used for a run this long, so this is the fake
 * pointer road: every verb an agent can send arrives here (argv joined by
 * 0x1f, as the shim sends it), acts on the page through Playwright, and
 * answers the window's envelope. What the window's hand costs in time is
 * charged as the window's own code spends it — a `wait` sleeps its whole
 * duration, a look waits for the page to settle the eye's way, each acting
 * step costs the hand's measured step — and every one of those numbers is a
 * named constant below with where it was read.
 *
 * The person is fake too: a hand-off attaches a synthetic scan to the
 * upload field after the person's own time for it, and is logged for the
 * oracle (handoffs.jsonl).
 */

import { createServer } from "node:http";
import { mkdir, writeFile, appendFile } from "node:fs/promises";
import { join } from "node:path";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";

// The fake display: one browser window the size of the look the window
// sends (its first screenshot rung, 1280 px on the long edge), placed at the
// origin, one pixel per point.
export const SCREEN = { width: 1280, height: 800 };
// The app the page stands in, as a desktop look names it.
export const BROWSER_APP = "Google Chrome";
const BROWSER_PID = 4242;
const WINDOW_ID = 1;
// The hand's cost per acting step, measured on the installed window
// (wiki: a-click-by-reading-makes-a-batch-a-whole-plan, 2026-09-13: 204 ms a
// step for 20 steps, coordinate and accessibility roads alike).
const HAND_STEP_MS = 200;
// The eye's settle (crates/zerocode-core/src/computer_use.rs
// COMPUTER_SETTLE_MS, EYE_QUIET_MS, EYE_SETTLE_MAX_MS): a look after an act
// that repainted nothing waits 350 ms, one that did waits for 100 ms of
// quiet, and none waits past one second.
const SETTLE_NOTHING_MS = 350;
const SETTLE_QUIET_MS = 100;
const SETTLE_MAX_MS = 1000;
const SETTLE_POLL_MS = 25;
// `wait-for` polls the tree every 250 ms (COMPUTER_WAIT_FOR_POLL_MS).
const WAIT_FOR_POLL_MS = 250;
// A wheel line, in CSS pixels (Chromium's kPixelsPerLineStep).
const PIXELS_PER_LINE = 40;
// The batch's whole deadline (COMPUTER_USE_DEADLINE_SECONDS).
const BATCH_DEADLINE_MS = 65_000;
// The tree a look carries at most (the helper's TreeRenderer maxNodes).
const TREE_MAX_NODES = 1200;
// The synthetic scan the fake person attaches.
const SCAN = { name: "passport-scan-sample.png", mimeType: "image/png",
  buffer: Buffer.from("89504e470d0a1a0a0000000d4948445200000001000000010806000000" +
    "1f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082", "hex") };
// The xdotool-style key names an agent sends, in Playwright's words.
const KEY_NAMES = {
  return: "Enter", enter: "Enter", tab: "Tab", escape: "Escape", esc: "Escape", space: " ",
  backspace: "Backspace", delete: "Delete", up: "ArrowUp", down: "ArrowDown", left: "ArrowLeft",
  right: "ArrowRight", page_down: "PageDown", page_up: "PageUp", pagedown: "PageDown", pageup: "PageUp",
  home: "Home", end: "End", cmd: "Meta", command: "Meta", super: "Meta", ctrl: "Control",
  control: "Control", alt: "Alt", option: "Alt", shift: "Shift",
};

const sleep = (ms) => new Promise((done) => setTimeout(done, Math.max(0, ms)));

class Refusal extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

/* The words after each `--flag`, and the bare `--switch`es, of one argv. */
function flagsOf(argv) {
  const flags = {};
  for (let at = 1; at < argv.length; at += 1) {
    const word = argv[at];
    if (!word.startsWith("--")) continue;
    const next = argv[at + 1];
    if (next === undefined || next.startsWith("--")) flags[word.slice(2)] = true;
    else { flags[word.slice(2)] = next; at += 1; }
  }
  return flags;
}

function chord(words) {
  return String(words).split("+").map((part) => KEY_NAMES[part.trim().toLowerCase()] ||
    (part.trim().length === 1 ? part.trim() : part.trim()[0].toUpperCase() + part.trim().slice(1))).join("+");
}

export async function startDesk({ url, out, personAttachMs, headless = true }) {
  await mkdir(join(out, "shots"), { recursive: true });
  const browser = await chromium.launch({ headless });
  const page = await browser.newPage({ viewport: SCREEN });
  // The page's own clock of its last change, for the eye's settle.
  await page.addInitScript(() => {
    window.__deskChangedAt = performance.now();
    new MutationObserver(() => { window.__deskChangedAt = performance.now(); })
      .observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    for (const kind of ["input", "change", "scroll"]) {
      addEventListener(kind, () => { window.__deskChangedAt = performance.now(); }, true);
    }
  });
  await page.goto(url);
  const cdp = await page.context().newCDPSession(page);
  const tally = { verbs: {}, steps: {}, waitMs: 0, handMs: 0, settleMs: 0, pngBytes: [], handoffs: 0, looks: 0 };
  const lastLook = new Map();
  let shots = 0;
  let pointer = { x: 0, y: 0 };
  let actedAt = null;

  const count = (table, verb) => { table[verb] = (table[verb] || 0) + 1; };

  /* What the hand costs beyond what Playwright took. */
  async function handStep(started) {
    const owed = HAND_STEP_MS - (Date.now() - started);
    if (owed > 0) await sleep(owed);
    tally.handMs += Math.max(HAND_STEP_MS, Date.now() - started);
    actedAt = Date.now();
  }

  /* The eye's settle after the last act: nothing repainted → 350 ms, else
   * 100 ms of quiet, never past one second. */
  async function settle() {
    if (actedAt === null) return { settled: true };
    const started = Date.now();
    for (;;) {
      const since = Date.now() - actedAt;
      const quietFor = await page.evaluate(() => performance.now() - window.__deskChangedAt);
      const changedAfterAct = quietFor < since;
      if (!changedAfterAct && since >= SETTLE_NOTHING_MS) break;
      if (changedAfterAct && quietFor >= SETTLE_QUIET_MS) break;
      if (since >= SETTLE_MAX_MS) { tally.settleMs += Date.now() - started; return { settled: false }; }
      await sleep(SETTLE_POLL_MS);
    }
    tally.settleMs += Date.now() - started;
    return { settled: true };
  }

  async function picture(clip) {
    const png = await page.screenshot({ type: "png", clip });
    shots += 1;
    const path = join(out, "shots", `${shots}.png`);
    await writeFile(path, png, { mode: 0o600 });
    tally.pngBytes.push(png.length);
    return { png, path };
  }

  /* The page's accessibility tree as the helper renders an app's: one line
   * per element, tabs for depth, its number, role, name and value. */
  async function tree() {
    const { nodes } = await cdp.send("Accessibility.getFullAXTree");
    const byId = new Map(nodes.map((node) => [node.nodeId, node]));
    const lines = [`App=${BROWSER_APP} (pid ${BROWSER_PID})`, `Window: "${await page.title()}"`];
    let index = 0;
    const walk = (node, depth) => {
      if (index >= TREE_MAX_NODES) return;
      const shown = !node.ignored && node.role && !["none", "generic", "InlineTextBox", "LineBreak"].includes(node.role.value);
      if (shown) {
        const name = node.name && node.name.value ? ` ${node.name.value}` : "";
        const value = node.value && node.value.value !== undefined && node.value.value !== "" ? `, Value: ${node.value.value}` : "";
        lines.push(`${"\t".repeat(depth)}${index} ${node.role.value}${name}${value}`);
        index += 1;
      }
      for (const child of node.childIds || []) {
        const next = byId.get(child);
        if (next) walk(next, shown ? depth + 1 : depth);
      }
    };
    walk(nodes[0], 0);
    return lines.join("\n");
  }

  async function look(flags, verb) {
    tally.looks += 1;
    const settled = flags.settle ? await settle() : { settled: true };
    const shot = await picture();
    const viewer = flags.viewer || "";
    const before = lastLook.get(viewer);
    let changed = null;
    if (flags.diff && before) changed = before.equals(shot.png) ? [] : [{ x: 0, y: 0, width: SCREEN.width, height: SCREEN.height }];
    lastLook.set(viewer, shot.png);
    const answer = {
      verb,
      screenshot: { path: shot.path, width: SCREEN.width, height: SCREEN.height, scale: 1 },
      origin: { x: 0, y: 0 }, scale: 1, cursor: pointer,
      tree: flags.app ? { window: await page.title(), text: await tree() } : null,
      text: null, changed,
    };
    if (flags.settle) answer.settle = settled;
    return answer;
  }

  /* An element by what it reads, the way the helper's query matches: a
   * case-blind piece of its accessible name. One match presses; more are
   * ambiguous; none is not found. */
  async function byReading(flags) {
    const words = String(flags.label || flags.text || "").toLowerCase();
    const role = flags.role ? String(flags.role).toLowerCase() : null;
    const found = await page.evaluate(({ words, role }) => {
      const nameOf = (element) => {
        const labelled = element.getAttribute("aria-labelledby");
        if (labelled) return labelled.split(/\s+/).map((id) => document.getElementById(id)?.textContent || "").join(" ");
        if (element.getAttribute("aria-label")) return element.getAttribute("aria-label");
        if (element.labels && element.labels.length) return [...element.labels].map((label) => label.textContent).join(" ");
        if (element.tagName === "FIELDSET") return element.querySelector("legend")?.textContent || "";
        return element.textContent || element.value || "";
      };
      const roleOf = (element) => element.getAttribute("role") || ({ INPUT: element.type === "radio" ? "radio" :
        element.type === "checkbox" ? "checkbox" : "textbox", SELECT: "combobox", TEXTAREA: "textbox", BUTTON: "button",
      A: "link" })[element.tagName] || element.tagName.toLowerCase();
      const all = [...document.querySelectorAll("input, select, textarea, button, a, [role], label")]
        .filter((element) => element.getClientRects().length);
      const hits = all.filter((element) => nameOf(element).toLowerCase().includes(words) && (!role || roleOf(element).includes(role)));
      const exact = hits.filter((element) => nameOf(element).trim().toLowerCase() === words);
      const chosen = hits.length === 1 ? hits : exact.length === 1 ? exact : hits;
      chosen.forEach((element, at) => element.setAttribute("data-desk-hit", String(at)));
      return { count: chosen.length, names: chosen.slice(0, 5).map((element) => nameOf(element).trim().slice(0, 60)) };
    }, { words, role });
    if (found.count === 0) throw new Refusal("element_not_found", `no element reads "${flags.label || flags.text}"`);
    if (found.count > 1) throw new Refusal("ambiguous_target", `${found.count} elements read so: ${found.names.join(" | ")}`);
    const handle = page.locator("[data-desk-hit='0']");
    await page.evaluate(() => document.querySelectorAll("[data-desk-hit]").forEach((element) => element.removeAttribute("data-desk-hit")));
    return handle;
  }

  /* One verb, as the window runs it. */
  async function run(argv) {
    const verb = argv[0];
    const flags = flagsOf(argv);
    count(tally.verbs, verb);
    const started = Date.now();
    switch (verb) {
      case "observe": return look(flags, verb);
      case "screenshot": return look({ ...flags, diff: false }, verb);
      case "zoom": {
        const [x0, y0, x1, y1] = String(flags.region).split(",").map(Number);
        const shot = await picture({ x: x0, y: y0, width: Math.max(1, x1 - x0), height: Math.max(1, y1 - y0) });
        return { verb, screenshot: { path: shot.path, width: x1 - x0, height: y1 - y0, scale: 1 }, origin: { x: x0, y: y0 } };
      }
      case "mouse-move":
        pointer = { x: Number(flags.x), y: Number(flags.y) };
        await page.mouse.move(pointer.x, pointer.y);
        await handStep(started);
        return { path: "synthetic" };
      case "mouse-click": {
        pointer = { x: Number(flags.x), y: Number(flags.y) };
        const clicks = Number(flags["click-count"] || 1);
        await page.mouse.click(pointer.x, pointer.y, { button: flags["mouse-button"] || "left", clickCount: clicks });
        await handStep(started);
        return { path: "synthetic", clickCount: clicks };
      }
      case "click": {
        if (!flags.app) throw new Refusal("invalid_request", "click needs --app with --label/--role, or use mouse-click");
        const target = await byReading(flags);
        const editable = await target.evaluate((element) => element.matches("input:not([type=radio]):not([type=checkbox]), textarea, select, [contenteditable]"));
        if (editable) await target.focus(); else await target.click();
        await handStep(started);
        return { action: { pressed: true } };
      }
      case "mouse-drag":
        await page.mouse.move(Number(flags["from-x"]), Number(flags["from-y"]));
        await page.mouse.down();
        await page.mouse.move(Number(flags["to-x"]), Number(flags["to-y"]));
        await page.mouse.up();
        await handStep(started);
        return { path: "synthetic" };
      case "mouse-scroll": {
        const dy = -Number(flags.dy || 0) * PIXELS_PER_LINE;
        const dx = -Number(flags.dx || 0) * PIXELS_PER_LINE;
        await page.mouse.move(Number(flags.x), Number(flags.y));
        await page.mouse.wheel(dx, dy);
        await handStep(started);
        return { path: "synthetic" };
      }
      case "key":
        await page.keyboard.press(chord(flags.key));
        await handStep(started);
        return { path: "synthetic" };
      case "type": {
        const pieces = String(flags.text).split(/(\t|\n)/);
        for (const piece of pieces) {
          if (piece === "\t") await page.keyboard.press("Tab");
          else if (piece === "\n") await page.keyboard.press("Enter");
          else if (piece) await page.keyboard.type(piece);
        }
        await handStep(started);
        return { path: "synthetic", verification: { state: "verified" } };
      }
      case "wait": {
        const ms = Number(flags.ms || 0);
        await sleep(ms);
        tally.waitMs += ms;
        return { waitedMs: ms, capped: false };
      }
      case "activate":
        await handStep(started);
        return { name: flags.app, pid: BROWSER_PID, active: true };
      case "launch":
        return { name: flags.app, pid: BROWSER_PID, ready: true };
      case "open":
        if (flags.url) await page.goto(String(flags.url));
        await handStep(started);
        return { opened: flags.url || flags.path };
      case "list-all-windows":
        return { windows: [{ id: WINDOW_ID, app: BROWSER_APP, pid: BROWSER_PID, title: await page.title(),
          x: 0, y: 0, width: SCREEN.width, height: SCREEN.height, own: false }] };
      case "cursor-position":
        return pointer;
      case "status":
        return { helper: { running: true }, pace: { mode: "unlimited" } };
      case "read":
      case "find":
        return { text: await tree() };
      case "wait-for": {
        const deadline = Date.now() + Number(flags["timeout-ms"] || 0);
        const words = String(flags.text || flags.label || "").toLowerCase();
        for (;;) {
          const seen = (await tree()).toLowerCase().includes(words);
          if (seen !== Boolean(flags.absent)) return { found: !flags.absent };
          if (Date.now() >= deadline) throw new Refusal("timeout", `"${flags.text || flags.label}" did not ${flags.absent ? "leave" : "appear"}`);
          await sleep(WAIT_FOR_POLL_MS);
        }
      }
      case "handoff": {
        await sleep(personAttachMs);
        const upload = page.locator("input[type=file]");
        if (await upload.count()) await upload.first().setInputFiles(SCAN);
        tally.handoffs += 1;
        await appendFile(join(out, "handoffs.jsonl"), `${JSON.stringify({ at_ms: Date.now(), reason_chars: String(flags.reason || "").length })}\n`);
        actedAt = Date.now();
        return { resumed: true, reason: flags.reason };
      }
      case "batch": return batch(JSON.parse(flags.commands));
      default:
        throw new Refusal("invalid_request", `this desk does not answer \`${verb}\``);
    }
  }

  /* The window's batch: each step on the same road as alone, stopped at the
   * first refusal, every report stamped with its time. */
  async function batch(commands) {
    const began = Date.now();
    const steps = [];
    let refusedAt = null;
    for (const [at, argv] of commands.entries()) {
      count(tally.steps, argv[0]);
      const stepStarted = Date.now();
      try {
        if (Date.now() - began > BATCH_DEADLINE_MS) throw new Refusal("timeout", "the batch ran out of time");
        const result = await run(argv);
        steps.push({ n: at + 1, verb: argv[0], ok: true, result, ms: Date.now() - stepStarted });
      } catch (error) {
        steps.push({ n: at + 1, verb: argv[0], ok: false, error: { code: error.code || "failed", message: error.message }, ms: Date.now() - stepStarted });
        refusedAt = at + 1;
        break;
      }
    }
    const result = { ran: steps.filter((step) => step.ok).length, of: commands.length, elapsedMs: Date.now() - began, steps };
    if (refusedAt) result.refusedAt = refusedAt;
    return result;
  }

  async function answer(argv) {
    try {
      const result = await run(argv);
      const failed = argv[0] === "batch" && result.refusedAt;
      if (failed) {
        // The window's refused batch (computer_use::batch_answer): the
        // refused step's own code, its words led by where it stood.
        const step = result.steps[result.refusedAt - 1];
        const message = `step ${step.n} of ${result.of} (${step.verb}): ${step.error.message}`;
        return { status: 409, body: { ok: false, error: { code: step.error.code, message }, result } };
      }
      return { status: 200, body: { ok: true, result } };
    } catch (error) {
      return { status: 409, body: { ok: false, error: { code: error.code || "failed", message: error.message } } };
    }
  }

  const doors = { computer: answer };
  const server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", async () => {
      const door = doors[request.url.replace(/^\//, "")];
      if (!door) { response.writeHead(404).end(); return; }
      const argv = body.split("\x1f").filter((word, at, all) => !(at === all.length - 1 && word === ""));
      const { status, body: reply } = await door(argv);
      response.writeHead(status, { "content-type": "application/json" }).end(`${JSON.stringify(reply)}\n`);
    });
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return {
    port: server.address().port,
    page,
    tally,
    doors,
    async close() {
      server.close();
      await browser.close();
    },
  };
}
