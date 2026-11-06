/* ---- 치는 손 밑에서 화면이 바뀌면 글자는 어디로 가는가 (t-69526) ----------
 *
 * 창 기록(window-errors.log, 2026-10-21)이 남긴 순서가 이 수트의 대본이다:
 * 19:46:01.063 `watch main: [6]` — 가운데 화면이 워커 w-68712의 판(term 6)으로
 * 바뀌었고, 1.4초 뒤 19:46:02.485 `term 6 taken over … by ime` — 사람이 치던
 * 한글이 그 판으로 들어가 판이 「사람이 넘겨받음」이 되었다. 사람은 워커에
 * 손대지 않았다고 했고, 판은 다섯 시간 동안 아무 편지도 받지 못했다. 기록에는
 * 화면을 바꾼 길이 남지 않았다.
 *
 * 장면 넷을 이 창의 입력 층에 그대로 재생한다. 글자는 모두 합성이고, 사람이 친
 * 글은 어디에도 없다(t-11740).
 *
 *   1. 사람의 판 A에서 조합하는 중에 프로그램의 길(`notify:activate`)이 워커 판
 *      B를 앞에 세운다. 조합 중이던 음절과 이어 친 글, 그 뒤의 Enter가 B로 가면
 *      안 된다 — A가 화면에서 내려갔으므로 버리고, 한 번 말한다(토스트 하나,
 *      창 기록 한 줄: 종류와 글자 수만). 한 박자 쉰 뒤의 글은 앞에 보이는 판으로
 *      간다. 화면 전환에는 그 길이 적힌다(`by program notify:activate …`).
 *   2. 사람의 클릭은 막지 않는다 — 탭을 누르고 곧바로 친 글은 누른 판으로 가고,
 *      그 전환은 `by person …`으로 적힌다.
 *   3. 조합은 시작한 판으로 — 나란히 선 판 C로 키보드를 옮겨도(`term:focus-pane`)
 *      A가 보이는 동안 조합의 확정 글자와 이어 친 글은 A로 간다.
 *   4. 입력칸에서 치던 중에 프로그램이 키보드를 터미널로 가져가도 그 글은 어느
 *      판으로도 가지 않는다.
 *   5. 사람의 판 A에서 한글을 치는 중에 코디가 워커를 띄우고, 자리 판단의 답이
 *      그 판을 A 옆에 붙이며 키보드를 가져간다(t-97132). 그 답은 백엔드 사건의
 *      `await` 뒤에 오므로 예전에는 프로그램의 이동으로 읽히지 않았다 — 조합 중인
 *      음절은 A로 갔지만 낱말 사이의 띄어쓰기와 Enter가 워커 판으로 가서 판이
 *      「사람이 넘겨받음」이 되었다. 이제 그 글도 A로 간다. 입력칸에서 치던
 *      중이었다면 장면 4처럼 어느 판으로도 가지 않는다.
 *   6. 조합을 열어 둔 채 한 박자 넘게 쉬는 동안 프로그램이 키보드를 나란히 선
 *      판 C로 옮긴다(t-97132). 열린 조합은 타자가 이어지는 중이라는 뜻이므로,
 *      이어 친 음절은 A가 보이는 동안 A로 간다 — 예전에는 열린 음절만 A로 가고
 *      다음 음절부터 C로 갔다.
 *   7. 장면 6처럼 조합을 연 채 쉬다가, 이번에는 사람이 판 C를 클릭한다. 사람의
 *      이동은 런을 닫으므로 열린 음절만 A에서 끝나고 그 뒤의 글은 누른 C로 간다.
 *   8. 열린 조합의 쉼에도 상한이 있다(`OPEN_COMPOSITION_GAP_MS`, t-97449). 그보다
 *      오래 쉬면 사람은 바뀐 화면을 본 것이다 — 열린 음절은 A에서 끝나고 그 뒤의
 *      글은 화면을 따라 C로 간다. 예전에는 열 분을 쉬어도 계속 A로 갔다.
 *   9. 벨이 다른 체크아웃의 워커 판을 부른다(t-97449). 체크아웃을 옮기는 길은
 *      `set_active_worktree`의 답을 기다린 뒤 무대를 그 체크아웃으로 바꾸고, 다시
 *      목록 읽기를 기다린 뒤에야 판을 앞세운다. 그 사이에 이어 친 글과 Enter가
 *      되살아난 워커 판으로 가면 안 되고, 그 무대 바꿈은 벨의 길로 적힌다.
 *
 * 그리고 백엔드가 「워커 판에 사람의 글이 들어갔다」고 말하면(`term:taken-over`)
 * 기존 토스트로 한 번 알린다. 마지막으로 성한 음절의 지연 — 커밋이 닫히는 순간
 * 부터 그 글이 판에 닿기까지 — 을 재어 INFO 줄로 남긴다: 이 고침이 한글 입력을
 * 늦추지 않았다는 수다. */

import { openWindowTestPage } from "./window-boot.mjs";

const SCRIPT = Object.freeze({
  /* 판 번호들 — 이 수트만 쓰는 자리. */
  terms: Object.freeze({
    person: 9901, worker: 9902, splitA: 9911, splitC: 9912, field: 9921, fieldWorker: 9922, latency: 9931,
    summoner: 9941, summoned: 9942, pauseA: 9951, pauseC: 9952, fieldSummoner: 9961, fieldSummoned: 9962,
    clickA: 9971, clickC: 9972, capA: 9981, capC: 9982, bellPerson: 9991, bellWorker: 9992,
  }),
  /* 장면 9의 다른 체크아웃 — 합성. */
  bellCheckout: Object.freeze({ path: "/tmp/zerocode-window-test/wt-t97449-bell", branch: "wt/t97449-bell" }),
  /* 장면 5의 소환: 자리 판단이 읽는 원장 자리 — 모두 합성. */
  summonSeat: Object.freeze({
    run: "run-9941", worker: "w-9942", dispatch: "dp-9942", task: "t-9942", brief: "synthetic brief", briefChars: 15,
  }),
  /* 장면 5의 자리 판단 답: 판을 코디 옆에 붙인다. */
  splitAnswer: Object.freeze({ outcome: "answered", chosen: "split", applied: true, offered: ["tab", "split", "background"] }),
  /* 자리 판단의 답이 판을 옮겨 놓기까지 기다리는 한도. 창의 타자 간격(1초)보다 짧게. */
  answerWaitMs: 600,
  /* 사건 사이 간격: 사람이 치는 속도. 창이 한 줄로 이어진 타자로 읽는 간격(1초) 안이다. */
  paceMs: 60,
  /* 「한 박자 쉰」 간격: 창의 타자 간격(1초)보다 넉넉히 길게. */
  pauseMs: 1300,
  /* 반쪽 글자를 붙드는 48 ms가 판가름날 만큼. */
  settleMs: 160,
  latencyRounds: 40,
  notSentToast: "화면이 바뀌어 친 글자를 보내지 않았습니다",
  /* 합성 글자 하나 — U+1F600, UTF-16 두 단위. */
  wideKey: "\u{1F600}",
});

/* 이 창의 입력 층에 브라우저가 내는 열 그대로를 쏘는 손 — `ime-broken-commit`의
 * 재생과 같은 모양. 페이지 안에서 한 번 세우고, 장면들이 함께 쓴다. */
export function standImeHands(script) {
  const sink = document.getElementById("key-sink");
  const wait = (ms) => new Promise((done) => setTimeout(done, ms));
  const fire = (kind, data) => {
    if (kind === "start") {
      sink.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    } else if (kind === "update") {
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionupdate", { data, bubbles: true }));
      sink.dispatchEvent(new InputEvent("input", { isComposing: true, data, bubbles: true }));
    } else if (kind === "key") {
      sink.dispatchEvent(new KeyboardEvent("keydown", { key: data, keyCode: 229, bubbles: true, cancelable: true }));
    } else if (kind === "end") {
      sink.value = data;
      sink.dispatchEvent(new CompositionEvent("compositionend", { data, bubbles: true }));
    } else if (kind === "enter") {
      sink.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", keyCode: 13, bubbles: true, cancelable: true }));
    } else if (kind === "press") {
      // A key the IME does not hold, by the character it carries.
      sink.dispatchEvent(new KeyboardEvent("keydown", { key: data, bubbles: true, cancelable: true }));
    } else if (kind === "type") {
      // The text such a key inserts outside any composition — the space
      // between two words.
      sink.value = data;
      sink.dispatchEvent(new InputEvent("input", { data, inputType: "insertText", bubbles: true }));
    }
  };
  const rec = { texts: [], keys: [], watches: [], logs: [] };
  window.__ANSWER__.term_text = (args) => {
    rec.texts.push({ term: args.term, text: args.text, human: args.human === true, at: performance.now() });
    return null;
  };
  window.__ANSWER__.term_key = (args) => (rec.keys.push({ term: args.term, key: args.press.key }), null);
  window.__ANSWER__.log_window_error = (args) => (rec.logs.push(String(args?.message ?? "")), null);
  const priorIpc = window.__IPC_LOG__;
  window.__IPC_LOG__ = (command, args, options) => {
    priorIpc?.(command, args, options);
    if (command === "set_watched_terms") {
      rec.watches.push({ terms: [...(args?.terms ?? [])].sort((a, b) => a - b), road: args?.road ?? null });
    }
  };
  window.__IME_HANDS__ = {
    sink,
    rec,
    wait,
    tell: (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    play: async (ring) => {
      for (const [kind, data] of ring) {
        fire(kind, data);
        await wait(script.paceMs);
      }
    },
    /* 음절 하나를 조합 사건으로: 시작, 첫 낱자, 키(229), 완성, 키, 끝. */
    syllable: (first, whole, keys) => [
      ["start"], ["update", first], ["key", keys[0]], ["update", whole], ["key", keys[1]], ["end", whole],
    ],
    /* 끝나지 않은 음절 — macOS의 한글 입력기는 마지막 음절을 다음 키가 올 때까지 조합 중으로 둔다. */
    openSyllable: (first, whole, keys) => [
      ["start"], ["update", first], ["key", keys[0]], ["update", whole], ["key", keys[1]],
    ],
    fire,
    /* 장면 7과 8의 무대: 판 A와 나란히 선 판 C, 키보드는 A에. 「안」을 치고 「녕」을 연 채 둔다. */
    openSyllableBesideC: async (a, c) => {
      const hands = window.__IME_HANDS__;
      await wait(script.pauseMs);
      mountTermTab(a, { agent: "Zo", worktree: activeWorktreePath }, { placement: "tab", focus: true });
      const split = tabOfTerm(a);
      tileTermPane(split, c, { agent: "Zo" }, "right", true, a);
      setActivePane(split, a);
      sink.focus();
      await wait(script.pauseMs);
      const texts = rec.texts.length;
      await hands.play([...hands.syllable("ㅇ", "안", ["ㅇ", "ㅏ"]), ...hands.openSyllable("ㄴ", "녕", ["ㄴ", "ㅇ"])]);
      return texts;
    },
    /* 열린 음절을 닫고 두 음절을 더 친 뒤, A와 C가 받은 글. */
    finishBesideC: async (a, c, texts) => {
      const hands = window.__IME_HANDS__;
      await hands.play([["end", "녕"], ...hands.syllable("ㅎ", "하", ["ㅎ", "ㅏ"]), ...hands.syllable("ㅅ", "세", ["ㅅ", "ㅔ"])]);
      await wait(script.settleMs);
      return { activeC: tabOfTerm(a).activePane === c, toA: hands.textTo(a, texts), toC: hands.textTo(c, texts) };
    },
    textTo: (term, from = 0) => rec.texts.slice(from).filter((one) => one.term === term).map((one) => one.text).join(""),
    keysTo: (term, from = 0) => rec.keys.slice(from).filter((one) => one.term === term).map((one) => one.key),
    roadsTo: (term, from = 0) => rec.watches.slice(from).filter((one) => one.terms.includes(term)).map((one) => one.road),
    toasts: (words) => [...document.querySelectorAll(".toasts .toast")].filter((node) => node.textContent.includes(words)).length,
  };
  return true;
}

/* 장면 1: 사람의 판 A, 워커 판 B, 그리고 프로그램이 B를 앞에 세운다. */
export async function sceneProgramRaisesTheWorker(script) {
  const { rec, wait, tell, play, syllable, textTo, keysTo, roadsTo, toasts, sink } = window.__IME_HANDS__;
  const { person, worker } = script.terms;
  const W = activeWorktreePath;
  mountTermTab(person, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  paneAgents.set(person, "zo");
  tell("term:worker", { parent: person, term: worker, worktree: W, agent: "zo" });
  await wait(script.settleMs);
  setActiveTab(tabOfTerm(person).id);
  sink.focus();
  await wait(script.pauseMs);
  const texts = rec.texts.length;
  const keys = rec.keys.length;
  const logs = rec.logs.length;
  const watches = rec.watches.length;
  await play(syllable("ㅇ", "안", ["ㅇ", "ㅏ"]));
  // 둘째 음절을 조합하는 중에 화면이 바뀐다 — 독 클릭이 벨이 울린 판으로 데려가는 길.
  await play([["start"], ["update", "ㄴ"], ["key", "ㄴ"], ["update", "녀"], ["key", "ㅕ"]]);
  tell("notify:activate", { worktree: W, term: worker });
  await play([["update", "녕"], ["key", "ㅇ"], ["end", "녕"]]);
  await play([
    ...syllable("ㅎ", "하", ["ㅎ", "ㅏ"]),
    ...syllable("ㅅ", "세", ["ㅅ", "ㅔ"]),
    ...syllable("ㅇ", "요", ["ㅇ", "ㅛ"]),
    // A character outside the basic plane, two UTF-16 units long: held off
    // like the rest, and named in the black box by no more than its kind.
    ["press", script.wideKey],
    ["enter"],
  ]);
  await wait(script.settleMs);
  // 백엔드의 넘겨받음 규칙을 흉내 낸 수: 워커 판에 사람의 글이 처음 닿는 순간 하나(t-17644).
  const marked = new Set(rec.texts.slice(texts)
    .filter((one) => one.human && one.text !== "" && one.term === worker).map((one) => one.term)).size;
  const seen = {
    stageShowsWorker: activeTabId === tabOfTerm(worker).id,
    toPerson: textTo(person, texts),
    toWorker: textTo(worker, texts),
    workerKeys: keysTo(worker, keys),
    personKeys: keysTo(person, keys),
    marked,
    toast: toasts(script.notSentToast),
  };
  // 한 박자 쉰 뒤: 사람은 바뀐 화면을 봤고, 이제 친 글은 앞에 보이는 판으로 간다.
  await wait(script.pauseMs);
  const later = rec.texts.length;
  await play(syllable("ㄱ", "가", ["ㄱ", "ㅏ"]));
  await wait(script.settleMs);
  seen.afterPause = textTo(worker, later);
  seen.notSent = rec.logs.slice(logs).filter((line) => line.startsWith("typing not sent"));
  seen.roads = roadsTo(worker, watches);
  // 다음 장면의 클릭은 바뀐 화면을 본 사람의 것이다.
  await wait(script.pauseMs);
  return { seen, person: tabOfTerm(person).id, worker: tabOfTerm(worker).id, watches: rec.watches.length, texts: rec.texts.length };
}

/* 장면 2의 타자 한 음절 — 클릭은 시험이 페이지 밖에서 진짜 마우스로 한다. */
export async function typeOneSyllable({ first, whole, keys }) {
  const { play, syllable } = window.__IME_HANDS__;
  await play(syllable(first, whole, keys));
  return true;
}

export function sceneClickTyping({ texts, watches, person, worker }) {
  const { textTo, roadsTo } = window.__IME_HANDS__;
  return { toPerson: textTo(person, texts), toWorker: textTo(worker, texts), roads: roadsTo(worker, watches) };
}

/* 장면 3: 조합은 시작한 판으로 — 그 판이 보이는 동안. */
export async function sceneCompositionKeepsItsPane(script) {
  const { rec, wait, tell, play, syllable, textTo, sink } = window.__IME_HANDS__;
  const { splitA, splitC } = script.terms;
  const W = activeWorktreePath;
  mountTermTab(splitA, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  const split = tabOfTerm(splitA);
  tileTermPane(split, splitC, { agent: "Zo" }, "right", true, splitA);
  setActivePane(split, splitA);
  sink.focus();
  await wait(script.pauseMs);
  const texts = rec.texts.length;
  await play([["start"], ["update", "ㄱ"], ["key", "ㄱ"], ["update", "가"], ["key", "ㅏ"]]);
  // 코디가 판 하나를 이름으로 불렀다 — 키보드가 C로 간다. 사람의 손은 아니다.
  tell("term:focus-pane", splitC);
  await play([["end", "가"], ...syllable("ㄴ", "나", ["ㄴ", "ㅏ"])]);
  await wait(script.settleMs);
  const seen = {
    activeC: split.activePane === splitC,
    keyboardOnA: keyboardTarget()?.term === splitA,
    toA: textTo(splitA, texts),
    toC: textTo(splitC, texts),
  };
  await wait(script.pauseMs);
  const later = rec.texts.length;
  await play(syllable("ㄷ", "다", ["ㄷ", "ㅏ"]));
  await wait(script.settleMs);
  seen.afterPauseC = textTo(splitC, later);
  return seen;
}

/* 장면 4: 입력칸에서 치던 중에 프로그램이 키보드를 터미널로 가져간다. */
export async function sceneFieldTypingIsNotYanked(script) {
  const { rec, wait, tell, play, syllable, textTo, sink } = window.__IME_HANDS__;
  const { field: pane, fieldWorker } = script.terms;
  const W = activeWorktreePath;
  await wait(script.pauseMs);
  mountTermTab(pane, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  tell("term:worker", { parent: pane, term: fieldWorker, worktree: W, agent: "zo" });
  await wait(script.settleMs);
  const field = document.createElement("input");
  field.type = "text";
  field.setAttribute("aria-label", "t-69526 field");
  document.body.append(field);
  field.focus();
  const texts = rec.texts.length;
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true }));
  field.value += "a";
  await wait(script.paceMs);
  tell("notify:activate", { worktree: W, term: fieldWorker });
  const sinkHasFocus = document.activeElement === sink;
  await play(syllable("ㄹ", "라", ["ㄹ", "ㅏ"]));
  await wait(script.settleMs);
  field.remove();
  return { sinkHasFocus, toWorker: textTo(fieldWorker, texts), toPane: textTo(pane, texts) };
}

/* 백엔드의 말: 워커 판에 사람의 글이 들어갔다 — 두 번 말해도 토스트는 하나. */
export async function sceneTakenOverIsSaidOnce(script) {
  const { wait, tell, toasts } = window.__IME_HANDS__;
  const worker = script.terms.fieldWorker;
  tell("term:taken-over", { term: worker, worker: `w-${worker}` });
  tell("term:taken-over", { term: worker, worker: `w-${worker}` });
  await wait(script.settleMs);
  return toasts(`워커 w-${worker}의 판에 보냈습니다`);
}

/* 장면 5: 사람의 판 A에서 치는 중에 소환된 워커가 A 옆에 붙으며 키보드를 가져간다
 * (t-97132). 자리 판단의 답은 `term:worker` 처리의 `await` 뒤에 온다. */
export async function sceneSummonedWorkerTakesNothing(script) {
  const { rec, wait, tell, play, syllable, openSyllable, textTo, keysTo, sink } = window.__IME_HANDS__;
  const { summoner, summoned } = script.terms;
  const W = activeWorktreePath;
  await wait(script.pauseMs);
  mountTermTab(summoner, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  paneAgents.set(summoner, "zo");
  window.__ANSWER__.judge_worker_room = () => script.splitAnswer;
  sink.focus();
  await wait(script.settleMs);
  const texts = rec.texts.length;
  const keys = rec.keys.length;
  await play([...syllable("ㅇ", "안", ["ㅇ", "ㅏ"]), ...openSyllable("ㄴ", "녕", ["ㄴ", "ㅇ"])]);
  tell("term:worker", { parent: summoner, term: summoned, worktree: W, agent: "zo", seat: script.summonSeat });
  const host = () => tabOfTerm(summoner);
  const asked = performance.now();
  while (host()?.activePane !== summoned && performance.now() - asked < script.answerWaitMs) await wait(5);
  const split = host() === tabOfTerm(summoned) && host()?.activePane === summoned;
  await play([
    ["end", "녕"],
    ["press", " "],
    ["type", " "],
    ...syllable("ㅎ", "하", ["ㅎ", "ㅏ"]),
    ...syllable("ㅅ", "세", ["ㅅ", "ㅔ"]),
    ...syllable("ㅇ", "요", ["ㅇ", "ㅛ"]),
    ["enter"],
  ]);
  await wait(script.settleMs);
  delete window.__ANSWER__.judge_worker_room;
  // 백엔드의 넘겨받음 규칙을 흉내 낸 수: 워커 판에 사람의 글이 닿은 판의 수(t-17644).
  const marked = new Set(rec.texts.slice(texts)
    .filter((one) => one.human && one.text !== "" && one.term === summoned).map((one) => one.term)).size;
  return {
    split,
    toPerson: textTo(summoner, texts),
    toWorker: textTo(summoned, texts),
    personKeys: keysTo(summoner, keys),
    workerKeys: keysTo(summoned, keys),
    marked,
  };
}

/* 장면 5의 입력칸 판: 사람이 입력칸에서 치는 중에 소환된 워커가 옆에 붙으며 키보드를
 * 터미널로 가져간다(t-97132). 그 뒤의 글은 장면 4처럼 어느 판으로도 가지 않는다. */
export async function sceneSummonedWorkerTakesNothingFromAField(script) {
  const { rec, wait, tell, play, syllable, textTo, keysTo, sink } = window.__IME_HANDS__;
  const { fieldSummoner, fieldSummoned } = script.terms;
  const W = activeWorktreePath;
  await wait(script.pauseMs);
  mountTermTab(fieldSummoner, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  paneAgents.set(fieldSummoner, "zo");
  window.__ANSWER__.judge_worker_room = () => script.splitAnswer;
  const field = document.createElement("textarea");
  field.setAttribute("aria-label", "t-97132 field");
  document.body.append(field);
  field.focus();
  const texts = rec.texts.length;
  const keys = rec.keys.length;
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true }));
  field.value += "a";
  await wait(script.paceMs);
  tell("term:worker", {
    parent: fieldSummoner, term: fieldSummoned, worktree: W, agent: "zo", seat: { ...script.summonSeat, worker: `w-${fieldSummoned}` },
  });
  const host = () => tabOfTerm(fieldSummoner);
  const asked = performance.now();
  while (host()?.activePane !== fieldSummoned && performance.now() - asked < script.answerWaitMs) await wait(5);
  const sinkHasFocus = document.activeElement === sink;
  await play([...syllable("ㅎ", "하", ["ㅎ", "ㅏ"]), ["press", " "], ["type", " "], ["enter"]]);
  await wait(script.settleMs);
  delete window.__ANSWER__.judge_worker_room;
  field.remove();
  return {
    split: host() === tabOfTerm(fieldSummoned) && host()?.activePane === fieldSummoned,
    sinkHasFocus,
    toWorker: textTo(fieldSummoned, texts),
    workerKeys: keysTo(fieldSummoned, keys),
    toPane: textTo(fieldSummoner, texts),
  };
}

/* 장면 6: 조합을 열어 둔 채 쉬는 동안 프로그램이 키보드를 나란히 선 판 C로 옮긴다
 * (t-97132). */
export async function scenePausedCompositionKeepsItsPane(script) {
  const { rec, wait, tell, play, syllable, openSyllable, textTo, sink } = window.__IME_HANDS__;
  const { pauseA, pauseC } = script.terms;
  const W = activeWorktreePath;
  await wait(script.pauseMs);
  mountTermTab(pauseA, { agent: "Zo", worktree: W }, { placement: "tab", focus: true });
  const split = tabOfTerm(pauseA);
  tileTermPane(split, pauseC, { agent: "Zo" }, "right", true, pauseA);
  setActivePane(split, pauseA);
  sink.focus();
  await wait(script.pauseMs);
  const texts = rec.texts.length;
  await play([...syllable("ㅇ", "안", ["ㅇ", "ㅏ"]), ...openSyllable("ㄴ", "녕", ["ㄴ", "ㅇ"])]);
  // 사람은 「녕」을 연 채 다음 말을 고른다 — 그 사이 코디가 판 C를 이름으로 부른다.
  await wait(script.settleMs);
  tell("term:focus-pane", pauseC);
  await wait(script.pauseMs);
  await play([["end", "녕"], ...syllable("ㅎ", "하", ["ㅎ", "ㅏ"]), ...syllable("ㅅ", "세", ["ㅅ", "ㅔ"])]);
  await wait(script.settleMs);
  return {
    activeC: split.activePane === pauseC,
    toA: textTo(pauseA, texts),
    toC: textTo(pauseC, texts),
  };
}

/* 장면 7의 앞: 조합을 연 채 쉰다 — 클릭은 시험이 페이지 밖에서 진짜 마우스로 한다. */
export async function sceneOpenPauseBeforeAClick(script) {
  const { clickA, clickC } = script.terms;
  const texts = await window.__IME_HANDS__.openSyllableBesideC(clickA, clickC);
  await window.__IME_HANDS__.wait(script.pauseMs);
  return texts;
}

export function sceneTypingAfterTheClick({ script, texts }) {
  return window.__IME_HANDS__.finishBesideC(script.terms.clickA, script.terms.clickC, texts);
}

/* 장면 8: 열린 조합의 쉼이 상한을 넘는 동안 프로그램이 키보드를 판 C로 옮긴다. 기다리는
 * 시간은 창의 상수에서 읽는다 — 이 수트가 따로 적으면 둘이 갈라진다. */
export async function scenePauseBeyondTheCapFollowsTheScreen(script) {
  const { wait, tell, openSyllableBesideC, finishBesideC } = window.__IME_HANDS__;
  const { capA, capC } = script.terms;
  const capMs = typeof OPEN_COMPOSITION_GAP_MS === "number" ? OPEN_COMPOSITION_GAP_MS : null;
  if (capMs === null) return { capMs };
  const texts = await openSyllableBesideC(capA, capC);
  await wait(script.settleMs);
  tell("term:focus-pane", capC);
  await wait(capMs + script.pauseMs);
  return { capMs, ...(await finishBesideC(capA, capC, texts)) };
}

/* 장면 9: 벨이 다른 체크아웃의 워커 판을 부르는 동안 사람이 계속 친다(t-97449). 그
 * 체크아웃의 목록 읽기를 붙들어, 무대가 그 체크아웃으로 바뀐 뒤 벨이 판을 앞세우기
 * 전의 틈에 글을 친다. */
export async function sceneBellInAnotherCheckoutTakesNothing(script) {
  const { rec, wait, play, syllable, openSyllable, textTo, keysTo, roadsTo, sink } = window.__IME_HANDS__;
  const { bellPerson, bellWorker } = script.terms;
  const { path: there, branch } = script.bellCheckout;
  await wait(script.pauseMs);
  mountTermTab(bellPerson, { agent: "Zo", worktree: activeWorktreePath }, { placement: "tab", focus: true });
  paneAgents.set(bellPerson, "zo");
  mountTermTab(bellWorker, { agent: "Zo", worktree: there }, { placement: "tab", focus: false });
  paneAgents.set(bellWorker, "zo");
  // 전에 들른 체크아웃이다: 무대는 그 체크아웃의 살아 있는 에이전트 판으로 곧바로 돌아온다.
  restoredWorkspaces.add(there);
  setActiveTab(tabOfTerm(bellPerson).id);
  const catalog = await invoke("project_catalog");
  let release;
  const held = new Promise((done) => { release = done; });
  window.__ANSWER__.project_catalog = () => held.then(() => catalog);
  window.__ANSWER__.set_active_worktree = () => branch;
  sink.focus();
  await wait(script.settleMs);
  const texts = rec.texts.length;
  const keys = rec.keys.length;
  const watches = rec.watches.length;
  await play([...syllable("ㅇ", "안", ["ㅇ", "ㅏ"]), ...openSyllable("ㄴ", "녕", ["ㄴ", "ㅇ"])]);
  // 벨의 번쩍임은 움직임 줄이기 아래에서 붙자마자 벗으므로, 벗기 전의 값까지 기록으로 읽는다.
  const called = (classes) => (classes ?? "").split(/\s+/).includes("is-called");
  let flashed = false;
  const watching = new MutationObserver((records) => {
    for (const one of records) {
      if (one.target.matches?.(`.pane-slot[data-term="${bellWorker}"]`) &&
        (called(one.oldValue) || one.target.classList.contains("is-called"))) flashed = true;
    }
  });
  watching.observe(document.body, { subtree: true, attributes: true, attributeOldValue: true, attributeFilter: ["class"] });
  // 벨 처리기의 약속: 그것이 끝났는지가 「아직 틈인가」의 답이다.
  let answered = false;
  const answering = Promise.all((window.__LISTENERS__["notify:activate"] ?? [])
    .map((hear) => hear({ payload: { worktree: there, term: bellWorker } })))
    .finally(() => { answered = true; });
  const worker = () => tabOfTerm(bellWorker);
  const asked = performance.now();
  while (activeTabId !== worker()?.id && performance.now() - asked < script.answerWaitMs) await wait(5);
  const restored = activeTabId === worker()?.id;
  await play([
    ["end", "녕"],
    ["press", " "],
    ["type", " "],
    ...syllable("ㅎ", "하", ["ㅎ", "ㅏ"]),
    ...syllable("ㅅ", "세", ["ㅅ", "ㅔ"]),
    ["enter"],
  ]);
  // 벨은 아직 답하는 중이다 — 위의 글은 모두 그 틈에 쳤다.
  const inTheGap = !answered && !flashed;
  release();
  delete window.__ANSWER__.project_catalog;
  delete window.__ANSWER__.set_active_worktree;
  await Promise.race([answering, wait(script.answerWaitMs)]);
  await wait(script.settleMs);
  for (const one of watching.takeRecords()) {
    if (one.target.matches?.(`.pane-slot[data-term="${bellWorker}"]`) && called(one.oldValue)) flashed = true;
  }
  watching.disconnect();
  const marked = new Set(rec.texts.slice(texts)
    .filter((one) => one.human && one.text !== "" && one.term === bellWorker).map((one) => one.term)).size;
  return {
    restored,
    inTheGap,
    called: answered && flashed,
    toPerson: textTo(bellPerson, texts),
    toWorker: textTo(bellWorker, texts),
    workerKeys: keysTo(bellWorker, keys),
    marked,
    roads: roadsTo(bellWorker, watches),
  };
}

/* 지연: 성한 음절의 커밋이 닫히는 순간부터 그 글이 판에 닿기까지. */
export async function sceneSyllableLatency(script) {
  const { rec, wait, play, sink, fire } = window.__IME_HANDS__;
  const pane = script.terms.latency;
  await wait(script.pauseMs);
  mountTermTab(pane, { agent: "Zo", worktree: activeWorktreePath }, { placement: "tab", focus: true });
  sink.focus();
  await wait(script.settleMs);
  const delays = [];
  for (let n = 0; n < script.latencyRounds; n += 1) {
    const from = rec.texts.length;
    await play([["start"], ["update", "ㄱ"], ["key", "ㄱ"], ["update", "가"], ["key", "ㅏ"]]);
    const closed = performance.now();
    fire("end", "가");
    const landed = rec.texts.slice(from).find((one) => one.term === pane && one.text === "가");
    delays.push(landed ? landed.at - closed : null);
    await wait(script.paceMs);
  }
  const measured = delays.filter((one) => one !== null).sort((a, b) => a - b);
  const at = (q) => (measured.length
    ? Number(measured[Math.min(measured.length - 1, Math.floor(q * measured.length))].toFixed(3))
    : null);
  return {
    rounds: delays.length,
    landed: measured.length,
    p50: at(0.5),
    p90: at(0.9),
    max: measured.length ? Number(measured.at(-1).toFixed(3)) : null,
  };
}

export async function testImeStageSwitch(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standImeHands, SCRIPT);
    const first = await page.evaluate(sceneProgramRaisesTheWorker, SCRIPT);
    const one = first.seen;
    // 장면 2: 진짜 마우스로 A의 탭을 누르고 한 음절, B의 탭을 누르고 곧바로 한 음절.
    await page.click(`[data-tab="${first.person}"]`);
    await page.evaluate(typeOneSyllable, { first: "ㄷ", whole: "다", keys: ["ㄷ", "ㅏ"] });
    await page.click(`[data-tab="${first.worker}"]`);
    await page.evaluate(typeOneSyllable, { first: "ㄹ", whole: "라", keys: ["ㄹ", "ㅏ"] });
    await page.waitForTimeout(SCRIPT.settleMs);
    const two = await page.evaluate(sceneClickTyping, {
      texts: first.texts, watches: first.watches, person: SCRIPT.terms.person, worker: SCRIPT.terms.worker,
    });
    const three = await page.evaluate(sceneCompositionKeepsItsPane, SCRIPT);
    const four = await page.evaluate(sceneFieldTypingIsNotYanked, SCRIPT);
    const takenToast = await page.evaluate(sceneTakenOverIsSaidOnce, SCRIPT);
    const five = await page.evaluate(sceneSummonedWorkerTakesNothing, SCRIPT);
    const fiveField = await page.evaluate(sceneSummonedWorkerTakesNothingFromAField, SCRIPT);
    const six = await page.evaluate(scenePausedCompositionKeepsItsPane, SCRIPT);
    // 장면 7: 조합을 연 채 쉰 뒤 진짜 마우스로 C를 누르고 이어 친다.
    const sevenTexts = await page.evaluate(sceneOpenPauseBeforeAClick, SCRIPT);
    await page.click(`.pane-slot[data-term="${SCRIPT.terms.clickC}"]`);
    const seven = await page.evaluate(sceneTypingAfterTheClick, { script: SCRIPT, texts: sevenTexts });
    const eight = await page.evaluate(scenePauseBeyondTheCapFollowsTheScreen, SCRIPT);
    const latency = await page.evaluate(sceneSyllableLatency, SCRIPT);
    // 체크아웃을 옮기므로 마지막에.
    const nine = await page.evaluate(sceneBellInAnotherCheckoutTakesNothing, SCRIPT);

    console.log(
      `INFO  ime-stage-switch: scene 1 — to the worker ${[...one.toWorker].length} letters + ${one.workerKeys.length} keys, ` +
      `takeover marks ${one.marked}, not-sent toasts ${one.toast}, not-sent lines ${one.notSent.length}; ` +
      `syllable latency p50 ${latency.p50} ms, p90 ${latency.p90} ms, max ${latency.max} ms over ${latency.landed}/${latency.rounds}`,
    );
    ok(
      "장면 1: 조합 중에 프로그램이 워커 판 B를 앞에 세워도 그 뒤의 글과 Enter는 B로 가지 않고, B는 넘겨받음이 되지 않는다",
      one.stageShowsWorker && one.toPerson === "안" && one.toWorker === "" && one.workerKeys.length === 0 &&
        one.personKeys.length === 0 && one.marked === 0,
      JSON.stringify(one),
    );
    ok(
      "장면 1: 버린 글은 한 번 말한다 — 토스트 하나, 창 기록 한 줄(종류와 글자 수만, 글자는 없이)",
      one.toast === 1 && one.notSent.length === 1 &&
        one.notSent[0].includes("ime 4") && one.notSent[0].includes("key Enter 1") &&
        one.notSent[0].includes("key chord 1") && !one.notSent[0].includes(SCRIPT.wideKey) &&
        /^[\x20-\x7e·]*$/.test(one.notSent[0]),
      JSON.stringify(one.notSent),
    );
    ok(
      "장면 1: 한 박자 쉰 뒤의 글은 앞에 보이는 판으로 간다",
      one.afterPause === "가",
      JSON.stringify({ afterPause: one.afterPause }),
    );
    ok(
      "장면 1: 그 화면 전환은 창 기록에 프로그램의 길로 적힌다",
      one.roads.length > 0 && typeof one.roads[0] === "string" && one.roads[0].startsWith("program notify:activate"),
      JSON.stringify(one.roads),
    );
    ok(
      "장면 2: 사람이 탭을 누르고 곧바로 친 글은 누른 판으로 가고, 그 전환은 사람의 길로 적힌다",
      two.toPerson === "다" && two.toWorker === "라" && two.roads.length > 0 &&
        two.roads.every((road) => typeof road === "string" && road.startsWith("person ")),
      JSON.stringify(two),
    );
    ok(
      "장면 3: 키보드가 나란히 선 판 C로 옮겨 가도, A가 보이는 동안 조합의 확정 글자와 이어 친 글은 A로 간다",
      three.activeC && three.keyboardOnA && three.toA === "가나" && three.toC === "",
      JSON.stringify(three),
    );
    ok("장면 3: 한 박자 쉰 뒤에는 키보드가 C에 있다", three.afterPauseC === "다", JSON.stringify(three));
    ok(
      "장면 4: 입력칸에서 치던 글은 프로그램이 키보드를 터미널로 가져가도 어느 판으로도 가지 않는다",
      four.sinkHasFocus && four.toWorker === "" && four.toPane === "",
      JSON.stringify(four),
    );
    ok("백엔드가 워커 판에 사람의 글이 들어갔다고 두 번 말해도 토스트는 한 번이다", takenToast === 1, JSON.stringify({ takenToast }));
    ok(
      "장면 5: 치는 중에 소환된 워커가 옆에 붙으며 키보드를 가져가도, 띄어쓰기와 Enter까지 사람의 판으로 가고 워커 판은 넘겨받음이 되지 않는다",
      five.split && five.toPerson === "안녕 하세요" && five.personKeys.join() === "Enter" &&
        five.toWorker === "" && five.workerKeys.length === 0 && five.marked === 0,
      JSON.stringify(five),
    );
    ok(
      "장면 5: 입력칸에서 치는 중에 소환된 워커가 키보드를 터미널로 가져가도, 그 뒤의 띄어쓰기와 Enter는 워커 판으로 가지 않는다",
      fiveField.split && fiveField.sinkHasFocus && fiveField.toWorker === "" && fiveField.workerKeys.length === 0 &&
        fiveField.toPane === "",
      JSON.stringify(fiveField),
    );
    ok(
      "장면 6: 조합을 열어 둔 채 쉬는 동안 키보드가 판 C로 옮겨 가도, 이어 친 음절은 A가 보이는 동안 A로 간다",
      six.activeC && six.toA === "안녕하세" && six.toC === "",
      JSON.stringify(six),
    );
    ok(
      "장면 7: 조합을 연 채 쉬다가 사람이 판 C를 누르면, 열린 음절만 A에서 끝나고 그 뒤의 글은 누른 C로 간다",
      seven.activeC && seven.toA === "안녕" && seven.toC === "하세",
      JSON.stringify(seven),
    );
    ok(
      "장면 8: 열린 조합의 쉼이 상한을 넘으면 열린 음절만 A에서 끝나고 그 뒤의 글은 화면을 따라 C로 간다",
      eight.capMs !== null && eight.activeC && eight.toA === "안녕" && eight.toC === "하세",
      JSON.stringify(eight),
    );
    ok(
      "장면 9: 벨이 다른 체크아웃의 워커 판을 부르는 틈에 친 글과 Enter는 워커 판으로 가지 않고, 판은 넘겨받음이 되지 않는다",
      nine.restored && nine.inTheGap && nine.called && nine.toPerson === "안" && nine.toWorker === "" &&
        nine.workerKeys.length === 0 && nine.marked === 0,
      JSON.stringify(nine),
    );
    ok(
      "장면 9: 그 체크아웃으로 무대를 바꾼 이동은 창 기록에 벨의 길로 적힌다",
      nine.roads.length > 0 && typeof nine.roads[0] === "string" && nine.roads[0].startsWith("program notify:activate"),
      JSON.stringify(nine.roads),
    );
    ok(
      "성한 음절은 커밋이 닫힌 그 박자에 판에 닿는다",
      latency.landed === latency.rounds && latency.max !== null && latency.max < 16,
      JSON.stringify(latency),
    );
    ok("화면 전환 장면을 재생하는 동안 렌더러 오류는 없었다", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
