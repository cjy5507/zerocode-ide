/* ---- 지식 그래프 / the second brain's knowledge graph ---------------------
 *
 * 사용자 요청: "X 글에서 하는 방식대로 지식 그래프도 보였으면" — Obsidian의
 * 그래프 뷰 그림이다. 볼트의 `wiki/` 페이지가 점이고 `[[위키링크]]`가 선이며,
 * 아직 쓰지 않은 페이지를 가리키는 링크는 **유령 점**으로 남는다. 스캔은
 * `second_brain_graph`(Rust)의 것이고, 이 파일이 하는 일은 그 답을 배치하고
 * 그리는 것뿐이다 — 창은 볼트를 걷지 않는다.
 *
 * 캔버스의 기구는 에이전트 그래프와 나눠 쓴다(`shell.js`의 「shared graph
 * mechanics」): 미는 몸짓, 바퀴의 배율, 프레임 예약, 리사이즈 관찰, 그리고
 * **재고 나서 쓰는** 간선 페인터. 나누는 것과 나누지 않는 것의 경계는 측정된
 * 것이다 — 두 표면은 배율의 뜻이 서로 다르다. 에이전트 그래프의 배율은
 * 리플로(격자와 낱말이 실제로 커진다)이고, 이 표면의 배율은 viewBox(그림이
 * 제 좌표계에 있고 점의 크기는 화면 픽셀로 고정된다)다. 한쪽 기구를 다른 쪽에
 * 억지로 끼우는 것은 재사용이 아니라 두 표면을 다 나쁘게 만드는 일이다. */

/* 하나뿐인 탭. 보드와 같은 이유로 id가 고정이다 — 볼트 하나의 그림 하나. */
const wiredKnowledgeViews = new WeakSet();
const KNOWLEDGE_TAB = Object.freeze({ id: "knowledge", kind: "knowledge" });

/* 다시 읽기의 바닥. 탭을 볼 때마다·에이전트가 끝날 때마다 다시 묻지만, 이보다
 * 자주는 묻지 않는다 — 캐시가 있어도 5,000개의 stat은 공짜가 아니다. */
const KNOWLEDGE_REFRESH_FLOOR_MS = 1500;
/* 군집의 색 칸 수. 색값은 이 파일에 한 자도 없다 — 점의 색은 CSS가
 * `data-hue`로 고르고, 여덟 칸은 토큰(`--knowledge-hue-0..7`)이 든다: 그래프
 * 레인 램프 다섯(`--git-graph-lane-1..5`, IBM의 색약 안전 다섯)에 두 테마 모두
 * 값을 가진 신호색 둘과 그 둘의 중간 하나. 3차부터 색은 태그가 아니라 **군집**의
 * 것이다(`knowledgeCommunities`) — 볼트 27쪽 중 15쪽이 같은 첫 태그였고, 그
 * 그림에서 색은 아무것도 가르지 못했다. 여덟 뒤의 군집은 여덟로 나눈 나머지를
 * 입는다. 아이덴티티 레인(`--lane-*`)은 라이트 판에서 한 색으로 접히므로
 * 여전히 빌릴 수 없다. */
const KNOWLEDGE_HUES = 8;
/* 칩으로 세우는 태그의 상한. 백 개의 칩은 필터가 아니라 두 번째 목록이다. */
const KNOWLEDGE_TAG_CHIPS = 12;
/* 툴바에는 상위 여섯만 서고 나머지는 `+N` 뒤에 선다. 열두 개 전체는 좁은 판의
 * 렌즈 팝오버 안에서 다시 펼쳐진다. */
const KNOWLEDGE_TAGS_INLINE = 6;
/* 볼트 개요가 세우는 줄 수 — 허브도 최근도 이만큼. 다섯인 것은 이것이 목록이
 * 아니라 **첫인상**이기 때문이다: 스무 줄짜리 개요는 그림 옆의 두 번째 그림이
 * 되고, 그때 사람이 읽는 것은 그림이 아니라 목록이다. */
const KNOWLEDGE_OVERVIEW_ROWS = 5;
/* 허브가 될 수 있는 몫. 차수 문턱 하나만으로는 큰 볼트에서 허브가 뜻을 잃는다 —
 * 실측: 1020 페이지에서 `degree >= 4`가 656개였다. 볼트의 3분의 2는 허브가
 * 아니라 그냥 페이지이고, 허브는 halo를 쓰고 배율과 무관하게 이름을 다는
 * 자리이므로 그 656은 프레임 예산에도 값을 매긴다. 그래서 문턱은 토큰의 차수와
 * 「상위 5%」 중 **엄한 쪽**이다. */
const KNOWLEDGE_HUB_SHARE = 0.05;
/* 슬라이서의 프리셋 — 지금에서 거슬러 잰 창. 판을 지을 때와 누를 때 이 표 하나를
 * 읽는다(낱말의 키와 한국어, 창의 길이). `ms` 0은 전체다. */
const KNOWLEDGE_SLICER_PRESETS = Object.freeze([
  { id: "all", key: "knowledge.slicerAll", word: "전체", ms: 0 },
  { id: "1h", key: "knowledge.slicer1h", word: "1시간", ms: 3_600_000 },
  { id: "24h", key: "knowledge.slicer24h", word: "24시간", ms: 86_400_000 },
  { id: "7d", key: "knowledge.slicer7d", word: "7일", ms: 7 * 86_400_000 },
  { id: "30d", key: "knowledge.slicer30d", word: "30일", ms: 30 * 86_400_000 },
]);
/* 연결 만들기(t-4140 S5)가 쓸 수 있는 관계 — frontmatter의 다섯 키(자리 1~5). 본문 링크
 * (0)와 merge 후보(6)는 사람이 적는 키가 아니다. */
const KNOWLEDGE_LINK_KINDS = Object.freeze([1, 2, 3, 4, 5]);
/* 방향 둘: 이 페이지에서 대상으로, 대상에서 이 페이지로. 첫 줄이 기본이다. */
const KNOWLEDGE_LINK_DIRECTIONS = Object.freeze([
  { id: "outward", key: "knowledge.linkOutward", word: "이 페이지 → 대상" },
  { id: "inward", key: "knowledge.linkInward", word: "대상 → 이 페이지" },
]);
/* 관리용 문서(t-4140 S6): 볼트 규약(AGENTS.md)이 목차와 일지로 쓰는 두 페이지. 그 링크는
 * 표시 설정으로 켜고 끈다 — 그림에서만이다. 페이지는 모델·검색·관계 목록에 그대로
 * 있고, 파일명만 보고 삭제되거나 의미 관계에서 자동으로 빠지지 않는다. */
const KNOWLEDGE_NAV_PAGES = Object.freeze(["wiki/index.md", "wiki/log.md"]);
/* 인스펙터의 두 탭(t-4140 S4). 첫 줄이 기본이다. */
const KNOWLEDGE_INSPECTOR_TABS = Object.freeze([
  { id: "page", key: "knowledge.tabPage", word: "페이지" },
  { id: "activity", key: "knowledge.tabActivity", word: "활동" },
]);
/* 포커스 깊이의 세 칸. Obsidian의 로컬 그래프가 1~3을 주는 것과 같은 범위이고,
 * 같은 이유다 — 4홉이면 대개 볼트 전체가 밝아져 포커스가 뜻을 잃는다. */
const KNOWLEDGE_FOCUS_DEPTHS = Object.freeze([1, 2, 3]);
/* 탐색의 두 모드(t-4140). 표의 순서는 툴바의 순서다; 첫 방문이 어느 모드로 서는지는
 * `knowledgeStartMode`가 정한다(승인된 시안의 화면 = 주변 탐색). Obsidian이
 * 전체 그래프와 활성 노트의 로컬 그래프를 따로 주는 것과 같은 둘이고, 주변 탐색은
 * **흐리기가 아니라 하위 그래프만 그리는** 모드다: 중심과 깊이 안의 점·선만 DOM에
 * 서고 나머지는 빠진다. 투명도만 낮추는 것은 보관 메모리도 DOM도 줄이지 않는다. */
const KNOWLEDGE_MODES = Object.freeze([
  { id: "global", key: "knowledge.modeGlobal", word: "전체 지도", glyph: "orbit" },
  /* `first`: 이 볼트에 기억이 없는 첫 방문이 서는 모드 — 승인된 시안의 첫 화면. */
  { id: "local", key: "knowledge.modeLocal", word: "주변 탐색", glyph: "crosshair", first: true },
]);
const KNOWLEDGE_FIRST_MODE = KNOWLEDGE_MODES.find((row) => row.first === true)?.id ?? KNOWLEDGE_MODES[0].id;
/* 주변 탐색의 수 셋 — 리터럴은 이 표에만 산다.
 *
 * `foldDegree`: 이 차수부터의 허브는 중심이 아닌 한 **접힌다** — 깊이 2·3의 걸음이
 * 그 허브의 이웃을 끌어오지 않고, 생략한 연결 수를 관계 종류별 묶음으로 보인다.
 * 색인 페이지 하나가 천 쪽을 가리키는 볼트에서 깊이 2는 그 페이지를 지나는 순간
 * 볼트 전체가 되고, 그때 주변 탐색은 뜻을 잃는다. 펼치기는 명시적이다.
 * `historyMax`: ← 이전이 되짚을 수 있는 걸음. `candidates`: 검색 후보 목록의 줄 수. */
const KNOWLEDGE_EXPLORE = Object.freeze({
  foldDegree: 12,
  historyMax: 40,
  candidates: 8,
  /* 마지막 자리(모드·중심·깊이)를 설정 문서에 적기까지 미루는 시간 — 중심을 세 번
   * 옮기는 손이 디스크를 세 번 쓰지 않게(S2). */
  persistMs: 400,
});
/* 시각 수치의 주소. 값은 tokens.css 한 곳에 있고 판을 지을 때 한 번 읽는다. */
/* 판 폭의 티어마다 이름표 예산 하나(09-16). 좁은 판은 글자를 줄이는 대신 수를
 * 줄인다 — 「작은 화면에서는 표시량을 줄인다. 글자를 작게 줄여 해결하지 않는다」. */
const KNOWLEDGE_LABEL_BUDGET = Object.freeze({
  wide: "--knowledge-label-budget",
  middle: "--knowledge-label-budget-middle",
  compact: "--knowledge-label-budget-compact",
  tiny: "--knowledge-label-budget-tiny",
});
/* 점의 세 단(09-16) — 잎 · 대표 지식 · 군집의 중심. 자리가 곧 위계라 배열의
 * 순서를 바꾸는 것은 크기를 바꾸는 일이다. */
const KNOWLEDGE_TIER_LEAF = 0;
const KNOWLEDGE_TIER_MAJOR = 1;
const KNOWLEDGE_TIER_CORE = 2;
const KNOWLEDGE_TIER_WORD = Object.freeze(["leaf", "major", "core"]);
const KNOWLEDGE_TOKENS = Object.freeze({
  radiusMin: "--knowledge-node-radius-min",
  radiusMax: "--knowledge-node-radius-max",
  radiusDegree: "--knowledge-node-radius-degree",
  haloScale: "--knowledge-halo-scale",
  labelPx: "--knowledge-label-px",
  labelGap: "--knowledge-label-gap",
  edgeLabelPx: "--knowledge-label-edge-px",
  markerPx: "--knowledge-marker-px",
  fitRatio: "--knowledge-fit-ratio",
  margin: "--knowledge-margin",
  /* 주변 탐색의 고리(시안 이식): 이웃 사이 호의 길이, 첫 고리의 최소 반지름, 고리 사이
   * 간격, 세로 눌림. */
  ringPitch: "--knowledge-ring-pitch",
  ringMin: "--knowledge-ring-min",
  ringGap: "--knowledge-ring-gap",
  ringSquash: "--knowledge-ring-squash",
  ringLabelRoom: "--knowledge-ring-label-room",
  /* 성좌 배치(09-16): 군집 원반의 치수와 이완, 원반 안의 씨앗 고리. */
  clusterPitch: "--knowledge-cluster-pitch",
  clusterMinRadius: "--knowledge-cluster-min-radius",
  clusterGap: "--knowledge-cluster-gap",
  clusterSweeps: "--knowledge-cluster-sweeps",
  clusterLink: "--knowledge-cluster-link",
  clusterPull: "--knowledge-cluster-pull",
  clusterHold: "--knowledge-cluster-hold",
  coreRing: "--knowledge-core-ring",
  coreRingGap: "--knowledge-core-ring-gap",
  /* 위계의 세 크기와 대표 지식의 몫. */
  coreRadius: "--knowledge-core-radius",
  majorRadius: "--knowledge-major-radius",
  majorShare: "--knowledge-major-share",
  majorMax: "--knowledge-major-max",
  /* 이름표 격자. */
  plateWide: "--knowledge-plate-wide",
  labelCell: "--knowledge-label-cell",
  labelPadX: "--knowledge-label-pad-x",
  labelPadY: "--knowledge-label-pad-y",
  labelEmWide: "--knowledge-label-em-wide",
  labelEmNarrow: "--knowledge-label-em-narrow",
  clusterLabelLift: "--knowledge-cluster-label-lift",
  clusterCountPx: "--knowledge-cluster-count-px",
  clusterLeadMax: "--knowledge-cluster-lead-max",
  clusterLeadGap: "--knowledge-cluster-lead-gap",
  clusterCountGap: "--knowledge-cluster-count-gap",
  zoomMin: "--knowledge-zoom-min",
  zoomMax: "--knowledge-zoom-max",
  zoomStep: "--knowledge-zoom-step",
  hubDegree: "--knowledge-hub-degree",
  tierWide: "--knowledge-tier-wide",
  toolbarWide: "--knowledge-toolbar-wide",
  tierCompact: "--knowledge-tier-compact",
  tierTiny: "--knowledge-tier-tiny",
  yScaleMiddle: "--knowledge-y-scale-middle",
  yScaleCompact: "--knowledge-y-scale-compact",
  nebulaScale: "--knowledge-nebula-scale",
  nebulaPad: "--knowledge-nebula-pad",
  clusterLabelPx: "--knowledge-cluster-label-px",
  clusterLabelUntil: "--knowledge-cluster-label-until",
  labelMax: "--knowledge-label-max",
  settleFrames: "--knowledge-settle-frames",
  flyMs: "--knowledge-fly-ms",
  dragBudget: "--knowledge-drag-budget",
  arriveMs: "--knowledge-arrive",
  /* 라이브 층(t-2931): 새 점·새 선이 한 번 맥동하는 시간과 그 골. */
  pulseMs: "--knowledge-pulse-ms",
  pulseDip: "--knowledge-pulse-dip",
  pulseMax: "--knowledge-pulse-max",
  /* 잉크의 무게와 흐림(6차). SVG는 이 수들을 **규칙**으로 입는다(`.is-ghost`가
   * 제 stroke-width를 안다) — GL 페인터는 같은 수를 손에 들어야 같은 그림을
   * 그린다. 그래서 표에 이름이 선다: 값은 여전히 tokens.css 한 곳이다. */
  nodeStroke: "--knowledge-node-stroke",
  nodeSelectedStroke: "--knowledge-node-selected-stroke",
  nodeGhostStroke: "--knowledge-node-ghost-stroke",
  nodeGhostDash: "--knowledge-node-ghost-dash",
  nodeMatchStroke: "--knowledge-node-match-stroke",
  edgeMentionsWidth: "--knowledge-edge-mentions-width",
  edgeLitWidth: "--knowledge-edge-lit-width",
  edgeGhostDash: "--knowledge-edge-ghost-dash",
  dimOpacity: "--knowledge-dim-opacity",
  dimEdgeOpacity: "--knowledge-dim-edge-opacity",
  farNodeOpacity: "--knowledge-far-node-opacity",
  farEdgeOpacity: "--knowledge-far-edge-opacity",
  nebulaCore: "--knowledge-nebula-core",
  nebulaEdgeWeight: "--knowledge-nebula-edge-weight",
  nebulaEdgeWidth: "--knowledge-nebula-edge-width",
  activityRingWidth: "--knowledge-activity-ring-width",
  activityRingDash: "--knowledge-activity-ring-dash",
  /* GL만의 둘 — SDF 가장자리의 부드러움과 활동 고리가 몸에서 떨어지는 거리. */
  glFeather: KNOWLEDGE_GL_TOKENS.feather,
  glRingGap: KNOWLEDGE_GL_TOKENS.ringGap,
  glPixelCap: KNOWLEDGE_GL_TOKENS.pixelCap,
});
/* 간선의 종류. 0은 본문의 `[[위키링크]]`이고 나머지 다섯은 프론트매터가 제
 * 이름으로 선언한 관계다 — 백엔드의 `kind` 문자열이 이 순서 그대로다.
 * **자리가 곧 코드다**: 모델이 이 배열의 인덱스를 `Uint8Array`에 실어 나르므로
 * 가운데에 끼워 넣는 것은 옛 코드를 다른 뜻으로 읽는 일이다 — 새 종류는 뒤에만
 * 붙는다. */
const KNOWLEDGE_EDGE_KINDS = Object.freeze([
  "mentions",
  "related",
  "implements",
  "depends_on",
  "supersedes",
  "contradicts",
  /* 뒤에 붙은 일곱째(t-2931): 백엔드가 센 merge 후보 쌍. 볼트에 적힌 관계가
   * 아니라 **물음**이고, 「합칠 후보만」 렌즈가 켜졌을 때만 모델에 선다. */
  "merge",
  /* 뒤에 붙은 여덟째(P4): 공급망의 취약점 → 구성요소. OSV의 발견이고, 공급망 렌즈가 켜졌을 때만
   * 선다. 구성요소 → 구성요소는 새 종류가 아니라 볼트의 `depends_on` 그대로다(같은 낱말, 같은 선). */
  "affects",
]);
/* 방향이 있는 관계 — 화살촉을 다는 종류. `related`·`mentions`·`merge`는 방향이 없거나 물음이다.
 * 화살촉의 정의(`knowledgeDefsHtml`)와 선의 옷(`paintFrame`의 `dress`)이 이 한 줄을 읽는다. */
const KNOWLEDGE_EDGE_DIRECTED = Object.freeze(["implements", "depends_on", "supersedes", "contradicts", "affects"]);
/* 낱말 → 자리. 위 배열의 역이고, 자리를 세는 곳(건강 카드의 두 수)이 읽는다. */
const KNOWLEDGE_EDGE_CODE = Object.freeze(
  Object.fromEntries(KNOWLEDGE_EDGE_KINDS.map((kind, code) => [kind, code])),
);

/* 군집 탐지 (3차).
 *
 * 카파시가 청한 도구(Graphify)와 InfraNodus가 하는 그대로, 색은 링크 위상의
 * **커뮤니티**다 — 결정적 Louvain(노드 순서 = 인덱스, 동률 = 낮은 id). 타입
 * 관계는 프론트매터가 이름 붙여 선언한 관계라 본문의 대괄호보다 두 배 무겁다.
 * 페이지가 셋도 안 되는 군집은 이름도 색도 얻지 않는다: 고아 하나가 군집 하나이고,
 * 그 백 개에 색을 주면 색은 다시 아무것도 가르지 못한다. 단계와 훑기의 상한은
 * 천 노드에서 몇 밀리초 안에 끝나게 잡은 것이다(첫 그림 300ms 계약 안). */
const KNOWLEDGE_COMMUNITY = Object.freeze({
  mentionsWeight: 1,
  typedWeight: 2,
  minSize: 3,
  /* 군집 안에서 「대표 지식」으로 설 수 있는 가장 낮은 차수. 이웃이 하나뿐인
   * 쪽은 군집의 얼굴이 아니다 — 작은 군집에서 몫만으로 고르면 잎 하나가 대표가
   * 된다. */
  majorDegree: 3,
  maxLevels: 8,
  maxSweeps: 20,
  /* 캔버스 위 이름표의 글자 수 상한 — 그 뒤는 말줄임. */
  labelMax: 24,
  /* 부동소수의 동률. */
  epsilon: 1e-9,
});

/* 결정적 포스-디렉티드 배치.
 *
 * 결정적이어야 하는 이유는 이것이 사람의 기억이 되기 때문이다 — 같은 볼트를
 * 두 번 열었을 때 다른 그림이 나오면 "왼쪽 위의 그 덩어리"라는 말이 성립하지
 * 않는다. 그래서 난수가 한 곳도 없다: 첫 자리는 노드 id의 해시와 황금각으로
 * 정하고, 겹친 두 점을 떼는 손짓도 인덱스에서 만든다.
 *
 * 반발은 격자로 근사한다. 전부 대 전부는 천 노드에서 오십만 쌍이고 그것은 한
 * 프레임에 들어가지 않는다 — 제 칸과 이웃 여덟 칸만 보고, 한 칸에서 세는
 * 이웃도 상한을 둔다(뭉친 칸이 다시 O(n²)이 되는 것을 막는다). */
const KNOWLEDGE_FORCE = Object.freeze({
  /* 반복 예산. 천 노드 아래는 이만큼, 그 위는 노드 수에 반비례해 줄어든다 —
   * 한 번의 훑기가 비싸질수록 예산을 줄이지 않으면 큰 판에서만 창이 오래
   * 바쁘다. */
  budget: 260,
  smallGraph: 1000,
  minBudget: 60,
  /* 이미 놓여 있던 점이 대부분이면(필터를 바꾼 판) 처음부터 앉히지 않는다. */
  settleBudget: 70,
  /* 한 프레임에 쓸 수 있는 밀리초. 이 밖에서는 계산하지 않는다 — 배치가 한
   * 프레임을 통째로 먹으면 그 판은 스크롤도 입력도 받지 못한다. */
  frameMs: 4,
  repulsion: 500,
  spring: 0.05,
  /* 군집 사이를 건너는 선의 몫(09-16) — 같은 주제 안의 선에 대한 비율. */
  interSpring: 0.2,
  springLength: 46,
  damping: 0.8,
  /* 한 훑기에 점이 움직일 수 있는 최대 거리.
   *
   * 반발은 거리의 제곱에 반비례하므로 상한이 없으면 겹친 두 점의 한 번의
   * 충돌이 점을 수천 단위 밖으로 날려 보내고, 그것을 데려오는 힘은 중력뿐이다.
   * 실측: 상한이 없을 때 천 노드의 그림은 반발 1200·중력 0.012에서 22,902단위,
   * 반발 380·중력 0.09로 낮춰도 15,749단위로 퍼졌다 — 스프링 길이의 삼백 배이고,
   * 그 배율에서 점들은 그래프가 아니라 먼지다. 힘의 세기를 더 낮추는 것은 답이
   * 아니었다: 터지는 것은 평균이 아니라 **한 쌍**이므로, 고칠 자리는 그 한
   * 걸음의 상한이다(Fruchterman-Reingold의 온도와 같은 자리). 하네스가 퍼진
   * 폭을 재고 있으므로 이 수를 건드리는 다음 사람은 숫자로 답을 받는다. */
  maxStep: 20,
  /* 반발 격자 한 칸의 크기와, 한 변의 칸 수 상한. */
  cell: 90,
  cells: 96,
  /* 한 칸에서 세는 이웃의 상한. */
  neighbours: 24,
  /* Initial spiral, deterministic overlap nudge and serialization precision.
   * These were formerly anonymous literals in the hot paths. */
  initialRadius: 0.85,
  hashModulo: 997,
  angleJitter: 0.5,
  overlapModuloX: 7,
  overlapModuloY: 5,
  overlapCentreX: 3,
  overlapCentreY: 2,
  overlapScale: 0.1,
  overlapNudge: 0.02,
  inversePrecision: 1000,
  coordinateDigits: 1,
  /* 군집 원반의 이완이 한 판에서 쓸 수 있는 「쌍 방문」의 총량(09-16). 이름 있는
   * 군집이 열넷이면 훑기는 토큰의 상한까지 돌고, 백스물이면 여든세 번에서 멈춘다 —
   * 어느 볼트에서도 이 한 수가 배치의 값을 묶는다. */
  clusterVisits: 1200000,
  clusterMinSweeps: 48,
  /* 주제의 이름이 빈 자리를 찾아 올라가 보는 횟수. */
  clusterLabelTries: 3,
  /* 노드의 화면 반지름 합을 다시 한 번 띄운다. 단순한 고정 충돌 반지름은 큰
   * 허브를 작은 점과 같은 칸에 욱여넣으므로, 충돌 거리 자체가 두 반지름에서
   * 나온다. */
  collisionGap: 2,
  collisionStrength: 0.9,
  /* 대표 페이지가 홈을 쥐는 힘의 배수 — 그것이 원반의 가운데이고, 가운데가
   * 흔들리면 「이 주제의 중심은 저것」이 흔들린다. */
  coreGrip: 6,
  /* 옆자리 이름표의 세로 중심 — 글자 크기에 대한 몫(baseline을 글자의 가운데로). */
  labelMiddle: 0.35,
  /* 좁은 판에서 주제의 이름판이 접히기 전에 봐주는 칸 수. 0이면 한 칸만 스쳐도
   * 접힌다. 좁은 판의 이름판이 한 줄이 된 뒤로는 그 엄격함이 값을 치르지 않는다:
   * 열셋이 다 서면서도 겹치지 않는다. */
  plateSlack: 0,
});

const KNOWLEDGE_GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

/* 사람의 것들 — 다시 그려도 남고, 판이 아니라 사람에게 속한다. */
let knowledgeQuery = "";
let knowledgeOrphansOnly = false;
let knowledgeGhostsOnly = false;
let knowledgeTypedOnly = false;
/* 건강 카드의 줄 하나가 켠 렌즈 — 백엔드의 lint 표(`graph.lint`)에 이름이 있는
 * 페이지만 남긴다: `index_gaps`·`missing_frontmatter`·`undeclared_relations`,
 * 아니면 null. 고아·유령은 제 렌즈(위)를 그대로 쓴다. */
let knowledgeLintLens = null;
let knowledgeShowSources = false;
/* 라이브 층의 렌즈 셋(t-2931): 창 안에서 회상되거나 바뀐 것만, 한 번도 회상되지
 * 않은 것만, 그리고 merge 후보 쌍만. 창의 길이는 백엔드의 한 표가 정한다. */
let knowledgeAliveOnly = false;
let knowledgeColdOnly = false;
let knowledgeMergeOnly = false;
const knowledgeTagsPicked = new Set();
let knowledgeSelectedKey = null;
/* 다른 표면(작업 상황판의 「참고한 지식」)이 고른 페이지 — 답이 있는 첫 그림이
 * 읽고 놓는다(`paintKnowledgeView`). */
let knowledgeRevealKey = null;
let knowledgeHoverKey = null;
let knowledgeFocusDepth = 1;
/* 시간 슬라이서 (Bloom 문법): 창은 컷오프 시각 하나다 — 0이면 전체('all'). 슬라이더는
 * 그림의 시간 폭 위의 백분율로, 프리셋은 지금에서 거슬러 잰 벽시계의 창으로 이것을
 * 정한다(`KNOWLEDGE_SLICER_PRESETS`). */
let knowledgeSlicerCutoff = 0;
/* 최단 경로 (Bloom 문법): 선택 A + Shift-클릭 B -> BFS 경로. 사람이 고른 것은 두
 * 페이지이므로 열쇠 둘(`{ sourceKey, targetKey }`)만 든다 — 길은 그림마다 다시 찾는다. */
let knowledgePath = null;
/* 밝힌 군집의 순위, 없으면 -1. 순위는 그림(위상)의 것이라 위상이 바뀌면 놓는다. */
let knowledgeClusterPicked = -1;
/* 탐색 모드(t-4140)와 그 살림. 주변 탐색의 중심은 고른 점(`knowledgeSelectedKey`)이다 —
 * 중심 없는 주변은 없으므로 그 모드에서 고르기를 놓는 것은 전체 지도로 돌아가는
 * 일이다. 방문 이력은 중심·깊이·배율의 스택이고 ← 이전이 되짚는다. 펼친 허브는
 * **이 탐색**의 것이라 중심이 옮으면 다시 접힌다. 전체 지도의 카메라는 주변으로
 * 들어갈 때 들어 두었다가 돌아올 때 그대로 돌려준다. */
let knowledgeMode = KNOWLEDGE_MODES[0].id;
const knowledgeHistory = [];
const knowledgeUnfolded = new Set();
let knowledgeUnfoldGeneration = 0;
let knowledgeGlobalCamera = null;
/* 진입 규칙(S2)의 살림. 문(`openKnowledgeGraph`)을 열 때마다 「어느 모드로 서는가」를
 * 한 번 묻고(`knowledgeEntryPending`), 답이 있는 첫 그림이 그것을 푼다. 「연결 보기」는
 * 열쇠 옆에 모드를 싣는다(`knowledgeRevealMode`). 마지막 자리는 설정 문서의 한 줄이고
 * (`second_brain_explore`, 볼트별) 창은 제 사본을 따로 들지 않는다 — 시야와 같은 손. */
let knowledgeEntryPending = false;
let knowledgeRevealMode = null;
let knowledgeExploreLines = {};
let knowledgeExploreTimer = 0;
/* 검색 후보(S3) 중 ↑↓가 짚은 줄, 없으면 -1. 글자 하나에 놓는다. */
let knowledgeCandidateAt = -1;
/* 인스펙터의 켜진 탭(S4). 새 점을 고르면 「페이지」로 돌아온다 — 카드가 답이다. */
let knowledgeInspectorTab = KNOWLEDGE_INSPECTOR_TABS[0].id;
/* 목차·일지 표시(S6). 기본은 켜짐 — 지금까지의 그림 그대로. */
/* 목차·일지를 그림에 세울 것인가 — 기본은 접힘(09-16).
 *
 * `wiki/log.md`와 `wiki/index.md`는 볼트의 거의 모든 쪽을 가리킨다(실측 볼트에서
 * 차수 353·336, 전체 간선 2,168 중 689 = 32%). 그 둘이 서 있는 전체 지도는 어떤
 * 배치를 써도 한 덩어리다 — 모든 점이 같은 두 점에 매여 있으므로. 지도의 물음은
 * 「어떤 주제가 있고 무엇이 무엇과 통하는가」이고 목차는 그 물음의 답이 아니다.
 *
 * 접었다고 지운 것은 아니다: 두 쪽은 모델·검색·관계 목록·인스펙터에 그대로 있고,
 * 머리의 「목차·일지 표시」 한 번이면 그림에도 돌아온다(t-4140 S6의 계약 그대로,
 * 바뀐 것은 그 스위치의 기본값 하나다). */
let knowledgeNavShown = false;
/* 연결 만들기(S5)의 살림: 고른 대상의 열쇠, 후보 중 짚은 줄, 방향, 마지막에 쓴 관계(⌘Z가
 * 되돌린다). 서식이 닫히면 대상은 놓는다. */
let knowledgeLinkTarget = null;
let knowledgeLinkCandidateAt = -1;
let knowledgeLinkDirection = KNOWLEDGE_LINK_DIRECTIONS[0].id;
let knowledgeLastRelate = null;

/* 백엔드의 마지막 답과 그것을 물은 시각. 볼트 하나에 그림 하나이므로 판이
 * 아니라 창이 들고 있다. */
let knowledgeReport = null;
let knowledgeError = null;
let knowledgeLoading = false;
let knowledgeAskedAt = 0;
let knowledgeGeneration = 0;
/* 바닥 안에 들어온 워처의 청을 바닥이 끝나는 자리로 미루는 타이머 하나(t-2931). */
let knowledgeRefreshTimer = 0;
/* 마지막 답이 지난 답에 견줘 새로 가져온 것 — 페이지 id와 선의 열쇠. 한 번 맥동하고
 * (`knowledgePulsePending`), 낱말은 다음 답까지 남는다(하네스가 읽는다). */
let knowledgeFreshKeys = new Set();
let knowledgeFreshEdges = new Set();
let knowledgePulsePending = false;
/* 맥동한 요소의 수 — 하네스의 계량기. */
let knowledgePulses = 0;
/* 이 볼트의 「raw 취합」 빠른 명령 — 빈 상태의 단추가 누르는 것. */
let knowledgeIngest = null;

/* 판마다의 것: 좌표, 배율, 그리고 그 판에 실제로 서 있는 DOM. */
const knowledgeLayouts = new WeakMap();
const knowledgeTunings = new WeakMap();
const knowledgeFitFrames = new WeakMap();
const knowledgeStepFrames = new WeakMap();
// Keep one derived model, not one per query or per historical vault reply.
let knowledgeModelCache = null;
let knowledgeTopologyGeneration = 0;

/* 배치가 몇 번 돌았고 점을 몇 개 지었는가 — 하네스가 성능을 재는 계량기다.
 * 에이전트 그래프의 `agentGraphLayoutRuns`와 같은 자리. */
let knowledgeLayoutRuns = 0;
let knowledgeNodeCreations = 0;
let knowledgeFirstPaintMs = 0;

/* FNV-1a. 해시가 필요한 곳은 두 곳뿐이다 — 첫 자리의 지터와 태그의 색 칸 —
 * 그리고 둘 다 **같은 입력에 같은 답**이 요구의 절반이다. */
function knowledgeHash(text) {
  let hash = 2166136261;
  for (let at = 0; at < text.length; at += 1) {
    hash ^= text.charCodeAt(at);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

/* 움직임을 줄이라는 판인가. CSS의 같은 미디어 질의가 전이와 애니메이션을 끄고,
 * 여기서는 앉는 과정과 카메라 트윈을 즉시로 접는다. */
function knowledgeMotionReduced() {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/* 한 프레임에 돌릴 훑기 수.
 *
 * 앉는 과정이 **보이는** 것이 3차의 요점이다(Obsidian은 열 때마다 그림이 제
 * 모양을 찾는 것을 보여 준다). 작은 볼트의 260훑기는 한 프레임에 다 끝나고, 그때
 * 사람이 보는 것은 정지 화면이다. 그래서 남은 예산을 토큰의 프레임 수로 나눈
 * 만큼만 한 프레임에 돌린다 — 시간 예산(`frameMs`)은 여전히 상한이라 큰 판에서는
 * 그것이 먼저 닿는다. 움직임을 줄이라는 판에서는 상한이 없다: 즉시 앉는다. */
function knowledgePace(steps, tuning) {
  if (knowledgeMotionReduced()) return Number.POSITIVE_INFINITY;
  return Math.max(1, Math.ceil(steps / Math.max(1, tuning.settleFrames)));
}

function knowledgeTuning(view) {
  const held = knowledgeTunings.get(view);
  if (held) return held;
  const style = getComputedStyle(view);
  const number = (name) => Number.parseFloat(style.getPropertyValue(name));
  const tuning = Object.freeze({
    ...Object.fromEntries(Object.entries(KNOWLEDGE_TOKENS).map(([key, name]) => [key, number(name)])),
    labelBudget: Object.freeze(Object.fromEntries(
      Object.entries(KNOWLEDGE_LABEL_BUDGET).map(([key, name]) => [key, number(name)]),
    )),
    /* 스며드는 층의 완화 곡선 — 창의 표준 곡선 그대로, 숫자가 아니라 낱말이라 따로 든다. */
    ease: style.getPropertyValue("--ease-standard").trim() || "ease",
  });
  knowledgeTunings.set(view, tuning);
  return tuning;
}

/* 이 그림에서 허브로 설 수 있는 가장 낮은 차수.
 *
 * 세는 것은 정렬이 아니라 차수의 히스토그램이다 — 천 개를 정렬하는 것은 위상이
 * 바뀔 때마다 치르는 값이고, 답에 필요한 것은 순서가 아니라 몫이다. 위에서부터
 * 더해 내려오다 몫을 넘기기 직전의 차수가 답이고, 토큰의 문턱보다 낮아지지는
 * 않는다: 작은 볼트에서 「상위 5%」는 한 개이고, 이웃이 둘인 점 하나를 허브라고
 * 부르는 그림은 허브를 말하지 않는다. */
function knowledgeHubFloor(degree, count, tuning) {
  if (count === 0) return tuning.hubDegree;
  let top = 0;
  for (let at = 0; at < count; at += 1) if (degree[at] > top) top = degree[at];
  const tally = new Int32Array(top + 1);
  for (let at = 0; at < count; at += 1) tally[degree[at]] += 1;
  const want = Math.max(1, Math.round(count * KNOWLEDGE_HUB_SHARE));
  let seen = 0;
  let cut = top + 1;
  for (let deg = top; deg >= 0; deg -= 1) {
    seen += tally[deg];
    if (seen > want) break;
    cut = deg;
  }
  return Math.max(tuning.hubDegree, cut);
}

/* 점의 모양 — 종류마다 하나이고, 모양이 곧 뜻이다(「포자 형태의 동그라미로만
 * 되어있는데 의미를 넣어서」, 2026-09-17). 종류 다섯이 모양 다섯에 하나씩 닿는다 —
 * 두 종류가 한 모양을 나눠 쓰면 모양은 다시 뜻을 가르지 못한다.
 *
 *   ● circle    page           볼트의 지식 페이지
 *   ◌ ring      ghost          링크만 있고 아직 없는 페이지 — 채우지 않은 점선 고리
 *   ◆ diamond   source         페이지가 인용한 원본
 *   ■ square    component      소프트웨어 구성요소(SBOM의 패키지·크레이트)
 *   ▲ triangle  vulnerability  알려진 취약점(SCA의 발견) */
const KNOWLEDGE_NODE_SHAPES = Object.freeze({
  page: "circle",
  ghost: "ring",
  source: "diamond",
  component: "square",
  vulnerability: "triangle",
});

/* 윤곽의 좌표는 화면 픽셀의 소수 둘째 자리까지 쓴다. 점의 자리(`coordinateDigits`, 한
 * 자리)를 그대로 쓰면 가장 작은 점(`--knowledge-node-radius-min` 3px)의 마름모가
 * 반지름 3.76 → 3.8로 반올림되어 넓이가 2.1% 커지고(사각은 3.1%), 「같은 크기 = 같은
 * 넓이」의 허용(2%)을 넘는다. 둘째 자리에서는 세 모양 모두 0.1% 안이다. */
const KNOWLEDGE_SHAPE_DIGITS = 2;

function knowledgePathNumber(value) {
  return value.toFixed(KNOWLEDGE_SHAPE_DIGITS);
}

/* 모양의 기하 — 모양마다 한 줄이고, SVG 손·GL 손·범례가 모두 이 줄을 읽는다: 한 줄을
 * 고치면 세 곳이 함께 바뀌고, 셋이 어긋날 자리가 없다.
 *
 * `reach`는 외접원 반지름 ÷ 같은 넓이의 원의 반지름이다. 크기는 연결의 수를 말하는
 * 채널이라 모양이 그것을 비틀면 안 된다 — 같은 크기의 점이면 모양이 달라도 같은 넓이여야
 * 한다. 그리고 배치·이름표 격자·집기·선의 끝은 점의 반지름을 **점이 차지하는 원**으로
 * 읽으므로, 모든 모양은 그 원에 내접한다. 두 조건을 함께 만족하는 수는 넓이 식에서 한 번에
 * 나온다(외접원 반지름 R, 원의 반지름 r):
 *   사각·마름모  2R² = πr²              → R/r = √(π/2)
 *   정삼각형     (3√3/4)R² = πr²        → R/r = √(4π/(3√3))
 * 고리는 채우지 않지만 그것이 두른 넓이가 원과 같다.
 *
 * `code`는 GL 손의 조각 셰이더가 고르는 거리 함수의 번호이고(원·고리 0, 사각 1,
 * 마름모 2, 삼각 3), `dashed`는 테두리를 점선으로 끊는가다 — 고리의 점선은 GL에서는
 * 이 칸이, SVG에서는 같은 종류의 옷(`.is-ghost`)이 긋고, 두 손의 픽셀 대조가 둘이
 * 같은 그림인지 묻는다. `outline`은 외접원 반지름 R에 내접한 윤곽의 SVG 경로이고,
 * `null`이면 경로가 아니라 `<circle r=R>`이다. */
const KNOWLEDGE_SHAPES = Object.freeze({
  circle: Object.freeze({ reach: 1, code: 0, dashed: false, outline: null }),
  ring: Object.freeze({ reach: 1, code: 0, dashed: true, outline: null }),
  square: Object.freeze({
    reach: Math.sqrt(Math.PI / 2),
    code: 1,
    dashed: false,
    outline: (reach) => {
      const edge = knowledgePathNumber(reach * Math.SQRT1_2);
      const back = knowledgePathNumber(-reach * Math.SQRT1_2);
      return `M${back} ${back}H${edge}V${edge}H${back}Z`;
    },
  }),
  diamond: Object.freeze({
    reach: Math.sqrt(Math.PI / 2),
    code: 2,
    dashed: false,
    outline: (reach) => {
      const tip = knowledgePathNumber(reach);
      const back = knowledgePathNumber(-reach);
      return `M0 ${back}L${tip} 0L0 ${tip}L${back} 0Z`;
    },
  }),
  triangle: Object.freeze({
    reach: Math.sqrt((4 * Math.PI) / (3 * Math.sqrt(3))),
    code: 3,
    dashed: false,
    /* 위를 가리키는 정삼각형 — 꼭짓점이 (0, −R), 밑변이 y = R/2. */
    outline: (reach) => {
      const across = knowledgePathNumber((reach * Math.sqrt(3)) / 2);
      const back = knowledgePathNumber((-reach * Math.sqrt(3)) / 2);
      const base = knowledgePathNumber(reach / 2);
      return `M0 ${knowledgePathNumber(-reach)}L${across} ${base}L${back} ${base}Z`;
    },
  }),
});

/* 가장 멀리 닿는 모양의 외접원 비율 — 범례의 표본 상자가 모든 모양을 담는 폭이다. */
const KNOWLEDGE_SHAPE_REACH_MOST = Math.max(
  ...Object.values(KNOWLEDGE_SHAPES).map((row) => row.reach),
);

/* 종류의 모양 이름. 표에 없는 종류는 페이지의 원으로 선다 — 모르는 것을 모르는 모양으로
 * 그리지 않는다. */
function knowledgeShapeOf(kind) {
  return KNOWLEDGE_NODE_SHAPES[kind] ?? KNOWLEDGE_NODE_SHAPES.page;
}

/* 모양 하나를 SVG 요소 하나로 — 그림의 점과 범례의 표본이 같은 문을 지난다. 요소의
 * 종류(`circle`·`path`)가 모양과 어긋나면 새 요소를 지어 돌려주고, 부르는 쪽이 그것을
 * 제자리에 붙인다. `reach`는 외접원 반지름이다. */
function knowledgeShapeInk(held, shape, reach) {
  const row = KNOWLEDGE_SHAPES[shape];
  const element = row.outline === null ? "circle" : "path";
  const ink = held !== null && held.localName === element
    ? held
    : document.createElementNS(SVG_NS, element);
  writeAttribute(ink, "data-shape", shape);
  if (row.outline === null) writeAttribute(ink, "r", String(reach));
  else writeAttribute(ink, "d", row.outline(reach));
  return ink;
}

/* 점의 크기는 단(tier)이 정한다 (09-16).
 *
 * 군집의 대표 페이지는 `core`, 군집 안에서 연결이 많은 쪽은 `major`, 나머지는
 * 잎이다. 잎만 √차수의 램프를 따르고 위의 둘은 제 크기를 갖는다 — 그래야 「이
 * 덩어리의 중심은 저것」이 배율을 읽지 않고도 보인다(PNP 15쪽의 큰 점·중간 점·
 * 작은 점이 하는 일). 색을 못 보는 눈에도 크기는 남는다. */
function knowledgeNodeRadius(degree, tuning, tier = KNOWLEDGE_TIER_LEAF) {
  if (tier === KNOWLEDGE_TIER_CORE) return tuning.coreRadius;
  if (tier === KNOWLEDGE_TIER_MAJOR) return tuning.majorRadius;
  return Math.min(
    tuning.radiusMax,
    tuning.radiusMin + Math.sqrt(degree) * tuning.radiusDegree,
  );
}

/* 군집마다 한 중심과 몇 개의 대표 지식 (09-16).
 *
 * 중심은 군집 안에서 연결이 가장 많은 페이지(`communities.core`)이고, 대표 지식은
 * 그 다음으로 연결이 많은 몇 쪽이다 — 몇 쪽인가는 멤버 수의 몫과 절대 상한 중
 * 작은 쪽이고, 이웃이 `majorDegree`에 못 미치는 쪽은 아무리 순위가 높아도 대표가
 * 되지 않는다(잎 하나가 군집의 얼굴이 되지 않게).
 *
 * 목차·일지는 어느 단에도 들지 않는다: 차수는 볼트에서 가장 크지만 그것은 주제의
 * 중심이 아니라 목록이다. */
function knowledgeTiers(model, community, core, named, tuning) {
  const count = model.count;
  const tier = new Uint8Array(count);
  if (count === 0) return tier;
  const degree = model.degree;
  const wanted = new Int32Array(named);
  const size = new Int32Array(named);
  for (let at = 0; at < count; at += 1) {
    const rank = community[at];
    if (rank < named && model.kinds[at] === "page") size[rank] += 1;
  }
  for (let rank = 0; rank < named; rank += 1) {
    /* 이름을 얻은 주제에는 대표 지식이 적어도 하나 선다 — 전체 지도의 약속이
     * 「주제와 대표 지식」이므로, 넷짜리 주제가 중심 하나와 잎 셋으로만 서면 그
     * 약속의 절반이 빈다. 자격(차수 `majorDegree`)을 갖춘 후보가 없으면 그래도
     * 서지 않는다: 자리는 비워 둘 수 있어도 지어낼 수는 없다. */
    wanted[rank] = Math.min(tuning.majorMax,
      Math.max(1, Math.ceil(size[rank] * tuning.majorShare)));
  }
  for (let rank = 0; rank < named; rank += 1) {
    if (core[rank] >= 0) tier[core[rank]] = KNOWLEDGE_TIER_CORE;
  }
  /* 후보를 한 번 훑어 차수 내림차순(동률은 열쇠순)으로 세우고, 군집마다 몫만큼
   * 집는다. 정렬은 후보에만 들고 그 수는 페이지 수 이하다. */
  const candidates = [];
  for (let at = 0; at < count; at += 1) {
    const rank = community[at];
    if (rank >= named || model.kinds[at] !== "page") continue;
    if (tier[at] === KNOWLEDGE_TIER_CORE) continue;
    if (degree[at] < KNOWLEDGE_COMMUNITY.majorDegree) continue;
    if (KNOWLEDGE_NAV_PAGES.includes(model.keys[at])) continue;
    candidates.push(at);
  }
  candidates.sort((left, right) => degree[right] - degree[left]
    || (model.keys[left] < model.keys[right] ? -1 : 1));
  const taken = new Int32Array(named);
  for (const at of candidates) {
    const rank = community[at];
    if (taken[rank] >= wanted[rank]) continue;
    taken[rank] += 1;
    tier[at] = KNOWLEDGE_TIER_MAJOR;
  }
  return tier;
}

/* 이름표 상자의 폭을 글자에서 어림한다 (09-16, em 단위).
 *
 * DOM에 물으면 답은 정확하지만 그 물음 하나가 방금 쓴 수천 개의 SVG를 동기
 * 레이아웃으로 재게 한다(이 파일이 이미 두 번 그 값을 치렀다 —
 * `arriveKnowledgeLayers`·`knowledgeViewBox`의 주석). 한글·한자·가나는 한 몸,
 * 로마자와 숫자는 그 절반 남짓으로 세면 이름표 격자를 짜기에 충분하다. */
function knowledgeLabelEm(word, tuning) {
  let em = 0;
  for (const letter of word) {
    const code = letter.codePointAt(0);
    em += code > 0x1100 ? tuning.labelEmWide : tuning.labelEmNarrow;
  }
  return em;
}

/* 판 폭의 네 티어.
 *
 * 문턱은 토큰 셋(`--knowledge-tier-*`)에 한 번만 적혀 있고 그 값은 `knowledgeTuning`
 * 이 판을 지을 때 읽는다. 답을 **클래스**로 두는 것은 컨테이너 쿼리가 제 컨테이너
 * 자신에게는 옷을 입히지 못하기 때문이다 — 이 판은 스스로가 재는 상자이고, 인스펙터를
 * 옆에 둘지 아래에 둘지는 그 상자 자신의 배치다. 뷰포트(`@media`)를 보지 않는 것이
 * 요점이다: 같은 창에서도 판이 반으로 갈리면 이 판의 폭은 절반이고, 그때 옳은 배치는
 * 창의 폭이 아니라 판의 폭이 정한다. */
const KNOWLEDGE_TIERS = Object.freeze(["wide", "middle", "compact", "tiny"]);

function paintKnowledgeTier(view) {
  const wide = view.clientWidth;
  /* 서 있지 않은 판은 폭이 0이고, 0에서 고른 티어는 사람이 보게 될 티어가 아니다 —
   * 들고 있던 답을 그대로 둔다. */
  if (wide === 0) return false;
  const tuning = knowledgeTuning(view);
  const tier = wide >= tuning.tierWide ? "wide"
    : wide >= tuning.tierCompact ? "middle"
      : wide >= tuning.tierTiny ? "compact"
        : "tiny";
  view.classList.toggle("is-toolbar-compact", wide < tuning.toolbarWide);
  if (view.dataset.knowledgeTier === tier) return false;
  view.dataset.knowledgeTier = tier;
  for (const name of KNOWLEDGE_TIERS) view.classList.toggle(`is-tier-${name}`, name === tier);
  /* 티어가 바뀌면 캔버스의 몫도 바뀐다. 사람이 배율을 쥐고 있으면 손대지 않는 것은
   * 에이전트 그래프의 같은 규칙(`agentGraphZoomTaken`) 그대로다. */
  const layout = knowledgeLayouts.get(view);
  // A new or cloned leaf has not painted its topology yet; its final paint
  // performs the initial fit, and resize can refit an existing one. Whether the
  // picture has stood is asked of `paintedModel`, which BOTH hands write — the
  // SVG hand's node and edge arrays stay empty on the GL hand, so asking them
  // left a GL picture unfitted after a tier change (verified 09-16).
  if (layout && layout.paintedModel === layout.model &&
      layout.clusterEls.length === layout.namedCount && !layout.zoomTaken) fitKnowledgeGraph(view, layout);
  return true;
}

/* 다른 표면에서 한 페이지로 — 그래프를 열고 그 점을 고르고 가운데로. 첫 그림은
 * 답이 오기 전일 수 있어(빈 모델) 열쇠는 답이 있는 그림이 읽는다. 검색·태그는
 * 놓는다: 검색은 디밍이고, 고른 점이 흐려 보이면 온 이유가 사라진다. */
function revealKnowledgePage(key, { mode = null } = {}) {
  knowledgeQuery = "";
  knowledgeTagsPicked.clear();
  knowledgeSelectedKey = key;
  knowledgeRevealKey = key;
  /* 「연결 보기」(S2)는 그 페이지의 주변 탐색으로 들어간다 — 모드를 열쇠 옆에 싣고,
   * 답이 있는 첫 그림이 진입 규칙으로 푼다. 모드 없는 청(회상 줄)은 지금의 모드다. */
  knowledgeRevealMode = mode;
  openWorkbenchView("knowledge");
}

/* 회상 줄의 좌석: `term-<n>`이면 그 판이고, 열려 있으면 작업 상황판의 그 카드로
 * 가는 단추(라벨은 그 대화의 이름), 닫혔으면 이름만 남는 꺼진 단추다. 세션 열쇠
 * (판을 모르던 훅)는 갈 곳이 없어 서지 않는다. */
function knowledgeSeatTerm(note) {
  const at = typeof note === "string" && note.startsWith("term-") ? Number(note.slice(5)) : NaN;
  return Number.isInteger(at) ? at : null;
}
function knowledgeSeatNode(note) {
  const term = knowledgeSeatTerm(note);
  if (term === null) return null;
  const tab = tabOfTerm(term);
  const seat = document.createElement("button");
  seat.type = "button";
  seat.className = "knowledge-inspector-row knowledge-seat";
  seat.dataset.knowledgeSeat = String(term);
  seat.textContent = tab ? (paneTitleOf(tab, term) || tabLabel(tab)) : note;
  seat.disabled = !tab;
  seat.dataset.tip = tab ? t("knowledge.openOnBoard", "작업 상황판에서 보기") : t("knowledge.seatGone", "닫힌 판");
  return seat;
}

function openKnowledgeGraph() {
  /* 문을 여는 것은 보겠다는 청이므로 들고 있던 답을 낡은 것으로 표시한다. 다시
   * 읽는 일 자체는 무대의 문(`paintKnowledgeStage`)이 한다 — 여기서 한 번 더
   * 부르면 탭을 여는 몸짓 하나가 볼트를 두 번 훑는다. */
  knowledgeAskedAt = 0;
  /* 문을 여는 것은 「어느 모드로 서는가」를 한 번 묻는 일이기도 하다(S2). */
  knowledgeEntryPending = true;
  openTab({ ...KNOWLEDGE_TAB });
}

/* 사이드바의 문은 볼트가 있을 때만 선다. 없는 볼트의 그래프를 여는 줄은
 * 「설정에서 폴더를 고르세요」 한 문장을 보여 주기 위한 줄이고, 그 문장은
 * 연동 카드가 이미 하고 있다. */
function paintKnowledgeEntry() {
  const row = el("nav-knowledge");
  if (row) row.hidden = secondBrainVault === "";
}

/* 색 칸마다 그라데이션 셋 — 점의 채움(가운데 밝게), 허브의 halo(가운데에서
 * 가장자리로 사라지는 원), 군집의 성운(같은 모양, 더 옅게) — 과 지향성 관계의
 * 화살표. `related`와 `mentions`는 방향을 그리지 않는다. 그라데이션의 불투명도는
 * 숫자가 아니라 `<stop>`의 클래스다: 그 수는 토큰에 산다(`--knowledge-halo-core`,
 * `--knowledge-nebula-core`). 색 칸이 없는 허브(군집이 아닌 점)는 중립 halo를 쓴다. */
function knowledgeDefsHtml() {
  const stop = (offset, color, className) =>
    `<stop offset="${offset}" stop-color="${color}" class="${className}"></stop>`;
  const disc = (id, color, coreClass) => `<radialGradient id="${id}">`
    + stop(0, color, coreClass) + stop(1, color, "knowledge-stop-fade") + `</radialGradient>`;
  const gradients = Array.from({ length: KNOWLEDGE_HUES }, (unused, hue) =>
    `<radialGradient id="knowledge-node-gradient-${hue}">`
      + `<stop offset="0" stop-color="var(--knowledge-hue-${hue}-center)"></stop>`
      + `<stop offset="1" stop-color="var(--knowledge-hue-${hue})"></stop>`
      + `</radialGradient>`
    + disc(`knowledge-halo-gradient-${hue}`, `var(--knowledge-hue-${hue})`, "knowledge-stop-halo-core")
    + disc(`knowledge-nebula-gradient-${hue}`, `var(--knowledge-hue-${hue})`, "knowledge-stop-nebula-core"))
    .join("");
  const neutral = disc("knowledge-halo-gradient-neutral", "var(--ink-figure)", "knowledge-stop-halo-core");
  const arrows = KNOWLEDGE_EDGE_DIRECTED.map((kind) =>
    `<marker id="knowledge-arrow-${kind}" viewBox="0 0 1 1" refX="1" refY="0.5" `
    + `markerUnits="userSpaceOnUse" orient="auto-start-reverse">`
    + `<polygon points="0 0, 1 .5, 0 1" fill="var(--knowledge-typed-${kind.replace("_", "-")})">`
    + `</polygon></marker>`).join("");
  return gradients + neutral + arrows;
}

/* 범례의 모양 표본 — 그림의 점과 같은 기하(`KNOWLEDGE_SHAPES`)를 같은 문
 * (`knowledgeShapeInk`)으로 그린다. 표본은 크기 1의 점이고, 모양마다 넓이가 같으므로
 * 가장 멀리 닿는 모양(`KNOWLEDGE_SHAPE_REACH_MOST`)이 표본 상자를 정한다. */
function knowledgeLegendShape(kind, shape) {
  const box = KNOWLEDGE_SHAPE_REACH_MOST;
  const mark = document.createElementNS(SVG_NS, "svg");
  mark.setAttribute("class", `knowledge-legend-node is-${kind}`);
  mark.setAttribute("viewBox", `${-box} ${-box} ${box * 2} ${box * 2}`);
  mark.setAttribute("aria-hidden", "true");
  const ink = knowledgeShapeInk(null, shape, KNOWLEDGE_SHAPES[shape].reach);
  ink.setAttribute("class", "knowledge-legend-ink");
  mark.appendChild(ink);
  return mark;
}

function buildKnowledgeView() {
  const root = document.createElement("section");
  root.className = "file-view knowledge-view";
  // 첫 leaf의 사본이 이 이름에 답한다 — 마크업이 선언한 판들과 같게, 그리고
  // `docHost`가 나머지 사본에서 떼어 낸다.
  root.id = "knowledge-view";
  root.hidden = true;

  const head = document.createElement("header");
  head.className = "knowledge-head";
  const name = document.createElement("span");
  name.className = "knowledge-name";
  name.dataset.i18n = "knowledge.title";
  name.textContent = t("knowledge.title", "지식 그래프");

  /* 「전체 지도 | 주변 탐색」(t-4140). 눌린 쪽이 지금의 모드이고 `aria-pressed`가
   * 그것을 말한다 — 색만으로 말하지 않는다. 낱말은 표(`KNOWLEDGE_MODES`)의 것. */
  const modes = document.createElement("div");
  modes.className = "knowledge-mode";
  modes.setAttribute("role", "group");
  modes.dataset.i18nAria = "knowledge.modes";
  modes.setAttribute("aria-label", t("knowledge.modes", "탐색 모드"));
  for (const row of KNOWLEDGE_MODES) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn knowledge-mode-step";
    one.dataset.knowledgeMode = row.id;
    one.setAttribute("aria-pressed", "false");
    /* 좁은 판은 낱말을 접고 그림만 남기므로 이름은 따로 든다. */
    one.dataset.i18nAria = row.key;
    one.setAttribute("aria-label", t(row.key, row.word));
    one.innerHTML = icon(row.glyph);
    const word = document.createElement("span");
    word.dataset.i18n = row.key;
    word.textContent = t(row.key, row.word);
    one.appendChild(word);
    modes.appendChild(one);
  }

  const searchBox = document.createElement("div");
  searchBox.className = "knowledge-search";
  const find = document.createElement("input");
  find.className = "settings-input knowledge-query";
  find.type = "search";
  find.dataset.i18nPlaceholder = "knowledge.search";
  find.placeholder = t("knowledge.search", "제목·태그 검색");
  find.dataset.i18nAria = "knowledge.search";
  find.setAttribute("aria-label", t("knowledge.search", "제목·태그 검색"));
  /* 후보 목록을 든 입력은 콤보박스다(S7): 열림은 `aria-expanded`가 말한다(`setKnowledgeSuggesting`). */
  find.setAttribute("role", "combobox");
  find.setAttribute("aria-autocomplete", "list");
  find.setAttribute("aria-expanded", "false");

  const searchSuggest = document.createElement("div");
  searchSuggest.className = "knowledge-search-suggest";
  searchSuggest.setAttribute("aria-label", t("knowledge.searchSuggest", "검색 토큰"));

  /* 검색 후보(t-4140 S3): 입력 중에 제목·폴더·관계 힌트를 든 줄이 상한만큼 선다. 진짜
   * 리스트박스라 ↑↓가 옮기고(`aria-selected`) Enter가 고른다; 줄마다 「주변 탐색으로」.
   * 토큰 칩과 저장 문구는 그 아래 그대로다. */
  const results = document.createElement("ul");
  results.className = "knowledge-search-results";
  results.setAttribute("role", "listbox");
  results.dataset.i18nAria = "knowledge.candidates";
  results.setAttribute("aria-label", t("knowledge.candidates", "검색 후보"));
  const noCandidates = document.createElement("p");
  noCandidates.className = "knowledge-candidates-empty";
  noCandidates.hidden = true;
  noCandidates.dataset.i18n = "knowledge.noCandidates";
  noCandidates.textContent = t("knowledge.noCandidates", "일치하는 페이지가 없습니다");

  const tokenChipsBox = document.createElement("div");
  tokenChipsBox.className = "knowledge-search-tokens";
  for (const tok of ["tag:", "rel:", "since:", "kind:", "hub", "sev:"]) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "btn knowledge-token-chip";
    chip.dataset.token = tok;
    chip.textContent = tok;
    /* `sev:`은 공급망의 낱말이라 그 렌즈가 켜졌을 때만 선다(`paintKnowledgeHead`). */
    chip.hidden = tok === "sev:";
    tokenChipsBox.appendChild(chip);
  }

  const savedPhrasesBox = document.createElement("div");
  savedPhrasesBox.className = "knowledge-saved-phrases";
  const savedPhrasesHead = document.createElement("div");
  savedPhrasesHead.className = "knowledge-saved-phrases-head";
  const phrasesTitle = document.createElement("span");
  phrasesTitle.dataset.i18n = "knowledge.savedPhrases";
  phrasesTitle.textContent = t("knowledge.savedPhrases", "저장된 검색어");
  const savePhraseBtn = document.createElement("button");
  savePhraseBtn.type = "button";
  savePhraseBtn.className = "btn knowledge-save-phrase";
  /* ★는 낱말 밖에 선다 — 키를 든 자리에 함께 적으면 `applyLocale`이 그 자리를 키의
   * 낱말로 갈아 쓰며 ★를 떨군다. */
  const savePhraseWord = document.createElement("span");
  savePhraseWord.dataset.i18n = "knowledge.savePhrase";
  savePhraseWord.textContent = t("knowledge.savePhrase", "저장");
  savePhraseBtn.append("★ ", savePhraseWord);
  savedPhrasesHead.append(phrasesTitle, savePhraseBtn);

  const phrasesList = document.createElement("div");
  phrasesList.className = "knowledge-saved-phrases-list";

  savedPhrasesBox.append(savedPhrasesHead, phrasesList);
  searchSuggest.append(results, noCandidates, tokenChipsBox, savedPhrasesBox);
  searchBox.append(find, searchSuggest);

  const sift = document.createElement("div");
  sift.className = "knowledge-sift";
  /* 낱말은 행이 들고 다니고 `t`가 갈아입힌다 — 테마 픽커가 제 이름을 그렇게
   * 싣는 것과 같은 문법(`{ code, key, name }`). 지어지는 판은 로드 시점의
   * 낱말을 굽고, `docHost`가 붙일 때 `applyLocale`이 키를 다시 읽는다. */
  for (const row of [
    { flag: "orphans", key: "knowledge.orphansOnly", word: "고아 페이지만",
      short: { key: "knowledge.orphansShort", word: "고아" }, glyph: "circle" },
    { flag: "ghosts", key: "knowledge.ghostsOnly", word: "유령 링크만",
      short: { key: "knowledge.ghostsShort", word: "유령" }, glyph: "circle-dashed" },
    { flag: "typed", key: "knowledge.typedOnly", word: "타입 관계만",
      short: { key: "knowledge.typedShort", word: "타입" }, glyph: "link" },
    { flag: "sources", key: "knowledge.showSources", word: "원본도",
      short: { key: "knowledge.sourcesShort", word: "원본" }, glyph: "file" },
    /* 라이브 층의 둘(t-2931). 「살아 있는 것만」은 창 안에서 회상되거나 바뀐
     * 페이지, 「합칠 후보만」은 백엔드가 센 쌍의 양 끝. 답에 라이브 층이 없으면
     * (옛 백엔드) 둘 다 서지 않는다. */
    { flag: "alive", key: "knowledge.aliveOnly", word: "살아 있는 것만",
      short: { key: "knowledge.aliveShort", word: "활동" }, glyph: "activity" },
    { flag: "merge", key: "knowledge.mergeOnly", word: "합칠 후보만",
      short: { key: "knowledge.mergeShort", word: "합칠?" }, glyph: "waypoints" },
    /* 공급망(P4): 활성 워크스페이스의 구성요소 ■와 알려진 취약점 ▲를 볼트 곁에 세운다. 답에 없는 점을
     * 만드는 렌즈라 켜면 백엔드에 묻는다(`toggleKnowledgeSupply`). 기본은 꺼짐이다. */
    { flag: "supply", key: "knowledge.supplyLens", word: "공급망 — 구성요소와 취약점",
      short: { key: "knowledge.supplyShort", word: "공급망" }, glyph: "shield" },
  ]) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn knowledge-flag";
    one.dataset.knowledgeFlag = row.flag;
    one.dataset.i18nAria = row.key;
    one.setAttribute("aria-label", t(row.key, row.word));
    one.innerHTML = icon(row.glyph);
    const short = document.createElement("span");
    short.dataset.i18n = row.short.key;
    short.textContent = t(row.short.key, row.short.word);
    one.appendChild(short);
    one.setAttribute("aria-pressed", "false");
    sift.appendChild(one);
  }
  /* 표시 설정(t-4140 S6)은 렌즈가 아니라 그림의 설정이다 — 목차·일지의 점과 선을 그림에서
   * 접는다. 눌려 있음이 「보임」이고, 기본이 눌림이다. 시안의 자리대로 머리에 제 단추로
   * 선다; 좁은 판은 낱말을 접고 그림만 남긴다(이름은 `aria-label`이 든다). */
  const navToggle = document.createElement("button");
  navToggle.type = "button";
  navToggle.className = "btn knowledge-flag knowledge-nav-toggle";
  navToggle.dataset.knowledgeFlag = "nav";
  navToggle.dataset.i18nAria = "knowledge.navPages";
  navToggle.setAttribute("aria-label", t("knowledge.navPages", "목차·일지 표시"));
  navToggle.setAttribute("aria-pressed", "false");
  navToggle.innerHTML = icon("list");
  const navWord = document.createElement("span");
  navWord.dataset.i18n = "knowledge.navPages";
  navWord.textContent = t("knowledge.navPages", "목차·일지 표시");
  navToggle.appendChild(navWord);
  const tags = document.createElement("div");
  tags.className = "knowledge-tags";
  const chips = document.createElement("div");
  chips.className = "knowledge-chips";
  chips.dataset.i18nAria = "knowledge.tags";
  chips.setAttribute("aria-label", t("knowledge.tags", "태그"));
  const moreTagsToggle = document.createElement("button");
  moreTagsToggle.type = "button";
  moreTagsToggle.className = "btn knowledge-tags-more-toggle";
  moreTagsToggle.setAttribute("aria-expanded", "false");
  const moreTags = document.createElement("div");
  moreTags.className = "knowledge-tags-popover";
  tags.append(chips, moreTagsToggle, moreTags);
  /* 「보기」(시안 이식): 렌즈·태그·시간 슬라이서·저장 시야·다시 읽기는 한 메뉴 뒤에 산다.
   * 머리에 남는 것은 모드·검색·목차 표시·상태뿐이라 첫 화면이 시안처럼 읽힌다 — 접힌
   * 자리는 팝오버이지 쓰레기통이 아니다(어느 티어에서도 같은 문). */
  const lensToggle = document.createElement("button");
  lensToggle.type = "button";
  lensToggle.className = "btn knowledge-lens-toggle";
  lensToggle.dataset.i18nAria = "knowledge.viewMenu";
  lensToggle.setAttribute("aria-label", t("knowledge.viewMenu", "보기"));
  lensToggle.innerHTML = `${icon("sliders")}<span data-i18n="knowledge.viewMenu">${t("knowledge.viewMenu", "보기")}</span>`;
  lensToggle.setAttribute("aria-expanded", "false");
  const lenses = document.createElement("div");
  lenses.className = "knowledge-lens-popover";
  lenses.append(sift, tags);

  const gap = document.createElement("span");
  gap.className = "knowledge-gap";
  const stat = document.createElement("span");
  stat.className = "knowledge-stat";
  stat.setAttribute("role", "status");
  stat.setAttribute("aria-live", "polite");
  const statFull = document.createElement("span");
  statFull.className = "knowledge-stat-full";
  const statCompact = document.createElement("span");
  statCompact.className = "knowledge-stat-compact";
  stat.append(statFull, statCompact);
  const again = document.createElement("button");
  again.type = "button";
  again.className = "btn knowledge-refresh";
  again.dataset.i18nAria = "knowledge.refresh";
  again.setAttribute("aria-label", t("knowledge.refresh", "다시 읽기"));
  again.dataset.tip = t("knowledge.refresh", "다시 읽기");
  again.innerHTML = icon("refresh");

  const slicer = document.createElement("div");
  slicer.className = "knowledge-slicer";
  const slicerToggle = document.createElement("button");
  slicerToggle.type = "button";
  slicerToggle.className = "btn knowledge-slicer-toggle";
  slicerToggle.setAttribute("aria-expanded", "false");
  slicerToggle.dataset.i18nAria = "knowledge.slicer";
  slicerToggle.setAttribute("aria-label", t("knowledge.slicer", "시간 슬라이서"));
  slicerToggle.innerHTML = `${icon("clock")}<span class="knowledge-slicer-label" data-i18n="knowledge.slicerAll">${t("knowledge.slicerAll", "전체")}</span>`;

  const slicerPopover = document.createElement("div");
  slicerPopover.className = "knowledge-slicer-popover";
  const slicerHead = document.createElement("div");
  slicerHead.className = "knowledge-slicer-head";
  const slicerTitle = document.createElement("span");
  slicerTitle.dataset.i18n = "knowledge.slicer";
  slicerTitle.textContent = t("knowledge.slicer", "시간 슬라이서");
  const slicerVal = document.createElement("span");
  slicerVal.className = "knowledge-slicer-value";
  slicerVal.dataset.i18n = "knowledge.slicerAll";
  slicerVal.textContent = t("knowledge.slicerAll", "전체");
  slicerHead.append(slicerTitle, slicerVal);

  const slicerSlider = document.createElement("input");
  slicerSlider.type = "range";
  slicerSlider.className = "knowledge-slicer-slider";
  slicerSlider.min = "0";
  slicerSlider.max = "100";
  slicerSlider.value = "0";
  slicerSlider.dataset.i18nAria = "knowledge.slicer";
  slicerSlider.setAttribute("aria-label", t("knowledge.slicer", "시간 슬라이서"));

  const slicerPresets = document.createElement("div");
  slicerPresets.className = "knowledge-slicer-presets";
  for (const preset of KNOWLEDGE_SLICER_PRESETS) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `btn knowledge-slicer-preset${preset.id === "all" ? " is-active" : ""}`;
    btn.dataset.preset = preset.id;
    btn.dataset.i18n = preset.key;
    btn.textContent = t(preset.key, preset.word);
    slicerPresets.appendChild(btn);
  }
  slicerPopover.append(slicerHead, slicerSlider, slicerPresets);
  slicer.append(slicerToggle, slicerPopover);

  const scene = document.createElement("div");
  scene.className = "knowledge-scene";
  const sceneToggle = document.createElement("button");
  sceneToggle.type = "button";
  sceneToggle.className = "btn knowledge-scene-toggle";
  sceneToggle.setAttribute("aria-expanded", "false");
  sceneToggle.dataset.i18nAria = "knowledge.scenes";
  sceneToggle.setAttribute("aria-label", t("knowledge.scenes", "저장 시야"));
  sceneToggle.textContent = "★";

  const scenePopover = document.createElement("div");
  scenePopover.className = "knowledge-scene-popover";
  const sceneHead = document.createElement("div");
  sceneHead.className = "knowledge-scene-head";
  const sceneTitle = document.createElement("span");
  sceneTitle.dataset.i18n = "knowledge.scenes";
  sceneTitle.textContent = t("knowledge.scenes", "저장 시야");
  sceneHead.appendChild(sceneTitle);

  const sceneCreate = document.createElement("div");
  sceneCreate.className = "knowledge-scene-create";
  const sceneInput = document.createElement("input");
  sceneInput.type = "text";
  sceneInput.className = "settings-input knowledge-scene-input";
  sceneInput.dataset.i18nPlaceholder = "knowledge.sceneNamePlaceholder";
  sceneInput.placeholder = t("knowledge.sceneNamePlaceholder", "시야 이름");
  sceneInput.dataset.i18nAria = "knowledge.sceneNamePlaceholder";
  sceneInput.setAttribute("aria-label", t("knowledge.sceneNamePlaceholder", "시야 이름"));

  const sceneSave = document.createElement("button");
  sceneSave.type = "button";
  sceneSave.className = "btn knowledge-scene-save";
  sceneSave.dataset.i18n = "knowledge.sceneSave";
  sceneSave.textContent = t("knowledge.sceneSave", "저장");
  sceneCreate.append(sceneInput, sceneSave);

  const sceneList = document.createElement("div");
  sceneList.className = "knowledge-scene-list";

  scenePopover.append(sceneHead, sceneCreate, sceneList);
  scene.append(sceneToggle, scenePopover);

  const tools = document.createElement("div");
  tools.className = "knowledge-view-tools";
  tools.append(slicer, scene, again);
  lenses.appendChild(tools);
  head.append(name, modes, searchBox, navToggle, gap, stat, lensToggle, lenses);

  const body = document.createElement("div");
  body.className = "knowledge-body";
  const surface = document.createElement("div");
  surface.className = "knowledge-surface";
  const canvas = document.createElement("div");
  canvas.className = "knowledge-canvas";
  canvas.tabIndex = 0;
  // 격자 하나에 천 개의 tab 정지점을 두지 않는다: 판 전체가 하나의 위젯이고,
  // 화살표가 점 사이를 걷는다(`knowledgeWalk`).
  canvas.setAttribute("role", "application");
  canvas.dataset.i18nAria = "knowledge.canvas";
  canvas.setAttribute("aria-label", t("knowledge.canvas", "지식 그래프 캔버스"));
  // 점과 선은 **한** <svg> 안에 산다. 둘을 두 <svg>로 나누면 viewBox가 두 벌이
  // 되고, 그 둘이 한 프레임이라도 어긋나면 선이 점에서 떨어진 그림이 된다.
  const picture = document.createElementNS(SVG_NS, "svg");
  picture.setAttribute("class", "knowledge-picture");
  picture.setAttribute("preserveAspectRatio", "xMidYMid meet");
  const defs = document.createElementNS(SVG_NS, "defs");
  defs.innerHTML = knowledgeDefsHtml();
  // 군집의 층이 맨 뒤에 선다: 성운과 이름표는 점과 선 **뒤**의 바탕이지 그 위의
  // 장식이 아니다 — 선이 성운을 가로지르고 점이 성운 위에 앉는다.
  const clusters = document.createElementNS(SVG_NS, "g");
  clusters.setAttribute("class", "knowledge-clusters");
  const edges = document.createElementNS(SVG_NS, "g");
  edges.setAttribute("class", "knowledge-edges");
  const edgeLabels = document.createElementNS(SVG_NS, "g");
  edgeLabels.setAttribute("class", "knowledge-edge-labels");
  const nodes = document.createElementNS(SVG_NS, "g");
  nodes.setAttribute("class", "knowledge-nodes");
  picture.append(defs, clusters, edges, edgeLabels, nodes);
  canvas.append(picture);

  /* 배율의 세 단추. 낱말은 에이전트 그래프의 것을 그대로 쓴다 — 같은 동작에
   * 두 벌의 번역을 두면 한쪽만 고쳐지는 날이 온다. */
  const zoom = document.createElement("div");
  zoom.className = "knowledge-zoom";
  for (const row of [
    { what: "out", key: "board.graph.zoomOut", word: "축소", glyph: "minus" },
    { what: "in", key: "board.graph.zoomIn", word: "확대", glyph: "plus" },
    { what: "fit", key: "board.graph.fit", word: "전체 보기", glyph: "square" },
  ]) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = `btn knowledge-zoom-${row.what}`;
    one.dataset.i18nAria = row.key;
    one.dataset.i18nTitle = row.key;
    one.setAttribute("aria-label", t(row.key, row.word));
    // 툴팁은 이 창의 계약대로 `dataset.tip`이다 — 네이티브 title은 잡음이다.
    one.dataset.tip = t(row.key, row.word);
    one.innerHTML = icon(row.glyph);
    zoom.appendChild(one);
  }
  const scale = document.createElement("span");
  scale.className = "knowledge-zoom-label";
  /* 시안의 순서: − 100% + ⌗ — 배율의 수가 두 단추 사이에 선다. */
  zoom.insertBefore(scale, zoom.children[1] ?? null);

  const legendToggle = document.createElement("button");
  legendToggle.type = "button";
  legendToggle.className = "btn agent-graph-legend-toggle knowledge-legend-toggle";
  legendToggle.dataset.i18nAria = "knowledge.legend";
  legendToggle.dataset.i18nTitle = "knowledge.legend";
  legendToggle.setAttribute("aria-label", t("knowledge.legend", "범례"));
  legendToggle.dataset.tip = t("knowledge.legend", "범례");
  legendToggle.textContent = "?";
  legendToggle.setAttribute("aria-expanded", "false");
  /* 범례의 「?」는 배율 상자의 끝에 선다(시안의 오른쪽 아래) — 모든 티어에서 접힌 범례를 연다. */
  zoom.appendChild(legendToggle);

  const legend = document.createElement("section");
  legend.className = "agent-graph-legend knowledge-legend";
  const legendHead = document.createElement("p");
  legendHead.className = "agent-graph-legend-head";
  legendHead.dataset.i18n = "knowledge.legend";
  legendHead.textContent = t("knowledge.legend", "범례");
  const legendList = document.createElement("ul");
  legendList.className = "agent-graph-legend-list knowledge-legend-list";
  for (const row of [
    { kind: "page", key: "knowledge.nodePage", word: "페이지" },
    { kind: "ghost", key: "knowledge.nodeGhost", word: "유령" },
    { kind: "source", key: "knowledge.nodeSource", word: "원본" },
    /* 회상은 점의 종류가 아니라 페이지를 두른 점선 고리다(`.knowledge-halo`, GL의 고리
     * 패스) — 그래서 모양을 종류에게 묻지 않고 제 이름으로 든다. */
    { kind: "recalled", shape: "ring", key: "knowledge.nodeRecalled", word: "회상됨" },
    /* 공급망(P4)의 줄은 렌즈가 켜졌을 때만 선다(`supply`) — 모양 둘, 생태계 둘, 멤버의 테두리,
     * 심각도 여섯, 정보성 권고. 표본은 그림의 점과 같은 낱말(`words`)을 들어 같은 잉크를 입는다. */
    { kind: "component", key: "knowledge.nodeComponent", word: "구성요소", supply: true },
    ...KNOWLEDGE_SUPPLY_ECOSYSTEMS.map((one) => ({ kind: "component", label: one.registry, supply: true,
      words: { ecosystem: one.id, member: "false" } })),
    { kind: "component", key: "knowledge.nodeMember", word: "워크스페이스 멤버", supply: true,
      words: { ecosystem: KNOWLEDGE_SUPPLY_ECOSYSTEMS[0].id, member: "true" } },
    { kind: "vulnerability", key: "knowledge.nodeVulnerability", word: "취약점", supply: true },
    ...KNOWLEDGE_SUPPLY_SEVERITIES.map((one) => ({ kind: "vulnerability", key: one.key, word: one.word, supply: true,
      words: { severity: one.id } })),
    { kind: "vulnerability", key: "knowledge.supplyInformational", word: "정보성 권고", supply: true,
      words: { severity: "unknown", informational: "unmaintained" } },
  ]) {
    const item = document.createElement("li");
    item.dataset.nodeKind = row.kind;
    const mark = knowledgeLegendShape(row.kind, row.shape ?? knowledgeShapeOf(row.kind));
    for (const [name, value] of Object.entries(row.words ?? {})) mark.dataset[name] = value;
    const word = document.createElement("span");
    if (row.key === undefined) {
      word.textContent = row.label;
    } else {
      word.dataset.i18n = row.key;
      word.textContent = t(row.key, row.word);
    }
    if (row.supply) {
      item.dataset.legendSupply = "true";
      item.hidden = true;
    }
    item.append(mark, word);
    legendList.appendChild(item);
  }
  for (const row of [
    { kind: "mentions", key: "knowledge.edgeMentions", word: "mentions" },
    { kind: "related", key: "knowledge.edgeRelated", word: "related" },
    { kind: "implements", key: "knowledge.edgeImplements", word: "implements" },
    { kind: "depends_on", key: "knowledge.edgeDependsOn", word: "depends_on" },
    { kind: "supersedes", key: "knowledge.edgeSupersedes", word: "supersedes" },
    { kind: "contradicts", key: "knowledge.edgeContradicts", word: "contradicts" },
    { kind: "merge", key: "knowledge.edgeMerge", word: "merge?" },
    { kind: "affects", key: "knowledge.edgeAffects", word: "affects", supply: true },
  ]) {
    const item = document.createElement("li");
    item.dataset.edgeKind = row.kind;
    if (row.supply) {
      item.dataset.legendSupply = "true";
      item.hidden = true;
    }
    const mark = document.createElement("span");
    mark.className = `knowledge-legend-edge kind-${row.kind}`;
    const word = document.createElement("span");
    word.dataset.i18n = row.key;
    word.textContent = t(row.key, row.word);
    /* 키 곁의 한 줄이 그 키의 뜻이다 — 범례가 낱말만 늘어놓으면, 선의 뜻을 아는
     * 사람만 그림을 읽는다. */
    const why = document.createElement("span");
    why.className = "knowledge-legend-why";
    why.dataset.i18n = KNOWLEDGE_EDGE_WHY[row.kind].key;
    why.textContent = knowledgeEdgeWhy(row.kind);
    item.append(mark, word, why);
    legendList.appendChild(item);
  }
  legend.append(legendHead, legendList);

  const inspector = document.createElement("aside");
  inspector.className = "knowledge-inspector";
  inspector.setAttribute("role", "status");
  inspector.setAttribute("aria-live", "polite");
  /* 두 탭(t-4140 S4): 「페이지」(개요 또는 카드)와 「활동」(BUS LOG). 활동 띠는 늘 새 줄이
   * 오는 자리라 개요 안에 두면 상시 스크롤이 탐색을 방해했다. 탭의 옷은 에이전트
   * 인스펙터의 것을 그대로 입는다(`agent-inspector-tabs`) — 같은 위젯에 두 벌의 옷을
   * 두지 않는다. */
  const tabs = document.createElement("div");
  tabs.className = "agent-inspector-tabs knowledge-inspector-tabs";
  tabs.setAttribute("role", "tablist");
  tabs.dataset.i18nAria = "knowledge.inspectorTabs";
  tabs.setAttribute("aria-label", t("knowledge.inspectorTabs", "패널 탭"));
  for (const row of KNOWLEDGE_INSPECTOR_TABS) {
    const one = document.createElement("button");
    one.type = "button";
    one.setAttribute("role", "tab");
    one.dataset.knowledgeTab = row.id;
    one.setAttribute("aria-selected", "false");
    one.dataset.i18n = row.key;
    one.textContent = t(row.key, row.word);
    tabs.appendChild(one);
  }
  const pagePanel = document.createElement("div");
  pagePanel.className = "knowledge-inspector-page";
  pagePanel.setAttribute("role", "tabpanel");
  pagePanel.append(buildKnowledgeOverview(), buildKnowledgeCard());
  inspector.append(tabs, pagePanel, buildKnowledgeActivity());

  const empty = document.createElement("div");
  empty.className = "knowledge-empty";
  empty.hidden = true;
  const said = document.createElement("p");
  said.className = "knowledge-empty-word";
  said.dataset.i18n = "knowledge.emptyWord";
  said.textContent = t(
    "knowledge.emptyWord",
    "아직 지식 페이지가 없습니다. raw/에 기사나 메모를 던져 두고 에이전트에게 «취합해»라고 말하면 여기에 그림이 자랍니다.",
  );
  const act = document.createElement("button");
  act.type = "button";
  act.className = "btn btn--primary knowledge-empty-act";
  empty.append(said, act);

  const trouble = document.createElement("p");
  trouble.className = "knowledge-error";
  trouble.setAttribute("role", "alert");
  trouble.hidden = true;

  /* 떠 있는 것들은 **캔버스 위**에 뜬다. 배율 상자와 범례를 한 상자의 두 자식으로
   * 두면 `absolute`가 재는 바닥이 캔버스가 아니라 범례까지 포함한 상자가 되고,
   * 그때 배율 상자는 그림이 아니라 범례를 덮는다 — 실측: 그 판에서 노드 3종의
   * 표시가 상자 뒤로 통째로 사라졌다. */
  /* 빵부스러기(t-4140): 주변 탐색의 「어디에 있는가」. ← 이전은 방문 이력을 되짚고,
   * 「전체 지도」는 뿌리로 돌아가는 문이며, 지금의 중심이 `aria-current`다. 전체
   * 지도에서는 서지 않는다 — 그때의 자리는 툴바의 토글이 이미 말한다. */
  const crumb = document.createElement("nav");
  crumb.className = "knowledge-crumb";
  crumb.hidden = true;
  crumb.dataset.i18nAria = "knowledge.crumb";
  crumb.setAttribute("aria-label", t("knowledge.crumb", "탐색 위치"));
  const back = document.createElement("button");
  back.type = "button";
  back.className = "btn knowledge-crumb-back";
  back.disabled = true;
  back.dataset.i18nAria = "knowledge.back";
  back.setAttribute("aria-label", t("knowledge.back", "이전 탐색"));
  back.dataset.tip = t("knowledge.back", "이전 탐색");
  back.innerHTML = icon("arrow-left");
  const crumbRoot = document.createElement("button");
  crumbRoot.type = "button";
  crumbRoot.className = "btn knowledge-crumb-root";
  crumbRoot.dataset.i18n = KNOWLEDGE_MODES[0].key;
  crumbRoot.textContent = t(KNOWLEDGE_MODES[0].key, KNOWLEDGE_MODES[0].word);
  const sep = document.createElement("span");
  sep.className = "knowledge-crumb-sep";
  sep.setAttribute("aria-hidden", "true");
  sep.textContent = "›";
  const here = document.createElement("span");
  here.className = "knowledge-crumb-here";
  here.setAttribute("aria-current", "location");
  const crumbCount = document.createElement("span");
  crumbCount.className = "knowledge-crumb-count";
  crumb.append(back, crumbRoot, sep, here);

  /* 연결 깊이(시안 이식): 빵부스러기 아래 한 줄 — 낱말, 세 단, 그려진 페이지 수. 주변
   * 탐색의 깊이는 그려지는 반경이라 캔버스 위에 서고, 전체 지도에서는 빵부스러기와 함께
   * 접힌다. 세그먼트는 진짜 라디오 그룹이다 — 셋 중 하나라는 것이 이 위젯의 뜻이고,
   * 낭독기가 그 뜻을 읽을 수 있어야 한다(§8). 단의 낱말은 그릴 때 쓴다(`paintKnowledgeMode`). */
  const depths = document.createElement("div");
  depths.className = "knowledge-depth";
  depths.hidden = true;
  depths.setAttribute("role", "radiogroup");
  depths.dataset.i18nAria = "knowledge.focusDepth";
  depths.setAttribute("aria-label", t("knowledge.focusDepth", "포커스 깊이"));
  const depthWord = document.createElement("span");
  depthWord.className = "knowledge-depth-word";
  depthWord.dataset.i18n = "knowledge.depthWord";
  depthWord.textContent = t("knowledge.depthWord", "연결 깊이");
  depths.appendChild(depthWord);
  for (const depth of KNOWLEDGE_FOCUS_DEPTHS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn knowledge-depth-step";
    one.dataset.knowledgeDepth = String(depth);
    one.setAttribute("role", "radio");
    one.setAttribute("aria-checked", "false");
    depths.appendChild(one);
  }
  depths.appendChild(crumbCount);

  /* 군집 범례(시안 이식): 왼쪽 아래의 점과 이름. 지금 그려진 점이 속한 이름 있는 군집만
   * 서고, 누르면 그 군집을 밝힌다 — 카드의 군집 줄과 같은 손(`data-knowledge-cluster`). */
  const clusterLegend = document.createElement("div");
  clusterLegend.className = "knowledge-cluster-legend";
  clusterLegend.hidden = true;
  clusterLegend.setAttribute("role", "group");
  clusterLegend.dataset.i18nAria = "knowledge.clusters";
  clusterLegend.setAttribute("aria-label", t("knowledge.clusters", "군집"));

  const stage = document.createElement("div");
  stage.className = "knowledge-stage";
  stage.append(canvas, crumb, depths, clusterLegend, zoom);
  surface.append(stage, legend);
  body.append(surface, inspector, empty, trouble);
  root.append(head, body);
  return root;
}

/* 볼트 건강의 줄 — 카파시의 lint 목록을 백엔드의 표(`graph.lint`) 그대로. 창은
 * 아무것도 세지 않는다: `zerocode vault-lint`가 찍는 표와 이 카드가 읽는 표는
 * `zerocode_core::second_brain_lint` 한 producer의 것이다. `door`는 줄을 누르면
 * 무엇이 되는가 — 렌즈(그 목록의 페이지만 남김), 관계 낱말 검색(디밍만),
 * 아니면 없음(raw 항목은 그림에 점이 없다). `count`는 표의 어느 열인가. */
const KNOWLEDGE_HEALTH_ROWS = [
  { lint: "orphans", count: "orphans", key: "knowledge.lintOrphans", word: "고아 페이지", door: "lens" },
  { lint: "ghosts", count: "ghost_links", key: "knowledge.lintGhosts", word: "없는 페이지로의 링크", door: "lens" },
  { lint: "index_gaps", count: "index_gaps", key: "knowledge.lintIndexGaps", word: "색인에 없는 페이지", door: "lens" },
  { lint: "missing_frontmatter", count: "missing_frontmatter", key: "knowledge.lintMissingFrontmatter", word: "source·ingested_at 없는 페이지", door: "lens" },
  { lint: "undeclared_relations", count: "undeclared_relations", key: "knowledge.lintUndeclaredRelations", word: "관계 키 없는 본문 링크", door: "lens" },
  { lint: "unlogged_raw", count: "unlogged_raw", key: "knowledge.lintUnloggedRaw", word: "취합 안 된 raw 항목", door: "none" },
  { lint: "contradictions", count: "contradictions", key: "knowledge.lintContradictions", word: "모순", door: "search" },
  { lint: "superseded", count: "superseded", key: "knowledge.lintSuperseded", word: "대체된 페이지", door: "search" },
  /* 라이브 층의 셋(t-2931): 오늘 회상된 페이지·한 번도 회상되지 않은 페이지·
   * merge 후보 쌍. `live`인 줄의 수는 표의 열이 아니라 라이브 층이 채우고
   * (`knowledgeModel`), 위상 서명에는 싣지 않는다. 누르면 각각 활동 렌즈·
   * 냉각 렌즈·dedupe 렌즈다. */
  { lint: "recalledToday", count: "recalled_today", key: "knowledge.lintRecalledToday", word: "오늘 회상된 페이지", door: "live", live: true },
  { lint: "neverRecalled", count: "never_recalled", key: "knowledge.lintNeverRecalled", word: "회상된 적 없는 페이지", door: "live", live: true },
  { lint: "merge", count: "merge_candidates", key: "knowledge.lintMerge", word: "합칠 후보", door: "live", live: true },
];

/* 표의 한 줄이 이름 짓는 페이지 id들 — 문자열 목록이거나 `{ page }` 행 목록이다. */
function knowledgeLintPages(rows) {
  return new Set((Array.isArray(rows) ? rows : [])
    .map((row) => (typeof row === "string" ? row : row?.page))
    .filter((id) => typeof id === "string"));
}

/* 목록 한 칸: 제목 하나와 그 아래의 <ul>. 개요의 세 칸과 페이지 카드의 두 열이
 * 같은 뼈대를 쓰는 것은 같은 것이기 때문이다 — 누를 수 있는 이름들의 목록. */
function knowledgeListSection(className, key, word) {
  const box = document.createElement("section");
  box.className = className;
  const head = document.createElement("h4");
  head.className = "knowledge-inspector-head";
  head.dataset.i18n = key;
  // 낱말은 부르는 쪽이 `t()`로 이미 골라 왔다 — 카탈로그 검사가 그 자리를 본다.
  head.textContent = word;
  const list = document.createElement("ul");
  list.className = "knowledge-inspector-list";
  box.append(head, list);
  return box;
}

/* 목록의 한 줄. 이름은 진짜 <button>이고(§8), 오른쪽의 작은 글자는 그 줄이 왜
 * 여기 있는지다 — 연결 수이거나 관계의 이름이거나 수정 시각. */
function knowledgeListRow(key, label, note, fill = null) {
  const row = document.createElement("li");
  if (fill !== null) {
    row.className = "knowledge-meter";
    row.style.setProperty("--meter-fill", String(fill));
  }
  const press = document.createElement("button");
  press.type = "button";
  press.className = "knowledge-inspector-row";
  press.dataset.knowledgeKey = key;
  press.textContent = label;
  const said = document.createElement("span");
  said.className = "knowledge-inspector-note";
  said.textContent = note;
  row.append(press, said);
  return row;
}

/* 고른 것이 없을 때의 인스펙터 — 「이 볼트는 어떤 모양인가」.
 *
 * 빈 칸에 「점을 누르세요」 한 줄을 두는 것은 아직 아무것도 누르지 않은 사람에게
 * 아무것도 주지 않는 일이다. 그림을 처음 연 사람이 묻는 것은 「무엇이 중심인가·
 * 무엇이 많은가·무엇이 최근인가」이고, 그 셋은 이미 답 안에 있다. */
function buildKnowledgeOverview() {
  const box = document.createElement("section");
  box.className = "knowledge-overview";
  const title = document.createElement("h3");
  title.className = "knowledge-inspector-title";
  title.dataset.i18n = "knowledge.overview";
  title.textContent = t("knowledge.overview", "볼트 개요");
  const counts = document.createElement("dl");
  counts.className = "knowledge-overview-counts";
  for (const row of [
    { what: "pages", key: "knowledge.countPages", word: "페이지" },
    { what: "links", key: "knowledge.countLinks", word: "링크" },
    { what: "ghosts", key: "knowledge.countGhosts", word: "유령" },
    { what: "orphans", key: "knowledge.countOrphans", word: "고아" },
  ]) {
    const tile = document.createElement("div");
    tile.className = "knowledge-stat-tile";
    const name = document.createElement("dt");
    name.dataset.i18n = row.key;
    name.textContent = t(row.key, row.word);
    const said = document.createElement("dd");
    said.dataset.knowledgeCount = row.what;
    tile.append(said, name);
    counts.appendChild(tile);
  }
  /* 볼트 건강 — 카파시의 lint 목록을 백엔드의 표 그대로(`KNOWLEDGE_HEALTH_ROWS`).
   * 줄은 판이 지어질 때 한 번 서고(낱말은 키를 들고 있어 `applyLocale`이
   * 갈아입힌다), 개요를 다시 그릴 때 바뀌는 것은 오른쪽의 수와 「깨끗함」의
   * 옷뿐이다. 누르면 그 목록으로 간다 — 고아·유령·색인·frontmatter·미선언은
   * 렌즈, 모순·대체는 관계 낱말 검색, raw 항목은 문이 없다(그림에 점이 없다). */
  const health = knowledgeListSection(
    "knowledge-overview-health",
    "knowledge.health",
    t("knowledge.health", "볼트 건강"),
  );
  const healthList = health.querySelector(".knowledge-inspector-list");
  for (const row of KNOWLEDGE_HEALTH_ROWS) {
    const line = knowledgeListRow(`lint:${row.lint}`, t(row.key, row.word), "", 0);
    line.classList.add("knowledge-health-row");
    line.dataset.knowledgeLint = row.lint;
    const press = line.querySelector("button");
    press.dataset.knowledgeLint = row.lint;
    press.dataset.i18n = row.key;
    if (row.door === "none") {
      press.disabled = true;
      press.setAttribute("aria-disabled", "true");
    }
    healthList.appendChild(line);
  }
  box.append(
    title,
    counts,
    /* 공급망(P4)의 절 — 렌즈가 켜졌을 때만 선다. 조회 상태의 한 줄과 「다시 확인」이 여기 산다. */
    buildKnowledgeSupplyOverview(),
    health,
    knowledgeListSection("knowledge-overview-clusters", "knowledge.clusters", t("knowledge.clusters", "군집")),
    knowledgeListSection("knowledge-overview-hubs", "knowledge.hubs", t("knowledge.hubs", "허브")),
    knowledgeListSection("knowledge-overview-tags", "knowledge.tagShare", t("knowledge.tagShare", "태그 분포")),
    knowledgeListSection("knowledge-overview-kinds", "knowledge.relationKinds", t("knowledge.relationKinds", "관계 종류")),
    knowledgeListSection("knowledge-overview-recent", "knowledge.recent", t("knowledge.recent", "최근 수정")),
  );
  return box;
}

/* 「활동」 탭(t-4140 S4) — BUS LOG(t-2931), 영상의 그 띠. 무엇이 생기고 고쳐지고
 * 이어졌는가(워처의 diff), 무엇이 어느 판에 회상되었는가(추적), 무엇이 취합되었는가
 * (일지의 꼬리) — 최신순, 상한은 백엔드의 표. 답에 라이브 층이 없으면 띠는 서지
 * 않는다. 개요 밖의 제 탭에 있는 것은 새 줄이 늘 오는 자리라서다. */
function buildKnowledgeActivity() {
  const panel = document.createElement("div");
  panel.className = "knowledge-inspector-activity";
  panel.setAttribute("role", "tabpanel");
  panel.hidden = true;
  const bus = knowledgeListSection("knowledge-overview-bus", "knowledge.busLog", t("knowledge.busLog", "BUS LOG"));
  bus.hidden = true;
  const quiet = document.createElement("p");
  quiet.className = "knowledge-activity-quiet";
  quiet.dataset.i18n = "knowledge.activityQuiet";
  quiet.textContent = t("knowledge.activityQuiet", "아직 기록된 활동이 없습니다");
  panel.append(bus, quiet);
  return panel;
}

/* 고른 것이 있을 때의 인스펙터 — 그 한 페이지의 카드. */
function buildKnowledgeCard() {
  const box = document.createElement("section");
  box.className = "knowledge-card";
  box.hidden = true;
  const title = document.createElement("h3");
  title.className = "knowledge-inspector-title";
  const where = document.createElement("p");
  where.className = "knowledge-inspector-path";
  const folder = document.createElement("p");
  folder.className = "knowledge-inspector-folder";
  const tags = document.createElement("p");
  tags.className = "knowledge-inspector-tags";
  /* 이 페이지가 속한 군집 — 색 점과 이름 하나. 누르면 그 군집만 밝힌다(개요의
   * 군집 줄과 같은 손). */
  const cluster = document.createElement("p");
  cluster.className = "knowledge-inspector-cluster";
  const clusterHead = document.createElement("span");
  clusterHead.className = "knowledge-inspector-head";
  clusterHead.dataset.i18n = "knowledge.clusters";
  clusterHead.textContent = t("knowledge.clusters", "군집");
  const clusterPress = document.createElement("button");
  clusterPress.type = "button";
  clusterPress.className = "knowledge-inspector-row knowledge-cluster-press";
  cluster.append(clusterHead, clusterPress);
  const excerpt = document.createElement("p");
  excerpt.className = "knowledge-inspector-excerpt";

  /* 포커스 깊이는 캔버스 위 빵부스러기 아래에 산다(`buildKnowledgeView`, 시안 이식). */
  const backlinks = document.createElement("p");
  backlinks.className = "knowledge-backlinks";
  const when = document.createElement("p");
  when.className = "knowledge-modified";

  const chainSection = document.createElement("section");
  chainSection.className = "knowledge-inspector-chain";
  chainSection.hidden = true;
  const chainHead = document.createElement("h4");
  chainHead.className = "knowledge-inspector-head";
  chainHead.dataset.i18n = "knowledge.pathChain";
  chainHead.textContent = t("knowledge.pathChain", "최단 경로");
  const chainBody = document.createElement("div");
  chainBody.className = "knowledge-chain-body";
  chainSection.append(chainHead, chainBody);

  const acts = document.createElement("div");
  acts.className = "knowledge-inspector-acts";
  const open = document.createElement("button");
  open.type = "button";
  open.className = "btn btn--primary knowledge-inspector-open";
  open.dataset.i18n = "knowledge.openPage";
  open.textContent = t("knowledge.openPage", "페이지 열기");
  const center = document.createElement("button");
  center.type = "button";
  center.className = "btn knowledge-inspector-center";
  center.dataset.i18n = "knowledge.center";
  center.textContent = t("knowledge.center", "중심으로");
  const request = workbenchButton("knowledge-inspector-request", { key: "workbench.request", word: "이 지식으로 작업 요청" }, () => {});
  request.onclick = null;
  /* 「연결 추가」(t-4140 S5): 이 페이지에서 다른 페이지로 관계 하나를 frontmatter에 적는다. */
  const addLink = document.createElement("button");
  addLink.type = "button";
  addLink.className = "btn knowledge-inspector-link";
  addLink.dataset.i18n = "knowledge.addLink";
  addLink.textContent = t("knowledge.addLink", "연결 추가");
  acts.append(open, center, request, addLink);

  box.append(
    title,
    where,
    folder,
    tags,
    cluster,
    excerpt,
    /* 고른 점이 구성요소·취약점이면 그 사실들(P4) — 페이지의 절들은 비어 물러선다. */
    buildKnowledgeSupplyCard(),
    acts,
    buildKnowledgeLinkForm(),
    knowledgeListSection("knowledge-relations-outgoing", "knowledge.outgoing", t("knowledge.outgoing", "나가는 관계")),
    knowledgeListSection("knowledge-relations-incoming", "knowledge.incoming", t("knowledge.incoming", "들어오는 관계")),
    backlinks,
    knowledgeListSection("knowledge-recalled-by", "knowledge.recalledBy", t("knowledge.recalledBy", "이 페이지를 본 작업")),
    when,
    chainSection,
  );
  return box;
}

/* 연결 만들기의 서식(t-4140 S5) — 카드 안의 한 절, 새 패널이 아니다. 대상은 검색 후보와
 * 같은 매처로 찾고(`knowledgePageMatches`), 관계 유형은 frontmatter의 다섯 키, 방향은
 * 둘 중 하나, 미리보기는 바뀔 줄이다. 저장은 문 하나(`second_brain_relate`)이고
 * 되돌리기는 같은 문의 반대 호출이다. */
function buildKnowledgeLinkForm() {
  const form = document.createElement("section");
  form.className = "knowledge-link-form";
  form.hidden = true;
  const head = document.createElement("h4");
  head.className = "knowledge-inspector-head";
  head.dataset.i18n = "knowledge.linkForm";
  head.textContent = t("knowledge.linkForm", "연결 만들기");
  const target = document.createElement("input");
  target.type = "text";
  target.className = "settings-input knowledge-link-target";
  target.setAttribute("role", "combobox");
  target.setAttribute("aria-autocomplete", "list");
  target.setAttribute("aria-expanded", "false");
  target.dataset.i18nPlaceholder = "knowledge.linkTarget";
  target.placeholder = t("knowledge.linkTarget", "연결할 페이지");
  target.dataset.i18nAria = "knowledge.linkTarget";
  target.setAttribute("aria-label", t("knowledge.linkTarget", "연결할 페이지"));
  const candidates = document.createElement("ul");
  candidates.className = "knowledge-search-results knowledge-link-candidates";
  candidates.setAttribute("role", "listbox");
  candidates.hidden = true;
  const kind = document.createElement("select");
  kind.className = "settings-input knowledge-link-kind";
  kind.dataset.i18nAria = "knowledge.linkKind";
  kind.setAttribute("aria-label", t("knowledge.linkKind", "관계의 의미"));
  for (const code of KNOWLEDGE_LINK_KINDS) {
    const option = document.createElement("option");
    option.value = KNOWLEDGE_EDGE_KINDS[code];
    option.textContent = KNOWLEDGE_EDGE_KINDS[code];
    kind.appendChild(option);
  }
  const directions = document.createElement("div");
  directions.className = "knowledge-link-directions";
  directions.setAttribute("role", "radiogroup");
  directions.dataset.i18nAria = "knowledge.linkDirection";
  directions.setAttribute("aria-label", t("knowledge.linkDirection", "방향"));
  for (const row of KNOWLEDGE_LINK_DIRECTIONS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn knowledge-link-direction";
    one.dataset.knowledgeLinkDirection = row.id;
    one.setAttribute("role", "radio");
    one.setAttribute("aria-checked", "false");
    one.dataset.i18n = row.key;
    one.textContent = t(row.key, row.word);
    directions.appendChild(one);
  }
  const preview = document.createElement("p");
  preview.className = "knowledge-link-preview";
  preview.setAttribute("aria-live", "polite");
  const acts = document.createElement("div");
  acts.className = "knowledge-inspector-acts";
  const save = document.createElement("button");
  save.type = "button";
  save.className = "btn btn--primary knowledge-link-save";
  save.disabled = true;
  save.dataset.i18n = "knowledge.linkSave";
  save.textContent = t("knowledge.linkSave", "저장");
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn knowledge-link-cancel";
  cancel.dataset.i18n = "knowledge.linkCancel";
  cancel.textContent = t("knowledge.linkCancel", "취소");
  acts.append(save, cancel);
  form.append(head, target, candidates, kind, directions, preview, acts);
  return form;
}

/* ---- 그림에 서는 것 고르기 ----
 *
 * 필터는 창에서 한다. 백엔드는 볼트를 한 번 읽고 전부를 답하고, 검색·칩·
 * 「고아만」·「유령만」은 그 답의 부분집합을 고른다 — 글자 하나에 5,000개의
 * stat을 다시 하는 것은 필터가 아니라 재스캔이다. `sources`만 예외로 백엔드에
 * 묻는다: 그것은 답에 **없는** 노드를 만드는 일이라 부분집합으로 고를 수 없다. */
// Compare the existing integer arrays instead of copying the whole graph into
// a signature string and then into SVG data attributes. Counts alone miss a
// rewire between the same pages or a change in the kind of a relation.
function sameKnowledgeTopology(left, right) {
  if (!left || left.vault !== right.vault || left.count !== right.count
      || left.edgeCount !== right.edgeCount) return false;
  /* 공급망의 접힌 수(P4)는 선이 같아도 달라질 수 있다 — 멤버의 「+N」은 이름표의 폭이다. */
  if (left.supplyFoldStamp !== right.supplyFoldStamp) return false;
  for (let at = 0; at < right.count; at += 1) {
    if (left.keys[at] !== right.keys[at] || left.kinds[at] !== right.kinds[at]) return false;
  }
  for (let at = 0; at < right.edgeCount; at += 1) {
    if (left.from[at] !== right.from[at] || left.to[at] !== right.to[at]
        || left.kind[at] !== right.kind[at]) return false;
  }
  return true;
}

function knowledgeModel(report) {
  const lens = [knowledgeOrphansOnly, knowledgeGhostsOnly, knowledgeTypedOnly,
    knowledgeAliveOnly, knowledgeColdOnly, knowledgeMergeOnly, knowledgeLintLens].join("|");
  /* 공급망 렌즈(P4)가 켜졌으면 답의 점과 선이 볼트 뒤에 붙는다(`knowledgeSupplyView`) — 답·볼트·펼침이
   * 그대로면 같은 객체라 캐시의 문지기가 된다. 꺼졌으면 `null`이고 아래는 볼트만의 그림이다. */
  const supply = knowledgeSupplyView(report?.graph ?? null);
  if (knowledgeModelCache?.report === report && knowledgeModelCache.lens === lens
      && knowledgeModelCache.supply === supply) {
    return knowledgeModelCache.model;
  }
  const nodes = supply?.nodes ?? report?.graph?.nodes ?? [];
  const graphEdges = supply?.edges ?? report?.graph?.edges ?? [];
  /* 라이브 층(t-2931). 없는 답(옛 백엔드)은 고리도 띠도 후보도 없이 그림만 선다 —
   * 창은 제 숫자를 갖지 않으므로 표가 없으면 창(window)도 없다. */
  const live = report?.live ?? null;
  const limits = live?.limits ?? null;
  const nowMs = live?.now_ms ?? Date.now();
  const windowMs = limits ? limits.activity_window_minutes * 60_000 : 0;
  const todayMs = limits ? limits.today_hours * 3_600_000 : 0;
  const recalledAt = new Map((live?.recalled ?? []).map((row) => [row.id, row.at_ms]));
  const mergePairs = live?.merge ?? [];
  /* 후보 쌍은 렌즈가 켜졌을 때만 선이 된다 — 물음표 선 예순 개가 볼트 전체에
   * 늘 그려지면 그림은 관계가 아니라 의심을 말한다. */
  const pairs = knowledgeMergeOnly
    ? [...graphEdges, ...mergePairs.map((pair) => ({ from: pair.left, to: pair.right, kind: "merge" }))]
    : graphEdges;
  const alive = new Uint8Array(nodes.length);
  const recalledNow = new Uint8Array(nodes.length);
  if (limits) {
    for (let at = 0; at < nodes.length; at += 1) {
      const node = nodes[at];
      if (node.kind !== "page") continue;
      const recalled = recalledAt.get(node.id) ?? 0;
      recalledNow[at] = recalled > 0 && nowMs - recalled <= windowMs ? 1 : 0;
      const changed = node.modified_ms > 0 && nowMs - node.modified_ms <= windowMs;
      alive[at] = recalledNow[at] === 1 || changed ? 1 : 0;
    }
  }
  const mergeSide = new Uint8Array(nodes.length);
  if (knowledgeMergeOnly) {
    for (const pair of mergePairs) {
      if (pair.left < nodes.length) mergeSide[pair.left] = 1;
      if (pair.right < nodes.length) mergeSide[pair.right] = 1;
    }
  }
  /* 백엔드의 lint 표 — 렌즈의 멤버십은 여기서 온다. 창이 「고아」를 제 손으로
   * 세면 카드의 수와 `zerocode vault-lint`의 수가 갈릴 수 있으므로 세지 않는다. */
  const lintTable = report?.graph?.lint ?? null;
  const lintSets = {
    orphans: knowledgeLintPages(lintTable?.orphans),
    index_gaps: knowledgeLintPages(lintTable?.index_gaps),
    missing_frontmatter: knowledgeLintPages(lintTable?.missing_frontmatter),
    undeclared_relations: knowledgeLintPages(lintTable?.undeclared_relations),
  };
  const lintLens = knowledgeLintLens !== null ? (lintSets[knowledgeLintLens] ?? new Set()) : null;
  /* 「유령 링크만」은 유령 점 **과 그것을 가리키는 페이지**다. 유령만 남기면
   * 선이 없는 점들이 뜨고, 그 화면은 "어느 페이지가 없는 것을 가리키는가"라는
   * 물음에 답하지 못한다. */
  const ghostSide = new Uint8Array(nodes.length);
  if (knowledgeGhostsOnly) {
    for (const edge of pairs) {
      if (nodes[edge.to]?.kind !== "ghost") continue;
      ghostSide[edge.to] = 1;
      ghostSide[edge.from] = 1;
    }
  }
  /* 「타입 관계만」도 같은 손이다 — 남는 것은 프론트매터가 선언한 선과 그 선의
   * 양 끝이고, 그 밖의 점은 선 없는 점이 되므로 서지 않는다. */
  const typedSide = new Uint8Array(nodes.length);
  if (knowledgeTypedOnly) {
    for (const edge of pairs) {
      if (knowledgeEdgeCode(edge) === 0) continue;
      typedSide[edge.from] = 1;
      typedSide[edge.to] = 1;
    }
  }
  const seat = new Int32Array(nodes.length).fill(-1);
  const keys = [];
  const titles = [];
  const held = [];
  /* 자리 → 답의 인덱스. 라이브 층의 배열들은 답의 순서로 세어 두었고, 자리마다
   * 그것을 되찾는 데 `indexOf`를 쓰면 천 점에서 백만 번의 비교다. */
  const origin = [];
  for (let at = 0; at < nodes.length; at += 1) {
    const node = nodes[at];
    /* 이름이 구성요소의 이름과 글자 그대로 같은 유령은 그 구성요소로 섰다(P4) — 유령의 점은 서지 않는다. */
    if (supply?.skip?.[at] === 1) continue;
    if (knowledgeGhostsOnly && ghostSide[at] === 0) continue;
    if (knowledgeTypedOnly && typedSide[at] === 0) continue;
    if (knowledgeOrphansOnly && !lintSets.orphans.has(node.id)) continue;
    if (lintLens !== null && !lintLens.has(node.id)) continue;
    if (knowledgeAliveOnly && alive[at] === 0) continue;
    if (knowledgeColdOnly && !(node.kind === "page" && !recalledAt.has(node.id))) continue;
    if (knowledgeMergeOnly && mergeSide[at] === 0) continue;
    seat[at] = held.length;
    held.push(node);
    origin.push(at);
    keys.push(node.id);
    titles.push(node.title);
  }
  const count = held.length;
  /* 평평한 정수 배열 둘. 간선을 객체로 들면 이천 개의 작은 객체가 매 프레임
   * 읽히고, 배치의 안쪽 고리는 그 간접 참조를 이천 번 곱하기 반복 수만큼 낸다. */
  const from = new Int32Array(pairs.length);
  const to = new Int32Array(pairs.length);
  const ghost = new Uint8Array(pairs.length);
  const kind = new Uint8Array(pairs.length);
  let edgeCount = 0;
  const degree = new Int32Array(count);
  const relationKinds = new Uint8Array(count);
  for (const edge of pairs) {
    const left = seat[edge.from];
    const right = seat[edge.to];
    if (left < 0 || right < 0) continue;
    const code = knowledgeEdgeCode(edge);
    if (knowledgeTypedOnly && code === 0) continue;
    from[edgeCount] = left;
    to[edgeCount] = right;
    ghost[edgeCount] = held[right].kind === "ghost" || held[left].kind === "ghost" ? 1 : 0;
    kind[edgeCount] = code;
    relationKinds[left] |= 1 << code;
    relationKinds[right] |= 1 << code;
    edgeCount += 1;
    degree[left] += 1;
    degree[right] += 1;
  }
  /* 이웃 목록은 CSR 한 벌 — 호버가 이웃을 밝히는 데 쓰고, 프레임마다 다시
   * 짓지 않는다. 인접 「맵의 배열」이 아닌 것은 천 개의 작은 배열이 천 개의
   * 할당이기 때문이다. */
  const start = new Int32Array(count + 1);
  for (let at = 0; at < edgeCount; at += 1) {
    start[from[at] + 1] += 1;
    start[to[at] + 1] += 1;
  }
  for (let at = 0; at < count; at += 1) start[at + 1] += start[at];
  const cursor = Int32Array.from(start.subarray(0, count));
  const neighbour = new Int32Array(edgeCount * 2);
  const throughEdge = new Int32Array(edgeCount * 2);
  for (let at = 0; at < edgeCount; at += 1) {
    throughEdge[cursor[from[at]]] = at;
    neighbour[cursor[from[at]]] = to[at];
    cursor[from[at]] += 1;
    throughEdge[cursor[to[at]]] = at;
    neighbour[cursor[to[at]]] = from[at];
    cursor[to[at]] += 1;
  }
  /* 관계 낱말은 검색의 건초더미에도 든다(3차): `contradicts`를 치면 모순 관계에
   * 걸린 페이지가 밝는다 — 건강 카드의 모순·대체됨 줄이 이 길로 간다. 본문의
   * 대괄호(`mentions`)는 이름이 아니라 들지 않는다. */
  const relationWords = Array.from({ length: count }, () => []);
  for (let at = 0; at < edgeCount; at += 1) {
    if (kind[at] === 0) continue;
    const word = KNOWLEDGE_EDGE_KINDS[kind[at]];
    if (!relationWords[from[at]].includes(word)) relationWords[from[at]].push(word);
    if (!relationWords[to[at]].includes(word)) relationWords[to[at]].push(word);
  }
  /* 라이브 층의 세 수도 볼트 **전체**의 것이다: 오늘 회상된 페이지, 추적에 한 번도
   * 없는 페이지, merge 후보 쌍. */
  let recalledToday = 0;
  let recalledPages = 0;
  const pageIds = new Set(nodes.filter((node) => node.kind === "page").map((node) => node.id));
  for (const [id, at] of recalledAt) {
    if (!pageIds.has(id)) continue;
    recalledPages += 1;
    if (nowMs - at <= todayMs) recalledToday += 1;
  }
  const neverRecalled = live ? Math.max(0, (report?.graph?.pages ?? 0) - recalledPages) : 0;
  /* 백엔드가 최신순으로 세어 상한을 두었지만, 여기서 한 번 더 세우는 것은 마흔
   * 줄이라 공짜이고 그 약속이 창 쪽에서도 참이 되기 때문이다. */
  const bus = [...(live?.bus ?? [])]
    .sort((left, right) => right.at_ms - left.at_ms)
    .slice(0, limits?.bus_rows_max ?? 0);
  /* 볼트 건강의 수는 렌즈와 무관하게 볼트 **전체**의 것이고, 백엔드의 표에서
   * 그대로 온다 — 걸러 놓은 그림에서 세면 「고아만」을 켜는 순간 모순이 0이
   * 된다. 표의 수가 바뀌면(링크는 그대로인데 frontmatter가 채워진 판) 개요도
   * 다시 그려야 한다. 개요는 모델을, 배치는 위상을 따로 비교한다. */
  /* 라이브 층의 세 수(t-2931)는 같은 표의 세 줄로 들어간다 — 카드는 표 하나만
   * 읽는다. merge 후보는 백엔드의 lint 표가 세어 준 것이 있으면 그것, 아니면
   * 라이브 층이 센 쌍의 수다(두 producer가 한 함수라 같은 수다). 서명에는 표의
   * 제 줄만 갱신한다: 회상 하나가 천 개의 점을 다시 짓게 해서는 안 된다. */
  const lintCounts = lintTable?.counts == null ? null : {
    ...lintTable.counts,
    recalled_today: recalledToday,
    never_recalled: neverRecalled,
    merge_candidates: lintTable.counts.merge_candidates ?? mergePairs.length,
  };
  const model = {
    live,
    limits,
    nowMs,
    /* 자리마다: 창 안에서 회상되었는가(고리), 마지막 회상 시각. */
    recalledNow: Uint8Array.from(origin, (at) => recalledNow[at]),
    recalledAt: held.map((node) => recalledAt.get(node.id) ?? 0),
    bus,
    busStamp: bus.map((row) => `${row.at_ms}|${row.kind}|${row.page ?? ""}`).join(","),
    mergeable: mergePairs.length > 0,
    vault: report?.vault ?? "",
    count,
    keys,
    titles,
    kinds: held.map((node) => node.kind),
    tags: held.map((node) => node.tags),
    excerpts: held.map((node) => node.excerpt ?? ""),
    folders: held.map((node) => node.folder ?? ""),
    sources: held.map((node) => node.source ?? null),
    modified: held.map((node) => node.modified_ms),
    inLinks: held.map((node) => node.in_links ?? 0),
    outLinks: held.map((node) => node.out_links ?? 0),
    relationWords,
    relationKinds,
    lint: lintTable === null ? null : { ...lintTable, counts: lintCounts },
    degree,
    from,
    to,
    ghost,
    kind,
    edgeCount,
    start,
    neighbour,
    throughEdge,
    /* 칩에 세울 태그. 백엔드가 이미 많이 쓰인 순으로 세어 두었다. */
    catalog: (report?.graph?.tags ?? []).slice(0, KNOWLEDGE_TAG_CHIPS),
    /* 「타입 관계만」이 설 자리가 있는가. 백엔드가 세어 준 것으로 답한다 —
     * 필터가 이미 걸린 부분집합에서 세면 켜자마자 제 조건을 잃는 칩이 된다.
     * 낡은 답에는 이 목록이 아예 없고, 그때의 답은 「없다」이다. */
    typed: (report?.graph?.kinds ?? []).some((row) => row.kind !== "mentions" && row.count > 0),
    total: report?.graph ?? null,
    /* 그림이 들 수 있는 점 전부 — 볼트의 점과 렌즈가 붙인 점. 자리 기억을 놓을 때 이 목록을 읽는다. */
    nodeList: nodes,
  };
  knowledgeSupplySeats(model, supply, origin);
  const previous = knowledgeModelCache?.model;
  model.signature = sameKnowledgeTopology(previous, model)
    ? previous.signature : String(++knowledgeTopologyGeneration);
  knowledgeModelCache = { report, lens, supply, model };
  return model;
}

/* 검색 토큰 문법 (Bloom 문법): tag:<t> rel:<relation> since:<Nd|Nh>
 * kind:<page|ghost|source|component|vulnerability> sev:<critical|high|medium|low|none|unknown> hub
 * 및 자유 텍스트 혼합. 매칭은 디밍만(멤버십·좌표 불변). 관계 낱말은 프론트매터 키 그대로 번역 금지.
 * `sev:`은 공급망 렌즈(P4)의 취약점을 답의 심각도 낱말 그대로 고른다. */
function parseKnowledgeSearchTokens(raw) {
  const parts = raw.trim().split(/\s+/).filter(Boolean);
  const tokens = [];
  const free = [];
  for (const part of parts) {
    const lower = part.toLocaleLowerCase();
    if (lower === "hub") {
      tokens.push({ type: "hub" });
    } else if (lower.startsWith("tag:")) {
      tokens.push({ type: "tag", value: part.slice(4).toLocaleLowerCase() });
    } else if (lower.startsWith("rel:")) {
      tokens.push({ type: "rel", value: part.slice(4).toLocaleLowerCase() });
    } else if (lower.startsWith("since:")) {
      const val = part.slice(6);
      const match = /^(\d+)([dh])$/i.exec(val);
      if (match) {
        const count = Number(match[1]);
        const unit = match[2].toLowerCase();
        const ms = count * (unit === "d" ? 86400000 : 3600000);
        tokens.push({ type: "since", cutoff: Date.now() - ms });
      } else {
        free.push(part);
      }
    } else if (lower.startsWith("kind:")) {
      tokens.push({ type: "kind", value: part.slice(5).toLocaleLowerCase() });
    } else if (lower.startsWith("sev:")) {
      tokens.push({ type: "sev", value: part.slice(4).toLocaleLowerCase() });
    } else {
      free.push(part);
    }
  }
  return { tokens, free, active: tokens.length > 0 || free.length > 0 };
}

/* 한 페이지가 자유 낱말 전부를 품는가 — 제목·열쇠·태그·관계 낱말에서. 검색의 디밍과
 * 연결 만들기의 대상 후보(S5)가 같은 물음을 같은 손으로 묻는다. */
function knowledgePageMatches(model, at, words) {
  return words.every((word) => graphSearchMatch(word.toLocaleLowerCase(), [
    model.titles[at], model.keys[at], ...model.tags[at], ...model.relationWords[at],
  ]));
}

/* 태그 칩이 검색 상자에 적는 말. 맨 태그가 검색 문법의 토큰으로 읽히는 이름이면
 * (`hub`, `kind:page` 같은 태그) 칩은 제 태그를 고르지 못했다 — 그때만 `tag:` 토큰으로
 * 적는다. 토큰인가는 파서 자신이 답한다. */
function knowledgeTagQuery(tag) {
  return parseKnowledgeSearchTokens(tag).tokens.length === 0 ? tag : `tag:${tag}`;
}

/* 검색어가 칩 밖에서 바뀌면(손으로 친 글자·토큰 칩·저장된 검색어) 칩의 불도 그 말을
 * 따른다: 켜진 칩은 그 태그가 지금의 검색어일 때만 켜져 있다. 따르지 않으면 켜진 채
 * 남은 칩을 다시 누르는 손이 태그를 다시 거는 대신 검색어를 지웠다. */
function followKnowledgeChips() {
  const said = knowledgeQuery.trim().toLocaleLowerCase();
  for (const tag of [...knowledgeTagsPicked]) {
    if (knowledgeTagQuery(tag).toLocaleLowerCase() !== said) knowledgeTagsPicked.delete(tag);
  }
}

function paintKnowledgeSearch(view, layout) {
  const parsed = parseKnowledgeSearchTokens(knowledgeQuery);
  const searching = parsed.active;
  const matched = layout.searchMatch;
  matched.fill(0);
  let matches = 0;
  const model = layout.model;

  for (let at = 0; at < layout.count; at += 1) {
    let yes = searching;
    if (yes) {
      for (const tok of parsed.tokens) {
        if (tok.type === "hub") {
          /* 허브는 그림이 허브로 그린 점이다(`paintKnowledgeNodes`가 토큰의 차수와
           * 「상위 5%」 중 엄한 쪽으로 단 `data-hub`). 차수 4를 따로 세던 대체 길은 큰
           * 볼트에서 페이지의 3분의 2를 「허브」로 밝혔다(K23). */
          if (layout.nodeEls[at]?.dataset.hub !== "true") { yes = false; break; }
        } else if (tok.type === "tag") {
          const hasTag = model.tags[at]?.some((t) => t.toLocaleLowerCase() === tok.value);
          if (!hasTag) { yes = false; break; }
        } else if (tok.type === "rel") {
          const code = KNOWLEDGE_EDGE_CODE[tok.value];
          const hasRel = Number.isInteger(code) && (model.relationKinds[at] & (1 << code)) !== 0;
          if (!hasRel) { yes = false; break; }
        } else if (tok.type === "since") {
          const time = Math.max(model.modified[at] || 0, model.recalledAt[at] || 0);
          if (time < tok.cutoff) { yes = false; break; }
        } else if (tok.type === "kind") {
          const nodeKind = model.kinds[at]?.toLocaleLowerCase() ?? "";
          if (nodeKind !== tok.value) { yes = false; break; }
        } else if (tok.type === "sev") {
          if (KNOWLEDGE_SUPPLY_SEVERITIES[model.severity?.[at] ?? -1]?.id !== tok.value) { yes = false; break; }
        }
      }

      if (yes && parsed.free.length > 0 && !knowledgePageMatches(model, at, parsed.free)) yes = false;
    }

    matched[at] = yes ? 1 : 0;
    if (yes) matches += 1;
    layout.nodeEls[at]?.classList.toggle("is-search-match", yes);
    layout.nodeEls[at]?.classList.toggle("is-search-dim", searching && !yes);
  }
  for (let at = 0; at < layout.model.edgeCount; at += 1) {
    const line = knowledgeEdgeLine(layout, at);
    if (line === null) continue;
    const yes = searching
      && matched[layout.model.from[at]] === 1
      && matched[layout.model.to[at]] === 1;
    line.classList.toggle("is-search-match", yes);
    line.classList.toggle("is-search-dim", searching && !yes);
  }
  view.querySelector(".knowledge-picture").classList.toggle("is-searching", searching);
  return matches;
}

/* ---- 검색 후보 (t-4140 S3) ----
 *
 * 후보는 검색이 밝힌 점(`layout.searchMatch`) 중 상한만큼이다 — 매칭을 두 번 하지
 * 않는다. 순서: 제목이 자유 낱말로 **시작**하는 것, 자유 낱말을 **품은** 것, 그 밖
 * (토큰만의 검색)의 셋을 등급으로, 등급 안에서는 연결이 많은 순, 동률은 열쇠순 —
 * 개요의 허브 목록과 같은 결정성이다. */
function knowledgeCandidates(layout) {
  if (knowledgeQuery.trim() === "") return [];
  const model = layout.model;
  /* 등급은 자유 낱말 전체(한 구절)로 잰다 — 「개념 1」을 치면 「개념 1」이 「개념 10」 앞이다. */
  const phrase = parseKnowledgeSearchTokens(knowledgeQuery).free.join(" ").toLocaleLowerCase();
  const rows = [];
  for (let at = 0; at < layout.count; at += 1) {
    if (layout.searchMatch[at] !== 1) continue;
    const title = model.titles[at].toLocaleLowerCase();
    const rank = phrase === "" ? 3
      : title === phrase ? 0
        : title.startsWith(phrase) ? 1
          : title.includes(phrase) ? 2 : 3;
    rows.push({ at, rank });
  }
  rows.sort((left, right) => left.rank - right.rank
    || model.degree[right.at] - model.degree[left.at]
    || (model.keys[left.at] < model.keys[right.at] ? -1 : 1));
  return rows.slice(0, KNOWLEDGE_EXPLORE.candidates).map((row) => row.at);
}

/* 한 줄의 힌트: 폴더, 유령이면 「아직 없는 페이지」, 그리고 관계 낱말들. */
function knowledgeCandidateNote(model, at) {
  const parts = [];
  if (model.kinds[at] === "ghost") parts.push(t("knowledge.ghostNode", "아직 없는 페이지"));
  const supplied = knowledgeSupplyNote(model, at);
  if (supplied !== "") parts.push(supplied);
  if (model.folders[at] !== "") parts.push(model.folders[at]);
  parts.push(...model.relationWords[at]);
  return parts.join(" · ");
}

function paintKnowledgeCandidates(view, layout) {
  const list = view.querySelector(".knowledge-search-results");
  const empty = view.querySelector(".knowledge-candidates-empty");
  const model = layout.model;
  const seats = knowledgeCandidates(layout);
  if (knowledgeCandidateAt >= seats.length) knowledgeCandidateAt = seats.length - 1;
  const rows = seats.map((at, index) => {
    const row = document.createElement("li");
    row.setAttribute("role", "option");
    row.dataset.knowledgeKey = model.keys[at];
    row.setAttribute("aria-selected", String(index === knowledgeCandidateAt));
    const press = document.createElement("button");
    press.type = "button";
    press.className = "knowledge-candidate";
    press.tabIndex = -1;
    const title = document.createElement("span");
    title.className = "knowledge-candidate-title";
    title.textContent = model.titles[at];
    const note = document.createElement("span");
    note.className = "knowledge-candidate-note";
    note.textContent = knowledgeCandidateNote(model, at);
    press.append(title, note);
    const explore = document.createElement("button");
    explore.type = "button";
    explore.className = "btn knowledge-candidate-explore";
    explore.dataset.knowledgeExplore = model.keys[at];
    explore.textContent = t("knowledge.candidateExplore", "주변 탐색으로");
    explore.setAttribute("aria-label", t(
      "knowledge.candidateExploreOf",
      "«{{name}}»의 주변 탐색으로",
      { name: model.titles[at] },
    ));
    row.append(press, explore);
    return row;
  });
  reconcileElementOrder(list, rows);
  list.hidden = rows.length === 0;
  empty.hidden = !(rows.length === 0 && knowledgeQuery.trim() !== "");
}

/* 검색 상자의 열림 — 클래스 하나와 콤보박스의 `aria-expanded`(S7), 한 손으로. */
function setKnowledgeSuggesting(view, on) {
  view.querySelector(".knowledge-search")?.classList.toggle("is-suggesting", on);
  view.querySelector(".knowledge-query")?.setAttribute("aria-expanded", String(on));
}

/* 후보를 고른다. 전체 지도에서는 고르고 가운데로(검색은 남는다 — 그 점은 밝은 점이다);
 * 주변 탐색으로 가는 길은 검색을 놓는다 — 흐린 이웃은 온 이유를 가린다. 상자는 닫힌다. */
function pickKnowledgeCandidate(view, key, { explore = false } = {}) {
  const layout = knowledgeLayouts.get(view);
  if (!layout || key === null) return;
  const find = view.querySelector(".knowledge-query");
  setKnowledgeSuggesting(view, false);
  knowledgeCandidateAt = -1;
  if (explore || knowledgeMode === "local") {
    knowledgeQuery = "";
    find.value = "";
    knowledgeTagsPicked.clear();
    /* 이미 중심인 점을 다시 고르면 옮길 것이 없다 — 놓은 검색만 다시 그린다. */
    const already = knowledgeMode === "local" && knowledgeSelectedKey === key;
    if (knowledgeMode === "local") exploreKnowledge(view, key);
    else setKnowledgeMode(view, "local", { centre: key });
    if (already) void paintKnowledgeView();
    return;
  }
  const seat = layout.model.keys.indexOf(key);
  if (seat < 0) return;
  selectKnowledgeNode(view, key);
  knowledgeCenterOn(view, layout, seat);
}

/* ---- 연결 만들기 (t-4140 S5) ---- */

/* 서식을 연다(대상을 미리 들고 올 수 있다 — Alt+드래그의 길). 출발은 고른 페이지다. */
function openKnowledgeLinkForm(view, { target = null } = {}) {
  const layout = knowledgeLayouts.get(view);
  const seat = knowledgeSelectedKey === null ? -1 : layout?.model.keys.indexOf(knowledgeSelectedKey) ?? -1;
  if (!layout || seat < 0 || layout.model.kinds[seat] !== "page") return;
  const form = view.querySelector(".knowledge-link-form");
  knowledgeLinkTarget = target !== null && layout.model.keys.includes(target) ? target : null;
  knowledgeLinkCandidateAt = -1;
  knowledgeLinkDirection = KNOWLEDGE_LINK_DIRECTIONS[0].id;
  const find = form.querySelector(".knowledge-link-target");
  find.value = knowledgeLinkTarget === null ? "" : layout.model.titles[layout.model.keys.indexOf(knowledgeLinkTarget)];
  form.querySelector(".knowledge-link-kind").value = KNOWLEDGE_EDGE_KINDS[KNOWLEDGE_LINK_KINDS[0]];
  form.hidden = false;
  const linkBtn = view.querySelector(".knowledge-inspector-link");
  if (linkBtn) {
    linkBtn.classList.add("is-active");
    linkBtn.setAttribute("aria-expanded", "true");
  }
  paintKnowledgeLinkForm(view, layout);
  (knowledgeLinkTarget === null ? find : form.querySelector(".knowledge-link-save")).focus();
}

function closeKnowledgeLinkForm(view) {
  const form = view.querySelector(".knowledge-link-form");
  form.hidden = true;
  knowledgeLinkTarget = null;
  knowledgeLinkCandidateAt = -1;
  form.querySelector(".knowledge-link-candidates").hidden = true;
  form.querySelector(".knowledge-link-target").setAttribute("aria-expanded", "false");
  const linkBtn = view.querySelector(".knowledge-inspector-link");
  if (linkBtn) {
    linkBtn.classList.remove("is-active");
    linkBtn.setAttribute("aria-expanded", "false");
  }
}

/* 대상 후보: 입력의 자유 낱말로 페이지를 찾는다 — 출발 페이지와 유령·원본은 빼고, 상한은
 * 검색 후보와 같은 표. */
function knowledgeLinkCandidates(layout, typed) {
  const model = layout.model;
  const words = typed.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const seats = [];
  for (let at = 0; at < model.count; at += 1) {
    if (model.keys[at] === knowledgeSelectedKey || model.kinds[at] !== "page") continue;
    if (knowledgePageMatches(model, at, words)) seats.push(at);
  }
  seats.sort((left, right) => model.degree[right] - model.degree[left]
    || (model.keys[left] < model.keys[right] ? -1 : 1));
  return seats.slice(0, KNOWLEDGE_EXPLORE.candidates);
}

/* 서식의 옷: 후보 목록, 방향의 `aria-checked`, 미리보기, 저장 단추. */
function paintKnowledgeLinkForm(view, layout) {
  const form = view.querySelector(".knowledge-link-form");
  if (form.hidden) return;
  const model = layout.model;
  const find = form.querySelector(".knowledge-link-target");
  const list = form.querySelector(".knowledge-link-candidates");
  const seats = knowledgeLinkTarget === null ? knowledgeLinkCandidates(layout, find.value) : [];
  if (knowledgeLinkCandidateAt >= seats.length) knowledgeLinkCandidateAt = seats.length - 1;
  reconcileElementOrder(list, seats.map((at, index) => {
    const row = document.createElement("li");
    row.setAttribute("role", "option");
    row.dataset.knowledgeLinkKey = model.keys[at];
    row.setAttribute("aria-selected", String(index === knowledgeLinkCandidateAt));
    const press = document.createElement("button");
    press.type = "button";
    press.className = "knowledge-candidate";
    press.tabIndex = -1;
    const title = document.createElement("span");
    title.className = "knowledge-candidate-title";
    title.textContent = model.titles[at];
    const note = document.createElement("span");
    note.className = "knowledge-candidate-note";
    note.textContent = knowledgeCandidateNote(model, at);
    press.append(title, note);
    row.appendChild(press);
    return row;
  }));
  list.hidden = seats.length === 0;
  find.setAttribute("aria-expanded", String(seats.length > 0));
  for (const one of form.querySelectorAll("[data-knowledge-link-direction]")) {
    const on = one.dataset.knowledgeLinkDirection === knowledgeLinkDirection;
    writeAttribute(one, "aria-checked", String(on));
    one.classList.toggle("is-active", on);
  }
  const save = form.querySelector(".knowledge-link-save");
  save.disabled = knowledgeLinkTarget === null;
  const preview = form.querySelector(".knowledge-link-preview");
  if (knowledgeLinkTarget === null) {
    say(preview, () => t("knowledge.linkPickTarget", "연결할 페이지를 고르세요"));
    return;
  }
  const plan = knowledgeLinkPlan(layout);
  say(preview, () => t(
    "knowledge.linkPreview",
    "«{{from}}» → {{kind}} → «{{to}}» · «{{from}}»의 frontmatter {{kind}}에 [[{{link}}]] 추가",
    plan,
  ));
}

/* 무엇을 쓸 것인가: 방향에 따라 출발과 대상이 바뀐다. 링크 글은 백엔드가 짓는 것과 같은
 * 규칙(wiki/ 아래의 경로, .md 없이)으로 미리 보인다. */
function knowledgeLinkPlan(layout) {
  const model = layout.model;
  const here = knowledgeSelectedKey;
  const there = knowledgeLinkTarget;
  const outward = knowledgeLinkDirection === KNOWLEDGE_LINK_DIRECTIONS[0].id;
  const from = outward ? here : there;
  const to = outward ? there : here;
  const kind = layout.view?.querySelector(".knowledge-link-kind")?.value
    ?? KNOWLEDGE_EDGE_KINDS[KNOWLEDGE_LINK_KINDS[0]];
  const title = (key) => model.titles[model.keys.indexOf(key)] ?? key;
  return { from: title(from), to: title(to), kind, link: to.replace(/^wiki\//u, "").replace(/\.md$/u, ""), fromKey: from, toKey: to };
}

/* 문 하나로 쓰고, 같은 문으로 되돌린다. 답이 오면 그림을 다시 읽는다 — 워처도 말하겠지만
 * 사람이 방금 한 일은 바로 보여야 한다. */
async function relateKnowledge(view, { from, to, kind, remove = false }) {
  const vault = knowledgeReport?.vault ?? secondBrainVault;
  try {
    const wrote = await invoke("second_brain_relate", { path: vault, from, to, kind, remove });
    if (!remove) knowledgeLastRelate = { vault, from, to, kind };
    else if (knowledgeLastRelate && knowledgeLastRelate.from === from && knowledgeLastRelate.to === to
      && knowledgeLastRelate.kind === kind) knowledgeLastRelate = null;
    if (wrote?.changed === false) {
      toast(remove ? t("knowledge.linkAbsent", "그 관계는 이미 없습니다") : t("knowledge.linkPresent", "이미 같은 관계가 있습니다"));
    } else if (remove) {
      toast(t("knowledge.linkUndone", "연결을 되돌렸습니다"));
    } else {
      toast(t("knowledge.linked", "연결했습니다 · ⌘/Ctrl+Z로 되돌리기"), "", {
        action: { label: t("knowledge.undoLink", "되돌리기"), run: () => void relateKnowledge(view, { from, to, kind, remove: true }) },
      });
    }
    await refreshKnowledgeGraph({ force: true });
  } catch (error) {
    showError(String(error));
  }
}

/* 시간 슬라이서 (Bloom 문법): 노드의 수정 시각(modified_ms)과 회상 시각(recalledAt)을
 * 기준으로 디밍만 적용한다. 멤버십·좌표 불변, 재배치 없음. */
/* 선의 옷은 여기서 입히지 않는다: 선의 class는 프레임의 `dress`가 매번 통째로 쓰므로,
 * 여기서 붙인 흐림은 다음 프레임(배율·팬·선택·검색 한 글자)에 지워졌다(K19). 여기는
 * 점마다의 답(`sliceMatch`)과 점의 옷을 두고, 선은 그 답을 읽는 프레임이 입힌다 —
 * 부르는 쪽이 프레임을 그린다. */
function paintKnowledgeSlice(view, layout) {
  const cutoff = knowledgeSlicerCutoff;
  const slicing = cutoff > 0;
  const model = layout.model;
  const inside = layout.sliceMatch;
  for (let at = 0; at < layout.count; at += 1) {
    const time = Math.max(model.modified[at] || 0, model.recalledAt[at] || 0);
    const yes = !slicing || time >= cutoff;
    inside[at] = yes ? 1 : 0;
    layout.nodeEls[at]?.classList.toggle("is-slice-dim", !yes);
  }
  view.querySelector(".knowledge-picture").classList.toggle("is-sliced", slicing);
}

/* 계약의 낱말을 자리로 옮긴다. 낱말을 모르거나(옛 답) 이름이 낯설면 본문의
 * `[[위키링크]]`로 읽는다 — 알 수 없는 관계를 타입 관계로 세우면 「타입
 * 관계만」이 백엔드가 세어 준 수와 어긋난다. */
function knowledgeEdgeCode(edge) {
  const at = KNOWLEDGE_EDGE_KINDS.indexOf(edge.kind ?? "mentions");
  return at < 0 ? 0 : at;
}

/* 자리 → 사람에게 보일 낱말. 볼트에 적힌 다섯 관계는 제 이름 그대로이고, merge
 * 후보는 관계가 아니라 물음이라 물음표를 단다(t-2931). */
function knowledgeEdgeWord(code) {
  return code === KNOWLEDGE_EDGE_CODE.merge
    ? t("knowledge.edgeMerge", "merge?")
    : KNOWLEDGE_EDGE_KINDS[code];
}

/* 관계 낱말의 뜻을, 쉬운 말로 (09-16).
 *
 * 낱말 자체는 번역하지 않는다 — `depends_on`은 프론트매터가 볼트에 적은 **키**이고,
 * 그것을 옮기면 사람이 파일에서 찾을 낱말과 화면의 낱말이 갈린다(1차 규칙). 그래서
 * 키는 키 그대로 두고 그 **뜻**을 한 줄 곁들인다: 「무엇과 어떤 이유로 연결되는가」에
 * 답하는 것은 키가 아니라 이 한 줄이다. 범례와 인스펙터의 관계 줄이 같은 표를 읽는다.
 *
 * 키를 낱말로 짓지 않고 표에 적어 두는 것은 `no_catalog_key_is_shipped_unused`가
 * 코드에서 **글자 그대로**의 키를 찾기 때문이다. */
const KNOWLEDGE_EDGE_WHY = Object.freeze({
  mentions: { key: "knowledge.whyMentions", word: "본문에서 가리킵니다" },
  related: { key: "knowledge.whyRelated", word: "같이 읽을 쪽입니다" },
  implements: { key: "knowledge.whyImplements", word: "이것을 실제로 구현합니다" },
  depends_on: { key: "knowledge.whyDependsOn", word: "이것이 없으면 성립하지 않습니다" },
  supersedes: { key: "knowledge.whySupersedes", word: "이것을 대신합니다" },
  contradicts: { key: "knowledge.whyContradicts", word: "이것과 어긋납니다" },
  merge: { key: "knowledge.whyMerge", word: "내용이 겹쳐 합칠 후보입니다" },
  affects: { key: "knowledge.whyAffects", word: "이 구성요소에 알려진 취약점입니다" },
});

function knowledgeEdgeWhy(kind) {
  const held = KNOWLEDGE_EDGE_WHY[kind];
  return held ? t(held.key, held.word) : "";
}

/* 저장 시야 (Bloom 문법): { lens, tag chips, search text, selected node id,
 * focus depth, camera as fit-relative scale + pan }을 볼트별로 설정 저장소에
 * JSON 한 줄로 보관하고 복원한다.
 *
 * 줄의 주인은 설정 문서(`second_brain_scenes`)다. 창은 제 사본을 따로 들지 않는다 —
 * 부팅 보고와 모든 설정 스냅샷이 `applySettingsSnapshot`을 지나며 이 표를 갈아
 * 끼우고(`noteKnowledgeScenes`), 읽는 쪽은 여기서만 푼다. 창이 메모리에만 든
 * 사본에서 읽던 판은 새로 읽은 창의 목록이 비었고, 그 창의 첫 저장이 백엔드의
 * 「볼트의 줄 통째 교체」로 디스크의 시야를 전부 지웠다(K18). */
let knowledgeSceneLines = {};
/* 푼 줄 — 검색 한 글자마다 목록을 다시 그리므로 줄을 매번 풀지 않는다. 표가
 * 갈리면 함께 버린다. */
const knowledgeSceneStores = new Map();

function noteKnowledgeScenes(lines) {
  knowledgeSceneLines = lines !== null && typeof lines === "object" ? lines : {};
  knowledgeSceneStores.clear();
}

function getVaultSceneStore(vault) {
  if (!vault) return { scenes: [], phrases: [] };
  const held = knowledgeSceneStores.get(vault);
  if (held) return held;
  let store = { scenes: [], phrases: [] };
  const raw = knowledgeSceneLines[vault];
  if (typeof raw === "string" && raw !== "") {
    try {
      const parsed = JSON.parse(raw);
      store = Array.isArray(parsed)
        ? { scenes: parsed, phrases: [] }
        : {
            scenes: Array.isArray(parsed?.scenes) ? parsed.scenes : [],
            phrases: Array.isArray(parsed?.phrases) ? parsed.phrases : [],
          };
    } catch {
      // 읽을 수 없는 줄은 빈 목록이다 — 그 줄에서 되찾을 시야가 없다.
    }
  }
  knowledgeSceneStores.set(vault, store);
  return store;
}

/* 쓰기는 다른 설정과 같은 문(`commitSetting`)을 지난다: 거절되면 권위 있는 문서를
 * 다시 읽어 이 표를 되돌리고 오류를 말한다. 그림은 먼저 — 답을 기다리는 동안에도
 * 목록은 방금 저장한 것을 보인다. */
async function saveVaultSceneStore(vault, store) {
  if (!vault) return;
  const line = JSON.stringify(store);
  knowledgeSceneLines = { ...knowledgeSceneLines, [vault]: line };
  knowledgeSceneStores.set(vault, store);
  await commitSetting("second_brain_scenes", "set_second_brain_scenes", { vault, scenes: line });
}

function captureCurrentKnowledgeScene(name, view, layout) {
  return {
    name,
    lens: {
      orphans: knowledgeOrphansOnly,
      ghosts: knowledgeGhostsOnly,
      typed: knowledgeTypedOnly,
      sources: knowledgeShowSources,
      alive: knowledgeAliveOnly,
      cold: knowledgeColdOnly,
      merge: knowledgeMergeOnly,
      lint: knowledgeLintLens,
      nav: knowledgeNavShown,
      supply: knowledgeSupplyShown,
    },
    tags: [...knowledgeTagsPicked],
    tagChips: [...knowledgeTagsPicked],
    chips: [...knowledgeTagsPicked],
    search: knowledgeQuery,
    searchText: knowledgeQuery,
    selectedNodeId: knowledgeSelectedKey,
    selectedId: knowledgeSelectedKey,
    selected: knowledgeSelectedKey,
    focusDepth: knowledgeFocusDepth,
    depth: knowledgeFocusDepth,
    mode: knowledgeMode,
    camera: {
      scale: layout ? layout.zoom : 1,
      zoom: layout ? layout.zoom : 1,
      panX: layout ? layout.panX : 0,
      panY: layout ? layout.panY : 0,
    },
  };
}

async function restoreKnowledgeScene(view, scene) {
  if (!scene) return;
  knowledgeOrphansOnly = !!scene.lens?.orphans;
  knowledgeGhostsOnly = !!scene.lens?.ghosts;
  knowledgeTypedOnly = !!scene.lens?.typed;
  /* 「원본도」는 답에 없는 점을 만드는 렌즈다 — 깃발이 바뀌면 툴바의 깃발과 같은 문으로
   * 백엔드에 다시 묻는다. 깃발만 세우면 원본 점이 서지 않고, 시야가 고른 원본 점은
   * 「없는 점」으로 떨어졌다. */
  const refetch = knowledgeShowSources !== !!scene.lens?.sources;
  knowledgeShowSources = !!scene.lens?.sources;
  knowledgeAliveOnly = !!scene.lens?.alive;
  /* 냉각 렌즈(건강 카드의 「회상된 적 없음」)도 멤버십을 바꾸는 렌즈라 시야에 든다. */
  knowledgeColdOnly = !!scene.lens?.cold;
  knowledgeMergeOnly = !!scene.lens?.merge;
  knowledgeLintLens = scene.lens?.lint ?? null;
  /* 표시 설정(S6)도 시야의 것이다; 옛 시야(없는 칸)는 켜진 채다. */
  knowledgeNavShown = scene.lens?.nav !== false;
  /* 공급망 렌즈(P4)도 답에 없는 점을 만든다 — 켜지는 시야는 「원본도」와 같은 이유로 다시 묻는다.
   * 옛 시야(없는 칸)는 꺼진 채다. */
  const supplyAsk = !knowledgeSupplyShown && scene.lens?.supply === true;
  knowledgeSupplyShown = scene.lens?.supply === true;

  knowledgeTagsPicked.clear();
  const tags = scene.tags ?? scene.tagChips ?? scene.chips ?? [];
  for (const tag of tags) knowledgeTagsPicked.add(tag);

  knowledgeQuery = scene.search ?? scene.searchText ?? scene.query ?? "";
  const find = view.querySelector(".knowledge-query");
  if (find) find.value = knowledgeQuery;

  knowledgeFocusDepth = scene.focusDepth ?? scene.depth ?? 1;

  if (refetch) await refreshKnowledgeGraph({ force: true });
  else await paintKnowledgeView();
  if (supplyAsk) await refreshKnowledgeSupply({ force: true });

  const layout = knowledgeLayouts.get(view);
  if (layout) {
    const targetId = scene.selectedNodeId ?? scene.selectedId ?? scene.selected ?? null;
    const present = targetId !== null && layout.model.keys.includes(targetId);
    /* 시야의 모드(t-4140): 주변 탐색은 고른 점이 그림에 있을 때만 되살아난다 — 중심
     * 없는 주변은 없다. 카메라는 모드 뒤에 얹는다(모드가 카메라를 놓으므로). */
    if (scene.mode === "local" && present) setKnowledgeMode(view, "local", { centre: targetId, paint: false });
    else if (knowledgeMode === "local") setKnowledgeMode(view, KNOWLEDGE_MODES[0].id, { paint: false });
    const cam = scene.camera;
    if (cam) {
      layout.zoom = cam.scale ?? cam.zoom ?? 1;
      layout.panX = cam.panX ?? 0;
      layout.panY = cam.panY ?? 0;
      layout.zoomTaken = true;
    }
    selectKnowledgeNode(view, present ? targetId : null);
    if (knowledgeMode === "local") await paintKnowledgeView();
    paintKnowledgeFrame(view, layout);
  }
}

function paintKnowledgeSceneList(view) {
  const popover = view.querySelector(".knowledge-scene-popover");
  if (!popover) return;
  const listEl = popover.querySelector(".knowledge-scene-list");
  if (!listEl) return;
  listEl.innerHTML = "";
  const vault = knowledgeReport?.vault ?? secondBrainVault;
  const store = getVaultSceneStore(vault);
  if (!store.scenes || store.scenes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "knowledge-scene-empty";
    empty.dataset.i18n = "knowledge.sceneEmpty";
    empty.textContent = t("knowledge.sceneEmpty", "저장된 시야가 없습니다");
    listEl.appendChild(empty);
    return;
  }
  for (const scene of store.scenes) {
    const item = document.createElement("div");
    item.className = "knowledge-scene-item";

    const loadBtn = document.createElement("button");
    loadBtn.type = "button";
    loadBtn.className = "btn knowledge-scene-load";
    loadBtn.dataset.i18nAria = "knowledge.sceneRestore";
    loadBtn.setAttribute("aria-label", t("knowledge.sceneRestore", "시야 복원"));
    const nameSpan = document.createElement("span");
    nameSpan.className = "knowledge-scene-name";
    nameSpan.textContent = scene.name;
    loadBtn.appendChild(nameSpan);
    loadBtn.onclick = () => void restoreKnowledgeScene(view, scene);

    const deleteBtn = document.createElement("button");
    deleteBtn.type = "button";
    deleteBtn.className = "btn knowledge-scene-delete";
    deleteBtn.dataset.i18nAria = "knowledge.sceneDelete";
    deleteBtn.setAttribute("aria-label", t("knowledge.sceneDelete", "시야 삭제"));
    deleteBtn.textContent = "✕";
    deleteBtn.onclick = async (e) => {
      e.stopPropagation();
      store.scenes = store.scenes.filter((s) => s.name !== scene.name);
      await saveVaultSceneStore(vault, store);
      paintKnowledgeSceneList(view);
    };

    item.append(loadBtn, deleteBtn);
    listEl.appendChild(item);
  }
}

function paintKnowledgeSavedPhrases(view) {
  const listEl = view.querySelector(".knowledge-saved-phrases-list");
  if (!listEl) return;
  listEl.innerHTML = "";
  const vault = knowledgeReport?.vault ?? secondBrainVault;
  const store = getVaultSceneStore(vault);
  const find = view.querySelector(".knowledge-query");
  for (const phrase of store.phrases || []) {
    const chip = document.createElement("div");
    chip.className = "btn knowledge-search-phrase-chip";

    const text = document.createElement("span");
    text.className = "knowledge-phrase-text";
    text.textContent = phrase;
    text.onclick = () => {
      if (find) find.value = phrase;
      knowledgeQuery = phrase;
      followKnowledgeChips();
      void paintKnowledgeView();
    };

    const del = document.createElement("button");
    del.type = "button";
    del.className = "btn knowledge-delete-phrase";
    del.textContent = "✕";
    del.dataset.i18nAria = "knowledge.deletePhrase";
    del.setAttribute("aria-label", t("knowledge.deletePhrase", "검색어 삭제"));
    del.onclick = async (e) => {
      e.stopPropagation();
      store.phrases = (store.phrases || []).filter((p) => p !== phrase);
      await saveVaultSceneStore(vault, store);
      paintKnowledgeSavedPhrases(view);
    };

    chip.append(text, del);
    listEl.appendChild(chip);
  }
}

/* ---- 군집 (3차) ----
 *
 * 결정적 Louvain. 난수가 없다: 노드는 인덱스 순으로 걷고, 이득이 같으면 낮은
 * 군집 id를 고르며, 제 군집과 같으면 머문다. 같은 볼트는 언제나 같은 군집이고,
 * 그래서 이름표도 색도 기억이 된다. 단계마다 군집을 한 점으로 접어 다시 걷고,
 * 더 옮길 것이 없거나 상한에 닿으면 멈춘다. 답은 노드마다의 군집 id(0부터,
 * 처음 나타난 순서)다. 자기 고리(접힌 군집의 안쪽 무게)는 차수에 두 번 들고
 * 이웃 목록에는 들지 않는다 — 표준의 관례 그대로. */
function knowledgeLouvain(count, from, to, weight, edgeCount) {
  const membership = new Int32Array(count);
  for (let at = 0; at < count; at += 1) membership[at] = at;
  let size = count;
  let heads = from;
  let tails = to;
  let weights = weight;
  let edges = edgeCount;
  for (let level = 0; level < KNOWLEDGE_COMMUNITY.maxLevels && edges > 0; level += 1) {
    const degree = new Float64Array(size);
    let total = 0;
    for (let at = 0; at < edges; at += 1) {
      degree[heads[at]] += weights[at];
      degree[tails[at]] += weights[at];
      total += weights[at];
    }
    if (total === 0) break;
    const twoM = total * 2;
    const start = new Int32Array(size + 1);
    for (let at = 0; at < edges; at += 1) {
      if (heads[at] === tails[at]) continue;
      start[heads[at] + 1] += 1;
      start[tails[at] + 1] += 1;
    }
    for (let at = 0; at < size; at += 1) start[at + 1] += start[at];
    const cursor = Int32Array.from(start.subarray(0, size));
    const adjacent = new Int32Array(start[size]);
    const adjacentWeight = new Float64Array(start[size]);
    for (let at = 0; at < edges; at += 1) {
      if (heads[at] === tails[at]) continue;
      adjacent[cursor[heads[at]]] = tails[at];
      adjacentWeight[cursor[heads[at]]] = weights[at];
      cursor[heads[at]] += 1;
      adjacent[cursor[tails[at]]] = heads[at];
      adjacentWeight[cursor[tails[at]]] = weights[at];
      cursor[tails[at]] += 1;
    }
    const community = new Int32Array(size);
    for (let at = 0; at < size; at += 1) community[at] = at;
    const tot = Float64Array.from(degree);
    /* 이웃 군집으로 가는 무게. 무게는 양수라 0이 「아직 안 본 군집」이고, 본
     * 것들은 목록에 적어 두었다가 그것만 되돌린다 — 노드마다 배열을 지우지 않는다. */
    const kin = new Float64Array(size);
    const touched = new Int32Array(size);
    let movedAny = false;
    for (let sweep = 0; sweep < KNOWLEDGE_COMMUNITY.maxSweeps; sweep += 1) {
      let moved = 0;
      for (let at = 0; at < size; at += 1) {
        const own = community[at];
        let touchedCount = 0;
        for (let seat = start[at]; seat < start[at + 1]; seat += 1) {
          const other = community[adjacent[seat]];
          if (kin[other] === 0) {
            touched[touchedCount] = other;
            touchedCount += 1;
          }
          kin[other] += adjacentWeight[seat];
        }
        tot[own] -= degree[at];
        let best = own;
        let bestGain = kin[own] - (tot[own] * degree[at]) / twoM;
        for (let seat = 0; seat < touchedCount; seat += 1) {
          const other = touched[seat];
          if (other === own) continue;
          const gain = kin[other] - (tot[other] * degree[at]) / twoM;
          const better = gain > bestGain + KNOWLEDGE_COMMUNITY.epsilon;
          const tied = best !== own
            && Math.abs(gain - bestGain) <= KNOWLEDGE_COMMUNITY.epsilon
            && other < best;
          if (better || tied) {
            best = other;
            bestGain = gain;
          }
        }
        tot[best] += degree[at];
        if (best !== own) {
          community[at] = best;
          moved += 1;
        }
        for (let seat = 0; seat < touchedCount; seat += 1) kin[touched[seat]] = 0;
      }
      if (moved === 0) break;
      movedAny = true;
    }
    if (!movedAny) break;
    const renamed = new Int32Array(size).fill(-1);
    let next = 0;
    for (let at = 0; at < size; at += 1) {
      if (renamed[community[at]] < 0) {
        renamed[community[at]] = next;
        next += 1;
      }
    }
    for (let at = 0; at < count; at += 1) membership[at] = renamed[community[membership[at]]];
    if (next === size) break;
    /* 접기: 군집 사이의 무게를 더해 다음 단계의 그래프로. Map의 순서는 넣은
     * 순서이고 넣은 순서는 간선의 순서라, 이것도 결정적이다. */
    const merged = new Map();
    for (let at = 0; at < edges; at += 1) {
      const left = renamed[community[heads[at]]];
      const right = renamed[community[tails[at]]];
      const key = left <= right ? left * next + right : right * next + left;
      merged.set(key, (merged.get(key) ?? 0) + weights[at]);
    }
    heads = new Int32Array(merged.size);
    tails = new Int32Array(merged.size);
    weights = new Float64Array(merged.size);
    let seat = 0;
    for (const [key, held] of merged) {
      heads[seat] = Math.floor(key / next);
      tails[seat] = key % next;
      weights[seat] = held;
      seat += 1;
    }
    edges = merged.size;
    size = next;
  }
  return membership;
}

/* 군집의 이름과 태그 칩의 색.
 *
 * 이름은 태그의 tf·idf다 — 군집 안에서 흔하고(tf) 다른 군집에는 드문(idf) 태그.
 * 볼트 27쪽 중 15쪽에 붙은 `zo`는 어느 군집의 이름도 되지 못한다: 모든 군집에
 * 있으므로 idf가 0이다. 군집이 하나뿐이면 idf가 전부 0이라 tf가 고르고, 태그가
 * 하나도 없는 군집은 연결이 가장 많은 멤버의 제목을 쓴다. 동률은 이름순 —
 * 이 목록도 그림처럼 결정적이어야 한다. 칩의 색은 그 태그가 가장 많이 사는
 * 군집의 색이다. */
function knowledgeClusterNames(model, of, size, named) {
  const count = model.count;
  const tagShare = new Map();
  for (let at = 0; at < count; at += 1) {
    if (model.kinds[at] !== "page" || of[at] >= named) continue;
    for (const tag of model.tags[at]) {
      let held = tagShare.get(tag);
      if (held === undefined) {
        held = new Int32Array(named);
        tagShare.set(tag, held);
      }
      held[of[at]] += 1;
    }
  }
  const strongest = new Int32Array(named).fill(-1);
  for (let at = 0; at < count; at += 1) {
    const rank = of[at];
    if (rank >= named || model.kinds[at] !== "page") continue;
    if (strongest[rank] < 0 || model.degree[at] > model.degree[strongest[rank]]) strongest[rank] = at;
  }
  const names = new Array(named).fill("");
  const eps = KNOWLEDGE_COMMUNITY.epsilon;
  /* 한 이름은 한 군집의 것이다(09-16). 같은 태그가 두 군집에서 가장 높은 tf·idf를
   * 받는 일은 흔하고(실측 볼트에서 `zo`가 셋), 이름이 같은 세 덩어리는 이름이
   * 없는 것과 같다 — 순위가 높은 쪽이 먼저 가져가고 다음 군집은 그 다음 태그를
   * 고른다. 남은 태그가 없으면 연결이 가장 많은 멤버의 제목이 이름이 된다. */
  const taken = new Set();
  for (let rank = 0; rank < named; rank += 1) {
    let bestTag = "";
    let bestScore = -1;
    let bestShare = -1;
    for (const [tag, held] of tagShare) {
      if (held[rank] === 0 || taken.has(tag)) continue;
      let seenIn = 0;
      for (let other = 0; other < named; other += 1) if (held[other] > 0) seenIn += 1;
      const share = held[rank] / size[rank];
      const score = share * Math.log(named / seenIn);
      const wins = score > bestScore + eps
        || (Math.abs(score - bestScore) <= eps
          && (share > bestShare + eps
            || (Math.abs(share - bestShare) <= eps && tag < bestTag)));
      if (wins) {
        bestTag = tag;
        bestScore = score;
        bestShare = share;
      }
    }
    if (bestTag !== "") taken.add(bestTag);
    const word = bestTag !== "" ? bestTag
      : strongest[rank] >= 0 ? model.titles[strongest[rank]] : "";
    names[rank] = knowledgeShortWord(word, KNOWLEDGE_COMMUNITY.labelMax);
  }
  const tagHue = new Map();
  for (const row of model.catalog) {
    const held = tagShare.get(row.tag);
    if (held === undefined) continue;
    let home = -1;
    for (let rank = 0; rank < named; rank += 1) {
      if (held[rank] > 0 && (home < 0 || held[rank] > held[home])) home = rank;
    }
    if (home >= 0) tagHue.set(row.tag, home % KNOWLEDGE_HUES);
  }
  /* `strongest`는 군집의 **대표 페이지**이기도 하다(09-16): 배치가 그것을 원반의
   * 가운데에 앉히고, 이름표 격자가 이름을 가장 먼저 세운다. 여기서 이미 세었으므로
   * 배치가 다시 세지 않는다. */
  return { names, tagHue, core: strongest };
}

/* 그림의 군집들 — 순위·크기·색·이름·홈 포인트.
 *
 * 순위는 페이지 수 내림차순(동률은 먼저 나온 노드)이고 색은 순위다. 페이지가
 * `minSize`에 못 미치는 군집은 이름도 색도 홈도 없다(고아 하나가 군집 하나다).
 * 크기가 순위를 정하므로 이름 있는 군집은 언제나 앞의 `named`개다. 홈은 순위
 * 링 위의 자리 — 배치의 군집 인력이 여기로 끈다(Cosmograph의 군집 힘과 같은
 * 자리). 링의 반지름은 군집 수의 제곱근을 따라 자란다: 스물의 군집이 다섯의
 * 링에 서면 성운이 겹친다. */
function knowledgeCommunities(model, tuning) {
  const count = model.count;
  /* 목차·일지가 거는 링크는 세지 않는다(09-16). `wiki/log.md`와 `wiki/index.md`는
   * 볼트의 거의 모든 쪽을 가리키므로(실측 볼트에서 차수 353·336, 전체 간선의 32%)
   * 그것을 주제의 증거로 세면 모든 쪽이 한 주제가 된다 — 목차는 「무엇이 있는가」의
   * 목록이지 「무엇이 무엇과 통하는가」가 아니다. 그림에서 접느냐 펴느냐(`nav` 설정)와
   * 무관하게, **주제를 세는 자리에서는** 언제나 빠진다: 설정 하나가 색을 뒤집으면
   * 사람이 외운 지도가 그때마다 다른 지도가 된다. */
  const navSeat = new Uint8Array(count);
  for (let at = 0; at < count; at += 1) {
    navSeat[at] = KNOWLEDGE_NAV_PAGES.includes(model.keys[at]) ? 1 : 0;
  }
  const from = new Int32Array(model.edgeCount);
  const to = new Int32Array(model.edgeCount);
  const weight = new Float64Array(model.edgeCount);
  let topical = 0;
  for (let at = 0; at < model.edgeCount; at += 1) {
    if (navSeat[model.from[at]] === 1 || navSeat[model.to[at]] === 1) continue;
    from[topical] = model.from[at];
    to[topical] = model.to[at];
    weight[topical] = model.kind[at] === 0
      ? KNOWLEDGE_COMMUNITY.mentionsWeight
      : KNOWLEDGE_COMMUNITY.typedWeight;
    topical += 1;
  }
  const membership = knowledgeLouvain(count, from, to, weight, topical);
  let ids = 0;
  for (let at = 0; at < count; at += 1) ids = Math.max(ids, membership[at] + 1);
  const pages = new Int32Array(ids);
  const first = new Int32Array(ids).fill(-1);
  for (let at = 0; at < count; at += 1) {
    const id = membership[at];
    if (model.kinds[at] === "page") pages[id] += 1;
    if (first[id] < 0) first[id] = at;
  }
  const order = Array.from({ length: ids }, (unused, id) => id)
    .sort((left, right) => pages[right] - pages[left] || first[left] - first[right]);
  const rankOf = new Int32Array(ids);
  order.forEach((id, rank) => {
    rankOf[id] = rank;
  });
  const of = new Int32Array(count);
  for (let at = 0; at < count; at += 1) of[at] = rankOf[membership[at]];
  const size = new Int32Array(ids);
  const hue = new Int8Array(ids);
  let named = 0;
  for (let rank = 0; rank < ids; rank += 1) {
    size[rank] = pages[order[rank]];
    const eligible = size[rank] >= KNOWLEDGE_COMMUNITY.minSize;
    hue[rank] = eligible ? rank % KNOWLEDGE_HUES : -1;
    if (eligible) named += 1;
  }
  const { homeX, homeY, homed, homeR } = knowledgeClusterHomes(model, of, ids, named, tuning);
  const { names, tagHue, core } = knowledgeClusterNames(model, of, size, named);
  return { count: ids, of, size, hue, names, homeX, homeY, homed, homeR, named, tagHue, core };
}

/* 군집 원반의 자리 (성좌 배치, 09-16).
 *
 * PNP 15쪽의 그림에서 읽히는 것은 「큰 중심 하나와 그 둘레의 작은 점들」이라는
 * 한 덩어리가 아니라, 그런 덩어리가 **여럿이고 서로 떨어져 있다**는 사실이다.
 * 여백이 곧 경계다 — 색을 못 보는 눈에도, 이름표가 아직 안 뜬 배율에서도.
 *
 * 그래서 군집마다 원반 하나를 준다. 반지름은 멤버 수의 제곱근에 비례하므로
 * **넓이가 멤버 수에 비례**하고(스무 쪽의 주제가 다섯 쪽의 주제보다 두 배 넓다),
 * 원반끼리는 `gap`만큼 떨어질 때까지 서로를 민다. 그 위에 군집 사이 링크가 인력을
 * 걸어, 실제로 많이 오가는 두 주제가 이웃에 선다 — 배치가 관계를 말하되 없는
 * 관계를 지어내지는 않는다.
 *
 * 이름 없는 군집(페이지 셋 미만 — 고아와 그 부스러기)은 이완에 넣지 않고 바깥
 * 고리에 둘러 세운다. 백 개의 티끌을 이완시키는 값은 이차인데 그 답은 「바깥」
 * 한 마디로 족하다.
 *
 * 난수는 없다. 씨앗은 황금각 나선, 순서는 군집의 순위(멤버 수 내림차순), 동률은
 * 인덱스 — 같은 볼트는 언제나 같은 성좌다. */
function knowledgeClusterHomes(model, of, ids, named, tuning) {
  const homeX = new Float32Array(ids);
  const homeY = new Float32Array(ids);
  const homed = new Uint8Array(ids);
  const homeR = new Float32Array(ids);
  if (ids === 0) return { homeX, homeY, homed, homeR };
  const members = new Int32Array(ids);
  for (let at = 0; at < model.count; at += 1) members[of[at]] += 1;
  for (let rank = 0; rank < ids; rank += 1) {
    const disc = tuning.clusterPitch * Math.sqrt(Math.max(1, members[rank]));
    homeR[rank] = rank < named ? Math.max(tuning.clusterMinRadius, disc) : disc;
    homed[rank] = 1;
  }
  const gap = tuning.clusterGap;
  /* 씨앗: 이름 있는 원반을 황금각 나선에 뿌린다. 나선의 폭은 원반들의 넓이 합에서
   * 나오므로 큰 볼트는 넓게, 작은 볼트는 좁게 시작한다 — 이완이 좁혀야 할 거리도
   * 넓혀야 할 거리도 처음부터 작다. */
  let area = 0;
  for (let rank = 0; rank < named; rank += 1) area += (homeR[rank] + gap / 2) ** 2;
  const spread = Math.sqrt(Math.max(1, area));
  for (let rank = 0; rank < named; rank += 1) {
    const angle = rank * KNOWLEDGE_GOLDEN_ANGLE;
    const reach = spread * Math.sqrt((rank + 0.5) / named);
    homeX[rank] = Math.cos(angle) * reach;
    homeY[rank] = Math.sin(angle) * reach;
  }
  /* 군집 사이의 실. 본문 링크 하나가 1, 프론트매터가 이름을 준 관계가 2 —
   * 커뮤니티를 셀 때와 같은 저울이다. 목차·일지의 링크는 여기서도 세지 않는다. */
  const tie = new Map();
  let strongestTie = 1;
  for (let at = 0; at < model.edgeCount; at += 1) {
    if (KNOWLEDGE_NAV_PAGES.includes(model.keys[model.from[at]])
      || KNOWLEDGE_NAV_PAGES.includes(model.keys[model.to[at]])) continue;
    const left = of[model.from[at]];
    const right = of[model.to[at]];
    if (left === right || left >= named || right >= named) continue;
    const key = left < right ? left * named + right : right * named + left;
    const held = (tie.get(key) ?? 0) + (model.kind[at] === 0
      ? KNOWLEDGE_COMMUNITY.mentionsWeight
      : KNOWLEDGE_COMMUNITY.typedWeight);
    tie.set(key, held);
    if (held > strongestTie) strongestTie = held;
  }
  const ties = [...tie].map(([key, held]) => [Math.floor(key / named), key % named,
    held / strongestTie]);
  const sweeps = named < 2 ? 0 : Math.max(
    KNOWLEDGE_FORCE.clusterMinSweeps,
    Math.min(tuning.clusterSweeps, Math.floor(KNOWLEDGE_FORCE.clusterVisits / (named * named))),
  );
  const link = tuning.clusterLink;
  for (let sweep = 0; sweep < sweeps; sweep += 1) {
    for (const [left, right, pull] of ties) {
      const gapX = homeX[right] - homeX[left];
      const gapY = homeY[right] - homeY[left];
      const reach = Math.hypot(gapX, gapY) || KNOWLEDGE_COMMUNITY.epsilon;
      const rest = homeR[left] + homeR[right] + gap;
      const step = ((reach - rest) * link * pull) / reach;
      homeX[left] += gapX * step;
      homeY[left] += gapY * step;
      homeX[right] -= gapX * step;
      homeY[right] -= gapY * step;
    }
    /* 겹침 풀기. 두 원반이 `gap`보다 가까우면 모자란 만큼을 반씩 나눠 물러선다 —
     * 이 한 줄이 「군집 사이 여백」을 그림의 성질로 만든다. */
    for (let left = 0; left < named; left += 1) {
      for (let right = left + 1; right < named; right += 1) {
        let gapX = homeX[right] - homeX[left];
        let gapY = homeY[right] - homeY[left];
        let reach = Math.hypot(gapX, gapY);
        const rest = homeR[left] + homeR[right] + gap;
        if (reach >= rest) continue;
        if (reach < KNOWLEDGE_COMMUNITY.epsilon) {
          /* 완전히 포갠 둘은 인덱스가 방향을 준다 — 난수 없이, 같은 볼트에서
           * 언제나 같은 쪽으로. */
          const angle = (left * KNOWLEDGE_GOLDEN_ANGLE) % (Math.PI * 2);
          gapX = Math.cos(angle);
          gapY = Math.sin(angle);
          reach = 1;
        }
        const push = (rest - reach) / (2 * reach);
        homeX[left] -= gapX * push;
        homeY[left] -= gapY * push;
        homeX[right] += gapX * push;
        homeY[right] += gapY * push;
      }
    }
  }
  /* 이름 없는 군집은 바깥 고리에 — 고리의 반지름은 이름 있는 원반들이 닿는
   * 가장 먼 자리에서 한 칸 더 나간 곳이고, 고리 위의 자리 사이는 서로의 반지름
   * 합보다 넓다. */
  let fringe = 0;
  for (let rank = 0; rank < named; rank += 1) {
    fringe = Math.max(fringe, Math.hypot(homeX[rank], homeY[rank]) + homeR[rank]);
  }
  const strays = ids - named;
  if (strays > 0) {
    /* 이름 없는 군집 — 고아와 그 부스러기 — 은 이름 있는 벌판 바깥의 띠에 황금각
     * 나선으로 앉는다. 한 줄짜리 고리로 두르면 그 고리의 둘레가 티끌의 수에 비례해
     * 자라고, 그림의 크기를 정하는 것이 주제가 아니라 고아가 된다(실측: 천 쪽 중
     * 이백넷이 고아인 판에서 그림의 폭이 12,860 — 스프링 길이의 280배). 나선은
     * 넓이로 자라므로 그 수가 제곱근으로만 들어온다. */
    let widest = 0;
    for (let rank = named; rank < ids; rank += 1) widest = Math.max(widest, homeR[rank]);
    const pitch = 2 * widest + tuning.clusterPitch;
    const inner = fringe + gap + widest;
    for (let rank = named; rank < ids; rank += 1) {
      const seat = rank - named;
      const angle = seat * KNOWLEDGE_GOLDEN_ANGLE;
      const reach = Math.sqrt(inner * inner + (seat * pitch * pitch) / Math.PI);
      homeX[rank] = Math.cos(angle) * reach;
      homeY[rank] = Math.sin(angle) * reach;
    }
  }
  return { homeX, homeY, homed, homeR };
}

/* 그림 위의 낱말은 짧다 — 제목은 문장이고 문장 서른다섯 개는 안개다. 온전한
 * 제목은 카드의 것이다. */
function knowledgeShortWord(word, max) {
  return word.length > max ? `${word.slice(0, Math.max(1, max - 1))}…` : word;
}

/* 군집의 낱말 — 이름이 없으면 순위로 부른다. */
function knowledgeClusterWord(layout, rank) {
  return layout.communityNames[rank]
    || t("knowledge.clusterUnnamed", "군집 {{rank}}", { rank: rank + 1 });
}

/* ---- 배치 ---- */

function knowledgeLayout(view, model) {
  const held = knowledgeLayouts.get(view);
  const placed = held?.model?.vault === model.vault ? held.placed : new Map();
  // A lens may hide a page temporarily. Only the complete backend reply can
  // retire its position, even when the visible topology has not changed.
  /* 점의 DOM도 자리처럼 볼트의 것이다(t-4140): 주변 탐색이 DOM에서 뺀 점은 여기
   * 남았다가 전체 지도로 돌아올 때 다시 붙는다 — 천 개를 다시 짓는 대신. 삭제된
   * 페이지의 것은 자리와 함께 놓는다. */
  const nodeCache = held?.model?.vault === model.vault ? held.nodeCache : new Map();
  /* 공급망의 답이 새로 온 판(P4)도 온전한 답이다 — 렌즈를 끄고 켜는 몸짓(답 있음 ↔ 없음)은 아니다. */
  const heldSupply = held?.model?.supply?.answer ?? null;
  const supplyAnswered = heldSupply !== null && model.supply !== null && heldSupply !== model.supply.answer;
  if (placed.size > 0 && (held?.model?.total !== model.total || supplyAnswered)) {
    const present = new Set(model.nodeList.map((node) => node.id));
    for (const key of placed.keys()) if (!present.has(key)) placed.delete(key);
    for (const key of nodeCache.keys()) if (!present.has(key)) nodeCache.delete(key);
  }
  if (held?.signature === model.signature) {
    if (held.model !== model) {
      const names = knowledgeClusterNames(model, held.community, held.communitySize, held.namedCount);
      held.communityNames = names.names;
      held.tagHue = names.tagHue;
      /* 같은 위상이면 차수도 같으므로 대표 페이지는 그대로다 — 제목만 새로 읽는다. */
      held.communityCore = names.core;
    }
    held.model = model;
    return held;
  }
  const count = model.count;
  const tuning = knowledgeTuning(view);
  const communities = knowledgeCommunities(model, tuning);
  const tier = knowledgeTiers(model, communities.of, communities.core, communities.named, tuning);
  knowledgeSupplyTiers(model, tier);
  const layout = {
    signature: model.signature,
    model,
    /* 판 자신 — 카드의 서식(S5)이 제 판을 되찾는 손잡이. */
    view,
    tuning,
    count,
    /* 군집은 위상이 바뀔 때만 다시 센다. 제목·태그만 바뀌면 이름만 갱신한다. */
    community: communities.of,
    communityCount: communities.count,
    communitySize: communities.size,
    communityHue: communities.hue,
    communityNames: communities.names,
    communityHomeX: communities.homeX,
    communityHomeY: communities.homeY,
    communityHomed: communities.homed,
    /* 원반의 반지름(09-16). 훑기가 점을 여기 안에 붙들고, 성운과 군집 이름표가
     * 같은 수를 읽는다 — 그림의 경계와 옷이 한 수에서 나온다. */
    communityHomeR: communities.homeR,
    /* 군집의 대표 페이지, 순위별 자리(없으면 -1). */
    communityCore: communities.core,
    namedCount: communities.named,
    tagHue: communities.tagHue,
    /* 점의 단 — 잎·대표 지식·중심. 크기와 이름표 우선순위가 이것을 읽는다. */
    tier,
    /* 이름이 실제로 선 점(1). 격자가 매 프레임 채우고 점의 옷과 군집의 「+n」이
     * 읽는다. */
    labelShown: new Uint8Array(count),
    /* 이름이 **어느 자리**에 섰는가 — 아래(0)·위(1)·오른쪽(2)·왼쪽(3), 서지
     * 않았으면 -1. SVG는 그 자리를 <text>의 좌표로 쓰고 GL은 오버레이의 자리로
     * 쓴다: 두 손이 같은 격자의 답을 읽는다는 것이 이 배열의 뜻이다. */
    labelWhere: new Int8Array(count).fill(-1),
    /* 격자가 그 이름표에게 **예약한 상자의 한가운데**(판 픽셀). 자리 번호
     * (`labelWhere`)는 고리 위에서 네 후보를 모두 0으로 말하므로, 번호에서 자리를
     * 다시 셈하는 손은 고리에서 틀린다 — 그래서 격자가 쥔 상자의 중심을 적고, 요소
     * 없이 그리는 손(GL의 오버레이)은 이것을 그대로 읽는다. 쥔 자리에 서므로
     * 「겹치지 않는다」가 오버레이에서도 참이다. SVG는 제 <text> 좌표를 쓴다. */
    labelAtX: new Float32Array(count),
    labelAtY: new Float32Array(count),
    /* 이름표의 어림 폭(em). 점의 층이 이름을 쓸 때 함께 센다. */
    labelEm: new Float32Array(count),
    /* 군집마다 이름을 접은 쪽의 수 — 「+n」의 n. */
    clusterFolded: new Int32Array(communities.count),
    labelStamp: "",
    /* 성운의 살림 — 프레임마다 다시 채우는 합계와 답. 판을 지을 때 한 번. */
    clusterTally: new Int32Array(communities.count),
    clusterX: new Float32Array(communities.count),
    clusterY: new Float32Array(communities.count),
    clusterSpread: new Float32Array(communities.count),
    /* 군집 이름표가 실제로 선 자리(그림 좌표, x·y 짝) — 이름표 격자가 그 상자를
     * 먼저 예약한다. */
    clusterAt: new Float32Array(communities.count * 2),
    /* 그려진 원반의 반지름 — 이름표의 높이가 이것에서 나온다. */
    clusterReach: new Float32Array(communities.count),
    clusterEls: [],
    /* 사람의 손(3차): 쥔 점, 방금 끈 몸짓(클릭을 삼킨다), 카메라 비행, 한
     * 프레임의 훑기 수. */
    pinned: -1,
    dragged: false,
    flight: null,
    pace: 1,
    x: new Float32Array(count),
    y: new Float32Array(count),
    drawY: new Float32Array(count),
    vx: new Float32Array(count),
    vy: new Float32Array(count),
    /* 점이 차지하는 원 — 모양은 이 원에 내접하고, 넓이는 크기가 말한 원과 같다
     * (`KNOWLEDGE_SHAPES`의 `reach`). */
    radius: Float32Array.from(model.degree, (degree, at) => knowledgeNodeRadius(
      degree,
      tuning,
      tier[at],
    ) * KNOWLEDGE_SHAPES[knowledgeShapeOf(model.kinds[at])].reach),
    /* 반발 격자의 살림. 한 번 지어 두고 매 훑기마다 다시 채운다 — 프레임마다
     * 배열 넷을 새로 짓는 것이 배치보다 비싸지는 판이 있다. */
    cellOf: new Int32Array(count),
    order: new Int32Array(count),
    counts: new Int32Array(KNOWLEDGE_FORCE.cells * KNOWLEDGE_FORCE.cells + 1),
    cursor: new Int32Array(KNOWLEDGE_FORCE.cells * KNOWLEDGE_FORCE.cells),
    bounds: { minX: -1, maxX: 1, minY: -1, maxY: 1 },
    zoom: held?.zoom ?? 1,
    panX: 0,
    panY: 0,
    scale: 1,
    /* 배율의 역수 — 화면 픽셀로 서야 하는 것들이 읽는다. */
    inverse: 1,
    /* 사람이 배율을 쥐었으면 판은 저절로 앉지 않는다 — 에이전트 그래프의 같은
     * 문(`agentGraphZoomTaken`)과 같은 이유로. 쥔 순간의 그림 폭도 함께 든다. */
    zoomTaken: held?.zoomTaken ?? false,
    /* 얼린 폭과 높이는 아래에서 정한다 — 새 점이 하나도 없는 판만 물려받는다. */
    fitSpan: 0,
    fitTall: 0,
    /* 이미 놓여 있던 자리. 필터를 바꿔도 남은 점은 제자리에서 시작한다 —
     * 「왼쪽 위의 그 덩어리」가 필터 한 번에 사라지지 않게. 볼트가 바뀌면
     * 버린다: 두 볼트가 같은 파일 이름을 쓰는 것은 흔한 일이고, 남의 볼트에서
     * 얻은 자리에서 시작한 배치는 같은 볼트를 두 번 다르게 그린다. */
    placed,
    nodeCache,
    geometryRevision: 0,
    paintedGeometry: null,
    nodeEls: [],
    measured: [],
    /* 그려지는 부분집합(t-4140). `null`은 전부다. 주변 탐색과 표시 설정이 채우고,
     * 점·선의 DOM, 배치의 훑기, 경계, 통계가 이것을 읽는다. 도장은 「무엇이 그려지는가」
     * 의 서명 — 위상은 그대로인데 도장이 바뀌면 점의 층만 다시 선다. */
    /* 판 크기의 사본 — 한 번의 그리기 안에서, 쓰기 전에 읽은 것(`knowledgeViewBox`). */
    viewport: null,
    /* 지금 보고 있는 그림 좌표의 창 — 이름표가 화면 밖으로 나갔는지를 이것으로
     * 가른다. 프레임이 채운다. */
    viewBoxRect: null,
    /* 이름표 후보의 순서(단 → 차수 → 열쇠). 위상이 정하므로 판과 함께 한 번. */
    labelOrder: null,
    /* 자리를 화면 픽셀로 옮기는 투영기(6차). 판과 함께 한 번 지어지고 프레임마다
     * 카메라만 갈아 낀다 — 이름표 격자와 픽킹이 이것 하나를 묻는다. */
    project: null,
    /* 한 프레임의 일감 수 — 페인터가 적고 하네스가 읽는다. 판마다 한 객체이고
     * 제자리에서 고쳐 쓴다. */
    paintStats: { draws: 0, points: 0, edges: 0, labels: 0 },
    /* 이름표 격자의 살림 — 칸마다 마지막으로 예약된 세대와 그 칸의 임자(점의
     * 자리 + 1, 이름표면 0). 지우지 않고 세대를 올린다(천 칸을 매 프레임 지우는
     * 값을 내지 않는다). */
    labelCells: null,
    labelOwner: null,
    labelCols: 0,
    labelRows: 0,
    labelGeneration: 0,
    drawn: null,
    drawnEdge: null,
    drawnStamp: "all",
    drawnMeasured: null,
    drawnMeasuredStamp: "",
    /* 선의 <path>, 선 번호로. `paintGraphEdges`가 세운 순서는 **그려진** 선의 순서라
     * 부분집합에서는 `children[at]`이 `edges[at]`이 아니다 — 이 표가 그 사이를 잇는다. */
    edgeEls: [],
    edgeElsStamp: "",
    /* 접힌 허브: 자리마다 생략한 연결 수(0이면 접히지 않음). 전체 지도에서는 공급망 멤버의 접힌
     * 구성요소 수다(P4) — 새 판이 「전부」의 도장으로 서도 그 수를 들고 선다. */
    folded: model.supplyFolded === null ? null : Int32Array.from(model.supplyFolded),
    /* 주변 탐색의 고리(시안 이식). 고리가 서 있는 동안 `x`·`y`는 화면의 고리 좌표이고
     * 전체 지도의 자리는 `mapX`·`mapY`(와 그 경계)에 들어 있다 — 돌아갈 때 그대로
     * 되돌아온다. `ring`은 고리의 중심·반지름(깊이별)·눌림; `null`이면 지도 그대로다.
     * `ringAngle`은 자리마다의 각도 — 바깥 고리가 어버이의 각도로 줄을 선다. */
    ring: null,
    mapX: null,
    mapY: null,
    mapBounds: { minX: -1, maxX: 1, minY: -1, maxY: 1 },
    ringAngle: new Float32Array(count),
    /* 고리 위 이름표의 자리(점 기준 x·y·방향) — 격자가 읽는다. */
    ringLabelAt: new Float32Array(count * 3),
    /* 목차·일지를 접은 판의 선 가면(S6) — 고리가 선 동안 지도를 훑을 때 읽는다. */
    navEdgeMask: null,
    left: 0,
    /* 지금 밝혀 둔 것 — 점과 선을 따로 센다. 되돌릴 때 훑을 자리가 이것이고,
     * 그래서 호버 한 번의 비용은 이웃의 수이지 그림의 크기가 아니다. */
    lit: new Set(),
    litNodes: new Set(),
    /* 포커스 BFS와 검색은 같은 크기의 배열을 제자리에서 다시 쓴다. */
    focusVisited: new Uint8Array(count),
    focusQueue: new Int32Array(count),
    focusLevel: new Uint8Array(count),
    focusEdgeVisited: new Uint8Array(model.edgeCount),
    focusNodeCount: 0,
    focusEdgeList: new Int32Array(model.edgeCount),
    focusEdgeCount: 0,
    /* 주변 탐색의 BFS 살림(t-4140) — 포커스의 것과 따로다: 둘은 같은 판에서 다른
     * 깊이로 걷고, 한 벌을 나눠 쓰면 뒤에 걷는 쪽이 앞의 답을 지운다. */
    drawnQueue: new Int32Array(count),
    drawnLevel: new Uint8Array(count),
    /* 관리용 문서의 가면(S6) — 접을 때 한 번 채운다. */
    navMask: null,
    searchMatch: new Uint8Array(count),
    /* 슬라이서의 답(점마다: 창 안인가)과 최단 경로의 답(선마다: 경로인가). 선의
     * 옷은 프레임의 `dress`가 이 배열에서 읽는다 — 검색의 `searchMatch`와 같은 손. */
    sliceMatch: new Uint8Array(count).fill(1),
    pathEdge: new Uint8Array(model.edgeCount),
    /* 경로 위의 점(6차). SVG는 이것을 클래스로 입고 GL은 플래그로 읽는다 — 한
     * 배열이라 두 그림이 같은 점을 밝힌다. */
    pathNode: new Uint8Array(count),
    pathShown: false,
    /* 이 그림에서 찾은 경로의 길(`knowledgeRoute`) — 자리와 선의 번호라 판과 함께 산다. */
    pathChain: null,
    /* 밝은 선의 낱말들. 위상이 바뀌면 이 판과 함께 버려진다 — 인덱스가 다른
     * 선을 가리키게 된 낱말은 틀린 낱말이다. */
    edgeLabels: new Map(),
  };
  layout.labelOrder = knowledgeLabelOrder(model, tier);
  layout.project = knowledgeProjector(layout);
  const fresh = knowledgeSeat(layout);
  layout.left = knowledgeBudget(count, fresh);
  layout.pace = knowledgePace(layout.left, layout.tuning);
  /* 얼린 폭(`fitSpan`)은 **모든 점이 이미 놓여 있던 판**만 물려받는다. 새 점이 있는
   * 판은 앉으면서 폭이 크게 바뀌므로 옛 폭은 틀린 배율이다 — 실측: 마흔 쪽에서
   * 얼린 폭을 천 쪽이 물려받자 그림이 네 배로 확대되고 성운 열아홉이 화면 전체를
   * 덮는 그라데이션이 되어 프레임이 50→265ms가 됐다. 사람이 다시 카메라를 쥐면
   * 그때의 폭이 얼린다. */
  layout.fitSpan = fresh === 0 ? (held?.fitSpan ?? 0) : 0;
  layout.fitTall = fresh === 0 ? (held?.fitTall ?? 0) : 0;
  knowledgeBounds(layout);
  /* 짚고 있던 점은 이 그림에 없을 수 있다(필터가 그것을 걸렀다면). 들고 있으면
   * 같은 열쇠로 들어온 다음 몸짓이 「바뀐 것 없음」으로 일찍 돌아서고, 흐림이
   * 걷히지 않은 판이 남는다. 밝힌 군집도 같다 — 순위는 이 위상의 것이다. */
  knowledgeHoverKey = null;
  knowledgeClusterPicked = -1;
  knowledgeLayouts.set(view, layout);
  return layout;
}

/* 첫 자리 (성좌 배치, 09-16).
 *
 * 이미 놓여 있던 점은 제자리에서 시작한다(공간 기억). 새 점은 **제 군집의 원반
 * 안에** 앉는데, 그 안에서의 자리는 군집의 중심에서 몇 걸음 떨어졌는가로 정한다:
 * 중심은 홈에, 한 걸음은 첫 고리에, 두 걸음은 둘째 고리에 — 뉴런의 가지가 여기서
 * 나온다. 같은 고리 위의 점들은 고르게 벌어지고, 바깥 고리의 점은 제 어버이의
 * 각도 곁에 선다(주변 탐색의 고리와 같은 규칙, `knowledgeRingArrange`).
 *
 * 걸음은 군집 **안**에서만 센다 — 옆 군집을 지나가는 길은 이 원반의 자리를
 * 정하지 않는다. 군집 안에서 중심에 닿지 않는 점(다른 부분으로 갈라진 조각)은
 * 가장 바깥 고리에 선다.
 *
 * 난수는 없다: 걸음도 순서도 각도도 전부 입력에서 나온다. */
function knowledgeSeat(layout) {
  const { count, model, x, y, community, communityHomeX, communityHomeY, communityHomeR } = layout;
  const { start, neighbour, keys } = model;
  const { coreRing, coreRingGap } = layout.tuning;
  let fresh = 0;
  const wanted = [];
  for (let at = 0; at < count; at += 1) {
    const seated = layout.placed.get(keys[at]);
    if (seated) {
      x[at] = seated[0];
      y[at] = seated[1];
      continue;
    }
    fresh += 1;
    wanted.push(at);
  }
  if (fresh === 0) return 0;
  /* 군집 안의 걸음 수 — 중심에서 시작하는 너비 우선. 한 번의 훑기로 모든 군집을
   * 함께 센다(큐 하나, 방문표 하나). */
  const hop = new Int32Array(count).fill(-1);
  const queue = layout.drawnQueue;
  let head = 0;
  let tail = 0;
  for (let rank = 0; rank < layout.namedCount; rank += 1) {
    const seat = layout.communityCore[rank];
    if (seat < 0) continue;
    hop[seat] = 0;
    queue[tail] = seat;
    tail += 1;
  }
  while (head < tail) {
    const seat = queue[head];
    head += 1;
    for (let edge = start[seat]; edge < start[seat + 1]; edge += 1) {
      const other = neighbour[edge];
      if (hop[other] >= 0 || community[other] !== community[seat]) continue;
      hop[other] = hop[seat] + 1;
      queue[tail] = other;
      tail += 1;
    }
  }
  /* 고리마다 몇 개가 서는가 — 자리를 나눠 주기 전에 센다. 중심에 닿지 않은 점
   * (hop < 0)은 한 칸 더 바깥의 고리로 보낸다. */
  const deepest = new Map();
  const ringOf = new Int32Array(count);
  const tally = new Map();
  const ringKey = (rank, ring) => rank * (count + 2) + ring;
  for (const at of wanted) {
    const rank = community[at];
    const ring = hop[at] >= 0 ? hop[at] : -1;
    ringOf[at] = ring;
    const held = deepest.get(rank) ?? 0;
    if (ring > held) deepest.set(rank, ring);
  }
  for (const at of wanted) {
    const rank = community[at];
    if (ringOf[at] < 0) ringOf[at] = (deepest.get(rank) ?? 0) + 1;
    const key = ringKey(rank, ringOf[at]);
    tally.set(key, (tally.get(key) ?? 0) + 1);
  }
  const seatOn = new Map();
  for (const at of wanted) {
    const rank = community[at];
    const ring = ringOf[at];
    if (ring === 0) {
      x[at] = communityHomeX[rank];
      y[at] = communityHomeY[rank];
      continue;
    }
    const key = ringKey(rank, ring);
    const on = seatOn.get(key) ?? 0;
    seatOn.set(key, on + 1);
    const room = tally.get(key) ?? 1;
    /* 각도는 고리 위의 자리 순서에서, 고리마다 황금각만큼 돌려 둔다 — 안쪽과
     * 바깥쪽의 바퀴살이 한 줄로 겹치지 않게. */
    const angle = ((on + 0.5) / room) * Math.PI * 2 + ring * KNOWLEDGE_GOLDEN_ANGLE;
    const reach = Math.min(
      communityHomeR[rank] * KNOWLEDGE_FORCE.initialRadius,
      coreRing + (ring - 1) * coreRingGap,
    );
    x[at] = communityHomeX[rank] + Math.cos(angle) * reach;
    y[at] = communityHomeY[rank] + Math.sin(angle) * reach;
  }
  return fresh;
}

/* 반복 예산.
 *
 * 대부분이 새 점이면 처음부터 앉히고, 대부분이 이미 놓여 있던 점이면(필터를
 * 바꾼 판) 자리를 다듬는 만큼만 돈다. 천 노드 위에서 예산이 줄어드는 것은 한
 * 번의 훑기가 노드 수에 비례해 비싸지기 때문이다 — 예산을 그대로 두면 큰 판에
 * 서만 창이 오래 바쁘다. */
function knowledgeBudget(count, fresh) {
  if (count === 0) return 0;
  if (fresh === 0) return 0;
  if (fresh * 2 <= count) return KNOWLEDGE_FORCE.settleBudget;
  if (count <= KNOWLEDGE_FORCE.smallGraph) return KNOWLEDGE_FORCE.budget;
  return Math.max(
    KNOWLEDGE_FORCE.minBudget,
    Math.round((KNOWLEDGE_FORCE.budget * KNOWLEDGE_FORCE.smallGraph) / count),
  );
}

/* 그림의 경계 — 그려진 점들의 것이다(t-4140): 주변 탐색의 카메라는 하위 그래프를
 * 채우고, 전체 지도로 돌아오면 다시 전체를 잰다. 고리가 서 있으면 경계는 고리의
 * 것이다(시안 이식): 이웃이 셋뿐이어도 중심이 화면의 가운데에 선다 — 점들의 경계로
 * 재면 성긴 고리의 중심이 한쪽으로 밀린다. */
function knowledgeBounds(layout) {
  const ring = layout.ring;
  if (ring !== null) {
    const bounds = layout.bounds;
    bounds.minX = ring.x - ring.radius;
    bounds.maxX = ring.x + ring.radius;
    bounds.minY = ring.y - ring.radius * ring.squash;
    bounds.maxY = ring.y + ring.radius * ring.squash;
    return;
  }
  knowledgeExtent(layout.count, layout.x, layout.y, layout.drawn, layout.bounds);
}

/* 좌표 배열의 경계 — 그려진 점만(`drawn`이 `null`이면 전부). 지도의 경계(`mapBounds`)와
 * 화면의 경계가 같은 손으로 잰다. */
function knowledgeExtent(count, x, y, drawn, bounds) {
  let seen = 0;
  let minX = 0;
  let maxX = 0;
  let minY = 0;
  let maxY = 0;
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    if (seen === 0) {
      minX = maxX = x[at];
      minY = maxY = y[at];
    } else {
      if (x[at] < minX) minX = x[at];
      if (x[at] > maxX) maxX = x[at];
      if (y[at] < minY) minY = y[at];
      if (y[at] > maxY) maxY = y[at];
    }
    seen += 1;
  }
  if (seen === 0) {
    bounds.minX = -1;
    bounds.maxX = 1;
    bounds.minY = -1;
    bounds.maxY = 1;
    return;
  }
  bounds.minX = minX;
  bounds.maxX = maxX;
  bounds.minY = minY;
  bounds.maxY = maxY;
}

/* 한 번의 훑기. 격자를 다시 채우고, 반발과 인장과 중력을 세고, 적분한다.
 *
 * 이 함수 안에 DOM도 시계도 없다: 부르는 쪽(`knowledgeSettle`)이 프레임 예산을
 * 지키고, 이것은 순수하게 수를 움직인다. */
function knowledgeForceStep(layout, x = layout.x, y = layout.y, drawn = layout.drawn,
  drawnEdge = layout.drawnEdge, bounds = layout.bounds) {
  /* 좌표·부분집합·경계는 인자다: 고리가 서 있는 동안 `knowledgeSettle`은 화면의 배열
   * 대신 지도의 배열(`mapX`·`mapY`·`mapBounds`)을 넘긴다 — 같은 훑기, 다른 종이. */
  const { count, vx, vy, cellOf, order, counts, cursor } = layout;
  const { from, to, edgeCount } = layout.model;
  if (count === 0) return;
  knowledgeLayoutRuns += 1;
  const { minX, maxX, minY, maxY } = bounds;
  const span = Math.max(maxX - minX, maxY - minY, 1);
  const cells = Math.min(
    KNOWLEDGE_FORCE.cells,
    Math.max(1, Math.ceil(span / KNOWLEDGE_FORCE.cell)),
  );
  const size = span / cells;
  const buckets = cells * cells;
  counts.fill(0, 0, buckets + 1);
  /* 그려지지 않은 점과 선은 훑기에서도 빠진다(t-4140): 주변 탐색의 배치 비용은
   * 하위 그래프의 것이고, 숨은 점은 제자리에 남는다 — 그것이 공간 기억이다. */
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const col = Math.min(cells - 1, Math.max(0, Math.floor((x[at] - minX) / size)));
    const row = Math.min(cells - 1, Math.max(0, Math.floor((y[at] - minY) / size)));
    const home = row * cells + col;
    cellOf[at] = home;
    counts[home + 1] += 1;
  }
  for (let at = 0; at < buckets; at += 1) counts[at + 1] += counts[at];
  cursor.set(counts.subarray(0, buckets));
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    order[cursor[cellOf[at]]] = at;
    cursor[cellOf[at]] += 1;
  }
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const home = cellOf[at];
    const col = home % cells;
    const row = (home - col) / cells;
    let pushX = 0;
    let pushY = 0;
    for (let downRow = -1; downRow <= 1; downRow += 1) {
      const nearRow = row + downRow;
      if (nearRow < 0 || nearRow >= cells) continue;
      for (let acrossCol = -1; acrossCol <= 1; acrossCol += 1) {
        const nearCol = col + acrossCol;
        if (nearCol < 0 || nearCol >= cells) continue;
        const bucket = nearRow * cells + nearCol;
        const stop = Math.min(counts[bucket + 1], counts[bucket] + KNOWLEDGE_FORCE.neighbours);
        for (let where = counts[bucket]; where < stop; where += 1) {
          const other = order[where];
          if (other === at) continue;
          let gapX = x[at] - x[other];
          let gapY = y[at] - y[other];
          let far = gapX * gapX + gapY * gapY;
          if (far < 0.01) {
            /* 겹친 두 점을 떼는 손짓도 인덱스에서 만든다. 난수를 쓰면 같은
             * 볼트가 두 번 다른 그림이 되고, 이 그림은 사람의 기억이다. */
            gapX = ((at % KNOWLEDGE_FORCE.overlapModuloX) - KNOWLEDGE_FORCE.overlapCentreX)
              * KNOWLEDGE_FORCE.overlapScale + KNOWLEDGE_FORCE.overlapNudge;
            gapY = ((other % KNOWLEDGE_FORCE.overlapModuloY) - KNOWLEDGE_FORCE.overlapCentreY)
              * KNOWLEDGE_FORCE.overlapScale + KNOWLEDGE_FORCE.overlapNudge;
            far = gapX * gapX + gapY * gapY;
          }
          const reach = Math.sqrt(far);
          const clearance = (layout.radius[at] + layout.radius[other])
            * KNOWLEDGE_FORCE.collisionGap;
          const collision = reach < clearance
            ? (clearance - reach) * KNOWLEDGE_FORCE.collisionStrength
            : 0;
          const push = KNOWLEDGE_FORCE.repulsion / far + collision;
          pushX += (gapX / reach) * push;
          pushY += (gapY / reach) * push;
        }
      }
    }
    vx[at] += pushX;
    vy[at] += pushY;
  }
  const springCommunity = layout.community;
  for (let at = 0; at < edgeCount; at += 1) {
    if (drawnEdge !== null && drawnEdge[at] === 0) continue;
    const left = from[at];
    const right = to[at];
    const gapX = x[right] - x[left];
    const gapY = y[right] - y[left];
    const reach = Math.sqrt(gapX * gapX + gapY * gapY) || 0.01;
    const strength = springCommunity[left] === springCommunity[right]
      ? KNOWLEDGE_FORCE.spring
      : KNOWLEDGE_FORCE.spring * KNOWLEDGE_FORCE.interSpring;
    const pull = ((reach - KNOWLEDGE_FORCE.springLength) * strength) / reach;
    vx[left] += gapX * pull;
    vy[left] += gapY * pull;
    vx[right] -= gapX * pull;
    vy[right] -= gapY * pull;
  }
  const { community, communityHomed, communityHomeX, communityHomeY, communityHomeR,
    communityCore, pinned } = layout;
  const { clusterPull, clusterHold } = layout.tuning;
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    /* 제 군집의 홈으로 끌리고, 제 원반 안에 머문다 (성좌 배치, 09-16).
     *
     * 3차의 군집 인력은 「방향만 주는」 약한 힘이었고(0.006), 그래서 열넷의 군집이
     * 한 덩어리로 겹쳤다 — 실측: 412점 2,168선의 볼트에서 모든 색이 같은 자리에
     * 섞였다. 지금 홈은 원반의 가운데이고, 원반 밖으로 나간 점은 테두리로 되당겨진다.
     * 되당김이 인력보다 센 것이 요점이다: 옆 군집의 스프링이 점 하나를 끌어당겨도
     * 그 점은 제 주제의 원반을 떠나지 않는다.
     *
     * 이 인력이 예전의 전역 중력을 대신한다. 모든 군집이 홈을 가지므로 원점으로
     * 끄는 힘은 「어느 주제에도 속하지 않는 가운데」를 만들 뿐이었다. */
    const rank = community[at];
    if (communityHomed[rank] === 1) {
      const towardX = communityHomeX[rank] - x[at];
      const towardY = communityHomeY[rank] - y[at];
      /* 모두가 제 주제의 홈으로 끌리고, 대표 페이지는 더 세게 끌린다.
       *
       * 두 힘이 하나로 모이면 PNP 15쪽의 모양이 된다: 가운데에 큰 중심 하나가
       * 앉고(`coreGrip`이 그것을 놓지 않는다) 나머지가 그 둘레에 모인다. 잎에게도
       * 인력이 필요한 것은, 그것이 없으면 이웃 주제로 뻗은 스프링이 점들을 원반의
       * 한쪽 벽에 몰아붙이기 때문이다(실측 09-16: 중심만 끄는 판에서 군집의 공이
       * 원반의 가장자리로 밀려 빈 원이 남았다). */
      vx[at] += towardX * clusterPull;
      vy[at] += towardY * clusterPull;
      if (at === communityCore[rank]) {
        vx[at] += towardX * clusterPull * KNOWLEDGE_FORCE.coreGrip;
        vy[at] += towardY * clusterPull * KNOWLEDGE_FORCE.coreGrip;
      }
      const far = Math.hypot(towardX, towardY);
      const room = communityHomeR[rank];
      if (far > room) {
        const over = ((far - room) / far) * clusterHold;
        vx[at] += towardX * over;
        vy[at] += towardY * over;
      }
    }
    /* 이웃 군집으로 건너가는 스프링은 약하게 당긴다(09-16). 두 주제를 잇는 선은
     * 「이 둘이 통한다」는 사실이지 「이 둘이 한자리에 있어야 한다」는 요구가
     * 아니다 — 같은 세기로 당기면 원반 스물이 다시 한 덩어리가 된다. 선은 그대로
     * 그려지고, 달라지는 것은 배치가 그 선을 얼마나 믿는가뿐이다. */
    /* 쥔 점은 포인터의 것이다 — 이웃에게 힘은 주되 제 자리는 옮기지 않는다. */
    if (at === pinned) {
      vx[at] = 0;
      vy[at] = 0;
      continue;
    }
    let stepX = vx[at] * KNOWLEDGE_FORCE.damping;
    let stepY = vy[at] * KNOWLEDGE_FORCE.damping;
    const speed = Math.sqrt(stepX * stepX + stepY * stepY);
    if (speed > KNOWLEDGE_FORCE.maxStep) {
      const trim = KNOWLEDGE_FORCE.maxStep / speed;
      stepX *= trim;
      stepY *= trim;
    }
    vx[at] = stepX;
    vy[at] = stepY;
    x[at] += stepX;
    y[at] += stepY;
    /* 원반 밖으로 나간 점은 테두리 위로 되돌린다 — 힘이 아니라 **제약**이다.
     *
     * 무른 되당김만으로는 부족했다(실측 09-16: 412쪽 볼트에서 열셋의 원반이
     * 가운데에서 다시 겹쳤다): 차수 스물의 점 하나가 이웃 군집으로 뻗은 선
     * 스물에 끌리면 그 합이 되당김을 이긴다. 이 한 줄이 「원반끼리 겹치지 않으면
     * 군집끼리도 겹치지 않는다」를 그림의 성질로 만든다 — 하네스가 점마다 잴 수
     * 있는 성질로. */
    if (communityHomed[community[at]] === 1) {
      const rank = community[at];
      const outX = x[at] - communityHomeX[rank];
      const outY = y[at] - communityHomeY[rank];
      const far = Math.hypot(outX, outY);
      const room = communityHomeR[rank];
      if (far > room && far > KNOWLEDGE_COMMUNITY.epsilon) {
        x[at] = communityHomeX[rank] + (outX / far) * room;
        y[at] = communityHomeY[rank] + (outY / far) * room;
      }
    }
  }
  knowledgeExtent(count, x, y, drawn, bounds);
}

/* ---- 그리기 ----
 *
 * 배율은 viewBox다. 그림은 제 좌표계에 살고 판이 그중 어디를 보는지만 바뀌므로,
 * 배율 한 번에 리플로가 없고 간선 이천 개를 다시 재지도 않는다. 그 대신 점과
 * 낱말은 배율의 **역수**를 제 transform에 걸고 있다 — 그래야 들어가도 점이
 * 커지지 않고 제목이 판을 뒤덮지 않는다(Obsidian의 그래프도 그렇게 움직인다). */
function knowledgeViewBox(view, layout) {
  /* 판의 크기는 그림을 쓰기 **전에** 읽어 둔 것을 쓴다(`paintKnowledgeView`가 든다).
   * 점과 선을 쓴 뒤에 `clientWidth`를 읽으면 그 읽기가 방금 쓴 삼천 개의 요소를 동기
   * 레이아웃으로 재게 한다 — 실측: 천 쪽의 전체 지도 복귀에서 14ms가 이 한 줄이었다.
   * 그 밖의 길(팬·배율·비행·리사이즈)은 깨끗한 판을 읽으므로 값이 없다. */
  const canvas = view.querySelector(".knowledge-canvas");
  const held = layout.viewport;
  const wide = (held !== null && held.fresh ? held.wide : canvas.clientWidth) || 1;
  const tall = (held !== null && held.fresh ? held.tall : canvas.clientHeight) || 1;
  const { minX, maxX, minY, maxY } = layout.bounds;
  /* 배율의 바탕은 그림의 폭이다 — 사람이 카메라를 쥐기 전까지. 앉는 동안 그림이
   * 자라며 배율이 따라 줄어드는 것은 첫인상의 피어남이지만, 점을 끌거나 바퀴를
   * 돌린 뒤에도 폭을 따라가면 끄는 손 아래에서 화면이 숨을 쉰다(실측: 60px 끈
   * 점이 53px로 읽혔다). 그래서 손이 닿는 순간의 폭을 얼려 둔다(`fitSpan`). */
  let scale;
  let centreOffsetY = 0;
  if (layout.ring !== null) {
    /* 주변 탐색의 고리는 두 축 다 판 안에 든다(contain, 시안 이식): 폭으로만 맞추면
     * 세로가 긴 판에서 이웃이 판 밖에 섰다. 고리의 경계는 훑기로 자라지 않으므로
     * 얼린 폭이 필요 없다. 좁은 티어의 세로 눌림은 그린 높이에 이미 들어 있다. */
    const spanX = Math.max(maxX - minX, 1);
    const spanY = Math.max((maxY - minY) * knowledgeYScale(view, layout), 1);
    const labelRoom = Math.min(layout.tuning.ringLabelRoom, wide * (1 - layout.tuning.fitRatio));
    // Leave room for the two navigation rows above and the legend/zoom below.
    // Cap the gutters on short leaves so the camera always has a positive span.
    const topRoom = Math.min(layout.tuning.margin * 2, tall * (1 - layout.tuning.fitRatio));
    const bottomRoom = Math.min(layout.tuning.margin, tall * (1 - layout.tuning.fitRatio));
    scale = Math.min((wide - labelRoom * 2) / spanX,
      ((tall - topRoom - bottomRoom) * layout.tuning.fitRatio) / spanY)
      * layout.zoom;
    centreOffsetY = (bottomRoom - topRoom) / (2 * scale);
  } else {
    const spanX = layout.zoomTaken && layout.fitSpan > 0
      ? layout.fitSpan
      : Math.max(maxX - minX, 1);
    /* 두 축 다 판 안에 넣는다(contain, 09-16).
     *
     * 폭만 맞추던 옛 셈은 판이 짧아지면 그림의 위아래를 잘랐고, 잘린 자리의 주제
     * 이름판은 판 안으로 끌려와 서로 포개졌다(실측 900×760: 겹친 이름 상자 110쌍).
     * 세로 눌림(`knowledgeYScale`)은 그 잘림을 줄일 뿐 없애지 못한다 — 없애는
     * 것은 두 축을 다 재는 이 한 줄이다. 두 축의 배율은 여전히 같은 수이므로
     * 점도 원반도 찌그러지지 않는다(원반의 눌림은 `ry`가 따로 든다).
     *
     * 폭을 얼린 판(사람이 카메라를 쥔 뒤)은 세로도 그때의 것을 쓴다 — 끄는 손
     * 아래에서 화면이 숨 쉬지 않게. */
    const spanY = Math.max(
      (layout.zoomTaken && layout.fitTall > 0 ? layout.fitTall : (maxY - minY))
        * knowledgeYScale(view, layout),
      1,
    );
    scale = Math.min((wide * layout.tuning.fitRatio) / spanX,
      (tall * layout.tuning.fitRatio) / spanY) * layout.zoom;
  }
  const boxWide = wide / scale;
  const boxTall = tall / scale;
  const midX = (minX + maxX) / 2 + layout.panX;
  const midY = (minY + maxY) / 2 + layout.panY + centreOffsetY;
  return {
    x: midX - boxWide / 2,
    y: midY - boxTall / 2,
    wide: boxWide,
    tall: boxTall,
    scale,
  };
}

/* 좁은 티어에서는 그림을 세로로 눌러 그린다 — 인스펙터가 아래로 내려온 판은
 * 가로로 넓고 세로로 얕다. 그리는 쪽(`paintKnowledgeFrame`)과 포인터를 그림
 * 좌표로 되돌리는 쪽(드래그)이 같은 수를 읽는다. */
function knowledgeYScale(view, layout) {
  if (view.classList.contains("is-tier-compact") || view.classList.contains("is-tier-tiny")) {
    return layout.tuning.yScaleCompact;
  }
  return view.classList.contains("is-tier-middle") ? layout.tuning.yScaleMiddle : 1;
}

/* 군집의 성운과 이름표 — 프레임마다 제자리에서 (3차).
 *
 * 중심은 멤버의 평균, 반지름은 중심에서 가장 먼 멤버(점의 반지름까지)에 토큰의
 * 배율과 여백. 합계 배열은 판을 지을 때 한 번 만든 것을 다시 채우므로 프레임에
 * 할당이 없고, 이름 없는 군집(작은 것)은 DOM이 없어 셈에서 빠진다. 중심의 y는
 * 눌리지 않은 좌표로도 든다(`clusterY`) — 카메라가 그리로 날 때 읽는다. */
function paintKnowledgeClusters(layout, inverse, yScale, middleY) {
  const els = layout.clusterEls;
  if (els.length === 0) return;
  const { count, x, y, community, radius, drawn } = layout;
  const tally = layout.clusterTally;
  const centreX = layout.clusterX;
  const centreY = layout.clusterY;
  const spread = layout.clusterSpread;
  const named = els.length;
  tally.fill(0, 0, named);
  spread.fill(0, 0, named);
  /* 원반의 가운데는 **홈**이다(09-16) — 그려진 멤버의 무게중심이 아니라.
   *
   * 무게중심으로 그리면 한쪽으로 쏠린 군집의 원반이 홈에서 벗어나고, 이완이
   * 약속한 「원반끼리 gap만큼 떨어진다」가 그림에서 깨진다(실측: 「board」와
   * 「artifacts」의 원반이 겹쳤다). 훑기의 제약이 모든 점을 홈에서 `homeR` 안에
   * 붙들고 있으므로, 홈을 가운데로 삼은 원반은 정의상 서로 겹치지 않는다. */
  for (let rank = 0; rank < named; rank += 1) {
    centreX[rank] = layout.communityHomeX[rank];
    centreY[rank] = layout.communityHomeY[rank];
  }
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const rank = community[at];
    if (rank >= named) continue;
    tally[rank] += 1;
    /* 퍼짐은 **눌리지 않은** 좌표에서 잰다 — 원반은 그림과 같은 비율로 눌린
     * 타원이므로, 반지름 하나(rx)가 그 비율 밖에서 나와야 한다. */
    const far = Math.hypot(x[at] - centreX[rank], y[at] - centreY[rank]) + radius[at] * inverse;
    if (far > spread[rank]) spread[rank] = far;
  }
  const digits = KNOWLEDGE_FORCE.coordinateDigits;
  const { nebulaScale, nebulaPad } = layout.tuning;
  for (let rank = 0; rank < named; rank += 1) {
    const held = els[rank];
    if (tally[rank] === 0) continue;
    const homeX = centreX[rank];
    const homeY = middleY + (centreY[rank] - middleY) * yScale;
    /* 원반은 그려진 멤버가 실제로 차지한 넓이에서 나오되(렌즈가 절반을 숨긴
     * 군집은 절반의 원반) 배치가 약속한 반지름을 넘지 않는다 — 넘는 순간
     * 이웃 원반과 겹치고, 겹친 원반은 「어디까지가 이 주제」를 말하지 못한다. */
    /* 그려진 멤버가 실제로 차지한 넓이에서 나오되(렌즈가 절반을 숨긴 군집은
     * 절반의 원반) 배치가 약속한 반지름을 넘지 않는다. */
    const reach = Math.min(
      layout.communityHomeR[rank],
      spread[rank] * nebulaScale + nebulaPad,
    );
    writeAttribute(held.nebula, "cx", homeX.toFixed(digits));
    writeAttribute(held.nebula, "cy", homeY.toFixed(digits));
    writeAttribute(held.nebula, "rx", reach.toFixed(digits));
    writeAttribute(held.nebula, "ry", (reach * yScale).toFixed(digits));
    layout.clusterReach[rank] = reach;
  }
  placeKnowledgeClusterLabels(layout, inverse, yScale, middleY);
}

/* 군집 이름표의 자리만 — 판을 훑지 않는다(군집 수만큼의 짧은 고리).
 *
 * 이름은 원반의 **위**에 선다: 가운데는 대표 페이지와 그 이름의 자리다. 판 밖으로
 * 밀려난 이름은 판 안쪽으로 붙인다 — 화면에서 잘린 이름은 이름이 아니다. 훑기를
 * 건너뛰는 팬 프레임도 이 한 고리는 돈다. */
function placeKnowledgeClusterLabels(layout, inverse, yScale, middleY) {
  const els = layout.clusterEls;
  if (els.length === 0) return;
  const { clusterLabelLift, margin } = layout.tuning;
  const box = layout.viewBoxRect;
  for (let rank = 0; rank < els.length; rank += 1) {
    if (layout.clusterTally[rank] === 0) continue;
    const homeX = layout.clusterX[rank];
    const homeY = middleY + (layout.clusterY[rank] - middleY) * yScale;
    let labelX = homeX;
    let labelY = homeY - layout.clusterReach[rank] * yScale - clusterLabelLift * inverse;
    if (box !== null) {
      const room = margin * inverse;
      labelX = Math.min(Math.max(labelX, box.x + room), box.x + box.wide - room);
      labelY = Math.min(Math.max(labelY, box.y + room), box.y + box.tall - room);
    }
    layout.clusterAt[rank * 2] = labelX;
    layout.clusterAt[rank * 2 + 1] = labelY;
  }
}

/* ---- 이름표 격자 (09-16) --------------------------------------------------
 *
 * 「노드와 라벨이 서로 겹치지 않게 한다. 공간이 부족하면 세부 내용을 접고,
 * 생략된 개수를 표시한다」 — 이 두 문장이 아래 함수의 전부다.
 *
 * 차수의 문턱으로 이름표를 켜고 끄던 3차의 방식(`--knowledge-lod-*`)은 「몇
 * 개가 서는가」는 정했지만 「그것들이 서로 겹치는가」는 묻지 않았다. 실측(실제
 * 볼트 412쪽, 1600×1000): 선 이름표 21개 중 겹친 쌍이 27, 900×760에서는 118.
 * 글자를 줄이는 것은 답이 아니다(작은 글자는 겹치지 않아도 안 읽힌다). 답은
 * 자리를 **예약**하는 것이다.
 *
 * 화면을 한 칸이 `--knowledge-label-cell`인 격자로 덮고, 우선순위가 높은 이름부터
 * 제 상자가 덮는 칸을 예약한다. 이미 남이 쥔 칸을 덮어야 하는 이름은 서지 않는다.
 * 순서는 (1) 군집의 이름 (2) 사람이 지금 묻고 있는 것 — 고른 점·짚은 이웃·검색이
 * 맞춘 점·경로 (3) 펼친 군집의 멤버 (4) 군집의 대표 페이지 (5) 대표 지식
 * (6) 나머지를 차수 순으로. 판이 좁을수록 예산이 작다.
 *
 * 상자의 폭은 글자에서 어림한다(`knowledgeLabelEm`) — DOM에 물으면 한 프레임에
 * 수백 번의 동기 레이아웃이다. 격자는 지우지 않고 세대를 올린다.
 *
 * 서지 못한 이름의 수는 군집마다 세어 이름표 아래 「+n」으로 선다: 접었다는 사실을
 * 그림이 스스로 말한다. */

/* 이름표 후보의 순서 — 단(중심 → 대표 지식 → 잎 → 유령), 그 안에서 차수 내림차순,
 * 동률은 열쇠순. 위상이 정하므로 판을 지을 때 한 번. */
function knowledgeLabelOrder(model, tier) {
  const count = model.count;
  const order = Array.from({ length: count }, (unused, at) => at);
  const rank = (at) => (model.kinds[at] === "ghost" ? -1 : tier[at]);
  order.sort((left, right) => rank(right) - rank(left)
    || model.degree[right] - model.degree[left]
    || (model.keys[left] < model.keys[right] ? -1 : 1));
  return Int32Array.from(order);
}

/* 격자의 한 상자가 비어 있는가. 판 밖으로 나가는 상자는 비어 있지 않다 — 화면에서
 * 잘린 이름은 이름이 아니므로. */
function knowledgeLabelBoxFree(layout, left, top, right, bottom, self = -1, overBodies = false) {
  const { labelCells, labelOwner, labelCols, labelRows, labelGeneration } = layout;
  if (left < 0 || top < 0 || right >= labelCols || bottom >= labelRows) return false;
  for (let row = top; row <= bottom; row += 1) {
    const base = row * labelCols;
    for (let col = left; col <= right; col += 1) {
      /* 제 몸이 쥔 칸은 제게는 빈 칸이다(09-16). 이름표는 언제나 제 점의 바로 곁에
       * 서므로, 칸의 크기(14px)가 「점의 가장자리」와 「이름의 첫 줄」을 같은 칸으로
       * 묶는 일이 흔하다 — 그 한 칸 때문에 모든 이름이 접혔다(실측: 서는 이름 0개).
       * 그래서 칸마다 임자를 적어 두고, 임자가 저 자신이면 지나간다.
       *
       * `overBodies`는 **남의 점**은 덮어도 좋다는 청이다(임자가 0인 칸 = 남의 이름표는
       * 여전히 막는다). 접힌 점의 「+N」만 이 청을 쓴다: 이름 하나가 남의 점을 살짝 덮는 것과,
       * 두 제목이 서로를 덮어 둘 다 안 읽히는 것은 다른 일이다(09-17 실측: 무조건 세우면 천 개의
       * 판에서 이름표가 한 쌍 겹쳤다). */
      const held = labelOwner[base + col];
      if (labelCells[base + col] !== labelGeneration || held === self) continue;
      if (!overBodies || held === 0) return false;
    }
  }
  return true;
}

/* 상자가 이미 쥐어진 칸을 몇 개나 덮는가 — 판 밖으로 나가는 칸도 찬 칸으로 센다.
 *
 * 「빈 자리가 있는가」(`…Free`)로는 자리를 **고를** 수 없다: 여섯 자리가 모두 조금씩
 * 차 있으면 어느 것이 덜 찼는지를 묻게 되고, 그 답이 있어야 주제의 이름판이 가장
 * 한산한 곳에 선다. */
function knowledgeLabelBoxLoad(layout, left, top, right, bottom) {
  const { labelCells, labelCols, labelRows, labelGeneration } = layout;
  let load = 0;
  for (let row = top; row <= bottom; row += 1) {
    for (let col = left; col <= right; col += 1) {
      if (row < 0 || col < 0 || row >= labelRows || col >= labelCols) load += 1;
      else if (labelCells[row * labelCols + col] === labelGeneration) load += 1;
    }
  }
  return load;
}

/* 상자 하나를 쥔다. 판 밖의 칸은 건드리지 않는다.
 *
 * 묻는 것(`…Free`)과 쥐는 것이 따로인 이유: 이름표 하나가 **두 상자**일 수 있고
 * (군집의 이름과 그 아래 한 줄), 둘 중 하나만 비었을 때 앞의 것을 쥐어 버리면
 * 쓰지도 않을 자리를 남에게서 빼앗는다. */
function markKnowledgeLabelBox(layout, left, top, right, bottom, owner = 0) {
  const { labelCells, labelOwner, labelCols, labelRows, labelGeneration } = layout;
  const fromRow = Math.max(0, top);
  const toRow = Math.min(labelRows - 1, bottom);
  const fromCol = Math.max(0, left);
  const toCol = Math.min(labelCols - 1, right);
  for (let row = fromRow; row <= toRow; row += 1) {
    const base = row * labelCols;
    for (let col = fromCol; col <= toCol; col += 1) {
      labelCells[base + col] = labelGeneration;
      labelOwner[base + col] = owner;
    }
  }
}

function placeKnowledgeLabels(view, layout, box, inverse, project = layout.project) {
  const count = layout.count;
  const tuning = layout.tuning;
  const shown = layout.labelShown;
  const folded = layout.clusterFolded;
  if (count === 0) return;
  const onRing = layout.ring !== null;
  /* 격자의 크기는 판의 픽셀에서 나온다 — 그림 좌표가 아니라. 배율이 바뀌면 같은
   * 판이 다른 수의 이름을 담고, 그것이 「확대하면 세부가 드러난다」의 기제다. */
  const cell = Math.max(1, tuning.labelCell);
  const wide = Math.max(1, Math.ceil((box.wide * box.scale) / cell));
  const tall = Math.max(1, Math.ceil((box.tall * box.scale) / cell));
  if (layout.labelCells === null || layout.labelCols !== wide || layout.labelRows !== tall) {
    layout.labelCells = new Int32Array(wide * tall);
    layout.labelOwner = new Int32Array(wide * tall);
    layout.labelCols = wide;
    layout.labelRows = tall;
    layout.labelGeneration = 0;
  }
  layout.labelGeneration += 1;
  /* 세대가 한 바퀴 돌면(이론상) 옛 표시가 새 세대로 읽힌다 — 그때 한 번 지운다. */
  if (layout.labelGeneration === 1) layout.labelCells.fill(0);
  shown.fill(0);
  layout.labelWhere.fill(-1);
  folded.fill(0);
  const drawn = layout.drawn;
  const { radius, labelEm, community, tier } = layout;
  const cellOf = (value) => Math.floor(value / cell);
  /* 그림 좌표 → 판 픽셀. 이름표의 크기는 화면 픽셀로 고정되어 있으므로 상자는
   * 언제나 같은 크기이고, 바뀌는 것은 점들 사이의 거리다.
   *
   * 그 한 물음을 **투영기**가 진다(6차 P1): 2D에서는 카메라의 선형 변환이고 3D
   * 렌즈에서는 카메라 행렬이다. 격자·우선순위·예산·네 자리·임자 규칙은 한 줄도
   * 바뀌지 않는다 — 바뀌는 것은 「그 점이 화면 어디인가」를 누가 답하는가뿐이다. */
  const screenX = (at) => project.screenX(at);
  const screenY = (at) => project.screenY(at);
  /* 상자의 크기는 그 글자가 실제로 쓰이는 크기에서 나온다 — 군집의 이름은 본문
   * 이름표보다 크고, 그 아래 한 줄은 작다. 한 수(`labelPx`)로 셋을 재면 큰 글자가
   * 제 상자 밖으로 삐져나와 격자가 지킨다고 한 약속을 깬다. */
  const boxOf = (centreX, middleY2, emWidth, px) => {
    const half = (emWidth * px) / 2 + tuning.labelPadX;
    const halfHigh = px / 2 + tuning.labelPadY;
    return [cellOf(centreX - half), cellOf(middleY2 - halfHigh),
      cellOf(centreX + half), cellOf(middleY2 + halfHigh)];
  };
  const free = (centreX, middleY2, emWidth, px = tuning.labelPx, self = -1, overBodies = false) =>
    knowledgeLabelBoxFree(layout, ...boxOf(centreX, middleY2, emWidth, px), self, overBodies);
  const load = (centreX, middleY2, emWidth, px = tuning.labelPx) =>
    knowledgeLabelBoxLoad(layout, ...boxOf(centreX, middleY2, emWidth, px));
  const mark = (centreX, middleY2, emWidth, px = tuning.labelPx) =>
    markKnowledgeLabelBox(layout, ...boxOf(centreX, middleY2, emWidth, px));
  const claim = (centreX, middleY2, emWidth, px = tuning.labelPx, self = -1, overBodies = false) => {
    if (!free(centreX, middleY2, emWidth, px, self, overBodies)) return false;
    mark(centreX, middleY2, emWidth, px);
    return true;
  };
  /* 아래가 막혔으면 위를 본다. 이름표 하나에 자리가 둘이면 같은 판에 서는 이름의
   * 수가 눈에 띄게 늘고(실측 09-16: 20 → 34), 두 자리 다 막힌 이름만 접힌다.
   * 「위」는 클래스 하나로 말한다 — 옷은 CSS가 입는다. */
  /* 이름표 하나에 자리 넷 — 아래, 위, 오른쪽, 왼쪽. 실측(09-16, 실제 볼트 412쪽):
   * 아래 하나만 보면 20개, 위를 더하면 34개, 좌우까지 더하면 그 이상이 선다. 네
   * 자리 다 막힌 이름만 접힌다. 「어느 자리인가」는 클래스 하나로 말하고 옷은
   * CSS가 입는다. */
  /* 후보 상자 하나를 쥐어 본다 — 쥐면 그 중심을 적는다(`labelAtX/Y`). */
  const standAt = (at, seatX, seatY, overBodies = false) => {
    if (!claim(seatX, seatY, labelEm[at], tuning.labelPx, at + 1, overBodies)) return false;
    layout.labelAtX[at] = seatX;
    layout.labelAtY[at] = seatY;
    return true;
  };
  const seatLabel = (at, overBodies = false) => {
    const centreX = screenX(at);
    const centreY = screenY(at);
    const room = radius[at] + tuning.labelGap;
    const half = (labelEm[at] * tuning.labelPx) / 2;
    if (onRing) {
      /* 고리가 고른 자리를 먼저 보고, 막혔으면 반대쪽·위·아래를 본다. 네 자리
       * 다 막힌 이름만 접힌다 — 서른 개의 제목이 서로를 덮는 화면보다, 스물다섯
       * 개가 읽히는 화면이 낫다. */
      const side = layout.ringLabelAt[at * 3 + 2];
      const seatY = centreY + layout.ringLabelAt[at * 3 + 1] - tuning.labelPx / 2;
      const seatX = centreX + layout.ringLabelAt[at * 3] + side * half;
      if (standAt(at, seatX, seatY, overBodies)) return 0;
      if (standAt(at, centreX - layout.ringLabelAt[at * 3] - side * half, seatY, overBodies)) return 0;
      if (standAt(at, centreX, centreY + room + tuning.labelPx / 2, overBodies)) return 0;
      if (standAt(at, centreX, centreY - room - tuning.labelPx / 2, overBodies)) return 0;
      return -1;
    }
    if (standAt(at, centreX, centreY + room + tuning.labelPx / 2, overBodies)) return 0;
    if (standAt(at, centreX, centreY - room - tuning.labelPx / 2, overBodies)) return 1;
    if (standAt(at, centreX + room + half, centreY, overBodies)) return 2;
    if (standAt(at, centreX - room - half, centreY, overBodies)) return 3;
    return -1;
  };
  const wear = (at, where) => {
    shown[at] = 1;
    layout.labelWhere[at] = where;
    /* 고리 위에서는 자리를 이미 `paintKnowledgeRingLabel`이 썼다 — 격자는 설지
     * 말지만 정한다. */
    if (onRing) return;
    const word = layout.nodeEls[at]?.querySelector(".knowledge-label");
    if (!word) return;
    const room = radius[at] + tuning.labelGap;
    const middle = tuning.labelPx * KNOWLEDGE_FORCE.labelMiddle;
    const seat = where === 1 ? [0, -room, "middle"]
      : where === 2 ? [room, middle, "start"]
        : where === 3 ? [-room, middle, "end"]
          : [0, room + tuning.labelPx, "middle"];
    writeAttribute(word, "x", String(seat[0]));
    writeAttribute(word, "y", String(seat[1]));
    writeAttribute(word, "text-anchor", seat[2]);
  };
  /* (0-a) 점의 몸은 글자의 자리가 아니다. 이름표를 나눠 주기 전에 그려진 모든
   * 원이 제 자리를 쥔다 — 그래야 이름이 남의 점 위에 얹히지 않고, 촘촘한 공
   * 안에서는 바깥쪽 점만 이름을 얻는다(그것이 옳다: 공 안쪽은 이미 꽉 찼다).
   * 값은 점 하나에 칸 한둘이므로 점의 수에 선형이다. */
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const dotX = screenX(at);
    const dotY = screenY(at);
    markKnowledgeLabelBox(layout,
      cellOf(dotX - radius[at]), cellOf(dotY - radius[at]),
      cellOf(dotX + radius[at]), cellOf(dotY + radius[at]), at + 1);
  }
  /* (0-b) 판 위에 떠 있는 조작부의 자리도 쥔다 — 군집 범례, 배율 단추, 빵부스러기.
   * 그 뒤에 숨은 이름은 서 있어도 읽히지 않으므로, 격자에게는 그 자리도 「찬 자리」다. */
  const canvasBox = view.querySelector(".knowledge-canvas")?.getBoundingClientRect();
  if (canvasBox) {
    for (const held of view.querySelectorAll(
      ".knowledge-cluster-legend, .knowledge-zoom, .knowledge-crumb, .knowledge-legend",
    )) {
      if (held.hidden || getComputedStyle(held).display === "none") continue;
      const seat = held.getBoundingClientRect();
      if (seat.width === 0 || seat.height === 0) continue;
      markKnowledgeLabelBox(layout,
        cellOf(seat.left - canvasBox.left), cellOf(seat.top - canvasBox.top),
        cellOf(seat.right - canvasBox.left), cellOf(seat.bottom - canvasBox.top));
    }
  }
  /* (0-c) 사람이 **지금 고른** 쪽의 이름은 주제의 이름판보다 먼저 자리를 쥔다.
   * 지도의 첫 낱말은 주제이지만, 고르는 손이 묻고 있는 것은 그 한 쪽이다. */
  /* 네 자리가 다 막혔어도 서는 이름 — 그리고 그 자리를 **쥔다**. 쥐지 않으면 뒤에 오는 이름표가 그
   * 위에 앉는다(실측 09-16: 고른 쪽의 이름과 이웃의 이름이 겹친 한 쌍). 이름 하나가 남의 점을 살짝
   * 덮는 것과, 두 제목이 서로를 덮어 둘 다 안 읽히는 것은 다른 일이다. 이 예외를 얻는 것은 사람이
   * 고른 점과 접힌 점뿐이다: 접힌 점의 「+N」은 그 아래 무엇이 접혀 있는지를 말하는 유일한 표시이고,
   * 펼치는 문의 이름이다. */
  const standAnyway = (at) => {
    const room = radius[at] + tuning.labelGap;
    const seatX = screenX(at);
    const seatY = screenY(at) + room + tuning.labelPx / 2;
    mark(seatX, seatY, labelEm[at]);
    layout.labelAtX[at] = seatX;
    layout.labelAtY[at] = seatY;
  };
  const selectedSeat = knowledgeSelectedKey === null ? -1
    : layout.model.keys.indexOf(knowledgeSelectedKey);
  if (selectedSeat >= 0 && (drawn === null || drawn[selectedSeat] === 1)) {
    const where = seatLabel(selectedSeat);
    if (where < 0) standAnyway(selectedSeat);
    wear(selectedSeat, Math.max(0, where));
  }
  /* (1) 군집의 이름과 그 아래 두 줄. */
  const spot = knowledgeClusterPicked;
  const tierWord = view.dataset.knowledgeTier ?? "wide";
  if (!onRing) {
    const digits = KNOWLEDGE_FORCE.coordinateDigits;
    /* 세 줄짜리 이름판을 감당할 만큼 판이 넓은가 — 묻는 것은 **캔버스**의 폭이다.
     * 티어는 판(인스펙터를 포함한 뷰) 전체의 폭이라, 인스펙터가 아래로 쌓이는
     * 폭에서는 캔버스가 좁은데도 티어는 넓다고 답한다. */
    const canvasWide = box.wide * box.scale;
    const short = canvasWide < tuning.plateWide;
    view.querySelector(".knowledge-picture").classList.toggle("is-short-plate", short);
    const lift = short ? tuning.clusterLabelPx * 2
      : tuning.clusterLabelPx + tuning.clusterCountGap + tuning.clusterCountPx;
    for (let rank = 0; rank < layout.namedCount; rank += 1) {
      const held = layout.clusterEls[rank];
      if (!held || layout.clusterTally[rank] === 0) continue;
      const name = knowledgeClusterWord(layout, rank);
      const homeX = (layout.clusterX[rank] - box.x) * box.scale;
      const anchorX = (layout.clusterAt[rank * 2] - box.x) * box.scale;
      const anchorY = (layout.clusterAt[rank * 2 + 1] - box.y) * box.scale;
      const wideEm = knowledgeLabelEm(name, tuning);
      /* 아래 두 줄은 제 글자로 잰다 — 대표 지식의 제목도, 「15쪽 · 이름 +13」도
       * 주제의 이름보다 길다. */
      const leadEm = knowledgeLabelEm(knowledgeClusterLeadWord(layout, rank), tuning);
      const tallyEm = knowledgeLabelEm(knowledgeClusterCountWord(layout, rank), tuning);
      /* 원반의 위가 막혔으면 둘레를 돌며 빈 자리를 찾는다 — 위, 한 줄 더 위,
       * 두 줄 더 위, 그리고 아래. 이웃한 두 원반의 이름이 같은 자리를 원하는 일은
       * 흔하고(순위가 높은 군집이 먼저 쥔다), 빈 자리를 못 찾은 이름은 그래도
       * 선다: 주제의 이름은 지도의 첫 낱말이라 접지 않는다. 대신 그 자리를
       * 쥐어 두어 뒤에 오는 이름표들이 피해 간다. */
      const reach = layout.clusterReach[rank] * box.scale * knowledgeYScale(view, layout);
      const homeY = (layout.clusterY[rank] - box.y) * box.scale;
      const sideways = (Math.max(wideEm * tuning.clusterLabelPx,
        leadEm * tuning.clusterCountPx, tallyEm * tuning.clusterCountPx) / 2)
        + tuning.clusterLabelLift;
      const below = homeY + reach + tuning.clusterLabelLift + tuning.clusterLabelPx;
      /* 원반의 위·아래·양옆 여섯 자리 중 **가장 한산한** 곳. 빈 자리를 찾을 때까지
       * 훑고 첫 빈 자리에 서는 방식은, 여섯이 다 조금씩 차 있을 때 마지막 자리에
       * 그냥 서게 만든다(실측 09-16: 주제 이름판 일곱이 남의 공 위에 얹혔다). */
      /* 위로 셋, 아래로 둘, 양옆으로 하나씩 — 그리고 각각을 좌우로 한 칸씩 밀어
       * 본 자리까지 — 원반 하나에 서른다섯 자리. 재는 값은 칸 몇 개를 세는 일이고,
       * 그 값으로 열세 개의 이름판이 서로를 피한다. */
      const seats = [];
      for (const [seatX, seatY] of [
        [homeX, anchorY], [homeX, anchorY - lift], [homeX, anchorY - lift * 2],
        [homeX, below], [homeX, below + lift],
        [homeX - reach - sideways, homeY], [homeX + reach + sideways, homeY],
      ]) {
        seats.push([seatX, seatY],
          [seatX - sideways, seatY], [seatX + sideways, seatY],
          [seatX - sideways * 2, seatY], [seatX + sideways * 2, seatY]);
      }
      /* 좁은 판의 이름판은 **한 줄**이다 — 주제의 이름만. 대표 지식과 쪽 수는
       * 인스펙터와 군집 범례에 그대로 있고, 좁은 화면에서 줄어드는 것은 글자
       * 크기가 아니라 표시량이다(사용자 조건). 세 줄짜리 판 열셋은 560px 판에서
       * 둘만 남겼다; 한 줄짜리는 열셋이 다 선다. */
      const plateLoad = (seatX, seatY) => load(seatX, seatY, wideEm, tuning.clusterLabelPx)
        + (short ? 0 : load(seatX, seatY + tuning.clusterLeadGap, leadEm, tuning.clusterCountPx)
          + load(seatX, seatY + tuning.clusterCountGap, tallyEm, tuning.clusterCountPx));
      let [centreX, centreY] = seats[0];
      let lightest = Number.POSITIVE_INFINITY;
      for (const [seatX, seatY] of seats) {
        const weight = plateLoad(seatX, seatY);
        if (weight >= lightest) continue;
        lightest = weight;
        centreX = seatX;
        centreY = seatY;
        if (weight === 0) break;
      }
      /* 스물한 자리를 다 봐도 빈 곳이 없으면 그 이름판은 **접는다** — 폭을
       * 묻지 않는다. 겹쳐 선 이름은 서 있어도 읽히지 않으므로, 그것이 「표시량을
       * 줄인다」의 뜻이다(글자를 줄이지 않는다 — 사용자 조건). 순위가 큰 주제부터
       * 자리를 고르므로 남는 것은 언제나 큰 주제들이고, 접힌 주제는 아래 왼쪽의
       * 군집 범례가 여전히 이름으로 부른다. 넓은 판에서는 자리가 늘 있어 아무것도
       * 접히지 않는다(실측 1600×1000: 열셋 전부). */
      const folds = lightest > KNOWLEDGE_FORCE.plateSlack;
      held.label.classList.toggle("is-folded", folds);
      if (folds) continue;
      mark(centreX, centreY, wideEm, tuning.clusterLabelPx);
      if (!short) {
        mark(centreX, centreY + tuning.clusterLeadGap, leadEm, tuning.clusterCountPx);
        mark(centreX, centreY + tuning.clusterCountGap, tallyEm, tuning.clusterCountPx);
      }
      const graphX = box.x + centreX / box.scale;
      const graphY = box.y + centreY / box.scale;
      layout.clusterAt[rank * 2] = graphX;
      layout.clusterAt[rank * 2 + 1] = graphY;
      writeAttribute(held.label, "transform",
        `translate(${graphX.toFixed(digits)} ${graphY.toFixed(digits)}) scale(${inverse})`);
    }
  }
  /* (2) 사람이 지금 묻고 있는 것. 예산 밖이고 서로 겹치지도 않는다 — 고른 점의
   * 이름이 배율 때문에 숨는 화면은 답이 아니다. */
  const asked = [];
  const ask = (at) => {
    if (at < 0 || at >= count || shown[at] === 1) return;
    if (drawn !== null && drawn[at] === 0) return;
    asked.push(at);
  };
  /* 고른 쪽의 **직접 이웃**도 이름을 얻는다(09-16). 선이 밝아도 그 끝이 이름 없는
   * 점이면 「무엇과 연결되는가」는 여전히 답이 없다 — 인스펙터의 목록과 그림이
   * 같은 것을 말하게 하는 자리가 여기다. */
  if (selectedSeat >= 0 && !onRing) {
    /* 고리 위에서는 이웃이 곧 그림 전체다 — 거기서 이웃을 「무조건」으로 두면
     * 격자가 아무것도 고르지 못하고 서른 개의 제목이 서로를 덮는다. 고리에서
     * 무조건인 것은 중심 하나이고 나머지는 격자가 고른다. */
    const { start, neighbour } = layout.model;
    for (let edge = start[selectedSeat]; edge < start[selectedSeat + 1]; edge += 1) {
      ask(neighbour[edge]);
    }
  }
  for (const at of layout.litNodes) ask(at);
  /* 전체 지도의 「+N」(공급망 멤버의 접힌 구성요소, P4)은 펼치는 문의 이름이다 — 격자의 규칙은 지키되
   * 먼저 고른다. 주변 탐색의 접힌 허브는 제 옷(`data-label-tier`)이 이미 세운다. */
  if (layout.folded !== null && !onRing) {
    for (let at = 0; at < count; at += 1) if (layout.folded[at] > 0) ask(at);
  }
  if (knowledgeQuery.trim() !== "") {
    for (let at = 0; at < count; at += 1) if (layout.searchMatch[at] === 1) ask(at);
  }
  for (const at of asked) {
    /* 이웃들은 먼저 고를 권리를 얻되 규칙은 지킨다: 스물넷의 이웃을 무조건
     * 세우면 스물네 쌍이 서로를 덮고(실측 09-16), 그 화면은 「무엇과 연결되는가」에
     * 답하지 못한다. 자리를 못 얻은 이웃의 제목은 인스펙터의 관계 목록에 온전히 있다.
     * 접힌 점만 예외다(위) — 「+N」은 접힌 것이 있다는 유일한 표시다. */
    let where = seatLabel(at);
    /* 접힌 점만 한 번 더 묻는다 — 남의 점 위에 서도 좋으냐고. 남의 **이름표** 위에는 서지 않는다. */
    if (where < 0 && !onRing && (layout.folded?.[at] ?? 0) > 0) where = seatLabel(at, true);
    if (where >= 0) wear(at, where);
  }
  /* (3)~(6) 예산 안에서 순서대로. 펼친 군집의 멤버가 먼저다 — 군집을 고른 손이
   * 묻는 것은 그 주제의 세부이므로. */
  let budget = tuning.labelBudget[tierWord] ?? tuning.labelBudget.wide;
  /* 「+n」이 세는 것은 **쪽**이다 — 유령(아직 없는 페이지)은 이 주제의 쪽이 아니고,
   * 군집 줄의 「42쪽」과 다른 것을 세면 두 수가 서로를 반박한다. */
  const kinds = layout.model.kinds;
  const countsAsFolded = (at) => kinds[at] === "page";
  const order = layout.labelOrder;
  const rounds = spot >= 0 ? 2 : 1;
  for (let round = 0; round < rounds; round += 1) {
    for (let seat = 0; seat < order.length; seat += 1) {
      const at = order[seat];
      if (shown[at] === 1) continue;
      if (drawn !== null && drawn[at] === 0) continue;
      const inSpot = spot >= 0 && community[at] === spot;
      if (rounds === 2 && (round === 0) !== inSpot) continue;
      if (budget <= 0) {
        if (countsAsFolded(at)) folded[community[at]] += 1;
        continue;
      }
      const centreX = screenX(at);
      const centreY = screenY(at);
      if (centreX < 0 || centreY < 0 || centreX > box.wide * box.scale
        || centreY > box.tall * box.scale) continue;
      const where = seatLabel(at);
      if (where >= 0) {
        wear(at, where);
        budget -= 1;
      } else if (countsAsFolded(at)) {
        folded[community[at]] += 1;
      }
    }
  }
  /* 옷을 입힌다. 점마다 클래스 하나이고, 바뀐 것만 쓴다.
   *
   * 이름이 서지 않은 점은 자리도 기본값으로 되돌린다 — 고리에서 옆으로 눕혔던
   * 이름표가 전체 지도로 돌아와 그 자세로 남으면, 다음에 그 이름이 설 때 격자가
   * 잰 자리와 실제로 서는 자리가 어긋난다. */
  const nodeEls = layout.nodeEls;
  for (let at = 0; at < count; at += 1) {
    const node = nodeEls[at];
    if (!node) continue;
    node.classList.toggle("is-named", shown[at] === 1);
    if (shown[at] === 1 || onRing) continue;
    const word = node.querySelector(".knowledge-label");
    if (!word) continue;
    writeAttribute(word, "x", "0");
    writeAttribute(word, "y", String(radius[at] + tuning.labelPx + tuning.labelGap));
    writeAttribute(word, "text-anchor", "middle");
  }
  for (let rank = 0; rank < layout.namedCount; rank += 1) {
    const held = layout.clusterEls[rank];
    if (!held?.count) continue;
    writeTextContent(held.count, knowledgeClusterCountWord(layout, rank));
  }
}

/* 주제의 대표 지식 — 그 군집에서 연결이 가장 많은 페이지의 제목(짧게). */
function knowledgeClusterLeadWord(layout, rank) {
  const seat = layout.communityCore[rank];
  if (seat === undefined || seat < 0) return "";
  return knowledgeShortWord(layout.model.titles[seat], layout.tuning.clusterLeadMax);
}

/* 군집 이름 아래 한 줄: 「쪽 n」, 접은 이름이 있으면 「쪽 n · 이름 +m」. */
function knowledgeClusterCountWord(layout, rank) {
  const pages = layout.communitySize[rank];
  const hidden = layout.clusterFolded[rank];
  if (hidden <= 0) return t("knowledge.clusterPages", "{{count}}쪽", { count: pages });
  return t("knowledge.clusterPagesFolded", "{{count}}쪽 · 이름 +{{folded}}",
    { count: pages, folded: hidden });
}

/* ---- 페인터 하나의 자리 (6차 P1) -------------------------------------------
 *
 * 그리는 손이 하나가 아니게 된다: 오늘의 SVG와, 만 쪽을 위한 WebGL2(P2).
 * 둘은 **같은 `layout`을 읽고** 같은 카메라를 받는다 — 배치(`knowledgeForceStep`)도
 * 이름표 격자(`placeKnowledgeLabels`)도 비행(`flyKnowledgeTo`)도 한 벌이고,
 * 페인터가 다르게 하는 것은 「무엇으로 칠하는가」뿐이다.
 *
 * 인터페이스: `mount(view)` · `paintTopology(layout)` ·
 * `paintFrame(layout, camera, { cameraOnly })` · `pick(px, py) → seat|null` ·
 * `dispose()`.
 *
 * 위상의 순간(`paintTopology`)이 프레임과 따로인 것은 창이 **그 둘 사이에서**
 * 옷을 입히기 때문이다: 검색·포커스·경로·슬라이서·선택이 선 점에 클래스를 쓰고,
 * 머리의 「매치 n」은 그 옷을 세어 얻는 수다. 한 호출로 묶으면 그 페인터들이
 * 아직 서지 않은 점 위에서 돈다.
 */

/* 판마다 페인터 하나. 판이 사라지면 함께 사라진다(`WeakMap`). */
const knowledgePainters = new WeakMap();

/* 페인터의 표 — 한 줄이 한 페인터이고, 줄의 순서가 선호다. 사람이 고르지 않는다:
 * 설 수 있는(`able`) 첫 줄이 선다. 그리는 방식은 사람이 할 결정이 아니라 판이 할
 * 결정이고, 「SVG로 그리기 / GL로 그리기」는 고를 이유를 주지 못하는 구현 낱말이었다
 * (「지식그래프 svg gl 문구 빼줘」, 2026-09-17).
 *
 * GL이 먼저인 근거는 창의 엔진(WebKit)에서 두 손을 짝지어 잰 표다
 * (docs/design/knowledge-graph-3d-20260916/README.md §2): 궤도 처리 p95가 볼트
 * 412점 21→5 ms, 1,000쪽 47→6, 5,000쪽 213→12, 10,000쪽 365→16 ms이고, 프레임
 * 간격 p95도 모든 규모에서 GL이 낮다(55→25 … 662→39 ms). SVG가 이기는 칸은 412점의
 * 첫 그림 하나(133→174 ms)이고 그 차이는 셰이더를 한 번 굽는 값이다 — 그래서 규모
 * 문턱을 새로 만들지 않는다. WebGL2가 없는 판(크로미엄 게이트)과 문맥을 잃은 판에서는
 * SVG가 선다. */
const KNOWLEDGE_PAINTERS = Object.freeze([
  Object.freeze({ id: "gl", make: () => makeKnowledgeGlPainter(), able: () => knowledgeGlSupported() }),
  Object.freeze({ id: "svg", make: () => makeKnowledgeSvgPainter(), able: () => true }),
]);

/* GL이 서지 못했다 — 문맥을 잃었거나 애초에 없었다. 이 창의 남은 시간 동안 SVG가
 * 그리고, 사람에게는 아무 말도 하지 않는다: 그림은 그대로 서고 사람이 할 일은 없다.
 * 접는 것이 먼저인 것은 빈 캔버스를 보여 주는 것이 그림이 아니기 때문이다. */
function knowledgeGlFellBack(view) {
  knowledgePainterKind = "svg";
  const held = knowledgePainters.get(view);
  if (held !== undefined && held.id === "gl") {
    held.dispose();
    knowledgePainters.delete(view);
  }
  const tab = knowledgeTab();
  if (tab) void paintKnowledgeView(tab);
}

/* 못박힌 페인터 — `null`이면 표가 고른다. GL이 무너진 판은 여기에 "svg"를 적고,
 * 시험은 두 손을 번갈아 재려고 여기에 손을 적는다. */
let knowledgePainterKind = null;

/* 지금 서야 할 손의 이름. */
function knowledgePainterChoice() {
  return knowledgePainterKind ?? KNOWLEDGE_PAINTERS.find((row) => row.able()).id;
}

/* 이 판의 페인터. 다른 손이 서야 하면 쓰던 것이 제 것을 **놓고**(`dispose`) 새것이
 * 선다 — 놓지 않으면 GPU 버퍼와 리스너가 판마다 쌓인다. */
function knowledgePainterFor(view) {
  const wanted = knowledgePainterChoice();
  const held = knowledgePainters.get(view);
  if (held !== undefined && held.id === wanted) return held;
  if (held !== undefined) {
    held.dispose();
    knowledgePainters.delete(view);
  }
  const row = KNOWLEDGE_PAINTERS.find((one) => one.id === wanted);
  const painter = row.make();
  painter.id = row.id;
  painter.mount(view);
  /* 손이 서지 못했으면(GL 문맥이 없는 판) `mount`가 이미 손을 되돌려 놓았다 —
   * 그 죽은 손을 판에 매어 두면 다음 프레임이 그것에게 그리라고 한다. */
  if (knowledgePainterChoice() !== row.id) return knowledgePainterFor(view);
  knowledgePainters.set(view, painter);
  return painter;
}

/* 세로 눌림을 먹인 그리기 좌표. 두 페인터가 같은 수를 읽어야 같은 그림이므로
 * 이 한 줄은 페인터 밖에 산다 — 눌림의 규칙은 카메라의 것이지 손의 것이 아니다. */
function knowledgeSeatDrawY(layout, camera) {
  const { count, y, drawY } = layout;
  const { middleY, yScale } = camera;
  for (let at = 0; at < count; at += 1) drawY[at] = middleY + (y[at] - middleY) * yScale;
}

/* 투영기 — 자리(seat)를 판의 픽셀로. 2D의 카메라는 선형 변환 하나이고, 3D 렌즈는
 * 같은 자리에 제 행렬을 둔다(P3). 판마다 하나이므로 프레임에 할당이 없다.
 *
 * 이름표 격자가 이것 하나만 묻는다: 「이 점은 화면 어디에 있는가」. 그 물음의 답이
 * 카메라에서 나오는 한 격자는 3D에서도 한 벌이다. */
function knowledgeProjector(layout) {
  return {
    layout,
    camera: null,
    screenX(at) {
      return (this.layout.x[at] - this.camera.x) * this.camera.scale;
    },
    screenY(at) {
      return (this.layout.drawY[at] - this.camera.y) * this.camera.scale;
    },
  };
}

/* 카메라 — 이 프레임이 그림의 어디를 어떤 배율로 보는가.
 *
 * 배율의 역수(화면 픽셀로 서는 것들이 읽는다), 세로 눌림과 그 축까지 한 자리에서
 * 나온다: 두 페인터가 같은 수를 쓰지 않으면 같은 그림이 아니다. */
function knowledgeCamera(view, layout) {
  const camera = knowledgeViewBox(view, layout);
  camera.yScale = knowledgeYScale(view, layout);
  camera.middleY = (layout.bounds.minY + layout.bounds.maxY) / 2;
  /* 화면 픽셀로 고정되는 것 둘: 점의 크기와 낱말의 크기. 배율의 역수를 노드
   * 하나하나에 쓰는 대신 한 번만 계산해 둔다. */
  camera.inverse = Math.round((1 / camera.scale) * KNOWLEDGE_FORCE.inversePrecision)
    / KNOWLEDGE_FORCE.inversePrecision;
  layout.scale = camera.scale;
  layout.viewBoxRect = camera;
  layout.inverse = camera.inverse;
  layout.project.camera = camera;
  return camera;
}

/* 포인터 아래의 점. 그림이 SVG일 때는 브라우저의 히트테스트가 이 답을 알고 있지만
 * (점마다 요소가 있으므로) GL에는 요소가 없다 — 그래서 답은 **투영한 자리**에서
 * 나온다: 점의 화면 반지름은 `radius[at]`이다(점은 배율의 역수로 되돌려 그려지므로).
 * 두 페인터가 같은 답을 내는지는 하네스가 대조한다.
 *
 * 훑는 것은 점의 수다. 만 점의 투영이 0.5 ms 안이라는 설계의 어림이 참인지도
 * 하네스가 재고, 참이 아니게 되는 날 격자(`--knowledge-3d-pick-cell`)가 그 자리에
 * 들어온다 — 재기 전에 넣지 않는다. */
function knowledgePickSeat(layout, px, py) {
  const { count, drawn, radius, project } = layout;
  if (project.camera === null) return -1;
  let best = -1;
  let closest = Number.POSITIVE_INFINITY;
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const gapX = px - project.screenX(at);
    const gapY = py - project.screenY(at);
    const reach = radius[at];
    const gap = gapX * gapX + gapY * gapY;
    if (gap > reach * reach || gap >= closest) continue;
    closest = gap;
    best = at;
  }
  return best;
}

/* 오늘의 그림 — SVG. 5차까지의 페인트 코드가 그대로 이 안에 산다(옮긴 것이지
 * 고친 것이 아니다). 점마다 <g> 셋이고, 그 값이 만 쪽에서 9만 요소가 된다는 것이
 * 6차가 두 번째 손을 짓는 이유다(P0의 표). */
function makeKnowledgeSvgPainter() {
  return {
    id: "svg",
    view: null,
    mount(view) {
      this.view = view;
      view.querySelector(".knowledge-picture")?.classList.remove("is-gl");
      /* 앞의 손이 세워 둔 도장을 지운다. GL은 제 위상을 올리고 나서 같은 도장을
       * 남기는데, 그것을 그대로 두면 이 손이 「이미 서 있다」고 읽고 빈 판을
       * 그린다(실측: GL에서 돌아온 판의 점이 0개였다). */
      const host = view.querySelector(".knowledge-nodes");
      if (host) host.dataset.knowledgeSignature = "";
      const layout = knowledgeLayouts.get(view);
      if (layout !== undefined) {
        layout.paintedModel = null;
        layout.paintedGeometry = null;
      }
    },
    /* 위상의 순간 — 점의 DOM. 창은 이 뒤에 옷을 입힌다. */
    paintTopology(layout) {
      paintKnowledgeNodes(this.view, layout);
    },
    /* 옷만 바뀐 순간(호버). SVG는 부르는 쪽이 방금 쓴 클래스로 이미 입었다 —
     * 할 일이 없다. 요소 없이 그리는 손만 여기서 제 옷을 다시 채운다. */
    paintDress() {},
  /* 한 프레임 — 오늘의 그림 그대로다. 카메라는 밖에서 재어 들어오고
   * (`knowledgeCamera`), 여기서 하는 일은 그 카메라로 SVG를 쓰는 것뿐이다. */
  paintFrame(layout, camera, { cameraOnly = false } = {}) {
    const view = this.view;
    const model = layout.model;
    const picture = view.querySelector(".knowledge-picture");
    const inverse = camera.inverse;
    writeAttribute(picture, "viewBox", `${camera.x} ${camera.y} ${camera.wide} ${camera.tall}`);
    const previous = layout.paintedGeometry;
    // A pan changes only the SVG camera. Revisit nodes and edges when a force
    // step, zoom or container tier changes their actual screen geometry.
    if (cameraOnly && previous?.revision === layout.geometryRevision
        && previous.scale === camera.scale && previous.zoom === layout.zoom
        && previous.yScale === camera.yScale && previous.middleY === camera.middleY) {
      /* 점도 선도 그대로지만 **창**이 옮겨 갔다(09-16): 화면 밖으로 나간 이름은
       * 자리를 내놓고, 들어온 자리에는 새 이름이 선다. 훑는 것은 군집 수만큼의
       * 이름표와 이름표 격자 하나뿐이고, 선은 한 번도 재지 않는다. */
      placeKnowledgeClusterLabels(layout, inverse, camera.yScale, camera.middleY);
      placeKnowledgeLabels(view, layout, camera, inverse);
      return;
    }
    /* `userSpaceOnUse` markers live in graph units, so their viewport gets the same
     * inverse scale as labels and nodes. The resulting triangle remains the token's
     * exact screen-pixel size at every zoom. */
    const markerSize = layout.tuning.markerPx * inverse;
    for (const marker of picture.querySelectorAll("marker")) {
      writeAttribute(marker, "markerWidth", String(markerSize));
      writeAttribute(marker, "markerHeight", String(markerSize));
    }
    /* 군집의 이름표는 fit에서 이만큼 들어가면 물러선다 — 절대 배율이 아니라 배율의
     * 배수다: 서른다섯 쪽의 볼트는 fit부터가 라벨 LOD의 절대 문턱 위에 있다. */
    picture.classList.toggle("is-zoomed-in", layout.zoom >= layout.tuning.clusterLabelUntil);
    const count = layout.count;
    const nodeEls = layout.nodeEls;
    const x = layout.x;
    const drawY = layout.drawY;
    knowledgeSeatDrawY(layout, camera);
    for (let at = 0; at < count; at += 1) {
      const node = nodeEls[at];
      if (!node) continue;
      writeAttribute(
        node,
        "transform",
        `translate(${x[at].toFixed(KNOWLEDGE_FORCE.coordinateDigits)} `
          + `${drawY[at].toFixed(KNOWLEDGE_FORCE.coordinateDigits)}) scale(${inverse})`,
      );
      if (layout.ring !== null) paintKnowledgeRingLabel(layout, at, node);
    }
    /* 간선은 재고 나서 쓴다 — 그 규율은 에이전트 그래프와 나눠 쓰는
     * `paintGraphEdges`의 것이다. 여기서 재는 것은 좌표뿐이고, 좌표가 지난
     * 프레임과 같은 선은 경로 문자열을 짓지도 않는다. */
    paintKnowledgeClusters(layout, inverse, camera.yScale, camera.middleY);
    const measured = layout.measured;
    const { from, to, ghost, kind, edgeCount } = model;
    const spot = knowledgeClusterPicked;
    const community = layout.community;
    const { sliceMatch, pathEdge, pathShown, drawnEdge } = layout;
    for (let at = 0; at < edgeCount; at += 1) {
      if (drawnEdge !== null && drawnEdge[at] === 0) continue;
      const held = measured[at];
      const seat = held.at;
      const fromX = x[from[at]];
      const fromY = drawY[from[at]];
      const toX = x[to[at]];
      const toY = drawY[to[at]];
      const gapX = toX - fromX;
      const gapY = toY - fromY;
      const reach = Math.hypot(gapX, gapY);
      const fromRadius = layout.radius[from[at]] * inverse;
      const toRadius = layout.radius[to[at]] * inverse;
      if (reach > fromRadius + toRadius) {
        const unitX = gapX / reach;
        const unitY = gapY / reach;
        seat[0] = fromX + unitX * fromRadius;
        seat[1] = fromY + unitY * fromRadius;
        seat[2] = toX - unitX * toRadius;
        seat[3] = toY - unitY * toRadius;
      } else {
        seat[0] = fromX;
        seat[1] = fromY;
        seat[2] = toX;
        seat[3] = toY;
      }
      held.lit = layout.lit.has(at);
      /* 같은 주제 안의 선인가, 주제를 건너는 선인가(09-16).
       *
       * 군집 안의 본문 링크는 그 덩어리의 모양을 그리는 실이라 옅어도 제 일을 하고,
       * 군집을 건너는 본문 링크는 더 옅다 — 주제 사이에서 읽혀야 하는 것은 프론트매터가
       * 이름을 준 관계이지 스쳐 지나간 대괄호가 아니다. 이름 있는 관계는 이 구분 밖에서
       * 제 색과 화살표를 그대로 지킨다: 옷을 바꾸는 것이지 관계를 지우는 것이 아니다. */
      held.inter = community[from[at]] !== community[to[at]];
      held.hue = held.inter ? -1 : layout.communityHue[community[from[at]]];
      held.ghost = ghost[at] === 1;
      held.kind = KNOWLEDGE_EDGE_KINDS[kind[at]];
      held.focused = layout.focusEdgeVisited[at] === 1;
      /* 바퀴살(시안): 고리의 중심에 닿는 선만 또렷하다. */
      held.spoke = layout.ring !== null && (from[at] === layout.ring.seat || to[at] === layout.ring.seat);
      held.searchMatch = layout.searchMatch[from[at]] === 1 && layout.searchMatch[to[at]] === 1;
      held.searchDim = knowledgeQuery.trim() !== "" && !held.searchMatch;
      /* 밝힌 군집의 선은 양 끝이 다 그 군집일 때 밝다 — 포커스의 규칙과 같다. */
      held.spotlit = spot >= 0 && community[from[at]] === spot && community[to[at]] === spot;
      /* 슬라이서 창 밖의 선은 양 끝이 다 창 안이 아닐 때 흐리다 — 점의 규칙 그대로. */
      held.sliceDim = sliceMatch[from[at]] === 0 || sliceMatch[to[at]] === 0;
      held.pathLit = pathEdge[at] === 1;
      held.pathDim = pathShown && !held.pathLit;
    }
    /* 그려지는 선만 페인터에 넘긴다(t-4140). 부분집합의 목록은 도장이 바뀔 때만 다시
     * 고르고, 프레임마다는 같은 배열을 건넨다 — 페인터는 좌표가 그대로인 선을
     * 짓지도 않는다. */
    let edges = measured;
    if (drawnEdge !== null) {
      if (layout.drawnMeasuredStamp !== layout.drawnStamp || layout.drawnMeasured === null) {
        layout.drawnMeasured = measured.filter((unused, at) => drawnEdge[at] === 1);
        layout.drawnMeasuredStamp = layout.drawnStamp;
      }
      edges = layout.drawnMeasured;
    }
    const drawnHost = view.querySelector(".knowledge-edges");
    /* 선의 <g>는 창고에 들지 않는다 — 실측(천 쪽, 7회 중앙값): 뗀 <g> 2,497개를 되붙이는
     * 전체 지도 복귀가 71ms, 새로 짓는 것이 46ms. 점은 반대로 되붙이는 쪽이 싸다
     * (`nodeCache`, 5ms) — 점은 옷·색·이름표를 들고 있고 선은 좌표 하나뿐이다. */
    paintGraphEdges(drawnHost, edges, {
      path: ({ at: [fromX, fromY, toX, toY] }) =>
        `M ${fromX.toFixed(KNOWLEDGE_FORCE.coordinateDigits)} `
          + `${fromY.toFixed(KNOWLEDGE_FORCE.coordinateDigits)} L `
          + `${toX.toFixed(KNOWLEDGE_FORCE.coordinateDigits)} `
          + `${toY.toFixed(KNOWLEDGE_FORCE.coordinateDigits)}`,
      dress: (group, edge, line) => {
        /* 본문 링크는 지금의 옷 그대로다. 프론트매터가 이름을 준 선만 제 이름을
         * 입는다 — 여섯 종류에 여섯 벌의 색을 두지 않는 것은 색이 이미 태그의
         * 것이기 때문이고, 여기서 다른 것은 선의 굵기와 실선 여부 하나다. */
        writeAttribute(line, "class", [
          "knowledge-edge",
          edge.ghost ? "is-ghost" : "",
          /* merge 후보(t-2931)는 타입 관계의 옷을 입지 않는다 — 볼트가 선언한
           * 것이 아니라 백엔드가 센 물음이다. */
          edge.kind === "mentions" || edge.kind === "merge" ? "" : "is-typed",
          edge.kind === "merge" ? "is-merge" : "",
          edge.kind === "mentions" ? "" : `kind-${edge.kind}`,
          edge.lit ? "is-lit" : "",
          edge.focused ? "is-focus-lit" : "",
          edge.spoke ? "is-spoke" : "",
          edge.searchMatch ? "is-search-match" : "",
          edge.searchDim ? "is-search-dim" : "",
          edge.spotlit ? "is-spotlit" : "",
          edge.sliceDim ? "is-slice-dim" : "",
          edge.pathLit ? "is-path-lit" : "",
          edge.pathDim ? "is-path-dim" : "",
          edge.inter ? "is-inter" : "is-intra",
        ].filter(Boolean).join(" "));
        if (edge.hue >= 0) writeAttribute(group, "data-hue", String(edge.hue));
        else group.removeAttribute("data-hue");
        writeAttribute(line, "vector-effect", "non-scaling-stroke");
        const directed = KNOWLEDGE_EDGE_DIRECTED.includes(edge.kind);
        if (directed) {
          writeAttribute(line, "marker-end", `url(#knowledge-arrow-${edge.kind})`);
        } else if (line.hasAttribute("marker-end")) {
          line.removeAttribute("marker-end");
        }
      },
    });
    /* 선 번호 → <path>의 표. 페인터가 세운 순서는 **그려진** 선의 순서이므로 도장이
     * 바뀔 때(그리고 새 판이 이 DOM을 입양할 때) 한 번 다시 잇는다. */
    if (layout.edgeElsStamp !== layout.drawnStamp || layout.edgeEls.length !== edgeCount) {
      const els = new Array(edgeCount).fill(null);
      const groups = drawnHost.children;
      let seat = 0;
      for (let at = 0; at < edgeCount; at += 1) {
        if (drawnEdge !== null && drawnEdge[at] === 0) continue;
        els[at] = groups[seat]?.firstElementChild ?? null;
        seat += 1;
      }
      layout.edgeEls = els;
      layout.edgeElsStamp = layout.drawnStamp;
    }
    placeKnowledgeLabels(view, layout, camera, inverse);
    paintKnowledgeEdgeLabels(view, layout);
    const scaleWord = view.querySelector(".knowledge-zoom-label");
    if (scaleWord) writeTextContent(scaleWord, `${Math.round(layout.zoom * 100)}%`);
    layout.paintedGeometry = { revision: layout.geometryRevision, scale: camera.scale,
      zoom: layout.zoom, yScale: camera.yScale, middleY: camera.middleY };
  },
    /* 포인터 아래의 점. SVG에서는 이 답을 쓰는 것이 하네스의 대조뿐이다 —
     * 사람의 손은 요소 자신이 받는다. */
    pick(px, py) {
      const layout = knowledgeLayouts.get(this.view);
      return layout === undefined ? -1 : knowledgePickSeat(layout, px, py);
    },
    /* 놓는다: 세 층의 요소와 판의 창고. 다른 손이 그리기 시작할 때 이 그림이
     * 아래에 남아 있으면 두 그림이 겹친다. */
    dispose() {
      const view = this.view;
      this.view = null;
      if (view === null) return;
      for (const layer of [".knowledge-nodes", ".knowledge-edges", ".knowledge-edge-labels"]) {
        view.querySelector(layer)?.replaceChildren();
      }
      const host = view.querySelector(".knowledge-nodes");
      if (host) {
        host.dataset.knowledgeSignature = "";
        host.dataset.knowledgeVault = "";
      }
      const layout = knowledgeLayouts.get(view);
      if (layout === undefined) return;
      layout.nodeCache.clear();
      layout.edgeLabels.clear();
      layout.nodeEls = [];
      layout.edgeEls = [];
      layout.edgeElsStamp = "";
      layout.paintedModel = null;
      layout.paintedGeometry = null;
    },
  };
}

/* 한 프레임 — 카메라를 재고 이 판의 손에게 넘긴다. 창의 모든 길(팬·배율·비행·
 * 선택·훑기)이 여전히 이 한 문으로 그린다. */
function paintKnowledgeFrame(view, layout, { cameraOnly = false } = {}) {
  knowledgePainterFor(view).paintFrame(layout, knowledgeCamera(view, layout), { cameraOnly });
}

/* Labels face away from the ring, leaving the connections in its centre clear.
 * This uses graph coordinates, never a DOM measurement. A settled pan still
 * returns before visiting nodes; global node construction restores the anchors. */
/* 고리 위 이름표의 자리 — 그래프 좌표에서만 나온다(DOM을 재지 않는다). 두 손이
 * 이 한 셈을 쓴다: SVG는 그 수를 <text>에 쓰고, 요소 없이 그리는 손은 격자가 이
 * 수(`ringLabelAt`)로 상자를 쥐게 한 뒤 쥔 상자의 중심을 읽는다. 답은 배정밀도로
 * 재사용 객체 하나에 담긴다 — `ringLabelAt`은 Float32라 SVG 속성의 글자가 달라지고,
 * 점마다 객체를 짓는 것은 프레임의 할당이다. */
const knowledgeRingSeat = { x: 0, y: 0, side: 0 };
function knowledgeRingLabelSeat(layout, at) {
  const { ring, tuning, radius, inverse } = layout;
  const dx = layout.x[at] - layout.x[ring.seat];
  const dy = layout.y[at] - layout.y[ring.seat];
  const centre = at === ring.seat;
  const side = centre || Math.abs(dx) < tuning.labelGap * inverse ? 0 : Math.sign(dx);
  const offsetX = side * (radius[at] + tuning.labelGap);
  const offsetY = !centre && dy < 0
    ? -radius[at] - tuning.labelGap
    : radius[at] + tuning.labelPx + tuning.labelGap;
  /* 격자에게 이 이름표가 어디에 섰는지 알린다(09-16) — 고리는 제 자리 규칙을
   * 갖지만 「겹치지 않는다」는 두 화면이 나눠 갖는 계약이다. 자리는 여기서,
   * 설지 말지는 격자에서. */
  layout.ringLabelAt[at * 3] = offsetX;
  layout.ringLabelAt[at * 3 + 1] = offsetY;
  layout.ringLabelAt[at * 3 + 2] = side;
  knowledgeRingSeat.x = offsetX;
  knowledgeRingSeat.y = offsetY;
  knowledgeRingSeat.side = side;
  return knowledgeRingSeat;
}

function paintKnowledgeRingLabel(layout, at, node) {
  const seat = knowledgeRingLabelSeat(layout, at);
  const word = node.querySelector(".knowledge-label");
  writeAttribute(word, "text-anchor", seat.side < 0 ? "end" : seat.side > 0 ? "start" : "middle");
  writeAttribute(word, "x", String(seat.x));
  writeAttribute(word, "y", String(seat.y));
}

/* 관계의 낱말은 지금 밝은 선에만 선다.
 *
 * 이천 개의 선에 이천 개의 <text>를 상시로 두면 그것은 범례가 아니라 안개다 —
 * 그리고 그 글자들은 배율이 바뀔 때마다 다시 재어진다. 그래서 여기 서는 것은
 * **호버·포커스가 밝힌 선 중 이름을 가진 것**뿐이고, 그 수는 이웃의 수이지
 * 그림의 크기가 아니다. `mentions`는 본문의 대괄호일 뿐 이름이 아니므로 낱말을
 * 얻지 못한다.
 *
 * 클래스는 **붙이기 전에** 쓴다. 이 층은 호버가 세는 그림(`.knowledge-picture`)
 * 안에 살고, 붙인 뒤에 class를 쓰면 그 쓰기가 호버 한 번의 값에 얹힌다 —
 * 하네스가 이웃 + 선 + 1을 재고 있는 바로 그 수다. */
function paintKnowledgeEdgeLabels(view, layout) {
  const host = view.querySelector(".knowledge-edge-labels");
  /* 밝은 선이 하나도 없고 층도 이미 비어 있으면 한 걸음도 걷지 않는다 — 배치가
   * 앉는 동안 이 함수는 프레임마다 지나고, 그 프레임의 보통은 「설 낱말이
   * 없다」이다. */
  if (layout.lit.size === 0 && layout.focusEdgeCount === 0 && host.children.length === 0) return;
  const model = layout.model;
  const held = layout.edgeLabels;
  const wanted = [];
  const drawnEdge = layout.drawnEdge;
  for (let at = 0; at < model.edgeCount; at += 1) {
    /* 낱말을 얻는 것은 **방향이 있는** 관계뿐이다(3차). `related`는 색이 이미
     * 말하고 방향이 없어 낱말이 더하는 것이 없다 — 실제 볼트에서 허브 하나를
     * 고르면 「related」 서른셋이 그림을 덮었다. 그려지지 않은 선에는 낱말도 없다. */
    const shown = (layout.lit.has(at) || layout.focusEdgeVisited[at] === 1)
      && (drawnEdge === null || drawnEdge[at] === 1)
      && model.kind[at] !== KNOWLEDGE_EDGE_CODE.mentions
      && model.kind[at] !== KNOWLEDGE_EDGE_CODE.related;
    if (!shown) continue;
    let word = held.get(at);
    if (word === undefined) {
      word = document.createElementNS(SVG_NS, "text");
      word.setAttribute("class", "knowledge-edge-label");
      word.setAttribute("text-anchor", "middle");
      word.textContent = knowledgeEdgeWord(model.kind[at]);
      held.set(at, word);
    }
    /* 점의 제목과 같은 손: 자리는 그래프 좌표로, 크기는 배율의 역수로. 이것이
     * 없으면 낱말이 그림과 함께 커져 배율 두 배에서 판을 뒤덮는다. */
    const midX = (layout.x[model.from[at]] + layout.x[model.to[at]]) / 2;
    const midY = (layout.drawY[model.from[at]] + layout.drawY[model.to[at]]) / 2;
    writeAttribute(
      word,
      "transform",
      `translate(${midX.toFixed(KNOWLEDGE_FORCE.coordinateDigits)} `
        + `${midY.toFixed(KNOWLEDGE_FORCE.coordinateDigits)}) scale(${layout.inverse})`,
    );
    wanted.push(word);
  }
  reconcileElementOrder(host, wanted);
}

/* 점들의 DOM. 위상이 그대로면 다시 짓지 않는다 — 프레임마다 하는 일은
 * `transform` 한 줄 쓰기이고, 이 함수는 필터나 스캔이 위상을 바꿀 때만 돈다. */
function paintKnowledgeNodes(view, layout) {
  const model = layout.model;
  const host = view.querySelector(".knowledge-nodes");
  const sameTopology = host.dataset.knowledgeSignature === model.signature
    && layout.measured.length === model.edgeCount;
  const sameDrawn = host.dataset.knowledgeDrawn === layout.drawnStamp;
  if (sameTopology && sameDrawn && layout.paintedModel === model) return;
  /* 서 있는 것과 들어 둔 것(t-4140) 둘 다 이미 있는 점이다 — 주변 탐색이 뺀 점은
   * 판의 창고(`nodeCache`)에 남아 있고, 돌아오면 다시 붙을 뿐 다시 지어지지 않는다. */
  const cache = layout.nodeCache;
  const existing = host.dataset.knowledgeVault === model.vault
    ? new Map([...cache, ...[...host.children].map((node) => [node.dataset.graphKey, node])])
    : new Map();
  const count = model.count;
  const kinds = model.kinds;
  const keys = model.keys;
  const degree = model.degree;
  const titles = model.titles;
  const tuning = layout.tuning;
  const hubFloor = knowledgeHubFloor(degree, count, tuning);
  const drawn = layout.drawn;
  const folded = layout.folded;
  const built = new Array(count);
  const standing = [];
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) {
      built[at] = null;
      continue;
    }
    const node = existing.get(keys[at]) ?? document.createElementNS(SVG_NS, "g");
    if (!existing.has(keys[at])) knowledgeNodeCreations += 1;
    cache.set(keys[at], node);
    const kind = kinds[at];
    const baseClass = kind === "page" ? "knowledge-node is-page"
      : kind === "ghost" ? "knowledge-node is-ghost"
      : `knowledge-node is-${kind}`;
    /* 접힌 허브(t-4140)는 모양으로도 말한다: 옷 하나와 생략한 수. */
    const fold = folded === null ? 0 : folded[at];
    writeAttribute(node, "class", baseClass
      + (keys[at] === knowledgeSelectedKey ? " is-selected" : "")
      + (layout.litNodes.has(at) ? " is-lit" : "")
      + (fold > 0 ? " is-folded" : ""));
    if (fold > 0) node.dataset.folded = String(fold);
    else delete node.dataset.folded;
    node.dataset.graphKey = keys[at];
    node.dataset.graphSeat = String(at);
    /* 색은 군집의 것이다(3차). 유령과 원본은 형태가 뜻이라 색을 입지 않는다. */
    const rank = layout.community[at];
    node.dataset.community = String(rank);
    const hue = layout.communityHue[rank];
    if (hue >= 0 && kind === "page") node.dataset.hue = String(hue);
    else delete node.dataset.hue;
    const deg = degree[at];
    const hub = deg >= hubFloor;
    if (hub) node.dataset.hub = "true";
    else delete node.dataset.hub;
    /* 단(09-16): 크기와 이름표 우선순위의 근거이고, 옷에서는 중심·대표 지식의
     * 테두리가 이것을 읽는다. 「+N」을 단 접힌 허브는 배율과 무관하게 이름이 선다. */
    node.dataset.tier = KNOWLEDGE_TIER_WORD[layout.tier[at]];
    /* 주변 탐색의 접힌 허브는 이름을 격자 밖에서도 세운다(펼칠 수 있어야 하므로). 전체 지도의
     * 「+N」(공급망 멤버)은 격자가 고른다 — 멤버가 열이면 열 개의 이름이 격자를 건너뛰게 된다. */
    if (fold > 0 && knowledgeMode === "local") node.dataset.labelTier = "hub";
    else delete node.dataset.labelTier;
    if (knowledgeSupplyKind(kind)) knowledgeSupplyDress(node, model, at);
    const radius = layout.radius[at];
    /* 모든 점의 halo를 위상 생성 때 한 번 세운다. 호버 때 원을 만들면 DOM churn과
     * 쓰기 수가 포인터 속도에 붙는다. 보통 점의 원은 투명하고 허브·선택·호버만
     * CSS가 밝힌다. */
    const halo = node.querySelector(".knowledge-halo") ?? document.createElementNS(SVG_NS, "circle");
    halo.setAttribute("class", "knowledge-halo");
    halo.setAttribute("r", String(radius * tuning.haloScale));
    if (!halo.parentNode) node.appendChild(halo);
    /* 점의 몸은 종류의 모양이다(`KNOWLEDGE_NODE_SHAPES`). 서 있던 요소가 모양과 어긋나면
     * (`circle`이던 점이 `path`의 모양이 되면) 새 요소가 그 자리에 선다. */
    const held = node.querySelector(".knowledge-dot");
    const dot = knowledgeShapeInk(held, knowledgeShapeOf(kind), radius);
    if (held !== null && held !== dot) held.remove();
    dot.setAttribute("class", "knowledge-dot");
    const word = node.querySelector(".knowledge-label") ?? document.createElementNS(SVG_NS, "text");
    word.setAttribute("class", "knowledge-label");
    word.setAttribute("text-anchor", "middle");
    word.setAttribute("x", "0");
    word.setAttribute("y", String(radius + tuning.labelPx + tuning.labelGap));
    const shortened = knowledgeShortWord(titles[at], tuning.labelMax);
    knowledgeLabelWords(word, shortened, fold);
    /* 이름표 상자의 폭은 여기서 어림해 둔다 — 이름을 쓰는 바로 그 자리이고,
     * 격자는 프레임마다 그 수를 읽기만 한다(DOM에 묻지 않는다). */
    layout.labelEm[at] = knowledgeLabelEm(shortened, tuning)
      + (fold > 0 ? knowledgeLabelEm(`+${fold}`, tuning) : 0);
    if (!dot.parentNode) node.insertBefore(dot, word.parentNode ? word : null);
    if (!word.parentNode) node.appendChild(word);
    built[at] = node;
    standing.push(node);
  }
  reconcileElementOrder(host, standing);
  host.dataset.knowledgeSignature = model.signature;
  host.dataset.knowledgeVault = model.vault;
  host.dataset.knowledgeDrawn = layout.drawnStamp;
  layout.nodeEls = built;
  layout.paintedModel = model;
  view.querySelector(".knowledge-picture").classList.toggle("is-tracing", layout.litNodes.size > 0);
  if (existing.size === 0) arriveKnowledgeLayers(view, layout);
  /* 선의 껍데기도 위상과 함께 한 번만 짓는다. `at`은 프레임마다 제자리에서
   * 고쳐 쓰는 배열이고(그래서 `paintGraphEdges`가 좌표를 복사해 둔다), 이것이
   * 매 프레임 이천 개의 객체를 짓지 않는 이유다. */
  if (!sameTopology) layout.measured = Array.from({ length: model.edgeCount }, (unused, at) => ({
    key: `${model.keys[model.from[at]]}>${model.keys[model.to[at]]}>${KNOWLEDGE_EDGE_KINDS[model.kind[at]]}`,
    at: [0, 0, 0, 0],
    lit: false,
    focused: false,
    spoke: false,
    searchMatch: false,
    searchDim: false,
    inter: false,
    hue: -1,
    spotlit: false,
    sliceDim: false,
    pathLit: false,
    pathDim: false,
    ghost: model.ghost[at] === 1,
    /* 낱말로 든다 — 코드는 모델의 것이고, 사람에게 보일 자리(선의 클래스,
     * 인스펙터의 낱말)가 읽는 것은 낱말이다. */
    kind: KNOWLEDGE_EDGE_KINDS[model.kind[at]],
  }));
}

/* 이름표의 두 낱말(t-4140): 제목과, 접힌 허브라면 생략한 연결의 수. 제목은 첫
 * 텍스트 노드 하나이고 수는 그 뒤의 <tspan>이라, 제목만 바뀐 판은 수를 건드리지
 * 않고 수만 바뀐 판은 제목을 다시 쓰지 않는다. */
function knowledgeLabelWords(word, title, fold) {
  const text = word.firstChild;
  if (text === null || text.nodeType !== Node.TEXT_NODE) word.textContent = title;
  else if (text.data !== title) text.data = title;
  let tail = word.querySelector(".knowledge-fold");
  if (fold > 0) {
    if (tail === null) {
      tail = document.createElementNS(SVG_NS, "tspan");
      tail.setAttribute("class", "knowledge-fold");
      tail.setAttribute("dx", "0.4em");
      word.appendChild(tail);
    }
    writeTextContent(tail, `+${fold}`);
  } else if (tail !== null) {
    tail.remove();
  }
}

/* 선 번호의 <path>, 그려져 있으면. 선의 층을 번호로 직접 세는 것은 전체 지도에서만
 * 맞는 셈이었다(t-4140) — 표는 프레임이 잇는다(`paintKnowledgeFrame`). */
function knowledgeEdgeLine(layout, at) {
  return layout.edgeEls[at] ?? null;
}

/* 도착(3차): 새 위상의 세 층이 한 번 스며든다.
 *
 * Web Animations로 — 클래스도 강제 레이아웃도 없이. 클래스를 뗐다 붙여 애니메이션을
 * 재시작하는 길은 그 사이의 읽기 하나(`offsetWidth`)가 방금 지은 삼천 개의 SVG
 * 요소를 동기 레이아웃으로 재게 했다(실측: 천 페이지의 점 페인트 4→27ms). 층 하나에
 * 애니메이션 하나이고 점마다는 아니다. 움직임을 줄이라는 판에서는 스며들지 않는다. */
function arriveKnowledgeLayers(view, layout) {
  if (knowledgeMotionReduced() || !(layout.tuning.arriveMs > 0)) return;
  for (const layer of view.querySelectorAll(".knowledge-clusters, .knowledge-edges, .knowledge-nodes")) {
    if (typeof layer.animate !== "function") return;
    layer.animate(
      [{ opacity: 0 }, { opacity: 1 }],
      { duration: layout.tuning.arriveMs, easing: layout.tuning.ease },
    );
  }
}

/* 활동 고리(t-2931): 창 안에서 회상된 점은 halo를 고리로 두른다.
 *
 * 매 판 지나지만 쓰는 것은 **달라진 점**뿐이다 — `classList.toggle`은 이미 그
 * 상태인 요소에는 아무것도 쓰지 않는다. 창의 길이는 모델이 백엔드의 표로 이미
 * 재어 두었고, 여기는 옷을 입힐 뿐이다. */
function paintKnowledgeActivity(layout) {
  const on = layout.model.recalledNow;
  for (let at = 0; at < layout.count; at += 1) {
    layout.nodeEls[at]?.classList.toggle("is-recalled", on[at] === 1);
  }
}

/* 새 것의 맥동(t-2931): 지난 답에 없던 점과 선이 한 번 깜빡인다.
 *
 * `arriveKnowledgeLayers`와 같은 손 — Web Animations, 클래스도 강제 레이아웃도
 * 없이 — 그러나 층이 아니라 **요소**마다다: 맥동하는 것은 바뀐 것뿐이고 그 수는
 * 워처가 들은 변화의 수이지 그림의 크기가 아니다. 한 번뿐이다(`knowledgePulsePending`):
 * 같은 답을 다시 그리는 판(렌즈·검색)은 다시 깜빡이지 않는다. 움직임을 줄이라는
 * 판에서는 낱말(`knowledgeFreshKeys`)만 남고 맥동은 없고, 표의 상한을 넘는
 * 변화는 맥동이 아니라 도착이다. */
function pulseKnowledgeFresh(view, layout) {
  if (!knowledgePulsePending) return;
  knowledgePulsePending = false;
  const { pulseMs, pulseDip, pulseMax, ease } = layout.tuning;
  if (knowledgeMotionReduced() || !(pulseMs > 0)) return;
  /* 통째로 바뀐 그림은 변화가 아니라 새 그림이다 — 그것은 도착(층 하나의 페이드)
   * 으로 스며들고, 요소마다의 맥동은 표의 상한 아래에서만이다. 실측: 천 쪽의
   * 점과 선이 한 프레임에 함께 맥동하면 540ms. */
  if (knowledgeFreshKeys.size + knowledgeFreshEdges.size > pulseMax) return;
  const model = layout.model;
  const wanted = [];
  for (let at = 0; at < model.count; at += 1) {
    if (knowledgeFreshKeys.has(model.keys[at]) && layout.nodeEls[at]) wanted.push(layout.nodeEls[at]);
  }
  for (let at = 0; at < model.edgeCount; at += 1) {
    const key = `${model.keys[model.from[at]]}>${model.keys[model.to[at]]}>${KNOWLEDGE_EDGE_KINDS[model.kind[at]]}`;
    if (!knowledgeFreshEdges.has(key)) continue;
    const line = knowledgeEdgeLine(layout, at);
    if (line) wanted.push(line);
  }
  for (const one of wanted) {
    if (typeof one.animate !== "function") return;
    one.animate(
      [{ opacity: 1 }, { opacity: pulseDip }, { opacity: 1 }, { opacity: pulseDip }, { opacity: 1 }],
      { duration: pulseMs, easing: ease },
    );
    knowledgePulses += 1;
  }
}

/* 지난 답에 견줘 이번 답이 새로 가져온 것(t-2931): 새 페이지, 수정 시각이 바뀐
 * 페이지, 지난 답에 없던 선. 원본 점은 세지 않는다 — 「원본도」를 켜는 몸짓이
 * 볼트가 바뀐 것으로 읽히면 안 된다. 첫 답(지난 답 없음)은 아무것도 새것이 아니다:
 * 그때 그림 전체는 이미 도착(`arriveKnowledgeLayers`)으로 스며든다. */
function noteKnowledgeFresh(before, after) {
  knowledgeFreshKeys = new Set();
  knowledgeFreshEdges = new Set();
  knowledgePulsePending = false;
  if (!before?.graph || !after?.graph || before.vault !== after.vault) return;
  const was = new Map(before.graph.nodes.map((node) => [node.id, node.modified_ms]));
  for (const node of after.graph.nodes) {
    if (node.kind !== "page") continue;
    const held = was.get(node.id);
    if (held === undefined || held !== node.modified_ms) knowledgeFreshKeys.add(node.id);
  }
  const keyOf = (graph, edge) => {
    const from = graph.nodes[edge.from];
    const to = graph.nodes[edge.to];
    if (!from || !to || from.kind === "source" || to.kind === "source") return null;
    return `${from.id}>${to.id}>${edge.kind ?? "mentions"}`;
  };
  const had = new Set(before.graph.edges.map((edge) => keyOf(before.graph, edge)));
  for (const edge of after.graph.edges) {
    const key = keyOf(after.graph, edge);
    if (key !== null && !had.has(key)) knowledgeFreshEdges.add(key);
  }
  knowledgePulsePending = knowledgeFreshKeys.size > 0 || knowledgeFreshEdges.size > 0;
}

/* 군집의 층 — 위상이 바뀔 때만 (3차).
 *
 * 이름 있는 군집마다 성운 원 하나와 이름표 하나. 성운이 전부 앞에, 이름표가
 * 전부 뒤에 서는 것은 한 군집의 성운이 이웃 군집의 이름을 덮지 않게 하기
 * 위해서다. 이름표는 진짜 단추처럼 읽힌다(`role="button"`): 누르면 그 군집만
 * 밝히고 카메라가 그리로 난다. */
function paintKnowledgeClusterLayer(view, layout) {
  const host = view.querySelector(".knowledge-clusters");
  const dressLabel = (label, rank) => {
    const name = knowledgeClusterWord(layout, rank);
    writeTextContent(label.firstElementChild, name);
    writeAttribute(label, "aria-label",
      t("knowledge.spotlightCluster", "«{{name}}» 군집만 밝히기", { name }));
    const lead = label.querySelector(".knowledge-cluster-lead");
    if (lead) writeTextContent(lead, knowledgeClusterLeadWord(layout, rank));
  };
  if (host.dataset.knowledgeSignature === layout.signature) {
    /* 같은 위상의 새 판(점의 층이 제 DOM을 다시 입양하는 것과 같은 경우)은 서
     * 있는 것을 그대로 든다 — 앞의 절반이 성운, 뒤의 절반이 이름표 묶음이다. */
    if (layout.clusterEls.length === 0 && host.children.length > 0) {
      const half = host.children.length / 2;
      layout.clusterEls = Array.from({ length: half }, (unused, rank) => ({
        nebula: host.children[rank],
        label: host.children[half + rank],
        lead: host.children[half + rank].querySelector(".knowledge-cluster-lead"),
        count: host.children[half + rank].lastElementChild,
      }));
    }
    for (let rank = 0; rank < layout.clusterEls.length; rank += 1) {
      dressLabel(layout.clusterEls[rank].label, rank);
    }
    return;
  }
  const built = [];
  const nebulas = [];
  const labels = [];
  for (let rank = 0; rank < layout.namedCount; rank += 1) {
    const hue = String(layout.communityHue[rank]);
    /* 원반은 타원이다(09-16): 좁은 판에서 그림은 세로로 눌리므로(`knowledgeYScale`)
     * 원을 그리면 눌린 거리 위에 눌리지 않은 반지름이 얹혀 이웃 원반과 겹친다
     * (실측 900×760: 겹친 원반 셋). 그림과 같은 비율로 눌린 타원은 그 겹침을
     * 정의상 만들지 않는다. */
    const nebula = document.createElementNS(SVG_NS, "ellipse");
    nebula.setAttribute("class", "knowledge-nebula");
    nebula.dataset.hue = hue;
    nebula.dataset.community = String(rank);
    /* 이름표는 묶음이다(09-16): 주제의 이름 한 줄과, 그 아래 「쪽 n · 이름 +m」
     * 한 줄. 둘이 한 몸이라 한 번의 transform으로 함께 선다. */
    const label = document.createElementNS(SVG_NS, "g");
    label.setAttribute("class", "knowledge-cluster-label");
    label.setAttribute("role", "button");
    label.setAttribute("tabindex", "0");
    label.dataset.hue = hue;
    label.dataset.community = String(rank);
    const name = document.createElementNS(SVG_NS, "text");
    name.setAttribute("class", "knowledge-cluster-name");
    name.setAttribute("text-anchor", "middle");
    /* 주제의 이름 아래 그 주제의 **대표 지식** 한 줄(09-16).
     *
     * 전체 지도의 약속은 「주제와 대표 지식」인데, 군집의 공이 촘촘해질수록 그
     * 가운데 점의 이름표가 격자에서 자리를 얻기 어렵다 — 그래서 대표 페이지의
     * 제목은 이름표 경쟁에 맡기지 않고 주제의 이름 바로 아래에 붙인다. 이 판은
     * 통째로 한 번 자리를 예약하므로 어느 배율에서도 읽히고 겹치지 않는다. */
    const lead = document.createElementNS(SVG_NS, "text");
    lead.setAttribute("class", "knowledge-cluster-lead");
    lead.setAttribute("text-anchor", "middle");
    lead.setAttribute("y", String(layout.tuning.clusterLeadGap));
    const tally = document.createElementNS(SVG_NS, "text");
    tally.setAttribute("class", "knowledge-cluster-count");
    tally.setAttribute("text-anchor", "middle");
    tally.setAttribute("y", String(layout.tuning.clusterCountGap));
    label.append(name, lead, tally);
    built.push({ nebula, label, lead, count: tally });
    dressLabel(label, rank);
    nebulas.push(nebula);
    labels.push(label);
  }
  reconcileElementOrder(host, [...nebulas, ...labels]);
  host.dataset.knowledgeSignature = layout.signature;
  layout.clusterEls = built;
}

/* 한 군집만 밝힌다 (3차) — Graphify의 클릭 가능한 커뮤니티 범례.
 *
 * 검색과 같은 종류의 상태다: 멤버십도 좌표도 그대로이고 옷만 바뀐다. 점의 옷은
 * 여기서, 선의 옷은 프레임의 `dress`가(둘 다 붙이기 전에 재는 계약을 지킨다).
 * 성운과 이름표와 인스펙터의 군집 단추도 같은 답을 입는다. */
function paintKnowledgeSpotlight(view, layout) {
  const picture = view.querySelector(".knowledge-picture");
  if (knowledgeClusterPicked >= layout.namedCount) knowledgeClusterPicked = -1;
  const spot = knowledgeClusterPicked;
  picture.classList.toggle("is-spotlight", spot >= 0);
  for (let at = 0; at < layout.count; at += 1) {
    layout.nodeEls[at]?.classList.toggle("is-spotlit", spot >= 0 && layout.community[at] === spot);
  }
  layout.clusterEls.forEach((held, rank) => {
    held.nebula.classList.toggle("is-spotlit", rank === spot);
    held.label.classList.toggle("is-spotlit", rank === spot);
  });
  for (const press of view.querySelectorAll("[data-knowledge-cluster]")) {
    const on = spot >= 0 && Number(press.dataset.knowledgeCluster) === spot;
    press.classList.toggle("is-active", on);
    writeAttribute(press, "aria-pressed", String(on));
  }
}

function toggleKnowledgeCluster(view, rank) {
  const layout = knowledgeLayouts.get(view);
  if (!layout || rank < 0 || rank >= layout.namedCount) return;
  knowledgeClusterPicked = knowledgeClusterPicked === rank ? -1 : rank;
  paintKnowledgeSpotlight(view, layout);
  paintKnowledgeFrame(view, layout);
  /* 고리 위에서는 날지 않는다 — 고리는 이미 다 보인다. */
  if (layout.ring !== null) return;
  const { minX, maxX, minY, maxY } = layout.bounds;
  /* 주제를 고르는 것은 「그 주제를 펼쳐 달라」는 뜻이다(09-16): 카메라가 그 원반을
   * 판에 맞게 채우도록 날아가고, 그 배율에서 점들이 벌어지므로 세부 지식의 제목이
   * 격자에서 자리를 얻는다. 되놓으면 전체 지도로 돌아온다 — 사람이 외운 배율(1)로.
   *
   * 배율의 셈은 카메라의 정의에서 곧장 나온다: 화면의 원반 지름은
   * `2·reach·scale`이고 `scale = (판 폭 · fitRatio / 그림 폭) · zoom`이므로,
   * 원반이 fit과 같은 몫을 차지하는 배율은 `그림 폭 / 2·reach`다. */
  if (knowledgeClusterPicked < 0) {
    flyKnowledgeTo(view, layout, { panX: 0, panY: 0, zoom: 1 });
    return;
  }
  const reach = Math.max(1, layout.communityHomeR[rank]);
  const span = Math.max(maxX - minX, 1);
  flyKnowledgeTo(view, layout, {
    panX: layout.clusterX[rank] - (minX + maxX) / 2,
    panY: layout.clusterY[rank] - (minY + maxY) / 2,
    zoom: Math.min(layout.tuning.zoomMax, Math.max(1, span / (2 * reach))),
  });
}

/* 볼트 건강의 네 줄 중 어느 것이 켜져 있는가 — 고아·유령은 렌즈, 모순·대체는
 * 관계 낱말 검색. 줄 넷이라 매 판 지나도 값이 없다. */
function paintKnowledgeHealthMarks(view) {
  const query = knowledgeQuery.trim().toLocaleLowerCase();
  const lit = {
    orphans: knowledgeOrphansOnly,
    ghosts: knowledgeGhostsOnly,
    index_gaps: knowledgeLintLens === "index_gaps",
    missing_frontmatter: knowledgeLintLens === "missing_frontmatter",
    undeclared_relations: knowledgeLintLens === "undeclared_relations",
    contradictions: query === "contradicts",
    superseded: query === "supersedes",
    recalledToday: knowledgeAliveOnly,
    neverRecalled: knowledgeColdOnly,
    merge: knowledgeMergeOnly,
  };
  for (const press of view.querySelectorAll("button[data-knowledge-lint]")) {
    const on = lit[press.dataset.knowledgeLint] === true;
    press.classList.toggle("is-active", on);
    writeAttribute(press, "aria-pressed", String(on));
  }
}

/* 배치를 프레임에 나눠 돌린다.
 *
 * requestAnimationFrame 밖에서는 한 번도 계산하지 않고, 한 프레임 안에서도
 * 예산(`frameMs`)을 넘기지 않는다. 천 노드의 배치를 통째로 한 프레임에 넣으면
 * 그 프레임 동안 창은 스크롤도 입력도 받지 못한다 — 그리고 남은 예산이 없으면
 * 이 고리는 스스로 멈춘다. 살아 있음의 조건은 「보고 있을 때만」이다. */
function knowledgeSettle(view) {
  const layout = knowledgeLayouts.get(view);
  if (!layout) return;
  scheduleGraphFrame(knowledgeStepFrames, view, () => {
    const held = knowledgeLayouts.get(view);
    if (!held || view.hidden || !view.isConnected) return;
    const began = performance.now();
    let ran = 0;
    /* 고리가 서 있는 동안(주변 탐색)의 훑기는 지도의 것이다: 화면의 고리는 그대로고
     * 전체 지도가 `mapX`·`mapY` 위에서 뒤에서 앉는다 — 첫 화면이 주변 탐색인 볼트도
     * 전체 지도로 나가면 이미 앉은 지도를 만난다. 목차·일지를 접은 판은 지도에서도
     * 그 가면대로 훑는다(S6). */
    const onMap = held.ring !== null;
    const mask = knowledgeNavShown ? null : held.navMask;
    const edgeMask = knowledgeNavShown ? null : held.navEdgeMask;
    /* 두 상한 중 먼저 닿는 쪽: 시간(큰 판)과 보폭(작은 판, `knowledgePace`). */
    while (held.left > 0 && ran < held.pace
      && performance.now() - began < KNOWLEDGE_FORCE.frameMs) {
      if (onMap) knowledgeForceStep(held, held.mapX, held.mapY, mask, edgeMask, held.mapBounds);
      else knowledgeForceStep(held);
      held.left -= 1;
      ran += 1;
    }
    if (ran > 0) {
      /* 자리(`placed`)는 언제나 지도의 것이다 — 고리 좌표는 들어오지 않는다. */
      const seatX = onMap ? held.mapX : held.x;
      const seatY = onMap ? held.mapY : held.y;
      if (!onMap) held.geometryRevision += 1;
      for (let at = 0; at < held.count; at += 1) {
        const key = held.model.keys[at];
        const seat = held.placed.get(key);
        if (seat) {
          seat[0] = seatX[at];
          seat[1] = seatY[at];
        } else {
          held.placed.set(key, [seatX[at], seatY[at]]);
        }
      }
    }
    paintKnowledgeFrame(view, held);
    if (held.left > 0) knowledgeSettle(view);
  });
}

/* ---- 사람의 손 ---- */

function knowledgeNeighbours(layout, seat) {
  const { start, neighbour, throughEdge } = layout.model;
  const nodes = new Set([seat]);
  const edges = new Set();
  for (let at = start[seat]; at < start[seat + 1]; at += 1) {
    nodes.add(neighbour[at]);
    edges.add(throughEdge[at]);
  }
  return { nodes, edges };
}

/* 호버는 이웃만 만진다.
 *
 * 천 개의 점에 클래스를 쓰고 지우는 것은 마우스가 움직일 때마다 천 번의
 * 쓰기이고, 그 표면은 호버 하나에 프레임을 떨어뜨린다. 그러므로 만지는 것은
 * **방금 밝혔던 것과 이제 밝힐 것**뿐이고 — 둘 다 이웃 수만큼이다 — 나머지를
 * 흐리는 일은 판 하나에 붙은 클래스가 CSS로 한다.
 *
 * 프레임을 다시 그리지도 않는다: 좌표는 한 픽셀도 움직이지 않았고, 이천 개의
 * 선을 다시 훑는 것은 마우스가 움직일 때마다 지불할 값이 아니다. `layout.lit`을
 * 함께 갱신해 두므로 다음 프레임의 옷도 여기와 같은 답을 입는다.
 *
 * 선의 <g>는 인덱스로 찾는다 — `reconcileElementOrder`가 넘겨받은 순서 그대로
 * 세우므로, `children[at]`이 `edges[at]`이다. */
function litKnowledge(view, layout, key) {
  const picture = view.querySelector(".knowledge-picture");
  const edgeAt = (at) => knowledgeEdgeLine(layout, at);
  for (const at of layout.litNodes) layout.nodeEls[at]?.classList.remove("is-lit");
  for (const at of layout.lit) edgeAt(at)?.classList.remove("is-lit");
  const seat = key === null ? -1 : layout.model.keys.indexOf(key);
  picture.classList.toggle("is-tracing", seat >= 0);
  if (seat < 0) {
    layout.lit = new Set();
    layout.litNodes = new Set();
    knowledgePainters.get(view)?.paintDress(layout);
    return;
  }
  const near = knowledgeNeighbours(layout, seat);
  for (const at of near.nodes) layout.nodeEls[at]?.classList.add("is-lit");
  for (const at of near.edges) edgeAt(at)?.classList.add("is-lit");
  layout.lit = near.edges;
  layout.litNodes = near.nodes;
  paintKnowledgeEdgeLabels(view, layout);
  /* 옷이 바뀌었다. 호버는 프레임을 부르지 않으므로(클래스가 곧 그림이므로) 요소
   * 없이 그리는 손에게는 여기서 알린다 — 알리지 않으면 GL 판의 호버는 다음
   * 프레임까지 그림에 닿지 않는다(검증 09-16). 아직 선 손이 없으면 곧 올 그림이
   * 옷까지 입힌다. */
  knowledgePainters.get(view)?.paintDress(layout);
}

/* 고른 점에서 깊이 N까지를 밝힌다.
 *
 * 걸어 다니는 것은 모델이 이미 들고 있는 CSR이고, 방문 배열도 큐도 판이 지어질
 * 때 한 번 할당해 둔 것을 제자리에서 다시 쓴다 — 깊이를 세 번 바꾸는 몸짓이
 * 배열 여섯 개를 새로 짓지 않게(§6). 되돌릴 때 훑는 것도 **방금 밝힌 것**뿐이라
 * 값이 그림의 크기를 따르지 않는다. 큐가 곧 밝힌 것의 목록이므로, 지우는 고리가
 * 큐를 읽고 나서야 BFS가 그 자리를 다시 쓴다. */
/* 한 점에서 깊이 N까지의 BFS — 포커스와 주변 탐색이 같은 걸음을 걷는다(t-4140).
 *
 * 걷는 것은 모델의 CSR이고 `visited`·`queue`·`level`은 부르는 쪽이 준 배열이다(판이
 * 지어질 때 한 번 할당한 것). `mask`가 있으면 그 밖의 점은 없는 점이다(그려지지 않은
 * 점을 포커스가 밝히지 않게). `fold`가 있으면 중심이 아닌 허브(차수가 표의 문턱
 * 이상, 펼치지 않은 것)는 걸음이 지나가지 않고 그 자리에 1을 적는다 — 생략한 수는
 * 부르는 쪽이 센다. 답은 큐에 선 점의 수다. */
function knowledgeReach(layout, seat, depth, visited, queue, level, { mask = null, fold = null } = {}) {
  const { start, neighbour, degree, keys } = layout.model;
  visited.fill(0);
  let read = 0;
  let write = 1;
  queue[0] = seat;
  visited[seat] = 1;
  level[seat] = 0;
  while (read < write) {
    const here = queue[read];
    read += 1;
    if (level[here] >= depth) continue;
    if (fold !== null && here !== seat && degree[here] >= KNOWLEDGE_EXPLORE.foldDegree
        && !knowledgeUnfolded.has(keys[here])) {
      fold[here] = 1;
      continue;
    }
    for (let at = start[here]; at < start[here + 1]; at += 1) {
      const next = neighbour[at];
      if (visited[next] === 1) continue;
      if (mask !== null && mask[next] === 0) continue;
      visited[next] = 1;
      level[next] = level[here] + 1;
      queue[write] = next;
      write += 1;
    }
  }
  return write;
}

/* 주변 탐색이 그리는 하위 그래프(t-4140): 중심에서 깊이까지, 접힌 허브 너머는 빼고.
 *
 * 답은 판의 `drawn`(점)·`drawnEdge`(양 끝이 다 선 선)·`folded`(자리마다 생략한 연결
 * 수)와 도장이다. 전체 지도는 `null` — 「전부」이고 아무것도 세지 않는다. 도장이
 * 바뀌었으면 경계도 다시 잰다: 카메라는 그려진 것을 채운다. 참을 답하면 도장이
 * 바뀐 것이다. */
function knowledgeSubgraph(layout) {
  const model = layout.model;
  const local = knowledgeMode === "local";
  const seat = local && knowledgeSelectedKey !== null ? model.keys.indexOf(knowledgeSelectedKey) : -1;
  /* 목차·일지를 접은 판(S6)은 전체 지도에서도 부분집합이다 — 같은 가면, 같은 손. */
  const navHidden = !knowledgeNavShown;
  const stamp = local
    ? `local|${seat}|${knowledgeFocusDepth}|${knowledgeUnfoldGeneration}|${navHidden ? "nav" : ""}`
    : (navHidden ? "all-nav" : "all");
  if (stamp === layout.drawnStamp) return false;
  layout.drawnStamp = stamp;
  /* 전체 지도로 나가면 고리를 놓고 지도의 좌표를 되돌린다(시안 이식). */
  if (!local) knowledgeRingRelease(layout);
  /* 전체 지도의 「+N」은 공급망 멤버의 접힌 구성요소 수다(P4, `model.supplyFolded`) — 판이 제 배열에
   * 옮겨 든다(모델의 배열을 판이 지우지 않게). 주변 탐색의 「+N」은 그 탐색이 접은 허브의 것이다. */
  const supplyFolded = model.supplyFolded;
  if (!local && !navHidden) {
    layout.drawn = null;
    layout.drawnEdge = null;
    layout.folded = supplyFolded === null ? null : (layout.folded ?? new Int32Array(model.count));
    if (supplyFolded !== null) layout.folded.set(supplyFolded);
    knowledgeBounds(layout);
    return true;
  }
  const count = model.count;
  const drawn = layout.drawn ?? new Uint8Array(count);
  const drawnEdge = layout.drawnEdge ?? new Uint8Array(model.edgeCount);
  const folded = layout.folded ?? new Int32Array(count);
  drawn.fill(0);
  drawnEdge.fill(0);
  folded.fill(0);
  /* 접힌 관리용 문서는 걸음이 지나지 않는 점이다(가면) — 전체 지도에서는 그 점만 빠진다. */
  let mask = null;
  if (navHidden) {
    mask = layout.navMask ?? new Uint8Array(count);
    for (let at = 0; at < count; at += 1) mask[at] = KNOWLEDGE_NAV_PAGES.includes(model.keys[at]) ? 0 : 1;
    layout.navMask = mask;
    const edgeMask = layout.navEdgeMask ?? new Uint8Array(model.edgeCount);
    for (let at = 0; at < model.edgeCount; at += 1) edgeMask[at] = mask[model.from[at]] & mask[model.to[at]];
    layout.navEdgeMask = edgeMask;
  }
  if (!local) {
    drawn.set(mask);
    for (let at = 0; at < model.edgeCount; at += 1) {
      drawnEdge[at] = drawn[model.from[at]] & drawn[model.to[at]];
    }
    if (supplyFolded !== null) folded.set(supplyFolded);
  } else if (seat >= 0) {
    knowledgeReach(layout, seat, knowledgeFocusDepth, drawn, layout.drawnQueue, layout.drawnLevel,
      { fold: folded, mask });
    const { from, to, start, neighbour } = model;
    for (let at = 0; at < model.edgeCount; at += 1) {
      drawnEdge[at] = drawn[from[at]] & drawn[to[at]];
    }
    /* 접힌 허브의 생략 수: 그 이웃 중 서지 않은 것. 전부 서 있으면 접힌 것이 아니다. */
    for (let at = 0; at < count; at += 1) {
      if (folded[at] === 0) continue;
      let omitted = 0;
      for (let seatAt = start[at]; seatAt < start[at + 1]; seatAt += 1) {
        if (drawn[neighbour[seatAt]] === 0) omitted += 1;
      }
      folded[at] = omitted;
    }
  }
  layout.drawn = drawn;
  layout.drawnEdge = drawnEdge;
  layout.folded = folded;
  if (local && seat >= 0) knowledgeRingArrange(layout, seat);
  else knowledgeRingRelease(layout);
  knowledgeBounds(layout);
  return true;
}

/* ---- 주변 탐색의 고리 (시안 이식) ----
 *
 * 주변 탐색은 중심을 가운데에 두고 이웃을 고리에 세운다 — 승인된 시안
 * (`docs/design/knowledge-graph-20260914/prototype.html`)의 형태다. 전체 지도의 좌표를
 * 그대로 쓰면 이웃이 지도 위 제자리에 흩어져 세로로 넘치고(09-14 실측: 1009×825 판에서
 * 이웃이 y 942·992·−179), 그 그림은 「주변」으로 읽히지 않는다.
 *
 * 깊이 k의 점은 k번째 고리에 선다. 첫 고리는 군집 순이라 색의 띠가 이어지고, 바깥
 * 고리는 제 어버이(한 단 안쪽의 첫 이웃)의 각도 순이라 자식이 어버이 곁에 서고 선이
 * 바퀴살로 뻗는다. 고리의 반지름은 점의 수가 정한다(점 사이 호 ≥ 토큰의 pitch) —
 * 이웃이 많은 중심은 큰 고리다. 자리는 전부 입력에서 나오므로 같은 중심은 언제나 같은
 * 그림이다.
 *
 * 전체 지도의 좌표는 `mapX`·`mapY`에 들어 있다가 돌아갈 때 그대로 되돌아온다 — 공간
 * 기억은 지도의 것이고 고리는 이 화면의 것이다. 고리가 선 동안의 훑기(`knowledgeSettle`)
 * 도 지도 위에서 돈다. */
function knowledgeRingArrange(layout, seat) {
  const { count, x, y, drawn, drawnLevel, community, ringAngle } = layout;
  const { start, neighbour, keys } = layout.model;
  const { ringPitch, ringMin, ringGap, ringSquash } = layout.tuning;
  if (layout.ring === null) {
    layout.mapX = Float32Array.from(x);
    layout.mapY = Float32Array.from(y);
    knowledgeExtent(count, layout.mapX, layout.mapY, null, layout.mapBounds);
  }
  const centreX = layout.mapX[seat];
  const centreY = layout.mapY[seat];
  x[seat] = centreX;
  y[seat] = centreY;
  ringAngle[seat] = 0;
  let deepest = 0;
  for (let at = 0; at < count; at += 1) {
    if (drawn[at] === 1 && drawnLevel[at] > deepest) deepest = drawnLevel[at];
  }
  const byKey = (left, right) => (keys[left] < keys[right] ? -1 : keys[left] > keys[right] ? 1 : 0);
  const radii = [0];
  let radius = 0;
  for (let level = 1; level <= deepest; level += 1) {
    const ring = [];
    for (let at = 0; at < count; at += 1) {
      if (drawn[at] === 1 && at !== seat && drawnLevel[at] === level) ring.push(at);
    }
    if (ring.length === 0) {
      radii.push(radius);
      continue;
    }
    if (level > 1) {
      for (const at of ring) {
        let parent = seat;
        for (let near = start[at]; near < start[at + 1]; near += 1) {
          const other = neighbour[near];
          if (drawn[other] === 1 && drawnLevel[other] === level - 1) {
            parent = other;
            break;
          }
        }
        ringAngle[at] = ringAngle[parent];
      }
      ring.sort((left, right) => ringAngle[left] - ringAngle[right]
        || community[left] - community[right] || byKey(left, right));
    } else {
      ring.sort((left, right) => community[left] - community[right] || byKey(left, right));
    }
    radius = Math.max(level === 1 ? ringMin : radius + ringGap, (ring.length * ringPitch) / (2 * Math.PI));
    radii.push(radius);
    const step = (2 * Math.PI) / ring.length;
    for (let index = 0; index < ring.length; index += 1) {
      const at = ring[index];
      const angle = -Math.PI / 2 + index * step;
      ringAngle[at] = angle;
      x[at] = centreX + Math.cos(angle) * radius;
      y[at] = centreY + Math.sin(angle) * radius * ringSquash;
    }
  }
  layout.ring = { seat, x: centreX, y: centreY, radius: radius > 0 ? radius : ringMin, radii, squash: ringSquash };
  layout.geometryRevision += 1;
}

/* 고리를 놓는다 — 지도의 좌표가 돌아오고, 다음 경계는 다시 점들의 것이다. */
function knowledgeRingRelease(layout) {
  if (layout.ring === null) return;
  layout.x.set(layout.mapX);
  layout.y.set(layout.mapY);
  layout.mapX = null;
  layout.mapY = null;
  layout.ring = null;
  layout.geometryRevision += 1;
}

/* 접힌 허브의 묶음(t-4140): 생략한 연결을 관계 종류별로 센 낱말. 관계의 자리 순서
 * (`KNOWLEDGE_EDGE_KINDS`)로 서고, 낱말은 볼트의 관계 이름 그대로다. */
function knowledgeFoldWords(layout, seat) {
  const { start, neighbour, throughEdge, kind } = layout.model;
  const tally = new Int32Array(KNOWLEDGE_EDGE_KINDS.length);
  let omitted = 0;
  for (let at = start[seat]; at < start[seat + 1]; at += 1) {
    if (layout.drawn[neighbour[at]] === 1) continue;
    tally[kind[throughEdge[at]]] += 1;
    omitted += 1;
  }
  const parts = [t("knowledge.foldedLinks", "접힘 {{count}}", { count: omitted })];
  for (let code = 0; code < tally.length; code += 1) {
    if (tally[code] > 0) parts.push(`${knowledgeEdgeWord(code)} ${tally[code]}`);
  }
  return parts.join(" · ");
}

function knowledgeFocus(view, layout) {
  const model = layout.model;
  const picture = view.querySelector(".knowledge-picture");
  const visited = layout.focusVisited;
  const queue = layout.focusQueue;
  const level = layout.focusLevel;
  const edgeVisited = layout.focusEdgeVisited;
  for (let at = 0; at < layout.focusNodeCount; at += 1) {
    layout.nodeEls[queue[at]]?.classList.remove("is-focus-lit");
  }
  layout.focusNodeCount = 0;
  visited.fill(0);
  edgeVisited.fill(0);
  /* 그려진 경로가 있으면 포커스는 물러선다 — 흐림은 경로의 것(`is-path-dim`)이다.
   * 포커스의 흐림이 남으면 경로의 먼 끝, 사람이 방금 누른 점까지 흐려진다(K20). */
  if (layout.pathShown) {
    picture.classList.remove("is-focused");
    layout.focusEdgeCount = 0;
    return;
  }
  const seat = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
  picture.classList.toggle("is-focused", seat >= 0);
  if (seat < 0) return;
  /* 주변 탐색에서 깊이는 그려지는 반경이다(`knowledgeSubgraph`) — 밝히는 것은 직접
   * 연결뿐이라 첫 고리는 또렷하고 둘째 고리는 물러선다(t-4140). */
  const depth = knowledgeMode === "local" ? KNOWLEDGE_FOCUS_DEPTHS[0] : knowledgeFocusDepth;
  const write = knowledgeReach(layout, seat, depth, visited, queue, level, { mask: layout.drawn });
  for (let at = 0; at < write; at += 1) {
    layout.nodeEls[queue[at]]?.classList.add("is-focus-lit");
  }
  layout.focusNodeCount = write;
  /* 선은 **양 끝이 다 밝을 때** 밝다. 한쪽만 든 선은 그림 밖으로 나가는 화살표가
   * 되고, 그 화살표는 여기 없는 점을 가리킨다. */
  let lit = 0;
  for (let at = 0; at < model.edgeCount; at += 1) {
    if (visited[model.from[at]] === 1 && visited[model.to[at]] === 1) {
      edgeVisited[at] = 1;
      lit += 1;
    }
  }
  layout.focusEdgeCount = lit;
}

/* 최단 경로 (Bloom 문법): 무방향 BFS로 sourceSeat에서 targetSeat까지의 최단 경로를 찾는다.
 * 같은 거리면 타입 간선(frontmatter 관계)을 mentions보다 우선한다. */
function findKnowledgeShortestPath(model, sourceSeat, targetSeat) {
  if (sourceSeat < 0 || targetSeat < 0 || sourceSeat === targetSeat) return null;
  const count = model.count;
  const { start, neighbour, throughEdge, kind } = model;
  const dist = new Int32Array(count).fill(-1);
  const typedScore = new Int32Array(count).fill(0);
  const parent = new Map();
  const queue = [sourceSeat];
  dist[sourceSeat] = 0;
  typedScore[sourceSeat] = 0;

  while (queue.length > 0) {
    const u = queue.shift();
    if (u === targetSeat) break;
    const edges = [];
    for (let at = start[u]; at < start[u + 1]; at += 1) {
      const v = neighbour[at];
      const edge = throughEdge[at];
      const edgeKind = kind[edge];
      edges.push({ v, edge, edgeKind });
    }
    edges.sort((a, b) => (b.edgeKind > 0 ? 1 : 0) - (a.edgeKind > 0 ? 1 : 0));

    for (const { v, edge, edgeKind } of edges) {
      const isTyped = edgeKind > 0 ? 1 : 0;
      const nextDist = dist[u] + 1;
      const nextTyped = typedScore[u] + isTyped;
      if (dist[v] === -1) {
        dist[v] = nextDist;
        typedScore[v] = nextTyped;
        parent.set(v, { prev: u, edge, kind: edgeKind });
        queue.push(v);
      } else if (dist[v] === nextDist && nextTyped > typedScore[v]) {
        typedScore[v] = nextTyped;
        parent.set(v, { prev: u, edge, kind: edgeKind });
      }
    }
  }

  if (dist[targetSeat] === -1) return null;

  const nodes = [];
  const edges = [];
  let curr = targetSeat;
  while (curr !== sourceSeat) {
    nodes.push(curr);
    const p = parent.get(curr);
    edges.push(p.edge);
    curr = p.prev;
  }
  nodes.push(sourceSeat);
  nodes.reverse();
  edges.reverse();
  return { nodes, edges };
}

/* 경로의 길 — 두 열쇠를 **이 그림**(모델)에서 다시 찾는다. 자리와 선의 번호는 모델의
 * 것이라 렌즈 하나·워처의 새 답 하나에 다른 페이지를 가리키게 되고, 그 번호로 밝힌
 * 경로는 엉뚱한 점과 엉뚱한 사슬이었다(K21). 끝점이 그림에 없거나 이어지지 않으면
 * 길이 없다(null) — 열쇠는 남아 있어 그 페이지들이 돌아오면 경로도 돌아온다. */
function knowledgeRoute(model, path) {
  if (path === null) return null;
  return findKnowledgeShortestPath(
    model,
    model.keys.indexOf(path.sourceKey),
    model.keys.indexOf(path.targetKey),
  );
}

function runKnowledgeShortestPath(view, layout, sourceKey, targetKey) {
  const path = { sourceKey, targetKey };
  const route = knowledgeRoute(layout.model, path);
  if (route === null) return;
  knowledgePath = path;
  highlightKnowledgePath(view, layout, route);
  knowledgeFocus(view, layout);
  paintKnowledgeInspector(view, layout);
  paintKnowledgeFrame(view, layout);
}

/* 경로를 놓는다. 선의 옷은 다음 프레임이 벗긴다 — 부르는 쪽이 프레임을 그린다. */
function clearKnowledgePath(view, layout) {
  if (!knowledgePath) return;
  knowledgePath = null;
  highlightKnowledgePath(view, layout);
  const chain = view.querySelector(".knowledge-inspector-chain");
  if (chain) chain.hidden = true;
}

/* 경로를 밝힌다(길이 없으면 벗긴다): 점의 옷은 여기서, 선의 옷은 `pathEdge`를 읽는
 * 프레임의 `dress`가 — 여기서 선에 붙인 클래스는 다음 프레임이 통째로 다시 쓰는 class
 * 속성에 지워졌다(K19). 길은 판에 든다(`pathChain`) — 인스펙터의 사슬이 같은 답을 읽는다. */
function highlightKnowledgePath(view, layout, route = knowledgeRoute(layout.model, knowledgePath)) {
  const picture = view.querySelector(".knowledge-picture");
  /* 벗길 것도 없으면 한 걸음도 걷지 않는다 — 판(`picture`)은 판이 갈려도 남으므로
   * 그 옷이 「그려진 경로가 있는가」의 답이다. */
  if (route === null && !layout.pathShown && !picture.classList.contains("is-path")) return;
  layout.pathChain = route;
  layout.pathShown = route !== null;
  picture.classList.toggle("is-path", layout.pathShown);
  const pathNodes = new Set(route?.nodes ?? []);
  for (let at = 0; at < layout.count; at += 1) {
    const on = pathNodes.has(at);
    layout.pathNode[at] = on ? 1 : 0;
    layout.nodeEls[at]?.classList.toggle("is-path-lit", on);
    layout.nodeEls[at]?.classList.toggle("is-path-dim", layout.pathShown && !on);
  }
  layout.pathEdge.fill(0);
  for (const at of route?.edges ?? []) layout.pathEdge[at] = 1;
}

function selectKnowledgeNode(view, key) {
  /* 주변 탐색에서 고르는 것은 중심을 옮기는 일이고(t-4140), 놓는 것은 중심 없는
   * 주변이 없으므로 전체 지도로 돌아가는 일이다. 같은 점을 다시 고르면 아래의
   * 보통 길이다 — 카드와 프레임만 다시 선다. */
  if (knowledgeMode === "local" && knowledgeLayouts.has(view)) {
    if (key === null) {
      setKnowledgeMode(view, "global");
      return;
    }
    if (key !== knowledgeSelectedKey) {
      exploreKnowledge(view, key);
      return;
    }
  }
  const before = knowledgeSelectedKey;
  knowledgeSelectedKey = key;
  const layout = knowledgeLayouts.get(view);
  if (!layout) return;
  if (before !== key) {
    clearKnowledgePath(view, layout);
    const seatBefore = before === null ? -1 : layout.model.keys.indexOf(before);
    layout.nodeEls[seatBefore]?.classList.remove("is-selected");
  }
  const seat = key === null ? -1 : layout.model.keys.indexOf(key);
  layout.nodeEls[seat]?.classList.add("is-selected");
  knowledgeFocus(view, layout);
  paintKnowledgeInspector(view, layout);
  paintKnowledgeFrame(view, layout);
}

/* 화살표가 점 사이를 걷는다. 순서는 백엔드가 정렬해 준 그대로 — 경로순이므로
 * 「같은 폴더의 다음 페이지」가 다음 정지점이고, 그것이 키보드로 볼트를 읽는
 * 사람이 기대하는 순서다. */
function knowledgeWalk(view, layout, key) {
  const model = layout.model;
  if (model.count === 0) return;
  /* 주변 탐색에서는 그려진 점 사이를 걷는다(t-4140) — 걸음마다 중심이 옮고, 카메라는
   * 새 중심의 주변을 채우므로 따로 날지 않는다. */
  const drawn = layout.drawn;
  const seats = [];
  for (let seat = 0; seat < model.count; seat += 1) {
    if (drawn === null || drawn[seat] === 1) seats.push(seat);
  }
  if (seats.length === 0) return;
  const here = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
  const at = seats.indexOf(here);
  const last = seats.length - 1;
  const step = key === "Home" ? 0
    : key === "End" ? last
      : key === "ArrowDown" || key === "ArrowRight" ? Math.min(last, at + 1)
        : Math.max(0, at <= 0 ? 0 : at - 1);
  const next = seats[step];
  selectKnowledgeNode(view, model.keys[next]);
  if (drawn === null) knowledgeCenterOn(view, layout, next);
}

/* ---- 탐색 모드 (t-4140) ---- */

/* 주변 탐색의 첫 중심 — 고른 점, 아니면 마지막 방문, 아니면 가장 최근에 회상된
 * 페이지(사람이 방금 쓴 지식), 아니면 연결이 가장 많은 페이지(개요의 첫 허브와 같은
 * 규칙: 차수 내림차순, 동률은 열쇠순이라 결정적이다). 목차·일지(`KNOWLEDGE_NAV_PAGES`)는
 * 차수로 중심이 되지 못한다 — 천 쪽을 가리키는 색인을 중심에 두면 주변 탐색이 볼트
 * 전체가 된다. 그림에 없는 페이지는 중심이 되지 못한다. 페이지 수는 여기 어디에도
 * 없다 — 노드 수로 모드를 고르지 않는 것은 소스 계약이 센다(`knowledgeStartMode`). */
function knowledgeDefaultCentre(layout) {
  const model = layout.model;
  if (knowledgeSelectedKey !== null && model.keys.includes(knowledgeSelectedKey)) return knowledgeSelectedKey;
  for (let back = knowledgeHistory.length - 1; back >= 0; back -= 1) {
    const held = knowledgeHistory[back];
    if (held.vault === model.vault && model.keys.includes(held.key)) return held.key;
  }
  let best = -1;
  let recalled = -1;
  for (let at = 0; at < model.count; at += 1) {
    if (model.kinds[at] !== "page") continue;
    const when = model.recalledAt[at] || 0;
    if (when > 0 && (recalled < 0 || when > (model.recalledAt[recalled] || 0)
        || (when === model.recalledAt[recalled] && model.keys[at] < model.keys[recalled]))) recalled = at;
    if (KNOWLEDGE_NAV_PAGES.includes(model.keys[at])) continue;
    if (best < 0 || model.degree[at] > model.degree[best]
        || (model.degree[at] === model.degree[best] && model.keys[at] < model.keys[best])) best = at;
  }
  if (recalled >= 0) return model.keys[recalled];
  return best < 0 ? null : model.keys[best];
}

/* 어느 모드로 서는가 — 「연결 보기」로 들어왔으면 주변, 아니면 이 볼트의 마지막
 * 모드, 처음이면 **주변 탐색**: 승인된 시안(`docs/design/knowledge-graph-20260914/
 * prototype.html`)이 그 화면이고, 330쪽 볼트의 전체 지도는 첫 화면으로 읽히지 않는다
 * (09-14 실측). 중심은 `knowledgeDefaultCentre`가 고르고, 전체 지도는 한 번 누르면
 * 이 볼트의 마지막 모드로 남는다. 노드 수는 여기 없다: 사람의 화면을 그림의 크기가
 * 갈아 끼우지 않는다는 것이 이 함수의 계약이고, 소스 계약이 `count`·`length`·`pages`가
 * 이 안에 없음을 센다. */
function knowledgeStartMode(stored, reveal) {
  if (reveal !== null) return "local";
  if (KNOWLEDGE_MODES.some((row) => row.id === stored?.mode)) return stored.mode;
  return KNOWLEDGE_FIRST_MODE;
}

/* 모드를 바꾼다. 주변으로 들어갈 때 전체 지도의 카메라를 들어 두고 새 중심의 주변을
 * 채우도록 카메라를 놓는다; 돌아올 때 그 카메라를 돌려준다. 중심 없이는 들어가지
 * 않는다(`knowledgeDefaultCentre`가 없다고 하면 그대로). `paint`를 끄면 부르는 쪽이
 * 그린다 — 시야 복원처럼 카메라를 뒤에 얹는 길. */
function setKnowledgeMode(view, mode, { centre = null, paint = true } = {}) {
  const layout = knowledgeLayouts.get(view);
  if (!layout) {
    knowledgeMode = mode;
    return;
  }
  const was = knowledgeMode;
  if (mode === "local") {
    const key = centre ?? knowledgeDefaultCentre(layout);
    if (key === null) return;
    if (was !== "local") {
      knowledgeGlobalCamera = {
        zoom: layout.zoom, panX: layout.panX, panY: layout.panY,
        zoomTaken: layout.zoomTaken, fitSpan: layout.fitSpan, fitTall: layout.fitTall,
      };
      layout.flight = null;
      layout.zoom = 1;
      layout.panX = 0;
      layout.panY = 0;
      layout.zoomTaken = false;
      layout.fitSpan = 0;
      layout.fitTall = 0;
      knowledgeMode = "local";
    }
    if (key !== knowledgeSelectedKey) {
      clearKnowledgePath(view, layout);
      knowledgeSelectedKey = key;
    }
    knowledgeUnfolded.clear();
    knowledgeUnfoldGeneration += 1;
  } else {
    if (was === "local" && knowledgeGlobalCamera !== null) {
      layout.flight = null;
      layout.zoom = knowledgeGlobalCamera.zoom;
      layout.panX = knowledgeGlobalCamera.panX;
      layout.panY = knowledgeGlobalCamera.panY;
      layout.zoomTaken = knowledgeGlobalCamera.zoomTaken;
      layout.fitSpan = knowledgeGlobalCamera.fitSpan;
      layout.fitTall = knowledgeGlobalCamera.fitTall ?? 0;
      knowledgeGlobalCamera = null;
    }
    knowledgeMode = KNOWLEDGE_MODES[0].id;
  }
  if (was !== knowledgeMode) noteKnowledgeExplore(layout.model.vault);
  if (paint) void paintKnowledgeView();
}

/* 주변 탐색에서 중심을 옮긴다: 지금의 중심·깊이·카메라를 이력에 얹고, 펼친 허브를
 * 다시 접고, 새 중심의 주변을 채우도록 팬을 놓는다 — 배율은 사람의 것이라 둔다. */
function exploreKnowledge(view, key) {
  const layout = knowledgeLayouts.get(view);
  if (!layout || key === null || key === knowledgeSelectedKey) return;
  if (knowledgeSelectedKey !== null) {
    knowledgeHistory.push({
      vault: layout.model.vault, key: knowledgeSelectedKey, depth: knowledgeFocusDepth,
      zoom: layout.zoom, panX: layout.panX, panY: layout.panY,
    });
    if (knowledgeHistory.length > KNOWLEDGE_EXPLORE.historyMax) knowledgeHistory.shift();
  }
  knowledgeUnfolded.clear();
  knowledgeUnfoldGeneration += 1;
  clearKnowledgePath(view, layout);
  knowledgeSelectedKey = key;
  layout.flight = null;
  layout.panX = 0;
  layout.panY = 0;
  layout.fitSpan = 0;
  layout.fitTall = 0;
  noteKnowledgeExplore(layout.model.vault);
  void paintKnowledgeView();
}

/* ← 이전: 이력의 맨 위 걸음으로 — 중심·깊이·배율·팬 그대로. 다른 볼트의 걸음과
 * 그림에서 사라진 페이지의 걸음은 건너뛴다. */
function knowledgeBack(view) {
  const layout = knowledgeLayouts.get(view);
  if (!layout) return;
  let held = knowledgeHistory.pop();
  while (held !== undefined
    && (held.vault !== layout.model.vault || !layout.model.keys.includes(held.key))) {
    held = knowledgeHistory.pop();
  }
  if (held === undefined) return;
  knowledgeFocusDepth = held.depth;
  knowledgeUnfolded.clear();
  knowledgeUnfoldGeneration += 1;
  clearKnowledgePath(view, layout);
  knowledgeSelectedKey = held.key;
  layout.flight = null;
  layout.zoom = held.zoom;
  layout.panX = held.panX;
  layout.panY = held.panY;
  layout.fitSpan = 0;
  layout.fitTall = 0;
  noteKnowledgeExplore(layout.model.vault);
  void paintKnowledgeView();
}

/* ---- 마지막 자리 (S2) ----
 *
 * 볼트별 JSON 한 줄(`second_brain_explore`): `{ mode, centre, depth }`. 줄의 주인은
 * 설정 문서다 — 부팅 보고와 모든 스냅샷이 `applySettingsSnapshot`을 지나며 이 표를
 * 갈아 끼우고(`noteKnowledgeExploreLines`), 읽는 쪽은 여기서만 푼다. 시야의 표
 * (`knowledgeSceneLines`)와 같은 손이고 같은 이유다(K18). */
function noteKnowledgeExploreLines(lines) {
  knowledgeExploreLines = lines !== null && typeof lines === "object" ? lines : {};
}

function knowledgeExploreFor(vault) {
  const raw = knowledgeExploreLines[vault];
  if (typeof raw !== "string" || raw === "") return null;
  try {
    const parsed = JSON.parse(raw);
    return parsed !== null && typeof parsed === "object" ? parsed : null;
  } catch {
    // 읽을 수 없는 줄은 없는 줄이다 — 그때의 답은 「처음」이다.
    return null;
  }
}

/* 탐색의 자리가 바뀌었다 — 어느 볼트에서. 쓰기는 미뤄서 한 번이고(`persistMs`),
 * 적는 것은 그때의 모드·중심·깊이다. 같은 줄은 다시 쓰지 않는다. */
function noteKnowledgeExplore(vault) {
  if (!vault) return;
  if (knowledgeExploreTimer !== 0) clearTimeout(knowledgeExploreTimer);
  knowledgeExploreTimer = setTimeout(() => {
    knowledgeExploreTimer = 0;
    void saveKnowledgeExplore(vault, {
      mode: knowledgeMode, centre: knowledgeSelectedKey, depth: knowledgeFocusDepth,
    });
  }, KNOWLEDGE_EXPLORE.persistMs);
}

/* 쓰기는 다른 설정과 같은 문(`commitSetting`)을 지난다: 표는 먼저 갱신하고, 거절되면
 * 그 문이 권위 있는 문서를 다시 읽어 이 표를 되돌린다. */
async function saveKnowledgeExplore(vault, explore) {
  const line = JSON.stringify(explore);
  if (knowledgeExploreLines[vault] === line) return;
  knowledgeExploreLines = { ...knowledgeExploreLines, [vault]: line };
  await commitSetting("second_brain_explore", "set_second_brain_explore", { vault, explore: line });
}

/* 문을 연 뒤의 첫 그림에서 한 번: 어느 모드로 서는가(`knowledgeStartMode`), 어디를
 * 중심으로, 어느 깊이로. 「연결 보기」의 열쇠는 그 페이지, 저장된 줄의 중심은 그림에
 * 있을 때만 — 없으면 기본 중심(`knowledgeDefaultCentre`)이다. 답이 아직 없거나 다른
 * 볼트의 옛 답이면 묻지 않은 채 둔다. */
function applyKnowledgeEntry(view, layout) {
  if (!knowledgeEntryPending || knowledgeReport === null) return;
  if (knowledgeReport.vault !== (secondBrainVault || knowledgeReport.vault)) return;
  knowledgeEntryPending = false;
  const model = layout.model;
  const reveal = knowledgeRevealMode === "local" && knowledgeRevealKey !== null
    && model.keys.includes(knowledgeRevealKey) ? knowledgeRevealKey : null;
  const stored = knowledgeExploreFor(model.vault);
  const mode = knowledgeStartMode(stored, reveal);
  if (mode === "local") {
    const remembered = typeof stored?.centre === "string" && model.keys.includes(stored.centre)
      ? stored.centre : null;
    if (reveal === null && KNOWLEDGE_FOCUS_DEPTHS.includes(stored?.depth)) knowledgeFocusDepth = stored.depth;
    setKnowledgeMode(view, "local", { centre: reveal ?? remembered, paint: false });
  } else if (knowledgeMode === "local") {
    setKnowledgeMode(view, KNOWLEDGE_MODES[0].id, { paint: false });
  }
}

/* 이 볼트의 이력이 있는가 — ← 이전이 설 수 있는가. */
function knowledgeHasHistory(vault) {
  return knowledgeHistory.some((held) => held.vault === vault);
}

/* 토글과 빵부스러기의 옷. 눌린 모드는 `aria-pressed`, 중심은 `aria-current`, 그리고
 * 그려진 페이지 수와 접힌 허브 수 — 「전체와 표시 중인 수를 구분한다」. */
function paintKnowledgeMode(view, layout) {
  const model = layout.model;
  for (const step of view.querySelectorAll(".knowledge-mode-step")) {
    const on = step.dataset.knowledgeMode === knowledgeMode;
    writeAttribute(step, "aria-pressed", String(on));
    step.classList.toggle("is-active", on);
  }
  const crumb = view.querySelector(".knowledge-crumb");
  const local = knowledgeMode === "local";
  crumb.hidden = !local;
  view.querySelector(".knowledge-picture").classList.toggle("is-local", local);
  /* 연결 깊이의 줄은 빵부스러기와 함께 선다(시안). 단의 낱말은 매 그림에 쓴다 — 셋뿐이고,
   * 자리 잡은 수(`{{depth}}`)를 든 낱말은 `data-i18n`이 갈아입히지 못한다. */
  const depths = view.querySelector(".knowledge-depth");
  depths.hidden = !local;
  for (const step of depths.querySelectorAll("[data-knowledge-depth]")) {
    const depth = Number(step.dataset.knowledgeDepth);
    const on = depth === knowledgeFocusDepth;
    writeTextContent(step, t("knowledge.depthStep", "{{depth}}단계", { depth }));
    writeAttribute(step, "aria-checked", String(on));
    step.classList.toggle("is-active", on);
  }
  if (!local) return;
  const seat = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
  writeTextContent(crumb.querySelector(".knowledge-crumb-here"), seat >= 0 ? model.titles[seat] : "");
  crumb.querySelector(".knowledge-crumb-back").disabled = !knowledgeHasHistory(model.vault);
  let pages = 0;
  let folded = 0;
  for (let at = 0; at < model.count; at += 1) {
    if (layout.drawn?.[at] === 1 && model.kinds[at] === "page") pages += 1;
    if ((layout.folded?.[at] ?? 0) > 0) folded += 1;
  }
  say(view.querySelector(".knowledge-crumb-count"), () => (folded > 0
    ? t("knowledge.crumbCountFolded", "페이지 {{pages}} · 접힌 허브 {{folded}}", { pages, folded })
    : t("knowledge.crumbCount", "페이지 {{pages}}", { pages })));
}

/* 군집 범례(시안 이식): 그려진 점이 속한 이름 있는 군집마다 점 하나와 이름. 눌림은
 * `paintKnowledgeSpotlight`가 다른 `data-knowledge-cluster` 손잡이와 함께 칠한다. */

function paintKnowledgeClusterLegend(view, layout) {
  const host = view.querySelector(".knowledge-cluster-legend");
  const named = layout.namedCount;
  const { count, community, drawn } = layout;
  const tally = new Int32Array(named);
  for (let at = 0; at < count; at += 1) {
    if (drawn !== null && drawn[at] === 0) continue;
    const rank = community[at];
    if (rank < named) tally[rank] += 1;
  }
  const rows = [];
  for (let rank = 0; rank < named; rank += 1) {
    if (tally[rank] === 0) continue;
    let key = host.querySelector(`[data-knowledge-cluster="${rank}"]`);
    if (!key) {
      key = document.createElement("button");
      key.type = "button";
      key.className = "btn knowledge-cluster-key";
      key.dataset.knowledgeCluster = String(rank);
      key.setAttribute("aria-pressed", "false");
      const dot = document.createElement("i");
      dot.className = "knowledge-cluster-dot";
      dot.setAttribute("aria-hidden", "true");
      key.append(dot, document.createElement("span"));
    }
    writeAttribute(key, "data-hue", String(layout.communityHue[rank]));
    writeTextContent(key.lastChild, knowledgeClusterWord(layout, rank));
    rows.push(key);
  }
  reconcileElementOrder(host, rows);
  host.hidden = rows.length === 0;
}

/* 깊이를 고른다 — 캔버스 위의 줄과 카드의 손이 같은 문을 쓴다. 주변 탐색의 깊이는
 * 그려지는 반경이라(t-4140) 점의 층부터 다시 선다; 전체 지도에서는 포커스의 반경이다. */
function pickKnowledgeDepth(view, layout, depth) {
  knowledgeFocusDepth = depth;
  if (knowledgeMode === "local") {
    noteKnowledgeExplore(layout.model.vault);
    void paintKnowledgeView();
    return;
  }
  knowledgeFocus(view, layout);
  paintKnowledgeMode(view, layout);
  paintKnowledgeInspector(view, layout);
  paintKnowledgeFrame(view, layout);
}

function knowledgeCenterOn(view, layout, seat) {
  const { minX, maxX, minY, maxY } = layout.bounds;
  flyKnowledgeTo(view, layout, {
    panX: layout.x[seat] - (minX + maxX) / 2,
    panY: layout.y[seat] - (minY + maxY) / 2,
  });
}

/* 첫 fit과 티어의 fit은 즉시다 — 아직 아무도 보고 있지 않거나 판이 방금 모양을
 * 바꿨다. 단추의 fit은 난다: 사람이 보던 곳에서 전체로 **돌아가는** 길이 보여야
 * 어디서 왔는지 잃지 않는다. */
function fitKnowledgeGraph(view, layout, { fly = false } = {}) {
  /* 「전체 보기」는 카메라를 그림에 돌려주는 청이다 — 얼려 둔 폭도 함께 놓는다. */
  layout.zoomTaken = false;
  layout.fitSpan = 0;
  layout.fitTall = 0;
  if (fly) {
    flyKnowledgeTo(view, layout, { panX: 0, panY: 0, zoom: 1 });
    return;
  }
  layout.flight = null;
  layout.zoom = 1;
  layout.panX = 0;
  layout.panY = 0;
  paintKnowledgeFrame(view, layout);
}

/* 사람이 카메라를 쥔다 — 배율을 돌리거나 점을 끄는 순간. 그 순간의 그림 폭이
 * 그 뒤의 배율의 바탕이 된다(`knowledgeViewBox`). 이미 쥐고 있으면 그대로다. */
function takeKnowledgeCamera(layout) {
  layout.zoomTaken = true;
  if (layout.fitSpan > 0) return;
  layout.fitSpan = Math.max(layout.bounds.maxX - layout.bounds.minX, 1);
  layout.fitTall = Math.max(layout.bounds.maxY - layout.bounds.minY, 1);
}

/* 카메라는 난다 (3차).
 *
 * 중심으로·걷기·더블클릭·군집 이름표는 목적지로 **뛰지** 않고 토큰의 시간 동안
 * easeOutCubic으로 간다 — 뛰면 「어디서 어디로」가 사라지고, 사람은 그림 위에서
 * 제 자리를 다시 찾아야 한다. 비행은 판이 하나씩만 든다(`layout.flight`): 새
 * 비행이 옛 것을 대신하고, 사람이 판을 밀거나 바퀴를 돌리면 그 손이 비행을
 * 끊는다. 움직임을 줄이라는 판에서는 즉시 닿는다. */
function flyKnowledgeTo(view, layout, { panX, panY, zoom = layout.zoom }) {
  const ms = knowledgeMotionReduced() ? 0 : layout.tuning.flyMs;
  if (!(ms > 0)) {
    layout.flight = null;
    layout.panX = panX;
    layout.panY = panY;
    layout.zoom = zoom;
    paintKnowledgeFrame(view, layout);
    return;
  }
  const flight = {
    began: performance.now(),
    ms,
    fromPanX: layout.panX,
    fromPanY: layout.panY,
    fromZoom: layout.zoom,
    panX,
    panY,
    zoom,
  };
  layout.flight = flight;
  const step = () => {
    /* 비행은 **그 판**의 것이다. 렌즈가 판을 갈아 끼우면(새 위상, 새 `layout`)
     * 옛 판에 묶인 걸음은 여기서 멈춘다 — 실측: 「전체 보기」 뒤에 켠 「타입
     * 관계만」에서 옛 판의 걸음이 지워진 선 스물다섯을 도로 그렸다. */
    if (knowledgeLayouts.get(view) !== layout || layout.flight !== flight || !view.isConnected) {
      return;
    }
    const share = Math.min(1, (performance.now() - flight.began) / flight.ms);
    const eased = 1 - (1 - share) ** 3;
    layout.panX = flight.fromPanX + (flight.panX - flight.fromPanX) * eased;
    layout.panY = flight.fromPanY + (flight.panY - flight.fromPanY) * eased;
    layout.zoom = flight.fromZoom + (flight.zoom - flight.fromZoom) * eased;
    paintKnowledgeFrame(view, layout);
    if (share < 1) scheduleGraphFrame(knowledgeFitFrames, view, step);
    else layout.flight = null;
  };
  scheduleGraphFrame(knowledgeFitFrames, view, step);
}

function takeKnowledgeZoom(view, layout, next, at = null) {
  const before = layout.zoom;
  layout.flight = null;
  takeKnowledgeCamera(layout);
  layout.zoom = clampGraphZoom(next, layout.tuning.zoomMin, layout.tuning.zoomMax);
  if (layout.zoom === before) return;
  if (at === null) {
    paintKnowledgeFrame(view, layout);
    return;
  }
  /* 커서 아래의 점이 제자리에 남는 배율. 판 가운데를 기준으로 키우면 사람이
   * 보던 곳이 화면 밖으로 밀려나고, 그 배율은 「어디를 보고 있었는가」를 잃는다. */
  const canvas = view.querySelector(".knowledge-canvas");
  const frame = canvas.getBoundingClientRect();
  const shareX = frame.width === 0 ? 0.5 : (at.clientX - frame.left) / frame.width;
  const shareY = frame.height === 0 ? 0.5 : (at.clientY - frame.top) / frame.height;
  const box = knowledgeViewBox(view, layout);
  layout.panX += at.graphX - (box.x + shareX * box.wide);
  layout.panY += at.graphY - (box.y + shareY * box.tall);
  paintKnowledgeFrame(view, layout);
}

function knowledgeWheelZoom(view, layout, event) {
  const canvas = view.querySelector(".knowledge-canvas");
  const frame = canvas.getBoundingClientRect();
  const box = knowledgeViewBox(view, layout);
  const shareX = frame.width === 0 ? 0.5 : (event.clientX - frame.left) / frame.width;
  const shareY = frame.height === 0 ? 0.5 : (event.clientY - frame.top) / frame.height;
  takeKnowledgeZoom(view, layout, layout.zoom * graphWheelZoomFactor(event.deltaY), {
    clientX: event.clientX,
    clientY: event.clientY,
    graphX: box.x + shareX * box.wide,
    graphY: box.y + shareY * box.tall,
  });
}

async function openKnowledgePage(view, key) {
  const layout = knowledgeLayouts.get(view);
  const seat = layout?.model.keys.indexOf(key) ?? -1;
  if (seat < 0) return;
  if (layout.model.kinds[seat] === "ghost") {
    /* 조사 없이. 페이지 이름은 무엇이든 될 수 있고, 「는/은」은 그 이름의 끝
     * 음절이 정한다 — 한쪽으로 고정한 조사는 절반의 이름에서 틀린다. */
    showError(t(
      "knowledge.ghostPage",
      "«{{name}}» — 아직 없는 페이지입니다. 이 링크를 가리키는 페이지에서 만들어 주세요.",
      { name: layout.model.titles[seat] },
    ));
    return;
  }
  try {
    const path = await invoke("second_brain_page", { path: layout.model.vault, id: key });
    await openFile(path);
  } catch (error) {
    showError(String(error));
  }
}

async function requestFromKnowledge(view, key) {
  const model = knowledgeLayouts.get(view)?.model;
  const report = knowledgeReport;
  const seat = model?.keys.indexOf(key) ?? -1;
  if (seat < 0 || model.kinds[seat] === "ghost" || !wtNewScrim.hidden) return;
  const button = view.querySelector(".knowledge-inspector-request");
  if (button.disabled) return;
  button.disabled = true;
  const project = activeProjectPath;
  const tabId = activeTabId;
  const formGeneration = worktreeFormGeneration;
  try {
    // The backend resolves a vault-relative ID and rejects missing/outside files.
    const path = await invoke("second_brain_page", { path: model.vault, id: key });
    if (knowledgeSelectedKey !== key || activeTabId !== tabId || !view.isConnected ||
        formGeneration !== worktreeFormGeneration || !wtNewScrim.hidden ||
        workspaceBoardOpen || knowledgeReport !== report) return;
    openWorktreeFormForProject(project);
    wtSpec.value = t("workbench.requestBrief", "참고 지식: {{title}}\n원본 파일: {{path}}\n\n이 자료를 참고하여 진행할 작업과 완료 조건을 적어 주세요.", {
      title: model.titles[seat], path,
    });
    wtSpec.dispatchEvent(new Event("input", { bubbles: true }));
    wtSpec.focus();
  } catch (error) {
    showError(String(error));
  } finally {
    button.disabled = false;
  }
}

/* ---- 고른 점의 이야기 ---- */

/* 인스펙터는 두 상태이고, 둘 다 판이 지어질 때 이미 서 있다.
 *
 * 고쳐 쓰는 것은 **글자와 목록의 줄**이지 판 자체가 아니다(`replaceChildren` 0회).
 * 이유는 실측된 것이다 — 3a32c83c: 같은 자리를 지웠다 다시 짓는 갱신이 WebKit에서
 * 살아 있는 할당을 남겼고, 스무 분에 14.6MB/분이던 기울기가 제자리 갱신으로
 * 2.16MB/분이 되었다. 여기도 사람이 그림을 읽는 내내 몇 초에 한 번씩 지나는
 * 자리다. */
function paintKnowledgeInspector(view, layout) {
  const panel = view.querySelector(".knowledge-inspector");
  const model = layout.model;
  const seat = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
  /* 새로 고른 점은 「페이지」 탭으로 데려온다(S4) — 같은 점을 다시 그리는 판(깊이·검색)은
   * 사람이 고른 탭을 지키고, 활동 띠는 어느 탭에서든 제 도장으로 갱신된다. */
  if (seat >= 0 && layout.inspectedKey !== knowledgeSelectedKey) knowledgeInspectorTab = KNOWLEDGE_INSPECTOR_TABS[0].id;
  layout.inspectedKey = knowledgeSelectedKey;
  for (const tab of panel.querySelectorAll("[data-knowledge-tab]")) {
    writeAttribute(tab, "aria-selected", String(tab.dataset.knowledgeTab === knowledgeInspectorTab));
  }
  const activity = panel.querySelector(".knowledge-inspector-activity");
  const page = panel.querySelector(".knowledge-inspector-page");
  page.hidden = knowledgeInspectorTab !== "page";
  activity.hidden = knowledgeInspectorTab !== "activity";
  paintKnowledgeBus(layout, activity);
  activity.querySelector(".knowledge-activity-quiet").hidden = !activity.querySelector(".knowledge-overview-bus").hidden;
  const overview = panel.querySelector(".knowledge-overview");
  const card = panel.querySelector(".knowledge-card");
  overview.hidden = seat >= 0;
  card.hidden = seat < 0;
  paintKnowledgeOverview(layout, overview);
  if (seat >= 0) paintKnowledgeCard(layout, card, seat);
}

/* 볼트 개요. 그림이 그대로면 이 목록도 그대로이므로, 위상의 서명이 문지기다. */
function paintKnowledgeOverview(layout, box) {
  const model = layout.model;
  const graph = model.total;
  /* 건강 카드는 위상이 그대로여도 바뀐다(회상 하나가 그렇다) — 서명의 문 **앞**에서
   * 제 도장으로 지난다. BUS LOG도 그렇고, 그것은 「활동」 탭의 것이다(S4). */
  paintKnowledgeHealth(layout, box);
  /* 공급망의 절(P4)도 위상과 따로 바뀐다 — 조회 상태의 한 줄은 답이 오기 전에도 말한다. */
  paintKnowledgeSupplyOverview(layout, box);
  if (box.dataset.knowledgeStamp === model.signature && layout.overviewModel === model) return;
  box.dataset.knowledgeStamp = model.signature;
  layout.overviewModel = model;
  /* 유령과 고아는 백엔드의 lint 표에서 — 타일과 건강 카드가 같은 수를 보인다. */
  const lintCounts = model.lint?.counts ?? {};
  const counted = {
    pages: model.kinds.filter((kind) => kind === "page").length,
    links: model.edgeCount,
    ghosts: lintCounts.ghost_links ?? 0,
    orphans: lintCounts.orphans ?? 0,
  };
  for (const [what, value] of Object.entries(counted)) {
    const said = box.querySelector(`[data-knowledge-count="${what}"]`);
    if (said) writeTextContent(said, String(value));
  }
  /* 허브: 연결이 많은 순서. 같은 차수에서 열쇠로 가르는 것은 이 목록도 그림처럼
   * 결정적이어야 하기 때문이다 — 같은 볼트를 두 번 열어 다른 다섯이 나오면 그
   * 목록은 기억이 되지 못한다. */
  const byDegree = [...model.keys.keys()]
    .sort((left, right) => model.degree[right] - model.degree[left]
      || (model.keys[left] < model.keys[right] ? -1 : 1))
    .slice(0, KNOWLEDGE_OVERVIEW_ROWS);
  const topDegree = model.degree[byDegree[0]] || 1;
  reconcileElementOrder(
    box.querySelector(".knowledge-overview-hubs .knowledge-inspector-list"),
    byDegree.map((at) => knowledgeListRow(
      model.keys[at],
      model.titles[at],
      String(model.degree[at]),
      model.degree[at] / topDegree,
    )),
  );
  const topTag = Math.max(1, ...model.catalog.map((row) => row.count));
  reconcileElementOrder(
    box.querySelector(".knowledge-overview-tags .knowledge-inspector-list"),
    model.catalog.map((row) => {
      const held = knowledgeListRow(row.tag, row.tag, String(row.count), row.count / topTag);
      held.querySelector("button").dataset.knowledgeTag = row.tag;
      return held;
    }),
  );
  const kinds = graph?.kinds ?? [];
  const topKind = Math.max(1, ...kinds.map((row) => row.count));
  reconcileElementOrder(
    box.querySelector(".knowledge-overview-kinds .knowledge-inspector-list"),
    kinds.map((row) => {
      const held = document.createElement("li");
      held.className = "knowledge-meter";
      held.style.setProperty("--meter-fill", String(row.count / topKind));
      const name = document.createElement("span");
      name.className = "knowledge-relation-word";
      name.dataset.edgeKind = row.kind;
      name.textContent = row.kind;
      const said = document.createElement("span");
      said.className = "knowledge-inspector-note";
      said.textContent = String(row.count);
      held.append(name, said);
      return held;
    }),
  );
  /* 최근 수정은 페이지만 센다: 유령에는 파일이 없고 원본의 시각은 볼트가 아니라
   * 내려받은 날짜다. */
  const byTime = [...model.keys.keys()]
    .filter((at) => model.kinds[at] === "page")
    .sort((left, right) => model.modified[right] - model.modified[left]
      || (model.keys[left] < model.keys[right] ? -1 : 1))
    .slice(0, KNOWLEDGE_OVERVIEW_ROWS);
  reconcileElementOrder(
    box.querySelector(".knowledge-overview-recent .knowledge-inspector-list"),
    byTime.map((at, rank) => knowledgeListRow(
      model.keys[at],
      model.titles[at],
      knowledgeWhen(model.modified[at]),
      (byTime.length - rank) / byTime.length,
    )),
  );
  /* 군집(3차): 색 점·이름·페이지 수. 고유한 색을 가진 여덟까지만 — 그 뒤는 색이
   * 되풀이되어 목록이 그림을 설명하지 못한다. */
  const topSize = Math.max(1, layout.communitySize[0] ?? 0);
  const clusters = [];
  for (let rank = 0; rank < Math.min(layout.namedCount, KNOWLEDGE_HUES); rank += 1) {
    const held = knowledgeListRow(
      `cluster:${rank}`,
      knowledgeClusterWord(layout, rank),
      String(layout.communitySize[rank]),
      layout.communitySize[rank] / topSize,
    );
    held.classList.add("knowledge-cluster-row");
    held.dataset.hue = String(layout.communityHue[rank]);
    held.querySelector("button").dataset.knowledgeCluster = String(rank);
    clusters.push(held);
  }
  reconcileElementOrder(
    box.querySelector(".knowledge-overview-clusters .knowledge-inspector-list"),
    clusters,
  );
  box.querySelector(".knowledge-overview-clusters").hidden = clusters.length === 0;
}

/* 볼트 건강(3차): 카파시의 lint 넷, 그리고 라이브 층의 셋(t-2931). 줄은 서 있고
 * 수와 옷만 바뀐다. 막대는 페이지 수 대비다 — 열둘 중 둘이 고아인 볼트와 천 중
 * 둘인 볼트는 다른 그림이다. 쓰기는 값이 달라진 줄에만 닿는다. */
function paintKnowledgeHealth(layout, box) {
  const model = layout.model;
  const graph = model.total;
  /* 백엔드의 lint 표 그대로(`KNOWLEDGE_HEALTH_ROWS`) — 창은 아무것도 세지 않는다.
   * 라이브 층의 세 줄도 `knowledgeModel`이 같은 표의 열로 채워 두었다. */
  const lintCounts = model.lint?.counts ?? {};
  const lint = Object.fromEntries(KNOWLEDGE_HEALTH_ROWS
    .map((row) => [row.lint, lintCounts[row.count] ?? 0]));
  const pages = Math.max(1, graph?.pages ?? 0);
  for (const row of box.querySelectorAll(".knowledge-health-row")) {
    const value = lint[row.dataset.knowledgeLint] ?? 0;
    writeTextContent(row.querySelector(".knowledge-inspector-note"), String(value));
    row.classList.toggle("is-clean", value === 0);
    row.style.setProperty("--meter-fill", String(Math.min(1, value / pages)));
  }
}

/* BUS LOG(t-2931). 줄은 백엔드가 이미 최신순으로 세어 상한을 두었고, 여기서는
 * 그 줄들을 세울 뿐이다 — 같은 띠를 다시 짓지 않는 것은 도장(`busStamp`)이 지킨다.
 * 페이지가 그림에 서 있는 줄은 누를 수 있는 이름이고(고르면 그 점으로 간다), 서
 * 있지 않은 줄은 낱말만 남는다. */
function paintKnowledgeBus(layout, box) {
  const model = layout.model;
  const strip = box.querySelector(".knowledge-overview-bus");
  if (!strip) return;
  strip.hidden = model.live === null || model.bus.length === 0;
  if (strip.dataset.knowledgeStamp === model.busStamp) return;
  strip.dataset.knowledgeStamp = model.busStamp;
  const words = {
    created: () => t("knowledge.busCreated", "생성"),
    updated: () => t("knowledge.busUpdated", "수정"),
    linked: () => t("knowledge.busLinked", "연결"),
    recalled: () => t("knowledge.busRecalled", "회상"),
    ingested: () => t("knowledge.busIngested", "취합"),
  };
  const rows = model.bus.map((row) => {
    const line = document.createElement("li");
    line.className = "knowledge-bus-row";
    line.dataset.busKind = row.kind;
    const seat = row.page === null || row.page === undefined ? -1 : model.keys.indexOf(row.page);
    const press = document.createElement("button");
    press.type = "button";
    press.className = "knowledge-inspector-row";
    if (seat >= 0) press.dataset.knowledgeKey = model.keys[seat];
    else press.disabled = true;
    /* 이름은 그 줄이 말하는 것: 생기고·고쳐지고·회상된 줄은 페이지의 제목, 이어진
     * 줄과 취합된 줄은 「무엇 → 무엇」·일지의 말 그대로. */
    const byTitle = row.kind === "created" || row.kind === "updated" || row.kind === "recalled";
    press.textContent = byTitle && seat >= 0 ? model.titles[seat] : row.note;
    const said = document.createElement("span");
    said.className = "knowledge-inspector-note";
    said.textContent = `${(words[row.kind] ?? (() => row.kind))()} · ${knowledgeClock(row.at_ms, model.nowMs)}`;
    line.append(press, said);
    /* 회상된 줄은 누가 봤는가까지 적혀 있다(note = 판). 그 판이 지금 열려 있으면
     * 작업 상황판의 그 카드로 가는 문이다 — 그래프에서 작업으로 가는 유일한 증명된 간선. */
    if (row.kind === "recalled") {
      const seat = knowledgeSeatNode(row.note);
      if (seat) line.append(seat);
    }
    return line;
  });
  reconcileElementOrder(strip.querySelector(".knowledge-inspector-list"), rows);
}

/* 띠의 시각: 하루 안이면 시:분, 그 밖이면 날짜 — 로케일이 정하는 대로. */
function knowledgeClock(ms, nowMs) {
  if (!ms) return "—";
  const when = new Date(ms);
  if (Math.abs(nowMs - ms) < 86_400_000) {
    return when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  return when.toLocaleDateString();
}

/* 수정 시각. 로케일이 정하는 대로 — 이 창의 다른 날짜들과 같은 손이다. */
function knowledgeWhen(ms) {
  if (!ms) return "—";
  return new Date(ms).toLocaleDateString();
}

/* 고른 페이지의 카드. */
function paintKnowledgeCard(layout, box, seat) {
  const model = layout.model;
  const ghost = model.kinds[seat] === "ghost";
  /* 공급망의 점(P4)은 페이지가 아니다 — 경로 줄은 구성요소의 purl이고(취약점은 제목이 곧 id라 서지
   * 않는다), 열 파일·적을 frontmatter·백링크가 없다. 그 사실들은 공급망 절이 든다. */
  const supplied = knowledgeSupplyKind(model.kinds[seat]);
  const supplyRow = supplied ? model.supplyRow?.[seat] ?? -1 : -1;
  writeTextContent(box.querySelector(".knowledge-inspector-title"), model.titles[seat]);
  const where = box.querySelector(".knowledge-inspector-path");
  where.hidden = supplied && model.kinds[seat] === "vulnerability";
  if (ghost) say(where, () => t("knowledge.ghostNode", "아직 없는 페이지"));
  else {
    say(where, null);
    writeTextContent(where, supplyRow >= 0 && model.kinds[seat] === "component"
      ? model.supply.answer.components[supplyRow].purl
      : model.keys[seat]);
  }
  writeAttribute(where, "title", where.textContent);
  const folder = box.querySelector(".knowledge-inspector-folder");
  folder.hidden = model.folders[seat] === "";
  if (!folder.hidden) {
    say(folder, () => t("knowledge.folder", "폴더 {{folder}}", { folder: model.folders[seat] }));
  }
  const tags = box.querySelector(".knowledge-inspector-tags");
  tags.hidden = model.tags[seat].length === 0;
  if (!tags.hidden) writeTextContent(tags, model.tags[seat].join(" · "));
  /* 군집(3차). 이름 없는 군집(작은 것)의 페이지는 줄이 서지 않는다. */
  const cluster = box.querySelector(".knowledge-inspector-cluster");
  const rank = layout.community[seat];
  cluster.hidden = rank >= layout.namedCount;
  if (!cluster.hidden) {
    const press = cluster.querySelector(".knowledge-cluster-press");
    const name = knowledgeClusterWord(layout, rank);
    writeTextContent(press, name);
    writeAttribute(press, "data-knowledge-cluster", String(rank));
    writeAttribute(press, "data-hue", String(layout.communityHue[rank]));
    writeAttribute(
      press,
      "aria-label",
      t("knowledge.spotlightCluster", "«{{name}}» 군집만 밝히기", { name }),
    );
    const on = knowledgeClusterPicked === rank;
    press.classList.toggle("is-active", on);
    writeAttribute(press, "aria-pressed", String(on));
  }
  const excerpt = box.querySelector(".knowledge-inspector-excerpt");
  excerpt.hidden = model.excerpts[seat] === "";
  if (!excerpt.hidden) writeTextContent(excerpt, model.excerpts[seat]);

  /* 관계는 방향이 뜻의 전부다(§2). 그래서 두 열이고, 각 줄은 이웃의 이름과 그
   * 관계의 낱말이다 — 「A가 B를 supersedes」와 「B가 A를 supersedes」는 반대의
   * 사실이고, 한 열에 섞으면 그 둘이 같은 줄로 읽힌다. */
  const outward = [];
  const inward = [];
  for (let at = model.start[seat]; at < model.start[seat + 1]; at += 1) {
    const edge = model.throughEdge[at];
    const row = { other: model.neighbour[at], kind: model.kind[edge] };
    (model.from[edge] === seat ? outward : inward).push(row);
  }
  const order = (left, right) => left.kind - right.kind
    || (model.keys[left.other] < model.keys[right.other] ? -1 : 1);
  /* 줄마다 연결의 이유(S4): 관계의 선 모양이 표식으로(범례의 선과 같은 옷,
   * `knowledge-legend-edge kind-*`), 방향이 화살표로, 관계 낱말이 글로 — 색만이 뜻을
   * 나르지 않는다. 낱말 칸(`note`)은 관계 낱말 그대로다: 그 칸을 읽는 손이 여럿이다. */
  const rows = (held, arrow) => held.sort(order).map((row) => {
    const line = knowledgeListRow(model.keys[row.other], model.titles[row.other], knowledgeEdgeWord(row.kind));
    line.classList.add("knowledge-relation-row");
    /* 키 뒤에 그 키의 뜻 — 「왜 연결됐는지」에 답하는 것은 이 한 마디다. 방향은
     * 절의 머리말(나가는/들어오는)과 화살표가 이미 말했다. */
    const why = document.createElement("span");
    why.className = "knowledge-relation-why";
    why.textContent = knowledgeEdgeWhy(KNOWLEDGE_EDGE_KINDS[row.kind]);
    line.querySelector(".knowledge-inspector-note").appendChild(why);
    const mark = document.createElement("span");
    mark.className = `knowledge-relation-mark knowledge-legend-edge kind-${KNOWLEDGE_EDGE_KINDS[row.kind]}`;
    mark.setAttribute("aria-hidden", "true");
    const dir = document.createElement("span");
    dir.className = "knowledge-relation-dir";
    dir.setAttribute("aria-hidden", "true");
    dir.textContent = arrow;
    line.prepend(mark, dir);
    /* 접힌 허브의 줄(t-4140): 관계 낱말 뒤에 생략한 연결의 묶음, 그리고 펼치는 단추.
     * 펼치기는 이 단추뿐이다 — 그림의 점을 누르는 것은 중심을 옮기는 일이다. */
    const fold = layout.folded === null ? 0 : layout.folded[row.other];
    if (fold > 0) {
      /* 전체 지도의 「+N」은 공급망 멤버의 접힌 구성요소이고(P4) 펼치는 문도 그것의 것이다; 주변
       * 탐색의 「+N」은 접은 허브의 생략한 연결이다. 한 판에 두 뜻이 섞이지 않는다(`knowledgeSubgraph`). */
      const supplyFold = knowledgeMode !== "local";
      const note = line.querySelector(".knowledge-inspector-note");
      note.textContent = `${knowledgeEdgeWord(row.kind)} · ${supplyFold
        ? knowledgeSupplyFoldWords(fold) : knowledgeFoldWords(layout, row.other)}`;
      const unfold = document.createElement("button");
      unfold.type = "button";
      unfold.className = "btn knowledge-unfold";
      if (supplyFold) unfold.dataset.knowledgeSupplyUnfold = model.keys[row.other];
      else unfold.dataset.knowledgeUnfold = model.keys[row.other];
      unfold.textContent = t("knowledge.unfold", "펼치기");
      unfold.setAttribute("aria-label", supplyFold
        ? t("knowledge.supplyUnfoldOf", "«{{name}}» 아래 접힌 구성요소 {{count}}개 펼치기",
          { name: model.titles[row.other], count: fold })
        : t(
          "knowledge.unfoldHub",
          "«{{name}}»의 접힌 연결 {{count}}개 펼치기",
          { name: model.titles[row.other], count: fold },
        ));
      line.appendChild(unfold);
    }
    return line;
  });
  const out = box.querySelector(".knowledge-relations-outgoing");
  out.hidden = outward.length === 0;
  const into = box.querySelector(".knowledge-relations-incoming");
  into.hidden = inward.length === 0;
  /* 같은 선택을 다시 그리는 일은 흔하다(깊이를 바꾸는 몸짓마다 지난다) — 그때
   * 두 열은 한 글자도 달라지지 않으므로 짓지도 않는다. 그려진 부분집합의 도장도
   * 든다: 접힌 허브의 줄은 펼치는 순간 달라진다. */
  const stamp = `${model.signature}|${model.keys[seat]}|${layout.drawnStamp}`;
  if (box.dataset.knowledgeStamp !== stamp || layout.cardModel !== model) {
    box.dataset.knowledgeStamp = stamp;
    layout.cardModel = model;
    reconcileElementOrder(out.querySelector(".knowledge-inspector-list"), rows(outward, "→"));
    reconcileElementOrder(into.querySelector(".knowledge-inspector-list"), rows(inward, "←"));
  }

  /* 백링크는 「이 페이지를 가리키는 것이 몇인가」이고, 그중 이름을 가진 관계가
   * 몇인가는 따로 센다 — 스무 개가 가리키는 페이지와 스무 개가 `supersedes`하는
   * 페이지는 같은 수의 다른 사실이다. */
  const backlinks = box.querySelector(".knowledge-backlinks");
  backlinks.hidden = supplied;
  say(backlinks, () => t(
    "knowledge.backlinks",
    "백링크 {{count}} · 타입 관계 {{typed}}",
    { count: inward.length, typed: inward.filter((row) => row.kind !== 0).length },
  ));
  /* 이 페이지를 본 작업(회상 추적, 최근 BUS 안에서): 판마다 한 줄, 최신 먼저.
   * 열린 판은 작업 상황판의 카드로 가는 문이고, 닫힌 판은 이름만 남는다. */
  const seen = box.querySelector(".knowledge-recalled-by");
  const pageKey = model.keys[seat];
  const byNote = new Map();
  for (const row of model.bus) {
    if (row.kind !== "recalled" || row.page !== pageKey || byNote.has(row.note)) continue;
    byNote.set(row.note, row.at_ms);
  }
  seen.hidden = byNote.size === 0;
  const seenStamp = `${model.busStamp}|${pageKey}`;
  if (!seen.hidden && seen.dataset.knowledgeStamp !== seenStamp) {
    seen.dataset.knowledgeStamp = seenStamp;
    const seatRows = [...byNote].sort((a, b) => b[1] - a[1]).map(([note, at]) => {
      const row = document.createElement("li");
      let door = knowledgeSeatNode(note);
      if (door === null) {
        door = document.createElement("span");
        door.className = "knowledge-inspector-row";
        door.textContent = note;
      }
      const said = document.createElement("span");
      said.className = "knowledge-inspector-note";
      said.textContent = knowledgeClock(at, model.nowMs);
      row.append(door, said);
      return row;
    });
    reconcileElementOrder(seen.querySelector(".knowledge-inspector-list"), seatRows);
  }
  const when = box.querySelector(".knowledge-modified");
  when.hidden = ghost || !model.modified[seat];
  if (!when.hidden) {
    say(when, () => t("knowledge.modified", "수정 {{when}}", {
      when: knowledgeWhen(model.modified[seat]),
    }));
  }

  const chainSection = box.querySelector(".knowledge-inspector-chain");
  if (chainSection) {
    if (layout.pathChain !== null) {
      chainSection.hidden = false;
      const chainBody = chainSection.querySelector(".knowledge-chain-body");
      chainBody.innerHTML = "";
      const { nodes, edges } = layout.pathChain;
      for (let i = 0; i < nodes.length; i += 1) {
        if (i > 0) {
          const arrow1 = document.createElement("span");
          arrow1.className = "knowledge-chain-arrow";
          arrow1.textContent = " → ";
          const relWord = document.createElement("span");
          relWord.className = "knowledge-chain-rel";
          const edgeKind = model.kind[edges[i - 1]];
          relWord.textContent = KNOWLEDGE_EDGE_KINDS[edgeKind] ?? "mentions";
          const arrow2 = document.createElement("span");
          arrow2.className = "knowledge-chain-arrow";
          arrow2.textContent = " → ";
          chainBody.append(arrow1, relWord, arrow2);
        }
        const nodeSpan = document.createElement("span");
        nodeSpan.className = "knowledge-chain-node";
        nodeSpan.textContent = model.titles[nodes[i]] || model.keys[nodes[i]];
        chainBody.appendChild(nodeSpan);
      }
    } else {
      chainSection.hidden = true;
    }
  }
  /* 유령에는 열 파일이 없다 — 문 대신 「이 링크를 가리키는 페이지」 목록이 답이다. 적을
   * frontmatter도 없으니 「연결 추가」도 서지 않는다; 원본도 마찬가지다. */
  const editable = model.kinds[seat] === "page";
  box.querySelector(".knowledge-inspector-open").hidden = ghost || supplied;
  box.querySelector(".knowledge-inspector-request").hidden = ghost || supplied;
  box.querySelector(".knowledge-inspector-link").hidden = !editable;
  paintKnowledgeSupplyCard(layout, box, seat);
  /* 다른 점을 고르면 서식은 닫힌다 — 서식의 출발은 고른 페이지다. */
  if (layout.linkFormFor !== knowledgeSelectedKey) {
    layout.linkFormFor = knowledgeSelectedKey;
    closeKnowledgeLinkForm(layout.view);
  }
  paintKnowledgeLinkForm(layout.view, layout);
}

/* ---- 머리의 낱말과 칩 ---- */

/* 통계 줄은 같은 사실을 두 벌로 든다.
 *
 * 좁은 판에서 다섯 개의 수를 한 줄에 밀어 넣으면 그 줄은 넘치거나 잘리고, 둘 다
 * 「읽을 수 없다」의 다른 이름이다. 그래서 긴 벌과 짧은 벌이 **함께** 서 있고
 * 어느 쪽이 보이는가는 티어가 정한다 — 지우고 다시 짓지 않는 것은 이 줄이
 * `aria-live`이기 때문이다: 판을 좁혔다는 이유로 낭독기가 문장을 다시 읽으면
 * 그것은 새 소식이 아니라 잡음이다. */
function paintKnowledgeStat(view, layout, matches) {
  const model = layout.model;
  const stat = view.querySelector(".knowledge-stat");
  const full = view.querySelector(".knowledge-stat-full");
  const compact = view.querySelector(".knowledge-stat-compact");
  const graph = model.total;
  if (knowledgeLoading || graph === null) {
    const word = () => (knowledgeLoading ? t("knowledge.reading", "읽는 중…") : t("knowledge.noVault", "볼트를 고르세요"));
    say(full, word);
    say(compact, word);
    return;
  }
  /* 그림에 선 것과 볼트가 가진 것 둘 다. 걸러 놓고 「페이지 12」만 말하면
   * 사람은 나머지가 사라진 줄 안다. 세는 것은 **페이지**다: 유령과 원본을
   * 함께 세면 「페이지 15/12」라고 적히고, 그 분수는 아무 뜻이 없다. */
  let shown = 0;
  for (let at = 0; at < model.count; at += 1) {
    if (model.kinds[at] === "page" && (layout.drawn === null || layout.drawn[at] === 1)) shown += 1;
  }
  /* 검색은 멤버십을 바꾸지 않으므로 그림의 수로는 몇이 맞았는지 알 수 없다 —
   * 그 수가 사는 자리가 여기다. */
  const searching = knowledgeQuery.trim() !== "";
  const found = () => (searching
    ? ` · ${t("knowledge.statMatches", "매치 {{matches}}", { matches })}`
    : "");
  const fullWord = t(
    "knowledge.stat",
    "페이지 {{shown}}/{{pages}} · 링크 {{edges}} · 군집 {{clusters}} · 유령 {{ghosts}} · 고아 {{orphans}} · {{ms}}ms",
    {
      shown,
      pages: graph.pages,
      edges: model.edgeCount,
      clusters: layout.namedCount,
      ghosts: graph.ghosts,
      orphans: graph.orphans,
      ms: knowledgeReport?.scanned_ms ?? 0,
    },
  ) + knowledgeSupplyStatWord(model) + found();
  const compactWord = t(
    "knowledge.statCompact",
    "페이지 {{shown}} · 링크 {{edges}}",
    { shown, edges: model.edgeCount },
  ) + found();
  say(full, () => fullWord);
  say(compact, () => compactWord);
  stat.dataset.tip = fullWord;
}

function paintKnowledgeHead(view, layout, matches) {
  const model = layout.model;
  paintKnowledgeStat(view, layout, matches);
  for (const button of view.querySelectorAll("[data-knowledge-flag]")) {
    const flag = button.dataset.knowledgeFlag;
    const on = flag === "orphans" ? knowledgeOrphansOnly
      : flag === "ghosts" ? knowledgeGhostsOnly
        : flag === "typed" ? knowledgeTypedOnly
          : flag === "alive" ? knowledgeAliveOnly
            : flag === "merge" ? knowledgeMergeOnly
              : flag === "nav" ? knowledgeNavShown
                : flag === "supply" ? knowledgeSupplyShown
                  : knowledgeShowSources;
    writeAttribute(button, "aria-pressed", String(on));
    button.classList.toggle("is-active", on);
    /* 타입 관계가 한 줄도 없는 볼트에서 이 칩은 아무것도 고르지 못한다 — 눌러
     * 놓고 빈 그림을 보여 주는 대신 서지 않는다. 라이브 층의 둘도 같은 규칙:
     * 표가 없는 답에는 활동이 없고, 후보가 없는 볼트에는 물음이 없다. */
    if (flag === "typed") button.hidden = !model.typed;
    if (flag === "alive") button.hidden = model.live === null;
    if (flag === "merge") button.hidden = !model.mergeable;
  }
  /* 공급망의 낱말들(P4) — `sev:` 칩과 범례의 줄은 렌즈가 켜졌을 때만 선다. */
  for (const held of view.querySelectorAll('.knowledge-token-chip[data-token="sev:"], .knowledge-legend [data-legend-supply]')) {
    held.hidden = !knowledgeSupplyShown;
  }
  const tags = view.querySelector(".knowledge-tags");
  const paintChips = (host, rows) => {
    const wanted = [];
    for (const row of rows) {
      const held = [...tags.querySelectorAll(".knowledge-chip")]
        .find((one) => one.dataset.knowledgeTag === row.tag);
      const chip = held ?? document.createElement("button");
      if (!held) {
        chip.type = "button";
        chip.dataset.knowledgeTag = row.tag;
      }
      /* 칩의 점은 그 태그가 가장 많이 사는 군집의 색이다(3차) — 군집이 바뀌면
       * 칩도 따라 바뀌므로 판마다 다시 쓴다(같으면 쓰지 않는다). */
      const hue = layout.tagHue.get(row.tag);
      if (hue === undefined) chip.removeAttribute("data-hue");
      else writeAttribute(chip, "data-hue", String(hue));
      writeClassName(chip, "knowledge-chip");
      writeTextContent(chip, `${row.tag} · ${row.count}`);
      /* 칩이 켜졌다는 것은 그 태그가 지금의 검색어라는 뜻이다 — 칩과 검색 상자가
       * 같은 것을 말하지 않으면, 지우는 손이 둘이 되고 둘 중 하나는 잊힌다. */
      const picked = knowledgeTagsPicked.has(row.tag);
      writeAttribute(chip, "aria-pressed", String(picked));
      chip.classList.toggle("is-active", picked);
      wanted.push(chip);
    }
    reconcileElementOrder(host, wanted);
  };
  const inline = model.catalog.slice(0, KNOWLEDGE_TAGS_INLINE);
  const overflow = model.catalog.slice(KNOWLEDGE_TAGS_INLINE);
  paintChips(view.querySelector(".knowledge-chips"), inline);
  paintChips(view.querySelector(".knowledge-tags-popover"), overflow);
  const more = view.querySelector(".knowledge-tags-more-toggle");
  more.hidden = overflow.length === 0;
  writeTextContent(more, `+${overflow.length}`);
  const moreWord = t("knowledge.moreTags", "태그 {{count}}개 더 보기", { count: overflow.length });
  writeAttribute(more, "aria-label", moreWord);
  more.dataset.tip = moreWord;
  if (more.hidden) {
    view.querySelector(".knowledge-tags-popover").classList.remove("is-open");
    writeAttribute(more, "aria-expanded", "false");
  }
}

/* 빈 볼트에 서는 한 문장과 한 단추.
 *
 * 단추가 누르는 것은 이 볼트의 「raw 취합」 빠른 명령이다 — 세팅이 만들어 둔
 * 바로 그것이라, 여기에 두 번째 프롬프트를 적지 않는다. 에이전트 없이 저장된
 * 명령(터미널에 글자를 넣는 종류)은 판 하나를 필요로 하므로, 그때는 raw 폴더를
 * 여는 단추가 대신 선다. */
function paintKnowledgeEmpty(view, model) {
  const empty = view.querySelector(".knowledge-empty");
  const drawnNothing = model.count === 0;
  empty.hidden = !drawnNothing || knowledgeLoading || knowledgeError !== null;
  view.querySelector(".knowledge-canvas").classList.toggle("is-empty", drawnNothing);
  if (empty.hidden) return;
  const said = empty.querySelector(".knowledge-empty-word");
  const filtered = (knowledgeReport?.graph?.pages ?? 0) > 0;
  say(said, () => filtered
    ? t("knowledge.emptyFilter", "이 조건에 맞는 페이지가 없습니다. 검색어와 필터를 지워 보세요.")
    : t(
      "knowledge.emptyWord",
      "아직 지식 페이지가 없습니다. raw/에 기사나 메모를 던져 두고 에이전트에게 «취합해»라고 말하면 여기에 그림이 자랍니다.",
    ));
  const act = empty.querySelector(".knowledge-empty-act");
  if (filtered) {
    say(act, () => t("knowledge.clearFilters", "필터 지우기"));
    act.onclick = () => {
      knowledgeQuery = "";
      knowledgeOrphansOnly = false;
      knowledgeGhostsOnly = false;
      knowledgeTypedOnly = false;
      knowledgeLintLens = null;
      knowledgeAliveOnly = false;
      knowledgeColdOnly = false;
      knowledgeMergeOnly = false;
      knowledgeTagsPicked.clear();
      view.querySelector(".knowledge-query").value = "";
      void paintKnowledgeView();
    };
    return;
  }
  if (knowledgeIngest?.agent) {
    say(act, () => knowledgeIngest.label);
    act.onclick = () => {
      runQuickCommand(knowledgeIngest);
      /* 취합이 끝나면 그림이 자란다 — 그 끝을 기다리는 것은 `hook:agent`이고,
       * 이 판이 보이는 동안에만 다시 읽는다. */
    };
    return;
  }
  say(act, () => t("knowledge.openRaw", "raw 폴더 열기"));
  act.onclick = () => {
    const vault = knowledgeReport?.vault ?? secondBrainVault;
    if (vault) invoke("fs_reveal", { path: `${vault}/raw` }).catch(showError);
  };
}

/* ---- 손 매기 ---- */

function wireKnowledgeView(view) {
  paintWorkbenchNavigation(view, "knowledge");
  if (wiredKnowledgeViews.has(view)) return;
  wiredKnowledgeViews.add(view);
  // A clone can inherit a pending source button, but not its pending request.
  view.querySelector(".knowledge-inspector-request").disabled = false;
  view.dataset.knowledgeWired = "yes";
  const canvas = view.querySelector(".knowledge-canvas");

  /* 미는 몸짓은 에이전트 그래프와 같은 손이다 — 무엇이 움직이는지만 다르다:
   * 저쪽은 스크롤이고 이쪽은 viewBox의 가운데다. */
  let fromPanX = 0;
  let fromPanY = 0;
  wireGraphDrag(canvas, {
    onGrab: () => {
      const layout = knowledgeLayouts.get(view);
      fromPanX = layout?.panX ?? 0;
      fromPanY = layout?.panY ?? 0;
      // 사람의 손이 카메라를 쥐면 비행은 끝난다.
      if (layout) layout.flight = null;
    },
    onDrag: (moveX, moveY) => {
      const layout = knowledgeLayouts.get(view);
      if (!layout) return;
      canvas.classList.add("is-panning");
      layout.panX = fromPanX - moveX / layout.scale;
      layout.panY = fromPanY - moveY / layout.scale;
      paintKnowledgeFrame(view, layout, { cameraOnly: true });
    },
    onEnd: () => canvas.classList.remove("is-panning"),
  });
  /* 맨 바퀴가 배율이다 — 에이전트 그래프와 다른 판단이고, 다른 이유가 있다:
   * 저 판은 세로로 이어지는 목록이라 바퀴에 선약이 있고, 이 판에는 스크롤이
   * 아예 없다(보이는 곳은 viewBox가 정한다). */
  canvas.onwheel = (event) => {
    const layout = knowledgeLayouts.get(view);
    if (!layout) return;
    event.preventDefault();
    knowledgeWheelZoom(view, layout, event);
  };
  /* 포인터 아래의 점 — 그린 손에게 묻는다.
   *
   * SVG에서는 브라우저의 히트테스트가 답을 알고 있다(점마다 요소가 있으므로) —
   * 그 답이 언제나 먼저다. GL에는 요소가 없으니 페인터가 투영한 자리에서 고른다
   * (`knowledgePickSeat`, 두 손이 나눠 쓰는 한 벌). 좌표는 캔버스의 왼쪽 위에서 잰
   * 판 픽셀이고, 그것이 이름표 격자와 투영기가 쓰는 자와 같은 자다. */
  const knowledgeUnder = (event) => {
    /* 점의 요소(SVG) 또는 점의 이름(GL 오버레이) — 둘 다 제 점의 열쇠를 든다. */
    const held = event.target.closest(".knowledge-node, .knowledge-gl-label");
    if (held) return held.dataset.graphKey ?? null;
    const layout = knowledgeLayouts.get(view);
    if (!layout || knowledgePainterFor(view).id === "svg") return null;
    const box = canvas.getBoundingClientRect();
    const seat = knowledgePainterFor(view).pick(event.clientX - box.left, event.clientY - box.top);
    return seat < 0 ? null : layout.model.keys[seat];
  };
  canvas.onpointermove = (event) => {
    const layout = knowledgeLayouts.get(view);
    // 점을 끌고 있는 동안 지나치는 점들은 짚은 것이 아니다.
    if (!layout || layout.pinned >= 0) return;
    const key = knowledgeUnder(event);
    if (key === knowledgeHoverKey) return;
    knowledgeHoverKey = key;
    litKnowledge(view, layout, key);
  };
  canvas.onpointerleave = () => {
    const layout = knowledgeLayouts.get(view);
    if (!layout || knowledgeHoverKey === null) return;
    knowledgeHoverKey = null;
    litKnowledge(view, layout, null);
  };
  /* 누르는 것은 **고르는** 일이다. 문을 여는 것은 인스펙터의 단추이거나 Enter다 —
   * 지도 위의 한 점을 짚었다는 이유로 편집기 탭이 열리면, 그림을 읽는 동안 사람은
   * 제가 열지 않은 파일 열두 개를 얻는다. 빈 곳을 누르는 것은 고르기를 놓는 일이다. */
  canvas.onclick = (event) => {
    const layout = knowledgeLayouts.get(view);
    /* 끌고 놓은 손은 고르는 손이 아니다(3차) — 그 몸짓의 click은 삼킨다. */
    if (layout?.dragged) {
      layout.dragged = false;
      return;
    }
    const label = event.target.closest(".knowledge-cluster-label");
    if (label) {
      toggleKnowledgeCluster(view, Number(label.dataset.community));
      return;
    }
    const key = knowledgeUnder(event);
    if (event.shiftKey && knowledgeSelectedKey !== null && key !== null && key !== knowledgeSelectedKey) {
      runKnowledgeShortestPath(view, layout, knowledgeSelectedKey, key);
      return;
    }
    /* 주변 탐색의 빈 곳은 아무것도 아니다(t-4140) — 중심을 놓는 손은 Esc와 토글이다. */
    if (key === null && knowledgeMode === "local") return;
    clearKnowledgePath(view, layout);
    selectKnowledgeNode(view, key);
  };
  canvas.ondblclick = (event) => {
    const layout = knowledgeLayouts.get(view);
    const key = layout === undefined ? null : knowledgeUnder(event);
    if (key === null || !layout) return;
    event.preventDefault();
    /* 한 비행으로 중심과 배율을 함께. 배율을 쥔 것이므로 판은 저절로 앉지 않는다. */
    const seat = layout.model.keys.indexOf(key);
    if (seat < 0) return;
    const { minX, maxX, minY, maxY } = layout.bounds;
    takeKnowledgeCamera(layout);
    flyKnowledgeTo(view, layout, {
      panX: layout.x[seat] - (minX + maxX) / 2,
      panY: layout.y[seat] - (minY + maxY) / 2,
      zoom: clampGraphZoom(
        layout.zoom + layout.tuning.zoomStep * 2,
        layout.tuning.zoomMin,
        layout.tuning.zoomMax,
      ),
    });
  };
  canvas.onkeydown = (event) => {
    const layout = knowledgeLayouts.get(view);
    /* ⌘/Ctrl+Z: 마지막에 적은 관계를 같은 문으로 되돌린다(S5). 되돌릴 것이 없으면 창의 것. */
    if (layout && (event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "z"
        && knowledgeLastRelate !== null) {
      event.preventDefault();
      event.stopPropagation();
      void relateKnowledge(view, { ...knowledgeLastRelate, remove: true });
      return;
    }
    if (!layout || event.metaKey || event.ctrlKey || event.altKey) return;
    /* 주제의 이름판은 키보드로도 눌린다(09-16) — 그것이 「이 주제를 펼쳐라」의
     * 단추이므로, 마우스에만 달린 문은 문이 아니다. */
    const plate = event.target.closest?.(".knowledge-cluster-label");
    if (plate && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      toggleKnowledgeCluster(view, Number(plate.dataset.community));
      return;
    }
    if (event.key === "Enter" && knowledgeSelectedKey !== null) {
      event.preventDefault();
      void openKnowledgePage(view, knowledgeSelectedKey);
      return;
    }
    /* Esc는 고르기를 놓는다 — 그리고 **여기서 멈춘다**.
     *
     * 창의 Esc 사다리는 마지막 칸에서 「읽고 있는 판」을 닫는다(`closeTab`), 그리고
     * 그 칸은 `defaultPrevented`를 묻지 않는다. 막기만 하고 흘려보내면 포커스를
     * 푸는 한 번의 Esc가 탭까지 함께 닫는다 — 실측: 이 한 줄이 없을 때 하네스의
     * 다음 라운드가 「보이는 지식 판이 없다」로 죽었다. 고른 것이 없을 때는 붙잡지
     * 않는다: 그때의 Esc는 정말로 판을 닫으라는 말이고, 그렇게 두 칸의 사다리가
     * 된다. */
    if (event.key === "Escape") {
      if (knowledgePath !== null) {
        event.preventDefault();
        event.stopPropagation();
        clearKnowledgePath(view, layout);
        knowledgeFocus(view, layout);
        paintKnowledgeInspector(view, layout);
        paintKnowledgeFrame(view, layout);
        return;
      }
      /* 주변 탐색의 Esc는 전체 지도로 나가는 걸음이다(t-4140) — 고른 점은 남아, 다음
       * Esc가 그것을 놓고 그다음이 판을 닫는다: 세 칸의 사다리. */
      if (knowledgeMode === "local") {
        event.preventDefault();
        event.stopPropagation();
        setKnowledgeMode(view, "global");
        return;
      }
      if (knowledgeSelectedKey === null) return;
      event.preventDefault();
      event.stopPropagation();
      selectKnowledgeNode(view, null);
      return;
    }
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
      return;
    }
    event.preventDefault();
    knowledgeWalk(view, layout, event.key);
  };

  view.querySelector(".knowledge-zoom-out").onclick = () => {
    const layout = knowledgeLayouts.get(view);
    if (layout) takeKnowledgeZoom(view, layout, layout.zoom - layout.tuning.zoomStep);
  };
  view.querySelector(".knowledge-zoom-in").onclick = () => {
    const layout = knowledgeLayouts.get(view);
    if (layout) takeKnowledgeZoom(view, layout, layout.zoom + layout.tuning.zoomStep);
  };
  view.querySelector(".knowledge-zoom-fit").onclick = () => {
    const layout = knowledgeLayouts.get(view);
    if (layout) fitKnowledgeGraph(view, layout, { fly: true });
  };
  /* 모드 토글과 빵부스러기(t-4140). */
  for (const step of view.querySelectorAll(".knowledge-mode-step")) {
    step.onclick = () => setKnowledgeMode(view, step.dataset.knowledgeMode);
  }
  view.querySelector(".knowledge-crumb-back").onclick = () => knowledgeBack(view);
  view.querySelector(".knowledge-crumb-root").onclick = () => setKnowledgeMode(view, KNOWLEDGE_MODES[0].id);
  view.querySelector(".knowledge-depth").onclick = (event) => {
    const step = event.target.closest("[data-knowledge-depth]");
    const layout = knowledgeLayouts.get(view);
    if (step && layout) pickKnowledgeDepth(view, layout, Number(step.dataset.knowledgeDepth));
  };
  view.querySelector(".knowledge-cluster-legend").onclick = (event) => {
    const key = event.target.closest("[data-knowledge-cluster]");
    if (key) toggleKnowledgeCluster(view, Number(key.dataset.knowledgeCluster));
  };
  /* 팝오버 둘은 판이 지어질 때 한 번 만들어지고(§6), 여는 몸짓이 하는 일은 클래스
   * 하나와 `aria-expanded` 하나뿐이다 — 열 때마다 목록을 새로 짓는 팝오버는 열 때마다
   * 제 안의 상태를 잃는다. */
  const popover = (toggleSelector, panelSelector) => {
    const toggle = view.querySelector(toggleSelector);
    const panel = view.querySelector(panelSelector);
    toggle.onclick = () => {
      const open = !panel.classList.contains("is-open");
      panel.classList.toggle("is-open", open);
      writeAttribute(toggle, "aria-expanded", String(open));
    };
  };
  popover(".knowledge-lens-toggle", ".knowledge-lens-popover");
  popover(".knowledge-tags-more-toggle", ".knowledge-tags-popover");
  popover(".knowledge-legend-toggle", ".knowledge-legend");
  popover(".knowledge-slicer-toggle", ".knowledge-slicer-popover");
  popover(".knowledge-scene-toggle", ".knowledge-scene-popover");

  const sceneEl = view.querySelector(".knowledge-scene");
  const sceneToggle = sceneEl?.querySelector(".knowledge-scene-toggle");
  const scenePopover = sceneEl?.querySelector(".knowledge-scene-popover");
  const sceneInput = scenePopover?.querySelector(".knowledge-scene-input");
  const sceneSave = scenePopover?.querySelector(".knowledge-scene-save");

  if (sceneToggle) {
    const origSceneClick = sceneToggle.onclick;
    sceneToggle.onclick = () => {
      if (origSceneClick) origSceneClick();
      if (scenePopover?.classList.contains("is-open")) {
        paintKnowledgeSceneList(view);
      }
    };
  }

  if (sceneInput && sceneSave) {
    sceneInput.onkeydown = (e) => {
      if (e.key === "Enter") sceneSave.click();
    };
  }

  if (sceneSave) {
    sceneSave.onclick = async () => {
      const name = sceneInput?.value.trim();
      if (!name) return;
      const layout = knowledgeLayouts.get(view);
      const scene = captureCurrentKnowledgeScene(name, view, layout);
      const vault = knowledgeReport?.vault ?? secondBrainVault;
      const store = getVaultSceneStore(vault);
      const idx = store.scenes.findIndex((s) => s.name === name);
      if (idx >= 0) {
        store.scenes[idx] = scene;
      } else {
        store.scenes.push(scene);
      }
      await saveVaultSceneStore(vault, store);
      if (sceneInput) sceneInput.value = "";
      paintKnowledgeSceneList(view);
    };
  }

  const slicerEl = view.querySelector(".knowledge-slicer");
  const slicerToggleEl = slicerEl?.querySelector(".knowledge-slicer-toggle");
  const slicerLabelEl = slicerEl?.querySelector(".knowledge-slicer-label");
  const slicerValEl = slicerEl?.querySelector(".knowledge-slicer-value");
  const sliderEl = slicerEl?.querySelector(".knowledge-slicer-slider");

  /* 그림의 시간 폭 — 슬라이더의 0~100이 걸리는 자. 시각이 없는 점(유령)은 세지 않고,
   * 폭이 없으면 벽시계의 처음부터 지금까지다. */
  const sliceRange = (layout) => {
    let minT = Number.POSITIVE_INFINITY;
    let maxT = Number.NEGATIVE_INFINITY;
    for (let at = 0; at < layout.count; at += 1) {
      const time = Math.max(layout.model.modified[at] || 0, layout.model.recalledAt[at] || 0);
      if (time > 0) {
        if (time < minT) minT = time;
        if (time > maxT) maxT = time;
      }
    }
    return Number.isFinite(minT) && minT < maxT ? { minT, maxT } : { minT: 0, maxT: Date.now() };
  };
  const allWord = () => t("knowledge.slicerAll", "전체");
  /* 창 하나를 건다: 컷오프(0이면 전체), 머리의 낱말, 슬라이더의 자리, 켜진 프리셋.
   * 낱말은 `say`로 쓴다 — 두 자리가 `data-i18n="knowledge.slicerAll"`을 들고 있어
   * 언어를 바꾸는 `applyLocale`이 잘라 쓰는 동안에도 「전체」로 되돌아가지 않는다. */
  const applySlice = ({ cutoff, word, position, preset }) => {
    const layout = knowledgeLayouts.get(view);
    if (!layout) return;
    knowledgeSlicerCutoff = cutoff;
    if (sliderEl) sliderEl.value = String(position);
    if (slicerLabelEl) say(slicerLabelEl, word);
    if (slicerValEl) say(slicerValEl, word);
    slicerToggleEl?.classList.toggle("is-active", cutoff > 0);
    for (const b of slicerEl?.querySelectorAll(".knowledge-slicer-preset") ?? []) {
      b.classList.toggle("is-active", b.dataset.preset === preset);
    }
    paintKnowledgeSlice(view, layout);
    /* 선의 옷은 프레임이 입힌다 — 다음 애니메이션 프레임에 한 번. 끄는 손의 input
     * 여러 번이 한 번의 그림이 되고, 판을 재는 읽기(`clientWidth`)가 방금 쓴 점의
     * 옷을 이 핸들러 안에서 동기로 재계산시키지 않는다(실측: 1020쪽에서 0.9→20ms).
     * 남은 훑기가 없으면 `knowledgeSettle`은 그림만 그린다. */
    knowledgeSettle(view);
  };

  if (sliderEl) {
    sliderEl.oninput = () => {
      const layout = knowledgeLayouts.get(view);
      if (!layout) return;
      const val = Number(sliderEl.value);
      const { minT, maxT } = sliceRange(layout);
      applySlice(val === 0
        ? { cutoff: 0, word: allWord, position: 0, preset: "all" }
        : { cutoff: minT + (val / 100) * (maxT - minT), word: () => `${val}%`, position: val, preset: null });
    };
  }

  /* 프리셋은 벽시계의 창이다: 「24시간」은 지금에서 24시간 전이 컷오프다. 그 시각을
   * 그림의 시간 폭 위의 백분율로 옮겨 1% 단위로 반올림하고 1~100에 가두던 판은 400일
   * 볼트의 「24시간」에서 3시간 전의 페이지를 흐렸고, 닷새 안의 볼트의 「30일」에서 첫
   * 한 시간을 흐렸다(K24). 백분율은 슬라이더의 손잡이 자리로만 쓴다. */
  slicerEl?.querySelector(".knowledge-slicer-presets")?.addEventListener("click", (e) => {
    const row = KNOWLEDGE_SLICER_PRESETS.find(
      (one) => one.id === e.target.closest(".knowledge-slicer-preset")?.dataset.preset,
    );
    const layout = knowledgeLayouts.get(view);
    if (!row || !layout) return;
    if (row.ms === 0) {
      applySlice({ cutoff: 0, word: allWord, position: 0, preset: row.id });
      return;
    }
    const cutoff = Date.now() - row.ms;
    const { minT, maxT } = sliceRange(layout);
    applySlice({
      cutoff,
      word: () => t(row.key, row.word),
      position: Math.max(0, Math.min(100, Math.round(((cutoff - minT) / (maxT - minT)) * 100))),
      preset: row.id,
    });
  });
  view.querySelector(".knowledge-refresh").onclick = () => void refreshKnowledgeGraph({ force: true });
  const searchBox = view.querySelector(".knowledge-search");
  const find = view.querySelector(".knowledge-query");
  find.value = knowledgeQuery;
  find.oninput = () => {
    knowledgeQuery = find.value;
    knowledgeCandidateAt = -1;
    /* 치는 손 아래에서만 상자가 선다 — 고른 뒤 닫힌 상자는 다음 글자에 다시 열리고,
     * 포커스 없이 값만 바뀐 상자(시야 복원)는 열리지 않는다. */
    if (document.activeElement === find) setKnowledgeSuggesting(view, true);
    followKnowledgeChips();
    void paintKnowledgeView();
  };
  /* 후보의 키보드(S3): ↑↓가 줄을 짚고 Enter가 고른다(Mod+Enter는 주변 탐색으로), Esc는
   * 상자를 닫는다. 짚은 줄이 없는 Enter는 첫 줄이다. */
  const resultsList = view.querySelector(".knowledge-search-results");
  find.onkeydown = (event) => {
    const options = [...resultsList.querySelectorAll('[role="option"]')];
    /* 열린 상자의 Esc는 상자를 닫는 것이고 **여기서 멈춘다** — 창의 Esc 사다리는
     * `defaultPrevented`를 묻지 않고 마지막 칸에서 읽고 있는 판을 닫는다(캔버스의 Esc와
     * 같은 이유). 닫힌 상자의 Esc는 그 사다리의 것이다. */
    if (event.key === "Escape") {
      if (!searchBox?.classList.contains("is-suggesting")) return;
      event.preventDefault();
      event.stopPropagation();
      setKnowledgeSuggesting(view, false);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (options.length === 0) return;
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      knowledgeCandidateAt = (knowledgeCandidateAt + step + options.length) % options.length;
      options.forEach((row, index) => writeAttribute(row, "aria-selected", String(index === knowledgeCandidateAt)));
      return;
    }
    if (event.key === "Enter" && options.length > 0) {
      event.preventDefault();
      const row = options[Math.max(0, knowledgeCandidateAt)];
      pickKnowledgeCandidate(view, row.dataset.knowledgeKey, { explore: event.metaKey || event.ctrlKey });
    }
  };
  resultsList.onclick = (event) => {
    const explore = event.target.closest("[data-knowledge-explore]");
    if (explore) {
      pickKnowledgeCandidate(view, explore.dataset.knowledgeExplore, { explore: true });
      return;
    }
    const row = event.target.closest('[role="option"]');
    if (row) pickKnowledgeCandidate(view, row.dataset.knowledgeKey);
  };
  find.onfocus = () => {
    setKnowledgeSuggesting(view, true);
    paintKnowledgeSavedPhrases(view);
  };
  find.onclick = () => {
    setKnowledgeSuggesting(view, true);
    paintKnowledgeSavedPhrases(view);
  };

  const tokenChips = view.querySelectorAll(".knowledge-token-chip");
  for (const chip of tokenChips) {
    chip.onclick = () => {
      const tok = chip.dataset.token || chip.textContent.trim();
      const cur = find.value.trim();
      if (!cur) {
        find.value = tok;
      } else {
        find.value = `${cur} ${tok}`;
      }
      knowledgeQuery = find.value;
      followKnowledgeChips();
      find.focus();
      void paintKnowledgeView();
    };
  }

  const savePhraseBtn = view.querySelector(".knowledge-save-phrase");
  if (savePhraseBtn) {
    savePhraseBtn.onclick = async () => {
      const phrase = knowledgeQuery.trim() || find.value.trim();
      if (!phrase) return;
      const vault = knowledgeReport?.vault ?? secondBrainVault;
      const store = getVaultSceneStore(vault);
      if (!store.phrases) store.phrases = [];
      if (!store.phrases.includes(phrase)) {
        store.phrases.push(phrase);
        await saveVaultSceneStore(vault, store);
      }
      paintKnowledgeSavedPhrases(view);
    };
  }

  document.addEventListener("pointerdown", (e) => {
    if (!e.target.closest(".knowledge-search")) setKnowledgeSuggesting(view, false);
  });
  /* 목록의 키보드(S7): 같은 목록 안에서 ↑↓가 줄 사이를 옮기고 Home/End가 끝으로 간다.
   * 줄은 진짜 <button>이라 Tab으로도 닿고, 고르는 것은 그 단추의 클릭 그대로다. */
  view.querySelector(".knowledge-inspector").onkeydown = (event) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const row = event.target.closest(".knowledge-inspector-row");
    const list = row?.closest(".knowledge-inspector-list");
    if (!row || !list) return;
    const rows = [...list.querySelectorAll(".knowledge-inspector-row")].filter((one) => !one.disabled);
    const at = rows.indexOf(row);
    if (at < 0) return;
    event.preventDefault();
    const next = event.key === "Home" ? 0
      : event.key === "End" ? rows.length - 1
        : event.key === "ArrowDown" ? Math.min(rows.length - 1, at + 1)
          : Math.max(0, at - 1);
    rows[next].focus();
  };
  /* 인스펙터의 손은 하나다. 목록의 줄들은 선택이 옮길 때마다 다시 서지만 이
   * 손은 판이 지어질 때 한 번 매어진다 — 줄마다 리스너를 매면 백 줄짜리 개요가
   * 백 개의 리스너이고, 그 백 개는 다음 선택에서 조용히 새는 백 개다(§6). */
  view.querySelector(".knowledge-inspector").onclick = (event) => {
    const layout = knowledgeLayouts.get(view);
    if (!layout) return;
    /* 탭(S4): 옷 하나와 두 판의 `hidden`뿐 — 목록은 그대로 서 있다. */
    const tab = event.target.closest("[data-knowledge-tab]");
    if (tab) {
      knowledgeInspectorTab = tab.dataset.knowledgeTab;
      paintKnowledgeInspector(view, layout);
      return;
    }
    /* 공급망 절의 손(P4) — 펼치기·OSV·심각도 줄·다시 확인. */
    if (knowledgeSupplyInspectorClick(view, event)) return;
    const step = event.target.closest("[data-knowledge-depth]");
    if (step) {
      pickKnowledgeDepth(view, layout, Number(step.dataset.knowledgeDepth));
      return;
    }
    /* 접힌 허브를 펼친다(t-4140) — 이 탐색 안에서만; 중심이 옮으면 다시 접힌다. */
    const unfold = event.target.closest("[data-knowledge-unfold]");
    if (unfold) {
      knowledgeUnfolded.add(unfold.dataset.knowledgeUnfold);
      knowledgeUnfoldGeneration += 1;
      void paintKnowledgeView();
      return;
    }
    const cluster = event.target.closest("[data-knowledge-cluster]");
    if (cluster) {
      toggleKnowledgeCluster(view, Number(cluster.dataset.knowledgeCluster));
      return;
    }
    /* 건강 카드의 줄(3차): 고아·유령은 렌즈를 켜고 끄며, 모순·대체는 관계 낱말을
     * 검색 상자에 넣는다 — 검색은 멤버십을 바꾸지 않으므로 모순이 어디 사는지가
     * 그림 그대로 보인다. 다시 누르면 놓는다. */
    const lint = event.target.closest("button[data-knowledge-lint]")?.dataset.knowledgeLint;
    if (lint !== undefined) {
      const door = KNOWLEDGE_HEALTH_ROWS.find((row) => row.lint === lint)?.door;
      if (door === "none") return;
      if (lint === "orphans") knowledgeOrphansOnly = !knowledgeOrphansOnly;
      else if (lint === "ghosts") knowledgeGhostsOnly = !knowledgeGhostsOnly;
      else if (door === "lens") knowledgeLintLens = knowledgeLintLens === lint ? null : lint;
      /* 라이브 층의 셋(t-2931)은 렌즈다 — 툴바의 「살아 있는 것만」·「합칠 후보만」과
       * 같은 손이고, 「회상된 적 없음」은 이 줄만이 여는 렌즈다. */
      else if (lint === "recalledToday") knowledgeAliveOnly = !knowledgeAliveOnly;
      else if (lint === "neverRecalled") knowledgeColdOnly = !knowledgeColdOnly;
      else if (lint === "merge") knowledgeMergeOnly = !knowledgeMergeOnly;
      else {
        const word = lint === "contradictions" ? "contradicts" : "supersedes";
        knowledgeQuery = knowledgeQuery.trim().toLocaleLowerCase() === word ? "" : word;
        knowledgeTagsPicked.clear();
        find.value = knowledgeQuery;
      }
      void paintKnowledgeView();
      return;
    }
    if (event.target.closest(".knowledge-inspector-request")) {
      if (knowledgeSelectedKey !== null) void requestFromKnowledge(view, knowledgeSelectedKey);
      return;
    }
    if (event.target.closest(".knowledge-inspector-open")) {
      if (knowledgeSelectedKey !== null) void openKnowledgePage(view, knowledgeSelectedKey);
      return;
    }
    /* 연결 만들기(S5): 열기·후보 고르기·방향·저장·취소 — 서식 안의 손은 여기 한 곳이다. */
    if (event.target.closest(".knowledge-inspector-link")) {
      const form = view.querySelector(".knowledge-link-form");
      if (form && !form.hidden && knowledgeLinkTarget === null) {
        closeKnowledgeLinkForm(view);
      } else {
        openKnowledgeLinkForm(view);
      }
      return;
    }
    if (event.target.closest(".knowledge-link-form")) {
      const option = event.target.closest("[data-knowledge-link-key]");
      if (option) {
        knowledgeLinkTarget = option.dataset.knowledgeLinkKey;
        view.querySelector(".knowledge-link-target").value = layout.model.titles[layout.model.keys.indexOf(knowledgeLinkTarget)];
        paintKnowledgeLinkForm(view, layout);
        return;
      }
      const direction = event.target.closest("[data-knowledge-link-direction]");
      if (direction) {
        knowledgeLinkDirection = direction.dataset.knowledgeLinkDirection;
        paintKnowledgeLinkForm(view, layout);
        return;
      }
      if (event.target.closest(".knowledge-link-cancel")) {
        closeKnowledgeLinkForm(view);
        return;
      }
      if (event.target.closest(".knowledge-link-save") && knowledgeLinkTarget !== null) {
        const plan = knowledgeLinkPlan(layout);
        closeKnowledgeLinkForm(view);
        void relateKnowledge(view, { from: plan.fromKey, to: plan.toKey, kind: plan.kind });
      }
      return;
    }
    const seatDoor = event.target.closest("[data-knowledge-seat]");
    if (seatDoor) {
      if (!seatDoor.disabled) void revealTaskBoardPane(Number(seatDoor.dataset.knowledgeSeat));
      return;
    }
    const seatOf = (key) => (key === null ? -1 : layout.model.keys.indexOf(key));
    if (event.target.closest(".knowledge-inspector-center")) {
      const here = seatOf(knowledgeSelectedKey);
      if (here >= 0) knowledgeCenterOn(view, layout, here);
      return;
    }
    const row = event.target.closest("[data-knowledge-key]");
    if (!row) return;
    /* 태그 줄은 검색으로 간다 — 툴바의 칩과 같은 몸짓이고, 같은 이유로 멤버십을
     * 건드리지 않는다. */
    const tag = row.dataset.knowledgeTag;
    if (tag !== undefined) {
      knowledgeTagsPicked.clear();
      knowledgeTagsPicked.add(tag);
      knowledgeQuery = knowledgeTagQuery(tag);
      find.value = knowledgeQuery;
      void paintKnowledgeView();
      return;
    }
    const seat = seatOf(row.dataset.knowledgeKey);
    if (seat < 0) return;
    selectKnowledgeNode(view, row.dataset.knowledgeKey);
    /* 주변 탐색은 새 중심의 주변을 채우므로 따로 날지 않는다(t-4140). */
    if (knowledgeMode !== "local") knowledgeCenterOn(view, layout, seat);
  };
  /* 연결 서식의 입력(S5): 글자마다 후보, ↑↓·Enter로 고르기, 관계 유형은 바뀌는 대로 미리보기. */
  const linkTarget = view.querySelector(".knowledge-link-target");
  linkTarget.oninput = () => {
    const layout = knowledgeLayouts.get(view);
    if (!layout) return;
    knowledgeLinkTarget = null;
    knowledgeLinkCandidateAt = -1;
    paintKnowledgeLinkForm(view, layout);
  };
  linkTarget.onkeydown = (event) => {
    const layout = knowledgeLayouts.get(view);
    const options = [...view.querySelectorAll('.knowledge-link-candidates [role="option"]')];
    if (!layout) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeKnowledgeLinkForm(view);
      return;
    }
    if ((event.key === "ArrowDown" || event.key === "ArrowUp") && options.length > 0) {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      knowledgeLinkCandidateAt = (knowledgeLinkCandidateAt + step + options.length) % options.length;
      options.forEach((row, index) => writeAttribute(row, "aria-selected", String(index === knowledgeLinkCandidateAt)));
      return;
    }
    if (event.key === "Enter" && options.length > 0) {
      event.preventDefault();
      knowledgeLinkTarget = options[Math.max(0, knowledgeLinkCandidateAt)].dataset.knowledgeLinkKey;
      linkTarget.value = layout.model.titles[layout.model.keys.indexOf(knowledgeLinkTarget)];
      paintKnowledgeLinkForm(view, layout);
    }
  };
  view.querySelector(".knowledge-link-kind").onchange = () => {
    const layout = knowledgeLayouts.get(view);
    if (layout) paintKnowledgeLinkForm(view, layout);
  };
  /* 인스펙터의 줄에 포인터를 올리면 그 점이 밝는다(3차) — 캔버스의 호버와 같은
   * 손이고 같은 열쇠를 든다(캔버스로 돌아온 포인터가 「바뀐 것 없음」으로 읽히지
   * 않게). 점이 아닌 줄(군집·건강)은 밝힌 것을 놓는다. */
  const panel = view.querySelector(".knowledge-inspector");
  panel.onpointerover = (event) => {
    const layout = knowledgeLayouts.get(view);
    if (!layout) return;
    const key = event.target.closest("[data-knowledge-key]")?.dataset.knowledgeKey ?? null;
    const next = key !== null && layout.model.keys.includes(key) ? key : null;
    if (next === knowledgeHoverKey) return;
    knowledgeHoverKey = next;
    litKnowledge(view, layout, next);
  };
  panel.onpointerleave = () => {
    const layout = knowledgeLayouts.get(view);
    if (!layout || knowledgeHoverKey === null) return;
    knowledgeHoverKey = null;
    litKnowledge(view, layout, null);
  };
  wireKnowledgeNodeDrag(view, canvas);
  for (const button of view.querySelectorAll("[data-knowledge-flag]")) {
    button.onclick = () => {
      const flag = button.dataset.knowledgeFlag;
      if (flag === "orphans") knowledgeOrphansOnly = !knowledgeOrphansOnly;
      else if (flag === "ghosts") knowledgeGhostsOnly = !knowledgeGhostsOnly;
      else if (flag === "typed") knowledgeTypedOnly = !knowledgeTypedOnly;
      else if (flag === "alive") knowledgeAliveOnly = !knowledgeAliveOnly;
      else if (flag === "merge") knowledgeMergeOnly = !knowledgeMergeOnly;
      else if (flag === "nav") knowledgeNavShown = !knowledgeNavShown;
      else if (flag === "supply") {
        /* 공급망(P4)은 답에 없는 점을 만드는 렌즈라 켜면 백엔드에 묻는다 — 그리는 일도 그 문이 한다. */
        toggleKnowledgeSupply();
        return;
      } else {
        // 원본 노드는 답에 없는 점을 만드는 일이라 백엔드에 다시 묻는다.
        knowledgeShowSources = !knowledgeShowSources;
        void refreshKnowledgeGraph({ force: true });
        return;
      }
      void paintKnowledgeView();
    };
  }
  /* 태그 칩은 렌즈가 아니라 검색이다.
   *
   * 멤버십을 바꾸는 것은 렌즈 넷뿐이라는 것이 이 판의 규칙이고(§3), 태그로
   * 고르는 일은 「core라고 적힌 것들을 밝혀 달라」는 청이지 나머지를 없애
   * 달라는 청이 아니다 — 지운 그림에서는 「core가 무엇 옆에 있는가」를 볼 수
   * 없고, 그것이 태그를 짚는 사람이 보려던 것이다. */
  view.querySelector(".knowledge-tags").onclick = (event) => {
    const tag = event.target.closest("[data-knowledge-tag]")?.dataset.knowledgeTag;
    if (!tag) return;
    const off = knowledgeTagsPicked.has(tag);
    knowledgeTagsPicked.clear();
    if (!off) knowledgeTagsPicked.add(tag);
    knowledgeQuery = off ? "" : knowledgeTagQuery(tag);
    find.value = knowledgeQuery;
    void paintKnowledgeView();
  };
  /* 판이 넓어지면 보는 자리도 넓어진다. viewBox는 판의 폭에서 나오므로 다시
   * 그리는 것 말고 할 일이 없다 — 그리고 이 관찰자는 창에 하나뿐이다. */
  watchGraphResize(canvas, () => {
    const layout = knowledgeLayouts.get(view);
    if (layout && !view.hidden) paintKnowledgeFrame(view, layout);
  });
  /* 티어를 재는 상자는 캔버스가 아니라 판이다 — 캔버스의 폭은 인스펙터가 옆에 섰는지에
   * 따라 달라지므로, 그것으로 티어를 고르면 두 티어 사이를 오가는 고리가 된다. */
  watchGraphResize(view, () => {
    if (!view.hidden) paintKnowledgeTier(view);
  });
}

/* 점을 쥐고 끈다 (3차) — Obsidian의 손.
 *
 * 판의 pan(`wireGraphDrag`)보다 먼저 서는 것은 이 리스너가 점의 층에 있어서다:
 * 이벤트는 점에서 캔버스로 올라가고, 여기서 멈추면 pan은 시작하지 않는다. 슬롭
 * 안이면 아무것도 하지 않는다 — 그 몸짓은 클릭이고 클릭은 캔버스의 것이다. 슬롭을
 * 넘으면 점은 포인터의 것이 되고(`pinned`), 포인터가 움직일 때마다 훑기 예산을
 * 재충전해 이웃이 스프링으로 따라온다 — 한 프레임에 한 훑기(Obsidian도 그렇게 한
 * 틱씩 돈다). 놓으면 판이 남은 예산만큼 마저 앉고, 자리는 `placed`에 남는다.
 *
 * 끈 몸짓의 click은 캔버스가 삼킨다(`layout.dragged`). 포인터가 창 밖에서 놓여
 * click이 오지 않은 판을 위해, 다음 pointerdown이 그 깃발을 내린다 — 캡처
 * 단계라 점의 층보다 먼저 지난다. */
function wireKnowledgeNodeDrag(view, canvas) {
  canvas.addEventListener("pointerdown", () => {
    const layout = knowledgeLayouts.get(view);
    if (layout) layout.dragged = false;
  }, { capture: true });
  const layer = view.querySelector(".knowledge-nodes");
  layer.onpointerdown = (event) => {
    if (event.button !== 0) return;
    const node = event.target.closest(".knowledge-node");
    const layout = knowledgeLayouts.get(view);
    if (!node || !layout) return;
    event.stopPropagation();
    const seat = Number(node.dataset.graphSeat);
    /* Alt+드래그는 잇는 손이다(S5) — 점은 제자리에 두고, 놓은 자리의 점을 대상으로 서식을
     * 연다. 수식키가 없는 드래그는 여전히 배치의 손이라 둘이 섞이지 않는다. */
    if (event.altKey) {
      canvas.classList.add("is-linking");
      const finish = (held) => {
        window.removeEventListener("pointerup", finish);
        window.removeEventListener("pointercancel", finish);
        canvas.classList.remove("is-linking");
        if (held.type === "pointercancel") return;
        const landed = document.elementFromPoint(held.clientX, held.clientY)?.closest(".knowledge-node");
        const target = landed?.dataset.graphKey ?? null;
        if (target === null || target === layout.model.keys[seat]) return;
        if (knowledgeSelectedKey !== layout.model.keys[seat]) selectKnowledgeNode(view, layout.model.keys[seat]);
        layout.dragged = true;
        openKnowledgeLinkForm(view, { target });
      };
      window.addEventListener("pointerup", finish);
      window.addEventListener("pointercancel", finish);
      return;
    }
    const startX = event.clientX;
    const startY = event.clientY;
    const fromX = layout.x[seat];
    const fromY = layout.y[seat];
    let moving = false;
    const move = (held) => {
      if (!moving
        && Math.hypot(held.clientX - startX, held.clientY - startY) < GRAPH_DRAG_SLOP) return;
      if (!moving) {
        moving = true;
        layout.pinned = seat;
        layout.dragged = true;
        layout.flight = null;
        /* 끄는 손 아래에서 화면이 숨 쉬지 않게 — 그 순간의 폭이 배율의 바탕이다. */
        takeKnowledgeCamera(layout);
        canvas.classList.add("is-dragging");
      }
      const yScale = knowledgeYScale(view, layout);
      layout.x[seat] = fromX + (held.clientX - startX) / layout.scale;
      layout.y[seat] = fromY + (held.clientY - startY) / (layout.scale * yScale);
      layout.geometryRevision += 1;
      layout.vx[seat] = 0;
      layout.vy[seat] = 0;
      /* 고리 위의 점은 물리가 없다(시안과 같다) — 끈 자리에 그대로 선다. 지도의 훑기를
       * 다시 켜면 보이지 않는 지도만 40번 더 도는 일이다. */
      if (layout.ring === null) layout.left = Math.max(layout.left, layout.tuning.dragBudget);
      layout.pace = 1;
      knowledgeSettle(view);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      if (!moving) return;
      layout.pinned = -1;
      canvas.classList.remove("is-dragging");
      knowledgeBounds(layout);
      knowledgeSettle(view);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  };
}

/* ---- 묻고 그리기 ---- */

async function refreshKnowledgeGraph({ force = false, trailing = false } = {}) {
  const vault = secondBrainVault || knowledgeReport?.vault || "";
  if (vault === "") {
    knowledgeReport = null;
    knowledgeError = t("knowledge.noVault", "볼트를 고르세요");
    await paintKnowledgeView();
    return;
  }
  const now = Date.now();
  if (!force && now - knowledgeAskedAt < KNOWLEDGE_REFRESH_FLOOR_MS) {
    /* 워처의 청(t-2931)은 바닥 안에서 버려지지 않고 **미뤄진다**: 연달아 들린
     * 변화는 바닥이 끝나는 자리에서 한 번 다시 읽힌다. 타이머는 하나뿐이라 열
     * 번 울려도 한 번이다. 사람의 청(탭·훅)은 옛 규칙 그대로 버린다 — 그 청은
     * 다음 문에서 어차피 다시 온다. */
    if (trailing && knowledgeRefreshTimer === 0) {
      knowledgeRefreshTimer = setTimeout(() => {
        knowledgeRefreshTimer = 0;
        const tab = knowledgeTab();
        if (tab && stillShowing(tab)) void refreshKnowledgeGraph({ trailing: true });
      }, KNOWLEDGE_REFRESH_FLOOR_MS - (now - knowledgeAskedAt) + 1);
    }
    return;
  }
  knowledgeAskedAt = now;
  const generation = ++knowledgeGeneration;
  knowledgeLoading = true;
  knowledgeError = null;
  await paintKnowledgeView();
  try {
    const answer = await invoke("second_brain_graph", {
      path: vault,
      sources: knowledgeShowSources,
      /* 일지의 시각은 벽시계이고 백엔드는 시간대 표가 없다 — 창의 것을 준다. */
      utcOffsetMinutes: new Date().getTimezoneOffset(),
    });
    if (generation !== knowledgeGeneration) return;
    noteKnowledgeFresh(knowledgeReport, answer);
    knowledgeReport = answer;
    // 빈 상태의 단추가 누를 것. 목록은 작고 이 판을 열 때만 묻는다.
    try {
      const commands = (await invoke("list_quick_commands")) ?? [];
      // 이 볼트의 것, 그리고 세팅이 지은 것. 뒷자리만 보면 사람이 손으로 만든
      // 「…-ingest」가 이 단추를 차지한다.
      knowledgeIngest = commands.find((one) => one.workspace === answer.vault
        && one.id.startsWith("second-brain-") && one.id.endsWith("-ingest")) ?? null;
    } catch {
      knowledgeIngest = null;
    }
  } catch (error) {
    if (generation !== knowledgeGeneration) return;
    knowledgeError = String(error);
  }
  if (generation !== knowledgeGeneration) return;
  knowledgeLoading = false;
  await paintKnowledgeView();
}

function knowledgeTab() {
  return tabs.find((held) => held.kind === KNOWLEDGE_TAB.kind) ?? null;
}

/* 무대가 이 탭을 드러낼 때 지나는 문. 여기서만 다시 읽는 것은 「보고 있을
 * 때만」이 이 표면의 규칙이기 때문이다 — 필터 한 글자는 이 문을 지나지 않는다.
 * 볼트를 지켜보는 것은 백엔드의 워처 레인이고(t-2931, 상한 있는 목록을 같은
 * 폴러가 stat한다 — 두 번째 스캐너는 여전히 없다), 그 말은 `second-brain:changed`로
 * 온다. */
function paintKnowledgeStage(tab) {
  void refreshKnowledgeGraph();
  /* 공급망 렌즈가 켜져 있으면 그 답도 — 잠금 파일은 판을 보지 않는 동안에도 바뀐다(바닥 시간 안의
   * 청은 버린다, `refreshKnowledgeSupply`). */
  void refreshKnowledgeSupply();
  return paintKnowledgeView(tab);
}

/* 취합이 끝나면 그림이 자란다. 끝났다는 말은 훅이 하고, 다시 읽는 것은 이 판이
 * 사람 앞에 서 있을 때뿐이다 — 볼트 전체를 stat하는 watcher는 여전히 금지다
 * (stat 폭풍); 레인이 따르는 것은 표가 정한 수의 폴더와 최근 페이지뿐이다.
 *
 * 이 문을 여는 손은 `shell.js`의 `hook:agent` 리스너다. 여기에 두 번째 리스너를
 * 세우지 않는 이유는 그 이벤트의 **첫** 블록을 읽는 소스 핀이 셋 있고, 합쳐진
 * 건초더미에서 어느 것이 첫 블록인지는 파일 순서가 정한다는 것이다 — 손을 하나
 * 더 매는 것이 그 셋을 남의 블록 위에서 읽게 만든다(실측: 셋 다 빨강). */
function noteKnowledgeAgentState(state) {
  if (state !== "done" && state !== "idle") return;
  invalidateTaskBoardRecalls();
  const tab = knowledgeTab();
  if (!tab || !stillShowing(tab)) return;
  void refreshKnowledgeGraph();
}

async function paintKnowledgeView(tab = knowledgeTab()) {
  if (!tab) return;
  const view = docHost(tab.pane, KNOWLEDGE_TAB.kind);
  wireKnowledgeView(view);
  const trouble = view.querySelector(".knowledge-error");
  trouble.hidden = knowledgeError === null;
  trouble.textContent = knowledgeError ?? "";
  const model = knowledgeModel(knowledgeReport);
  paintKnowledgeEmpty(view, model);
  const layout = knowledgeLayout(view, model);
  applyKnowledgeEntry(view, layout);
  /* 주변 탐색의 중심이 그림에서 사라졌으면(렌즈·볼트의 변화) 가장 강한 페이지로
   * 옮기고, 그것도 없으면 전체 지도다(t-4140). 답이 오기 전의 빈 그림은 판정하지
   * 않는다 — 부팅이 든 모드를 빈 그림이 지워서는 안 된다. */
  if (knowledgeMode === "local" && knowledgeReport !== null
      && (knowledgeSelectedKey === null || !model.keys.includes(knowledgeSelectedKey))) {
    knowledgeSelectedKey = knowledgeDefaultCentre(layout);
    if (knowledgeSelectedKey === null) knowledgeMode = KNOWLEDGE_MODES[0].id;
  }
  knowledgeSubgraph(layout);
  paintKnowledgeTier(view);
  /* 판의 크기는 여기서, 점과 선을 쓰기 전에 읽는다 — 프레임의 viewBox가 이것을 쓴다. */
  const canvas = view.querySelector(".knowledge-canvas");
  layout.viewport = { wide: canvas.clientWidth, tall: canvas.clientHeight, fresh: true };
  knowledgePainterFor(view).paintTopology(layout);
  paintKnowledgeActivity(layout);
  paintKnowledgeClusterLayer(view, layout);
  /* 검색과 포커스는 점이 선 뒤에야 옷을 입힐 수 있고, 머리의 「매치 n」은 그 옷을
   * 세어 얻는 수다 — 그래서 순서가 이것이다. 밝힌 군집은 인스펙터의 단추도
   * 입으므로 인스펙터 뒤에 온다. */
  const matches = paintKnowledgeSearch(view, layout);
  paintKnowledgeCandidates(view, layout);
  paintKnowledgeSlice(view, layout);
  /* 경로가 먼저다: 포커스는 그려진 경로가 있으면 물러서므로 그 답을 읽는다. */
  highlightKnowledgePath(view, layout);
  knowledgeFocus(view, layout);
  paintKnowledgeHead(view, layout, matches);
  paintKnowledgeMode(view, layout);
  paintKnowledgeHealthMarks(view);
  paintKnowledgeInspector(view, layout);
  paintKnowledgeClusterLegend(view, layout);
  paintKnowledgeSpotlight(view, layout);
  paintKnowledgeSceneList(view);
  paintKnowledgeSavedPhrases(view);
  paintKnowledgeFrame(view, layout);
  layout.viewport.fresh = false;
  pulseKnowledgeFresh(view, layout);
  if (layout.left > 0) knowledgeSettle(view);
  /* 다른 표면이 고른 페이지: 답이 있는 그림에서 한 번, 고르고 가운데로. */
  if (knowledgeRevealKey !== null && knowledgeReport !== null) {
    const seat = model.keys.indexOf(knowledgeRevealKey);
    /* 「연결 보기」의 열쇠는 진입 규칙이 이미 중심으로 세웠고(S2), 주변 탐색은 새 중심의
     * 주변을 채우므로 따로 날지 않는다. */
    const seated = knowledgeRevealMode === "local" && knowledgeMode === "local";
    knowledgeRevealKey = null;
    knowledgeRevealMode = null;
    if (seat >= 0 && !seated) {
      selectKnowledgeNode(view, model.keys[seat]);
      knowledgeCenterOn(view, layout, seat);
    }
  }
}


/* 볼트가 움직였다는 백엔드의 말(t-2931): 워처 레인이 `wiki/`와 그 폴더들, 최근
 * 페이지들의 stamp가 움직인 것을 들었다. 판이 서 있으면 바닥을 지나 한 번 다시
 * 읽고, 서 있지 않으면 들고 있던 답을 낡은 것으로만 표시한다 — 다음 문에서 어차피
 * 다시 읽는다. 폴러가 아니다. */
listen("second-brain:changed", () => {
  invalidateTaskBoardRecalls();
  const tab = knowledgeTab();
  if (!tab || !stillShowing(tab)) {
    knowledgeAskedAt = 0;
    return;
  }
  void refreshKnowledgeGraph({ trailing: true });
});
