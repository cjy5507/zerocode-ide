/* ---- 일정 (t-67482) -----------------------------------------------------------
 *
 * 작업 상황판의 「일정」 탭: 빌드 줄의 일감, 그 가운데 결합 게이트의 묶음, 발행
 * 레인, 워커 과업을 시간축 하나에 막대로 세운다. 끝난 것은 시작~끝, 도는 것은
 * 시작~예상 끝, 기다리는 것은 예상 시작~예상 끝이다. 사람이 아침마다 물은 「빌드 뒤
 * 다음 일정은」「언제 반영되나」에 창이 먼저 답한다.
 *
 * 읽는 것은 둘이다. 빌드 줄과 레인은 창의 읽기 전용 명령 하나(`schedule_timeline`)가
 * 읽는다 — 파일의 사실과, 같은 이름의 지난 소요 중앙값으로 낸 예상 소요까지. 워커
 * 과업은 데스크가 이미 읽는 원장의 과업 행(`deskLedger.tasks`의 `times`)이다. 예상
 * 소요(과거의 셈)와 실행기의 줄 목록은 백엔드의 것이고, 지금을 기준으로 막대를 놓는 일 —
 * 도는 것의 남은 시간, 기다리는 일감이 그 줄들에 서는 차례 — 은 이 파일의 것이다: 시간이
 * 흘러도 백엔드를 다시 부르지 않는다.
 *
 * 그림은 부품 하나다(`paintBoardScheduleHost`): 어느 그릇에 담겨도 그 그릇의 폭으로
 * 접힌다(shell.css 「일정」 절의 `@container board-schedule`). 그릇은 작업 상황판의 네 탭
 * 가운데 「일정」의 블록이다(shell-board-now.js의 `BOARD_TABS`, t-67607의 시안 B).
 *
 * 그림은 둘이다(t-77628, 사람이 통과시킨 시안 B p-e7e48c0f0a3959fa): 「지금 앞뒤」와 「하루」는 출발
 * 안내판 넉 줄(다음 발행 · 게이트 · 착지 대기 · 끝남 — 큰 시각이나 수 하나, 색 띠와 낱말, 행동 칩)과
 * 캘린더 하루 보기식 세로 시간표(열 넷, 시간이 아래로, 「지금」 가로선, 겹치면 나란히)이고, 「한 주」는
 * 가로 막대 차트 그대로다. 모델은 하나(`boardScheduleModel`)이고 그 행이 막대도 블록도 된다. 기다리는
 * 과업은 그것을 실은 게이트를 안다 — 일감의 `# task:` 줄이나 `what` 줄의 과업 id로 — 그래서 왜 기다리는지
 * 말하고, 게이트가 돌면 그 예상 끝을 물려받는다. */

/* 일정의 박자와 범위, 한 표. */
const BOARD_SCHEDULE = Object.freeze({
  /* 탭이 보일 때만 도는 박자. 묻을 때 쥔 판 번호를 함께 보내, 파일이 그대로면
   * 백엔드는 「그대로」만 답하고 여기는 아무것도 다시 그리지 않는다. */
  everyMs: 5_000,
  /* 「지금」 선과 남은 시간은 분 단위로 움직인다 — 5초에 옮길 픽셀은 없다. */
  clockMs: 60_000,
  /* 세 확대: 지금 앞뒤로 보이는 길이. 뒤쪽은 가장 늦은 예상 끝까지 늘어나되 상한이
   * 있다(`mostAfterMs`). */
  zooms: Object.freeze({
    /* 눈금 간격의 후보(`ticksMs`): 축의 폭에 눈금 글이 들어가는 가장 촘촘한 것을 고른다. 축의 폭을
     * 눈금 글 하나의 자리(`tickRoomPx`, CSS px)로 나눈 수만큼 눈금이 선다. 지금 앞뒤와 하루는 시각
     * (「09:00」, 고정폭 12 px로 약 36 px)으로, 한 주는 날짜(「10. 21. (수)」, 다섯 언어 가운데 가장
     * 넓어 약 84 px)로 읽는다 — 어느 쪽이든 두 글 사이에 12 px 넘게 남는다(t-71840). */
    /* 지금 앞뒤 — 처음 여는 확대(t-71840): 사람이 통과시킨 상황판 시안 v2(p-7c4a751fccf3359d)의
     * 「지금 도는 일」 축 그대로, 정시에 맞춰(`alignMs`) 지금의 한 시간 앞에서 네 시간. 가장 늦은
     * 예상 끝이 그 뒤면 정시 단위로 늘어난다. */
    now: Object.freeze({
      alignMs: 3_600_000, beforeMs: 3_600_000, afterMs: 3 * 3_600_000, mostAfterMs: 11 * 3_600_000,
      ticksMs: Object.freeze([3_600_000, 2 * 3_600_000, 3 * 3_600_000]), tickRoomPx: 72,
    }),
    day: Object.freeze({
      beforeMs: 16 * 3_600_000, afterMs: 8 * 3_600_000, mostAfterMs: 24 * 3_600_000,
      ticksMs: Object.freeze([3_600_000, 2 * 3_600_000, 3 * 3_600_000, 6 * 3_600_000, 12 * 3_600_000]),
      tickRoomPx: 72,
    }),
    // Four days apart stands only on a narrow axis: a date in the board's monospace is wider than an hour.
    week: Object.freeze({
      beforeMs: 5 * 86_400_000, afterMs: 2 * 86_400_000, mostAfterMs: 4 * 86_400_000,
      ticksMs: Object.freeze([86_400_000, 2 * 86_400_000, 4 * 86_400_000]), tickRoomPx: 96,
    }),
  }),
  /* 축 위 「지금」 칩의 자리(CSS px): 두 글자와 안쪽 여백(약 40 px)에 틈. 칩과 눈금 글의 반폭을 더한
   * 거리 안에 선 눈금은 글을 감추고 시간 선만 남긴다 — 시안 v2의 칩은 정시 글 곁에 서지, 그 위에
   * 겹치지 않는다(t-71840). */
  nowChipPx: 48,
  /* 세로 시간표의 홈통(t-77628): 눈금 글 하나의 자리(높이, CSS px)와 「지금 hh:mm」 칩의 높이. 칩과 눈금 글의
   * 반높이를 더한 거리 안에 든 눈금은 글을 감춘다 — 축의 규칙 그대로, 세로로. */
  calTickRoomPx: 28,
  calChipPx: 18,
  /* 게이트 블록의 단계 칸 줄: 칸 하나의 폭과 칸 사이 틈(CSS px) — 상태 점(10 px)과 같은 크기. */
  stepCellPx: 10,
  stepGapPx: 3,
  /* 눈금 글 둘 사이에 남기는 틈(CSS px). 한 글이 차지하는 자리는 `tickRoomPx`와 (잰 글 폭 + 이 틈) 가운데
   * 큰 쪽이다 — 글꼴이 커지거나 낱말이 길어지면 자리가 따라 커진다(검토 t-69069 P3 ⑧). */
  tickGapPx: 12,
  /* 끝난 것은 묶음마다 가장 최근에 끝난 이만큼만 행으로 서고, 나머지는 「+N 끝남」 접힌 줄 하나다 —
   * 사람이 그 줄을 누르면 다 선다. 도는 것·기다리는 것은 늘 다 선다. 네 묶음이 같은 규칙이다(t-71840;
   * 빌드 줄의 끝난 일감이 먼저 이렇게 섰다). */
  endedShown: 12,
  /* 가장 늦은 예상 끝 뒤에 남기는 여백. */
  tailMs: 30 * 60_000,
  /* 도는 레인의 막대가 빗금 안에 이름을 적는 앞 단계의 최소 예상 소요: 이보다 짧은 단계(flakes·push·
   * swap 같은 몇 초)는 이름을 세우지 않는다 — 시안의 「gate-zo · build · publish」. */
  namedStageSecs: 60,
});

/* 묶음 넷, 읽는 차례대로. */
const BOARD_SCHEDULE_GROUPS = Object.freeze([
  { id: "lanes", key: "board.schedule.groupLanes", word: "발행" },
  { id: "gates", key: "board.schedule.groupGates", word: "게이트" },
  { id: "line", key: "board.schedule.groupLine", word: "빌드 줄" },
  { id: "workers", key: "board.schedule.groupWorkers", word: "워커 과업" },
]);

/* 일감의 상태(백엔드 `Job::state`)마다 막대의 상태. 상태는 색만으로 말하지 않는다 —
 * 행의 글이 같은 것을 말하고, 막대의 모양(빗금·점선·꽉 참)이 한 번 더 말한다. */
const BOARD_SCHEDULE_JOB_STATES = Object.freeze({
  passed: "done",
  failed: "failed",
  capped: "failed",
  refused: "failed",
  lost: "idle",
  running: "running",
  waiting: "waiting",
  held: "held",
});

/* 실행기의 세 차례(0- · 보통 · 9-): 기다리는 일감이 줄에 서는 순서. */
const BOARD_SCHEDULE_CLASS_RANK = Object.freeze({ first: 0, plain: 1, last: 2 });

/* 끝난 막대의 상태: 묶음마다 최근 몇만 행이 되고 나머지는 접힌다(`boardScheduleEndedKept`). 끝 기록이
 * 없는 것(`idle`)은 도는 것처럼 늘 선다 — 볼 일이다. */
const BOARD_SCHEDULE_ENDED_STATES = Object.freeze(["done", "failed"]);

/* 워커 과업의 단계(데스크의 `DESK_STAGES` 낱말)마다 막대의 상태. */
const BOARD_SCHEDULE_TASK_STATES = Object.freeze({
  dispatched: "running",
  reported: "waiting",
  verified: "waiting",
  stale: "waiting",
  merged: "done",
  deployed: "done",
  nothing_to_land: "done",
  waived: "done",
  failed: "failed",
});

/* 백엔드의 마지막 온전한 답. `unchanged` 답은 이것을 그대로 둔다. */
let boardScheduleAnswer = null;
let boardScheduleAsking = false;
let boardScheduleAgain = false;
/* 마지막으로 물은 때. 탭을 다시 열면 이것이 박자 하나보다 오래됐을 때만 곧바로
 * 묻는다 — 열 때마다 묻지도, 첫 박자를 기다리지도 않는다(데스크와 같은 규칙). */
let boardScheduleAskedAt = 0;
/* 사람이 고른 것: 확대, 접은 묶음, 편 게이트, 연 로그. 고른 탭은 상황판의 것이다
 * (`boardNowTab`). 창의 기억이지 원장의 것이 아니다 — 판이 다시 그려져도 남는다. */
const boardScheduleChoice = { zoom: "now", folded: new Set(), opened: new Set(), log: null };
/* 고른 것이 바뀔 때마다 오르는 수: 그림의 서명에 든다. */
let boardScheduleChoiceRevision = 0;
/* 연 로그의 답(`LogTail`), 읽는 중이면 `{ reading: true }`. */
let boardScheduleLog = null;
/* 그릇마다 마지막으로 그린 것의 서명 — 같으면 박자는 아무것도 쓰지 않는다. */
const boardSchedulePainted = new WeakMap();
/* 그린 적이 있는 그릇. 복제된 판(`docHost`)은 첫 판의 노드를 죽은 마크업으로 들고
 * 오므로, 처음 그릴 때 한 번 비운다 — 데스크와 같은 규칙. */
const boardScheduleBuilt = new WeakSet();
/* 시각과 길이의 글: 같은 분, 같은 언어면 같은 글이라 한 번 짓고 기억한다.
 * `toLocaleTimeString`은 부를 때마다 형식기를 새로 지어(한 번 약 18 µs), 50행을 다시
 * 그리는 박자의 절반 넘게가 그것이었다(t-67482: 효율 코어에서 7.55 ms). */
const boardScheduleWords = new Map();

function boardScheduleRemembered(key, make) {
  let said = boardScheduleWords.get(key);
  if (said === undefined) {
    if (boardScheduleWords.size >= 4_096) boardScheduleWords.clear();
    said = make();
    boardScheduleWords.set(key, said);
  }
  return said;
}

/* 일정이 서 있는 그릇들: 작업 보기의 보드에서 「일정」 탭을 고른 판. */
function boardScheduleHosts() {
  return deskViews()
    .map((view) => view.querySelector(".task-board-schedule"))
    .filter((host) => host && !host.hidden);
}

function noteBoardScheduleChoice() {
  boardScheduleChoiceRevision += 1;
}

/* 「일정」 탭의 블록. 데스크의 그림(`paintCoordinatorDesk`)이 탭을 쓴 뒤에 부른다 — 그릇은
 * 표면이 지금 든 탭(`data-board-tab`)이 「일정」이고 원장의 런이 있어 탭이 설 때만
 * 보이고(`data-board-ledger`, `paintBoardTabs`), 그때만 묻고 박자를 돌린다. 탭을 바꾸는
 * 손은 상황판의 것이다(`chooseBoardTab`). */
function paintBoardSchedule(view) {
  const surface = view.querySelector(".task-board-surface");
  const host = surface?.querySelector(".task-board-schedule");
  if (!host) return;
  const chosen = surface.dataset.boardTab === "schedule" && surface.dataset.boardLedger === "on";
  // A tab that opens asks at once, with no revision: the range it shows starts now.
  const opening = chosen && host.hidden;
  writeHidden(host, !chosen);
  boardSchedulePoller.sync();
  if (!chosen) return;
  if (opening || Date.now() - boardScheduleAskedAt >= BOARD_SCHEDULE.everyMs) refreshBoardSchedule({ fresh: opening });
  paintBoardScheduleHost(host, view, Date.now());
}

/* 확대의 기준 순간: 정시에 맞추는 확대(지금 앞뒤)는 지금이 든 정시, 나머지는 지금. */
function boardScheduleAnchor(zoom, now) {
  return zoom.alignMs ? Math.floor(now / zoom.alignMs) * zoom.alignMs : now;
}

/* 보이는 범위의 왼쪽 끝: 백엔드는 이보다 먼저 끝난 것을 보내지 않는다. */
function boardScheduleFrom() {
  const zoom = BOARD_SCHEDULE.zooms[boardScheduleChoice.zoom] ?? BOARD_SCHEDULE.zooms.now;
  return boardScheduleAnchor(zoom, Date.now()) - zoom.beforeMs;
}

/* 백엔드에 묻는다. 쥔 판 번호를 함께 보내고, 같은 판이면 답은 「그대로」뿐이다.
 * 탭을 처음 열거나 확대를 바꾸면 판 번호 없이 묻는다 — 보이는 범위가 달라졌다. */
function refreshBoardSchedule({ fresh = false } = {}) {
  if (boardScheduleAsking) {
    boardScheduleAgain = boardScheduleAgain || fresh;
    return;
  }
  boardScheduleAsking = true;
  boardScheduleAskedAt = Date.now();
  const known = fresh || !boardScheduleAnswer ? null : boardScheduleAnswer.revision;
  invoke("schedule_timeline", { known, fromMs: boardScheduleFrom() })
    .then((answer) => {
      if (answer && typeof answer === "object" && !answer.unchanged) {
        boardScheduleAnswer = answer;
        // The 지금 tab's 빌드 줄 card reads the same answer (its foot, the lane's steps ahead, t-70028).
        if (boardNowTab === "now") scheduleDeskPaint();
      }
      paintBoardScheduleHosts();
    })
    .catch(() => {})
    .finally(() => {
      boardScheduleAsking = false;
      if (boardScheduleAgain) {
        boardScheduleAgain = false;
        refreshBoardSchedule({ fresh: true });
      }
    });
}

/* 로그 하나를 연다: 같은 명령에 일감 이름이나 레인 sha를 실어 끝 줄들을 받는다.
 * 프로젝트 밖의 파일이라 편집기의 문(`read_text_file`)은 거절한다 — 그래서 이 탭
 * 아래 칸에 보인다. 같은 것을 다시 누르면 닫는다. */
function openBoardScheduleLog(ask) {
  const same = boardScheduleChoice.log && JSON.stringify(boardScheduleChoice.log) === JSON.stringify(ask);
  boardScheduleChoice.log = same ? null : ask;
  boardScheduleLog = same ? null : { reading: true };
  noteBoardScheduleChoice();
  paintBoardScheduleHosts();
  if (same) return;
  invoke("schedule_timeline", { known: boardScheduleAnswer?.revision ?? null, fromMs: boardScheduleFrom(), log: ask })
    .then((answer) => {
      if (JSON.stringify(boardScheduleChoice.log) !== JSON.stringify(ask)) return;
      if (answer && !answer.unchanged) boardScheduleAnswer = answer;
      boardScheduleLog = answer?.log ?? { missing: true, lines: [] };
    })
    .catch(() => {
      if (JSON.stringify(boardScheduleChoice.log) === JSON.stringify(ask)) boardScheduleLog = { missing: true, lines: [] };
    })
    .finally(() => {
      noteBoardScheduleChoice();
      paintBoardScheduleHosts();
    });
}

function paintBoardScheduleHosts() {
  const now = Date.now();
  for (const view of deskViews()) {
    const host = view.querySelector(".task-board-schedule");
    if (host && !host.hidden) paintBoardScheduleHost(host, view, now);
  }
}

/* ---- 배치: 지금을 기준으로 막대를 놓는다 --------------------------------------- */

/* 기다리는 일감이 실행기의 줄에 서는 차례를 흉내 낸다. 줄은 실행기 기록이 말한 것이다(백엔드의
 * `line.runner_lanes`). 도는 일감이 예상 끝에 그 줄을 비우고, 기다리는 일감은 0- → 보통 → 9-
 * 차례로, 같은 차례 안에서는 넣은 차례로, 먼저 비는 줄에 선다. 같은 주인의 일감은 한 번에
 * 하나다(실행기의 규칙). 가드가 세운 일감은 줄에 서지 않고, 기록이 아직 어느 줄도 말하지 않았으면
 * 기다리는 일감도 그처럼 지금에 선다 — 그 행은 「시작 모름」이라 말한다. 예상이지 약속이 아니다 —
 * 행의 글이 「예상」이라고 말한다. */
function boardScheduleQueuePlan(jobs, runnerLanes, now) {
  const placed = new Map();
  const lanes = new Map(runnerLanes.map((lane) => [lane, now]));
  const owners = new Map();
  const secs = (job) => Number(job.expected?.secs ?? job.cap_secs) || 0;
  const unplaced = (job) => ({ start: now, end: now + secs(job) * 1_000, late: false });
  for (const job of jobs) {
    if (job.state !== "running") continue;
    const start = Number(job.start_ms) || now;
    const due = start + secs(job) * 1_000;
    const end = Math.max(now, due);
    placed.set(job.name, { start, end, late: now > due });
    if (lanes.has(job.lane)) lanes.set(job.lane, Math.max(lanes.get(job.lane), end));
    owners.set(job.who, Math.max(owners.get(job.who) ?? now, end));
  }
  const waiting = jobs.filter((job) => job.state === "waiting").sort((a, b) =>
    (BOARD_SCHEDULE_CLASS_RANK[a.class] ?? 1) - (BOARD_SCHEDULE_CLASS_RANK[b.class] ?? 1)
      || (Number(a.queued_ms) || 0) - (Number(b.queued_ms) || 0) || a.name.localeCompare(b.name));
  for (const job of waiting) {
    const first = [...lanes].sort((a, b) => a[1] - b[1] || a[0] - b[0])[0];
    if (!first) {
      placed.set(job.name, unplaced(job));
      continue;
    }
    const [lane, free] = first;
    const start = Math.max(free, owners.get(job.who) ?? now);
    const end = start + secs(job) * 1_000;
    lanes.set(lane, end);
    owners.set(job.who, end);
    placed.set(job.name, { start, end, late: false });
  }
  for (const job of jobs) {
    if (job.state === "held") placed.set(job.name, unplaced(job));
  }
  return placed;
}

/* 기다리는 일감의 시작을 모르는 까닭, 없으면 "". 줄이 멈췄으면(PAUSE) "paused"로 행이 「줄 멈춤」을
 * 말하고, 측정이 조용한 기계를 기다리거나(QUIET_WANTED) 디스크가 하한 아래거나 실행기 기록이 아직 어느
 * 줄도 말하지 않았으면 "unknown"으로 「시작 모름」만 말한다 — 앞의 둘은 머리 글이 까닭을 말한다. 어느
 * 쪽이든 기다리는 행은 시작 시각을 말하지 않는다: 머리 글과 행이 같은 말을 한다(검토 t-69069 P2-2). */
function boardScheduleStall(facts, runnerLanes) {
  if (facts.paused === true) return "paused";
  return facts.quiet_wanted === true || facts.disk_low === true || runnerLanes.length === 0 ? "unknown" : "";
}

/* 도는 레인의 예상 끝: 지금 단계는 시작 + 중앙값, 그 뒤의 단계는 중앙값을 차례로 더한다.
 * 지금 단계가 제 중앙값보다 오래 돌고 있으면 끝은 「지금 + 남은 단계의 중앙값」으로 다시 서고,
 * 넘은 만큼(`lateMs`)을 막대가 「지연 중」으로 말한다 — 예측 모델이 아니라 표시 규칙이다
 * (코디 m-67652). 근거 없는 단계가 하나라도 있으면 그 끝은 아래 한계다. */
function boardScheduleLaneEnd(run, now) {
  const ahead = Array.isArray(run.ahead) ? run.ahead : [];
  const current = Array.isArray(run.stages) ? run.stages[run.stages.length - 1] : null;
  if (!current || ahead.length === 0) return { end: now, lateMs: 0, known: false };
  const due = Number(current.start_ms) + (Number(ahead[0].expected?.secs) || 0) * 1_000;
  let end = Math.max(now, due);
  for (const stage of ahead.slice(1)) end += (Number(stage.expected?.secs) || 0) * 1_000;
  // An answer that says its stages ahead had no lane's plan (`ahead_planned: false`) holds the stage it is in alone:
  // what is left of the lane is not known.
  return { end, lateMs: Math.max(0, now - due),
    known: run.ahead_planned !== false && ahead.every((stage) => Number(stage.expected?.samples) > 0) };
}

/* 길이 한 마디: 1분이 안 되면 그렇다고. 글은 분 수로 정해진다. */
function boardScheduleSpan(ms) {
  const minutes = ms > 0 ? Math.floor(ms / 60_000) : -1;
  return boardScheduleRemembered(`span:${minutes}:${document.documentElement.lang}`,
    () => usageDuration(ms) ?? t("board.desk.underMinute", "1분 미만"));
}

/* 시각 한 마디 — 시안 B의 시계(`boardClock`): 하루 안이면 24시간 시:분, 같은 분의 순간은 같은
 * 글이다(그 시계가 이 기억에 둔다). */
function boardScheduleClock(ms, now) {
  return boardClock(ms, now);
}

/* 예상의 근거 한 마디 — 행의 팁에 선다. */
function boardScheduleBasis(expected) {
  if (!expected) return "";
  const time = boardScheduleSpan((Number(expected.secs) || 0) * 1_000);
  return Number(expected.samples) > 0
    ? t("board.schedule.basisHistory", "지난 {{count}}회 중앙값 {{time}}", { count: expected.samples, time })
    : t("board.schedule.basisCap", "근거 없음 · 상한 {{time}}", { time });
}

/* 일감 한 줄의 말: 끝난 것은 판정과 길이, 도는 것은 남은 시간, 기다리는 것은 예상 시작 — 시작을
 * 모르면(`boardScheduleStall`) 그렇다고. */
function boardScheduleJobSaid(job, place, now, stall) {
  const took = boardScheduleSpan((Number(job.secs) || 0) * 1_000);
  switch (job.state) {
    case "passed": return t("board.schedule.passed", "통과 · {{time}}", { time: took });
    case "failed": return Number.isInteger(job.rc)
      ? t("board.schedule.failedRc", "실패 rc {{rc}} · {{time}}", { rc: job.rc, time: took })
      : t("board.schedule.failed", "실패 · {{time}}", { time: took });
    case "capped": return t("board.schedule.capped", "시간 초과 · {{time}}", { time: took });
    case "refused": return t("board.schedule.refused", "줄이 거절함");
    case "lost": return t("board.schedule.lost", "끝 기록 없음");
    case "held": return t("board.schedule.held", "가드가 세움 · 시작 모름");
    case "running": return place?.late
      ? t("board.schedule.late", "+{{time}} 지연 중", {
        time: boardScheduleSpan(now - (Number(job.start_ms) + (Number(job.expected?.secs) || 0) * 1_000)),
      })
      : t("board.schedule.left", "{{time}} 남음", { time: boardScheduleSpan((place?.end ?? now) - now) });
    default: return stall === "paused"
      ? t("board.schedule.paused", "줄 멈춤 · 시작 모름")
      : stall
        ? t("board.schedule.startUnknown", "시작 모름")
        : t("board.schedule.startsAt", "{{time}} 시작 예상", { time: boardScheduleClock(place?.start ?? now, now) });
  }
}

/* 행 하나의 모양: 그릴 것은 이것뿐이다. 시안 v2의 「지금 도는 일」 한 행 그대로 — 이름과 그 밑 한 줄
 * (`sub`: 상태 점과 낱말 · 자세히), 막대와 그 안의 이름 둘(`nowLabel`: 칠한 구간, `aheadLabel`: 빗금이나
 * 점선 구간), 끝 칸의 굵은 시각과 그 밑 한 줄(`eta`·`etaSub`). `said`는 그 행의 말 한 문장이다 — 행의
 * 이름표(aria)와 팁이 읽는다. `doneShare`는 막대의 칠한 몫이다(없으면 상태로 정한다). */
function boardScheduleRow(fields) {
  return {
    depth: 0, basis: "history", act: null, sub: "", eta: "", etaSub: "", nowLabel: "", aheadLabel: "",
    doneShare: null, ...fields,
  };
}

/* 둘째 줄: 시안 B의 상태 점과 낱말(「지금」 탭의 `BOARD_RUN_STATES`) 뒤에 자세히. 막대의 상태마다
 * 그 낱말 — 도는 것, 기다리는 것, 멈춘 것, 끝난 것. */
const BOARD_SCHEDULE_RUN_STATE = Object.freeze({
  running: "run", waiting: "queue", held: "queue", idle: "halt", failed: "halt", done: "done",
});

function boardScheduleSub(state, ...details) {
  const word = BOARD_RUN_STATES[BOARD_SCHEDULE_RUN_STATE[state] ?? "queue"];
  return [t(word.key, word.word), ...details].filter(Boolean).join(" · ");
}

/* 끝 칸의 굵은 한 마디(시안 v2의 「끝」, 시안 B의 끝 칸 낱말): 도는 것·기다리는 것은 예상 끝의 시각이고,
 * 시작을 모르면 「—」, 끝난 것은 「끝」, 멈춘 것은 「멈춤」이다. */
function boardScheduleEndWord(state, end, now, unknown) {
  if (state === "done") return t("board.schedule.ended", "끝");
  if (state === "failed" || state === "idle") return t(BOARD_RUN_STATES.halt.key, BOARD_RUN_STATES.halt.word);
  return unknown ? "—" : boardScheduleClock(end, now);
}

/* 원장이나 git이 적은 순간 하나, 적지 않았으면 null. `Number(null)`은 0 — 1970년의 한 순간이다. */
function boardScheduleMoment(ms) {
  const moment = ms == null ? NaN : Number(ms);
  return Number.isFinite(moment) ? moment : null;
}

/* 끝난 것 가운데 행이 될 것: 가장 최근에 끝난(`end`) `endedShown`개 — 사람이 그 묶음의 접힌 줄을
 * 폈으면 다 — 와, 그 수를 넘는 끝난 것의 수(`more`). 네 묶음이 이 하나로 접는다(t-71840). */
function boardScheduleEndedKept(ended, opened) {
  const newest = [...ended].sort((a, b) => b.end - a.end);
  return {
    kept: opened ? newest : newest.slice(0, BOARD_SCHEDULE.endedShown),
    more: Math.max(0, newest.length - BOARD_SCHEDULE.endedShown),
  };
}

/* 한 묶음 안의 차례: 시작 차례, 같으면 키 차례 — 네 묶음이 같다(t-71840). 게이트의 일감은 제 머리 바로
 * 아래에 같은 차례로 선다. */
function boardScheduleOrder(a, b) {
  return a.start - b.start || a.key.localeCompare(b.key);
}

/* 일감 이름에서 차례(`0-`·`9-`)와 넣은 시각(epoch)을 뗀 것: 행의 이름은 그것이면 된다. */
function boardScheduleJobLabel(name) {
  return String(name).replace(/^[09]-/, "").replace(/^\d{9,11}-/, "");
}

/* 일감이 실은 과업의 id(t-77628): `# task:` 줄(`tasks`)이 있으면 그것, 없으면 `# what:` 줄에 적힌
 * 「t-77350 grid + t-77414/77495/77530 ctty helper」 꼴 — 빗금 뒤의 수는 앞 id의 접두를 생략한 것이다.
 * 지금 줄의 일감은 `# task:` 줄을 적지 않아 `what`이 유일한 길이고, 게이트 도구가 그 줄을 적게 되면
 * 창은 바꿀 것이 없다. */
function boardScheduleJobTaskIds(job) {
  const ids = new Set((Array.isArray(job.tasks) ? job.tasks : []).map(String).filter((id) => /^t-\d+$/.test(id)));
  if (ids.size === 0 && typeof job.what === "string") {
    for (const hit of job.what.matchAll(/\bt-(\d+)((?:\/\d+)*)/g)) {
      const base = hit[1];
      ids.add(`t-${base}`);
      for (const more of hit[2].split("/")) {
        // 「t-77414/495」 is t-77495: a shorter number after the slash borrows the first id's leading digits.
        if (more) ids.add(`t-${more.length >= base.length ? more : base.slice(0, base.length - more.length) + more}`);
      }
    }
  }
  return ids;
}

/* 과업 id → 그 과업을 실은 게이트. 둘 이상이 실었으면 가장 나중에 넣은 일감의 게이트다. */
function boardScheduleTaskGates(jobs) {
  const carried = new Map();
  for (const job of jobs) {
    if (!job.gate) continue;
    const at = Number(job.queued_ms) || Number(job.start_ms) || 0;
    for (const id of boardScheduleJobTaskIds(job)) {
      const held = carried.get(id);
      if (!held || at >= held.at) carried.set(id, { gate: job.gate, at });
    }
  }
  return carried;
}

/* 같은 열에서 시간이 겹치는 블록은 캘린더처럼 나란히 선다(t-77628): 시작 차례로 걸으며 겹치는 것끼리 한
 * 묶음이고, 묶음 안에서는 먼저 비는 부열에 선다. 부열의 수는 묶음의 것이라, 혼자인 블록은 열을 다 쓴다.
 * 블록은 글 한 줄보다 낮아지지 않으므로(`leastMs`: 그 최소 높이가 덮는 시간) 2분짜리 둘이 3분 간격으로 서면
 * 그림에서는 겹친다 — 겹침은 그린 길이로 센다. 같은 입력이면 같은 자리다. 행에 `col`·`cols`를 적는다. */
function boardScheduleSubColumns(rows, leastMs = 0) {
  const until = (row) => Math.max(row.end, row.start + Math.max(1, leastMs));
  let cluster = [];
  let reach = -Infinity;
  const place = () => {
    const ends = [];
    for (const row of cluster) {
      let col = ends.findIndex((end) => end <= row.start);
      if (col < 0) col = ends.push(row.start) - 1;
      ends[col] = until(row);
      row.col = col;
    }
    for (const row of cluster) row.cols = ends.length;
    cluster = [];
  };
  for (const row of rows) {
    if (cluster.length > 0 && row.start >= reach) place();
    cluster.push(row);
    reach = Math.max(reach, until(row));
  }
  if (cluster.length > 0) place();
  return rows;
}

function boardScheduleJobRow(job, plan, now, stall, depth) {
  const place = plan.get(job.name);
  const start = Number(job.start_ms) || place?.start || Number(job.end_ms) || now;
  const end = Number(job.end_ms) || place?.end || start;
  const short = job.kind && depth > 0
    ? [job.part, job.kind].filter(Boolean).join(" · ") : boardScheduleJobLabel(job.name);
  const state = BOARD_SCHEDULE_JOB_STATES[job.state] ?? "idle";
  const said = boardScheduleJobSaid(job, place, now, stall);
  // A gate's job says what it is expected to take; a job of its own says whose it is.
  const detail = depth > 0 ? boardScheduleBasis(job.expected) : job.who;
  return boardScheduleRow({
    key: `job:${job.name}`,
    depth,
    label: short,
    tip: [job.name, job.who, job.what, boardScheduleBasis(job.expected)].filter(Boolean).join(" · "),
    start,
    end,
    state,
    basis: job.expected && Number(job.expected.samples) === 0 && ["running", "waiting", "held"].includes(job.state)
      ? "cap" : "history",
    said,
    sub: boardScheduleSub(state, detail),
    eta: boardScheduleEndWord(state, end, now, job.state === "held" || (job.state === "waiting" && Boolean(stall))),
    etaSub: said,
    nowLabel: job.state === "running" ? job.kind ?? "" : "",
    act: { log: { job: job.name } },
    paused: Boolean(stall) && job.state === "waiting",
    late: job.state === "running" && Boolean(place?.late),
  });
}

/* 레인이 끝난 결과(`result`) → 행의 상태. 여기 없는 결과(`ended`)는 그저 끝난 행이다. */
const BOARD_SCHEDULE_LANE_STATES = Object.freeze({ passed: "done", failed: "failed" });

function boardScheduleLaneRows(lanes, now) {
  return lanes.map((run) => {
    const stages = Array.isArray(run.stages) ? run.stages : [];
    const tip = stages.map((stage) => Number.isFinite(stage.end_ms)
      ? t("board.schedule.tipStage", "{{name}} {{time}}", {
        name: stage.name, time: boardScheduleSpan(stage.end_ms - stage.start_ms),
      })
      : stage.name).join(" · ");
    const label = t("board.schedule.laneName", "{{name}} 레인", { name: run.version ? `v${run.version}` : String(run.sha) });
    if (run.running) {
      const { end, lateMs, known } = boardScheduleLaneEnd(run, now);
      const current = stages[stages.length - 1];
      const stage = current?.name ?? "";
      const ahead = Array.isArray(run.ahead) ? run.ahead.slice(1) : [];
      const next = ahead[0]?.name;
      return boardScheduleRow({
        key: `lane:${run.sha}:${run.start_ms}`, label, tip, start: run.start_ms, end, state: "running",
        basis: known ? "history" : "none", late: lateMs > 0,
        said: lateMs > 0
          ? t("board.schedule.laneLate", "{{stage}} · +{{late}} 지연 중 · {{time}} 남음", {
            stage, late: boardScheduleSpan(lateMs), time: boardScheduleSpan(end - now),
          })
          : t("board.schedule.laneRunning", "{{stage}} · {{time}} 남음", { stage, time: boardScheduleSpan(end - now) }),
        sub: boardScheduleSub("running",
          t("board.schedule.laneSub", "{{stage}} {{time}}째", { stage, time: boardScheduleSpan(now - Number(current?.start_ms)) }),
          next ? t("board.schedule.laneNext", "다음 {{stage}}", { stage: next }) : ""),
        eta: boardScheduleClock(end, now),
        etaSub: lateMs > 0
          ? t("board.schedule.late", "+{{time}} 지연 중", { time: boardScheduleSpan(lateMs) })
          : t("board.schedule.left", "{{time}} 남음", { time: boardScheduleSpan(end - now) }),
        // The stage it is in fills the bar to now; the stages ahead that take a minute or more name the stripes.
        nowLabel: stage,
        aheadLabel: ahead.filter((one) => (Number(one.expected?.secs) || 0) >= BOARD_SCHEDULE.namedStageSecs)
          .map((one) => one.name).join(" · "),
        act: { log: { lane: run.sha } },
      });
    }
    // The answer says how the lane ended (`result`); the lane's own words are only its name for it.
    const said = DESK_RELEASE_VERDICTS[DESK_RELEASE_OF_RESULT[run.result]];
    const verdict = said
      ? t(said.key, said.word, { phase: "" })
      : String(run.verdict ?? t("board.schedule.lost", "끝 기록 없음"));
    const end = Number(run.end_ms) || run.start_ms;
    const state = BOARD_SCHEDULE_LANE_STATES[run.result] ?? "idle";
    return boardScheduleRow({
      key: `lane:${run.sha}:${run.start_ms}`, label, tip: [tip, run.reason].filter(Boolean).join(" · "),
      start: run.start_ms, end, state,
      said: t("board.schedule.laneEnded", "{{verdict}} · {{time}}", { verdict, time: boardScheduleSpan(end - run.start_ms) }),
      sub: boardScheduleSub(state, verdict, boardScheduleClock(end, now)),
      eta: boardScheduleEndWord(state, end, now, false),
      etaSub: t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - run.start_ms) }),
      nowLabel: verdict,
      act: { log: { lane: run.sha } },
    });
  });
}

/* 결합 게이트: 이름의 `-l<NN>` 토막마다 머리 한 행과, 펴면 바로 아래에 서는 그 일감들. 도는 일감이나
 * 기다리는 일감이 있는 게이트는 처음부터 펴져 있다. 머리 행은 제 단계들의 상태(`steps`, 세로 시간표의
 * 단계 칸 줄)와 사실 한 벌(`facts`: 안내판과 기다리는 과업이 읽는다)을 함께 든다. */
function boardScheduleGateRows(jobs, plan, now, choice, stall) {
  const gates = new Map();
  for (const job of jobs) {
    if (!job.gate) continue;
    if (!gates.has(job.gate)) gates.set(job.gate, []);
    gates.get(job.gate).push(job);
  }
  const entries = [];
  for (const [gate, members] of gates) {
    const placed = members.map((job) => boardScheduleJobRow(job, plan, now, stall, 1));
    const start = Math.min(...placed.map((row) => row.start));
    const reach = Math.max(...placed.map((row) => row.end));
    // A step's last job speaks for it: a step that failed and passed on its rerun passed.
    const latest = new Map();
    for (const job of members) {
      const step = job.kind ?? job.name;
      const held = latest.get(step);
      if (!held || (Number(job.queued_ms) || 0) >= (Number(held.queued_ms) || 0)) latest.set(step, job);
    }
    const steps = [...latest.values()];
    // The present attempt (t-77851·t-77880): where its earliest step began — the words 「걸림」 and the gate's
    // length measure from here — and where its latest step began — the board compares gates by this, so a gate run
    // again under the same name, or one step of it run again, is the newer story. No step begun yet: null.
    const begun = steps.map((job) => Number(job.start_ms) || 0).filter((ms) => ms > 0);
    const startedAt = begun.length > 0 ? Math.min(...begun) : null;
    const lastStart = begun.length > 0 ? Math.max(...begun) : null;
    const began = startedAt ?? start;
    // A waiting gate's one time word (t-77892 P3-3) — the row's `said` (its aria-label), its `etaSub` and so the
    // board's line all say it: the earliest expected start among its waiting steps (`placed` keeps `members`' order)
    // — the new attempt's when it is only queued, the next step's when some step already ran. Only held steps left
    // (the guard holds them; no lane plans them), or a stalled line: the start is unknown. No past moment is ever
    // called an expected start.
    const planned = new Map(members.map((job, i) => [job.name, placed[i]]));
    const ahead = steps.filter((job) => job.state === "waiting").map((job) => planned.get(job.name)?.start).filter(Number.isFinite);
    const nextStart = ahead.length > 0 ? Math.min(...ahead) : null;
    const startWord = stall || nextStart === null ? "" : boardScheduleClock(nextStart, now);
    const count = (states) => steps.filter((job) => states.includes(job.state)).length;
    const open = count(["running", "waiting", "held"]);
    const failed = count(["failed", "capped", "refused"]);
    const green = count(["passed"]);
    const parts = [...new Set(members.map((job) => job.part).filter(Boolean))].sort();
    // A step still running keeps the gate running; a failed step stops it even while others queue (review P3-3).
    const state = count(["running"]) > 0 ? "running" : failed > 0 ? "failed" : open > 0 ? "waiting" : "done";
    // A stopped gate ends where its last finished step ended — the steps queued behind it will not run (review r2 P2);
    // a gate still going reaches the expected end of its steps.
    const finished = members.map((job) => Number(job.end_ms)).filter((ms) => Number.isFinite(ms) && ms > 0);
    const end = state === "failed" ? (finished.length > 0 ? Math.max(...finished) : now) : reach;
    const said = state === "running"
      ? t("board.schedule.gateRunning", "{{done}}/{{total}} · {{time}} 남음", {
        done: steps.length - open, total: steps.length, time: boardScheduleSpan(end - now),
      })
      : state === "waiting"
        ? startWord
          ? t("board.schedule.gateWaiting", "대기 {{count}}개 · {{time}} 시작 예상", { count: open, time: startWord })
          : t("board.schedule.gateWaitingUnknown", "대기 {{count}}개 · 시작 모름", { count: open })
        : state === "failed"
          ? t("board.schedule.gateFailed", "실패 {{count}}개 · {{time}}", { count: failed, time: boardScheduleSpan(end - began) })
          : t("board.schedule.gatePassed", "통과 {{count}}개 · {{time}}", {
            count: steps.length, time: boardScheduleSpan(end - began),
          });
    const opened = choice.opened.has(gate) || (open > 0 && !choice.opened.has(`-${gate}`));
    // The bar's two names (the passed draft's 「12개 초록」): what ended, or the step running and the steps after it.
    const kinds = (states) => steps.filter((job) => states.includes(job.state)).map((job) => job.kind ?? job.name);
    const tally = state === "done"
      ? t("board.schedule.gateDone", "{{done}}/{{total}} 초록", { done: steps.length, total: steps.length })
      : state === "failed"
        ? t("board.schedule.gateFailedCount", "실패 {{count}}개", { count: failed })
        : `${steps.length - open}/${steps.length}`;
    const basis = placed.some((row) => row.basis === "cap") ? "cap" : "history";
    const unknown = state === "waiting" && !startWord;
    // The tasks the gate carries, by its jobs' `# task:` lines or their `what` lines (t-77628).
    const carried = new Set();
    for (const job of members) {
      for (const id of boardScheduleJobTaskIds(job)) carried.add(id);
    }
    const failedJob = steps.find((job) => ["failed", "capped", "refused"].includes(job.state)) ?? null;
    // The job that tells the gate's story (t-77628 review P2-1): the step that failed, else the one running,
    // else the one queued last — a block and the board's line open its log, since a block holds no rows to unfold.
    // A gate none of whose steps has run tells no story yet: no job, no chip (review r2 P3-1).
    const telling = failedJob ?? steps.find((job) => job.state === "running")
      ?? members.filter((job) => Number(job.end_ms) > 0).sort((a, b) => Number(b.end_ms) - Number(a.end_ms))[0] ?? null;
    const facts = {
      gate, state, start, end, basis, unknown, green, total: steps.length, failed,
      failedKinds: kinds(["failed", "capped", "refused"]), failedJob: failedJob?.name ?? null,
      logJob: telling?.name ?? null, carried, opened,
      // When the gate was queued last: the moment its latest job was put on the line.
      queuedAt: Math.max(0, ...members.map((job) => Number(job.queued_ms) || 0)),
      startedAt, lastStart,
      // The waiting gate's one time word, for the rows of the tasks it carries as well (t-77931 P2): "" = unknown.
      startWord,
    };
    entries.push({
      head: boardScheduleRow({
        key: `gate:${gate}`, label: t("board.schedule.gateName", "{{gate}} 결합 게이트", { gate }),
        tip: [parts.join("·"), ...members.map((job) => job.name)].filter(Boolean).join(" · "), start, end, state,
        basis, said,
        sub: boardScheduleSub(state, tally, state === "done" || state === "failed" ? boardScheduleClock(end, now) : ""),
        eta: boardScheduleEndWord(state, end, now, unknown),
        etaSub: state === "running"
          ? t("board.schedule.left", "{{time}} 남음", { time: boardScheduleSpan(end - now) })
          : state === "waiting"
            ? !startWord
              ? t("board.schedule.startUnknown", "시작 모름")
              : startedAt !== null
                // Between its steps the gate's first step is past: the next step's expected start is the word (t-77880).
                ? t("board.schedule.nextStepAt", "다음 단계 {{time}} 시작 예상", { time: startWord })
                : t("board.schedule.startsAt", "{{time}} 시작 예상", { time: startWord })
            : t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - began) }),
        nowLabel: state === "done"
          ? t("board.schedule.gateBarPassed", "{{count}}개 초록", { count: steps.length })
          : state === "failed"
            ? t("board.schedule.gateBarFailed", "{{count}}개 실패", { count: failed })
            : kinds(["running"]).join(" · "),
        aheadLabel: kinds(["waiting", "held"]).join(" · "),
        act: { fold: gate, opened }, opened, paused: Boolean(stall) && state === "waiting",
        // The step cells of the vertical timetable's gate block, in the steps' order.
        steps: steps.map((job) => BOARD_SCHEDULE_JOB_STATES[job.state] ?? "idle"),
        facts,
      }),
      rows: opened ? placed : [],
      facts,
    });
  }
  return entries;
}

/* 게이트 밖의 일감: 도는 것·기다리는 것은 다, 끝난 것은 범위 안에서 가장 최근에 끝난 몇만 — 나머지는
 * 수다. 끝난 것은 접은 뒤에 행으로 짓는다: 답 하나가 일감 수백을 싣는다. */
function boardScheduleLineRows(jobs, plan, now, stall, from, opened) {
  const outside = jobs.filter((job) => !job.gate);
  const moving = (job) => ["running", "waiting", "held", "lost"].includes(job.state);
  const ended = outside.filter((job) => !moving(job) && (Number(job.end_ms) || 0) >= from)
    .map((job) => ({ job, end: Number(job.end_ms) || 0 }));
  const { kept, more } = boardScheduleEndedKept(ended, opened);
  return {
    rows: [...kept.map((one) => one.job), ...outside.filter(moving)]
      .map((job) => boardScheduleJobRow(job, plan, now, stall, 0)),
    more,
  };
}

/* 끝난 과업의 끝(t-71840): git이 말한 착지(병합 커밋의 시각), 그것이 없으면 그 일이 실린 발행, 그것도
 * 없으면 원장이 적은 완료·검증·넘김·시도 끝 가운데 가장 늦은 것. 아무것도 적히지 않았으면 null이다 —
 * 끝난 일에 지금을 쓰면 막대가 「지금」 선까지 늘어나고 「N일 걸림」이 부푼다(사람 10-23 사진의 t-66760). */
function boardScheduleTaskEnd(times, landing) {
  const told = boardScheduleMoment(landing?.landed_ms) ?? boardScheduleMoment(landing?.released_ms);
  if (told !== null) return told;
  const written = [times?.landed_ms, times?.verified_ms, times?.handed_ms, times?.ended_ms]
    .map(boardScheduleMoment).filter((ms) => ms !== null);
  return written.length > 0 ? Math.max(...written) : null;
}

/* 워커 과업: 데스크가 읽은 원장의 과업 가운데 시각을 가진 것. 막대는 첫 시도의 시작부터 — 도는 것과
 * 기다리는 것은 지금까지, 끝난 것은 그 끝(`boardScheduleTaskEnd`)까지 — 서고, 그 안에 넘김과 검증의
 * 자리를 표시한다. 끝난 것은 범위 안에서 가장 최근에 끝난 몇만 행이 되고 나머지는 수다. `landings`는
 * 일정의 답이 git에서 읽어 온 착지·발행이다(과업 id마다). `link`는 게이트와의 이음(t-77628): 과업 id →
 * 실은 게이트(`carried`), 게이트 → 그 사실(`gates`) — 기다리는 과업이 왜 기다리는지와, 실린 게이트가
 * 돌면 그 예상 끝을 물려받는다. */
function boardScheduleTaskRows(tasks, now, landings, from, opened, link = null) {
  const moving = [];
  const ended = [];
  for (const task of Array.isArray(tasks) ? tasks : []) {
    const start = boardScheduleMoment(task?.times?.started_ms);
    if (start === null || !(task.stage in BOARD_SCHEDULE_TASK_STATES)) continue;
    const landing = landings?.[task.id] ?? null;
    if (!BOARD_SCHEDULE_ENDED_STATES.includes(BOARD_SCHEDULE_TASK_STATES[task.stage])) {
      moving.push({ task, landing, start, end: now, known: true });
      continue;
    }
    const at = boardScheduleTaskEnd(task.times, landing);
    const end = Math.max(start, at ?? start);
    if (end >= from) ended.push({ task, landing, start, end, known: at !== null });
  }
  const { kept, more } = boardScheduleEndedKept(ended, opened);
  return { rows: [...kept, ...moving].map((one) => boardScheduleTaskRow(one, now, link)), more };
}

/* 기다리는 과업과 그것을 실은 게이트(t-77628): 게이트가 돌거나 기다리면 그 예상 끝을 물려받고 셋째 줄이
 * 「l80에 실림 → 도는 중 · 15:40 끝 예상」을 말한다. 실패했으면 지금에서 끝나고 「실패로 멈춤」, 통과했으면
 * 「코디 차례」다. 실린 게이트가 없으면 null. */
function boardScheduleTaskRide(task, now, link, since) {
  const ride = link?.carried.get(task.id);
  const gate = ride ? link.gates.get(ride.gate) : null;
  if (!gate) return null;
  // A gate that ended before this hand-in is an old story (review P3-2): it does not say why the task waits now.
  if (["done", "failed"].includes(gate.state) && gate.end < since) return null;
  const word = gate.state === "running"
    ? t("board.schedule.whyRunning", "{{gate}}에 실림 → 도는 중 · {{time}} 끝 예상", {
      gate: gate.gate, time: boardScheduleClock(gate.end, now),
    })
    : gate.state === "waiting"
      // The gate's own time word — its next waiting step's expected start, never its first attempt's past start.
      ? gate.unknown || !gate.startWord
        ? t("board.schedule.whyWaitingUnknown", "{{gate}}에 실림 → 차례 기다림 · 시작 모름", { gate: gate.gate })
        : t("board.schedule.whyWaiting", "{{gate}}에 실림 → 차례 기다림 · {{time}} 시작 예상", {
          gate: gate.gate, time: gate.startWord,
        })
      : gate.state === "failed"
        ? t("board.schedule.whyFailed", "{{gate}}에 실림 → 실패로 멈춤", { gate: gate.gate })
        : t("board.schedule.whyPassed", "{{gate}} 통과 → 코디 차례", { gate: gate.gate });
  const inherits = ["running", "waiting"].includes(gate.state) && gate.end > now;
  return { gate: gate.gate, state: gate.state, end: inherits ? gate.end : now, basis: inherits ? gate.basis : "history",
    inherits, word, act: gate.failedJob ? { log: { job: gate.failedJob } } : { fold: gate.gate, opened: gate.opened } };
}

function boardScheduleTaskRow({ task, landing, start, end, known }, now, link = null) {
  const times = task.times;
  const stage = DESK_STAGES.find((one) => one.id === task.stage);
  const word = stage ? t(stage.key, stage.word) : String(task.stage);
  const since = Math.max(Number(times.verified_ms) || 0, Number(times.handed_ms) || 0, start);
  const state = BOARD_SCHEDULE_TASK_STATES[task.stage];
  const ride = state === "waiting" ? boardScheduleTaskRide(task, now, link, since) : null;
  const waited = boardScheduleSpan(now - since);
  const said = state === "running"
    ? t("board.schedule.taskRunning", "{{stage}} · {{time}}째", { stage: word, time: boardScheduleSpan(now - start) })
    : state === "waiting"
      ? [t("board.schedule.taskWaiting", "{{stage}} · 기다린 지 {{time}}", { stage: word, time: waited }), ride?.word]
        .filter(Boolean).join(" · ")
      : known
        ? t("board.schedule.taskLanded", "{{stage}} · {{time}} 걸림", { stage: word, time: boardScheduleSpan(end - start) })
        : t("board.schedule.taskNoEnd", "{{stage}} · 끝 기록 없음", { stage: word });
  const clock = (ms) => {
    const moment = boardScheduleMoment(ms);
    return moment === null ? "—" : boardScheduleClock(moment, now);
  };
  // The passed draft's second line names the worker carrying it (「w-67501 · 코드 작성」): the board's own reading.
  const carrier = deskTaskCarrier(task);
  const handed = boardScheduleMoment(times.handed_ms);
  const over = BOARD_SCHEDULE_ENDED_STATES.includes(state);
  // A waiting task carried by a gate still running ends where the gate is expected to (t-77628).
  const until = ride?.inherits ? ride.end : end;
  return boardScheduleRow({
    key: `task:${task.run}/${task.id}`, label: `${task.id} ${task.title}`, id: task.id, title: task.title,
    tip: [`${task.id} · ${task.title}`, task.run, t("board.schedule.tipTask",
      "시작 {{start}} · 넘김 {{handed}} · 검증 {{verified}} · 착지 {{landed}} · 발행 {{released}}", {
        start: clock(times.started_ms), handed: clock(times.handed_ms), verified: clock(times.verified_ms),
        landed: clock(landing?.landed_ms ?? times.landed_ms), released: clock(landing?.released_ms),
      })].filter(Boolean).join(" · "),
    start, end: until, state, said, basis: ride?.inherits ? ride.basis : "history",
    sub: boardScheduleSub(state, carrier?.worker, word, over && known ? boardScheduleClock(end, now) : ""),
    why: ride?.word ?? "",
    ride: ride ? { gate: ride.gate, state: ride.state, inherits: ride.inherits } : null,
    waitedMs: state === "waiting" ? now - since : 0,
    // The end cell speaks the way the Now tab's does: how long it has run, 「대기」, or that it ended.
    eta: state === "running"
      ? t("board.schedule.since", "{{time}}째", { time: boardScheduleSpan(now - start) })
      : state === "waiting"
        ? t("board.now.stepAhead", "대기")
        : boardScheduleEndWord(state, end, now, false),
    etaSub: state === "running"
      ? ""
      : state === "waiting"
        ? t("board.schedule.waited", "기다린 지 {{time}}", { time: waited })
        : known
          ? t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - start) })
          : t("board.schedule.lost", "끝 기록 없음"),
    // A finished bar carries its stage; one waiting is filled to its hand-in and dashed from there, named by its stage.
    nowLabel: over ? word : "",
    aheadLabel: state === "waiting" ? word : "",
    doneShare: state === "waiting" && handed !== null && until > start
      ? Math.min(1, Math.max(0, (handed - start) / (until - start))) : null,
    act: { task: { run: task.run, id: task.id, stage: task.stage } },
  });
}

/* 눈금 글을 재는 자: 축의 눈금과 같은 글꼴로 낱말의 폭(CSS px)을 잰다. 글꼴은 서 있는 눈금이 말하고, 아직
 * 눈금이 없으면 토큰이 말한다. 잴 수 없으면 null — 눈금은 간격 후보의 자리(`tickRoomPx`)만으로 선다. */
let boardScheduleCanvas = null;

function boardScheduleTickFont(axis) {
  const tick = axis.querySelector(".board-schedule-tick");
  return tick ? getComputedStyle(tick).font : getComputedStyle(axis).getPropertyValue("--type-label-12-mono").trim();
}

/* 홈통의 눈금 글 한 줄의 높이(CSS px): 서 있는 눈금의 줄 높이, 아직 없으면 0 — 자리는 그보다 작지 않다. */
function boardScheduleTickHeight(track) {
  const tick = track.querySelector(".board-schedule-tick");
  return tick ? parseFloat(getComputedStyle(tick).lineHeight) || 0 : 0;
}

function boardScheduleTickMeasure(font) {
  if (!font) return null;
  boardScheduleCanvas ??= document.createElement("canvas").getContext("2d");
  const context = boardScheduleCanvas;
  if (!context) return null;
  return (text) => boardScheduleRemembered(`measure:${font}:${text}`, () => {
    context.font = font;
    return context.measureText(text).width;
  });
}

/* 한 간격의 눈금들: 그 간격의 현지 시각 경계마다. */
function boardScheduleTickMoments(from, to, step) {
  const day = step >= 86_400_000;
  const first = new Date(from);
  first.setMinutes(0, 0, 0);
  if (day) first.setHours(0);
  let at = first.getTime();
  const hours = step / 3_600_000;
  while (at < from || (!day && new Date(at).getHours() % hours !== 0)) at += day ? step : 3_600_000;
  const tongue = document.documentElement.lang || undefined;
  const ticks = [];
  for (; at <= to; at += step) {
    const moment = at;
    ticks.push({
      at,
      label: boardScheduleRemembered(`tick:${moment}:${day}:${tongue}`, () => (day
        ? new Date(moment).toLocaleDateString(tongue, { month: "numeric", day: "numeric", weekday: "short" })
        : new Date(moment).toLocaleTimeString(tongue, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }))),
    });
  }
  return ticks;
}

/* 눈금 글 하나의 자리: 간격 후보가 말하는 자리와 잰 가장 넓은 글 + 틈 가운데 큰 쪽. */
function boardScheduleTickRoom(ticks, zoom, measure) {
  if (!measure) return zoom.tickRoomPx;
  const widest = Math.max(0, ...ticks.map((tick) => measure(tick.label)));
  return Math.max(zoom.tickRoomPx, Math.ceil(widest + BOARD_SCHEDULE.tickGapPx));
}

/* 눈금: 축의 폭에 글이 들어가는 가장 촘촘한 간격으로. 어느 간격도 안 맞으면 가장 성긴 간격의 눈금을
 * 솎아 글이 겹치지 않게 한다. */
function boardScheduleTicks(from, to, width, zoom, measure = null) {
  let sparsest = [];
  let room = zoom.tickRoomPx;
  const fewest = Math.max(2, Math.floor(width / zoom.tickRoomPx));
  for (const step of zoom.ticksMs) {
    // A step that is too many for the candidate's own room is too many for any wider one: its labels are not built.
    if ((to - from) / step > fewest && step !== zoom.ticksMs.at(-1)) continue;
    const ticks = boardScheduleTickMoments(from, to, step);
    room = boardScheduleTickRoom(ticks, zoom, measure);
    if ((to - from) / step <= Math.max(2, Math.floor(width / room))) return { ticks, room };
    sparsest = ticks;
  }
  const every = Math.ceil(sparsest.length / Math.max(1, Math.floor(width / room)));
  return { ticks: sparsest.filter((_, at) => at % every === 0), room };
}

/* 세로 시간표를 그리는 확대(t-77628): 「지금 앞뒤」와 「하루」. 「한 주」는 가로 막대 그대로다 — 캘린더
 * 주간 보기(날짜 7열)는 다음 판의 것이다. */
function boardScheduleVertical(zoomId) {
  return zoomId !== "week";
}

/* 안내판의 넉 줄(t-77628) — 공항 출발 안내판의 문법: 한 줄에 무엇 · 큰 글(시각이나 수) 하나 · 낱말 · 비고 · 행동.
 * 다음 발행, 게이트, 착지 대기, 끝남. 줄은 비어도 남고 「—」·「이 범위에 없음」을 말한다. 수는 시간표의 행
 * 목록에서 센다 — 따로 세지 않는다. */
function boardScheduleBoardLines(groups, now) {
  const group = (id) => groups.find((one) => one.id === id);
  const rows = (id) => (group(id)?.rows ?? []).filter((row) => row.depth === 0);
  const clock = (ms) => boardScheduleClock(ms, now);
  const notInRange = t("board.schedule.notInRange", "이 범위에 없음");
  const logWord = t("board.schedule.actLog", "로그 →");
  const recordWord = t("board.schedule.actRecord", "기록 →");
  const stateWord = (state) => (state === "running"
    ? t(BOARD_RUN_STATES.run.key, BOARD_RUN_STATES.run.word)
    : state === "waiting"
      ? t(BOARD_RUN_STATES.queue.key, BOARD_RUN_STATES.queue.word)
      : state === "failed" ? t("board.schedule.wordFailed", "실패") : t("board.schedule.wordPassed", "통과"));
  const tones = { running: "run", waiting: "wait", failed: "halt", done: "done" };

  // 1. The next release: the lane running, else the last one that ended in the range.
  const lanes = rows("lanes");
  const running = lanes.find((row) => row.state === "running") ?? null;
  const lastLane = lanes.filter((row) => row.state !== "running").sort((a, b) => b.end - a.end)[0] ?? null;
  const lane = running
    ? {
      tone: running.late ? "wait" : "run", big: running.eta, word: running.etaSub,
      note: [running.sub, running.basis === "none" ? t("board.schedule.basisNone", "근거 없음") : ""].filter(Boolean).join(" · "),
      act: running.act, actWord: logWord,
    }
    : {
      tone: "quiet", big: "—", word: t("board.schedule.none", "잡힌 것 없음"),
      note: lastLane
        ? t("board.schedule.lastLane", "마지막 {{name}} {{time}} · {{verdict}}", {
          name: lastLane.label, time: clock(lastLane.end), verdict: lastLane.nowLabel,
        })
        : notInRange,
      act: lastLane?.act ?? null, actWord: lastLane ? logWord : "",
    };

  // 2. The gate, by one rule (review r4·r5, t-77851·t-77880): the gate with a step running; else, of the gates that
  // actually started — a step finished or failed — the one whose latest step began last (passed, failed or waiting
  // between steps); else the one queued last. A pass after a failure is the newer word; a gate between its steps is
  // still the story in play; a gate run again under the same name, or one step of it run again, is the newer story,
  // not its first attempt. Between gates that overlapped on two lanes it is the later start, not the later end — a
  // choice (t-77892 P3-1): comparing ends would hide a gate waiting between its steps behind an earlier failure.
  // Two gates whose latest steps began at the same moment are decided, not left to the rows' order: the failed one,
  // then the one between steps, then the passed, then the higher gate number.
  const attempt = (row) => row.facts?.lastStart ?? row.start;
  const told = { failed: 3, waiting: 2, done: 1 };
  const number = (row) => Number(String(row.facts?.gate ?? "").match(/\d+/)?.[0]) || 0;
  const decided = (a, b) => (told[b.state] ?? 0) - (told[a.state] ?? 0) || number(b) - number(a)
    || String(b.facts?.gate ?? "").localeCompare(String(a.facts?.gate ?? ""));
  const gateHeads = rows("gates").slice().sort((a, b) => attempt(b) - attempt(a) || decided(a, b));
  const started = (row) => Boolean(row.facts?.logJob);
  const queuedLast = gateHeads.slice()
    .sort((a, b) => (b.facts?.queuedAt ?? 0) - (a.facts?.queuedAt ?? 0) || decided(a, b))[0] ?? null;
  const gate = gateHeads.find((row) => row.state === "running") ?? gateHeads.find(started) ?? queuedLast;
  const facts = gate?.facts ?? null;
  const gateLine = gate && facts
    ? {
      tone: tones[gate.state] ?? "quiet", name: facts.gate,
      big: gate.state === "done" || gate.state === "failed" ? clock(gate.end) : gate.eta, word: stateWord(gate.state),
      note: [
        facts.failedKinds.length > 0
          ? t("board.schedule.failedStep", "{{kind}} 실패", { kind: facts.failedKinds.join(" · ") }) : "",
        t("board.schedule.gateDone", "{{done}}/{{total}} 초록", { done: facts.green, total: facts.total }),
        facts.carried.size > 0 ? t("board.schedule.carriedCount", "과업 {{count}}건 실림", { count: facts.carried.size }) : "",
        gate.etaSub,
      ].filter(Boolean).join(" · "),
      act: facts.logJob ? { log: { job: facts.logJob } } : null,
      actWord: facts.logJob ? logWord : "",
    }
    : { tone: "quiet", name: "", big: "—", word: "", note: notInRange, act: null, actWord: "" };

  // 3. Tasks waiting to land, and what they wait for.
  const waiting = rows("workers").filter((row) => row.state === "waiting");
  const oldest = waiting.slice().sort((a, b) => b.waitedMs - a.waitedMs)[0] ?? null;
  const rides = waiting.filter((row) => row.ride);
  const byGate = new Map();
  for (const row of rides) byGate.set(row.ride.gate, (byGate.get(row.ride.gate) ?? 0) + 1);
  const [topGate, topCount] = [...byGate].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0] ?? [null, 0];
  const topState = rides.find((row) => row.ride.gate === topGate)?.ride.state ?? null;
  const blocked = rides.some((row) => row.ride.state === "failed");
  const gating = rides.some((row) => ["running", "waiting"].includes(row.ride.state));
  const waitingLine = {
    tone: waiting.length === 0 ? "quiet" : blocked ? "halt" : "wait",
    big: t("board.schedule.countOf", "{{count}}건", { count: waiting.length }),
    word: waiting.length === 0
      ? t("board.schedule.noneWaiting", "없음")
      : blocked
        ? t("board.schedule.blocked", "막힘")
        : gating ? t("board.schedule.gateRunningWord", "게이트 도는 중") : t("board.schedule.coordTurn", "코디 차례"),
    note: oldest
      ? [
        t("board.schedule.oldest", "가장 오래 {{time}} ({{id}})", { time: boardScheduleSpan(oldest.waitedMs), id: oldest.id }),
        topGate
          ? t("board.schedule.carriedBy", "{{count}}건 {{gate}}에 실림 → {{state}}", {
            count: topCount, gate: topGate, state: stateWord(topState),
          })
          : "",
      ].filter(Boolean).join(" · ")
      : "",
    act: oldest?.act ?? null, actWord: oldest ? recordWord : "",
  };

  // 4. What ended in the range, the folded ones counted in.
  const ended = rows("workers").filter((row) => BOARD_SCHEDULE_ENDED_STATES.includes(row.state));
  const endedCount = ended.length + (group("workers")?.more ?? 0);
  const last = ended.slice().sort((a, b) => b.end - a.end)[0] ?? null;
  const endedLine = {
    tone: endedCount > 0 ? "done" : "quiet",
    big: t("board.schedule.countOf", "{{count}}건", { count: endedCount }),
    word: t(BOARD_RUN_STATES.done.key, BOARD_RUN_STATES.done.word),
    note: last
      ? t("board.schedule.lastEnded", "마지막 {{time}} · {{stage}}", { time: clock(last.end), stage: last.nowLabel })
      : notInRange,
    act: endedCount > 0 ? { history: true } : null, actWord: endedCount > 0 ? recordWord : "",
  };

  return {
    head: {
      now: t("board.schedule.nowClock", "지금 {{time}}", { time: clock(now) }),
      sum: [
        running
          ? t("board.schedule.boardLaneAt", "다음 발행 {{time}}", { time: running.eta })
          : t("board.schedule.boardLaneNone", "다음 발행 없음"),
        gate ? t("board.schedule.boardGate", "게이트 {{word}}", { word: gateLine.word }) : "",
        t("board.schedule.boardWaiting", "착지 대기 {{count}}", { count: waiting.length }),
      ].filter(Boolean).join(" · "),
    },
    lines: [
      { id: "lane", what: t("board.schedule.lineLane", "다음 발행"), whatSub: t("board.schedule.lineLaneSub", "발행 레인"), ...lane },
      {
        id: "gate", what: [t("board.schedule.lineGate", "게이트"), gateLine.name].filter(Boolean).join(" "),
        whatSub: t("board.schedule.lineGateSub", "결합 게이트"), ...gateLine,
      },
      {
        id: "waiting", what: t("board.schedule.lineWaiting", "착지 대기"),
        whatSub: t("board.schedule.lineWaitingSub", "보고·검증 뒤 · 코디 차례"), ...waitingLine,
      },
      { id: "ended", what: t("board.schedule.lineEnded", "끝남"), whatSub: t("board.schedule.lineEndedSub", "이 범위"), ...endedLine },
    ],
  };
}

/* 그릴 것 한 벌: 보이는 범위, 눈금, 묶음 넷과 그 행들, 안내판 넉 줄. 백엔드의 답과 데스크의 과업,
 * 지금, 고른 것, 축의 폭에서 DOM 없이 짓는다 — 장면 시험이 이것을 그대로 읽는다. 세로 시간표를 그리는
 * 확대면(`vertical`) 시각 홈통의 눈금(`gutter`)과 블록의 부열(`col`·`cols`)도 여기서 정한다 — 시간당
 * 높이(`hourPx`)는 CSS의 것이라 그릇이 읽어 넘긴다. */
function boardScheduleModel(answer, tasks, now, choice, width, loud = false, measure = null, metrics = null) {
  // The vertical timetable's measures, read off the stylesheet by the holder: the hour's height, a tick's line
  // height and a block's least height (review P3-8: the stylesheet holds them once).
  const hourPx = Number(metrics?.hourPx) || 0;
  const tickPx = Number(metrics?.tickPx) || 0;
  const leastPx = Number(metrics?.leastPx) || 0;
  const zoom = BOARD_SCHEDULE.zooms[choice.zoom] ?? BOARD_SCHEDULE.zooms.now;
  const vertical = boardScheduleVertical(choice.zoom);
  const jobs = Array.isArray(answer?.jobs) ? answer.jobs : [];
  const lanes = Array.isArray(answer?.lanes) ? answer.lanes : [];
  const facts = answer?.line ?? {};
  const runnerLanes = Array.isArray(facts.runner_lanes) ? facts.runner_lanes : [];
  const stall = boardScheduleStall(facts, runnerLanes);
  const plan = boardScheduleQueuePlan(jobs, runnerLanes, now);
  const anchor = boardScheduleAnchor(zoom, now);
  const from = anchor - zoom.beforeMs;
  const opened = (group) => choice.opened.has(`ended:${group}`);
  // The gates first: the tasks they carry read their facts (t-77628).
  const gates = boardScheduleGateRows(jobs, plan, now, choice, stall);
  const link = { carried: boardScheduleTaskGates(jobs), gates: new Map(gates.map((entry) => [entry.facts.gate, entry.facts])) };
  // The line and the workers fold their finished rows before they build them; lanes and gates are few.
  const line = boardScheduleLineRows(jobs, plan, now, stall, from, opened("line"));
  const workers = boardScheduleTaskRows(tasks, now, answer?.landings, from, opened("workers"), link);
  // Each group as entries: a row, and the rows standing under it (a gate's jobs).
  const alone = (row) => ({ head: row, rows: [] });
  const built = {
    lanes: { entries: boardScheduleLaneRows(lanes, now).map(alone), more: null },
    gates: { entries: gates, more: null },
    line: { entries: line.rows.map(alone), more: line.more },
    workers: { entries: workers.rows.map(alone), more: workers.more },
  };
  const latest = Math.max(now, ...Object.values(built).flatMap((group) => group.entries)
    .flatMap((entry) => [entry.head, ...entry.rows])
    .filter((row) => ["running", "waiting", "held"].includes(row.state)).map((row) => row.end));
  // An aligned zoom ends on the hour too — the passed draft's 09:00–13:00.
  const reach = Math.max(anchor + zoom.afterMs, latest + BOARD_SCHEDULE.tailMs);
  const to = Math.min(anchor + zoom.mostAfterMs, zoom.alignMs ? Math.ceil(reach / zoom.alignMs) * zoom.alignMs : reach);
  const span = Math.max(1, to - from);
  const percent = (ms) => Math.min(100, Math.max(0, ((ms - from) / span) * 100));
  const seen = (row) => row.end >= from && row.start <= to;
  // A block is at least one line tall (`--schedule-block-least`): the time that height covers, for the side-by-side test.
  const leastMs = hourPx > 0 ? (leastPx / hourPx) * 3_600_000 : 0;
  const groups = BOARD_SCHEDULE_GROUPS.map((group) => {
    let { entries, more } = built[group.id];
    entries = entries.filter((entry) => seen(entry.head));
    if (more === null) {
      const ended = (entry) => BOARD_SCHEDULE_ENDED_STATES.includes(entry.head.state);
      const folded = boardScheduleEndedKept(entries.filter(ended).map((entry) => ({ entry, end: entry.head.end })),
        opened(group.id));
      const kept = new Set(folded.kept.map((one) => one.entry));
      entries = entries.filter((entry) => !ended(entry) || kept.has(entry));
      more = folded.more;
    }
    const rows = entries.sort((a, b) => boardScheduleOrder(a.head, b.head))
      .flatMap((entry) => [entry.head, ...entry.rows.filter(seen).sort(boardScheduleOrder)]).map((row) => ({
      ...row,
      // 기계가 붐비면(1분 부하 > 코어 수, 데스크의 기계 띠와 같은 판정) 앞으로 남은 예상 곁에 그렇다고만
      // 적는다 — 값은 바꾸지 않는다(코디 m-67652).
      said: loud && row.depth === 0 && ["running", "waiting"].includes(row.state)
        ? t("board.schedule.loud", "{{said}} · 부하 높음·늦어질 수 있음", { said: row.said }) : row.said,
      // A block cut at the range's start says since when (t-77628); the chart's rows keep their words.
      sub: vertical && row.depth === 0 && row.start < from
        ? [row.sub, t("board.schedule.fromAt", "{{time}}부터", { time: boardScheduleClock(row.start, now) })].filter(Boolean).join(" · ")
        : row.sub,
      from: percent(row.start),
      to: percent(Math.max(row.end, row.start)),
      done: row.doneShare ?? (row.state === "running" && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : row.state === "waiting" ? 0 : 1),
      // The vertical timetable's block (t-77628): cut at the top when it began before the range, and the
      // share of a waiting task's block that is the gate's time still ahead.
      cut: row.start < from,
      // A gate's block cannot unfold its jobs (the timetable draws heads only): it opens its telling job's log.
      act: vertical && row.depth === 0 && row.act?.fold ? (row.facts?.logJob ? { log: { job: row.facts.logJob } } : null) : row.act,
      mark: row.ride?.inherits && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : null,
      col: 0,
      cols: 1,
    }));
    const heads = boardScheduleSubColumns(rows.filter((row) => row.depth === 0), leastMs);
    return {
      id: group.id,
      label: t(group.key, group.word),
      folded: choice.folded.has(group.id),
      count: heads.length,
      waiting: heads.filter((row) => ["waiting", "held"].includes(row.state)).length,
      ended: heads.filter((row) => BOARD_SCHEDULE_ENDED_STATES.includes(row.state)).length + more,
      // The most sub-columns one cluster of the column needs: the column is at least that many narrow ones wide.
      subcols: Math.max(1, ...heads.map((row) => row.cols)),
      rows,
      more,
      moreOpened: opened(group.id),
    };
  });
  const heading = zoom === BOARD_SCHEDULE.zooms.now
    ? t("board.now.running", "지금 도는 일") : boardScheduleZoomWord(choice.zoom);
  const nowAt = percent(now);
  const axis = boardScheduleTicks(from, to, width, zoom, measure);
  const covers = (axis.room + BOARD_SCHEDULE.nowChipPx) / 2;
  // The vertical gutter's ticks are counted by its height, as the axis's by its width (t-71840's rule).
  const hours = span / 3_600_000;
  const height = hourPx > 0 ? hours * hourPx : 0;
  const tickRoom = Math.max(BOARD_SCHEDULE.calTickRoomPx, Math.ceil(tickPx + BOARD_SCHEDULE.tickGapPx));
  const gutter = vertical && height > 0
    ? boardScheduleTicks(from, to, height, { ticksMs: zoom.ticksMs, tickRoomPx: tickRoom })
    : { ticks: [], room: tickRoom };
  const chipReach = (gutter.room + BOARD_SCHEDULE.calChipPx) / 2;
  return {
    from, to, now, nowAt, groups, stall, heading, vertical, hours,
    ticks: axis.ticks.map((tick) => {
      const at = percent(tick.at);
      return { ...tick, at, covered: (Math.abs(at - nowAt) / 100) * width < covers };
    }),
    gutter: gutter.ticks.map((tick) => {
      const at = percent(tick.at);
      return { ...tick, at, covered: (Math.abs(at - nowAt) / 100) * height < chipReach };
    }),
    board: boardScheduleBoardLines(groups, now),
    // Nothing moving is expected to end after now: the timetable says so under the 「now」 line.
    aheadEmpty: latest <= now,
  };
}

/* ---- 그림 ----------------------------------------------------------------------- */

function boardScheduleElement(tag, className, text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/* 행이나 블록이나 안내판의 줄을 누르면: 게이트를 펴거나 접고, 로그를 열고, 과업은 「기록」 탭으로. */
function boardScheduleAct(view, act) {
  if (!act) return;
  if (act.fold) {
    const gate = act.fold;
    boardScheduleChoice.opened.delete(gate);
    boardScheduleChoice.opened.delete(`-${gate}`);
    boardScheduleChoice.opened.add(act.opened ? `-${gate}` : gate);
    noteBoardScheduleChoice();
    paintBoardScheduleHosts();
  } else if (act.log) openBoardScheduleLog(act.log);
  else if (act.task) openBoardScheduleTask(view, act.task);
  else if (act.history) chooseBoardTab(view, "history", false);
}

/* 행동 칩의 말: 로그가 열리는지, 기록 탭으로 가는지, 게이트가 펴지는지 — 손이 닿은 행이 말한다(t-77628). */
function boardScheduleActWord(act) {
  if (!act) return "";
  if (act.log) return t("board.schedule.actLog", "로그 →");
  if (act.task || act.history) return t("board.schedule.actRecord", "기록 →");
  if (act.fold) return act.opened ? t("board.schedule.actFold", "접기 →") : t("board.schedule.actOpen", "펴기 →");
  return "";
}

/* 그릇의 뼈대: 머리(확대와 줄의 사정), 가로 차트(눈금·「지금」 선), 안내판과 세로 시간표(t-77628), 묶음 넷,
 * 로그 칸, 빈 말. 한 번 짓고 키로 살아남는다. 묶음 상자 넷은 하나뿐이고, 확대에 따라 가로 차트와 세로
 * 시간표 사이를 옮겨 다닌다 — 한 주는 차트의 행으로, 지금 앞뒤와 하루는 시간표의 열로 선다. */
function boardScheduleSkeleton(host, view) {
  const head = boardScheduleElement("div", "board-schedule-head");
  const zoom = boardScheduleElement("div", "board-mode board-schedule-zoom");
  zoom.setAttribute("role", "group");
  for (const id of Object.keys(BOARD_SCHEDULE.zooms)) {
    const button = boardScheduleElement("button", "");
    button.type = "button";
    button.dataset.scheduleZoom = id;
    button.onclick = () => {
      if (boardScheduleChoice.zoom === id) return;
      boardScheduleChoice.zoom = id;
      noteBoardScheduleChoice();
      paintBoardScheduleHosts();
      refreshBoardSchedule({ fresh: true });
    };
    zoom.append(button);
  }
  head.append(zoom, boardScheduleElement("p", "board-schedule-note"));
  // The card of the passed draft's 「지금 도는 일」: a head (name · count · what a bar says), the axis in a
  // table head's band with the 「지금」 chip on it, then the groups.
  const chart = boardScheduleElement("div", "board-schedule-chart");
  const card = boardScheduleElement("header", "board-schedule-card-head");
  card.append(boardScheduleElement("strong", "board-schedule-title"), boardScheduleElement("span", "board-schedule-count"),
    boardScheduleElement("p", "board-schedule-legend"));
  const band = boardScheduleElement("div", "board-schedule-axis-band");
  band.setAttribute("aria-hidden", "true");
  const axis = boardScheduleElement("div", "board-schedule-axis");
  axis.__now = boardScheduleElement("span", "board-schedule-now");
  band.append(axis);
  chart.append(card, band);
  const boxes = [];
  for (const group of BOARD_SCHEDULE_GROUPS) {
    const box = boardScheduleElement("div", "board-schedule-group");
    box.dataset.group = group.id;
    const fold = boardScheduleElement("button", "board-schedule-group-head");
    fold.type = "button";
    fold.onclick = () => {
      if (boardScheduleChoice.folded.has(group.id)) boardScheduleChoice.folded.delete(group.id);
      else boardScheduleChoice.folded.add(group.id);
      noteBoardScheduleChoice();
      paintBoardScheduleHosts();
    };
    const rows = boardScheduleElement("div", "board-schedule-rows");
    // The group's folded line of finished rows (t-71840): a press shows them all, a second folds them again.
    const more = boardScheduleElement("button", "board-schedule-more");
    more.type = "button";
    more.onclick = () => {
      const key = `ended:${group.id}`;
      if (boardScheduleChoice.opened.has(key)) boardScheduleChoice.opened.delete(key);
      else boardScheduleChoice.opened.add(key);
      noteBoardScheduleChoice();
      paintBoardScheduleHosts();
    };
    box.append(fold, rows, more);
    boxes.push(box);
  }
  chart.append(...boxes);
  // The departure board and the vertical timetable (t-77628, the person's draft B).
  const cal = boardScheduleElement("section", "board-schedule-cal");
  const board = boardScheduleElement("div", "board-schedule-board");
  const boardHead = boardScheduleElement("header", "board-schedule-board-head");
  const boardNow = boardScheduleElement("strong", "board-schedule-board-now");
  const boardSum = boardScheduleElement("span", "board-schedule-board-sum");
  boardHead.append(boardNow, boardSum);
  board.append(boardHead);
  const lines = [];
  for (const id of ["lane", "gate", "waiting", "ended"]) {
    const line = boardScheduleElement("button", "board-schedule-line");
    line.type = "button";
    line.dataset.line = id;
    line.append(boardScheduleElement("span", "board-schedule-line-what"), boardScheduleElement("span", "board-schedule-line-big"),
      boardScheduleElement("span", "board-schedule-line-note"));
    line.onclick = () => boardScheduleAct(view, line.__act);
    lines.push(line);
  }
  board.append(...lines);
  const day = boardScheduleElement("div", "board-schedule-day");
  const body = boardScheduleElement("div", "board-schedule-cal-body");
  const gutter = boardScheduleElement("div", "board-schedule-gutter");
  gutter.setAttribute("aria-hidden", "true");
  const track = boardScheduleElement("div", "board-schedule-gutter-track");
  track.__now = boardScheduleElement("span", "board-schedule-now");
  gutter.append(boardScheduleElement("span", "board-schedule-gutter-head"), track);
  const aheadEmpty = boardScheduleElement("p", "board-schedule-ahead-empty");
  body.append(gutter, aheadEmpty);
  day.append(body);
  // What a block's fill, stripes, dashes and zigzag say — one line the eye can read (review P3-6).
  const calLegend = boardScheduleElement("p", "board-schedule-cal-legend");
  cal.append(board, day, calLegend);
  const log = boardScheduleElement("section", "board-schedule-log");
  log.hidden = true;
  const logHead = boardScheduleElement("header", "board-schedule-log-head");
  const close = boardScheduleElement("button", "board-desk-letter-act board-schedule-log-close");
  close.type = "button";
  close.onclick = () => {
    if (boardScheduleChoice.log) openBoardScheduleLog(boardScheduleChoice.log);
  };
  logHead.append(boardScheduleElement("strong", "board-schedule-log-title"), close);
  log.append(logHead, boardScheduleElement("pre", "board-schedule-log-lines"));
  const empty = boardScheduleElement("p", "board-schedule-empty");
  host.replaceChildren(head, chart, cal, log, empty);
  host.__parts = { head, chart, card, band, axis, boxes, cal, boardNow, boardSum, lines, day, body, gutter, track, aheadEmpty, calLegend, log, empty };
}

/* 행 하나: 단추 하나에 이름 · 막대 · 말. 막대의 자리는 커스텀 속성 셋(`--from`·`--to`·
 * `--done`)이고, 워커 과업은 넘김·검증의 자리 둘을 더 든다. */
function boardScheduleRowNode(view) {
  const row = boardScheduleElement("button", "board-schedule-row");
  row.type = "button";
  row.append(boardScheduleElement("span", "board-schedule-label"), boardScheduleElement("span", "board-schedule-bar"),
    boardScheduleElement("span", "board-schedule-said"));
  row.onclick = () => boardScheduleAct(view, row.__row?.act);
  return row;
}

/* 블록 하나(t-77628): 단추 하나에 이름(`<b>`, id는 가상 요소) — 요소 둘. 게이트의 블록만 단계 칸 줄(`<i>`)을
 * 하나 더 든다. 둘째 줄·셋째 줄·행동 칩은 `data-*`를 읽는 가상 요소다. 자리는 커스텀 속성(`--from`·`--to`:
 * 범위의 퍼센트, `--col`·`--cols`: 부열)이고 CSS가 시간당 높이로 푼다. */
function boardScheduleBlockNode(view, steps) {
  const block = boardScheduleElement("button", "board-schedule-block");
  block.type = "button";
  block.append(boardScheduleElement("b", "board-schedule-label"));
  if (steps) block.append(boardScheduleElement("i", "board-schedule-steps"));
  block.onclick = () => boardScheduleAct(view, block.__row?.act);
  return block;
}

/* 행의 글은 있는 글 노드의 `data`만 바꾼다. `textContent`로 노드를 갈아 끼우면 창
 * 스타일시트의 구조 선택자(`:has()` 32개, `:empty` 55개)가 다시 따져져, 행 하나에 효율
 * 코어에서 2 ms가 들었다(t-67482). 처음 쓰는 노드는 `writeTextContent`가 짓는다. */
function writeBoardScheduleText(node, value) {
  const text = node.firstChild;
  if (text?.nodeType === Node.TEXT_NODE && !text.nextSibling) {
    if (text.data !== value) text.data = value;
  } else {
    writeTextContent(node, value);
  }
}

function paintBoardScheduleRow(node, row) {
  node.__row = row;
  const from = row.from.toFixed(2);
  const to = row.to.toFixed(2);
  const done = (row.done * 100).toFixed(1);
  // What the row was last painted with: a row the beat did not move reads nothing back from
  // the DOM (twelve reads a row, fifty rows a beat, were most of a moved beat's cost after the
  // words were remembered — t-67482).
  const painted = [row.key, row.state, row.basis, row.depth, row.tip, row.label, row.said, row.sub, row.eta, row.etaSub,
    row.nowLabel, row.aheadLabel, Boolean(row.act?.fold), Boolean(row.opened), Boolean(row.paused), Boolean(row.late),
    from, to, done].join("\u0000");
  if (node.__painted === painted) return;
  node.__painted = painted;
  writeAttribute(node, "data-key", row.key);
  writeAttribute(node, "data-bar-state", row.state);
  writeAttribute(node, "data-basis", row.basis);
  writeAttribute(node, "data-depth", String(row.depth));
  writeAttribute(node, "data-tip", row.tip);
  writeAttribute(node, "data-said", row.said);
  writeAttribute(node, "aria-label", [row.label, row.sub, row.said].filter(Boolean).join(" · "));
  if (row.act?.fold) writeAttribute(node, "aria-expanded", String(Boolean(row.opened)));
  else if (node.hasAttribute("aria-expanded")) node.removeAttribute("aria-expanded");
  writeClass(node, "is-paused", Boolean(row.paused));
  writeClass(node, "is-late", Boolean(row.late));
  // The second lines and the bar's two names are the cells' generated content (`data-*`), not
  // elements: a row stays four elements however much it says.
  const [label, bar, said] = node.children;
  writeBoardScheduleText(label, row.label);
  writeAttribute(label, "data-sub", row.sub);
  writeBoardScheduleText(said, row.eta);
  writeAttribute(said, "data-sub", row.etaSub);
  writeCustomProperty(bar, "--from", from);
  writeCustomProperty(bar, "--to", to);
  writeCustomProperty(bar, "--done", done);
  writeAttribute(bar, "data-now-label", row.nowLabel);
  writeAttribute(bar, "data-ahead-label", row.aheadLabel);
}

/* 게이트 블록의 단계 칸 줄: 칸 하나가 10 px, 틈 3 px — 요소가 아니라 그라디언트 하나다. 상태는 칸의 색이고
 * 블록의 둘째 줄이 같은 것을 수로 말한다. */
const BOARD_SCHEDULE_STEP_INKS = Object.freeze({
  done: "var(--signal-ready)", failed: "var(--signal-halt)", running: "var(--status-dot-run)",
  waiting: "var(--edge-mark)", held: "var(--edge-mark)", idle: "var(--edge-mark)",
});

function boardScheduleStepsPaint(steps) {
  const cell = BOARD_SCHEDULE.stepCellPx;
  const gap = BOARD_SCHEDULE.stepGapPx;
  const stops = steps.map((state, index) => {
    const ink = BOARD_SCHEDULE_STEP_INKS[state] ?? BOARD_SCHEDULE_STEP_INKS.idle;
    const start = index * (cell + gap);
    return `transparent ${start}px, ${ink} ${start}px, ${ink} ${start + cell}px, transparent ${start + cell}px`;
  });
  return { image: `linear-gradient(to right, ${stops.join(", ")})`, width: `${steps.length * (cell + gap) - gap}px` };
}

function paintBoardScheduleBlock(node, row) {
  node.__row = row;
  const from = row.from.toFixed(2);
  const to = row.to.toFixed(2);
  const done = (row.done * 100).toFixed(1);
  const mark = row.mark === null || row.mark === undefined ? "" : (row.mark * 100).toFixed(1);
  const steps = Array.isArray(row.steps) ? row.steps.join(",") : "";
  const actWord = boardScheduleActWord(row.act);
  const painted = [row.key, row.state, row.basis, row.tip, row.label, row.said, row.sub, row.why, row.eta, row.etaSub,
    row.nowLabel, row.aheadLabel, Boolean(row.act?.fold), Boolean(row.opened), Boolean(row.paused), Boolean(row.late),
    Boolean(row.cut), row.ride?.state ?? "", from, to, done, mark, row.col, row.cols, steps, actWord].join("\u0000");
  if (node.__painted === painted) return;
  node.__painted = painted;
  writeAttribute(node, "data-key", row.key);
  writeAttribute(node, "data-bar-state", row.state);
  writeAttribute(node, "data-basis", row.basis);
  writeAttribute(node, "data-depth", "0");
  writeAttribute(node, "data-tip", row.tip);
  writeAttribute(node, "data-said", row.said);
  writeAttribute(node, "data-why", row.why ?? "");
  writeAttribute(node, "data-eta", row.eta);
  writeAttribute(node, "data-eta-sub", row.etaSub);
  writeAttribute(node, "data-now-label", row.nowLabel);
  writeAttribute(node, "data-ahead-label", row.aheadLabel);
  writeAttribute(node, "data-steps", steps);
  writeAttribute(node, "data-act", actWord);
  writeAttribute(node, "aria-label", [row.label, row.sub, row.said].filter(Boolean).join(" · "));
  if (row.act?.fold) writeAttribute(node, "aria-expanded", String(Boolean(row.opened)));
  else if (node.hasAttribute("aria-expanded")) node.removeAttribute("aria-expanded");
  writeClass(node, "is-paused", Boolean(row.paused));
  writeClass(node, "is-late", Boolean(row.late));
  writeClass(node, "is-cut", Boolean(row.cut));
  writeClass(node, "is-inherited", mark !== "");
  writeAttribute(node, "data-ride-state", row.ride?.state ?? "");
  const [label, strip] = node.children;
  writeBoardScheduleText(label, row.title ?? row.label);
  writeAttribute(label, "data-id", row.id ?? "");
  writeAttribute(label, "data-sub", row.sub);
  writeCustomProperty(node, "--from", from);
  writeCustomProperty(node, "--to", to);
  writeCustomProperty(node, "--done", done);
  writeCustomProperty(node, "--mark", mark === "" ? "0" : mark);
  writeCustomProperty(node, "--col", String(row.col ?? 0));
  writeCustomProperty(node, "--cols", String(row.cols ?? 1));
  if (strip && Array.isArray(row.steps)) {
    const paint = boardScheduleStepsPaint(row.steps);
    writeCustomProperty(strip, "--schedule-steps", paint.image);
    writeCustomProperty(strip, "--schedule-steps-width", paint.width);
  }
}

/* 눈금 하나하나를 축이나 홈통에 세운다: 글, 자리(`--at`), 칩 밑에 들면 글만 감춤. 안 그리는 쪽은 비운다. */
function paintBoardScheduleTicks(axis, ticks) {
  const held = [...axis.children].filter((node) => node !== axis.__now);
  reconcileElementOrder(axis, [...ticks.map((tick, at) => {
    const node = held[at] ?? boardScheduleElement("span", "board-schedule-tick");
    writeTextContent(node, tick.label);
    writeCustomProperty(node, "--at", tick.at.toFixed(2));
    writeClass(node, "is-covered", tick.covered);
    return node;
  }), axis.__now]);
}

/* 눈금마다 선 하나: 가로 차트는 트랙의 왼→오른쪽, 세로 시간표는 위→아래. 카드(차트)나 몸통(시간표)에 둔다. */
function boardScheduleGridPaint(ticks, direction) {
  if (ticks.length === 0) return "none";
  return `linear-gradient(to ${direction}, ${ticks.map((tick) => {
    const at = `${tick.at.toFixed(2)}%`;
    return `transparent ${at}, var(--edge-rule) ${at}, var(--edge-rule) calc(${at} + var(--rule-width)), transparent calc(${at} + var(--rule-width))`;
  }).join(", ")})`;
}

/* 안내판 넉 줄을 쓴다(t-77628): 줄마다 무엇·큰 글·낱말·비고·색 띠·행동 칩. */
function paintBoardScheduleBoard(parts, board) {
  writeTextContent(parts.boardNow, board.head.now);
  writeTextContent(parts.boardSum, board.head.sum);
  for (const [index, line] of board.lines.entries()) {
    const node = parts.lines[index];
    node.__act = line.act;
    const painted = [line.tone, line.what, line.whatSub, line.big, line.word, line.note, line.actWord].join("\u0000");
    if (node.__painted === painted) continue;
    node.__painted = painted;
    const [what, big, note] = node.children;
    writeBoardScheduleText(what, line.what);
    writeAttribute(what, "data-sub", line.whatSub);
    writeBoardScheduleText(big, line.big);
    writeAttribute(big, "data-word", line.word);
    writeBoardScheduleText(note, line.note);
    writeAttribute(node, "data-tone", line.tone);
    writeAttribute(node, "data-act", line.actWord);
    writeAttribute(node, "aria-label", [line.what, line.big, line.word, line.note].filter(Boolean).join(" · "));
  }
}

/* 그릇 하나를 그린다. 답·데스크·분·고른 것·언어·폭이 지난번과 같으면 아무것도
 * 쓰지 않는다 — 조용한 박자는 DOM을 건드리지 않는다. 막대와 「지금」 선은 분 단위의
 * 지금에 선다: 같은 분 안의 박자는 바뀐 행만 고쳐 쓰고 막대 하나 옮기지 않는다
 * (t-67482: 밀리초의 지금으로는 박자마다 막대 몇 개가 반올림 경계를 넘어 다시 쓰였다).
 * 눈금 수는 축의 폭으로 센다. 넓은 그릇의 축은 이름 칸과 말 칸을 뺀 자리라, 그릇 폭으로
 * 세면 1440 px 창의 축 약 450 px에 세 시간 눈금 여덟이 붙어 섰다(t-67482).
 * 한 주는 가로 차트에, 지금 앞뒤와 하루는 안내판과 세로 시간표에 그린다(t-77628): 묶음 상자
 * 넷을 그쪽으로 옮기고, 행이나 블록을 키로 맞춘다 — 다른 종류의 노드는 다시 짓는다. */
function paintBoardScheduleHost(host, view, now) {
  if (!boardScheduleBuilt.has(host)) {
    boardScheduleSkeleton(host, view);
    boardScheduleBuilt.add(host);
  }
  const parts = host.__parts;
  const vertical = boardScheduleVertical(boardScheduleChoice.zoom);
  const width = vertical ? host.clientWidth : parts.axis.clientWidth;
  const desk = typeof deskLedger === "undefined" ? null : deskLedger;
  // The machine strip's own reading (`machine_load`, shell-board.js): read, never asked a second time.
  const machine = typeof deskMachine === "undefined" ? null : deskMachine;
  const minute = Math.floor(now / BOARD_SCHEDULE.clockMs) * BOARD_SCHEDULE.clockMs;
  const said = [boardScheduleAnswer, desk, machine, minute, boardScheduleChoiceRevision,
    document.documentElement.lang, width, vertical];
  const before = boardSchedulePainted.get(host);
  if (before && before.every((one, at) => one === said[at])) return;
  boardSchedulePainted.set(host, said);
  // The type the ticks are set in, and the hour's height, are read only when the schedule is drawn again — a
  // look that changes nothing (the same answer, minute and width) does not ask the page's style.
  const styled = vertical ? getComputedStyle(host) : null;
  const metrics = vertical ? {
    hourPx: parseFloat(styled.getPropertyValue("--schedule-hour")) || 0,
    tickPx: boardScheduleTickHeight(parts.track),
    leastPx: parseFloat(styled.getPropertyValue("--schedule-block-least")) || 0,
  } : null;
  const model = boardScheduleModel(boardScheduleAnswer, desk?.tasks, minute, boardScheduleChoice, width,
    machine?.load?.loud === true, vertical ? null : boardScheduleTickMeasure(boardScheduleTickFont(parts.axis)), metrics);
  paintBoardScheduleHead(parts.head, model);
  writeAttribute(host, "data-schedule-zoom", boardScheduleChoice.zoom);
  writeHidden(parts.chart, vertical);
  writeHidden(parts.cal, !vertical);
  const nowAt = model.nowAt.toFixed(2);
  if (vertical) {
    paintBoardScheduleBoard(parts, model.board);
    paintBoardScheduleTicks(parts.axis, []);
    paintBoardScheduleTicks(parts.track, model.gutter);
    writeTextContent(parts.track.__now, model.board.head.now);
    writeCustomProperty(parts.body, "--now", nowAt);
    writeCustomProperty(parts.body, "--schedule-hours", model.hours.toFixed(3));
    writeCustomProperty(parts.body, "--schedule-grid", boardScheduleGridPaint(model.gutter, "bottom"));
    const legendBlocks = t("board.schedule.legendBlocks", "칠함 = 지난·끝남 · 빗금 = 앞으로(지난 소요로 잰 예상) · 점선 = 근거 없음 · 톱니 = 범위 앞에서 시작");
    writeAttribute(parts.gutter, "data-tip", legendBlocks);
    writeTextContent(parts.calLegend, legendBlocks);
    writeTextContent(parts.aheadEmpty, t("board.schedule.aheadEmpty", "앞으로 잡힌 일 없음"));
    writeHidden(parts.aheadEmpty, !model.aheadEmpty);
    reconcileElementOrder(parts.chart, [parts.card, parts.band]);
    reconcileElementOrder(parts.body, [parts.gutter, ...parts.boxes, parts.aheadEmpty]);
  } else {
    const [named, count, legend] = parts.card.children;
    writeTextContent(named, model.heading);
    writeTextContent(count, String(model.groups.reduce((sum, group) => sum + group.count, 0)));
    writeTextContent(legend, t("board.schedule.legend", "막대 끝 = 지난 소요로 잰 예상 · 빗금 = 아직 안 온 구간 · 점선 = 근거 없음"));
    paintBoardScheduleTicks(parts.track, []);
    paintBoardScheduleTicks(parts.axis, model.ticks);
    writeTextContent(parts.axis.__now, t("board.schedule.nowLine", "지금"));
    writeCustomProperty(parts.chart, "--now", nowAt);
    // The track's hour lines (the draft's quarter rules): one line on each tick, drawn by every row's track.
    writeCustomProperty(parts.chart, "--schedule-grid", boardScheduleGridPaint(model.ticks, "right"));
    reconcileElementOrder(parts.body, [parts.gutter, parts.aheadEmpty]);
    reconcileElementOrder(parts.chart, [parts.card, parts.band, ...parts.boxes]);
  }
  const queueWord = t(BOARD_RUN_STATES.queue.key, BOARD_RUN_STATES.queue.word);
  const doneWord = t(BOARD_RUN_STATES.done.key, BOARD_RUN_STATES.done.word);
  let shown = 0;
  for (const [index, group] of model.groups.entries()) {
    const box = parts.boxes[index];
    const [fold, list, more] = box.children;
    writeBoardScheduleText(fold, group.label);
    // The column's head counts its blocks (t-77628): 「7 · 차례 기다림 4 · 끝남 3」 — a zero is left out —, or that
    // the range holds none. The column is at least as wide as its widest cluster of blocks standing side by side.
    writeAttribute(fold, "data-sub", !vertical ? ""
      : group.count === 0 && group.more === 0
        ? t("board.schedule.notInRange", "이 범위에 없음")
        : [String(group.count), group.waiting > 0 ? `${queueWord} ${group.waiting}` : "", group.ended > 0 ? `${doneWord} ${group.ended}` : ""]
          .filter(Boolean).join(" · "));
    writeCustomProperty(box, "--schedule-subcols", String(vertical ? group.subcols : 1));
    writeAttribute(fold, "aria-expanded", String(!group.folded));
    // A column stays when it is empty — the timetable's four columns are its grammar; a chart's group folds away.
    writeHidden(box, !vertical && group.rows.length === 0 && group.more === 0);
    const kind = vertical ? "board-schedule-block" : "board-schedule-row";
    const held = new Map([...list.children].filter((node) => node.classList.contains(kind)).map((node) => [node.dataset.key, node]));
    const rows = group.folded ? [] : vertical ? group.rows.filter((row) => row.depth === 0) : group.rows;
    reconcileElementOrder(list, rows.map((row) => {
      if (vertical) {
        const steps = Array.isArray(row.steps);
        let node = held.get(row.key);
        if (node && Boolean(node.children[1]) !== steps) node = null;
        node ??= boardScheduleBlockNode(view, steps);
        paintBoardScheduleBlock(node, row);
        return node;
      }
      const node = held.get(row.key) ?? boardScheduleRowNode(view);
      paintBoardScheduleRow(node, row);
      return node;
    }));
    writeHidden(list, rows.length === 0 && !vertical);
    const moreWords = group.more === 0 || group.folded ? ""
      : group.moreOpened
        ? t("board.schedule.lessEnded", "끝난 {{count}}개 접기", { count: group.more })
        : t("board.schedule.moreEnded", "+{{count}} 끝남", { count: group.more });
    writeTextContent(more, moreWords);
    writeAttribute(more, "aria-expanded", String(group.moreOpened));
    writeHidden(more, moreWords === "");
    shown += group.rows.length;
  }
  writeTextContent(parts.empty, shown > 0 ? ""
    : boardScheduleAnswer ? t("board.schedule.empty", "이 범위에 일정이 없어요") : t("board.schedule.reading", "읽는 중…"));
  writeHidden(parts.empty, shown !== 0);
  paintBoardScheduleLog(parts.log);
}

/* 확대 하나의 말 — 단추와 카드 머리가 같은 말을 쓴다. 지금 앞뒤의 카드는 시안 v2의 이름 그대로
 * 「지금 도는 일」이다. */
function boardScheduleZoomWord(id) {
  return id === "week" ? t("board.schedule.week", "한 주")
    : id === "day" ? t("board.schedule.day", "하루") : t("board.schedule.zoomNow", "지금 앞뒤");
}

/* 빌드 줄이 없는 프로젝트의 한 문장 (t-83513) — 일정의 머리와 「지금」 탭의 빌드 줄 카드가 같은 말을
 * 쓴다. 줄을 찾은 자리는 백엔드가 프로젝트의 워크스페이스 폴더 아래 한 곳(`LINE_FOLDER`)에서 찾은
 * 그 폴더다. */
function boardNoLineWords() {
  return t("board.schedule.noLine", "이 프로젝트엔 빌드 줄이 없습니다");
}

/* 머리: 확대 셋과, 빌드 줄이 왜 서 있는지 한 줄. */
function paintBoardScheduleHead(head, model) {
  const [zoom, note] = head.children;
  writeAttribute(zoom, "aria-label", t("board.schedule.zoom", "확대"));
  for (const button of zoom.children) {
    const id = button.dataset.scheduleZoom;
    writeTextContent(button, boardScheduleZoomWord(id));
    writeAttribute(button, "aria-pressed", String(boardScheduleChoice.zoom === id));
  }
  const line = boardScheduleAnswer?.line ?? {};
  const words = [
    line.found === false ? boardNoLineWords() : "",
    line.found === false && line.folder ? line.folder : "",
    line.paused
      ? line.pause_note
        ? t("board.schedule.linePaused", "줄 멈춤 · {{note}}", { note: line.pause_note })
        : t("board.schedule.linePausedPlain", "줄 멈춤")
      : "",
    line.disk_low ? t("board.schedule.diskLow", "디스크가 하한 아래라 줄이 기다림") : "",
    line.quiet_wanted ? t("board.schedule.quiet", "측정이 조용한 기계를 기다림") : "",
  ].filter(Boolean).join(" · ");
  writeTextContent(note, words);
  writeHidden(note, words === "");
}

/* 연 로그: 이름, 닫기, 끝 줄들. 읽는 중이면 그렇다고, 없으면 그렇다고. */
function paintBoardScheduleLog(log) {
  const asked = boardScheduleChoice.log;
  writeHidden(log, !asked);
  if (!asked) return;
  const [head, lines] = log.children;
  const [title, close] = head.children;
  writeTextContent(title, t("board.schedule.log", "로그 · {{name}}", { name: asked.job ?? asked.lane ?? "" }));
  writeTextContent(close, t("board.schedule.logClose", "닫기"));
  const held = boardScheduleLog;
  const text = held?.reading
    ? t("board.schedule.logReading", "읽는 중…")
    : held?.missing || !Array.isArray(held?.lines)
      ? t("board.schedule.logMissing", "로그를 읽을 수 없어요")
      : held.lines.join("\n");
  writeTextContent(lines, text);
}

/* 워커 과업의 행을 누르면 「기록」 탭의 그 과업으로 간다 — 과업의 넘김·검증·비용이 서는
 * 곳이다: 작업 목록에 그 과업이 있으면 고르고, 없으면 데스크의 과업 흐름에서 그 단계를
 * 펴고 그 행을 보인다. */
function openBoardScheduleTask(view, task) {
  chooseBoardTab(view, "history", false);
  const key = JSON.stringify([task.run, task.id]);
  const group = taskBoardModels.get(view)?.groups.find((one) => one.key === key);
  if (group) {
    selectTaskBoardMember(view, group.lead.key);
    return;
  }
  deskChoice.stage = task.stage;
  paintCoordinatorDesk(view);
  view.querySelector(`.board-desk-task[data-task="${CSS.escape(`${task.run}/${task.id}`)}"]`)
    ?.scrollIntoView({ block: "center" });
}

/* 보이는 동안의 박자. 보드가 숨거나 다른 탭으로 가면 멈춘다. */
const boardSchedulePoller = idlePoller({
  wanted: () => boardScheduleHosts().length > 0,
  every: BOARD_SCHEDULE.everyMs,
  tick: () => refreshBoardSchedule(),
  onResume: () => refreshBoardSchedule({ fresh: true }),
});
