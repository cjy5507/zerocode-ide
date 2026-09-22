/* ---- 지식 그래프의 공급망 층 (P4·P5) ---------------------------------------
 *
 * 사람의 말(2026-09-17): 「네모 세모 그리고 취약점까지 보여줘 … 의미를 넣어서 SCA와 SBOM 요소도」.
 * 설계는 docs/design/knowledge-supply-chain-20260917.md이고, 이 파일이 읽는 것은 그 §5.3의 답
 * (`supply_chain_graph`) 하나다 — 잠금 파일을 읽고 OSV에 묻는 일은 백엔드의 것이고, 창은 그 답을
 * 볼트의 그림에 **덧붙이는** 렌즈다.
 *
 * 이 파일이 지는 것 넷:
 *   1. 렌즈의 살림 — 켬(기본 꺼짐), 묻기(활성 워크스페이스, 바닥 시간, 세대), 조회 상태의 한 줄.
 *   2. 결합 — 답의 구성요소·취약점을 볼트의 점 뒤에 붙이고, 볼트의 유령 링크 중 이름이 구성요소의
 *      이름과 **글자 그대로** 같은 것을 그 구성요소로 잇는다(추측 매칭은 없다).
 *   3. 접기(G7) — 서는 것은 멤버·직접 의존·취약점이 닿는 경로(와 볼트가 이름으로 가리키는 구성요소)
 *      뿐이고, 나머지는 그것에 먼저 닿는 멤버의 「+N」이다. 펼치기는 멤버마다의 명시적인 문이다.
 *   4. 카드 — 개요의 공급망 절과, 고른 구성요소·취약점의 사실들.
 *
 * 그리는 일은 여기 없다: 모양은 `KNOWLEDGE_NODE_SHAPES`, 옷은 점이 든 낱말(`knowledgeSupplyDress`)을
 * CSS가 읽고, GL 손은 같은 옷을 입힌 견본에서 색을 읽는다. */

/* 점의 열쇠 — 볼트의 id(`wiki/…md`·`ghost:…`·원본 경로)와 섞이지 않게 층의 이름을 앞세운다. 뒤는
 * 답의 id 그대로다(구성요소는 purl 또는 `<purl> (<source>)`, 취약점은 묶음의 이름). */
const KNOWLEDGE_SUPPLY_KEYS = Object.freeze({ component: "sbom:", vulnerability: "osv:" });

/* 생태계 — 답의 낱말(`ecosystem`)과 사람이 아는 레지스트리의 이름. 자리가 곧 GL 견본의 줄이다. */
const KNOWLEDGE_SUPPLY_ECOSYSTEMS = Object.freeze([
  Object.freeze({ id: "cargo", registry: "crates.io" }),
  Object.freeze({ id: "npm", registry: "npm" }),
]);

/* 심각도 — 답의 여섯 낱말, 답의 순서(높은 순). 자리가 곧 GL 견본의 줄이고, 잉크는
 * `--knowledge-severity-<id>`(tokens.css 한 곳)다. */
const KNOWLEDGE_SUPPLY_SEVERITIES = Object.freeze([
  Object.freeze({ id: "critical", key: "knowledge.supplySeverityCritical", word: "심각" }),
  Object.freeze({ id: "high", key: "knowledge.supplySeverityHigh", word: "높음" }),
  Object.freeze({ id: "medium", key: "knowledge.supplySeverityMedium", word: "보통" }),
  Object.freeze({ id: "low", key: "knowledge.supplySeverityLow", word: "낮음" }),
  Object.freeze({ id: "none", key: "knowledge.supplySeverityNone", word: "위협 없음" }),
  Object.freeze({ id: "unknown", key: "knowledge.supplySeverityUnknown", word: "등급 없음" }),
]);

/* 출처(`origin`)의 사람 말. */
const KNOWLEDGE_SUPPLY_ORIGINS = Object.freeze({
  registry: Object.freeze({ key: "knowledge.supplyOriginRegistry", word: "공개 레지스트리" }),
  git: Object.freeze({ key: "knowledge.supplyOriginGit", word: "git 저장소" }),
  path: Object.freeze({ key: "knowledge.supplyOriginPath", word: "로컬 경로" }),
  private: Object.freeze({ key: "knowledge.supplyOriginPrivate", word: "사설 레지스트리 또는 출처 없음" }),
});

/* RustSec의 정보성 권고(`informational`) — 취약점과 다른 옷을 입는 근거(§5.3). */
const KNOWLEDGE_SUPPLY_INFORMATIONAL = Object.freeze({
  unmaintained: Object.freeze({ key: "knowledge.supplyUnmaintained", word: "유지 중단" }),
  unsound: Object.freeze({ key: "knowledge.supplyUnsound", word: "불건전" }),
  notice: Object.freeze({ key: "knowledge.supplyNotice", word: "공지" }),
});

/* 조회 상태의 한 줄 — 답의 낱말(`lookup.state`·`lookup.reason`)이 줄을 고르고, 창은 이유를 짐작하지
 * 않는다. `reason`의 `http_<status>`는 수를 든 한 줄이고, 표에 없는 낱말은 그 낱말 그대로 선다.
 * `asking`은 답의 낱말이 아니라 창이 묻고 있는 동안이다. */
const KNOWLEDGE_SUPPLY_LOOKUP = Object.freeze({
  asking: Object.freeze({ key: "knowledge.supplyAsking", word: "공급망을 확인하는 중…" }),
  fresh: Object.freeze({ key: "knowledge.supplyFresh", word: "OSV 확인 {{when}}" }),
  freshQuiet: Object.freeze({ key: "knowledge.supplyNothingAsked", word: "OSV에 물을 공개 패키지가 없습니다" }),
  cached: Object.freeze({ key: "knowledge.supplyCached", word: "저장된 OSV 답 {{when}} · 이번 요청 없음" }),
  offline: Object.freeze({ key: "knowledge.supplyOffline", word: "OSV에 닿지 못했습니다(오프라인)" }),
  timeout: Object.freeze({ key: "knowledge.supplyTimeout", word: "OSV 조회 시간이 다 됐습니다" }),
  http: Object.freeze({ key: "knowledge.supplyHttp", word: "OSV가 HTTP {{status}}로 답했습니다" }),
  bad_answer: Object.freeze({ key: "knowledge.supplyBadAnswer", word: "OSV의 답을 읽지 못했습니다" }),
  held: Object.freeze({ key: "knowledge.supplyHeld", word: "지난 확인 {{when}}의 취약점" }),
  bare: Object.freeze({ key: "knowledge.supplyBare", word: "취약점 없이 구성요소만" }),
});

/* 렌즈의 살림. 켬은 사람의 것이고(시야가 실어 나른다), 답은 워크스페이스 하나의 것이다. */
let knowledgeSupplyShown = false;
let knowledgeSupplyAnswer = null;
let knowledgeSupplyAsking = false;
/* 문이 거절한 문장(`Err(String)`) — 그대로 줄에 선다. */
let knowledgeSupplyError = null;
/* 물은 워크스페이스(`null`이면 백엔드의 활성 워크스페이스)와 그 시각, 그리고 늦게 온 답을 버리는 세대. */
let knowledgeSupplyAskedRoot;
let knowledgeSupplyAskedAt = 0;
let knowledgeSupplyGeneration = 0;
/* 펼친 멤버의 열쇠 — 같은 워크스페이스의 답 안에서만 뜻이 있다. */
const knowledgeSupplyUnfolded = new Set();
let knowledgeSupplyUnfoldGeneration = 0;
/* 결합의 답 하나 — 답·볼트·펼침이 그대로면 다시 짓지 않는다(검색 한 글자마다 지나는 자리다). */
let knowledgeSupplyJoined = null;

function knowledgeSupplyKind(kind) {
  return kind === "component" || kind === "vulnerability";
}

/* 구성요소의 이름 — 판이 있으면 `이름@판`. 그림의 제목과 카드의 줄이 같은 한 문을 지난다. */
function knowledgeSupplyComponentTitle(row) {
  return row.version ? `${row.name}@${row.version}` : row.name;
}

/* 생태계의 사람 말 — 표에 없는 낱말(새 생태계)은 답의 낱말 그대로. */
function knowledgeSupplyRegistry(ecosystem) {
  return KNOWLEDGE_SUPPLY_ECOSYSTEMS.find((one) => one.id === ecosystem)?.registry ?? ecosystem;
}

/* 렌즈를 켜고 끈다. 켜는 몸짓은 묻는 청이고(바닥 시간을 넘는다), 끄는 몸짓은 그림만 되돌린다 —
 * 답은 들고 있어 다시 켜면 같은 워크스페이스의 그림이 곧장 선다. */
function toggleKnowledgeSupply() {
  knowledgeSupplyShown = !knowledgeSupplyShown;
  if (knowledgeSupplyShown) void refreshKnowledgeSupply({ force: true });
  else void paintKnowledgeView();
}

/* 공급망을 묻는다 — 렌즈가 켜져 있을 때만. 워크스페이스는 창이 보고 있는 체크아웃이고(없으면
 * 백엔드의 활성 워크스페이스), 다른 워크스페이스의 답과 펼침은 놓는다. `refresh`는 카드의
 * 「다시 확인」이다(24 h 캐시를 넘어 OSV에 다시). */
async function refreshKnowledgeSupply({ force = false, refresh = false } = {}) {
  if (!knowledgeSupplyShown) return;
  const root = activeWorktreePath ?? null;
  const moved = root !== knowledgeSupplyAskedRoot;
  const now = Date.now();
  if (!force && !refresh && !moved && now - knowledgeSupplyAskedAt < KNOWLEDGE_REFRESH_FLOOR_MS) return;
  if (moved) {
    knowledgeSupplyAnswer = null;
    knowledgeSupplyUnfolded.clear();
    knowledgeSupplyUnfoldGeneration += 1;
  }
  knowledgeSupplyAskedRoot = root;
  knowledgeSupplyAskedAt = now;
  const generation = ++knowledgeSupplyGeneration;
  knowledgeSupplyAsking = true;
  knowledgeSupplyError = null;
  await paintKnowledgeView();
  try {
    const answer = await invoke("supply_chain_graph", root === null ? { refresh } : { root, refresh });
    if (generation !== knowledgeSupplyGeneration) return;
    knowledgeSupplyAnswer = answer;
  } catch (error) {
    if (generation !== knowledgeSupplyGeneration) return;
    /* 거절은 문장 그대로 — 서 있던 구성요소(같은 워크스페이스의 지난 답)는 그대로 둔다. */
    knowledgeSupplyError = String(error);
  }
  knowledgeSupplyAsking = false;
  await paintKnowledgeView();
}

/* 멤버 하나를 펼치거나 다시 접는다 — 같은 문이 두 몸짓이다. */
function toggleKnowledgeSupplyUnfold(key) {
  if (knowledgeSupplyUnfolded.has(key)) knowledgeSupplyUnfolded.delete(key);
  else knowledgeSupplyUnfolded.add(key);
  knowledgeSupplyUnfoldGeneration += 1;
  void paintKnowledgeView();
}

/* 렌즈가 켜진 답의 결합 — 없으면 `null`(볼트만의 그림). */
function knowledgeSupplyView(graph) {
  const answer = knowledgeSupplyShown ? knowledgeSupplyAnswer : null;
  if (answer === null) return null;
  const held = knowledgeSupplyJoined;
  if (held !== null && held.answer === answer && held.graph === graph
      && held.unfoldGeneration === knowledgeSupplyUnfoldGeneration) return held;
  knowledgeSupplyJoined = knowledgeSupplyJoin(answer, graph);
  return knowledgeSupplyJoined;
}

/* 답과 볼트를 한 목록으로.
 *
 * 걸음은 한 번이다: 멤버를 답의 순서로 줄 세운 너비 우선(선은 답의 순서 — kind → from → to)이 구성요소
 * 마다 임자(`root`, 먼저 닿은 멤버)·어버이(`parent`)·걸음 수(`hops`)를 적는다. 그 셋이 서는 집합과 접힌
 * 수와 카드의 경로를 함께 답한다 — 경로는 멤버에서 그 구성요소까지의 한 가장 짧은 사슬이고, 카드가
 * 말하는 사슬과 그림에 선 사슬이 같은 걸음의 것이다.
 *
 * 돌려주는 것: 볼트의 점 뒤에 붙인 `nodes`(볼트 점 객체는 그대로, 붙인 점은 볼트 점과 같은 꼴), 이은
 * `edges`, 유령 중 구성요소로 이어진 자리(`skip`), 붙인 점마다 답의 자리(`rows`)와 옷의 낱말, 그리고
 * 구성요소마다의 사실(임자·어버이·접힘·의존 수·영향). */
function knowledgeSupplyJoin(answer, graph) {
  const components = Array.isArray(answer.components) ? answer.components : [];
  const vulnerabilities = Array.isArray(answer.vulnerabilities) ? answer.vulnerabilities : [];
  const count = components.length;
  const vaultNodes = graph?.nodes ?? [];
  const vaultEdges = graph?.edges ?? [];
  const inRange = (at, size) => Number.isInteger(at) && at >= 0 && at < size;

  /* 의존의 CSR(답의 순서 그대로)과 영향의 두 방향. */
  const outStart = new Int32Array(count + 1);
  const inCount = new Int32Array(count);
  const affectsOf = Array.from({ length: count }, () => []);
  const affectedBy = Array.from({ length: vulnerabilities.length }, () => []);
  for (const edge of answer.edges ?? []) {
    if (edge.kind === "depends_on" && inRange(edge.from, count) && inRange(edge.to, count)) {
      outStart[edge.from + 1] += 1;
      inCount[edge.to] += 1;
    } else if (edge.kind === "affects" && inRange(edge.from, vulnerabilities.length) && inRange(edge.to, count)) {
      affectsOf[edge.to].push(edge.from);
      affectedBy[edge.from].push(edge.to);
    }
  }
  for (let at = 0; at < count; at += 1) outStart[at + 1] += outStart[at];
  const outCursor = Int32Array.from(outStart.subarray(0, count));
  const outTo = new Int32Array(outStart[count]);
  for (const edge of answer.edges ?? []) {
    if (edge.kind !== "depends_on" || !inRange(edge.from, count) || !inRange(edge.to, count)) continue;
    outTo[outCursor[edge.from]] = edge.to;
    outCursor[edge.from] += 1;
  }

  const root = new Int32Array(count).fill(-1);
  const parent = new Int32Array(count).fill(-1);
  const hops = new Int32Array(count).fill(-1);
  const queue = new Int32Array(count);
  let tail = 0;
  for (let at = 0; at < count; at += 1) {
    if (components[at].member !== true) continue;
    root[at] = at;
    hops[at] = 0;
    queue[tail] = at;
    tail += 1;
  }
  for (let head = 0; head < tail; head += 1) {
    const at = queue[head];
    for (let edge = outStart[at]; edge < outStart[at + 1]; edge += 1) {
      const next = outTo[edge];
      if (hops[next] >= 0) continue;
      hops[next] = hops[at] + 1;
      root[next] = root[at];
      parent[next] = at;
      queue[tail] = next;
      tail += 1;
    }
  }

  /* 볼트의 유령 중 이름이 글자 그대로 구성요소의 이름인 것. 같은 이름의 판이 여럿이면 그 전부가
   * 그 이름이다 — 판을 고르는 것이 추측이다. */
  const byName = new Map();
  components.forEach((row, at) => {
    const held = byName.get(row.name);
    if (held === undefined) byName.set(row.name, [at]);
    else held.push(at);
  });
  const redirect = new Map();
  const skip = new Uint8Array(vaultNodes.length);
  vaultNodes.forEach((node, at) => {
    if (node.kind !== "ghost") return;
    const named = byName.get(node.title);
    if (named === undefined) return;
    redirect.set(at, named);
    skip[at] = 1;
  });

  /* 서는 집합. 펼치기 전의 집합(`always`)과 펼친 멤버의 몫을 따로 세어, 접힌 수가 펼침과 무관한
   * 「이 멤버 아래의 몫」(`rooted`)을 잃지 않게 한다. */
  const always = new Uint8Array(count);
  for (let at = 0; at < count; at += 1) {
    if (components[at].member === true || hops[at] === 1) always[at] = 1;
  }
  for (let at = 0; at < count; at += 1) {
    if (affectsOf[at].length === 0) continue;
    always[at] = 1;
    for (let step = parent[at]; step >= 0; step = parent[step]) always[step] = 1;
  }
  for (const named of redirect.values()) for (const at of named) always[at] = 1;
  const idOf = new Map(components.map((row, at) => [row.id, at]));
  const unfolded = new Uint8Array(count);
  for (const key of knowledgeSupplyUnfolded) {
    const at = idOf.get(key.slice(KNOWLEDGE_SUPPLY_KEYS.component.length));
    if (at !== undefined && key.startsWith(KNOWLEDGE_SUPPLY_KEYS.component)) unfolded[at] = 1;
  }
  const visible = Uint8Array.from(always);
  const rooted = new Int32Array(count);
  for (let at = 0; at < count; at += 1) {
    /* 어느 멤버에서도 닿지 않는 구성요소(잠금 파일의 외톨이)는 임자가 없어 어느 「+N」에도 들지
     * 않는다 — 머리의 「구성요소 서는 수/전부」가 그것까지 센다. */
    if (root[at] < 0 || always[at] === 1) continue;
    rooted[root[at]] += 1;
    if (unfolded[root[at]] === 1) visible[at] = 1;
  }
  const folded = new Int32Array(count);
  for (let at = 0; at < count; at += 1) folded[at] = unfolded[at] === 1 ? 0 : rooted[at];

  /* 붙일 점 — 서는 구성요소(답의 순서), 그리고 취약점 전부(답의 순서). */
  const base = vaultNodes.length;
  const seatOf = new Int32Array(count).fill(-1);
  const appended = [];
  const rows = [];
  for (let at = 0; at < count; at += 1) {
    if (visible[at] === 0) continue;
    const row = components[at];
    seatOf[at] = appended.length;
    rows.push(at);
    appended.push({
      id: `${KNOWLEDGE_SUPPLY_KEYS.component}${row.id}`,
      title: knowledgeSupplyComponentTitle(row),
      tags: [],
      kind: "component",
      modified_ms: 0,
      out_links: outStart[at + 1] - outStart[at],
      in_links: inCount[at] + affectsOf[at].length,
      source: null,
      excerpt: "",
      folder: "",
    });
  }
  const vulnerabilityFrom = appended.length;
  vulnerabilities.forEach((row, at) => {
    rows.push(at);
    appended.push({
      id: `${KNOWLEDGE_SUPPLY_KEYS.vulnerability}${row.id}`,
      title: row.id,
      tags: [],
      kind: "vulnerability",
      modified_ms: 0,
      out_links: affectedBy[at].length,
      in_links: 0,
      source: null,
      excerpt: "",
      folder: "",
    });
  });

  const edges = [];
  const named = (at, held) => (held === undefined ? [at] : held.map((one) => base + seatOf[one]));
  for (const edge of vaultEdges) {
    const from = redirect.get(edge.from);
    const to = redirect.get(edge.to);
    if (from === undefined && to === undefined) {
      edges.push(edge);
      continue;
    }
    /* 이름이 같은 판이 여럿이면 그 선은 그 전부로 간다 — 관계는 그대로. */
    for (const left of named(edge.from, from)) {
      for (const right of named(edge.to, to)) {
        edges.push({ from: left, to: right, kind: edge.kind, provenance: edge.provenance });
      }
    }
  }
  /* 답의 선은 근거(t-5966)도 답의 것 그대로 — 잠금 파일과 OSV는 기계가 잰 것이고, 그 낱말은
   * 백엔드가 적어 보낸다. */
  for (const edge of answer.edges ?? []) {
    if (edge.kind === "depends_on") {
      if (!inRange(edge.from, count) || !inRange(edge.to, count)) continue;
      if (seatOf[edge.from] < 0 || seatOf[edge.to] < 0) continue;
      edges.push({ from: base + seatOf[edge.from], to: base + seatOf[edge.to], kind: edge.kind,
        provenance: edge.provenance });
    } else if (edge.kind === "affects") {
      if (!inRange(edge.from, vulnerabilities.length) || !inRange(edge.to, count) || seatOf[edge.to] < 0) continue;
      edges.push({ from: base + vulnerabilityFrom + edge.from, to: base + seatOf[edge.to], kind: edge.kind,
        provenance: edge.provenance });
    }
  }

  return {
    answer,
    graph,
    unfoldGeneration: knowledgeSupplyUnfoldGeneration,
    base,
    nodes: appended.length === 0 && redirect.size === 0 ? vaultNodes : vaultNodes.concat(appended),
    edges,
    skip: redirect.size === 0 ? null : skip,
    rows: Int32Array.from(rows),
    vulnerabilityFrom,
    components: count,
    drawnComponents: vulnerabilityFrom,
    root,
    parent,
    folded,
    rooted,
    unfolded,
    affectsOf,
    affectedBy,
    dependencies: (at) => outStart[at + 1] - outStart[at],
    dependents: (at) => inCount[at],
  };
}

/* 모델의 자리마다 공급망의 낱말 — 답의 자리, 생태계·멤버·심각도·정보성의 번호, 접힌 수. 볼트만의
 * 그림에서는 전부 `null`이고 읽는 쪽이 그것을 묻는다(렌즈를 끈 판이 배열 다섯을 짓지 않게). */
function knowledgeSupplySeats(model, joined, origin) {
  if (joined === null) {
    model.supply = null;
    model.supplyRow = null;
    model.ecosystem = null;
    model.member = null;
    model.severity = null;
    model.informational = null;
    model.supplyFolded = null;
    model.supplyFoldStamp = "";
    return;
  }
  const count = model.count;
  const supplyRow = new Int32Array(count).fill(-1);
  const ecosystem = new Int8Array(count).fill(-1);
  const member = new Uint8Array(count);
  const severity = new Int8Array(count).fill(-1);
  const informational = new Uint8Array(count);
  const folded = new Int32Array(count);
  const stamp = [];
  const { answer, base } = joined;
  for (let seat = 0; seat < count; seat += 1) {
    const appended = origin[seat] - base;
    if (appended < 0) continue;
    const row = joined.rows[appended];
    supplyRow[seat] = row;
    if (appended < joined.vulnerabilityFrom) {
      const component = answer.components[row];
      ecosystem[seat] = KNOWLEDGE_SUPPLY_ECOSYSTEMS.findIndex((one) => one.id === component.ecosystem);
      member[seat] = component.member === true ? 1 : 0;
      folded[seat] = joined.folded[row];
      if (folded[seat] > 0) stamp.push(`${seat}:${folded[seat]}`);
    } else {
      const vulnerability = answer.vulnerabilities[row];
      severity[seat] = KNOWLEDGE_SUPPLY_SEVERITIES.findIndex((one) => one.id === vulnerability.severity);
      informational[seat] = vulnerability.informational ? 1 : 0;
    }
  }
  model.supply = joined;
  model.supplyRow = supplyRow;
  model.ecosystem = ecosystem;
  model.member = member;
  model.severity = severity;
  model.informational = informational;
  model.supplyFolded = stamp.length === 0 ? null : folded;
  model.supplyFoldStamp = stamp.join(",");
}

/* 점의 옷의 낱말 — CSS와 GL 견본이 같은 낱말을 읽는다. 열쇠가 층의 이름을 앞세우므로 한 열쇠의 점은
 * 언제나 같은 종류이고, 그래서 지울 낱말은 그 종류 안의 것뿐이다. */
function knowledgeSupplyDress(node, model, at) {
  if (model.kinds[at] === "component") {
    const ecosystem = KNOWLEDGE_SUPPLY_ECOSYSTEMS[model.ecosystem?.[at] ?? -1]?.id;
    if (ecosystem === undefined) delete node.dataset.ecosystem;
    else if (node.dataset.ecosystem !== ecosystem) node.dataset.ecosystem = ecosystem;
    const member = String(model.member?.[at] === 1);
    if (node.dataset.member !== member) node.dataset.member = member;
    return;
  }
  const severity = KNOWLEDGE_SUPPLY_SEVERITIES[model.severity?.[at] ?? -1]?.id;
  if (severity === undefined) delete node.dataset.severity;
  else if (node.dataset.severity !== severity) node.dataset.severity = severity;
  const row = model.supplyRow?.[at] ?? -1;
  const informational = row < 0 ? null : model.supply.answer.vulnerabilities[row]?.informational ?? null;
  if (informational === null) delete node.dataset.informational;
  else if (node.dataset.informational !== informational) node.dataset.informational = informational;
}

/* 심각도 한 칸의 사람 말. */
function knowledgeSupplySeverityWord(id) {
  const row = KNOWLEDGE_SUPPLY_SEVERITIES.find((one) => one.id === id);
  return row ? t(row.key, row.word) : id;
}

/* 확인 시각 — 날짜(개요의 다른 날짜와 같은 손)와 시:분. */
function knowledgeSupplyWhen(ms) {
  if (!ms) return "—";
  return `${knowledgeWhen(ms)} ${new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

/* 조회 상태의 한 줄 — `{ state, reason, words }`. 줄의 낱말은 답의 낱말에서만 나온다. */
function knowledgeSupplyLookupLine() {
  if (knowledgeSupplyAsking) {
    const row = KNOWLEDGE_SUPPLY_LOOKUP.asking;
    return { state: "asking", reason: "", words: () => t(row.key, row.word) };
  }
  if (knowledgeSupplyError !== null) {
    const said = knowledgeSupplyError;
    return { state: "error", reason: "", words: () => said };
  }
  const lookup = knowledgeSupplyAnswer?.lookup ?? null;
  if (lookup === null) return { state: "", reason: "", words: () => "" };
  const when = knowledgeSupplyWhen(lookup.checkedAt);
  if (lookup.state === "fresh" && lookup.checkedAt == null) {
    const row = KNOWLEDGE_SUPPLY_LOOKUP.freshQuiet;
    return { state: lookup.state, reason: "", words: () => t(row.key, row.word) };
  }
  if (lookup.state === "fresh" || lookup.state === "cached") {
    const row = KNOWLEDGE_SUPPLY_LOOKUP[lookup.state];
    return { state: lookup.state, reason: "", words: () => t(row.key, row.word, { when }) };
  }
  const reason = String(lookup.reason ?? "");
  const status = /^http_(\d+)$/u.exec(reason)?.[1] ?? null;
  const row = status !== null ? KNOWLEDGE_SUPPLY_LOOKUP.http : KNOWLEDGE_SUPPLY_LOOKUP[reason] ?? null;
  const tail = lookup.checkedAt == null ? KNOWLEDGE_SUPPLY_LOOKUP.bare : KNOWLEDGE_SUPPLY_LOOKUP.held;
  return {
    state: String(lookup.state ?? ""),
    reason,
    words: () => `${row === null ? reason : t(row.key, row.word, { status })} · ${t(tail.key, tail.word, { when })}`,
  };
}

/* 판의 머리에 한 줄 — 서 있는 구성요소와 답의 구성요소, 취약점. */
function knowledgeSupplyStatWord(model) {
  const joined = model.supply;
  if (joined === null) return "";
  let shown = 0;
  let vulnerabilities = 0;
  for (let at = 0; at < model.count; at += 1) {
    if (model.kinds[at] === "component") shown += 1;
    else if (model.kinds[at] === "vulnerability") vulnerabilities += 1;
  }
  return ` · ${t("knowledge.statSupply", "구성요소 {{shown}}/{{total}} · 취약점 {{vulnerabilities}}",
    { shown, total: joined.components, vulnerabilities })}`;
}

/* 검색 후보의 힌트 — 구성요소는 레지스트리, 취약점은 심각도. */
function knowledgeSupplyNote(model, at) {
  const row = model.supplyRow?.[at] ?? -1;
  if (row < 0) return "";
  if (model.kinds[at] === "component") return KNOWLEDGE_SUPPLY_ECOSYSTEMS[model.ecosystem[at]]?.registry ?? "";
  return knowledgeSupplySeverityWord(model.supply.answer.vulnerabilities[row].severity);
}

/* 범례·줄의 작은 표본 — 그림의 점과 같은 기하(`knowledgeLegendShape`)에 같은 낱말. */
function knowledgeSupplyMark(kind, words) {
  const mark = knowledgeLegendShape(kind, knowledgeShapeOf(kind));
  for (const [name, value] of Object.entries(words)) mark.dataset[name] = value;
  return mark;
}

/* ---- 개요의 공급망 절 ---- */

function buildKnowledgeSupplyOverview() {
  const box = knowledgeListSection("knowledge-overview-supply", "knowledge.supply", t("knowledge.supply", "공급망"));
  box.hidden = true;
  const head = box.querySelector(".knowledge-inspector-head");
  const severities = box.querySelector(".knowledge-inspector-list");
  severities.classList.add("knowledge-supply-severities");
  const lookup = document.createElement("p");
  lookup.className = "knowledge-supply-lookup";
  const counts = document.createElement("p");
  counts.className = "knowledge-supply-counts";
  const lockfiles = document.createElement("ul");
  lockfiles.className = "knowledge-inspector-list knowledge-supply-lockfiles";
  const recheck = document.createElement("button");
  recheck.type = "button";
  recheck.className = "btn knowledge-supply-recheck";
  recheck.dataset.i18n = "knowledge.supplyRecheck";
  recheck.textContent = t("knowledge.supplyRecheck", "다시 확인");
  /* 그림은 「무엇이 잘못됐나」를 삼각형 하나씩 답한다. 보고서는 나머지 반쪽 —
   * 권고마다 멤버가 올릴 직접 의존, 그 의존마다 올리면 닫히는 것들 — 을 파일
   * 하나로 내놓는다. 판정은 코어의 순수 함수라 문서와 그림이 어긋날 수 없다. */
  const report = document.createElement("button");
  report.type = "button";
  report.className = "btn knowledge-supply-report";
  report.dataset.i18n = "knowledge.supplyReport";
  report.textContent = t("knowledge.supplyReport", "보고서");
  const wrote = document.createElement("p");
  wrote.className = "settings-hint knowledge-supply-wrote";
  wrote.hidden = true;
  head.after(lookup, counts);
  box.append(lockfiles, recheck, report, wrote);
  return box;
}

function paintKnowledgeSupplyOverview(layout, box) {
  const section = box.querySelector(".knowledge-overview-supply");
  if (!section) return;
  section.hidden = !knowledgeSupplyShown;
  if (section.hidden) return;
  const model = layout.model;
  const line = knowledgeSupplyLookupLine();
  const lookup = section.querySelector(".knowledge-supply-lookup");
  writeAttribute(lookup, "data-lookup-state", line.state);
  writeAttribute(lookup, "data-lookup-reason", line.reason);
  say(lookup, line.words);
  section.querySelector(".knowledge-supply-recheck").disabled = knowledgeSupplyAsking;
  const reportButton = section.querySelector(".knowledge-supply-report");
  if (reportButton) reportButton.disabled = knowledgeSupplyAsking || knowledgeSupplyAnswer === null;
  const answer = knowledgeSupplyAnswer;
  const counts = section.querySelector(".knowledge-supply-counts");
  counts.hidden = answer === null;
  if (answer === null) {
    reconcileElementOrder(section.querySelector(".knowledge-supply-severities"), []);
    reconcileElementOrder(section.querySelector(".knowledge-supply-lockfiles"), []);
    return;
  }
  const joined = model.supply;
  const shown = joined === null ? 0 : joined.drawnComponents;
  say(counts, () => t("knowledge.supplyCounts", "구성요소 {{shown}}/{{total}} · 취약점 {{vulnerabilities}} · 잠금 파일 {{lockfiles}}", {
    shown, total: answer.components.length, vulnerabilities: answer.vulnerabilities.length, lockfiles: answer.lockfiles.length,
  }));
  const query = knowledgeQuery.trim().toLocaleLowerCase();
  const stamp = `${query}|${locale}`;
  if (section.dataset.knowledgeStamp === stamp && layout.supplyOverviewAnswer === answer) return;
  section.dataset.knowledgeStamp = stamp;
  layout.supplyOverviewAnswer = answer;
  const tally = new Map();
  for (const row of answer.vulnerabilities) tally.set(row.severity, (tally.get(row.severity) ?? 0) + 1);
  const top = Math.max(1, ...tally.values());
  reconcileElementOrder(section.querySelector(".knowledge-supply-severities"), KNOWLEDGE_SUPPLY_SEVERITIES
    .filter((row) => (tally.get(row.id) ?? 0) > 0)
    .map((row) => {
      const count = tally.get(row.id);
      const held = knowledgeListRow("", t(row.key, row.word), String(count), count / top);
      const press = held.querySelector("button");
      delete press.dataset.knowledgeKey;
      press.dataset.knowledgeSev = row.id;
      const on = query === `sev:${row.id}`;
      press.classList.toggle("is-active", on);
      press.setAttribute("aria-pressed", String(on));
      press.prepend(knowledgeSupplyMark("vulnerability", { severity: row.id }));
      return held;
    }));
  reconcileElementOrder(section.querySelector(".knowledge-supply-lockfiles"), answer.lockfiles.map((row) => {
    const item = document.createElement("li");
    const path = document.createElement("span");
    path.className = "knowledge-supply-lockfile";
    path.textContent = row.path;
    const said = document.createElement("span");
    said.className = "knowledge-inspector-note";
    said.textContent = row.unreadable
      ? t("knowledge.supplyUnreadable", "읽지 못함: {{why}}", { why: row.unreadable })
      : t("knowledge.supplyLockfileCount", "{{ecosystem}} · 구성요소 {{count}}", {
        ecosystem: knowledgeSupplyRegistry(row.ecosystem),
        count: row.componentCount,
      });
    item.append(path, said);
    return item;
  }));
}

/* ---- 고른 구성요소·취약점의 카드 ---- */

function buildKnowledgeSupplyCard() {
  const box = document.createElement("section");
  box.className = "knowledge-inspector-supply";
  box.hidden = true;
  const facts = document.createElement("dl");
  facts.className = "knowledge-supply-facts";
  const summary = document.createElement("p");
  summary.className = "knowledge-inspector-excerpt knowledge-supply-summary";
  const acts = document.createElement("div");
  acts.className = "knowledge-inspector-acts knowledge-supply-acts";
  const osv = document.createElement("button");
  osv.type = "button";
  osv.className = "btn knowledge-supply-osv";
  osv.dataset.i18n = "knowledge.supplyOsv";
  osv.textContent = t("knowledge.supplyOsv", "OSV에서 보기");
  const unfold = document.createElement("button");
  unfold.type = "button";
  unfold.className = "btn knowledge-unfold knowledge-supply-unfold";
  acts.append(osv, unfold);
  box.append(
    facts,
    summary,
    acts,
    knowledgeListSection("knowledge-supply-vulnerabilities", "knowledge.supplyVulnerabilities",
      t("knowledge.supplyVulnerabilities", "이 구성요소의 취약점")),
    knowledgeListSection("knowledge-supply-affected", "knowledge.supplyAffected",
      t("knowledge.supplyAffected", "영향받는 구성요소")),
    knowledgeListSection("knowledge-supply-paths", "knowledge.supplyPaths",
      t("knowledge.supplyPaths", "멤버에서 닿는 경로")),
  );
  return box;
}

/* 사실 한 줄 — 이름(dt)과 값(dd). 값의 자리는 이름표(`data-supply-fact`)로 읽힌다. */
function knowledgeSupplyFact(name, head, value) {
  const term = document.createElement("dt");
  term.textContent = head;
  const said = document.createElement("dd");
  said.dataset.supplyFact = name;
  said.textContent = value;
  return [term, said];
}

/* 사슬의 한 걸음 — 그림에 선 점이면 고르는 단추, 아니면 낱말. */
function knowledgeSupplyStep(model, key, title) {
  const step = document.createElement("button");
  step.type = "button";
  step.className = "knowledge-inspector-row knowledge-supply-step";
  step.dataset.knowledgeKey = key;
  step.textContent = title;
  step.disabled = !model.keys.includes(key);
  return step;
}

function paintKnowledgeSupplyCard(layout, box, seat) {
  const section = box.querySelector(".knowledge-inspector-supply");
  const model = layout.model;
  const row = model.supplyRow?.[seat] ?? -1;
  section.hidden = row < 0;
  if (row < 0) return;
  const joined = model.supply;
  const { answer } = joined;
  const component = model.kinds[seat] === "component";
  const stamp = `${model.signature}|${model.keys[seat]}|${joined.unfoldGeneration}|${locale}|${answer.lookup?.checkedAt}`;
  if (section.dataset.knowledgeStamp === stamp && layout.supplyCardModel === model) return;
  section.dataset.knowledgeStamp = stamp;
  layout.supplyCardModel = model;
  const facts = [];
  const summary = section.querySelector(".knowledge-supply-summary");
  const osv = section.querySelector(".knowledge-supply-osv");
  const unfold = section.querySelector(".knowledge-supply-unfold");
  const listOf = (className) => section.querySelector(`.${className}`);
  const rowsInto = (className, rows) => {
    const held = listOf(className);
    held.hidden = rows.length === 0;
    reconcileElementOrder(held.querySelector(".knowledge-inspector-list"), rows);
  };
  if (component) {
    const part = answer.components[row];
    const origin = KNOWLEDGE_SUPPLY_ORIGINS[part.origin];
    const originWord = origin ? t(origin.key, origin.word) : String(part.origin);
    facts.push(
      ...knowledgeSupplyFact("ecosystem", t("knowledge.supplyEcosystem", "생태계"),
        knowledgeSupplyRegistry(part.ecosystem)),
      ...knowledgeSupplyFact("version", t("knowledge.supplyVersion", "판"), part.version || "—"),
      ...knowledgeSupplyFact("origin", t("knowledge.supplyOrigin", "출처"),
        part.source ? `${originWord} · ${part.source}` : originWord),
      ...knowledgeSupplyFact("dependencies", t("knowledge.supplyDependencyHead", "의존"),
        t("knowledge.supplyDependencies", "의존 {{dependencies}} · 의존하는 쪽 {{dependents}}",
          { dependencies: joined.dependencies(row), dependents: joined.dependents(row) })),
      ...knowledgeSupplyFact("lockfiles", t("knowledge.supplyLockfiles", "잠금 파일"), part.lockfiles.join(", ")),
    );
    if (part.member === true) {
      facts.push(...knowledgeSupplyFact("member", t("knowledge.supplyMemberHead", "워크스페이스"),
        t("knowledge.supplyMember", "워크스페이스 멤버")));
    }
    if (joined.folded[row] > 0) {
      facts.push(...knowledgeSupplyFact("folded", t("knowledge.supplyFoldedHead", "접힘"),
        knowledgeSupplyFoldWords(joined.folded[row])));
    }
    summary.hidden = true;
    osv.hidden = true;
    unfold.hidden = joined.rooted[row] === 0;
    if (!unfold.hidden) {
      const open = joined.unfolded[row] === 1;
      unfold.dataset.knowledgeSupplyUnfold = model.keys[seat];
      unfold.textContent = open ? t("knowledge.supplyRefold", "다시 접기") : t("knowledge.unfold", "펼치기");
      unfold.setAttribute("aria-label", open
        ? t("knowledge.supplyRefoldOf", "«{{name}}» 아래 구성요소 {{count}}개 다시 접기", { name: model.titles[seat], count: joined.rooted[row] })
        : t("knowledge.supplyUnfoldOf", "«{{name}}» 아래 접힌 구성요소 {{count}}개 펼치기", { name: model.titles[seat], count: joined.rooted[row] }));
    } else {
      knowledgeSupplyForgetUnfold(unfold);
    }
    rowsInto("knowledge-supply-vulnerabilities", joined.affectsOf[row].map((at) => {
      const found = answer.vulnerabilities[at];
      const line = knowledgeListRow(`${KNOWLEDGE_SUPPLY_KEYS.vulnerability}${found.id}`, found.id,
        knowledgeSupplySeverityWord(found.severity));
      line.querySelector("button").prepend(knowledgeSupplyMark("vulnerability", { severity: found.severity }));
      return line;
    }));
    rowsInto("knowledge-supply-affected", []);
    rowsInto("knowledge-supply-paths", []);
  } else {
    const found = answer.vulnerabilities[row];
    const informational = KNOWLEDGE_SUPPLY_INFORMATIONAL[found.informational] ?? null;
    facts.push(
      ...knowledgeSupplyFact("aliases", t("knowledge.supplyAliases", "별칭"),
        found.aliases.length > 0 ? found.aliases.join(" · ") : "—"),
      ...knowledgeSupplyFact("severity", t("knowledge.supplySeverity", "심각도"),
        found.score === null ? knowledgeSupplySeverityWord(found.severity)
          : `${knowledgeSupplySeverityWord(found.severity)} · CVSS ${found.score}`),
    );
    if (informational !== null) {
      facts.push(...knowledgeSupplyFact("informational", t("knowledge.supplyInformational", "정보성 권고"),
        t(informational.key, informational.word)));
    }
    facts.push(
      ...knowledgeSupplyFact("fixed", t("knowledge.supplyFixed", "고친 판"), found.fixed.length > 0
        ? found.fixed.map((one) => `${one.name} ${one.version}`).join(" · ")
        : t("knowledge.supplyNoFix", "알려진 고친 판 없음")),
      ...knowledgeSupplyFact("checked", t("knowledge.supplyChecked", "마지막 확인"),
        knowledgeSupplyWhen(answer.lookup?.checkedAt ?? 0)),
    );
    summary.hidden = found.summary === "";
    writeTextContent(summary, found.summary);
    osv.hidden = !found.url;
    osv.dataset.url = found.url ?? "";
    unfold.hidden = true;
    knowledgeSupplyForgetUnfold(unfold);
    const targets = joined.affectedBy[row];
    rowsInto("knowledge-supply-vulnerabilities", []);
    rowsInto("knowledge-supply-affected", targets.map((at) => {
      const part = answer.components[at];
      return knowledgeListRow(`${KNOWLEDGE_SUPPLY_KEYS.component}${part.id}`, knowledgeSupplyComponentTitle(part),
        knowledgeSupplyRegistry(part.ecosystem));
    }));
    rowsInto("knowledge-supply-paths", targets.map((at) => {
      const chain = [];
      for (let step = at; step >= 0; step = joined.parent[step]) chain.unshift(step);
      const item = document.createElement("li");
      item.className = "knowledge-supply-path";
      if (joined.root[at] < 0) {
        const lone = document.createElement("span");
        lone.className = "knowledge-inspector-note";
        lone.textContent = t("knowledge.supplyNoPath", "멤버에서 닿는 경로 없음");
        item.append(knowledgeSupplyStep(model, `${KNOWLEDGE_SUPPLY_KEYS.component}${answer.components[at].id}`,
          knowledgeSupplyComponentTitle(answer.components[at])), lone);
        return item;
      }
      chain.forEach((step, index) => {
        if (index > 0) {
          const arrow = document.createElement("span");
          arrow.className = "knowledge-chain-arrow";
          arrow.setAttribute("aria-hidden", "true");
          arrow.textContent = "›";
          item.append(arrow);
        }
        const part = answer.components[step];
        item.append(knowledgeSupplyStep(model, `${KNOWLEDGE_SUPPLY_KEYS.component}${part.id}`,
          knowledgeSupplyComponentTitle(part)));
      });
      return item;
    }));
  }
  reconcileElementOrder(section.querySelector(".knowledge-supply-facts"), facts);
}

/* 서지 않는 펼치기 문은 앞 카드의 이름을 들고 있지 않는다 — 언어가 바뀐 뒤의 카드에 옛 언어의
 * 이름이 남지 않게. */
function knowledgeSupplyForgetUnfold(unfold) {
  delete unfold.dataset.knowledgeSupplyUnfold;
  unfold.removeAttribute("aria-label");
  writeTextContent(unfold, "");
}

/* 접힌 구성요소의 낱말 — 멤버의 「+N」이 무엇을 세는가. */
function knowledgeSupplyFoldWords(count) {
  return t("knowledge.supplyFolded", "접힌 구성요소 {{count}}", { count });
}

/* 인스펙터의 공급망 손 — 펼치기·OSV·심각도 줄·다시 확인. 받았으면 참. */
function knowledgeSupplyInspectorClick(view, event) {
  const unfold = event.target.closest("[data-knowledge-supply-unfold]");
  if (unfold) {
    toggleKnowledgeSupplyUnfold(unfold.dataset.knowledgeSupplyUnfold);
    return true;
  }
  const osv = event.target.closest(".knowledge-supply-osv");
  if (osv) {
    if (osv.dataset.url) routeHttpLink(osv.dataset.url, event);
    return true;
  }
  const severity = event.target.closest("button[data-knowledge-sev]");
  if (severity) {
    const word = `sev:${severity.dataset.knowledgeSev}`;
    knowledgeQuery = knowledgeQuery.trim().toLocaleLowerCase() === word ? "" : word;
    knowledgeTagsPicked.clear();
    view.querySelector(".knowledge-query").value = knowledgeQuery;
    void paintKnowledgeView();
    return true;
  }
  if (event.target.closest(".knowledge-supply-recheck")) {
    void refreshKnowledgeSupply({ force: true, refresh: true });
    return true;
  }
  if (event.target.closest(".knowledge-supply-report")) {
    void writeKnowledgeSupplyReport();
    return true;
  }
  return false;
}

/* 보고서 한 장을 쓴다. 명령이 워크스페이스의 output/ 에 파일을 남기고 어디에
 * 남겼는지 답하므로, 창은 그 줄만 말한다. 실패는 같은 자리에 그대로 적는다 —
 * 보고서 하나 때문에 그림이 사라지지는 않는다. */
async function writeKnowledgeSupplyReport() {
  const view = document.querySelector(".knowledge-view");
  const wrote = view === null ? null : view.querySelector(".knowledge-supply-wrote");
  const button = view === null ? null : view.querySelector(".knowledge-supply-report");
  if (button !== null) button.disabled = true;
  try {
    const written = await invoke("supply_chain_report", {});
    if (wrote !== null) {
      wrote.hidden = false;
      say(wrote, t("knowledge.supplyReportWrote", "보고서를 {path} 에 썼습니다 — 권고 {advisories}건, 올릴 곳 {raise}곳")
        .replace("{path}", written.path)
        .replace("{advisories}", String(written.advisories))
        .replace("{raise}", String(written.raise)));
    }
  } catch (error) {
    if (wrote !== null) {
      wrote.hidden = false;
      say(wrote, t("knowledge.supplyReportFailed", "보고서를 쓰지 못했습니다") + ` — ${error}`);
    }
  } finally {
    if (button !== null) button.disabled = false;
  }
}

/* 공급망의 단(P4) — 멤버와 취약점은 대표 지식의 크기와 이름표 우선순위로 선다. 잎의 크기(3 px)로는
 * 삼각형의 잉크가 읽히지 않고(한 변 8 px), 멤버는 그 워크스페이스의 얼굴이다. 나머지 구성요소는
 * 연결의 수를 따르는 잎이다. */
function knowledgeSupplyTiers(model, tier) {
  if (model.supply === null) return;
  for (let at = 0; at < model.count; at += 1) {
    if (model.member[at] === 1 || model.kinds[at] === "vulnerability") tier[at] = KNOWLEDGE_TIER_MAJOR;
  }
}
