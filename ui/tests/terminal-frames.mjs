/* 터미널 프레임의 모양, 한 곳에서.
 *
 * 백엔드가 보내는 프레임(`GridDelta`, packed `WireRow`: text + runs + zw,
 * zerocode-pty grid.rs)과 같은 모양을 짓는 손. 출력 경로를 재는 자가 둘이다 —
 * `terminal-throughput.mjs`(apply 의 값)와 `terminal-pacing.mjs`(생산 → 그림의
 * 박자) — 그리고 둘이 다른 빌드 로그를 재면 두 숫자는 서로를 설명하지 못한다.
 *
 * **소스로 건너가는 함수다.** 페이지(`page.evaluate`)와 워커(Blob) 안에서 같은
 * 행을 지어야 하므로 이 함수는 바깥의 무엇도 붙잡지 않는다: `toString()` 으로
 * 옮겨 `new Function` 으로 다시 세우면 그대로 돈다. 그래서 import 도, 모듈 밖
 * 상수도 쓰지 않는다. */
export function terminalFrameFixture(ROWS, COLS) {
  /* 빌드 로그 한 줄의 모양: 앞머리 동사에 색이 하나, 괄호 안 경로에 흐리게
   * 하나. 행당 run 두셋 — 진짜 출력이 그렇다. 전부 평범한 글자로 재면
   * `sameRun` 캐시가 실제보다 잘 맞고, 전부 무지개로 재면 실제보다 못 맞는다. */
  const GREEN = { indexed: 2 };
  const DIM = { indexed: 8 };
  const VERBS = ["Compiling", "Checking", "Finished", "Running", "Fresh"];
  const logRow = (n) => {
    const verb = VERBS[n % VERBS.length];
    const body = ` zerocode-shell v1.3.95 (/Users/dev/2026/zerocode/crates/zerocode-shell) #${n}`;
    const text = `   ${verb}${body}`.padEnd(COLS, " ").slice(0, COLS);
    return {
      text,
      // [at, len, style] — 백엔드의 `WireRow::runs` 와 같은 삼중항.
      runs: [
        [3, verb.length, { fg: GREEN, bold: true }],
        [3 + verb.length + body.indexOf(" ("), Math.max(0, body.length - body.indexOf(" (")), { fg: DIM }],
      ].filter(([, len]) => len > 0),
    };
  };
  const packedRow = (index, n) => ({ index, ...logRow(n), zw: [] });
  /* 한 프레임의 나머지 필드. 백엔드가 매 프레임 싣는 상태를 빠짐없이 — 빠진
   * 필드는 뷰가 「옛 백엔드」로 읽고 다른 길을 탄다. */
  const base = (over) => ({
    rows: [], scrolled_lines: 0, cursor: [ROWS - 1, 0], title: null, alt_screen: false,
    size: [ROWS, COLS], full: false, cursor_visible: true, mouse_tracking: "off",
    mouse_sgr: false, bell: false, view_offset: 0, scrollback_len: 0, folds: [],
    ...over,
  });
  return { logRow, packedRow, base };
}

/* 실제 판의 크기. 24×80 은 1978년의 숫자이고, 이 창에서 사람이 실제로 여는
 * 판은 이쪽에 가깝다 — 그리고 행당 비용이 열 수에 선형이므로 좁은 판에서 잰
 * 숫자는 넓은 판의 버벅임을 설명하지 못한다. */
export const REAL_PANE = Object.freeze({ rows: 60, cols: 220 });

/* 빌드 로그가 한 라운드(펌프의 한 박자)에 흘리는 줄 수. 16ms 에 세 줄은 초당
 * 180줄 — `cargo build` 가 크레이트를 넘기는 속도다. 두 자가 같은 흐름을 재야
 * apply 의 값과 박자의 값이 같은 장면을 말한다. */
export const BUILD_LOG_LINES_PER_ROUND = 3;

/* 이 모양을 페이지나 워커 안에서 다시 세우는 한 줄. */
export const terminalFrameFixtureSource = terminalFrameFixture.toString();
