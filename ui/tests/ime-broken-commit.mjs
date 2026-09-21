/* ---- 조합이 반쪽에서 끊기면 낱자가 판으로 새어 나간다 (t-5835) ----------
 *
 * 창 로그(`window-errors.log`, 2026-09-22 00:5x~01:2x)에 「ime: bare jamo
 * left through drain: ㄴ/ㅎ/ㅎ/ㅁ/ㅣ」가 다섯 번 찍혔고, 줄마다 그 직전의 IME
 * 흔적 링이 따라붙었다. 그 링이 이 수트의 대본이다 — 아래 이벤트 열과 간격은
 * 지어낸 것이 아니라 그 로그에서 옮겨 적은 것이고, 링이 서로 겹치는 구간에서는
 * 사건 뒤에 사람이 무엇을 이어 쳤는지까지 읽어 왔다.
 *
 * 링이 말해 주는 이 창의 규칙 둘:
 *
 *   1. 조합 이벤트가 먼저 오고, 그것을 일으킨 keydown(229)이 뒤에 온다.
 *      `comp.start`·`comp.update ㅍ` 다음에 `key.ime ㅍ`가 2 ms 뒤에 찍힌다.
 *   2. 그래서 성한 커밋은 언제나 제 키를 바로 뒤에 달고 있다 — 로그에 남은
 *      성한 커밋 444건에서 커밋 다음 사건까지 p50 3 ms·p90 15 ms이고 404건이
 *      20 ms 안이다. 반대로 사람이 아무것도 누르지 않았는데 조합을 빼앗긴
 *      커밋은 가장 빠른 것도 97 ms 뒤에야 다음 사건을 봤다.
 *
 * 그러니 「반쪽 글자만 든 커밋 뒤에 아무 키도 오지 않는다」가 사건의 서명이다.
 * 이 수트는 그 서명을 다섯 번 재생해 낱자가 판에 닿는지 세고, 같은 로그에서
 * 뽑은 성한 조합들(가·받침 교정·ㅋㅋ)과 조합 중 Enter·백스페이스·훔쳐진
 * 머리(계속)를 대조군으로 둔다. */

import { openWindowTestPage } from "./window-boot.mjs";

/* 흔적 링 한 줄: [앞 줄로부터 ms, 무엇, 글]. `key`는 IME가 쥔 keydown(229),
 * `update`의 글은 그 순간 조합 상자에 선 글자 — 드레인이 읽는 sink 값이다. */
const LEAKS = [
  // 6467줄: 「…데」 뒤 `니`를 백스페이스로 `ㄴ`까지 지웠더니 98 ms 뒤 혼자 끊겼다.
  { jamo: "ㄴ", ring: [
    [0, "start"], [0, "update", "니"], [1, "key", "ㅣ"],
    [490, "update", "ㄴ"], [1, "key", "Backspace"],
    [98, "update", "ㄴ"], [2, "end", "ㄴ"],
  ] },
  // 6714줄: `ㅎ` 하나를 친 뒤 410 ms, 그리고 99 ms 뒤 사람은 `아…`를 이어 쳤다.
  { jamo: "ㅎ", ring: [
    [0, "start"], [0, "update", "ㅎ"], [2, "key", "ㅎ"],
    [410, "update", "ㅎ"], [2, "end", "ㅎ"],
    [99, "start"], [0, "update", "ㅇ"], [2, "key", "ㅇ"],
    [156, "update", "아"], [9, "key", "ㅏ"],
    [34, "update", "앟"], [1, "key", "ㅎ"],
    [202, "update", "아"], [4, "end", "아"],
  ], thenTyped: "아" },
  // 6921줄: 「그렇게 」 뒤 같은 모양, 그리고 105 ms 뒤 `안…`.
  { jamo: "ㅎ", ring: [
    [0, "start"], [0, "update", "ㅎ"], [2, "key", "ㅎ"],
    [308, "update", "ㅎ"], [2, "end", "ㅎ"],
    [105, "start"], [0, "update", "ㅇ"], [2, "key", "ㅇ"],
    [210, "update", "아"], [1, "key", "ㅏ"],
    [3, "update", "안"], [1, "key", "ㄴ"],
    [123, "update", "않"], [1, "key", "ㅎ"],
    [75, "update", "안"], [2, "end", "안"],
  ], thenTyped: "안" },
  // 7143줄: 초점이 판 여기저기를 오간 뒤 `ㅁ` 하나, 581 ms, 그리고 아무것도.
  { jamo: "ㅁ", ring: [
    [0, "start"], [0, "update", "ㅁ"], [2, "key", "ㅁ"],
    [581, "update", "ㅁ"], [2, "end", "ㅁ"],
  ] },
  // 7371줄: 성한 `미` 하나가 먼저 나가고(대조군이 사건 안에 들어 있다),
  // 이어진 `ㅣ` 조합이 481 ms 뒤 혼자 끊겼다.
  { jamo: "ㅣ", ring: [
    [0, "start"], [0, "update", "ㅁ"], [1, "key", "ㅁ"],
    [160, "update", "미"], [1, "key", "ㅣ"],
    [98, "update", "미"], [2, "end", "미"],
    [3, "start"], [0, "update", "ㅣ"], [1, "key", "ㅣ"],
    [481, "update", "ㅣ"], [2, "end", "ㅣ"],
  ], alsoTyped: "미" },
];

/* 성한 조합들 — 로그에서 옮겼거나(받침 교정) 같은 모양으로 지었다.
 * `sends`는 이 열이 판에 내놓아야 하는 글월 전부, 친 차례대로. */
const HEALTHY = [
  // 가: 한 음절이 통째로 커밋되고, 그것을 일으킨 다음 조합이 2 ms 뒤에 선다.
  { name: "가", ring: [
    [0, "start"], [0, "update", "ㄱ"], [2, "key", "ㄱ"],
    [116, "update", "가"], [1, "key", "ㅏ"],
    [140, "update", "가"], [2, "end", "가"],
    [2, "start"], [0, "update", "ㄴ"], [2, "key", "ㄴ"],
  ], sends: "가" },
  // 받침 교정: 로그 6467줄 링의 머리 그대로 — 풋에 ㅜ가 오면 ㅅ이 다음
  // 음절로 옮겨 가 푸가 커밋되고 수가 새로 선다.
  { name: "받침교정", ring: [
    [0, "start"], [0, "update", "ㅍ"], [2, "key", "ㅍ"],
    [56, "update", "푸"], [1, "key", "ㅜ"],
    [155, "update", "풋"], [1, "key", "ㅅ"],
    [137, "update", "푸"], [3, "end", "푸"],
    [5, "start"], [0, "update", "수"], [1, "key", "ㅜ"],
  ], sends: "푸" },
  // ㅋㅋ: 낱자가 정말로 판에 가야 하는 경우. 둘째 ㅋ이 첫 ㅋ을 커밋시키고 그
  // 키는 2 ms 뒤에 온다 — 사람이 아무리 천천히 쳐도 이 간격은 엔진의 것이다.
  { name: "ㅋㅋ", ring: [
    [0, "start"], [0, "update", "ㅋ"], [2, "key", "ㅋ"],
    [340, "update", "ㅋ"], [2, "end", "ㅋ"],
    [2, "start"], [0, "update", "ㅋ"], [2, "key", "ㅋ"],
    [295, "update", "ㅋ"], [2, "end", "ㅋ"],
    [2, "key", " "],
  ], sends: "ㅋ|ㅋ" },
];

/* 조합 중 Enter·백스페이스·훔쳐진 머리. 앞의 둘은 IME가 쥔 키이고, 셋째는
 * 입력 문맥이 늦어 이 창이 직접 조합한 머리가 단어로 이어 붙는 길이다. */
const HABITS = [
  // Enter는 조합을 커밋시키는 키다. `commitEnter`가 쓰인 차례 — 키가 먼저 오고
  // 조합이 끝난다 — 에서는 글이 먼저, Enter가 뒤에 다시 발화된다.
  { name: "enter", ring: [
    [0, "start"], [0, "update", "ㅎ"], [2, "key", "ㅎ"],
    [90, "update", "하"], [1, "key", "ㅏ"], [90, "update", "한"], [1, "key", "ㄴ"],
    [90, "key", "Enter"], [1, "update", "한"], [2, "end", "한"],
  ], says: "한+Enter" },
  // 이 창의 흔적 링이 보여 주는 차례 — 조합이 먼저 끝나고 229 Enter가 뒤에
  // 온다 — 에서는 음절이 꼭 한 번 나가고, 텍스트로 줄바꿈이 새지 않는다.
  // (그 Enter가 키로 다시 발화되지 않는 것은 이 고침 이전부터의 일이고
  //  t-5835의 범위가 아니다 — 보고서에 적는다.)
  { name: "enter-끝먼저", ring: [
    [0, "start"], [0, "update", "ㅎ"], [2, "key", "ㅎ"],
    [90, "update", "하"], [1, "key", "ㅏ"], [90, "update", "한"], [1, "key", "ㄴ"],
    [90, "update", "한"], [2, "end", "한"], [2, "key", "Enter"],
  ], says: "한+" },
  { name: "backspace", ring: [
    [0, "start"], [0, "update", "ㄱ"], [2, "key", "ㄱ"],
    [90, "update", "가"], [1, "key", "ㅏ"], [90, "update", "ㄱ"], [1, "key", "Backspace"],
    [90, "update", "가"], [1, "key", "ㅏ"], [90, "update", "가"], [2, "end", "가"],
    [2, "start"], [0, "update", "ㄴ"], [2, "key", "ㄴ"],
  ], says: "가+" },
  { name: "계속", ring: [
    [0, "raw", "ㄱ"],
    [0, "start"], [0, "update", "ㅖ"], [2, "key", "ㅖ"],
    [90, "update", "계"], [2, "end", "ㅖ"],
    [2, "start"], [0, "update", "ㅅ"], [2, "key", "ㅅ"],
    [90, "update", "소"], [1, "key", "ㅗ"], [90, "update", "속"], [1, "key", "ㄱ"],
    [90, "update", "속"], [2, "end", "속"], [2, "raw", "Enter"],
  ], says: "계속+Enter" },
];

/* 조합 지연: 커밋이 닫히는 순간(`compositionend`)부터 그 글월이 판에 닿기까지.
 * `before`를 치고 시계를 세운 뒤 `rest`를 링의 간격대로 마저 친다 — 음절
 * 하나와 낱자 하나를 같은 자로 잰다. */
const LATENCY = [
  {
    name: "syllable", sends: "가",
    before: [[0, "start"], [0, "update", "ㄱ"], [2, "key", "ㄱ"],
      [60, "update", "가"], [1, "key", "ㅏ"], [80, "update", "가"]],
    rest: [[2, "end", "가"], [2, "start"], [0, "update", "ㄴ"], [2, "key", "ㄴ"],
      [60, "update", "나"], [1, "key", "ㅏ"], [60, "update", "나"], [2, "end", "나"]],
  },
  {
    name: "bareJamo", sends: "ㅋ",
    before: [[0, "start"], [0, "update", "ㅋ"], [2, "key", "ㅋ"], [300, "update", "ㅋ"]],
    rest: [[2, "end", "ㅋ"], [2, "start"], [0, "update", "ㅋ"], [2, "key", "ㅋ"],
      [295, "update", "ㅋ"], [2, "end", "ㅋ"], [2, "key", " "]],
  },
];

/* 한 박자 더 — 붙들린 반쪽이 있으면 여기서 판가름난다. 흔적 링의 사건들이
 * 다음 사건을 본 가장 이른 시각(97 ms)보다 넉넉히 길게. */
const SETTLE_MS = 160;

/* 커밋이 닫힌 뒤 글월이 판에 닿기까지 허용하는 시간. 링에서 그 글월을 놓아
 * 주는 사건(다음 조합의 첫 키)은 커밋 뒤 6 ms 안에 오므로, 한 프레임이면
 * 낱자도 음절도 같은 박자라는 말이 된다. */
const LATENCY_BOUND_MS = 16;

export async function exerciseBrokenCommit(script) {
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const settle = () => wait(script.settleMs);
  const seen = { leaked: [], kept: [], healthy: [], habits: [], latency: {} };
  const term = await openTermTab();
  termView(term);
  const sink = document.getElementById("key-sink");
  sink.focus();

  const texts = [];
  const keys = [];
  window.__ANSWER__.term_text = (args) => (texts.push(args.text), null);
  window.__ANSWER__.term_key = (args) => (keys.push(args.press.key), null);

  /* 실제 IME가 내는 열 그대로: `compositionstart` → `compositionupdate` →
   * 조합 중 `input` → 229 `keydown`, 끝은 `compositionend`. sink 값은
   * 드레인이 읽는 자리이므로 update와 end가 모두 세운다. `raw`는 IME가 쥐지
   * 않은 평범한 keydown. */
  const replay = async (ring) => {
    for (const [gap, kind, data] of ring) {
      if (gap > 0) await wait(gap);
      if (kind === "start") {
        sink.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
      } else if (kind === "update") {
        sink.value = data;
        sink.dispatchEvent(new CompositionEvent("compositionupdate", { data, bubbles: true }));
        sink.dispatchEvent(new InputEvent("input", { isComposing: true, data, bubbles: true }));
      } else if (kind === "key") {
        sink.dispatchEvent(new KeyboardEvent("keydown", {
          key: data, keyCode: 229, bubbles: true, cancelable: true,
        }));
      } else if (kind === "raw") {
        sink.dispatchEvent(new KeyboardEvent("keydown", {
          key: data, bubbles: true, cancelable: true,
        }));
      } else {
        sink.value = data;
        sink.dispatchEvent(new CompositionEvent("compositionend", { data, bubbles: true }));
      }
    }
  };
  const onlyJamo = (text) => /^[\u3130-\u318f]+$/.test(text);

  // 사건 다섯. 끊긴 커밋이 판에 내놓은 낱자만 센다.
  for (const incident of script.leaks) {
    texts.length = 0;
    await replay(incident.ring);
    await settle();
    const strayed = texts.filter(onlyJamo);
    if (strayed.length) seen.leaked.push(`${incident.jamo}:${strayed.join("")}`);
    // 사람이 이어 친 글은 그대로 가야 한다.
    for (const one of [incident.alsoTyped, incident.thenTyped]) {
      if (one && !texts.includes(one)) seen.kept.push(`missing:${one}`);
    }
  }

  // 성한 조합들. 낼 것을 내고, 낼 것만 낸다.
  for (const one of script.healthy) {
    texts.length = 0;
    await replay(one.ring);
    await settle();
    seen.healthy.push(`${one.name}=${texts.join("|")}`);
  }

  // 조합 중 Enter·백스페이스, 그리고 훔쳐진 머리의 단어 이음.
  for (const one of script.habits) {
    texts.length = 0;
    keys.length = 0;
    await replay(one.ring);
    await settle();
    seen.habits.push(`${one.name}=${texts.join("|")}+${keys.join("|")}`);
  }

  // 지연: 커밋이 닫히는 순간부터 그 글월이 판에 닿기까지.
  for (const one of script.latency) {
    texts.length = 0;
    await replay(one.before);
    let at = null;
    window.__ANSWER__.term_text = (args) => {
      texts.push(args.text);
      if (at === null && args.text === one.sends) at = performance.now();
      return null;
    };
    const from = performance.now();
    await replay(one.rest);
    await settle();
    window.__ANSWER__.term_text = (args) => (texts.push(args.text), null);
    seen.latency[one.name] = at === null ? null : Number((at - from).toFixed(1));
  }

  delete window.__ANSWER__.term_text;
  delete window.__ANSWER__.term_key;
  for (const listener of window.__LISTENERS__["term:exited"] ?? []) listener({ payload: { term } });
  return seen;
}

export async function testImeBrokenCommit(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exerciseBrokenCommit, {
      leaks: LEAKS, healthy: HEALTHY, habits: HABITS, latency: LATENCY, settleMs: SETTLE_MS,
    });
    ok(
      "반쪽에서 끊긴 다섯 사건을 로그의 간격 그대로 재생해도 낱자는 판에 닿지 않고, 사람이 이어 친 글은 남는다",
      seen.leaked.length === 0 && seen.kept.length === 0,
      JSON.stringify({ leaked: seen.leaked, kept: seen.kept }),
    );
    ok(
      "성한 조합은 그대로다 — 가와 받침 교정은 음절째로, ㅋㅋ는 낱자 둘로 판에 간다",
      seen.healthy.join(";") === HEALTHY.map((one) => `${one.name}=${one.sends}`).join(";"),
      JSON.stringify(seen.healthy),
    );
    ok(
      "조합 중 Enter는 글 다음에 키로, 백스페이스는 고친 음절만, 훔쳐진 머리는 단어로 이어진다",
      seen.habits.join(";") === HABITS.map((one) => `${one.name}=${one.says}`).join(";"),
      JSON.stringify(seen.habits),
    );
    ok(
      "커밋을 일으킨 키가 온 순간부터 판까지, 낱자도 음절과 같은 박자다",
      LATENCY.every((one) =>
        seen.latency[one.name] !== null && seen.latency[one.name] < LATENCY_BOUND_MS),
      JSON.stringify(seen.latency),
    );
    ok("반쪽 커밋을 재생하는 동안 렌더러 오류는 없었다", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
