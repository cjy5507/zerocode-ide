/* 아티팩트 — 흐름 (t-68458).
 *
 * 사람이 통과시킨 2차 시안(아티팩트 p-4958d4720ba9e103 v3)의 첫 보기: 지금 보는 프로젝트에서
 * 무슨 일이 있었는지를 묶음 셋 — 지금 · 다음 발행 전 · 발행된 것 — 으로 세운다. 과업 하나는 한
 * 줄이다: 한 일의 문장과 전→후 수, 여섯 걸음의 띠와 상태의 말, 남긴 아티팩트의 칩. 줄을 펼치면
 * 무엇 · 왜 · 대가와 걸음마다의 한 줄이 시간순으로 서고, 칩을 누르면 곁의 읽는 칸(서랍 그대로)에
 * 그 아티팩트가 서며 칸의 머리가 어느 걸음의 것인지 말한다.
 *
 * 줄이 말하는 사실은 런타임이 원장과 git에서 읽은 한 벌(`artifact_flow`)뿐이다 — 단계의 낱말은
 * 데스크의 것(desk.rs `STAGES`)이고 착지 · 발행의 시각은 git이 말한다. 여기는 그 낱말에 붙는 말과
 * 표식, 걸음의 차례를 정한다.
 *
 * 그리기는 갤러리의 규칙 그대로다: 줄은 과업 id로 살아남아 바뀐 줄만 다시 입고, 같은 답이 다시
 * 오면 아무것도 쓰지 않는다. 묻는 일은 탭을 열 때 · 프로젝트를 바꿀 때 · 카탈로그나 원장이
 * 움직였다는 말이 올 때뿐이다. */

/* 여섯 걸음, 차례대로 — 줄의 띠와 펼친 걸음의 목록이 이 표 하나를 읽는다. */
const ARTIFACT_FLOW_STEPS = Object.freeze([
  { step: "task", key: "artifacts.flow.stepTask", word: "과업" },
  { step: "try", key: "artifacts.flow.stepTry", word: "시도" },
  { step: "handin", key: "artifacts.flow.stepHandin", word: "넘김" },
  { step: "review", key: "artifacts.flow.stepReview", word: "검토" },
  { step: "land", key: "artifacts.flow.stepLand", word: "착지" },
  { step: "ship", key: "artifacts.flow.stepShip", word: "발행" },
]);

/* 데스크의 단계 낱말(desk.rs `STAGES`) → 상태의 말 · 표식의 모양 · 신호 · 지금 걸음. 색만으로
 * 말하지 않는다: 모양과 말이 같이 선다. `at`은 그 단계가 서 있는 걸음이고(그 앞은 끝난 걸음),
 * `stop`은 그 걸음에서 멈췄다는 뜻, `skip`은 건너뛰는 걸음이다. `at`이 없으면 걸음이 모두
 * 끝났고, `unread`는 원장이 말하지 못한다는 뜻이다(과업 다음의 걸음은 모른다). */
const ARTIFACT_FLOW_WAITING = Object.freeze({ key: "artifacts.flow.stageWaiting", word: "차례 기다림", shape: "wait", signal: "quiet", at: "try" });
const ARTIFACT_FLOW_HALTED = Object.freeze({ key: "artifacts.flow.stageHalted", word: "멈춤", shape: "cross", signal: "halt", at: "try", stop: true });
const ARTIFACT_FLOW_STAGES = Object.freeze({
  pending: ARTIFACT_FLOW_WAITING,
  ready: ARTIFACT_FLOW_WAITING,
  dispatched: { key: "artifacts.flow.stageWorking", word: "도는 중", shape: "ring", signal: "ready", at: "try" },
  gate: { key: "artifacts.flow.stageGate", word: "결정 기다림", shape: "half", signal: "wait", at: "try" },
  blocked: ARTIFACT_FLOW_HALTED,
  reported: { key: "artifacts.flow.stageReported", word: "검토 대기", shape: "half", signal: "wait", at: "review" },
  stale: { key: "artifacts.flow.stageStale", word: "다시 검토 대기", shape: "half", signal: "wait", at: "review" },
  verified: { key: "artifacts.flow.stageVerified", word: "미반영", shape: "dot", signal: "wait", at: "land" },
  merged: { key: "artifacts.flow.stageMerged", word: "반영됨", shape: "dot", signal: "done", at: "ship" },
  deployed: { key: "artifacts.flow.stageDeployed", word: "발행됨", shape: "check", signal: "done", at: null },
  failed: ARTIFACT_FLOW_HALTED,
  unreviewable: { key: "artifacts.flow.stageUnreviewable", word: "끝남 · 검토 기록 없음", shape: "check", signal: "quiet", at: null, skip: ["review", "land", "ship"] },
  waived: { key: "artifacts.flow.stageWaived", word: "끝남 · 검토 생략(사람)", shape: "check", signal: "quiet", at: null, skip: ["review", "land", "ship"] },
  nothing_to_land: { key: "artifacts.flow.stageNothingToLand", word: "끝남 · 착지할 것 없음", shape: "check", signal: "done", at: null, skip: ["land", "ship"] },
  unknown: { key: "artifacts.flow.stageUnknown", word: "확인 안 됨", shape: "wait", signal: "quiet", at: null, unread: true },
  closed: { key: "artifacts.flow.stageClosed", word: "닫힘", shape: "skip", signal: "quiet", at: null, skip: ["try", "handin", "review", "land", "ship"] },
});
/* 「지금」의 묶음이 담는 단계 — 그 머리의 메모가 이 단계들의 말이다. */
const ARTIFACT_FLOW_NOW_STAGES = Object.freeze(["dispatched", "reported", "verified"]);
/* 발행된 과업의 말 — 실린 판의 이름이 붙는다(「발행됨 v1.1.83」). */
const ARTIFACT_FLOW_SHIPPED_IN = Object.freeze({ key: "artifacts.flow.stageShippedIn", word: "발행됨 {{version}}" });

/* 걸음 하나의 결 — 펼친 목록의 표식과 띠의 칸이 같은 낱말을 읽는다. */
const ARTIFACT_FLOW_TONES = Object.freeze({
  done: { shape: "check", signal: "done" },
  now: { shape: "ring", signal: null },
  stop: { shape: "cross", signal: "halt" },
  wait: { shape: "wait", signal: "quiet" },
  skip: { shape: "skip", signal: "quiet" },
});

/* 표식의 모양 — 14×14 칸 하나에 그리는 것. 바탕 원(`ground`)이 시간의 선을 끊고, 선(`line`)과
 * 칠(`fill`)과 칼금(`cut`)은 CSS가 신호의 토큰으로 칠한다. */
const ARTIFACT_FLOW_MARKS = Object.freeze({
  ring: [["circle", { cx: 7, cy: 7, r: 4.75, part: "line", "stroke-dasharray": "21 9", transform: "rotate(-90 7 7)" }]],
  half: [["circle", { cx: 7, cy: 7, r: 4.75, part: "line" }], ["path", { d: "M7 2.25a4.75 4.75 0 0 1 0 9.5z", part: "fill" }]],
  dot: [["circle", { cx: 7, cy: 7, r: 5, part: "fill" }]],
  check: [["circle", { cx: 7, cy: 7, r: 6, part: "fill" }], ["path", { d: "m4.4 7.2 1.8 1.8 3.5-3.7", part: "cut" }]],
  cross: [["circle", { cx: 7, cy: 7, r: 6, part: "fill" }], ["path", { d: "m5 5 4 4M9 5 5 9", part: "cut" }]],
  wait: [["circle", { cx: 7, cy: 7, r: 4.25, part: "line" }]],
  skip: [["path", { d: "M4.5 7h5", part: "line" }]],
  note: [["circle", { cx: 7, cy: 7, r: 3.25, part: "line" }]],
});
/* 여섯 칸 띠의 한 칸 — 너비 · 틈 · 높이(뷰박스 단위). 칸의 색은 CSS가 결(`is-done`…)로 고른다. */
const ARTIFACT_FLOW_TRACK = Object.freeze({ cell: 8, gap: 2, height: 4 });

/* 묶음 셋, 읽는 차례대로. 수는 묶음의 머리에만 선다. */
const ARTIFACT_FLOW_GROUPS = Object.freeze([
  { group: "now", key: "artifacts.flow.groupNow", word: "지금" },
  { group: "next", key: "artifacts.flow.groupNext", word: "다음 발행 전" },
  { group: "shipped", key: "artifacts.flow.groupShipped", word: "발행된 것" },
]);

/* 줄의 칩 — 과업이 남긴 것의 종류마다 하나, 이 차례로. `has`는 런타임이 센 수(`chips`)를 읽고,
 * 낱말과 그림은 카탈로그의 종류 표를 빌린다(같은 종류에 두 이름을 두지 않는다). */
const ARTIFACT_FLOW_CHIPS = Object.freeze([
  { chip: "report", glyph: ARTIFACT_KIND_GLYPH.report, words: ARTIFACT_KIND_WORDS.report, has: (chips) => Boolean(chips.report) },
  { chip: "evidence", glyph: "terminal", words: ARTIFACT_KIND_WORDS.evidence, has: (chips) => (chips.parts?.logs ?? 0) + (chips.parts?.other ?? 0) > 0 },
  { chip: "pair", glyph: "columns", words: { key: "artifacts.flow.chipPair", word: "전후 사진" }, has: (chips) => (chips.pairs ?? 0) > 0 },
  { chip: "picture", glyph: ARTIFACT_KIND_GLYPH.screenshot, words: { key: "artifacts.flow.chipPicture", word: "사진" }, has: (chips) => (chips.singles ?? 0) > 0 },
  { chip: "page", glyph: ARTIFACT_KIND_GLYPH.page, words: ARTIFACT_KIND_WORDS.page, has: (chips) => Boolean(chips.page) },
  { chip: "session", glyph: "mouse-pointer-click", words: { key: "artifacts.flow.chipSession", word: "Computer Use 기록" }, has: (chips) => (chips.sessions ?? 0) > 0 },
]);

/* 몇 번째 시도에 냈는지 — 셋까지는 말로 하고(첫 화면의 수는 묶음의 머리에만 선다), 그 뒤는 수로. */
const ARTIFACT_FLOW_TRIES = Object.freeze([
  { key: "artifacts.flow.firstTry", word: "한 번에" },
  { key: "artifacts.flow.secondTry", word: "두 번째 시도에" },
  { key: "artifacts.flow.thirdTry", word: "세 번째 시도에" },
]);

/* 빈 프로젝트가 말하는 다음 세 걸음 — 무엇이 어떤 차례로 쌓이는지. */
const ARTIFACT_FLOW_EMPTY_STEPS = Object.freeze([
  { key: "artifacts.flow.emptyStep1", word: "과업이 열리면 「지금」에 한 줄이 섭니다." },
  { key: "artifacts.flow.emptyStep2", word: "워커가 넘기면 한 일이 문장으로, 보고서와 증거가 칩으로 붙습니다." },
  { key: "artifacts.flow.emptyStep3", word: "검토 · 착지 · 발행이 지나면 그 발행의 묶음으로 내려갑니다." },
]);

/* 펼친 줄의 세 줄 — 보고서의 「세 줄」 절이 있으면 그것이다. */
const ARTIFACT_FLOW_THREE = Object.freeze([
  { part: "what", key: "artifacts.flow.what", word: "무엇" },
  { part: "why", key: "artifacts.flow.why", word: "왜" },
  { part: "cost", key: "artifacts.flow.cost", word: "대가" },
]);

/* 런타임이 마지막으로 답한 흐름, 그 답의 글(같은 답이면 그리지 않는다), 그 답을 물은 거르개. */
let artifactFlowAnswer = null;
let artifactFlowSaid = "";
let artifactFlowAskedFor = "";
let artifactFlowAsking = false;
let artifactFlowAgain = false;
/* 사람이 펼친 것 — 줄(과업 id), 발행(판 이름), 「기록」(묶음 이름), 남은 일의 무더기(`SETTLE_PILES`의
 * 종류). 판이 다시 그려져도 남는다. */
const artifactFlowOpen = new Set();
const artifactFlowReleasesOpen = new Set();
const artifactFlowRecordsOpen = new Set();
const artifactFlowPilesOpen = new Set();
/* 읽는 칸에 선 것 — 누른 칩의 줄과 종류(`{ task, chip }`, 「기록」은 `{ record }`). */
let artifactFlowReading = null;
/* 하네스가 세는 수 — 줄 노드가 몇 번 만들어졌는가. */
let artifactFlowRowCreations = 0;
/* 판마다의 번호 — 묶음 머리의 id가 판 둘(나란히 연 탭)에서 겹치지 않게. */
let artifactFlowListSerial = 0;
/* 요일의 이름(「화요일」) — 로케일이 바뀔 때만 다시 짓는다. */
let artifactFlowWeekdayCode = null;
let artifactFlowWeekday = null;

/* ---- 묻기 ---------------------------------------------------------------- */

/* 흐름이 서는 프로젝트: 사람이 머리의 고르개로 고른 것, 아니면 지금 보는 프로젝트. 「전체」는
 * 갤러리의 것이라, 흐름은 그때 지금 보는 프로젝트로 선다. */
function artifactFlowProject() {
  const picked = artifactProjects.find((row) => row.value === artifactFilter.project);
  if (picked) return picked;
  const here = projects.find((one) => one.path === activeProjectPath) ?? projects[0] ?? null;
  if (!here) return null;
  return {
    value: here.path,
    label: here.name ?? basename(here.path),
    roots: [...new Set([here.path, ...here.worktrees.map((worktree) => worktree.path)])],
  };
}

/* 런타임에게 묻는 것: 프로젝트의 폴더와, 그 프로젝트의 행이 놓일 수 있는 폴더들. */
function artifactFlowAsk() {
  const project = artifactFlowProject();
  return project ? { root: project.value, roots: project.roots } : null;
}

/* 흐름을 묻는다. 이 명령을 모르는 런타임(옛 답)은 흐름이 없다고 읽는다. */
async function askArtifactFlow() {
  const ask = artifactFlowAsk();
  if (!ask) return { ask, answer: null };
  try {
    return { ask, answer: await invoke("artifact_flow", { ask }) };
  } catch {
    return { ask, answer: null };
  }
}

/* 답을 적는다 — 같은 거르개의 같은 답이면 `false`(그릴 것이 없다). 사라진 줄의 펼침과 읽기는 닫는다. */
function noteArtifactFlow({ ask, answer }) {
  const askedFor = JSON.stringify(ask);
  const said = JSON.stringify(answer);
  if (askedFor === artifactFlowAskedFor && said === artifactFlowSaid) return false;
  artifactFlowAskedFor = askedFor;
  artifactFlowSaid = said;
  artifactFlowAnswer = answer && typeof answer === "object" ? answer : null;
  const shown = new Set(artifactFlowLines().map((line) => line.task));
  for (const task of [...artifactFlowOpen]) if (!shown.has(task)) artifactFlowOpen.delete(task);
  if (artifactFlowReading?.task && !shown.has(artifactFlowReading.task)) closeArtifactFlowReading();
  return true;
}

/* 흐름을 다시 묻고, 움직였으면 서 있는 판을 그린다. 묻는 사이 또 청이 오면 답이 온 뒤 한 번 더. */
async function refreshArtifactFlow(view) {
  if (artifactFlowAsking) {
    artifactFlowAgain = true;
    return;
  }
  artifactFlowAsking = true;
  let moved = false;
  try {
    moved = noteArtifactFlow(await askArtifactFlow());
  } finally {
    artifactFlowAsking = false;
  }
  if (moved && view && !view.hidden) paintArtifactFlowAll(view);
  if (artifactFlowAgain) {
    artifactFlowAgain = false;
    void refreshArtifactFlow(view);
  }
}

/* ---- 읽기 ---------------------------------------------------------------- */

/* 답의 줄 전부 — 지금 · 다음 발행 전 · 발행마다. */
function artifactFlowLines() {
  const flow = artifactFlowAnswer;
  if (!flow) return [];
  return [
    ...(flow.now?.tasks ?? []),
    ...(flow.next?.tasks ?? []),
    ...(flow.releases ?? []).flatMap((release) => release.tasks ?? []),
  ];
}

function artifactFlowLine(task) {
  return artifactFlowLines().find((line) => line.task === task) ?? null;
}

/* 흐름에 설 것이 하나도 없는가 — 줄도 「기록」도. 답이 없는 것도 빈 것이다. */
function artifactFlowIsEmpty() {
  const flow = artifactFlowAnswer;
  if (!flow) return true;
  const groups = [flow.now, flow.next, ...(flow.releases ?? []), flow.left, flow.unreviewed];
  return groups.every((group) => (group?.tasks?.length ?? 0) === 0 && (group?.records?.length ?? 0) === 0);
}

/* 거르개가 남기는 줄인가: 검색어의 낱말마다 줄의 말 · 제목 · 과업 번호 · 수 어딘가에 있고, 고른
 * 에이전트가 그 과업의 시도에 있다. */
function artifactFlowAdmits(line) {
  const agent = artifactFilter.agent;
  if (agent && !(line.attempts ?? []).some((attempt) => attempt.agent === agent)) return false;
  const needle = artifactFilter.query.trim().toLowerCase();
  if (needle === "") return true;
  const hay = [line.task, line.said, line.title, line.gain ?? ""].join("\n").toLowerCase();
  return needle.split(/\s+/).filter(Boolean).every((word) => hay.includes(word));
}

function artifactFlowStage(line) {
  return ARTIFACT_FLOW_STAGES[line.stage] ?? ARTIFACT_FLOW_STAGES.unknown;
}

/* 상태의 말: 발행된 과업은 실린 판의 이름과 함께. */
function artifactFlowStageWords(line) {
  if (line.stage === "deployed" && line.ship?.version) {
    return t(ARTIFACT_FLOW_SHIPPED_IN.key, ARTIFACT_FLOW_SHIPPED_IN.word, { version: line.ship.version });
  }
  const stage = artifactFlowStage(line);
  return t(stage.key, stage.word);
}

/* 여섯 걸음 각각의 결 — 단계가 서 있는 걸음의 앞은 끝났고, 그 걸음은 지금(멈췄으면 멈춤), 뒤는 남았다. */
function artifactFlowTrack(line) {
  const stage = artifactFlowStage(line);
  const at = ARTIFACT_FLOW_STEPS.findIndex((row) => row.step === stage.at);
  return ARTIFACT_FLOW_STEPS.map((row, index) => {
    if (stage.skip?.includes(row.step)) return "skip";
    if (at < 0) return stage.unread && index > 0 ? "wait" : "done";
    if (index < at) return "done";
    if (index === at) return stage.stop ? "stop" : "now";
    return "wait";
  });
}

/* 줄의 둘째 줄의 「누가 · 몇 번째에」: 도는 과업은 하는 중, 끝난 과업은 몇 번째 시도에 냈는지. */
function artifactFlowWho(line) {
  const attempts = line.attempts ?? [];
  const last = attempts.at(-1);
  if (!last) return "";
  const who = agentName(last.agent ?? "") || last.agent || "";
  if (line.stage === "dispatched") return t("artifacts.flow.working", "{{agent}} · 하는 중", { agent: who });
  const said = ARTIFACT_FLOW_TRIES[attempts.length - 1];
  const tries = said
    ? t(said.key, said.word)
    : t("artifacts.flow.nthTry", "{{n}}번째 시도에", { n: artifactCountWord(attempts.length) });
  return who ? `${who} · ${tries}` : tries;
}

/* 오늘부터 이레 전까지 날마다의 0시 — 날이 바뀔 때만 다시 짓는다(발행 줄마다 날짜 열네 개를 짓지 않게). */
let artifactFlowDayStarts = null;

function artifactFlowDays() {
  const today = artifactDayStart(0);
  if (artifactFlowDayStarts?.[0] !== today) {
    artifactFlowDayStarts = [...Array(ARTIFACT_WEEK_DAYS).keys()].map((days) => artifactDayStart(days));
  }
  return artifactFlowDayStarts;
}

/* 날의 말 — 오늘 · 어제 · 이번 주의 요일(숫자 없이), 그보다 앞은 달과 날. */
function artifactFlowDayWords(ms) {
  const back = artifactFlowDays().findIndex((start) => ms >= start);
  if (back === 0) return t(ARTIFACT_PERIODS[0].key, ARTIFACT_PERIODS[0].word);
  if (back === 1) return t("artifacts.flow.yesterday", "어제");
  if (back !== -1) {
    const code = artifactLocaleCode();
    if (artifactFlowWeekday === null || artifactFlowWeekdayCode !== code) {
      artifactFlowWeekdayCode = code;
      artifactFlowWeekday = new Intl.DateTimeFormat(code, { weekday: "long" });
    }
    return artifactFlowWeekday.format(new Date(ms));
  }
  return artifactFormatters().day.format(new Date(ms));
}

/* 한 행이 이 줄의 몇 번째 시도의 것인가 — 행의 워커가 그 시도의 워커다. 없으면 0. */
function artifactFlowAttemptOf(line, row) {
  const worker = row?.origin?.worker ?? "";
  return (line.attempts ?? []).findIndex((attempt) => attempt.worker === worker) + 1;
}

/* 읽는 칸 머리의 「어느 걸음의 무엇」: 「보고서 · 시도 2의 최종 보고」 — 그 과업의 상태(「반영됨」)는 곁의
 * 표식이 말한다. 「기록」은 기록이다. */
function artifactFlowWhere(row) {
  const kind = artifactKindWord(row);
  const reading = artifactFlowReading;
  if (!reading?.task) return [kind, t("artifacts.flow.record", "기록")].join(" · ");
  const line = artifactFlowLine(reading.task);
  if (!line) return kind;
  const parts = [kind];
  if (row.kind === "report" && row.subtype === "review") {
    parts.push(t("artifacts.flow.whereReview", "검토의 {{what}}", { what: t("artifacts.flow.reviewReport", "검증 보고") }));
  } else {
    const attempt = artifactFlowAttemptOf(line, row);
    const what = row.kind !== "report"
      ? kind
      : row.id === line.chips?.report
        ? t("artifacts.flow.finalReport", "최종 보고")
        : t("artifacts.flow.earlierReport", "진행 보고");
    if (attempt > 0) parts.push(t("artifacts.flow.whereTry", "시도 {{n}}의 {{what}}", { n: artifactCountWord(attempt), what }));
  }
  return parts.join(" · ");
}

/* ---- 판 짓기 -------------------------------------------------------------- */

const ARTIFACT_FLOW_SVG = "http://www.w3.org/2000/svg";

function artifactFlowSvg(tag, attrs) {
  const node = document.createElementNS(ARTIFACT_FLOW_SVG, tag);
  for (const [name, value] of Object.entries(attrs)) {
    if (name === "part") node.setAttribute("class", `artifact-flow-${value}`);
    else node.setAttribute(name, String(value));
  }
  return node;
}

/* 한 번 지은 원형 — 줄 · 발행 줄 · 표식 · 띠 · 칩 · 아이콘. 판을 처음부터 그릴 때는 이것을 복제한다:
 * 노드를 하나씩 짓고 속성을 하나씩 쓰고 아이콘의 HTML을 줄마다 읽는 값이 복제 한 번으로 준다. 원형에는
 * 로케일의 글이 없다 — 말은 입힐 때 쓴다. */
const artifactFlowPrototypes = new Map();

function artifactFlowClone(key, build) {
  let prototype = artifactFlowPrototypes.get(key);
  if (!prototype) {
    prototype = build();
    artifactFlowPrototypes.set(key, prototype);
  }
  return prototype.cloneNode(true);
}

/* 스프라이트의 그림 하나 — 그 HTML은 한 번만 읽는다. 창의 문서에서 읽어, 복제가 붙을 때 문서를 옮기지
 * 않는다(`<template>`의 내용은 다른 문서의 것이다). */
function artifactFlowIcon(name) {
  return artifactFlowClone(`icon:${name}`, () => {
    const holder = document.createElement("div");
    holder.innerHTML = icon(name);
    return holder.firstElementChild;
  });
}

/* 표식의 그림 — 바탕 원과 모양. */
function artifactFlowMarkSvg(shape) {
  const known = ARTIFACT_FLOW_MARKS[shape] ? shape : "note";
  return artifactFlowClone(`mark:${known}`, () => {
    const svg = artifactFlowSvg("svg", { viewBox: "0 0 14 14", "aria-hidden": "true" });
    svg.appendChild(artifactFlowSvg("circle", { cx: 7, cy: 7, r: 7, part: "ground" }));
    for (const [tag, attrs] of ARTIFACT_FLOW_MARKS[known]) svg.appendChild(artifactFlowSvg(tag, attrs));
    return svg;
  });
}

/* 표식 하나 — 모양이 바뀐 때에만 다시 그린다. */
function dressArtifactFlowMark(mark, shape, signal) {
  writeAttribute(mark, "data-signal", signal ?? "quiet");
  if (mark._shape === shape) return;
  mark._shape = shape;
  mark.replaceChildren(artifactFlowMarkSvg(shape));
}

function artifactFlowMark(className, shape, signal) {
  const mark = document.createElement("span");
  mark.className = className;
  mark.setAttribute("aria-hidden", "true");
  dressArtifactFlowMark(mark, shape, signal);
  return mark;
}

/* 여섯 칸의 띠 — 그림 하나(칸은 그 안의 사각형)라 줄마다 요소는 하나다. 결이 바뀐 칸만 고친다. */
function artifactFlowTrackNode() {
  return artifactFlowClone("track", () => {
    const { cell, gap, height } = ARTIFACT_FLOW_TRACK;
    const width = ARTIFACT_FLOW_STEPS.length * cell + (ARTIFACT_FLOW_STEPS.length - 1) * gap;
    const svg = artifactFlowSvg("svg", { class: "artifact-flow-track", viewBox: `0 0 ${width} ${height}`, "aria-hidden": "true" });
    ARTIFACT_FLOW_STEPS.forEach((row, index) => {
      svg.appendChild(artifactFlowSvg("rect", { x: index * (cell + gap), y: 0, width: cell, height, rx: height / 2, "data-step": row.step }));
    });
    return svg;
  });
}

function dressArtifactFlowTrack(svg, tones) {
  [...svg.children].forEach((rect, index) => {
    const value = `is-${tones[index] ?? "wait"}`;
    if (rect.getAttribute("class") !== value) rect.setAttribute("class", value);
  });
}

/* 칩 하나 — 누르면 곁의 읽는 칸에 선다. 단추의 말은 종류이고, 무엇인지는 풍선(`data-tip`)이 말한다. */
function artifactFlowChip(row, label, tip) {
  const chip = artifactFlowClone(`chip:${row.glyph}`, () => {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "artifact-flow-chip";
    one.append(artifactFlowIcon(row.glyph));
    return one;
  });
  chip.append(label);
  chip.dataset.flowChip = row.chip;
  if (tip) chip.dataset.tip = tip;
  return chip;
}

/* 흐름의 상자 — 판마다 한 번 짓는다: 등록된 저장소 밖 폴더의 한 줄, 묶음이 설 목록과 빈 프로젝트의
 * 안내. 묶음과 줄은 그릴 때 열쇠로 선다(`paintArtifactFlowList`). */
function buildArtifactFlow() {
  const flow = document.createElement("section");
  flow.className = "artifacts-flow";
  flow.hidden = true;
  // 런타임은 창이 등록한 저장소의 폴더에만 git을 묻는다(t-69070 P2) — 묻지 않은 폴더는 그 사실을 한 줄로.
  const outside = document.createElement("p");
  outside.className = "artifact-flow-outside";
  outside.hidden = true;
  outside.dataset.i18n = "artifacts.flow.outside";
  outside.textContent = t("artifacts.flow.outside", "등록된 저장소 밖 폴더 — git 기록 없음");
  // git 답이 시간 안에 오지 않았다: 들고 있던 지난 답 그대로 두고 다시 읽는 중이라고만 한 줄로(t-69107 ③).
  const stale = document.createElement("p");
  stale.className = "artifact-flow-stale";
  stale.hidden = true;
  say(stale, () => t("artifacts.flow.stale", "다시 읽는 중"));
  const list = document.createElement("div");
  list.className = "artifact-flow-list";
  artifactFlowListSerial += 1;
  list.dataset.flowSerial = String(artifactFlowListSerial);
  list.tabIndex = 0;
  // 화살표와 Enter는 이 목록의 것 — 터미널의 키 받침이 첫 키에 초점을 가져가지 않게.
  list.dataset.keyboardOwner = "true";
  list.dataset.i18nAria = "artifacts.flow.list";
  list.setAttribute("aria-label", t("artifacts.flow.list", "이 프로젝트의 흐름"));
  const empty = document.createElement("div");
  empty.className = "artifact-flow-empty";
  empty.hidden = true;
  const mark = document.createElement("span");
  mark.className = "artifacts-empty-mark";
  mark.append(artifactFlowIcon("workflow"));
  const word = document.createElement("p");
  word.className = "artifact-flow-empty-word";
  const hint = document.createElement("p");
  hint.className = "artifact-flow-empty-hint";
  hint.dataset.i18n = "artifacts.flow.emptyHint";
  hint.textContent = t("artifacts.flow.emptyHint", "에이전트에게 일을 맡기면 이 프로젝트의 과업 · 보고서 · 증거가 여기에 차례로 쌓입니다.");
  const steps = document.createElement("ol");
  steps.className = "artifact-flow-empty-steps";
  for (const row of ARTIFACT_FLOW_EMPTY_STEPS) {
    const item = document.createElement("li");
    item.dataset.i18n = row.key;
    item.textContent = t(row.key, row.word);
    steps.appendChild(item);
  }
  const all = document.createElement("button");
  all.type = "button";
  all.className = "btn artifact-flow-empty-all";
  all.dataset.i18n = "artifacts.flow.emptyAll";
  all.textContent = t("artifacts.flow.emptyAll", "전체 목록 보기");
  empty.append(mark, word, hint, steps, all);
  flow.append(outside, stale, list, empty);
  return flow;
}

/* 줄 하나의 골격 — 표식 · 문장 · 상태(띠와 말) · 둘째 줄(칩 · 누가 · 번호 · 펼침) · 펼친 자리. 원형을
 * 복제하고, 표식의 그림과 말은 처음 입을 때 선다. */
function makeArtifactFlowRow() {
  artifactFlowRowCreations += 1;
  return artifactFlowClone("row", artifactFlowRowPrototype);
}

function artifactFlowRowPrototype() {
  const row = document.createElement("li");
  row.className = "artifact-flow-item";
  row.tabIndex = -1;
  const mark = document.createElement("span");
  mark.className = "artifact-flow-node";
  mark.setAttribute("aria-hidden", "true");
  const said = document.createElement("p");
  said.className = "artifact-flow-said";
  const words = document.createElement("span");
  words.className = "artifact-flow-words";
  const dash = document.createElement("span");
  dash.className = "artifact-flow-dash";
  dash.setAttribute("aria-hidden", "true");
  dash.textContent = " — ";
  const gain = document.createElement("span");
  gain.className = "artifact-flow-gain";
  said.append(words, dash, gain);
  const state = document.createElement("span");
  state.className = "artifact-flow-state";
  const stateWord = document.createElement("span");
  stateWord.className = "artifact-flow-state-word";
  state.append(artifactFlowTrackNode(), stateWord);
  const meta = document.createElement("div");
  meta.className = "artifact-flow-meta";
  const chips = document.createElement("span");
  chips.className = "artifact-flow-chips";
  const who = document.createElement("span");
  who.className = "artifact-flow-who";
  const id = document.createElement("span");
  id.className = "artifact-flow-id";
  const more = document.createElement("button");
  more.type = "button";
  more.className = "artifact-flow-more";
  more.append(artifactFlowIcon("chevron"));
  meta.append(chips, who, id, more);
  const detail = document.createElement("div");
  detail.className = "artifact-flow-detail";
  detail.hidden = true;
  row.append(mark, said, state, meta, detail);
  return row;
}

/* 여섯 걸음의 이름을 이은 말 — 상태 풍선의 앞머리. 로케일마다 한 번 잇는다. */
let artifactFlowStepsSaid = null;

function artifactFlowStepsWords() {
  const code = artifactLocaleCode();
  if (artifactFlowStepsSaid?.code !== code) {
    artifactFlowStepsSaid = { code, words: ARTIFACT_FLOW_STEPS.map((step) => t(step.key, step.word)).join(" → ") };
  }
  return artifactFlowStepsSaid.words;
}

/* 줄의 옷 — 줄의 말이나 펼침 · 누른 칩이 달라진 때에만 다시 입는다. 같은 답의 같은 줄은 서명도
 * 짓지 않고 지나간다; 새 답의 줄은 서명(그 말 전부)이 같으면 아무것도 쓰지 않는다. */
function dressArtifactFlowRow(row, line) {
  const open = artifactFlowOpen.has(line.task);
  const reading = artifactFlowReading?.task === line.task ? artifactFlowReading.chip ?? "" : "";
  const shown = [open, reading, artifactLocaleCode()].join("\n");
  if (row._line === line && row._shown === shown) return;
  row._line = line;
  row._shown = shown;
  const signature = `${JSON.stringify(line)}\n${shown}`;
  if (row._signature === signature) return;
  row._signature = signature;
  row.dataset.task = line.task;
  row.dataset.stage = line.stage;
  writeAttribute(row, "aria-expanded", open ? "true" : "false");
  row.classList.toggle("is-reading", reading !== "");
  const stage = artifactFlowStage(line);
  dressArtifactFlowMark(row.querySelector(".artifact-flow-node"), stage.shape, stage.signal);
  writeTextContent(row.querySelector(".artifact-flow-words"), line.said);
  const said = row.querySelector(".artifact-flow-said");
  said.dataset.tip = line.title && line.title !== line.said ? line.title : "";
  writeHidden(row.querySelector(".artifact-flow-dash"), !line.gain);
  writeTextContent(row.querySelector(".artifact-flow-gain"), line.gain ?? "");
  const state = row.querySelector(".artifact-flow-state");
  writeAttribute(state, "data-signal", stage.signal);
  dressArtifactFlowTrack(state.querySelector(".artifact-flow-track"), artifactFlowTrack(line));
  const words = artifactFlowStageWords(line);
  writeTextContent(state.querySelector(".artifact-flow-state-word"), words);
  state.dataset.tip = `${artifactFlowStepsWords()} — ${words}`;
  const chips = ARTIFACT_FLOW_CHIPS.filter((one) => one.has(line.chips ?? {})).map((one) => {
    const chip = artifactFlowChip(one, t(one.words.key, one.words.word), "");
    writeAttribute(chip, "aria-pressed", reading === one.chip ? "true" : "false");
    return chip;
  });
  const box = row.querySelector(".artifact-flow-chips");
  if (chips.length === 0) {
    const none = document.createElement("span");
    none.className = "artifact-flow-none";
    none.textContent = t("artifacts.flow.noChips", "아직 낸 아티팩트 없음");
    box.replaceChildren(none);
  } else {
    box.replaceChildren(...chips);
  }
  writeTextContent(row.querySelector(".artifact-flow-who"), artifactFlowWho(line));
  writeTextContent(row.querySelector(".artifact-flow-id"), line.task);
  const more = row.querySelector(".artifact-flow-more");
  const moreWords = open ? t("artifacts.flow.fold", "걸음 접기") : t("artifacts.flow.unfold", "걸음 펼치기");
  writeAttribute(more, "aria-label", moreWords);
  more.dataset.tip = moreWords;
  writeAttribute(more, "aria-expanded", open ? "true" : "false");
  const detail = row.querySelector(".artifact-flow-detail");
  writeHidden(detail, !open);
  if (open) paintArtifactFlowDetail(detail, line);
  else detail.replaceChildren();
}

/* 펼친 줄: 무엇 · 왜 · 대가(보고서에 그 절이 없으면 넘긴 말), 그리고 걸음마다 한 줄. */
function paintArtifactFlowDetail(detail, line) {
  const parts = [];
  if (line.three) {
    const three = document.createElement("dl");
    three.className = "artifact-flow-three";
    for (const row of ARTIFACT_FLOW_THREE) {
      three.append(artifactLine("dt", "", t(row.key, row.word)), artifactLine("dd", "", line.three[row.part] ?? ""));
    }
    parts.push(three);
  } else if (line.summary) {
    const told = document.createElement("dl");
    told.className = "artifact-flow-three";
    told.append(artifactLine("dt", "", t("artifacts.flow.summary", "넘긴 말")), artifactLine("dd", "", line.summary));
    parts.push(told);
  }
  const steps = document.createElement("ol");
  steps.className = "artifact-flow-steps";
  steps.dataset.i18nAria = "artifacts.flow.steps";
  steps.setAttribute("aria-label", t("artifacts.flow.steps", "이 과업이 지난 걸음"));
  steps.append(...artifactFlowStepRows(line));
  parts.push(steps);
  detail.replaceChildren(...parts);
}

/* 걸음 한 줄 — 표식 · 이름 · 언제 · 한 줄 · 발(칩과 번호). */
function artifactFlowStepRow(step, tone, { name, when, said, foot = [], tip = "" }) {
  const row = document.createElement("li");
  row.className = "artifact-flow-step";
  row.dataset.step = step;
  row.dataset.tone = tone;
  const look = ARTIFACT_FLOW_TONES[tone] ?? ARTIFACT_FLOW_TONES.wait;
  row.append(
    artifactFlowMark("artifact-flow-mark", look.shape, look.signal),
    artifactLine("span", "artifact-flow-step-name", name),
    artifactLine("span", "artifact-flow-step-when", when),
    artifactLine("p", "artifact-flow-step-said", said),
  );
  if (tip) row.querySelector(".artifact-flow-step-when").dataset.tip = tip;
  const bottom = document.createElement("div");
  bottom.className = "artifact-flow-step-foot";
  bottom.append(...foot);
  row.appendChild(bottom);
  return row;
}

/* 걸음의 시각 — 있으면 시계의 말, 없으면 「아직」, 건너뛴 걸음은 「없음」. */
function artifactFlowWhen(ms, tone) {
  if (ms) return artifactClockWords(ms);
  return tone === "skip" ? t("artifacts.flow.none", "없음") : t("artifacts.flow.notYet", "아직");
}

/* 그 걸음의 번호 · 해시 — 발의 흐린 글, 고정폭. */
function artifactFlowIds(...ids) {
  const said = ids.filter(Boolean).join(" · ");
  return said ? [artifactLine("span", "artifact-flow-step-ids", said)] : [];
}

/* 한 시도의 행들 — 그 과업의 묶음(`artifact_bundle`)에서, 행의 워커가 그 시도의 워커인 것. 사라진
 * 파일은 줄을 긋고 까닭을 단다. 묶음을 아직 모르면 칩 없이 선다(펼칠 때 묻는다). */
function artifactFlowAttemptChips(line, attempt) {
  const bundle = artifactBundles.get(line.task);
  if (!bundle) return [];
  const missing = new Set(bundle.missing ?? []);
  return (bundle.rows ?? [])
    .filter((row) => row.origin?.worker === attempt.worker && !(row.kind === "report" && row.subtype === "review"))
    .map((row) => artifactFlowRowChip(row, missing.has(row.id)));
}

/* 검토의 행들 — 그 과업의 검증 보고. */
function artifactFlowReviewChips(line) {
  const bundle = artifactBundles.get(line.task);
  if (!bundle) return [];
  const missing = new Set(bundle.missing ?? []);
  return (bundle.rows ?? [])
    .filter((row) => row.kind === "report" && row.subtype === "review")
    .map((row) => artifactFlowRowChip(row, missing.has(row.id)));
}

/* 행 하나의 칩 — 종류의 말, 풍선은 제목. 사라진 행은 누를 수 없고 줄이 그어진다. */
function artifactFlowRowChip(row, gone) {
  const kind = row.kind === "screenshot" ? "picture" : row.kind === "evidence" ? "evidence" : row.kind === "report" ? "report" : "page";
  const look = ARTIFACT_FLOW_CHIPS.find((one) => one.chip === kind);
  const chip = artifactFlowChip(look, artifactKindWord(row), row.title);
  chip.dataset.flowRow = row.id;
  if (gone) {
    chip.classList.add("is-lost");
    chip.disabled = true;
    chip.dataset.tip = `${row.title} — ${t("artifacts.flow.lostWhy", "작업 폴더를 정리할 때 함께 삭제됐습니다")}`;
  }
  return chip;
}

/* 걸음 여섯의 줄들, 시간순 — 과업 · 시도마다 · 넘김 · 검토 · 착지 · 발행. */
function artifactFlowStepRows(line) {
  const tones = artifactFlowTrack(line);
  const toneOf = (step) => tones[ARTIFACT_FLOW_STEPS.findIndex((row) => row.step === step)] ?? "wait";
  const nameOf = (step) => {
    const row = ARTIFACT_FLOW_STEPS.find((one) => one.step === step);
    return t(row.key, row.word);
  };
  const rows = [];
  rows.push(artifactFlowStepRow("task", toneOf("task"), {
    name: nameOf("task"),
    when: artifactFlowWhen(line.opened_ms, toneOf("task")),
    said: line.title,
    foot: artifactFlowIds(line.task),
  }));
  const attempts = line.attempts ?? [];
  attempts.forEach((attempt, index) => {
    const last = index === attempts.length - 1;
    const tone = !last ? (attempt.succeeded === false ? "stop" : "done") : toneOf("try");
    const span = attempt.ended_ms
      ? t("artifacts.flow.span", "{{from}} ~ {{to}}", { from: artifactClockWords(attempt.started_ms), to: artifactClockWords(attempt.ended_ms) })
      : t("artifacts.flow.since", "{{from}} ~", { from: artifactClockWords(attempt.started_ms) });
    const said = attempt.said
      ?? (attempt.ended_ms === null || attempt.ended_ms === undefined
        ? t("artifacts.flow.tryOpen", "하는 중입니다")
        : attempt.succeeded
          ? t("artifacts.flow.handedIn", "넘겼습니다")
          : t("artifacts.flow.tryEnded", "보고 없이 끝났습니다"));
    const who = [agentName(attempt.agent ?? "") || attempt.agent, attempt.model].filter(Boolean).join(" · ");
    rows.push(artifactFlowStepRow("try", tone, {
      name: t("artifacts.flow.stepTryN", "시도 {{n}}", { n: artifactCountWord(index + 1) }),
      when: span,
      said,
      foot: [...artifactFlowAttemptChips(line, attempt), ...(who ? [artifactLine("span", "artifact-flow-step-note", who)] : []),
        ...artifactFlowIds(attempt.dispatch, attempt.worker)],
    }));
  });
  if (attempts.length === 0) {
    rows.push(artifactFlowStepRow("try", toneOf("try"), {
      name: nameOf("try"), when: artifactFlowWhen(null, toneOf("try")), said: t("artifacts.flow.tryNone", "아직 시도가 없습니다"),
    }));
  }
  const handIn = line.hand_in;
  rows.push(artifactFlowStepRow("handin", toneOf("handin"), {
    name: nameOf("handin"),
    when: artifactFlowWhen(handIn?.at_ms, toneOf("handin")),
    said: handIn ? (line.gain ?? t("artifacts.flow.handedIn", "넘겼습니다")) : t("artifacts.flow.handInNone", "아직 낸 것이 없습니다"),
    foot: artifactFlowIds(handIn?.source ? handIn.source.slice(0, ARTIFACT_HASH_SHORT) : ""),
    tip: handIn?.source ?? "",
  }));
  const review = line.review;
  const lost = artifactBundles.get(line.task)?.missing?.length ?? 0;
  rows.push(artifactFlowStepRow("review", toneOf("review"), {
    name: nameOf("review"),
    when: artifactFlowWhen(review?.at_ms, toneOf("review")),
    said: review
      ? t("artifacts.flow.reviewed", "코디가 검토해 통과시켰습니다")
      : toneOf("review") === "skip" ? t("artifacts.flow.reviewSkip", "검토할 것이 없습니다") : t("artifacts.flow.reviewWaiting", "검토를 기다립니다"),
    foot: [...artifactFlowReviewChips(line),
      ...(lost > 0 ? [artifactLine("span", "artifact-flow-step-note", t("artifacts.flow.lost", "증거 {{n}}개는 작업 폴더를 정리할 때 함께 삭제됐습니다 — 보고서는 남아 있습니다", { n: artifactCountWord(lost) }))] : [])],
  }));
  const land = line.land;
  const base = artifactFlowAnswer?.base ?? "";
  rows.push(artifactFlowStepRow("land", toneOf("land"), {
    name: nameOf("land"),
    when: artifactFlowWhen(land?.at_ms, toneOf("land")),
    said: land
      ? land.line ?? (base ? t("artifacts.flow.landedOn", "{{base}}에 넣었습니다", { base }) : t("artifacts.flow.landed", "넣었습니다"))
      : toneOf("land") === "skip" ? t("artifacts.flow.landSkip", "착지할 것이 없습니다") : t("artifacts.flow.landWaiting", "아직 넣지 않았습니다"),
    foot: artifactFlowIds(land?.commit ? land.commit.slice(0, ARTIFACT_HASH_SHORT) : ""),
    tip: land ? (land.told === "git" ? t("artifacts.flow.toldGit", "git의 병합 커밋이 말한 시각") : t("artifacts.flow.toldLedger", "원장의 기록 — 시각은 적혀 있지 않습니다")) : "",
  }));
  const ship = line.ship;
  rows.push(artifactFlowStepRow("ship", toneOf("ship"), {
    name: nameOf("ship"),
    when: artifactFlowWhen(ship?.at_ms, toneOf("ship")),
    said: ship
      ? t("artifacts.flow.shippedIn", "{{version}}에 실려 나갔습니다", { version: ship.version })
      : toneOf("ship") === "skip" ? t("artifacts.flow.shipSkip", "발행할 것이 없습니다") : t("artifacts.flow.shipWaiting", "다음 발행을 기다립니다"),
  }));
  return rows;
}

/* ---- 묶음 그리기 ---------------------------------------------------------- */

/* 열쇠와 서명으로 사는 노드 — 서명이 같으면 있던 노드 그대로, 다르면 새로 짓는다. 발행 줄 ·
 * 「기록」 · 지난 발행의 한 줄이 이것으로 산다: 같은 답을 다시 그리는 것은 아무것도 짓지 않는다. */
function artifactFlowKept(view, key, signature, build) {
  const pool = view._artifactFlowKept ?? (view._artifactFlowKept = new Map());
  const held = pool.get(key);
  if (held && held._said === signature) return held;
  const node = build();
  node._said = signature;
  pool.set(key, node);
  return node;
}

/* 묶음의 머리 — 이름 · 수 · 한 줄 메모. 열쇠로 산다. */
function artifactFlowGroupHead(list, key, word, count, note) {
  const heads = list._heads ?? (list._heads = new Map());
  let head = heads.get(key);
  if (!head) {
    head = document.createElement("h3");
    head.className = "artifact-flow-group-head";
    head.id = `artifact-flow-group-${key}-${list.dataset.flowSerial}`;
    const name = document.createElement("span");
    name.className = "artifact-flow-group-name";
    const sep = document.createElement("span");
    sep.className = "artifact-flow-group-sep";
    sep.setAttribute("aria-hidden", "true");
    sep.textContent = "·";
    const n = document.createElement("b");
    const memo = document.createElement("span");
    memo.className = "artifact-flow-group-note";
    head.append(name, sep, n, memo);
    heads.set(key, head);
  }
  const [name, sep, n, memo] = head.children;
  writeTextContent(name, word);
  writeHidden(sep, count === null);
  writeHidden(n, count === null);
  writeTextContent(n, count === null ? "" : artifactCountWord(count));
  writeTextContent(memo, note);
  return head;
}

/* 한 묶음의 줄들 — 열쇠(과업 id)로 산 노드를 꺼내 입히고 차례대로 세운다. */
function artifactFlowRows(view, lines) {
  const pool = view._artifactFlowRows ?? (view._artifactFlowRows = new Map());
  return lines.map((line) => {
    let row = pool.get(line.task);
    if (!row) {
      row = makeArtifactFlowRow();
      pool.set(line.task, row);
    }
    dressArtifactFlowRow(row, line);
    return row;
  });
}

/* 과업에 묶이지 않은 기록 — 접으면 한 줄(제목들과 종류의 칩), 펼치면 기록마다 한 줄. */
function artifactFlowRecordRows(view, key, records) {
  if (records.length === 0) return [];
  const open = artifactFlowRecordsOpen.has(key);
  const pressed = artifactFlowReading?.record ?? "";
  const signature = [records.map((row) => `${row.id}:${row.modified_ms}`).join(","), open, pressed, artifactLocaleCode()].join("\n");
  const head = artifactFlowKept(view, `records:${key}`, signature, () => {
    const item = document.createElement("li");
    item.className = "artifact-flow-item is-records";
    item.dataset.records = key;
    item.tabIndex = -1;
    item.setAttribute("aria-expanded", open ? "true" : "false");
    const said = artifactLine("p", "artifact-flow-said", t("artifacts.flow.records", "과업에 묶이지 않은 기록 — {{titles}}", {
      titles: records.map((row) => row.title).join(" · "),
    }));
    const meta = document.createElement("div");
    meta.className = "artifact-flow-meta";
    if (!open) meta.append(...records.map((row) => artifactFlowRecordChip(row)));
    const more = document.createElement("button");
    more.type = "button";
    more.className = "artifact-flow-more";
    more.append(artifactFlowIcon("chevron"));
    more.setAttribute("aria-expanded", open ? "true" : "false");
    const moreWords = open ? t("artifacts.flow.recordsFold", "기록 접기") : t("artifacts.flow.recordsUnfold", "기록 펼치기");
    more.setAttribute("aria-label", moreWords);
    more.dataset.tip = moreWords;
    meta.appendChild(more);
    item.append(artifactFlowMark("artifact-flow-node", "note", "quiet"), said, meta);
    return item;
  });
  if (!open) return [head];
  return [head, ...records.map((row) => artifactFlowKept(view, `record:${row.id}`, [row.modified_ms, pressed === row.id, artifactLocaleCode()].join("\n"), () => {
    const one = document.createElement("li");
    one.className = "artifact-flow-item is-record";
    one.dataset.record = row.id;
    one.tabIndex = -1;
    const meta = document.createElement("div");
    meta.className = "artifact-flow-meta";
    meta.append(artifactFlowRecordChip(row), artifactLine("span", "artifact-flow-who", [
      agentName(row.origin?.agent ?? "") || row.origin?.agent, artifactClockWords(row.modified_ms),
    ].filter(Boolean).join(" · ")));
    one.append(artifactFlowMark("artifact-flow-node", "note", "quiet"), artifactLine("p", "artifact-flow-said", row.title), meta);
    return one;
  }))];
}

function artifactFlowRecordChip(row) {
  const kind = row.kind === "report" ? "report" : row.kind === "screenshot" ? "picture" : row.kind === "evidence" ? "session" : "page";
  const look = ARTIFACT_FLOW_CHIPS.find((one) => one.chip === kind);
  const chip = artifactFlowChip(look, artifactKindWord(row), row.title);
  chip.dataset.flowRecord = row.id;
  chip.setAttribute("aria-pressed", artifactFlowReading?.record === row.id ? "true" : "false");
  return chip;
}

/* 발행 한 줄 — 판 이름 · 언제 · 든 과업의 이름 · 수, 누르면 그 발행의 줄이 펼쳐진다. 머리는 서명으로
 * 살고, 펼친 줄들은 그릴 때마다 열쇠의 노드로 다시 맞춘다. */
function artifactFlowReleaseRow(view, release, lines) {
  const open = artifactFlowReleasesOpen.has(release.version);
  const count = lines.length + (release.more ?? 0);
  const day = artifactFlowDayWords(release.at_ms);
  const digest = lines.map((line) => line.title || line.said).join(" · ");
  const signature = [release.version, release.at_ms, open, count, day, digest, artifactLocaleCode()].join("\n");
  const box = artifactFlowKept(view, `release:${release.version}`, signature, () => {
    const item = artifactFlowClone("release", artifactFlowReleasePrototype);
    item.dataset.release = release.version;
    item.setAttribute("aria-expanded", open ? "true" : "false");
    const head = item.firstElementChild;
    head.dataset.flowRelease = release.version;
    head.setAttribute("aria-expanded", open ? "true" : "false");
    // 풍선의 시각은 처음 가리킬 때 쓴다(`artifactFlowReleaseTip`) — 판을 그릴 때마다 발행 줄 수만큼 시각을
    // 서식하지 않게.
    head.dataset.flowReleaseAt = String(release.at_ms);
    const [, name, when, said, tally] = head.children;
    name.textContent = release.version;
    when.textContent = t("artifacts.flow.releasedOn", "{{day}} 발행", { day });
    said.textContent = digest;
    tally.textContent = t("artifacts.flow.releaseCount", "과업 {{n}}", { n: artifactCountWord(count) });
    if (open) {
      const rows = document.createElement("ul");
      rows.className = "artifact-flow-release-rows";
      item.appendChild(rows);
    }
    return item;
  });
  if (open) {
    reconcileElementOrder(box.querySelector(".artifact-flow-release-rows"), [
      ...artifactFlowRows(view, lines),
      ...artifactFlowRecordRows(view, `release:${release.version}`, release.records ?? []),
    ]);
  }
  return box;
}

/* 발행 줄의 원형 — 표식 · 판 이름 · 언제 · 든 과업의 이름 · 수 · 갈매기. */
function artifactFlowReleasePrototype() {
  const item = document.createElement("li");
  item.className = "artifact-flow-release";
  const head = document.createElement("button");
  head.type = "button";
  head.className = "artifact-flow-release-head";
  const mark = document.createElement("span");
  mark.className = "artifact-flow-node";
  mark.setAttribute("aria-hidden", "true");
  mark.dataset.signal = "done";
  mark.append(artifactFlowMarkSvg("check"));
  head.append(
    mark,
    artifactLine("span", "artifact-flow-release-name", ""),
    artifactLine("span", "artifact-flow-release-when", ""),
    artifactLine("span", "artifact-flow-release-digest", ""),
    artifactLine("span", "artifact-flow-release-count", ""),
    artifactFlowIcon("chevron"),
  );
  item.appendChild(head);
  return item;
}

/* ---- 남은 일의 무더기 (t-71220) ---------------------------------------------
 *
 * 「지금」 아래의 접힌 줄 둘 — 정리 필요 · 검토 없이 끝남. 줄과 수, 누를 이름은 런타임이 실은
 * 그대로이고(`left`·`unreviewed`, core `settle::left_behind`), 낱말과 누름은 상황판과 같은 한
 * 곳이다(`SETTLE_PILES`·`pressSettlePile`, shell-board-now.js). 펼치면 오래된 것부터 과업 · 런 ·
 * 나이가 한 줄씩 선다. 거르개가 걸린 동안은 맞는 줄만 서고 누름은 물러난다 — 누름은 무더기 전부의
 * 것이다. */

/* 서 있을 무더기와 그 줄들. 든 일이 없어도 방금 누른 답(되돌리기)이 서 있으면 선다. */
function artifactFlowPiles(flow, admitted, narrowed) {
  const now = Date.now();
  return SETTLE_PILES.map((pile) => {
    const held = flow?.[pile.flow];
    const lines = admitted(held);
    const count = narrowed ? lines.length : (held?.tasks?.length ?? 0) + (held?.more ?? 0);
    return { pile, held, lines, count, key: `flow:${pile.kind}` };
  }).filter(({ key, count, lines }) => (narrowed ? lines.length > 0 : settlePileStands(key, count, now)));
}

/* 무더기 줄의 원형 — 접힘 단추(표식 · 이름 · 수 · 메모 · 갈매기)와 누름 단추, 그 아래 답의 한 줄과
 * 되돌리기. 단추 안에 단추를 두지 않는다: 접힘과 누름은 나란한 둘이다. */
function artifactFlowPilePrototype() {
  const item = document.createElement("li");
  item.className = "artifact-flow-pile";
  const head = document.createElement("div");
  head.className = "artifact-flow-pile-head";
  const fold = document.createElement("button");
  fold.type = "button";
  fold.className = "artifact-flow-pile-fold";
  const mark = document.createElement("span");
  mark.className = "artifact-flow-node";
  mark.setAttribute("aria-hidden", "true");
  fold.append(
    mark,
    artifactLine("span", "artifact-flow-release-name", ""),
    artifactLine("span", "artifact-flow-release-count", ""),
    artifactLine("span", "artifact-flow-release-digest", ""),
    artifactFlowIcon("chevron"),
  );
  const press = document.createElement("button");
  press.type = "button";
  press.className = "btn artifact-flow-pile-press";
  head.append(fold, press);
  const said = document.createElement("p");
  said.className = "artifact-flow-pile-said";
  const undo = document.createElement("button");
  undo.type = "button";
  undo.className = "board-link artifact-flow-pile-undo";
  said.append(artifactLine("span", "artifact-flow-pile-words", ""), undo);
  item.append(head, said);
  return item;
}

/* 무더기 한 줄. 머리는 그릴 때마다 입고(답의 한 줄이 시간에 따라 접힌다), 펼친 줄들은 열쇠의 노드로
 * 다시 맞춘다. */
function artifactFlowPileRow(view, { pile, held, lines, count, key }, narrowed) {
  const pool = view._artifactFlowPiles ?? (view._artifactFlowPiles = new Map());
  let item = pool.get(pile.kind);
  if (!item) {
    item = artifactFlowClone("pile", artifactFlowPilePrototype);
    item.dataset.pile = pile.kind;
    item.querySelector(".artifact-flow-pile-fold").dataset.flowPile = pile.kind;
    item.querySelector(".artifact-flow-pile-press").dataset.flowPilePress = pile.kind;
    item.querySelector(".artifact-flow-pile-undo").dataset.flowPileUndo = pile.kind;
    pool.set(pile.kind, item);
  }
  const open = artifactFlowPilesOpen.has(pile.kind);
  const flow = artifactFlowAnswer;
  const now = Date.now();
  writeAttribute(item, "aria-expanded", open ? "true" : "false");
  const fold = item.querySelector(".artifact-flow-pile-fold");
  writeAttribute(fold, "aria-expanded", open ? "true" : "false");
  dressArtifactFlowMark(fold.querySelector(".artifact-flow-node"), pile.press === "waive" ? "check" : "wait", "quiet");
  const [, name, tally, note] = fold.children;
  writeTextContent(name, t(pile.key, pile.word));
  writeTextContent(tally, t("artifacts.flow.releaseCount", "과업 {{n}}", { n: artifactCountWord(count) }));
  writeTextContent(note, settlePileNote(pile, Number(flow?.quiet_days) || 0));
  const names = held?.named ?? [];
  const pressMax = Number(flow?.press_max) || 0;
  const press = item.querySelector(".artifact-flow-pile-press");
  writeTextContent(press, settlePressWords(pile, names.length, pressMax));
  writeHidden(press, narrowed || names.length === 0);
  writeDisabled(press, settlePresses.get(key)?.sending === true);
  const answer = settleSaid(key, now);
  const said = item.querySelector(".artifact-flow-pile-said");
  writeHidden(said, answer.said === "");
  writeAttribute(said, "data-tone", answer.tone);
  writeTextContent(said.firstElementChild, answer.said);
  const undo = said.lastElementChild;
  writeTextContent(undo, t("board.settle.undo", "되돌리기"));
  writeHidden(undo, !answer.undo);
  let rows = item.querySelector(":scope > .artifact-flow-release-rows");
  if (!open) {
    rows?.remove();
    return item;
  }
  if (!rows) {
    rows = document.createElement("ul");
    rows.className = "artifact-flow-release-rows";
    item.appendChild(rows);
  }
  const shown = lines.map((line) => artifactFlowKept(view, `pile:${line.task}`,
    [line.left, line.said, line.run, line.last_ms, artifactLocaleCode(), Math.floor(now / 60_000)].join("\n"), () => {
      const row = document.createElement("li");
      row.className = "artifact-flow-item is-pile-row";
      row.dataset.pileTask = line.task;
      const look = line.left === "look";
      const meta = document.createElement("div");
      meta.className = "artifact-flow-meta";
      meta.append(
        artifactLine("span", "artifact-flow-who", settleRowWhere(line, now)),
        artifactLine("span", "artifact-flow-id", line.task),
      );
      if (look) meta.prepend(artifactLine("span", "artifact-flow-pile-look", t(SETTLE_LEFT_WORDS.look.key, SETTLE_LEFT_WORDS.look.word)));
      row.append(artifactFlowMark("artifact-flow-node", look ? "half" : pile.press === "waive" ? "check" : "wait", look ? "wait" : "quiet"),
        artifactLine("p", "artifact-flow-said", line.said), meta);
      return row;
    }));
  const unseen = narrowed ? 0 : held?.more ?? 0;
  if (unseen > 0) {
    shown.push(artifactFlowKept(view, `pile-more:${pile.kind}`, [unseen, artifactLocaleCode()].join("\n"), () => {
      const line = document.createElement("li");
      line.className = "artifact-flow-older";
      line.append(artifactFlowIcon("archive"), t("board.settle.more", "그 밖에 {{count}}건", { count: unseen }));
      return line;
    }));
  }
  reconcileElementOrder(rows, shown);
  return item;
}

/* 펼친 무더기 한 줄의 「어디 · 언제」 — 런과 마지막으로 손댄 지 얼마. */
function settleRowWhere(line, now) {
  return [line.run ?? "", t("board.desk.mailAge", "{{time}} 전", { time: agoWord(Number(line.last_ms) || 0, now) })]
    .filter(Boolean).join(" · ");
}

/* 무더기의 누름 — 확인 창에 처음 몇 줄(닫히거나 생략될 일)의 과업 · 런 · 나이를 싣는다. */
function pressArtifactFlowPile(view, kind) {
  const pile = SETTLE_PILES.find((one) => one.kind === kind);
  const flow = artifactFlowAnswer;
  const held = pile ? flow?.[pile.flow] : null;
  if (!held) return;
  const rows = (held.tasks ?? []).filter((line) => line.left === pile.names)
    .map((line) => ({ task: line.task, run: line.run ?? "", words: line.said, touched: Number(line.last_ms) || 0 }));
  void pressSettlePile(`flow:${kind}`, pile, rows, held.named ?? [], Number(flow.press_max) || 0, () => paintArtifactFlow(view));
}

/* 지난 발행의 한 줄 — 판의 범위 · 언제까지 · 든 과업의 수. */
function artifactFlowOlderRow(view, older) {
  const versions = older.releases > 1 ? `${older.oldest} ~ ${older.newest}` : older.newest;
  const said = t("artifacts.flow.older", "{{versions}} · {{day}}까지 · 과업 {{n}}", {
    versions, day: artifactFlowDayWords(older.until_ms), n: artifactCountWord(older.tasks),
  });
  return artifactFlowKept(view, "older", said, () => {
    const line = document.createElement("li");
    line.className = "artifact-flow-older";
    line.append(artifactFlowIcon("archive"), said);
    return line;
  });
}

/* 묶음 하나의 칸 — 머리와 줄의 목록. */
function artifactFlowSection(list, key, head, rows) {
  const sections = list._sections ?? (list._sections = new Map());
  let section = sections.get(key);
  if (!section) {
    section = document.createElement("section");
    section.className = "artifact-flow-group";
    section.dataset.group = key;
    const items = document.createElement("ul");
    items.className = "artifact-flow-rows";
    section.append(head, items);
    section.setAttribute("aria-labelledby", head.id);
    sections.set(key, section);
  }
  reconcileElementOrder(section.lastElementChild, rows);
  return section;
}

/* 흐름 전부 — 묶음 셋을 세우고, 빈 프로젝트는 무엇이 없는지와 다음에 할 일을 말한다. */
function paintArtifactFlowList(view) {
  const flowBox = view.querySelector(".artifacts-flow");
  const list = flowBox.querySelector(".artifact-flow-list");
  const empty = flowBox.querySelector(".artifact-flow-empty");
  const flow = artifactFlowAnswer;
  writeHidden(flowBox.querySelector(".artifact-flow-outside"), flow?.git !== "outside");
  writeHidden(flowBox.querySelector(".artifact-flow-stale"), flow?.git !== "stale");
  const admitted = (group) => (group?.tasks ?? []).filter(artifactFlowAdmits);
  const narrowed = artifactFilter.query.trim() !== "" || Boolean(artifactFilter.agent);
  const now = admitted(flow?.now);
  const next = admitted(flow?.next);
  const nextRecords = narrowed ? [] : flow?.next?.records ?? [];
  const releases = (flow?.releases ?? [])
    .map((release) => ({ release, lines: admitted(release) }))
    .filter(({ release, lines }) => lines.length > 0 || (!narrowed && (release.records?.length ?? 0) > 0));
  const piles = artifactFlowPiles(flow, admitted, narrowed);
  const nothing = now.length === 0 && piles.length === 0 && next.length === 0 && nextRecords.length === 0 && releases.length === 0;
  writeHidden(empty, !nothing);
  writeHidden(list, nothing);
  if (nothing) {
    const project = artifactFlowProject();
    writeTextContent(empty.querySelector(".artifact-flow-empty-word"), narrowed
      ? t("artifacts.flow.emptyNarrowed", "이 거르개에 맞는 한 일이 없습니다")
      : t("artifacts.flow.empty", "{{project}}에서는 아직 한 일이 없습니다", { project: project?.label ?? "" }));
    for (const part of empty.querySelectorAll(".artifact-flow-empty-hint, .artifact-flow-empty-steps")) writeHidden(part, narrowed);
    return;
  }
  const sections = [];
  const [nowGroup, nextGroup, shippedGroup] = ARTIFACT_FLOW_GROUPS;
  if (now.length > 0 || piles.length > 0) {
    // 「지금」의 메모는 그 묶음이 담는 상태의 말 셋이다 — 상태의 표에서 읽는다. 남은 일의 무더기는
    // 그 아래 접힌 줄로 서고, 「지금」의 수에 들지 않는다(t-71220).
    const note = ARTIFACT_FLOW_NOW_STAGES.map((stage) => t(ARTIFACT_FLOW_STAGES[stage].key, ARTIFACT_FLOW_STAGES[stage].word)).join(" · ");
    const head = artifactFlowGroupHead(list, nowGroup.group, t(nowGroup.key, nowGroup.word), now.length + (flow.now?.more ?? 0), note);
    sections.push(artifactFlowSection(list, nowGroup.group, head, [
      ...artifactFlowRows(view, now),
      ...piles.map((held) => artifactFlowPileRow(view, held, narrowed)),
    ]));
  }
  if (next.length > 0 || nextRecords.length > 0) {
    const newest = flow.releases?.[0]?.version ?? "";
    const note = [
      flow.base ? t("artifacts.flow.onBase", "{{base}}에 반영됨", { base: flow.base }) : "",
      newest ? t("artifacts.flow.afterRelease", "{{version}} 뒤", { version: newest }) : "",
    ].filter(Boolean).join(" · ");
    const head = artifactFlowGroupHead(list, nextGroup.group, t(nextGroup.key, nextGroup.word), next.length + (flow.next.more ?? 0), note);
    sections.push(artifactFlowSection(list, nextGroup.group, head,
      [...artifactFlowRows(view, next), ...artifactFlowRecordRows(view, nextGroup.group, nextRecords)]));
  }
  if (releases.length > 0) {
    const head = artifactFlowGroupHead(list, shippedGroup.group, t(shippedGroup.key, shippedGroup.word), null,
      t("artifacts.flow.shippedNote", "새 발행부터 · 누르면 펼침"));
    const rows = releases.map(({ release, lines }) => artifactFlowReleaseRow(view, release, lines));
    if (flow.older && !narrowed) rows.push(artifactFlowOlderRow(view, flow.older));
    sections.push(artifactFlowSection(list, shippedGroup.group, head, rows));
  }
  reconcileElementOrder(list, sections);
  // 판에서 빠진 줄의 노드는 풀에서도 버린다 — 다음 답에 그 과업이 다시 오면 새로 짓는다.
  const standing = new Set([...list.querySelectorAll(".artifact-flow-item[data-task]")].map((row) => row.dataset.task));
  const pool = view._artifactFlowRows;
  if (pool) for (const task of [...pool.keys()]) if (!standing.has(task)) pool.delete(task);
}

/* 흐름 탭의 그림 전부: 흐름 · 판의 자세 · 읽는 칸. 흐름 밖의 탭에서는 상자만 숨는다. */
function paintArtifactFlow(view) {
  const flow = artifactFilter.tab === ARTIFACT_FLOW_TAB;
  const box = view.querySelector(".artifacts-flow");
  writeHidden(box, !flow);
  view.classList.toggle("is-tab-flow", flow);
  view.classList.toggle("is-flow-reading", flow && artifactFlowReading !== null && artifactSelectedId !== null);
  if (!flow) return;
  paintArtifactFlowList(view);
}

/* 답이 움직였다: 흐름 · 머리(프로젝트의 말) · 읽는 칸을 함께. */
function paintArtifactFlowAll(view) {
  paintArtifactHead(view);
  paintArtifactFlow(view);
  paintArtifactDrawer(view);
}

/* ---- 읽는 칸 ----------------------------------------------------------- */

/* 칩이 여는 행: 보고서와 페이지는 줄이 든 그것, 나머지는 그 과업의 묶음에서 그 종류의 첫 행. */
function artifactFlowChipRow(line, chip, bundle) {
  if (chip === "report") return line.chips?.report ?? null;
  if (chip === "page") return line.chips?.page ?? null;
  const pictures = bundle?.pictures ?? [];
  if (chip === "pair") return pictures.find((one) => one.before && one.after)?.after ?? null;
  if (chip === "picture") return pictures.find((one) => one.single)?.single ?? null;
  const evidence = bundle?.evidence ?? [];
  if (chip === "session") return evidence.find((one) => one.steps)?.id ?? null;
  return evidence[0]?.id ?? null;
}

/* 칩을 눌렀다: 그 과업의 묶음을 들고(없으면 묻고), 그 행을 읽는 칸에 세운다. 다시 누르면 닫힌다.
 * 전후 사진은 읽는 칸의 묶음에 선 짝의 카드를 열어 바로 비교한다. */
async function openArtifactFlowChip(view, task, chip) {
  if (artifactFlowReading?.task === task && artifactFlowReading.chip === chip) {
    standArtifactFlowReading(view, null, null);
    return;
  }
  const line = artifactFlowLine(task);
  if (!line) return;
  const bundle = await askArtifactBundle(task);
  const id = artifactFlowChipRow(line, chip, bundle);
  if (!id || !artifactRows.has(id)) return;
  standArtifactFlowReading(view, { task, chip }, id);
  view.querySelector(".artifacts-drawer").scrollTop = 0;
  if (chip === "pair") {
    const card = [...view.querySelectorAll(".artifact-bundle-cards button[data-before]")].find((one) => one.dataset.after === id);
    if (card) void openArtifactCompare(view, card);
  }
}

/* 「기록」의 칩을 눌렀다 — 과업이 없으니 묶음도 없다: 그 행 하나를 읽는 칸에 세운다. */
function openArtifactFlowRecord(view, id) {
  if (artifactFlowReading?.record === id) {
    standArtifactFlowReading(view, null, null);
    return;
  }
  const flow = artifactFlowAnswer;
  const groups = [flow?.next, ...(flow?.releases ?? [])];
  const row = groups.flatMap((group) => group?.records ?? []).find((one) => one.id === id);
  if (!row) return;
  if (!artifactRows.has(id)) artifactRows.set(id, row);
  standArtifactFlowReading(view, { record: id }, id);
}

/* 읽는 칸에 세운다 — 과업의 칩이면 그 과업의 묶음과 함께, 「기록」이면 그 행 하나. `null`이면 닫는다.
 * 앞서 연 전후 비교는 닫는다(과업별 탭의 `openArtifactTask`처럼): 묶음이 같으면 다시 그려지지 않아
 * 남은 비교가 새로 누른 것을 가린다. */
function standArtifactFlowReading(view, reading, id) {
  closeArtifactCompare(view);
  if (reading === null) {
    closeArtifactFlowReading();
  } else {
    artifactTaskOpen = reading.task ?? null;
    artifactFlowReading = reading;
    artifactSelectedId = id;
  }
  paintArtifactFlow(view);
  paintArtifactDrawer(view);
}

/* 읽는 칸을 닫는다 — 흐름을 떠났거나 그 줄이 사라졌다. */
function closeArtifactFlowReading() {
  if (artifactFlowReading === null) return;
  artifactFlowReading = null;
  artifactTaskOpen = null;
  artifactSelectedId = null;
}

/* 읽는 칸의 머리 — 흐름에서 연 것이면 어느 걸음의 무엇인지와 그 과업의 상태, 그리고 그 과업의 한
 * 줄(누르면 흐름에서 그 줄이 펼쳐진다). 흐름 밖의 읽기에서는 숨는다. */
function paintArtifactFlowReader(drawer, row) {
  const where = drawer.querySelector(".artifact-flow-where");
  const badge = drawer.querySelector(".artifact-flow-badge");
  const context = drawer.querySelector(".artifact-flow-context");
  const reading = artifactFilter.tab === ARTIFACT_FLOW_TAB ? artifactFlowReading : null;
  const line = reading?.task ? artifactFlowLine(reading.task) : null;
  writeHidden(where, !reading);
  writeHidden(badge, !line);
  writeHidden(context, !line);
  // 흐름에서 연 것은 머리가 그 걸음을 말한다 — 막대의 과업 칩 · 착지 · 시각 · 만든 이 · 시도 고르개는 그 줄과
  // 걸음이 이미 하는 말이라 물러난다(시안의 머리는 어느 걸음의 무엇 · 상태 · 세부 정보뿐이다).
  for (const part of drawer.querySelectorAll(ARTIFACT_FLOW_READER_HIDES)) part.classList.toggle("is-flow-hidden", Boolean(reading));
  if (!reading) return;
  writeTextContent(where, artifactFlowWhere(row));
  if (!line) return;
  const stage = artifactFlowStage(line);
  dressArtifactFlowMark(badge.querySelector(".artifact-flow-node"), stage.shape, stage.signal);
  // The context line leads with the same step shape as the badge: it is built with an empty circle (t-69107 ⑨).
  dressArtifactFlowMark(context.querySelector(".artifact-flow-node"), stage.shape, stage.signal);
  writeTextContent(badge.querySelector(".artifact-flow-badge-word"), artifactFlowStageWords(line));
  writeTextContent(context.querySelector(".artifact-flow-context-said"), line.gain ? `${line.said} — ${line.gain}` : line.said);
  const task = context.querySelector(".artifact-flow-context-task");
  writeTextContent(task, line.task);
  task.dataset.task = line.task;
  task.dataset.tip = t("artifacts.flow.contextOpen", "흐름에서 이 과업의 걸음 펼치기");
}

/* 흐름에서 읽는 동안 막대에서 물러나는 것들 — 그 줄과 걸음이 같은 말을 한다. */
const ARTIFACT_FLOW_READER_HIDES = [
  ".artifact-detail-bar .artifact-detail-task",
  ".artifact-detail-bar .artifact-detail-landing",
  ".artifact-detail-bar .artifact-detail-when",
  ".artifact-detail-bar .artifact-detail-maker",
  ".artifact-detail-bar .artifact-detail-attempts",
].join(", ");

/* 서랍에 흐름의 머리 셋을 단다 — 서랍을 짓는 김에 한 번(`buildArtifactDrawer`). */
function buildArtifactFlowReader(bar, detail) {
  const where = document.createElement("span");
  where.className = "artifact-flow-where";
  where.hidden = true;
  const badge = document.createElement("span");
  badge.className = "artifact-flow-badge";
  badge.hidden = true;
  const badgeWord = document.createElement("span");
  badgeWord.className = "artifact-flow-badge-word";
  badge.append(artifactFlowMark("artifact-flow-node", "wait", "quiet"), badgeWord);
  bar.querySelector(".artifact-detail-when").before(where, badge);
  const context = document.createElement("p");
  context.className = "artifact-flow-context";
  context.hidden = true;
  const contextSaid = document.createElement("span");
  contextSaid.className = "artifact-flow-context-said";
  const contextTask = document.createElement("button");
  contextTask.type = "button";
  contextTask.className = "artifact-flow-chip is-task artifact-flow-context-task";
  context.append(artifactFlowMark("artifact-flow-node", "wait", "quiet"), contextSaid, contextTask);
  detail.querySelector(".artifact-detail-title").before(context);
}

/* ---- 손 매기 (판마다 한 번) ----------------------------------------------- */

/* 발행 줄의 풍선 — 처음 가리키거나 초점이 올 때 쓴다. 이 상자의 손이 문서의 풍선 손보다 먼저 받는다
 * (`pointerover` · `focusin`은 문서까지 올라간다). 로케일이나 날이 바뀌면 줄을 새로 지어 다시 쓴다. */
function artifactFlowReleaseTip(event) {
  const head = event.target.closest?.("[data-flow-release]");
  if (!head || head.dataset.tip) return;
  head.dataset.tip = t("artifacts.flow.releaseTip", "{{version}} — {{time}} 발행", {
    version: head.dataset.flowRelease,
    time: artifactClockWords(Number(head.dataset.flowReleaseAt)),
  });
}

function wireArtifactFlow(view) {
  const box = view.querySelector(".artifacts-flow");
  box.addEventListener("pointerover", artifactFlowReleaseTip);
  box.addEventListener("focusin", artifactFlowReleaseTip);
  box.addEventListener("click", (event) => {
    const chip = event.target.closest("[data-flow-chip]");
    const row = event.target.closest(".artifact-flow-item[data-task]");
    if (chip?.dataset.flowRecord) {
      openArtifactFlowRecord(view, chip.dataset.flowRecord);
      return;
    }
    if (chip?.dataset.flowRow && row) {
      if (!artifactRows.has(chip.dataset.flowRow)) return;
      standArtifactFlowReading(view, { task: row.dataset.task, chip: chip.dataset.flowChip }, chip.dataset.flowRow);
      return;
    }
    if (chip && row) {
      void openArtifactFlowChip(view, row.dataset.task, chip.dataset.flowChip);
      return;
    }
    const pile = event.target.closest("[data-flow-pile]");
    if (pile) {
      const kind = pile.dataset.flowPile;
      if (artifactFlowPilesOpen.has(kind)) artifactFlowPilesOpen.delete(kind);
      else artifactFlowPilesOpen.add(kind);
      paintArtifactFlow(view);
      return;
    }
    const pilePress = event.target.closest("[data-flow-pile-press]");
    if (pilePress) {
      pressArtifactFlowPile(view, pilePress.dataset.flowPilePress);
      return;
    }
    const pileUndo = event.target.closest("[data-flow-pile-undo]");
    if (pileUndo) {
      undoSettle(`flow:${pileUndo.dataset.flowPileUndo}`, () => paintArtifactFlow(view));
      return;
    }
    const release = event.target.closest("[data-flow-release]");
    if (release) {
      const version = release.dataset.flowRelease;
      if (artifactFlowReleasesOpen.has(version)) artifactFlowReleasesOpen.delete(version);
      else artifactFlowReleasesOpen.add(version);
      paintArtifactFlow(view);
      return;
    }
    const records = event.target.closest(".artifact-flow-item.is-records");
    if (records) {
      const key = records.dataset.records;
      if (artifactFlowRecordsOpen.has(key)) artifactFlowRecordsOpen.delete(key);
      else artifactFlowRecordsOpen.add(key);
      paintArtifactFlow(view);
      return;
    }
    if (event.target.closest(".artifact-flow-empty-all")) {
      // 전체 목록 — 행이 있는 첫 탭, 아무 행도 없으면 페이지·문서의 빈 화면.
      const has = (tab) => (artifactTabCounts.get(tab.tab) ?? 0) > 0;
      artifactTabPicked = true;
      artifactFilter.tab = [artifactTabRow(ARTIFACT_PAGES_TAB), ...ARTIFACT_TABS].find(has)?.tab ?? ARTIFACT_PAGES_TAB;
      changeArtifactFilter(view);
      return;
    }
    // 펼친 걸음 안의 누름은 펼침을 건드리지 않는다 — 글을 고르거나 칩을 누르는 손이다.
    if (!row || (event.target.closest(".artifact-flow-detail") && !event.target.closest(".artifact-flow-more"))) return;
    toggleArtifactFlowRow(view, row.dataset.task);
  });
  box.addEventListener("keydown", (event) => {
    const row = event.target.closest(".artifact-flow-item[data-task]");
    if (!row || event.target !== row || (event.key !== "Enter" && event.key !== " ")) return;
    event.preventDefault();
    toggleArtifactFlowRow(view, row.dataset.task);
  });
  // 읽는 칸 머리의 과업 칩 — 흐름에서 그 줄의 걸음을 펼치고 그 줄로 간다.
  view.querySelector(".artifact-flow-context-task").addEventListener("click", (event) => {
    const task = event.currentTarget.dataset.task;
    if (!task) return;
    if (!artifactFlowOpen.has(task)) toggleArtifactFlowRow(view, task);
    view._artifactFlowRows?.get(task)?.scrollIntoView({ block: "nearest" });
  });
}

function toggleArtifactFlowRow(view, task) {
  if (artifactFlowOpen.has(task)) {
    artifactFlowOpen.delete(task);
  } else {
    artifactFlowOpen.add(task);
    // 걸음의 칩은 그 과업의 묶음에서 온다 — 처음 펼칠 때 한 번 묻고, 답이 오면 그 줄만 다시 입는다.
    if (!artifactBundles.has(task)) {
      void askArtifactBundle(task).then(() => {
        const row = view._artifactFlowRows?.get(task);
        const line = artifactFlowLine(task);
        if (row && line && artifactFlowOpen.has(task)) {
          row._line = null;
          row._signature = "";
          dressArtifactFlowRow(row, line);
        }
      });
    }
  }
  paintArtifactFlow(view);
}

/* 런타임의 말: 원장이 움직였다 — 흐름이 서 있을 때만 다시 묻는다(답이 같으면 그리지 않는다). */
listen("ledger:changed", () => {
  const tab = artifactsTab();
  if (!tab || !stillShowing(tab) || artifactFilter.tab !== ARTIFACT_FLOW_TAB) return;
  const view = docHost(tab.pane, ARTIFACTS_TAB.kind);
  if (view?._artifactsWired) void refreshArtifactFlow(view);
});
