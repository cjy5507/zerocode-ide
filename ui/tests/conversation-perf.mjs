/* 대화 뷰의 무게 — 400턴 전사 하나로 재는 다섯 수 (t-6323 B0).
 *
 * docs/design/agent-conversation-claude-code-grammar-20260915.md §10. 「숫자
 * 없는 최적화 금지」의 자다: 먼저 재고, 고치고, 같은 픽스처·같은 부하로 다시
 * 잰다. 전/후는 같은 이 파일로, 전은 기준 커밋의 `ui/`를 스냅샷으로 풀어 그
 * 안에서 돌린다(`git archive <ref> ui` → 이 파일을 복사 → 실행).
 *
 * 픽스처(`conversationFixture`)는 결정적이다 — 사람의 말 40·답 100·생각 60·
 * 도구 200(그중 diff 60, 긴 출력 40), 그리고 그림 10(사람이 붙인 것 5, 도구가
 * 돌려준 스크린숏 5). 도구 결과는 제 호출 행에 합류하므로 행은 정확히 400이다.
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
 */
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
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
        tool: { call_id: id, name: "Edit", input: JSON.stringify({ file_path: `src/module_${tool}.rs` }), is_error: false,
          edits: [{ path: `src/module_${tool}.rs`, lines: diffLines(tool, count) }] },
      });
      turns.push({ role: "tool_result", text: `The file src/module_${tool}.rs has been updated.`, at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: false } });
    } else if (kind === 3 || kind === 7) {
      turns.push({
        role: "tool", text: `Bash · cargo test -p module_${tool}`, at_ms: stamp(),
        tool: { call_id: id, name: "Bash", input: `cargo test -p module_${tool}`, is_error: false },
      });
      turns.push({ role: "tool_result", text: longOutput(tool), at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: tool % 20 === 7 } });
    } else if (kind === 9 && tool % 40 === 9) {
      // A screenshot came back — the Computer Use shape: a picture and a line.
      turns.push({
        role: "tool", text: `mcp__computer-use__screenshot · display ${tool}`, at_ms: stamp(),
        tool: { call_id: id, name: "mcp__computer-use__screenshot", input: "{}", is_error: false },
      });
      turns.push({ role: "tool_result", text: "screenshot taken", at_ms: stamp(),
        tool: { call_id: id, name: "", input: "", is_error: false },
        images: [{ media_type: "image/png", at: `wire:${tool}` }] });
    } else {
      const name = ["Read", "Grep", "Glob"][tool % 3];
      turns.push({
        role: "tool", text: `${name} · src/module_${tool}.rs`, at_ms: stamp(),
        tool: { call_id: id, name, input: JSON.stringify({ file_path: `src/module_${tool}.rs`, offset: 10 + tool }), is_error: false },
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

/* The words a streaming answer grows through: a few paragraphs, a list and a
 * fence, long enough that the settled part keeps growing while it streams. */
function streamingAnswer() {
  const parts = [];
  for (let block = 0; block < 6; block += 1) {
    parts.push(`단락 ${block}: 스트리밍 중인 답은 델타가 도착한 다음 프레임에 화면에 있어야 한다. ` +
      "터미널과 확장은 둘 다 도착한 대로 그리고, 여기도 그렇다. 쓰는 중인 블록만 다시 그리고 닫힌 블록은 한 번 그린다.");
    parts.push("");
    parts.push("- 하나: 닫힌 블록은 markdown으로 한 번\n- 둘: 열린 블록도 markdown으로\n- 셋: 페이싱 없음");
    parts.push("");
    parts.push("```js\nconst cut = settledCut(text, text.length);\nif (cut > row.__settledEnd) paint(cut);\n```");
    parts.push("");
  }
  return parts.join("\n");
}

/* Open the fixture as a wire session's page — the one page that has both the
 * transcript and a live answer — with the session working, so the live answer
 * row and the status line stand. Returns once the page has painted. */
export async function openFixtureConversation(page, turns) {
  await page.evaluate(async ({ history, png }) => {
    window.__PERF_LIVE__ = [];
    window.__ANSWER__.wire_start = (args) => ({ id: 31, agent: args.agent, protocol: "claude-stream", version: "2.1.278", model: null, session: null });
    window.__ANSWER__.wire_log = () => ({
      found: true, skipped: false, next: 0, turns: [], status: "working", asks: [],
      live: window.__PERF_LIVE__, agent: "claude", protocol: "claude-stream",
      models: [], modes: [], commands: [], version: "2.1.278",
    });
    window.__ANSWER__.wire_stop = () => null;
    // A picture's bytes, when the page asks for one (A8's lazy load).
    window.__ANSWER__.wire_image = () => png;
    await openWirePage("claude", "/tmp/zerocode-window-test", { history });
    await pollHelperPages();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 50));
    await window.__PAINTED__();
  }, { history: turns, png: FIXTURE_PNG });
}

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
    over16: scroll.gaps.filter((gap) => gap > 16.7).length / Math.max(1, scroll.gaps.length),
    over20: scroll.gaps.filter((gap) => gap > 20).length / Math.max(1, scroll.gaps.length),
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

function webkitType() {
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
  const { summary, taken } = await measureRounds({ engine, rounds });
  const mb = (bytes) => (bytes === null ? "—" : `${(bytes / 1024 / 1024).toFixed(1)} MB`);
  console.log(`engine ${summary.engine}, ${summary.rounds} rounds (medians)`);
  console.log(`  rows ${summary.rows} · list elements ${summary.listElements} · document elements ${summary.documentElements} · diff rows ${summary.diffRows}`);
  console.log(`  nodes (DOM counters) ${summary.nodes ?? "—"} · listeners ${summary.listeners ?? "—"}`);
  console.log(`  JS heap ${mb(summary.heapBytes)} · embedder (DOM) heap ${mb(summary.embedderBytes)} · renderer RSS ${mb(summary.rssBytes)} (the conversation's share ${mb(summary.rssConversationBytes)}, the window before it ${mb(summary.rssBeforeBytes)})`);
  console.log(`  delta paint p50 ${summary.paintP50.toFixed(2)} ms · p95 ${summary.paintP95.toFixed(2)} ms (${summary.paints} paints)`);
  console.log(`  scroll frames ${summary.scrollFrames} · >16.7 ms ${(summary.over16 * 100).toFixed(1)}% · >20 ms ${(summary.over20 * 100).toFixed(1)}% · p95 ${summary.scrollP95.toFixed(1)} ms`);
  console.log(`  list elements after the scroll ${summary.listElementsAfterScroll}`);
  if (out) writeFileSync(out, `${JSON.stringify({ summary, taken }, null, 2)}\n`);
}
