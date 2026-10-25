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
 * 가운데 「일정」의 블록이다(shell-board-now.js의 `BOARD_TABS`, t-67607의 시안 B). */

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
 * 기다리는 일감이 있는 게이트는 처음부터 펴져 있다. */
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
    const end = Math.max(...placed.map((row) => row.end));
    // A step's last job speaks for it: a step that failed and passed on its rerun passed.
    const latest = new Map();
    for (const job of members) {
      const step = job.kind ?? job.name;
      const held = latest.get(step);
      if (!held || (Number(job.queued_ms) || 0) >= (Number(held.queued_ms) || 0)) latest.set(step, job);
    }
    const steps = [...latest.values()];
    const count = (states) => steps.filter((job) => states.includes(job.state)).length;
    const open = count(["running", "waiting", "held"]);
    const failed = count(["failed", "capped", "refused"]);
    const parts = [...new Set(members.map((job) => job.part).filter(Boolean))].sort();
    const state = count(["running"]) > 0 ? "running" : open > 0 ? "waiting" : failed > 0 ? "failed" : "done";
    const said = state === "running"
      ? t("board.schedule.gateRunning", "{{done}}/{{total}} · {{time}} 남음", {
        done: steps.length - open, total: steps.length, time: boardScheduleSpan(end - now),
      })
      : state === "waiting"
        ? stall
          ? t("board.schedule.gateWaitingUnknown", "대기 {{count}}개 · 시작 모름", { count: open })
          : t("board.schedule.gateWaiting", "대기 {{count}}개 · {{time}} 시작 예상", {
            count: open, time: boardScheduleClock(start, now),
          })
        : state === "failed"
          ? t("board.schedule.gateFailed", "실패 {{count}}개 · {{time}}", { count: failed, time: boardScheduleSpan(end - start) })
          : t("board.schedule.gatePassed", "통과 {{count}}개 · {{time}}", {
            count: steps.length, time: boardScheduleSpan(end - start),
          });
    const opened = choice.opened.has(gate) || (open > 0 && !choice.opened.has(`-${gate}`));
    // The bar's two names (the passed draft's 「12개 초록」): what ended, or the step running and the steps after it.
    const kinds = (states) => steps.filter((job) => states.includes(job.state)).map((job) => job.kind ?? job.name);
    const tally = state === "done"
      ? t("board.schedule.gateDone", "{{done}}/{{total}} 초록", { done: steps.length, total: steps.length })
      : state === "failed"
        ? t("board.schedule.gateFailedCount", "실패 {{count}}개", { count: failed })
        : `${steps.length - open}/${steps.length}`;
    entries.push({
      head: boardScheduleRow({
        key: `gate:${gate}`, label: t("board.schedule.gateName", "{{gate}} 결합 게이트", { gate }),
        tip: [parts.join("·"), ...members.map((job) => job.name)].filter(Boolean).join(" · "), start, end, state,
        basis: placed.some((row) => row.basis === "cap") ? "cap" : "history", said,
        sub: boardScheduleSub(state, tally, state === "done" || state === "failed" ? boardScheduleClock(end, now) : ""),
        eta: boardScheduleEndWord(state, end, now, state === "waiting" && Boolean(stall)),
        etaSub: state === "running"
          ? t("board.schedule.left", "{{time}} 남음", { time: boardScheduleSpan(end - now) })
          : state === "waiting"
            ? stall
              ? t("board.schedule.startUnknown", "시작 모름")
              : t("board.schedule.startsAt", "{{time}} 시작 예상", { time: boardScheduleClock(start, now) })
            : t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - start) }),
        nowLabel: state === "done"
          ? t("board.schedule.gateBarPassed", "{{count}}개 초록", { count: steps.length })
          : state === "failed"
            ? t("board.schedule.gateBarFailed", "{{count}}개 실패", { count: failed })
            : kinds(["running"]).join(" · "),
        aheadLabel: kinds(["waiting", "held"]).join(" · "),
        act: { fold: gate, opened }, opened, paused: Boolean(stall) && state === "waiting",
      }),
      rows: opened ? placed : [],
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
 * 일정의 답이 git에서 읽어 온 착지·발행이다(과업 id마다). */
function boardScheduleTaskRows(tasks, now, landings, from, opened) {
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
  return { rows: [...kept, ...moving].map((one) => boardScheduleTaskRow(one, now)), more };
}

function boardScheduleTaskRow({ task, landing, start, end, known }, now) {
  const times = task.times;
  const stage = DESK_STAGES.find((one) => one.id === task.stage);
  const word = stage ? t(stage.key, stage.word) : String(task.stage);
  const since = Math.max(Number(times.verified_ms) || 0, Number(times.handed_ms) || 0, start);
  const state = BOARD_SCHEDULE_TASK_STATES[task.stage];
  const said = state === "running"
    ? t("board.schedule.taskRunning", "{{stage}} · {{time}}째", { stage: word, time: boardScheduleSpan(now - start) })
    : state === "waiting"
      ? t("board.schedule.taskWaiting", "{{stage}} · 기다린 지 {{time}}", { stage: word, time: boardScheduleSpan(now - since) })
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
  return boardScheduleRow({
    key: `task:${task.run}/${task.id}`, label: `${task.id} ${task.title}`,
    tip: [`${task.id} · ${task.title}`, task.run, t("board.schedule.tipTask",
      "시작 {{start}} · 넘김 {{handed}} · 검증 {{verified}} · 착지 {{landed}} · 발행 {{released}}", {
        start: clock(times.started_ms), handed: clock(times.handed_ms), verified: clock(times.verified_ms),
        landed: clock(landing?.landed_ms ?? times.landed_ms), released: clock(landing?.released_ms),
      })].filter(Boolean).join(" · "),
    start, end, state, said,
    sub: boardScheduleSub(state, carrier?.worker, word, over && known ? boardScheduleClock(end, now) : ""),
    // The end cell speaks the way the Now tab's does: how long it has run, 「대기」, or that it ended.
    eta: state === "running"
      ? t("board.schedule.since", "{{time}}째", { time: boardScheduleSpan(now - start) })
      : state === "waiting"
        ? t("board.now.stepAhead", "대기")
        : boardScheduleEndWord(state, end, now, false),
    etaSub: state === "running"
      ? ""
      : state === "waiting"
        ? t("board.schedule.waited", "기다린 지 {{time}}", { time: boardScheduleSpan(now - since) })
        : known
          ? t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - start) })
          : t("board.schedule.lost", "끝 기록 없음"),
    // A finished bar carries its stage; one waiting is filled to its hand-in and dashed from there, named by its stage.
    nowLabel: over ? word : "",
    aheadLabel: state === "waiting" ? word : "",
    doneShare: state === "waiting" && handed !== null && end > start
      ? Math.min(1, Math.max(0, (handed - start) / (end - start))) : null,
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

/* 그릴 것 한 벌: 보이는 범위, 눈금, 묶음 넷과 그 행들. 백엔드의 답과 데스크의 과업,
 * 지금, 고른 것, 축의 폭에서 DOM 없이 짓는다 — 장면 시험이 이것을 그대로 읽는다. */
function boardScheduleModel(answer, tasks, now, choice, width, loud = false, measure = null) {
  const zoom = BOARD_SCHEDULE.zooms[choice.zoom] ?? BOARD_SCHEDULE.zooms.now;
  const jobs = Array.isArray(answer?.jobs) ? answer.jobs : [];
  const lanes = Array.isArray(answer?.lanes) ? answer.lanes : [];
  const facts = answer?.line ?? {};
  const runnerLanes = Array.isArray(facts.runner_lanes) ? facts.runner_lanes : [];
  const stall = boardScheduleStall(facts, runnerLanes);
  const plan = boardScheduleQueuePlan(jobs, runnerLanes, now);
  const anchor = boardScheduleAnchor(zoom, now);
  const from = anchor - zoom.beforeMs;
  const opened = (group) => choice.opened.has(`ended:${group}`);
  // The line and the workers fold their finished rows before they build them; lanes and gates are few.
  const line = boardScheduleLineRows(jobs, plan, now, stall, from, opened("line"));
  const workers = boardScheduleTaskRows(tasks, now, answer?.landings, from, opened("workers"));
  // Each group as entries: a row, and the rows standing under it (a gate's jobs).
  const alone = (row) => ({ head: row, rows: [] });
  const built = {
    lanes: { entries: boardScheduleLaneRows(lanes, now).map(alone), more: null },
    gates: { entries: boardScheduleGateRows(jobs, plan, now, choice, stall), more: null },
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
      from: percent(row.start),
      to: percent(Math.max(row.end, row.start)),
      done: row.doneShare ?? (row.state === "running" && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : row.state === "waiting" ? 0 : 1),
    }));
    return {
      id: group.id,
      label: t(group.key, group.word),
      folded: choice.folded.has(group.id),
      count: rows.filter((row) => row.depth === 0).length,
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
  return { from, to, now, nowAt, ticks: axis.ticks.map((tick) => {
    const at = percent(tick.at);
    return { ...tick, at, covered: (Math.abs(at - nowAt) / 100) * width < covers };
  }), groups, stall, heading };
}

/* ---- 그림 ----------------------------------------------------------------------- */

function boardScheduleElement(tag, className, text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/* 그릇의 뼈대: 머리(확대와 줄의 사정), 차트(눈금·「지금」 선·묶음 넷), 로그 칸, 빈 말.
 * 한 번 짓고 키로 살아남는다. */
function boardScheduleSkeleton(host) {
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
    chart.append(box);
  }
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
  host.replaceChildren(head, chart, log, boardScheduleElement("p", "board-schedule-empty"));
}

/* 행 하나: 단추 하나에 이름 · 막대 · 말. 막대의 자리는 커스텀 속성 셋(`--from`·`--to`·
 * `--done`)이고, 워커 과업은 넘김·검증의 자리 둘을 더 든다. */
function boardScheduleRowNode(view) {
  const row = boardScheduleElement("button", "board-schedule-row");
  row.type = "button";
  row.append(boardScheduleElement("span", "board-schedule-label"), boardScheduleElement("span", "board-schedule-bar"),
    boardScheduleElement("span", "board-schedule-said"));
  row.onclick = () => {
    const act = row.__row?.act;
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
  };
  return row;
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

/* 그릇 하나를 그린다. 답·데스크·분·고른 것·언어·축의 폭이 지난번과 같으면 아무것도
 * 쓰지 않는다 — 조용한 박자는 DOM을 건드리지 않는다. 막대와 「지금」 선은 분 단위의
 * 지금에 선다: 같은 분 안의 박자는 바뀐 행만 고쳐 쓰고 막대 하나 옮기지 않는다
 * (t-67482: 밀리초의 지금으로는 박자마다 막대 몇 개가 반올림 경계를 넘어 다시 쓰였다).
 * 눈금 수는 축의 폭으로 센다. 넓은 그릇의 축은 이름 칸과 말 칸을 뺀 자리라, 그릇 폭으로
 * 세면 1440 px 창의 축 약 450 px에 세 시간 눈금 여덟이 붙어 섰다(t-67482). */
function paintBoardScheduleHost(host, view, now) {
  if (!boardScheduleBuilt.has(host)) {
    boardScheduleSkeleton(host);
    boardScheduleBuilt.add(host);
  }
  const [head, chart, log, empty] = host.children;
  const [card, band, ...boxes] = chart.children;
  const axis = band.firstElementChild;
  const width = axis.clientWidth;
  const desk = typeof deskLedger === "undefined" ? null : deskLedger;
  // The machine strip's own reading (`machine_load`, shell-board.js): read, never asked a second time.
  const machine = typeof deskMachine === "undefined" ? null : deskMachine;
  const minute = Math.floor(now / BOARD_SCHEDULE.clockMs) * BOARD_SCHEDULE.clockMs;
  const said = [boardScheduleAnswer, desk, machine, minute, boardScheduleChoiceRevision,
    document.documentElement.lang, width];
  const before = boardSchedulePainted.get(host);
  if (before && before.every((one, at) => one === said[at])) return;
  boardSchedulePainted.set(host, said);
  // The type the ticks are set in is read only when the schedule is drawn again — a look that changes nothing
  // (the same answer, minute and width) does not ask the page's style.
  const model = boardScheduleModel(boardScheduleAnswer, desk?.tasks, minute, boardScheduleChoice, width,
    machine?.load?.loud === true, boardScheduleTickMeasure(boardScheduleTickFont(axis)));
  paintBoardScheduleHead(head, model);
  const [named, count, legend] = card.children;
  writeTextContent(named, model.heading);
  writeTextContent(count, String(model.groups.reduce((sum, group) => sum + group.count, 0)));
  writeTextContent(legend, t("board.schedule.legend", "막대 끝 = 지난 소요로 잰 예상 · 빗금 = 아직 안 온 구간 · 점선 = 근거 없음"));
  const ticks = [...axis.children].filter((node) => node !== axis.__now);
  reconcileElementOrder(axis, [...model.ticks.map((tick, at) => {
    const node = ticks[at] ?? boardScheduleElement("span", "board-schedule-tick");
    writeTextContent(node, tick.label);
    writeCustomProperty(node, "--at", tick.at.toFixed(2));
    writeClass(node, "is-covered", tick.covered);
    return node;
  }), axis.__now]);
  writeTextContent(axis.__now, t("board.schedule.nowLine", "지금"));
  writeCustomProperty(chart, "--now", model.nowAt.toFixed(2));
  // The track's hour lines (the draft's quarter rules): one line on each tick, drawn by every row's track.
  writeCustomProperty(chart, "--schedule-grid", model.ticks.length === 0 ? "none" : `linear-gradient(to right, ${model.ticks
    .map((tick) => {
      const at = `${tick.at.toFixed(2)}%`;
      return `transparent ${at}, var(--edge-rule) ${at}, var(--edge-rule) calc(${at} + var(--rule-width)), transparent calc(${at} + var(--rule-width))`;
    }).join(", ")})`);
  let shown = 0;
  for (const [index, group] of model.groups.entries()) {
    const box = boxes[index];
    const [fold, list, more] = box.children;
    writeBoardScheduleText(fold, group.label);
    writeAttribute(fold, "aria-expanded", String(!group.folded));
    writeHidden(box, group.rows.length === 0 && group.more === 0);
    const held = new Map([...list.children].map((node) => [node.dataset.key, node]));
    const rows = group.folded ? [] : group.rows;
    reconcileElementOrder(list, rows.map((row) => {
      const node = held.get(row.key) ?? boardScheduleRowNode(view);
      paintBoardScheduleRow(node, row);
      return node;
    }));
    writeHidden(list, rows.length === 0);
    const moreWords = group.more === 0 || group.folded ? ""
      : group.moreOpened
        ? t("board.schedule.lessEnded", "끝난 {{count}}개 접기", { count: group.more })
        : t("board.schedule.moreEnded", "+{{count}} 끝남", { count: group.more });
    writeTextContent(more, moreWords);
    writeAttribute(more, "aria-expanded", String(group.moreOpened));
    writeHidden(more, moreWords === "");
    shown += group.rows.length;
  }
  writeTextContent(empty, shown > 0 ? ""
    : boardScheduleAnswer ? t("board.schedule.empty", "이 범위에 일정이 없어요") : t("board.schedule.reading", "읽는 중…"));
  writeHidden(empty, shown !== 0);
  paintBoardScheduleLog(log);
}

/* 확대 하나의 말 — 단추와 카드 머리가 같은 말을 쓴다. 지금 앞뒤의 카드는 시안 v2의 이름 그대로
 * 「지금 도는 일」이다. */
function boardScheduleZoomWord(id) {
  return id === "week" ? t("board.schedule.week", "한 주")
    : id === "day" ? t("board.schedule.day", "하루") : t("board.schedule.zoomNow", "지금 앞뒤");
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
    line.folder && line.found === false
      ? t("board.schedule.noLine", "빌드 줄 폴더를 찾지 못함 · {{path}}", { path: line.folder }) : "",
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
