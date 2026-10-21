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
  /* 두 확대: 지금 앞뒤로 보이는 길이. 뒤쪽은 가장 늦은 예상 끝까지 늘어나되 상한이
   * 있다(`mostAfterMs`). */
  zooms: Object.freeze({
    /* 눈금 간격의 후보(`ticksMs`): 축의 폭에 눈금 글이 들어가는 가장 촘촘한 것을 고른다.
     * 하루는 시각으로, 한 주는 날짜로 읽는다. */
    day: Object.freeze({
      beforeMs: 16 * 3_600_000, afterMs: 8 * 3_600_000, mostAfterMs: 24 * 3_600_000,
      ticksMs: Object.freeze([3_600_000, 2 * 3_600_000, 3 * 3_600_000, 6 * 3_600_000, 12 * 3_600_000]),
    }),
    week: Object.freeze({
      beforeMs: 5 * 86_400_000, afterMs: 2 * 86_400_000, mostAfterMs: 4 * 86_400_000,
      ticksMs: Object.freeze([86_400_000, 2 * 86_400_000]),
    }),
  }),
  /* 눈금 글 하나가 차지하는 폭(CSS px): 축의 폭을 이것으로 나눈 수만큼 눈금이 선다. */
  tickRoomPx: 72,
  /* 게이트 밖 일감 가운데 끝난 것은 가장 최근의 이만큼만 행으로 선다. 나머지는 수다. */
  lineShown: 12,
  /* 가장 늦은 예상 끝 뒤에 남기는 여백. */
  tailMs: 30 * 60_000,
});

/* 묶음 넷, 읽는 차례대로. */
const BOARD_SCHEDULE_GROUPS = Object.freeze([
  { id: "lanes", key: "board.schedule.groupLanes", word: "발행 레인" },
  { id: "gates", key: "board.schedule.groupGates", word: "결합 게이트" },
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

/* 워커 과업의 단계(데스크의 `DESK_STAGES` 낱말)마다 막대의 상태. */
const BOARD_SCHEDULE_TASK_STATES = Object.freeze({
  dispatched: "running",
  reported: "waiting",
  verified: "waiting",
  stale: "waiting",
  merged: "done",
  deployed: "done",
  nothing_to_land: "done",
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
const boardScheduleChoice = { zoom: "day", folded: new Set(), opened: new Set(), log: null };
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

/* 보이는 범위의 왼쪽 끝: 백엔드는 이보다 먼저 끝난 것을 보내지 않는다. */
function boardScheduleFrom() {
  const zoom = BOARD_SCHEDULE.zooms[boardScheduleChoice.zoom] ?? BOARD_SCHEDULE.zooms.day;
  return Date.now() - zoom.beforeMs;
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
      if (answer && typeof answer === "object" && !answer.unchanged) boardScheduleAnswer = answer;
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
  return { end, lateMs: Math.max(0, now - due), known: ahead.every((stage) => Number(stage.expected?.samples) > 0) };
}

/* 길이 한 마디: 1분이 안 되면 그렇다고. 글은 분 수로 정해진다. */
function boardScheduleSpan(ms) {
  const minutes = ms > 0 ? Math.floor(ms / 60_000) : -1;
  return boardScheduleRemembered(`span:${minutes}:${document.documentElement.lang}`,
    () => usageDuration(ms) ?? t("board.desk.underMinute", "1분 미만"));
}

/* 시각 한 마디(데스크의 `batchClock`): 같은 분의 순간은 같은 글이다. */
function boardScheduleClock(ms, now) {
  const near = Math.abs(now - ms) < 86_400_000;
  return boardScheduleRemembered(`clock:${Math.floor(ms / 60_000)}:${near}:${document.documentElement.lang}`,
    () => batchClock(ms, now));
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

/* 행 하나의 모양: 그릴 것은 이것뿐이다. */
function boardScheduleRow(fields) {
  return { depth: 0, basis: "history", marks: null, act: null, ...fields };
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
  return boardScheduleRow({
    key: `job:${job.name}`,
    depth,
    label: short,
    tip: [job.name, job.who, job.what, boardScheduleBasis(job.expected)].filter(Boolean).join(" · "),
    start,
    end,
    state: BOARD_SCHEDULE_JOB_STATES[job.state] ?? "idle",
    basis: job.expected && Number(job.expected.samples) === 0 && ["running", "waiting", "held"].includes(job.state)
      ? "cap" : "history",
    said: boardScheduleJobSaid(job, place, now, stall),
    act: { log: { job: job.name } },
    paused: Boolean(stall) && job.state === "waiting",
    late: job.state === "running" && Boolean(place?.late),
  });
}

function boardScheduleLaneRows(lanes, now) {
  return lanes.map((run) => {
    const stages = Array.isArray(run.stages) ? run.stages : [];
    const tip = stages.map((stage) => Number.isFinite(stage.end_ms)
      ? t("board.schedule.tipStage", "{{name}} {{time}}", {
        name: stage.name, time: boardScheduleSpan(stage.end_ms - stage.start_ms),
      })
      : stage.name).join(" · ");
    const label = run.version ? `v${run.version}` : String(run.sha);
    if (run.running) {
      const { end, lateMs, known } = boardScheduleLaneEnd(run, now);
      const stage = stages[stages.length - 1]?.name ?? "";
      return boardScheduleRow({
        key: `lane:${run.sha}:${run.start_ms}`, label, tip, start: run.start_ms, end, state: "running",
        basis: known ? "history" : "none", late: lateMs > 0,
        said: lateMs > 0
          ? t("board.schedule.laneLate", "{{stage}} · +{{late}} 지연 중 · {{time}} 남음", {
            stage, late: boardScheduleSpan(lateMs), time: boardScheduleSpan(end - now),
          })
          : t("board.schedule.laneRunning", "{{stage}} · {{time}} 남음", { stage, time: boardScheduleSpan(end - now) }),
        act: { log: { lane: run.sha } },
      });
    }
    const verdict = run.verdict === "green" || run.verdict === "red"
      ? t(DESK_RELEASE_VERDICTS[run.verdict].key, DESK_RELEASE_VERDICTS[run.verdict].word, { phase: "" })
      : String(run.verdict ?? t("board.schedule.lost", "끝 기록 없음"));
    const end = Number(run.end_ms) || run.start_ms;
    return boardScheduleRow({
      key: `lane:${run.sha}:${run.start_ms}`, label, tip: [tip, run.reason].filter(Boolean).join(" · "),
      start: run.start_ms, end, state: run.verdict === "green" ? "done" : run.verdict === "red" ? "failed" : "idle",
      said: t("board.schedule.laneEnded", "{{verdict}} · {{time}}", { verdict, time: boardScheduleSpan(end - run.start_ms) }),
      act: { log: { lane: run.sha } },
    });
  });
}

/* 결합 게이트: 이름의 `-l<NN>` 토막마다 한 행, 시작 차례로. 펴면 그 일감들이 바로
 * 아래에 선다. 도는 일감이나 기다리는 일감이 있는 게이트는 처음부터 펴져 있다. */
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
    entries.push({
      head: boardScheduleRow({
        key: `gate:${gate}`, label: parts.length > 0 ? `${gate} (${parts.join("·")})` : gate,
        tip: members.map((job) => job.name).join(" · "), start, end, state,
        basis: placed.some((row) => row.basis === "cap") ? "cap" : "history", said,
        act: { fold: gate, opened }, opened, paused: Boolean(stall) && state === "waiting",
      }),
      rows: opened ? placed.sort((a, b) => a.start - b.start) : [],
    });
  }
  return entries.sort((a, b) => a.head.start - b.head.start).flatMap((entry) => [entry.head, ...entry.rows]);
}

/* 게이트 밖의 일감: 도는 것·기다리는 것은 다, 끝난 것은 가장 최근의 몇만. */
function boardScheduleLineRows(jobs, plan, now, stall, from) {
  const outside = jobs.filter((job) => !job.gate);
  const open = outside.filter((job) => ["running", "waiting", "held", "lost"].includes(job.state));
  const ended = outside.filter((job) => !open.includes(job) && (Number(job.end_ms) || 0) >= from)
    .sort((a, b) => (Number(b.end_ms) || 0) - (Number(a.end_ms) || 0));
  const shown = [...ended.slice(0, BOARD_SCHEDULE.lineShown), ...open];
  return {
    rows: shown.map((job) => boardScheduleJobRow(job, plan, now, stall, 0)).sort((a, b) => a.start - b.start),
    more: Math.max(0, ended.length - BOARD_SCHEDULE.lineShown),
  };
}

/* 워커 과업: 데스크가 읽은 원장의 과업 가운데 시각을 가진 것. 막대는 첫 시도의
 * 시작부터 착지까지(아직이면 지금까지), 그 안에 넘김과 검증의 자리를 표시한다. */
function boardScheduleTaskRows(tasks, now) {
  const rows = [];
  for (const task of Array.isArray(tasks) ? tasks : []) {
    const times = task?.times;
    // `Number(null)` is 0, a moment in 1970: a time the ledger has not written is no time.
    const start = Number(times?.started_ms ?? NaN);
    if (!Number.isFinite(start) || !(task.stage in BOARD_SCHEDULE_TASK_STATES)) continue;
    const landed = Number(times.landed_ms ?? NaN);
    const end = Number.isFinite(landed) ? landed : now;
    const stage = DESK_STAGES.find((one) => one.id === task.stage);
    const word = stage ? t(stage.key, stage.word) : String(task.stage);
    const since = Math.max(Number(times.verified_ms) || 0, Number(times.handed_ms) || 0, start);
    const state = BOARD_SCHEDULE_TASK_STATES[task.stage];
    const said = state === "done"
      ? t("board.schedule.taskLanded", "{{stage}} · {{time}} 걸림", { stage: word, time: boardScheduleSpan(end - start) })
      : state === "running"
        ? t("board.schedule.taskRunning", "{{stage}} · {{time}}째", { stage: word, time: boardScheduleSpan(now - start) })
        : t("board.schedule.taskWaiting", "{{stage}} · 기다린 지 {{time}}", { stage: word, time: boardScheduleSpan(now - since) });
    const at = (ms) => (ms != null && Number.isFinite(Number(ms)) && end > start ? (Number(ms) - start) / (end - start) : null);
    const clock = (ms) => (ms != null && Number.isFinite(Number(ms)) ? boardScheduleClock(Number(ms), now) : "—");
    rows.push(boardScheduleRow({
      key: `task:${task.run}/${task.id}`, label: `${task.id} · ${task.title}`,
      tip: [`${task.id} · ${task.title}`, task.run, t("board.schedule.tipTask",
        "시작 {{start}} · 넘김 {{handed}} · 검증 {{verified}} · 착지 {{landed}}", {
          start: clock(times.started_ms), handed: clock(times.handed_ms),
          verified: clock(times.verified_ms), landed: clock(times.landed_ms),
        })].filter(Boolean).join(" · "),
      start, end, state, said, marks: { handed: at(times.handed_ms), verified: at(times.verified_ms) },
      act: { task: { run: task.run, id: task.id, stage: task.stage } },
    }));
  }
  return rows.sort((a, b) => a.start - b.start);
}

/* 눈금: 축의 폭에 들어가는 가장 촘촘한 간격으로, 그 간격의 현지 시각 경계마다. */
function boardScheduleTicks(from, to, width, steps) {
  const room = Math.max(2, Math.floor(width / BOARD_SCHEDULE.tickRoomPx));
  const step = steps.find((ms) => (to - from) / ms <= room) ?? steps.at(-1);
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
        : new Date(moment).toLocaleTimeString(tongue, { hour: "2-digit", minute: "2-digit" }))),
    });
  }
  return ticks;
}

/* 그릴 것 한 벌: 보이는 범위, 눈금, 묶음 넷과 그 행들. 백엔드의 답과 데스크의 과업,
 * 지금, 고른 것, 축의 폭에서 DOM 없이 짓는다 — 장면 시험이 이것을 그대로 읽는다. */
function boardScheduleModel(answer, tasks, now, choice, width, loud = false) {
  const zoom = BOARD_SCHEDULE.zooms[choice.zoom] ?? BOARD_SCHEDULE.zooms.day;
  const jobs = Array.isArray(answer?.jobs) ? answer.jobs : [];
  const lanes = Array.isArray(answer?.lanes) ? answer.lanes : [];
  const facts = answer?.line ?? {};
  const runnerLanes = Array.isArray(facts.runner_lanes) ? facts.runner_lanes : [];
  const stall = boardScheduleStall(facts, runnerLanes);
  const plan = boardScheduleQueuePlan(jobs, runnerLanes, now);
  const from = now - zoom.beforeMs;
  const line = boardScheduleLineRows(jobs, plan, now, stall, from);
  const built = {
    lanes: boardScheduleLaneRows(lanes, now),
    gates: boardScheduleGateRows(jobs, plan, now, choice, stall),
    line: line.rows,
    workers: boardScheduleTaskRows(tasks, now),
  };
  const latest = Math.max(now, ...Object.values(built).flat()
    .filter((row) => ["running", "waiting", "held"].includes(row.state)).map((row) => row.end));
  const to = Math.min(now + zoom.mostAfterMs, Math.max(now + zoom.afterMs, latest + BOARD_SCHEDULE.tailMs));
  const span = Math.max(1, to - from);
  const percent = (ms) => Math.min(100, Math.max(0, ((ms - from) / span) * 100));
  const groups = BOARD_SCHEDULE_GROUPS.map((group) => {
    const rows = built[group.id].filter((row) => row.end >= from && row.start <= to).map((row) => ({
      ...row,
      // 기계가 붐비면(1분 부하 > 코어 수, 데스크의 기계 띠와 같은 판정) 앞으로 남은 예상 곁에 그렇다고만
      // 적는다 — 값은 바꾸지 않는다(코디 m-67652).
      said: loud && row.depth === 0 && ["running", "waiting"].includes(row.state)
        ? t("board.schedule.loud", "{{said}} · 부하 높음·늦어질 수 있음", { said: row.said }) : row.said,
      from: percent(row.start),
      to: percent(Math.max(row.end, row.start)),
      done: row.state === "running" && row.end > row.start
        ? Math.min(1, Math.max(0, (now - row.start) / (row.end - row.start))) : row.state === "waiting" ? 0 : 1,
    }));
    return {
      id: group.id,
      label: t(group.key, group.word),
      folded: choice.folded.has(group.id),
      count: rows.filter((row) => row.depth === 0).length,
      rows,
      more: group.id === "line" ? line.more : 0,
    };
  });
  return { from, to, now, nowAt: percent(now), ticks: boardScheduleTicks(from, to, width, zoom.ticksMs).map((tick) =>
    ({ ...tick, at: percent(tick.at) })), groups, stall };
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
  const chart = boardScheduleElement("div", "board-schedule-chart");
  const axis = boardScheduleElement("div", "board-schedule-axis");
  axis.setAttribute("aria-hidden", "true");
  const now = boardScheduleElement("span", "board-schedule-now");
  now.setAttribute("aria-hidden", "true");
  chart.append(axis, now);
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
    box.append(fold, rows, boardScheduleElement("p", "board-schedule-more"));
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
  const handed = Number.isFinite(row.marks?.handed) ? (row.marks.handed * 100).toFixed(1) : "";
  const verified = Number.isFinite(row.marks?.verified) ? (row.marks.verified * 100).toFixed(1) : "";
  // What the row was last painted with: a row the beat did not move reads nothing back from
  // the DOM (twelve reads a row, fifty rows a beat, were most of a moved beat's cost after the
  // words were remembered — t-67482).
  const painted = [row.key, row.state, row.basis, row.depth, row.tip, row.label, row.said, Boolean(row.act?.fold),
    Boolean(row.opened), Boolean(row.paused), Boolean(row.late), from, to, done, handed, verified].join("\u0000");
  if (node.__painted === painted) return;
  node.__painted = painted;
  writeAttribute(node, "data-key", row.key);
  writeAttribute(node, "data-bar-state", row.state);
  writeAttribute(node, "data-basis", row.basis);
  writeAttribute(node, "data-depth", String(row.depth));
  writeAttribute(node, "data-tip", row.tip);
  writeAttribute(node, "aria-label", `${row.label} · ${row.said}`);
  if (row.act?.fold) writeAttribute(node, "aria-expanded", String(Boolean(row.opened)));
  else if (node.hasAttribute("aria-expanded")) node.removeAttribute("aria-expanded");
  writeClass(node, "is-paused", Boolean(row.paused));
  writeClass(node, "is-late", Boolean(row.late));
  const [label, bar, said] = node.children;
  writeBoardScheduleText(label, row.label);
  writeBoardScheduleText(said, row.said);
  writeCustomProperty(bar, "--from", from);
  writeCustomProperty(bar, "--to", to);
  writeCustomProperty(bar, "--done", done);
  writeAttribute(node, "data-marks", [handed && "handed", verified && "verified"].filter(Boolean).join(" "));
  writeCustomProperty(bar, "--handed", handed);
  writeCustomProperty(bar, "--verified", verified);
}

/* 그릇 하나를 그린다. 답·데스크·분·고른 것·언어·눈금 수가 지난번과 같으면 아무것도
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
  const [axis, nowLine, ...boxes] = chart.children;
  const width = axis.clientWidth;
  const desk = typeof deskLedger === "undefined" ? null : deskLedger;
  // The machine strip's own reading (`machine_load`, shell-board.js): read, never asked a second time.
  const machine = typeof deskMachine === "undefined" ? null : deskMachine;
  const minute = Math.floor(now / BOARD_SCHEDULE.clockMs) * BOARD_SCHEDULE.clockMs;
  const said = [boardScheduleAnswer, desk, machine, minute, boardScheduleChoiceRevision,
    document.documentElement.lang, Math.floor(width / BOARD_SCHEDULE.tickRoomPx)];
  const before = boardSchedulePainted.get(host);
  if (before && before.every((one, at) => one === said[at])) return;
  boardSchedulePainted.set(host, said);
  const model = boardScheduleModel(boardScheduleAnswer, desk?.tasks, minute, boardScheduleChoice, width,
    machine?.load?.loud === true);
  paintBoardScheduleHead(head, model);
  const ticks = [...axis.children];
  reconcileElementOrder(axis, model.ticks.map((tick, at) => {
    const node = ticks[at] ?? boardScheduleElement("span", "board-schedule-tick");
    writeTextContent(node, tick.label);
    writeCustomProperty(node, "--at", tick.at.toFixed(2));
    return node;
  }));
  writeCustomProperty(chart, "--now", model.nowAt.toFixed(2));
  writeAttribute(nowLine, "data-said", t("board.schedule.nowLine", "지금 {{time}}", { time: boardScheduleClock(minute, minute) }));
  let shown = 0;
  for (const [index, group] of model.groups.entries()) {
    const box = boxes[index];
    const [fold, list, more] = box.children;
    writeBoardScheduleText(fold, t("board.schedule.groupHead", "{{name}} · {{count}}", { name: group.label, count: group.count }));
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
    const moreWords = group.more > 0 && !group.folded
      ? t("board.schedule.more", "끝난 일감 {{count}}개 더 — 이 범위의 더 이른 것", { count: group.more }) : "";
    writeTextContent(more, moreWords);
    writeHidden(more, moreWords === "");
    shown += group.rows.length;
  }
  writeTextContent(empty, shown > 0 ? ""
    : boardScheduleAnswer ? t("board.schedule.empty", "이 범위에 일정이 없어요") : t("board.schedule.reading", "읽는 중…"));
  writeHidden(empty, shown !== 0);
  paintBoardScheduleLog(log);
}

/* 머리: 확대 둘과, 빌드 줄이 왜 서 있는지 한 줄. */
function paintBoardScheduleHead(head, model) {
  const [zoom, note] = head.children;
  writeAttribute(zoom, "aria-label", t("board.schedule.zoom", "확대"));
  for (const button of zoom.children) {
    const id = button.dataset.scheduleZoom;
    writeTextContent(button, id === "week" ? t("board.schedule.week", "한 주") : t("board.schedule.day", "하루"));
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
