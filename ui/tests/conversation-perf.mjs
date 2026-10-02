/* 대화 뷰의 무게 — 400턴 전사 하나로 재는 다섯 수 (t-6323 B0).
 *
 * docs/design/agent-conversation-claude-code-grammar-20260915.md §10. 「숫자
 * 없는 최적화 금지」의 자다: 먼저 재고, 고치고, 같은 픽스처·같은 부하로 다시
 * 잰다. 전/후는 같은 이 파일로, 전은 기준 커밋의 `ui/`를 스냅샷으로 풀어 그
 * 안에서 돌린다(`git archive <ref> ui` → 이 파일을 복사 → 실행).
 *
 * 픽스처(`conversationFixture`)는 결정적이다 — 사람의 말 40·답 100·생각 60·
 * 도구 200(그중 diff 60, 긴 출력 40), 그리고 그림 10(사람이 붙인 것 5, 도구가
 * 돌려준 스크린숏 5). 도구 결과는 제 호출 행에 합류하므로 턴은 정확히 400이다.
 * 같은 종류의 끝난 걸음이 이어 서면 한 행으로 접히므로(t-15682) 목록의 행은
 * 그보다 적을 수 있다 — 접힌 행은 제 걸음들을 `__members`로 든다.
 *
 * 다섯 수:
 *   1. DOM 노드 — 목록 안의 요소 수와 문서 전체의 요소 수, 그리고 CDP
 *      `Memory.getDOMCounters`의 노드(글 노드 포함, 떨어진 것까지).
 *   2. JS 힙 — CDP `HeapProfiler.collectGarbage` 두 번 뒤 `Runtime.getHeapUsage`.
 *      대화를 열기 전의 힙을 빼서 대화의 몫만 남긴다.
 *   3. 렌더러 RSS — 이 노드 프로세스가 띄운 브라우저의 렌더러(Chromium) 또는
 *      WebKit의 WebContent 프로세스를 `ps`로 읽는다. 설치 앱의 웹뷰는 WebKit이고,
 *      Playwright의 WebKit이 그 엔진의 대역이다(같은 픽스처, 같은 기계).
 *   4. 델타 한 번 그리기 — 선의 살아 있는 답에 델타 300개를 한 폴씩 먹이고,
 *      `paintLiveAnswerNow`(프레임당 한 번 도는 그리기)를 감싸 잰다. p50·p95.
 *   5. 스크롤 프레임 — 목록 위에서 휠 60번(프레임마다 한 번), 그동안의 rAF
 *      간격 중 16.7 ms를 넘은 비율(그리고 한 프레임을 통째로 놓친 20 ms 초과).
 *
 * 실행:
 *   node ui/tests/conversation-perf.mjs                    Chromium, 5판 중앙값
 *   node ui/tests/conversation-perf.mjs --engine webkit    WebKit(설치 앱 웹뷰의 엔진)
 *   node ui/tests/conversation-perf.mjs --rounds 3 --json out.json
 *
 * 사람이 느끼는 네 장면(열기·입력·스트리밍·누수, t-22095)은 같은 픽스처 위의
 * `--scenarios`다 — 아래 「사람이 느끼는 네 장면」.
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { cpus, loadavg } from "node:os";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* A 1×1 PNG — what an attached picture or a screenshot answers with when the
 * page asks for its bytes. The page's cost is the node and the fetch, not the
 * pixels. */
export const FIXTURE_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

const pad = (n) => String(n).padStart(3, "0");

function diffLines(at, count) {
  const lines = [];
  for (let line = 0; line < count; line += 1) {
    const kind = line % 5 === 0 ? "del" : line % 5 === 1 ? "add" : "ctx";
    lines.push({
      kind,
      text: `${kind === "del" ? "let" : "const"} value_${at}_${line} = compute(${line}, "${"x".repeat(line % 17)}");`,
      old: null,
      new: null,
    });
  }
  return lines;
}

function longOutput(at) {
  const lines = [];
  for (let line = 0; line < 200; line += 1) {
    lines.push(`test module_${at}::case_${pad(line)} ... ok (${(line * 7) % 97} ms)`);
  }
  return lines.join("\n");
}

function answerText(at) {
  return [
    `### 단계 ${at}`,
    "",
    `원인은 \`paint_${at}\`가 폴마다 목록을 다시 세운 것입니다. 고친 뒤에는 새 턴만 잇고, 조용한 폴은 DOM을 건드리지 않습니다.`,
    "",
    "- 첫째: 상한에서 지운 행만 앞에서 지운다",
    "- 둘째: 살아 있는 호출만 다시 입힌다",
    "- 셋째: 스크롤은 읽는 사람의 것이다",
    "",
    "```rust",
    `fn paint_${at}(list: &mut List) {`,
    "    for turn in list.fresh() {",
    "        list.push(row_of(turn));",
    "    }",
    "    list.settle();",
    "}",
    "```",
    "",
    `자세한 것은 ui/shell.js:${1000 + at}에 있습니다.`,
  ].join("\n");
}

function briefing(at) {
  const lines = [`# 브리핑 ${at}`, ""];
  for (let line = 0; line < 60; line += 1) {
    lines.push(`- 항목 ${line}: 이 줄은 긴 브리핑의 한 줄이고, 확장처럼 처음 몇 줄만 보이고 나머지는 접혀야 한다 (${line}).`);
  }
  return lines.join("\n");
}

/* The 400-row transcript, as the backend's turns: every call's result joins
 * its call (`holdHelperTurns`), so `turns` rows stand. Twenty blocks of
 * twenty: 2 said by the person, 5 answers, 3 thoughts, 10 calls. */
export function conversationFixture({ blocks = 20 } = {}) {
  const turns = [];
  let tool = 0;
  let user = 0;
  let at = Date.parse("2026-09-23T00:00:00Z");
  const stamp = () => {
    at += 1500;
    return at;
  };
  const call = () => {
    const id = `call-${pad(tool)}`;
    const kind = tool % 10;
    if (kind === 0 || kind === 5 || kind === 8) {
      const count = tool % 6 === 0 ? 120 : 24;
      turns.push({
        role: "tool", text: `Edit · src/module_${tool}.rs`, at_ms: stamp(),
        tool: { call_id: id, name: "Edit", kind: "edit", input: JSON.stringify({ file_path: `src/module_${tool}.rs` }), is_error: false,
          edits: [{ path: `src/module_${tool}.rs`, lines: diffLines(tool, count) }] },
      });
      turns.push({ role: "tool_result", text: `The file src/module_${tool}.rs has been updated.`, at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: false } });
    } else if (kind === 3 || kind === 7) {
      turns.push({
        role: "tool", text: `Bash · cargo test -p module_${tool}`, at_ms: stamp(),
        tool: { call_id: id, name: "Bash", kind: "bash", input: `cargo test -p module_${tool}`, is_error: false },
      });
      turns.push({ role: "tool_result", text: longOutput(tool), at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: tool % 20 === 7 } });
    } else if (kind === 9 && tool % 40 === 9) {
      // A screenshot came back — the Computer Use shape: a picture and a line.
      turns.push({
        role: "tool", text: `mcp__computer-use__screenshot · display ${tool}`, at_ms: stamp(),
        tool: { call_id: id, name: "mcp__computer-use__screenshot", kind: "mcp__computer-use__screenshot", input: "{}", is_error: false },
      });
      turns.push({ role: "tool_result", text: "screenshot taken", at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: false },
        images: [{ media_type: "image/png", at: `wire:${tool}` }] });
    } else {
      const [name, reduced] = [["Read", "read"], ["Grep", "grep"], ["Glob", "grep"]][tool % 3];
      turns.push({
        role: "tool", text: `${name} · src/module_${tool}.rs`, at_ms: stamp(),
        tool: { call_id: id, name, kind: reduced, input: JSON.stringify({ file_path: `src/module_${tool}.rs`, offset: 10 + tool }), is_error: false },
      });
      turns.push({ role: "tool_result", text: `${40 + tool} lines\nfn main() {}\n// …`, at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: false } });
    }
    tool += 1;
  };
  for (let block = 0; block < blocks; block += 1) {
    const said = user % 10 === 0 ? briefing(user) : `${user}번째 부탁: 이 파일을 고치고 시험을 돌려줘.`;
    const images = user % 8 === 4 ? [{ media_type: "image/png", at: `wire:${1000 + user}` }] : undefined;
    turns.push({ role: "user", text: said, at_ms: stamp(), ...(images && { images }) });
    user += 1;
    turns.push({ role: "thinking", text: `**읽기 ${block}**\n먼저 파일을 읽고 무엇이 문제인지 본다.`, at_ms: stamp() });
    call();
    call();
    call();
    turns.push({ role: "assistant", text: answerText(block * 5), at_ms: stamp() });
    call();
    call();
    turns.push({ role: "thinking", text: `**고치기 ${block}**\n두 곳을 고친다.`, at_ms: stamp() });
    call();
    call();
    turns.push({ role: "assistant", text: answerText(block * 5 + 1), at_ms: stamp() });
    call();
    turns.push({ role: "assistant", text: answerText(block * 5 + 2), at_ms: stamp() });
    const again = user % 8 === 4 ? [{ media_type: "image/png", at: `wire:${1000 + user}` }] : undefined;
    turns.push({ role: "user", text: `${user}번째 부탁: 계속.`, at_ms: stamp(), ...(again && { images: again }) });
    user += 1;
    turns.push({ role: "thinking", text: `**확인 ${block}**\n시험이 통과하는지 본다.`, at_ms: stamp() });
    call();
    call();
    turns.push({ role: "assistant", text: answerText(block * 5 + 3), at_ms: stamp() });
    call();
    call();
    turns.push({ role: "assistant", text: answerText(block * 5 + 4), at_ms: stamp() });
  }
  return turns;
}

/* What each language's fence says in a streaming answer — synthetic, a line
 * or three, the shapes an agent's answer carries. */
const STREAM_FENCE_CODE = Object.freeze({
  js: "const cut = settledCut(text, text.length);\nif (cut > row.__settledEnd) paint(cut);",
  ts: "const cut: number = settledCut(text, text.length);\nif (cut > row.settledEnd) paint(cut);",
  rust: "fn settle(text: &str) -> usize {\n    text.rfind(\"\\n\\n\").map_or(0, |at| at + 2)\n}",
  python: "def settle(text: str) -> int:\n    return text.rfind(\"\\n\\n\") + 2",
  sh: "node ui/tests/conversation-perf.mjs --scenarios --engine webkit",
  json: "{ \"tokens\": 300, \"rate\": 50, \"fences\": [\"ts\", \"rust\"] }",
  diff: "- paint(text.slice(0, cut));\n+ paint(text.slice(row.settledEnd, cut));",
});

/* The fences of the stream the Hermes-shaped scenarios feed: one language a
 * block, the mix an answer about code carries. */
export const MIXED_STREAM_FENCES = Object.freeze(["ts", "rust", "python", "sh", "json", "diff"]);

/* The words a streaming answer grows through: a few paragraphs, a list and a
 * fence, long enough that the settled part keeps growing while it streams.
 * One fence language a block, in turn — `js` alone is the text B0's delta
 * paint (4) was measured on, so its number stays comparable. */
export function streamingAnswer({ fences = ["js"] } = {}) {
  const parts = [];
  for (let block = 0; block < 6; block += 1) {
    parts.push(`단락 ${block}: 스트리밍 중인 답은 델타가 도착한 다음 프레임에 화면에 있어야 한다. ` +
      "터미널과 확장은 둘 다 도착한 대로 그리고, 여기도 그렇다. 쓰는 중인 블록만 다시 그리고 닫힌 블록은 한 번 그린다.");
    parts.push("");
    parts.push("- 하나: 닫힌 블록은 markdown으로 한 번\n- 둘: 열린 블록도 markdown으로\n- 셋: 페이싱 없음");
    parts.push("");
    const language = fences[block % fences.length];
    parts.push(`\`\`\`${language}\n${STREAM_FENCE_CODE[language]}\n\`\`\``);
    parts.push("");
  }
  return parts.join("\n");
}

/* Open the fixture as a wire session's page — the one page that has both the
 * transcript and a live answer — with the session working, so the live answer
 * row and the status line stand. Returns once the page has painted.
 *
 * The wire's log answers what a test hands it: the live words in
 * `__PERF_LIVE__`, and the turns in `__PERF_TURNS__` once each (an answer
 * that closed). With `timed`, the open is measured as a person meets it —
 * from the moment the page is asked to open to the first frame painted after
 * its rows stand (`mountMs` when the rows stand, `firstPaintMs` when that
 * frame is on screen, `from` on the page's clock). */
export async function openFixtureConversation(page, turns, { timed = false } = {}) {
  return page.evaluate(async ({ history, png, timed }) => {
    window.__PERF_LIVE__ = [];
    window.__PERF_TURNS__ = [];
    window.__ANSWER__.wire_start = (args) => ({ id: 31, agent: args.agent, protocol: "claude-stream", version: "2.1.278", model: null, session: null });
    window.__ANSWER__.wire_log = () => ({
      found: true, skipped: false, next: 0, turns: window.__PERF_TURNS__.splice(0), status: "working", asks: [],
      live: window.__PERF_LIVE__, agent: "claude", protocol: "claude-stream",
      models: [], modes: [], commands: [], version: "2.1.278",
    });
    window.__ANSWER__.wire_stop = () => null;
    // A picture's bytes, when the page asks for one (A8's lazy load).
    window.__ANSWER__.wire_image = () => png;
    const from = performance.now();
    await openWirePage("claude", "/tmp/zerocode-window-test", { history });
    const mounted = performance.now();
    // What stood when the open returned — the paint below is the rows' only
    // if they did.
    const rowsAtMount = document.querySelectorAll("#worker-view .helper-turns > [data-turn]").length;
    // The frame after the rows stand, once it is on screen: a task posted
    // from inside the frame's callback runs after the frame is drawn.
    const painted = timed
      ? await new Promise((done) => requestAnimationFrame(() => {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => done(performance.now());
        channel.port2.postMessage(null);
      }))
      : null;
    await pollHelperPages();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 50));
    await window.__PAINTED__();
    return timed ? { from, mountMs: mounted - from, firstPaintMs: painted - from, rowsAtMount } : null;
  }, { history: turns, png: FIXTURE_PNG, timed });
}

/* One display frame at 60 Hz, and a frame that missed a whole one — the two
 * lines every frame number here is read against. */
const FRAME_MS = 16.7;
const MISSED_FRAME_MS = 20;

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};

const quantile = (values, q) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
};

/* The processes a `ps` sees, as rows. */
function processTable() {
  const said = execFileSync("ps", ["-axo", "pid=,ppid=,rss=,command="], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  return said.split("\n").filter(Boolean).map((line) => {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/);
    return match ? { pid: Number(match[1]), ppid: Number(match[2]), rss: Number(match[3]) * 1024, command: match[4] } : null;
  }).filter(Boolean);
}

/* The renderer this harness's page lives in. Chromium's renderers are the
 * children of the browser this node process started; the page's is the one
 * that appeared when the page was opened (`before` is the set that stood
 * then). WebKit's content processes are XPC services under launchd, known by
 * the Playwright build's own path. */
export function rendererRss(engine, before) {
  const rows = processTable();
  if (engine === "webkit") {
    const ours = rows.filter((row) => /ms-playwright\/webkit-\d+/.test(row.command) && /WebContent/.test(row.command) && !before.has(row.pid));
    return ours.reduce((most, row) => Math.max(most, row.rss), 0);
  }
  const browsers = rows.filter((row) => row.ppid === process.pid);
  const children = rows.filter((row) => browsers.some((one) => one.pid === row.ppid) && /--type=renderer/.test(row.command) && !before.has(row.pid));
  return children.reduce((most, row) => Math.max(most, row.rss), 0);
}

export function standingPids() {
  return new Set(processTable().map((row) => row.pid));
}

/* One page, one set of numbers. `cdp` is null on WebKit, whose heap and node
 * counts come from the DOM alone. */
export async function measureConversation(page, { engine = "chromium", before = new Set(), deltas = 300, wheels = 60 } = {}) {
  const cdp = engine === "chromium" ? await page.context().newCDPSession(page) : null;
  const gc = async () => {
    if (!cdp) return;
    await cdp.send("HeapProfiler.collectGarbage");
    await cdp.send("HeapProfiler.collectGarbage");
  };
  // V8's own heap and the embedder's (Blink's Oilpan, where the DOM lives)
  // — a node is not a JavaScript object, and the JS heap alone misses it.
  const heap = async () => {
    if (!cdp) return null;
    await gc();
    const usage = await cdp.send("Runtime.getHeapUsage");
    return { js: usage.usedSize, embedder: usage.embedderHeapUsedSize ?? 0 };
  };
  const counters = async () => (cdp ? await cdp.send("Memory.getDOMCounters") : null);
  const heapBefore = await heap();
  // The window's own weight before the conversation, so the conversation's
  // share of the renderer can be told from the window's.
  const rssBefore = rendererRss(engine, before);
  await openFixtureConversation(page, conversationFixture());
  // Two beats for anything the first paint deferred (a lazy body, an observer).
  await page.evaluate(async () => {
    for (let beat = 0; beat < 4; beat += 1) await window.__PAINTED__();
  });
  const dom = await page.evaluate(() => {
    const list = document.querySelector("#worker-view .helper-turns");
    return {
      rows: list?.querySelectorAll(":scope > [data-turn]").length ?? 0,
      turns: [...(list?.querySelectorAll(":scope > [data-turn]") ?? [])].reduce((sum, row) => sum + (row.__members?.length ?? 1), 0),
      listElements: list?.getElementsByTagName("*").length ?? 0,
      documentElements: document.getElementsByTagName("*").length,
      diffRows: list?.querySelectorAll(".diff-line").length ?? 0,
    };
  });
  // The heap first: its two collections take the nodes the page let go, so
  // the counters that follow count what the page holds, not what it dropped.
  const heapOpen = await heap();
  const domCounters = await counters();
  const rss = rendererRss(engine, before);

  // (4) One delta's paint: `paintLiveAnswerNow` wrapped where it is looked up.
  const paint = await page.evaluate(async ({ text, count }) => {
    const took = [];
    const inner = window.paintLiveAnswerNow;
    window.paintLiveAnswerNow = function measured(...args) {
      const from = performance.now();
      try {
        return inner.apply(this, args);
      } finally {
        took.push(performance.now() - from);
      }
    };
    const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
    try {
      for (let at = 1; at <= count; at += 1) {
        const upTo = Math.round((text.length * at) / count);
        window.__PERF_LIVE__ = [{ role: "assistant", text: text.slice(0, upTo) }];
        await pollHelperPages();
        await frame();
      }
      await frame();
    } finally {
      window.paintLiveAnswerNow = inner;
    }
    return took;
  }, { text: streamingAnswer(), count: deltas });

  // (5) The wheel: sixty turns upward through the history, one a frame, while
  // the page writes down when each frame began.
  const box = await page.evaluate(() => {
    const list = document.querySelector("#worker-view .helper-turns");
    const rect = list.getBoundingClientRect();
    window.__PERF_FRAMES__ = [];
    window.__PERF_RECORDING__ = true;
    const tick = (at) => {
      window.__PERF_FRAMES__.push(at);
      if (window.__PERF_RECORDING__) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, top: list.scrollTop };
  });
  await page.mouse.move(box.x, box.y);
  for (let wheel = 0; wheel < wheels; wheel += 1) {
    await page.mouse.wheel(0, -240);
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => done())));
  }
  const scroll = await page.evaluate(() => {
    window.__PERF_RECORDING__ = false;
    const frames = window.__PERF_FRAMES__;
    const gaps = [];
    for (let at = 1; at < frames.length; at += 1) gaps.push(frames[at] - frames[at - 1]);
    const list = document.querySelector("#worker-view .helper-turns");
    return { gaps, travelled: list.scrollTop };
  });
  const listAfterScroll = await page.evaluate(() => document.querySelector("#worker-view .helper-turns")?.getElementsByTagName("*").length ?? 0);
  return {
    engine,
    rows: dom.rows,
    turns: dom.turns,
    listElements: dom.listElements,
    documentElements: dom.documentElements,
    diffRows: dom.diffRows,
    nodes: domCounters?.nodes ?? null,
    listeners: domCounters?.jsEventListeners ?? null,
    heapBytes: heapOpen !== null && heapBefore !== null ? heapOpen.js - heapBefore.js : null,
    embedderBytes: heapOpen !== null && heapBefore !== null ? heapOpen.embedder - heapBefore.embedder : null,
    rssBytes: rss,
    rssBeforeBytes: rssBefore,
    rssConversationBytes: rss - rssBefore,
    paintP50: median(paint),
    paintP95: quantile(paint, 0.95),
    paints: paint.length,
    scrollFrames: scroll.gaps.length,
    over16: scroll.gaps.filter((gap) => gap > FRAME_MS).length / Math.max(1, scroll.gaps.length),
    over20: scroll.gaps.filter((gap) => gap > MISSED_FRAME_MS).length / Math.max(1, scroll.gaps.length),
    scrollP95: quantile(scroll.gaps, 0.95),
    listElementsAfterScroll: listAfterScroll,
    scrolledFrom: box.top,
  };
}

/* Rounds of fresh pages, the median of each number. */
export async function measureRounds({ engine = "chromium", rounds = 5 } = {}) {
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch({ headless: true });
  const taken = [];
  try {
    for (let round = 0; round < rounds; round += 1) {
      const before = standingPids();
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        taken.push(await measureConversation(page, { engine, before }));
        if (faults.length) taken.at(-1).faults = faults.slice(0, 3);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const keys = Object.keys(taken[0]).filter((key) => typeof taken[0][key] === "number");
  const summary = { engine, rounds };
  for (const key of keys) summary[key] = median(taken.map((one) => one[key]));
  return { summary, taken };
}

/* ---- 사람이 느끼는 네 장면 (t-22095 P0) ------------------------------------
 *
 * Hermes Agent의 데스크톱 앱이 제 대화 화면을 재는 네 가지(그들의 저장소
 * `scripts/perf`, profile-typing-lag.md·transcript.mjs — 방법만 배운다)를 이
 * 파일의 픽스처 위에서 그대로:
 *   (a) 열기 — 200턴과 400턴 대화를 열 때 첫 그림까지, 그리고 그 뒤 1.5 s의
 *       긴 작업(수·합·최대).
 *   (b) 입력 — 입력창에 120자를 초당 20자로: 키 → 다음 그림 p50/p90/p99/max,
 *       그동안 16.7 ms를 넘은 프레임.
 *   (c) 스트리밍 — 섞인 markdown(문단·목록·여러 언어의 울타리) 300토큰을 초당
 *       50과 125토큰으로: 프레임 p50/p95/p99, 5 s당 긴 작업과 가장 긴 것,
 *       markdown을 그리는 데 쓴 메인 스레드 시간, 그동안 친 키의 p95.
 *   (d) 누수 — 입력과 스트리밍 30바퀴: 리스너와 DOM 노드의 전/후.
 *
 * 델타는 실제 길로 온다: CLI가 말할 때마다 백엔드가 `wire:update`를 쏘고 그
 * 리스너가 `wire_log`를 읽는다 — 여기서도 그 리스너를 부른다. 긴 작업은
 * Chromium에서 Long Tasks API로, 그 API가 없는 WebKit에서는 심장박동(제로
 * 타임아웃 사슬)의 틈으로 잰다 — 틈은 그 작업보다 길거나 같으므로 WebKit의
 * 수는 위쪽 경계다. 키 → 그림은 keydown의 `timeStamp`(엔진이 키를 받은 시각)
 * 부터 그 뒤 첫 프레임이 그려진 시각(프레임 콜백이 올린 작업이 도는 때)까지.
 *
 *   node ui/tests/conversation-perf.mjs --scenarios [--engine webkit] [--rounds 5] [--json out.json]
 *   node ui/tests/conversation-perf.mjs --scenarios --against <checkout>   그 체크아웃과 이 체크아웃을 한 브라우저에서
 *                                                                          판마다 번갈아 — 전/후(A/B), 같은 창끼리(A/A)
 *   node ui/tests/conversation-perf.mjs --compare a.json b.json      두 실행이 몇 %로 맞는지
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/conversation-perf.mjs --scenarios   저사양(cpu4x, Chromium)
 */

/* The scenarios' own numbers — Hermes's, where they measured one:
 * measure-latency's 120 characters at 20 a second, the synthetic stream's 300
 * tokens at 50 and at 125 a second, transcript.mjs's 1.5 s for the open's
 * after-work to settle, and their windows of 5 s. The leak rounds type and
 * stream briefly — thirty of them is the count, not the length — but each
 * stream runs past its first block, so a fence closes and settles in every
 * round and whatever the page builds for a settled block is let go with it. */
export const SCENARIOS = Object.freeze({
  openTurns: [200, 400],
  openSettleMs: 1500,
  typing: Object.freeze({ chars: 120, cps: 20 }),
  stream: Object.freeze({ tokens: 300, rates: [50, 125], cps: 20 }),
  leak: Object.freeze({ rounds: 30, chars: 40, cps: 50, tokens: 120, rate: 125 }),
  windowMs: 5000,
  /* A page's quiet moment between two scenarios: frames settle and what the
   * last one left (a closed answer, a fold) is drawn before the next begins. */
  quietMs: 500,
});

/* A task this long is a long task — the Long Tasks API's own line. */
const LONG_TASK_MS = 50;

/* The heartbeat's beat where the API is missing: a chain of zero timeouts is
 * clamped to 4 ms once it nests (HTML's timer rule), so a free main thread
 * beats about that often, and a gap of 50 ms is a thread held that long. */
const HEARTBEAT_MS = 4;

/* What the person types: plain words — no `/` or `@`, which open the
 * composer's menus, and no Enter, which sends. */
const TYPED = "the quick brown fox jumps over the lazy dog ";

/* The key that chooses everything in a box on this platform. */
const SELECT_ALL = process.platform === "darwin" ? "Meta+A" : "Control+A";

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/* The page's recorder, once a page: long tasks (or the heartbeat's gaps),
 * frames, key → paint, and the time the one markdown road (`paintHelperProse`)
 * and the live answer's paint (`paintLiveAnswerNow`) take — both wrapped where
 * they are looked up, as B0's delta paint is. It records only between
 * `begin()` and `end()`, each recording its own generation: a frame or a key
 * still in flight from the last one never lands in the next. The stream
 * driver and the close are its hands. */
async function standRecorder(page) {
  await page.evaluate(({ longTaskMs, heartbeatMs }) => {
    if (window.__FEEL__) return;
    const feel = {
      recording: false, generation: 0, begunAt: 0, longtasks: [], frames: [], keys: [], prose: [], live: [], sent: [],
      source: (PerformanceObserver.supportedEntryTypes ?? []).includes("longtask") ? "longtask" : "heartbeat",
      beat: 0, lastBeat: 0,
    };
    window.__FEEL__ = feel;
    // The API hands its entries over later than they end; each is kept by
    // when it began, and `end()` takes what is still queued.
    const tasks = [];
    const observer = feel.source === "longtask"
      ? new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) tasks.push({ at: entry.startTime, ms: entry.duration });
      })
      : null;
    observer?.observe({ type: "longtask" });
    const gapTo = (now) => {
      const gap = now - feel.lastBeat;
      if (gap >= longTaskMs) tasks.push({ at: feel.lastBeat, ms: gap });
      feel.lastBeat = now;
    };
    const beat = () => {
      gapTo(performance.now());
      if (feel.recording) feel.beat = setTimeout(beat, heartbeatMs);
    };
    const frameOf = (generation) => {
      const frame = (at) => {
        if (generation !== feel.generation || !feel.recording) return;
        feel.frames.push(at);
        requestAnimationFrame(frame);
      };
      return frame;
    };
    feel.begin = () => {
      for (const list of [tasks, feel.frames, feel.keys, feel.prose, feel.live, feel.sent]) list.length = 0;
      feel.generation += 1;
      feel.recording = true;
      feel.begunAt = performance.now();
      requestAnimationFrame(frameOf(feel.generation));
      if (feel.source === "heartbeat") {
        feel.lastBeat = feel.begunAt;
        feel.beat = setTimeout(beat, heartbeatMs);
      }
      return feel.begunAt;
    };
    feel.end = () => {
      const to = performance.now();
      if (observer) for (const entry of observer.takeRecords()) tasks.push({ at: entry.startTime, ms: entry.duration });
      else gapTo(to);
      feel.recording = false;
      clearTimeout(feel.beat);
      return {
        to,
        longtasks: tasks.filter((task) => task.at >= feel.begunAt), frames: [...feel.frames], keys: [...feel.keys],
        prose: [...feel.prose], live: [...feel.live], sent: [...feel.sent], source: feel.source,
      };
    };
    // Key → the frame after it, on screen. The engine's own stamp for when
    // the key arrived, unless it is on another clock than the page's.
    window.addEventListener("keydown", (event) => {
      if (!feel.recording) return;
      const generation = feel.generation;
      const now = performance.now();
      const from = Math.abs(now - event.timeStamp) < 60_000 ? event.timeStamp : now;
      requestAnimationFrame(() => {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => {
          if (generation === feel.generation) feel.keys.push({ at: from, ms: performance.now() - from });
        };
        channel.port2.postMessage(null);
      });
    }, true);
    const timed = (name, into) => {
      const inner = window[name];
      window[name] = function timedPaint(...args) {
        const from = performance.now();
        try {
          return inner.apply(this, args);
        } finally {
          if (feel.recording) into.push({ at: from, ms: performance.now() - from });
        }
      };
    };
    timed("paintHelperProse", feel.prose);
    timed("paintLiveAnswerNow", feel.live);
    // The backend's word that the wire said something — what the page's own
    // listener hears from a real session.
    const told = () => {
      for (const heard of window.__LISTENERS__["wire:update"] ?? []) heard({ payload: { id: 31 } });
    };
    // `tokens` pieces of `text` at `rate` a second, on schedule: when the page
    // falls behind, the next wake carries every piece that is due by then, the
    // way a busy page's poll reads every delta that arrived meanwhile.
    feel.stream = ({ text, tokens, rate }) => new Promise((done) => {
      const from = performance.now();
      let sent = 0;
      const step = () => {
        const due = Math.min(tokens, Math.floor(((performance.now() - from) * rate) / 1000) + 1);
        if (due > sent) {
          sent = due;
          window.__PERF_LIVE__ = [{ role: "assistant", text: text.slice(0, Math.round((text.length * sent) / tokens)) }];
          if (feel.recording) feel.sent.push({ at: performance.now(), sent });
          told();
        }
        if (sent >= tokens) {
          done({ from, to: performance.now() });
          return;
        }
        setTimeout(step, Math.max(0, from + (sent * 1000) / rate - performance.now()));
      };
      step();
    });
    // The words close: into the answer's turn, or into nothing (a stream
    // that stopped) — either way the live list is empty on the same read.
    feel.close = (text) => {
      if (text !== null) window.__PERF_TURNS__.push({ role: "assistant", text, at_ms: Date.now() });
      window.__PERF_LIVE__ = [];
      told();
    };
  }, { longTaskMs: LONG_TASK_MS, heartbeatMs: HEARTBEAT_MS });
}

const frames = (page, count) => page.evaluate(async (count) => {
  for (let beat = 0; beat < count; beat += 1) await window.__PAINTED__();
}, count);

async function quiet(page) {
  await frames(page, 2);
  await sleep(SCENARIOS.quietMs);
}

const COMPOSER = "#worker-view .worker-composer-box";

/* A key typed on a fixed beat lands on one phase of the frame clock for the
 * whole run — 20 a second is every sixth frame at 120 Hz — and its latency is
 * that phase's alone: rounds of the same page read 4 ms or 9 ms by where the
 * first key fell. Each key is moved by its own fraction of a frame instead
 * (the golden ratio's steps cover a frame evenly), so a run meets every phase
 * a person's typing does; the beat is still `cps` on average. */
const GOLDEN_STEP = (Math.sqrt(5) - 1) / 2;
const keyPhase = (at) => ((at * GOLDEN_STEP) % 1) * FRAME_MS;

/* `chars` characters into the focused box, `cps` a second on schedule, until
 * `until` (a node clock time) when it is given. */
async function typeAt(page, { chars, cps, until = Infinity }) {
  const from = Date.now();
  let typed = 0;
  for (let at = 0; at < chars; at += 1) {
    const due = from + (at * 1000) / cps + keyPhase(at);
    if (due > until) break;
    const wait = due - Date.now();
    if (wait > 0) await sleep(wait);
    await page.keyboard.type(TYPED[at % TYPED.length]);
    typed += 1;
  }
  return typed;
}

/* Empty the box the way a person does: everything chosen, then deleted. */
async function clearComposer(page) {
  await page.focus(COMPOSER);
  await page.keyboard.press(SELECT_ALL);
  await page.keyboard.press("Backspace");
}

/* What a recording says about frames, keys and long tasks, over `ms`. */
function readRecording(taken, ms) {
  const gaps = [];
  for (let at = 1; at < taken.frames.length; at += 1) gaps.push(taken.frames[at] - taken.frames[at - 1]);
  const keys = taken.keys.map((key) => key.ms);
  const long = taken.longtasks.map((task) => task.ms);
  const per = (count) => (count * SCENARIOS.windowMs) / Math.max(1, ms);
  const sum = (values) => values.reduce((total, value) => total + value, 0);
  return {
    gaps,
    keys,
    frames: gaps.length,
    framesOver16: gaps.filter((gap) => gap > FRAME_MS).length,
    framesOver20: gaps.filter((gap) => gap > MISSED_FRAME_MS).length,
    longtasks: long.length,
    longtasksPer5s: per(long.length),
    longtaskTotalMs: sum(long),
    longtaskMaxMs: long.length ? Math.max(...long) : 0,
    proseMs: sum(taken.prose.map((one) => one.ms)),
    prosePer5sMs: per(sum(taken.prose.map((one) => one.ms))),
    proseCalls: taken.prose.length,
    livePaints: taken.live.length,
    liveMs: sum(taken.live.map((one) => one.ms)),
    source: taken.source,
  };
}

/* (a) A conversation of `turns` turns opened on a fresh page: the first paint
 * and what the main thread did until it settled. */
export async function measureOpen(page, { turns }) {
  await standRecorder(page);
  await quiet(page);
  await page.evaluate(() => window.__FEEL__.begin());
  const opened = await openFixtureConversation(page, conversationFixture({ blocks: turns / 20 }), { timed: true });
  await sleep(SCENARIOS.openSettleMs);
  const taken = await page.evaluate(() => window.__FEEL__.end());
  const read = readRecording(taken, taken.to - opened.from);
  return {
    turns,
    rowsAtMount: opened.rowsAtMount,
    mountMs: opened.mountMs,
    firstPaintMs: opened.firstPaintMs,
    longtasks: read.longtasks,
    longtaskTotalMs: read.longtaskTotalMs,
    longtaskMaxMs: read.longtaskMaxMs,
    source: read.source,
  };
}

/* (b) Typing into the composer of the page as it stands, nothing streaming. */
export async function measureTyping(page, { chars = SCENARIOS.typing.chars, cps = SCENARIOS.typing.cps } = {}) {
  await standRecorder(page);
  await clearComposer(page);
  await quiet(page);
  const from = await page.evaluate(() => window.__FEEL__.begin());
  const typed = await typeAt(page, { chars, cps });
  await frames(page, 2);
  const taken = await page.evaluate(() => window.__FEEL__.end());
  await clearComposer(page);
  const read = readRecording(taken, taken.to - from);
  return { typed, keys: read.keys, gaps: read.gaps, frames: read.frames, framesOver16: read.framesOver16, framesOver20: read.framesOver20 };
}

/* (c) A stream of the mixed answer at `rate` tokens a second, typed into all
 * the while at `cps`, then closed into its turn. The stream's window is from
 * its first token to the frame after its last; the close is apart. */
export async function measureStream(page, { rate, tokens = SCENARIOS.stream.tokens, cps = SCENARIOS.stream.cps } = {}) {
  await standRecorder(page);
  await clearComposer(page);
  await quiet(page);
  const text = streamingAnswer({ fences: MIXED_STREAM_FENCES });
  const begun = await page.evaluate(() => window.__FEEL__.begin());
  await page.evaluate(({ text, tokens, rate }) => {
    window.__FEEL_STREAM__ = window.__FEEL__.stream({ text, tokens, rate });
  }, { text, tokens, rate });
  const typed = await typeAt(page, { chars: Number.POSITIVE_INFINITY, cps, until: Date.now() + ((tokens - 1) * 1000) / rate });
  const streamed = await page.evaluate(() => window.__FEEL_STREAM__);
  await frames(page, 2);
  const taken = await page.evaluate(() => window.__FEEL__.end());
  // The close: the answer's turn arrives and the live row gives way to it.
  await page.evaluate(() => window.__FEEL__.begin());
  await page.evaluate((text) => window.__FEEL__.close(text), text);
  await frames(page, 2);
  await sleep(SCENARIOS.quietMs);
  const closed = await page.evaluate(() => window.__FEEL__.end());
  await clearComposer(page);
  const ms = taken.to - begun;
  const read = readRecording(taken, ms);
  const close = readRecording(closed, SCENARIOS.windowMs);
  return {
    rate,
    tokens,
    streamMs: streamed.to - streamed.from,
    windowMs: ms,
    sends: taken.sent.length,
    typed,
    keys: read.keys,
    gaps: read.gaps,
    framesOver16: read.framesOver16,
    framesOver20: read.framesOver20,
    longtasks: read.longtasks,
    longtasksPer5s: read.longtasksPer5s,
    longtaskMaxMs: read.longtaskMaxMs,
    proseMs: read.proseMs,
    prosePer5sMs: read.prosePer5sMs,
    proseCalls: read.proseCalls,
    livePaints: read.livePaints,
    liveMs: read.liveMs,
    closeProseMs: close.proseMs,
    closeLongtaskMaxMs: close.longtaskMaxMs,
    source: read.source,
  };
}

/* (d) Rounds of typing and streaming on one page, each left as it found the
 * page — the box emptied, the stream stopped into nothing — so what the
 * counters gain between the first round and the last is what was kept. The
 * first round is the warm-up (a menu, a cache made once), as Hermes found. */
export async function measureLeaks(page, { engine, rounds = SCENARIOS.leak.rounds } = {}) {
  const { chars, cps, tokens, rate } = SCENARIOS.leak;
  await standRecorder(page);
  const cdp = engine === "chromium" ? await page.context().newCDPSession(page) : null;
  const text = streamingAnswer({ fences: MIXED_STREAM_FENCES });
  const count = async () => {
    if (cdp) {
      await cdp.send("HeapProfiler.collectGarbage");
      await cdp.send("HeapProfiler.collectGarbage");
    }
    const counters = cdp ? await cdp.send("Memory.getDOMCounters") : null;
    const heap = cdp ? await cdp.send("Runtime.getHeapUsage") : null;
    // What the document holds attached, the one count WebKit can give.
    const drawn = await page.evaluate(() => {
      const walker = document.createTreeWalker(document, NodeFilter.SHOW_ALL);
      let nodes = 0;
      while (walker.nextNode()) nodes += 1;
      return { elements: document.getElementsByTagName("*").length, attached: nodes };
    });
    return {
      nodes: counters?.nodes ?? null,
      listeners: counters?.jsEventListeners ?? null,
      documents: counters?.documents ?? null,
      heapBytes: heap?.usedSize ?? null,
      ...drawn,
    };
  };
  let first = null;
  for (let round = 0; round < rounds; round += 1) {
    await page.focus(COMPOSER);
    await typeAt(page, { chars, cps });
    await clearComposer(page);
    await page.evaluate(({ text, tokens, rate }) => window.__FEEL__.stream({ text, tokens, rate }), { text, tokens, rate });
    await page.evaluate(() => window.__FEEL__.close(null));
    await frames(page, 2);
    if (round === 0) first = await count();
  }
  const last = await count();
  const grew = (key) => (first[key] === null ? null : last[key] - first[key]);
  return {
    rounds,
    first,
    last,
    nodesGrew: grew("nodes"),
    listenersGrew: grew("listeners"),
    elementsGrew: grew("elements"),
    attachedGrew: grew("attached"),
    heapGrewBytes: grew("heapBytes"),
  };
}

/* One round: a 200-turn and a 400-turn open on fresh pages; on the 400-turn
 * page, typing, then each stream rate. */
async function scenarioRound(browser, origin) {
  const round = { faults: [] };
  for (const turns of SCENARIOS.openTurns) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      round[`open${turns}`] = await measureOpen(page, { turns });
      if (turns !== Math.max(...SCENARIOS.openTurns)) continue;
      await quiet(page);
      round.typing = await measureTyping(page);
      for (const rate of SCENARIOS.stream.rates) round[`stream${rate}`] = await measureStream(page, { rate });
    } finally {
      round.faults.push(...faults.slice(0, 3));
      await page.close();
    }
  }
  return round;
}

const percentile = (values, q) => (values.length ? quantile(values, q) : null);

/* The rounds read together: a single number a round (an open's first paint, a
 * stream's long tasks) by its median, a distribution (keys, frames) pooled
 * over every round first, and the longest long task as the longest seen. */
export function summarizeScenarios(rounds) {
  const pick = (name) => rounds.map((round) => round[name]).filter(Boolean);
  const mid = (name, key) => median(pick(name).map((one) => one[key]));
  const pool = (name, key) => pick(name).flatMap((one) => one[key]);
  const summary = {};
  for (const turns of SCENARIOS.openTurns) {
    const name = `open${turns}`;
    summary[name] = {
      rowsAtMount: mid(name, "rowsAtMount"),
      mountMs: mid(name, "mountMs"),
      firstPaintMs: mid(name, "firstPaintMs"),
      longtasks: mid(name, "longtasks"),
      longtaskTotalMs: mid(name, "longtaskTotalMs"),
      longtaskMaxMs: mid(name, "longtaskMaxMs"),
    };
  }
  const keys = pool("typing", "keys");
  const gaps = pool("typing", "gaps");
  summary.typing = {
    keys: keys.length,
    p50: percentile(keys, 0.5), p90: percentile(keys, 0.9), p99: percentile(keys, 0.99), max: keys.length ? Math.max(...keys) : null,
    framesOver16: gaps.filter((gap) => gap > FRAME_MS).length / Math.max(1, gaps.length),
    framesOver20: gaps.filter((gap) => gap > MISSED_FRAME_MS).length / Math.max(1, gaps.length),
    keysOver16: keys.filter((key) => key > FRAME_MS).length,
  };
  for (const rate of SCENARIOS.stream.rates) {
    const name = `stream${rate}`;
    const frameGaps = pool(name, "gaps");
    const typedKeys = pool(name, "keys");
    summary[name] = {
      frameP50: percentile(frameGaps, 0.5), frameP95: percentile(frameGaps, 0.95), frameP99: percentile(frameGaps, 0.99),
      framesOver16: frameGaps.filter((gap) => gap > FRAME_MS).length / Math.max(1, frameGaps.length),
      longtasksPer5s: mid(name, "longtasksPer5s"),
      longtaskMaxMs: Math.max(0, ...pick(name).map((one) => one.longtaskMaxMs)),
      longtaskMaxMsMedian: mid(name, "longtaskMaxMs"),
      prosePer5sMs: mid(name, "prosePer5sMs"),
      proseMs: mid(name, "proseMs"),
      livePaints: mid(name, "livePaints"),
      sends: mid(name, "sends"),
      typingP50: percentile(typedKeys, 0.5), typingP95: percentile(typedKeys, 0.95), typingP99: percentile(typedKeys, 0.99),
      typed: typedKeys.length,
      closeProseMs: mid(name, "closeProseMs"),
      closeLongtaskMaxMs: Math.max(0, ...pick(name).map((one) => one.closeLongtaskMaxMs)),
    };
  }
  return summary;
}

/* The leak test, on a page of its own. */
async function leakRound(browser, origin, engine) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openFixtureConversation(page, conversationFixture());
    await quiet(page);
    return { ...(await measureLeaks(page, { engine })), faults: faults.slice(0, 3) };
  } finally {
    await page.close();
  }
}

/* What a run of `taken` rounds and its leak test come to. */
function scenarioResult({ engine, rounds, machine, taken, leaks }) {
  return {
    engine,
    throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 0) || 1,
    rounds,
    machine,
    longtaskSource: taken[0]?.open400?.source ?? null,
    summary: { ...summarizeScenarios(taken), leaks },
    taken,
  };
}

/* Rounds of the four scenarios in one browser, then one leak test on a page
 * of its own. The machine as the run began and ended: a run that started
 * over its cores is read as such, never averaged in silently. */
export async function measureScenarios({ engine = "chromium", rounds = 5 } = {}) {
  const machine = { cores: cpus().length, loadBefore: loadavg()[0], loadAfter: null };
  const { files, origin } = await createWindowServer();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  const taken = [];
  let leaks = null;
  try {
    for (let round = 0; round < rounds; round += 1) taken.push(await scenarioRound(browser, origin));
    leaks = await leakRound(browser, origin, engine);
  } finally {
    await browser.close();
    files.close();
  }
  machine.loadAfter = loadavg()[0];
  return scenarioResult({ engine, rounds, machine, taken, leaks });
}

/* Two windows in one run (`--against <checkout>`): that checkout's and this
 * one's, served side by side, their rounds alternating in one browser — the
 * first of a round changes sides every round — so a machine whose load moves
 * while it runs moves both alike. Run against a checkout of the same window
 * it says how far apart two runs of one window come; against the commit
 * before a change, what the change did. */
export async function measureScenariosAgainst({ engine = "chromium", rounds = 5, against } = {}) {
  const machine = { cores: cpus().length, loadBefore: loadavg()[0], loadAfter: null };
  const there = await import(pathToFileURL(resolve(against, "ui/tests/window-boot.mjs")).href);
  const sides = [
    { name: "against", server: await there.createWindowServer(), taken: [], leaks: null },
    { name: "here", server: await createWindowServer(), taken: [], leaks: null },
  ];
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  try {
    for (let round = 0; round < rounds; round += 1) {
      for (const side of round % 2 === 0 ? sides : [...sides].reverse()) {
        side.taken.push(await scenarioRound(browser, side.server.origin));
      }
    }
    for (const side of sides) side.leaks = await leakRound(browser, side.server.origin, engine);
  } finally {
    await browser.close();
    for (const side of sides) side.server.files.close();
  }
  machine.loadAfter = loadavg()[0];
  return Object.fromEntries(sides.map((side) => [side.name, scenarioResult({ engine, rounds, machine, taken: side.taken, leaks: side.leaks })]));
}

/* The scenarios' numbers as lines. */
function printScenarios(result) {
  const ms = (value) => (value === null || value === undefined ? "—" : `${value.toFixed(1)} ms`);
  const pct = (value) => `${(value * 100).toFixed(1)}%`;
  const { summary } = result;
  console.log(`engine ${result.engine} · cpu ×${result.throttle} · ${result.rounds} rounds · long tasks by ${result.longtaskSource} · load ${result.machine.loadBefore.toFixed(1)} → ${result.machine.loadAfter.toFixed(1)} on ${result.machine.cores} cores`);
  for (const turns of SCENARIOS.openTurns) {
    const open = summary[`open${turns}`];
    console.log(`  (a) open ${turns} turns: first paint ${ms(open.firstPaintMs)} (rows stood ${ms(open.mountMs)}, ${open.rowsAtMount} rows) · long tasks ${open.longtasks} · total ${ms(open.longtaskTotalMs)} · max ${ms(open.longtaskMaxMs)}`);
  }
  const typing = summary.typing;
  console.log(`  (b) typing ${typing.keys} keys: p50 ${ms(typing.p50)} · p90 ${ms(typing.p90)} · p99 ${ms(typing.p99)} · max ${ms(typing.max)} · keys >16.7 ms ${typing.keysOver16} · frames >16.7 ms ${pct(typing.framesOver16)}`);
  for (const rate of SCENARIOS.stream.rates) {
    const stream = summary[`stream${rate}`];
    console.log(`  (c) stream ${rate} tok/s: frames p50 ${ms(stream.frameP50)} · p95 ${ms(stream.frameP95)} · p99 ${ms(stream.frameP99)} · >16.7 ms ${pct(stream.framesOver16)}`);
    console.log(`      long tasks ${stream.longtasksPer5s.toFixed(2)} per 5 s · longest ${ms(stream.longtaskMaxMs)} (median of rounds ${ms(stream.longtaskMaxMsMedian)}) · markdown ${ms(stream.prosePer5sMs)} per 5 s (${ms(stream.proseMs)} a stream, ${stream.livePaints} live paints)`);
    console.log(`      typing while it streams (${stream.typed} keys): p50 ${ms(stream.typingP50)} · p95 ${ms(stream.typingP95)} · p99 ${ms(stream.typingP99)} · the close: markdown ${ms(stream.closeProseMs)}, longest task ${ms(stream.closeLongtaskMaxMs)}`);
  }
  const leaks = summary.leaks;
  if (leaks) {
    const grew = (value) => (value === null ? "—" : `${value >= 0 ? "+" : ""}${value}`);
    console.log(`  (d) ${leaks.rounds} rounds: listeners ${leaks.first.listeners ?? "—"} → ${leaks.last.listeners ?? "—"} (${grew(leaks.listenersGrew)}) · nodes ${leaks.first.nodes ?? "—"} → ${leaks.last.nodes ?? "—"} (${grew(leaks.nodesGrew)}) · elements ${grew(leaks.elementsGrew)} · attached nodes ${grew(leaks.attachedGrew)} · JS heap ${leaks.heapGrewBytes === null ? "—" : `${(leaks.heapGrewBytes / 1024).toFixed(0)} KB`}`);
  }
}

/* Two runs of the same scenarios, number by number: how far apart they are,
 * as a share of the larger. Numbers that are zero in both agree. */
export function compareScenarios(a, b) {
  const rows = [];
  const walk = (left, right, path) => {
    for (const [key, value] of Object.entries(left ?? {})) {
      const other = right?.[key];
      if (typeof value === "number" && typeof other === "number") {
        const apart = value === other ? 0 : Math.abs(other - value) / Math.max(Math.abs(value), Math.abs(other));
        rows.push({ path: `${path}${key}`, a: value, b: other, apart });
      } else if (value && typeof value === "object" && !Array.isArray(value)) {
        walk(value, other, `${path}${key}.`);
      }
    }
  };
  walk(a.summary, b.summary, "");
  return rows;
}

export function webkitType() {
  // The same resolution `window-boot.mjs` walks for chromium, for webkit.
  const require = createRequire(import.meta.url);
  let entry;
  try {
    entry = require.resolve("playwright");
  } catch {
    const roots = execFileSync("npm", ["root", "-g"], { encoding: "utf8" }).trim();
    entry = require.resolve(`${roots}/playwright`);
  }
  return require(entry).webkit;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const engine = option("--engine", "chromium");
  const rounds = Number(option("--rounds", "5"));
  const out = option("--json", null);
  const compared = argv.indexOf("--compare");
  if (compared >= 0) {
    const [a, b] = argv.slice(compared + 1, compared + 3).map((path) => JSON.parse(readFileSync(path, "utf8")));
    for (const row of compareScenarios(a, b)) {
      console.log(`${row.apart > 0.05 ? "APART" : "AGREE"} ${row.path}: ${row.a.toFixed(2)} · ${row.b.toFixed(2)} (${(row.apart * 100).toFixed(1)}%)`);
    }
    process.exit(0);
  }
  if (argv.includes("--scenarios")) {
    const against = option("--against", null);
    if (against !== null) {
      const result = await measureScenariosAgainst({ engine, rounds, against });
      console.log("== the checkout run against");
      printScenarios(result.against);
      console.log("== this checkout");
      printScenarios(result.here);
      console.log("== this checkout against the other, number by number");
      for (const row of compareScenarios(result.against, result.here)) {
        console.log(`${row.apart > 0.05 ? "APART" : "AGREE"} ${row.path}: ${row.a.toFixed(2)} → ${row.b.toFixed(2)} (${(row.apart * 100).toFixed(1)}%)`);
      }
      if (out) writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`);
      process.exit(0);
    }
    const result = await measureScenarios({ engine, rounds });
    printScenarios(result);
    if (out) writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`);
    process.exit(0);
  }
  const { summary, taken } = await measureRounds({ engine, rounds });
  const mb = (bytes) => (bytes === null ? "—" : `${(bytes / 1024 / 1024).toFixed(1)} MB`);
  console.log(`engine ${summary.engine}, ${summary.rounds} rounds (medians)`);
  console.log(`  rows ${summary.rows} of ${summary.turns} turns · list elements ${summary.listElements} · document elements ${summary.documentElements} · diff rows ${summary.diffRows}`);
  console.log(`  nodes (DOM counters) ${summary.nodes ?? "—"} · listeners ${summary.listeners ?? "—"}`);
  console.log(`  JS heap ${mb(summary.heapBytes)} · embedder (DOM) heap ${mb(summary.embedderBytes)} · renderer RSS ${mb(summary.rssBytes)} (the conversation's share ${mb(summary.rssConversationBytes)}, the window before it ${mb(summary.rssBeforeBytes)})`);
  console.log(`  delta paint p50 ${summary.paintP50.toFixed(2)} ms · p95 ${summary.paintP95.toFixed(2)} ms (${summary.paints} paints)`);
  console.log(`  scroll frames ${summary.scrollFrames} · >16.7 ms ${(summary.over16 * 100).toFixed(1)}% · >20 ms ${(summary.over20 * 100).toFixed(1)}% · p95 ${summary.scrollP95.toFixed(1)} ms`);
  console.log(`  list elements after the scroll ${summary.listElementsAfterScroll}`);
  if (out) writeFileSync(out, `${JSON.stringify({ summary, taken }, null, 2)}\n`);
}
