/* 저사양 재현 — 생산에서 그림까지의 박자 (M2).
 *
 * docs/design/terminal-display-paced-frames-20260917.md §3 의 M2 이고, §6 의
 * 합격선을 단정한다. 보고는 「저사양 PC에서 버벅거린다」였고, 그 문장을 게이트에
 * 세울 수 있는 모양으로 옮긴 것이 이것이다.
 *
 * **무엇을 흉내 내는가.** 느린 기계의 웹뷰. Windows 의 WebView2 가 Chromium
 * 이므로 CDP `Emulation.setCPUThrottlingRate` 로 페이지의 메인 스레드를 1·4·6배
 * 느리게 한다. 백엔드는 **다른 프로세스**라 느려지지 않는다 — 그래서 생산자는
 * 페이지가 아니라 워커에 산다. 스로틀은 페이지의 메인 스레드에 걸리고, 워커는
 * 제 박자로 계속 만든다. 워커가 메인 스레드로 부치는 메시지는 메인 스레드가
 * 바쁜 동안 줄을 서므로, 이벤트가 IPC 에 줄을 서는 모양이 그대로 선다(가설 H1).
 *
 * **생산자는 펌프의 모양이다.** 펌프와 같은 박자(`PUMP_INTERVAL`, 펌프 소스에서
 * 읽는다)로 60×220 화면에 두 무늬를 만든다 — 빌드 로그 스크롤(`scroll`)과 TUI
 * 전체 다시 그리기(`redraw`). 화면의 판마다 판 번호(`__v`)와 만든 시각(`__at`)을
 * 싣는다. 가져가기(`take`)는 그리드의 규칙대로 마지막으로 가져간 뒤의 변경을
 * 한 장으로 합친다: 스크롤은 줄 수를 더하고 노출된 아래만, 다시 그리기는 마지막
 * 화면만. `redraw` 는 동기화 출력(`CSI ?2026h … ?2026l`)으로 다시 그리는 Claude
 * Code 판의 모양이기도 하다 — 그리드는 닫히지 않은 원자 프레임을 내주지 않으므로
 * 가져가는 쪽이 보는 것은 언제나 다 그린 화면 한 장이다.
 *
 * **길은 창이 당기는 길이다** (설계 §4). 워커는 백엔드의 독자 장부
 * (`zerocode-pty` readers.rs)를 한 독자만큼 흉내 낸다: 아무도 기다리지 않는
 * 독자에게만 알림(`term:dirty:<라벨>`)을 보내고, 당김(`term_pull`)에는 몫과 새로
 * 가져간 것을 JSON 바이트 한 답으로 준다. 비지 않은 당김은 독자를 「흐르는 중」으로
 * 두고, 빈 당김은 조용히 두며, 말없이 `RENOTIFY_AFTER` 가 지나면 다시 알린다 —
 * 그 값도 소스에서 읽는다. 펌프의 추격도 같은 규칙으로 흉내 낸다(`pump_loop`,
 * `ChaseRound`): 키가 가면 쓰기가 답을 기다리기 시작하고, 펌프는 곧바로 한
 * 라운드를 돈다(깨운 라운드). 무엇이든 움직인 라운드는 추격을 끝내지만, 쓰기가
 * 답을 기다리는 동안은 끝내지 않는다. 에코는 `CHASE_INTERVAL` 뒤에 닿고, 추격이
 * 서 있으면 그 추격 라운드가, 아니면 다음 박자가 파싱한다. 답을 파싱한
 * 라운드의 look(`Look::Chase`)은 흐르는 독자에게도 알린다.
 *
 * **이웃.** 에이전트 여럿이 함께 도는 창에서는 다른 셸이 거의 늘 출력한다. 장면마다
 * 이웃이 조용한 판과 흐르는 판(`NEIGHBOURS`)을 따로 잰다 — 흐르는 이웃은 펌프의 모든
 * 라운드를 움직인다.
 *
 * **무엇을 재는가.**
 * - 생산 → 적용 → 그림: 뷰의 `applyFrames` 를 하네스가 감싸(배송된 렌더러에는
 *   계기가 없다) 한 답이 적용한 마지막 판을 적고, 다음 rAF 에서 그 판이 이 프레임의
 *   그림이 된다. 그림이
 *   **끝난** 시각은 rAF 안에서 부친 메시지가 돌아오는 때 — 그 태스크는 이 프레임의
 *   style·layout·paint 뒤에 돈다. 지연은 그 시각 − 그 판을 만든 시각. 만든 시각은
 *   워커의 시계라 페이지의 시계로 옮겨 읽는다: 둘 다 `timeOrigin + now()` 지만 두
 *   원점은 ms 단위로 어긋난다(한 장면이 −3.8 ms를 냈다). 장면마다 왕복
 *   `CLOCK_SYNC_ROUNDS` 번 중 가장 짧은 것으로 차를 재고, 그 오차(왕복의 절반)를 함께
 *   적는다. 에코를 실은 판과 출력만 실은 판은 따로도 적는다 — 에코를 빨리 만드는 길은
 *   만든 시각이 앞당겨져 이 값이 커지고, 사람이 보는 값은 아래의 키 에코다.
 * - 초별 p50·p95·최대(만든 시각으로 묶는다 — 지연이 자라는지는 나중에 만든 것이
 *   더 늦게 그려지는지로 읽힌다).
 * - 만든/적용한/그린 수, 그리고 **그려지기 전에 덮인 적용** — 한 화면 프레임 안에서
 *   마지막 것을 뺀 적용은 사람이 한 번도 못 본 DOM 일이다. 적용 하나는 DOM 을 한
 *   번 칠하는 단위, 곧 당김의 답 하나다. 덮인 적용은 누가 불렀는지로 나눈다: 키의
 *   에코를 실은 적용은 제 프레임의 덮인 적용 하나씩을 키 몫으로 가져가고(흐르는
 *   화면 위의 에코는 그 프레임의 두 번째 칠이다), 남는 것은 흐름 몫이다.
 * - **같은 프레임에 다시 쓴 행** — 덮인 적용이 실제로 버린 DOM 일. 적용마다 그
 *   적용이 쓴 행을 DOM 에서 읽고(`MutationObserver` 의 기록을 적용 앞뒤로 비운다),
 *   한 프레임 안에서 둘 이상의 적용이 쓴 행은 마지막 것을 뺀 쓰기를 센다.
 * - long task(50ms 넘은 메인 스레드 일).
 * - 키 에코: Node 가 진짜 키를 누르고(`page.keyboard`, 입력 우선순위 그대로),
 *   `keydown` 의 `timeStamp` 에서 그 글자를 실은 판이 그려질 때까지.
 *
 * **못 보는 것.** WKWebView 의 래스터·합성, 진짜 IPC 의 직렬화, Rust 의 파서.
 * M1(실제 앱, 코디네이터)이 그쪽을 본다.
 *
 * 밀어 보내던 옛 길(`term:screen`)의 숫자는 이 자의 첫 판(커밋 「test(terminal): a
 * slow machine, reproduced」)이 그 길 위에서 잰 것이고 설계 문서 §6·§7에 있다 — 그
 * 길은 이제 없으므로 이 파일에는 당기는 길만 있다. */
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { installTerminalWaits } from "./terminal-grid-selection.mjs";
import {
  BUILD_LOG_LINES_PER_ROUND,
  REAL_PANE,
  terminalFrameFixtureSource,
} from "./terminal-frames.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/* 펌프의 박자와 독자의 재알림은 그 소스에서 읽는다. 손으로 적으면 박자가 바뀌는
 * 날 이 자는 다른 기계를 재고, 그날 고칠 사람은 이 파일을 볼 이유가 없다. */
const rustMillis = async (path, name) => {
  const source = await readFile(resolve(ROOT, ...path), "utf8");
  const found = source.match(
    new RegExp(`const ${name}: Duration = Duration::from_millis\\((\\d+)\\);`),
  );
  if (!found) throw new Error(`${path.at(-1)} no longer names ${name} in milliseconds`);
  return Number(found[1]);
};
const PUMP_PATH = ["crates", "zerocode-shell", "src", "main.rs"];
const READERS_PATH = ["crates", "zerocode-pty", "src", "readers.rs"];
export const PUMP_BEAT = Object.freeze({
  intervalMs: await rustMillis(PUMP_PATH, "PUMP_INTERVAL"),
  chaseMs: await rustMillis(PUMP_PATH, "CHASE_INTERVAL"),
  renotifyMs: await rustMillis(READERS_PATH, "RENOTIFY_AFTER"),
});

/* 스로틀 배율과 장면 길이. 합격선(§6)은 6배 10초이고, 1배·4배는 같은 자로 잰
 * 곡선이다 — 어디서 무너지기 시작하는지가 그 사이에 있다. */
export const PACING_PLAN = Object.freeze([
  Object.freeze({ rate: 1, seconds: 3 }),
  Object.freeze({ rate: 4, seconds: 3 }),
  Object.freeze({ rate: 6, seconds: 10 }),
]);
const PASS_RATE = 6;
const PATTERNS = Object.freeze(["scroll", "redraw"]);
/* 다른 셸이 조용한가, 흐르는가 — 흐르는 이웃은 펌프의 모든 라운드를 움직인다. */
const NEIGHBOURS = Object.freeze([false, true]);

/* §6 의 합격선: 스로틀 6배에서 지연은 화면 프레임 간격의 두 배 이하, 마지막
 * 1초의 p95 는 첫 1초의 1.2배 이하(자라지 않음). 그리고 모든 장면에서 흐름이
 * 부른 덮인 적용은 0 — 키가 부를 수 있는 것은 에코를 실은 적용 하나에 하나뿐이라,
 * 키 하나가 그보다 많이 칠하면 그 칠은 흐름 몫으로 떨어져 이 선에 걸린다. */
const PASS_LINE = Object.freeze({ latencyFrames: 2, growth: 1.2, flowOverwritten: 0 });

/* 키를 누르는 간격 — 빠른 타자(초당 여섯 자 남짓). */
const KEY_EVERY_MS = 160;
/* 누를 글자들. 글자마다 다르게 눌러야 어느 `keydown` 이 어느 쓰기였는지 짝지어진다. */
const KEYS = "abcdefghijklmnopqrstuvwxyz";
/* 생산자가 첫 화면(바닥)을 깔고 재기 시작할 때까지의 여유 — 바닥의 적용은 재지 않는다. */
const WARM_MS = 400;
/* Round trips the page makes to read the worker's clock before a scene. The
 * tightest one sets the offset; more only tighten the bound it is good to. */
const CLOCK_SYNC_ROUNDS = 24;
/* 장면이 끝난 뒤 밀린 판을 다 그리기까지 기다리는 상한. 밀어 보내기는 6배에서
 * 장면 길이만큼 밀릴 수 있으므로 넉넉히 — 넘으면 그 자체가 판정이다. */
const DRAIN_LIMIT_MS = 120_000;

/* The producer — the backend, in a worker so the page's throttle does not slow
 * it. Serialised whole into a Blob: it captures nothing from this module. */
function pacingBackend() {
  const clock = () => performance.timeOrigin + performance.now();
  let plan = null;
  let fixture = null;
  // What the grid holds and what was last taken from it.
  let version = 0;
  let taken = 0;
  let produced = 0;
  let line = 0;
  let takenLine = 0;
  let generation = 0;
  let typed = "";
  let echoes = [];
  const producedAt = new Map();
  let running = false;
  let began = 0;
  let endAt = 0;
  let round = 0;
  // The one reader this window is (`FrameReaders`, readers.rs): what a look
  // took for it, whether it is owed the whole screen, and what it is expected
  // to do next (`Awaited`): `null` quiet, else since when, and whether it was
  // told (`told`) or came back with frames and is flowing.
  let share = [];
  let owes = true;
  let awaited = null;
  // Whether the pump is chasing a person's write (`chase` in `pump_loop`):
  // armed by a key, ended by a round that moves anything while no write waits
  // for its answer (`ChaseRound`).
  let chasing = false;
  // Whether a write is waiting for the child's answer
  // (`Pumped::unanswered_since`), and the echoes that reached the pty and
  // wait for a round to parse them.
  let awaiting = false;
  const arrived = [];

  const bump = () => {
    version += 1;
    producedAt.set(version, clock());
  };

  /* A frame leaves the grid: it carries the newest version and every echo
   * since the last one, and the grid's baseline moves to it. */
  const stamped = (delta) => {
    delta.__v = version;
    delta.__at = producedAt.get(version);
    delta.__echo = echoes;
    echoes = [];
    for (let old = taken + 1; old <= version; old += 1) producedAt.delete(old);
    taken = version;
    takenLine = line;
    return delta;
  };

  const promptRow = () => ({
    index: plan.rows - 1,
    text: `$ ${typed}`.padEnd(plan.cols, " ").slice(0, plan.cols),
    runs: [],
    zw: [],
  });

  /* The grid's take: everything since the last take, as one frame. */
  const take = () => {
    if (version === taken) return null;
    const { rows } = plan;
    let over;
    if (plan.pattern === "scroll") {
      const by = line - takenLine;
      const exposed = Math.min(rows, by);
      over = {
        scrolled_lines: by,
        rows: Array.from({ length: exposed }, (u, i) =>
          fixture.packedRow(rows - exposed + i, line - exposed + i)),
        scrollback_len: line,
      };
    } else if (plan.pattern === "redraw") {
      over = {
        rows: Array.from({ length: rows }, (u, i) => fixture.packedRow(i, generation * rows + i)),
        scrollback_len: line,
      };
    } else {
      over = {
        rows: [promptRow()],
        cursor: [rows - 1, Math.min(plan.cols - 1, 2 + typed.length)],
        scrollback_len: line,
      };
    }
    return stamped(fixture.base(over));
  };

  /* The grid's snapshot: the whole screen as it stands, paying a debt. */
  const snapshot = () => {
    const { rows } = plan;
    const screen = plan.pattern === "redraw"
      ? Array.from({ length: rows }, (u, i) => fixture.packedRow(i, generation * rows + i))
      : Array.from({ length: rows }, (u, i) => fixture.packedRow(i, line - rows + i));
    if (plan.pattern === "quiet") screen[rows - 1] = promptRow();
    return stamped(fixture.base({ full: true, rows: screen, scrollback_len: line }));
  };

  /* One shell's look (`FrameReaders::look`): a reader nobody expects is told,
   * and so is a flowing one on the round that parsed the answer to a write
   * (`Look::Chase`); the frame it will come for is taken for it now. Says
   * whether it told. */
  const look = (chase) => {
    const now = clock();
    const due = awaited === null
      || now - awaited.since >= plan.renotifyMs
      || (chase && !awaited.told);
    if (!due) return false;
    if (!owes) {
      const delta = take();
      if (delta) share.push(delta);
    }
    if (owes || share.length > 0) {
      awaited = { since: now, told: true };
      postMessage({ kind: "dirty" });
      return true;
    }
    return false;
  };

  /* The window's pull (`term_pull`): its share and what is new since, as the
   * JSON bytes the command answers with. Frames leave the reader flowing —
   * expected back, not told; an empty pull leaves it quiet. */
  const pull = (id, warm) => {
    let frames;
    if (owes) {
      owes = false;
      share = [];
      const floor = snapshot();
      if (warm) floor.__warm = true;
      frames = [floor];
    } else {
      frames = share.splice(0);
      const fresh = take();
      if (fresh) frames.push(fresh);
    }
    awaited = frames.length > 0 ? { since: clock(), told: false } : null;
    const answer = {
      screens: frames.length > 0 ? [{ term: plan.term, frames }] : [],
      missing: [],
    };
    const bytes = new TextEncoder().encode(JSON.stringify(answer)).buffer;
    postMessage({ kind: "pulled", id, bytes }, [bytes]);
  };

  /* The echoes that reached the pty, parsed: the child's answer. Says whether
   * there was any. */
  const parseEchoes = () => {
    if (arrived.length === 0) return false;
    for (const { id, text } of arrived.splice(0)) {
      typed = (typed + text).slice(-Math.floor(plan.cols / 2));
      echoes.push(id);
      bump();
      postMessage({ kind: "echoed", id, version });
    }
    awaiting = false;
    return true;
  };

  /* What a pump round does with this shell once its output is parsed: look,
   * and end the chase if the round moved anything — unless a write still
   * waits for its answer (`ChaseRound::chase_left`). Busy neighbours move
   * every round. */
  const pumpRound = ({ parsed, answered }) => {
    const told = look(answered);
    if ((parsed || told || plan.neighbours) && !awaiting) chasing = false;
  };

  /* One display-rate round: output lands in the grid, and the round sleeps the
   * REST of its beat (the pump's own rule), never a flat nap. */
  const beat = () => {
    if (!running) return;
    const at = clock();
    const printing = at < endAt && plan.pattern !== "quiet";
    if (printing) {
      if (plan.pattern === "scroll") line += plan.linesPerRound;
      else generation += 1;
      produced += 1;
      bump();
    }
    const answered = parseEchoes();
    pumpRound({ parsed: printing || answered, answered });
    if (at >= endAt) {
      running = false;
      postMessage({ kind: "ended", produced, version });
      return;
    }
    round += 1;
    setTimeout(beat, Math.max(0, began + round * plan.intervalMs - clock()));
  };

  onmessage = ({ data }) => {
    if (data.kind === "start") {
      plan = data.plan;
      fixture = new Function(`return (${plan.fixtureSource})`)()(plan.rows, plan.cols);
      line = plan.rows;
      takenLine = line;
      began = clock() + plan.warmMs;
      endAt = began + plan.seconds * 1000;
      // The window has just started reading: it is owed the screen, and told.
      look(false);
      running = true;
      setTimeout(beat, plan.warmMs);
      postMessage({ kind: "began", began, endAt });
    } else if (data.kind === "pull") {
      pull(data.id, clock() < began);
    } else if (data.kind === "key") {
      // A write starts waiting for its answer and wakes the pump, which runs a
      // round at once and arms its chase. The echo reaches the pty a chase
      // look later: the chase round parses it if the chase still stands, the
      // next beat if not.
      awaiting = true;
      chasing = true;
      pumpRound({ parsed: false, answered: false });
      setTimeout(() => {
        arrived.push({ id: data.id, text: data.text });
        // A stopped producer has no beat left to parse it: parsed now.
        if (!chasing && running) return;
        const answered = parseEchoes();
        pumpRound({ parsed: answered, answered });
      }, plan.chaseMs);
    } else if (data.kind === "clock") {
      postMessage({ kind: "clock", id: data.id, at: clock() });
    } else if (data.kind === "stop") {
      running = false;
    }
  };
}

/* The page's half: the road in, the meters, and the verdict's numbers. */
async function beginScene(plan) {
  const clock = () => performance.timeOrigin + performance.now();
  const { term } = plan;
  const view = termViews.get(term);
  const state = {
    began: 0,
    endAt: 0,
    ended: false,
    produced: 0,
    finalVersion: 0,
    applies: 0,
    // What each measured apply since the last display frame wrote: the rows,
    // and how many keys' echoes it carried.
    sinceFrame: [],
    overwritten: 0,
    overwrittenByKeys: 0,
    rowWrites: 0,
    rewrittenRows: 0,
    last: null,
    appliedVersion: 0,
    echoedVersion: 0,
    echoesOut: 0,
    echoesBack: 0,
    echoPending: [],
    keyAt: new Map(),
    paints: [],
    echoMs: [],
    gaps: [],
    longTasks: [],
    framing: true,
    lastFrame: 0,
  };
  const worker = new Worker(URL.createObjectURL(
    new Blob([`(${plan.backendSource})()`], { type: "text/javascript" }),
  ));

  /* The road in — exactly what the backend reaches: the notice under this
   * window's own name, and the answer to the window's own pull. The pull is
   * the window's (`pullTermFrames`); the harness only stands where the
   * command's answer comes from. */
  const hearNotice = () => {
    for (const listener of window.__LISTENERS__[`term:dirty:${WINDOW_LABEL}`] ?? []) {
      listener({ payload: null });
    }
  };
  const pulls = new Map();
  let pullSeq = 0;
  const priorPull = window.__ANSWER__.term_pull;
  window.__ANSWER__.term_pull = () => new Promise((answer) => {
    pullSeq += 1;
    pulls.set(pullSeq, answer);
    worker.postMessage({ kind: "pull", id: pullSeq });
  });

  /* The worker's stamps, read on this page's clock. Each side reads
   * `timeOrigin + now()` off its own origin, and two origins taken at two
   * moments disagree by milliseconds — a scene once measured production to
   * paint at −3.8 ms. */
  const pings = new Map();
  state.clock = { offset: 0, bound: Infinity };
  const onPage = (at) => at - state.clock.offset;
  const began = new Promise((done) => {
    worker.onmessage = ({ data }) => {
      if (data.kind === "dirty") hearNotice();
      else if (data.kind === "pulled") {
        const answer = pulls.get(data.id);
        pulls.delete(data.id);
        answer?.(data.bytes);
      } else if (data.kind === "began") {
        state.began = onPage(data.began);
        state.endAt = onPage(data.endAt);
        done();
      } else if (data.kind === "ended") {
        state.ended = true;
        state.produced = data.produced;
        state.finalVersion = data.version;
      } else if (data.kind === "clock") {
        pings.get(data.id)?.(data.at);
      } else if (data.kind === "echoed") {
        state.echoesBack += 1;
        state.echoedVersion = Math.max(state.echoedVersion, data.version);
      }
    };
  });

  /* Applied: which version the DOM now shows, counted per display frame. One
   * application is one answer painted once, however many frames it brought.
   * The rows it wrote come off the DOM itself: the records its paint left,
   * emptied before it starts and read as it returns. */
  const written = new MutationObserver(() => {});
  written.observe(view.pre, { subtree: true, childList: true, characterData: true, attributes: true });
  const rowsWritten = () => {
    const rows = new Set();
    for (const record of written.takeRecords()) {
      const { target } = record;
      const row = (target.nodeType === Node.ELEMENT_NODE ? target : target.parentElement)?.closest(".term-row");
      if (row) rows.add(row);
      for (const added of record.addedNodes) {
        if (added.classList?.contains("term-row")) rows.add(added);
      }
    }
    return rows;
  };
  const applyFrames = view.applyFrames;
  view.applyFrames = function meteredApply(frames) {
    written.takeRecords();
    const out = applyFrames.call(this, frames);
    const rows = rowsWritten();
    const newest = frames.at(-1);
    if (newest?.__v !== undefined && !frames.some((frame) => frame.__warm)) {
      state.applies += 1;
      state.last = newest;
      state.appliedVersion = Math.max(state.appliedVersion, newest.__v);
      let echoes = 0;
      for (const frame of frames) {
        state.echoPending.push(...frame.__echo);
        echoes += frame.__echo.length;
      }
      state.rowWrites += rows.size;
      state.sinceFrame.push({ rows, echoes });
    }
    return out;
  };

  /* Painted: the task after a rAF runs once this frame's rendering is done. */
  const stamps = [];
  const post = new MessageChannel();
  post.port1.onmessage = () => {
    const at = clock();
    const { shown, echoes } = stamps.shift();
    state.paints.push({ at, produced: onPage(shown.__at), version: shown.__v, echo: echoes.length > 0 });
    for (const id of echoes) {
      const pressed = state.keyAt.get(id);
      if (pressed !== undefined) state.echoMs.push(at - pressed);
    }
  };
  const frame = (stamp) => {
    const at = clock();
    if (state.lastFrame) state.gaps.push({ at, gap: stamp - state.lastFrame });
    state.lastFrame = stamp;
    const drawn = state.sinceFrame.splice(0);
    if (drawn.length > 0) {
      // Every apply but the last was painted over before this frame drew it.
      // An apply that carried an echo takes one of those as its key's; the
      // rest are output's. The rows a later apply wrote again are what they
      // cost the DOM.
      const over = drawn.length - 1;
      let echoing = 0;
      const writes = new Map();
      for (const { rows, echoes } of drawn) {
        if (echoes > 0) echoing += 1;
        if (over > 0) for (const row of rows) writes.set(row, (writes.get(row) ?? 0) + 1);
      }
      state.overwritten += over;
      state.overwrittenByKeys += Math.min(over, echoing);
      for (const count of writes.values()) state.rewrittenRows += count - 1;
      stamps.push({ shown: state.last, echoes: state.echoPending.splice(0) });
      post.port2.postMessage(null);
    }
    if (state.framing) requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  const longTasks = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      state.longTasks.push({ at: performance.timeOrigin + entry.startTime, ms: entry.duration });
    }
  });
  longTasks.observe({ type: "longtask" });

  /* The keys: a real `keydown` stamps the press, the window's own routing
   * writes it, and the stubbed pty hands it to the producer. */
  const pressedAt = new Map();
  const onKey = (event) => {
    if (event.key.length === 1) pressedAt.set(event.key, performance.timeOrigin + event.timeStamp);
  };
  document.addEventListener("keydown", onKey, true);
  const priorText = window.__ANSWER__.term_text;
  let keySeq = 0;
  window.__ANSWER__.term_text = ({ text }) => {
    keySeq += 1;
    state.keyAt.set(keySeq, pressedAt.get(text) ?? clock());
    state.echoesOut += 1;
    worker.postMessage({ kind: "key", id: keySeq, text });
    return null;
  };
  keySink.focus();

  window.__PACING__ = {
    state,
    worker,
    finish: () => {
      state.framing = false;
      longTasks.disconnect();
      written.disconnect();
      document.removeEventListener("keydown", onKey, true);
      window.__ANSWER__.term_text = priorText;
      if (priorPull) window.__ANSWER__.term_pull = priorPull;
      else delete window.__ANSWER__.term_pull;
      view.applyFrames = applyFrames;
      worker.terminate();
      // A pull still in the air when the backend goes has nothing to answer
      // with — the window's next pull asks the stub again.
      for (const answer of pulls.values()) answer(new TextEncoder().encode("{\"screens\":[],\"missing\":[]}").buffer);
      pulls.clear();
    },
  };
  // The offset from the tightest round trip, good to half of it.
  for (let id = 1; id <= plan.clockSyncRounds; id += 1) {
    const sent = clock();
    // eslint-disable-next-line no-await-in-loop
    const theirs = await new Promise((answer) => {
      pings.set(id, answer);
      worker.postMessage({ kind: "clock", id });
    });
    const got = clock();
    pings.delete(id);
    if ((got - sent) / 2 < state.clock.bound) {
      state.clock = { offset: theirs - (sent + got) / 2, bound: (got - sent) / 2 };
    }
  }
  worker.postMessage({ kind: "start", plan });
  await began;
  return { began: state.began, endAt: state.endAt };
}

async function finishScene({ drainLimitMs }) {
  const { state, finish } = window.__PACING__;
  const clock = () => performance.timeOrigin + performance.now();
  const waitedFrom = clock();
  // Everything made has been applied, and the last application has been
  // painted. A scene that applied nothing (no key landed) is settled once the
  // producer has said it ended.
  const settled = () => state.ended
    && state.echoesBack === state.echoesOut
    && state.appliedVersion >= Math.max(state.echoedVersion, state.finalVersion)
    && state.sinceFrame.length === 0
    && (state.applies === 0
      || (state.paints.length > 0 && state.paints.at(-1).version >= state.appliedVersion));
  let drained = true;
  while (!settled()) {
    if (clock() - waitedFrom > drainLimitMs) {
      drained = false;
      break;
    }
    // eslint-disable-next-line no-await-in-loop
    await new Promise((done) => setTimeout(done, 20));
  }
  const drainMs = clock() - waitedFrom;
  finish();
  delete window.__PACING__;

  const percentile = (values, p) => {
    if (values.length === 0) return 0;
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))];
  };
  const round = (n) => Math.round(n * 10) / 10;
  const stat = (values) => ({
    n: values.length,
    p50: round(percentile(values, 0.5)),
    p95: round(percentile(values, 0.95)),
    max: round(values.length ? Math.max(...values) : 0),
  });
  const inWindow = (at) => at >= state.began && at < state.endAt;
  const paints = state.paints.filter((paint) => inWindow(paint.produced));
  const seconds = Math.round((state.endAt - state.began) / 1000);
  const perSecond = Array.from({ length: seconds }, (u, second) => stat(
    paints
      .filter((paint) => Math.floor((paint.produced - state.began) / 1000) === second)
      .map((paint) => paint.at - paint.produced),
  ));
  const tasks = state.longTasks.filter((task) => inWindow(task.at));
  return {
    drained,
    drainMs: round(drainMs),
    produced: state.produced,
    applied: state.applies,
    painted: state.paints.length,
    overwritten: state.overwritten,
    overwrittenByKeys: state.overwrittenByKeys,
    keys: state.echoesOut,
    rows: { written: state.rowWrites, rewritten: state.rewrittenRows },
    frameMs: round(percentile(state.gaps.filter((gap) => inWindow(gap.at)).map((gap) => gap.gap), 0.5)),
    latency: stat(paints.map((paint) => paint.at - paint.produced)),
    floor: round(paints.length ? Math.min(...paints.map((paint) => paint.at - paint.produced)) : 0),
    outputFrames: stat(paints.filter((paint) => !paint.echo).map((paint) => paint.at - paint.produced)),
    echoFrames: stat(paints.filter((paint) => paint.echo).map((paint) => paint.at - paint.produced)),
    clock: { offset: round(state.clock.offset), bound: round(state.clock.bound) },
    perSecond,
    longTasks: {
      n: tasks.length,
      totalMs: round(tasks.reduce((sum, task) => sum + task.ms, 0)),
      max: round(tasks.length ? Math.max(...tasks.map((task) => task.ms)) : 0),
    },
    echo: stat(state.echoMs),
  };
}

const delay = (ms) => new Promise((done) => setTimeout(done, ms));

/* A scene's name: its pattern, its throttle, and whether the neighbours flow. */
const sceneName = ({ pattern, rate, neighbours }) => `${pattern} ${rate}×${neighbours ? " 이웃" : ""}`;

/* One scene under one throttle: produce, type over it, drain, count. */
async function runScene(page, cdp, { term, pattern, rate, seconds, neighbours }) {
  await cdp.send("Emulation.setCPUThrottlingRate", { rate });
  try {
    const { began, endAt } = await page.evaluate(beginScene, {
      term,
      pattern,
      seconds,
      neighbours,
      rows: REAL_PANE.rows,
      cols: REAL_PANE.cols,
      intervalMs: PUMP_BEAT.intervalMs,
      chaseMs: PUMP_BEAT.chaseMs,
      renotifyMs: PUMP_BEAT.renotifyMs,
      linesPerRound: BUILD_LOG_LINES_PER_ROUND,
      warmMs: WARM_MS,
      clockSyncRounds: CLOCK_SYNC_ROUNDS,
      fixtureSource: terminalFrameFixtureSource,
      backendSource: pacingBackend.toString(),
    });
    // Type for as long as the producer produces. The page's clock and this
    // process's are both wall clocks; the scene's own span is what matters.
    const typingEnds = Date.now() + (endAt - began) + WARM_MS - KEY_EVERY_MS;
    await delay(WARM_MS);
    for (let n = 0; Date.now() < typingEnds; n += 1) {
      // eslint-disable-next-line no-await-in-loop
      await page.keyboard.press(KEYS[n % KEYS.length]);
      // eslint-disable-next-line no-await-in-loop
      await delay(KEY_EVERY_MS);
    }
    return await page.evaluate(finishScene, { drainLimitMs: DRAIN_LIMIT_MS });
  } finally {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  }
}

const say = (name, m) =>
  `${name}: 생산→그림 p50 ${m.latency.p50}ms p95 ${m.latency.p95}ms 최대 ${m.latency.max}ms`
  + ` · 화면 프레임 ${m.frameMs}ms · 초별 p95 [${m.perSecond.map((s) => s.p95).join(", ")}]`
  + ` · 만든 ${m.produced} 적용 ${m.applied} 그림 ${m.painted}`
  + ` 덮인 적용 ${m.overwritten}(키 몫 ${m.overwrittenByKeys}, 키 ${m.keys})`
  + ` · 행 쓰기 ${m.rows.written} 같은 프레임에 다시 쓴 행 ${m.rows.rewritten}`
  + ` · long task ${m.longTasks.n}개 ${m.longTasks.totalMs}ms(최대 ${m.longTasks.max})`
  + ` · 키 에코 n ${m.echo.n} p50 ${m.echo.p50}ms p95 ${m.echo.p95}ms 최대 ${m.echo.max}ms`
  + ` · 밀린 것 다 그리기 ${m.drainMs}ms${m.drained ? "" : " (상한 초과)"}`
  + ` · 출력 프레임 생산→그림 n ${m.outputFrames.n} p50 ${m.outputFrames.p50}ms p95 ${m.outputFrames.p95}ms`
  + ` · 에코 프레임 생산→그림 n ${m.echoFrames.n} p50 ${m.echoFrames.p50}ms p95 ${m.echoFrames.p95}ms`
  + ` · 가장 짧은 생산→그림 ${m.floor}ms · 워커 시계 차 ${m.clock.offset}±${m.clock.bound}ms`;

export async function testTerminalPacing(page, ok) {
  await installTerminalWaits(page);
  const term = await page.evaluate(async () => {
    window.__HOLD_POLLERS__ = true;
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const opened = await openTermTab({ placement: "tab" });
    const view = termView(opened);
    setActiveTab(tabOfTerm(opened).id);
    await window.__TERMINAL_READY__(opened, view);
    return opened;
  });
  const cdp = await page.context().newCDPSession(page);
  const measured = [];
  try {
    const routed = await page.evaluate((opened) => keyboardTarget()?.term === opened, term);
    ok("terminal-pacing: 누른 키가 재는 판으로 간다", routed, `term ${term}`);
    for (const { rate, seconds } of PACING_PLAN) {
      for (const pattern of [...PATTERNS, "quiet"]) {
        for (const neighbours of NEIGHBOURS) {
          const scene = { pattern, rate, seconds, neighbours };
          // eslint-disable-next-line no-await-in-loop
          const m = await runScene(page, cdp, { term, ...scene });
          measured.push({ ...scene, ...m });
          ok(`terminal-pacing ${sceneName(scene)} 측정`, true, say(sceneName(scene), m));
        }
      }
    }
  } finally {
    await cdp.detach().catch(() => {});
  }

  /* 판정 — §6 의 합격선. 지연은 스로틀 6배에서. */
  for (const m of measured.filter((one) => one.rate === PASS_RATE && PATTERNS.includes(one.pattern))) {
    const name = `${sceneName(m)} ${m.seconds}s`;
    const first = m.perSecond[0]?.p95 ?? 0;
    const last = m.perSecond.at(-1)?.p95 ?? 0;
    ok(
      `terminal-pacing ${name}: 생산→그림 p95 가 화면 프레임 간격의 ${PASS_LINE.latencyFrames}배 안`,
      m.drained && m.latency.p95 <= m.frameMs * PASS_LINE.latencyFrames,
      `p95 ${m.latency.p95}ms · 선 ${Math.round(m.frameMs * PASS_LINE.latencyFrames * 10) / 10}ms`
      + ` (화면 프레임 ${m.frameMs}ms)${m.drained ? "" : " · 밀린 판을 상한 안에 다 못 그림"}`,
    );
    ok(
      `terminal-pacing ${name}: 지연이 자라지 않는다`,
      m.drained && last <= first * PASS_LINE.growth,
      `첫 1초 p95 ${first}ms · 마지막 1초 p95 ${last}ms · 선 ${Math.round(first * PASS_LINE.growth * 10) / 10}ms`,
    );
  }
  /* 판정 — 덮인 적용은 모든 장면에서. */
  for (const m of measured) {
    ok(
      `terminal-pacing ${sceneName(m)} ${m.seconds}s: 흐름이 부른 덮인 적용이 없다`,
      m.overwritten - m.overwrittenByKeys <= PASS_LINE.flowOverwritten,
      `덮인 적용 ${m.overwritten}(키 몫 ${m.overwrittenByKeys}, 키 ${m.keys}) / 적용 ${m.applied}`
      + ` · 같은 프레임에 다시 쓴 행 ${m.rows.rewritten} / 행 쓰기 ${m.rows.written}`,
    );
  }
  /* 판정 — 자 자신. 그림은 생산보다 앞설 수 없다: 워커 시계 차의 오차 밖으로
   * 음수인 생산→그림은 자가 틀렸다는 뜻이다. */
  for (const m of measured) {
    ok(
      `terminal-pacing ${sceneName(m)} ${m.seconds}s: 생산→그림이 시계 오차 밖으로 음수가 아니다`,
      m.floor >= -m.clock.bound,
      `가장 짧은 생산→그림 ${m.floor}ms · 워커 시계 차 ${m.clock.offset}±${m.clock.bound}ms`,
    );
  }
  return measured;
}
