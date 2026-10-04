/* The long-form bench's fake desk (t-37883): the form in a headless Chromium,
 * answered the way the window answers `zerocode-computer` — so an agent's
 * real Computer road (zo's tool, its looks, its batches) runs against the
 * fixture without the person's pointer, keyboard or screen.
 *
 * The real pointer may not be used for a run this long, so this is the fake
 * pointer road: every verb an agent sends arrives here (argv joined by 0x1f,
 * as the shim sends it), acts on the page through Playwright, and answers the
 * window's envelope. Two scenes:
 *
 * - `phone`: what the person's session drove — the iPhone Mirroring window
 *   showing a phone app. One 1920 × 1080 display; the window 436 × 958 (the
 *   size every look of that session had); no accessibility tree (pixels
 *   only); input reaches the phone `lagMs` late and the mirrored picture keeps
 *   repainting `streamTailMs` after the phone stops changing; the window
 *   answers to its English name, its Korean name and its bundle id.
 * - `chrome`: the web form in a desktop browser with its accessibility tree.
 *
 * What the window's hand costs in time is charged as the person's session
 * measured it (each verb's p50 step ms), a `wait` sleeps its whole duration
 * and a look settles the eye's way — every number a named constant below
 * with where it was read. The person is fake too: a hand-off attaches a
 * synthetic scan after the person's own time for it, logged for the oracle.
 */

import { createServer } from "node:http";
import { mkdir, writeFile, appendFile } from "node:fs/promises";
import { join } from "node:path";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";

// The scenes. A window rect is in display points; `screen` is the phone's
// own CSS size, drawn into the window below its `screenTop` bar.
export const SCENES = {
  phone: {
    display: { width: 1920, height: 1080 },
    app: { names: ["iPhone Mirroring", "iPhone 미러링"], bundleId: "com.apple.ScreenContinuity", pid: 4243 },
    window: { x: 742, y: 61, width: 436, height: 958 },
    screen: { width: 390, height: 844, top: 14 },
    page: "phone.html",
    accessible: false,
    // Input reaches the phone this late, and the mirrored picture repaints
    // this long after the phone stops changing (the session's looks after a
    // batch were still moving at the eye's 1 s cap 18 times in 110).
    lagMs: 250,
    streamTailMs: 600,
    // A mirrored picture is never the same twice (that session's looks:
    // changed non-empty 80 times, empty 0).
    streamNoise: true,
  },
  chrome: {
    display: { width: 1280, height: 800 },
    app: { names: ["Google Chrome"], bundleId: "com.google.Chrome", pid: 4242 },
    window: { x: 0, y: 0, width: 1280, height: 800 },
    page: "",
    accessible: true,
    lagMs: 0,
    streamTailMs: 0,
    streamNoise: false,
  },
};
// The long edge of the first screenshot rung (computer_use_protocol
// SCREENSHOT_RESIZE first rung): a look is never wider than this.
const LOOK_LONG_EDGE = 1280;
// Each acting verb's hand time, the p50 of the person's session's batch
// reports (coordinator m-37970, 10-04): activate 8, key 4, click 22, scroll 20.
const HAND_MS = { activate: 8, key: 4, "mouse-click": 22, "mouse-scroll": 20, "mouse-move": 4, "mouse-drag": 22, type: 4, click: 22 };
// The eye's settle (crates/zerocode-core/src/computer_use.rs
// COMPUTER_SETTLE_MS, EYE_QUIET_MS, EYE_SETTLE_MAX_MS): a look after an act
// that repainted nothing waits 350 ms, one that did waits for 100 ms of
// quiet, and none waits past one second.
const SETTLE_NOTHING_MS = 350;
const SETTLE_QUIET_MS = 100;
const SETTLE_MAX_MS = 1000;
const SETTLE_POLL_MS = 25;
// `wait-for` polls every 250 ms (COMPUTER_WAIT_FOR_POLL_MS).
const WAIT_FOR_POLL_MS = 250;
// A wheel line, in CSS pixels (Chromium's kPixelsPerLineStep).
const PIXELS_PER_LINE = 40;
// The batch's whole deadline (COMPUTER_USE_DEADLINE_SECONDS).
const BATCH_DEADLINE_MS = 65_000;
// The tree a look carries at most (the helper's TreeRenderer maxNodes).
const TREE_MAX_NODES = 1200;
const WINDOW_ID = 1;
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
// What every frame of the page keeps for the eye: when it last changed, and
// how many of its transitions are running.
const CHANGE_CLOCK = () => {
  window.__deskChangedAt = performance.now();
  window.__deskMoving = 0;
  const touch = () => { window.__deskChangedAt = performance.now(); };
  new MutationObserver(touch).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  for (const kind of ["input", "change", "scroll"]) addEventListener(kind, touch, true);
  addEventListener("transitionrun", () => { window.__deskMoving += 1; touch(); }, true);
  for (const kind of ["transitionend", "transitioncancel"]) {
    addEventListener(kind, () => { window.__deskMoving = Math.max(0, window.__deskMoving - 1); touch(); }, true);
  }
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
  return String(words).split("+").map((part) => {
    const key = part.trim();
    return KEY_NAMES[key.toLowerCase()] || (key.length === 1 ? key : key[0].toUpperCase() + key.slice(1));
  }).join("+");
}

/* The phone's window on the display: a dark bar, the phone screen scaled to
 * the window's width, and one pixel the stream repaints on every frame. */
function phoneDesk(scene, url) {
  const { window: box, screen } = scene;
  const scale = box.width / screen.width;
  return `<!doctype html><html><body style="margin:0;width:${scene.display.width}px;height:${scene.display.height}px;
background:linear-gradient(135deg,#3a4a6b,#7b5a8c);overflow:hidden">
<div style="position:absolute;left:24px;top:24px;width:660px;height:1032px;background:#1e1e22;border-radius:10px;color:#9a9aa2;
font:13px monospace;padding:16px;box-sizing:border-box;white-space:pre">zerocode — zo\n\n› register the trip on the phone\n\n⏺ Computer(batch)\n⏺ Computer(observe)</div>
<div style="position:absolute;left:${box.x}px;top:${box.y}px;width:${box.width}px;height:${box.height}px;background:#000;border-radius:28px;overflow:hidden">
<div id="noise" style="position:absolute;left:${box.width / 2}px;top:4px;width:2px;height:2px;background:#000"></div>
<iframe src="${url}${scene.page}" style="position:absolute;left:0;top:${screen.top}px;width:${screen.width}px;height:${screen.height}px;border:0;
transform:scale(${scale});transform-origin:0 0"></iframe></div></body></html>`;
}

export async function startDesk({ url, out, personAttachMs, scene: sceneName = "phone", headless = true, overrides = {} }) {
  const scene = { ...SCENES[sceneName], ...overrides };
  await mkdir(join(out, "shots"), { recursive: true });
  const browser = await chromium.launch({ headless });
  const page = await browser.newPage({ viewport: scene.display });
  await page.addInitScript(CHANGE_CLOCK);
  if (scene.accessible) await page.goto(`${url}${scene.page}`);
  else await page.setContent(phoneDesk(scene, url), { waitUntil: "load" });
  const content = () => (scene.accessible ? page.mainFrame() : page.frames().find((frame) => frame.url().endsWith(scene.page)));
  await content().waitForFunction(() => typeof LongForm === "object" && LongForm.spec !== null);
  const cdp = await page.context().newCDPSession(page);
  const tally = { verbs: {}, steps: {}, waitMs: 0, handMs: 0, settleMs: 0, settledFalse: 0, pngBytes: [], handoffs: 0, looks: 0 };
  const lastLook = new Map();
  let shots = 0;
  let pointer = { x: 0, y: 0 };
  let actedAt = null;
  // Input reaches the app `lagMs` after it is sent, in the order it was sent.
  let delivered = Promise.resolve();
  const deliver = (apply) => {
    const due = Date.now() + scene.lagMs;
    delivered = delivered.then(async () => {
      await sleep(due - Date.now());
      await apply().catch(() => {});
    });
  };

  const count = (table, verb) => { table[verb] = (table[verb] || 0) + 1; };

  /* The hand's time for one acting verb, as the session measured it. */
  async function handStep(verb, started) {
    const owed = (HAND_MS[verb] ?? 0) - (Date.now() - started);
    if (owed > 0) await sleep(owed);
    tally.handMs += Date.now() - started;
    actedAt = Date.now();
  }

  /* When the app's picture last changed, on the desk's clock — the mirrored
   * stream repainting `streamTailMs` beyond it. */
  async function repaintedAt() {
    const quietFor = await content().evaluate(() => (window.__deskMoving ? 0 : performance.now() - window.__deskChangedAt));
    return Date.now() - quietFor + scene.streamTailMs;
  }

  /* The eye's settle after the last act: nothing repainted → 350 ms, else
   * 100 ms of quiet, never past one second. */
  async function settle() {
    if (actedAt === null) return { settled: true };
    const started = Date.now();
    let settled = true;
    for (;;) {
      const now = Date.now();
      const since = now - actedAt;
      const last = await repaintedAt();
      const repainted = last > actedAt;
      if (!repainted && since >= SETTLE_NOTHING_MS) break;
      if (repainted && now - last >= SETTLE_QUIET_MS) break;
      if (since >= SETTLE_MAX_MS) { settled = false; tally.settledFalse += 1; break; }
      await sleep(SETTLE_POLL_MS);
    }
    const waitedMs = Date.now() - started;
    tally.settleMs += waitedMs;
    return settled ? { settled } : { settled, waitedMs };
  }

  async function picture(clip) {
    if (scene.streamNoise) {
      await page.evaluate(() => {
        const dot = document.getElementById("noise");
        if (dot) dot.style.background = `rgb(${Math.floor(Math.random() * 40)},0,0)`;
      });
    }
    const scale = Math.min(1, LOOK_LONG_EDGE / Math.max(clip.width, clip.height));
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png", clip: { ...clip, scale } });
    const png = Buffer.from(data, "base64");
    shots += 1;
    const path = join(out, "shots", `${shots}.png`);
    await writeFile(path, png, { mode: 0o600 });
    tally.pngBytes.push(png.length);
    return { png, path, scale, width: Math.round(clip.width * scale), height: Math.round(clip.height * scale) };
  }

  /* The app an `--app` names: any of its names (case-blind) or its bundle id. */
  function appOf(query) {
    const asked = String(query).trim().toLowerCase();
    const known = [...scene.app.names, scene.app.bundleId].map((name) => name.toLowerCase());
    if (!known.includes(asked) && asked !== String(scene.app.pid)) throw new Refusal("app_not_found", `app '${query}' not found`);
    return scene.app;
  }

  /* The app's accessibility tree as the helper renders it. A mirrored phone
   * has none beyond its window. */
  async function tree() {
    const lines = [`App=${scene.app.names[0]} (pid ${scene.app.pid})`, `Window: "${scene.app.names[0]}"`];
    if (!scene.accessible) return { text: [...lines, `0 standard window ${scene.app.names[0]}`].join("\n"), elementCount: 1 };
    const { nodes } = await cdp.send("Accessibility.getFullAXTree");
    const byId = new Map(nodes.map((node) => [node.nodeId, node]));
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
    return { text: lines.join("\n"), elementCount: index };
  }

  async function look(flags, verb) {
    tally.looks += 1;
    const settled = flags.settle ? await settle() : null;
    const app = flags.app ? appOf(flags.app) : null;
    const clip = app ? { ...scene.window } : { x: 0, y: 0, ...scene.display };
    const shot = await picture(clip);
    // The window keeps a viewer's last picture per place, named as asked.
    const place = `${flags.viewer || ""}/${app ? `app:${flags.app}` : "desktop"}`;
    const before = lastLook.get(place);
    let changed = null;
    if (flags.diff && before) changed = before.equals(shot.png) ? [] : [{ x: clip.x, y: clip.y, width: clip.width, height: clip.height }];
    lastLook.set(place, shot.png);
    const answer = {
      verb,
      screenshot: { path: shot.path, width: shot.width, height: shot.height, scale: shot.scale },
      origin: { x: clip.x, y: clip.y }, scale: shot.scale, cursor: pointer,
      tree: app ? { window: scene.app.names[0], ...(await tree()) } : null,
      text: null, changed,
    };
    if (settled) answer.settle = settled;
    return answer;
  }

  /* An element by what it reads, the way the helper's query matches: a
   * case-blind piece of its accessible name. A phone mirror has none. */
  async function byReading(flags) {
    if (!scene.accessible) throw new Refusal("element_not_found", `no element in '${flags.app}' reads "${flags.label || flags.text}"`);
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
    if (found.count !== 1) {
      await page.evaluate(() => document.querySelectorAll("[data-desk-hit]").forEach((element) => element.removeAttribute("data-desk-hit")));
      if (found.count === 0) throw new Refusal("element_not_found", `no element reads "${flags.label || flags.text}"`);
      throw new Refusal("ambiguous_target", `${found.count} elements read so: ${found.names.join(" | ")}`);
    }
    return page.locator("[data-desk-hit='0']");
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
        const clip = { x: x0, y: y0, width: Math.max(1, x1 - x0), height: Math.max(1, y1 - y0) };
        const shot = await picture(clip);
        return { verb, screenshot: { path: shot.path, width: shot.width, height: shot.height, scale: shot.scale }, origin: { x: x0, y: y0 } };
      }
      case "mouse-move":
        pointer = { x: Number(flags.x), y: Number(flags.y) };
        deliver(() => page.mouse.move(pointer.x, pointer.y));
        await handStep(verb, started);
        return { path: "synthetic" };
      case "mouse-click": {
        const at = { x: Number(flags.x), y: Number(flags.y) };
        pointer = at;
        const clicks = Number(flags["click-count"] || 1);
        deliver(() => page.mouse.click(at.x, at.y, { button: flags["mouse-button"] || "left", clickCount: clicks }));
        await handStep(verb, started);
        return { path: "synthetic", clickCount: clicks };
      }
      case "click": {
        if (!flags.app) throw new Refusal("invalid_request", "click needs --app with --label/--role, or use mouse-click");
        appOf(flags.app);
        const target = await byReading(flags);
        const editable = await target.evaluate((element) => element.matches("input:not([type=radio]):not([type=checkbox]), textarea, select, [contenteditable]"));
        deliver(async () => {
          if (editable) await target.focus(); else await target.click();
          await page.evaluate(() => document.querySelectorAll("[data-desk-hit]").forEach((element) => element.removeAttribute("data-desk-hit")));
        });
        await handStep(verb, started);
        return { action: { pressed: true } };
      }
      case "mouse-drag": {
        const [from, to] = [[Number(flags["from-x"]), Number(flags["from-y"])], [Number(flags["to-x"]), Number(flags["to-y"])]];
        deliver(async () => { await page.mouse.move(...from); await page.mouse.down(); await page.mouse.move(...to); await page.mouse.up(); });
        await handStep(verb, started);
        return { path: "synthetic" };
      }
      case "mouse-scroll": {
        const at = { x: Number(flags.x), y: Number(flags.y) };
        const [dx, dy] = [-Number(flags.dx || 0) * PIXELS_PER_LINE, -Number(flags.dy || 0) * PIXELS_PER_LINE];
        deliver(async () => { await page.mouse.move(at.x, at.y); await page.mouse.wheel(dx, dy); });
        await handStep(verb, started);
        return { path: "synthetic" };
      }
      case "key": {
        const keys = chord(flags.key);
        deliver(() => page.keyboard.press(keys));
        await handStep(verb, started);
        return { path: "synthetic" };
      }
      case "type": {
        const text = String(flags.text);
        deliver(async () => {
          for (const piece of text.split(/(\t|\n)/)) {
            if (piece === "\t") await page.keyboard.press("Tab");
            else if (piece === "\n") await page.keyboard.press("Enter");
            else if (piece) await page.keyboard.type(piece);
          }
        });
        await handStep(verb, started);
        return { path: "synthetic", verification: { state: scene.accessible ? "verified" : "unverified" } };
      }
      case "wait": {
        const ms = Number(flags.ms || 0);
        await sleep(ms);
        tally.waitMs += ms;
        return { waitedMs: ms, capped: false };
      }
      case "activate": {
        appOf(flags.app);
        await handStep(verb, started);
        return { name: scene.app.names[0], pid: scene.app.pid, active: true };
      }
      case "launch":
        appOf(flags.app);
        return { name: scene.app.names[0], pid: scene.app.pid, ready: true };
      case "open":
        if (flags.url && scene.accessible) await page.goto(String(flags.url));
        await handStep(verb, started);
        return { opened: flags.url || flags.path };
      case "list-all-windows":
        return { windows: [{ id: WINDOW_ID, app: scene.app.names[0], bundleId: scene.app.bundleId, pid: scene.app.pid,
          title: scene.app.names[0], ...scene.window, own: false }] };
      case "cursor-position":
        return pointer;
      case "status":
        return { helper: { running: true }, pace: { mode: "unlimited" } };
      case "read":
      case "find":
        if (flags.app) appOf(flags.app);
        return { text: (await tree()).text };
      case "wait-for": {
        const deadline = Date.now() + Number(flags["timeout-ms"] || 0);
        const words = String(flags.text || flags.label || "").toLowerCase();
        for (;;) {
          const seen = (await tree()).text.toLowerCase().includes(words);
          if (seen !== Boolean(flags.absent)) return { found: !flags.absent };
          if (Date.now() >= deadline) throw new Refusal("timeout", `"${flags.text || flags.label}" did not ${flags.absent ? "leave" : "appear"}`);
          await sleep(WAIT_FOR_POLL_MS);
        }
      }
      case "handoff": {
        await sleep(personAttachMs);
        if (scene.accessible) {
          const upload = page.locator("input[type=file]");
          if (await upload.count()) await upload.first().setInputFiles(SCAN);
        } else {
          await content().evaluate(({ name, size }) => window.longFormAttach && window.longFormAttach({ name, size }),
            { name: SCAN.name, size: SCAN.buffer.length });
        }
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
      if (argv[0] === "batch" && result.refusedAt) {
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
    scene,
    content,
    tally,
    doors,
    // Every input sent has reached the app.
    settled: () => delivered,
    async close() {
      server.close();
      await browser.close();
    },
  };
}
