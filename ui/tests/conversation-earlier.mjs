/* A long conversation opens on its newest turns and reads up to its first
 * (t-82901). The person, 10-29: 「대화창이 마지막 부분만 보이는데… 안 보이니까
 * 너무 어색」 — a Codex pane whose view held the last chunk of its transcript,
 * two turns under 「↑ 이전 기록 · 끝부분만 보는 중」, and no way up — and,
 * 17:2x, 「열면 바로 그려지지 않음」: a first read of 40 turns kept the page
 * empty while it read.
 *
 * The backend here is the transcript the reads stand for: synthetic lines at
 * a synthetic stride, each turn carrying the place its line begins at, read
 * back from the end within the last chunk (`FIRST_REACH`) and from a place by
 * turns (`PAGE_REACH`: `TRANSCRIPT_SAID_TURNS` said turns or
 * `TRANSCRIPT_BACK_ROWS` turns of any kind) the way `transcript_log_back`
 * reads them. Read off the drawn page: the newest chunk drawn before anything
 * above it is answered, then one read up filling the said turns above it, the
 * way up as a button, a press and a push up each asking the page's own door
 * once with `before`, the rows standing above with the reader's row where it
 * stood and nothing twice, a result that came before its call joining it,
 * the way up gone at the first turn, the page's hold bounded and the way back
 * to the newest turns — for a pane's page (both treatments), a wire's and a
 * helper's — and no read at all for scrolling in between. */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* What one read back gathers, as the backend's constants say it
 * (shell_runtime.rs): the said turns and the turns of any kind a read from a
 * place stops at, and the chunk a first read from the end keeps within. */
export const BACK = Object.freeze({ said: 40, rows: 200, chunk: 256 * 1024 });
/* The synthetic file: blocks of a person's ask, a thought with an answer and
 * a call on one line, the call's result, and the answer — three said turns,
 * six turns (five rows once the result joins its call). Each line weighs 16
 * KiB, so the last chunk holds a handful of turns, as the person's long
 * sessions' did. */
export const EARLIER_BLOCKS = 120;
const STRIDE = 16 * 1024;

/* The transcript, page-side: `window.__TRANSCRIPT__` reads it as the backend
 * would — on from a cursor, back from the end within its last chunk, back
 * from a place by turns — and counts every ask (`__ASKS__`). `holdAbove`
 * holds every read from a place until `__LET_ABOVE__()`, so a test reads the
 * page as it stands before anything above its newest turns is answered;
 * `__HOLD_ABOVE__()` holds the reads from then on the same way. A function
 * in `__NEXT_ABOVE__` answers the next read from a place in its stead, once —
 * a refusal it throws, or an answer that brings nothing to see. */
function standTranscript({ blocks, back, stride, tailFrom = null, holdAbove = false }) {
  const lines = [];
  const at0 = Date.parse("2026-10-29T04:00:00Z");
  for (let block = 0; block < blocks; block += 1) {
    const call = { call_id: `call-${block}`, name: "Bash", kind: "bash", input: `ls ${block}`, is_error: false };
    lines.push([{ role: "user", text: `부탁 ${block}` }]);
    lines.push([
      { role: "thinking", text: `**읽기 ${block}**` },
      { role: "assistant", text: `살펴봅니다 ${block}` },
      { role: "tool", text: `Bash · ls ${block}`, tool: call },
    ]);
    lines.push([{ role: "tool_result", text: `out ${block}`, tool: { call_id: call.call_id, name: "", input: "", is_error: false } }]);
    lines.push([{ role: "assistant", text: `답 ${block}` }]);
  }
  const placed = lines.map((turns, at) => turns.map((turn, one) => ({ ...turn, line: at * stride, at_ms: at0 + at * 1000 + one })));
  const size = lines.length * stride;
  const said = (turn) => turn.role === "user" || turn.role === "assistant";
  // Back from `end` a whole line at a time, until the said turns or the
  // turns of any kind are met — and, from the file's end, only the lines that
  // begin inside its last chunk.
  const backFrom = (end, floor = -Infinity) => {
    let at = end / stride;
    const turns = [];
    let saidCount = 0;
    while (at > 0 && (at - 1) * stride > floor && saidCount < back.said && turns.length < back.rows) {
      at -= 1;
      turns.unshift(...placed[at]);
      saidCount += placed[at].filter(said).length;
    }
    return { found: true, skipped: false, turns, start: at * stride, next: end, more: false, folded: at > 0 };
  };
  const tail = () => {
    // A tail that opens on the result of a call above it, when asked.
    if (tailFrom !== null) {
      const turns = placed.slice(tailFrom).flat();
      return { found: true, skipped: false, turns, start: tailFrom * stride, next: size, more: false, folded: true };
    }
    return backFrom(size, size - back.chunk);
  };
  let letAbove = null;
  let above = holdAbove ? new Promise((done) => { letAbove = done; }) : null;
  window.__LET_ABOVE__ = () => letAbove?.();
  window.__HOLD_ABOVE__ = () => {
    above = new Promise((done) => { letAbove = done; });
  };
  window.__NEXT_ABOVE__ = null;
  window.__ASKS__ = [];
  window.__TRANSCRIPT__ = {
    size,
    stride,
    // What the first read answers, asked by no page.
    tail,
    read(args) {
      window.__ASKS__.push({ after: args.after ?? null, before: args.before ?? null });
      if (Number.isFinite(args.before)) {
        const once = window.__NEXT_ABOVE__;
        window.__NEXT_ABOVE__ = null;
        const answer = () => (once ? once(args) : backFrom(args.before));
        return above ? above.then(answer) : answer();
      }
      if (args.after === null || args.after === undefined) return tail();
      if (args.after === 0) return { found: true, skipped: false, turns: placed.flat(), start: 0, next: size, more: false, folded: false };
      return { found: true, skipped: false, turns: [], start: args.after, next: size, more: false, folded: false };
    },
  };
}

/* What the page on screen shows of its conversation. */
function readList() {
  const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
  const rows = [...list.querySelectorAll(":scope > [data-turn]")];
  const said = rows.filter((row) => row.classList.contains("is-user") || row.classList.contains("is-assistant"));
  const words = said.map((row) => row.__turn?.text ?? "");
  const door = list.querySelector(":scope > .chat-earlier-door");
  const held = list.__run.helper;
  const keys = held.turns.map((turn) => turn.seq);
  return {
    said: said.length,
    rows: rows.length,
    words,
    twice: words.length - new Set(words).size,
    first: words[0] ?? null,
    last: words.at(-1) ?? null,
    door: door ? { button: door.tagName === "BUTTON" && door.type === "button", words: door.textContent, first: list.firstElementChild === door } : null,
    from: held.from,
    held: held.turns.length,
    cut: held.cut === true,
    ordered: keys.every((key, at) => at === 0 || key > keys[at - 1]),
    orphans: held.turns.filter((turn) => turn.role === "tool_result").length,
    briefing: list.querySelector(":scope > .is-briefing")?.__turn?.text ?? null,
    foot: (() => {
      const foot = list.parentElement.querySelector(":scope > .chat-foot-door");
      return foot ? { shown: foot.classList.contains("is-shown"), label: foot.getAttribute("aria-label") } : null;
    })(),
  };
}

/* The page on screen's list, its top row's place, and how far it stands from the list's top. */
function placeOfTopRow() {
  const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
  const top = list.getBoundingClientRect().top;
  const row = [...list.querySelectorAll(":scope > .helper-turn.is-assistant, :scope > .helper-turn.is-user")]
    .find((one) => one.getBoundingClientRect().bottom > top + 40);
  return row ? { words: row.__turn.text, offset: row.getBoundingClientRect().top - top } : null;
}

async function settle(page) {
  await page.evaluate(async () => {
    for (let at = 0; at < 4; at += 1) {
      await new Promise((done) => setTimeout(done, 20));
      await window.__PAINTED__();
    }
  });
}

/* A pane whose agent named a long transcript, its conversation view open. */
async function openLongPane(page, options = {}) {
  await page.evaluate(async ({ shape, source }) => {
    // eslint-disable-next-line no-new-func
    new Function("shape", `(${source})(shape)`)(shape);
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, "codex");
    hookStates.set(term, "idle");
    window.__ANSWER__.pane_log = (args) => window.__TRANSCRIPT__.read(args);
    await setPaneChat(term, true, { wire: false });
    window.__TERM__ = term;
  }, { shape: { blocks: EARLIER_BLOCKS, back: BACK, stride: STRIDE, ...options }, source: standTranscript.toString() });
  await settle(page);
}

/* Until no read from above is out for the page on screen. */
async function reached(page) {
  await page.waitForFunction(() => {
    const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
    return list.__run.helper.reaching !== true;
  });
}

/* What the first read answers: its said turns and where they begin. */
function tailOfTranscript() {
  const tail = window.__TRANSCRIPT__.tail();
  return { said: tail.turns.filter((turn) => turn.role === "user" || turn.role === "assistant").length, start: tail.start };
}

/* A press on the way up, and the paint that follows its read. */
async function pressEarlier(page) {
  await page.evaluate(async () => {
    const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
    list.scrollTop = 0;
    await new Promise((done) => requestAnimationFrame(done));
    // A page with no way up has nothing to press: the checks after say so.
    list.querySelector(":scope > .chat-earlier-door")?.click();
  });
  await reached(page);
  await settle(page);
}

/* A long transcript, page-side (t-82901): `blocks` blocks whose result lines
 * weigh what this machine's long sessions' did (10-29: 847 turns in 23.8 MiB,
 * some 28 KiB a turn) — `resultBytes` a result line on disk, the clipped
 * detail the page is handed (16 Ki characters). `window.__LONG__.read`
 * answers as the page's backend does: the first read, from the end, with the
 * last 256 KiB's whole lines — the window before t-82901 read the same — or,
 * with `tail: "turns"`, back from the end by turns as the first read did
 * from ecc719bcb until the page drew its newest chunk first; a read from a
 * place by turns, which only a page with a way up asks; and on from a cursor a chunk at a time. With `ipc`, an answer goes as
 * the backend's does — written out before anything is timed, parsed by the
 * page when it arrives — `readMs` later, by kind (`tail`, `before`, `after`):
 * the backend's own time for that read, measured apart (it is not the page's). */
export async function standLongTranscript(page, { blocks = 400, resultBytes = 60 * 1024, tail = "chunk", ipc = false, readMs = {} } = {}) {
  await page.evaluate(({ blocks, resultBytes, back, tail, ipc, readMs }) => {
    const CHUNK = 256 * 1024;
    const CLIPPED = 16 * 1024;
    const detail = "결과 줄 · fn main() { println!(\"hello\"); }\n".repeat(CLIPPED / 40).slice(0, CLIPPED);
    const lines = [];
    let offset = 0;
    const at0 = Date.parse("2026-10-29T04:00:00Z");
    const push = (turns, bytes) => {
      lines.push({ at: offset, turns: turns.map((turn, one) => ({ ...turn, line: offset, at_ms: at0 + lines.length * 1000 + one })) });
      offset += bytes;
    };
    for (let block = 0; block < blocks; block += 1) {
      const call = { call_id: `call-${block}`, name: "Read", kind: "read", input: JSON.stringify({ file_path: `src/module_${block}.rs` }), is_error: false };
      push([{ role: "user", text: `${block}번째 부탁: 이 모듈을 읽고 고칠 곳을 찾아 줘.` }], 900);
      push([
        { role: "thinking", text: `**읽기 ${block}**\n먼저 파일을 읽는다.` },
        { role: "assistant", text: `src/module_${block}.rs를 읽겠습니다.` },
        { role: "tool", text: `Read · src/module_${block}.rs`, tool: call },
      ], 2400);
      push([{ role: "tool_result", text: detail, tool: { call_id: call.call_id, name: "", input: "", is_error: false } }], resultBytes);
      push([{ role: "assistant", text: `${block}번째 답: 고칠 곳은 두 군데입니다. 첫째는 경계 검사, 둘째는 오류 문장입니다.` }], 1400);
    }
    const size = offset;
    const said = (turn) => turn.role === "user" || turn.role === "assistant";
    const index = (byte) => lines.findIndex((line) => line.at >= byte);
    const answer = (from, to, extra) => ({
      found: true, skipped: false, more: false, turns: lines.slice(from, to).flatMap((line) => line.turns),
      start: lines[from]?.at ?? size, next: to < lines.length ? lines[to].at : size, folded: from > 0, ...extra,
    });
    const backFrom = (end) => {
      let at = end >= size ? lines.length : index(end);
      const to = at;
      let saidCount = 0;
      let rows = 0;
      while (at > 0 && saidCount < back.said && rows < back.rows) {
        at -= 1;
        saidCount += lines[at].turns.filter(said).length;
        rows += lines[at].turns.length;
      }
      return answer(at, to);
    };
    const read = (args) => {
      if (Number.isFinite(args.before)) return backFrom(args.before);
      // The last chunk's whole lines.
      if (args.after === null || args.after === undefined) return answer(index(size - CHUNK + 1), lines.length);
      if (args.after >= size) return answer(lines.length, lines.length);
      // On from a cursor: whole lines within a chunk.
      const from = index(args.after);
      let to = from;
      while (to < lines.length && lines[to].at - lines[from].at < CHUNK) to += 1;
      return { ...answer(from, Math.max(to, from + 1)), more: to < lines.length };
    };
    const kind = (args) => (Number.isFinite(args.before) ? "before" : args.after === null || args.after === undefined ? "tail" : "after");
    const answerOf = (args) => (kind(args) === "tail" && tail === "turns" ? backFrom(size) : read(args));
    const written = new Map();
    const key = (args) => JSON.stringify([args.after ?? null, args.before ?? null]);
    const text = (args) => {
      if (!written.has(key(args))) written.set(key(args), JSON.stringify(answerOf(args)));
      return written.get(key(args));
    };
    window.__LONG__ = {
      size,
      read(args) {
        if (!ipc) return answerOf(args);
        const sent = text(args);
        return new Promise((done) => setTimeout(() => done(JSON.parse(sent)), readMs[kind(args)] ?? 0));
      },
      // The answers a page opening on the file asks, written out ahead.
      warm() {
        const first = answerOf({ after: null });
        text({ after: null });
        text({ after: first.next });
        if (first.start > 0) text({ before: first.start });
        return { start: first.start, rows: first.turns.length };
      },
    };
  }, { blocks, resultBytes, back: BACK, tail, ipc, readMs });
}

/* Long panes for the memory census (t-82901): `panes` terminal tabs, each
 * wearing its conversation on the long transcript (`standLongTranscript`).
 * `toTop` presses the way up until the first turn stands. */
export async function standLongPaneChats(page, { panes = 1, blocks = 400, resultBytes = 60 * 1024, toTop = false, helper = false } = {}) {
  await standLongTranscript(page, { blocks, resultBytes });
  return page.evaluate(async ({ panes, toTop, helper }) => {
    const { size, read } = window.__LONG__;
    if (helper) {
      // A helper's page on the same file: read on from its first line to its
      // end, the page holding its bound of the newest turns — what a window
      // without a way up could already hold of such a file.
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      paneAgents.set(term, "claude");
      hookStates.set(term, "working");
      window.__ANSWER__.subagent_log = (args) => read(args);
      await openHelperPage({ term, tab: owner, worktree: owner.worktree, agent: "claude" }, { id: "long-helper", name: "긴 도우미", state: "running" });
      for (let beat = 0; beat < 200 && activeHelperPage()?.worker.helper.next < size; beat += 1) {
        await pollHelperPages();
        await window.__PAINTED__();
      }
      const shown = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
      const page = tabs.find((tab) => tab.id === `helper:${term}:long-helper`);
      return { panes: 0, transcriptMiB: Math.round((size / 1048576) * 10) / 10, heldTurns: [page?.worker.helper.turns.length ?? 0],
        presses: 0, rowsDrawn: shown?.querySelectorAll(":scope > [data-turn]").length ?? 0 };
    }
    const terms = [];
    for (let pane = 0; pane < panes; pane += 1) {
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "codex");
      hookStates.set(term, "idle");
      terms.push(term);
    }
    window.__ANSWER__.pane_log = (args) => read(args);
    for (const term of terms) {
      setActiveTab(tabOfTerm(term).id);
      await setPaneChat(term, true, { wire: false });
      for (let beat = 0; beat < 6; beat += 1) {
        await new Promise((done) => setTimeout(done, 30));
        await window.__PAINTED__();
      }
    }
    let presses = 0;
    if (toTop) {
      const list = () => [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
      for (; presses < 200; presses += 1) {
        const door = list()?.querySelector(":scope > .chat-earlier-door");
        if (!door || door.hidden) break;
        door.click();
        while (list().__run.helper.reaching) await new Promise((done) => setTimeout(done, 10));
        await window.__PAINTED__();
      }
    }
    const shown = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
    const held = terms.map((term) => paneChats.get(term)?.run.helper.turns.length ?? 0);
    return { panes, transcriptMiB: Math.round((size / 1048576) * 10) / 10, heldTurns: held, presses,
      rowsDrawn: shown?.querySelectorAll(":scope > [data-turn]").length ?? 0 };
  }, { panes, toTop, helper });
}

/* Press to first turn on a long pane (t-82901; the person, 10-29 17:2x:
 * 「대화창이 최적화 안 된 것 같아, 열면 바로 그려지지 않음 — 터미널 전환
 * 시」). A Codex pane standing on its terminal, the long transcript behind it
 * (`standLongTranscript`, its answers parsed as they arrive and late by the
 * backend's own time for each read), the head's 대화 switch pressed, and from
 * the press:
 *   frameMs      the first frame drawn after it
 *   firstTurnMs  the first frame drawn with a row of the conversation in view
 *   inViewAtFrame the rows in view in the first frame (0: it stood empty)
 *   filledMs     the frame the page's rows last grew in, once its reads settle
 *                (a read from above after the first paint, if the page asks one)
 *   rows         the rows standing then
 * once for the first open (`first`) and `OPEN.again` times terminal → 대화
 * again (`again`, what the page already holds).
 *
 *   node ui/tests/conversation-earlier.mjs --open [--rounds 5] [--engine webkit] [--json out.json]
 *        [--tail chunk|turns] [--tail-ms N] [--before-ms N] [--after-ms N]
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/conversation-earlier.mjs --open        cpu4x (Chromium)
 * `ZEROCODE_WINDOW_UI=<a checkout's ui>` runs another window's page against the
 * same transcript, its first read answered as its own backend's was. */
export const OPEN = Object.freeze({ again: 3, patienceMs: 5000, settleFrames: 30, awayMs: 300 });

export async function measureLongPaneOpen(page, { tail = "chunk", readMs = {} } = {}) {
  await standLongTranscript(page, { tail, ipc: true, readMs });
  return page.evaluate(async ({ again, patienceMs, settleFrames, awayMs }) => {
    const warm = window.__LONG__.warm();
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, "codex");
    hookStates.set(term, "idle");
    // The person's pick, so nothing but the switch stands the conversation.
    paneChatOf(term).chosen = true;
    window.__ANSWER__.pane_log = (args) => window.__LONG__.read(args);
    paintViewToggle();
    await window.__PAINTED__();
    // The next frame, once it is on screen: a task posted from its callback
    // runs after it is drawn.
    const drawn = () => new Promise((done) => requestAnimationFrame(() => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => done(performance.now());
      channel.port2.postMessage(null);
    }));
    const shown = () => {
      const list = paneChats.get(term)?.host?.querySelector(".helper-turns");
      if (!list || !list.checkVisibility()) return { rows: 0, inView: 0 };
      const box = list.getBoundingClientRect();
      const rows = [...list.querySelectorAll(":scope > [data-turn]")];
      const inView = rows.filter((row) => {
        const at = row.getBoundingClientRect();
        return at.height > 0 && at.bottom > box.top && at.top < box.bottom;
      }).length;
      return { rows: rows.length, inView };
    };
    const press = async () => {
      const from = performance.now();
      el("view-toggle-chat").click();
      const frame = await drawn();
      const atFrame = shown();
      let turnAt = atFrame.inView > 0 ? frame : null;
      while (turnAt === null && performance.now() - from < patienceMs) {
        const at = await drawn();
        if (shown().inView > 0) turnAt = at;
      }
      // Until the rows stop growing and no read is out.
      let rows = shown().rows;
      let grewAt = turnAt;
      for (let still = 0; still < settleFrames && performance.now() - from < patienceMs;) {
        const at = await drawn();
        const now = shown().rows;
        const out = paneChats.get(term).run.helper.reaching === true;
        if (now !== rows) {
          rows = now;
          grewAt = at;
          still = 0;
        } else if (!out) {
          still += 1;
        }
      }
      return { frameMs: frame - from, firstTurnMs: turnAt === null ? null : turnAt - from, inViewAtFrame: atFrame.inView,
        filledMs: grewAt === null ? null : grewAt - from, rows };
    };
    const first = await press();
    const repeats = [];
    for (let round = 0; round < again; round += 1) {
      el("view-toggle-term").click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, awayMs));
      repeats.push(await press());
    }
    return { warm, first, again: repeats };
  }, OPEN);
}

/* What a press back to a long pane's conversation restyles (t-84136): the
 * elements Chromium recalculated a style for between the press on 대화 and the
 * frame that draws it, by the ids its trace names them by — and of those, the
 * ones standing in the conversation's rows that are not among the parts a page
 * behind the front holds still. Those the stylesheet names (`.is-behind` …
 * `animation-play-state: paused`, shell.css), read from it here, start again;
 * nothing else in a row has a reason to. The press back
 * that cost a long pane 46 ms at a 4x CPU restyled every span in the window
 * and matched the whole document again, for `hidden`s that changed elsewhere.
 * Chromium only: the trace is its own. */
export async function restyledOnReturn(browser, page) {
  await measureLongPaneOpen(page);
  await page.evaluate(async (awayMs) => {
    el("view-toggle-term").click();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, awayMs));
  }, OPEN.awayMs);
  await browser.startTracing(page, { categories: ["blink.user_timing", "devtools.timeline", "disabled-by-default-devtools.timeline.invalidationTracking"] });
  const rows = await page.evaluate(async () => {
    performance.mark("restyle-press");
    el("view-toggle-chat").click();
    await new Promise((done) => requestAnimationFrame(() => {
      const channel = new MessageChannel();
      channel.port1.onmessage = done;
      channel.port2.postMessage(null);
    }));
    performance.mark("restyle-drawn");
    const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility());
    return list?.querySelectorAll(":scope > [data-turn]").length ?? 0;
  });
  const trace = JSON.parse((await browser.stopTracing()).toString());
  const events = trace.traceEvents ?? trace;
  const at = (name) => events.find((one) => one.name === name && String(one.cat).includes("user_timing"))?.ts ?? null;
  const [from, to] = [at("restyle-press"), at("restyle-drawn")];
  const restyled = new Set(events
    .filter((one) => one.name === "StyleRecalcInvalidationTracking" && from !== null && to !== null && one.ts >= from && one.ts <= to)
    .map((one) => one.args?.data?.nodeId)
    .filter(Number.isFinite));
  // The parts a page behind the front holds still, as the stylesheet names
  // them: what each paused rule styles under `.is-behind`.
  const held = await page.evaluate(() => [...document.styleSheets].flatMap((sheet) => [...sheet.cssRules])
    .filter((rule) => rule.style?.animationPlayState === "paused" && rule.selectorText.includes(".is-behind "))
    .flatMap((rule) => rule.selectorText.split(/,(?![^(]*\))/).map((one) => one.split(".is-behind ").at(-1).replace(/::(before|after)$/, "").trim()))
    .join(", "));
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("DOM.enable");
  const inRows = [];
  for (const backendNodeId of restyled) {
    const node = await cdp.send("DOM.resolveNode", { backendNodeId }).catch(() => null);
    if (!node?.object?.objectId) continue;
    const { result } = await cdp.send("Runtime.callFunctionOn", {
      objectId: node.object.objectId,
      returnByValue: true,
      arguments: [{ value: held }],
      functionDeclaration: `function (held) {
        if (!(this instanceof Element) || !this.closest(".pane-chat .helper-turns > [data-turn]")) return null;
        return { name: this.tagName.toLowerCase() + (this.className ? "." + String(this.className).split(" ").join(".") : ""), held: held !== "" && this.matches(held) };
      }`,
    });
    if (result.value) inRows.push(result.value);
  }
  await cdp.detach();
  const still = inRows.filter((one) => !one.held);
  return { rows, held, restyled: restyled.size, inRows: inRows.length, still: still.length, names: [...new Set(still.map((one) => one.name))].slice(0, 6) };
}

/* Rounds of the open, each on a fresh page; medians. */
export async function measureLongPaneOpenRounds({ engine = "chromium", rounds = 5, tail = "chunk", readMs = {} } = {}) {
  const { webkitType } = await import("./conversation-perf.mjs");
  const { loadNote } = await import("./machine-load.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  const taken = [];
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width: 1440, height: 900 });
        taken.push({ ...(await measureLongPaneOpen(page, { tail, readMs })), faults: faults.slice(0, 3) });
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const median = (values) => {
    const kept = values.filter((one) => typeof one === "number").sort((a, b) => a - b);
    if (!kept.length) return null;
    const mid = Math.floor(kept.length / 2);
    return kept.length % 2 ? kept[mid] : (kept[mid - 1] + kept[mid]) / 2;
  };
  const sum = (hops) => Object.fromEntries(["frameMs", "firstTurnMs", "inViewAtFrame", "filledMs", "rows"].map((key) => [key, median(hops.map((hop) => hop[key]))]));
  return {
    engine, throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 0) || 1, rounds, tail, readMs, ui: process.env.ZEROCODE_WINDOW_UI ?? "this checkout",
    load: loadNote(), first: sum(taken.map((one) => one.first)), again: sum(taken.flatMap((one) => one.again)),
    faults: taken.flatMap((one) => one.faults).slice(0, 3), taken,
  };
}

export async function testConversationEarlier(browser, origin, ok) {
  // ---- a pane's page, the timeline treatment
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openLongPane(page, { holdAbove: true });
      const tail = await page.evaluate(tailOfTranscript);
      const first = await page.evaluate(readList);
      // The reads that name no cursor: the first, and what it asks above it
      // (the follow-up read on from its cursor is the poll's own).
      const firstReads = await page.evaluate(() => window.__ASKS__.filter((ask) => ask.after === null));
      ok(`a long pane's conversation stands on its newest turns — the last chunk's ${tail.said} said turns — before anything above them is answered`,
        firstReads[0]?.before === null && tail.said > 0 && tail.said < BACK.said && first.said === tail.said &&
          first.last === `답 ${EARLIER_BLOCKS - 1}` && first.twice === 0 && first.from === tail.start,
        JSON.stringify({ firstReads, said: first.said, tail, last: first.last, from: first.from }));
      ok("once they are drawn, the page asks its door once for what stands above them, back from where they begin",
        firstReads.length === 2 && firstReads[1].before === tail.start,
        JSON.stringify(firstReads));
      await page.evaluate(() => window.__LET_ABOVE__());
      await reached(page);
      await settle(page);
      const opened = await page.evaluate(readList);
      const foot = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        return Math.round(list.scrollHeight - list.scrollTop - list.clientHeight);
      });
      ok(`the said turns above stand over them — ${BACK.said} more, in order, none twice — and the reader stays at the newest`,
        opened.said === tail.said + BACK.said && opened.twice === 0 && opened.ordered && opened.last === first.last &&
          opened.words.slice(BACK.said).join("|") === first.words.join("|") && opened.from < tail.start && foot <= 2,
        JSON.stringify({ said: opened.said, first: opened.first, from: opened.from, foot }));
      ok("the way up is a button at the list's head, above every row, worded in the catalog's words",
        opened.door?.button === true && opened.door.first === true &&
          opened.door.words === (await page.evaluate(() => t("worker.earlierMore", "이전 기록 더 보기"))),
        JSON.stringify(opened.door));

      // Scrolling in between asks nothing — counted once the read the first
      // answer's paint asks after itself (160 ms) has come and gone.
      const quiet = await page.evaluate(async () => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        await new Promise((done) => setTimeout(done, 400));
        const asked = window.__ASKS__.length;
        for (let step = 0; step < 12; step += 1) {
          list.scrollTop = list.scrollHeight * (0.3 + 0.05 * step);
          list.dispatchEvent(new WheelEvent("wheel", { deltaY: step % 2 ? 120 : -120, bubbles: true }));
          await new Promise((done) => requestAnimationFrame(done));
        }
        await new Promise((done) => setTimeout(done, 60));
        return window.__ASKS__.length - asked;
      });
      ok("scrolling through the middle of the conversation asks the transcript nothing", quiet === 0, `asks: ${quiet}`);

      // A press reads what stands above, once, from where the page holds.
      const before = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        list.scrollTop = 0;
        return list.__run.helper.from;
      });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(done)));
      const stood = await page.evaluate(placeOfTopRow);
      const asked = await page.evaluate(() => window.__ASKS__.length);
      await pressEarlier(page);
      const pressed = await page.evaluate(readList);
      const pressAsks = await page.evaluate((from) => window.__ASKS__.slice(from), asked);
      const kept = await page.evaluate((words) => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        const row = [...list.querySelectorAll(":scope > [data-turn]")].find((one) => one.__turn?.text === words);
        return row ? row.getBoundingClientRect().top - list.getBoundingClientRect().top : null;
      }, stood?.words);
      ok("a press on the way up asks the pane's door once, back from the place the page holds from",
        pressAsks.length === 1 && pressAsks[0].before === before && pressAsks[0].after === null,
        JSON.stringify({ before, pressAsks }));
      ok(`the turns read from above stand over the rows — ${BACK.said} more said turns, in order, none twice`,
        pressed.said === opened.said + BACK.said && pressed.twice === 0 && pressed.ordered && pressed.last === opened.last &&
          pressed.words.slice(BACK.said).join("|") === opened.words.join("|") && pressed.from < before,
        JSON.stringify({ said: pressed.said, twice: pressed.twice, first: pressed.first, from: pressed.from }));
      ok("the row being read stays where it stood while the turns above it arrive",
        stood !== null && kept !== null && Math.abs(kept - stood.offset) <= 1,
        JSON.stringify({ stood, kept }));

      // A push up at the top asks the same, once for the push's repeats.
      const pushAsked = await page.evaluate(async () => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        list.scrollTop = 0;
        await new Promise((done) => requestAnimationFrame(done));
        const asked = window.__ASKS__.length;
        for (let push = 0; push < 3; push += 1) list.dispatchEvent(new WheelEvent("wheel", { deltaY: -120, bubbles: true }));
        return window.__ASKS__.length - asked;
      });
      await page.waitForFunction(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        return list.__run.helper.reaching !== true;
      });
      await settle(page);
      const pushed = await page.evaluate(readList);
      ok("a push up at the list's top reads up as the press does — one read for the push's three wheel turns",
        pushAsked === 1 && pushed.said === opened.said + 2 * BACK.said && pushed.twice === 0,
        JSON.stringify({ pushAsked, said: pushed.said }));

      // Up to the first turn: the hold stays bounded, and its newest end goes.
      for (let press = 0; press < 12; press += 1) {
        const at = await page.evaluate(() => [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null).__run.helper.from);
        if (!(at > 0)) break;
        await pressEarlier(page);
      }
      const top = await page.evaluate(readList);
      const cap = await page.evaluate(() => HELPER_TURN_CAP);
      ok("read up to its first line, the page holds the conversation's first turn as its brief and the way up is gone",
        top.from === 0 && top.first === "부탁 0" && top.briefing === "부탁 0" && top.door === null && top.twice === 0 && top.ordered,
        JSON.stringify({ from: top.from, first: top.first, briefing: top.briefing, door: top.door }));
      ok(`the page's hold stays at its bound of ${cap} — the newest turns, far from the reader up above, went, and the foot's button says it goes back to them`,
        top.held <= cap && top.rows <= cap && top.cut && top.last !== `답 ${EARLIER_BLOCKS - 1}` &&
          top.foot?.shown === true &&
          top.foot.label === (await page.evaluate(() => t("worker.toNewest", "최근 기록으로"))),
        JSON.stringify({ held: top.held, rows: top.rows, cut: top.cut, last: top.last, foot: top.foot }));
      const backAsked = await page.evaluate(() => window.__ASKS__.length);
      await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        list.parentElement.querySelector(":scope > .chat-foot-door")?.click();
      });
      await settle(page);
      await reached(page);
      await settle(page);
      const back = await page.evaluate(readList);
      const backReads = await page.evaluate((from) => window.__ASKS__.slice(from).filter((ask) => ask.after === null), backAsked);
      ok("the foot's button goes back to the newest turns as the page first opened: one read back from the end, one above it, the same said turns and the way up again",
        backReads.length === 2 && backReads[0].before === null && backReads[1].before === tail.start && !back.cut &&
          back.said === opened.said && back.last === `답 ${EARLIER_BLOCKS - 1}` && back.door?.first === true && back.twice === 0,
        JSON.stringify({ backReads, cut: back.cut, said: back.said, last: back.last }));
      ok("the long pane's page raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- the folded treatment, and a tail that opens on a result above its call
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await foldConversations(page);
      // The tail opens on block 100's result line: its call stands above.
      await openLongPane(page, { tailFrom: 100 * 4 + 2, holdAbove: true });
      const opened = await page.evaluate(readList);
      await page.evaluate(() => window.__LET_ABOVE__());
      await reached(page);
      await settle(page);
      const filled = await page.evaluate(readList);
      await pressEarlier(page);
      const pressed = await page.evaluate(readList);
      const pills = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        const rows = [...list.querySelectorAll(":scope > [data-turn]")];
        const members = rows.flatMap((row) => (row.__members ?? []).map((turn) => turn.seq));
        const held = list.__run.helper.turns.filter((turn) => turn.role === "tool" || turn.role === "thinking" || turn.role === "tool_result");
        const call = list.__run.helper.turns.find((turn) => turn.tool?.call_id === "call-100" && turn.role === "tool");
        return {
          adjacent: rows.some((row, at) => at > 0 && row.classList.contains("helper-group") && rows[at - 1].classList.contains("helper-group")),
          once: members.length === new Set(members).size && held.every((turn) => members.includes(turn.seq)),
          joined: call?.output ?? null,
        };
      });
      ok("folded, a tail that opened on a result shows it on its own until its call is read from above, and then the call holds it — no result left without its call, no pill beside a pill, every step in one pill once",
        opened.orphans === 1 && filled.orphans === 0 && pressed.orphans === 0 && pills.joined === "out 100" && !pills.adjacent && pills.once &&
          filled.twice === 0 && pressed.twice === 0,
        JSON.stringify({ opened: opened.orphans, filled: filled.orphans, pressed: pressed.orphans, pills }));
      ok("the folded page raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- a wire's page, continuing a pane: what stands above is that pane's transcript
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      const seen = await page.evaluate(async ({ shape, source }) => {
        // eslint-disable-next-line no-new-func
        new Function("shape", `(${source})(shape)`)(shape);
        const tail = window.__TRANSCRIPT__.tail();
        window.__ANSWER__.wire_start = (args) => ({ id: 51, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: "s-1" });
        window.__ANSWER__.wire_log = () => ({
          found: true, skipped: false, next: 0, turns: [], status: "idle", asks: [], live: [], agent: "claude",
          protocol: "claude-stream", model: null, models: [], mode: null, modes: [], commands: [], version: "2.1.280", tasks: [],
        });
        window.__ANSWER__.wire_stop = () => null;
        window.__WIRE_ASKS__ = [];
        window.__ANSWER__.wire_history = (args) => {
          window.__WIRE_ASKS__.push(args);
          return window.__TRANSCRIPT__.read({ before: args.before });
        };
        await openWirePage("claude", "/tmp/zerocode-window-test", { history: tail.turns, historyFrom: tail.start, historyEnd: tail.next });
        await pollHelperPages();
        await window.__PAINTED__();
        return { from: tail.start };
      }, { shape: { blocks: EARLIER_BLOCKS, back: BACK, stride: STRIDE, holdAbove: true }, source: standTranscript.toString() });
      await settle(page);
      const opened = await page.evaluate(readList);
      const tail = await page.evaluate(tailOfTranscript);
      const asked = await page.evaluate(() => window.__WIRE_ASKS__.slice());
      await page.evaluate(() => window.__LET_ABOVE__());
      await reached(page);
      await settle(page);
      const filled = await page.evaluate(readList);
      await pressEarlier(page);
      const pressed = await page.evaluate(readList);
      const asks = await page.evaluate(() => ({ wire: window.__WIRE_ASKS__.slice(), pane: window.__ASKS__.length }));
      ok("a wire's page that continues a pane stands on the pane's newest turns, then reads what stands above them through its own door, once, from where that history began",
        opened.said === tail.said && asked.length === 1 && asked[0].id === 51 && asked[0].before === seen.from &&
          filled.said === tail.said + BACK.said && filled.twice === 0,
        JSON.stringify({ opened: opened.said, tail, asked, filled: filled.said }));
      ok("the wire's page wears the way up, and a press reads up through its door from where the page holds",
        filled.door?.first === true && asks.wire.length === 2 && asks.wire[1].before === filled.from &&
          pressed.said === filled.said + BACK.said && pressed.twice === 0,
        JSON.stringify({ door: filled.door, asks, said: pressed.said }));
      ok("the wire's page raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- a helper's page: read from its first line, its oldest let go, then read up again
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.evaluate(async ({ shape, source }) => {
        // eslint-disable-next-line no-new-func
        new Function("shape", `(${source})(shape)`)(shape);
        const term = await openTermTab({ placement: "tab" });
        const owner = tabOfTerm(term);
        paneAgents.set(term, "claude");
        hookStates.set(term, "working");
        window.__ANSWER__.subagent_log = (args) => window.__TRANSCRIPT__.read(args);
        await openHelperPage({ term, tab: owner, worktree: owner.worktree, agent: "claude" }, { id: "long-helper", name: "긴 도우미", state: "running" });
        await pollHelperPages();
        await window.__PAINTED__();
      }, { shape: { blocks: EARLIER_BLOCKS, back: BACK, stride: STRIDE }, source: standTranscript.toString() });
      await settle(page);
      const opened = await page.evaluate(readList);
      const asked = await page.evaluate(() => window.__ASKS__.length);
      await pressEarlier(page);
      const pressed = await page.evaluate(readList);
      const asks = await page.evaluate((from) => window.__ASKS__.slice(from), asked);
      const cap = await page.evaluate(() => HELPER_TURN_CAP);
      const block = (words) => Number(String(words).split(" ").at(-1));
      // It opened holding its bound, so what comes from above takes the place
      // of its newest turns: its first said turn is an older one, its hold the same.
      ok("a helper's page that let its oldest turns go wears the way up and reads them back through its own door, from the line its first kept turn stands on — its hold still at its bound",
        opened.from > 0 && opened.held === cap && opened.door?.first === true && asks.length === 1 && asks[0].before === opened.from &&
          block(pressed.first) < block(opened.first) && pressed.held <= cap && pressed.cut && pressed.twice === 0 && pressed.ordered,
        JSON.stringify({ from: opened.from, door: opened.door, asks, first: [opened.first, pressed.first], held: [opened.held, pressed.held] }));
      ok("the helper's page raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- a read from above that brings nothing to see says what it did (t-84136)
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openLongPane(page);
      const door = () => page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        return list.querySelector(":scope > .chat-earlier-door .chat-earlier-words")?.textContent ?? null;
      });
      const words = await page.evaluate(() => ({
        more: t("worker.earlierMore", "이전 기록 더 보기"),
        failed: t("worker.earlierFailed", "이전 기록을 읽지 못했습니다 — 다시 읽기"),
        skipped: t("worker.earlierSkipped", "너무 긴 기록 한 줄을 건너뛰었습니다 — 이어서 보기"),
        empty: t("worker.earlierEmpty", "그 구간엔 보여 줄 턴이 없었습니다 — 이어서 보기"),
      }));
      const press = async (next) => {
        await page.evaluate((kind) => {
          window.__NEXT_ABOVE__ = kind === null ? null : (args) => {
            // A file replaced under the page, as the backend refuses it.
            if (kind === "failed") throw new Error(`the transcript is shorter than the place ${args.before} the page reads back from — it was replaced`);
            // One line passed over, too long to take — or records that draw no turn.
            const start = args.before - window.__TRANSCRIPT__.stride;
            return { found: true, skipped: kind === "skipped", turns: [], start, next: args.before, more: true, folded: start > 0 };
          };
        }, next);
        const before = await page.evaluate(readList);
        await pressEarlier(page);
        return { before, after: await page.evaluate(readList), words: await door() };
      };
      const failed = await press("failed");
      ok("a read from above the backend refuses leaves the page where it held and the way up says the read failed and that a press reads again",
        failed.words === words.failed && failed.after.from === failed.before.from && failed.after.said === failed.before.said,
        JSON.stringify({ words: failed.words, from: [failed.before.from, failed.after.from] }));
      const again = await press(null);
      ok("the press after it reads up as any press does, and the way up says what it does again",
        again.words === words.more && again.after.said === again.before.said + BACK.said && again.after.from < again.before.from,
        JSON.stringify({ words: again.words, said: [again.before.said, again.after.said] }));
      const skipped = await press("skipped");
      const marked = await page.evaluate(() => [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null).__run.helper.skipped);
      ok("a read that passed over a line too long to show says so on the way up, the page reads on from above that line, and it counts among what the page skipped",
        skipped.words === words.skipped && skipped.after.from < skipped.before.from && skipped.after.said === skipped.before.said && marked === true,
        JSON.stringify({ words: skipped.words, from: [skipped.before.from, skipped.after.from], marked }));
      // However narrow the page, those words stand on one line inside the
      // way up, and its tip says them whole.
      await page.setViewportSize({ width: 320, height: 800 });
      await settle(page);
      const narrow = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        const door = list.querySelector(":scope > .chat-earlier-door");
        const words = door.querySelector(".chat-earlier-words");
        const [box, wordsBox, listBox] = [door, words, list].map((one) => one.getBoundingClientRect());
        return { inside: box.left >= listBox.left - 1 && box.right <= listBox.right + 1 && wordsBox.right <= box.right + 1,
          line: wordsBox.height <= box.height, tip: door.dataset.tip, words: words.textContent };
      });
      await page.setViewportSize({ width: 1280, height: 860 });
      await settle(page);
      ok("in a narrow page those words stand on one line inside the way up, and its tip says them whole",
        narrow.inside && narrow.line && narrow.tip === words.skipped && narrow.words === words.skipped,
        JSON.stringify(narrow));
      const empty = await press("empty");
      ok("a read through records that draw no turn says so on the way up, and the page reads on from above them",
        empty.words === words.empty && empty.after.from < empty.before.from && empty.after.said === empty.before.said,
        JSON.stringify({ words: empty.words, from: [empty.before.from, empty.after.from] }));
      const onward = await press(null);
      ok("and a press after that reads the turns above, worded as the way up again",
        onward.words === words.more && onward.after.said === onward.before.said + BACK.said,
        JSON.stringify({ words: onward.words, said: [onward.before.said, onward.after.said] }));
      ok("the page that read nothing to see raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- a reader gone down to the newest turns while a read from above is out keeps them (t-84136)
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openLongPane(page);
      const cap = await page.evaluate(() => HELPER_TURN_CAP);
      // Up until one more read would pass the page's hold.
      let step = 0;
      for (let press = 0; press < 12; press += 1) {
        const before = (await page.evaluate(readList)).held;
        if (step > 0 && before + step > cap) break;
        await pressEarlier(page);
        step = (await page.evaluate(readList)).held - before;
      }
      const stood = await page.evaluate(async () => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        window.__HOLD_ABOVE__();
        list.scrollTop = 0;
        await new Promise((done) => requestAnimationFrame(done));
        list.querySelector(":scope > .chat-earlier-door").click();
        // Down to the newest turns while the read is out.
        list.scrollTop = list.scrollHeight;
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        return { top: list.scrollTop, gap: list.scrollHeight - list.scrollTop - list.clientHeight };
      });
      const before = await page.evaluate(readList);
      await page.evaluate(() => window.__LET_ABOVE__());
      await reached(page);
      await settle(page);
      const kept = await page.evaluate(readList);
      const now = await page.evaluate(() => {
        const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
        return { top: list.scrollTop, gap: list.scrollHeight - list.scrollTop - list.clientHeight };
      });
      ok("a read from above that would push the newest turns out of the page's hold while its reader is among them is not held: the newest turns, the reader's place and where the page reads up from all stay",
        step > 0 && before.held + step > cap && stood.gap <= 2 && kept.held === before.held && kept.last === `답 ${EARLIER_BLOCKS - 1}` &&
          !kept.cut && kept.from === before.from && Math.abs(now.top - stood.top) <= 1 && now.gap <= 2,
        JSON.stringify({ step, cap, held: [before.held, kept.held], last: kept.last, cut: kept.cut, from: [before.from, kept.from], stood, now }));
      // From the top, the same read is held, and the newest end goes.
      await pressEarlier(page);
      const up = await page.evaluate(readList);
      ok("pressed again from the top, the read is held, the newest turns far from the reader go and the foot's button says the way back",
        up.from < kept.from && up.held <= cap && up.cut && up.foot?.shown === true,
        JSON.stringify({ from: [kept.from, up.from], held: up.held, cut: up.cut, foot: up.foot }));
      ok("the page whose reader went down raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  // ---- a press back to a long pane's conversation restyles none of its rows (t-84136)
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.setViewportSize({ width: 1440, height: 900 });
      const seen = await restyledOnReturn(browser, page);
      ok(`turned back to from its terminal, a long pane's conversation of ${seen.rows} rows is drawn restyling none of them — only the parts held still behind it start again`,
        seen.rows > BACK.said && seen.held !== "" && seen.still === 0,
        JSON.stringify(seen));
      ok("the press back raised no errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href && process.argv.includes("--open")) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const readMs = Object.fromEntries(["tail", "before", "after"].map((kind) => [kind, Number(option(`--${kind}-ms`, "0"))]));
  const result = await measureLongPaneOpenRounds({ engine: option("--engine", "chromium"), rounds: Number(option("--rounds", "5")),
    tail: option("--tail", "chunk"), readMs });
  const ms = (value) => (value === null ? "—" : `${value.toFixed(1)} ms`);
  console.log(`${result.engine} · cpu ×${result.throttle} · ${result.ui} · first read ${result.tail} · backend ms ${JSON.stringify(readMs)} · ${result.rounds} rounds (medians) · ${result.load}`);
  for (const name of ["first", "again"]) {
    const one = result[name];
    console.log(`  ${name}: frame ${ms(one.frameMs)} · first turn ${ms(one.firstTurnMs)} (${one.inViewAtFrame} rows in view in the first frame) · filled ${ms(one.filledMs)} · ${one.rows} rows`);
  }
  if (result.faults.length) console.log(`  FAULTS ${result.faults.join(" | ")}`);
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`);
} else if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testConversationEarlier(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
