/* ---- 실시간 조율 지도 (t-7288) ---------------------------------------------
 *
 * 보드의 `관계` 그림에 켜고 끄는 손잡이 하나. 켜면 세 가지를 더 그린다 —
 * 누가 누구에게 일을 맡겼는가, 누가 무엇을 기다리는가, 그리고 방금 실제로
 * 일어난 사건 하나. 작업 목록은 기본값 그대로이고(`agentBoardMode`), 세 번째
 * 모드도 두 번째 렌더러도 없다.
 *
 * 이 파일이 드는 것은 **본 사건 장부** 하나다. 그리는 손은 이 장부를 읽기만
 * 한다. 장부가 따로 서 있는 이유는 「방금 생긴 일」이라는 판정이 그림의 일이
 * 아니기 때문이다: 같은 판을 두 번 그려도, 숨겼다 돌아와도, 오래된 snapshot이
 * 늦게 도착해도 사건의 수는 달라지지 않아야 한다.
 *
 * **지어내지 않는 것들.** 이 파일은 `invoke`를 부르지 않는다 — 새 백엔드
 * 호출도, 모델 호출도, 메일 확인/답장도 없다. 보는 일이 우편을 소비하면
 * 코디네이터의 받은함이 보는 것만으로 비워진다. 그리고 원장에 없는 사실은
 * 그리지 않는다: 없는 것은 `unknown`이고, 잘린 snapshot은 「완료」가 아니다.
 * 원장의 행도 전역으로 들고 있지 않는다 — 판 하나가 물은 한 벌(`boardCardsAndLedger`)이
 * 카드와 함께 이 파일로 건너오고, 그 판의 모델이 그 한 벌을 제 `source`에 든다.
 *
 * **자리는 신원이 아니다.** `term:N`은 이 화면 안에서 노드를 찾는 **탐색키**일
 * 뿐이고, 주체는 `run/worker/dispatch` 셋이다. 그래서 사건과 맥박은 제가 일어난
 * 때의 주체를 적어 두고(`seats`), 수의 기준선은 같은 두 주체 사이에서만 잇는다
 * (`ends`). 판이 다시 앉거나 시도가 바뀌면 그 자리의 새 주체는 앞 주체의 맥박도,
 * 수의 기준선도, 사건의 근거도 물려받지 않는다. 셋 중 하나라도 없는 카드는
 * `unverified`이고, 그런 카드는 배정·결과 사건을 만들지 않는다.
 *
 * **revision이 없는 스트림.** `GraphOverlaySnapshot`에는 revision 필드가
 * 없다. 그래서 여기서도 만들지 않는다. 최신성은 셋을 **함께** 써서 판정한다 —
 * 실제 event id, 관계마다의 stamp watermark, 그리고 판이 물은 차례와 범위의
 * 세대(`boardCommit`, shell.js). stamp 하나만으로는 같은 시각의 두 사건을 가릴
 * 수 없으므로, watermark는 **엄격히 옛것**만 버리고 같은 시각의 다른 id는 서로
 * 다른 실제 사건으로 받는다. */

/* 손잡이. 기본은 꺼짐 — 실시간 지도는 고르는 그림이지 기본 그림이 아니다. */
let agentGraphLive = false;

/* 상한과 양. 셋 다 **데이터의 양**이라 토큰이 아니라 상수다(맥박이 사는
 * 길이와 한 판의 맥박 수는 시각 수치라 토큰이 든다 — `agentGraphTuning`). */
const AGENT_GRAPH_LIVE = Object.freeze({
  /* 기억하는 관계의 수. 세션 내내 무한히 쌓이면 이것이 두 번째 원장이 된다.
   * 넘치면 오래 손대지 않은 관계부터 잊고, 잊은 것이 다시 나타나면 **조용히
   * 다시 맞춘다**(아래 `liveForgetFloor`) — 맥박 하나를 놓치는 쪽을 고른다. */
  laneKeep: 256,
  /* 한 관계에서 **같은 밀리초**에 일어난 서로 다른 사건을 몇 개까지 기억하는가.
   * 완전한 중복 제거가 성립하는 것은 이 수 안에서다. 넘으면 그 관계는
   * `truncated`가 되고, 그 시각이 지나갈 때까지 같은 시각의 모르는 사건은
   * 새 것도 옛것도 아닌 채로 조용히 맞춘다 — 화면은 그 사실을 숨기지 않는다. */
  sameStampKeep: 16,
  /* 인스펙터가 그리는 최근 사건 줄 수. 근거 보존은 원장의 일이고 이 목록은
   * 그 원장으로 가는 손잡이일 뿐이다. */
  eventKeep: 24,
});

/* ---- 장부 ------------------------------------------------------------------ */

/* 관계 하나가 지금까지 말한 것: `{ stamp, keys, count, ends, truncated }`.
 *
 * 중복 제거의 **경계를 여기 한 곳에 적는다**. 사건 키의 전역 목록 하나로는
 * 이 일을 할 수 없다: A@T와 B@T가 지나간 뒤 상한이 A를 밀어내면, 마지막 키는
 * B이고 stamp는 같으므로 다시 온 A@T가 새 사건으로 보인다. 그래서 기억하는
 * 것은 「마지막 키」가 아니라 **watermark 밀리초에 본 키들의 집합**이다.
 *
 *   · `evStamp < stamp` — 옛 사건. 버린다. 집합을 보지 않아도 된다.
 *   · `evStamp === stamp` — `keys`에 있으면 같은 사건, 없으면 같은 시각의
 *     **다른 실제 사건**이다. 합치지 않는다. 다만 그 시각의 집합이 온전하지
 *     않으면(`truncated`) 그 판정을 내릴 수 없으므로 조용히 맞춘다.
 *   · `evStamp > stamp` — 새 사건. watermark가 올라가고 집합은 그 하나로
 *     다시 시작한다.
 *
 * 성립하는 범위는 둘이다 — 한 밀리초에 같은 관계에서 `sameStampKeep`개까지,
 * 그리고 `laneKeep`개까지의 관계. 그 밖에서는 완전한 중복 제거를 주장하지
 * 않고 **조용한 재동기화**로 물러난다(`adopt`): 지금 상태를 본 것으로 적고
 * 맥박은 내지 않는다. 놓친 맥박은 아무것도 거짓말하지 않지만, 지어낸 맥박은
 * 없던 사건을 만든다.
 *
 * `count`는 같은 두 주체(`ends`) 사이의 **실제 증가**만 세기 위한 기준선이고,
 * 새 사건과 함께만 움직인다(`agentGraphLiveTally`). */
const liveLanes = new Map();

/* 잊은 것의 바닥. 상한에 밀려난 관계는 이름표를 남기지 않는다 — 밀려난 이름을
 * 모아 두는 표는 그 자체로 두 번째 장부가 되어, 되돌아오지 않는 관계의 수만큼
 * 끝없이 자란다. 대신 **밀려난 관계들이 마지막으로 본 시각의 최댓값** 하나만
 * 든다.
 *
 * 기록이 없는 관계의 사건이 이 바닥 이하라면, 그것이 잊은 관계의 옛 사건인지
 * 처음 보는 관계의 사건인지 가릴 수 없으므로 조용히 맞춘다. 바닥보다 늦은
 * 사건은 어느 쪽이든 이 창이 본 적 없는 사건이다 — 잊은 관계였다면 그 관계의
 * 마지막 기억보다 뒤이고, 처음 보는 관계라면 말 그대로 처음이다. 아무것도
 * 잊지 않은 판에는 바닥이 없다. */
let liveForgetFloor = -Infinity;
/* 몇 번 잊었는가. 수 하나라 상한이 필요 없고, 화면이 「잊은 관계가 있다」를
 * 말하는 근거다. */
let liveForgotten = 0;
/* 지금 뛰는 맥박: 키(노드 또는 간선) → { beat, eventKey, untilMs, seats }. */
const livePulses = new Map();
/* 인스펙터가 읽는 최근 사건, 새 것이 앞. */
let liveEvents = [];
/* 사람이 목록에서 누른 사건의 키. 그 줄이 제 근거를 편다. */
let liveSelectedEventKey = null;
/* 맥박이 꺼질 때를 기다리는 시계 하나. */
let livePulseTimer = null;
/* 다음 판을 **조용히 삼킨다**: 첫 snapshot·손잡이를 켠 순간·범위 이동·부서진
 * 판의 복구, 그리고 마지막으로 보이던 지도를 떠났다(숨은 문서·숨은 자리·작업
 * 목록·닫힌 판) 돌아온 첫 판. 놓친 backlog가 방금 생긴 활동처럼 터지지 않는다.
 * 이 빚은 **보이는** 판만 치른다(`agentGraphLiveShown`). */
let liveBaselineDue = true;
/* 범위가 움직일 때 올라가는 빗장. 이보다 낮은 세대에서 떠난 비동기 갱신은
 * 지금의 지도·선택·근거를 덮지 못한다(`boardCommit`). revision 주장이 아니다. */
let liveGeneration = 0;
/* 이 파일이 문서에 건 귀의 수. 거는 손이 하나라 세는 곳도 하나다. */
let liveListeners = 0;

/* ---- 주체 ------------------------------------------------------------------- */

/* 자리에 지금 앉은 것의 신원 — `run/worker/dispatch` 셋을 그대로 잇는다. 빈
 * 칸이 있어도 신원은 신원이다: 빈 칸은 같은 빈 칸끼리만 같다. */
function agentGraphLiveSeat(place) {
  return [place?.run ?? "", place?.workerId ?? "", place?.dispatchId ?? ""].join("\u001f");
}

/* 이 자리에 지금 앉아 있는 **주체**. 셋 다 있어야 주체이고, 하나라도 없으면
 * `null`이다 — 그런 카드는 확인되지 않은 것(`unverified`)이지 「배정 없음」이
 * 아니다. */
function agentGraphLiveSubject(place) {
  const run = place?.run ?? "";
  const worker = place?.workerId ?? "";
  const dispatch = place?.dispatchId ?? "";
  return run && worker && dispatch ? agentGraphLiveSeat(place) : null;
}

/* ---- 무엇이 실시간인가 ------------------------------------------------------ */

function agentGraphLiveOn() {
  return agentGraphLive;
}

/* 맥박이 **도는가**. 숨긴 판에서는 돌지 않는다 — 보이지 않는 그림의 애니메이션은
 * 아무에게도 말하지 않으면서 프레임을 먹는다. 움직임을 줄이라는 판에서는 맥박이
 * 서지만 돌지는 않는다: 그 판정은 CSS의 같은 미디어 질의가 하고(shell.css의
 * `prefers-reduced-motion` 절), 여기서는 사건의 자취를 지우지 않는다 — 무엇이
 * 방금 일어났는지는 이 표면이 있는 이유이고, 애니메이션은 그것을 말하는 한
 * 가지 방법일 뿐이다. */
function agentGraphLiveAnimating() {
  return agentGraphLive && !document.hidden;
}

/* 지도가 지금 **보이는가** — 손잡이가 켜졌고, 문서가 앞에 있고, 보드가 작업 목록이
 * 아니며, 관계 그림으로 서서 숨지 않은 판이 하나라도 있다. 장부가 사건을 적을 수
 * 있는 것은 이 동안뿐이다: 아무도 보지 않는 동안 읽은 것은 사건이 아니라 backlog
 * 이고, 다시 보인 첫 판이 그것을 조용히 기준선으로 삼는다.
 *
 * 보드의 모드를 판의 옷(`is-task-board`)과 **함께** 묻는 것은, 모드가 바뀐 뒤 그
 * 옷이 바뀌기 전에 장부가 먼저 읽는 판이 있기 때문이다 — 작업 공간에서 작업 목록을
 * 여는 손은 모드를 적고 나서 그린다. */
function agentGraphLiveShown() {
  return agentGraphLive && agentGraphPictureOn() && agentGraphLiveViews().length > 0;
}

/* 관계 그림이 설 수 있는 때인가 — 문서가 앞에 있고 보드가 작업 목록이 아니다. 판
 * 한 장이 그림으로 서 있는지는 아래 `agentGraphViewStands`가 답한다. 둘로 나눈
 * 것은 행성계(shell-board-orbit.js)가 프레임마다 **제가 그린 판만** 같은 규칙으로
 * 묻기 때문이다 — 문서 전체의 판을 매 프레임 훑지 않고, 「보이는가」의 판정을
 * 두 벌 두지도 않는다 (t-9444). */
function agentGraphPictureOn() {
  return !document.hidden && agentBoardMode !== "tasks";
}

/* ---- 사건의 신원 ------------------------------------------------------------ */

/* 한 사건의 키. 앞머리가 종류, 나머지가 **실제로 기록된 식별자**다. 같은
 * `created_ms`를 가진 서로 다른 두 사건은 id가 달라 절대 합쳐지지 않는다. */
function agentGraphLiveEventKey(kind, parts) {
  return [kind, ...parts].join("\u001f");
}

/* 관계 하나가 말한 것을 장부에 접는다. 돌려주는 답은 셋이다:
 *
 *   `"seen"`  — 이미 아는 사건이거나 watermark보다 옛것. 아무 일도 없다.
 *   `"adopt"` — 이 관계를 **조용히 다시 맞춘다**. 잊었을지 모르는 관계이거나,
 *               그 시각의 기억이 온전하지 않다. 지금 상태를 적고 맥박은 내지
 *               않는다.
 *   `"new"`   — 처음 보는 실제 사건. 맥박의 후보다.
 *
 * `laneKeep`은 다시 넣는 것으로 최근 순서를 지킨다 — Map은 넣은 차례를
 * 기억하므로, 손댄 관계를 지웠다 다시 넣으면 첫 키가 늘 가장 오래 손대지 않은
 * 관계다. */
function agentGraphLiveLane(lane, stamp, key, ends = "") {
  const held = liveLanes.get(lane);
  if (held === undefined) {
    if (stamp > liveForgetFloor) {
      agentGraphLiveKeepLane(lane,
        { stamp, keys: new Set([key]), count: null, ends, truncated: false });
      return "new";
    }
    /* 잊었을지 모르는 관계. watermark는 바닥까지 올린다 — 이 관계의 옛 사건이
     * 바닥 아래 어디에 있었는지 모르므로, 바닥 이하의 무엇도 뒤늦게 새 사건이
     * 되지 못하게. 그 시각에 본 키의 집합도 모르므로 온전하지 않다. */
    const watermark = Math.max(stamp, liveForgetFloor);
    agentGraphLiveKeepLane(lane, { stamp: watermark,
      keys: new Set(watermark === stamp ? [key] : []), count: null, ends, truncated: true });
    return "adopt";
  }
  liveLanes.delete(lane);
  liveLanes.set(lane, held);
  if (stamp < held.stamp) return "seen";
  if (stamp > held.stamp) {
    held.stamp = stamp;
    held.keys = new Set([key]);
    held.truncated = false;
    return "new";
  }
  if (held.keys.has(key)) return "seen";
  /* 같은 밀리초의 기억이 온전하지 않다. 모르는 키가 그 시각의 새 사건인지,
   * 기억에서 밀려난 옛 사건인지 이제는 가릴 수 없다 — 새 것으로 세면 옛 사건이
   * 방금 일어난 일처럼 다시 뛴다. 시각이 지나갈 때까지(위의 `stamp >`) 조용히
   * 맞추고, 키도 더 쌓지 않는다. */
  if (held.truncated) return "adopt";
  if (held.keys.size >= AGENT_GRAPH_LIVE.sameStampKeep) {
    held.truncated = true;
    return "adopt";
  }
  held.keys.add(key);
  return "new";
}

/* 수의 기준선을 움직이는 한 손 — 장부의 판정(`agentGraphLiveLane`)과 함께 정한다.
 * 기준선은 **새 사건과 함께만** 움직인다:
 *
 *   `new`   — 이 판의 수가 새 기준선이다. 증가는 기준선이 같은 두 주체 사이의 것일
 *             때만 센다. 수가 **줄었으면**(보존 기간이 옛 통을 거두었거나 주소가 다른
 *             카드로 옮겨 갔다) 증가를 추정하지 않고 모른다고 한다.
 *   `seen`  — 이미 본 사건이거나 옛것이다. 다시 온 판의 수는 그 사건 때의 수이지
 *             지금의 수가 아니므로 기준선에 손대지 않는다. 같은 시각의 옛 ID도 여기로
 *             온다 — 옛 stamp만 막으면 그 길이 열리고, 낮아진 수가 다음 새 사건의
 *             증가를 부풀린다.
 *   `adopt` — 새 것인지 옛것인지 가릴 수 없는 판이다. 그 수를 기준선으로 삼으면
 *             다음 증가가 추정이 되므로, 기준선을 모름으로 둔다.
 *
 * 돌려주는 것은 이 판의 증가(모르면 `null`)다. */
function agentGraphLiveTally(held, verdict, baseline, count, ends) {
  if (verdict === "seen") return null;
  held.ends = ends;
  if (verdict === "adopt") {
    held.count = null;
    return null;
  }
  held.count = count;
  return baseline !== null && count >= baseline ? count - baseline : null;
}

function agentGraphLiveKeepLane(lane, value) {
  liveLanes.set(lane, value);
  while (liveLanes.size > AGENT_GRAPH_LIVE.laneKeep) {
    const [oldest, gone] = liveLanes.entries().next().value;
    if (oldest === lane) break;
    liveLanes.delete(oldest);
    liveForgetFloor = Math.max(liveForgetFloor, gone.stamp);
    liveForgotten += 1;
  }
}

/* 맥박의 후보 하나를 인스펙터의 목록에 적는다. baseline 중이면 목록에도
 * 적지 않는다 — 처음 서는 판은 사건이 일어난 판이 아니다. */
function agentGraphLiveNote(event) {
  if (liveBaselineDue) return false;
  liveEvents = [event, ...liveEvents].slice(0, AGENT_GRAPH_LIVE.eventKeep);
  return true;
}

/* 중복 제거가 **어디까지** 성립하는가. 화면과 보고가 같은 낱말로 말하도록
 * 장부가 스스로 답한다 — 그 시각의 기억이 온전하지 않은 관계의 수와, 상한에
 * 밀려 잊은 적이 있는지. */
function agentGraphLiveCoverage() {
  let truncated = 0;
  for (const lane of liveLanes.values()) if (lane.truncated) truncated += 1;
  return {
    lanes: liveLanes.size,
    laneKeep: AGENT_GRAPH_LIVE.laneKeep,
    sameStampKeep: AGENT_GRAPH_LIVE.sameStampKeep,
    truncated,
    forgotten: liveForgotten,
    complete: truncated === 0 && liveForgotten === 0,
  };
}

/* ---- 한 판의 사실을 장부에 접는다 -------------------------------------------- */

/* 이 창이 방금 받은 snapshot 하나. 돌려주는 것은 없다 — 장부가 움직이고,
 * 그리는 손이 그 장부를 읽는다.
 *
 * `places`는 카드마다의 run/task/dispatch/worker 신원이고, `overlays`는
 * 백엔드가 이미 투영한 관계이며, `ledger`는 **이 판이 카드와 함께 물은** 원장
 * 행이다(자리 → 행). 이 함수는 그 셋만 읽는다.
 *
 * 늦게 도착한 답을 거르는 것은 이 함수의 일이 아니다: 부르는 쪽이 답을 쓰기
 * 전에 차례와 세대를 묻고(`boardCommit`), 거절된 답은 여기까지 오지 않는다. */
function agentGraphLiveObserve(answer, places, now = Date.now(), { ledger = null } = {}) {
  if (!agentGraphLive) {
    /* 꺼져 있는 동안에도 baseline은 빚으로 남는다: 다시 켜는 판이 그동안의
     * backlog를 한꺼번에 터뜨리지 않도록. 지도가 서 있지 않은 판에서 이
     * 함수가 하는 일은 이 한 줄뿐이다. */
    liveBaselineDue = true;
    return;
  }
  /* 지도가 보이지 않는 동안 읽은 판(숨은 자리·작업 목록·숨은 문서)은 backlog다.
   * 장부는 그 판도 지금 상태로 접어 두지만 사건으로 적지 않고, 빚도 **치르지
   * 않는다** — 다시 보인 첫 판이 치른다. 숨은 동안의 판이 빚을 먼저 치르면, 그 뒤
   * 숨은 사이에 쌓인 것이 돌아온 판에서 새 사건이 된다. */
  const shown = agentGraphLiveShown();
  if (!shown) liveBaselineDue = true;
  const overlays = answer?.overlays ?? {};
  const rows = ledger ?? new Map();
  /* 이 판의 카드, 자리마다. 대기 판정은 원장의 행과 **이 판의** 카드를 함께
   * 묻는다(`deskWorkerHealth`) — 지난 판의 모델에서 카드를 꺼내 오면 한 박자
   * 뒤진 상태로 사유를 고르게 된다. */
  const cards = new Map((answer?.columns ?? [])
    .flatMap((column) => (column.cards ?? []).map((card) => [card.pane, card])));
  const seatOf = (pane) => agentGraphLiveSeat(places?.get(pane));
  const fresh = [];

  /* ⓪ 주체가 바뀐 자리의 맥박은 내린다. 판이 다시 앉았든, 새 시도가 같은 판을
   *    물려받았든, 화면의 키는 같아도 **사건의 주인이 다른 사람**이다 — 앞
   *    주체의 사건이 새 주체의 자리에서 빛나지 않게. 이 판에 없는 자리는 주체에
   *    대해 아무것도 말하지 않으므로 그 맥박은 제 수명대로 둔다. */
  for (const [key, pulse] of [...livePulses]) {
    if (pulse.seats.some(([pane, seat]) => places?.has(pane) && seatOf(pane) !== seat)) {
      livePulses.delete(key);
    }
  }

  /* ① 배정과 교신 — 실제 메일 행. `MessageKind::Dispatch`가 배정이고, 두
   *    코디네이터 좌석 사이의 행이 교신이다. 이름이나 cwd가 같다는 이유로
   *    이어 붙인 선은 하나도 없다: 끝점은 백엔드가 주소 → 카드로 매핑한 것
   *    그대로다(`graph_overlay_snapshot_for_seats`). */
  for (const edge of Array.isArray(overlays?.mail) ? overlays.mail : []) {
    const last = edge.last_message;
    if (!last?.id) continue;
    const stamp = Number(last.created_ms) || 0;
    const lane = `mail:${edge.from}>${edge.to}`;
    const key = agentGraphLiveEventKey("msg", [last.run ?? "", last.id]);
    /* 맥박이 앉을 자리는 **그림이 쓰는 키**다 — 장부의 lane은 카드의 날 id로
     * 세고(그림이 어떻게 그리든 관계는 같은 관계다), 맥박은 모델이 간선에
     * 적어 둔 그 키로 앉는다. 둘을 하나로 쓰면 그림이 키를 바꾸는 날 장부가
     * 모든 관계를 처음 보는 것으로 읽는다. */
    const edgeKey = `overlay:mail:${agentGraphAgentKey(edge.from)}>${agentGraphAgentKey(edge.to)}`;
    /* 두 끝의 주체. 같은 두 자리라도 앉은 사람이 바뀌면 그 사이의 통 수는 다른
     * 두 사람의 수다 — 기준선은 같은 두 주체 사이에서만 잇는다. */
    const ends = `${seatOf(edge.from)}\u001e${seatOf(edge.to)}`;
    const prior = liveLanes.get(lane);
    const baseline = prior !== undefined && prior.ends === ends ? prior.count : null;
    const verdict = agentGraphLiveLane(lane, stamp, key, ends);
    /* 이 관계에서 **몇 통이 늘었는가**. 기준선이 없으면(처음 보는 관계, 끝의
     * 주체가 바뀐 판, 다시 맞춘 관계, 다시 켠 판) 증가가 아니라 baseline이다 —
     * 재연결을 활동으로 읽지 않는다. 기준선이 언제 움직이는가는 장부의 판정과
     * 한 손에서 정한다(`agentGraphLiveTally`). */
    const added = agentGraphLiveTally(liveLanes.get(lane), verdict, baseline,
      Number(edge.count) || 0, ends);
    if (verdict !== "new") continue;
    const event = {
      key,
      kind: "message",
      at: stamp,
      observedAt: now,
      from: agentGraphAgentKey(edge.from),
      to: agentGraphAgentKey(edge.to),
      edgeKey,
      added,
      seats: [[edge.from, seatOf(edge.from)], [edge.to, seatOf(edge.to)]],
      /* 근거는 원장의 것이다. 여기 드는 것은 그 원장으로 가는 식별자뿐 —
       * 본문도 과업 산문도 담지 않는다. */
      evidence: {
        messageId: last.id,
        run: last.run ?? "",
        address: { from: last.from ?? "", to: last.to ?? "" },
        messageKind: last.kind ?? "",
      },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  /* ② 선행 관계의 **전이**. 선언 자체는 사건이 아니다 — 같은 선언을 매 판
   *    다시 읽는다고 무언가 일어난 것이 아니므로, 키에 두 상태를 싣는다. */
  for (const fact of Array.isArray(overlays?.task_dependencies) ? overlays.task_dependencies : []) {
    const lane = `dep:${fact.run}:${fact.task}:${fact.dependency}`;
    const stamp = Number(fact.task_created_ms) || 0;
    const state = `${fact.task_state ?? ""}>${fact.dependency_state ?? ""}`;
    const key = agentGraphLiveEventKey("dep",
      [fact.run ?? "", fact.task ?? "", fact.dependency ?? "", state]);
    if (agentGraphLiveLane(lane, stamp, key) !== "new") continue;
    const ends = [fact.from, fact.to].filter(Boolean);
    const event = {
      key,
      kind: "dependency",
      /* 전이가 **언제** 일어났는지는 이 판에 없다. `task_created_ms`는 후속
       * 과업이 생긴 때이지 선행 조건의 상태가 바뀐 때가 아니므로, 그 시각을
       * 이 사건의 나이로 빌려 쓰지 않는다 — 발생 시각은 모름이고, 이 창이 본
       * 때를 따로 든다. 과업이 생긴 때는 근거의 한 칸으로 남는다. */
      at: 0,
      observedAt: now,
      from: fact.from ? agentGraphAgentKey(fact.from) : null,
      to: agentGraphAgentKey(fact.to),
      edgeKey: fact.from
        ? `overlay:dependency:${agentGraphAgentKey(fact.from)}>${agentGraphAgentKey(fact.to)}`
        : null,
      added: null,
      seats: ends.map((pane) => [pane, seatOf(pane)]),
      evidence: {
        run: fact.run ?? "",
        taskId: fact.task ?? "",
        dependencyId: fact.dependency ?? "",
        taskState: fact.task_state ?? "",
        dependencyState: fact.dependency_state ?? null,
        taskCreated: stamp,
      },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  /* ③ 시도와 결과 — 카드의 dispatch 신원과 t-6815의 권위 seam. 워커의 주장
   *    (`reported`)과 코디네이터의 사실(검증/병합/배포)은 **다른 사건**이고,
   *    키가 달라 서로를 덮지 않는다. 재시도는 `dispatch_id`가 다르므로 그
   *    자체로 다른 시도이고, 관계의 이름부터 다르다.
   *
   *    셋 중 하나라도 없는 카드는 주체가 없다 — 사건을 만들지 않는다. */
  for (const [pane, place] of places ?? []) {
    const subject = agentGraphLiveSubject(place);
    if (subject === null) continue;
    const lane = `work:${subject}`;
    const started = Number(place.dispatchStarted) || 0;
    const stage = ledgerReviewStage(place, rows.get(pane) ?? null);
    const key = agentGraphLiveEventKey("work", [subject, stage]);
    if (agentGraphLiveLane(lane, started, key) !== "new") continue;
    const assignment = stage === "dispatched";
    const event = {
      key,
      kind: assignment ? "assignment" : "result",
      /* 배정의 시각은 배차가 시작된 때 그 자체다. 결과(보고·검증·병합·배포)가
       * **언제** 적혔는지는 이 판에 없다 — 원장 행은 그 단계에 이르렀다는
       * 사실만 싣는다. 그러니 결과의 나이를 배차의 시작으로 빌리지 않는다. */
      at: assignment ? started : 0,
      observedAt: now,
      from: null,
      to: `agent:${pane}`,
      edgeKey: null,
      added: null,
      seats: [[pane, agentGraphLiveSeat(place)]],
      evidence: {
        run: place.run ?? "",
        taskId: place.taskId ?? "",
        workerId: place.workerId ?? "",
        dispatchId: place.dispatchId,
        retryOf: place.retryOf ?? null,
        stage,
        dispatchStarted: started,
      },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  /* ④ 기다림의 **사유가 바뀐** 판. 나이가 흐르는 것은 사건이 아니다 —
   *    사유와 그 사유가 기록된 stamp가 키를 이루므로, 30초 박자가 나이 낱말을
   *    바꾸어도 맥박은 없다. */
  for (const [pane, row] of rows) {
    const wait = agentGraphLiveWaitOf(row, cards.get(pane) ?? null, now);
    if (!wait) continue;
    const lane = `wait:${row.run ?? ""}\u001f${row.worker ?? ""}`;
    const key = agentGraphLiveEventKey("wait",
      [row.run ?? "", row.worker ?? "", wait.cause, String(wait.since)]);
    if (agentGraphLiveLane(lane, wait.since, key) !== "new") continue;
    const place = places?.get(pane);
    const event = {
      key,
      kind: "wait",
      at: wait.since,
      observedAt: now,
      from: null,
      to: `agent:${pane}`,
      edgeKey: null,
      added: null,
      seats: [[pane, agentGraphLiveSeat(place)]],
      evidence: { run: row.run ?? "", workerId: row.worker ?? "",
        dispatchId: place?.dispatchId ?? "", cause: wait.cause, since: wait.since },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  if (liveBaselineDue) {
    /* 처음 한 판은 통째로 삼킨다. 위에서 관계마다의 watermark는 이미 지금
     * 상태로 서 있고 `fresh`는 비어 있다 — 그림은 조용히 지금의 모습으로 선다.
     * 빚을 치르는 것은 **보이는** 판뿐이다. */
    if (shown) liveBaselineDue = false;
    return;
  }
  agentGraphLiveStartPulses(fresh, now);
}

/* 돌아온 지도가 빚을 지고 있으면 이 판을 기준선으로 접는다 — 그림이 그대로라
 * 그리기를 건너뛰는 판에서도. 숨었다 돌아온 첫 판이 떠나기 전과 같은 판이면 그리는
 * 손은 서명을 보고 건너뛰고, 그러면 빚이 다음에 **달라진** 판으로 넘어가 돌아온
 * 뒤에 실제로 일어난 첫 사건을 삼킨다. 빚이 없거나 지도가 보이지 않으면 아무것도
 * 하지 않는다 — 조용한 판에서 이 손의 값은 0이다. 같은 판을 한 번 더 접는 것은
 * 아무것도 새로 세지 않는다(장부의 신원 중복 제거). */
function agentGraphLiveRepay(answer, places, now, bundle) {
  if (liveBaselineDue && agentGraphLiveShown()) agentGraphLiveObserve(answer, places, now, bundle);
}

/* ---- 맥박 -------------------------------------------------------------------- */

/* 맥박을 접을 때 쓰는 최근의 차례. 발생 시각을 아는 사건은 그 시각으로, 모르는
 * 사건은 이 창이 본 때로 — 차례를 매길 뿐, 어느 쪽도 화면에 시각으로 서지
 * 않는다. */
function agentGraphLiveRecency(event) {
  return event.at > 0 ? event.at : event.observedAt;
}

/* 방금 본 사건들을 짧은 한 번의 맥박으로. 한 판에 도는 맥박의 수에는 상한이
 * 있고(토큰), 넘친 것은 **애니메이션만** 접는다 — 사건 목록과 간선의 실제
 * 수는 그대로 다 보인다. 근거를 숨기지도, 사건 수를 새로 만들지도 않는다. */
function agentGraphLiveStartPulses(fresh, now) {
  if (fresh.length === 0 || !agentGraphLiveAnimating()) return;
  const tuning = agentGraphLiveTuning();
  if (!tuning) return;
  const until = now + tuning.pulseMs;
  /* 상한을 넘길 때 **무엇을 접는가**. 장부가 쌓는 차례(메일 → 의존 → 시도 →
   * 기다림)로 자르면 메일이 늘 이기고 기다림은 한 번도 뛰지 못한다 — 종류로
   * 우열을 매긴 적이 없는데 그리는 차례가 우열을 만든 셈이다. 늦은 것부터
   * 든다: 접히는 것은 언제나 **더 오래된 사건**이다. */
  const ordered = [...fresh]
    .sort((left, right) => agentGraphLiveRecency(right) - agentGraphLiveRecency(left))
    .slice(0, tuning.burst)
    /* 넣는 차례는 **오래된 것부터**다. `Map`은 넣은 차례를 기억하므로, 그렇게
     * 넣어야 아래의 잘라내기가 앞에서부터 「가장 오래 전에 뛴 것」을 집는다. */
    .reverse();
  for (const event of ordered) {
    const key = event.edgeKey ?? event.to;
    if (!key) continue;
    /* 박자는 **그 자리의 지난 박자**를 뒤집는다. 판마다 하나로 번갈아 적으면
     * 한 판을 건너뛴 자리가 두 판 만에 같은 글자를 다시 받고, 같은 글자를 다시
     * 쓰는 것은 쓰기가 아니므로(값이 같으면 안 쓴다) 그 맥박은 뛰지 않는다. */
    const beat = livePulses.get(key)?.beat === "a" ? "b" : "a";
    /* 지웠다 다시 넣어 차례를 맨 뒤로 옮긴다 — 다시 뛴 자리는 가장 최근이다.
     * 맥박도 제가 누구의 사건인지 적어 둔다(`seats`): 주체가 바뀐 자리에서
     * 앞사람의 맥박이 계속 빛나지 않게. */
    livePulses.delete(key);
    livePulses.set(key, { beat, eventKey: event.key, untilMs: until, seats: event.seats });
  }
  /* 상한은 **한 번에 화면에서 뛰는 수**다. 판 하나에 들어오는 수만 자르면
   * 맥박이 900 ms를 사는 동안 판이 여러 번 오고, 판마다 여섯씩 쌓여 눈앞에는
   * 스물이 뛴다 — 상한이 있다는 말이 무색해진다. 넘친 것은 가장 오래 전에 뛴
   * 것부터 접고, 접히는 것은 **애니메이션뿐**이다: 사건 목록도 간선의 수도
   * 그대로 남는다. */
  while (livePulses.size > tuning.burst) {
    const [oldest] = livePulses.keys();
    livePulses.delete(oldest);
  }
  agentGraphLiveArmExpiry(now);
  for (const view of agentGraphLiveViews()) dressAgentGraphLive(view);
}

/* 맥박이 꺼지는 때를 기다리는 시계 하나. 시계가 **하나**인 것은 맥박마다
 * 시계를 두면 판 하나에 수십 개의 타이머가 서기 때문이다. */
function agentGraphLiveArmExpiry(now) {
  /* **가장 먼저** 꺼질 맥박에 맞춘다. 마지막으로 켜진 것에 맞추면, 앞서 켜진
   * 맥박이 제 창을 넘겨 계속 빛난다 — 「짧은 한 번」의 길이를 토큰이 정하는데
   * 화면은 그보다 오래 뛰는 셈이다. */
  let next = Infinity;
  for (const pulse of livePulses.values()) next = Math.min(next, pulse.untilMs);
  if (livePulseTimer !== null) clearTimeout(livePulseTimer);
  livePulseTimer = null;
  if (!Number.isFinite(next)) return;
  livePulseTimer = window.setTimeout(() => {
    livePulseTimer = null;
    /* 그 사이 지도를 보일 판이 모두 사라졌으면(닫힘·숨김·작업 목록) 남은 맥박도
     * 함께 거두고 돌아온 첫 판의 빚을 남긴다 — 떠나는 문과 같은 한 손이다.
     * 아무도 보지 못할 맥박을 위해 시계를 다시 걸지 않는다. */
    if (!agentGraphLiveShown()) {
      agentGraphLiveSettle();
      return;
    }
    const views = agentGraphLiveViews();
    const at = Date.now();
    for (const [key, pulse] of [...livePulses]) {
      if (pulse.untilMs <= at) livePulses.delete(key);
    }
    for (const view of views) dressAgentGraphLive(view);
    agentGraphLiveArmExpiry(at);
  }, Math.max(1, next - now));
}

/* 뷰를 떠나거나 손잡이를 끄거나 판이 숨으면 — 시계도 맥박도 남기지 않는다.
 * 들어갔다 나오기를 되풀이해도 활성 핸들이 늘지 않는 것은 이 한 손 때문이다.
 *
 * 지우는 판은 **보이는 판만이 아니다**. 숨은 판의 노드에 남은 박자는 그 판이
 * 다시 보이는 순간 CSS 애니메이션을 처음부터 다시 틀고, 그것은 숨김에서 돌아온
 * 판이 backlog를 방금 일처럼 뛰게 하는 길이다. 박자를 적은 적이 있는 판만
 * 실제로 훑는다(`liveDressedViews`). */
function agentGraphLiveStop() {
  if (livePulseTimer !== null) clearTimeout(livePulseTimer);
  livePulseTimer = null;
  livePulses.clear();
  for (const view of document.querySelectorAll(".agent-board")) dressAgentGraphLive(view);
}

/* 지도를 보일 판이 하나도 남지 않았으면 — 판을 닫든, 작업 목록으로 돌리든, 판을
 * 품은 자리가 숨든 — 두 가지를 **지금** 한다. 시계와 맥박을 거두고, 돌아온 첫 판이
 * 조용한 기준선이 되도록 빚을 남긴다. 떠난 사이 원장에 적힌 것은 이 지도가 본 적
 * 없는 backlog이고, 그것이 돌아온 판에서 방금 일어난 일처럼 뛰면 오래전 일이 지금
 * 막 일어난 것이 된다. 빚을 남기는 것은 맥박이 있든 없든이다 — 조용한 판을 떠나도
 * 돌아오는 판은 같다. 그 빚은 다시 **보이는** 판이 치른다(`agentGraphLiveObserve`).
 *
 * 보드가 이미 판의 크기를 재는 관찰자(`watchAgentGraphSize`)와 그리기
 * (`paintAgentGraph`)가 그 문을 지나며 이 손을 부른다. 묻는 것은 문서의 판 몇 장과
 * 그 조상의 `hidden`뿐이라 배치를 읽지 않고, 지도가 꺼진 판에서는 아무것도 묻지
 * 않는다 — 그 판의 빚은 이미 남아 있다. */
function agentGraphLiveSettle() {
  if (!agentGraphLive || agentGraphLiveShown()) return;
  liveBaselineDue = true;
  if (livePulseTimer !== null || livePulses.size > 0) agentGraphLiveStop();
}

/* 지금 이 장부가 들고 있는 모든 것의 수 — 들어갔다 나오기를 되풀이해도, 관계와
 * 판이 끝없이 바뀌어도 늘지 않는다는 것을 시험이 실제 scheduler 경계에서 세기
 * 위한 한 줄. 잊은 관계의 바닥과 횟수는 수 둘이라 여기 크기가 없다. */
function agentGraphLiveHandles() {
  let keys = 0;
  for (const lane of liveLanes.values()) keys += lane.keys.size;
  return {
    timers: livePulseTimer === null ? 0 : 1,
    pulses: livePulses.size,
    lanes: liveLanes.size,
    keys,
    events: liveEvents.length,
    listeners: liveListeners,
  };
}

/* 맥박의 길이와 한 판의 상한은 시각 수치라 토큰이 든다. 뷰마다 한 번 읽는
 * `agentGraphTuning`에 실려 있으므로 여기서 두 번째 사본을 두지 않는다. */
function agentGraphLiveTuning() {
  const [view] = agentGraphLiveViews();
  const tuning = view ? agentGraphTuning(view) : null;
  return tuning && Number.isFinite(tuning.livePulseMs) && Number.isFinite(tuning.liveBurst)
    ? { pulseMs: tuning.livePulseMs, burst: tuning.liveBurst }
    : null;
}

/* 지금 그림이 서 있는 보드 판. 작업 목록으로 서 있는 판은 관계 그림이 아니고,
 * 제 자신이나 품은 자리가 숨은 판은 아무에게도 보이지 않는다(탭을 옮기면 판이,
 * 무대가 다른 쪽으로 가면 그 위의 자리가 `hidden`을 받는다).
 *
 * 탭 장부가 아니라 **문서**에게 묻는다. 팝아웃으로 보드를 빼면 본창의
 * `boardTab()`은 빈손이 되고(그 탭이 저쪽으로 갔다), 그러면 이 손이 판을 찾지
 * 못해 맥박이 아예 서지 않는다 — 판은 저쪽 문서에 멀쩡히 서 있는데. 문서에
 * 묻는 쪽은 본창·팝아웃·복제된 판을 모두 같은 규칙으로 답한다. */
function agentGraphLiveViews() {
  return [...document.querySelectorAll(".agent-board")].filter(agentGraphViewStands);
}

/* 판 한 장이 관계 그림으로 서서 숨지 않았는가 — 제 자신이나 품은 자리가 `hidden`이면
 * 아무에게도 보이지 않는다. */
function agentGraphViewStands(view) {
  return !view.classList.contains("is-task-board") && view.closest("[hidden]") === null;
}

/* 맥박 하나를 노드·간선에 적는 유일한 손. 쓰는 것은 `data-live-beat` 하나뿐이라
 * 간선의 `class`를 쓰는 손(`paintAgentGraphEdges`의 옷)과 부딪히지 않는다.
 * 배치를 읽지 않고, 값이 같으면 쓰지 않는다 — 조용한 판에서 이 손은 DOM을
 * 한 번도 건드리지 않는다. */
/* 지금 화면에 맥박 표시가 적혀 있는 판들. 판마다 기억하는 것은 손잡이를 끈
 * 판에서 이 손이 **아예 아무것도 하지 않게** 하기 위해서다 — 지도가 없는 판은
 * 보드의 기본값이고, 거기서 매 그리기마다 노드와 간선을 훑는 것은 아무도 보지
 * 못할 것을 찾는 값이다. 지울 것이 남은 판만 예외로 한 번 더 지난다. */
const liveDressedViews = new WeakSet();

function dressAgentGraphLive(view) {
  const running = agentGraphLiveAnimating();
  /* 적을 것도 지울 것도 없으면 훑지 않는다. 손잡이가 켜져 있어도 마찬가지다 —
   * 아무 사건도 없는 판에서 노드와 간선을 지나는 것은 언제나 빈손이다. */
  if (livePulses.size === 0 && !liveDressedViews.has(view)) return;
  let wrote = false;
  for (const node of view.querySelectorAll(".agent-graph-node[data-graph-key]")) {
    const beat = running ? livePulses.get(node.dataset.graphKey)?.beat ?? "" : "";
    if (beat !== "") wrote = true;
    writeLiveBeat(node, beat);
  }
  for (const group of view.querySelectorAll(".agent-graph-edges [data-graph-edge]")) {
    const line = group.querySelector("path");
    if (!line) continue;
    const beat = running ? livePulses.get(group.getAttribute("data-graph-edge"))?.beat ?? "" : "";
    if (beat !== "") wrote = true;
    writeLiveBeat(line, beat);
  }
  if (wrote) liveDressedViews.add(view);
  else liveDressedViews.delete(view);
}

function writeLiveBeat(node, beat) {
  if (beat === "") {
    if (node.hasAttribute("data-live-beat")) node.removeAttribute("data-live-beat");
    return;
  }
  writeAttribute(node, "data-live-beat", beat);
}

/* ---- 기다림 ------------------------------------------------------------------ */

/* 「무엇을 기다리는가」의 사유는 **원장의 낱말**이고, 그 표는 이미 하나 있다
 * (`DESK_HEALTH`, 코디네이터 데스크). 여기서 더하는 것은 그 여섯 중 어느 것이
 * 기다림이며 그 기다림이 **언제 기록되었는가**뿐이다 — 표를 두 번 쓰지 않는다.
 *
 * stamp가 없는 사유는 나이를 말하지 않는다. 「답 기다림」에는 물은 때가 행에
 * 적혀 있지 않으므로 `0`이고, 화면은 사유만 말한다. 없는 시각을 곁의 다른
 * 시각으로 메우면 그것이 곧 추측이다. */
const AGENT_GRAPH_WAIT_SINCE = Object.freeze({
  gone: (row) => Number(row.pane_missing_since_ms) || 0,
  walled: (row) => Number(row.wall?.observed_at_ms) || 0,
  asking: () => 0,
  asleep: (row) => Number(row.quiet_at) || 0,
});

function agentGraphLiveWaitOf(row, card, now = Date.now()) {
  if (!row) return null;
  const health = deskWorkerHealth(row, card, now);
  const since = AGENT_GRAPH_WAIT_SINCE[health?.id];
  if (!since) return null;
  return {
    cause: health.id,
    key: health.key,
    word: health.word,
    state: health.state,
    since: since(row),
    /* 벽만은 **언제까지**를 원장이 적어 두었다. 나머지 셋은 끝을 모르므로
     * 끝을 말하지 않는다. */
    until: health.id === "walled" && Number.isFinite(row.wall?.resets_at_ms)
      ? row.wall.resets_at_ms
      : null,
  };
}

/* 카드 한 장이 그릴 기다림. 노드의 칸과 판의 서명이 **같은 이 한 손**으로
 * 묻는다 — 둘이 따로 지으면 칸이 바뀐 판을 서명이 모르고, 그 칸만 옛 낱말에
 * 굳는다. 원장의 행은 이 판이 카드와 함께 물은 한 벌(`ledger`)에서만 온다.
 *
 * 없으면 `null`이라 배지가 서지 않는다 — 「모름」을 「없음」으로 읽지 않도록,
 * 사유를 아는 카드만 배지를 든다. 주체가 없는 카드(`run/worker/dispatch` 중
 * 하나라도 없는)는 `unverified`다: 원장이 이 자리를 누구의 것이라고 말하지
 * 않았으므로, 사유도 말하지 않는다. */
function agentGraphLiveWaitFor(card, place, ledger, now = Date.now()) {
  if (!agentGraphLive) return null;
  if (agentGraphLiveSubject(place) === null) {
    return agentGraphLiveWaitWords({ cause: "unverified", state: "idle", since: 0, until: null,
      key: "board.live.unverified", word: "확인되지 않음" }, now);
  }
  const wait = agentGraphLiveWaitOf(ledger?.get(card.pane) ?? null, card, now);
  return wait ? agentGraphLiveWaitWords(wait, now) : null;
}

/* 모델이 노드마다 부르는 문. */
function agentGraphLiveWait(entry, now = Date.now(), ledger = null) {
  return agentGraphLiveWaitFor(entry.card, entry.place, ledger, now);
}

/* 기다림이 화면에 서는 낱말, 한 번만 짓는다. 낱말은 코디네이터 데스크가 이미
 * 쓰는 그 표의 것이고(`DESK_HEALTH`의 `key`/`word`), 나이는 **기록된 stamp**
 * 에서만 나온다. 나이를 모르는 기다림은 **모른다고 말한다** — 곁의 다른
 * 시각으로 메우면 그것이 곧 추측이다. */
function agentGraphLiveWaitWords(wait, now) {
  const text = t(wait.key, wait.word);
  const stands = wait.until !== null && Number.isFinite(wait.until)
    ? usageCountdown(wait.until - now)
    : "";
  const said = stands
    || (wait.since > 0 ? t("board.desk.mailAge", "{{time}} 전", { time: agoWord(wait.since, now) }) : "");
  const tip = said
    ? `${text} · ${said}`
    : `${text} · ${t("board.live.sinceUnknown", "기록된 시작 시각 없음")}`;
  return { ...wait, text, said, tip };
}

/* 판의 서명에 드는 기다림의 낱말들. 나이 낱말은 시계가 흐르면 바뀌는데 카드의
 * 어느 필드도 움직이지 않으므로, 서명이 이것을 세지 않으면 박자가 와도 판이
 * 그대로라 칸만 옛 나이에 굳는다. 지도가 꺼진 판(보드의 기본값)은 아무것도
 * 더하지 않는다. */
function agentGraphLiveWaitSaid(columns, places, ledger, now) {
  if (!agentGraphLive) return null;
  const said = [];
  for (const column of columns) {
    for (const card of column.cards) {
      const wait = agentGraphLiveWaitFor(card, places.get(card.pane), ledger, now);
      if (wait) said.push(`${card.pane}\u001f${wait.cause}\u001f${wait.said}`);
    }
  }
  return said.join("\u001e");
}

/* 노드가 늘 드는 한 칸. **조건부 자식으로 두지 않는다** — 조건이 바뀔 때마다
 * 카드 전체를 다시 짓게 되고, 이 표면은 훅 하나에 한 번씩 다시 그려진다
 * (`updateAgentGraphNode`의 다섯 칸과 같은 이유). 기다릴 것이 없으면 비어
 * 있고, 빈 칸은 CSS가 접는다(`:empty`). */
function agentGraphWaitNode() {
  const chip = document.createElement("span");
  chip.className = "agent-graph-wait";
  chip.append(
    Object.assign(document.createElement("span"), { className: "agent-graph-wait-cause" }),
    Object.assign(document.createElement("span"), { className: "agent-graph-wait-age" }),
  );
  return chip;
}

/* 그 칸을 채우는 유일한 손. 쓰는 낱말은 모델이 이미 지은 것이다
 * (`agentGraphLiveWaitWords`) — 칸과 서명이 같은 시각의 같은 낱말을 읽는다. */
function dressAgentGraphWait(chip, wait) {
  if (!chip) return;
  const cause = chip.querySelector(".agent-graph-wait-cause");
  const age = chip.querySelector(".agent-graph-wait-age");
  if (!wait) {
    writeClassName(chip, "agent-graph-wait");
    writeTextContent(cause, "");
    writeTextContent(age, "");
    if (chip.hasAttribute("data-tip")) chip.removeAttribute("data-tip");
    if (chip.hasAttribute("aria-label")) chip.removeAttribute("aria-label");
    return;
  }
  writeClassName(chip, `agent-graph-wait is-${wait.cause} is-${wait.state}`);
  writeTextContent(cause, wait.text);
  writeTextContent(age, wait.said);
  writeAttribute(chip, "data-tip", wait.tip);
  writeAttribute(chip, "aria-label", wait.tip);
}

/* ---- 권위 -------------------------------------------------------------------- */

/* 단계의 이름과 그 낱말, 한 표. 사건 줄이 단계를 말할 때와 원장의 판정을
 * 단계로 되읽을 때가 같은 표를 읽는다. 낱말은 t-6815의 seam
 * (`ledgerReviewWord`)이 쓰는 그 키들이다 — 코디네이터가 적은 사실 셋, 워커가
 * 적은 **주장** 셋(원장이 사실과 떼어 둔 것), 그리고 보고. 주장은 제 낱말로만
 * 서고 사실의 단계가 되지 않는다.
 *
 * `flag`는 그 seam이 그 낱말을 고를 때 읽는 원장 행의 칸이다(`reported`는 행의
 * 보고, 나머지는 `review`의 칸). 지난 결과 사건을 누를 때 그 사건이 적은 사실이
 * 지금도 **같은 사실로** 서 있는지 이 칸으로 묻는다(`agentGraphLiveStageHolds`). */
const AGENT_GRAPH_LIVE_STAGES = Object.freeze([
  { stage: "deployed", flag: "deployed", key: "board.deployed", word: "배포됨" },
  { stage: "merged", flag: "merged", key: "board.merged", word: "병합됨" },
  { stage: "verified", flag: "verified", key: "board.verified", word: "검증됨" },
  { stage: "claimed-deployed", flag: "claimed_deployed", key: "board.claimedDeployed", word: "배포됐다 함" },
  { stage: "claimed-merged", flag: "claimed_merged", key: "board.claimedMerged", word: "병합됐다 함" },
  { stage: "claimed-verified", flag: "claimed_verified", key: "board.claimedVerified", word: "검증됐다 함" },
  { stage: "reported", flag: "reported", key: "board.awaitingReview", word: "검증 대기" },
]);

/* 「어디까지 왔는가」를 **원장이 적은 만큼만**. 판정은 t-6815의 한 손
 * (`ledgerReviewWord`)이 하고, 여기서는 그 손이 답한 낱말을 위의 표로 단계
 * 이름으로 되읽을 뿐이다 — 워커의 주장(`reported`)이 검증·병합·배포 배지가
 * 되는 길은 이 파일에 없고, 그 seam이 권위를 바꾸면 이 지도도 따라 바뀐다. */
function ledgerReviewStage(place, row) {
  const said = ledgerReviewWord({
    reported: place?.reported === true || row?.reported === true,
    review: row?.review ?? null,
  });
  return AGENT_GRAPH_LIVE_STAGES.find((one) => t(one.key, one.word) === said)?.stage
    ?? "dispatched";
}

/* 한 결과 단계가 적은 사실이 이 행에 **지금도** 서 있는가. 단계가 앞으로 가도 옛
 * 사실은 남을 수 있다(검증된 뒤 병합됨 — 검증은 그대로다). 사실이 철회되면(검증이
 * 거두어지고 워커의 주장만 남음) 그 단계의 사건은 지금의 판이 드는 근거가 아니다. */
function agentGraphLiveStageHolds(stage, place, row) {
  const held = AGENT_GRAPH_LIVE_STAGES.find((one) => one.stage === stage);
  return held ? agentGraphLiveFlagHolds(held, place, row) : false;
}

/* 표의 한 줄이 가리키는 원장 칸이 참인가 — 위와 아래(서명)가 같은 이 한 손으로 묻는다. */
function agentGraphLiveFlagHolds(held, place, row) {
  return held.flag === "reported"
    ? place?.reported === true || row?.reported === true
    : row?.review?.[held.flag] === true;
}

/* 판의 서명에 드는 결과의 사실들 — 카드마다, 지금 서 있는 단계의 칸들. 지도가
 * 원장 행에서 읽는 것은 대기 사유만이 아니다: 결과 사건과 지난 사건의 근거 판정은
 * 행의 `review`를 읽는데, 카드는 그 칸을 싣지 않는다(`reported`만 싣는다). 서명이
 * 이것을 세지 않으면 검토만 바뀐 판은 그리기를 건너뛰어, 지도는 그 결과를 보지
 * 못하고 모델의 원장 한 벌도 옛 행에 머문다. 지도가 꺼진 판(보드의 기본값)은
 * 아무것도 더하지 않는다. */
function agentGraphLiveResultsSaid(columns, places, ledger) {
  if (!agentGraphLive) return null;
  const said = [];
  for (const column of columns) {
    for (const card of column.cards) {
      const place = places.get(card.pane);
      if (agentGraphLiveSubject(place) === null) continue;
      const row = ledger?.get(card.pane) ?? null;
      const holding = AGENT_GRAPH_LIVE_STAGES
        .filter((one) => agentGraphLiveFlagHolds(one, place, row))
        .map((one) => one.stage);
      if (holding.length > 0) said.push(`${card.pane}\u001f${holding.join(",")}`);
    }
  }
  return said.join("\u001e");
}

/* ---- 간선 -------------------------------------------------------------------- */

/* 실시간 지도가 더 그리는 선. 오버레이 고르개는 셋 중 **하나**를 고르지만
 * 실시간 지도는 「누가 누구에게」와 「누가 무엇을 기다리는가」를 함께 답해야
 * 하므로, 메일과 의존을 합집합으로 든다.
 *
 * 이미 오버레이로 서 있는 선은 빼고 돌려준다 — 같은 키의 선을 두 번 담으면
 * 하나의 `<g>`를 두 번 맞추는 셈이고, 그 둘 중 어느 쪽이 지금인지는 아무도
 * 말할 수 없다.
 *
 * 배치는 건드리지 않는다: 노드의 자리도 `topologySignature`도 그대로이므로
 * 맞춤이 다시 앉지 않고, 사람이 고른 것도 스크롤도 움직이지 않는다. */
function agentGraphLiveEdges(mail, dependencies, overlayEdges, visibleKeys) {
  if (!agentGraphLive) return [];
  const drawn = new Set((overlayEdges ?? []).map((edge) => edge.key));
  const live = [];
  for (const edge of [...(mail ?? []), ...(dependencies ?? [])]) {
    if (drawn.has(edge.key)) continue;
    if (!visibleKeys.has(edge.from) || !visibleKeys.has(edge.to)) continue;
    live.push(edge);
  }
  return live;
}

/* ---- 인스펙터가 읽는 것 ------------------------------------------------------- */

function agentGraphLiveRecentEvents() {
  return liveEvents;
}

/* 이 판에 **오지 않는** 사실, 한 표.
 *
 * 화면이 「모름」이라고 말할 때 그것이 무엇을 뜻하는지 여기 한 곳에 적는다 —
 * 그림의 이름표와 인스펙터의 주석이 같은 문장을 두 곳에서 따로 지으면 한
 * 화면이 같은 빈칸을 두 가지로 설명한다. 넷 다 원장에는 있고 이 창의 선까지
 * 오지 않는 것이며, 추측으로 메우지 않고 없다고 말한다.
 *
 *   delivery — `InboxRow.open`이 오지 않아 대기·전달·확인을 나눌 수 없다
 *   reply    — `MessageRow.thread`가 오지 않아 답장은 기록된 연결이 아니다
 *   summoner — `WorkerRow.started_by`가 오지 않아 판 없는 워커의 소환자를 모른다
 *   outside  — 카드로 매핑되지 않아 버려진 끝점의 **수**가 snapshot에 없다 */
/* 낱말은 제 키와 **한 줄에** 선다 — 창의 한글 라벨 계약
 * (`no_label_reaches_the_window_hardcoded`)은 `key: "`가 선 줄의 한글만 번역되는
 * 데이터 행으로 읽는다. 키와 낱말을 두 줄로 나누면 낱말만 남은 줄이 창에 박힌
 * 라벨로 읽힌다. */
const AGENT_GRAPH_LIVE_UNKNOWN = Object.freeze([
  { id: "delivery",
    key: "board.live.deliveryUnknown", word: "대기·전달·확인·답장을 나눌 기록은 이 스냅샷에 없습니다. 미확인 수 외의 배달 상태는 원장에서 확인합니다." },
  { id: "reply", key: "board.live.replyUnknown", word: "답장 연결은 이 스냅샷에 없습니다." },
  { id: "summoner",
    key: "board.live.summonerUnknown", word: "판을 들고 있지 않은 워커의 소환자는 이 스냅샷에 없습니다." },
  { id: "outside", key: "board.live.outsideUnknown", word: "이 보기 밖 끝점의 수는 스냅샷에 없습니다." },
]);

function agentGraphLiveUnknownWord(id) {
  const held = AGENT_GRAPH_LIVE_UNKNOWN.find((one) => one.id === id);
  return held ? t(held.key, held.word) : "";
}

/* 메일 한 줄이 말할 수 있는 배달 상태.
 *
 * 오늘 이 판에 오는 것은 `unread` 하나뿐이다. 그것이 0이라는 사실은 **전달도
 * 확인도 답장도 증명하지 않는다** — 미확인 목록에 없다는 것뿐이다. 그러므로
 * 답은 둘이다: 미확인이 있으면 그 수, 아니면 `unknown`. 넷째를 만들지 않는다. */
function agentGraphLiveDeliveryWord(edge) {
  const unread = Number(edge?.unread) || 0;
  return unread > 0
    ? t("board.live.pending", "미확인 {{count}}", { count: unread })
    : t("board.live.deliveryNotSaid", "배달 상태 미제공");
}

/* 사건 한 줄의 낱말. 종류는 이 표에서, 단계는 이미 있는 낱말에서 — 「검증됨/
 * 병합됨/배포됨/검증 대기」는 t-6815의 seam이 쓰는 그 낱말 그대로다. */
function agentGraphLiveKindWord(kind) {
  if (kind === "message") return t("board.graph.mail", "메일");
  if (kind === "dependency") return t("board.graph.dependency", "의존");
  if (kind === "assignment") return t("board.live.assignment", "배정");
  if (kind === "result") return t("board.live.result", "결과");
  return t("board.live.wait", "대기");
}

function agentGraphLiveStageWord(stage) {
  const held = AGENT_GRAPH_LIVE_STAGES.find((one) => one.stage === stage);
  return held ? t(held.key, held.word) : t("board.desk.stageDispatched", "진행");
}

/* 기다림의 사유를 코디네이터 데스크가 쓰는 그 표의 낱말로 — 한 화면이 같은
 * 기다림을 두 낱말로 부르지 않도록. */
function agentGraphLiveCauseWord(cause) {
  const health = DESK_HEALTH.find((one) => one.id === cause);
  return health ? t(health.key, health.word) : cause;
}

/* 사건 하나가 **무엇을 근거로 하는가**, 실제 식별자만. 없는 것은 쓰지 않는다. */
function agentGraphLiveEventFacts(event) {
  const source = event.evidence ?? {};
  if (event.kind === "message") {
    return [source.run, source.messageId, source.messageKind].filter(Boolean).join(" · ");
  }
  if (event.kind === "dependency") {
    return [source.run, `${source.dependencyId} → ${source.taskId}`,
      agentGraphTaskStateWord(source.taskState)].filter(Boolean).join(" · ");
  }
  if (event.kind === "wait") {
    /* 줄이 사유를 말하지 않으면 「대기」라는 종류만 남고, 그것은 이 줄이 답해야
     * 할 질문에 답하지 않는다. */
    return [source.run, source.workerId, agentGraphLiveCauseWord(source.cause)]
      .filter(Boolean).join(" · ");
  }
  return [source.run, source.taskId, source.dispatchId,
    agentGraphLiveStageWord(source.stage)].filter(Boolean).join(" · ");
}

/* 사건 한 줄의 「언제」. 발생 시각을 아는 사건은 그 나이를, 모르는 사건은
 * **모른다고** 말하고 이 창이 본 때를 따로 적는다 — 곁의 다른 시각(과업이 생긴
 * 때, 배차가 시작된 때)을 사건의 나이로 빌려 쓰면 그것이 곧 추측이다. */
function agentGraphLiveWhenWord(event, now) {
  if (event.at > 0) return t("board.desk.mailAge", "{{time}} 전", { time: agoWord(event.at, now) });
  return `${t("board.live.occurredUnknown", "발생 시각 미제공")} · ${
    t("board.live.observedAgo", "관측 {{time}} 전", { time: agoWord(event.observedAt, now) })}`;
}

/* 누른 사건의 근거가 **지금 어디에 있는가**. 답은 셋이다:
 *
 *   `"current"` — 지금의 관계·판이 바로 이 사건의 근거를 든다. 이미 있는 문
 *                 (관계 선택, 노드 선택)으로 가면 같은 식별자가 선다.
 *   `"past"`    — 끝점은 그대로이지만 그곳의 근거는 이제 다른 것이다: 같은 두
 *                 자리 사이에 더 새 메시지가 왔거나, 같은 판에 다른 시도가
 *                 앉았거나, 의존의 상태가 그 뒤로 바뀌었다. **같은 시도**에서도
 *                 그렇다 — 기다림의 사유나 그 사유가 적힌 때가 바뀌었거나, 결과가
 *                 적은 사실이 철회되었다.
 *   `"outside"` — 이 사건의 끝점이 지금 스냅샷에 없다.
 *
 * 둘째와 셋째에서 지금의 관계나 판으로 가면, 그곳이 보여 주는 것은 이 사건의
 * 근거가 아니라 그 뒤의 것이다 — 누른 줄과 열린 근거가 서로 다른 사건을
 * 말하게 된다. 판정은 사건 종류마다 그 사건의 키가 된 근거를 그대로 견준다:
 * 자리 셋만 같다고 지금이 아니다. */
function agentGraphLiveEventReach(view, event) {
  const model = agentGraphFullModel(view);
  if (!model) return "outside";
  const source = event.evidence ?? {};
  if (event.kind === "message") {
    const relation = agentGraphRelations(model).find((edge) => edge.key === event.edgeKey);
    if (!relation) return "outside";
    return relation.evidence?.id === source.messageId && (relation.evidence?.run ?? "") === source.run
      ? "current" : "past";
  }
  if (event.kind === "dependency") {
    const fact = (model.source.overlays.task_dependencies ?? []).find((one) =>
      (one.run ?? "") === source.run && (one.task ?? "") === source.taskId
      && (one.dependency ?? "") === source.dependencyId);
    if (!fact) return "outside";
    if ((fact.task_state ?? "") !== source.taskState
      || (fact.dependency_state ?? null) !== source.dependencyState) return "past";
    const reachable = event.edgeKey
      ? agentGraphRelations(model).some((edge) => edge.key === event.edgeKey)
      : model.agents.some((entry) => entry.key === event.to);
    return reachable ? "current" : "outside";
  }
  const entry = model.agents.find((one) => one.key === event.to);
  if (!entry) return "outside";
  const [pane, seat] = event.seats?.[0] ?? [];
  if (agentGraphLiveSeat(entry.place) !== seat) return "past";
  /* 같은 자리, 같은 시도. 원장 행은 이 판이 카드와 함께 물은 그 한 벌에서 온다. */
  const row = model.source.ledger?.get(pane) ?? null;
  if (event.kind === "wait") {
    const wait = agentGraphLiveWaitOf(row, entry.card);
    return wait?.cause === source.cause && wait.since === source.since ? "current" : "past";
  }
  if (event.kind === "result") {
    return agentGraphLiveStageHolds(source.stage, entry.place, row) ? "current" : "past";
  }
  return "current";
}

/* 사건 한 줄을 누른다. 근거가 지금도 같은 자리에 있으면 이 표면이 이미 가진
 * 문으로 간다. 아니면 **가지 않는다** — 그 줄이 제 근거(원장의 식별자)를 펴고,
 * 왜 지금의 관계로 대신 열지 않는지를 말한다. */
function agentGraphLiveOpenEvent(view, event) {
  liveSelectedEventKey = event.key;
  if (agentGraphLiveEventReach(view, event) === "current") {
    if (event.edgeKey) selectAgentGraphRelation(view, event.edgeKey);
    else selectAgentGraphEntity(view, event.to, { focus: true });
    return;
  }
  const model = agentGraphModels.get(view);
  if (model) paintAgentGraphInspector(view, model);
}

/* 편 줄의 근거: 이 사건이 일어났을 때 원장이 적은 식별자, 그대로. 낱말은
 * 인스펙터가 이미 쓰는 이름표들이다. */
function agentGraphLiveEventDetail(event, reach) {
  const block = taskBoardElement("div", "agent-live-event-detail");
  block.append(taskBoardElement("p", "agent-relation-note", reach === "outside"
    ? t("board.live.eventOutside",
      "이 사건의 끝점은 지금 스냅샷에 없습니다. 아래 식별자로 원장에서 확인합니다.")
    : t("board.live.eventNotLatest",
      "이 사건의 근거는 지금 스냅샷의 최신 기록이 아닙니다. 지금 기록으로 대신 열지 않고, 아래 식별자로 원장에서 확인합니다.")));
  const source = event.evidence ?? {};
  const fields = [[t("board.graph.run", "런"), source.run]];
  if (event.kind === "message") {
    fields.push(
      [t("board.graph.messageId", "메시지 ID"), source.messageId],
      [t("board.graph.sender", "발신 주소"), source.address?.from],
      [t("board.graph.recipient", "수신 주소"), source.address?.to]);
  } else if (event.kind === "dependency") {
    fields.push(
      [t("board.graph.upstreamTask", "선행 과업"),
        `${source.dependencyId} · ${agentGraphTaskStateWord(source.dependencyState)}`],
      [t("board.graph.downstreamTask", "후속 과업"),
        `${source.taskId} · ${agentGraphTaskStateWord(source.taskState)}`],
      [t("board.graph.taskCreated", "후속 과업 생성"), agentGraphTimeWord(source.taskCreated)]);
  } else if (event.kind === "wait") {
    fields.push(
      [t("board.graph.workerId", "워커 ID"), source.workerId],
      [t("board.graph.dispatchId", "배차 ID"), source.dispatchId],
      [t("board.live.wait", "대기"), agentGraphLiveCauseWord(source.cause)]);
  } else {
    fields.push(
      [t("board.graph.taskId", "과업 ID"), source.taskId],
      [t("board.graph.workerId", "워커 ID"), source.workerId],
      [t("board.graph.dispatchId", "배차 ID"), source.dispatchId],
      [t("board.graph.retryOf", "이전 시도"), source.retryOf],
      [t("board.graph.attemptStarted", "시도 시작"), agentGraphTimeWord(source.dispatchStarted)]);
    /* 결과 사건은 그때 원장이 적은 사실을 제 낱말로 든다 — 같은 시도에서 그 사실이
     * 철회되었으면, 지금의 판이 아니라 이 줄이 그것을 말한다. */
    if (event.kind === "result") {
      fields.push([t("board.live.result", "결과"), agentGraphLiveStageWord(source.stage)]);
    }
  }
  fields.push(
    [t("board.live.occurredAt", "발생 시각"), event.at > 0
      ? agentGraphTimeWord(event.at) : t("board.live.occurredUnknown", "발생 시각 미제공")],
    [t("board.live.observedAt", "관측 시각"), agentGraphTimeWord(event.observedAt)]);
  for (const [label, value] of fields) {
    const row = agentGraphDetailField(label, value);
    if (row) block.append(row);
  }
  return block;
}

/* 인스펙터가 사건 목록을 다시 지을지 묻는 서명. 사건의 키와 나이 낱말(시계가
 * 흐르면 바뀐다), 그리고 사람이 편 줄과 그 줄의 근거가 지금 어디에 있는가. */
function agentGraphLiveEventsSaid(view, now = Date.now()) {
  const selected = liveEvents.find((event) => event.key === liveSelectedEventKey);
  return [
    liveEvents.map((event) => `${event.key}\u001f${agentGraphLiveWhenWord(event, now)}`),
    selected ? [selected.key, agentGraphLiveEventReach(view, selected)] : null,
  ];
}

/* 인스펙터의 관계 탭 머리에 서는 한 구역. **새 문을 만들지 않는다** — 줄을
 * 누르면 이 표면이 이미 가진 문으로 간다(관계 선택, 노드 선택). 그리고 이
 * 목록은 제 범위를 스스로 적는다: 이 판이 실어 온 것은 관계마다 마지막 한
 * 통이므로, 여기 없는 사건이 일어나지 않았다는 뜻은 아니다. */
function agentGraphLiveEventsNode(view, now = Date.now()) {
  const block = taskBoardElement("section", "agent-live-events");
  block.append(taskBoardElement("h4", "agent-live-events-head",
    t("board.live.events", "최근 사건")));
  block.append(taskBoardElement("p", "agent-live-events-scope",
    t("board.live.eventsScope",
      "관계마다 원장이 실어 온 마지막 메시지까지만 셉니다. 전문과 나머지 통은 원장에 있습니다.")));
  const coverage = agentGraphLiveCoverage();
  if (coverage.truncated > 0) {
    block.append(taskBoardElement("p", "agent-live-events-note",
      t("board.live.coverageTruncated",
        "중복 제거 범위가 끊긴 관계 {{count}}개 — 그 구간의 사건은 표시되지 않을 수 있습니다.",
        { count: coverage.truncated })));
  }
  if (coverage.forgotten > 0) {
    block.append(taskBoardElement("p", "agent-live-events-note",
      t("board.live.coverageForgotten",
        "기억 상한을 넘어 잊은 관계가 있습니다 — 그 관계의 사건은 표시되지 않을 수 있습니다.")));
  }
  /* 매핑되지 않아 백엔드가 버린 끝점의 **수**는 이 판에 오지 않는다. 지어내는
   * 대신 미제공이라 적는다 — 문장은 위의 한 표에서 온다. */
  block.append(taskBoardElement("p", "agent-live-events-note",
    agentGraphLiveUnknownWord("outside")));
  const list = taskBoardElement("ol", "agent-live-event-list");
  for (const event of agentGraphLiveRecentEvents()) {
    const row = taskBoardElement("li", "agent-live-event");
    const button = taskBoardElement("button", `agent-live-event-main is-${event.kind}`);
    button.type = "button";
    button.dataset.liveEvent = event.key;
    const selected = event.key === liveSelectedEventKey;
    writeAttribute(button, "aria-pressed", String(selected));
    button.append(
      taskBoardElement("strong", "agent-live-event-kind", agentGraphLiveKindWord(event.kind)),
      taskBoardElement("span", "agent-live-event-facts", agentGraphLiveEventFacts(event)),
      taskBoardElement("span", "agent-live-event-when", agentGraphLiveWhenWord(event, now)),
    );
    if (Number.isFinite(event.added) && event.added > 1) {
      button.append(taskBoardElement("span", "agent-live-event-added",
        t("board.live.added", "이번 판에 {{count}}통 늘었습니다", { count: event.added })));
    }
    button.onclick = () => agentGraphLiveOpenEvent(view, event);
    row.append(button);
    if (selected) {
      const reach = agentGraphLiveEventReach(view, event);
      if (reach !== "current") row.append(agentGraphLiveEventDetail(event, reach));
    }
    list.append(row);
  }
  if (list.childElementCount === 0) {
    block.append(taskBoardElement("p", "agent-relation-note",
      t("board.live.noEvents", "이 보기를 연 뒤로 새로 기록된 사건이 없습니다.")));
  } else block.append(list);
  return block;
}

/* ---- 손잡이와 수명 ------------------------------------------------------------ */

/* 켜고 끄기. 켜는 판은 **조용히** 선다: 그동안 쌓인 것을 한꺼번에 터뜨리는
 * 대신 지금의 모습을 baseline으로 삼는다. 끄는 판은 시계와 맥박을 거둔다. */
function setAgentGraphLive(view, on) {
  if (agentGraphLive === on) return;
  agentGraphLive = on;
  liveGeneration += 1;
  if (on) {
    liveBaselineDue = true;
  } else {
    agentGraphLiveStop();
    liveEvents = [];
    liveSelectedEventKey = null;
  }
}

/* run이나 범위가 움직이면 — 늦게 도착할 이전 응답이 지금의 지도를 덮지 못하게
 * 세대를 올리고, 새 범위의 지금 모습을 다시 baseline으로 삼는다. */
function agentGraphLiveScopeMoved() {
  liveGeneration += 1;
  liveBaselineDue = true;
  /* 사건 목록도 함께 내려놓는다: 그 줄들은 **떠나온 범위**에서 일어난 일이고,
   * 누르면 지금 화면에 없는 관계로 가려 든다. 근거는 원장에 그대로 남는다. */
  liveEvents = [];
  liveSelectedEventKey = null;
  agentGraphLiveStop();
}

/* 고른 범위가 스냅샷에서 **사라진** 판. 사람이 범위를 옮긴 것은 아니지만, 지도가
 * 서 있던 자리가 없어졌으므로 범위를 떠난 것과 같이 다룬다 — 떠나온 범위의 맥박과
 * 사건 줄을 내리고, 그 사이 떠난 답이 덮지 못하게 세대를 올린다. 같은 범위가
 * 사라져 있는 동안 판마다 다시 떠나지는 않는다: 한 번 사라진 범위를 기억하고,
 * 그 범위가 돌아오면 잊는다. */
let liveScopeGone = null;

function agentGraphLiveScopePresent(key, present) {
  if (present) {
    if (liveScopeGone === key) liveScopeGone = null;
    return;
  }
  /* 지도가 꺼진 판은 기준선이 이미 빚으로 남아 있다 — 켜는 판이 조용히 선다. */
  if (!agentGraphLive || liveScopeGone === key) return;
  liveScopeGone = key;
  agentGraphLiveScopeMoved();
}

function agentGraphLiveGeneration() {
  return liveGeneration;
}

/* 부서진 판이 다시 서거나 창이 숨었다 돌아오면 — backlog는 사건이 아니다. */
function agentGraphLiveListen(target, type, handler) {
  target.addEventListener(type, handler);
  liveListeners += 1;
}

agentGraphLiveListen(document, "visibilitychange", () => {
  /* 숨는 문서도 돌아오는 문서도 빚을 남긴다 — 숨은 동안 읽은 판은 사건이 아니고,
   * 돌아온 첫 판은 조용한 기준선이다. */
  liveBaselineDue = true;
  if (document.hidden) agentGraphLiveStop();
});
