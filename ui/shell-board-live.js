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
 *
 * **자리는 신원이 아니다.** `term:N`은 이 화면 안에서 노드를 찾는 **탐색키**일
 * 뿐이고, 주체는 `run/worker/dispatch` 셋이다. 판이 다시 앉거나 시도가 바뀌면
 * 그 자리의 주체가 바뀐 것이므로, 앞 주체가 남긴 맥박·watermark·수의 기준선을
 * 그 자리에서 **버린다**(아래 `liveSubjects`) — 앞사람의 사건이 뒷사람의
 * 사건으로 넘어가지 않게. 셋 중 하나라도 없는 카드는 `unverified`이고, 그런
 * 카드는 배정·결과 사건을 만들지 않는다.
 *
 * **revision이 없는 스트림.** `GraphOverlaySnapshot`에는 revision 필드가
 * 없다. 그래서 여기서도 만들지 않는다. 최신성은 셋을 **함께** 써서 판정한다 —
 * 실제 event id, 관계마다의 stamp watermark, 그리고 범위가 움직일 때 올라가는
 * 세대 빗장. stamp 하나만으로는 같은 시각의 두 사건을 가릴 수 없으므로,
 * watermark는 **엄격히 옛것**만 버리고 같은 시각의 다른 id는 서로 다른 실제
 * 사건으로 받는다. */

/* 손잡이. 기본은 꺼짐 — 실시간 지도는 고르는 그림이지 기본 그림이 아니다. */
let agentGraphLive = false;

/* 상한과 양. 둘 다 **데이터의 양**이라 토큰이 아니라 상수다(맥박이 사는
 * 길이와 한 판의 맥박 수는 시각 수치라 토큰이 든다 — `agentGraphTuning`). */
const AGENT_GRAPH_LIVE = Object.freeze({
  /* 기억하는 관계의 수. 세션 내내 무한히 쌓이면 이것이 두 번째 원장이 된다.
   * 넘치면 오래 손대지 않은 관계부터 버리고, 버린 관계가 다시 나타나면
   * **조용히 다시 맞춘다**(아래 `adopt`) — 맥박 하나를 놓치는 쪽을 고른다. */
  laneKeep: 256,
  /* 한 관계에서 **같은 밀리초**에 일어난 서로 다른 사건을 몇 개까지 기억하는가.
   * 완전한 중복 제거가 성립하는 것은 이 수 안에서다. 넘으면 그 관계는
   * `truncated`가 되고 조용한 재동기화로 넘어간다 — 그 창의 사건은 맥박이
   * 되지 않을 수 있고, 화면은 그 사실을 숨기지 않는다. */
  sameStampKeep: 16,
  /* 인스펙터가 그리는 최근 사건 줄 수. 근거 보존은 원장의 일이고 이 목록은
   * 그 원장으로 가는 손잡이일 뿐이다. */
  eventKeep: 24,
});

/* ---- 장부 ------------------------------------------------------------------ */

/* 관계 하나가 지금까지 말한 것: `{ stamp, keys, count, truncated }`.
 *
 * 중복 제거의 **경계를 여기 한 곳에 적는다**. 사건 키의 전역 목록 하나로는
 * 이 일을 할 수 없다: A@T와 B@T가 지나간 뒤 상한이 A를 밀어내면, 마지막 키는
 * B이고 stamp는 같으므로 다시 온 A@T가 새 사건으로 보인다. 그래서 기억하는
 * 것은 「마지막 키」가 아니라 **watermark 밀리초에 본 키들의 집합**이다.
 *
 *   · `evStamp < stamp` — 옛 사건. 버린다. 집합을 보지 않아도 된다.
 *   · `evStamp === stamp` — `keys`에 있으면 같은 사건, 없으면 같은 시각의
 *     **다른 실제 사건**이다. 합치지 않는다.
 *   · `evStamp > stamp` — 새 사건. watermark가 올라가고 집합은 그 하나로
 *     다시 시작한다.
 *
 * 이것으로 「새 사건 누락」과 「옛 사건 재생」이 **동시에** 풀리는 것은 아니다.
 * 성립하는 범위는 둘이다 — 한 밀리초에 같은 관계에서 `sameStampKeep`개까지,
 * 그리고 `laneKeep`개까지의 관계. 그 밖에서는 완전한 중복 제거를 주장하지
 * 않고 **조용한 재동기화**로 물러난다(`adopt`): 지금 상태를 본 것으로 적고
 * 맥박은 내지 않는다. 놓친 맥박은 아무것도 거짓말하지 않지만, 지어낸 맥박은
 * 없던 사건을 만든다.
 *
 * `count`는 같은 주체·같은 관계에서의 **실제 증가**만 세기 위한 기준선이다. */
const liveLanes = new Map();
/* 상한에 밀려난 관계의 이름표. 이것이 있어야 「처음 보는 관계」와 「기억을
 * 잃은 관계」를 가릴 수 있다. 이 표까지 넘치면 그 구별이 끝나므로, 그 사실을
 * 적어 두고(`liveDropTruncated`) 그 뒤로는 모든 낯선 관계를 조용히 다시
 * 맞춘다 — 놓친 맥박은 아무것도 거짓말하지 않는다. */
const liveDropped = new Set();
let liveDropTruncated = false;

/* 자리마다의 주체 — `run/worker/dispatch`. 이것이 바뀌면 그 자리의 모든 기억을
 * 버린다.
 *
 * 사라진 자리를 **솎아 내지 않는다**. 판이 풀렸다가 같은 번호로 다시 열리는
 * 것이 바로 앞사람의 사건을 뒷사람이 물려받는 길이고, 그 자리의 주체를 잊으면
 * 다음 판에서 「처음 보는 자리」로 읽혀 버릴 것을 버리지 못한다. 키는 이 창이
 * 연 터미널 번호이므로 한 세션 동안의 수는 그 자체로 상한이다. */
const liveSubjects = new Map();
/* 지금 뛰는 맥박: 키(노드 또는 간선) → { beat, eventKey, untilMs }. */
const livePulses = new Map();
/* 인스펙터가 읽는 최근 사건, 새 것이 앞. */
let liveEvents = [];
/* 맥박이 꺼질 때를 기다리는 시계 하나. */
let livePulseTimer = null;
/* 다음 판을 **조용히 삼킨다**: 첫 snapshot·손잡이를 켠 순간·범위 이동·부서진
 * 판의 복구·숨김에서 돌아온 첫 판. 놓친 backlog가 방금 생긴 활동처럼 터지지
 * 않는다. */
let liveBaselineDue = true;
/* 범위가 움직일 때 올라가는 빗장. 이보다 낮은 세대에서 떠난 비동기 갱신은
 * 지금의 지도·선택·근거를 덮지 못한다. revision 주장이 아니다. */
let liveGeneration = 0;

/* 판이 마지막으로 읽은 원장 행, 카드의 자리마다. `boardCards()`가 이미 물은
 * 그 한 벌을 그대로 들고 있는다 — 같은 순간의 같은 답이라야 노드의 대기 사유와
 * 그 노드의 카드가 같은 snapshot을 말한다. 따로 물으면 두 시각이 한 카드에
 * 실린다. */
const liveLedgerRows = new Map();

function rememberAgentGraphLedgerRows(rows) {
  liveLedgerRows.clear();
  for (const row of Array.isArray(rows) ? rows : []) {
    if (!row?.worker) continue;
    liveLedgerRows.set(
      typeof row.term === "number" ? `term:${row.term}` : `worker:${row.worker}`,
      row,
    );
  }
}

function agentGraphLedgerRow(pane) {
  return liveLedgerRows.get(pane) ?? null;
}

/* ---- 주체 ------------------------------------------------------------------- */

/* 이 자리에 지금 앉아 있는 **주체**. 셋 다 있어야 주체이고, 하나라도 없으면
 * `null`이다 — 그런 카드는 확인되지 않은 것(`unverified`)이지 「배정 없음」이
 * 아니다. */
function agentGraphLiveSubject(place) {
  const run = place?.run ?? "";
  const worker = place?.workerId ?? "";
  const dispatch = place?.dispatchId ?? "";
  return run && worker && dispatch ? `${run}\u001f${worker}\u001f${dispatch}` : null;
}

/* 자리의 주체가 바뀌었으면 그 자리의 기억을 버린다. 판이 다시 앉았든, 새
 * 시도가 같은 판을 물려받았든, 화면의 키는 같아도 **사건의 주인이 다른
 * 사람**이다. */
function agentGraphLiveReseat(pane, subject) {
  const held = liveSubjects.get(pane);
  if (held === subject) return false;
  if (subject === null) liveSubjects.delete(pane);
  else liveSubjects.set(pane, subject);
  if (held === undefined) return false;
  livePulses.delete(`agent:${pane}`);
  /* 이 자리를 끝점으로 들고 있던 관계만. 이름으로 부분 일치를 보면
   * `term:30`이 `term:301`을 함께 지우므로, 관계는 제 끝점을 적어 둔다. */
  for (const [lane, value] of [...liveLanes]) {
    if (value.panes?.includes(pane)) liveLanes.delete(lane);
  }
  return true;
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

/* ---- 사건의 신원 ------------------------------------------------------------ */

/* 한 사건의 키. 앞머리가 종류, 나머지가 **실제로 기록된 식별자**다. 같은
 * `created_ms`를 가진 서로 다른 두 사건은 id가 달라 절대 합쳐지지 않는다. */
function agentGraphLiveEventKey(kind, parts) {
  return [kind, ...parts].join("\u001f");
}

/* 관계 하나가 말한 것을 장부에 접는다. 돌려주는 답은 셋이다:
 *
 *   `"seen"`  — 이미 아는 사건이거나 watermark보다 옛것. 아무 일도 없다.
 *   `"adopt"` — 이 관계를 **조용히 다시 맞춘다**. 기억이 없거나(처음 보는
 *               관계, 상한에 밀려났던 관계) 같은 시각의 기억이 상한을 넘었다.
 *               지금 상태를 적고 맥박은 내지 않는다.
 *   `"new"`   — 처음 보는 실제 사건. 맥박의 후보다.
 *
 * `laneKeep`은 다시 넣는 것으로 최근 순서를 지킨다 — Map은 넣은 차례를
 * 기억하므로, 손댄 관계를 지웠다 다시 넣으면 첫 키가 늘 가장 오래 손대지 않은
 * 관계다. */
function agentGraphLiveLane(lane, stamp, key, panes = []) {
  const held = liveLanes.get(lane);
  if (held === undefined) {
    /* 처음 보는 관계인가, 상한에 밀려났던 관계인가. 그 둘은 **다른 답**을
     * 받는다: 처음 보는 관계에서 일어난 일은 실제로 처음 보는 사건이고,
     * 밀려났던 관계는 이 창이 그 사이의 기억을 잃었으므로 조용히 다시
     * 맞춘다. 셋째 경우 — 밀려난 관계의 이름표까지 넘쳤을 때 — 는 둘을 가릴
     * 수 없으므로 보수적인 쪽으로 간다. */
    const dropped = liveDropped.delete(lane) || liveDropTruncated;
    agentGraphLiveKeepLane(lane,
      { stamp, keys: new Set([key]), count: null, truncated: dropped, panes });
    return dropped ? "adopt" : "new";
  }
  liveLanes.delete(lane);
  if (stamp < held.stamp) {
    liveLanes.set(lane, held);
    return "seen";
  }
  if (stamp > held.stamp) {
    agentGraphLiveKeepLane(lane,
      { stamp, keys: new Set([key]), count: held.count, truncated: false, panes });
    return "new";
  }
  if (held.keys.has(key)) {
    liveLanes.set(lane, held);
    return "seen";
  }
  if (held.keys.size >= AGENT_GRAPH_LIVE.sameStampKeep) {
    /* 같은 밀리초의 기억이 상한을 넘었다. 여기서부터는 이 관계의 그 시각에
     * 대해 완전한 중복 제거를 말할 수 없으므로, 주장하는 대신 물러난다. */
    agentGraphLiveKeepLane(lane,
      { stamp, keys: new Set([key]), count: held.count, truncated: true, panes });
    return "adopt";
  }
  held.keys.add(key);
  held.panes = panes;
  liveLanes.set(lane, held);
  return "new";
}

function agentGraphLiveKeepLane(lane, value) {
  liveLanes.set(lane, value);
  while (liveLanes.size > AGENT_GRAPH_LIVE.laneKeep) {
    const [oldest] = liveLanes.keys();
    if (oldest === lane) break;
    liveLanes.delete(oldest);
    liveDropped.add(oldest);
    if (liveDropped.size > AGENT_GRAPH_LIVE.laneKeep) liveDropTruncated = true;
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
 * 장부가 스스로 답한다 — 상한에 밀려 조용히 다시 맞춘 관계가 있으면 그 수를
 * 함께 든다. */
function agentGraphLiveCoverage() {
  let truncated = 0;
  for (const lane of liveLanes.values()) if (lane.truncated) truncated += 1;
  return {
    lanes: liveLanes.size,
    laneKeep: AGENT_GRAPH_LIVE.laneKeep,
    sameStampKeep: AGENT_GRAPH_LIVE.sameStampKeep,
    dropped: liveDropped.size,
    droppedTruncated: liveDropTruncated,
    truncated,
    complete: truncated === 0 && liveDropped.size === 0 && !liveDropTruncated,
  };
}

/* ---- 한 판의 사실을 장부에 접는다 -------------------------------------------- */

/* 이 창이 방금 받은 snapshot 하나. 돌려주는 것은 없다 — 장부가 움직이고,
 * 그리는 손이 그 장부를 읽는다.
 *
 * `places`는 카드마다의 run/task/dispatch/worker 신원이고, `overlays`는
 * 백엔드가 이미 투영한 관계다. 이 함수는 그 둘과 원장 행만 읽는다.
 *
 * `generation`은 이 snapshot이 떠날 때의 세대다. 그 사이 범위가 움직였으면
 * 늦게 도착한 이 답은 **통째로 버린다** — 지금의 지도를 옛 범위의 사실로
 * 덮지 않는다. */
function agentGraphLiveObserve(answer, places, now = Date.now(), generation = liveGeneration) {
  if (generation !== liveGeneration) return;
  if (!agentGraphLive) {
    /* 꺼져 있는 동안에도 baseline은 빚으로 남는다: 다시 켜는 판이 그동안의
     * backlog를 한꺼번에 터뜨리지 않도록. 지도가 서 있지 않은 판에서 이
     * 함수가 하는 일은 이 한 줄뿐이다. */
    liveBaselineDue = true;
    return;
  }
  const overlays = answer?.overlays ?? {};
  /* 이 판의 카드, 자리마다. 대기 판정은 원장의 행과 **이 판의** 카드를 함께
   * 묻는다(`deskWorkerHealth`) — 지난 판의 모델에서 카드를 꺼내 오면 한 박자
   * 뒤진 상태로 사유를 고르게 된다. */
  const cards = new Map((answer?.columns ?? [])
    .flatMap((column) => (column.cards ?? []).map((card) => [card.pane, card])));
  const fresh = [];

  /* ⓪ 자리마다의 주체를 먼저 맞춘다. 주체가 바뀐 자리는 이 판을 읽기 **전에**
   *    제 기억을 버리므로, 앞 주체의 watermark가 새 주체의 첫 사건을 삼키지
   *    않는다. */
  for (const [pane, place] of places ?? []) {
    agentGraphLiveReseat(pane, agentGraphLiveSubject(place));
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
    const before = liveLanes.get(lane)?.count ?? null;
    const verdict = agentGraphLiveLane(lane, stamp, key, [edge.from, edge.to]);
    /* 이 관계에서 **몇 통이 늘었는가**. 기준선이 없으면(처음 보는 관계, 자리가
     * 새 주체로 바뀐 판, 상한에 밀려 다시 맞춘 관계, 다시 켠 판) 증가가 아니라
     * baseline이다 — 재연결을 활동으로 읽지 않는다. */
    const count = Number(edge.count) || 0;
    const added = verdict === "new" && before !== null ? Math.max(0, count - before) : null;
    liveLanes.get(lane).count = count;
    if (verdict !== "new") continue;
    const event = {
      key,
      kind: "message",
      at: stamp,
      from: agentGraphAgentKey(edge.from),
      to: agentGraphAgentKey(edge.to),
      edgeKey,
      added,
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
    if (agentGraphLiveLane(lane, stamp, key, [fact.from, fact.to].filter(Boolean)) !== "new") continue;
    const event = {
      key,
      kind: "dependency",
      at: stamp,
      from: fact.from ? agentGraphAgentKey(fact.from) : null,
      to: agentGraphAgentKey(fact.to),
      edgeKey: fact.from
        ? `overlay:dependency:${agentGraphAgentKey(fact.from)}>${agentGraphAgentKey(fact.to)}`
        : null,
      added: null,
      evidence: {
        run: fact.run ?? "",
        taskId: fact.task ?? "",
        dependencyId: fact.dependency ?? "",
        taskState: fact.task_state ?? "",
        dependencyState: fact.dependency_state ?? null,
      },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  /* ③ 시도와 결과 — 카드의 dispatch 신원과 t-6815의 권위 seam. 워커의 주장
   *    (`reported`)과 코디네이터의 사실(검증/병합/배포)은 **다른 사건**이고,
   *    키가 달라 서로를 덮지 않는다. 재시도는 `dispatch_id`가 다르므로 그
   *    자체로 다른 시도이고, 위 ⓪에서 이미 자리의 기억을 새로 받았다.
   *
   *    셋 중 하나라도 없는 카드는 주체가 없다 — 사건을 만들지 않는다. */
  for (const [pane, place] of places ?? []) {
    const subject = agentGraphLiveSubject(place);
    if (subject === null) continue;
    const lane = `work:${subject}`;
    const started = Number(place.dispatchStarted) || 0;
    const row = agentGraphLedgerRow(pane);
    const stage = ledgerReviewStage(place, row);
    const key = agentGraphLiveEventKey("work", [subject, stage]);
    if (agentGraphLiveLane(lane, started, key, [pane]) !== "new") continue;
    const event = {
      key,
      kind: stage === "dispatched" ? "assignment" : "result",
      at: started,
      from: null,
      to: `agent:${pane}`,
      edgeKey: null,
      added: null,
      evidence: {
        run: place.run ?? "",
        taskId: place.taskId ?? "",
        workerId: place.workerId ?? "",
        dispatchId: place.dispatchId,
        retryOf: place.retryOf ?? null,
        stage,
      },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  /* ④ 기다림의 **사유가 바뀐** 판. 나이가 흐르는 것은 사건이 아니다 —
   *    사유와 그 사유가 기록된 stamp가 키를 이루므로, 30초 박자가 나이 낱말을
   *    바꾸어도 맥박은 없다. */
  for (const [pane, row] of liveLedgerRows) {
    const wait = agentGraphLiveWaitOf(row, cards.get(pane) ?? null, now);
    if (!wait) continue;
    const lane = `wait:${row.run ?? ""}\u001f${row.worker ?? ""}`;
    const key = agentGraphLiveEventKey("wait",
      [row.run ?? "", row.worker ?? "", wait.cause, String(wait.since)]);
    if (agentGraphLiveLane(lane, wait.since, key, [pane]) !== "new") continue;
    const event = {
      key,
      kind: "wait",
      at: wait.since,
      from: null,
      to: `agent:${pane}`,
      edgeKey: null,
      added: null,
      evidence: { run: row.run ?? "", workerId: row.worker ?? "", cause: wait.cause, since: wait.since },
    };
    if (agentGraphLiveNote(event)) fresh.push(event);
  }

  if (liveBaselineDue) {
    /* 처음 한 판은 통째로 삼킨다. 위에서 관계마다의 watermark는 이미 지금
     * 상태로 서 있고 `fresh`는 비어 있다 — 그림은 조용히 지금의 모습으로 선다. */
    liveBaselineDue = false;
    return;
  }
  agentGraphLiveStartPulses(fresh, now);
}

/* ---- 맥박 -------------------------------------------------------------------- */

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
   * 우열을 매긴 적이 없는데 그리는 차례가 우열을 만든 셈이다. 기록된 시각이
   * 늦은 것부터 든다: 접히는 것은 언제나 **더 오래된 사건**이다. */
  const ordered = [...fresh].sort((left, right) => (right.at || 0) - (left.at || 0));
  let room = tuning.burst;
  for (const event of ordered) {
    if (room <= 0) break;
    const key = event.edgeKey ?? event.to;
    if (!key) continue;
    /* 박자는 **그 자리의 지난 박자**를 뒤집는다. 판마다 하나로 번갈아 적으면
     * 한 판을 건너뛴 자리가 두 판 만에 같은 글자를 다시 받고, 같은 글자를 다시
     * 쓰는 것은 쓰기가 아니므로(값이 같으면 안 쓴다) 그 맥박은 뛰지 않는다. */
    const beat = livePulses.get(key)?.beat === "a" ? "b" : "a";
    livePulses.set(key, { beat, eventKey: event.key, untilMs: until });
    room -= 1;
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
    const at = Date.now();
    for (const [key, pulse] of [...livePulses]) {
      if (pulse.untilMs <= at) livePulses.delete(key);
    }
    for (const view of agentGraphLiveViews()) dressAgentGraphLive(view);
    agentGraphLiveArmExpiry(at);
  }, Math.max(1, next - now));
}

/* 뷰를 떠나거나 손잡이를 끄거나 판이 숨으면 — 시계도 맥박도 남기지 않는다.
 * 들어갔다 나오기를 되풀이해도 활성 핸들이 늘지 않는 것은 이 한 손 때문이다. */
function agentGraphLiveStop() {
  if (livePulseTimer !== null) clearTimeout(livePulseTimer);
  livePulseTimer = null;
  livePulses.clear();
  for (const view of agentGraphLiveViews()) dressAgentGraphLive(view);
}

/* 지금 서 있는 시계와 맥박의 수 — 들어갔다 나오기를 되풀이해도 늘지 않는다는
 * 것을 시험이 실제 scheduler 경계에서 세기 위한 한 줄. */
function agentGraphLiveHandles() {
  return {
    timers: livePulseTimer === null ? 0 : 1,
    pulses: livePulses.size,
    lanes: liveLanes.size,
    listeners: 1,
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

/* 지금 그림이 서 있는 보드 판. 작업 목록으로 서 있는 판은 관계 그림이 아니다.
 *
 * 탭 장부가 아니라 **문서**에게 묻는다. 팝아웃으로 보드를 빼면 본창의
 * `boardTab()`은 빈손이 되고(그 탭이 저쪽으로 갔다), 그러면 이 손이 판을 찾지
 * 못해 맥박이 아예 서지 않는다 — 판은 저쪽 문서에 멀쩡히 서 있는데. 문서에
 * 묻는 쪽은 본창·팝아웃·복제된 판을 모두 같은 규칙으로 답한다. */
function agentGraphLiveViews() {
  return [...document.querySelectorAll(".agent-board")]
    .filter((view) => !view.hidden && !view.classList.contains("is-task-board"));
}

/* 맥박 하나를 노드·간선에 적는 유일한 손. 쓰는 것은 `data-live-beat` 하나뿐이라
 * 간선의 `class`를 쓰는 손(`paintAgentGraphEdges`의 옷)과 부딪히지 않는다.
 * 배치를 읽지 않고, 값이 같으면 쓰지 않는다 — 조용한 판에서 이 손은 DOM을
 * 한 번도 건드리지 않는다. */
function dressAgentGraphLive(view) {
  const running = agentGraphLiveAnimating();
  for (const node of view.querySelectorAll(".agent-graph-node[data-graph-key]")) {
    writeLiveBeat(node, running ? livePulses.get(node.dataset.graphKey)?.beat ?? "" : "");
  }
  for (const group of view.querySelectorAll(".agent-graph-edges [data-graph-edge]")) {
    const line = group.querySelector("path");
    if (!line) continue;
    writeLiveBeat(line, running ? livePulses.get(group.getAttribute("data-graph-edge"))?.beat ?? "" : "");
  }
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

/* 노드 하나가 그릴 대기 사실. 모델이 부르고, 없으면 `null`이라 배지가 서지
 * 않는다 — 「모름」을 「없음」으로 읽지 않도록, 사유를 아는 카드만 배지를 든다.
 *
 * 주체가 없는 카드(`run/worker/dispatch` 중 하나라도 없는)는 `unverified`다:
 * 원장이 이 자리를 누구의 것이라고 말하지 않았으므로, 사유도 말하지 않는다. */
function agentGraphLiveWait(entry, now = Date.now()) {
  if (!agentGraphLive) return null;
  if (agentGraphLiveSubject(entry.place) === null) {
    return { cause: "unverified", key: "board.live.unverified", word: "확인되지 않음",
      state: "idle", since: 0, until: null, ageWord: "" };
  }
  const wait = agentGraphLiveWaitOf(agentGraphLedgerRow(entry.card.pane), entry.card, now);
  if (!wait) return null;
  return { ...wait, ageWord: wait.since > 0 ? agoWord(wait.since, now) : "" };
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

/* 그 칸을 채우는 유일한 손. 낱말은 코디네이터 데스크가 이미 쓰는 그 표의
 * 것이고(`DESK_HEALTH`의 `key`/`word`), 나이는 **기록된 stamp**에서만 나온다 —
 * stamp가 없는 사유는 나이를 말하지 않는다. */
function dressAgentGraphWait(chip, wait, now = Date.now()) {
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
  const word = t(wait.key, wait.word);
  const stands = wait.until !== null && Number.isFinite(wait.until)
    ? usageCountdown(wait.until - now)
    : "";
  const said = stands
    || (wait.ageWord ? t("board.desk.mailAge", "{{time}} 전", { time: wait.ageWord }) : "");
  writeClassName(chip, `agent-graph-wait is-${wait.cause} is-${wait.state}`);
  writeTextContent(cause, word);
  writeTextContent(age, said);
  /* 나이를 모르는 기다림은 **모른다고 말한다**. 곁의 다른 시각으로 메우면
   * 그것이 곧 추측이다. */
  const tip = said
    ? `${word} · ${said}`
    : `${word} · ${t("board.live.sinceUnknown", "기록된 시작 시각 없음")}`;
  writeAttribute(chip, "data-tip", tip);
  writeAttribute(chip, "aria-label", tip);
}

/* ---- 권위 -------------------------------------------------------------------- */

/* 「어디까지 왔는가」를 **원장이 적은 만큼만**. 판정은 t-6815의 한 손
 * (`ledgerReviewWord`)이 하고, 여기서는 그 손이 답한 낱말을 단계 이름으로
 * 되읽을 뿐이다 — 워커의 주장(`reported`)이 검증·병합·배포 배지가 되는 길은
 * 이 파일에 없다. */
function ledgerReviewStage(place, row) {
  const review = row?.review ?? {};
  if (review.deployed) return "deployed";
  if (review.merged) return "merged";
  if (review.verified) return "verified";
  if (place?.reported === true || row?.reported === true) return "reported";
  return "dispatched";
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
const AGENT_GRAPH_LIVE_UNKNOWN = Object.freeze([
  { id: "delivery", key: "board.live.deliveryUnknown",
    word: "대기·전달·확인·답장을 나눌 기록은 이 스냅샷에 없습니다. 미확인 수 외의 배달 상태는 원장에서 확인합니다." },
  { id: "reply", key: "board.live.replyUnknown",
    word: "답장 연결은 이 스냅샷에 없습니다." },
  { id: "summoner", key: "board.live.summonerUnknown",
    word: "판을 들고 있지 않은 워커의 소환자는 이 스냅샷에 없습니다." },
  { id: "outside", key: "board.live.outsideUnknown",
    word: "이 보기 밖 끝점의 수는 스냅샷에 없습니다." },
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
  if (stage === "deployed") return t("board.deployed", "배포됨");
  if (stage === "merged") return t("board.merged", "병합됨");
  if (stage === "verified") return t("board.verified", "검증됨");
  if (stage === "reported") return t("board.awaitingReview", "검증 대기");
  return t("board.desk.stageDispatched", "진행");
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
    /* 사유의 낱말은 코디네이터 데스크가 쓰는 그 표에서 온다 — 한 화면이 같은
     * 기다림을 두 낱말로 부르지 않도록. 줄이 사유를 말하지 않으면 「대기」라는
     * 종류만 남고, 그것은 이 줄이 답해야 할 질문에 답하지 않는다. */
    const health = DESK_HEALTH.find((one) => one.id === source.cause);
    return [source.run, source.workerId, health ? t(health.key, health.word) : source.cause]
      .filter(Boolean).join(" · ");
  }
  return [source.run, source.taskId, source.dispatchId,
    agentGraphLiveStageWord(source.stage)].filter(Boolean).join(" · ");
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
  if (!coverage.complete) {
    block.append(taskBoardElement("p", "agent-live-events-note",
      t("board.live.coverageTruncated",
        "중복 제거 범위가 끊긴 관계 {{count}}개 — 그 구간의 사건은 표시되지 않을 수 있습니다.",
        { count: coverage.truncated })));
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
    button.append(
      taskBoardElement("strong", "agent-live-event-kind", agentGraphLiveKindWord(event.kind)),
      taskBoardElement("span", "agent-live-event-facts", agentGraphLiveEventFacts(event)),
      taskBoardElement("span", "agent-live-event-when",
        event.at > 0 ? t("board.desk.mailAge", "{{time}} 전", { time: agoWord(event.at, now) })
          : t("board.live.sinceUnknown", "기록된 시작 시각 없음")),
    );
    if (Number.isFinite(event.added) && event.added > 1) {
      button.append(taskBoardElement("span", "agent-live-event-added",
        t("board.live.added", "이번 판에 {{count}}통 늘었습니다", { count: event.added })));
    }
    button.onclick = () => {
      if (event.edgeKey) selectAgentGraphRelation(view, event.edgeKey);
      else if (event.to) selectAgentGraphEntity(view, event.to, { focus: true });
    };
    row.append(button);
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
  agentGraphLiveStop();
}

function agentGraphLiveGeneration() {
  return liveGeneration;
}

/* 부서진 판이 다시 서거나 창이 숨었다 돌아오면 — backlog는 사건이 아니다. */
document.addEventListener("visibilitychange", () => {
  if (document.hidden) agentGraphLiveStop();
  else liveBaselineDue = true;
});
