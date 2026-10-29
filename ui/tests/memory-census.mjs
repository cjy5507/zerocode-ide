/* 창 렌더러의 메모리, 장면마다 (t-54317).
 *
 * 사람이 쓰는 창(WKWebView)에는 계기를 걸지 않는다 — vmmap·footprint·heap은 걸린
 * 프로세스를 멈출 수 있다. 같은 `ui/`를 Playwright의 WebKit(설치 앱 웹뷰의 엔진)과
 * Chromium으로 띄우고, 스텁 백엔드 위에 장면을 세운 뒤, **이 스크립트가 띄운** 페이지의
 * 프로세스만 밖에서 읽는다: footprint(물리 사용량과 그 dirty·swapped)와, WebKit에서는
 * heap의 크기 분포. Chromium에서는 CDP로 GC 두 번 뒤의 JS 힙·DOM 노드를 더 읽는다.
 * RSS 한 숫자로는 판정하지 않는다. 절대값은 WKWebView의 값이 아니므로 판정은 같은
 * 계기로 잰 전/후의 비다(`docs` 밖의 근거: 2026-08-31 WebContent 판정과 같은 방법).
 *
 * 장면:
 *   empty          부팅만 한 창
 *   panes-1/10/30  터미널 판 N개, 판마다 60×220 한 화면(빌드 로그 모양의 프레임)
 *   scrollback     판 하나에 5,000줄을 흘린 뒤 히스토리 한가운데를 보는 화면
 *   conversation   400턴 대화(conversation-perf의 픽스처) — 재기만 한다
 *   chat-long      긴 전사(400블록, 결과 줄 60 KiB — 이 기계의 긴 세션의 무게) 판 하나의 대화 보기를 연 화면
 *                  (t-82901). 읽기는 각 창의 백엔드대로 답한다: 위로 읽는 길이 있는 창엔 턴 단위, 없는 창엔
 *                  마지막 256 KiB(`standLongPaneChats`) — 전/후가 저마다의 백엔드로 재진다
 *   chat-long-3    같은 판 셋을 차례로 연 화면(앞의 둘은 뒤에 선다)
 *   chat-long-top  판 하나를 「이전 기록 더 보기」로 첫 턴까지 읽어 올린 화면 — 붙잡는 턴의 상한이 서는지
 *   chat-long-held 같은 전사를 처음부터 끝까지 읽어 상한(400행)만큼 든 헬퍼 페이지 — 위로 읽는 길이 없던 창도
 *                  이미 들 수 있던 상태, `chat-long-top`의 기준. 이 둘은 읽기를 많이 하고 놓은 것이 많아 할당기가
 *                  페이지를 돌려주는 박자(`LAYER_SETTLE_MS`)를 기다린 뒤 잰다
 *   gallery        스크린숏 증거 카드 360장을 끝까지 내려 본 갤러리(그림은 사람의 카탈로그에서
 *                  잰 크기 분포: 1280×720·3460×2464·640×480·925×601, 압축 크기 중앙값 약 271 KiB)
 *   knowledge      합성 볼트 1,500쪽의 지식 그래프(평면 지도)
 *   sidebar        코디 셋·워커 다섯·체크아웃 열셋의 합성 원장이 선 사이드바(t-56141 `sidebar-team`의 장면) —
 *                  팀 보기가 있는 창은 팀을, 없는 창(기준 커밋)은 가지 목록을 그린다
 *   layers         숨긴 것이 쥔 그래픽(t-80604 r2): 기기 픽셀 2배 창에 판 8개(일곱은 숨은 탭), 에이전트
 *                  궤도 보드와 지식 그래프를 한 번 열었다가 마지막 판으로 돌아온 화면. `layers-no-orbit`·
 *                  `layers-no-knowledge`·`layers-panes`는 그중 하나씩 뺀 같은 장면 — 차이가 그 후보의 몫이다.
 *                  `layers-chat`은 `layers-panes`에 400턴 대화 하나를 열었다가 판으로 돌아와 그 페이지를 뒤에
 *                  세운 장면(`.is-behind`, 창은 많아야 두 장을 세워 둔다 — `CHAT_PARK`). `layers-chat-gone`은 같은
 *                  장면에서 뒤에 선 페이지를 창이 메모리를 돌려줄 때처럼 문서에서 뺀 것(`sweepBehind({ tight })`) —
 *                  둘의 차이가 뒤에 선 페이지의 몫이다. `layers-universe`는 지식 그래프를 입체 우주(3D)로 연
 *                  같은 장면 — `layers`(평면 지도)와의 차이가 판 뒤에 숨은 우주의 몫이다(t-82914, M1 r2 P3-7).
 *                  WebContent와 GPU 프로세스를 함께 읽고, 그래픽 표면(`(graphics)` 범주)을 따로 센다.
 *
 * 두 릴리즈의 `ui/`를 같은 하네스로 비교하려면 `ZEROCODE_WINDOW_UI=<다른 트리의 ui>`로 띄운다(window-boot.mjs).
 *
 * 저사양: 스크립트 전체를 `taskpolicy -b`(효율 코어·낮은 I/O 우선순위)로 돌리고 `--low-spec`로
 * 표시한다 — 띄운 브라우저도 그 정책을 물려받는다.
 *
 * 쉬는 비용(`--idle-s N`, t-82914): 장면을 세우고 가라앉힌 뒤 아무것도 하지 않는 N초 동안 페이지와
 * GPU 프로세스의 CPU·깨어남(tools/memory-census/idle.py, 커널 카운터), 명령별 IPC 횟수(`__COUNTS__`),
 * 타이머·rAF가 몇 번 울렸는지를 건 자리(파일:줄·함수)별로, Chromium에서는 CDP `Performance.getMetrics`의
 * 메인 스레드 시간을 잰다. 타이머를 세는 손은 페이지가 뜨기 전에 깔리므로 이 옵션을 준 판의 메모리 수는
 * 그 손의 몫을 조금 더 진다 — 메모리 판과 쉬는 판은 따로 돌린다. cpu4x(CDP 스로틀)는 페이지를 멈춰 세우는
 * 방식이라 운영체제가 보는 CPU %를 올리지 않는다: 쉬는 CPU는 normal·low에서 읽는다.
 *
 * 판 전환(`--switch-rounds N`, t-82914): `panes-*` 장면에서 탭을 하나씩 앞에 세우기를 N바퀴 — 앞에 세운
 * 때부터 그 프레임이 그려진 뒤(rAF 안에서 부친 메시지가 도는 때)까지, p50·p95. 다음 프레임을 기다리는
 * 몫(60 Hz에서 많아야 16.7 ms)이 들어 있다.
 *
 *   node ui/tests/memory-census.mjs [--engine webkit|chromium] [--scenarios a,b] [--low-spec] [--idle-s 60]
 *                                   [--switch-rounds 3] [--json out.json]
 */
import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { crc32, deflateSync } from "node:zlib";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { conversationFixture, openFixtureConversation, standingPids, webkitType } from "./conversation-perf.mjs";
import { installTerminalWaits } from "./terminal-grid-selection.mjs";
import { REAL_PANE, terminalFrameFixtureSource } from "./terminal-frames.mjs";
import { seedKnowledgeWindow } from "./knowledge-fixture.mjs";
import { seedUniverseVault } from "./knowledge-universe-fixture.mjs";
import { standTeamScene } from "./sidebar-team.mjs";
import { openOrbit } from "./board-orbit.mjs";
import { standLongPaneChats } from "./conversation-earlier.mjs";

export const SCENARIOS = Object.freeze([
  "empty", "panes-1", "panes-10", "panes-30", "scrollback", "conversation", "gallery", "knowledge", "sidebar",
  "layers", "layers-no-orbit", "layers-no-knowledge", "layers-panes", "layers-chat", "layers-chat-gone", "layers-universe",
  "chat-long", "chat-long-3", "chat-long-top", "chat-long-held",
]);
/* 읽기를 많이 하고 놓은 것이 많은 장면 — 할당기가 놓은 페이지를 돌려준 뒤 잰다. */
const CHURNING = new Set(["chat-long-top", "chat-long-held"]);
/* 숨긴 것 장면의 판 수 — 사람 창의 판(8~10)에 맞춘다. 일곱은 숨은 탭이다. */
const LAYER_PANES = 8;
/* 사람의 창(레티나)과 같은 기기 픽셀 — 그래픽 표면의 크기는 이 배율의 제곱으로 간다. */
const LAYER_SCALE = 2;
/* 숨긴 것 장면을 세운 뒤 재기 전에 기다리는 시간 — WebKit 할당기가 놓은 페이지를 운영체제에 돌려주는
 * 박자(수 초)보다 길게: 2초에서는 같은 장면의 페이지 프로세스가 230·300 MiB 두 값 사이를 뛰었다. */
const LAYER_SETTLE_MS = 10000;
/* 스크롤백 장면이 흘리는 줄 수 — 창의 기본 스크롤백(`DEFAULT_SCROLLBACK_LINES`). */
const SCROLLBACK_LINES = 5000;
/* 한 프레임에 밀려 올라가는 줄 — 빌드 로그가 몰아칠 때의 한 당김. */
const SCROLL_STEP = 50;
/* 갤러리의 카탈로그: 증거 탭의 스크린숏 카드와, 그 탭이 아닌 보고서 몇. */
const GALLERY = Object.freeze({ screenshots: 360, reports: 40 });
/* 사람의 카탈로그에서 잰 스크린숏 크기(10-14, PNG 머리만 읽음: 940장, 1280×720 264·3460×2464 203·
 * 640×480 93·925×601 37). 합성 그림의 무늬 행 간격은 압축 크기가 잰 분포(중앙값 271 KiB, p90
 * 780 KiB)에 가깝게 고른 값이다. */
const PICTURES = Object.freeze([
  { width: 1280, height: 720, noisyEvery: 10, weight: 3 },
  { width: 3460, height: 2464, noisyEvery: 33, weight: 2 },
  { width: 640, height: 480, noisyEvery: 10, weight: 1 },
  { width: 925, height: 601, noisyEvery: 11, weight: 1 },
]);
const KNOWLEDGE_PAGES = 1500;
/* 장면을 세운 뒤 재기 전에 기다리는 시간 — 지연 그리기와 디코드가 끝나도록. */
const SETTLE_MS = 2000;
/* 장면 하나를 세우는 데 주는 시간. 넘으면 그 장면은 실패로 적고 다음 장면으로 간다 — 한 장면이
 * 멈춰 나머지 장면의 수까지 잃지 않게. */
const SCENARIO_PATIENCE_MS = 180000;

/* 쉬는 판에서 출처별로 남기는 줄 수 — 표는 상위 다섯을 쓰고, 나머지 몇 줄은 순위가 흔들릴 때를 위해. */
const IDLE_TOP = 8;
const IDLE_PROBE = fileURLToPath(new URL("../../tools/memory-census/idle.py", import.meta.url));

/* 타이머·rAF를 건 자리마다 울린 수를 세는 손. 페이지 스크립트보다 먼저 깔려 창 코드가 잡아 두는
 * `setTimeout` 따위도 이 손을 거친다. 자리는 스택에서 처음 만나는 창 파일(`*.js:줄`)과 그 함수 이름이다;
 * 창 파일이 없는 스택(하네스의 스텁)은 `harness`로 묶는다. 워커 안의 타이머는 세지 못한다. */
export const installTimerCensus = () => {
  const fired = new Map();
  const site = (kind) => {
    for (const line of String(new Error().stack).split("\n").slice(1)) {
      // WebKit spells a frame `name@url:line:col`, Chromium `    at name (url:line:col)`.
      // The hand's own frames come first, and WebKit files them under whatever script was loading.
      const at = line.match(/^([^@\s]*)@.*\/([\w.-]+\.js):(\d+):\d+/) ?? line.match(/(?:at\s+([^\s(]+)\s+\()?[^\s(@]*?\/([\w.-]+\.js):(\d+):\d+/);
      if (at && !/(?:^|\.)(?:site|census|counted)$/.test(at[1] ?? "")) return `${kind} ${at[1] || "(anonymous)"} ${at[2]}:${at[3]}`;
    }
    return `${kind} harness`;
  };
  const wrap = (kind, original) => function census(callback, ...rest) {
    if (typeof callback !== "function") return original.call(this, callback, ...rest);
    const where = site(kind);
    return original.call(this, function counted(...args) {
      fired.set(where, (fired.get(where) ?? 0) + 1);
      return callback.apply(this, args);
    }, ...rest);
  };
  window.setTimeout = wrap("timeout", window.setTimeout);
  window.setInterval = wrap("interval", window.setInterval);
  window.requestAnimationFrame = wrap("raf", window.requestAnimationFrame);
  if (window.requestIdleCallback) window.requestIdleCallback = wrap("idle", window.requestIdleCallback);
  window.__TIMER_FIRES__ = () => Object.fromEntries(fired);
};

/* 커널 카운터로 읽은 프로세스들의 쉬는 비용 — idle.py 한 번, `seconds` 동안. 노드를 막지 않는다. */
function idleCost(processes, seconds, folder) {
  const out = join(folder, `idle-${Date.now()}`);
  const args = [IDLE_PROBE, "--out", out, "--seconds", String(seconds), "--repeat", "1", "--threads", "4",
    ...Object.entries(processes).filter(([, pid]) => pid !== null).flatMap(([role, pid]) => ["--process", `${role}=${pid}`])];
  return new Promise((done, fail) => execFile("python3", args, { encoding: "utf8" }, (error) => {
    if (error) return fail(error);
    const line = JSON.parse(readFileSync(join(out, "idle.jsonl"), "utf8").trim().split("\n")[0]);
    rmSync(out, { recursive: true });
    done(line);
  }));
}

const perMinute = (before, after, seconds) => Object.entries(after)
  .map(([key, count]) => [key, Math.round(((count - (before[key] ?? 0)) * 60) / seconds * 10) / 10])
  .filter(([, rate]) => rate > 0).sort((a, b) => b[1] - a[1]);

/* 판 전환의 첫 그림: 탭마다 앞에 세운 때부터 그 프레임이 그려진 뒤까지. */
function measureSwitch(page, rounds) {
  return page.evaluate(async (rounds) => {
    const terms = window.__CENSUS_TERMS__ ?? [];
    const painted = () => new Promise((done) => requestAnimationFrame(() => {
      const channel = new MessageChannel();
      channel.port1.onmessage = done;
      channel.port2.postMessage(null);
    }));
    await painted();
    const taken = [];
    for (let round = 0; round < rounds; round += 1) {
      for (const term of terms) {
        const started = performance.now();
        setActiveTab(tabOfTerm(term).id);
        await painted();
        taken.push(performance.now() - started);
      }
    }
    taken.sort((a, b) => a - b);
    const at = (q) => Math.round(taken[Math.min(taken.length - 1, Math.floor(taken.length * q))] * 10) / 10;
    return taken.length ? { switches: taken.length, p50Ms: at(0.5), p95Ms: at(0.95), maxMs: at(1) } : null;
  }, rounds);
}

/* 장면이 선 페이지가 `seconds` 동안 아무것도 안 할 때 드는 것. */
async function measureIdle({ page, cdp, pids, seconds, folder }) {
  const counts = () => page.evaluate(() => ({ invokes: { ...window.__COUNTS__ }, timers: window.__TIMER_FIRES__?.() ?? {} }));
  const metrics = async () => (cdp ? Object.fromEntries((await cdp.send("Performance.getMetrics")).metrics.map((one) => [one.name, one.value])) : null);
  if (cdp) await cdp.send("Performance.enable");
  const before = await counts();
  const cdpBefore = await metrics();
  const kernel = await idleCost(pids, seconds, folder);
  const after = await counts();
  const cdpAfter = await metrics();
  const invokes = perMinute(before.invokes, after.invokes, kernel.wall_s);
  const timers = perMinute(before.timers, after.timers, kernel.wall_s);
  const role = (name) => kernel.rows.find((row) => row.role === name);
  const msPerMinute = (name) => (cdpAfter ? Math.round(((cdpAfter[name] - cdpBefore[name]) * 1000 * 60) / kernel.wall_s * 10) / 10 : null);
  return {
    seconds: kernel.wall_s, load1: kernel.conditions.load1, loud: kernel.conditions.loud,
    pageCpuPct: role("page")?.cpu_pct ?? null, pageIdleWakeupsPerS: role("page")?.idle_wakeups_per_s ?? null,
    pageInterruptWakeupsPerS: role("page")?.interrupt_wakeups_per_s ?? null,
    gpuCpuPct: role("gpu")?.cpu_pct ?? null, gpuInterruptWakeupsPerS: role("gpu")?.interrupt_wakeups_per_s ?? null,
    invokesPerMin: invokes.reduce((sum, [, rate]) => sum + rate, 0),
    timerFiresPerMin: timers.reduce((sum, [, rate]) => sum + rate, 0),
    rafPerMin: timers.filter(([site]) => site.startsWith("raf ")).reduce((sum, [, rate]) => sum + rate, 0),
    taskMsPerMin: msPerMinute("TaskDuration"), scriptMsPerMin: msPerMinute("ScriptDuration"),
    layoutMsPerMin: msPerMinute("LayoutDuration"), styleMsPerMin: msPerMinute("RecalcStyleDuration"),
    topInvokes: invokes.slice(0, IDLE_TOP), topTimers: timers.slice(0, IDLE_TOP),
    threads: kernel.threads,
  };
}

/* `promise`, 또는 `ms` 뒤의 실패. */
function within(ms, promise, what) {
  let timer;
  const late = new Promise((unused, fail) => {
    timer = setTimeout(() => fail(new Error(`${what}: not done in ${ms} ms`)), ms);
  });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
}

/* 합성 PNG 하나: 창 배경 같은 평평한 행 사이사이에 무늬 행. 어떤 사람의 화면도 아니다. */
export function syntheticPng({ width, height, noisyEvery }) {
  const stride = width * 4 + 1;
  const raw = Buffer.alloc(stride * height);
  let seed = (width * 31 + height) >>> 0;
  for (let y = 0; y < height; y += 1) {
    const at = y * stride;
    const noisy = y % noisyEvery === 0;
    for (let x = 0; x < width; x += 1) {
      const pixel = at + 1 + x * 4;
      if (noisy) {
        seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
        raw[pixel] = seed & 255;
        raw[pixel + 1] = (seed >>> 8) & 255;
        raw[pixel + 2] = (seed >>> 16) & 255;
      } else {
        raw[pixel] = 236;
        raw[pixel + 1] = 238;
        raw[pixel + 2] = 241;
      }
      raw[pixel + 3] = 255;
    }
  }
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const check = Buffer.alloc(4);
    check.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, check]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/* ---- 프로세스 읽기: 이 스크립트가 띄운 것만 ---------------------------------------- */

function processTable() {
  const said = execFileSync("ps", ["-axo", "pid=,ppid=,rss=,command="], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  return said.split("\n").map((line) => line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/))
    .filter(Boolean).map((match) => ({ pid: Number(match[1]), ppid: Number(match[2]), rss: Number(match[3]), command: match[4] }));
}

/* Of the processes that could be the page's, the one holding the most — a browser keeps a spare
 * content process beside the page's, and the spare holds next to nothing. */
const largest = (rows) => rows.reduce((most, row) => (most === null || row.rss > most.rss ? row : most), null)?.pid ?? null;

/* 페이지가 살 수 있는 프로세스들: WebKit은 Playwright 빌드의 WebContent(launchd 아래 XPC), Chromium은
 * 이 노드가 띄운 브라우저의 렌더러. `before`는 브라우저를 띄우기 전에 서 있던 pid들이다. 페이지를
 * 여럿 띄운 하네스(artifact-page-perf)는 이것을 다 읽는다. */
export function pageProcesses(engine, before) {
  const rows = processTable();
  if (engine === "webkit") {
    return rows.filter((row) => /ms-playwright\/webkit-\d+/.test(row.command) && /WebContent/.test(row.command)
      && !before.has(row.pid));
  }
  const browsers = new Set(rows.filter((row) => row.ppid === process.pid).map((row) => row.pid));
  return rows.filter((row) => browsers.has(row.ppid) && /--type=renderer/.test(row.command) && !before.has(row.pid));
}

/* 페이지의 그림을 받아 그리는 GPU 프로세스 — WebGL의 타깃과 텍스처는 여기에 산다. */
function gpuProcess(engine, before) {
  const rows = processTable();
  if (engine === "webkit") {
    return largest(rows.filter((row) => /ms-playwright\/webkit-\d+/.test(row.command) && /WebKit\.GPU/.test(row.command)
      && !before.has(row.pid)));
  }
  const browsers = new Set(rows.filter((row) => row.ppid === process.pid).map((row) => row.pid));
  return largest(rows.filter((row) => browsers.has(row.ppid) && /--type=gpu-process/.test(row.command)
    && !before.has(row.pid)));
}

/* 페이지가 사는 프로세스 하나. */
function pageProcess(engine, before) {
  return largest(pageProcesses(engine, before));
}

export function footprintOf(pid, folder) {
  const path = join(folder, `footprint-${pid}.json`);
  execFileSync("footprint", ["-j", path, "-f", "bytes", "--swapped", "-p", String(pid)], { stdio: "ignore" });
  const read = JSON.parse(readFileSync(path, "utf8")).processes[0];
  rmSync(path);
  const categories = Object.entries(read.categories ?? {});
  const sum = (pick) => categories.reduce((total, [name, one]) => total + pick(name, one), 0);
  return {
    footprint: read.footprint,
    peak: read.auxiliary?.phys_footprint_peak ?? null,
    // Surfaces the process owns: layer backing stores and canvases in a
    // page's process, render targets and textures in a GPU process.
    graphics: sum((name, one) => (name.includes("(graphics)") ? one.dirty ?? 0 : 0)),
    dirty: sum((name, one) => one.dirty ?? 0),
    swapped: sum((name, one) => one.swapped ?? 0),
    malloc: sum((name, one) => (/malloc/i.test(name) ? (one.dirty ?? 0) + (one.swapped ?? 0) : 0)),
    top: categories.map(([name, one]) => [name, (one.dirty ?? 0) + (one.swapped ?? 0)])
      .sort((a, b) => b[1] - a[1]).slice(0, 6),
  };
}

function heapOf(pid) {
  const said = execFileSync("heap", ["-q", "--showSizes", String(pid)], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  const totals = said.match(/^All zones: (\d+) nodes \((\d+) bytes\)/m);
  const sizes = said.match(/^All zones: \d+ nodes malloced - Sizes: (.*)$/m);
  return {
    nodes: totals ? Number(totals[1]) : null,
    bytes: totals ? Number(totals[2]) : null,
    histogram: sizes ? sizes[1].trim().split(/\s+/).slice(0, 24).join(" ") : null,
  };
}

/* ---- 장면 ------------------------------------------------------------------------ */

async function openPanes(page, count) {
  await installTerminalWaits(page);
  return page.evaluate(async ({ count, fixtureSource, pane }) => {
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const { packedRow, base } = new Function(`return (${fixtureSource})`)()(pane.rows, pane.cols);
    const terms = [];
    for (let at = 0; at < count; at += 1) {
      const term = await openTermTab({ placement: "tab" });
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      await window.__TERM_FEED__(term, base({
        full: true, rows: Array.from({ length: pane.rows }, (unused, row) => packedRow(row, at * pane.rows + row)),
      }));
      terms.push(term);
    }
    window.__CENSUS_TERMS__ = terms;
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    return { panes: terms.length, rows: document.querySelectorAll(".term-row").length };
  }, { count, fixtureSource: terminalFrameFixtureSource, pane: REAL_PANE });
}

async function scrollHistory(page) {
  await installTerminalWaits(page);
  return page.evaluate(async ({ fixtureSource, pane, lines, step }) => {
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const { packedRow, base } = new Function(`return (${fixtureSource})`)()(pane.rows, pane.cols);
    const term = await openTermTab({ placement: "tab" });
    const view = termView(term);
    setActiveTab(tabOfTerm(term).id);
    await window.__TERMINAL_READY__(term, view);
    await window.__TERM_FEED__(term, base({ full: true, rows: Array.from({ length: pane.rows }, (u, row) => packedRow(row, row)) }));
    // 빌드 로그가 흐른다: 한 프레임에 `step`줄씩 밀려 올라가고 아래 줄만 온다.
    for (let written = pane.rows; written < lines + pane.rows; written += step) {
      await window.__TERM_FEED__(term, base({
        scrolled_lines: step, scrollback_len: written - pane.rows + step,
        rows: Array.from({ length: step }, (u, at) => packedRow(pane.rows - step + at, written + at)),
      }));
    }
    // 그리고 히스토리 한가운데를 본다 — 창은 그 화면의 행만 든다.
    const offset = Math.floor(lines / 2);
    await window.__TERM_FEED__(term, base({
      full: true, view_offset: offset, scrollback_len: lines,
      rows: Array.from({ length: pane.rows }, (u, row) => packedRow(row, lines - offset + row)),
    }));
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    return { rows: document.querySelectorAll(".term-row").length };
  }, { fixtureSource: terminalFrameFixtureSource, pane: REAL_PANE, lines: SCROLLBACK_LINES, step: SCROLL_STEP });
}

async function openGallery(page, pictures) {
  return page.evaluate(async ({ pictures, catalog }) => {
    const now = Date.now();
    const minute = 60 * 1000;
    const rows = [];
    const classes = pictures.flatMap((picture, at) => Array(picture.weight).fill(at));
    for (let at = 0; at < catalog.screenshots + catalog.reports; at += 1) {
      const shot = at < catalog.screenshots;
      const id = `row-${String(at).padStart(4, "0")}`;
      const picture = pictures[classes[at % classes.length]];
      rows.push({
        id, kind: shot ? "screenshot" : "report", title: `합성 ${shot ? "스크린숏" : "보고서"} ${at}`,
        path: shot ? `/tmp/zerocode-window-test/computer-use/sessions/s-${at % 60}/${at}.png` : `/data/artifacts/run/${id}/REPORT.md`,
        bytes: shot ? picture.bytes : 9000, created_ms: now - at * minute, modified_ms: now - at * minute,
        origin: shot ? { automation: "computer-use" } : {}, tags: [],
        preview: shot ? { kind: "image", w: picture.width, h: picture.height } : { kind: "markdown", text: "합성 보고서" },
        source: shot ? "evidence" : "worker_report",
      });
    }
    window.__CENSUS_PREVIEWS__ = 0;
    window.__ANSWER__.artifacts_list = () => ({
      rows, total: rows.length, truncated: false, missing: [], missing_total: 0, missing_by_kind: {},
      unlinked_by_kind: {}, by_kind: { screenshot: catalog.screenshots, report: catalog.reports },
      retention_days: 30, thumb: { width: 320, height: 240, queue_max: 2 },
    });
    window.__ANSWER__.artifact_counts = () => ({
      total: rows.length, by_worker: {}, by_task: {}, by_run: {}, by_worktree: {}, by_automation: {}, report_by_worker: {},
    });
    window.__ANSWER__.artifact_thumbnail = () => ({ data_url: null, cached: false });
    // 런타임이 하듯 답마다 새 문자열: IPC의 답은 JSON을 거쳐 늘 새로 만들어진다.
    window.__ANSWER__.artifact_preview = (args) => {
      const row = rows.find((one) => one.id === args.id);
      if (row?.kind !== "screenshot") return { kind: "markdown", text: "합성 보고서", bytes: 12, truncated: false };
      window.__CENSUS_PREVIEWS__ += 1;
      const picture = pictures[classes[Number(row.id.slice(4)) % classes.length]];
      return JSON.parse(JSON.stringify({
        kind: "image", data_url: `data:image/png;base64,${picture.base64}`, bytes: picture.bytes, truncated: false,
      }));
    };
    dropTab("artifacts");
    artifactFilter.tab = "evidence";
    artifactFilter.origin = null;
    artifactTabPicked = true;
    artifactSelectedId = null;
    el("nav-artifacts").click();
    const frame = () => new Promise((done) => requestAnimationFrame(done));
    for (let beat = 0; beat < 600 && (artifactAsking || artifactRows.size === 0); beat += 1) await frame();
    const view = artifactsView();
    const grid = view.querySelector(".artifacts-grid");
    // 끝까지 내려 본다 — 가상화된 카드가 차례로 서고, 서는 카드마다 제 그림을 청할 수 있다.
    for (let top = 0; top <= grid.scrollHeight; top += Math.max(200, grid.clientHeight / 2)) {
      grid.scrollTop = top;
      grid.dispatchEvent(new Event("scroll"));
      await frame();
      await new Promise((done) => setTimeout(done, 30));
    }
    await new Promise((done) => setTimeout(done, 300));
    const held = [...artifactPreviews.values()].reduce((sum, one) => sum + (one?.data_url?.length ?? 0), 0);
    return {
      tab: artifactFilter.tab, cards: view.querySelectorAll(".artifact-card").length,
      previewAsks: window.__CENSUS_PREVIEWS__, previewsHeld: artifactPreviews.size, previewBytesHeld: held,
    };
  }, { pictures, catalog: GALLERY });
}

/* 숨긴 것 장면: 판 `LAYER_PANES`개를 탭으로 열고, 궤도 보드와 지식 그래프를 한 번씩 연 뒤 마지막
 * 판으로 돌아온다 — 사람이 보드와 그래프를 들여다보고 일하던 판으로 돌아온 창. 보드와 그래프는
 * 닫히지 않고 숨는다. 돌아온 뒤에 무엇이 보이는지 적는다: 숨김이 장면의 전제다. */
async function standLayers(page, { orbit, knowledge, chat, chatGone, dimension }) {
  const detail = await openPanes(page, LAYER_PANES);
  if (chat) await openFixtureConversation(page, conversationFixture());
  if (orbit) {
    await openOrbit(page);
    Object.assign(detail, await page.evaluate(() => ({ orbitViews: agentOrbitViews.size, orbitBroken: agentOrbitBroken,
      orbitBloom: window.__ORBIT__?.()?.bloom ?? null })));
  }
  if (knowledge) {
    Object.assign(detail, await openKnowledge(page, { dimension }));
    detail.knowledgePainter = await page.evaluate(() => {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      return view === null ? null : knowledgePainters.get(view)?.id ?? (knowledgeUniverses.has(view) ? "universe" : null);
    });
  }
  return { ...detail, ...await page.evaluate(async (gone) => {
    setActiveTab(tabOfTerm(window.__CENSUS_TERMS__.at(-1)).id);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    if (gone) {
      sweepBehind({ tight: true });
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }
    const shows = (selector) => document.querySelector(selector)?.checkVisibility() ?? null;
    return { boardShown: shows("#board-view"), knowledgeShown: shows(".knowledge-view"),
      termShown: termView(window.__CENSUS_TERMS__.at(-1)).host.checkVisibility(), chatsBehind: document.querySelectorAll(".is-behind").length,
      // Animations still running inside a page set behind: each one is a frame drawn for nobody.
      behindAnimations: document.getAnimations().filter((one) => one.playState === "running"
        && one.effect?.target?.closest?.(".is-behind")).length };
  }, Boolean(chatGone)) };
}

/* The graph as the person last left it: the flat map (`2d`) or the universe (`3d`), which stands once it is mounted. */
async function openKnowledge(page, { dimension = "2d" } = {}) {
  await page.evaluate(({ pages, dimension }) => {
    secondBrainVault = "/universe";
    window.__VAULT__ = window.__universeVaultSpec__(pages);
    noteKnowledgeExploreLines({ "/universe": JSON.stringify({ mode: "global", dimension }) });
    document.getElementById("nav-knowledge").click();
  }, { pages: KNOWLEDGE_PAGES, dimension });
  await page.waitForFunction(({ pages, dimension }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    if (view === null) return false;
    if (dimension === "3d") return knowledgeUniverses.has(view) && view.classList.contains("is-universe");
    const layout = knowledgeLayouts.get(view);
    return layout !== undefined && layout.count >= pages && layout.left === 0;
  }, { pages: KNOWLEDGE_PAGES, dimension }, { timeout: SCENARIO_PATIENCE_MS, polling: 250 });
  return page.evaluate(() => ({ nodes: document.querySelectorAll(".knowledge-view:not([hidden]) *").length }));
}

/* ---- 한 장면, 한 브라우저 ------------------------------------------------------------ */

async function measureScenario({ engine, name, origin, pictures, folder, idleS = 0, switchRounds = 0 }) {
  const before = standingPids();
  const type = engine === "webkit" ? webkitType() : chromium;
  const browser = await type.launch({ headless: true });
  try {
    let page;
    let faults = [];
    const layers = name.startsWith("layers");
    if (layers) {
      // The window's own fixtures (terminals, board, conversation) with the vault the knowledge view reads.
      ({ page, faults } = await openWindowTestPage(browser, origin, { before: async (one) => {
        if (idleS > 0) await one.addInitScript(installTimerCensus);
        await seedUniverseVault(one);
      },
        context: { viewport: { width: 1440, height: 900 }, deviceScaleFactor: LAYER_SCALE,
          // The person's window moves: an animation is part of what a layer costs.
          reducedMotion: "no-preference" } }));
    } else if (name === "knowledge") {
      const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
      page = await context.newPage();
      page.on("pageerror", (error) => faults.push(String(error).slice(0, 200)));
      if (idleS > 0) await page.addInitScript(installTimerCensus);
      await seedKnowledgeWindow(page);
      await seedUniverseVault(page);
      await page.goto(`${origin}/index.html`);
      await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
    } else {
      ({ page, faults } = await openWindowTestPage(browser, origin,
        { before: idleS > 0 ? (one) => one.addInitScript(installTimerCensus) : null }));
    }
    const cdp = engine === "chromium" ? await page.context().newCDPSession(page) : null;
    const stand = async () => {
      if (name.startsWith("panes-")) return openPanes(page, Number(name.slice("panes-".length)));
      if (name === "scrollback") return scrollHistory(page);
      if (name === "conversation") return (await openFixtureConversation(page, conversationFixture())) ?? {};
      if (name === "chat-long") return standLongPaneChats(page);
      if (name === "chat-long-3") return standLongPaneChats(page, { panes: 3 });
      if (name === "chat-long-top") return standLongPaneChats(page, { toTop: true });
      if (name === "chat-long-held") return standLongPaneChats(page, { helper: true });
      if (name === "gallery") return openGallery(page, pictures);
      if (name === "knowledge") return openKnowledge(page);
      if (name === "sidebar") return standTeamScene(page).then(() => page.evaluate(() => window.__ROWS__()));
      if (layers) {
        const chat = name.startsWith("layers-chat");
        return standLayers(page, { orbit: !["layers-no-orbit", "layers-panes"].includes(name) && !chat,
          knowledge: !["layers-no-knowledge", "layers-panes"].includes(name) && !chat,
          chat, chatGone: name === "layers-chat-gone", dimension: name === "layers-universe" ? "3d" : "2d" });
      }
      if (name !== "empty") throw new Error(`no scenario named ${name}`);
      return {};
    };
    const detail = await within(SCENARIO_PATIENCE_MS, stand(), name);
    await page.waitForTimeout(layers || CHURNING.has(name) ? LAYER_SETTLE_MS : SETTLE_MS);
    const row = { engine, scenario: name, ...detail };
    if (switchRounds > 0 && name.startsWith("panes-")) row.switch = await measureSwitch(page, switchRounds);
    if (cdp) {
      await cdp.send("HeapProfiler.collectGarbage");
      await cdp.send("HeapProfiler.collectGarbage");
      const usage = await cdp.send("Runtime.getHeapUsage");
      const counters = await cdp.send("Memory.getDOMCounters");
      Object.assign(row, { jsHeap: usage.usedSize, embedderHeap: usage.embedderHeapUsedSize ?? 0,
        domNodes: counters.nodes, listeners: counters.jsEventListeners });
    }
    const pid = pageProcess(engine, before);
    row.pid = pid;
    if (idleS > 0) {
      row.idle = await measureIdle({ page, cdp, pids: { page: pid, gpu: gpuProcess(engine, before) }, seconds: idleS, folder });
    }
    if (pid !== null) {
      row.process = footprintOf(pid, folder);
      if (engine === "webkit") row.heap = heapOf(pid);
    }
    const gpu = gpuProcess(engine, before);
    if (gpu !== null) row.gpu = footprintOf(gpu, folder);
    row.faults = faults.slice(0, 3);
    return row;
  } finally {
    await browser.close();
  }
}

export async function census({ engine = "webkit", scenarios = SCENARIOS, lowSpec = false, idleS = 0, switchRounds = 0, out = null } = {}) {
  const pictures = PICTURES.map((picture) => {
    const bytes = syntheticPng(picture);
    return { ...picture, bytes: bytes.length, base64: bytes.toString("base64") };
  });
  const folder = mkdtempSync(join(tmpdir(), "zerocode-memory-census-"));
  const { files, origin } = await createWindowServer();
  const rows = [];
  try {
    for (const name of scenarios) {
      let row;
      try {
        row = await measureScenario({ engine, name, origin, pictures, folder, idleS, switchRounds });
      } catch (error) {
        row = { engine, scenario: name, error: String(error?.stack ?? error).slice(0, 600) };
      }
      rows.push({ lowSpec, ...row });
      if (out) writeFileSync(out, `${JSON.stringify({ engine, lowSpec, rows }, null, 1)}\n`);
      const mib = (bytes) => (bytes === undefined || bytes === null ? "-" : (bytes / 1048576).toFixed(1));
      console.log(`${engine.padEnd(8)} ${lowSpec ? "low " : "    "}${name.padEnd(13)} footprint ${mib(row.process?.footprint)} MiB`
        + ` dirty ${mib(row.process?.dirty)} malloc ${mib(row.process?.malloc)} graphics ${mib(row.process?.graphics)}`
        + (row.gpu ? ` gpu ${mib(row.gpu.footprint)} MiB graphics ${mib(row.gpu.graphics)}` : "")
        + (row.heap ? ` heap ${mib(row.heap.bytes)} MiB/${row.heap.nodes} nodes` : "")
        + (row.jsHeap !== undefined ? ` js ${mib(row.jsHeap)} MiB dom ${row.domNodes}` : "")
        + (row.previewBytesHeld !== undefined ? ` previews ${row.previewsHeld}/${row.previewAsks} asks ${mib(row.previewBytesHeld)} MiB held` : "")
        + (row.idle ? ` idle page ${row.idle.pageCpuPct}% gpu ${row.idle.gpuCpuPct}% ipc ${row.idle.invokesPerMin}/min`
          + ` timers ${row.idle.timerFiresPerMin}/min raf ${row.idle.rafPerMin}/min` : "")
        + (row.switch ? ` switch p50 ${row.switch.p50Ms} ms p95 ${row.switch.p95Ms} ms` : "")
        + (row.faults?.length ? ` FAULT ${row.faults[0]}` : "")
        + (row.error ? ` ERROR ${row.error.split("\n")[0]}` : ""));
    }
  } finally {
    files.close();
    rmdirSync(folder);
  }
  return { engine, lowSpec, pictures: pictures.map(({ base64, ...kept }) => kept), rows, summary: summaryOf(rows) };
}

/* 장면마다 수 몇 개 — tools/low-spec/profile.mjs가 판들의 중앙값을 내는 모양(`summary`). */
function summaryOf(rows) {
  const kept = {};
  for (const row of rows) {
    if (row.error) continue;
    const idle = row.idle ?? {};
    kept[row.scenario] = Object.fromEntries(Object.entries({
      footprintMiB: row.process?.footprint, dirtyMiB: row.process?.dirty, graphicsMiB: row.process?.graphics,
      gpuFootprintMiB: row.gpu?.footprint, gpuGraphicsMiB: row.gpu?.graphics, jsHeapMiB: row.jsHeap,
    }).filter(([, bytes]) => typeof bytes === "number").map(([key, bytes]) => [key, Math.round(bytes / 10485.76) / 100])
      .concat(Object.entries({ domNodes: row.domNodes, idlePageCpuPct: idle.pageCpuPct, idleGpuCpuPct: idle.gpuCpuPct,
        idlePageInterruptWakeupsPerS: idle.pageInterruptWakeupsPerS, idleInvokesPerMin: idle.invokesPerMin,
        idleTimerFiresPerMin: idle.timerFiresPerMin, idleRafPerMin: idle.rafPerMin, idleTaskMsPerMin: idle.taskMsPerMin,
        idleScriptMsPerMin: idle.scriptMsPerMin, switchP50Ms: row.switch?.p50Ms, switchP95Ms: row.switch?.p95Ms })
        .filter(([, value]) => typeof value === "number")));
  }
  return kept;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const scenarios = option("--scenarios", SCENARIOS.join(",")).split(",").filter(Boolean);
  const out = option("--json", null);
  const result = await census({ engine: option("--engine", "webkit"), scenarios, lowSpec: argv.includes("--low-spec"),
    idleS: Number(option("--idle-s", "0")), switchRounds: Number(option("--switch-rounds", "0")), out });
  if (out) writeFileSync(out, `${JSON.stringify(result, null, 1)}\n`);
  // A scenario that could not be stood is a failed census, its row says why.
  if (result.rows.some((row) => row.error)) process.exitCode = 1;
}
