/* 터미널 출력 경로의 자.
 *
 * 이 창의 터미널이 기본 터미널보다 버벅인다는 보고는 오래됐고, 그 동안의
 * 고침은 전부 국소적이었다 — 캐럿의 스타일 flush 하나, wrap.rs 의 긴 낱말
 * 하나. 구조를 바꾸려면(바이너리 전송로, GPU 페인터, 백프레셔) **바꾸기 전과
 * 후를 같은 눈금으로** 읽어야 하고, 그 눈금이 여기다.
 *
 * 무엇을 재는가 — webview 반쪽 전부다. 백엔드가 보내는 것과 **같은 모양의**
 * 프레임(packed `WireRow`: text + runs + zw, zerocode-pty grid.rs:574)을
 * 스텁 백엔드의 당김 몫에 넣고(`__TERM_FEED__`), 창이 당겨 간 답을 붓는 손
 * (`applyTermFrames` → `applyFrames` → `paintRow` → DOM)의 벽시계를 잰다.
 * 스텁 백엔드 위에서 돌므로 Rust 반쪽(파서, serde, 몫)은 들어 있지 않다 —
 * 그쪽은 crate 의 자기 벤치가 잰다.
 *
 * 왜 Chromium 에서 잰 숫자가 WKWebView 를 말해 주는가: 말해 주지 않는다.
 * **절대값은 옮길 수 없다.** 옮길 수 있는 것은 같은 계기로 잰 전후의 비(比)와,
 * 프레임당 강제 레이아웃 읽기 횟수·DOM 노드 증가·와이어 바이트처럼 엔진이
 * 바뀌어도 변하지 않는 셈이다. 그 셋은 여기서 판정이고, 시간은 참고다.
 *
 * 그리고 이 계기가 **못 보는 것**을 먼저 적어 둔다 — 여기 숫자가 전부 초록인데
 * 사람은 여전히 버벅임을 볼 수 있고, 그때 이 목록이 다음에 볼 곳이다:
 *
 * - 래스터와 합성. 하네스는 headless Chromium 이고(`chromium.launch()`),
 *   headless 는 실제 GPU 합성을 타지 않는다. 실제 앱에서 WebKit.GPU 프로세스가
 *   40%를 쓰고 있어도 여기서는 보이지 않는다.
 * - 전송로. 당김의 답이 건너오는 길 — `term_pull` 의 JSON 바이트
 *   (`tauri::ipc::Response`) → IPC → 창의 `TextDecoder` — 은 스텁 백엔드
 *   뒤에 없다. 여기서는 스텁이 같은 바이트를 지어 건넨다.
 * - Rust 반쪽. 파서, `take_delta`, 독자의 몫. crate 의 자기 벤치가 잰다.
 * - 생산에서 그림까지의 박자. 느린 기계에서 프레임이 밀리는지는
 *   `terminal-pacing.mjs` 가 잰다(M2).
 *
 * 세 장면을 따로 재는 이유는 셋의 비용 구조가 다르기 때문이다:
 *
 * - `scroll`  빌드 로그가 흐른다. `scrolled_lines` 가 있고 노출된 아래 몇 행만
 *             온다. 가장 흔하고, 사람이 "출력이 버벅인다"고 말할 때의 장면.
 * - `redraw`  TUI 가 화면을 다시 그린다. `scrolled_lines` 0 에 전 행이 바뀐다.
 *             에이전트 판이 이것이다.
 * - `full`    리사이즈·대체 화면 진입. 한 프레임에 전 화면이 갈린다.
 *
 * 임계값은 **회귀를 잡을 만큼만** 느슨하다. 이 파일의 일은 게이트를 빨갛게
 * 만드는 것이 아니라 숫자를 남기는 것이고, 숫자는 detail 에 적혀 레인의 로그에
 * 그대로 선다. 임계를 조일 자격은 구조를 바꾼 커밋에 있다. */
import { installTerminalWaits } from "./terminal-grid-selection.mjs";
import { BUILD_LOG_LINES_PER_ROUND, REAL_PANE, terminalFrameFixtureSource } from "./terminal-frames.mjs";

/* 한 프레임이 나르는 것을 사람이 읽는 한 줄로. */
const say = (name, stat) =>
  `${name}: apply p50 ${stat.p50}ms p95 ${stat.p95}ms max ${stat.max}ms`
  + ` · 와이어 ${stat.bytes}B/프레임 (60fps 환산 ${stat.mbPerSec}MB/s)`
  + ` · 레이아웃 읽기 ${stat.layoutReads}/프레임${
    Object.keys(stat.readsBy ?? {}).length
      ? ` (${Object.entries(stat.readsBy).map(([k, v]) => `${k} ${v}`).join(", ")})`
      : ""
  } · 셀 객체 ${stat.cellObjects}/프레임`;

export async function testTerminalThroughput(page, ok) {
  await installTerminalWaits(page);
  const measured = await page.evaluate(async ({ fixtureSource, pane, linesPerRound }) => {
    const priorAnswers = { ...window.__ANSWER__ };
    // 되돌릴 것들. 어느 갈래로 빠져나가든 원래 창을 돌려준다 — 이 수트 뒤에
    // 오는 수트가 계수기가 매달린 DOM API 를 쓰면 그 수트의 숫자가 거짓이 된다.
    const restore = [];
    try {
      window.__ANSWER__.term_snapshot = () => null;
      window.__ANSWER__.term_resize = () => null;

      const term = await openTermTab({ placement: "tab" });
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);

      const { rows: ROWS, cols: COLS } = pane;

      /* 프레임의 모양은 두 자가 함께 쓴다 (terminal-frames.mjs). */
      const { packedRow, base } = new Function(`return (${fixtureSource})`)()(ROWS, COLS);

      /* 그리고 색이 촘촘한 행 — DOM 페인터의 최악 경우.
       *
       * 위의 빌드 로그는 행당 run 이 셋이라 span 도 셋이다. 그런데 사람이 실제로
       * 보는 것 중에는 네 칸마다 색이 바뀌는 것들이 있다: `ls --color` 의 목록,
       * 구문 강조된 diff, powerline 프롬프트, 대시보드형 TUI. 거기서는 행 하나가
       * span 쉰다섯 개가 되고, 화면 하나가 3,300 노드가 된다. 행당 span 수가
       * DOM 페인터의 비용을 정하는 변수이므로, 그 변수를 열 배로 밀었을 때도
       * 예산 안이면 페인터는 무죄다 — 그리고 그때는 다른 데를 봐야 한다.
       *
       * 그런데 「색이 촘촘한 행」에는 변수가 **둘** 숨어 있고, 처음 쟀을 때
       * 둘을 같이 밀어서 어느 쪽이 값을 냈는지 알 수 없었다:
       *
       *   1. 행당 run(=span) 수 — 페인터가 만드는 노드의 수.
       *   2. 글자 자체 — 박스드로잉(▏▎▍█)은 mono 스택에 없어 폰트 폴백과
       *      셰이핑을 부른다. 이것은 페인터가 아니라 글꼴의 값이다.
       *
       * 그래서 둘을 따로 민다. 넷을 재면 어느 변수가 벼랑인지 산술로 나온다. */
      const DENSE_RUN = 4;
      const BOXY = "▏▎▍▌▋▊▉█";
      const ASCII = "abcdef0123456789 ";
      const denseRow = (index, n, { runs: dense, boxy }) => {
        const text = `${n} `.padEnd(COLS, boxy ? BOXY : ASCII).slice(0, COLS);
        const runs = [];
        const step = dense ? DENSE_RUN : COLS;
        for (let at = 0; at < COLS; at += step) {
          runs.push([at, Math.min(step, COLS - at), { fg: { indexed: (at / step + n) % 256 } }]);
        }
        return { index, text, runs, zw: [] };
      };

      /* 붓는 손. 창은 당김의 답을 이 한 손으로 붓는다 — 한 셸이 가져온 프레임을
       * 모델에 차례로 넣고 한 번 그린다. 벽시계와 아래의 계수기는 이 손 안에서만
       * 돈다: 당김은 창의 박자로 오므로 흘린 쪽의 시계로는 붓는 값을 잴 수 없고,
       * 그 사이 창의 다른 일이 읽은 레이아웃은 그리는 길의 값이 아니다. */
      const applyAsShipped = window.applyTermFrames;
      const applying = { inside: false, times: [] };
      window.applyTermFrames = (...rest) => {
        const at = performance.now();
        applying.inside = true;
        try {
          return applyAsShipped(...rest);
        } finally {
          applying.inside = false;
          applying.times.push(performance.now() - at);
        }
      };
      restore.push(() => {
        window.applyTermFrames = applyAsShipped;
      });

      /* 계수기. `paintRow` 가 레이아웃을 읽지 않는다는 것은 이 저장소의 주장이고
       * (shell-term.js 의 "the paint path reads no layout"), 주장은 세어서
       * 지킨다. 여기서 세는 값이 0 이 아니면 폭풍 게이트에 구멍이 났다는 뜻이다. */
      let layoutReads = 0;
      // And WHICH door was opened. A count alone says a contract broke; the
      // breakdown says where to look, and the two cost the same to collect.
      let readsBy = {};
      const countOn = (owner, key) => {
        const original = Object.getOwnPropertyDescriptor(owner, key);
        if (!original) return;
        restore.push(() => Object.defineProperty(owner, key, original));
        const note = () => {
          if (!applying.inside) return;
          layoutReads += 1;
          readsBy[key] = (readsBy[key] ?? 0) + 1;
        };
        if (typeof original.value === "function") {
          Object.defineProperty(owner, key, {
            ...original,
            value: function (...args) { note(); return original.value.apply(this, args); },
          });
        } else if (original.get) {
          Object.defineProperty(owner, key, {
            ...original,
            get: function () { note(); return original.get.call(this); },
          });
        }
      };
      for (const key of ["getBoundingClientRect", "getClientRects", "getAnimations"]) {
        countOn(Element.prototype, key);
      }
      for (const key of ["offsetWidth", "offsetHeight", "offsetTop", "offsetLeft", "clientWidth", "clientHeight"]) {
        countOn(HTMLElement.prototype, key);
      }
      for (const key of ["scrollTop", "scrollHeight", "scrollWidth"]) countOn(Element.prototype, key);
      countOn(window, "getComputedStyle");

      const feed = (delta) => window.__TERM_FEED__(term, delta);

      // 바닥 상태: 한 번 전 화면을 채워 두고 시작한다. 빈 화면에서 첫 프레임을
      // 재면 span 을 만드는 비용만 재고 `sameRun` 의 적중은 못 재므로,
      // 사람이 보는 정상 상태와 다르다.
      await feed(base({ full: true, rows: Array.from({ length: ROWS }, (u, i) => packedRow(i, i)) }));
      await new Promise((done) => requestAnimationFrame(done));

      const percentile = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))];
      const round = (n) => Math.round(n * 1000) / 1000;

      /* 한 장면을 N 프레임 돌리고 숫자를 낸다.
       *
       * 벽시계는 붓는 손 하나만 감싼다 — 당김을 기다린 시간과 브라우저의 프레임
       * 일정은 들어가지 않는다. 프레임은 하나씩 흘리고 그것이 닿기를 기다리므로
       * 붓기 한 번이 곧 프레임 하나다. 장면이 끝난 뒤 한 번 rAF 를 기다려 밀린
       * 페인트를 확정한다. */
      const scene = async (name, frames, makeDelta) => {
        layoutReads = 0;
        readsBy = {};
        applying.times = [];
        let bytes = 0;
        let cellObjects = 0;
        const before = view.pre.querySelectorAll("span").length;
        for (let n = 0; n < frames; n += 1) {
          const delta = makeDelta(n);
          // 백엔드가 실제로 보내는 바이트. JSON.stringify 는 serde_json 이
          // 쓰는 것과 같은 표현이라 이 숫자는 그대로 와이어의 숫자다.
          bytes += JSON.stringify(delta).length;
          // `expandPackedRow` 가 만드는 객체 수 — 행마다 열 수만큼.
          cellObjects += delta.rows.reduce((sum, row) => sum + row.text.length, 0);
          // eslint-disable-next-line no-await-in-loop
          await feed(delta);
        }
        const after = view.pre.querySelectorAll("span").length;
        const times = applying.times.slice().sort((a, b) => a - b);
        const perFrameBytes = Math.round(bytes / frames);
        return {
          name,
          p50: round(percentile(times, 0.5)),
          p95: round(percentile(times, 0.95)),
          max: round(times[times.length - 1]),
          bytes: perFrameBytes,
          mbPerSec: round((perFrameBytes * 60) / (1024 * 1024)),
          layoutReads: round(layoutReads / frames),
          readsBy: Object.fromEntries(
            Object.entries(readsBy).map(([key, count]) => [key, round(count / frames)]),
          ),
          cellObjects: Math.round(cellObjects / frames),
          spansBefore: before,
          spansAfter: after,
        };
      };

      const FRAMES = 120;

      /* 장면 1 — 흐르는 출력, 빌드 로그의 속도(`BUILD_LOG_LINES_PER_ROUND`).
       * 노출된 아래 몇 행만 오고 나머지는 위로 밀린다. */
      let line = ROWS;
      const scroll = await scene("scroll", FRAMES, () => {
        const by = linesPerRound;
        const rows = Array.from({ length: by }, (u, i) => packedRow(ROWS - by + i, line + i));
        line += by;
        return base({ scrolled_lines: by, rows, scrollback_len: line });
      });
      await new Promise((done) => requestAnimationFrame(done));

      /* 장면 2 — 쏟아지는 출력. 한 프레임에 화면 절반이 흘러간다
       * (`cat` 한 큰 파일, `yes`, 에이전트의 긴 스트림). */
      const flood = await scene("flood", FRAMES, () => {
        const by = Math.floor(ROWS / 2);
        const rows = Array.from({ length: by }, (u, i) => packedRow(ROWS - by + i, line + i));
        line += by;
        return base({ scrolled_lines: by, rows, scrollback_len: line });
      });
      await new Promise((done) => requestAnimationFrame(done));

      /* 장면 2b — **한 화면 넘게** 쏟아진다.
       *
       * 라운드가 8ms 를 배수하게 된 뒤로 이것이 폭주의 실제 모습이다: 한 프레임이
       * 수천 줄을 나르므로 `scrolled_lines` 가 화면 행 수를 훌쩍 넘는다. 그때
       * 노드를 화면 수만큼 돌리는 것은 항등 순열이고, 값은 `appendChild` 60회와
       * 배열 세 개의 O(n²) shift 다. 부분 스크롤(위의 `flood`)과 따로 재야
       * 하는 이유가 그것이다 — 거기서는 회전이 진짜 일을 한다. */
      const page = await scene("page", FRAMES, () => {
        const by = ROWS * 40;
        const rows = Array.from({ length: ROWS }, (u, i) => packedRow(i, line + i));
        line += by;
        return base({ scrolled_lines: by, rows, scrollback_len: line });
      });
      await new Promise((done) => requestAnimationFrame(done));

      /* 장면 3 — TUI 재그리기. 스크롤 없이 전 행이 바뀐다. */
      const redraw = await scene("redraw", FRAMES, (n) => base({
        rows: Array.from({ length: ROWS }, (u, i) => packedRow(i, n * ROWS + i)),
      }));
      await new Promise((done) => requestAnimationFrame(done));

      /* 장면 4 — full 프레임. 리사이즈와 대체 화면 진입이 이것이다. */
      const full = await scene("full", Math.floor(FRAMES / 4), (n) => base({
        full: true,
        rows: Array.from({ length: ROWS }, (u, i) => packedRow(i, n * ROWS + i)),
      }));
      await new Promise((done) => requestAnimationFrame(done));

      /* 장면 5 — **화면이 실제로 따라오는가.**
       *
       * 위의 넷은 전부 `apply` 가 돌아오기까지의 벽시계다. 그런데 apply 가
       * 돌아온 시점에 화면은 아직 아무것도 안 그렸다 — style 재계산, 레이아웃,
       * 래스터, 합성은 전부 그 **뒤에** 엔진의 일정으로 일어난다. 실제 앱에서
       * GPU 프로세스가 40%를 쓰고 있을 때 apply 는 0.5ms 였을 수 있고, 그러면
       * 위의 넷은 전부 초록인 채로 사람은 버벅임을 본다.
       *
       * 그 구간을 재는 유일한 정직한 방법은 **프레임을 rAF 에 맞춰 하나씩
       * 먹이고 rAF 가 실제로 얼마 만에 돌아오는지 보는 것**이다. 엔진이
       * 따라오면 간격은 화면 주사율(16.7ms)에 붙고, 못 따라오면 벌어진다.
       * 벌어진 만큼이 style·layout·paint·composite 가 예산을 넘긴 양이다.
       *
       * 첫 몇 프레임은 버린다 — rAF 의 첫 간격은 이 코드가 아니라 직전 장면이
       * 남긴 일을 잰다. */
      const paced = async (name, frames, makeDelta) => {
        const gaps = [];
        let last = 0;
        const WARMUP = 5;
        for (let n = 0; n < frames + WARMUP; n += 1) {
          // eslint-disable-next-line no-await-in-loop
          const at = await new Promise((done) => requestAnimationFrame(done));
          if (n >= WARMUP && last) gaps.push(at - last);
          last = at;
          feed(makeDelta(n));
        }
        gaps.sort((a, b) => a - b);
        return {
          name,
          p50: round(percentile(gaps, 0.5)),
          p95: round(percentile(gaps, 0.95)),
          max: round(gaps[gaps.length - 1]),
          // 주사율을 못 따라간 프레임의 비율. 20ms 는 16.7ms 에 한 틱의
          // 흔들림을 얹은 값 — 그 위는 「한 프레임을 놓쳤다」는 뜻이다.
          dropped: round(gaps.filter((gap) => gap > 20).length / gaps.length),
          frames: gaps.length,
        };
      };

      // 정지 대조군: 프레임을 **안** 먹이고 같은 rAF 를 돈다. 이 기계와 이
      // 순간의 바닥이 얼마인지 알아야 위의 숫자가 뜻을 가진다.
      const idle = await paced("paced-idle", 60, () => base({ rows: [] }));
      const pacedFlood = await paced("paced-flood", 60, (n) => {
        const by = Math.floor(ROWS / 2);
        const rows = Array.from({ length: by }, (u, i) => packedRow(ROWS - by + i, line + n * by + i));
        return base({ scrolled_lines: by, rows, scrollback_len: line + n * by });
      });
      const pacedRedraw = await paced("paced-redraw", 60, (n) => base({
        rows: Array.from({ length: ROWS }, (u, i) => packedRow(i, n * ROWS + i)),
      }));
      /* 그 값이 **JS 안에 있는가 뒤에 있는가.**
       *
       * 위의 페이싱은 프레임 간격 전부를 재므로 apply 와 그 뒤의 style·layout·
       * paint 를 구별하지 못한다. 구별이 필요한 이유는 고칠 곳이 다르기
       * 때문이다: apply 가 값을 내면 페인터의 코드를 고치면 되고, apply 가
       * 싼데 간격이 벌어지면 남은 값은 **엔진이 인라인 박스 3,300개를 다루는
       * 값**이라 DOM 페인터로는 못 줄인다 — 그때가 글리프 아틀라스를 이야기할
       * 때다. 같은 밀도를 `scene` 으로 한 번 더 재서 둘을 뺀다. */
      const denseApply = await scene("dense-apply", FRAMES, (n) => base({
        rows: Array.from({ length: ROWS }, (u, i) => denseRow(i, n * ROWS + i, { runs: true, boxy: false })),
      }));
      await new Promise((done) => requestAnimationFrame(done));

      /* **글리프 아틀라스 페인터를 지을 값이 있는가.**
       *
       * 위에서 나온 답은 「촘촘한 화면의 87%가 apply 바깥」이었다. 그 87%는
       * 엔진이 인라인 박스 3,300개에 style·layout·paint 를 하는 값이고, DOM
       * 페인터의 자바스크립트를 아무리 고쳐도 안 줄어든다. 줄이려면 박스를
       * 없애야 하고, 그것이 canvas 다.
       *
       * 그런데 canvas 페인터는 선택·링크·폴드·CJK 폭·검색 표시를 전부 다시
       * 지어야 하는 며칠짜리 서브시스템이다. **짓기 전에 이길지 알아야 한다.**
       * 그래서 여기서 재는 것은 완성품이 아니라 **하한**이다: 같은 행을 같은
       * 자리에 같은 색으로 그리는, 페인터가 반드시 해야 하는 최소한의 일.
       * 진짜 페인터는 이보다 느리지 절대 빠를 수 없다. 이 하한이 DOM 을 크게
       * 이기지 못하면 지을 이유가 없고, 크게 이기면 그 차이가 예산이다.
       *
       * 아틀라스는 쓰지 않는다 — 아틀라스는 canvas2d 에서 색 입히기가
       * 어려워(글리프×색 조합마다 캐시) 그 자체가 설계 결정이고, 여기서 묻는
       * 것은 「박스를 없애면 얼마나 싼가」이지 「어느 캐시가 좋은가」가 아니다. */
      const canvasPaint = async (name, frames, makeRows) => {
        const box = view.pre.getBoundingClientRect();
        const dpr = window.devicePixelRatio || 1;
        const face = getComputedStyle(view.pre).font;
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(box.width * dpr));
        canvas.height = Math.max(1, Math.round(box.height * dpr));
        canvas.style.cssText =
          `position:fixed;left:0;top:0;width:${box.width}px;height:${box.height}px;`
          + "pointer-events:none;z-index:-1;opacity:0.01;";
        document.body.appendChild(canvas);
        const ctx = canvas.getContext("2d", { alpha: false });
        ctx.scale(dpr, dpr);
        ctx.font = face;
        ctx.textBaseline = "top";
        // 색 이름 해석은 프레임당 값이 아니다 — 팔레트는 고정이므로 한 번 푼다.
        const ink = Array.from({ length: 256 }, (u, i) =>
          getComputedStyle(document.documentElement).getPropertyValue(`--term-${i}`).trim() || "#c0c0c0");
        const cw = view.cell.width;
        const ch = view.cell.height;
        try {
          const gaps = [];
          let last = 0;
          const WARMUP = 5;
          for (let n = 0; n < frames + WARMUP; n += 1) {
            // eslint-disable-next-line no-await-in-loop
            const at = await new Promise((done) => requestAnimationFrame(done));
            if (n >= WARMUP && last) gaps.push(at - last);
            last = at;
            const rows = makeRows(n);
            ctx.fillStyle = "#101010";
            ctx.fillRect(0, 0, box.width, box.height);
            for (const row of rows) {
              const y = row.index * ch;
              for (const [col, len, style] of row.runs) {
                ctx.fillStyle = ink[style.fg?.indexed ?? 7] ?? "#c0c0c0";
                ctx.fillText(row.text.slice(col, col + len), col * cw, y);
              }
            }
          }
          gaps.sort((a, b) => a - b);
          return {
            name,
            p50: round(percentile(gaps, 0.5)),
            p95: round(percentile(gaps, 0.95)),
            max: round(gaps[gaps.length - 1]),
            dropped: round(gaps.filter((gap) => gap > 20).length / gaps.length),
            frames: gaps.length,
          };
        } finally {
          canvas.remove();
        }
      };

      const canvasSparse = await canvasPaint("canvas-sparse", 60, (n) =>
        Array.from({ length: ROWS }, (u, i) => denseRow(i, n * ROWS + i, { runs: false, boxy: false })));
      const canvasDense = await canvasPaint("canvas-dense", 60, (n) =>
        Array.from({ length: ROWS }, (u, i) => denseRow(i, n * ROWS + i, { runs: true, boxy: false })));

      /* 2×2. 「span 이 많아서」와 「글자가 무거워서」를 갈라 놓는다. */
      const dense = {};
      for (const [name, shape] of [
        ["ascii-sparse", { runs: false, boxy: false }],
        ["ascii-dense", { runs: true, boxy: false }],
        ["boxy-sparse", { runs: false, boxy: true }],
        ["boxy-dense", { runs: true, boxy: true }],
      ]) {
        // eslint-disable-next-line no-await-in-loop
        const stat = await paced(`paced-${name}`, 60, (n) => base({
          rows: Array.from({ length: ROWS }, (u, i) => denseRow(i, n * ROWS + i, shape)),
        }));
        stat.spans = view.pre.querySelectorAll("span").length;
        dense[name] = stat;
      }

      return {
        scroll, flood, page, redraw, full, denseApply,
        canvasSparse, canvasDense,
        idle, pacedFlood, pacedRedraw, dense,
        rows: ROWS, cols: COLS, frames: FRAMES,
      };
    } finally {
      for (const undo of restore.reverse()) undo();
      Object.assign(window.__ANSWER__, priorAnswers);
    }
  }, { fixtureSource: terminalFrameFixtureSource, pane: REAL_PANE, linesPerRound: BUILD_LOG_LINES_PER_ROUND });

  const { scroll, flood, redraw, full, idle, pacedFlood, pacedRedraw, dense } = measured;

  // 숫자는 전부 남긴다 — 임계를 넘지 않아도 레인의 로그가 이 줄을 들고 있으면
  // 다음 사람이 전후를 비교할 수 있다.
  for (const stat of [scroll, flood, measured.page, redraw, full, measured.denseApply]) {
    ok(`terminal-throughput ${stat.name} 측정`, true, say(stat.name, stat));
  }
  for (const stat of [idle, pacedFlood, pacedRedraw, measured.canvasSparse, measured.canvasDense,
    ...Object.values(dense)]) {
    ok(
      `terminal-throughput ${stat.name} 측정`,
      true,
      `${stat.name}: 프레임 간격 p50 ${stat.p50}ms p95 ${stat.p95}ms max ${stat.max}ms`
      + ` · 놓친 프레임 ${Math.round(stat.dropped * 100)}% (${stat.frames}프레임)`
      + (stat.spans === undefined ? "" : ` · 화면 span ${stat.spans}개`),
    );
  }

  /* 판정. 시간이 아니라 셈이다 — 엔진이 달라도 변하지 않는 것들. */

  /* 하나: paint 경로는 레이아웃을 읽지 않는다. 이 저장소의 명시적 계약
   * (shell-term.js 의 "the paint path reads no layout")이고, 깨지면 프레임마다
   * 강제 리플로가 들어가 있다는 뜻이다.
   *
   * 「프레임당 0」이 아니라 「프레임당 1 미만」으로 묻는 이유: 장면의 첫
   * 프레임은 판이 처음 자리를 잡는 프레임이라 한 번은 읽는다(측정: 120 프레임에
   * 1회, 0.008/프레임). 그 하나는 계약을 깨지 않는다 — 계약이 금하는 것은
   * **매 프레임** 읽는 것이고, 그것이 들어오면 이 값은 1 위로 뛴다. */
  const worstLayout = Math.max(
    scroll.layoutReads,
    flood.layoutReads,
    redraw.layoutReads,
    // 색이 촘촘한 화면도 같은 계약 아래 있다. 여기가 한때 96/프레임이었다:
    // `nearestReadableIndex` 가 캐시 미스마다 256색 팔레트를 토큰 하나씩 다시
    // 읽어서, 미스 한 번이 `getComputedStyle` 258회였다. 고친 뒤 최악 프레임이
    // 27.1ms 에서 9.6ms 로 내려왔다(중앙값은 2.2ms 로 그대로 — 이것은 꼬리의
    // 값이고, 사람이 끊김으로 읽는 것이 꼬리다).
    measured.denseApply.layoutReads,
  );
  ok(
    "terminal-throughput: 흐르는 프레임은 레이아웃을 읽지 않는다",
    worstLayout < 1,
    `프레임당 레이아웃 읽기 최대 ${worstLayout} (scroll ${scroll.layoutReads}, flood ${flood.layoutReads},`
    + ` redraw ${redraw.layoutReads}, dense ${measured.denseApply.layoutReads}) — 장면당 한 번은 판이 자리 잡는 값`,
  );

  /* 둘: 화면이 프레임을 따라온다.
   *
   * 정지 대조군과 **견주어서** 묻는다. 절대 간격은 이 기계가 그 순간 무엇을
   * 하고 있었는지에 달렸지만, 「아무것도 안 그릴 때」와 「전 화면을 그릴 때」의
   * 차이는 그리는 일 자체의 값이다. 두 배까지는 봐 준다 — 그 위는 렌더링
   * 파이프라인이 예산을 넘겼다는 뜻이고, 그때가 페인터를 바꿀 때다. */
  const times = (stat) => (idle.p95 > 0 ? Math.round((stat.p95 / idle.p95) * 100) / 100 : 0);
  /* 대조군보다 **얼마나 더** 놓쳤는가로 묻는다, 절대 수가 아니라.
   *
   * 「20ms 넘은 프레임의 비율」을 그대로 임계로 쓰면 이 판정은 코드가 아니라
   * 기계의 그 순간 부하를 잰다 — 레인은 같은 시각 여덟 갈래를 컴파일하고
   * 있고, 그때는 **아무것도 안 그리는** 정지 대조군도 20ms를 넘는다. 그러면
   * 초록이어야 할 커밋이 빨개지고, 플레이크 하나가 게이트의 나머지를 가린다
   * (함정 265). 그리는 일의 값은 「그릴 때와 안 그릴 때의 차이」이고, 그 차이는
   * 부하가 양쪽에 똑같이 실려도 남는다. */
  const extraDrops = Math.max(0, pacedRedraw.dropped - idle.dropped);
  ok(
    "terminal-throughput: 전 화면을 그려도 주사율을 따라간다",
    extraDrops <= 0.25,
    `paced-redraw 놓친 프레임 ${Math.round(pacedRedraw.dropped * 100)}%`
    + ` · 정지 대조군 ${Math.round(idle.dropped * 100)}%`
    + ` · 그리는 값 ${Math.round(extraDrops * 100)}%p (임계 25%p)`
    + ` · p95 ${pacedRedraw.p95}ms 대 ${idle.p95}ms, 배수 ${times(pacedRedraw)}×`,
  );

  /* 글리프 아틀라스를 지을 값이 있는가 — 이 한 줄이 며칠짜리 착공 여부를 정한다.
   *
   * canvas 쪽은 **하한**이다(선택·링크·폴드·CJK 없음). 진짜 페인터는 이보다
   * 느리지 빠를 수 없으므로, 여기서 크게 이기지 못하면 지을 이유가 없다. */
  {
    const dom = dense["ascii-dense"];
    const can = measured.canvasDense;
    const won = idle.p50 > 0 ? Math.round(((dom.p50 - can.p50) / idle.p50) * 100) / 100 : 0;
    ok(
      "terminal-throughput: 글리프 아틀라스를 지을 값이 있는가",
      true,
      `촘촘한 화면(span 3,300) — DOM p50 ${dom.p50}ms 대 canvas 하한 ${can.p50}ms`
      + ` · 정지 대조군 ${idle.p50}ms · 아낄 수 있는 최대 ${won}프레임분`
      + ` | 성긴 화면 — DOM ${dense["ascii-sparse"].p50}ms 대 canvas ${measured.canvasSparse.p50}ms`,
    );
  }

  /* 값이 JS 안에 있는가 뒤에 있는가 — 이 한 줄이 다음에 고칠 곳을 정한다. */
  {
    const inJs = measured.denseApply.p50;
    const whole = dense["ascii-dense"].p50;
    const behind = Math.round((whole - inJs) * 100) / 100;
    ok(
      "terminal-throughput: 촘촘한 화면의 값이 apply 안인가 뒤인가",
      true,
      `apply ${inJs}ms · 프레임 간격 ${whole}ms · 그 뒤(style·layout·paint) ${behind}ms`
      + ` = ${Math.round((behind / Math.max(whole, 0.001)) * 100)}%`,
    );
  }

  /* 그리고 2×2 의 산술. 어느 변수가 벼랑인지 한 줄로 말한다 — 이것이 다음
   * 커밋이 어디를 고칠지 정하는 줄이므로, 판정이 아니라 **낱말**로 적는다. */
  const byRuns = dense["ascii-dense"].p50 / Math.max(dense["ascii-sparse"].p50, 0.001);
  const byGlyph = dense["boxy-sparse"].p50 / Math.max(dense["ascii-sparse"].p50, 0.001);
  const round2 = (n) => Math.round(n * 100) / 100;
  ok(
    "terminal-throughput: 촘촘한 화면의 값이 어디서 나오는지",
    true,
    `span 수의 값 ${round2(byRuns)}× (${dense["ascii-sparse"].spans}→${dense["ascii-dense"].spans}개)`
    + ` · 글자(폰트 폴백)의 값 ${round2(byGlyph)}×`
    + ` · 둘 다 ${round2(dense["boxy-dense"].p50 / Math.max(dense["ascii-sparse"].p50, 0.001))}×`
    + ` [p50 ms: ascii-sparse ${dense["ascii-sparse"].p50} · ascii-dense ${dense["ascii-dense"].p50}`
    + ` · boxy-sparse ${dense["boxy-sparse"].p50} · boxy-dense ${dense["boxy-dense"].p50}]`,
  );

  // 최악 경우에도 주사율의 절반은 지킨다. 절반 아래로 떨어지면 사람이 끊김으로
  // 읽는 영역이고, 그때는 페인터를 손볼 때다.
  const worst = dense["boxy-dense"];
  ok(
    "terminal-throughput: 최악의 화면도 주사율의 절반은 지킨다",
    worst.p50 <= idle.p50 * 3,
    `boxy-dense p50 ${worst.p50}ms (정지 대조군 ${idle.p50}ms, 배수 ${round2(worst.p50 / Math.max(idle.p50, 0.001))}×)`
    + ` · 화면 span ${worst.spans}개`,
  );

  // 둘: span 이 프레임마다 새로 나지 않는다. 120 프레임을 돌린 뒤 노드 수가
  // 제자리면 재사용이 살아 있고, 늘었으면 노드가 샌다.
  const grew = Math.max(
    scroll.spansAfter - scroll.spansBefore,
    flood.spansAfter - flood.spansBefore,
    redraw.spansAfter - redraw.spansBefore,
  );
  ok(
    "terminal-throughput: 프레임을 돌려도 span 이 쌓이지 않는다",
    grew <= measured.rows,
    `120 프레임 뒤 span 증가 최대 ${grew}행분 (${measured.rows}행 판)`,
  );

  /* 셋: 시간. **아주** 느슨하게 — 16ms 가 아니라 50ms 다.
   *
   * 표시 예산은 16ms 이고 그것이 목표지만, 이 판정은 목표를 지키는 것이 아니라
   * **파국을 잡는** 것이다. 이 계기는 레인의 기계에서도 돌고, 그 기계는 같은
   * 순간 여덟 갈래의 컴파일을 하고 있다 — 16ms 로 조이면 코드가 아니라 부하를
   * 재게 되고, 플레이크 하나가 레인의 나머지를 가린다(함정 265). 목표와의
   * 거리는 위의 측정 줄이 말한다; 이 줄은 자릿수가 바뀌었을 때만 빨개진다. */
  ok(
    "terminal-throughput: 한 프레임이 자릿수를 바꾸지 않는다",
    flood.p95 < 50,
    `flood p95 ${flood.p95}ms · redraw p95 ${redraw.p95}ms (파국 임계 50ms, 표시 예산 16ms)`,
  );

  return measured;
}
