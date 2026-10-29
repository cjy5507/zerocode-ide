/* A long conversation opens on its newest turns and reads up to its first
 * (t-82901). The person, 10-29: 「대화창이 마지막 부분만 보이는데… 안 보이니까
 * 너무 어색」 — a Codex pane whose view held the last chunk of its transcript,
 * two turns under 「↑ 이전 기록 · 끝부분만 보는 중」, and no way up.
 *
 * The backend here is the transcript the reads stand for: synthetic lines at
 * a synthetic stride, each turn carrying the place its line begins at, read
 * back from a place by turns the way `transcript_log_back` reads them
 * (`TRANSCRIPT_SAID_TURNS` said turns or `TRANSCRIPT_BACK_ROWS` turns of any
 * kind). Read off the drawn page: the first paint's said turns, the way up as
 * a button, a press and a push up each asking the page's own door once with
 * `before`, the rows standing above with the reader's row where it stood and
 * nothing twice, a result that came before its call joining it, the way up
 * gone at the first turn, the page's hold bounded and the way back to the
 * newest turns — for a pane's page (both treatments), a wire's and a
 * helper's — and no read at all for scrolling in between. */
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* What one read back gathers, as the backend's constants say it
 * (shell_runtime.rs). */
export const BACK = Object.freeze({ said: 40, rows: 200 });
/* The synthetic file: blocks of a person's ask, a thought with an answer and
 * a call on one line, the call's result, and the answer — three said turns,
 * six turns (five rows once the result joins its call). */
export const EARLIER_BLOCKS = 120;
const STRIDE = 1000;

/* The transcript, page-side: `window.__TRANSCRIPT__` reads it as the backend
 * would — on from a cursor, back from the end, back from a place — and counts
 * every ask (`__ASKS__`). */
function standTranscript({ blocks, back, stride, tailFrom = null }) {
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
  const backFrom = (end) => {
    let at = end / stride;
    const turns = [];
    let saidCount = 0;
    while (at > 0 && saidCount < back.said && turns.length < back.rows) {
      at -= 1;
      turns.unshift(...placed[at]);
      saidCount += placed[at].filter(said).length;
    }
    return { found: true, skipped: false, turns, start: at * stride, next: end, more: false, folded: at > 0 };
  };
  window.__ASKS__ = [];
  window.__TRANSCRIPT__ = {
    size,
    read(args) {
      window.__ASKS__.push({ after: args.after ?? null, before: args.before ?? null });
      if (Number.isFinite(args.before)) return backFrom(args.before);
      if (args.after === null || args.after === undefined) {
        // A tail that opens on the result of a call above it, when asked.
        if (tailFrom !== null) {
          const turns = placed.slice(tailFrom).flat();
          return { found: true, skipped: false, turns, start: tailFrom * stride, next: size, more: false, folded: true };
        }
        return backFrom(size);
      }
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

/* A press on the way up, and the paint that follows its read. */
async function pressEarlier(page) {
  await page.evaluate(async () => {
    const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
    list.scrollTop = 0;
    await new Promise((done) => requestAnimationFrame(done));
    // A page with no way up has nothing to press: the checks after say so.
    list.querySelector(":scope > .chat-earlier-door")?.click();
  });
  await page.waitForFunction(() => {
    const list = [...document.querySelectorAll(".helper-turns")].find((one) => one.offsetParent !== null);
    return list.__run.helper.reaching !== true;
  });
  await settle(page);
}

/* Long panes for the memory census (t-82901): `panes` terminal tabs, each
 * wearing its conversation on a transcript of `blocks` blocks whose result
 * lines weigh what this machine's long sessions' did (10-29: 847 turns in
 * 23.8 MiB, some 28 KiB a turn) — `resultBytes` a result line on disk, the
 * clipped detail the page is handed (16 Ki characters). The reads answer as
 * the page's backend does: the new one by turns, or — on a page with no way
 * up, the window before t-82901 — the last 256 KiB's whole lines, so the two
 * windows are measured each against its own backend. `toTop` presses the way
 * up until the first turn stands. */
export async function standLongPaneChats(page, { panes = 1, blocks = 400, resultBytes = 60 * 1024, toTop = false } = {}) {
  return page.evaluate(async ({ panes, blocks, resultBytes, back, toTop }) => {
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
      const byTurns = typeof readEarlierTurns === "function";
      if (Number.isFinite(args.before)) return backFrom(args.before);
      if (args.after === null || args.after === undefined) {
        if (byTurns) return backFrom(size);
        // The last chunk's whole lines (the window before t-82901).
        return answer(index(size - CHUNK), lines.length);
      }
      if (args.after >= size) return answer(lines.length, lines.length);
      // On from a cursor: whole lines within a chunk.
      const from = index(args.after);
      let to = from;
      while (to < lines.length && lines[to].at - lines[from].at < CHUNK) to += 1;
      return { ...answer(from, Math.max(to, from + 1)), more: to < lines.length };
    };
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
  }, { panes, blocks, resultBytes, back: BACK, toTop });
}

export async function testConversationEarlier(browser, origin, ok) {
  // ---- a pane's page, the timeline treatment
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openLongPane(page);
      const opened = await page.evaluate(readList);
      const asks = await page.evaluate(() => window.__ASKS__.slice());
      ok(`a long pane's conversation opens on its newest ${BACK.said} said turns, read back by turns from the end — not the last chunk's two or three`,
        asks[0]?.after === null && asks[0]?.before === null && opened.said === BACK.said && opened.last === `답 ${EARLIER_BLOCKS - 1}` &&
          opened.twice === 0 && opened.from > 0,
        JSON.stringify({ asks: asks.slice(0, 3), said: opened.said, first: opened.first, last: opened.last, from: opened.from }));
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
        pressed.said === 2 * BACK.said && pressed.twice === 0 && pressed.ordered && pressed.last === opened.last &&
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
        pushAsked === 1 && pushed.said === 3 * BACK.said && pushed.twice === 0,
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
      const back = await page.evaluate(readList);
      const backAsks = await page.evaluate((from) => window.__ASKS__.slice(from), backAsked);
      ok("the foot's button goes back to the newest turns: one read back from the end, the newest said turns and the way up again",
        backAsks.length === 1 && backAsks[0].after === null && backAsks[0].before === null && !back.cut &&
          back.said === BACK.said && back.last === `답 ${EARLIER_BLOCKS - 1}` && back.door?.first === true && back.twice === 0,
        JSON.stringify({ backAsks, cut: back.cut, said: back.said, last: back.last }));
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
      await openLongPane(page, { tailFrom: 100 * 4 + 2 });
      const opened = await page.evaluate(readList);
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
        opened.orphans === 1 && pressed.orphans === 0 && pills.joined === "out 100" && !pills.adjacent && pills.once && pressed.twice === 0,
        JSON.stringify({ opened: opened.orphans, pressed: pressed.orphans, pills }));
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
        const tail = window.__TRANSCRIPT__.read({ after: null });
        window.__ASKS__.length = 0;
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
      }, { shape: { blocks: EARLIER_BLOCKS, back: BACK, stride: STRIDE }, source: standTranscript.toString() });
      await settle(page);
      const opened = await page.evaluate(readList);
      await pressEarlier(page);
      const pressed = await page.evaluate(readList);
      const asks = await page.evaluate(() => ({ wire: window.__WIRE_ASKS__.slice(), pane: window.__ASKS__.length }));
      ok("a wire's page that continues a pane wears the way up, and reads up through its own door from where the pane's history began",
        opened.door?.first === true && asks.wire.length === 1 && asks.wire[0].id === 51 && asks.wire[0].before === seen.from &&
          pressed.said === 2 * BACK.said && pressed.twice === 0,
        JSON.stringify({ door: opened.door, asks, said: pressed.said }));
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
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
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
