/* ---- 한국어 입력 회귀: herdr의 실기기 사례를 이 창의 입력 층에 (t-26598) ----
 *
 * herdr-web-ui(MIT)의 docs/terminal-input.md가 실기기에서 잡은 사례 셋을
 * ui/shell-input.js의 조합 길로 옮긴다.
 *
 *   1. 렌더러가 바쁠 때 「니다.」 — 조합 두 개와 마침표가 한 덩어리로 도착한다.
 *      대기 중인 반쪽 글자의 타이머가 아직 서 있어도 세 글자 모두 나가야 한다.
 *   2. 「가나다 」, 백스페이스 두 번, 「한글」 — 지운 뒤 편집기 문맥이 낡아도
 *      한글은 낱자(ㅎㅏㄴ글)가 아니라 음절로 나가야 한다.
 *   3. 「값」 뒤 모음이 오면 「갑사」 — 겹받침 ㅄ이 갈라져 ㅅ이 다음 음절로 간다.
 *      조합 사건으로 올 때와, 입력 문맥이 늦어 자모 그대로 올 때 둘 다.
 *
 * 글과 키는 나간 차례대로 한 줄에 적는다 — 따로 세면 백스페이스가 어느 글
 * 사이에 있었는지가 사라진다. 판 종류는 매개변수다: 입력 층은 판 종류를 가르지
 * 않으므로, 카탈로그의 여섯 판이 모두 같은 줄을 내야 한다. */

import { openWindowTestPage } from "./window-boot.mjs";

/* 판 종류 — 에이전트 카탈로그의 id. `antigravity`가 CLI 이름 `agy`다. */
const PANE_KINDS = ["claude", "codex", "zo", "antigravity", "kimi", "grok"];

/* 나간 줄의 표기: 글은 그대로, 키는 `<이름>`. */
const BACKSPACE = "<Backspace>";
const ENTER = "<Enter>";

/* 사건 열의 모양: [앞 사건으로부터 ms, 무엇, 글]. `start`·`update`·`end`는
 * 조합, `key`는 조합 중 keydown(229), `text`는 평범한 글자 한 자, `raw`는
 * 조합이 아닌 자모 keydown(입력 문맥이 늦은 길), `bs`·`enter`는 평범한 키다. */
const NIDA = [
  [0, "start"], [0, "update", "ㄴ"], [0, "key", "ㄴ"],
  [0, "update", "니"], [0, "key", "ㅣ"], [0, "key", "ㄷ"], [0, "end", "니"],
  [0, "start"], [0, "update", "ㄷ"], [0, "key", "ㄷ"],
  [0, "update", "다"], [0, "key", "ㅏ"], [0, "end", "다"],
  [0, "text", "."],
];

/* 「가나다 」, 백스페이스 두 번, 「한글」, Enter. 글자마다 조합 사건을 친다. */
const GANADA_BACKSPACE_HANGUL = [
  [0, "start"], [0, "update", "ㄱ"], [0, "key", "ㄱ"], [0, "update", "가"],
  [0, "key", "ㅏ"], [0, "update", "가"], [0, "key", "ㄴ"], [0, "end", "가"],
  [0, "start"], [0, "update", "ㄴ"], [0, "key", "ㄴ"], [0, "update", "나"],
  [0, "key", "ㅏ"], [0, "update", "나"], [0, "key", "ㄷ"], [0, "end", "나"],
  [0, "start"], [0, "update", "ㄷ"], [0, "key", "ㄷ"], [0, "update", "다"],
  [0, "key", "ㅏ"], [0, "end", "다"],
  [0, "text", " "],
  [0, "bs"], [0, "bs"],
  [0, "start"], [0, "update", "ㅎ"], [0, "key", "ㅎ"], [0, "update", "하"],
  [0, "key", "ㅏ"], [0, "update", "한"], [0, "key", "ㄴ"], [0, "key", "ㄱ"],
  [0, "end", "한"],
  [0, "start"], [0, "update", "ㄱ"], [0, "key", "ㄱ"], [0, "update", "그"],
  [0, "key", "ㅡ"], [0, "update", "그"], [0, "key", "ㄹ"], [0, "update", "글"],
  [0, "end", "글"],
  [0, "enter"],
];

/* 「값」은 ㄱ ㅏ ㅂ ㅅ, 여기에 모음 ㅏ가 오면 「갑」과 「사」로 갈린다. */
const GAB_SA_COMPOSED = [
  [0, "start"], [0, "update", "ㄱ"], [0, "key", "ㄱ"], [0, "update", "가"],
  [0, "key", "ㅏ"], [0, "update", "갑"], [0, "key", "ㅂ"], [0, "update", "값"],
  [0, "key", "ㅅ"], [0, "key", "ㅏ"], [0, "update", "갑"], [0, "end", "갑"],
  [0, "start"], [0, "update", "사"], [0, "end", "사"],
];

/* 같은 글을 조합 사건 없이 자모 keydown으로 친다 — 입력 문맥이 늦은 길. */
const GAB_SA_RAW = [
  [0, "raw", "ㄱ"], [0, "raw", "ㅏ"], [0, "raw", "ㅂ"], [0, "raw", "ㅅ"],
  [0, "raw", "ㅏ"], [0, "enter"],
];

/* 사례 하나: 사건 열, 나가야 할 줄. `burst: true`인 사례는 사건을 한 덩어리로
 * 쏜다 — 렌더러가 바빠 조합 끝, 다음 조합, 마침표가 타이머보다 먼저 도착하는 모양이다. */
const CASES = [
  { name: "니다.·버스트", burst: true, ring: NIDA, wants: ["니", "다", "."] },
  { name: "니다.·간격", burst: false, ring: NIDA, wants: ["니", "다", "."] },
  {
    // 반쪽 「ㅋ」이 붙들린 채 타이머가 서 있을 때 「니다.」가 도착한다.
    name: "니다.·반쪽 대기", burst: true,
    ring: [
      [0, "start"], [0, "update", "ㅋ"], [0, "key", "ㅋ"], [0, "end", "ㅋ"],
      ...NIDA,
    ],
    wants: ["ㅋ", "니", "다", "."],
  },
  {
    name: "가나다·지우기·한글", burst: false, ring: GANADA_BACKSPACE_HANGUL,
    wants: ["가", "나", "다", " ", BACKSPACE, BACKSPACE, "한", "글", ENTER],
  },
  {
    // 조합 사건 없이 자모만 한 덩어리로 온 「니다.」 — 입력 문맥이 늦은 길.
    name: "니다.·자모 버스트", burst: true,
    ring: [[0, "raw", "ㄴ"], [0, "raw", "ㅣ"], [0, "raw", "ㄷ"], [0, "raw", "ㅏ"], [0, "text", "."]],
    wants: ["니다", "."],
  },
  { name: "값+ㅏ·조합", burst: false, ring: GAB_SA_COMPOSED, wants: ["갑", "사"] },
  { name: "값+ㅏ·자모", burst: false, ring: GAB_SA_RAW, wants: ["갑사", ENTER] },
];

/* `burst`가 아닌 사례에서 사건과 사건 사이의 간격 — 사람이 치는 속도에 가깝다. */
const PACE_MS = 60;
/* 사례가 끝난 뒤 기다리는 시간: 반쪽 글자를 붙드는 48 ms 타이머가 돌 만큼.
 * 어절 타이머(1000 ms)는 각 사례가 Enter나 조합 끝으로 먼저 비운다. */
const SETTLE_MS = 160;

export async function exerciseKoreanRegression(script) {
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const started = performance.now();
  const sink = document.getElementById("key-sink");
  const lines = {};
  const timeline = [];
  window.__ANSWER__.term_text = (args) => (timeline.push(args.text), null);
  window.__ANSWER__.term_key = (args) => (timeline.push(`<${args.press.key}>`), null);

  /* 한 사건을 브라우저가 낼 모양으로 쏜다. 키다운이 막히지 않았을 때만 글자
   * 칸에 글이 들어가고, 그 칸의 입력 사건이 뒤따른다 — 낱자를 가르는 것은
   * 이 두 조각의 순서다. */
  const fire = (kind, data) => {
    if (kind === "start") {
      sink.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    } else if (kind === "update") {
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionupdate", { data, bubbles: true }));
      sink.dispatchEvent(new InputEvent("input", { isComposing: true, data, bubbles: true }));
    } else if (kind === "end") {
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionend", { data, bubbles: true }));
    } else if (kind === "key") {
      sink.dispatchEvent(new KeyboardEvent("keydown", {
        key: data, keyCode: 229, bubbles: true, cancelable: true,
      }));
    } else {
      const key = kind === "bs" ? "Backspace" : kind === "enter" ? "Enter" : data;
      const keyCode = kind === "bs" ? 8 : kind === "enter" ? 13 : 0;
      const open = sink.dispatchEvent(new KeyboardEvent("keydown", {
        key, keyCode, bubbles: true, cancelable: true,
      }));
      if (!open) return;
      if (kind === "text" || kind === "raw") {
        sink.value += data;
        sink.dispatchEvent(new InputEvent("input", { data, inputType: "insertText", bubbles: true }));
      } else if (kind === "bs" && sink.value !== "") {
        sink.value = sink.value.slice(0, -1);
        sink.dispatchEvent(new InputEvent("input", { inputType: "deleteContentBackward", bubbles: true }));
      }
    }
  };

  /* 조합 사이와 지우기 키 앞에서 편집 칸은 비어 있어야 한다 — 낡은 글이 남으면
   * 편집기 문맥이 낡아 한글이 낱자로 새는 길이 열린다 (herdr: Gboard). */
  const play = async (ring, burst) => {
    timeline.length = 0;
    const stale = [];
    for (const [, kind, data] of ring) {
      if ((kind === "start" || kind === "bs") && sink.value !== "") stale.push(sink.value);
      fire(kind, data);
      if (!burst) await wait(script.paceMs);
    }
    await wait(script.settleMs);
    return { line: [...timeline], stale };
  };

  for (const kind of script.kinds) {
    const term = await launchAgentTab({ agent: kind, prompt: "", ...spawnGrid({}) });
    mountTermTab(term, { agent: kind }, { focus: true });
    termView(term);
    sink.focus();
    lines[kind] = {};
    for (const one of script.cases) {
      lines[kind][one.name] = await play(one.ring, one.burst);
    }
    for (const listener of window.__LISTENERS__["term:exited"] ?? []) listener({ payload: { term } });
  }
  delete window.__ANSWER__.term_text;
  delete window.__ANSWER__.term_key;
  return { lines, ms: Math.round(performance.now() - started) };
}

export async function testImeKoreanRegression(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exerciseKoreanRegression, {
      kinds: PANE_KINDS,
      cases: CASES,
      paceMs: PACE_MS,
      settleMs: SETTLE_MS,
    });
    for (const one of CASES) {
      const got = PANE_KINDS.map((kind) => [kind, seen.lines[kind][one.name].line]);
      const wrong = got.filter(([, line]) => JSON.stringify(line) !== JSON.stringify(one.wants));
      ok(
        `${one.name}: 여섯 판 모두 「${one.wants.join("")}」 줄로 나간다`,
        wrong.length === 0,
        JSON.stringify({ wants: one.wants, wrong }),
      );
    }
    const stale = PANE_KINDS.flatMap((kind) => CASES.flatMap((one) =>
      seen.lines[kind][one.name].stale.map((value) => `${kind}/${one.name}:${value}`)));
    ok(
      "조합 사이와 지우기 키 앞에서 편집 칸은 비어 있다 — 낡은 문맥이 남지 않는다",
      stale.length === 0,
      JSON.stringify(stale),
    );
    ok("한국어 회귀 사례를 재생하는 동안 렌더러 오류는 없었다", faults.length === 0, faults.join("\n"));
    console.log(`INFO  ime-korean-regression: ${PANE_KINDS.length} kinds × ${CASES.length} cases in ${seen.ms} ms (page)`);
  } finally {
    await page.close();
  }
}
