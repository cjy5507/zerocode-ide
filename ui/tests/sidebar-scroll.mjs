import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";
import { median, quantile, webkitType } from "./coordinator-desk-perf.mjs";

/* The sidebar holds its rows while the person scrolls it (t-63568).
 *
 * Reported 2026-10-18 17:3x on v1.1.76: 「한번에 안올라가고 내려갓다 버벅거림」 — a wheel scroll up the sidebar
 * went down first, then caught up. The list is virtualized (`ui/shell-workspace.js`): rows outside the viewport
 * ± 10 stand as two spacers per section, and a row's height is known only once it is built.
 *
 * The scene is synthetic: two repositories, /sa with 200 workspaces and /sb with 40, each workspace with one or
 * two agent panes (`tabs`, `paneAgents`, `hook:agent`), working and resting in turn, so the rows stand at several
 * heights. No coordinator sits, so the sidebar draws the branch list. The viewport is 1280 × 900 px.
 *
 * Three probes in the page: a spy on the scroller's scrollTop setter, scrollTo and scrollBy (every write, with the
 * value before and after); the time of every wheel and scroll event; and, after every painted frame, the scroll
 * and the top of every fully visible row.
 *
 *   residual   for a row fully visible in two frames in a row: its move plus the person's scroll — the scroll's
 *              change less what the window wrote. 0 px means the row moved only by the person's scroll. A pair
 *              that ends with the scroll at 0 counts apart (atTop): the list may settle there.
 *   home       where the first section and row 0 stand at rest with the scroll at 0, read once per page before any
 *              scroll. At the top, a list above its home stops short of row 0 (short); one below it leaves a gap.
 *   step       one wheel tick of −100 px moves the first fully visible row down by 100 px.
 *   mid-scroll write   a write by the window within 100 ms after a wheel or scroll event the page saw before it (in
 *                      the page's own order: WebKit's clock gives whole ms). A scroll event fired inside a write of
 *                      the window is the window's own and does not count.
 *   overlap    how far the cleanup entry's box runs into the list's, while it shows.
 *
 *   ① a cold first pass up from the bottom: 12 ticks one at a time, then a burst of 30 ticks 40 ms apart;
 *   ② the same over the rows ① measured, with a second agent joining the row on the top edge mid-burst — while
 *      the window counts the person as scrolling, and with no write from the growth until its frame is painted;
 *   ③ a cold list: one burst from row 150 that keeps ticking past the top stands row 0 while it still ticks,
 *      and a burst that ends exactly at the top, as a fling ends — how far the list stopped short or left a
 *      gap there, and for how long. Each arrival at the top may move the rows beyond the scroll once (the list
 *      settling there), and from then on the list stands at its home;
 *   ④ a burst down to the end: the last row stands, the scroll is at its end and the bottom spacer is 0 px;
 *   ⑤ at rest every spacer equals the heights of the rows it stands for;
 *   ⑥ folding and unfolding the section under the hand keeps its header where it was;
 *   ⑦ scrollSidebarToPath stands a row the window had not built;
 *   ⑧ the window stays a window, and a row that stays in it keeps its node;
 *   ⑨ mid-burst, after a row above the viewport grew, the cleanup entry appears and hides again: it never runs
 *      into the list, in any frame;
 *   ⑩ a cold list reveals its active workspace, a row never built below rows never measured: the row stands
 *      built, fully visible and focused;
 *   and, with no browser, two traces that show the mid-scroll classifier reads the page's order, not the clock.
 *
 *   node ui/tests/sidebar-scroll.mjs [--engine webkit] [--json <file>]   alone, with the frame trace
 */

const SA = 200;
const SB = 40;
// One wheel tick, and a burst's ticks this far apart — roughly a trackpad's momentum.
const STEP = 100;
const TICK_MS = 40;
// Longer than the window's own rest (150 ms) and the frame after it.
const REST_MS = 400;
// A write this soon after a wheel or scroll event lands inside the person's scroll.
const WRITE_WINDOW_MS = 100;
const ROW0 = "/sa/wt-000";
const LAST_ROW = `/sb/wt-${String(SB - 1).padStart(3, "0")}`;

const round = (value) => (value === null || value === undefined ? value : Math.round(value * 10) / 10);

// `active`: the workspace the catalog says is active, as the backend says it — a refresh sets the window's own.
async function standScrollScene(page, { active = null } = {}) {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async ({ sa, sb, active }) => {
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    const flock = (name, count) => Array.from({ length: count }, (_, at) => ({
      path: `/${name}/wt-${String(at).padStart(3, "0")}`,
      branch: at === 0 ? "main" : `f/${name}-${at}`,
      is_main: at === 0,
      active: `/${name}/wt-${String(at).padStart(3, "0")}` === active,
      is_folder: false,
    }));
    window.__ANSWER__.project_catalog = () => [
      { name: "sa", path: "/sa", worktrees: flock("sa", sa) },
      { name: "sb", path: "/sb", worktrees: flock("sb", sb) },
    ];
    window.__ANSWER__.coordinator_seats = () => [];
    sidebarGroupBy = "repo";
    sidebarSortBy = "default";
    sidebarProjectOrder = "default";
    let term = 7000;
    // A pane that has spoken: its agent row stands under the workspace's row.
    window.__ADD_AGENT__ = (path, state) => {
      term += 1;
      tabs.push({ kind: "term", worktree: path, id: `tab-${term}`, layout: { type: "leaf", term } });
      paneAgents.set(term, "claude");
      tell("hook:agent", { term, state, agent: "claude", session: `s-${term}`, model: "claude-opus-5" });
    };
    for (const [name, count] of [["sa", sa], ["sb", sb]]) {
      for (let at = 0; at < count; at += 1) {
        const path = `/${name}/wt-${String(at).padStart(3, "0")}`;
        window.__ADD_AGENT__(path, at % 2 === 0 ? "working" : "done");
        if (at % 3 === 0) window.__ADD_AGENT__(path, at % 2 === 0 ? "done" : "working");
      }
    }
    setEveryProjectClosed(false);
    await refreshWorktrees();
    for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
  }, { sa: SA, sb: SB, active });
  await page.waitForTimeout(REST_MS);
}

async function installScrollProbes(page) {
  await page.evaluate((row0) => {
    const scroller = document.getElementById("sidebar-scroll");
    const own = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
    const read = () => own.get.call(scroller);
    const probe = {
      writes: [], events: [], frames: [], renders: [], recording: false, setup: false, writing: false, seq: 0, read,
    };
    window.__PROBE__ = probe;
    const wrote = (before, via) => {
      if (!probe.setup) probe.writes.push({ at: performance.now(), seq: (probe.seq += 1), before, after: read(), via });
    };
    Object.defineProperty(scroller, "scrollTop", {
      configurable: true,
      get() {
        return own.get.call(this);
      },
      set(value) {
        probe.writing = true;
        let before;
        try {
          before = own.get.call(this);
          own.set.call(this, value);
        } finally {
          probe.writing = false;
        }
        wrote(before, "scrollTop");
      },
    });
    for (const via of ["scrollTo", "scrollBy"]) {
      const held = scroller[via];
      scroller[via] = function spied(...args) {
        probe.writing = true;
        let before;
        try {
          before = read();
          held.apply(this, args);
        } finally {
          probe.writing = false;
        }
        wrote(before, via);
      };
    }
    // Moves the test makes in the person's place: they are not the window's writes.
    probe.place = (value) => {
      probe.setup = true;
      try {
        scroller.scrollTop = value;
      } finally {
        probe.setup = false;
      }
    };
    probe.reach = (path) => {
      probe.setup = true;
      try {
        scrollSidebarToPath(path);
      } finally {
        probe.setup = false;
      }
    };
    // A scroll event fired while the window writes is the window's, not the person's scroll. WebKit fires it after
    // the write instead, at its next rendering update — often in the same whole ms its clock gives the write — so
    // every write and event also takes its place in the page's own order (seq).
    for (const type of ["wheel", "scroll"]) {
      const note = () => probe.events.push({ type, at: performance.now(), seq: (probe.seq += 1), own: probe.writing });
      scroller.addEventListener(type, note, { capture: true, passive: true });
    }
    const render = window.renderSidebarWindows;
    window.renderSidebarWindows = function timedRender(...args) {
      const started = performance.now();
      try {
        return render.apply(this, args);
      } finally {
        probe.renders.push(performance.now() - started);
      }
    };
    // The home: where the first section and row 0 stand now, at rest with the scroll at 0, in scroll coordinates.
    const section = () => document.querySelector("#worktrees > .proj");
    const zero = () => document.querySelector(`#worktrees [data-row-key="${CSS.escape(row0)}"]`);
    const was = scroller.getBoundingClientRect();
    probe.home = section() && zero()
      ? {
        section: section().getBoundingClientRect().top - was.top + read(),
        row0: zero().getBoundingClientRect().top - was.top + read(),
      }
      : null;
    // What a painted frame shows: the scroll, the top of every fully visible row, the blank the spacers leave
    // inside the viewport, and the list against its home — the first section's offset (shift: + lower, − higher),
    // row 0's offset inside its section (inner; null while row 0 is not built), and the blank a list below its
    // home leaves in view above it (lead) — and the cleanup entry's overlap with the list.
    probe.sample = () => {
      const bounds = scroller.getBoundingClientRect();
      const scroll = read();
      const rows = {};
      let first = null;
      for (const node of document.querySelectorAll("#worktrees .wt-node[data-row-key]")) {
        const box = node.getBoundingClientRect();
        if (box.top < bounds.top - 0.5 || box.bottom > bounds.bottom + 0.5) continue;
        rows[node.dataset.rowKey] = box.top;
        first ??= node.dataset.rowKey;
      }
      let blank = 0;
      for (const pad of document.querySelectorAll("#worktrees .wt-window-pad")) {
        const box = pad.getBoundingClientRect();
        blank += Math.max(0, Math.min(box.bottom, bounds.bottom) - Math.max(box.top, bounds.top));
      }
      const head = section();
      const headTop = head ? head.getBoundingClientRect().top : null;
      const shift = head && probe.home ? headTop - (bounds.top + probe.home.section - scroll) : null;
      const row0Top = zero()?.getBoundingClientRect().top ?? null;
      const inner = row0Top !== null && shift !== null
        ? row0Top - (bounds.top + probe.home.row0 - scroll) - shift
        : null;
      const lead = shift !== null && shift > 0
        ? Math.max(0, Math.min(headTop, bounds.bottom) - Math.max(headTop - shift, bounds.top))
        : 0;
      const headPad = head?.querySelector(".worktree-list > .wt-window-pad") ?? null;
      const entry = document.getElementById("wsclean-entry");
      const overlap = entry.hidden
        ? 0
        : Math.max(0, entry.getBoundingClientRect().bottom - document.getElementById("worktrees").getBoundingClientRect().top);
      return {
        at: performance.now(),
        top: scroll,
        rows,
        first,
        blank,
        shift,
        inner,
        lead,
        headPad: headPad ? headPad.getBoundingClientRect().height : null,
        entry: !entry.hidden,
        overlap,
      };
    };
    // Read after the frame is painted: a task queued from the frame's own callback runs after its paint.
    const channel = new MessageChannel();
    channel.port1.onmessage = () => {
      if (probe.recording) probe.frames.push(probe.sample());
    };
    const tick = () => {
      if (!probe.recording) return;
      channel.port2.postMessage(0);
      requestAnimationFrame(tick);
    };
    probe.start = () => {
      probe.recording = true;
      requestAnimationFrame(tick);
    };
    probe.stop = () => {
      probe.recording = false;
    };
  }, ROW0);
}

async function overScroller(page) {
  const point = await page.evaluate(() => {
    const box = document.getElementById("sidebar-scroll").getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  await page.mouse.move(point.x, point.y);
}

const clock = (page) => page.evaluate(() => performance.now());

// The first fully visible row: its key and top, and the scroll it was read at.
const firstShown = (page) => page.evaluate(() => {
  const frame = window.__PROBE__.sample();
  return frame.first === null ? null : { key: frame.first, top: frame.rows[frame.first], scroll: frame.top };
});

// Frames until the scroll leaves `from` (at most 20), then until it holds for three frames in a row.
const calm = (page, from = null) => page.evaluate((from) => new Promise((done) => {
  const probe = window.__PROBE__;
  let frames = 0;
  let still = 0;
  let last = probe.read();
  let moved = from === null || last !== from;
  const look = () => {
    frames += 1;
    const now = probe.read();
    moved ||= now !== from;
    still = now === last ? still + 1 : 0;
    last = now;
    if ((still >= 3 && (moved || frames >= 20)) || frames >= 240) done(now);
    else requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}), from);

// Wheel ticks up, one at a time, each read as the move of the row that was first fully visible before it.
async function stepUp(page, count) {
  const steps = [];
  for (let at = 0; at < count; at += 1) {
    const before = await firstShown(page);
    await page.mouse.wheel(0, -STEP);
    const after = await calm(page, before.scroll);
    const top = await page.evaluate((key) => {
      const node = document.querySelector(`#worktrees [data-row-key="${CSS.escape(key)}"]`);
      return node ? node.getBoundingClientRect().top : null;
    }, before.key);
    steps.push({
      key: before.key,
      moved: top === null ? null : round(top - before.top),
      scrolled: round(before.scroll - after),
      clamped: after <= 0.5,
    });
  }
  return steps;
}

// Wheel ticks TICK_MS apart; each of `during` runs once, after the tick its `after` names, given the scroll from
// before that tick.
async function burst(page, count, { dy = -STEP, during = [] } = {}) {
  const runs = [during].flat();
  for (let at = 0; at < count; at += 1) {
    const due = runs.filter((one) => one.after === at);
    const from = due.length ? await page.evaluate(() => window.__PROBE__.read()) : null;
    await page.mouse.wheel(0, dy);
    for (const one of due) await one.run(from);
    await page.waitForTimeout(TICK_MS);
  }
}

// Mid-burst, once the tick's scroll has landed and been painted: a second agent joins the row on the top edge. The
// growth is read in the frame that paints it — the row's height before and after, its top against the scroller's
// (− above), the ms since the last wheel event, whether the window counts the person as scrolling (null where the
// window keeps no such count), and the window's writes (px each) from the growth until that frame is painted. No
// wheel comes until it returns, so the row stays on the edge until it grows.
const growOnEdge = (page, from) => page.evaluate((from) => new Promise((done) => {
  const probe = window.__PROBE__;
  const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
  const tenth = (value) => Math.round(value * 10) / 10;
  // A task queued from a frame's callback runs after that frame is painted.
  const afterPaint = (then) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = then;
    channel.port2.postMessage(0);
  };
  let frames = 0;
  const pick = () => {
    frames += 1;
    if (probe.read() === from && frames < 20) {
      requestAnimationFrame(() => afterPaint(pick));
      return;
    }
    const node = [...document.querySelectorAll("#worktrees .wt-node[data-row-key]")].find((one) => {
      const box = one.getBoundingClientRect();
      return box.top < bounds.top && box.bottom > bounds.top;
    });
    if (!node) {
      done(null);
      return;
    }
    const before = node.getBoundingClientRect().height;
    const mark = probe.writes.length;
    window.__ADD_AGENT__(node.dataset.rowKey, "working");
    // The agent paint asked for this frame first, so its callback has run: the row has grown, and the window has
    // not seen it yet.
    requestAnimationFrame(() => {
      const at = performance.now();
      const box = node.getBoundingClientRect();
      const wheel = probe.events.filter((event) => event.type === "wheel").at(-1);
      const grown = {
        key: node.dataset.rowKey,
        before: tenth(before),
        grew: tenth(box.height),
        edge: tenth(box.top - bounds.top),
        sinceWheelMs: wheel ? tenth(at - wheel.at) : null,
        scrolling: typeof sidebarScrolling === "function" ? sidebarScrolling() : null,
      };
      afterPaint(() => {
        grown.writes = probe.writes.slice(mark).map((write) => tenth(write.after - write.before));
        done(grown);
      });
    });
  };
  requestAnimationFrame(() => afterPaint(pick));
}), from);

// A second agent joins the row the window holds nearest above the viewport: it grows above all that shows.
const growAbove = (page) => page.evaluate(() => {
  const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
  const node = [...document.querySelectorAll("#worktrees .wt-node[data-row-key]")]
    .filter((one) => one.getBoundingClientRect().bottom < bounds.top)
    .at(-1);
  if (!node) return null;
  window.__ADD_AGENT__(node.dataset.rowKey, "working");
  return { key: node.dataset.rowKey, before: Math.round(node.getBoundingClientRect().height * 10) / 10 };
});

const showEntry = (page, shown) => page.evaluate((shown) => {
  document.getElementById("wsclean-entry").hidden = !shown;
}, shown);

// At rest every spacer stands for its rows: a measured row at its height, an unmeasured one at the height the
// window counts it at (`sidebarRowGuesses` where the window keeps one, else the estimate).
const spacersHold = (page) => page.evaluate(() => {
  const guesses = typeof sidebarRowGuesses === "undefined" ? null : sidebarRowGuesses;
  const counted = (path) => sidebarRowHeights.get(path) ?? guesses?.get(path) ?? SIDEBAR_ROW_ESTIMATE;
  return window.__ANSWER__.project_catalog().map((project) => {
    const list = document.querySelector(`#worktrees [data-project-path="${CSS.escape(project.path)}"] .worktree-list`);
    if (!list) return { project: project.path, missing: true };
    const paths = project.worktrees.map((one) => one.path);
    const built = [...list.querySelectorAll(":scope > .wt-node[data-row-key]")]
      .map((node) => paths.indexOf(node.dataset.rowKey));
    const sum = (from, to) => paths.slice(from, to).reduce((total, path) => total + counted(path), 0);
    const top = list.firstElementChild.getBoundingClientRect().height;
    const bottom = list.lastElementChild.getBoundingClientRect().height;
    const start = built.length ? built[0] : null;
    const end = built.length ? built.at(-1) + 1 : null;
    const off = built.length
      ? Math.max(Math.abs(top - sum(0, start)), Math.abs(bottom - sum(end, paths.length)))
      : Math.abs(top + bottom - sum(0, paths.length));
    return {
      project: project.path,
      start,
      end,
      top: Math.round(top),
      bottom: Math.round(bottom),
      off: Math.round(off * 10) / 10,
      whole: built.every((at, i) => at === start + i),
    };
  });
});

const spacersStand = (spacers) => spacers.every((one) => !one.missing && one.whole && one.off <= 1);

// The list at rest at its top: the scroll, the first section's top spacer, row 0, and the list against its home.
const topAtRest = (page) => page.evaluate((key) => {
  const frame = window.__PROBE__.sample();
  return { scroll: frame.top, headPad: frame.headPad, row0: key in frame.rows, shift: frame.shift, inner: frame.inner };
}, ROW0);

const atHome = (frame) =>
  frame.shift !== null && frame.inner !== null && Math.abs(frame.shift) <= 0.5 && Math.abs(frame.inner) <= 0.5;

// For every row fully visible in two frames in a row: its move plus the person's scroll. Frame pairs that end
// with the list at its top are kept apart — the motion there is the top settling.
function rowsHeld(taken, from, to) {
  const frames = taken.frames.filter((frame) => frame.at >= from && frame.at <= to);
  let worst = 0;
  let worstAt = null;
  let atTop = 0;
  let blank = 0;
  let lead = 0;
  let pairs = 0;
  for (let at = 1; at < frames.length; at += 1) {
    const was = frames[at - 1];
    const now = frames[at];
    const wrote = taken.writes
      .filter((write) => write.at > was.at && write.at <= now.at)
      .reduce((sum, write) => sum + (write.after - write.before), 0);
    const scrolled = now.top - was.top - wrote;
    const top = now.top <= 0.5;
    for (const key of Object.keys(now.rows)) {
      if (!(key in was.rows)) continue;
      pairs += 1;
      const off = Math.abs(now.rows[key] - was.rows[key] + scrolled);
      if (top) {
        atTop = Math.max(atTop, off);
      } else if (off > worst) {
        worst = off;
        worstAt = { ms: Math.round(now.at - from), key, scroll: round(now.top) };
      }
    }
    blank = Math.max(blank, now.blank);
    lead = Math.max(lead, now.lead);
  }
  return {
    frames: frames.length,
    pairs,
    worst: round(worst),
    worstAt,
    atTop: round(atTop),
    blank: round(blank),
    lead: round(lead),
  };
}

function writesInScroll(taken, from, to) {
  const writes = taken.writes.filter((write) => write.at >= from && write.at <= to);
  const inside = writes.filter((write) =>
    taken.events.some((event) => !event.own && event.seq < write.seq && write.at - event.at < WRITE_WINDOW_MS));
  return {
    writes: writes.length,
    inside: inside.length,
    first: inside.slice(0, 3).map((write) => ({ ms: Math.round(write.at - from), by: round(write.after - write.before) })),
  };
}

// The list arriving at its top: when the scroll first stood at 0 and how the list stood against its home then, how
// long after that the list stood at its home (shortMs), and when row 0 first stood fully visible — against the
// last tick of the burst.
function topArrival(taken, from, to) {
  const frames = taken.frames.filter((frame) => frame.at >= from && frame.at <= to);
  const reached = frames.find((frame) => frame.top <= 0.5) ?? null;
  const settled = reached
    ? frames.find((frame) => frame.at >= reached.at && frame.top <= 0.5 && atHome(frame)) ?? null
    : null;
  const shown = frames.find((frame) => ROW0 in frame.rows) ?? null;
  const ticks = taken.events.filter((event) => event.type === "wheel" && event.at >= from && event.at <= to);
  return {
    reachedMs: reached ? Math.round(reached.at - from) : null,
    shift: round(reached?.shift ?? null),
    inner: round(reached?.inner ?? null),
    shortMs: reached && settled ? Math.round(settled.at - reached.at) : null,
    row0Ms: shown ? Math.round(shown.at - from) : null,
    lastTickMs: ticks.length ? Math.round(ticks.at(-1).at - from) : null,
  };
}

// With the scroll at 0: a list above its home, or row 0 not built, stops short of row 0; a list below its home,
// or row 0 below its place in its section, leaves a gap.
function topState(frame) {
  if (frame.top > 0.5 || frame.shift === null || atHome(frame)) return null;
  if (frame.inner === null || frame.shift < -0.5 || frame.inner < -0.5) return "short";
  return "gap";
}

// Every stretch of frames with the scroll at 0 and the list off its home — how long each lasted, up to the frame
// that ended it, and how far off it stood.
function topEpisodes(frames) {
  const episodes = [];
  let open = null;
  for (const frame of frames) {
    const kind = topState(frame);
    if (open) open.to = frame.at;
    if (open && open.kind !== kind) open = null;
    if (kind === null) continue;
    if (!open) {
      open = { kind, from: frame.at, to: frame.at, px: 0 };
      episodes.push(open);
    }
    open.px = Math.max(open.px, Math.abs(frame.shift), Math.abs(frame.inner ?? 0));
  }
  return episodes.map((one) => ({ kind: one.kind, ms: Math.round(one.to - one.from), px: round(one.px) }));
}

// The rows' move beyond the scroll from one frame to the next: the median over the rows fully visible in both, +
// down and − up (null with no row in both).
function rowsMove(taken, was, now) {
  const wrote = taken.writes
    .filter((write) => write.at > was.at && write.at <= now.at)
    .reduce((sum, write) => sum + (write.after - write.before), 0);
  const scrolled = now.top - was.top - wrote;
  return median(Object.keys(now.rows).filter((key) => key in was.rows).map((key) => now.rows[key] - was.rows[key] + scrolled));
}

// Every arrival at the top — the scroll reaching 0 from above — with the moves the rows made beyond the scroll in
// the frames that end there (one at most: a drop, +, while the person scrolls up, or a credit closing, −), and
// whether the list stood at its home in every top frame from the last of them on.
function topArrivals(taken, from, to) {
  const frames = taken.frames.filter((frame) => frame.at >= from && frame.at <= to);
  const arrivals = [];
  let open = null;
  for (let at = 1; at < frames.length; at += 1) {
    const now = frames[at];
    if (now.top > 0.5) {
      open = null;
      continue;
    }
    if (!open) {
      open = { ms: Math.round(now.at - from), moves: [], home: true };
      arrivals.push(open);
    }
    const move = rowsMove(taken, frames[at - 1], now);
    if (move !== null && Math.abs(move) > 1) {
      open.moves.push(round(move));
      open.home = true;
    }
    if (!atHome(now)) open.home = false;
  }
  return arrivals;
}

const arrivalHolds = (arrival) => arrival.moves.length <= 1 && arrival.home;

const renderNumbers = (renders) => ({
  calls: renders.length,
  p50Ms: round(median(renders)),
  p95Ms: round(quantile(renders, 0.95)),
  maxMs: round(renders.length ? Math.max(...renders) : null),
});

const stepsHold = (steps) =>
  steps.every((step) => step.clamped || (step.moved !== null && Math.abs(step.moved - STEP) <= 1));

const stood = (held) => held.rows.worst <= 1 && held.writes.inside === 0;

// ②'s conditions, each printed: the four the other passes hold, and the growth as it must land — the row on the top
// edge grew while the window counted the person as scrolling, and the window wrote nothing until that frame was
// painted.
const secondHolds = (second) => ({
  steps: stepsHold(second.steps),
  rows: second.held.rows.worst <= 1,
  writes: second.held.writes.inside === 0,
  blank: second.held.rows.blank <= 1,
  grew: Boolean(second.grown) && second.grown.grew > second.grown.before,
  edge: Boolean(second.grown) && second.grown.edge < 0,
  scrolling: second.grown?.scrolling === true,
  quiet: Boolean(second.grown) && second.grown.writes.length === 0,
});

export async function testSidebarScroll({ browser, origin, ok, engine = "chromium" }) {
  const faults = [];
  const pages = [];

  // The classifier on two traces, with no browser. WebKit's clock gives whole ms: a write the page saw after a wheel
  // event in the same ms is inside the person's scroll, and a settle write whose own scroll event follows it in the
  // same ms, past the window's rest (150 ms) after the last wheel, is not. Timestamps alone fail one of the two.
  const traced = (events, write) => writesInScroll({ writes: [write], events }, 0, Infinity).inside;
  const afterWheel = traced(
    [{ type: "wheel", at: 1000, seq: 1, own: false }],
    { at: 1000, seq: 2, before: 500, after: 400 },
  );
  const beforeOwnScroll = traced(
    [{ type: "wheel", at: 1000, seq: 1, own: false }, { type: "scroll", at: 1160, seq: 3, own: false }],
    { at: 1160, seq: 2, before: 400, after: 433 },
  );
  ok(
    "the mid-scroll classifier, with no browser: a write the page saw after a wheel event in the same ms is inside the person's scroll",
    afterWheel === 1,
    JSON.stringify({ inside: afterWheel }),
  );
  ok(
    "the mid-scroll classifier, with no browser: a write its own scroll event follows in the same ms, 160 ms after the last wheel, is not",
    beforeOwnScroll === 0,
    JSON.stringify({ inside: beforeOwnScroll }),
  );
  const stand = async (scene = {}) => {
    const { page } = await openWindowTestPage(browser, origin, { faults });
    await standScrollScene(page, scene);
    await installScrollProbes(page);
    await overScroller(page);
    await page.evaluate(() => window.__PROBE__.start());
    return page;
  };
  const gather = async (page) => {
    const taken = await page.evaluate(() => {
      const probe = window.__PROBE__;
      probe.stop();
      return { frames: probe.frames, writes: probe.writes, events: probe.events, renders: probe.renders };
    });
    pages.push(taken);
    return taken;
  };
  const held = (taken, span) => {
    const frames = taken.frames.filter((frame) => frame.at >= span.from && frame.at <= span.to);
    return {
      rows: rowsHeld(taken, span.from, span.to),
      writes: writesInScroll(taken, span.from, span.to),
      arrivals: topArrivals(taken, span.from, span.to),
      episodes: topEpisodes(frames),
      overlap: round(frames.reduce((most, frame) => Math.max(most, frame.overlap), 0)),
    };
  };
  const toEnd = (page) => page.evaluate(() => window.__PROBE__.place(document.getElementById("sidebar-scroll").scrollHeight));

  const first = {};
  const second = {};
  const bottom = {};
  const cleanup = {};
  let spacers = null;
  let fold = null;
  let reach = null;
  let windowed = null;
  const page = await stand();
  try {
    // ① The bottom first, at rest — the test's own move, not scored. Every row between the first screen and the
    // last has never been built.
    await toEnd(page);
    await calm(page);
    await page.waitForTimeout(REST_MS);
    first.from = await clock(page);
    first.steps = await stepUp(page, 12);
    await burst(page, 30);
    await calm(page);
    await page.waitForTimeout(REST_MS);
    first.to = await clock(page);

    // ② The same way up, over the rows ① measured; mid-burst a second agent joins the row on the top edge.
    await toEnd(page);
    await calm(page);
    await page.waitForTimeout(REST_MS);
    second.from = await clock(page);
    second.steps = await stepUp(page, 12);
    await burst(page, 30, {
      during: { after: 14, run: async (from) => { second.grown = await growOnEdge(page, from); } },
    });
    await calm(page);
    await page.waitForTimeout(REST_MS);
    second.to = await clock(page);
    if (second.grown) {
      second.grown.after = await page.evaluate((key) => sidebarRowHeights.get(key) ?? null, second.grown.key);
    }

    // ④ Down to the end.
    bottom.from = await clock(page);
    const left = await page.evaluate(() => {
      const scroller = document.getElementById("sidebar-scroll");
      return scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
    });
    await burst(page, Math.ceil(left / STEP) + 8, { dy: STEP });
    await calm(page);
    await page.waitForTimeout(REST_MS);
    bottom.to = await clock(page);
    bottom.end = await page.evaluate((key) => {
      const scroller = document.getElementById("sidebar-scroll");
      const bounds = scroller.getBoundingClientRect();
      const box = document.querySelector(`#worktrees [data-row-key="${CSS.escape(key)}"]`)?.getBoundingClientRect() ?? null;
      const lists = [...document.querySelectorAll("#worktrees .worktree-list")];
      return {
        lastShown: box !== null && box.top >= bounds.top - 0.5 && box.bottom <= bounds.bottom + 0.5,
        atEnd: scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 1,
        bottomPad: lists.at(-1).lastElementChild.getBoundingClientRect().height,
      };
    }, LAST_ROW);

    // ⑤ At rest after ① to ④.
    spacers = await spacersHold(page);

    // ⑥ Deep in /sa, its header pinned to the top: fold it, then unfold it.
    await page.evaluate(() => window.__PROBE__.reach("/sa/wt-100"));
    await calm(page);
    await page.waitForTimeout(REST_MS);
    fold = await page.evaluate(async (restMs) => {
      const header = () => document.querySelector('.proj-row[data-project-path="/sa"]');
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
      const was = header().getBoundingClientRect().top;
      const pinned = Math.abs(was - bounds.top) <= 2;
      header().click();
      await wait(80);
      await window.__PAINTED__();
      await wait(restMs);
      const folded = header().getBoundingClientRect().top;
      const closed = closedProjects.has("/sa");
      header().click();
      await wait(80);
      await window.__PAINTED__();
      await wait(restMs);
      const unfolded = header().getBoundingClientRect().top;
      return { pinned, closed, open: !closedProjects.has("/sa"), was, folded, unfolded };
    }, REST_MS);

    // ⑦ From the top, a row the window has not built.
    await page.evaluate(() => window.__PROBE__.place(0));
    await calm(page);
    await page.waitForTimeout(REST_MS);
    reach = await page.evaluate(async () => {
      const key = "/sa/wt-150";
      const selector = `#worktrees [data-row-key="${CSS.escape(key)}"]`;
      const builtBefore = document.querySelector(selector) !== null;
      scrollSidebarToPath(key);
      await window.__PAINTED__();
      const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
      const box = document.querySelector(selector)?.getBoundingClientRect() ?? null;
      return {
        builtBefore,
        built: box !== null,
        shown: box !== null && box.top >= bounds.top - 0.5 && box.bottom <= bounds.bottom + 0.5,
      };
    });
    await page.waitForTimeout(REST_MS);
    reach.spacers = await spacersHold(page);

    // ⑧ The window's size, and a node that stays in it.
    windowed = await page.evaluate((overscan) => {
      const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
      const lists = [...document.querySelectorAll("#worktrees .worktree-list")].map((list) => {
        const rows = [...list.querySelectorAll(":scope > .wt-node")];
        const shown = rows.filter((node) => {
          const box = node.getBoundingClientRect();
          return box.bottom > bounds.top && box.top < bounds.bottom;
        }).length;
        return { rows: rows.length, shown, most: shown + 2 * overscan + 2 };
      });
      const frame = window.__PROBE__.sample();
      document.querySelector(`#worktrees [data-row-key="${CSS.escape(frame.first)}"]`).__kept = true;
      return { lists, kept: frame.first };
    }, 10);
    await burst(page, 3, { dy: STEP });
    await calm(page);
    await page.waitForTimeout(REST_MS);
    windowed.same = await page.evaluate(
      (key) => document.querySelector(`#worktrees [data-row-key="${CSS.escape(key)}"]`)?.__kept === true,
      windowed.kept,
    );

    // ⑨ Deep in /sa, mid-burst: a row above the viewport grows while the person scrolls (the window owes it), then
    // the cleanup entry appears, and hides again.
    await page.evaluate(() => window.__PROBE__.reach("/sa/wt-120"));
    await calm(page);
    await page.waitForTimeout(REST_MS);
    cleanup.from = await clock(page);
    await burst(page, 18, {
      during: [
        { after: 2, run: async () => { cleanup.grown = await growAbove(page); } },
        { after: 8, run: () => showEntry(page, true) },
        { after: 13, run: () => showEntry(page, false) },
      ],
    });
    await calm(page);
    await page.waitForTimeout(REST_MS);
    cleanup.to = await clock(page);
    if (cleanup.grown) {
      cleanup.grown.after = await page.evaluate((key) => sidebarRowHeights.get(key) ?? null, cleanup.grown.key);
    }

    const taken = await gather(page);
    first.held = held(taken, first);
    second.held = held(taken, second);
    bottom.held = held(taken, bottom);
    cleanup.held = held(taken, cleanup);
    cleanup.shown = taken.frames.filter((frame) => frame.entry && frame.at >= cleanup.from && frame.at <= cleanup.to).length;
  } finally {
    await page.close().catch(() => {});
  }

  // ③ Two cold lists, each placed at row 150 of /sa and left to rest before the burst.
  const atTheTop = async (whole) => {
    const top = {};
    const cold = await stand();
    try {
      await cold.evaluate(() => window.__PROBE__.reach("/sa/wt-150"));
      await calm(cold);
      await cold.waitForTimeout(REST_MS);
      top.from = await clock(cold);
      const distance = await cold.evaluate(() => window.__PROBE__.read());
      if (whole) {
        top.ticks = Math.ceil(distance / STEP) + 25;
        await burst(cold, top.ticks);
      } else {
        // Ticks that add up to the distance to the top: the last one lands exactly at 0, and nothing follows it.
        // Rounded up to the hundredth, so a fractional scroll cannot leave it a hair short of 0.
        const ticks = Math.floor(distance / STEP);
        const rest = Math.ceil((distance - ticks * STEP) * 100) / 100;
        top.ticks = ticks + (rest > 0 ? 1 : 0);
        await burst(cold, ticks);
        if (rest > 0) await cold.mouse.wheel(0, -rest);
      }
      await calm(cold);
      await cold.waitForTimeout(2 * REST_MS);
      top.to = await clock(cold);
      top.end = await topAtRest(cold);
      const taken = await gather(cold);
      top.held = held(taken, top);
      top.arrival = topArrival(taken, top.from, top.to);
    } finally {
      await cold.close().catch(() => {});
    }
    return top;
  };
  const past = await atTheTop(true);
  const fling = await atTheTop(false);

  // ⑩ A cold list whose catalog names as active a workspace the window never built, below rows never measured —
  // every one of them taller than the estimate — and the sidebar reveals it, as its crosshair does.
  const revealed = await (async () => {
    const cold = await stand({ active: "/sa/wt-180" });
    try {
      return await cold.evaluate(async ({ key, restMs }) => {
        const seek = () =>
          [...document.querySelectorAll("#worktrees .wt-row")].find((one) => one.dataset.worktreePath === key) ?? null;
        const builtBefore = seek() !== null;
        const named = activeWorktreePath === key;
        revealActiveWorktree();
        await new Promise((done) => setTimeout(done, restMs));
        await window.__PAINTED__();
        const row = seek();
        const bounds = document.getElementById("sidebar-scroll").getBoundingClientRect();
        const box = row?.getBoundingClientRect() ?? null;
        return {
          named,
          builtBefore,
          built: row !== null,
          shown: box !== null && box.top >= bounds.top - 0.5 && box.bottom <= bounds.bottom + 0.5,
          focused: row !== null && document.activeElement === row,
        };
      }, { key: "/sa/wt-180", restMs: REST_MS });
    } finally {
      await cold.close().catch(() => {});
    }
  })();

  const brief = (one) => ({
    steps: one.steps?.map((step) => step.moved),
    residual: one.held.rows,
    writes: one.held.writes,
  });
  ok(
    "① a cold first pass up the sidebar: each wheel tick moves the first visible row by 100 px, no visible row moves except by the scroll, no section shows a blank, and the window writes no scrollTop while the person scrolls",
    stepsHold(first.steps) && stood(first.held) && first.held.rows.blank <= 1,
    JSON.stringify({ engine, ...brief(first) }),
  );
  const secondConditions = secondHolds(second);
  ok(
    "② a second pass over measured rows, with the row on the top edge growing mid-burst: the same four hold, and the window took the growth while it counted the person as scrolling, with no write until that frame was painted",
    Object.values(secondConditions).every(Boolean),
    JSON.stringify({ engine, conditions: secondConditions, ...brief(second), grown: second.grown }),
  );
  ok(
    "③ a cold burst from row 150 that keeps ticking past the top stands row 0 fully visible while it still ticks, holds the rows everywhere else, and rests at the top with no gap",
    past.arrival.row0Ms !== null &&
      past.arrival.lastTickMs !== null &&
      past.arrival.row0Ms <= past.arrival.lastTickMs &&
      stood(past.held) &&
      past.held.arrivals.length > 0 &&
      past.held.arrivals.every(arrivalHolds) &&
      past.end.row0 &&
      past.end.scroll <= 0.5 &&
      atHome(past.end),
    JSON.stringify({ engine, ticks: past.ticks, arrival: past.arrival, arrivals: past.held.arrivals, residual: past.held.rows, writes: past.held.writes, end: past.end }),
  );
  ok(
    "③ a burst that ends exactly at the top, as a fling ends, stands the list at its home within 400 ms of reaching the top — how far it stood off its home on arrival (shift, inner), how far the rows moved there (atTop), and for how long (shortMs)",
    fling.arrival.reachedMs !== null &&
      fling.arrival.shortMs !== null &&
      fling.arrival.shortMs <= 400 &&
      stood(fling.held) &&
      fling.held.arrivals.length > 0 &&
      fling.held.arrivals.every(arrivalHolds) &&
      fling.end.row0 &&
      atHome(fling.end),
    JSON.stringify({ engine, ticks: fling.ticks, arrival: fling.arrival, arrivals: fling.held.arrivals, residual: fling.held.rows, writes: fling.held.writes, end: fling.end }),
  );
  ok(
    "④ a burst down to the end: the last row stands fully visible, the scroll is at its end and the bottom spacer is 0 px",
    bottom.end.lastShown && bottom.end.atEnd && bottom.end.bottomPad <= 0.5 && stood(bottom.held),
    JSON.stringify({ engine, end: bottom.end, residual: bottom.held.rows, writes: bottom.held.writes }),
  );
  ok(
    "⑤ at rest every spacer equals the heights of the rows it stands for, within 1 px",
    spacersStand(spacers),
    JSON.stringify({ engine, spacers }),
  );
  ok(
    "⑥ folding and unfolding the section under the hand keeps its header within 1 px",
    fold.pinned && fold.closed && fold.open && Math.abs(fold.folded - fold.was) <= 1 && Math.abs(fold.unfolded - fold.was) <= 1,
    JSON.stringify({ engine, ...fold }),
  );
  ok(
    "⑦ scrollSidebarToPath stands a row the window had not built, fully visible, and at rest the spacers equal their rows' heights",
    !reach.builtBefore && reach.built && reach.shown && spacersStand(reach.spacers),
    JSON.stringify({ engine, ...reach }),
  );
  ok(
    "⑧ each section builds at most its visible rows plus 10 on each side, and a row that stays in the window keeps its node",
    windowed.lists.every((one) => one.rows <= one.most) && windowed.same,
    JSON.stringify({ engine, ...windowed }),
  );
  ok(
    "⑨ the cleanup entry, appearing and hiding mid-scroll after a row above the viewport grew, never runs into the list, in any frame",
    Boolean(cleanup.grown) && cleanup.shown > 0 && cleanup.held.overlap <= 0.5,
    JSON.stringify({ engine, grown: cleanup.grown ?? null, shown: cleanup.shown, overlap: cleanup.held.overlap, residual: cleanup.held.rows }),
  );
  ok(
    "⑩ revealing the active workspace, a row never built below rows never measured, stands it built, fully visible and focused",
    revealed.named && !revealed.builtBefore && revealed.built && revealed.shown && revealed.focused,
    JSON.stringify({ engine, ...revealed }),
  );
  const cases = { first, second, bottom, cleanup, past, fling };
  const arrivals = Object.values(cases).flatMap((one) => one.held.arrivals);
  const moves = arrivals.flatMap((one) => one.moves);
  const episodes = Object.values(cases).flatMap((one) => one.held.episodes);
  ok(
    "every arrival at the top, over all cases: the rows move beyond the scroll at most once, and from then on the list stands at its home",
    arrivals.length > 0 && arrivals.every(arrivalHolds),
    JSON.stringify({ engine, arrivals }),
  );
  ok(
    "the numbers, with no page error: how often the list stood short of its home or left a gap at the top over all cases, each move at the top (drops with the person's scroll, credits closing against it), the widest blank above the list, the widest overlap, and the JS time of renderSidebarWindows",
    faults.length === 0,
    JSON.stringify({
      engine,
      short: episodes.filter((one) => one.kind === "short").length,
      gap: episodes.filter((one) => one.kind === "gap").length,
      episodes,
      arrivals: arrivals.length,
      drops: moves.filter((px) => px > 0),
      credits: moves.filter((px) => px < 0),
      atTop: Object.fromEntries(Object.entries(cases).map(([name, one]) => [name, one.held.rows.atTop])),
      lead: Math.max(...Object.values(cases).map((one) => one.held.rows.lead)),
      overlap: Math.max(...Object.values(cases).map((one) => one.held.overlap)),
      render: pages.map((one) => renderNumbers(one.renders)),
      faults: faults.slice(0, 3),
    }),
  );
  return { engine, cases: { ...cases, spacers, fold, reach, windowed, revealed }, episodes, pages };
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const engine = option("--engine", "chromium");
  const out = option("--json", null);
  const { files, origin } = await createWindowServer();
  const browser = engine === "webkit" ? await webkitType().launch({ headless: true }) : await launchWindowBrowser();
  const results = [];
  const ok = (name, pass, detail = "") => {
    results.push({ name, pass: !!pass, detail });
    console.log(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  };
  let trace = null;
  try {
    trace = await testSidebarScroll({ browser, origin, ok, engine });
  } finally {
    await browser.close();
    files.close();
  }
  const failed = results.filter((one) => !one.pass).length;
  console.log("");
  console.log(`${results.length - failed}/${results.length} passed (${engine})`);
  if (out) writeFileSync(out, `${JSON.stringify({ engine, results, trace }, null, 2)}\n`);
  process.exitCode = failed ? 1 : 0;
}
