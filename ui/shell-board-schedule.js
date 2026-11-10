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
 * 그림은 둘이다(t-77628·t-84802, 사람이 통과시킨 시안 B p-e7e48c0f0a3959fa와 그 둘째 판): 세 확대 모두 출발
 * 안내판 넉 줄(다음 발행 · 게이트 · 착지 대기 · 끝남 — 큰 시각이나 수 하나, 색 띠와 낱말, 행동 칩)을 위에 두고,
 * 「지금 앞뒤」와 「하루」는 가로 레인 타임라인(11-08 시안 v3: 묶음마다 레인 — 빌드는 빌드마다, 줄은 칸마다, 게이트와
 * 과업은 하나씩 —, 시간이 오른쪽으로, 「지금」 세로선, 「하루」는 빈 시간을 접고, 고른 막대는 오른쪽 쪽지가 말한다),
 * 「한 주」는 5일 전부터 내일까지 일곱 칸 달력(칸마다 그날의 블록, 넘치면 수, 날을 넘는 일은 칸마다 이어지는 조각)이다. 모델은
 * 하나(`boardScheduleModel`)이고 그 행이 어느 그림에서든 블록이 된다. 기다리는
 * 과업은 그것을 실은 게이트를 안다 — 일감의 `# task:` 줄이나 `what` 줄의 과업 id로 — 그래서 왜 기다리는지
 * 말하고, 게이트가 돌면 그 예상 끝을 물려받는다. */

/* 일정의 박자와 범위, 한 표. */
const BOARD_SCHEDULE = Object.freeze({
  /* 탭이 보일 때만 도는 박자. 묻을 때 쥔 판 번호를 함께 보내, 파일이 그대로면
   * 백엔드는 「그대로」만 답하고 여기는 아무것도 다시 그리지 않는다. */
  everyMs: 5_000,
  /* 답을 받은 지 이만큼이 지나면 그 답은 「지금」의 이야기가 아니다: 쌓인 일의 게이트 줄은 낡은 답으로 도는 중이라고
   * 말하지 않고 「—」를 말한다(검수 r2 P2-2). 박자(5초)의 열두 배 — 몇 번 묻고도 답이 없을 때만 넘는다. */
  staleMs: 60_000,
  /* 「지금」 선과 남은 시간은 분 단위로 움직인다 — 5초에 옮길 픽셀은 없다. */
  clockMs: 60_000,
  /* 세 확대: 지금 앞뒤로 보이는 길이. 뒤쪽은 가장 늦은 예상 끝까지 늘어나되 상한이
   * 있다(`mostAfterMs`). */
  zooms: Object.freeze({
    /* 눈금 간격의 후보(`ticksMs`): 홈통의 높이에 눈금 글이 들어가는 가장 촘촘한 것을 고른다(t-71840). */
    /* 지금 앞뒤 — 처음 여는 확대(t-71840): 사람이 통과시킨 상황판 시안 v2(p-7c4a751fccf3359d)의
     * 「지금 도는 일」 축 그대로, 정시에 맞춰(`alignMs`) 지금의 한 시간 앞에서 네 시간. 가장 늦은
     * 예상 끝이 그 뒤면 정시 단위로 늘어난다. */
    now: Object.freeze({
      alignMs: 3_600_000, beforeMs: 3_600_000, afterMs: 3 * 3_600_000, mostAfterMs: 11 * 3_600_000,
      ticksMs: Object.freeze([3_600_000, 2 * 3_600_000, 3 * 3_600_000]),
    }),
    day: Object.freeze({
      beforeMs: 16 * 3_600_000, afterMs: 8 * 3_600_000, mostAfterMs: 24 * 3_600_000,
      ticksMs: Object.freeze([3_600_000, 2 * 3_600_000, 3 * 3_600_000, 6 * 3_600_000, 12 * 3_600_000]),
    }),
    /* 한 주는 달력이다(t-84802): 칸은 오늘의 `daysBack`일 전부터 내일까지 일곱 날이다. 이 탭은 방금 끝난 일을 보는 곳이라,
     * 달력의 주로 자르면 일요일과 월요일에 칸이 비고 금·토요일 일로 돌아갈 길이 없다(검수 r1 P1-1). 오늘은 늘 여섯째 칸이고,
     * 경계는 그 기계의 자정이다(`boardScheduleWeekBoundaries`). */
    week: Object.freeze({ calendar: true, daysBack: 5 }),
  }),
  /* 달력의 칸 하나에 서는 블록 수(t-84802): 나머지는 「+N개 더」 하나다. 일곱 칸에 여섯이면 블록이 많아도 요소 약 100개이고,
   * 도는 것·실패한 것이 먼저 서서 접히는 것은 끝난 것부터다. */
  weekDayShown: 6,
  /* 「+N개 더」 한 번에 더 서는 블록 수: 눌린 한 프레임의 값은 칸의 수가 아니라 이것이 정한다 — 350개를 한 번에 세우면 저사양에서
   * 380 ms가 들었다(t-84802). 넉 줄 안팎의 화면 하나만큼이다. */
  weekDayPage: 24,
  /* 가로 축의 눈금 글 하나의 자리(11-08 시안 v3): 「09:00」 다섯 자는 고정폭 글자 크기의 약 3.2배 — 거기에 틈
   * (`tickGapPx`)을 더한 거리보다 가까운 눈금은 솎는다. 「지금」 칩 밑에 든 눈금은 글을 감춘다. */
  tickEm: 3.2,
  /* 「하루」의 접힌 빈 시간(11-08 시안 v3): 막대가 하나도 서지 않는 이보다 긴 틈은 `foldedMs` 몫의 칸 하나로 접힌다.
   * 「지금」 앞뒤 `nowRoomMs`는 늘 편다 — 지금 선이 접힌 칸에 묻히지 않게. */
  foldGapMs: 90 * 60_000,
  foldedMs: 28 * 60_000,
  nowRoomMs: 10 * 60_000,
  /* 편 칸의 첫 눈금이 칸의 왼끝에서 이보다 가까우면 서지 않는다 — 접힌 칸의 끝 글과 붙는다. */
  tickEdgeMs: 6 * 60_000,
  /* 막대의 최소 폭(CSS px, 시안의 2 px 사이 틈과 같은 4 px 눈금) — 1분짜리 일도 손이 닿는다. CSS의 `min-width`와 같은 값이다. */
  barLeastPx: 4,
  /* 게이트 블록의 단계 칸 줄: 칸 하나의 폭과 칸 사이 틈(CSS px) — 상태 점(10 px)과 같은 크기. */
  stepCellPx: 10,
  stepGapPx: 3,
  /* 눈금 글 둘 사이에 남기는 틈(CSS px): 글 한 줄의 높이에 더한 것이 눈금 하나의 자리다(검토 t-69069 P3 ⑧). */
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

/* 묶음 넷, 읽는 차례대로. 묶음은 그것을 채울 것이 이 프로젝트에 있을 때만 선다(`needs`): 빌드는 빌드 기록이
 * 하나라도 있을 때, 게이트와 빌드 줄은 이 프로젝트의 작업 폴더에 빌드 줄이 있을 때(`line.found`). */
const BOARD_SCHEDULE_GROUPS = Object.freeze([
  { id: "builds", key: "board.build.head", word: "빌드", needs: "builds",
    tip: { key: "board.schedule.tipBuilds", word: "이 프로젝트의 빌드 기록 — 빌드마다 한 줄" } },
  { id: "gates", key: "board.schedule.groupGates", word: "게이트", needs: "line",
    tip: { key: "board.schedule.tipGates", word: "착지 전에 과업들을 묶어 한 번에 도는 결합 시험" } },
  { id: "line", key: "board.schedule.groupLine", word: "빌드 줄", needs: "line",
    tip: { key: "board.schedule.tipLine", word: "이 컴퓨터에서 무거운 빌드와 시험이 차례로 도는 줄 — 칸마다 한 줄" } },
  { id: "workers", key: "board.schedule.groupWorkers", word: "워커 과업",
    tip: { key: "board.schedule.tipWorkers", word: "워커가 맡은 과업 — 시작부터 넘김·착지까지" } },
]);

/* 안내판 네 칸의 풍선 도움말(11-10): 칸의 이름이 무엇을 말하는지 한 줄. 이름은 그대로 두고 — 사람이 익힌 낱말이다 —
 * 처음 보는 사람이 가리키면 읽는다. */
const BOARD_SCHEDULE_LINE_TIPS = Object.freeze({
  build: { key: "board.schedule.tipLineBuild", word: "지금 도는 빌드가 언제 끝나는지 — 없으면 마지막 빌드의 결과" },
  gate: { key: "board.schedule.tipLineGate", word: "게이트 = 착지 전에 과업들을 묶어 도는 결합 시험 — 통과하면 main에 올라갑니다" },
  waiting: { key: "board.schedule.tipLineWaiting", word: "착지 대기 = 보고와 검증이 끝나 main에 올리기를 기다리는 과업" },
  ended: { key: "board.schedule.tipLineEnded", word: "이 시간대에 끝난 과업 — 누르면 기록 탭으로 갑니다" },
});

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

/* 백엔드의 마지막 온전한 답과 그것을 물은 체크아웃. `unchanged` 답은 이것을 그대로 둔다. */
let boardScheduleAnswer = null;
let boardScheduleFor;
let boardScheduleAsking = false;
let boardScheduleAgain = false;
/* 마지막으로 물은 때. 탭을 다시 열면 이것이 박자 하나보다 오래됐을 때만 곧바로
 * 묻는다 — 열 때마다 묻지도, 첫 박자를 기다리지도 않는다(데스크와 같은 규칙). */
let boardScheduleAskedAt = 0;
/* 답을 마지막으로 받은 시각 — 「그대로」 답도 받은 것이다. 판 번호 `read_ms`는 답이 바뀔 때만 움직이니 이 일은 못 한다
 * (검수 r2 P2-2). 쌓인 일의 게이트 줄이 낡은 답을 지금 일처럼 말하지 않게 하는 기준이다. */
let boardScheduleAnsweredAt = 0;
/* 사람이 고른 것: 확대, 접은 묶음, 편 게이트, 연 로그, 달력의 칸마다 더 세운 수(`dayMore`). 고른 탭은 상황판의 것이다
 * (`boardNowTab`). 창의 기억이지 원장의 것이 아니다 — 판이 다시 그려져도 남는다. */
const boardScheduleChoice = { zoom: "now", folded: new Set(), opened: new Set(), log: null, dayMore: new Map(), selected: null };
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

/* 화면의 프로젝트가 보일 답: 그 체크아웃으로 물은 것만이다 — 다른 프로젝트로 옮긴 뒤의 옛 답은 없는 것이다. */
function boardScheduleHere() {
  return boardScheduleFor === activeWorktreePath ? boardScheduleAnswer : null;
}

/* 새 답을 쥔다. 다른 프로젝트로 옮긴 뒤의 첫 답이면 앞 프로젝트에서 연 로그를 닫는다 — 이 프로젝트의 것이 아니다. */
function holdBoardScheduleAnswer(answer, asked) {
  if (boardScheduleFor !== asked && boardScheduleChoice.log) {
    boardScheduleChoice.log = null;
    boardScheduleLog = null;
    noteBoardScheduleChoice();
  }
  boardScheduleAnswer = answer;
  boardScheduleFor = asked;
}

/* 쌓인 일 패널이 떠 있는가 — 그 안의 게이트 줄이 일정의 답을 읽는다(B9). */
function stackedWorkIsUp() {
  const panel = typeof document === "undefined" ? null : document.getElementById("activity-pile");
  return Boolean(panel) && !panel.hidden;
}

function paintStackedWorkIfUp() {
  if (stackedWorkIsUp() && typeof paintStackedWork === "function") paintStackedWork();
}

/* 쌓인 일 패널이 그려질 때 일정을 다시 묻는다(검수 r2 P2-2) — 박자(`everyMs`)가 허락하는 만큼만, 판 번호를 쥐고. 답이
 * 오면 `refreshBoardSchedule`이 패널을 다시 그린다. 박자도 이때 세운다: 패널이 떠 있는 동안 5초마다 묻는다. */
function askBoardScheduleForPile() {
  boardSchedulePoller.sync();
  if (Date.now() - boardScheduleAskedAt >= BOARD_SCHEDULE.everyMs) refreshBoardSchedule();
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
  if (zoom.calendar) return boardScheduleWeekBoundaries(Date.now())[0];
  return boardScheduleAnchor(zoom, Date.now()) - zoom.beforeMs;
}

/* 달력의 일곱 날: 칸 일곱의 경계 여덟, 이 기계의 시간대에서 자정마다. 날짜로 세므로 시계가 바뀌는 날은 그 길이 그대로다
 * (23시간이나 25시간) — 24시간씩 더하면 그 뒤의 칸이 자정에서 어긋난다. 지금에서 정해지니 창을 켜 둔 채 자정을 넘으면 칸이
 * 하루씩 넘어간다. 범위는 앞으로만 가므로 쥔 답이 새 범위를 다 안다. */
function boardScheduleWeekBoundaries(now) {
  const today = new Date(now);
  const [year, month, date] = [today.getFullYear(), today.getMonth(), today.getDate()];
  return boardScheduleRemembered(`week:${new Date(year, month, date).getTime()}`, () => {
    const back = BOARD_SCHEDULE.zooms.week.daysBack;
    return Array.from({ length: 8 }, (_, at) => new Date(year, month, date - back + at).getTime());
  });
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
  // The builds are the selected project's: the checkout on screen is asked about, and an answer that comes back after
  // the person moved to another project is not drawn.
  const asked = activeWorktreePath;
  const held = boardScheduleHere();
  const known = fresh || !held ? null : held.revision;
  invoke("schedule_timeline", { known, fromMs: boardScheduleFrom(), worktree: asked })
    .then((answer) => {
      if (asked !== activeWorktreePath) return;
      if (answer && typeof answer === "object") boardScheduleAnsweredAt = Date.now();
      if (answer && typeof answer === "object" && !answer.unchanged) holdBoardScheduleAnswer(answer, asked);
      paintBoardScheduleHosts();
    })
    .catch(() => {})
    .finally(() => {
      boardScheduleAsking = false;
      // 쌓인 일 reads this answer's gate (B9): it is drawn again whatever the answer was — a new one, an unchanged one,
      // or none, since the words depend on how long ago one came.
      paintStackedWorkIfUp();
      if (boardScheduleAgain) {
        boardScheduleAgain = false;
        refreshBoardSchedule({ fresh: true });
      }
    });
}

/* 로그 하나를 연다: 같은 명령에 일감 이름이나 빌드의 실행(빌드 id와 시작 시각)을 실어 끝 줄들을 받는다.
 * 프로젝트 밖의 파일이라 편집기의 문(`read_text_file`)은 거절한다 — 그래서 이 탭
 * 아래 칸에 보인다. 같은 것을 다시 누르면 닫는다. */
function openBoardScheduleLog(ask) {
  const same = boardScheduleChoice.log && JSON.stringify(boardScheduleChoice.log) === JSON.stringify(ask);
  boardScheduleChoice.log = same ? null : ask;
  boardScheduleLog = same ? null : { reading: true };
  noteBoardScheduleChoice();
  paintBoardScheduleHosts();
  if (same) return;
  const asked = activeWorktreePath;
  invoke("schedule_timeline", { known: boardScheduleHere()?.revision ?? null, fromMs: boardScheduleFrom(), log: ask, worktree: asked })
    .then((answer) => {
      if (asked !== activeWorktreePath || JSON.stringify(boardScheduleChoice.log) !== JSON.stringify(ask)) return;
      if (answer && !answer.unchanged) holdBoardScheduleAnswer(answer, asked);
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
  const unplaced = (job) => ({ start: now, end: now + secs(job) * 1_000, late: false, lane: null });
  for (const job of jobs) {
    if (job.state !== "running") continue;
    const start = Number(job.start_ms) || now;
    const due = start + secs(job) * 1_000;
    const end = Math.max(now, due);
    placed.set(job.name, { start, end, late: now > due, lane: job.lane ?? null });
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
    placed.set(job.name, { start, end, late: false, lane });
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

/* 한 레인에서 시간이 겹치는 막대는 위아래로 나뉘어 선다(t-77628의 규칙을 11-08 가로 레인에): 시작 차례로 걸으며 겹치는
 * 것끼리 한 묶음이고, 묶음 안에서는 먼저 비는 줄에 선다. 줄의 수는 묶음의 것이라, 혼자인 막대는 레인의 높이를 다 쓴다.
 * 막대는 최소 폭보다 좁아지지 않으므로(`leastMs`: 그 폭이 덮는 시간) 겹침은 그린 길이로 센다. 같은 입력이면 같은
 * 자리다. 행에 `col`(줄)·`cols`(줄 수)를 적는다. */
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
    lane: depth === 0 ? boardScheduleJobLane(job, place) : undefined,
    label: short,
    // What the job is, in its writer's own words (`zo line add --what`): the head's sentence names a running job by it.
    what: typeof job.what === "string" ? job.what : "",
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

/* 줄의 일감이 서는 레인(11-08 시안 v3): 실행기의 칸 — 돌았거나 도는 일감은 제 칸, 기다리는 일감은 줄의 계획이 놓은 칸.
 * 칸을 모르는 일감(멈춘 줄, 가드가 잡은 일감)은 「칸 없음」 레인 하나에 함께 선다. */
function boardScheduleJobLane(job, place) {
  const lane = place?.lane ?? job.lane;
  return lane === null || lane === undefined
    ? { key: "line:none", name: t("board.schedule.laneNone", "칸 없음"), rank: Number.MAX_SAFE_INTEGER }
    : { key: `line:${lane}`, name: t("board.schedule.laneLine", "줄 칸 {{lane}}", { lane }), rank: Number(lane) || 0 };
}

/* 빌드 실행의 상태(`Build::state`) → 행의 상태. 통과와 실패는 끝난 행이라 묶음마다 최근 몇만 서고, 소식 없음·
 * 취소됨·끝남은 늘 서는 행이다(`idle`) — 볼 일이다. */
const BOARD_SCHEDULE_BUILD_STATES = Object.freeze({ waiting: "waiting", running: "running", passed: "done", failed: "failed" });

/* 이 프로젝트의 빌드 실행마다 한 행 — 빌드 카드가 읽는 그 기록이다. 도는 실행은 도는 단계의 평소 끝에 남은 단계의
 * 평소를 더한 끝까지 서고(`buildEnd`), 도는 단계가 평소를 넘었으면 끝은 「지금 + 남은 단계의 평소」로 다시 서고 넘은
 * 만큼을 막대가 「지연 중」으로 말한다 — 예측 모델이 아니라 표시 규칙이다(코디 m-67652). 단계 사이에 있거나 첫 단계
 * 앞이면 남은 단계의 평소가 지금부터다. 끝난 실행은 기록이 마지막으로 쓰인 때까지 선다. 단계의 이름은 기록이 말한다. */
function boardScheduleBuildRows(builds, now) {
  return builds.map((build) => {
    const steps = Array.isArray(build.steps) ? build.steps : [];
    const named = (step) => buildWords(step.words, step.name);
    const tip = steps.filter((step) => ["done", "failed", "running"].includes(step.state))
      .map((step) => (Number.isFinite(step.secs)
        ? t("board.schedule.tipStage", "{{name}} {{time}}", { name: named(step), time: boardScheduleSpan(step.secs * 1_000) })
        : named(step)))
      .join(" · ");
    const row = {
      key: `build:${build.id}:${build.started_ms}`, label: buildTitleWords(build), start: build.started_ms,
      // One lane per build (11-08 draft v3): its runs stand in a row, each bar named by its run's label.
      lane: { key: `build:${build.id}`, name: buildWords(build.name, build.id) },
      title: build.label || undefined,
      // A run whose record names its log opens it under the schedule.
      act: build.log ? { log: { build: { id: build.id, started_ms: build.started_ms } } } : null,
    };
    const state = BOARD_SCHEDULE_BUILD_STATES[build.state] ?? "idle";
    if (state === "running" || state === "waiting") {
      const running = buildEnd(build, now);
      const rest = running ? null : buildAheadUsual(steps);
      const end = running ? running.end : now + rest.ms;
      const lateMs = Math.max(0, running?.lateMs ?? 0);
      const left = boardScheduleSpan(end - now);
      const stage = running ? named(running.step) : "";
      const ahead = steps.slice(running ? steps.indexOf(running.step) + 1 : 0).filter((step) => step.state === "ahead");
      return boardScheduleRow({
        ...row, tip, end, state, basis: (running ? running.known : rest.known) ? "history" : "none", late: lateMs > 0,
        said: lateMs > 0
          ? t("board.schedule.buildLate", "{{stage}} · +{{late}} 지연 중 · {{time}} 남음", {
            stage, late: boardScheduleSpan(lateMs), time: left,
          })
          : stage ? t("board.schedule.buildRunning", "{{stage}} · {{time}} 남음", { stage, time: left })
            : t("board.schedule.left", "{{time}} 남음", { time: left }),
        sub: boardScheduleSub(state,
          running ? t("board.schedule.buildSub", "{{stage}} {{time}}째", { stage, time: boardScheduleSpan(now - running.since) }) : "",
          ahead[0] ? t("board.schedule.buildNext", "다음 {{stage}}", { stage: named(ahead[0]) }) : ""),
        eta: boardScheduleClock(end, now),
        etaSub: lateMs > 0
          ? t("board.schedule.late", "+{{time}} 지연 중", { time: boardScheduleSpan(lateMs) })
          : t("board.schedule.left", "{{time}} 남음", { time: left }),
        // The step it is in fills the bar to now; the steps ahead that take a minute or more name the stripes.
        nowLabel: stage,
        aheadLabel: ahead.filter((step) => (buildStepUsualMs(step) ?? 0) >= BOARD_SCHEDULE.namedStageSecs * 1_000)
          .map(named).join(" · "),
      });
    }
    // The run says how it ended, and the build card says it in the same words.
    const said = BUILD_STATES[build.state] ?? BUILD_STATES.ended;
    const verdict = t(said.key, said.word);
    const end = Math.max(build.started_ms, Number(build.updated_ms) || build.started_ms);
    const took = boardScheduleSpan(end - build.started_ms);
    return boardScheduleRow({
      ...row, tip: [tip, build.reason].filter(Boolean).join(" · "), end, state,
      said: t("board.schedule.buildEnded", "{{verdict}} · {{time}}", { verdict, time: took }),
      sub: boardScheduleSub(state, verdict, boardScheduleClock(end, now)),
      subBare: boardScheduleSub(state, verdict),
      eta: boardScheduleEndWord(state, end, now, false),
      etaSub: t("board.schedule.took", "{{time}} 걸림", { time: took }),
      nowLabel: verdict,
    });
  });
}

/* 결합 게이트: 이름의 `-l<NN>` 토막마다 머리 한 행과, 펴면 그 곁에 서는 그 일감들. 머리 행은 제 단계들의 상태(`steps`, 세로 시간표의
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
    // A step with no end on record (`lost`) neither passed nor failed: a gate holding one has no passing to claim.
    const lost = count(["lost"]);
    const parts = [...new Set(members.map((job) => job.part).filter(Boolean))].sort();
    // A step still running keeps the gate running; a failed step stops it even while others queue (review P3-3).
    const state = count(["running"]) > 0 ? "running" : failed > 0 ? "failed" : open > 0 ? "waiting" : lost > 0 ? "idle" : "done";
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
          : state === "idle"
            ? t("board.schedule.gateLost", "끝 기록 없음 {{count}}개", { count: lost })
            : t("board.schedule.gatePassed", "통과 {{count}}개 · {{time}}", {
              count: steps.length, time: boardScheduleSpan(end - began),
            });
    // The calendar unfolds a gate by a press only: a block's jobs stand in the cell, and a running gate would fill it.
    // The timetable draws no job row; a gate still going keeps its jobs in the model, for its reach.
    const opened = boardScheduleTimeline(choice.zoom) ? open > 0 : choice.opened.has(gate);
    // The bar's two names (the passed draft's 「12개 초록」): what ended, or the step running and the steps after it.
    const kinds = (states) => steps.filter((job) => states.includes(job.state)).map((job) => job.kind ?? job.name);
    const tally = state === "done"
      ? t("board.schedule.gateDone", "{{done}}/{{total}} 초록", { done: steps.length, total: steps.length })
      : state === "failed"
        ? t("board.schedule.gateFailedCount", "실패 {{count}}개", { count: failed })
        : state === "idle" ? `${green}/${steps.length}` : `${steps.length - open}/${steps.length}`;
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
      ?? members.filter((job) => Number(job.end_ms) > 0).sort((a, b) => Number(b.end_ms) - Number(a.end_ms))[0]
      // A step that began and left no end (`lost`) is the story too — else a newest gate of such steps tells nothing and the
      // board names an older, passed gate instead (r2 P3-3).
      ?? members.filter((job) => job.state === "lost" && Number(job.start_ms) > 0)
        .sort((a, b) => Number(b.start_ms) - Number(a.start_ms))[0] ?? null;
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
        subBare: state === "done" || state === "failed" ? boardScheduleSub(state, tally) : undefined,
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
            // No end on record (`idle`): there is no length to claim, so no 「N 걸림」 (r2 P3-2).
            : state === "idle" ? "" : t("board.schedule.took", "{{time}} 걸림", { time: boardScheduleSpan(end - began) }),
        nowLabel: state === "done"
          ? t("board.schedule.gateBarPassed", "{{count}}개 초록", { count: steps.length })
          : state === "failed"
            ? t("board.schedule.gateBarFailed", "{{count}}개 실패", { count: failed })
            : kinds([state === "idle" ? "lost" : "running"]).join(" · "),
        aheadLabel: kinds(["waiting", "held"]).join(" · "),
        act: { fold: gate, opened }, opened, paused: Boolean(stall) && state === "waiting",
        // The step cells of the gate's bar, in the steps' order, and their names for the drawer's step list.
        steps: steps.map((job) => BOARD_SCHEDULE_JOB_STATES[job.state] ?? "idle"),
        stepNames: steps.map((job) => job.kind ?? job.name),
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
  // A gate that ended before this hand-in is an old story (review P3-2): it does not say why the task waits now. A gate
  // with no end on record (`idle`) is told by its latest step's start (r2 P2-1) — it has no end to compare.
  if (["done", "failed"].includes(gate.state) && gate.end < since) return null;
  if (gate.state === "idle" && Math.max(gate.lastStart ?? 0, gate.end) < since) return null;
  const word = gate.state === "running"
    ? t("board.schedule.whyRunning", "{{gate}}에 실림 → 도는 중 · {{time}} 끝 예상", {
      gate: gate.gate, time: boardScheduleClock(gate.end, now),
    })
    : gate.state === "waiting"
      // The gate's own time word — its next waiting step's expected start, never its first attempt's past start.
      ? !gate.startWord
        ? t("board.schedule.whyWaitingUnknown", "{{gate}}에 실림 → 차례 기다림 · 시작 모름", { gate: gate.gate })
        : t("board.schedule.whyWaiting", "{{gate}}에 실림 → 차례 기다림 · {{time}} 시작 예상", {
          gate: gate.gate, time: gate.startWord,
        })
      : gate.state === "failed"
        ? t("board.schedule.whyFailed", "{{gate}}에 실림 → 실패로 멈춤", { gate: gate.gate })
        // A gate whose steps have no end on record neither passed nor failed: the task's row does not say 「통과」 (r2 P2-1).
        : gate.state === "idle"
          ? t("board.schedule.whyLost", "{{gate}}에 실림 → 끝 기록 없음", { gate: gate.gate })
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
    subBare: over && known ? boardScheduleSub(state, carrier?.worker, word) : undefined,
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

/* 한 간격의 눈금들: 그 간격의 현지 시각 경계마다. 간격은 모두 시간 단위다(`ticksMs`). */
function boardScheduleTickMoments(from, to, step) {
  const first = new Date(from);
  first.setMinutes(0, 0, 0);
  let at = first.getTime();
  const hours = step / 3_600_000;
  while (at < from || new Date(at).getHours() % hours !== 0) at += 3_600_000;
  const tongue = document.documentElement.lang || undefined;
  const ticks = [];
  for (; at <= to; at += step) {
    const moment = at;
    ticks.push({
      at,
      label: boardScheduleRemembered(`tick:${moment}:${tongue}`,
        () => new Date(moment).toLocaleTimeString(tongue, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" })),
    });
  }
  return ticks;
}

/* 가로 레인 타임라인을 그리는 확대(11-08 시안 v3): 「지금 앞뒤」와 「하루」. 「한 주」는 일곱 칸 달력이다(t-84802). */
function boardScheduleTimeline(zoomId) {
  return !BOARD_SCHEDULE.zooms[zoomId]?.calendar;
}

/* 달력이 칸에 세우는 차례는 상태가 먼저다(검수 r1 P2-1): 도는 것, 실패한 것, 기다리는 것, 끝 기록이 없는 것, 끝난 것. 같은 상태
 * 안에서는 레인, 게이트, 과업, 줄의 일감 차례다 — 칸이 넘치면 먼저 접히는 것은 뒤의 것이다. */
const BOARD_SCHEDULE_WEEK_STATES = Object.freeze({ running: 0, failed: 1, waiting: 2, held: 2, idle: 3, done: 4 });
const BOARD_SCHEDULE_WEEK_GROUPS = Object.freeze(["builds", "gates", "workers", "line"]);

/* 시각 한 마디, 어느 날의 것이든 시:분. `boardClock`은 하루 밖이면 날짜를 말하는데, 칸이 이미 그 날이다. */
function boardScheduleDayClock(ms) {
  return boardScheduleClock(ms, ms);
}

/* 달력의 한 칸에 선 조각 하나: 행의 모양에 그 칸의 자리와 말을 더한 것. 날을 넘는 일은 칸마다 한 조각이고(t-84802), 조각의
 * 첫 말이 그 칸에서의 시간을 말한다 — 시작한 날은 「hh:mm부터」, 끝나는 날은 「hh:mm까지」, 그 사이는 「이어짐」. 칠한
 * 몫(`done`·`mark`)은 행 전체의 몫을 이 칸의 시간으로 다시 잰 것이라, 지난 칸은 꽉 차고 앞으로 올 칸은 빗금이다. */
function boardScheduleWeekPiece(row, group, day, index, first, last, slot, now) {
  const comesIn = index > first || (row.cut && index === 0);
  const goesOn = index < last || (row.cutEnd && index === 6);
  const flow = comesIn && goesOn ? "both" : comesIn ? "in" : goesOn ? "out" : "";
  const unknown = row.paused || row.state === "held";
  // What is still going and has no expected end stops at now: a clock there would read differently every minute (review P3-3).
  const untilNow = ["running", "waiting", "held"].includes(row.state) && row.end <= now;
  const when = row.cut && index === 0 && comesIn && goesOn
    ? t("board.schedule.fromAt", "{{time}}부터", { time: boardScheduleClock(row.start, now) })
    : flow === "both" ? t("board.schedule.continues", "이어짐")
      : flow === "in" ? (untilNow ? t("board.schedule.untilNow", "지금까지")
        : t("board.schedule.untilAt", "{{time}}까지", { time: boardScheduleDayClock(row.end) }))
        : flow === "out" ? t("board.schedule.fromAt", "{{time}}부터", { time: boardScheduleDayClock(row.start) })
          : unknown ? "" : boardScheduleDayClock(row.start);
  const span = row.end - row.start;
  const near = Math.max(row.start, day.at);
  const far = Math.min(row.end, day.end);
  const share = (whole) => (whole === null || whole === undefined || far <= near ? whole
    : Math.min(1, Math.max(0, (row.start + whole * span - near) / (far - near))));
  return {
    ...row, slot, flow, group, cut: row.cut && index === 0, cutEnd: row.cutEnd && index === 6,
    // The cell is already that date: what ended says the hour it ended, and only a piece that holds no 「until」 has it to say.
    sub: row.subBare === undefined ? [when, row.sub].filter(Boolean).join(" · ")
      : [when, row.subBare, flow === "" ? boardScheduleDayClock(row.end) : ""].filter(Boolean).join(" · "),
    flowWords: [comesIn ? t("board.schedule.continuesFrom", "앞 날에서 이어짐") : "",
      goesOn ? t("board.schedule.continuesTo", "다음 날로 이어짐") : ""].filter(Boolean).join(" · "),
    done: share(row.done), mark: share(row.mark),
  };
}

/* 달력의 일곱 칸(t-84802): 칸마다 그날의 조각과 수, 머리의 말. 상태 차례(`BOARD_SCHEDULE_WEEK_STATES`)로 한 번 걸으며 자리를 잡으니,
 * 오늘 도는 레인과 게이트가 사흘째 기다리는 과업 뒤로 숨지 않는다(검수 r1 P2-1). 끝난 것은 가장 최근에 끝난 것부터다 — 이 탭은 방금
 * 끝난 일을 보는 곳이다. 날을 넘는 일은 닿는 모든 칸에서 빈 같은 자리(`slot`)를 찾아 조각들이 한 줄로 이어지고, 그런 자리가 없으면
 * 칸마다 따로 선다. 칸에 `weekDayShown`을 넘는 것은 수(`more`)이고, 사람이 누른 만큼(`extra`) 더 선다. 편 게이트의 일감은 게이트
 * 바로 뒤에 서고 그 수에 들지 않는다 — 사람이 보려고 편 것이다. 게이트가 수 뒤로 가면 일감도 함께 간다: 머리 없는 일감은 없다.
 * 같은 입력이면 같은 자리다. */
function boardScheduleWeekDays(groups, boundaries, now, choice, nowWords, leftOut = null) {
  const [from, to] = [boundaries[0], boundaries[7]];
  const tongue = document.documentElement.lang || undefined;
  const days = boundaries.slice(0, 7).map((at, index) => ({
    at, end: boundaries[index + 1], today: now >= at && now < boundaries[index + 1], extra: choice.dayMore?.get(at) ?? 0,
    taken: new Set(), placed: [], over: [], jobs: 0, partial: leftOut !== null && at < leftOut.before,
  }));
  const dayOf = (ms) => {
    let index = 0;
    while (index < 6 && ms >= days[index].end) index += 1;
    return index;
  };
  const stands = (row, group, head = null) => ({
    row, group, head, slots: new Map(), first: dayOf(Math.max(row.start, from)),
    last: dayOf(Math.max(Math.min(row.end > row.start ? row.end - 1 : row.start, to - 1), row.start, from)),
  });
  const walk = [];
  BOARD_SCHEDULE_WEEK_GROUPS.forEach((id, rank) => {
    const group = groups.find((one) => one.id === id);
    if (!group || group.folded) return;
    for (const row of group.rows) {
      if (row.depth === 0) walk.push({ entry: stands(row, id), jobs: [], rank });
      else walk.at(-1)?.jobs.push(stands(row, id, walk.at(-1).entry));
    }
  });
  const state = ({ entry }) => BOARD_SCHEDULE_WEEK_STATES[entry.row.state] ?? BOARD_SCHEDULE_WEEK_STATES.waiting;
  const ended = ({ entry }) => BOARD_SCHEDULE_ENDED_STATES.includes(entry.row.state);
  walk.sort((a, b) => state(a) - state(b) || a.rank - b.rank || (ended(a) ? b.entry.row.end - a.entry.row.end : 0));
  const shown = BOARD_SCHEDULE.weekDayShown;
  const free = (index, slot) => !days[index].taken.has(slot);
  const fits = (index, slot) => slot < shown + days[index].jobs && free(index, slot);
  const put = (entry, index, slot) => {
    days[index].taken.add(slot);
    days[index].placed.push({ entry, index, slot });
    entry.slots.set(index, slot);
  };
  for (const { entry, jobs } of walk) {
    const touched = Array.from({ length: entry.last - entry.first + 1 }, (_, at) => entry.first + at);
    // Work over midnight takes one slot in every day it touches, if there is one.
    let across = -1;
    if (touched.length > 1) {
      const most = Math.min(...touched.map((index) => shown + days[index].jobs));
      for (let slot = 0; across < 0 && slot < most; slot += 1) {
        if (touched.every((index) => fits(index, slot))) across = slot;
      }
    }
    for (const index of touched) {
      let slot = across;
      if (slot < 0) {
        slot = 0;
        while (slot < shown + days[index].jobs && !free(index, slot)) slot += 1;
      }
      if (fits(index, slot)) put(entry, index, slot);
      else days[index].over.push({ entry, index });
    }
    // A gate's unfolded jobs come right behind it, and go where it goes (review P3-2).
    for (const job of jobs) {
      for (let index = job.first; index <= job.last; index += 1) {
        const day = days[index];
        const held = entry.slots.get(index);
        if (held === undefined) {
          if (index >= entry.first && index <= entry.last) day.over.push({ entry: job, index });
          continue;
        }
        let slot = held + 1;
        while (!free(index, slot)) slot += 1;
        put(job, index, slot);
        day.jobs += 1;
      }
    }
  }
  let rows = 0;
  const built = days.map((day) => {
    const placed = day.placed.slice().sort((a, b) => a.slot - b.slot);
    let slot = (placed.at(-1)?.slot ?? -1) + 1;
    const unfolded = day.over.slice(0, day.extra);
    for (const over of unfolded) placed.push({ ...over, slot: slot++ });
    const more = day.over.length - unfolded.length;
    // The two buttons take a line each: what is left to stand, and what was stood.
    rows = Math.max(rows, slot + (more > 0 ? 1 : 0) + (unfolded.length > 0 ? 1 : 0));
    const moment = new Date(day.at);
    const weekday = boardScheduleRemembered(`week-day:${day.at}:${tongue}`, () => moment.toLocaleDateString(tongue, { weekday: "short" }));
    // The day of the month alone: a cell can be 5rem wide, and the full date stands in its name and its tip.
    const date = boardScheduleRemembered(`week-date:${day.at}:${tongue}`, () => moment.toLocaleDateString(tongue, { day: "numeric" }));
    const full = boardScheduleRemembered(`week-full:${day.at}:${tongue}`, () => moment.toLocaleDateString(tongue, { dateStyle: "full" }));
    const count = day.placed.length + day.over.length;
    // An answer that left finished jobs out says so on every day it reaches: the count there is short (review P2-2).
    const partial = day.partial ? t("board.schedule.dayPartial", "끝난 일감 일부는 빠짐") : "";
    return {
      at: day.at, end: day.end, today: day.today, count, more, unfolded: unfolded.length, moreSlot: slot, full,
      head: `${weekday} ${date}`, now: day.today ? nowWords : "", partial: day.partial,
      tip: [full, partial].filter(Boolean).join(" · "),
      name: [day.today
        ? t("board.schedule.dayNameToday", "{{date}} · 오늘 · 블록 {{count}}개", { date: full, count })
        : t("board.schedule.dayName", "{{date}} · 블록 {{count}}개", { date: full, count }), partial].filter(Boolean).join(" · "),
      pieces: placed.map(({ entry, index, slot: at }) => boardScheduleWeekPiece(entry.row, entry.group, day, index, entry.first, entry.last, at, now)),
    };
  });
  return { days: built, rows: rows + 1 };
}

/* 안내판의 넉 줄(t-77628) — 공항 출발 안내판의 문법: 한 줄에 무엇 · 큰 글(시각이나 수) 하나 · 낱말 · 비고 · 행동.
 * 빌드, 게이트, 착지 대기, 끝남. 줄은 비어도 남고 「—」·「이 범위에 없음」을 말한다 — 그 줄을 채울 묶음이 이
 * 프로젝트에 서지 않으면(`standing`) 줄도 서지 않는다. 수는 시간표의 행 목록에서 센다 — 따로 세지 않는다. */
function boardScheduleBoardLines(groups, now, outsideBuild = null) {
  const group = (id) => groups.find((one) => one.id === id);
  const stands = (id) => group(id)?.standing !== false;
  const rows = (id) => (group(id)?.rows ?? []).filter((row) => row.depth === 0);
  const clock = (ms) => boardScheduleClock(ms, now);
  const notInRange = t("board.schedule.notInRange", "이 범위에 없음");
  const logWord = t("board.schedule.actLog", "로그 →");
  const recordWord = t("board.schedule.actRecord", "기록 →");
  const stateWord = (state) => (state === "running"
    ? t(BOARD_RUN_STATES.run.key, BOARD_RUN_STATES.run.word)
    : state === "waiting"
      ? t(BOARD_RUN_STATES.queue.key, BOARD_RUN_STATES.queue.word)
      : state === "failed" ? t("board.schedule.wordFailed", "실패")
        : state === "idle" ? t("board.schedule.lost", "끝 기록 없음") : t("board.schedule.wordPassed", "통과"));
  const tones = { running: "run", waiting: "wait", failed: "halt", idle: "halt", done: "done" };

  // 1. The builds: the run in play — running, else waiting for its first step — else the last that ended in the range.
  const builds = rows("builds");
  const live = builds.find((row) => row.state === "running") ?? builds.find((row) => row.state === "waiting") ?? null;
  // The range may hold none that ended: the answer's own last run (`last_build`) is the one to name then.
  const lastBuild = builds.filter((row) => row.state !== "running" && row.state !== "waiting")
    .sort((a, b) => b.end - a.end)[0] ?? outsideBuild;
  const build = live
    ? {
      tone: live.late || live.state === "waiting" ? "wait" : "run", big: live.eta, word: live.etaSub,
      note: [live.sub, live.basis === "none" ? t("board.schedule.basisNone", "근거 없음") : ""].filter(Boolean).join(" · "),
      act: live.act, actWord: live.act ? logWord : "",
    }
    : {
      tone: "quiet", big: "—", word: t("board.schedule.none", "잡힌 것 없음"),
      note: lastBuild
        ? t("board.schedule.lastBuild", "마지막 {{time}} · {{verdict}}", { time: clock(lastBuild.end), verdict: lastBuild.nowLabel })
        : notInRange,
      act: lastBuild?.act ?? null, actWord: lastBuild?.act ? logWord : "",
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
      big: gate.state === "done" || gate.state === "failed" ? clock(gate.end) : gate.state === "idle" ? "—" : gate.eta,
      word: stateWord(gate.state),
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

  // The sentence on top (11-10): the one thing in play, in plain words — the build that runs and how long is left, else
  // the gate, else the job running on the build line, else what waits to land, else that nothing is scheduled. The
  // clock line under it keeps the answers side by side; this says which of them to read first.
  const liveGate = gate && (gate.state === "running" || gate.state === "waiting") ? gate : null;
  const lineRunning = rows("line").filter((row) => row.state === "running");
  const said = (name, state, left) => [name, [stateWord(state), left].filter(Boolean).join(" · ")].filter(Boolean).join(" — ");
  // (The name is `sentence`: this file's code never spells a tooltip's assignment, and the contract reads the letters.)
  const sentence = live
    ? said(live.label, live.state, live.etaSub)
    : liveGate
      ? said([t("board.schedule.lineGate", "게이트"), gateLine.name].filter(Boolean).join(" "), liveGate.state, liveGate.etaSub)
      : lineRunning.length > 0
        ? said(lineRunning[0].what || lineRunning[0].label, "running", lineRunning[0].etaSub)
        : waiting.length > 0
          ? t("board.schedule.titleWaiting", "착지를 기다리는 과업 {{count}}건", { count: waiting.length })
          : t("board.schedule.titleIdle", "지금 잡힌 일정이 없습니다");

  return {
    head: {
      title: sentence,
      now: t("board.schedule.nowClock", "지금 {{time}}", { time: clock(now) }),
      sum: [
        !stands("builds") ? ""
          : live ? t("board.schedule.boardBuildAt", "빌드 끝 {{time}}", { time: live.eta })
            : t("board.schedule.boardBuildNone", "도는 빌드 없음"),
        gate ? t("board.schedule.boardGate", "게이트 {{word}}", { word: gateLine.word }) : "",
        // 빌드 줄에서 도는 일감(11-10): 「도는 빌드 없음」 곁에서 줄의 일감이 도는 것을 말하지 않으면 아무것도 안 도는
        // 판으로 읽혔다.
        lineRunning.length > 0 ? t("board.schedule.boardLine", "빌드 줄 {{count}}개 도는 중", { count: lineRunning.length }) : "",
        t("board.schedule.boardWaiting", "착지 대기 {{count}}", { count: waiting.length }),
      ].filter(Boolean).join(" · "),
    },
    lines: [
      {
        id: "build", standing: stands("builds"), what: t("board.build.head", "빌드"), whatSub: (live ?? lastBuild)?.label ?? "",
        ...build,
      },
      {
        id: "gate", standing: stands("gates"), what: [t("board.schedule.lineGate", "게이트"), gateLine.name].filter(Boolean).join(" "),
        whatSub: t("board.schedule.lineGateSub", "착지 전 결합 시험"), ...gateLine,
      },
      {
        id: "waiting", standing: stands("workers"), what: t("board.schedule.lineWaiting", "착지 대기"),
        whatSub: t("board.schedule.lineWaitingSub", "보고·검증 뒤 · 코디 차례"), ...waitingLine,
      },
      {
        id: "ended", standing: stands("workers"), what: t("board.schedule.lineEnded", "끝남"),
        whatSub: t("board.schedule.lineEndedSub", "이 시간대에 끝난 과업"), ...endedLine,
      },
    ],
  };
}

/* 그릴 것 한 벌: 보이는 범위, 묶음 넷과 그 행들, 안내판 넉 줄. 백엔드의 답과 데스크의 과업, 지금, 고른 것에서
 * 짓는다 — 장면 시험이 이것을 그대로 읽는다. 레인 타임라인을 그리는 확대면(`timeline`) 축의 눈금(`gutter`)과 접힌
 * 칸(`gaps`), 묶음마다 레인(`lanes`)과 레인 안에서 겹친 막대의 줄(`col`·`cols`)도 여기서 정한다 — 축의 폭(`trackPx`)은
 * 그릇이 지켜보다 넘긴다. 달력이면 칸 일곱(`week`)을 정한다. */
function boardScheduleModel(answer, tasks, now, choice, loud = false, metrics = null) {
  // The lane timeline's measures, read by the holder (11-08 draft v3): the axis track's width, kept by a resize
  // observer, and the room one tick's words need.
  const trackPx = Number(metrics?.trackPx) || 0;
  const tickRoom = Number(metrics?.tickRoomPx) || 0;
  const zoom = BOARD_SCHEDULE.zooms[choice.zoom] ?? BOARD_SCHEDULE.zooms.now;
  const timeline = boardScheduleTimeline(choice.zoom);
  const jobs = Array.isArray(answer?.jobs) ? answer.jobs : [];
  const builds = Array.isArray(answer?.builds) ? answer.builds : [];
  const facts = answer?.line ?? {};
  // What this project has to fill a group with: a build run kept, a build line in its workspace folder.
  const outside = boardScheduleOutsideBuild(answer, builds, now);
  const has = { builds: builds.length > 0 || outside !== null, line: facts.found === true };
  const runnerLanes = Array.isArray(facts.runner_lanes) ? facts.runner_lanes : [];
  const stall = boardScheduleStall(facts, runnerLanes);
  const plan = boardScheduleQueuePlan(jobs, runnerLanes, now);
  const boundaries = zoom.calendar ? boardScheduleWeekBoundaries(now) : null;
  const anchor = boardScheduleAnchor(zoom, now);
  const from = boundaries ? boundaries[0] : anchor - zoom.beforeMs;
  // The calendar stands every finished row in its own day: what a cell cannot hold is that cell's count, not the group's.
  const opened = (group) => Boolean(boundaries) || choice.opened.has(`ended:${group}`);
  // The gates first: the tasks they carry read their facts (t-77628).
  const gates = boardScheduleGateRows(jobs, plan, now, choice, stall);
  const link = { carried: boardScheduleTaskGates(jobs), gates: new Map(gates.map((entry) => [entry.facts.gate, entry.facts])) };
  // The line and the workers fold their finished rows before they build them; builds and gates are few.
  const line = boardScheduleLineRows(jobs, plan, now, stall, from, opened("line"));
  const workers = boardScheduleTaskRows(tasks, now, answer?.landings, from, opened("workers"), link);
  // Each group as entries: a row, and the rows standing under it (a gate's jobs).
  const alone = (row) => ({ head: row, rows: [] });
  const built = {
    builds: { entries: boardScheduleBuildRows(builds, now).map(alone), more: null },
    gates: { entries: gates, more: null },
    line: { entries: line.rows.map(alone), more: line.more },
    workers: { entries: workers.rows.map(alone), more: workers.more },
  };
  const latest = Math.max(now, ...Object.values(built).flatMap((group) => group.entries)
    .flatMap((entry) => [entry.head, ...entry.rows])
    .filter((row) => ["running", "waiting", "held"].includes(row.state)).map((row) => row.end));
  // An aligned zoom ends on the hour too — the passed draft's 09:00–13:00.
  const reach = Math.max(anchor + zoom.afterMs, latest + BOARD_SCHEDULE.tailMs);
  const to = boundaries ? boundaries[7]
    : Math.min(anchor + zoom.mostAfterMs, zoom.alignMs ? Math.ceil(reach / zoom.alignMs) * zoom.alignMs : reach);
  const seen = (row) => row.end >= from && row.start <= to;
  // What each group keeps standing: its rows in the range, its finished ones folded to the newest few.
  const kept = BOARD_SCHEDULE_GROUPS.map((group) => {
    let { entries, more } = built[group.id];
    entries = entries.filter((entry) => seen(entry.head));
    if (more === null) {
      const ended = (entry) => BOARD_SCHEDULE_ENDED_STATES.includes(entry.head.state);
      const folded = boardScheduleEndedKept(entries.filter(ended).map((entry) => ({ entry, end: entry.head.end })),
        opened(group.id));
      const shown = new Set(folded.kept.map((one) => one.entry));
      entries = entries.filter((entry) => !ended(entry) || shown.has(entry));
      more = folded.more;
    }
    const rows = entries.sort((a, b) => boardScheduleOrder(a.head, b.head))
      .flatMap((entry) => [entry.head, ...entry.rows.filter(seen).sort(boardScheduleOrder)]);
    return { group, rows, more };
  });
  // 「하루」 folds the long stretches no bar stands in (11-08 draft v3); 「지금 앞뒤」 and the calendar are linear.
  const scale = boardScheduleScale(from, to, now, choice.zoom === "day"
    ? kept.filter(({ group }) => !choice.folded.has(group.id)).flatMap(({ rows }) => rows).map((row) => [row.start, row.end])
    : null);
  const percent = scale.at;
  // A bar is never narrower than `barLeastPx`: the time that width covers, for the overlap test inside a lane.
  const leastMs = trackPx > 0 ? (BOARD_SCHEDULE.barLeastPx / trackPx) * scale.total : 0;
  const groups = kept.map(({ group, rows: standingRows, more }) => {
    const rows = standingRows.map((row) => ({
      ...row,
      // 기계가 붐비면(1분 부하 > 코어 수, 데스크의 기계 띠와 같은 판정) 앞으로 남은 예상 곁에 그렇다고만
      // 적는다 — 값은 바꾸지 않는다(코디 m-67652).
      said: loud && row.depth === 0 && ["running", "waiting"].includes(row.state)
        ? t("board.schedule.loud", "{{said}} · 부하 높음·늦어질 수 있음", { said: row.said }) : row.said,
      // A bar cut at the range's start says since when (t-77628); the chart's rows keep their words.
      sub: timeline && row.depth === 0 && row.start < from
        ? [row.sub, t("board.schedule.fromAt", "{{time}}부터", { time: boardScheduleClock(row.start, now) })].filter(Boolean).join(" · ")
        : row.sub,
      from: percent(row.start),
      to: percent(Math.max(row.end, row.start)),
      done: row.doneShare ?? (row.state === "running" && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : row.state === "waiting" ? 0 : 1),
      // A bar cut at the left edge when it began before the range, and the share of a waiting task's bar that is
      // the gate's time still ahead.
      cut: row.start < from,
      cutEnd: row.end > to,
      // A gate's bar cannot unfold its jobs (the lanes draw heads only): it opens its telling job's log.
      act: timeline && row.depth === 0 && row.act?.fold ? (row.facts?.logJob ? { log: { job: row.facts.logJob } } : null) : row.act,
      mark: row.ride?.inherits && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : null,
      col: 0,
      cols: 1,
      // A bar is named by its run only on the lanes, where its lane names the build; the calendar keeps the whole name.
      title: timeline ? row.title : undefined,
    }));
    const heads = rows.filter((row) => row.depth === 0);
    if (boundaries) more = 0;
    return {
      id: group.id,
      label: t(group.key, group.word),
      standing: !group.needs || has[group.needs],
      folded: choice.folded.has(group.id),
      count: heads.length,
      waiting: heads.filter((row) => ["waiting", "held"].includes(row.state)).length,
      ended: heads.filter((row) => BOARD_SCHEDULE_ENDED_STATES.includes(row.state)).length + more,
      rows,
      lanes: boardScheduleLanesOf(group.id, heads, leastMs),
      more,
      moreOpened: opened(group.id),
    };
  });
  const nowAt = percent(now);
  const span = Math.max(1, to - from);
  const hours = span / 3_600_000;
  const axis = timeline ? boardScheduleAxis(scale, trackPx, zoom.ticksMs, tickRoom, nowAt, now) : { ticks: [], gaps: [] };
  const board = boardScheduleBoardLines(groups, now, outside);
  // The finished jobs the answer could not carry: said only while the cut falls inside what is on screen.
  const cut = answer?.jobs_left_out;
  const leftOut = Number(cut?.count) > 0 && Number(cut.before_ms) > from ? { count: Number(cut.count), before: Number(cut.before_ms) } : null;
  return {
    from, to, now, nowAt, groups, stall, timeline, hours, leftOut, scale,
    gutter: axis.ticks,
    gaps: axis.gaps,
    board,
    week: boundaries ? boardScheduleWeekDays(groups, boundaries, now, choice, board.head.now, leftOut) : null,
    // Nothing moving is expected to end after now: the timeline says so under its lanes.
    aheadEmpty: latest <= now,
  };
}

/* 시간 → 범위 안의 자리(0..100)(11-08 시안 v3). `spans`가 있으면(「하루」) 막대가 서는 구간과 「지금」 앞뒤만 편 그대로
 * 두고, 그 사이의 `foldGapMs`보다 긴 틈은 `foldedMs` 몫의 칸 하나로 접는다 — 빈 밤이 축의 절반을 먹지 않게. */
function boardScheduleScale(from, to, now, spans) {
  const pieces = [];
  if (spans) {
    const room = BOARD_SCHEDULE.nowRoomMs;
    const open = [[now - room, now + room], ...spans]
      .map(([a, b]) => [Math.max(from, a), Math.min(to, Math.max(a, b))])
      .filter(([a, b]) => b > a)
      .sort((x, y) => x[0] - y[0]);
    const merged = [];
    for (const [a, b] of open) {
      const last = merged.at(-1);
      if (last && a <= last[1]) last[1] = Math.max(last[1], b);
      else merged.push([a, b]);
    }
    let at = from;
    const between = (a, b) => {
      if (b <= a) return;
      pieces.push(b - a >= BOARD_SCHEDULE.foldGapMs ? { a, b, w: BOARD_SCHEDULE.foldedMs, gap: true } : { a, b, w: b - a });
    };
    for (const [a, b] of merged) {
      between(at, a);
      pieces.push({ a, b, w: b - a });
      at = b;
    }
    between(at, to);
  }
  if (pieces.length === 0) pieces.push({ a: from, b: to, w: Math.max(1, to - from) });
  const total = pieces.reduce((sum, piece) => sum + piece.w, 0);
  let sum = 0;
  for (const piece of pieces) {
    piece.from = (sum / total) * 100;
    sum += piece.w;
    piece.to = (sum / total) * 100;
  }
  const at = (ms) => {
    if (!(ms > from)) return 0;
    if (ms >= to) return 100;
    const piece = pieces.find((one) => ms <= one.b) ?? pieces.at(-1);
    return piece.from + ((ms - piece.a) / Math.max(1, piece.b - piece.a)) * (piece.to - piece.from);
  };
  return { at, pieces, total };
}

/* 축(11-08 시안 v3): 편 칸마다 현지 시각의 눈금 — 축의 폭에 글이 들어가는 가장 촘촘한 간격으로, 그래도 붙는 것은 솎고 —
 * 과 접힌 칸마다 그 길이 한 마디. 「지금」 칩 밑에 든 눈금은 선만 남고 글을 감춘다. */
function boardScheduleAxis(scale, trackPx, ticksMs, room, nowAt, now) {
  const gaps = scale.pieces.filter((piece) => piece.gap).map((piece) => ({
    from: piece.from, to: piece.to,
    words: t("board.schedule.gapWords", "⋯ {{time}}", { time: boardScheduleSpan(piece.b - piece.a) }),
    tip: t("board.schedule.gapTip", "{{from}}–{{to}} 빈 시간", {
      from: boardScheduleClock(piece.a, now), to: boardScheduleClock(piece.b, now),
    }),
  }));
  if (trackPx <= 0 || room <= 0) return { ticks: [], gaps };
  const pxPerMs = trackPx / scale.total;
  const step = ticksMs.find((one) => one * pxPerMs >= room) ?? ticksMs.at(-1);
  const ticks = [];
  for (const [index, piece] of scale.pieces.entries()) {
    if (piece.gap) continue;
    for (const tick of boardScheduleTickMoments(piece.a, piece.b, step)) {
      if (index > 0 && tick.at - piece.a < BOARD_SCHEDULE.tickEdgeMs) continue;
      const at = scale.at(tick.at);
      const last = ticks.at(-1);
      if (last && ((at - last.at) / 100) * trackPx < room) continue;
      ticks.push({ ...tick, at, covered: (Math.abs(at - nowAt) / 100) * trackPx < room });
    }
  }
  return { ticks, gaps };
}

/* 한 묶음의 레인(11-08 시안 v3): 행이 이름 댄 레인(`lane`)마다 하나 — 빌드는 빌드마다, 줄의 일감은 실행기의 칸마다 —, 이름이
 * 없는 행(게이트, 과업)은 제 레인 하나다. 레인의 차례는 첫 막대의 차례이고, 줄의 칸은 칸 번호 차례다. */
function boardScheduleLanesOf(groupId, heads, leastMs) {
  const lanes = new Map();
  for (const row of heads) {
    const lane = row.lane ?? { key: row.key, name: row.label };
    let entry = lanes.get(lane.key);
    if (!entry) {
      entry = { key: lane.key, name: lane.name, rank: lane.rank ?? 0, rows: [] };
      lanes.set(lane.key, entry);
    }
    entry.rows.push(row);
  }
  const all = [...lanes.values()];
  for (const lane of all) {
    boardScheduleSubColumns(lane.rows, leastMs);
    lane.depth = Math.max(1, ...lane.rows.map((row) => row.cols));
  }
  return groupId === "line" ? all.sort((a, b) => a.rank - b.rank || a.key.localeCompare(b.key)) : all;
}

/* 범위 밖의 마지막 빌드: 답이 실은 `last_build`, 없으면 — 조용한 동안 창이 옛 답을 그대로 쓰므로 범위가 밀려도 백엔드는
 * 새로 보내지 않는다 — 그 답의 실행 가운데 가장 최근에 끝난 것(검수 r2 P3-1). 하나도 없으면 null. */
function boardScheduleOutsideBuild(answer, builds, now) {
  const ended = answer?.last_build ?? builds.filter((build) => !BUILD_LIVE.has(build.state))
    .sort((a, b) => b.updated_ms - a.updated_ms)[0] ?? null;
  return ended ? boardScheduleBuildRows([ended], now)[0] : null;
}

/* ---- 그림 ----------------------------------------------------------------------- */

function boardScheduleElement(tag, className, text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/* 블록이나 안내판의 줄을 누르면: 게이트를 펴거나 접고, 로그를 열고, 과업은 「기록」 탭으로. */
function boardScheduleAct(view, act) {
  if (!act) return;
  if (act.fold) {
    if (act.opened) boardScheduleChoice.opened.delete(act.fold);
    else boardScheduleChoice.opened.add(act.fold);
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

/* 그릇의 뼈대: 머리(확대와 줄의 사정), 안내판과 세로 시간표(t-77628)와 일곱 칸 달력(t-84802), 묶음 넷, 로그 칸,
 * 빈 말. 한 번 짓고 키로 살아남는다. 묶음 상자 넷은 하나뿐이고, 확대에 따라 시간표와 달력 사이를 옮겨 다닌다 — 지금
 * 앞뒤와 하루는 시간표의 열로, 한 주는 달력 위의 머리 줄로 선다. */
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
  // The departure board (t-77628) — four cells over the lanes in the 11-08 draft v3.
  const cal = boardScheduleElement("section", "board-schedule-cal");
  const board = boardScheduleElement("div", "board-schedule-board");
  const boardHead = boardScheduleElement("header", "board-schedule-board-head");
  // 맨 위 한 문장(11-10): 다른 탭처럼 판의 바닥에, 지금 무엇이 돌고 얼마나 남았는지를 쉬운 말로. 안내판은 그 밑이다.
  const boardTitle = boardScheduleElement("h2", "board-schedule-title");
  const boardNow = boardScheduleElement("strong", "board-schedule-board-now");
  const boardSum = boardScheduleElement("span", "board-schedule-board-sum");
  boardHead.append(boardNow, boardSum);
  board.append(boardHead);
  const lines = [];
  for (const id of ["build", "gate", "waiting", "ended"]) {
    const line = boardScheduleElement("button", "board-schedule-line");
    line.type = "button";
    line.dataset.line = id;
    line.append(boardScheduleElement("span", "board-schedule-line-what"), boardScheduleElement("span", "board-schedule-line-big"),
      boardScheduleElement("span", "board-schedule-line-note"));
    line.onclick = () => boardScheduleAct(view, line.__act);
    lines.push(line);
  }
  board.append(...lines);
  // The lane timeline (11-08 draft v3): the axis row, the groups' lanes under it, and the drawer beside them.
  const day = boardScheduleElement("div", "board-schedule-day");
  const body = boardScheduleElement("div", "board-schedule-cal-body");
  const gutter = boardScheduleElement("div", "board-schedule-gutter");
  gutter.setAttribute("aria-hidden", "true");
  const track = boardScheduleElement("div", "board-schedule-gutter-track");
  track.__now = boardScheduleElement("span", "board-schedule-now");
  track.__ticks = [];
  track.__gaps = [];
  track.append(track.__now);
  // The track's width decides how many ticks fit: kept by an observer, so a paint never asks the page for layout.
  track.__width = 0;
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(([entry]) => {
      const width = Math.round(entry.contentRect.width);
      if (Math.abs(width - track.__width) < BOARD_SCHEDULE.barLeastPx) return;
      track.__width = width;
      paintBoardScheduleHosts();
    }).observe(track);
  }
  const gutterHead = boardScheduleElement("span", "board-schedule-gutter-head");
  gutter.append(gutterHead, track);
  const aheadEmpty = boardScheduleElement("p", "board-schedule-ahead-empty");
  body.append(gutter, aheadEmpty);
  const drawer = boardScheduleElement("aside", "board-schedule-drawer");
  drawer.setAttribute("aria-live", "polite");
  const drawerTitle = boardScheduleElement("h4", "board-schedule-drawer-title");
  drawerTitle.append(boardScheduleElement("span", "board-state board-schedule-drawer-state"),
    boardScheduleElement("span", "board-schedule-drawer-name"));
  const drawerAct = boardScheduleElement("button", "board-btn board-schedule-drawer-act");
  drawerAct.type = "button";
  drawerAct.onclick = () => boardScheduleAct(view, drawer.__row?.act);
  const drawerActs = boardScheduleElement("p", "board-schedule-drawer-acts");
  drawerActs.append(drawerAct);
  drawer.append(boardScheduleElement("p", "board-schedule-drawer-empty"), drawerTitle,
    boardScheduleElement("p", "board-schedule-drawer-said"), boardScheduleElement("p", "board-schedule-drawer-sub"),
    boardScheduleElement("p", "board-schedule-drawer-why"), boardScheduleElement("ol", "board-schedule-drawer-steps"),
    boardScheduleElement("p", "board-schedule-drawer-tip"), drawerActs);
  day.append(body, drawer);
  // What a block's fill, stripes, dashes and zigzag say — one line the eye can read (review P3-6).
  const calLegend = boardScheduleElement("p", "board-schedule-cal-legend");
  // The week's calendar (t-84802): the groups' heads, then seven cells, built while the week is the zoom (`boardScheduleWeekCells`).
  const week = boardScheduleElement("div", "board-schedule-week");
  week.setAttribute("role", "group");
  const weekGroups = boardScheduleElement("div", "board-schedule-week-groups");
  const weekDays = boardScheduleElement("div", "board-schedule-week-days");
  weekDays.addEventListener("focusin", (event) => boardScheduleWeekStop(weekDays, event.target));
  weekDays.addEventListener("keydown", (event) => boardScheduleWeekKey(event, weekDays));
  week.append(weekGroups, weekDays);
  cal.append(boardTitle, board, day, week, calLegend);
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
  host.replaceChildren(head, cal, log, empty);
  host.__parts = { head, boxes, cal, boardTitle, boardNow, boardSum, lines, day, body, gutter, gutterHead, track, aheadEmpty, drawer, calLegend, week,
    weekGroups, weekDays, log, empty };
}

/* 달력의 칸 일곱(t-84802): 칸마다 머리와 그 뒤의 블록, 끝에 「+N개 더」와 「N개 접기」. 한 주를 보는 동안만 서 있다 — 다른
 * 확대의 무게에 들지 않는다. 누른 단추가 사라지면 초점은 다른 단추로 간다 — 키보드로 누른 사람이 제자리를 잃지 않는다. */
function boardScheduleWeekCells() {
  return Array.from({ length: 7 }, () => {
    const cell = boardScheduleElement("div", "board-schedule-week-day");
    cell.setAttribute("role", "group");
    cell.tabIndex = -1;
    const button = (className, press) => {
      const node = boardScheduleElement("button", className);
      node.type = "button";
      node.tabIndex = -1;
      node.onclick = () => {
        const had = document.activeElement === node;
        press(Number(cell.dataset.day));
        noteBoardScheduleChoice();
        paintBoardScheduleHosts();
        if (had && node.hidden) (node === cell.__more ? cell.__less : cell.__more).focus();
      };
      return node;
    };
    const dayMore = boardScheduleChoice.dayMore;
    cell.__head = boardScheduleElement("div", "board-schedule-week-head");
    cell.__more = button("board-schedule-more", (at) => dayMore.set(at, (dayMore.get(at) ?? 0) + BOARD_SCHEDULE.weekDayPage));
    cell.__less = button("board-schedule-more board-schedule-week-less", (at) => dayMore.delete(at));
    cell.append(cell.__head, cell.__more, cell.__less);
    return cell;
  });
}

/* 달력의 탭 멈춤은 하나다(t-84802): 칸과 블록 수십 개를 탭으로 걷지 않게, 초점이 선 곳 하나만 `tabindex=0`이고 안에서는
 * 방향키로 다닌다. 처음엔 오늘의 칸이다. */
function boardScheduleWeekStop(days, node) {
  if (days.__stop === node) return;
  if (days.__stop) days.__stop.tabIndex = -1;
  node.tabIndex = 0;
  days.__stop = node;
}

/* 달력 안의 키: ←→는 옆 날의 같은 자리, ↑↓는 그날의 위아래, Home·End는 주의 첫 칸·끝 칸, 칸에서 Enter·Space는 첫 블록으로.
 * 날이 쌓인 좁은 폭에서는 ↑↓가 날의 끝에서 앞뒤 날로 넘어간다 — 보이는 차례 그대로다. */
function boardScheduleWeekKey(event, days) {
  const cell = event.target.closest?.(".board-schedule-week-day");
  if (!cell) return;
  const cells = [...days.children];
  const at = cells.indexOf(cell);
  const stops = (one) => [one, ...one.querySelectorAll(".board-schedule-block, .board-schedule-more:not([hidden])")];
  const mine = stops(cell);
  const place = mine.indexOf(event.target);
  const stacked = () => cells[0].offsetTop !== cells[1].offsetTop;
  const beside = (other) => {
    if (!other) return null;
    const theirs = stops(other);
    return theirs[Math.min(place, theirs.length - 1)];
  };
  let next = null;
  switch (event.key) {
    case "ArrowLeft": next = beside(cells[at - 1]); break;
    case "ArrowRight": next = beside(cells[at + 1]); break;
    case "ArrowDown": next = mine[place + 1] ?? (stacked() ? cells[at + 1] : null); break;
    case "ArrowUp": next = mine[place - 1] ?? (stacked() && at > 0 ? stops(cells[at - 1]).at(-1) : null); break;
    case "Home": next = cells[0]; break;
    case "End": next = cells[6]; break;
    case "Enter":
    case " ":
      if (event.target !== cell) return;
      next = mine[1] ?? null;
      break;
    default: return;
  }
  event.preventDefault();
  next?.focus();
}

/* 쌓인 일의 「게이트 일감」 줄(t-84799 B9): 일정의 마지막 답이 말하는 게이트 — 안내판의 둘째 줄과 같은 규칙으로 고른 것 —
 * 의 이름과 그 게이트에 일어난 일. 이 프로젝트에 빌드 줄이 없거나 아직 답이 없으면 `null`이고 줄은 서지 않는다. 답이
 * 게이트를 하나도 싣지 않았거나 낡았으면 빈 글이다. 글은 답 하나에 한 분(分)·한 언어에 한 번만 센다 — 쌓인 일 패널은
 * 사실이 바뀔 때만 다시 그리고, 이 줄은 그 사실 가운데 하나다. */
const boardScheduleGateWordsHeld = new WeakMap();

function boardScheduleGateWords(now = Date.now()) {
  const answer = boardScheduleHere();
  if (answer?.line?.found !== true) return null;
  if (!Array.isArray(answer.jobs)) return "";
  // An answer held for long is an old story, not the gate's present: a dash, whatever the answer says (r2 P2-2).
  if (now - boardScheduleAnsweredAt >= BOARD_SCHEDULE.staleMs) return "";
  const minute = Math.floor(now / 60_000) * 60_000;
  const held = boardScheduleGateWordsHeld.get(answer);
  if (held?.minute === minute && held.language === document.documentElement.lang) return held.words;
  const model = boardScheduleModel(answer, [], minute, { zoom: "now", folded: new Set(), opened: new Set(), log: null },
    false, { hourPx: 120, tickPx: 16, leastPx: 24 });
  const line = model.board.lines.find((one) => one.id === "gate");
  const words = line?.name ? [line.name, line.word, line.big === "—" ? "" : line.big].filter(Boolean).join(" · ") : "";
  boardScheduleGateWordsHeld.set(answer, { minute, language: document.documentElement.lang, words });
  return words;
}

/* 블록 하나(t-77628): 단추 하나에 이름(`<b>`, id는 가상 요소) — 요소 둘. 게이트의 블록만 단계 칸 줄(`<i>`)을
 * 하나 더 든다. 둘째 줄·셋째 줄·행동 칩은 `data-*`를 읽는 가상 요소다. 자리는 커스텀 속성(`--from`·`--to`:
 * 범위의 퍼센트, `--col`·`--cols`: 부열)이고 CSS가 시간당 높이로 푼다. */
function boardScheduleBlockNode(view, steps) {
  const block = boardScheduleElement("button", "board-schedule-block");
  block.type = "button";
  block.append(boardScheduleElement("b", "board-schedule-label"));
  if (steps) block.append(boardScheduleElement("i", "board-schedule-steps"));
  block.onclick = () => {
    if (block.closest(".board-schedule-week")) {
      boardScheduleAct(view, block.__row?.act);
      return;
    }
    // On the lanes a press picks the bar: the drawer beside them says its steps, its reason and its doors.
    boardScheduleChoice.selected = block.__row?.key ?? null;
    noteBoardScheduleChoice();
    paintBoardScheduleHosts();
  };
  return block;
}

/* 레인 하나(11-08 시안 v3): 이름 칸과 막대가 서는 길. */
function boardScheduleLaneNode() {
  const lane = boardScheduleElement("div", "board-schedule-lane");
  lane.append(boardScheduleElement("span", "board-schedule-lane-name"), boardScheduleElement("div", "board-schedule-lane-track"));
  return lane;
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

/* 게이트 블록의 단계 칸 줄: 칸 하나가 10 px, 틈 3 px — 요소가 아니라 그라디언트 하나다. 상태는 칸의 색이고
 * 블록의 둘째 줄이 같은 것을 수로 말한다. */
const BOARD_SCHEDULE_STEP_INKS = Object.freeze({
  done: "var(--signal-ready)", failed: "var(--signal-halt)", running: "var(--status-dot-run)",
  waiting: "var(--edge-mark)", held: "var(--edge-mark)", idle: "var(--edge-mark)",
});

function boardScheduleStepsPaint(steps) {
  const cell = BOARD_SCHEDULE.stepCellPx;
  const gap = BOARD_SCHEDULE.stepGapPx;
  const whole = steps.length * (cell + gap) - gap;
  // Stops as shares of the strip's width: a strip wider than a calendar cell shrinks with its cells instead of losing the last ones.
  const share = (px) => `${((px / whole) * 100).toFixed(2)}%`;
  const stops = steps.map((state, index) => {
    const ink = BOARD_SCHEDULE_STEP_INKS[state] ?? BOARD_SCHEDULE_STEP_INKS.idle;
    const start = index * (cell + gap);
    return `transparent ${share(start)}, ${ink} ${share(start)}, ${ink} ${share(start + cell)}, transparent ${share(start + cell)}`;
  });
  return { image: `linear-gradient(to right, ${stops.join(", ")})`, width: `${whole}px` };
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
    Boolean(row.cut), row.ride?.state ?? "", from, to, done, mark, row.col, row.cols, steps, actWord, row.depth,
    Boolean(row.cutEnd), row.flow ?? "", row.flowWords ?? "", row.group ?? "", row.slot ?? ""].join("\u0000");
  if (node.__painted === painted) return;
  node.__painted = painted;
  writeAttribute(node, "data-key", row.key);
  writeAttribute(node, "data-bar-state", row.state);
  writeAttribute(node, "data-basis", row.basis);
  writeAttribute(node, "data-depth", String(row.depth));
  writeAttribute(node, "data-tip", row.tip);
  writeAttribute(node, "data-said", row.said);
  writeAttribute(node, "data-why", row.why ?? "");
  writeAttribute(node, "data-steps", steps);
  writeAttribute(node, "data-act", actWord);
  writeAttribute(node, "aria-label", [row.label, row.sub, row.said, row.flowWords].filter(Boolean).join(" · "));
  if (row.act?.fold) writeAttribute(node, "aria-expanded", String(Boolean(row.opened)));
  else if (node.hasAttribute("aria-expanded")) node.removeAttribute("aria-expanded");
  writeClass(node, "is-paused", Boolean(row.paused));
  writeClass(node, "is-late", Boolean(row.late));
  writeClass(node, "is-cut", Boolean(row.cut));
  // The calendar's piece (t-84802): which edges carry on to another day, the group it belongs to, its line in the cell.
  if (row.group) {
    writeClass(node, "is-cut-end", Boolean(row.cutEnd));
    writeAttribute(node, "data-flow", row.flow);
    writeAttribute(node, "data-schedule-group", row.group);
    writeCustomProperty(node, "--slot", String(row.slot + 2));
  }
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

/* 축의 글(11-08 시안 v3): 눈금마다 시각 하나(`--at`), 칩 밑에 들면 글만 감춤; 접힌 칸마다 「⋯ 길이」 한 칸; 끝에 「지금」 칩. */
function paintBoardScheduleTicks(track, ticks, gaps) {
  const tickNodes = ticks.map((tick, at) => {
    const node = track.__ticks[at] ??= boardScheduleElement("span", "board-schedule-tick");
    writeBoardScheduleText(node, tick.label);
    writeCustomProperty(node, "--at", tick.at.toFixed(2));
    writeClass(node, "is-covered", tick.covered);
    return node;
  });
  const gapNodes = gaps.map((gap, at) => {
    const node = track.__gaps[at] ??= boardScheduleElement("span", "board-schedule-gap");
    writeBoardScheduleText(node, gap.words);
    writeAttribute(node, "data-tip", gap.tip);
    writeCustomProperty(node, "--from", gap.from.toFixed(2));
    writeCustomProperty(node, "--to", gap.to.toFixed(2));
    return node;
  });
  reconcileElementOrder(track, [...gapNodes, ...tickNodes, track.__now]);
}

/* 레인의 길 밑그림: 눈금마다 세로 선 하나, 접힌 칸마다 옅은 띠 하나 — 요소가 아니라 그라디언트 둘이다. */
function boardScheduleGridPaint(ticks, gaps) {
  const layers = [];
  if (ticks.length > 0) {
    layers.push(`linear-gradient(to right, ${ticks.map((tick) => {
      const at = `${tick.at.toFixed(2)}%`;
      return `transparent ${at}, var(--edge-rule) ${at}, var(--edge-rule) calc(${at} + var(--rule-width)), transparent calc(${at} + var(--rule-width))`;
    }).join(", ")})`);
  }
  if (gaps.length > 0) {
    layers.push(`linear-gradient(to right, ${gaps.map((gap) => {
      const [from, to] = [`${gap.from.toFixed(2)}%`, `${gap.to.toFixed(2)}%`];
      return `transparent ${from}, var(--surface-well) ${from}, var(--surface-well) ${to}, transparent ${to}`;
    }).join(", ")})`);
  }
  return layers.length > 0 ? layers.join(", ") : "none";
}

/* 축의 눈금 글 하나가 차지하는 자리(CSS px): 축 글자 크기의 `tickEm`배와 틈 — 스타일만 읽고 레이아웃은 묻지 않는다. */
function boardScheduleTickRoom(track) {
  // A tick's own size when one stands (a stylesheet may size the ticks apart from their track), else the track's.
  const tick = track.__ticks.find((one) => one.isConnected);
  const size = parseFloat(getComputedStyle(tick ?? track).fontSize) || 0;
  return size > 0 ? size * BOARD_SCHEDULE.tickEm + BOARD_SCHEDULE.tickGapPx : 0;
}

/* 묶음 하나의 레인들: 레인마다 이름과 막대들. 고른 막대는 `aria-pressed`로 서고, 막대는 키로 살아남는다. */
function paintBoardScheduleLanes(list, group, view, picked) {
  const held = new Map([...list.children].map((node) => [node.dataset.lane, node]));
  reconcileElementOrder(list, group.folded ? [] : group.lanes.map((lane) => {
    const node = held.get(lane.key) ?? boardScheduleLaneNode();
    writeAttribute(node, "data-lane", lane.key);
    writeCustomProperty(node, "--lane-rows", String(lane.depth));
    const [name, track] = node.children;
    writeBoardScheduleText(name, lane.name);
    writeAttribute(name, "data-tip", lane.name);
    const bars = new Map([...track.children].map((bar) => [bar.dataset.key, bar]));
    reconcileElementOrder(track, lane.rows.map((row) => {
      const steps = Array.isArray(row.steps);
      let bar = bars.get(row.key);
      if (bar && Boolean(bar.children[1]) !== steps) bar = null;
      bar ??= boardScheduleBlockNode(view, steps);
      paintBoardScheduleBlock(bar, row);
      writeAttribute(bar, "aria-pressed", String(row.key === picked));
      return bar;
    }));
    return node;
  }));
}

/* 쪽지가 말할 막대: 사람이 고른 것, 없으면 실패한 것, 도는 것, 첫 막대 차례(시안의 기본 고름). */
function boardSchedulePicked(model) {
  const rows = model.groups.filter((group) => group.standing && !group.folded)
    .flatMap((group) => group.lanes.flatMap((lane) => lane.rows));
  return rows.find((row) => row.key === boardScheduleChoice.selected)
    ?? rows.find((row) => row.state === "failed") ?? rows.find((row) => row.state === "running") ?? rows[0] ?? null;
}

/* 오른쪽 쪽지(11-08 시안 v3): 고른 막대의 이름과 상태, 그 문장과 둘째 줄, 까닭, 게이트면 단계마다 한 줄, 자세한 한 줄,
 * 그리고 그 막대의 문(로그·기록). 고를 막대가 없으면 무엇이 여기에 서는지 한 줄. */
function paintBoardScheduleDrawer(drawer, row) {
  drawer.__row = row;
  const [empty, title, said, sub, why, steps, tip, acts] = drawer.children;
  writeTextContent(empty, row ? "" : t("board.schedule.drawerEmpty", "막대를 누르면 그 일의 단계·까닭·로그가 여기에 섭니다."));
  writeHidden(empty, row !== null);
  for (const node of [title, said, sub]) writeHidden(node, row === null);
  if (!row) {
    for (const node of [why, steps, tip, acts]) writeHidden(node, true);
    reconcileElementOrder(steps, []);
    return;
  }
  const [dot, name] = title.children;
  writeAttribute(dot, "data-dot", BOARD_SCHEDULE_RUN_STATE[row.state] ?? "queue");
  writeBoardScheduleText(name, row.label);
  writeBoardScheduleText(said, row.said ?? "");
  writeBoardScheduleText(sub, row.sub ?? "");
  writeTextContent(why, row.why ?? "");
  writeHidden(why, !row.why);
  const names = Array.isArray(row.stepNames) ? row.stepNames : [];
  const items = [...steps.children];
  // One element a step: its state dot is the item's own `::before` (`.board-state`), its words the item's text.
  reconcileElementOrder(steps, names.map((stepName, at) => {
    const item = items[at] ?? boardScheduleElement("li", "board-state board-schedule-drawer-step");
    writeAttribute(item, "data-dot", BOARD_SCHEDULE_RUN_STATE[row.steps[at]] ?? "queue");
    writeBoardScheduleText(item, stepName);
    return item;
  }));
  writeAttribute(steps, "aria-label", t("board.schedule.drawerSteps", "단계 {{count}}", { count: names.length }));
  writeHidden(steps, names.length === 0);
  writeTextContent(tip, row.tip ?? "");
  writeHidden(tip, !row.tip);
  const word = boardScheduleActWord(row.act);
  writeTextContent(acts.firstChild, word);
  writeHidden(acts, word === "");
}

/* 안내판 넉 줄을 쓴다(t-77628): 줄마다 무엇·큰 글·낱말·비고·색 띠·행동 칩. */
function paintBoardScheduleBoard(parts, board) {
  writeBoardScheduleText(parts.boardTitle, board.head.title);
  writeTextContent(parts.boardNow, board.head.now);
  writeTextContent(parts.boardSum, board.head.sum);
  for (const [index, line] of board.lines.entries()) {
    const node = parts.lines[index];
    node.__act = line.act;
    writeHidden(node, !line.standing);
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
    const tip = BOARD_SCHEDULE_LINE_TIPS[line.id];
    if (tip) writeAttribute(node, "data-tip", t(tip.key, tip.word));
  }
}

/* 달력의 일곱 칸을 쓴다(t-84802): 칸은 그 자리에 있고, 머리와 조각을 그 칸의 키로 맞춘다. 칸을 옮겨 간 조각은 새로 짓는다. */
function paintBoardScheduleWeek(parts, week, view) {
  // The cells are built when the calendar opens; a person who stacked it wants today, and only at that moment (review P3-5).
  const opening = parts.weekDays.children.length === 0;
  if (opening) {
    parts.weekDays.append(...boardScheduleWeekCells());
    parts.weekDays.__stop = null;
  }
  const cells = parts.weekDays.children;
  writeCustomProperty(parts.weekDays, "--schedule-week-rows", String(week.rows));
  writeAttribute(parts.week, "aria-label", t("board.schedule.weekName", "{{back}}일 전부터 내일까지 일곱 날 · 방향키로 움직임", {
    back: BOARD_SCHEDULE.zooms.week.daysBack,
  }));
  for (const [index, day] of week.days.entries()) {
    const cell = cells[index];
    writeAttribute(cell, "data-day", String(day.at));
    writeAttribute(cell, "data-day-end", String(day.end));
    writeAttribute(cell, "aria-label", day.name);
    writeAttributeOrNone(cell, "aria-current", day.today ? "date" : null);
    writeBoardScheduleText(cell.__head, day.head);
    writeAttribute(cell.__head, "data-now", day.now);
    writeAttribute(cell.__head, "data-count", String(day.count));
    writeAttribute(cell.__head, "data-partial", day.partial ? "+" : "");
    writeAttribute(cell.__head, "data-tip", day.tip);
    const held = new Map([...cell.querySelectorAll(".board-schedule-block")].map((node) => [node.dataset.key, node]));
    const nodes = day.pieces.map((piece) => {
      const steps = Array.isArray(piece.steps);
      let node = held.get(piece.key);
      if (node && Boolean(node.children[1]) !== steps) node = null;
      if (!node) {
        node = boardScheduleBlockNode(view, steps);
        node.tabIndex = -1;
      }
      paintBoardScheduleBlock(node, piece);
      return node;
    });
    reconcileElementOrder(cell, [cell.__head, ...nodes, cell.__more, cell.__less]);
    const { __more: more, __less: less } = cell;
    writeHidden(more, day.more === 0);
    writeHidden(less, day.unfolded === 0);
    if (day.more > 0) {
      writeBoardScheduleText(more, t("board.schedule.moreInDay", "+{{count}}개 더", { count: day.more }));
      writeAttribute(more, "aria-label", t("board.schedule.moreInDayName", "{{date}} 블록 {{count}}개 더 보기", { date: day.full, count: day.more }));
      writeCustomProperty(more, "--slot", String(day.moreSlot + 2));
    }
    if (day.unfolded > 0) {
      writeBoardScheduleText(less, t("board.schedule.lessInDay", "{{count}}개 접기", { count: day.unfolded }));
      writeAttribute(less, "aria-label", t("board.schedule.lessInDayName", "{{date}} 블록 {{count}}개 접기", { date: day.full, count: day.unfolded }));
      writeCustomProperty(less, "--slot", String(day.moreSlot + (day.more > 0 ? 3 : 2)));
    }
  }
  // The one Tab stop stays where the person left it, while that block still stands; else today's cell.
  const stop = parts.weekDays.__stop;
  if (!stop || !stop.isConnected || stop.hidden) {
    const today = week.days.findIndex((day) => day.today);
    boardScheduleWeekStop(parts.weekDays, cells[Math.max(0, today)]);
  }
  // Stacked, today is the sixth day down. Side by side it is on screen already, and this reads the layout only here.
  if (opening && cells[0].offsetTop !== cells[1].offsetTop) cells[week.days.findIndex((day) => day.today)]?.scrollIntoView({ block: "start" });
}

/* 그릇 하나를 그린다. 답·데스크·분·고른 것·언어가 지난번과 같으면 아무것도 쓰지 않는다 — 조용한 박자는 DOM을 건드리지
 * 않고, 폭도 읽지 않는다: 폭에 따른 접힘은 스타일시트의 것이다. 블록과 칸의 말은 분 단위의 지금에 선다: 같은 분 안의
 * 박자는 바뀐 블록만 고쳐 쓴다(t-67482). 안내판은 세 확대가 함께 쓴다. 지금 앞뒤와 하루는 세로 시간표에(t-77628), 한 주는
 * 일곱 칸 달력에(t-84802) 그린다: 묶음 상자 넷을 그쪽으로 옮기고, 블록을 키로 맞춘다. */
function paintBoardScheduleHost(host, view, now) {
  if (!boardScheduleBuilt.has(host)) {
    boardScheduleSkeleton(host, view);
    boardScheduleBuilt.add(host);
  }
  const parts = host.__parts;
  const timeline = boardScheduleTimeline(boardScheduleChoice.zoom);
  const desk = typeof deskLedger === "undefined" ? null : deskLedger;
  // The machine strip's own reading (`machine_load`, shell-board.js): read, never asked a second time.
  const machine = typeof deskMachine === "undefined" ? null : deskMachine;
  const minute = Math.floor(now / BOARD_SCHEDULE.clockMs) * BOARD_SCHEDULE.clockMs;
  const answer = boardScheduleHere();
  const trackPx = timeline ? parts.track.__width : 0;
  const said = [answer, desk, machine, minute, boardScheduleChoiceRevision,
    document.documentElement.lang, timeline, trackPx];
  const before = boardSchedulePainted.get(host);
  if (before && before.every((one, at) => one === said[at])) return;
  boardSchedulePainted.set(host, said);
  // The tick's room is read only when the schedule is drawn again — a look that changes nothing asks no style.
  const metrics = timeline ? { trackPx, tickRoomPx: boardScheduleTickRoom(parts.track) } : null;
  const model = boardScheduleModel(answer, desk?.tasks, minute, boardScheduleChoice, machine?.load?.loud === true, metrics);
  paintBoardScheduleHead(parts.head, model);
  writeAttribute(host, "data-schedule-zoom", boardScheduleChoice.zoom);
  writeHidden(parts.day, !timeline);
  writeHidden(parts.week, timeline);
  paintBoardScheduleBoard(parts, model.board);
  const picked = timeline ? boardSchedulePicked(model) : null;
  if (timeline) {
    paintBoardScheduleTicks(parts.track, model.gutter, model.gaps);
    writeTextContent(parts.track.__now, model.board.head.now);
    writeTextContent(parts.gutterHead, boardScheduleChoice.zoom === "day"
      ? t("board.schedule.axisDay", "하루 · 빈 시간 접힘") : t("board.schedule.axisNow", "지금 앞뒤"));
    writeCustomProperty(parts.body, "--now", model.nowAt.toFixed(2));
    // The scale the bars were placed by: a scene reads a moment's place by it (the day folds its empty stretches).
    parts.body.__scale = model.scale;
    writeCustomProperty(parts.body, "--schedule-hours", model.hours.toFixed(3));
    writeCustomProperty(parts.body, "--schedule-grid", boardScheduleGridPaint(model.gutter, model.gaps));
    const legend = [
      t("board.schedule.legendBlocks", "칠함 = 지난·끝남 · 빗금 = 앞으로(지난 소요로 잰 예상) · 점선 = 근거 없음 · 톱니 = 범위 앞에서 시작"),
      model.gaps.length > 0 ? t("board.schedule.legendGaps", "⋯ = 막대가 없는 빈 시간을 접은 칸") : "",
    ].filter(Boolean).join(" · ");
    writeAttribute(parts.gutter, "data-tip", legend);
    writeTextContent(parts.calLegend, legend);
    writeTextContent(parts.aheadEmpty, t("board.schedule.aheadEmpty", "앞으로 잡힌 일 없음"));
    writeHidden(parts.aheadEmpty, !model.aheadEmpty);
    reconcileElementOrder(parts.weekGroups, []);
    reconcileElementOrder(parts.weekDays, []);
    reconcileElementOrder(parts.body, [parts.gutter, ...parts.boxes, parts.aheadEmpty]);
    paintBoardScheduleDrawer(parts.drawer, picked);
  } else {
    writeTextContent(parts.calLegend, t("board.schedule.legendWeek", "칠함 = 지난·끝남 · 빗금 = 앞으로(지난 소요로 잰 예상) · 점선 = 근거 없음 · 납작한 끝 = 옆 날로 이어짐 · 톱니 = 이 범위 밖으로 이어짐"));
    reconcileElementOrder(parts.body, [parts.gutter, parts.aheadEmpty]);
    // The lanes' axis and drawer hold nothing while the calendar stands: the week's weight is the week's alone.
    paintBoardScheduleTicks(parts.track, [], []);
    paintBoardScheduleDrawer(parts.drawer, null);
    reconcileElementOrder(parts.weekGroups, parts.boxes);
    paintBoardScheduleWeek(parts, model.week, view);
  }
  const queueWord = t(BOARD_RUN_STATES.queue.key, BOARD_RUN_STATES.queue.word);
  const doneWord = t(BOARD_RUN_STATES.done.key, BOARD_RUN_STATES.done.word);
  let shown = 0;
  for (const [index, group] of model.groups.entries()) {
    const box = parts.boxes[index];
    const [fold, list, more] = box.children;
    writeHidden(box, !group.standing);
    writeBoardScheduleText(fold, group.label);
    const groupTip = BOARD_SCHEDULE_GROUPS[index].tip;
    writeAttribute(fold, "data-tip", t(groupTip.key, groupTip.word));
    // The group's head counts its bars (t-77628): 「7 · 차례 기다림 4 · 끝남 3」 — a zero is left out —, or that the
    // range holds none.
    writeAttribute(fold, "data-sub", group.count === 0 && group.more === 0
      ? t("board.schedule.notInRange", "이 범위에 없음")
      : [String(group.count), group.waiting > 0 ? `${queueWord} ${group.waiting}` : "", group.ended > 0 ? `${doneWord} ${group.ended}` : ""]
        .filter(Boolean).join(" · "));
    writeAttribute(fold, "aria-expanded", String(!group.folded));
    // A group stays when it is empty — its head says 「이 범위에 없음」 —, unless this project has nothing to fill it
    // (`standing`). On the calendar the box is its head alone: its blocks stand in the day cells.
    if (timeline) paintBoardScheduleLanes(list, group, view, picked?.key ?? null);
    else reconcileElementOrder(list, []);
    writeHidden(list, !timeline || group.folded);
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
    : answer ? t("board.schedule.empty", "이 범위에 일정이 없어요") : t("board.schedule.reading", "읽는 중…"));
  writeHidden(parts.empty, shown !== 0);
  paintBoardScheduleLog(parts.log);
}

/* 확대 하나의 말. */
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
  const line = boardScheduleHere()?.line ?? {};
  const words = [
    line.paused
      ? line.pause_note
        ? t("board.schedule.linePaused", "줄 멈춤 · {{note}}", { note: line.pause_note })
        : t("board.schedule.linePausedPlain", "줄 멈춤")
      : "",
    line.disk_low ? t("board.schedule.diskLow", "디스크가 하한 아래라 줄이 기다림") : "",
    line.quiet_wanted ? t("board.schedule.quiet", "측정이 조용한 기계를 기다림") : "",
    model.leftOut ? t("board.schedule.jobsLeftOut", "끝난 일감이 많아 {{time}} 앞의 {{count}}개는 보이지 않음", {
      count: model.leftOut.count,
      time: boardScheduleRemembered(`left-out:${model.leftOut.before}:${document.documentElement.lang}`, () => new Date(model.leftOut.before)
        .toLocaleString(document.documentElement.lang || undefined, { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23" })),
    }) : "",
  ].filter(Boolean).join(" · ");
  writeTextContent(note, words);
  writeHidden(note, words === "");
}

/* 연 로그의 이름: 일감은 제 이름, 빌드의 실행은 답이 그 실행을 부르는 이름이고 답에 없으면 빌드의 id다. */
function boardScheduleLogName(asked) {
  if (asked.job) return asked.job;
  const run = asked.build ?? {};
  const answer = boardScheduleHere();
  const held = [...(answer?.builds ?? []), answer?.last_build]
    .find((build) => build?.id === run.id && build.started_ms === run.started_ms);
  return held ? buildTitleWords(held) : String(run.id ?? "");
}

/* 연 로그: 이름, 닫기, 끝 줄들. 읽는 중이면 그렇다고, 없으면 그렇다고. */
function paintBoardScheduleLog(log) {
  const asked = boardScheduleChoice.log;
  writeHidden(log, !asked);
  if (!asked) return;
  const [head, lines] = log.children;
  const [title, close] = head.children;
  writeTextContent(title, t("board.schedule.log", "로그 · {{name}}", { name: boardScheduleLogName(asked) }));
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
  wanted: () => boardScheduleHosts().length > 0 || stackedWorkIsUp(),
  every: BOARD_SCHEDULE.everyMs,
  tick: () => refreshBoardSchedule(),
  onResume: () => refreshBoardSchedule({ fresh: true }),
});
