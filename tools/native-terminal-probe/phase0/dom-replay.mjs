#!/usr/bin/env node
/* Phase 0, the DOM half: the window's own terminal (ui/shell-term.js) fed a
 * recording's frames, the same frames the GPUI replay draws.
 *
 *   node tools/native-terminal-probe/phase0/dom-replay.mjs timing  --recordings DIR --out DIR [--cases a,b] [--repeat N] [--level normal|low|cpu4x]
 *   node tools/native-terminal-probe/phase0/dom-replay.mjs capture --recordings DIR --out DIR [--cases a,b]
 *
 * The page is the window harness's (ui/tests/window-boot.mjs): the real
 * window in headless Chromium over a stub backend. Only one answer is ours:
 * `term_pull` returns, for one frame at a time, the bytes the backend's own
 * `term_pull` would have written for it — `{"screens":[{"term":…,"frames":[<the
 * recording's line>]}],"missing":[]}` — so the window pays the decode, the
 * parse and the apply it pays in the app. The notice that makes the window
 * pull is the one the stub sends (`term:dirty:<label>`).
 *
 * timing — one frame per display frame, as a streaming pane is fed. For every
 * frame: the apply (`view.applyFrames`, the window's own model update and DOM
 * writes), the decode before it (bytes returned → apply start: TextDecoder and
 * JSON.parse), and feed → frame done: from handing the bytes over, inside the
 * display frame's rAF, to the message posted in that rAF, which runs after the
 * frame's style, layout and paint (the definition terminal-pacing.mjs uses).
 * A frame applied after its own display frame's paint is counted, not hidden.
 * The compositor, rasterization and the GPU are not visible from here, and
 * headless Chromium does not composite on the GPU at all.
 *
 * capture — the same frames, and at each checkpoint of the recording the
 * screen the DOM holds: every glyph's text, its column from the browser's own
 * layout, its computed colour and weight, each run's background, the caret;
 * and a PNG of the grid (caret, scrollbar and fold handles hidden). Nothing is
 * timed in this pass. */
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { cpus, loadavg } from "node:os";
import { join, resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage, WINDOW_BROWSER_ARGS } from "../../../ui/tests/window-boot.mjs";
import { installTerminalWaits } from "../../../ui/tests/terminal-grid-selection.mjs";
import { powerState } from "../../low-spec/profile.mjs";

const USAGE = "usage: dom-replay.mjs timing|capture --recordings DIR --out DIR [--cases a,b] [--repeat N] [--level normal|low|cpu4x] [--scale N] [--idle N]";

function parse(argv) {
  const [mode, ...rest] = argv;
  if (!["timing", "capture"].includes(mode)) throw new Error(USAGE);
  const opt = (name, fallback) => {
    const at = rest.indexOf(name);
    return at >= 0 ? rest[at + 1] : fallback;
  };
  const recordings = opt("--recordings", null);
  const out = opt("--out", null);
  if (!recordings || !out) throw new Error(USAGE);
  return {
    mode,
    recordings: resolve(recordings),
    out: resolve(out),
    cases: (opt("--cases", "") || "").split(",").filter(Boolean),
    repeat: Number(opt("--repeat", "1")),
    level: opt("--level", "normal"),
    scale: Number(opt("--scale", "2")),
    idle: Number(opt("--idle", "120")),
  };
}

/* The cases a recording folder holds, in the order the recorder wrote them. */
function casesIn(dir, wanted) {
  const all = readdirSync(dir).filter((name) => name.endsWith(".meta.json")).map((name) => name.slice(0, -".meta.json".length)).sort();
  const picked = wanted.length > 0 ? wanted : all;
  for (const name of picked) if (!all.includes(name)) throw new Error(`no recording ${name} in ${dir}`);
  return picked.map((name) => ({
    name,
    meta: JSON.parse(readFileSync(join(dir, `${name}.meta.json`), "utf8")),
    lines: readFileSync(join(dir, `${name}.deltas.jsonl`), "utf8").split("\n").filter(Boolean),
  }));
}

/* Every process under the browser, with its resident memory: the sum is what the
 * renderer costs the machine while it draws. `ps` reads it, outside the page. */
function treeRss(rootPid) {
  const text = execFileSync("ps", ["-A", "-o", "pid=,ppid=,rss=,args="], { encoding: "utf8" });
  const procs = text.split("\n").map((line) => line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/)).filter(Boolean)
    .map(([, pid, ppid, rss, args]) => ({ pid: Number(pid), ppid: Number(ppid), rssKiB: Number(rss), args }));
  const inTree = new Set([rootPid]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const proc of procs) {
      if (!inTree.has(proc.pid) && inTree.has(proc.ppid)) {
        inTree.add(proc.pid);
        grew = true;
      }
    }
  }
  const kinds = {};
  let total = 0;
  for (const proc of procs) {
    if (!inTree.has(proc.pid)) continue;
    const kind = proc.pid === rootPid ? "browser" : (proc.args.match(/--type=([\w-]+)/)?.[1] ?? "other");
    kinds[kind] = (kinds[kind] ?? 0) + proc.rssKiB;
    total += proc.rssKiB;
  }
  return { totalKiB: total, kinds, processes: inTree.size };
}

/* The browser Playwright started for this run. Its `Browser` gives no process,
 * but the browser descends from this one: the topmost descendant whose command
 * is Chromium's and carries no `--type=` (the renderer, GPU and utility
 * processes do). */
function browserPid() {
  const text = execFileSync("ps", ["-A", "-o", "pid=,ppid=,args="], { encoding: "utf8" });
  const procs = text.split("\n").map((line) => line.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/)).filter(Boolean)
    .map(([, pid, ppid, args]) => ({ pid: Number(pid), ppid: Number(ppid), args }));
  const mine = new Set([process.pid]);
  for (let grew = true; grew;) {
    grew = false;
    for (const proc of procs) if (!mine.has(proc.pid) && mine.has(proc.ppid)) { mine.add(proc.pid); grew = true; }
  }
  const browsers = procs.filter((proc) => proc.pid !== process.pid && mine.has(proc.pid) && /chrom|headless_shell/i.test(proc.args) && !/--type=/.test(proc.args));
  const ids = new Set(browsers.map((proc) => proc.pid));
  const roots = browsers.filter((proc) => !ids.has(proc.ppid));
  if (roots.length !== 1) throw new Error(`the browser process: ${roots.length} candidates under this process; runtime memory cannot be read`);
  return roots[0].pid;
}

/* Opens the window, a terminal tab in front, and our pull answer in place. */
async function standTerminal(browser, origin, { scale }) {
  const { page, faults } = await openWindowTestPage(browser, origin, {
    // Large enough for a 60×220 grid beside the window's own chrome; the grid is
    // measured below and a grid that does not fit is refused, not clipped.
    context: { viewport: { width: 2560, height: 1440 }, deviceScaleFactor: scale },
  });
  await installTerminalWaits(page);
  const term = await page.evaluate(async () => {
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const opened = await openTermTab({ placement: "tab" });
    const view = termView(opened);
    setActiveTab(tabOfTerm(opened).id);
    await window.__TERMINAL_READY__(opened, view);
    return opened;
  });
  await page.evaluate((term) => {
    const encoder = new TextEncoder();
    const empty = encoder.encode('{"screens":[],"missing":[]}').buffer;
    const state = {
      term,
      answers: [],
      pending: null,
      pendingIndex: -1,
      pulledAt: 0,
      pulledIndex: -1,
      applies: [],
    };
    window.__PHASE0__ = state;
    window.__ANSWER__.term_pull = () => {
      if (state.pending === null) return empty;
      const bytes = state.pending;
      state.pending = null;
      state.pulledAt = performance.now();
      state.pulledIndex = state.pendingIndex;
      return bytes;
    };
    const view = termView(term);
    const shipped = view.applyFrames;
    view.applyFrames = (frames) => {
      const start = performance.now();
      try {
        return shipped(frames);
      } finally {
        state.applies.push({ index: state.pulledIndex, pulledAt: state.pulledAt, start, end: performance.now(), frames: frames.length });
      }
    };
    // The stub backend's notice, word for word: every `term:dirty:*` listener, after the current task.
    state.notice = () => queueMicrotask(() => {
      for (const [name, handlers] of Object.entries(window.__LISTENERS__)) {
        if (!name.startsWith("term:dirty:")) continue;
        for (const handler of handlers) handler({ payload: null });
      }
    });
    state.load = (lines) => {
      state.answers = lines.map((line) => encoder.encode(`{"screens":[{"term":${term},"frames":[${line}]}],"missing":[]}`).buffer);
      state.applies = [];
    };
    state.feed = (index) => {
      state.pending = state.answers[index];
      state.pendingIndex = index;
      state.notice();
    };
  }, term);
  return { page, faults, term };
}

/* Where the grid sits and whether all of it is on the page. */
const gridBox = (page, size) => page.evaluate(({ rows, cols }) => {
  const view = termView(window.__PHASE0__.term);
  const box = view.pre.getBoundingClientRect();
  const needW = cols * view.cell.width;
  const needH = rows * view.cell.height;
  return {
    left: box.left, top: box.top, width: box.width, height: box.height,
    cellW: view.cell.width, cellH: view.cell.height, needW, needH,
    fits: box.width + 0.5 >= needW && box.height + 0.5 >= needH
      && box.right <= window.innerWidth && box.bottom <= window.innerHeight,
    dpr: window.devicePixelRatio,
    viewport: { width: window.innerWidth, height: window.innerHeight },
  };
}, size);
const cutGrid = (name, box) => `${name}: the grid (${Math.round(box.needW)}×${Math.round(box.needH)}) is not whole in its ${Math.round(box.width)}×${Math.round(box.height)} box on a ${box.viewport.width}×${box.viewport.height} page; a frame drawn in part is neither timed nor pictured`;

/* The terminal's look as the page resolved it: the 256-colour palette, the
 * screen's own ink and ground, the caret, the face and the measured cell. The
 * GPUI replay paints with exactly these, so a difference in colour or place is
 * the renderer's and not the theme's. */
const exportTheme = (page) => page.evaluate(() => {
  const view = termView(window.__PHASE0__.term);
  const probe = document.createElement("span");
  view.pre.appendChild(probe);
  const resolveColor = (value) => {
    probe.style.color = "";
    probe.style.color = value;
    return getComputedStyle(probe).color;
  };
  const root = getComputedStyle(view.pre);
  const palette = Array.from({ length: 256 }, (unused, index) => resolveColor(`var(--term-${index})`));
  const theme = {
    palette,
    fg: resolveColor("var(--term-screen-fg)"),
    bg: getComputedStyle(view.host).backgroundColor,
    bgSolid: resolveColor("var(--term-bg-solid)"),
    cursor: resolveColor("var(--term-cursor)"),
    font: { family: root.fontFamily, size: root.fontSize, weight: root.fontWeight, lineHeight: root.lineHeight },
    // `.term .bold` and `.term .dim` (shell.css): the weight bold ink takes, and the share of its colour dim ink keeps.
    boldWeight: root.getPropertyValue("--term-bold-weight").trim(),
    dimInk: root.getPropertyValue("--term-dim-ink").trim(),
    cell: { width: view.cell.width, height: view.cell.height },
    dpr: window.devicePixelRatio,
    colorScheme: document.documentElement.dataset.theme ?? null,
  };
  probe.remove();
  return theme;
});

/* The plain CPU loop of tools/low-spec/spin.mjs, inside the page: how much slower this level makes the page's own thread. */
const calibrate = (page) => page.evaluate(() => {
  const started = performance.now();
  let x = 0;
  for (let i = 0; i < 3e8; i += 1) x = (x * 31 + i) | 0;
  return { ms: performance.now() - started, x };
});

/* One recording, one frame per display frame. */
const pacedRun = (page, { frames, idle }) => page.evaluate(async ({ frames, idle }) => {
  const state = window.__PHASE0__;
  state.applies = [];
  const samples = [];
  await new Promise((resolve) => {
    let k = 0;
    let last = null;
    const tick = (stamp) => {
      const gap = last === null ? null : stamp - last;
      last = stamp;
      if (k >= frames + idle) {
        resolve();
        return;
      }
      const fed = k < frames;
      const sample = { k, fed, stamp, gap, feedAt: performance.now(), doneAt: null };
      if (fed) state.feed(k);
      const channel = new MessageChannel();
      channel.port1.onmessage = () => {
        sample.doneAt = performance.now();
        channel.port1.close();
      };
      channel.port2.postMessage(null);
      samples.push(sample);
      k += 1;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  const byIndex = new Map();
  for (const apply of state.applies) {
    if (apply.index < 0) continue;
    const held = byIndex.get(apply.index) ?? [];
    held.push(apply);
    byIndex.set(apply.index, held);
  }
  return samples.map((sample) => {
    const applies = sample.fed ? (byIndex.get(sample.k) ?? []) : [];
    const apply = applies[0] ?? null;
    return {
      k: sample.k,
      fed: sample.fed,
      gapMs: sample.gap,
      feedToDoneMs: sample.doneAt === null ? null : sample.doneAt - sample.feedAt,
      applyMs: apply ? apply.end - apply.start : null,
      decodeMs: apply ? apply.start - apply.pulledAt : null,
      feedToApplyEndMs: apply ? apply.end - sample.feedAt : null,
      applies: applies.length,
      // Applied inside the display frame it was fed in: the apply ended before the
      // message posted in that frame's rAF ran (it runs after the frame's paint).
      sameFrame: apply !== null && sample.doneAt !== null && apply.end <= sample.doneAt,
    };
  });
}, { frames, idle });

function stats(values) {
  const sorted = values.filter((value) => typeof value === "number" && Number.isFinite(value)).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const at = (p) => sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * p) - 1))];
  const round = (n) => Math.round(n * 1000) / 1000;
  return { n: sorted.length, p50: round(at(0.5)), p95: round(at(0.95)), p99: round(at(0.99)), max: round(sorted.at(-1)) };
}

async function timing(args, cases) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true, args: [...WINDOW_BROWSER_ARGS] });
  const rootPid = browserPid();
  const results = [];
  try {
    const { page, faults, term } = await standTerminal(browser, origin, args);
    const theme = await exportTheme(page);
    const calibration = await calibrate(page);
    for (const recording of cases) {
      const size = { rows: recording.meta.rows, cols: recording.meta.cols };
      await page.evaluate((lines) => window.__PHASE0__.load(lines), recording.lines);
      for (let round = 1; round <= args.repeat; round += 1) {
        const before = { power: powerState(), rss: rootPid ? treeRss(rootPid) : null };
        const rssSeen = [];
        const sampler = setInterval(() => {
          if (rootPid) rssSeen.push(treeRss(rootPid).totalKiB);
        }, 250);
        const samples = await pacedRun(page, { frames: recording.lines.length, idle: args.idle });
        clearInterval(sampler);
        const after = { power: powerState(), rss: rootPid ? treeRss(rootPid) : null };
        const box = await gridBox(page, size);
        // The GPUI pass refuses a window that cannot hold the grid; the page is held to the same rule.
        if (!box.fits) throw new Error(cutGrid(recording.name, box));
        const fed = samples.filter((sample) => sample.fed);
        // The floor (frame 0, the reader's first full frame) builds every node; it is kept in the raw
        // samples and left out of the steady numbers.
        const steady = fed.slice(1);
        const idle = samples.filter((sample) => !sample.fed);
        results.push({
          renderer: "dom",
          case: recording.name,
          round,
          level: args.level,
          frames: fed.length,
          grid: box,
          summary: {
            feedToDoneMs: stats(steady.map((sample) => sample.feedToDoneMs)),
            applyMs: stats(steady.map((sample) => sample.applyMs)),
            decodeMs: stats(steady.map((sample) => sample.decodeMs)),
            gapMs: stats(steady.map((sample) => sample.gapMs)),
            idleGapMs: stats(idle.map((sample) => sample.gapMs)),
            idleFeedToDoneMs: stats(idle.map((sample) => sample.feedToDoneMs)),
            notSameFrame: steady.filter((sample) => !sample.sameFrame).length,
            notOneApply: fed.filter((sample) => sample.applies !== 1).length,
            rssPeakKiB: rssSeen.length ? Math.max(...rssSeen) : null,
            rssMedianKiB: rssSeen.length ? [...rssSeen].sort((a, b) => a - b)[rssSeen.length >> 1] : null,
          },
          before,
          after,
          samples,
        });
        console.log(`dom ${args.level} ${recording.name} #${round}: frames ${fed.length}, feed→done p95 ${results.at(-1).summary.feedToDoneMs?.p95} ms, apply p95 ${results.at(-1).summary.applyMs?.p95} ms, not same frame ${results.at(-1).summary.notSameFrame}, load ${before.power.load1}/${before.power.cores}${before.power.loud ? " LOUD" : ""}`);
      }
    }
    const engine = browser.version();
    const out = { renderer: "dom", engine, level: args.level, scale: args.scale, term, theme, calibration, faults, cores: cpus().length, load: loadavg(), results };
    mkdirSync(args.out, { recursive: true });
    writeFileSync(join(args.out, `dom-timing-${args.level}.json`), JSON.stringify(out));
    await page.close();
  } finally {
    await browser.close();
    files.close();
  }
}

/* What the DOM holds at this moment, read from the page's own layout. */
const readDom = (page) => page.evaluate(() => {
  const view = termView(window.__PHASE0__.term);
  const pre = view.pre;
  const box = pre.getBoundingClientRect();
  const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  const rows = [];
  for (const node of pre.querySelectorAll(":scope > .term-row")) {
    const rowBox = node.getBoundingClientRect();
    const spans = [];
    const glyphs = [];
    for (const span of node.children) {
      const text = span.textContent ?? "";
      const look = getComputedStyle(span);
      const spanBox = span.getBoundingClientRect();
      spans.push({
        text,
        cls: span.className,
        x: spanBox.left - box.left,
        w: spanBox.width,
        color: look.color,
        bg: look.backgroundColor,
        weight: look.fontWeight,
        italic: look.fontStyle,
        deco: look.textDecorationLine,
        opacity: look.opacity,
      });
      const textNode = span.firstChild;
      if (!textNode) continue;
      let offset = 0;
      for (const { segment } of segmenter.segment(text)) {
        const range = document.createRange();
        range.setStart(textNode, offset);
        range.setEnd(textNode, offset + segment.length);
        offset += segment.length;
        if (segment.trim() === "") continue;
        const rect = range.getBoundingClientRect();
        glyphs.push({ text: segment, x: rect.left - box.left, w: rect.width, span: spans.length - 1 });
      }
    }
    rows.push({ hidden: node.hidden, y: rowBox.top - box.top, spans, glyphs });
  }
  const caret = view.caret;
  return {
    origin: { left: box.left, top: box.top },
    cell: { width: view.cell.width, height: view.cell.height },
    rows,
    caret: { hidden: caret.hidden, transform: caret.style.transform, width: caret.style.width, height: caret.style.height },
  };
});

/* The fonts the browser actually drew the grid's text in, by family, with how
 * many glyphs each drew (CDP `CSS.getPlatformFontsForNode` over every run). A
 * family the stylesheet names is a wish; this is what was used — and the GPUI
 * grid is given the same one. */
async function platformFonts(page) {
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("DOM.enable");
    await cdp.send("CSS.enable");
    const { root } = await cdp.send("DOM.getDocument", { depth: -1 });
    const { nodeIds } = await cdp.send("DOM.querySelectorAll", { nodeId: root.nodeId, selector: "pre.term .term-row > span" });
    const used = {};
    let asciiSpans = 0;
    const ascii = {};
    for (const nodeId of nodeIds) {
      const { fonts } = await cdp.send("CSS.getPlatformFontsForNode", { nodeId });
      const { outerHTML } = await cdp.send("DOM.getOuterHTML", { nodeId });
      const plain = /^[\x20-\x7e<>="\/a-z0-9 -]*$/i.test(outerHTML);
      if (plain) asciiSpans += 1;
      for (const font of fonts) {
        const held = used[font.familyName] ?? { glyphs: 0, postScriptName: font.postScriptName ?? null, custom: font.isCustomFont };
        held.glyphs += font.glyphCount;
        used[font.familyName] = held;
        if (plain) ascii[font.familyName] = (ascii[font.familyName] ?? 0) + font.glyphCount;
      }
    }
    const primary = Object.entries(ascii).sort((a, b) => b[1] - a[1])[0]?.[0] ?? null;
    return { spans: nodeIds.length, asciiSpans, primary, ascii, used };
  } finally {
    await cdp.detach().catch(() => {});
  }
}

async function capture(args, cases) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true, args: [...WINDOW_BROWSER_ARGS] });
  try {
    const { page, faults, term } = await standTerminal(browser, origin, args);
    mkdirSync(args.out, { recursive: true });
    writeFileSync(join(args.out, "dom-theme.json"), JSON.stringify(await exportTheme(page), null, 1));
    const index = [];
    const fontsByCase = {};
    for (const recording of cases) {
      await page.evaluate((lines) => window.__PHASE0__.load(lines), recording.lines);
      const checkpoints = new Set(recording.meta.checkpoints);
      const states = [];
      for (let k = 0; k < recording.lines.length; k += 1) {
        await page.evaluate((k) => new Promise((resolve) => {
          requestAnimationFrame(() => {
            window.__PHASE0__.feed(k);
            const channel = new MessageChannel();
            channel.port1.onmessage = () => resolve();
            channel.port2.postMessage(null);
          });
        }), k);
        if (!checkpoints.has(k)) continue;
        const applied = await page.evaluate(() => window.__PHASE0__.applies.at(-1)?.index ?? -1);
        const state = await readDom(page);
        // The grid at the size this frame says it is (a resize changes it mid-recording).
        const size = await page.evaluate(() => {
          const view = termView(window.__PHASE0__.term);
          const rows = view.pre.querySelectorAll(":scope > .term-row").length;
          return rows;
        });
        const box = await gridBox(page, { rows: recording.meta.rows, cols: recording.meta.cols });
        if (!box.fits) throw new Error(cutGrid(recording.name, box));
        box.rowsShown = size;
        // The grid alone: the caret blinks, and the scrollbar and fold handles are overlays, not cells.
        await page.evaluate(() => {
          const hide = document.createElement("style");
          hide.dataset.phase0 = "hide";
          hide.textContent = ".term-caret,.term-rail,.term-to-live,.term-folds{visibility:hidden!important}";
          document.head.appendChild(hide);
          return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        });
        const png = join(args.out, `${recording.name}.${String(k).padStart(4, "0")}.dom.png`);
        const clip = { x: box.left, y: box.top, width: Math.ceil(box.needW), height: Math.ceil(box.needH) };
        await page.screenshot({ path: png, clip });
        await page.evaluate(() => document.querySelectorAll("style[data-phase0]").forEach((node) => node.remove()));
        // The same grid with the caret held still and shown: the cells that differ from the
        // picture above are where the caret was drawn — the caret seen in pixels, not read
        // from the element's position.
        let caretPng = null;
        if (!state.caret.hidden) {
          await page.evaluate(() => {
            const still = document.createElement("style");
            still.dataset.phase0 = "caret";
            still.textContent = ".term-rail,.term-to-live,.term-folds{visibility:hidden!important}"
              + ".term-caret,.term-caret::after{animation:none!important;visibility:visible!important}";
            document.head.appendChild(still);
            return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          });
          caretPng = join(args.out, `${recording.name}.${String(k).padStart(4, "0")}.caret.dom.png`);
          await page.screenshot({ path: caretPng, clip });
          await page.evaluate(() => document.querySelectorAll("style[data-phase0]").forEach((node) => node.remove()));
        }
        states.push({ frame: k, appliedIndex: applied, grid: box, clip, png, caretPng, state });
      }
      // The fonts behind the last checkpoint's screen, read once per case (it is a CDP call a run).
      const fonts = await platformFonts(page);
      fontsByCase[recording.name] = fonts;
      const file = join(args.out, `${recording.name}.dom-capture.json`);
      writeFileSync(file, JSON.stringify({ renderer: "dom", case: recording.name, frames: recording.lines.length, fonts, states }));
      index.push({ case: recording.name, file, checkpoints: states.length });
      console.log(`dom capture ${recording.name}: ${states.length} checkpoints, ASCII drawn in ${fonts.primary}`);
    }
    // The family the ASCII runs were drawn in, over every case: what the GPUI grid is asked to use.
    const asciiTotals = {};
    for (const fonts of Object.values(fontsByCase)) {
      for (const [family, glyphs] of Object.entries(fonts.ascii)) asciiTotals[family] = (asciiTotals[family] ?? 0) + glyphs;
    }
    const primary = Object.entries(asciiTotals).sort((a, b) => b[1] - a[1])[0]?.[0] ?? null;
    writeFileSync(join(args.out, "dom-fonts.json"), JSON.stringify({ primary, asciiTotals, byCase: fontsByCase }, null, 1));
    writeFileSync(join(args.out, "dom-capture-index.json"), JSON.stringify({ engine: browser.version(), scale: args.scale, term, faults, primaryFont: primary, index }));
    await page.close();
  } finally {
    await browser.close();
    files.close();
  }
}

const args = parse(process.argv.slice(2));
const cases = casesIn(args.recordings, args.cases);
if (args.mode === "timing") await timing(args, cases);
else await capture(args, cases);
