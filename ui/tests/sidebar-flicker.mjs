import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { webkitType } from "./coordinator-desk-perf.mjs";

/* The left sidebar under a working window, counted (v1.1.149, t-106395).
 *
 * The person, 11-13 10:00 on v1.1.147: 「왼쪽 네비 판이 계속 깜박 거려」. Their window then: one project with a dozen
 * checkouts and ten agent panes at work, a build line running, two more projects whose coordinators show their
 * workers under them, and the conversation list on top reading 「작업 중 7」. Their sidebar settings, read from their
 * preferences: grouped by repository, sorted by name, the branch list (not the team view), the conversation list on,
 * the compact agent rows.
 *
 * The scene stands that window from the window's own roads over the stub backend — `project_catalog`,
 * `hook:agent`, `hook:activity`, `term:title`, `ledger:changed` over `ledger_agents` and `coordinator_seats` — and a
 * probe on the whole sidebar (`aside.threads`) counts, while one source of news plays:
 *   mutations   every record a MutationObserver hears, by region (the conversation list, the branch list, the nav rows)
 *               and by kind: a write whose value did not move ("same"), a real attribute or text change, nodes in and
 *               out; the working dots' twelve-step ring (`wt-spin-N`, the window's own clock) is counted apart;
 *   rows        the sidebar's keyed rows (a checkout, an agent row, a conversation row, a project) taken out and put in:
 *               a row put in under a key that a different node held is REPLACED — its hover, its focus and its
 *               transitions start again; a row taken out and put back is MOVED;
 *   motion      `animationstart` and `transitionrun` inside the sidebar, and (in the measuring mode) rows whose top
 *               moved between two frames, each block whose height moved (the conversation list, the nav rows, the
 *               list's top, each project) and the list's scroll;
 *   paints      the agent paint's flushes, the list's refreshes, the conversation list's paints.
 *
 * The suite (`testSidebarFlicker`) pins, in a few beats each:
 *   ① the window's beats over data that did not move — the agent paint, the list's refresh, the ledger's beat, the
 *      conversation list — write nothing to the sidebar but the working dots' ring step;
 *   ② one agent's tool line moving writes that agent's row and nothing else, and one checkout's git answer moving
 *      writes that checkout's row and nothing else;
 *   ③ a storm of tool calls from every working agent replaces no row, and is drawn at most once a frame;
 *   ④ pressing agent rows one after another keeps every row's node — the pressed one and the one that was in front;
 *   ⑤ an agent ending its turn and starting the next keeps its row's node in the branch list — the dot, the chip and
 *      the second line its state shapes are new, the row is not — and every working ring turns on the document's clock.
 *
 *   node ui/tests/sidebar-flicker.mjs [--seconds 30] [--engine webkit] [--json out.json]
 *        each source alone for that long, on a fresh page, with the per-frame row sampler on; the table the report
 *        quotes. Before and after are the same file run in two trees (`git archive <base> ui | tar -x -C <dir>`, copy
 *        this file in): it uses nothing but roads the window had before the fix. */

const ZERO = "/z";
const ZERO_TREES = ["/z/main", ...Array.from({ length: 11 }, (_, at) => `/z/wt-${String(at + 1).padStart(2, "0")}`)];
/* Ten agent panes in the first project: seven at work, three resting. */
const ZERO_PANES = [
  { term: 9001, path: "/z/main", agent: "claude", hook: "working" },
  { term: 9002, path: "/z/main", agent: "codex", hook: "done" },
  { term: 9003, path: "/z/wt-01", agent: "claude", hook: "working" },
  { term: 9004, path: "/z/wt-02", agent: "claude", hook: "working" },
  { term: 9005, path: "/z/wt-03", agent: "zo", hook: "working" },
  { term: 9006, path: "/z/wt-04", agent: "claude", hook: "done" },
  { term: 9007, path: "/z/wt-05", agent: "codex", hook: "working" },
  { term: 9008, path: "/z/wt-06", agent: "claude", hook: "working" },
  { term: 9009, path: "/z/wt-07", agent: "claude", hook: "working" },
  { term: 9010, path: "/z/wt-08", agent: "claude", hook: "done" },
];
/* Two more projects, each a coordinator with two workers under it — the team cards of the report. */
const TEAMS = [
  { project: "work", root: "/w", run: "run-w", lead: 9101, workers: [{ term: 9102, path: "/w/wt-w1", hook: "working" },
    { term: 9103, path: "/w/wt-w2", hook: "done" }] },
  { project: "biz", root: "/b", run: "run-b", lead: 9201, workers: [{ term: 9202, path: "/b/wt-b1", hook: "working" },
    { term: 9203, path: "/b/wt-b2", hook: "working" }] },
];
/* The checkout whose git answer moves in the dirty source. */
const DIRTY_PATH = "/z/wt-03";
const SOURCES = ["quiet", "beats", "activity", "titles", "turns", "dirty", "press"];

/* Stands the scene in `page`. Every helper it leaves waits for the window's own beat to have painted. */
export async function standFlickerScene(page) {
  await page.setViewportSize({ width: 1280, height: 1400 });
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async ({ zero, zeroTrees, zeroPanes, teams, dirtyPath }) => {
    if (typeof setLocale === "function") setLocale("ko", { persist: false });
    if (typeof setPanelWidth === "function") setPanelWidth("sidebar", 395);
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    window.__TELL__ = tell;
    window.__ANSWER__.set_sidebar_view = () => null;
    window.__ACTIVE__ = "/z/main";
    window.__ANSWER__.set_active_worktree = ({ path }) => {
      window.__ACTIVE__ = path;
      return path.split("/").pop();
    };
    window.__DIRTY__ = { dirty: false, ahead: 1 };
    const tree = (path, at, isMain) => ({
      path, branch: isMain ? "main" : `wt/${path.split("/").pop()}`, is_main: isMain, active: path === window.__ACTIVE__,
      is_folder: false, ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
      landing: { compare_ref: "origin/main", ignored: false, detached: false, far_behind: false, state: "unlanded",
        dirty: path === dirtyPath ? window.__DIRTY__.dirty : false, ahead: path === dirtyPath ? window.__DIRTY__.ahead : 1 },
    });
    const external = { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] };
    window.__ANSWER__.project_catalog = () => [
      { name: "zerocode", path: zero, external, worktrees: zeroTrees.map((path, at) => tree(path, at, at === 0)) },
      ...teams.map((team) => ({ name: team.project, path: team.root, external, worktrees: [
        tree(`${team.root}/main`, 0, true), ...team.workers.map((worker, at) => tree(worker.path, at + 1, false))] })),
    ];
    const rowOf = (over) => ({
      run: "", worker: "", agent: "claude", state: "working", ledger: "active", hearing: "pending", hearing_at: 1,
      checkout: "", task: "", task_id: "", reported: false, failed: false, settled: false, session: null, at: 1,
      review: { verified: false, merged: false, deployed: false, written: false, author: null }, review_since_ms: null,
      closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null, kept: null, taken_over: false,
      pane: "", stalled: [], ...over,
    });
    window.__LEDGER__ = teams.flatMap((team) => team.workers.map((worker, at) => rowOf({
      run: team.run, worker: `w-${worker.term}`, term: worker.term, checkout: worker.path, task: `${team.project} 과업 ${at + 1}`,
      task_id: `t-${worker.term}`, pane: `%${worker.term % 100}`,
    })));
    window.__SEATS__ = teams.map((team) => ({ run: team.run, name: `${team.project} 런`, seat: true, term: team.lead,
      held: true, generation: 1, vacated_ms: null, mail: 0, news: 0, landing: 0, batched: 0 }));
    window.__ANSWER__.coordinator_seats = () => window.__SEATS__;
    /* Each pane's last word, so a beat can say it again unchanged. */
    window.__PANES__ = new Map();
    const seat = (term, path, agent, hook, parent) => {
      tabs.push({ kind: "term", worktree: path, id: `tab-${term}`, layout: { type: "leaf", term } });
      paneAgents.set(term, agent);
      if (parent !== undefined) paneParents.set(term, parent);
      const payload = { term, state: hook, agent, session: `s-${term}`, model: "claude-opus-5-5" };
      window.__PANES__.set(term, payload);
      tell("hook:agent", { ...payload, state: "working" });
      if (hook !== "working") tell("hook:agent", payload);
    };
    for (const pane of zeroPanes) seat(pane.term, pane.path, pane.agent, pane.hook);
    for (const team of teams) {
      seat(team.lead, `${team.root}/main`, "claude", "working");
      for (const worker of team.workers) seat(worker.term, worker.path, "claude", worker.hook, team.lead);
    }
    setEveryProjectClosed(false);
    setSidebarView({ groupBy: "repo", sortBy: "name", projectOrder: "default", layout: "branches" });
    if (typeof chatListSidebar !== "undefined") {
      chatListSidebar = true;
      if (typeof paintChatListSidebar === "function") paintChatListSidebar();
    }
    await refreshWorktrees();
    const frame = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    window.__FRAME__ = frame;
    /* The window's beat after news: the ledger read, the agent paint's frame and its 150 ms floor. */
    window.__SETTLED__ = async () => {
      await frame();
      await new Promise((done) => setTimeout(done, 200));
      await frame();
    };
    tell("ledger:changed", Date.now());
    for (let tries = 0; tries < 100 && paneLedger.size < window.__LEDGER__.length; tries += 1) {
      await new Promise((done) => setTimeout(done, 20));
    }
    scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
    await window.__SETTLED__();
    await refreshWorktrees();
    await window.__SETTLED__();
  }, { zero: ZERO, zeroTrees: ZERO_TREES, zeroPanes: ZERO_PANES, teams: TEAMS, dirtyPath: DIRTY_PATH });
}

/* The probe, left on the page as `window.__FLICKER__`: `start({ sample })` begins a count, `stop()` ends it and
 * answers what it heard. */
export async function installFlickerProbe(page) {
  await page.evaluate(() => {
    const aside = document.querySelector("aside.threads") ?? document.body;
    const ROWS = ".wt-node, .wt-agent, .chat-row, .proj, .team-row, .chat-group";
    const keyOf = (node) => {
      if (node.matches(".wt-node")) return `checkout:${node.querySelector(".wt-row")?.dataset.worktreePath ?? "?"}`;
      if (node.matches(".wt-agent")) {
        return `agent:${node.dataset.term ?? ""}|${node.dataset.sub ?? ""}|${node.dataset.history ? "history" : ""}`
          + `|${node.dataset.crew !== undefined ? "crew" : ""}|${node.dataset.work ?? ""}`;
      }
      if (node.matches(".chat-row")) {
        return `chat:${node.dataset.term ?? node.querySelector(".chat-row-title")?.textContent ?? "?"}`;
      }
      if (node.matches(".chat-group")) return `group:${node.dataset.group ?? "?"}`;
      if (node.matches(".proj")) return `project:${node.dataset.projectPath ?? node.dataset.stateGroup ?? "resting"}`;
      return `team:${node.dataset.key ?? node.textContent.slice(0, 20)}`;
    };
    const regionOf = (node) => {
      const at = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
      if (!at) return "detached";
      if (at.closest("#chat-list")) return "chat";
      if (at.closest("#worktrees")) return "list";
      if (at.closest("#sidebar-team")) return "team";
      if (at.closest(".nav-rows")) return "nav";
      return "other";
    };
    const describe = (node) => {
      const at = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
      if (!at) return "?";
      const name = typeof at.className === "string" ? at.className.split(/\s+/).find(Boolean) : at.getAttribute?.("class")?.split(/\s+/)[0];
      return `${at.localName}${at.id ? `#${at.id}` : ""}${name ? `.${name}` : ""}`;
    };
    const rowsIn = (node) => {
      if (node.nodeType !== Node.ELEMENT_NODE) return [];
      const found = node.matches(ROWS) ? [node] : [];
      return [...found, ...node.querySelectorAll(ROWS)];
    };
    const SPIN = /\bwt-spin-\d+\b/g;
    let held = null;
    const observer = new MutationObserver((records) => held?.take(records));
    window.__FLICKER__ = {
      start({ sample = false } = {}) {
        const count = {
          records: [], animations: {}, transitions: {}, shifts: 0, scrolls: 0, frames: 0, heights: {},
          paints: { flush: 0, refresh: 0, chat: 0, agents: 0 },
        };
        count.take = (records) => count.records.push(...records);
        held = count;
        observer.observe(aside, { subtree: true, childList: true, attributes: true, attributeOldValue: true,
          characterData: true, characterDataOldValue: true });
        count.onAnimation = (event) => {
          count.animations[event.animationName] = (count.animations[event.animationName] ?? 0) + 1;
        };
        count.onTransition = (event) => {
          const key = `${describe(event.target)} ${event.propertyName}`;
          count.transitions[key] = (count.transitions[key] ?? 0) + 1;
        };
        aside.addEventListener("animationstart", count.onAnimation, true);
        aside.addEventListener("transitionrun", count.onTransition, true);
        /* The paints, counted where the window calls them by name. */
        count.restore = [];
        for (const [name, slot] of [["flushAgentPaint", "flush"], ["refreshWorktrees", "refresh"], ["paintChatList", "chat"],
          ["paintWorktreeAgents", "agents"]]) {
          const real = window[name];
          if (typeof real !== "function") continue;
          window[name] = function counted(...args) {
            count.paints[slot] += 1;
            return real.apply(this, args);
          };
          count.restore.push(() => { window[name] = real; });
        }
        /* Rows whose top moved between two frames while they stood, and the scroller's own moves. */
        const scroller = document.getElementById("sidebar-scroll");
        const tops = new WeakMap();
        const heights = new Map();
        let lastScroll = scroller?.scrollTop ?? 0;
        const look = () => {
          if (held !== count) return;
          count.frames += 1;
          if (sample) {
            for (const node of aside.querySelectorAll(".wt-node > .wt-row, .wt-agent, .chat-row")) {
              const top = node.getBoundingClientRect().top;
              const was = tops.get(node);
              if (was !== undefined && Math.abs(was - top) > 0.5) count.shifts += 1;
              tops.set(node, top);
            }
            const now = scroller?.scrollTop ?? 0;
            if (now !== lastScroll) count.scrolls += 1;
            lastScroll = now;
            // Each block's height (the coordinator's ask, 11-13): the conversation list, the nav rows, the list's
            // top under them, and every project — a block that grows or shrinks pushes every row under it.
            const blocks = [["chat", document.getElementById("chat-list")], ["nav", aside.querySelector(".nav-rows")],
              ["scroller-top", scroller], ...[...aside.querySelectorAll("#worktrees > .proj")].map((node) =>
                [`project:${node.dataset.projectPath ?? "resting"}`, node])];
            for (const [name, node] of blocks) {
              if (!node) continue;
              const box = node.getBoundingClientRect();
              const size = name === "scroller-top" ? box.top : box.height;
              const was = heights.get(name);
              if (was !== undefined && Math.abs(was - size) > 0.5) {
                const held = count.heights[name] ?? { changes: 0, most: 0 };
                held.changes += 1;
                held.most = Math.max(held.most, Math.round(Math.abs(was - size)));
                count.heights[name] = held;
              }
              heights.set(name, size);
            }
          }
          requestAnimationFrame(look);
        };
        requestAnimationFrame(look);
      },
      stop() {
        const count = held;
        count.take(observer.takeRecords());
        observer.disconnect();
        held = null;
        aside.removeEventListener("animationstart", count.onAnimation, true);
        aside.removeEventListener("transitionrun", count.onTransition, true);
        for (const undo of count.restore) undo();
        /* A value's next value is the next record's old value for the same node and name, or what it holds now. */
        const after = new Map();
        const nextOf = new Map();
        for (let at = count.records.length - 1; at >= 0; at -= 1) {
          const record = count.records[at];
          if (record.type === "childList") continue;
          const slot = `${record.type}|${record.attributeName ?? ""}`;
          let byNode = after.get(record.target);
          if (!byNode) after.set(record.target, (byNode = new Map()));
          const now = byNode.has(slot)
            ? byNode.get(slot)
            : record.type === "attributes" ? record.target.getAttribute(record.attributeName) : record.target.data;
          nextOf.set(record, now);
          byNode.set(slot, record.oldValue);
        }
        const kinds = { spin: 0, same: 0, attribute: 0, text: 0, added: 0, removed: 0 };
        const regions = {};
        const where = {};
        const touched = new Set();
        const out = new Map();
        const into = new Map();
        let rebuilds = 0;
        const note = (region, kind, label) => {
          regions[region] = (regions[region] ?? 0) + 1;
          where[label] = (where[label] ?? 0) + 1;
          kinds[kind] += 1;
        };
        for (const record of count.records) {
          const region = regionOf(record.target);
          if (record.type === "childList") {
            if (record.target.id === "worktrees" && record.removedNodes.length > 0 && record.addedNodes.length > 0) rebuilds += 1;
            for (const node of record.removedNodes) {
              note(region, "removed", `${region} − ${describe(node)} from ${describe(record.target)}`);
              for (const row of rowsIn(node)) if (!out.has(row)) out.set(row, keyOf(row));
            }
            for (const node of record.addedNodes) {
              note(region, "added", `${region} + ${describe(node)} into ${describe(record.target)}`);
              for (const row of rowsIn(node)) into.set(row, keyOf(row));
            }
            const row = record.target.nodeType === Node.ELEMENT_NODE ? record.target.closest(ROWS) : null;
            if (row) touched.add(keyOf(row));
            continue;
          }
          const was = record.oldValue;
          const now = nextOf.get(record);
          if (record.type === "attributes" && record.attributeName === "class"
            && SPIN.test(now ?? "") && (was ?? "").replace(SPIN, "") === (now ?? "").replace(SPIN, "")) {
            SPIN.lastIndex = 0;
            kinds.spin += 1;
            continue;
          }
          SPIN.lastIndex = 0;
          const kind = was === now ? "same" : record.type === "attributes" ? "attribute" : "text";
          note(region, kind, `${region} ${kind} ${describe(record.target)}${record.attributeName ? ` [${record.attributeName}]` : ""}`);
          const at = record.target.nodeType === Node.ELEMENT_NODE ? record.target : record.target.parentElement;
          const row = at?.closest?.(ROWS);
          if (row) touched.add(keyOf(row));
        }
        const outKeys = new Set(out.values());
        let moved = 0;
        let replaced = 0;
        const replacedKeys = {};
        const movedKeys = {};
        for (const [row, key] of into) {
          if (out.has(row)) {
            moved += 1;
            movedKeys[key] = (movedKeys[key] ?? 0) + 1;
          }
          else if (outKeys.has(key)) {
            replaced += 1;
            replacedKeys[key] = (replacedKeys[key] ?? 0) + 1;
          }
        }
        const top = Object.entries(where).sort((one, two) => two[1] - one[1]).slice(0, 12);
        return {
          mutations: count.records.length - kinds.spin, kinds, regions, top,
          rows: { touched: [...touched].sort(), out: out.size, in: into.size, moved, movedKeys, replaced, replacedKeys, rebuilds },
          animations: count.animations, transitions: count.transitions, shifts: count.shifts, scrolls: count.scrolls,
          heights: count.heights,
          frames: count.frames, paints: count.paints,
        };
      },
    };
  });
}

/* Plays one source of news for `ms` and answers what the sidebar did. `events` counts the news told. */
export async function playSource(page, source, { ms, sample = false }) {
  return page.evaluate(async ({ source, ms, sample, dirtyPath, zeroPanes }) => {
    const tell = window.__TELL__;
    const working = [...window.__PANES__.values()].filter((pane) => pane.state === "working");
    const timers = [];
    let events = 0;
    let beat = 0;
    const every = (period, act) => timers.push(setInterval(() => {
      beat += 1;
      act(beat);
    }, period));
    /* The person's press: an agent row of the first project, one after another, the way the window log shows it
     * (「watch main: [6] by person click」, a press every half second or so). */
    const pressed = [];
    const pressable = zeroPanes.filter((pane) => pane.path === "/z/main" || pane.hook === "working").map((pane) => pane.term);
    window.__FLICKER__.start({ sample });
    if (source === "beats") {
      // Everything the window does on its own beats, over data that did not move.
      every(250, (at) => {
        for (const pane of window.__PANES__.values()) tell("hook:agent", pane);
        events += window.__PANES__.size;
        if (at % 4 === 0) {
          tell("ledger:changed", Date.now());
          void refreshWorktrees();
          if (typeof scheduleSidebarTeamPaint === "function") scheduleSidebarTeamPaint();
          events += 2;
        }
      });
    } else if (source === "activity") {
      // Every working agent calls a tool every 100 ms — the backend's batch cadence for a card.
      every(100, (at) => {
        for (const pane of working) {
          tell("hook:activity", { pane: `term:${pane.term}`,
            activities: [{ seq: at, activity: { verb: "read", target: `src/file-${pane.term}-${at}.rs`, phase: "started" } }] });
          tell("hook:agent", pane);
          events += 2;
        }
      });
    } else if (source === "titles") {
      // Each working agent names its terminal again every two seconds.
      every(2000, (at) => {
        for (const pane of working) {
          tell("term:title", { term: pane.term, title: `✳ 과업 ${pane.term} 단계 ${at}` });
          events += 1;
        }
      });
    } else if (source === "turns") {
      // One agent ends its turn or starts the next, every 1.5 s, round the working ones.
      every(1500, (at) => {
        const pane = working[at % working.length];
        const next = { ...pane, state: window.__PANES__.get(pane.term).state === "working" ? "done" : "working" };
        window.__PANES__.set(pane.term, next);
        tell("hook:agent", next);
        events += 1;
      });
    } else if (source === "dirty") {
      // One checkout's git answer moves every second; the backend says so (`worktree:landing`).
      every(1000, (at) => {
        window.__DIRTY__ = { dirty: at % 2 === 1, ahead: 1 + at };
        tell("worktree:landing", { path: dirtyPath });
        events += 1;
      });
    } else if (source === "press") {
      every(600, (at) => {
        const term = pressable[at % pressable.length];
        const row = document.querySelector(`#worktrees .wt-agent[data-term="${term}"]:not([data-sub])`);
        if (!row) return;
        pressed.push({ term, node: row });
        row.click();
        events += 1;
      });
    }
    await new Promise((done) => setTimeout(done, ms));
    for (const timer of timers) clearInterval(timer);
    await window.__SETTLED__();
    const found = window.__FLICKER__.stop();
    // A pressed row's node is still the row of its pane once the beat after it has painted.
    found.pressedKept = pressed.filter(({ term, node }) => node.isConnected
      && document.querySelector(`#worktrees .wt-agent[data-term="${term}"]:not([data-sub])`) === node).length;
    found.pressed = pressed.length;
    found.events = events;
    found.source = source;
    found.ms = ms;
    return found;
  }, { source, ms, sample, dirtyPath: DIRTY_PATH, zeroPanes: ZERO_PANES });
}

/* The rows the scene draws, by kind — to say the scene stood. */
async function sceneRows(page) {
  return page.evaluate(() => {
    const shown = (node) => node && !node.hidden && node.getClientRects().length > 0;
    return {
      checkouts: [...document.querySelectorAll("#worktrees .wt-row")].filter(shown).length,
      agents: [...document.querySelectorAll("#worktrees .wt-agent")].filter(shown).length,
      crew: [...document.querySelectorAll("#worktrees .wt-agent.is-crew")].filter(shown).length,
      working: document.querySelectorAll("#worktrees .wt-agent.is-working").length,
      chat: [...document.querySelectorAll("#chat-list .chat-row")].filter(shown).length,
      chatShown: !document.getElementById("chat-list")?.hidden,
    };
  });
}

export async function testSidebarFlicker({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await standFlickerScene(page);
    await installFlickerProbe(page);
    const scene = await sceneRows(page);
    ok(
      "the scene stands: three projects, the first with twelve checkouts and ten agents, two coordinators' workers under them, and the conversation list",
      scene.checkouts >= ZERO_TREES.length + 3 && scene.agents >= 10 && scene.crew >= 1 && scene.chatShown && scene.chat > 0,
      JSON.stringify(scene),
    );
    // The workers listed before the ledger named them left the list when it did: conversations that leave take their
    // room with them — only a row a turn hid in a folded group is held (⑦).
    const bootRoom = await page.evaluate(() => document.getElementById("chat-list").style.minHeight);
    ok(
      "⑦0 conversations that leave the list as the ledger arrives hold no room — the list stands at its own height",
      bootRoom === "",
      JSON.stringify({ bootRoom }),
    );
    const beats = await playSource(page, "beats", { ms: 1200 });
    ok(
      "① the window's beats over data that did not move — agent news said again, the list refreshed, the ledger's beat — write nothing to the sidebar but the working dots' ring step",
      beats.events > 0 && beats.mutations === 0,
      JSON.stringify({ mutations: beats.mutations, kinds: beats.kinds, top: beats.top.slice(0, 6), paints: beats.paints }),
    );
    const one = await page.evaluate(async () => {
      window.__FLICKER__.start();
      window.__TELL__("hook:activity", { pane: "term:9004",
        activities: [{ seq: 1, activity: { verb: "bash", target: "cargo test -p zerocode-shell", phase: "started" } }] });
      await window.__SETTLED__();
      return window.__FLICKER__.stop();
    });
    const dirty = await page.evaluate(async () => {
      window.__FLICKER__.start();
      window.__DIRTY__ = { dirty: true, ahead: 7 };
      window.__TELL__("worktree:landing", { path: "/z/wt-03" });
      for (let tries = 0; tries < 50 && worktreeLandings.get("/z/wt-03")?.ahead !== 7; tries += 1) {
        await new Promise((done) => setTimeout(done, 20));
      }
      await window.__SETTLED__();
      return window.__FLICKER__.stop();
    });
    // The agent's card says what its agents last said (`data-said` on the card's list), so the card of 9004 —
    // checkout /z/wt-02 — is written with its row; nothing else is, and nothing is written with the value it had.
    const agentKeys = ["agent:9004||||", "checkout:/z/wt-02"];
    ok(
      "② one agent's tool line moving writes that agent's row (and its card's word) and nothing else; one checkout's git answer moving writes that checkout's row and nothing else; neither writes a value it already had",
      JSON.stringify(one.rows.touched) === JSON.stringify(agentKeys) && one.rows.replaced === 0 && one.kinds.same === 0
        && dirty.rows.touched.length === 1 && dirty.rows.touched[0] === "checkout:/z/wt-03" && dirty.rows.replaced === 0
        && dirty.kinds.same === 0,
      JSON.stringify({ activity: { touched: one.rows.touched, same: one.kinds.same, top: one.top.slice(0, 6) },
        dirty: { touched: dirty.rows.touched, same: dirty.kinds.same, top: dirty.top.slice(0, 6) } }),
    );
    const storm = await playSource(page, "activity", { ms: 1500 });
    ok(
      "③ a storm of tool calls from every working agent replaces no row and moves none, and is drawn at most once a frame",
      storm.events > 50 && storm.rows.replaced === 0 && storm.rows.moved === 0 && storm.rows.rebuilds === 0
        && storm.paints.flush <= storm.frames + 1 && storm.paints.flush * 4 < storm.events,
      JSON.stringify({ events: storm.events, frames: storm.frames, paints: storm.paints, rows: storm.rows, top: storm.top.slice(0, 6) }),
    );
    const press = await playSource(page, "press", { ms: 2500 });
    ok(
      "④ pressing agent rows one after another keeps every row's node — the pressed one and the one that was in front — rebuilds no list, and starts no ring again (the rows' dots stay theirs)",
      press.pressed >= 3 && press.pressedKept === press.pressed && press.rows.replaced === 0 && press.rows.rebuilds === 0
        && Object.keys(press.animations).length === 0,
      JSON.stringify({ pressed: press.pressed, kept: press.pressedKept, animations: press.animations, rows: press.rows,
        top: press.top.slice(0, 6) }),
    );
    const turns = await playSource(page, "turns", { ms: 3200 });
    // The working rows' rings after it: each a turn of the document's clock (`startTime` 0), so a dot the turn made
    // new stands at the angle every other ring stands at.
    const rings = await page.evaluate(() => [...document.querySelectorAll("#worktrees .wt-agent.is-working .wt-agent-dot")]
      .flatMap((dot) => dot.getAnimations()).map((turn) => turn.startTime));
    // A conversation row that leaves 「작업 중」 for the folded 「오늘 끝남」 and comes back is a row that went and came:
    // only the branch list's rows are held to their nodes.
    const replacedHere = Object.keys(turns.rows.replacedKeys).filter((key) => !key.startsWith("chat:"));
    ok(
      "⑤ an agent ending its turn and starting the next keeps its row's node in the branch list, and its ring turns on the document's clock",
      turns.events >= 2 && replacedHere.length === 0 && turns.rows.rebuilds === 0 && rings.length > 0
        && rings.every((at) => at === 0),
      JSON.stringify({ events: turns.events, replaced: turns.rows.replacedKeys, rebuilds: turns.rows.rebuilds, rings,
        top: turns.top.slice(0, 6) }),
    );
    // Every working agent naming its terminal again: the words move, no row is built — the conversation rows hold
    // their titles in place, and a coordinator's crew line holds its node.
    const titles = await playSource(page, "titles", { ms: 2300 });
    ok(
      "⑥ agents naming their terminals again write their rows' words in place — no conversation row, agent row or crew line is built again",
      titles.events > 0 && titles.rows.replaced === 0 && titles.rows.moved === 0 && titles.rows.rebuilds === 0,
      JSON.stringify({ events: titles.events, replaced: titles.rows.replacedKeys, moved: titles.rows.movedKeys,
        top: titles.top.slice(0, 6) }),
    );

    /* ⑦ The conversation list heads the column above the branch list: a turn's row leaving the open 「작업 중」 for the
     * folded 「오늘 끝남」 moved the whole branch list 43px on every turn. The list holds its height across turns,
     * grows for a new conversation, and gives the room back on the person's hand or once still with the pointer away. */
    await page.mouse.move(1200, 700);
    const held = await page.evaluate(async () => {
      const chat = document.getElementById("chat-list");
      const scroller = document.getElementById("sidebar-scroll");
      const at = () => Math.round(scroller.getBoundingClientRect().top);
      const head = () => chat.querySelector('.chat-group[data-group="working"] .chat-group-head');
      const flip = async (term, state) => {
        const next = { ...window.__PANES__.get(term), state };
        window.__PANES__.set(term, next);
        window.__TELL__("hook:agent", next);
        await window.__SETTLED__();
      };
      // From the list's own height: the person folds 「작업 중」 and opens it again.
      head().click();
      head().click();
      await window.__SETTLED__();
      const turning = [...window.__PANES__.values()].filter((pane) => pane.state === "working" && pane.term < 9100)
        .slice(0, 3).map((pane) => pane.term);
      const start = at();
      const tops = [];
      for (const term of turning) {
        await flip(term, "done");
        tops.push(at());
      }
      const room = chat.style.minHeight;
      const armed = typeof settleTimers !== "undefined" && settleTimers.has("chat-list-held");
      for (const term of turning) {
        await flip(term, "working");
        tops.push(at());
      }
      const turns = { start, tops, moves: tops.filter((top) => top !== start).length, room, armed };
      // A new conversation, while the list holds room for three hidden rows: the three come back into their room,
      // and the new one grows the list — once.
      for (const term of turning) await flip(term, "done");
      const beforeNew = at();
      for (const term of turning) await flip(term, "working");
      const backIn = at();
      const term = 9301;
      tabs.push({ kind: "term", worktree: "/z/wt-09", id: `tab-${term}`, layout: { type: "leaf", term } });
      paneAgents.set(term, "claude");
      const pane = { term, state: "working", agent: "claude", session: `s-${term}`, model: "claude-opus-5-5" };
      window.__PANES__.set(term, pane);
      window.__TELL__("hook:agent", pane);
      await window.__SETTLED__();
      const grown = { beforeNew, backIn, after: at() };
      // The person folds a group: the room is given back at once.
      await flip(turning[0], "done");
      const roomBeforeFold = chat.style.minHeight;
      head().click();
      const roomAfterFold = chat.style.minHeight;
      head().click();
      await window.__SETTLED__();
      // Held again for the still release below.
      await flip(turning[1], "done");
      return { turns, grown, fold: { roomBeforeFold, roomAfterFold }, stillRoom: chat.style.minHeight,
        stillTop: at(), released: typeof releaseChatListHeldAway === "function" };
    });
    // Still: the release waits while the pointer is over the sidebar and comes once it leaves.
    await page.mouse.move(120, 700);
    const hovered = await page.evaluate(() => {
      if (typeof releaseChatListHeldAway === "function") releaseChatListHeldAway();
      return document.getElementById("chat-list").style.minHeight;
    });
    await page.mouse.move(1200, 700);
    const away = await page.evaluate(async () => {
      await window.__SETTLED__();
      return { room: document.getElementById("chat-list").style.minHeight,
        top: Math.round(document.getElementById("sidebar-scroll").getBoundingClientRect().top) };
    });
    ok(
      "⑦a an agent ending its turn and starting the next moves the branch list not once — the conversation list keeps its height and the hidden row's room waits for it",
      held.turns.tops.length === 6 && held.turns.moves === 0 && held.turns.room !== "" && held.turns.armed,
      JSON.stringify(held.turns),
    );
    ok(
      "⑦b a new conversation grows the list at once — the rows that came back take their room without a move, the new one moves it once",
      held.grown.backIn === held.grown.beforeNew && held.grown.after > held.grown.backIn,
      JSON.stringify(held.grown),
    );
    ok(
      "⑦c the person folding a group gives the held room back at once",
      held.fold.roomBeforeFold !== "" && held.fold.roomAfterFold === "",
      JSON.stringify(held.fold),
    );
    ok(
      "⑦d once still, the room is given back only when the pointer has left the sidebar — then in one move",
      held.released && held.stillRoom !== "" && hovered === held.stillRoom && away.room === "" && away.top !== held.stillTop,
      JSON.stringify({ stillRoom: held.stillRoom, hovered, away, stillTop: held.stillTop }),
    );
  } finally {
    await page.close();
  }
}

/* The measuring mode: each source alone on a fresh page for `seconds`, the row sampler on. */
export async function measureSidebarFlicker({ seconds = 30, engine = "chromium" } = {}) {
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch({ headless: true });
  const out = { engine, seconds, sources: {} };
  try {
    for (const source of SOURCES) {
      const faults = [];
      const { page } = await openWindowTestPage(browser, origin, { faults });
      try {
        await standFlickerScene(page);
        await installFlickerProbe(page);
        if (!out.scene) out.scene = await sceneRows(page);
        out.sources[source] = await playSource(page, source, { ms: seconds * 1000, sample: true });
        if (faults.length) out.sources[source].faults = faults.slice(0, 3);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  return out;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argument = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at > 0 && at + 1 < process.argv.length ? process.argv[at + 1] : fallback;
  };
  const found = await measureSidebarFlicker({ seconds: Number(argument("--seconds", "30")), engine: argument("--engine", "chromium") });
  const json = argument("--json", null);
  if (json) writeFileSync(json, `${JSON.stringify(found, null, 2)}\n`);
  console.log(`engine ${found.engine} · ${found.seconds} s a source · scene ${JSON.stringify(found.scene)}`);
  console.log("source    events  writes  same  attr  text  +nodes −nodes spin  rows-touched replaced moved rebuilds anim  trans shifts flush refresh chat");
  for (const [source, one] of Object.entries(found.sources)) {
    const anim = Object.values(one.animations).reduce((sum, value) => sum + value, 0);
    const trans = Object.values(one.transitions).reduce((sum, value) => sum + value, 0);
    console.log([source.padEnd(9), String(one.events).padStart(6), String(one.mutations).padStart(7), String(one.kinds.same).padStart(5),
      String(one.kinds.attribute).padStart(5), String(one.kinds.text).padStart(5), String(one.kinds.added).padStart(6),
      String(one.kinds.removed).padStart(6), String(one.kinds.spin).padStart(5), String(one.rows.touched.length).padStart(12),
      String(one.rows.replaced).padStart(8), String(one.rows.moved).padStart(5), String(one.rows.rebuilds).padStart(8),
      String(anim).padStart(5), String(trans).padStart(6), String(one.shifts).padStart(6), String(one.paints.flush).padStart(5),
      String(one.paints.refresh).padStart(7), String(one.paints.chat).padStart(4)].join(" "));
  }
  for (const [source, one] of Object.entries(found.sources)) {
    console.log(`\n${source}: ${one.top.map(([label, value]) => `${value}× ${label}`).join(" · ")}`);
    if (one.rows.replaced) console.log(`  replaced: ${JSON.stringify(one.rows.replacedKeys)}`);
    if (one.rows.moved) console.log(`  moved: ${JSON.stringify(one.rows.movedKeys)}`);
    if (Object.keys(one.transitions).length) console.log(`  transitions: ${JSON.stringify(one.transitions)}`);
    if (Object.keys(one.animations).length) console.log(`  animations: ${JSON.stringify(one.animations)}`);
    if (Object.keys(one.heights).length) console.log(`  heights (changes, largest px): ${JSON.stringify(one.heights)}`);
    if (one.scrolls) console.log(`  scroll moves: ${one.scrolls}`);
    if (one.faults) console.log(`  faults: ${one.faults.join(" | ")}`);
  }
}
