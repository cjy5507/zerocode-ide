/* ---- 지식 그래프의 우주 보기 — 빛의 그물 (사람의 11-12 요청, t-12443의 「실제 우주」를 잇는다) ---------
 *
 * 사람이 고른 그림은 「실제 우주」였다(09-28 18:3x). 그 우주는 은하의 모양을 지어 그렸고 — 나선 팔 · 먼지 · 이름
 * 없는 별 — 쪽 사이의 링크는 올려야만 보였다. 11-12에 사람이 다시 청했다: 「보기 편하고 … 지금 컨셉이 아니어도
 * 좋아」. 그래서 이 판의 빛은 전부 자료에서 온다:
 *
 *   · 쪽 하나는 별이다. 크기는 연결 수, 빛깔은 제 군집, 최근에 고친 쪽은 더 번진다.
 *   · 군집은 그 군집의 허브에서 퍼지는 나무다. 허브가 가운데, 허브와 바로 이어진 쪽이 그 둘레, 그 쪽에 이어진
 *     쪽이 그 밖이다 — 가지는 지어낸 선이 아니라 진짜 링크다(`knowledgeUniverseBurst`).
 *   · 군집 사이의 링크는 한 올 한 올 두 군집의 문을 지나는 다발로 묶인다. 굵은 다발이 필라멘트이고, 빛은 링크가
 *     가리키는 쪽으로 흐른다.
 *   · 나무에 들지 않은 군집 안의 링크는 가까이 가면 보이고, 쪽을 올리거나 고르면 그 쪽의 링크 전부가 켜진다.
 *
 * 군집의 자리는 평면 지도의 원반 자리 그대로이고(x·z) 높이만 더한다 — 외운 지도가 흔들리지 않는다(6차 설계
 * §4-a). 카메라 · 손 · 이름표 · 떠오름과 내려앉음은 t-12443의 것을 그대로 쓴다.
 *
 * 어디서 서는가. 전체 지도에서, WebGL2가 서는 판에서, 사람이 3D를 고른 동안(고른 적이 없으면 표의 기본 줄,
 * `KNOWLEDGE_DIMENSIONS`). 주변 탐색·관계 편집·점 끌기는 평면의 일이다(설계 §4-f). 평면 지도는 이 판 아래에서
 * 그대로 살고(두 페인터 중 하나), 우주가 서 있는 동안만 가려진다.
 *
 * 그리는 일은 있을 때만 한다. 비행·미끄러짐 중에는 화면의 박자로, 쉬는 동안의 움직임(아주 느린 회전과 빛의
 * 흐름)은 `ambient-gap`의 박자로만 잇고, 움직임을 끄면 한 장도 그리지 않는다. 판이 가려지면 멈춘다. 떠날
 * 때는 GPU의 것(기하·텍스처·프로그램·렌더 타깃·문맥)을 전부 놓는다. */

/* 전체 지도의 두 얼굴. 표의 순서가 머리의 순서이고, `first`가 사람이 고른 적 없는 판의 기본이다 —
 * 기본값은 이 한 줄에만 산다. 3D는 WebGL2가 서는 판에서만 설 수 있다(`knowledgeDimensionNow`). */
const KNOWLEDGE_DIMENSIONS = Object.freeze([
  Object.freeze({ id: "2d", key: "knowledge.dimension2d", word: "평면 지도로 보기", short: "2D" }),
  Object.freeze({ id: "3d", key: "knowledge.dimension3d", word: "우주로 보기", short: "3D", first: true }),
]);
const KNOWLEDGE_DIMENSION_FIRST = KNOWLEDGE_DIMENSIONS.find((row) => row.first === true)?.id
  ?? KNOWLEDGE_DIMENSIONS[0].id;

/* 판의 수 — 값은 tokens.css의 `--knowledge-3d-*` 한 곳에 있고 판마다 한 번 읽는다
 * (`knowledgeUniverseTuning`). 카메라 · 손 · 이름표의 수는 t-12443의 것이다. 셰이더가 읽는 모양의 수는 아래
 * 표들(`KNOWLEDGE_UNIVERSE_STAR` · `_EDGE` · `_AURA` · `_BURST` · `_CURVE`)에 한 번씩 산다. */
const KNOWLEDGE_UNIVERSE_TOKENS = Object.freeze({
  /* 시야각(도)과 앞뒤 자름 — 뒤 자름은 우주가 넓어져도 배경 별이 잘리지 않게 닿는 거리의 배수까지. */
  fov: "--knowledge-3d-fov",
  near: "--knowledge-3d-near",
  far: "--knowledge-3d-far",
  farReach: "--knowledge-3d-far-reach",
  pixelCap: "--knowledge-3d-dpr-cap",
  /* 처음 자리(`computeHome`): 비스듬히 내려다보는 두 각, 닿는 거리의 여백과 최소값, 판의 비의 하한,
   * 세로의 몫, 물러섬, 목표점의 들림. */
  homeYaw: "--knowledge-3d-home-yaw",
  homePitch: "--knowledge-3d-home-pitch",
  homeFit: "--knowledge-3d-home-fit",
  homePad: "--knowledge-3d-home-pad",
  homeLeast: "--knowledge-3d-home-least",
  homeAspect: "--knowledge-3d-home-aspect",
  homeTall: "--knowledge-3d-home-tall",
  homeLift: "--knowledge-3d-home-lift",
  /* 평면 지도의 자세 — 거의 수직으로 내려다보고, 별은 이 높이의 판 위에 선다. */
  flatPitch: "--knowledge-3d-flat-pitch",
  flatHeight: "--knowledge-3d-flat-height",
  pitchLow: "--knowledge-3d-pitch-low",
  pitchHigh: "--knowledge-3d-pitch-high",
  /* 다가갈 수 있는 가장 가까운 거리, 물러설 수 있는 처음 거리의 배수, 바퀴·단추의 걸음. */
  zoomMin: "--knowledge-3d-zoom-min",
  zoomMax: "--knowledge-3d-zoom-max",
  zoomWheel: "--knowledge-3d-zoom-wheel",
  zoomStep: "--knowledge-3d-zoom-step",
  zoomStepMs: "--knowledge-3d-zoom-step-ms",
  /* 손: 끌기의 문턱(px, 가로+세로), 궤도·옮기기의 px당 몫, 놓은 뒤 미끄러짐(줄어듦·멈춤 문턱·속도 상한
   * 둘·세로의 몫·놓기 직전의 창), 화살표 한 번의 궤도. */
  dragSlop: "--knowledge-3d-drag-slop",
  dragYaw: "--knowledge-3d-drag-yaw",
  dragPitch: "--knowledge-3d-drag-pitch",
  dragPan: "--knowledge-3d-drag-pan",
  glideDecay: "--knowledge-3d-glide-decay",
  glideStop: "--knowledge-3d-glide-stop",
  glideYaw: "--knowledge-3d-glide-yaw",
  glidePitch: "--knowledge-3d-glide-pitch",
  glidePitchShare: "--knowledge-3d-glide-pitch-share",
  glideWindow: "--knowledge-3d-glide-window",
  keyTurn: "--knowledge-3d-key-turn",
  /* 쉬는 동안 돌기: 쉰 시간, 도는 빠르기(rad/s), 올라서는 빠르기. 한 프레임의 시간 상한과 첫 프레임. 쉬는
   * 동안의 움직임만 남은 판이 다음 장까지 기다리는 시간(ms) — 화면의 박자(120 Hz면 8 ms)로 그릴 까닭이 없다. */
  idleDelay: "--knowledge-3d-idle-delay",
  idleSpin: "--knowledge-3d-idle-spin",
  idleRamp: "--knowledge-3d-idle-ramp",
  frameMost: "--knowledge-3d-frame-most",
  frameFirst: "--knowledge-3d-frame-first",
  ambientGap: "--knowledge-3d-ambient-gap",
  /* 문을 연 판·처음 자리로 돌아간 판·움직임을 켠 판이 이미 쉰 것으로 치는 시간. */
  openRest: "--knowledge-3d-open-rest",
  homeRest: "--knowledge-3d-home-rest",
  wakeRest: "--knowledge-3d-wake-rest",
  /* 날아가기: 쪽은 거리 max(`fly-page-dist`, R × `fly-page-reach`)에, 군집은 R × `fly-galaxy-reach` +
   * `fly-galaxy-pad`에, 필라멘트는 두 군집 사이 거리 × `fly-flow-reach` + `fly-flow-pad`에 pitch `fly-flow-pitch`로
   * 옆에서. 군집과 쪽은 지금 보는 방향 그대로 다가간다 — 나무는 어느 쪽에서 봐도 같은 꼴이다. */
  flyPage: "--knowledge-3d-fly-page",
  flyPageDist: "--knowledge-3d-fly-page-dist",
  flyPageReach: "--knowledge-3d-fly-page-reach",
  flyGalaxyReach: "--knowledge-3d-fly-galaxy-reach",
  flyGalaxyPad: "--knowledge-3d-fly-galaxy-pad",
  flyFlow: "--knowledge-3d-fly-flow",
  flyFlowReach: "--knowledge-3d-fly-flow-reach",
  flyFlowPad: "--knowledge-3d-fly-flow-pad",
  flyFlowPitch: "--knowledge-3d-fly-flow-pitch",
  /* 비행(ms): 떠오름·내려앉음·첫 그림(별이 허브에서 퍼지는 시간)·보통·처음 자리. */
  rise: "--knowledge-3d-rise",
  fold: "--knowledge-3d-fold",
  birth: "--knowledge-3d-birth",
  fly: "--knowledge-3d-fly",
  homeMs: "--knowledge-3d-home-ms",
  noticeMs: "--knowledge-3d-notice-ms",
  /* 평면 자리에서 세계로: 군집 반지름 = √쪽 × `world-root` × (쪽 / `world-pages`)^`world-spread`. */
  worldRoot: "--knowledge-3d-world-root",
  worldPages: "--knowledge-3d-world-pages",
  worldSpread: "--knowledge-3d-world-spread",
  /* 높이의 반폭 — 닿는 거리에 대한 몫(큰 군집, 작은 군집, 떠돌이 별). */
  liftGalaxy: "--knowledge-3d-lift-galaxy",
  liftCluster: "--knowledge-3d-lift-cluster",
  liftOrphan: "--knowledge-3d-lift-orphan",
  /* 별: 크기의 바탕 = ((1 + 연결) / (1 + 가운데값))^지수(천장 `star-ratio-cap`), 최근 = 오늘 1 → `age-span`일 0
   * (로그, 무릎 `age-scale`일). */
  starLumExp: "--knowledge-3d-star-lum-exp",
  starRatioCap: "--knowledge-3d-star-ratio-cap",
  starAgeScale: "--knowledge-3d-star-age-scale",
  starAgeSpan: "--knowledge-3d-star-age-span",
  /* 큰 군집(명판을 달고 필라멘트를 잇는 줄): 이름 있는 군집 중 순위 앞에서부터의 상한과 문턱(쪽 수 ≥
   * max(`galaxy-least`, `galaxy-share` × 쪽 / `world-pages`)). 색 칸이 없는 군집의 빛은 `quiet-share`만큼만. */
  galaxyMax: "--knowledge-3d-galaxy-max",
  galaxyLeast: "--knowledge-3d-galaxy-least",
  galaxyShare: "--knowledge-3d-galaxy-share",
  quietShare: "--knowledge-3d-quiet-share",
  /* 붐빔 — 화면에서 재는 수다(`KNOWLEDGE_UNIVERSE_CROWD`): 별의 속은 제 자리(제 군집의 별 사이 간격과 형제 사이 간격
   * 가운데 좁은 쪽)의 `crowd-ratio`를 넘지 않고, 더 줄 수 없는 만큼과 가지의 겹침(화면 1 px을 지나는 가지의 수가
   * `crowd-lines`를 넘는 몫)은 빛을 그 비의 `crowd-fade` 제곱으로 줄인다(바닥 `crowd-least`). 판이 작든 볼트가 크든
   * 군집은 덩어리가 되지 않고, 다가가면 별이 제 크기로 돌아온다. */
  crowdRatio: "--knowledge-3d-crowd-ratio",
  crowdLines: "--knowledge-3d-crowd-lines",
  crowdFade: "--knowledge-3d-crowd-fade",
  crowdLeast: "--knowledge-3d-crowd-least",
  /* 반지름: 큰 군집 = √쪽 × `world-root` × 몫 × `galaxy-radius`, 작은 군집 = √쪽 × 몫 × `cluster-sphere` ×
   * `cluster-grow` + `cluster-pad`. */
  galaxyRadius: "--knowledge-3d-galaxy-radius",
  clusterSphere: "--knowledge-3d-cluster-sphere",
  clusterGrow: "--knowledge-3d-cluster-grow",
  clusterPad: "--knowledge-3d-cluster-pad",
  /* 나무가 선 방향: 처음 시점 쪽으로 기울인 축(몫 `normal-lean`)에서 `tilt-least` + 씨앗 × `tilt-span` rad. */
  normalLean: "--knowledge-3d-normal-lean",
  tiltLeast: "--knowledge-3d-tilt-least",
  tiltSpan: "--knowledge-3d-tilt-span",
  /* 군집의 활동 — 최근 며칠 안에 고친 쪽의 몫(둘레 빛의 세기와 인스펙터의 한 줄). */
  activityDays: "--knowledge-3d-activity-days",
  /* 배경 별의 수. */
  decorSky: "--knowledge-3d-decor-sky",
  /* 군집의 빛깔 = `galaxy-tint-base` + (1 − base) × 선형(색 칸의 어두운 테마 값)을 색 없는 군집의 휘도로 맞춘 것. */
  galaxyTintBase: "--knowledge-3d-galaxy-tint-base",
  /* 필라멘트: 두 큰 군집 사이 링크가 몇 개부터인가, 굵은 차례로 몇 쌍까지인가. 그 밖의 쌍의 링크는 올리거나
   * 고를 때만 켜진다. */
  filamentLeast: "--knowledge-3d-filament-least",
  filamentMost: "--knowledge-3d-filament-most",
  /* 켜진 링크(올린 쪽 · 고른 쪽 · 경로)가 이 수를 넘으면 한 올의 빛을 (수의 비)^`lit-fade`로 줄인다 — 허브를 올려도
   * 판이 하얗게 날지 않는다. 줄어드는 바닥. */
  litFull: "--knowledge-3d-lit-full",
  litFade: "--knowledge-3d-lit-fade",
  litLeast: "--knowledge-3d-lit-least",
  /* 빛 번짐 사다리의 단 수(문턱 없음) — 세기와 합성의 노출은 테마마다 다르다(`KNOWLEDGE_UNIVERSE_THEMED`). 톤 매핑이
   * 색 비율을 지키는 몫(0은 채널마다의 ACES, 1은 가장 센 채널로 잰 ACES), 종이 위 먹빛에 싣는 색의 배수(휘도는 그대로). */
  bloomLevels: "--knowledge-3d-bloom-levels",
  toneKeep: "--knowledge-3d-tone-keep",
  inkChroma: "--knowledge-3d-ink-chroma",
  /* 이름표 — 들림이 이 값을 넘은 판에서만 선다. 명판: 군집의 화면 위 원(반지름의 `plate-ring`)에서 가장 낮은 곳 아래 틈만큼, 판 가장자리
   * 여백과 위·아래 띠, 둘레의 틈, 큰 명판을 입는 순위의 수, 가까이(처음 거리의 몫 밑) 가면 고른 것만. */
  labelLift: "--knowledge-3d-label-lift",
  plateRing: "--knowledge-3d-plate-ring",
  plateGap: "--knowledge-3d-plate-gap",
  plateEdge: "--knowledge-3d-plate-edge",
  plateTop: "--knowledge-3d-plate-top",
  plateBottom: "--knowledge-3d-plate-bottom",
  plateRoom: "--knowledge-3d-plate-room",
  plateLarge: "--knowledge-3d-plate-large",
  /* 「가까이」의 문턱 = max(처음 거리 × `close-in`, (군집으로 날아온 거리) × `close-in-fly`) — 날아온 자리는
   * 군집의 크기와 상관없이 가깝고, 조금 더 물러나면 먼 모습이다(디자이너 m-12642). */
  closeIn: "--knowledge-3d-close-in",
  closeInFly: "--knowledge-3d-close-in-fly",
  /* 쪽 이름: 풀, 쉴 때 이름을 다는 가장 큰 별의 수, 고른 군집 가까이의 이름 수, 쪽이 초점일 때의 이름 수, 후보 수와 화면 반지름 문턱, 초점의
   * 이웃·찾은 것의 덤, 후보를 고르는 위·아래 띠, 별에서 비킨 거리(반지름의 몫·최소·여백), 판 가장자리 여백, 놓는
   * 위·아래 띠, 한 줄의 높이. */
  labels: "--knowledge-3d-labels",
  labelsRest: "--knowledge-3d-labels-rest",
  labelsClose: "--knowledge-3d-labels-close",
  labelsFocus: "--knowledge-3d-labels-focus",
  labelsCandidates: "--knowledge-3d-labels-candidates",
  labelLeast: "--knowledge-3d-label-least",
  labelNeighbour: "--knowledge-3d-label-neighbour",
  labelHit: "--knowledge-3d-label-hit",
  labelScan: "--knowledge-3d-label-scan",
  labelOff: "--knowledge-3d-label-off",
  labelOffLeast: "--knowledge-3d-label-off-least",
  labelOffPad: "--knowledge-3d-label-off-pad",
  labelEdge: "--knowledge-3d-label-edge",
  labelTop: "--knowledge-3d-label-top",
  labelBottom: "--knowledge-3d-label-bottom",
  labelTall: "--knowledge-3d-label-tall",
  /* 필라멘트 수: 굵은 몇 쌍, 가운데 자리(t)와 화살이 가리키는 t, 한 줄의 높이·둘레의 틈, 화살 각의 걸음(1/rad). */
  flowTags: "--knowledge-3d-flow-tags",
  flowTagAt: "--knowledge-3d-flow-tag-at",
  flowTagAhead: "--knowledge-3d-flow-tag-ahead",
  /* 가운데 자리가 막히면 그 앞 · 뒤로 이만큼 옮긴 자리를 차례로 시험한다(디자이너 m-12642). */
  flowTagShift: "--knowledge-3d-flow-tag-shift",
  flowTagTall: "--knowledge-3d-flow-tag-tall",
  flowTagRoom: "--knowledge-3d-flow-tag-room",
  flowTagTurn: "--knowledge-3d-flow-tag-turn",
  /* 팁: 포인터에서 오른쪽·위로 비킨 자리, 판 오른쪽 끝에서 비울 넓이, 위의 최소, 둘레의 틈. */
  tipRight: "--knowledge-3d-tip-right",
  tipUp: "--knowledge-3d-tip-up",
  tipRoom: "--knowledge-3d-tip-room",
  tipTop: "--knowledge-3d-tip-top",
  tipGap: "--knowledge-3d-tip-gap",
});

/* 테마마다 다른 수 — 테마가 바뀌면 다시 읽는다(`knowledgeUniverseInks`): 합성의 노출, 빛 번짐의 세기, 별의 번짐의 몫.
 * 종이 위에서는 번짐이 빛이 아니라 얼룩이다 — 밝은 테마는 번짐 둘을 줄여 별 사이의 종이를 비운다. */
const KNOWLEDGE_UNIVERSE_THEMED = Object.freeze({
  exposure: "--knowledge-3d-exposure",
  bloom: "--knowledge-3d-bloom-strength",
  halo: "--knowledge-3d-halo",
});

/* 셰이더와 묶인 수 — 한 표. 군집은 텍스처 한 장의 줄이다(`texels` × `rows`, RGBA32F): 텍셀 0 = 우주의 가운데와
 * 반지름, 1 = 평면의 가운데와 반지름, 2 = 빛깔과 빛의 몫, 3 = 활동과 쪽 수. 떠돌이 별의 초점은 어느 줄도 아닌 번호다. */
const KNOWLEDGE_UNIVERSE_SHAPE = Object.freeze({
  texels: 4,
  rows: 128,
  noGalaxy: 999,
  /* 배경 별 가운데 먼 은하의 얼룩으로 그리는 수(시안의 48). */
  skySmudges: 48,
  /* 다발 한 올의 마디 수 — 4차 곡선을 이만큼의 곧은 조각으로 그린다. */
  fiberSegments: 24,
  /* 링크마다의 흐름 위상을 짓는 합동식(t-12443의 것 그대로 — 같은 볼트는 같은 위상이다). */
  lineSeed: 12345,
  lineMultiplier: 16807,
  lineModulus: 2147483647,
  /* 쪽 자리 텍스처 한 줄의 폭. */
  nodeWide: 1024,
  /* 이 깊이(w) 밑은 카메라 뒤다 — 투영하지 않는다. */
  behind: 0.1,
  /* 형제가 없는 별(군집의 허브 · 떠돌이 별)의 형제 사이 간격 — 어느 화면보다 넓다(`KNOWLEDGE_UNIVERSE_CROWD`). */
  roomy: 1e9,
});

/* 별의 크기 — 셰이더(`materials.stars`)와 고르기(`starSize`)가 같은 수를 읽는다. 속의 반지름(CSS px) =
 * clamp(`coreLeast` + `coreGrow` × 바탕^`corePower`, `coreMin`, `coreMost`) × clamp(기준 거리 / 거리, `nearLeast`,
 * `nearMost`). 올리거나 고른 별은 `hotGrow`배(적어도 `hotLeast`), 찾은 별은 적어도 `hitLeast`. 번짐의 반지름 = 속 ×
 * (`haloLeast` + `haloHub` × 허브의 몫) + `haloPad`. 허브의 몫 = smoothstep(`hubFrom`, `hubTo`, 바탕). 번짐의 세기 =
 * (`glow` + `glowHub` × 허브의 몫 + `glowFresh` × 최근²) × 테마의 몫(토큰 `halo`). 속의 흰빛 = `white` + `whiteHub` × 허브의 몫. 보통의 별은
 * `dim` + (1 − `dim`) × smoothstep(`dimFrom`, `dimTo`, 바탕)만큼 빛난다. 초점이 있는 판의 초점 밖 별은 `away`, 안의
 * 별은 `held`배이고 밖의 별은 빛깔도 `awayGrey`만큼 잿빛이다. 붐비는 자리의 별은 속이 제 자리의 몫(토큰
 * `crowd-ratio`)까지 작아지되 `crowdSize`배 밑으로는 줄지 않고, 번짐은 줄어든 몫만큼 여려진다 — 허브일수록 덜 주되
 * 제 형제 사이 간격은 넘지 않는다(`KNOWLEDGE_UNIVERSE_CROWD`). 가까이 가도 속은 `coreCeil` px, 점 하나의 반지름은
 * 기기 픽셀 `spriteMost`를 넘지 않는다(점 크기의 상한은 드라이버마다 다르다). 속의 가장자리는 `edge` px로 풀리고,
 * 번짐은 exp(-r² / (`haloFall` × 반지름²))로 여려져 `haloRim`부터 끝까지 0이다. 고른 별의 고리는 속 밖 `ringGap` px에
 * 폭 `ringWide`, 세기 `ringInk`, 흰빛 `ringWhite`; 찾은 별의 고리는 반지름의 `hitRing`에 폭 `hitWide`, 세기 `hitInk` ±
 * `hitBlink`(1초에 `hitBeat` rad). 고르기의 반지름은 max(속 × `pickShare`, `pickLeast` px), 겹치면 가까운 별이 먼저
 * (깊이 × `pickDepth`). */
const KNOWLEDGE_UNIVERSE_STAR = Object.freeze({
  coreLeast: 1.05,
  coreGrow: 1.1,
  corePower: 0.36,
  coreMin: 1,
  coreMost: 5.4,
  coreCeil: 10,
  spriteMost: 64,
  nearLeast: 0.45,
  nearMost: 1.6,
  hotGrow: 1.35,
  hotLeast: 3.4,
  hitLeast: 3,
  hitPad: 3,
  haloLeast: 2.1,
  haloHub: 2.6,
  haloPad: 2,
  hubFrom: 2,
  hubTo: 24,
  glow: 0.13,
  glowHub: 0.4,
  glowFresh: 0.34,
  white: 0.18,
  whiteHub: 0.3,
  dim: 0.62,
  dimFrom: 0.1,
  dimTo: 3,
  crowdSize: 0.5,
  away: 0.12,
  awayGrey: 0.7,
  held: 1.25,
  edge: 0.6,
  haloFall: 0.16,
  haloRim: 0.7,
  ringGap: 3,
  ringWide: 0.8,
  ringInk: 0.9,
  ringWhite: 0.7,
  hitRing: 0.8,
  hitWide: 1.2,
  hitInk: 0.72,
  hitBlink: 0.38,
  hitBeat: 4,
  pickShare: 1.6,
  pickLeast: 6,
  pickDepth: 0.0004,
});

/* 링크의 빛 — 가지(나무의 링크) · 현(나무에 들지 않은 군집 안의 링크와 떠돌이 별의 링크) · 올(군집 사이의 링크)이
 * 같은 수를 이 표에서 읽는다.
 *   가지: `branch` × 끝의 무게(부모 쪽 `trunkFrom` + `trunkGrow` × 줄기의 몫, 자식 쪽 `twigFrom` + `twigGrow` × 몫;
 *     줄기의 몫 = min(1, `trunkLeast` + (1 − `trunkLeast`) × √((딸린 쪽 − 1) / (군집의 쪽 × `trunkShare`)))).
 *   현: `chord` — 카메라가 처음 거리의 `closeNear`배 안에 들면 다 켜지고 `closeFar`배 밖에서는 꺼진다.
 *   올: `fiber` × 가늚(다발의 링크 n이 `fiberFull`을 넘으면 한 올이 (full / n)^`fiberFade`로 가늘다, 바닥 `fiberLeast` —
 *     굵은 다발일수록 밝되 판을 덮지 않는다), 두 끝에서 `fiberEnds`까지 여리다(`endsFrom`…`endsTo`). 링크가 `fiberMost`를 넘는
 *     다발은 고르게 뽑은 `fiberMost`올이 다발 전체의 빛을 나눠 든다(n × 가늚 / `fiberMost`) — 수백 올을 겹쳐 그리지 않는다.
 *     쉴 때 빛을 들지 않는 올(굵은 다발에 들지 않았거나 뽑히지 않은 올)은 꺼져 있고, 제 군집이 초점이면 `fiberFaint`로 켜진다.
 *   켜진 링크(올린 쪽 · 고른 쪽의 링크, 경로): `lit` × 판의 줄임(`lit-full`), 흰빛 `litWhite` — 추론된 링크는 `inferred`배, 경로는
 *     `path`배. 빛의 머리는 링크가 가리키는 쪽으로 `pulseSpeed`(1/s)로 달리고 꼬리는 길이의 `pulseTail`, 세기는
 *     `pulse`배다. 다발의 올은 쉴 때도 `fiberPulse`배의 머리가 흐른다(`fiberSpeed`).
 *   초점: 군집이 초점이면 그 군집의 것은 `focusOn`배, 남의 것은 `focusOff`배. 쪽이 초점이면(올림 · 고름) 그 쪽의
 *     군집의 가지와 현만 `focusHome`배로 남고 다발은 전부 `focusOff`배다 — 켜진 링크가 먼저 읽힌다. 군집 없는
 *     초점(찾기 · 시간 창)은 전부 `focusRest`배.
 *   거리: 빛은 clamp(기준 거리 / 거리, `nearLeast`, `nearMost`)배, 카메라 코앞(`veilFrom`…`veilTo`)의 링크는 걷힌다. */
const KNOWLEDGE_UNIVERSE_EDGE = Object.freeze({
  branch: 0.19,
  trunkFrom: 0.55,
  trunkGrow: 0.9,
  twigFrom: 0.16,
  twigGrow: 0.7,
  trunkLeast: 0.35,
  trunkShare: 0.2,
  chord: 0.04,
  closeNear: 0.22,
  closeFar: 0.5,
  fiber: 0.034,
  fiberFull: 24,
  fiberFade: 0.55,
  fiberLeast: 0.18,
  fiberMost: 64,
  fiberEnds: 0.1,
  endsFrom: 0.02,
  endsTo: 0.3,
  fiberFaint: 0.012,
  fiberPulse: 0.3,
  fiberSpeed: 0.11,
  fiberTail: 0.16,
  lit: 0.7,
  litWhite: 0.35,
  inferred: 0.5,
  path: 1.6,
  pulse: 1.4,
  pulseSpeed: 0.34,
  pulseTail: 0.3,
  focusOn: 1.25,
  focusOff: 0.14,
  focusHome: 0.7,
  focusRest: 0.4,
  nearLeast: 0.5,
  nearMost: 1.6,
  veilFrom: 4,
  veilTo: 22,
});

/* 군집 둘레의 빛 — 군집마다 카메라를 보는 사각형 하나: 반지름 = 군집 × `reach`, 세기 = (`base` + `active` × 활동) ×
 * 빛의 몫, 가운데에서 `fall`로 여려지고 `rimFrom`부터 끝까지 0으로. 초점 군집은 `focusOn`배, 남은 `focusOff`배. */
const KNOWLEDGE_UNIVERSE_AURA = Object.freeze({
  reach: 1.5,
  base: 0.026,
  active: 0.04,
  fall: 2.6,
  rimFrom: 0.72,
  focusOn: 1,
  focusOff: 0.3,
});

/* 군집의 나무(`knowledgeUniverseBurst`) — 허브에서 너비 우선으로 퍼진다. 가지의 길이 = `decay`^깊이 × (`leaf` +
 * `leafLinks` × log2(1 + 그 쪽의 연결) + `branch` × √((딸린 쪽 − 1) / 군집의 쪽)) × 흔들림(`wobbleLeast`…+`wobbleSpan`).
 * 허브의 자식은 온 구에, 그 밖의 자식은 부모가 온 방향을 축으로 한 원뿔 안에 황금각으로 선다 — 원뿔의 반각 =
 * min(`capMost`, `capLeast` + `capGrow` × √자식 수). 다 놓은 뒤 가장 먼 쪽이 반지름의 `fill`에 닿게 맞춘다. 씨앗의
 * 덧수 둘(자식들이 도는 위상, 길이의 흔들림)은 쪽의 열쇠에 더한다 — 쪽 하나가 늘어도 남의 가지가 돌지 않는다. */
const KNOWLEDGE_UNIVERSE_BURST = Object.freeze({
  decay: 0.56,
  leaf: 0.26,
  leafLinks: 0.07,
  branch: 1,
  wobbleLeast: 0.85,
  wobbleSpan: 0.3,
  capLeast: 0.4,
  capGrow: 0.13,
  capMost: 1.3,
  fill: 0.96,
  seedPhase: 7,
  seedWobble: 31,
});

/* 다발의 길 — 올의 셰이더와 이름표 자리(`knowledgeUniverseFilamentPoint`)가 같은 수를 이 표에서 읽는다. 한 올은
 * 다섯 점의 4차 곡선이다: 제 별, 제 군집의 문, 다발의 허리, 저쪽 군집의 문, 저쪽 별. 문은 가운데에서 저쪽
 * 군집 쪽으로 min(반지름 × `gate`, 사이 거리 × `gateMost`)만큼 나간 자리이고, 허리는 두 문의 가운데에서 옆으로
 * 길이 × `side` × 휨(다섯 단계), 위로 길이 × `up`만큼 휜다. 거의 수직인 쌍(옆 방향의 길이 < `upright`)의 옆은
 * x축이다. 휨의 단계와 흐름의 위상은 쌍의 두 줄 번호에서 짓는다(t-12443의 수). */
const KNOWLEDGE_UNIVERSE_CURVE = Object.freeze({
  gate: 0.95,
  gateMost: 0.3,
  side: 0.1,
  up: 0.05,
  upright: 0.001,
  bendLow: 7,
  bendHigh: 3,
  bendSteps: 5,
});

/* 첫 그림 — 문을 연 판에서 별들이 제 군집의 가운데(허브)에서 제 자리로 퍼진다(`uBirth` 0 → 1, 길이는 토큰
 * `birth`). 군집마다 줄 번호의 해시 × `stagger`만큼 늦게 떠나고, 떠난 뒤에는 세제곱으로 느려지며 앉는다. 링크와 둘레
 * 빛은 `ink`배 빠르게 켜진다. 평면에서 떠오르는 판은 떠오름이 그 일을 한다. */
const KNOWLEDGE_UNIVERSE_BIRTH = Object.freeze({
  stagger: 0.6,
  ink: 1.5,
});

/* 붐빔 — 별 하나의 자리(CSS px)는 둘 가운데 좁은 쪽이다.
 *   군집의 간격: 군집 안에서 별 하나가 차지하는 화면의 한 변 = √(π r² / 쪽), r = 군집의 화면 반지름(반지름 × 초점 거리 /
 *     군집 가운데의 깊이). 카메라가 군집 안으로 들어가면 가운데의 깊이는 반지름의 `near`배로 친다.
 *   형제 사이 간격(`map.room` — 나무가 자리를 놓을 때 적는다, `knowledgeUniverseBurst`): 부모의 자식 k가 길이 ℓ의 갓
 *     (반각 θ) 위에 고르게 서므로, 앞에서 본 갓의 원반(π ℓ² sin²θ — 허브의 자식은 온 구라 π ℓ²)을 k로 나눈 한 변 =
 *     ℓ sinθ √(π / k). 화면에서는 × 초점 거리 / 그 별의 깊이. 형제가 없는 별(군집의 허브 · 떠돌이 별)은 `roomy`다.
 * 셰이더(`gapOf` · `aRoom`)와 고르기 · 이름표(`rowGaps` · `starSize`)가 같은 식을 쓴다. 자리로 재는 셋:
 *   별: 속의 반지름 ≤ 자리 × `crowd-ratio`(토큰). 줄어드는 몫은 `KNOWLEDGE_UNIVERSE_STAR.crowdSize`가 바닥이고, 그 바닥에
 *     걸려 더 줄지 못한 만큼은 빛으로 줄인다.
 *   허브: 연결이 많은 별은 제 크기를 지키되 속의 반지름이 형제 사이 간격의 `hub`배를 넘지 않는다 — 허브가 많은 군집에서
 *     허브끼리 겹쳐 한 덩어리가 되지 않는다. 군집의 허브(부모 없는 쪽)는 언제나 제 크기다.
 *   가지: 화면 1 px을 지나는 가지의 수 ∝ √(π × 쪽) / 군집의 간격. 이 수가 `crowd-lines`(토큰)를 넘는 몫만큼 잔가지의
 *     빛이 준다 — 줄기(딸린 쪽이 많은 가지)는 줄지 않는다.
 * 빛을 맞춘 장면(2,650쪽)에서 잰 수: 판 1,365 × 1,137의 군집 간격은 약 10 px이고 별의 57%에서 자리의 몫이 바닥 밑이다.
 * 옆 패널이 열린 판(735 × 602)은 약 5.4 px · 93%다 — 멀리서는 작은 점이고, 다가가면 제 크기로 돌아온다. */
const KNOWLEDGE_UNIVERSE_CROWD = Object.freeze({
  near: 0.5,
  hub: 0.5,
});

/* 선형 빛의 휘도 무게(sRGB 원색, Rec. 709) — 군집 빛깔의 밝기를 맞출 때(`knowledgeUniverseTint`)와 합성 셰이더가
 * 종이 위 먹빛의 색만 벌리고 휘도는 그대로 둘 때 같은 무게를 읽는다. */
const KNOWLEDGE_UNIVERSE_LUMA = Object.freeze([0.2126, 0.7152, 0.0722]);
const KNOWLEDGE_UNIVERSE_LUMA_GLSL = `vec3(${KNOWLEDGE_UNIVERSE_LUMA.map(knowledgeUniverseFloat).join(", ")})`;

/* 쪽 이름 후보의 차례 — 초점의 별은 언제나 먼저, 쉴 때의 큰 별은 제 차례대로. */
const KNOWLEDGE_UNIVERSE_LABEL_FIRST = Object.freeze({ focus: 1e6, bright: 1e5 });

/* 무대 위에 늘 서 있는 상자 — 이름표는 이 밑에 서지 않는다. 앱에서는 배율 상자와 열린 범례이고, 놓는
 * 프레임마다 잰다. */
const KNOWLEDGE_UNIVERSE_COVERS = Object.freeze([".knowledge-zoom", ".knowledge-legend"]);

/* 셰이더에 싣는 수 — GLSL은 정수 글자를 실수로 읽지 않으므로 소수점을 붙인다. */
function knowledgeUniverseFloat(value) {
  return Number.isInteger(value) ? value.toFixed(1) : String(value);
}

/* 표 하나를 셰이더의 `#define`으로 — 이름은 머리말 + 대문자 열쇠다(`STAR_CORE_LEAST`). 셰이더의 글자에는 수가
 * 없고 이 이름만 있다: 표를 고치면 그림과 고르기가 함께 바뀐다. */
function knowledgeUniverseDefines(prefix, table) {
  return Object.entries(table).map(([key, value]) =>
    `#define ${prefix}_${key.replace(/[A-Z]/gu, (letter) => `_${letter}`).toUpperCase()} ${knowledgeUniverseFloat(value)}`)
    .join("\n");
}

/* 인스펙터의 우주 절: 필라멘트와 큰 별 몇 줄, 「얼마 전」의 문턱(하루 밑은 오늘, 두 주 밑은 날, 두 달 밑은 주,
 * 한 해 밑은 달, 그 위는 해). */
const KNOWLEDGE_UNIVERSE_LISTED = 6;
const KNOWLEDGE_UNIVERSE_AGE = Object.freeze({ today: 1, days: 14, weeks: 60, months: 365, week: 7, month: 30, year: 365 });

/* 판마다 한 번 읽는 수 — 테마와 무관하다(색은 따로 읽는다). */
const knowledgeUniverseTunings = new WeakMap();
function knowledgeUniverseTuning(view) {
  const held = knowledgeUniverseTunings.get(view);
  if (held !== undefined) return held;
  const style = getComputedStyle(view);
  const tuning = Object.freeze(Object.fromEntries(Object.entries(KNOWLEDGE_UNIVERSE_TOKENS)
    .map(([key, name]) => [key, Number.parseFloat(style.getPropertyValue(name))])));
  knowledgeUniverseTunings.set(view, tuning);
  return tuning;
}

/* 사람이 고른 전체 지도의 얼굴 — `null`은 고른 적이 없다는 뜻이다(표의 기본 줄). 문을 열 때 이
 * 볼트가 기억하는 쪽을 읽는다(`applyKnowledgeEntry`). */
let knowledgeDimension = null;
/* 판마다 우주 하나. 판이 사라지면 함께 사라진다. */
const knowledgeUniverses = new WeakMap();
/* 우주를 짓다가 무너진 창(셰이더가 서지 않는 드라이버) — 이 창의 남은 시간 동안 3D는 까닭을 말하고
 * 서지 않는다. */
let knowledgeUniverseFailed = false;
/* 사람이 3D를 누른 판 — 다음에 서는 우주는 평면 자리에서 떠오른다(z가 나는 전환). 문을 연 판은
 * 곧바로 처음 자리에 선다. */
let knowledgeUniverseRising = false;
/* 사람의 「움직임」(시안 `setMotion`, 디자이너 m-12467의 5) — 쉬고 9초 뒤의 느린 돌기와 링크를 따라 흐르는 빛이
 * 한 단추에 묶인다. 이 창에서 한 번 고르면 다음에 서는 우주도 따른다. 움직임을 줄이라는 판에서는 늘 멈춘다. */
let knowledgeUniverseMotion = true;

/* 이 창이 우주를 그릴 수 있는가 — WebGL2와 렌더러(`knowledgeGlSupported`). */
function knowledgeUniverseAble() {
  return !knowledgeUniverseFailed && knowledgeGlSupported();
}

/* 지금 전체 지도가 서야 할 얼굴. 고른 것, 없으면 표의 기본 — 우주를 그릴 수 없는 판에서는 평면이다.
 * 그림의 크기는 읽지 않는다: 볼트의 크기가 사람의 화면을 바꾸지 않는다(설계 knowledge-graph-20260914). */
function knowledgeDimensionNow() {
  const wanted = knowledgeDimension ?? KNOWLEDGE_DIMENSION_FIRST;
  return wanted === "3d" && knowledgeUniverseAble() ? "3d" : "2d";
}

/* 이 판에 우주가 서야 하는가. */
function knowledgeUniverseWanted(view, layout) {
  return knowledgeMode === KNOWLEDGE_MODES[0].id && knowledgeDimensionNow() === "3d"
    && layout !== undefined && layout.count > 0 && !view.hidden;
}

/* 우주가 평면 지도를 덮고 있는가 — 서 있고(`is-universe`: 떠오르는 동안과 내려앉는 동안도 평면의 층은 가려져
 * 있다) 문맥을 쥐고 있는 동안. 그동안 평면의 손은 그리지 않고 그리기 버퍼를 돌려준다(`paintKnowledgeFrame`). */
function knowledgeUniverseCovers(view) {
  const universe = knowledgeUniverses.get(view);
  return universe !== undefined && !universe.lost && view.classList.contains("is-universe");
}

/* 머리의 「2D | 3D」. 눌린 쪽은 지금 서 있는 얼굴이다(주변 탐색은 평면). 우주를 그릴 수 없는 판에서
 * 3D는 누를 수 없는 채로 서고(`aria-disabled` — 초점과 손은 받아 팁이 까닭을 말한다), 문맥을 잃은
 * 판에서는 누를 수 있다(돌아오면 연다고 말한다). */
function paintKnowledgeDimension(view) {
  const shown = knowledgeMode === KNOWLEDGE_MODES[0].id ? knowledgeDimensionNow() : "2d";
  const able = knowledgeUniverseAble();
  for (const step of view.querySelectorAll("[data-knowledge-dimension]")) {
    const id = step.dataset.knowledgeDimension;
    const on = id === shown;
    writeAttribute(step, "aria-pressed", String(on));
    step.classList.toggle("is-active", on);
    const row = KNOWLEDGE_DIMENSIONS.find((one) => one.id === id);
    if (id !== "3d" || row === undefined) continue;
    writeAttribute(step, "aria-disabled", String(!able));
    const words = able ? t(row.key, row.word)
      : t("knowledge.dimensionUnable", "이 창에서는 우주를 그릴 수 없어 평면 지도로 봅니다");
    writeAttribute(step, "aria-label", words);
    writeAttribute(step, "data-tip", words);
  }
}

/* 사람이 얼굴을 골랐다. 이 볼트의 기억에 적고(`noteKnowledgeExplore`), 3D면 전체 지도로 나와 평면
 * 자리에서 떠오르고, 2D면 우주가 평면 자세로 내려앉은 뒤 놓인다. */
function setKnowledgeDimension(view, id) {
  if (!KNOWLEDGE_DIMENSIONS.some((row) => row.id === id)) return;
  if (id === "3d" && !knowledgeUniverseAble()) return;
  const layout = knowledgeLayouts.get(view);
  const universe = knowledgeUniverses.get(view);
  knowledgeDimension = id;
  if (layout !== undefined) noteKnowledgeExplore(layout.model.vault);
  if (id === "3d") {
    if (universe?.lost) {
      universe.say(t("knowledge.universeWaiting", "그래픽 장치를 기다리는 중입니다 — 돌아오면 우주로 엽니다"));
      paintKnowledgeDimension(view);
      return;
    }
    knowledgeUniverseRising = true;
    if (knowledgeMode !== KNOWLEDGE_MODES[0].id) {
      setKnowledgeMode(view, KNOWLEDGE_MODES[0].id);
      return;
    }
    void paintKnowledgeView();
    return;
  }
  paintKnowledgeDimension(view);
  if (universe === undefined) return;
  universe.fold(() => leaveKnowledgeUniverse(view));
}

/* 우주를 놓는다 — 판의 옷을 평면으로 되돌리고 GPU의 것을 전부 놓는다. */
function leaveKnowledgeUniverse(view) {
  const universe = knowledgeUniverses.get(view);
  knowledgeUniverses.delete(view);
  view.classList.remove("is-universe");
  universe?.dispose();
  const layout = knowledgeLayouts.get(view);
  if (layout === undefined) return;
  paintKnowledgeUniverseInspector(view, layout);
  paintKnowledgeFrame(view, layout);
}

/* 그림의 문(`paintKnowledgeView`)이 끝에 부른다: 우주가 서야 하면 세우고(처음이면 짓는다), 아니면
 * 놓는다. 세운 우주는 위상이나 가림(접은 목차 · 일지)이 바뀐 판에서 다시 짓고, 그 밖에는 옷(고름·밝힘·찾기)만 고친다. */
function paintKnowledgeUniverse(view, layout) {
  paintKnowledgeDimension(view);
  const wanted = knowledgeUniverseWanted(view, layout);
  let universe = knowledgeUniverses.get(view);
  if (!wanted) {
    knowledgeUniverseRising = false;
    if (universe !== undefined && !universe.folding) leaveKnowledgeUniverse(view);
    return;
  }
  if (universe === undefined) {
    universe = makeKnowledgeUniverse(view);
    if (!universe.mount()) {
      universe.dispose();
      knowledgeUniverseFailed = true;
      knowledgeUniverseRising = false;
      paintKnowledgeDimension(view);
      return;
    }
    knowledgeUniverses.set(view, universe);
    if (!universe.build(layout)) {
      knowledgeUniverses.delete(view);
      universe.dispose();
      knowledgeUniverseFailed = true;
      knowledgeUniverseRising = false;
      paintKnowledgeDimension(view);
      return;
    }
    universe.enter(knowledgeUniverseRising);
    knowledgeUniverseRising = false;
    paintKnowledgeUniverseInspector(view, layout);
    return;
  }
  if ((universe.layout !== layout || universe.seenStamp !== layout.drawnStamp) && !universe.build(layout)) {
    leaveKnowledgeUniverse(view);
    knowledgeUniverseFailed = true;
    paintKnowledgeDimension(view);
    return;
  }
  /* 내려앉던 우주를 다시 불렀다(2D를 누르고 곧바로 3D) — 떠나보내지 않고 그 자리에서 다시 선다. */
  universe.unfold();
  knowledgeUniverseRising = false;
  universe.dress();
  paintKnowledgeUniverseInspector(view, layout);
}

/* 옷이 바뀐 순간(고름·밝힘·찾기·군집) — 평면의 프레임 문(`paintKnowledgeFrame`)이 부른다. 서 있는
 * 우주는 다음 프레임 한 장으로 따라온다. */
function dressKnowledgeUniverse(view) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined) return;
  universe.dress();
  const layout = knowledgeLayouts.get(view);
  if (layout !== undefined) paintKnowledgeUniverseInspector(view, layout);
}

/* 우주 안의 배율 단추 셋 — 서 있는 우주가 받으면 참이다(평면의 단추 손은 건너뛴다). */
function knowledgeUniverseZoom(view, what) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.lost || universe.folding) return false;
  if (what === "fit") universe.goHome();
  else universe.zoomStep(what === "in" ? 1 / universe.tune.zoomStep : universe.tune.zoomStep);
  return true;
}

/* 캔버스의 키 — 서 있는 우주는 화살표로 돈다(설계 §4-e). 받으면 참이다. */
function knowledgeUniverseKey(view, event) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.lost || universe.folding) return false;
  const turn = universe.tune.keyTurn;
  const turns = { ArrowLeft: [turn, 0], ArrowRight: [-turn, 0], ArrowUp: [0, -turn], ArrowDown: [0, turn] };
  const way = turns[event.key];
  if (way === undefined) return false;
  event.preventDefault();
  universe.turn(way[0], way[1]);
  return true;
}

/* 고친 때를 「얼마 전」으로(시안 `ageText`) — 말은 로케일의 상대 시각이다. */
function knowledgeUniverseAgo(ms, nowMs) {
  const A = KNOWLEDGE_UNIVERSE_AGE;
  const days = Math.max(0, (nowMs - ms) / 86_400_000);
  const words = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (days < A.today) return words.format(0, "day");
  if (days < A.days) return words.format(-Math.round(days), "day");
  if (days < A.weeks) return words.format(-Math.round(days / A.week), "week");
  if (days < A.months) return words.format(-Math.round(days / A.month), "month");
  return words.format(-Math.round((days / A.year) * 10) / 10, "year");
}

/* 인스펙터의 우주 절 — 우주가 서 있는 동안만: 가장 굵은 필라멘트(보낸 군집 → 받는 군집) 여섯, 그리고 고른 군집의
 * 쪽 수와 나무의 꼴(허브에 바로 이어진 쪽 · 그 가운데 가지를 친 쪽 · 가장 먼 걸음) · 최근 두 주에 고친 몫, 그 군집의 필라멘트와 큰 별. 「아름다운
 * 3D, 글자로는 0」이라는 비판에 대한 글자의 답이다(디자이너 m-12467의 6). 줄은 단추이고 누르면 그 흐름으로 난다
 * (`knowledgeUniverseFlow`). */
function paintKnowledgeUniverseInspector(view, layout) {
  const overview = view.querySelector(".knowledge-overview");
  if (overview === null) return;
  const universe = knowledgeUniverses.get(view);
  const map = universe?.map ?? null;
  let flows = overview.querySelector(".knowledge-overview-filaments");
  let galaxy = overview.querySelector(".knowledge-overview-galaxy");
  if (flows === null) {
    flows = knowledgeListSection("knowledge-overview-filaments", "knowledge.filaments",
      t("knowledge.filaments", "가장 굵은 필라멘트 (보낸 군집 → 받는 군집)"));
    galaxy = document.createElement("section");
    galaxy.className = "knowledge-overview-galaxy";
    const head = document.createElement("h4");
    head.className = "knowledge-inspector-head";
    const about = document.createElement("p");
    about.className = "knowledge-overview-galaxy-about";
    /* 군집의 두 목록: 그 군집의 굵은 필라멘트, 그리고 큰 별 — 「어느 쪽이 중심인가」를 글자로 답하는 줄(디자이너
     * m-12627). 별 줄을 누르면 그 쪽이 골라진다(인스펙터의 `[data-knowledge-key]` 손). */
    galaxy.append(head, about,
      knowledgeListSection("knowledge-overview-galaxy-flows", "knowledge.galaxyFilaments", t("knowledge.galaxyFilaments", "굵은 필라멘트")),
      knowledgeListSection("knowledge-overview-galaxy-stars", "knowledge.brightStars", t("knowledge.brightStars", "큰 별")));
    overview.querySelector(".knowledge-overview-counts")?.after(galaxy, flows);
  }
  const rank = knowledgeClusterPicked;
  flows.hidden = map === null || map.filaments.length === 0;
  galaxy.hidden = map === null || rank < 0 || rank >= map.rows;
  if (map === null) return;
  const most = map.filaments[0]?.n ?? 1;
  const row = (one, at) => {
    const line = knowledgeListRow(`flow:${at}`, t("knowledge.flowPair", "{{from}} → {{to}}", {
      from: knowledgeClusterWord(layout, one.from), to: knowledgeClusterWord(layout, one.to) }),
    `${one.sent}/${one.n}`, one.n / most);
    line.querySelector("button").dataset.knowledgeFlow = `${one.from},${one.to}`;
    return line;
  };
  reconcileElementOrder(flows.querySelector(".knowledge-inspector-list"),
    map.filaments.slice(0, KNOWLEDGE_UNIVERSE_LISTED).map(row));
  if (galaxy.hidden) return;
  const picked = map.galaxy[rank];
  writeTextContent(galaxy.querySelector(".knowledge-inspector-head"), t("knowledge.galaxyAbout",
    "{{name}} · {{pages}}쪽", { name: knowledgeClusterWord(layout, rank), pages: layout.communitySize[rank] }));
  writeTextContent(galaxy.querySelector(".knowledge-overview-galaxy-about"), t("knowledge.galaxyRecent",
    "허브에 바로 이어진 쪽 {{direct}}(가지를 친 쪽 {{branches}}) · 가장 먼 쪽까지 {{steps}}걸음 · 최근 2주에 고친 쪽 {{percent}}",
    { direct: picked.direct, branches: picked.branches, steps: picked.deepest,
      percent: `${Math.round(picked.activity * 100)}%` }));
  const own = map.filaments.filter((one) => one.from === rank || one.to === rank).slice(0, KNOWLEDGE_UNIVERSE_LISTED);
  const ownFlows = galaxy.querySelector(".knowledge-overview-galaxy-flows");
  ownFlows.hidden = own.length === 0;
  reconcileElementOrder(ownFlows.querySelector(".knowledge-inspector-list"), own.map(row));
  const model = layout.model;
  const brightest = map.members[rank].slice(0, KNOWLEDGE_UNIVERSE_LISTED);
  const brightMost = model.degree[brightest[0]] || 1;
  reconcileElementOrder(galaxy.querySelector(".knowledge-overview-galaxy-stars .knowledge-inspector-list"),
    brightest.map((at) => knowledgeListRow(model.keys[at], model.titles[at], String(model.degree[at]),
      model.degree[at] / brightMost)));
}

/* 필라멘트 줄을 눌렀다: 보낸 군집을 고르고 그 흐름을 옆에서 보는 자리로 난다. */
function knowledgeUniverseFlow(view, from, to) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.map === null) return;
  if (knowledgeClusterPicked !== from) toggleKnowledgeCluster(view, from);
  universe.flyToFlow(from, to);
}

/* 「보기」 메뉴의 우주 줄 — 우주가 서 있는 동안만 보인다(CSS `.knowledge-view.is-universe`). 「움직임」 하나. */
function buildKnowledgeUniverseControls() {
  const box = document.createElement("div");
  box.className = "knowledge-universe-controls";
  box.setAttribute("role", "group");
  box.dataset.i18nAria = "knowledge.universeControls";
  box.setAttribute("aria-label", t("knowledge.universeControls", "우주"));
  const motion = document.createElement("button");
  motion.type = "button";
  motion.className = "btn knowledge-flag knowledge-universe-motion";
  motion.classList.toggle("is-active", knowledgeUniverseMotion);
  motion.setAttribute("aria-pressed", String(knowledgeUniverseMotion));
  motion.dataset.i18n = "knowledge.universeMotion";
  motion.textContent = t("knowledge.universeMotion", "움직임");
  motion.dataset.i18nTitle = "knowledge.universeMotionTip";
  motion.dataset.tip = t("knowledge.universeMotionTip", "쉬는 동안 천천히 돌고 링크를 따라 빛이 흐릅니다");
  motion.onclick = () => {
    const view = motion.closest(".knowledge-view");
    if (view !== null) setKnowledgeUniverseMotion(view, !knowledgeUniverseMotion);
  };
  box.appendChild(motion);
  return box;
}

function setKnowledgeUniverseMotion(view, on) {
  knowledgeUniverseMotion = on;
  const button = view.querySelector(".knowledge-universe-motion");
  if (button !== null) {
    writeAttribute(button, "aria-pressed", String(on));
    button.classList.toggle("is-active", on);
  }
  knowledgeUniverses.get(view)?.setMotion(on);
}

/* 필라멘트 위의 한 점 — 올의 셰이더가 그리는 다발의 등뼈(두 군집의 가운데를 두 끝으로 한 4차 곡선)와 같은 식이라
 * 필라멘트 수가 그 빛 위에 선다. `share`는 0(보낸 군집) → 1(받는 군집). */
function knowledgeUniverseFilamentPoint(one, two, bend, share) {
  const C = KNOWLEDGE_UNIVERSE_CURVE;
  const dx = two.x - one.x;
  const dy = two.y - one.y;
  const dz = two.z - one.z;
  const length = Math.max(Math.hypot(dx, dy, dz), 1);
  const ux = dx / length;
  const uy = dy / length;
  const uz = dz / length;
  let sx = -uz;
  let sz = ux;
  const across = Math.hypot(sx, sz);
  if (across < C.upright) {
    sx = 1;
    sz = 0;
  } else {
    sx /= across;
    sz /= across;
  }
  const qx = -sz * uy;
  const qy = sz * ux - sx * uz;
  const qz = sx * uy;
  const leave = Math.min(one.radius * C.gate, length * C.gateMost);
  const arrive = Math.min(two.radius * C.gate, length * C.gateMost);
  const side = bend * (one.rank < two.rank ? 1 : -1) * length * C.side;
  const up = length * C.up;
  const gates = [[one.x + ux * leave, one.y + uy * leave, one.z + uz * leave],
    [two.x - ux * arrive, two.y - uy * arrive, two.z - uz * arrive]];
  const waist = [(gates[0][0] + gates[1][0]) / 2 + sx * side + qx * up, (gates[0][1] + gates[1][1]) / 2 + qy * up,
    (gates[0][2] + gates[1][2]) / 2 + sz * side + qz * up];
  const rest = 1 - share;
  const weights = [rest ** 4, 4 * rest ** 3 * share, 6 * rest ** 2 * share ** 2, 4 * rest * share ** 3, share ** 4];
  const points = [[one.x, one.y, one.z], gates[0], waist, gates[1], [two.x, two.y, two.z]];
  return [0, 1, 2].map((axis) => points.reduce((sum, point, at) => sum + point[axis] * weights[at], 0));
}

/* 군집의 가장 굵은 필라멘트 — 명판의 둘째 줄: 이 군집이 더 많이 보냈으면 →, 받았으면 ←. */
function knowledgeUniverseStrongest(map, row) {
  let best = null;
  for (const one of map.filaments) {
    if (one.a !== row && one.b !== row) continue;
    const sent = one.a === row ? one.ab : one.ba;
    const taken = one.a === row ? one.ba : one.ab;
    if (best === null || one.n > best.n) best = { other: one.a === row ? one.b : one.a, n: one.n, out: sent >= taken };
  }
  return best;
}

/* 범례의 우주 줄 — 우주가 서 있는 동안만 보이고 평면의 줄은 가려진다(CSS `.knowledge-view.is-universe`). 표본은
 * 그림의 기호 그대로다: 별의 크기, 최근 고친 쪽의 번짐, 가지, 필라멘트, 올린 쪽의 링크. */
const KNOWLEDGE_UNIVERSE_LEGEND = Object.freeze([
  Object.freeze({ marks: ["star"], words: [{ key: "knowledge.legendBrightness", word: "별의 크기 = 연결 수" }] }),
  Object.freeze({ marks: ["fresh"], words: [{ key: "knowledge.legendRecent", word: "빛이 번질수록 최근에 고친 쪽" }] }),
  Object.freeze({ marks: ["branch"], words: [{ key: "knowledge.legendBranch", word: "가지 = 허브에서 이어진 링크" }] }),
  Object.freeze({ marks: ["filament"],
    words: [{ key: "knowledge.legendFilament", word: "필라멘트 = 군집 사이 링크의 다발 · 빛은 링크가 가리키는 쪽으로" }] }),
  Object.freeze({ marks: ["solid", "faint"], words: [{ key: "knowledge.legendPointed", word: "올린 쪽의 링크" },
    { key: "knowledge.legendInferred", word: "추론" }] }),
]);

function appendKnowledgeUniverseLegend(list) {
  for (const row of KNOWLEDGE_UNIVERSE_LEGEND) {
    const item = document.createElement("li");
    item.dataset.legendUniverse = "true";
    row.marks.forEach((kind, at) => {
      const mark = document.createElement("span");
      mark.className = `knowledge-legend-universe kind-${kind}`;
      mark.setAttribute("aria-hidden", "true");
      const said = document.createElement("span");
      said.dataset.i18n = row.words[at].key;
      said.textContent = t(row.words[at].key, row.words[at].word);
      item.append(mark, said);
    });
    list.appendChild(item);
  }
}

/* ---- 셰이더 ---------------------------------------------------------------------------
 *
 * 재질은 `ShaderMaterial`이다 — three.js가 WebGL2에서 GLSL 300 es로 옮겨 싣는다(`attribute`·`varying`·
 * `gl_FragColor`를 제 머리의 정의로). 모양의 수는 글자에 없다: 표(`KNOWLEDGE_UNIVERSE_STAR` · `_EDGE` · `_AURA` ·
 * `_CURVE`)가 `#define`으로 들어가고(`knowledgeUniverseDefines`), 고르기와 이름표 자리가 같은 표를 읽는다. 배경 별과
 * 후처리(빛 번짐 · 톤 매핑)는 t-12443이 시안에서 옮긴 글자 그대로다.
 *
 * 섞기는 미리 곱한 알파다(ONE, ONE_MINUS_SRC_ALPHA): 모든 빛이 α = 0으로 더해진다 — 깊이도 정렬도 없다. */

/* 군집 텍셀 읽기 · 정수 해시 · 군집의 별 사이 간격(`gapOf` — `KNOWLEDGE_UNIVERSE_CROWD`, `uCrowd` = 토큰의 ratio ·
 * lines · fade · least) · 첫 그림의 자리(`born` — 제 군집의 가운데에서 제 자리로) · 배경 별의 색(Wikipedia 분광형 표의
 * D65 색을 선형으로). */
const KNOWLEDGE_UNIVERSE_GAL = `
${knowledgeUniverseDefines("BIRTH", KNOWLEDGE_UNIVERSE_BIRTH)}
${knowledgeUniverseDefines("CROWD", KNOWLEDGE_UNIVERSE_CROWD)}
uniform highp sampler2D tGal;
vec4 gal(int c, int t){ return texelFetch(tGal, ivec2(t, c), 0); }
uint hu(uint x){ x ^= x >> 16u; x *= 0x7feb352du; x ^= x >> 15u; x *= 0x846ca68bu; x ^= x >> 16u; return x; }
float hf(uint s){ return float(hu(s) >> 8u) * (1.0 / 16777216.0); }
uniform float uFocal; uniform vec4 uCrowd;
float gapOf(int c, float lift){
  vec4 G0 = gal(c, 0);
  float w = max((projectionMatrix * viewMatrix * vec4(mix(gal(c, 1).xyz, G0.xyz, lift), 1.0)).w, G0.w * CROWD_NEAR);
  float r = G0.w * uFocal / w;
  return sqrt(3.14159265 * r * r / max(gal(c, 3).y, 1.0));
}
uniform float uBirth;
vec3 born(vec3 p, float row){
  if (uBirth > 0.999 || row < -0.5) return p;
  int c = int(row + 0.5);
  float late = hf(uint(c) * 2654435761u) * BIRTH_STAGGER;
  float g = clamp(uBirth * (1.0 + BIRTH_STAGGER) - late, 0.0, 1.0);
  vec3 home = gal(c, 0).xyz;
  return home + (p - home) * (1.0 - pow(1.0 - g, 3.0));
}
vec3 starColor(float t){
  t = clamp(t, 0.0, 1.0) * 6.0;
  vec3 M = vec3(1.0, 0.493, 0.167), K = vec3(1.0, 0.731, 0.499), G = vec3(1.0, 0.867, 0.782), F = vec3(0.974, 0.933, 1.0);
  vec3 A = vec3(0.701, 0.759, 1.0), B = vec3(0.416, 0.542, 1.0), O = vec3(0.34, 0.477, 1.0);
  if (t < 1.0) return mix(M, K, t);
  if (t < 2.0) return mix(K, G, t - 1.0);
  if (t < 3.0) return mix(G, F, t - 2.0);
  if (t < 4.0) return mix(F, A, t - 3.0);
  if (t < 5.0) return mix(A, B, t - 4.0);
  return mix(B, O, t - 5.0);
}`;

/* 쪽의 자리 — 번호에서 텍셀 하나(평면과 우주 사이를 `uLift`로). w는 그 쪽의 줄(군집) 번호이고 떠돌이 별은 -1이다. */
const KNOWLEDGE_UNIVERSE_NODE_AT = `
uniform highp sampler2D tNode3; uniform highp sampler2D tNode2; uniform int uTexW; uniform float uLift;
vec4 nodeAt(int i){ ivec2 c = ivec2(i % uTexW, i / uTexW); vec4 p3 = texelFetch(tNode3, c, 0); if (uLift > 0.999) return p3; return mix(texelFetch(tNode2, c, 0), p3, uLift); }`;

/* 군집 둘레의 빛 — 군집마다 카메라를 보는 사각형 하나. 활동이 많은 군집이 더 번진다. */
const KNOWLEDGE_UNIVERSE_AURA_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
${knowledgeUniverseDefines("AURA", KNOWLEDGE_UNIVERSE_AURA)}
      attribute float aRow; uniform float uLift; uniform float uFocusGal;
      varying vec2 vQ; varying vec3 vInk;
      void main(){
        int c = int(aRow + 0.5);
        vec4 G0 = gal(c, 0), G1 = gal(c, 1), G2 = gal(c, 2), G3 = gal(c, 3);
        vec3 center = mix(G1.xyz, G0.xyz, uLift);
        vec3 right = vec3(viewMatrix[0][0], viewMatrix[1][0], viewMatrix[2][0]); vec3 up = vec3(viewMatrix[0][1], viewMatrix[1][1], viewMatrix[2][1]);
        vec3 w = center + (right * position.x + up * position.y) * (G0.w * AURA_REACH);
        float foc = uFocusGal < -0.5 ? 1.0 : (abs(float(c) - uFocusGal) < 0.5 ? AURA_FOCUS_ON : AURA_FOCUS_OFF);
        vQ = position.xy;
        vInk = G2.rgb * ((AURA_BASE + AURA_ACTIVE * G3.x) * G2.a * foc * uLift * uLift * min(1.0, uBirth * BIRTH_INK));
        gl_Position = projectionMatrix * viewMatrix * vec4(w, 1.0);
      }`;

const KNOWLEDGE_UNIVERSE_AURA_FRAG = `${knowledgeUniverseDefines("AURA", KNOWLEDGE_UNIVERSE_AURA)}
      varying vec2 vQ; varying vec3 vInk;
      void main(){
        float r2 = dot(vQ, vQ);
        float fall = exp(-r2 * AURA_FALL) * (1.0 - smoothstep(AURA_RIM_FROM, 1.0, sqrt(r2)));
        gl_FragColor = vec4(vInk * fall, 0.0);
      }`;

/* 무한히 먼 배경 별 — 번호에서 셰이더가 짓는다(별마다 float 하나). t-12443이 시안에서 옮긴 글자다: 씨앗의 밑
 * (`SKY_SEED`)도 그때의 무리 번호 그대로라 같은 하늘이 선다. */
const KNOWLEDGE_UNIVERSE_SKY_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
#define SKY_SEED 8388608.0
#define SKY_SMUDGES ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_SHAPE.skySmudges)}
      uniform float uLift; uniform float uDpr; uniform float uFocus;
      varying vec3 vCol; varying float vA; varying float vKind;
      void main(){
        float j = position.x;
        uint s = hu(uint(SKY_SEED + j + 0.5) * 747796405u + 2891336453u);
        float r0 = hf(s), r1 = hf(s + 11u), r2 = hf(s + 23u), r3 = hf(s + 37u), r4 = hf(s + 41u), r5 = hf(s + 53u);
        float lift2 = uLift * uLift;
        float z = r0 * 2.0 - 1.0, ph = 6.2831853 * r1, rr = sqrt(max(0.0, 1.0 - z * z));
        vec3 dir = vec3(rr * cos(ph), z, rr * sin(ph));
        vec4 clip = projectionMatrix * vec4(mat3(viewMatrix) * dir, 0.0);
        vA = 0.0; vKind = 0.0;
        if (clip.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; vCol = vec3(0.0); return; }
        clip.z = clip.w * 0.9999;
        gl_Position = clip;
        float m = pow(r2, 9.0);
        float I = (0.045 + 0.07 * r3 + 2.4 * m) * lift2 * (uFocus > 0.5 ? 0.55 : 1.0);
        if (j < SKY_SMUDGES) { vKind = 1.0; vCol = vec3(1.0, 0.88, 0.76) * (0.05 + 0.05 * r3) * lift2; vA = r5 * 3.1416; gl_PointSize = (4.0 + 6.0 * r4) * uDpr; return; }
        vCol = starColor(0.08 + 0.84 * r4) * I;
        gl_PointSize = (1.15 + 2.2 * sqrt(m)) * uDpr;
      }`;

const KNOWLEDGE_UNIVERSE_SKY_FRAG = `varying vec3 vCol; varying float vA; varying float vKind;
      void main(){
        vec2 p = gl_PointCoord - 0.5;
        float r2 = dot(p, p) * 4.0;
        float inside = step(r2, 1.0);
        vec3 o;
        if (vKind < 0.5) o = vCol * exp(-r2 * 4.5);
        else { float ca = cos(vA), sa = sin(vA); vec2 q = mat2(ca, -sa, sa, ca) * p; o = vCol * exp(-(q.x * q.x * 4.0 + q.y * q.y * 26.0) * 4.0); }
        gl_FragColor = vec4(o * inside, 0.0);
      }`;

/* 별(쪽): 또렷한 속과 번짐. 크기 = 연결 수, 빛깔 = 군집, 번짐 = 허브의 몫과 최근 고침. 올리거나 고른 별은 가는 고리,
 * 찾은 별은 깜박이는 조준 고리. `aStar` = (크기의 바탕, 최근 0…1, 줄 번호 또는 -1, 빛의 몫), `aRoom` = 형제 사이 간격
 * (세계의 길이 — `KNOWLEDGE_UNIVERSE_CROWD`). */
const KNOWLEDGE_UNIVERSE_STARS_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
${knowledgeUniverseDefines("STAR", KNOWLEDGE_UNIVERSE_STAR)}
      attribute vec3 aPos2; attribute vec4 aStar; attribute float aState; attribute float aRoom;
      uniform float uLift; uniform float uDpr; uniform float uRef; uniform float uFocus; uniform vec3 uNeutral; uniform float uHalo;
      varying vec3 vCol; varying float vI; varying float vCore; varying float vSprite; varying float vState; varying float vGlow; varying float vWhite;
      void main(){
        if (aStar.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; vCol = vec3(0.0); vI = 0.0; vCore = 0.0; vSprite = 1.0; vState = 0.0; vGlow = 0.0; vWhite = 0.0; return; }
        vec3 w = mix(aPos2, born(position, aStar.z), uLift);
        vec4 clip = projectionMatrix * viewMatrix * vec4(w, 1.0);
        gl_Position = clip;
        float st = aState;
        float near = clamp(uRef / max(clip.w, 1.0), STAR_NEAR_LEAST, STAR_NEAR_MOST);
        float lum = aStar.x;
        float core = min(clamp(STAR_CORE_LEAST + STAR_CORE_GROW * pow(lum, STAR_CORE_POWER), STAR_CORE_MIN, STAR_CORE_MOST) * near, STAR_CORE_CEIL);
        float hub = smoothstep(STAR_HUB_FROM, STAR_HUB_TO, lum);
        float size = 1.0; float over = 1.0;
        if (aStar.z > -0.5) {
          float room = aRoom * uFocal / max(clip.w, 1.0);
          float want = uCrowd.x * min(gapOf(int(aStar.z + 0.5), uLift), room) / core;
          size = clamp(want, STAR_CROWD_SIZE, 1.0);
          over = mix(clamp(pow(min(1.0, want / size), uCrowd.z), uCrowd.w, 1.0), 1.0, hub);
          size = mix(size, clamp(CROWD_HUB * room / core, size, 1.0), hub);
        }
        if (st < 1.5) core *= size;
        if (st > 1.5 && st < 2.5) core = max(core * STAR_HOT_GROW, STAR_HOT_LEAST);
        if (st > 2.5) core = max(core, STAR_HIT_LEAST);
        float sprite = min(core * (STAR_HALO_LEAST + STAR_HALO_HUB * hub) + STAR_HALO_PAD + (st > 2.5 ? STAR_HIT_PAD : 0.0), STAR_SPRITE_MOST / uDpr);
        gl_PointSize = 2.0 * sprite * uDpr;
        vCore = core * uDpr; vSprite = sprite * uDpr; vState = st;
        vGlow = (STAR_GLOW + STAR_GLOW_HUB * hub + STAR_GLOW_FRESH * aStar.y * aStar.y) * size * over * uHalo;
        vWhite = STAR_WHITE + STAR_WHITE_HUB * hub;
        float focus = uFocus > 0.5 ? (st > 0.5 ? STAR_HELD : STAR_AWAY) : 1.0;
        vI = focus * (STAR_DIM + (1.0 - STAR_DIM) * smoothstep(STAR_DIM_FROM, STAR_DIM_TO, lum)) * aStar.w * (st > 0.5 ? 1.0 : over);
        vec3 c = aStar.z < -0.5 ? uNeutral : gal(int(aStar.z + 0.5), 2).rgb;
        if (uFocus > 0.5 && st < 0.5) c = mix(c, vec3(dot(c, ${KNOWLEDGE_UNIVERSE_LUMA_GLSL})), STAR_AWAY_GREY);
        vCol = c;
      }`;

const KNOWLEDGE_UNIVERSE_STARS_FRAG = `${knowledgeUniverseDefines("STAR", KNOWLEDGE_UNIVERSE_STAR)}
      uniform float uTime; uniform float uDpr;
      varying vec3 vCol; varying float vI; varying float vCore; varying float vSprite; varying float vState; varying float vGlow; varying float vWhite;
      void main(){
        vec2 p = (gl_PointCoord - 0.5) * 2.0 * vSprite;
        float r = length(p);
        float disc = 1.0 - smoothstep(vCore - STAR_EDGE * uDpr, vCore + STAR_EDGE * uDpr, r);
        float halo = exp(-(r * r) / (STAR_HALO_FALL * vSprite * vSprite)) * vGlow * (1.0 - smoothstep(STAR_HALO_RIM * vSprite, vSprite, r));
        float ring = 0.0;
        if (vState > 2.5) ring = exp(-pow((r - STAR_HIT_RING * vSprite) / (STAR_HIT_WIDE * uDpr), 2.0)) * (STAR_HIT_INK + STAR_HIT_BLINK * sin(uTime * STAR_HIT_BEAT));
        else if (vState > 1.5) ring = exp(-pow((r - (vCore + STAR_RING_GAP * uDpr)) / (STAR_RING_WIDE * uDpr), 2.0)) * STAR_RING_INK;
        vec3 o = (mix(vCol, vec3(1.0), vWhite) * disc + vCol * halo) * vI + mix(vCol, vec3(1.0), STAR_RING_WHITE) * ring;
        gl_FragColor = vec4(o, 0.0);
      }`;

/* 곧은 링크(가지 · 현 · 떠돌이 별의 링크) — 끝점마다 `position` = (제 쪽 번호, 줄 번호 또는 -1, 무게: 가지는 그 끝의
 * 밝기, 현은 음수), `aFlow` = (끝 0|1, 흐름의 위상, 추론 0|1), `aLit` = 0 쉼 · 1 켜짐 · 2 경로. */
const KNOWLEDGE_UNIVERSE_THREADS_VERT = `${KNOWLEDGE_UNIVERSE_GAL}${KNOWLEDGE_UNIVERSE_NODE_AT}
${knowledgeUniverseDefines("EDGE", KNOWLEDGE_UNIVERSE_EDGE)}
      attribute vec3 aFlow; attribute float aLit;
      uniform float uRef; uniform float uFocus; uniform float uFocusGal; uniform float uFocusPage; uniform float uLitGain; uniform vec3 uNeutral;
      varying vec3 vRest; varying vec3 vPulse; varying vec3 vLit; varying float vT; varying float vPhase;
      void main(){
        vec4 seat = nodeAt(int(position.x + 0.5));
        vec4 clip = projectionMatrix * viewMatrix * vec4(born(seat.xyz, seat.w), 1.0);
        gl_Position = clip;
        float near = clamp(uRef / max(clip.w, 1.0), EDGE_NEAR_LEAST, EDGE_NEAR_MOST);
        float gain = smoothstep(EDGE_VEIL_FROM, EDGE_VEIL_TO, clip.w) * uLift * uLift * min(1.0, uBirth * BIRTH_INK);
        float row = position.y;
        vec4 ink = row < -0.5 ? vec4(uNeutral, 1.0) : gal(int(row + 0.5), 2);
        float crowd = 1.0;
        if (row > -0.5) {
          int c = int(row + 0.5);
          crowd = clamp(pow(uCrowd.y * gapOf(c, uLift) / sqrt(3.14159265 * max(gal(c, 3).y, 1.0)), uCrowd.z), uCrowd.w, 1.0);
          crowd = mix(crowd, 1.0, smoothstep(EDGE_TRUNK_FROM + EDGE_TRUNK_GROW * EDGE_TRUNK_LEAST, EDGE_TRUNK_FROM + EDGE_TRUNK_GROW, position.z));
        }
        float rest = (position.z < 0.0 ? EDGE_CHORD * (1.0 - smoothstep(EDGE_CLOSE_NEAR, EDGE_CLOSE_FAR, clip.w / uRef)) : EDGE_BRANCH * position.z) * crowd;
        float foc = 1.0;
        if (uFocusGal > -0.5) foc = abs(row - uFocusGal) < 0.5 ? (uFocusPage > 0.5 ? EDGE_FOCUS_HOME : EDGE_FOCUS_ON) : EDGE_FOCUS_OFF;
        else if (uFocus > 0.5) foc = EDGE_FOCUS_REST;
        vRest = ink.rgb * (rest * ink.a * foc * near * gain);
        vPulse = vec3(0.0);
        float lit = aLit > 1.5 ? EDGE_PATH : (aLit > 0.5 ? 1.0 : 0.0);
        vLit = mix(ink.rgb, vec3(1.0), EDGE_LIT_WHITE) * (EDGE_LIT * uLitGain * lit * (aFlow.z > 0.5 ? EDGE_INFERRED : 1.0) * gain);
        vT = aFlow.x; vPhase = aFlow.y;
      }`;

/* 다발의 올(군집 사이의 링크) — 인스턴스 하나가 링크 하나다: `aFiber` = (보낸 쪽, 받는 쪽, 무게: 굵은 다발의 올은
 * 가늚, 그 밖은 음수, 휨), `aFiberFlow` = (흐름의 위상, 추론 0|1), `aFiberLit` = 0 쉼 · 1 켜짐 · 2 경로. 길은
 * `KNOWLEDGE_UNIVERSE_CURVE`의 다섯 점이다. 빛이 없는 올(굵은 다발의 것도 켜진 것도 초점 군집의 것도 아닌 올)은 자리를
 * 읽기 전에 판 밖으로 보낸다 — 1만 쪽 볼트의 올 대부분이 그렇다. */
const KNOWLEDGE_UNIVERSE_FIBERS_VERT = `${KNOWLEDGE_UNIVERSE_GAL}${KNOWLEDGE_UNIVERSE_NODE_AT}
${knowledgeUniverseDefines("EDGE", KNOWLEDGE_UNIVERSE_EDGE)}
${knowledgeUniverseDefines("CURVE", KNOWLEDGE_UNIVERSE_CURVE)}
      attribute vec4 aFiber; attribute vec2 aFiberFlow; attribute float aFiberLit;
      uniform float uRef; uniform float uFocus; uniform float uFocusGal; uniform float uFocusPage; uniform float uLitGain;
      varying vec3 vRest; varying vec3 vPulse; varying vec3 vLit; varying float vT; varying float vPhase;
      void main(){
        float t = position.x;
        bool dark = aFiber.z < 0.0 && aFiberLit < 0.5;
        if (dark && (uFocusPage > 0.5 || uFocusGal < -0.5)) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); vRest = vec3(0.0); vPulse = vec3(0.0); vLit = vec3(0.0); vT = 0.0; vPhase = 0.0; return; }
        vec4 NA = nodeAt(int(aFiber.x + 0.5)), NB = nodeAt(int(aFiber.y + 0.5));
        int ra = int(NA.w + 0.5), rb = int(NB.w + 0.5);
        float mine = (uFocusPage < 0.5 && (abs(float(ra) - uFocusGal) < 0.5 || abs(float(rb) - uFocusGal) < 0.5)) ? 1.0 : 0.0;
        if (dark && mine < 0.5) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); vRest = vec3(0.0); vPulse = vec3(0.0); vLit = vec3(0.0); vT = 0.0; vPhase = 0.0; return; }
        vec4 GA = gal(ra, 0), GB = gal(rb, 0);
        vec3 ca = mix(gal(ra, 1).xyz, GA.xyz, uLift), cb = mix(gal(rb, 1).xyz, GB.xyz, uLift);
        vec3 d = cb - ca; float L = max(length(d), 1.0); vec3 dir = d / L;
        vec3 side = cross(dir, vec3(0.0, 1.0, 0.0)); side = length(side) < CURVE_UPRIGHT ? vec3(1.0, 0.0, 0.0) : normalize(side);
        vec3 up2 = cross(side, dir);
        vec3 gateA = ca + dir * min(GA.w * CURVE_GATE, L * CURVE_GATE_MOST), gateB = cb - dir * min(GB.w * CURVE_GATE, L * CURVE_GATE_MOST);
        vec3 waist = (gateA + gateB) * 0.5 + (side * (aFiber.w * (ra < rb ? 1.0 : -1.0) * L * CURVE_SIDE) + up2 * (L * CURVE_UP)) * uLift;
        float s = 1.0 - t;
        vec3 P = born(NA.xyz, NA.w) * (s * s * s * s) + gateA * (4.0 * s * s * s * t) + waist * (6.0 * s * s * t * t) + gateB * (4.0 * s * t * t * t) + born(NB.xyz, NB.w) * (t * t * t * t);
        vec4 clip = projectionMatrix * viewMatrix * vec4(P, 1.0);
        gl_Position = clip;
        float near = clamp(uRef / max(clip.w, 1.0), EDGE_NEAR_LEAST, EDGE_NEAR_MOST);
        float gain = smoothstep(EDGE_VEIL_FROM, EDGE_VEIL_TO, clip.w) * uLift * uLift * min(1.0, uBirth * BIRTH_INK);
        float rest = aFiber.z < 0.0 ? EDGE_FIBER_FAINT * mine : EDGE_FIBER * aFiber.z;
        float foc = 1.0;
        if (uFocusGal > -0.5) foc = mine > 0.5 ? EDGE_FOCUS_ON : EDGE_FOCUS_OFF;
        else if (uFocus > 0.5) foc = EDGE_FOCUS_REST;
        float ends = EDGE_FIBER_ENDS + (1.0 - EDGE_FIBER_ENDS) * smoothstep(EDGE_ENDS_FROM, EDGE_ENDS_TO, t) * smoothstep(1.0 - EDGE_ENDS_FROM, 1.0 - EDGE_ENDS_TO, t);
        vec4 IA = gal(ra, 2), IB = gal(rb, 2);
        vec3 ink = mix(IA.rgb, IB.rgb, t);
        float body = mix(IA.a, IB.a, t) * foc * ends * near * gain;
        vRest = ink * (rest * body);
        vPulse = ink * (rest > 0.0 ? EDGE_FIBER_PULSE * body : 0.0);
        float lit = aFiberLit > 1.5 ? EDGE_PATH : (aFiberLit > 0.5 ? 1.0 : 0.0);
        vLit = mix(ink, vec3(1.0), EDGE_LIT_WHITE) * (EDGE_LIT * uLitGain * lit * (aFiberFlow.y > 0.5 ? EDGE_INFERRED : 1.0) * gain);
        vT = t; vPhase = aFiberFlow.x;
      }`;

/* 링크의 조각 — 가지 · 현 · 올이 함께 쓴다. 쉬는 빛에 다발의 흐름(올만)과 켜진 링크의 달리는 머리를 더한다. */
const KNOWLEDGE_UNIVERSE_EDGE_FRAG = `${knowledgeUniverseDefines("EDGE", KNOWLEDGE_UNIVERSE_EDGE)}
      uniform float uTime; uniform float uFlow;
      varying vec3 vRest; varying vec3 vPulse; varying vec3 vLit; varying float vT; varying float vPhase;
      float head(float speed, float tail){
        float behind = fract(fract(uTime * speed + vPhase) - vT);
        return behind < tail ? pow(1.0 - behind / tail, 2.0) : 0.0;
      }
      void main(){
        vec3 o = vRest + vPulse * (head(EDGE_FIBER_SPEED, EDGE_FIBER_TAIL) * uFlow) + vLit * (1.0 + EDGE_PULSE * head(EDGE_PULSE_SPEED, EDGE_PULSE_TAIL) * uFlow);
        gl_FragColor = vec4(o, 0.0);
      }`;

/* 후처리: 빛 번짐 사다리(Jimenez 2014 — 13탭 내리기 + 3×3 텐트 올리기, 첫 단은 Karis 평균) → 톤 매핑 —
 * 시안 `FS_VERT`·`down`·`up`·`comp`. 톤 매핑은 채널마다의 ACES와 가장 센 채널로 잰 ACES(색 비율 그대로) 사이를
 * `tone-keep`만큼 섞고, 밝은 테마의 먹빛은 색만 `ink-chroma`배로 벌린 뒤 휘도를 v4의 먹빛에 돌려놓는다 — 종이와의
 * 대비는 v4 그대로다(t-14081). */
const KNOWLEDGE_UNIVERSE_POST_VERT = `varying vec2 vUv; void main(){ vUv = position.xy * 0.5 + 0.5; gl_Position = vec4(position.xy, 0.0, 1.0); }`;

const KNOWLEDGE_UNIVERSE_DOWN_FRAG = `uniform sampler2D tSrc; uniform vec2 uTexel; uniform float uKaris; varying vec2 vUv;
    vec3 s(float x, float y){ return texture2D(tSrc, vUv + vec2(x, y) * uTexel).rgb; }
    float w(vec3 c){ return 1.0 / (1.0 + max(max(c.r, c.g), c.b)); }
    void main(){
      vec3 a = s(-2.0, 2.0), b = s(0.0, 2.0), c = s(2.0, 2.0), d = s(-2.0, 0.0), e = s(0.0, 0.0), f = s(2.0, 0.0), g = s(-2.0, -2.0), h = s(0.0, -2.0), i = s(2.0, -2.0);
      vec3 j = s(-1.0, 1.0), k = s(1.0, 1.0), l = s(-1.0, -1.0), m = s(1.0, -1.0);
      vec3 o;
      if (uKaris > 0.5) {
        vec3 g0 = (j + k + l + m) * 0.25, g1 = (a + b + d + e) * 0.25, g2 = (b + c + e + f) * 0.25, g3 = (d + e + g + h) * 0.25, g4 = (e + f + h + i) * 0.25;
        float w0 = w(g0) * 0.5, w1 = w(g1) * 0.125, w2 = w(g2) * 0.125, w3 = w(g3) * 0.125, w4 = w(g4) * 0.125;
        o = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
      } else o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
      gl_FragColor = vec4(o, 1.0);
    }`;

const KNOWLEDGE_UNIVERSE_UP_FRAG = `uniform sampler2D tSrc; uniform vec2 uTexel; varying vec2 vUv;
    vec3 s(float x, float y){ return texture2D(tSrc, vUv + vec2(x, y) * uTexel).rgb; }
    void main(){
      vec3 o = s(0.0, 0.0) * 4.0 + (s(0.0, 1.0) + s(-1.0, 0.0) + s(1.0, 0.0) + s(0.0, -1.0)) * 2.0 + (s(-1.0, 1.0) + s(1.0, 1.0) + s(-1.0, -1.0) + s(1.0, -1.0));
      gl_FragColor = vec4(o / 16.0, 0.0);
    }`;

const KNOWLEDGE_UNIVERSE_COMP_FRAG = `uniform sampler2D tScene; uniform sampler2D tBloom; uniform float uBloom; uniform float uExposure; uniform float uDark; uniform vec3 uPaper; uniform float uTime; uniform float uKeep; uniform float uInk; varying vec2 vUv;
    vec3 aces(vec3 x){ return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0); }
    vec3 srgb(vec3 c){ c = clamp(c, 0.0, 1.0); return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c)); }
    float ign(vec2 p){ return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715)))); }
    void main(){
      vec3 c = (texture2D(tScene, vUv).rgb + texture2D(tBloom, vUv).rgb * uBloom) * uExposure;
      vec2 d = vUv - 0.5;
      float peak = max(max(c.r, c.g), c.b);
      vec3 m = mix(aces(c), c * (aces(vec3(peak)).r / max(peak, 1e-4)), uKeep) * (1.0 - 0.45 * dot(d, d));
      vec3 o;
      if (uDark > 0.5) o = srgb(m);
      else {
        float L = max(max(m.r, m.g), m.b);
        vec3 tone = m / max(L, 1e-4);
        float y = dot(tone, ${KNOWLEDGE_UNIVERSE_LUMA_GLSL});
        vec3 black = vec3(0.07, 0.08, 0.11);
        vec3 ink = max(black + mix(vec3(y), tone, uInk) * 0.08, 0.0);
        ink *= (dot(black, ${KNOWLEDGE_UNIVERSE_LUMA_GLSL}) + y * 0.08) / max(dot(ink, ${KNOWLEDGE_UNIVERSE_LUMA_GLSL}), 1e-4);
        o = srgb(mix(uPaper, ink, clamp(L * 1.3, 0.0, 1.0)));
      }
      o += (ign(gl_FragCoord.xy + fract(uTime * 7.0) * 64.0) - 0.5) / 255.0;
      gl_FragColor = vec4(o, 1.0);
    }`;

/* ---- 우주의 자료 — 평면 지도에서 짓는다 ------------------------------------------------ */

/* 씨앗 하나에서 이어지는 수(시안 `kg-data.js`의 `prng`) — 난수는 없다: 같은 볼트는 같은 우주다. */
function knowledgeUniverseRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let mixed = Math.imul(state ^ (state >>> 15), 1 | state);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/* 수 하나의 해시(시안 `universe`의 `hash`: 씨앗 x × 7919 + 17의 둘째 값). */
function knowledgeUniverseHash(seed) {
  const next = knowledgeUniverseRandom(seed * 7919 + 17);
  next();
  return next();
}

/* 볼트 크기의 몫(시안 `layout`의 spread) — (쪽 / `world-pages`)^`world-spread`. */
function knowledgeUniverseSpread(layout, pages) {
  const U = knowledgeUniverseTuning(layout.view);
  return (Math.max(1, pages) / U.worldPages) ** U.worldSpread;
}

/* 평면 자리를 세계로 옮기는 수: 평면 원반의 √쪽 당 반지름(`cluster-pitch`)을 시안의 r2 계수(`world-root` × 몫)로.
 * 원반을 넓히는 `cluster-spread`(t-12029)로는 나누지 않는다 — 넓힌 뒤 틈이 줄어든 원반의 자리를 그대로 옮기면 이웃
 * 군집이 겹친다(디자이너 m-12586). 반지름은 원반에서가 아니라 쪽 수에서 셈한다(`knowledgeUniverseMap`). */
function knowledgeUniverseScale(layout, pages) {
  const U = knowledgeUniverseTuning(layout.view);
  return (U.worldRoot * knowledgeUniverseSpread(layout, pages)) / Math.max(1e-6, layout.tuning.clusterPitch);
}

function knowledgeUniverseLinear(value) {
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function knowledgeUniverseLuminance(rgb) {
  return KNOWLEDGE_UNIVERSE_LUMA.reduce((sum, weight, channel) => sum + weight * rgb[channel], 0);
}

/* 군집의 빛깔(텍셀 2) — 색 칸 토큰의 어두운 테마 값(선형, 가장 센 채널을 1로)을 흰빛과 `galaxy-tint-base`만큼 섞고, 그
 * 휘도를 색 없는 군집의 고정값에 맞춘다(t-14081): 빛깔은 범례 칩·이름표 점과 같은 토큰의 색상에 그 채도의 (1 − base)를
 * 싣고 — 색 칸마다 같은 몫이다 —, 밝기는 색과 상관없이 색 없는 군집과 같다(자홍·보라처럼 휘도가 낮은 색의 군집이
 * 어두워지지 않는다). 색 칸이 없으면 고정값. */
function knowledgeUniverseTint(U, inks, hue) {
  if (hue < 0) return inks.neutral;
  const source = inks.tints.subarray(hue * 3, hue * 3 + 3);
  const peak = Math.max(1e-6, ...source);
  const mixed = Array.from(source, (value) => U.galaxyTintBase + (1 - U.galaxyTintBase) * (value / peak));
  const scale = knowledgeUniverseLuminance(inks.neutral) / Math.max(1e-6, knowledgeUniverseLuminance(mixed));
  return mixed.map((value) => value * scale);
}

/* 색과 테마마다의 수 — CSS에게 묻는다(평면의 GL 손과 같은 문). 군집 빛깔의 원천 여덟은 계산된 색으로 읽고
 * (`color-mix`까지 풀리게 견본의 `color`로), 선형으로 옮긴다. 테마가 바뀌면 다시 묻는다. */
function knowledgeUniverseInks(view, probe) {
  const style = getComputedStyle(view);
  const tints = new Float32Array(KNOWLEDGE_HUES * 3);
  const read = new Float32Array(4);
  for (let hue = 0; hue < KNOWLEDGE_HUES; hue += 1) {
    probe.style.color = `var(--knowledge-3d-tint-${hue})`;
    knowledgeGlColor(getComputedStyle(probe).color, read, 0);
    for (let channel = 0; channel < 3; channel += 1) tints[hue * 3 + channel] = knowledgeUniverseLinear(read[channel]);
  }
  probe.style.color = "var(--knowledge-3d-paper)";
  knowledgeGlColor(getComputedStyle(probe).color, read, 0);
  const paper = [0, 1, 2].map((channel) => knowledgeUniverseLinear(read[channel]));
  const neutral = style.getPropertyValue("--knowledge-3d-galaxy-tint-neutral").trim().split(/\s+/u).map(Number);
  const themed = Object.fromEntries(Object.entries(KNOWLEDGE_UNIVERSE_THEMED)
    .map(([key, name]) => [key, Number.parseFloat(style.getPropertyValue(name))]));
  const theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";
  return { theme, dark: theme === "dark", tints, paper, neutral, ...themed };
}

/* 군집 하나의 나무 — 그 군집의 허브(연결이 가장 많은 쪽, `list[0]`)에서 군집 안의 링크를 너비 우선으로 따라 퍼진다.
 * 한 켜를 연결이 많은 차례로 훑어 허브가 먼저 자식을 얻는다. 가려진 쪽(`hold.shown`이 0 — 평면 지도가 접은 목차 ·
 * 일지)은 지나지 않는다. 자리는 반지름 1의 틀 안에서 놓고(`hold.local`),
 * 부르는 쪽이 가장 먼 쪽(`far`)으로 나눠 군집의 반지름에 맞춘다. 군집 안에서 어느 쪽과도 이어지지 않은 쪽은
 * 허브의 잎 자리에 서되 가지는 없다(`hold.loose`) — 가지는 진짜 링크만이다. 자식마다 형제 사이 간격도 같은 틀의
 * 길이로 적는다(`hold.room` — `KNOWLEDGE_UNIVERSE_CROWD`).
 *
 * 답: 가장 먼 쪽의 거리, 허브에 바로 이어진 쪽의 수와 그 가운데 가지를 친 쪽(딸린 쪽이 있는 자식)의 수, 가장 깊은
 * 걸음. */
function knowledgeUniverseBurst(model, community, rank, list, hold) {
  const B = KNOWLEDGE_UNIVERSE_BURST;
  const { parent, depth, sub, local, heading, loose, seeds, shown, room } = hold;
  if (list.length === 0) return { far: 1e-6, direct: 0, branches: 0, deepest: 0 };
  const root = list[0];
  const size = list.length;
  const order = [root];
  depth[root] = 0;
  let level = [root];
  while (level.length > 0) {
    const next = [];
    level.sort((one, two) => model.degree[two] - model.degree[one] || one - two);
    for (const from of level) {
      for (let slot = model.start[from]; slot < model.start[from + 1]; slot += 1) {
        const to = model.neighbour[slot];
        if (community[to] !== rank || depth[to] >= 0 || (shown !== null && shown[to] === 0)) continue;
        depth[to] = depth[from] + 1;
        parent[to] = from;
        next.push(to);
        order.push(to);
      }
    }
    level = next;
  }
  for (const at of list) {
    if (depth[at] >= 0) continue;
    depth[at] = 1;
    parent[at] = root;
    loose[at] = 1;
    order.push(at);
  }
  const kids = new Map();
  for (let at = order.length - 1; at >= 1; at -= 1) {
    const child = order[at];
    sub[child] += 1;
    sub[parent[child]] += sub[child];
    const held = kids.get(parent[child]);
    if (held === undefined) kids.set(parent[child], [child]);
    else held.push(child);
  }
  sub[root] += 1;
  let far = 0;
  let direct = 0;
  let branches = 0;
  let deepest = 0;
  for (const from of order) {
    deepest = Math.max(deepest, depth[from]);
    const mine = kids.get(from);
    if (mine === undefined) continue;
    /* 자식의 차례는 열쇠의 씨앗 — 딸린 쪽이 많은 가지들이 한쪽에 몰리지 않고, 쪽 하나가 늘어도 차례가 흔들리지 않는다. */
    mine.sort((one, two) => seeds[one] - seeds[two] || one - two);
    const first = from === root;
    const ax = heading[from * 3];
    const ay = heading[from * 3 + 1];
    const az = heading[from * 3 + 2];
    /* 온 방향에 수직인 두 축. */
    let px = ay;
    let py = -ax;
    const across = Math.hypot(px, py);
    if (across < KNOWLEDGE_UNIVERSE_CURVE.upright) {
      px = 1;
      py = 0;
    } else {
      px /= across;
      py /= across;
    }
    const qx = -az * py;
    const qy = az * px;
    const qz = ax * py - ay * px;
    const half = Math.min(B.capMost, B.capLeast + B.capGrow * Math.sqrt(mine.length));
    const cap = first ? -1 : Math.cos(half);
    const share = (first ? 1 : Math.sin(half)) * Math.sqrt(Math.PI / mine.length);
    const phase = knowledgeUniverseHash(seeds[from] + B.seedPhase) * Math.PI * 2;
    const reach = B.decay ** depth[from];
    for (let at = 0; at < mine.length; at += 1) {
      const child = mine[at];
      const z = 1 - (1 - cap) * ((at + 0.5) / mine.length);
      const ring = Math.sqrt(Math.max(0, 1 - z * z));
      const turn = at * KNOWLEDGE_GOLDEN_ANGLE + phase;
      const cx = ring * Math.cos(turn);
      const cy = ring * Math.sin(turn);
      const dx = first ? cx : px * cx + qx * cy + ax * z;
      const dy = first ? z : py * cx + qy * cy + ay * z;
      const dz = first ? cy : qz * cy + az * z;
      const length = reach * (B.leaf + B.leafLinks * Math.log2(1 + model.degree[child])
        + B.branch * Math.sqrt((sub[child] - 1) / size))
        * (B.wobbleLeast + B.wobbleSpan * knowledgeUniverseHash(seeds[child] + B.seedWobble));
      room[child] = length * share;
      heading[child * 3] = dx;
      heading[child * 3 + 1] = dy;
      heading[child * 3 + 2] = dz;
      local[child * 3] = local[from * 3] + dx * length;
      local[child * 3 + 1] = local[from * 3 + 1] + dy * length;
      local[child * 3 + 2] = local[from * 3 + 2] + dz * length;
      far = Math.max(far, Math.hypot(local[child * 3], local[child * 3 + 1], local[child * 3 + 2]));
      if (first && loose[child] === 0) direct += 1;
      if (first && sub[child] > 1) branches += 1;
    }
  }
  return { far: Math.max(far, 1e-6), direct, branches, deepest };
}

/* 우주의 자료 — 평면 지도의 군집과 원반에서.
 *
 * 군집은 평면 지도가 이미 가른 것(Louvain 순위)이다. 이름 있는 군집이 줄이고(텍스처 줄이 닿는 데까지), 그 가운데
 * 앞에서부터 큰 군집(명판 · 필라멘트), 이름 없는 군집과 줄 밖은 떠돌이 별이다. 군집의 자리는 평면 원반의 자리
 * (x·z)에 높이만 더한 것이고, 씨앗(높이·기울기)은 순위가 아니라 군집의 대표 쪽 열쇠에서 뽑는다 — 쪽 하나가 늘어
 * 순위가 바뀌어도 군집이 뒤집히지 않는다. 쪽의 자리는 그 군집의 나무(`knowledgeUniverseBurst`)다.
 *
 * 평면 지도가 가린 쪽(`layout.drawn`이 0 — 접은 목차 · 일지)은 우주에도 서지 않는다: 어느 군집의 쪽도 아니고, 그
 * 쪽의 링크는 세지도 그리지도 켜지도 않는다. 가림이 바뀌면(`layout.drawnStamp`) 우주를 다시 짓는다. */
function knowledgeUniverseMap(layout, inks) {
  const U = knowledgeUniverseTuning(layout.view);
  const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
  const CURVE = KNOWLEDGE_UNIVERSE_CURVE;
  const { model, count, community, communitySize, namedCount, communityHomeX, communityHomeY, communityCore,
    communityHue } = layout;
  let pages = 0;
  for (let at = 0; at < count; at += 1) if (model.kinds[at] === "page") pages += 1;
  const scale = knowledgeUniverseScale(layout, pages);
  const spread = knowledgeUniverseSpread(layout, pages);
  const rows = Math.min(namedCount, SHAPE.rows);
  const least = Math.max(U.galaxyLeast, Math.round((U.galaxyShare * pages) / U.worldPages));
  let galaxies = 0;
  while (galaxies < Math.min(rows, U.galaxyMax) && communitySize[galaxies] >= least) galaxies += 1;
  /* 닿는 거리 — 큰 군집의 가장 먼 끝. 큰 군집이 없는 볼트는 줄 전부. */
  let reach = 0;
  const reachRows = galaxies > 0 ? galaxies : rows;
  for (let rank = 0; rank < reachRows; rank += 1) {
    reach = Math.max(reach, Math.hypot(communityHomeX[rank], communityHomeY[rank]) * scale
      + U.worldRoot * Math.sqrt(communitySize[rank]) * spread);
  }
  reach = Math.max(1, reach);
  /* 줄마다 쪽을 모은다 — 연결이 많은 차례로(같으면 열쇠 순), 차례 0이 허브다. */
  const shown = layout.drawn === null ? null : Uint8Array.from(layout.drawn);
  const shownEdge = layout.drawnEdge === null ? null : Uint8Array.from(layout.drawnEdge);
  const members = Array.from({ length: rows }, () => []);
  for (let at = 0; at < count; at += 1) {
    if (community[at] < rows && (shown === null || shown[at] === 1)) members[community[at]].push(at);
  }
  for (const list of members) {
    list.sort((left, right) => model.degree[right] - model.degree[left]
      || (model.keys[left] < model.keys[right] ? -1 : 1));
  }
  /* 활동 — 최근 `activity-days`일 안에 고친 쪽의 몫. 고친 때를 모르는 쪽은 오래된 것이다. */
  const windowMs = U.activityDays * 86_400_000;
  const activity = new Float32Array(rows);
  for (let row = 0; row < rows; row += 1) {
    let recent = 0;
    for (const at of members[row]) {
      if (model.modified[at] > 0 && model.nowMs - model.modified[at] < windowMs) recent += 1;
    }
    activity[row] = members[row].length > 0 ? recent / members[row].length : 0;
  }
  /* 나무가 선 축의 바탕: 처음 시점 쪽으로 `normal-lean`만큼 기울인 위쪽. */
  const camera = [Math.cos(U.homePitch) * Math.sin(U.homeYaw), Math.sin(U.homePitch),
    Math.cos(U.homePitch) * Math.cos(U.homeYaw)];
  let bx = camera[0] * U.normalLean;
  let by = 1 + camera[1] * U.normalLean;
  let bz = camera[2] * U.normalLean;
  const bl = Math.hypot(bx, by, bz);
  bx /= bl;
  by /= bl;
  bz /= bl;
  let ax = by;
  let ay = -bx;
  const al = Math.hypot(ax, ay) || 1;
  ax /= al;
  ay /= al;
  const qx = -bz * ay;
  const qy = bz * ax;
  const qz = bx * ay - by * ax;
  const gal = new Float32Array(SHAPE.texels * SHAPE.rows * 4);
  const put = (row, texel, a, b, c, d) => {
    const at = (row * SHAPE.texels + texel) * 4;
    gal[at] = a;
    gal[at + 1] = b;
    gal[at + 2] = c;
    gal[at + 3] = d;
  };
  const pos3 = new Float32Array(count * 3);
  const rowOf = new Int32Array(count).fill(-1);
  /* 나무의 살림 — 쪽마다 부모(없으면 -1), 걸음, 딸린 쪽 수(저 포함), 틀 안의 자리와 온 방향, 가지 없는 잎, 열쇠의 씨앗,
   * 형제 사이 간격(틀의 길이로 적고 아래에서 군집의 반지름에 맞춘다). */
  const hold = { parent: new Int32Array(count).fill(-1), depth: new Int32Array(count).fill(-1), sub: new Int32Array(count),
    local: new Float32Array(count * 3), heading: new Float32Array(count * 3), loose: new Uint8Array(count),
    seeds: new Uint32Array(count), shown, room: new Float32Array(count).fill(SHAPE.roomy) };
  for (let at = 0; at < count; at += 1) hold.seeds[at] = knowledgeHash(model.keys[at]);
  const galaxy = [];
  for (let row = 0; row < rows; row += 1) {
    const rank = row;
    const list = members[row];
    const core = communityCore[rank];
    const seed = core >= 0 ? hold.seeds[core] : hold.seeds[list[0] ?? 0];
    const h1 = knowledgeUniverseHash(seed + 1);
    const h2 = knowledgeUniverseHash(seed + 101);
    const h3 = knowledgeUniverseHash(seed + 211);
    const large = row < galaxies;
    const radius = large ? U.galaxyRadius * U.worldRoot * Math.sqrt(communitySize[rank]) * spread
      : U.clusterSphere * Math.sqrt(communitySize[rank]) * spread * U.clusterGrow + U.clusterPad;
    const height = (h1 - 0.5) * 2 * (large ? U.liftGalaxy : U.liftCluster) * reach;
    /* 기울기: 바탕에서 씨앗 방향으로 `tilt-least` + 씨앗 × `tilt-span` rad. */
    const tilt = U.tiltLeast + h2 * U.tiltSpan;
    const phi = h3 * Math.PI * 2;
    const st = Math.sin(tilt);
    const ct = Math.cos(tilt);
    const cp = Math.cos(phi);
    const sp = Math.sin(phi);
    let nx = bx * ct + (ax * cp + qx * sp) * st;
    let ny = by * ct + (ay * cp + qy * sp) * st;
    let nz = bz * ct + qz * sp * st;
    const nl = Math.hypot(nx, ny, nz);
    nx /= nl;
    ny /= nl;
    nz /= nl;
    let ux = ny;
    let uy = -nx;
    const ul = Math.hypot(ux, uy) || 1;
    ux /= ul;
    uy /= ul;
    const vx = -nz * uy;
    const vy = nz * ux;
    const vz = nx * uy - ny * ux;
    const cx = communityHomeX[rank] * scale;
    const cz = communityHomeY[rank] * scale;
    const hue = communityHue[rank];
    const tint = knowledgeUniverseTint(U, inks, hue);
    const weight = hue >= 0 ? 1 : U.quietShare;
    put(row, 0, cx, height, cz, radius);
    /* 텍셀 1(평면 자리)은 평면 카메라의 눌림을 따라 `placeSeats`가 채운다. */
    put(row, 2, tint[0], tint[1], tint[2], weight);
    put(row, 3, activity[row], list.length, 0, 0);
    const burst = knowledgeUniverseBurst(model, community, rank, list, hold);
    const fit = (radius * KNOWLEDGE_UNIVERSE_BURST.fill) / burst.far;
    for (const at of list) {
      rowOf[at] = row;
      const lx = hold.local[at * 3] * fit;
      const ly = hold.local[at * 3 + 1] * fit;
      const lz = hold.local[at * 3 + 2] * fit;
      pos3[at * 3] = cx + ux * lx + nx * ly + vx * lz;
      pos3[at * 3 + 1] = height + uy * lx + ny * ly + vy * lz;
      pos3[at * 3 + 2] = cz + nz * ly + vz * lz;
      if (hold.parent[at] >= 0) hold.room[at] *= fit;
    }
    galaxy.push({ rank, large, radius, x: cx, y: height, z: cz, activity: activity[row], hue, weight, size: list.length,
      hub: list[0] ?? -1, direct: burst.direct, branches: burst.branches, deepest: burst.deepest });
  }
  /* 필라멘트: 두 큰 군집 사이의 링크(선언·추론 모두)를 보낸 쪽 → 받는 쪽으로 세고, 합이 `filament-least` 이상인 쌍을
   * 굵은 차례로 `filament-most`쌍까지. 빛은 많이 보낸 쪽에서 흐른다. */
  const flows = new Map();
  for (let at = 0; at < model.edgeCount; at += 1) {
    if (shownEdge !== null && shownEdge[at] === 0) continue;
    const a = rowOf[model.from[at]];
    const b = rowOf[model.to[at]];
    if (a < 0 || b < 0 || a === b || a >= galaxies || b >= galaxies) continue;
    const low = Math.min(a, b);
    const high = Math.max(a, b);
    const key = low * SHAPE.rows + high;
    const held = flows.get(key) ?? { a: low, b: high, ab: 0, ba: 0, n: 0 };
    if (a < b) held.ab += 1;
    else held.ba += 1;
    held.n += 1;
    flows.set(key, held);
  }
  const chosen = [...flows.values()].filter((flow) => flow.n >= U.filamentLeast)
    .sort((left, right) => right.n - left.n || left.a - right.a || left.b - right.b)
    .slice(0, U.filamentMost);
  const most = chosen[0]?.n ?? 1;
  const filaments = chosen.map((flow) => {
    const forward = flow.ab >= flow.ba;
    return { a: flow.a, b: flow.b, n: flow.n, ab: flow.ab, ba: flow.ba,
      from: forward ? flow.a : flow.b, to: forward ? flow.b : flow.a, sent: Math.max(flow.ab, flow.ba),
      strength: Math.sqrt(flow.n / most), bend: knowledgeUniverseBend(flow.a, flow.b) };
  });
  return { scale, reach, rows, galaxies, pages, gal, pos3, rowOf, galaxy, members, filaments,
    parent: hold.parent, depth: hold.depth, sub: hold.sub, loose: hold.loose, room: hold.room, shown, shownEdge, stamp: layout.drawnStamp };
}

/* 한 쌍의 휨(옆으로 다섯 단계, -1…1) — 쌍의 두 줄 번호에서. 굵은 다발이든 한 올뿐인 쌍이든 같은 식이다. */
function knowledgeUniverseBend(low, high) {
  const C = KNOWLEDGE_UNIVERSE_CURVE;
  return ((low * C.bendLow + high * C.bendHigh) % C.bendSteps) / 2 - 1;
}

/* 링크를 그림의 두 통으로 나눈다 — 곧은 링크(가지 · 현)와 다발의 올. 평면 지도가 가리지 않은 링크는 모두 어느 한
 * 통에 든다: 쉴 때 보이지 않는 링크도 올리거나 고르면 켜져야 하기 때문이다(`refreshStates`). 가려진 링크(접은
 * 목차 · 일지의 것)는 어느 통에도 없다.
 *
 *   가지: 군집 안의 링크 중 나무의 부모–자식을 잇는 것(자식마다 하나 — 같은 둘을 잇는 둘째 링크는 현이다).
 *   현: 그 밖의 군집 안 링크와 떠돌이 별에 닿는 링크. 빛깔은 제 군집(떠돌이 별끼리는 색 없음).
 *   올: 서로 다른 두 군집의 쪽을 잇는 링크. 굵은 다발(`map.filaments`)의 올만 쉴 때 보이고 — 링크가 `fiberMost`를 넘는
 *     다발은 그 가운데 고르게 뽑은 올만 —, 그 무게는 다발의 가늚이다.
 *
 * 답의 `threadOf` · `fiberOf`는 링크 번호 → 그 통의 자리(없으면 -1), `branches`는 가지의 수(곧은 링크의 통에서
 * 앞쪽), `thick`은 쉬는 빛을 드는 올의 수(올의 통에서 앞쪽), `declared` · `inferred`는 쪽마다의 두 갈래 수다. */
function knowledgeUniverseEdges(model, map) {
  const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
  const E = KNOWLEDGE_UNIVERSE_EDGE;
  const edgeCount = model.edgeCount;
  const count = map.rowOf.length;
  const guessedCode = KNOWLEDGE_EDGE_PROVENANCE_CODE.inferred;
  const pairs = new Map();
  map.filaments.forEach((one) => pairs.set(one.a * SHAPE.rows + one.b, one));
  const threadOf = new Int32Array(edgeCount).fill(-1);
  const fiberOf = new Int32Array(edgeCount).fill(-1);
  const declared = new Int32Array(count);
  const inferred = new Int32Array(count);
  const claimed = new Uint8Array(count);
  /* 링크마다: 흐름의 위상, 추론인가, 가지라면 그 자식(아니면 -1). */
  const phases = new Float32Array(edgeCount);
  const guesses = new Uint8Array(edgeCount);
  const childOf = new Int32Array(edgeCount).fill(-1);
  const shownEdge = map.shownEdge;
  /* 통 안의 차례 — 곧은 링크는 가지가 앞이고 올은 굵은 다발의 것이 앞이다: 쉬는 판은 그 앞쪽만 싣는다(`branches` ·
   * `thick`, `spanLinks`). 현과 그 밖의 올은 뒤에 선다. */
  const chords = [];
  const thin = [];
  const met = new Map();
  let threads = 0;
  let fibers = 0;
  let seed = SHAPE.lineSeed;
  for (let at = 0; at < edgeCount; at += 1) {
    const from = model.from[at];
    const to = model.to[at];
    const guessed = model.provenance[at] === guessedCode
      || (model.provenance[at] === KNOWLEDGE_PROVENANCE_UNKNOWN && model.kind[at] === KNOWLEDGE_EDGE_CODE.mentions);
    guesses[at] = guessed ? 1 : 0;
    const tally = guessed ? inferred : declared;
    tally[from] += 1;
    tally[to] += 1;
    seed = (seed * SHAPE.lineMultiplier) % SHAPE.lineModulus;
    phases[at] = seed / SHAPE.lineModulus;
    if (shownEdge !== null && shownEdge[at] === 0) continue;
    const a = map.rowOf[from];
    const b = map.rowOf[to];
    if (a >= 0 && b >= 0 && a !== b) {
      /* 굵은 다발의 올 가운데 쉬는 빛을 드는 것 — 다발의 링크가 `fiberMost`를 넘으면 차례대로 고르게 뽑는다. */
      const key = Math.min(a, b) * SHAPE.rows + Math.max(a, b);
      const pair = pairs.get(key);
      if (pair === undefined) {
        thin.push(at);
        continue;
      }
      const nth = met.get(key) ?? 0;
      met.set(key, nth + 1);
      const picks = Math.min(pair.n, E.fiberMost);
      if (Math.floor(((nth + 1) * picks) / pair.n) !== Math.floor((nth * picks) / pair.n)) fiberOf[at] = fibers++;
      else thin.push(at);
      continue;
    }
    /* 가지인가 — 부모와 자식을 잇는 첫 링크. */
    let child = -1;
    if (a >= 0 && a === b) {
      if (map.parent[to] === from && map.loose[to] === 0 && claimed[to] === 0) child = to;
      else if (map.parent[from] === to && map.loose[from] === 0 && claimed[from] === 0) child = from;
    }
    if (child < 0) {
      chords.push(at);
      continue;
    }
    claimed[child] = 1;
    childOf[at] = child;
    threadOf[at] = threads++;
  }
  const branches = threads;
  for (const at of chords) threadOf[at] = threads++;
  const thick = fibers;
  for (const at of thin) fiberOf[at] = fibers++;
  const seat = new Float32Array(threads * 2 * 3);
  const flow = new Float32Array(threads * 2 * 3);
  const fiber = new Float32Array(fibers * 4);
  const fiberFlow = new Float32Array(fibers * 2);
  for (let at = 0; at < edgeCount; at += 1) {
    if (threadOf[at] < 0 && fiberOf[at] < 0) continue;
    const from = model.from[at];
    const to = model.to[at];
    const a = map.rowOf[from];
    const b = map.rowOf[to];
    const phase = phases[at];
    const guessed = guesses[at];
    if (fiberOf[at] >= 0) {
      const low = Math.min(a, b);
      const high = Math.max(a, b);
      const pair = pairs.get(low * SHAPE.rows + high);
      const slot = fiberOf[at];
      fiber[slot * 4] = from;
      fiber[slot * 4 + 1] = to;
      fiber[slot * 4 + 2] = slot >= thick ? -1
        : (pair.n * Math.max(E.fiberLeast, Math.min(1, (E.fiberFull / pair.n) ** E.fiberFade))) / Math.min(pair.n, E.fiberMost);
      fiber[slot * 4 + 3] = knowledgeUniverseBend(low, high);
      fiberFlow[slot * 2] = phase;
      fiberFlow[slot * 2 + 1] = guessed;
      continue;
    }
    const child = childOf[at];
    let fromWeight = -1;
    let toWeight = -1;
    if (child >= 0) {
      const trunk = Math.min(1, E.trunkLeast + (1 - E.trunkLeast)
        * Math.sqrt((map.sub[child] - 1) / Math.max(1, map.galaxy[a].size * E.trunkShare)));
      const near = E.trunkFrom + E.trunkGrow * trunk;
      const end = E.twigFrom + E.twigGrow * trunk;
      fromWeight = child === to ? near : end;
      toWeight = child === to ? end : near;
    }
    const row = a >= 0 ? a : b;
    const slot = threadOf[at] * 6;
    seat[slot] = from;
    seat[slot + 1] = row;
    seat[slot + 2] = fromWeight;
    seat[slot + 3] = to;
    seat[slot + 4] = row;
    seat[slot + 5] = toWeight;
    flow[slot] = 0;
    flow[slot + 1] = phase;
    flow[slot + 2] = guessed;
    flow[slot + 3] = 1;
    flow[slot + 4] = phase;
    flow[slot + 5] = guessed;
  }
  return { threads, fibers, thick, branches, seat, flow, fiber, fiberFlow, threadOf, fiberOf, declared, inferred };
}

/* ---- 한 판의 우주 ---------------------------------------------------------------------- */

function makeKnowledgeUniverse(view) {
  const U = knowledgeUniverseTuning(view);
  return {
    view,
    tune: U,
    host: null,
    canvas: null,
    labels: null,
    notice: null,
    noticeTimer: 0,
    renderer: null,
    scene: null,
    camera: null,
    uniforms: null,
    materials: null,
    /* 지은 것 — 떠날 때 놓는 목록. */
    built: { geometries: [], objects: [] },
    layout: null,
    /* 평면 자리가 마지막으로 옮겨진 판의 도장 — 앉기가 끝나면 한 번 더 옮긴다. */
    seatsRevision: -1,
    scale: 1,
    reach: 1,
    pos2: null,
    pos3: null,
    starGeometry: null,
    starState: null,
    starStateAttr: null,
    /* 우주의 자료(`knowledgeUniverseMap`)와 그 텍스처, 색(`knowledgeUniverseInks`)과 견본, 후처리의 살림. */
    map: null,
    /* 이 우주를 지은 때의 가림(`layout.drawnStamp`) — 바뀌면 다시 짓는다. */
    seenStamp: null,
    galTexture: null,
    skyCount: 0,
    filaments: [],
    /* 쪽 자리 텍스처 둘, 링크의 두 통(`knowledgeUniverseEdges`)과 그 켜짐(곧은 링크는 끝점마다, 올은 인스턴스마다),
     * 지금 켜 둔 링크의 번호들 — 초점이 바뀌면 그것만 끄고 새로 켠다. */
    node3: null,
    node2: null,
    nodeTextures: [],
    edges: null,
    threadLit: null,
    threadLitAttr: null,
    fiberLit: null,
    fiberLitAttr: null,
    fiberGeometry: null,
    threadGeometry: null,
    /* 카메라에서 가장 가까운 별의 깊이(`project`) — 현이 보일 만큼 가까운 별이 있는지를 잰다(`spanLinks`). */
    nearest: 0,
    litList: null,
    litCount: 0,
    /* 세계의 1이 깊이 1에서 차지하는 CSS px(`placeCamera`) — 군집의 화면 반지름을 잰다. 군집마다 별 사이 간격(`rowGaps`). */
    focal: 1,
    gaps: null,
    proj: null,
    projVersion: -1,
    viewProjection: null,
    /* 초점: 올린 별(우주만의 것), 찾은 별의 수, 마지막으로 본 고름·군집(바뀌면 난다). */
    hover: -1,
    hits: 0,
    seenSelected: null,
    seenTopic: -1,
    pickQueued: false,
    /* 고른 쪽의 자리 — `refreshStates`가 적고 이름표가 읽는다. */
    selectedSeat: -1,
    /* 이름표(시안 `buildTopicCards`·`updateLabels`): 큰 군집마다 명판, 쪽 이름의 풀, 굵은 필라멘트의 수, 올린 별의 팁.
     * 자리는 camVersion이 바뀐 프레임에만 다시 놓고(`labelVersion`), 겹침은 이미 놓인 상자들(`boxes`)로 막는다.
     * 명판의 크기는 한 번 재고(`measured`), 쪽 이름의 넓이는 쪽마다 한 번 잰다(`nameWidths`). */
    plates: [],
    names: [],
    flowTags: [],
    tip: null,
    tipBox: null,
    labelVersion: -1,
    measured: false,
    boxes: null,
    boxCount: 0,
    anchorX: null,
    anchorLow: null,
    anchorHigh: null,
    anchorSeen: null,
    order: null,
    candidates: null,
    candidateScores: null,
    bright: [],
    declared: null,
    inferred: null,
    nameWidths: null,
    nameFont: "",
    namePad: 0,
    nameHotPad: 0,
    nameContext: null,
    spot: new Float64Array(3),
    inks: null,
    probe: null,
    post: null,
    floatTargets: false,
    themeWatch: null,
    /* 카메라(시안 `S`): 궤도의 두 각, 거리, 목표점, 평면에서 떠오른 몫. */
    yaw: U.homeYaw,
    pitch: U.homePitch,
    dist: 300,
    tx: 0,
    ty: 0,
    tz: 0,
    lift: 1,
    home: null,
    saved: null,
    camVersion: 0,
    lastTouch: -1e9,
    dragging: false,
    tween: { on: false, t0: 0, dur: 1, from: {}, to: {}, done: null },
    /* 첫 그림(`bear`) — 별이 허브에서 퍼지는 동안. */
    birth: { on: false, t0: 0 },
    glide: { on: false, vx: 0, vy: 0 },
    spin: 1,
    time: 0,
    raf: 0,
    /* 쉬는 동안의 움직임만 남은 판이 다음 장을 기다리는 타이머(`ambient-gap`). */
    wait: 0,
    lastNow: 0,
    frames: 0,
    width: 1,
    height: 1,
    lost: false,
    folding: false,
    /* 사람의 「움직임」 — 끈 판과 움직임을 줄이라는 판에서는 돌지도 흐르지도 않는다. */
    motion: knowledgeUniverseMotion,
    down: null,
    moved: { x: 0, y: 0, t: 0 },
    clickable: false,
    pointer: { x: -1, y: -1, inside: false },
    unlisten: [],

    /* 캔버스·이름표 층·렌더러를 세운다. 서지 못하면 거짓 — 부르는 쪽이 놓고 평면으로 선다. */
    mount() {
      const stage = this.view.querySelector(".knowledge-canvas");
      if (stage === null || typeof THREE !== "object") return false;
      const host = document.createElement("div");
      host.className = "knowledge-universe";
      const canvas = document.createElement("canvas");
      canvas.className = "knowledge-universe-gl";
      canvas.setAttribute("role", "img");
      const labels = document.createElement("div");
      labels.className = "knowledge-universe-labels";
      /* 색을 묻는 견본 하나 — 군집 빛깔의 원천과 종이를 계산된 색으로 읽는다(`knowledgeUniverseInks`). */
      const probe = document.createElement("span");
      probe.className = "knowledge-universe-probe";
      probe.setAttribute("aria-hidden", "true");
      host.append(canvas, labels, probe);
      this.probe = probe;
      stage.appendChild(host);
      this.host = host;
      this.canvas = canvas;
      this.labels = labels;
      let renderer = null;
      try {
        /* 깊이도 스텐실도 없다 — 모든 것이 부드러운 스프라이트이고 빛은 더해진다(시안 `makeRenderer`). */
        renderer = new THREE.WebGLRenderer({ canvas, antialias: false, alpha: false, depth: false, stencil: false,
          powerPreference: "high-performance" });
      } catch (trouble) {
        invoke("note_webview_error", { text: `knowledge universe: ${trouble}` }).catch(() => {});
        return false;
      }
      this.renderer = renderer;
      if (!renderer.capabilities.isWebGL2) return false;
      renderer.toneMapping = THREE.NoToneMapping;
      renderer.sortObjects = false;
      renderer.autoClear = false;
      renderer.info.autoReset = false;
      /* 셰이더가 서지 않으면 three.js는 콘솔에 적고 빈 그림을 그린다 — 이 손은 던져서 평면으로
       * 돌아간다(`build`의 `compile`이 받는다). */
      renderer.debug.onShaderError = (context, program, vertexShader, fragmentShader) => {
        throw new Error([context.getProgramInfoLog(program),
          context.getShaderInfoLog(vertexShader), context.getShaderInfoLog(fragmentShader)]
          .filter((word) => word).join(" / "));
      };
      this.scene = new THREE.Scene();
      /* 원근 카메라 하나(시안은 번들에 이것이 없어 직교 카메라의 투영을 갈아 끼웠다 — 앱의 번들은 이것을
       * 싣는다, ui/vendor/build-three.mjs). 비는 판 크기가 정한다(`resize`). */
      const camera = new THREE.PerspectiveCamera(U.fov, 1, U.near, U.far);
      this.camera = camera;
      /* 모든 재질이 함께 보는 값: 평면↔우주의 몫, 시간, 초점과 초점 군집(쪽의 초점인가), 흐름, 첫 그림의 몫, 기기 픽셀, 밝기 기준 거리(처음 자리
       * 거리), 켜진 링크의 줄임, 색 없는 빛깔, 초점 거리(CSS px)와 붐빔의 토큰 넷, 군집 텍셀, 쪽 자리 둘과 그 폭. */
      this.uniforms = {
        uLift: { value: 1 }, uTime: { value: 0 }, uFocus: { value: 0 }, uFlow: { value: 1 }, uFocusGal: { value: -1 },
        uFocusPage: { value: 0 }, uBirth: { value: 1 }, uDpr: { value: 1 }, uRef: { value: 360 }, uLitGain: { value: 1 }, uNeutral: { value: new THREE.Vector3(1, 1, 1) },
        uFocal: { value: 1 }, uHalo: { value: 1 }, uCrowd: { value: new THREE.Vector4(U.crowdRatio, U.crowdLines, U.crowdFade, U.crowdLeast) },
        tGal: { value: null }, tNode3: { value: null }, tNode2: { value: null }, uTexW: { value: 1 },
      };
      /* 섞기: 미리 곱한 알파(ONE, ONE_MINUS_SRC_ALPHA) — 빛(α=0)은 더한다(시안 `mat`). */
      const material = (vertexShader, fragmentShader) => new THREE.ShaderMaterial({
        uniforms: this.uniforms, vertexShader, fragmentShader, transparent: true, depthTest: false, depthWrite: false,
        blending: THREE.NormalBlending, premultipliedAlpha: true,
      });
      this.materials = {
        aura: material(KNOWLEDGE_UNIVERSE_AURA_VERT, KNOWLEDGE_UNIVERSE_AURA_FRAG),
        sky: material(KNOWLEDGE_UNIVERSE_SKY_VERT, KNOWLEDGE_UNIVERSE_SKY_FRAG),
        threads: material(KNOWLEDGE_UNIVERSE_THREADS_VERT, KNOWLEDGE_UNIVERSE_EDGE_FRAG),
        fibers: material(KNOWLEDGE_UNIVERSE_FIBERS_VERT, KNOWLEDGE_UNIVERSE_EDGE_FRAG),
        stars: material(KNOWLEDGE_UNIVERSE_STARS_VERT, KNOWLEDGE_UNIVERSE_STARS_FRAG),
      };
      this.floatTargets = renderer.extensions.has("EXT_color_buffer_float")
        || renderer.extensions.has("EXT_color_buffer_half_float");
      this.makePost();
      this.inks = knowledgeUniverseInks(this.view, this.probe);
      this.applyInks();
      /* 테마가 바뀌면 색을 다시 묻고 한 장을 그린다(평면의 GL 손과 같은 귀). */
      this.themeWatch = new MutationObserver(() => {
        if (this.renderer === null) return;
        this.inks = knowledgeUniverseInks(this.view, this.probe);
        this.applyInks();
        this.invalidate();
      });
      this.themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
      this.wire();
      return true;
    },

    /* 후처리의 살림(시안 `down`·`up`·`comp`·`post`): 전면 삼각형 하나와 재질 셋. 목표는 판 크기를 따라
     * `makeTargets`가 짓는다. */
    makePost() {
      const U = this.tune;
      const pass = (options) => new THREE.ShaderMaterial({ vertexShader: KNOWLEDGE_UNIVERSE_POST_VERT,
        depthTest: false, depthWrite: false, transparent: false, ...options });
      const down = pass({
        uniforms: { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() }, uKaris: { value: 0 } },
        fragmentShader: KNOWLEDGE_UNIVERSE_DOWN_FRAG,
      });
      const up = pass({
        transparent: true, blending: THREE.AdditiveBlending, premultipliedAlpha: true,
        uniforms: { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() } },
        fragmentShader: KNOWLEDGE_UNIVERSE_UP_FRAG,
      });
      const comp = pass({
        uniforms: { tScene: { value: null }, tBloom: { value: null }, uBloom: { value: 0 },
          uExposure: { value: 1 }, uDark: { value: 1 }, uPaper: { value: new THREE.Vector3() }, uTime: { value: 0 },
          uKeep: { value: U.toneKeep }, uInk: { value: U.inkChroma } },
        fragmentShader: KNOWLEDGE_UNIVERSE_COMP_FRAG,
      });
      const triangle = new THREE.BufferGeometry();
      triangle.setAttribute("position", new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), 3));
      const quad = new THREE.Mesh(triangle, comp);
      quad.frustumCulled = false;
      const scene = new THREE.Scene();
      scene.add(quad);
      this.post = { scene, lens: new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1), quad, triangle, down, up, comp,
        target: null, mips: [] };
    },

    /* 장면의 HDR 목표 한 장과 빛 번짐 사다리(한 단마다 절반). 깊이·스텐실은 없다. 반정밀 부동소수를 쓸 수 없는
     * 판은 RGBA8로 내려앉는다(번짐이 약해진다). */
    makeTargets() {
      this.dropTargets();
      const post = this.post;
      const wide = Math.max(1, this.canvas.width);
      const tall = Math.max(1, this.canvas.height);
      const options = { depthBuffer: false, stencilBuffer: false };
      if (this.floatTargets) options.type = THREE.HalfFloatType;
      post.target = new THREE.WebGLRenderTarget(wide, tall, options);
      let mipWide = wide;
      let mipTall = tall;
      for (let level = 0; level < this.tune.bloomLevels; level += 1) {
        mipWide = Math.max(1, mipWide >> 1);
        mipTall = Math.max(1, mipTall >> 1);
        post.mips.push(new THREE.WebGLRenderTarget(mipWide, mipTall, options));
      }
    },

    dropTargets() {
      const post = this.post;
      if (post === null) return;
      post.target?.dispose();
      for (const mip of post.mips) mip.dispose();
      post.target = null;
      post.mips.length = 0;
    },

    /* 색이 바뀌었다(처음·테마): 합성의 테마·노출·종이, 색 없는 빛깔, 그리고 군집 빛깔(텍셀 2). */
    applyInks() {
      const inks = this.inks;
      const comp = this.post.comp.uniforms;
      comp.uDark.value = inks.dark ? 1 : 0;
      comp.uExposure.value = inks.exposure;
      comp.uPaper.value.set(inks.paper[0], inks.paper[1], inks.paper[2]);
      this.uniforms.uNeutral.value.set(inks.neutral[0], inks.neutral[1], inks.neutral[2]);
      this.uniforms.uHalo.value = inks.halo;
      if (this.map === null) return;
      const U = this.tune;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      for (const row of this.map.galaxy) {
        const at = (row.rank * SHAPE.texels + 2) * 4;
        const tint = knowledgeUniverseTint(U, inks, row.hue);
        for (let channel = 0; channel < 3; channel += 1) this.map.gal[at + channel] = tint[channel];
      }
      if (this.galTexture !== null) this.galTexture.needsUpdate = true;
    },

    /* 손과 귀. 우주 위의 몸짓은 우주의 것이다 — 평면 캔버스의 손(끌기·고르기·바퀴)에 닿지 않게
     * 여기서 멈춘다. */
    wire() {
      const on = (target, name, run, options) => {
        target.addEventListener(name, run, options);
        this.unlisten.push(() => target.removeEventListener(name, run, options));
      };
      const host = this.host;
      on(this.canvas, "webglcontextlost", (event) => {
        event.preventDefault();
        this.loseContext();
      });
      on(this.canvas, "webglcontextrestored", () => this.regainContext());
      on(document, "visibilitychange", () => {
        if (document.hidden) this.halt();
        else this.invalidate();
      });
      on(host, "pointerdown", (event) => {
        event.stopPropagation();
        if (event.target.closest("button, a, input")) return;
        if (event.button !== 0 && event.button !== 2) return;
        host.setPointerCapture?.(event.pointerId);
        this.down = { x: event.clientX, y: event.clientY, pan: event.shiftKey || event.button === 2 };
        this.moved = { x: event.clientX, y: event.clientY, t: performance.now() };
        this.clickable = true;
        this.glide.on = false;
        this.lastTouch = performance.now();
        this.view.querySelector(".knowledge-canvas")?.focus({ preventScroll: true });
      });
      on(host, "pointermove", (event) => {
        event.stopPropagation();
        const box = host.getBoundingClientRect();
        this.pointer.x = event.clientX - box.left;
        this.pointer.y = event.clientY - box.top;
        this.pointer.inside = true;
        if (this.down === null) {
          this.schedulePick();
          return;
        }
        this.drag(event);
      });
      const release = (event) => {
        event.stopPropagation();
        if (this.down === null) return;
        const dragged = this.dragging;
        this.dragging = false;
        host.classList.remove("is-dragging");
        this.down = null;
        if (dragged && !knowledgeMotionReduced() && performance.now() - this.moved.t < U.glideWindow) {
          this.glide.on = true;
          this.glide.vx = Math.max(-U.glideYaw, Math.min(U.glideYaw, this.glide.vx));
          this.glide.vy = Math.max(-U.glidePitch, Math.min(U.glidePitch, this.glide.vy))
            * U.glidePitchShare;
          this.invalidate();
        }
        if (this.clickable && event.type === "pointerup") this.press(event.shiftKey);
      };
      on(host, "pointerup", release);
      on(host, "pointercancel", release);
      on(host, "pointerleave", () => {
        this.pointer.inside = false;
        if (this.down === null) this.setHover(-1);
      });
      for (const name of ["click", "pointerover", "pointerout"]) on(host, name, (event) => event.stopPropagation());
      on(host, "dblclick", (event) => {
        event.stopPropagation();
        if (event.target.closest("button")) return;
        this.goHome();
      });
      on(host, "contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      on(host, "wheel", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (this.folding) return;
        this.lastTouch = performance.now();
        const step = Math.exp(event.deltaY * U.zoomWheel);
        this.dist = Math.max(U.zoomMin, Math.min(this.home.dist * U.zoomMax, this.dist * step));
        this.camVersion += 1;
        this.tween.on = false;
        this.invalidate();
      }, { passive: false });
      watchGraphResize(host, () => this.resize());
    },

    /* 끄는 손 — 궤도, 또는 Shift·오른쪽 끌기로 옮기기(시안의 손 그대로). */
    drag(event) {
      if (this.folding) return;
      const dx = event.clientX - this.moved.x;
      const dy = event.clientY - this.moved.y;
      if (Math.abs(event.clientX - this.down.x) + Math.abs(event.clientY - this.down.y) > U.dragSlop) {
        this.clickable = false;
        this.dragging = true;
        this.host.classList.add("is-dragging");
      }
      if (!this.dragging) return;
      const now = performance.now();
      const span = Math.max(1, now - this.moved.t);
      if (this.down.pan) {
        const step = this.dist * U.dragPan;
        const cos = Math.cos(this.yaw);
        const sin = Math.sin(this.yaw);
        this.tx -= (dx * cos - dy * sin) * step;
        this.tz += (-dx * sin - dy * cos) * step;
      } else {
        this.yaw -= dx * U.dragYaw;
        this.pitch = this.clampPitch(this.pitch + dy * U.dragPitch);
        this.glide.vx = (dx * U.dragYaw) / (span / 1000);
        this.glide.vy = (dy * U.dragPitch) / (span / 1000);
      }
      this.tween.on = false;
      this.camVersion += 1;
      this.moved = { x: event.clientX, y: event.clientY, t: now };
      this.lastTouch = now;
      this.invalidate();
    },

    clampPitch(pitch) {
      return Math.max(U.pitchLow, Math.min(U.pitchHigh, pitch));
    },

    /* 판 크기 — 캔버스의 기기 픽셀과 카메라의 비, 처음 자리. */
    resize() {
      if (this.renderer === null) return;
      const box = this.host.getBoundingClientRect();
      this.width = Math.max(1, Math.round(box.width));
      this.height = Math.max(1, Math.round(box.height));
      this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, U.pixelCap));
      this.renderer.setSize(this.width, this.height, false);
      this.camera.aspect = this.width / this.height;
      this.camera.updateProjectionMatrix();
      this.makeTargets();
      if (this.layout !== null) this.computeHome();
      this.camVersion += 1;
      this.invalidate();
    },

    /* 이 판의 그림을 짓는다 — 위상이 바뀐 판에서 다시. 셰이더가 서지 않으면 거짓. */
    build(layout) {
      this.clearScene();
      this.layout = layout;
      this.seatsRevision = -1;
      const model = layout.model;
      const count = layout.count;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      const map = knowledgeUniverseMap(layout, this.inks);
      this.map = map;
      this.seenStamp = map.stamp;
      this.scale = map.scale;
      this.reach = map.reach;
      this.pos3 = map.pos3;
      this.pos2 = new Float32Array(count * 3);
      this.filaments = map.filaments;
      /* 군집 텍스처 — 군집마다 텍셀 넷, 줄 128(RGBA32F, NEAREST). 셰이더가 번호로 읽는다. */
      const galTexture = new THREE.DataTexture(map.gal, SHAPE.texels, SHAPE.rows, THREE.RGBAFormat, THREE.FloatType);
      galTexture.minFilter = THREE.NearestFilter;
      galTexture.magFilter = THREE.NearestFilter;
      galTexture.needsUpdate = true;
      this.galTexture = galTexture;
      this.uniforms.tGal.value = galTexture;
      /* 별: 크기의 바탕 = ((1 + 연결) / (1 + 가운데값))^지수(천장까지), 최근 = 최근 고침(오늘 1 → 1년 0, 로그), 줄 번호,
       * 빛의 몫. 고친 때를 모르는 쪽은 오래된 것으로 친다(디자이너 답 m-12467). 가운데값은 볼트 전체의 것이다. */
      const sorted = Int32Array.from(model.degree).sort();
      const middle = count > 0 ? sorted[count >> 1] : 0;
      const nowMs = model.nowMs;
      const star = new Float32Array(count * 4);
      for (let at = 0; at < count; at += 1) {
        const days = model.modified[at] > 0 ? Math.max(0, (nowMs - model.modified[at]) / 86_400_000)
          : U.starAgeSpan;
        const row = map.rowOf[at];
        star[at * 4] = Math.min((1 + model.degree[at]) / (1 + middle), U.starRatioCap) ** U.starLumExp;
        star[at * 4 + 1] = Math.max(0, Math.min(1,
          1 - Math.log1p(days / U.starAgeScale) / Math.log1p(U.starAgeSpan / U.starAgeScale)));
        star[at * 4 + 2] = row;
        /* 가려진 쪽의 별은 빛이 없다 — 셰이더가 그리지 않고, 고르기와 이름표도 건너뛴다(`project`). */
        star[at * 4 + 3] = map.shown !== null && map.shown[at] === 0 ? 0 : row >= 0 ? map.galaxy[row].weight : U.quietShare;
      }
      this.placeSeats();
      /* 그리는 차례: 둘레 빛 0 → 배경 별 1 → 곧은 링크 2 → 다발의 올 3 → 별 4. 장면에 넣는 차례가 그 차례다
       * (정렬은 끈다). */
      const add = (object, order, key) => {
        object.frustumCulled = false;
        object.renderOrder = order;
        object.userData.key = key;
        this.scene.add(object);
        this.built.objects.push(object);
        return object;
      };
      const auraGeometry = new THREE.InstancedBufferGeometry();
      auraGeometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 1, -1, 0, 1, 1, 0, -1, 1, 0]), 3));
      auraGeometry.setIndex([0, 1, 2, 0, 2, 3]);
      auraGeometry.setAttribute("aRow", new THREE.InstancedBufferAttribute(Float32Array.from(map.galaxy, (row) => row.rank), 1));
      auraGeometry.instanceCount = map.galaxy.length;
      this.built.geometries.push(auraGeometry);
      add(new THREE.Mesh(auraGeometry, this.materials.aura), 0, "aura").visible = map.galaxy.length > 0;
      const sky = new Float32Array(U.decorSky);
      for (let step = 0; step < sky.length; step += 1) sky[step] = step;
      const skyGeometry = new THREE.BufferGeometry();
      skyGeometry.setAttribute("position", new THREE.BufferAttribute(sky, 1));
      this.built.geometries.push(skyGeometry);
      this.skyCount = sky.length;
      add(new THREE.Points(skyGeometry, this.materials.sky), 1, "sky");
      /* 쪽 자리 텍스처 둘(우주·평면, RGBA32F NEAREST) — 링크가 번호로 읽는다. */
      const wide = SHAPE.nodeWide;
      const tall = Math.max(1, Math.ceil(count / wide));
      this.node3 = new Float32Array(wide * tall * 4);
      this.node2 = new Float32Array(wide * tall * 4);
      const nodeTexture = (data) => {
        const texture = new THREE.DataTexture(data, wide, tall, THREE.RGBAFormat, THREE.FloatType);
        texture.minFilter = THREE.NearestFilter;
        texture.magFilter = THREE.NearestFilter;
        texture.needsUpdate = true;
        return texture;
      };
      this.nodeTextures = [nodeTexture(this.node3), nodeTexture(this.node2)];
      this.uniforms.tNode3.value = this.nodeTextures[0];
      this.uniforms.tNode2.value = this.nodeTextures[1];
      this.uniforms.uTexW.value = wide;
      this.fillNodes();
      /* 링크의 두 통 — 곧은 링크는 끝점 둘, 올은 인스턴스 하나에 마디 `fiberSegments`. 켜짐만 프레임 사이에 바뀐다. */
      const edges = knowledgeUniverseEdges(model, map);
      this.edges = edges;
      this.declared = edges.declared;
      this.inferred = edges.inferred;
      /* 켜 둔 링크의 장부 — 초점 쪽의 링크와 경로의 링크가 겹칠 수 있어 링크 수의 두 배까지 든다. */
      this.litList = new Int32Array(Math.max(1, model.edgeCount * 2));
      this.litCount = 0;
      this.threadLit = new Float32Array(edges.threads * 2);
      this.threadLitAttr = new THREE.BufferAttribute(this.threadLit, 1);
      this.threadLitAttr.setUsage(THREE.DynamicDrawUsage);
      const threadGeometry = new THREE.BufferGeometry();
      threadGeometry.setAttribute("position", new THREE.BufferAttribute(edges.seat, 3));
      threadGeometry.setAttribute("aFlow", new THREE.BufferAttribute(edges.flow, 3));
      threadGeometry.setAttribute("aLit", this.threadLitAttr);
      this.threadGeometry = threadGeometry;
      this.built.geometries.push(threadGeometry);
      add(new THREE.LineSegments(threadGeometry, this.materials.threads), 2, "threads").visible = edges.threads > 0;
      const segments = SHAPE.fiberSegments;
      const base = new Float32Array(segments * 2 * 3);
      for (let step = 0; step < segments; step += 1) {
        base[step * 6] = step / segments;
        base[step * 6 + 3] = (step + 1) / segments;
      }
      this.fiberLit = new Float32Array(edges.fibers);
      this.fiberLitAttr = new THREE.InstancedBufferAttribute(this.fiberLit, 1);
      this.fiberLitAttr.setUsage(THREE.DynamicDrawUsage);
      const fiberGeometry = new THREE.InstancedBufferGeometry();
      fiberGeometry.setAttribute("position", new THREE.BufferAttribute(base, 3));
      fiberGeometry.setAttribute("aFiber", new THREE.InstancedBufferAttribute(edges.fiber, 4));
      fiberGeometry.setAttribute("aFiberFlow", new THREE.InstancedBufferAttribute(edges.fiberFlow, 2));
      fiberGeometry.setAttribute("aFiberLit", this.fiberLitAttr);
      fiberGeometry.instanceCount = edges.thick;
      this.fiberGeometry = fiberGeometry;
      this.built.geometries.push(fiberGeometry);
      add(new THREE.LineSegments(fiberGeometry, this.materials.fibers), 3, "fibers").visible = edges.fibers > 0;
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(this.pos3, 3));
      geometry.setAttribute("aPos2", new THREE.BufferAttribute(this.pos2, 3));
      geometry.setAttribute("aStar", new THREE.BufferAttribute(star, 4));
      geometry.setAttribute("aRoom", new THREE.BufferAttribute(map.room, 1));
      this.starState = new Float32Array(count);
      this.starStateAttr = new THREE.BufferAttribute(this.starState, 1);
      this.starStateAttr.setUsage(THREE.DynamicDrawUsage);
      geometry.setAttribute("aState", this.starStateAttr);
      this.built.geometries.push(geometry);
      this.starGeometry = geometry;
      add(new THREE.Points(geometry, this.materials.stars), 4, "stars");
      this.proj = new Float32Array(count * 4);
      this.gaps = new Float32Array(Math.max(1, map.rows));
      this.projVersion = -1;
      this.viewProjection = new THREE.Matrix4();
      this.hover = -1;
      /* 쉬는 먼 자리에서 이름을 다는 별 — 연결이 가장 많은 쪽들(같으면 자리 순). 제목이 제 군집의 이름과 같은 별(이름
       * 없는 군집은 허브 제목이 이름이다)은 명판과 두 번 서지 않게 건너뛴다(디자이너 m-12642). */
      const sameAsGalaxy = (seat) => map.rowOf[seat] >= 0
        && knowledgeShortWord(model.titles[seat], KNOWLEDGE_COMMUNITY.labelMax) === knowledgeClusterWord(layout, map.rowOf[seat]);
      const byLinks = Array.from({ length: count }, (unused, at) => at)
        .sort((one, two) => model.degree[two] - model.degree[one] || one - two);
      this.bright = [];
      for (const seat of byLinks) {
        if (this.bright.length >= U.labelsRest) break;
        if ((map.shown === null || map.shown[seat] === 1) && !sameAsGalaxy(seat)) this.bright.push(seat);
      }
      this.applyInks();
      this.computeHome();
      this.buildLabels();
      writeAttribute(this.canvas, "aria-label", t("knowledge.universeSummary", "지식 그래프 우주 · 주제 {{topics}} · 쪽 {{pages}}",
        { topics: layout.namedCount, pages: count }));
      try {
        this.placeCamera();
        this.renderer.compile(this.scene, this.camera);
        this.renderer.compile(this.post.scene, this.post.lens);
      } catch (trouble) {
        invoke("note_webview_error", { text: `knowledge universe: ${trouble}` }).catch(() => {});
        return false;
      }
      return true;
    },

    /* 평면 자리(떠오르기 전)와 떠돌이 별의 자리. 평면 자리는 평면 지도가 그린 자리(좁은 판의 세로 눌림까지)이고,
     * 군집의 둘레 빛은 평면에서 제 원반의 가운데로 모인다(텍셀 1). 떠돌이 별은 평면 자리 그대로에 높이만 — 앉기가
     * 끝난 판에서 한 번 더 옮긴다(`dress`). */
    placeSeats() {
      const layout = this.layout;
      const map = this.map;
      const { count, x, y, drawY, model } = layout;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      const s = this.scale;
      const flat = U.flatHeight;
      const box = layout.viewBoxRect;
      const middleY = box?.middleY ?? 0;
      const yScale = box?.yScale ?? 1;
      for (let at = 0; at < count; at += 1) {
        this.pos2[at * 3] = x[at] * s;
        this.pos2[at * 3 + 1] = flat;
        this.pos2[at * 3 + 2] = drawY[at] * s;
        if (map.rowOf[at] >= 0) continue;
        const seed = knowledgeHash(model.keys[at]);
        this.pos3[at * 3] = x[at] * s;
        this.pos3[at * 3 + 1] = (knowledgeUniverseHash(seed + 5) - 0.5) * 2 * U.liftOrphan * this.reach;
        this.pos3[at * 3 + 2] = y[at] * s;
      }
      for (const row of map.galaxy) {
        const at = (row.rank * SHAPE.texels + 1) * 4;
        map.gal[at] = layout.communityHomeX[row.rank] * s;
        map.gal[at + 1] = flat;
        map.gal[at + 2] = (middleY + (layout.communityHomeY[row.rank] - middleY) * yScale) * s;
        map.gal[at + 3] = layout.communityHomeR[row.rank] * s;
      }
      this.seatsRevision = layout.geometryRevision;
      this.projVersion = -1;
      this.fillNodes();
      if (this.galTexture !== null) this.galTexture.needsUpdate = true;
      if (this.starGeometry !== null) {
        this.starGeometry.attributes.position.needsUpdate = true;
        this.starGeometry.attributes.aPos2.needsUpdate = true;
      }
    },

    /* 쪽 자리 텍스처를 채운다 — 우주의 자리(텍셀 = x, y, z, 줄 번호)와 평면의 자리. 떠돌이 별의 줄은 -1이다. */
    fillNodes() {
      if (this.node3 === null) return;
      const count = this.layout.count;
      const rowOf = this.map.rowOf;
      for (let at = 0; at < count; at += 1) {
        for (let axis = 0; axis < 3; axis += 1) {
          this.node3[at * 4 + axis] = this.pos3[at * 3 + axis];
          this.node2[at * 4 + axis] = this.pos2[at * 3 + axis];
        }
        this.node3[at * 4 + 3] = rowOf[at];
        this.node2[at * 4 + 3] = rowOf[at];
      }
      for (const texture of this.nodeTextures) texture.needsUpdate = true;
    },

    /* 처음 자리(시안 `computeHome`) — 닿는 거리의 구가 판에 들게. */
    computeHome() {
      const R = Math.max(this.reach + U.homePad, U.homeLeast);
      const vertical = Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2);
      const horizontal = vertical * Math.max(U.homeAspect, this.width / this.height);
      const dist = Math.max(R / horizontal, (R * U.homeTall) / vertical) * U.homeFit;
      this.home = { yaw: U.homeYaw, pitch: U.homePitch, dist, tx: 0, ty: 0, tz: R * U.homeLift };
      this.uniforms.uRef.value = dist;
      this.camera.far = Math.max(U.far, this.reach * U.farReach);
      this.camera.updateProjectionMatrix();
    },

    /* 평면 지도의 자세 — 지금 평면 카메라(`layout.viewBoxRect`)가 보는 그대로를 위에서 내려다본다. */
    flatPose() {
      const layout = this.layout;
      const box = layout.viewBoxRect;
      const s = this.scale;
      if (box === null) return { ...this.home, lift: 0 };
      const perWorld = box.scale / s;
      const dist = this.height / (2 * perWorld * Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2));
      return { yaw: 0, pitch: U.flatPitch, dist, tx: (box.x + box.wide / 2) * s, ty: U.flatHeight,
        tz: (box.y + box.tall / 2) * s, lift: 0 };
    },

    /* 판에 선다. 사람이 3D를 누른 판은 평면 자세에서 떠오르고(시안 `setMode("3d")`), 문을 연 판은 곧바로
     * 처음 자리에 선다(디자이너 답 8). */
    enter(rising) {
      this.view.classList.add("is-universe");
      /* 서는 순간의 고름·군집은 이미 본 것이다 — 우주는 처음 자리(또는 평면 자세)에서 선다. */
      this.seenSelected = knowledgeSelectedKey;
      this.seenTopic = knowledgeClusterPicked;
      this.resize();
      this.lastTouch = performance.now() - U.openRest;
      if (rising) {
        Object.assign(this, this.flatPose());
        this.camVersion += 1;
        this.flyTo({ ...(this.saved ?? this.home), lift: 1 }, U.rise);
      } else {
        Object.assign(this, this.home, { lift: 1 });
        this.camVersion += 1;
        this.bear();
      }
      this.refreshStates();
      /* 이 판이 덮는 동안 평면의 손은 그리기 버퍼를 돌려준다(`knowledgeUniverseCovers`). */
      knowledgePainters.get(this.view)?.park?.();
      this.invalidate();
    },

    /* 문을 연 판의 첫 그림 — 별들이 제 군집의 가운데에서 제 자리로 퍼진다(`KNOWLEDGE_UNIVERSE_BIRTH`). 움직임을 줄인
     * 판은 곧바로 선다. */
    bear() {
      const still = knowledgeMotionReduced() || !(U.birth > 0);
      this.birth.on = !still;
      this.birth.t0 = performance.now();
      this.uniforms.uBirth.value = still ? 1 : 0;
    },

    /* 평면으로 내려앉는다(시안 `setMode("2d")`) — 끝나면 부르는 쪽이 놓는다. 움직임을 줄인 판과 문맥을
     * 잃은 판은 곧바로. */
    fold(done) {
      if (this.folding) return;
      this.folding = true;
      this.saved = { yaw: this.yaw, pitch: this.pitch, dist: this.dist, tx: this.tx, ty: this.ty, tz: this.tz };
      if (this.lost || this.renderer === null || knowledgeMotionReduced()) {
        done();
        return;
      }
      this.flyTo(this.flatPose(), U.fold, done);
    },

    /* 내려앉는 중에 다시 3D를 눌렀다 — 접힘을 거두고 그 자리에서 다시 떠오른다. 새 비행이 접힘 끝의 약속(놓기)을
     * 갈아 치운다. */
    unfold() {
      if (!this.folding) return;
      this.folding = false;
      this.flyTo({ ...(this.saved ?? this.home), lift: 1 }, U.rise);
      this.invalidate();
    },

    /* 비행(시안 `flyTo`). 움직임을 줄인 판은 곧바로 도착한다. */
    flyTo(to, dur = U.fly, done = null) {
      const from = this.tween.from;
      for (const key of ["yaw", "pitch", "dist", "tx", "ty", "tz", "lift"]) from[key] = this[key];
      const target = this.tween.to;
      for (const key of ["yaw", "pitch", "dist", "tx", "ty", "tz", "lift"]) target[key] = to[key] ?? this[key];
      let turn = target.yaw - from.yaw;
      while (turn > Math.PI) turn -= Math.PI * 2;
      while (turn < -Math.PI) turn += Math.PI * 2;
      target.yaw = from.yaw + turn;
      this.tween.done = done;
      if (knowledgeMotionReduced() || dur <= 0) {
        this.tween.on = false;
        this.applyTween(1);
        this.tween.done = null;
        done?.();
        this.invalidate();
        return;
      }
      this.tween.on = true;
      this.tween.t0 = performance.now();
      this.tween.dur = dur;
      this.invalidate();
    },

    applyTween(progress) {
      const eased = progress < 0.5 ? 4 * progress ** 3 : 1 - (-2 * progress + 2) ** 3 / 2;
      const from = this.tween.from;
      const to = this.tween.to;
      this.yaw = from.yaw + (to.yaw - from.yaw) * eased;
      this.pitch = from.pitch + (to.pitch - from.pitch) * eased;
      this.dist = from.dist * (to.dist / from.dist) ** eased;
      this.tx = from.tx + (to.tx - from.tx) * eased;
      this.ty = from.ty + (to.ty - from.ty) * eased;
      this.tz = from.tz + (to.tz - from.tz) * eased;
      this.lift = from.lift + (to.lift - from.lift) * eased;
      this.camVersion += 1;
    },

    /* 누름: 별이면 그 쪽을 고른다 — 평면 지도와 같은 고름이라 인스펙터가 카드를 열고, 고름이 바뀐 것을 `dress`가 보고
     * 그 별로 난다. 고른 쪽이 있을 때 Shift로 누른 다른 별은 그 둘 사이의 경로다(평면 지도의 손 그대로 — 경로의
     * 링크가 그물을 따라 켜진다). 빈 곳이면 경로·고름·고른 군집을 놓는다. */
    press(shift = false) {
      if (this.folding || this.map === null) return;
      const seat = this.pickAt(this.pointer.x, this.pointer.y);
      if (seat >= 0) {
        const key = this.layout.model.keys[seat];
        if (shift && knowledgeSelectedKey !== null && key !== knowledgeSelectedKey) {
          void runKnowledgeShortestPath(this.view, this.layout, knowledgeSelectedKey, key);
          return;
        }
        clearKnowledgePath(this.view, this.layout);
        selectKnowledgeNode(this.view, key);
        return;
      }
      clearKnowledgePath(this.view, this.layout);
      if (knowledgeClusterPicked >= 0) toggleKnowledgeCluster(this.view, knowledgeClusterPicked);
      if (knowledgeSelectedKey !== null) selectKnowledgeNode(this.view, null);
    },

    /* 쪽으로 난다 — 그 별에, 제 군집이 판에 드는 거리까지. 보는 방향은 그대로다. */
    flyToStar(seat) {
      const row = this.map.rowOf[seat] >= 0 ? this.map.galaxy[this.map.rowOf[seat]] : null;
      this.flyTo({ tx: this.pos3[seat * 3], ty: this.pos3[seat * 3 + 1], tz: this.pos3[seat * 3 + 2],
        dist: Math.max(U.flyPageDist, (row?.radius ?? 0) * U.flyPageReach) }, U.flyPage);
    },

    /* 군집으로 난다 — 그 나무가 판에 가득 차는 거리까지. */
    flyToGalaxy(rank) {
      const row = this.map.galaxy[rank];
      if (row === undefined) return;
      this.flyTo({ tx: row.x, ty: row.y, tz: row.z, dist: row.radius * U.flyGalaxyReach + U.flyGalaxyPad }, U.fly);
    },

    /* 필라멘트 하나를 옆에서 보는 자리로 난다(시안 `showFlow`). */
    flyToFlow(from, to) {
      const a = this.map.galaxy[from];
      const b = this.map.galaxy[to];
      if (a === undefined || b === undefined) return;
      this.flyTo({ tx: (a.x + b.x) / 2, ty: (a.y + b.y) / 2, tz: (a.z + b.z) / 2,
        dist: Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z) * U.flyFlowReach + U.flyFlowPad,
        yaw: Math.atan2(b.z - a.z, -(b.x - a.x)) + Math.PI, pitch: U.flyFlowPitch }, U.flyFlow);
    },

    /* 별 하나의 쉬는 속의 지름(CSS px) — 셰이더와 같은 표(`KNOWLEDGE_UNIVERSE_STAR`). */
    starSize(seat, depth) {
      const S = KNOWLEDGE_UNIVERSE_STAR;
      const lum = this.starGeometry.attributes.aStar.array[seat * 4];
      const near = Math.max(S.nearLeast, Math.min(S.nearMost, this.uniforms.uRef.value / Math.max(depth, 1)));
      const row = this.map.rowOf[seat];
      const core = Math.min(S.coreCeil, Math.max(S.coreMin, Math.min(S.coreMost, S.coreLeast + S.coreGrow * lum ** S.corePower)) * near);
      if (row < 0) return 2 * core;
      /* 붐비는 군집의 별은 작다 — 속이 별 사이 간격의 몫을 넘지 않게, 허브일수록 덜(셰이더와 같은 식). */
      const step = Math.max(0, Math.min(1, (lum - S.hubFrom) / (S.hubTo - S.hubFrom)));
      const hub = step * step * (3 - 2 * step);
      const room = (this.map.room[seat] * this.focal) / Math.max(depth, 1);
      const size = Math.max(S.crowdSize, Math.min(1, (U.crowdRatio * Math.min(this.gaps[row], room)) / core));
      const most = Math.max(size, Math.min(1, (KNOWLEDGE_UNIVERSE_CROWD.hub * room) / core));
      return 2 * core * (size + (most - size) * hub);
    },

    /* 이름 있는 별을 화면에 투영한다(시안 `projectAll`) — 카메라가 움직인 뒤에만. 칸마다 x, y, 반지름, 깊이(뒤는 -1).
     * 가장 가까운 별의 깊이도 함께 적는다(`nearest` — `spanLinks`). */
    project(seat = -1) {
      if (this.projVersion !== this.camVersion) {
        this.placeCamera();
        this.projVersion = this.camVersion;
        const matrix = this.viewProjection.multiplyMatrices(this.camera.projectionMatrix, this.camera.matrixWorldInverse)
          .elements;
        const { pos2, pos3, proj, lift, width, height } = this;
        const count = this.layout.count;
        const shown = this.map.shown;
        this.rowGaps(matrix);
        let nearest = Number.POSITIVE_INFINITY;
        for (let at = 0; at < count; at += 1) {
          if (shown !== null && shown[at] === 0) {
            proj[at * 4 + 3] = -1;
            continue;
          }
          const x = pos2[at * 3] + (pos3[at * 3] - pos2[at * 3]) * lift;
          const y = pos2[at * 3 + 1] + (pos3[at * 3 + 1] - pos2[at * 3 + 1]) * lift;
          const z = pos2[at * 3 + 2] + (pos3[at * 3 + 2] - pos2[at * 3 + 2]) * lift;
          const depth = matrix[3] * x + matrix[7] * y + matrix[11] * z + matrix[15];
          if (depth < nearest) nearest = depth;
          if (depth <= KNOWLEDGE_UNIVERSE_SHAPE.behind) {
            proj[at * 4 + 3] = -1;
            continue;
          }
          proj[at * 4] = ((matrix[0] * x + matrix[4] * y + matrix[8] * z + matrix[12]) / depth * 0.5 + 0.5) * width;
          proj[at * 4 + 1] = (1 - ((matrix[1] * x + matrix[5] * y + matrix[9] * z + matrix[13]) / depth * 0.5 + 0.5)) * height;
          proj[at * 4 + 2] = this.starSize(at, depth) * 0.5;
          proj[at * 4 + 3] = depth;
        }
        this.nearest = nearest;
      }
      if (seat < 0) return null;
      return { x: this.proj[seat * 4], y: this.proj[seat * 4 + 1], r: this.proj[seat * 4 + 2], depth: this.proj[seat * 4 + 3] };
    },

    /* 군집마다 별 사이 간격(CSS px) — 셰이더의 `gapOf`와 같은 식(`KNOWLEDGE_UNIVERSE_CROWD`). 카메라가 움직인 뒤에만. */
    rowGaps(matrix) {
      const map = this.map;
      const gal = map.gal;
      const texels = KNOWLEDGE_UNIVERSE_SHAPE.texels;
      const lift = this.lift;
      for (const row of map.galaxy) {
        const at = row.rank * texels * 4;
        const x = gal[at + 4] + (gal[at] - gal[at + 4]) * lift;
        const y = gal[at + 5] + (gal[at + 1] - gal[at + 5]) * lift;
        const z = gal[at + 6] + (gal[at + 2] - gal[at + 6]) * lift;
        const depth = Math.max(matrix[3] * x + matrix[7] * y + matrix[11] * z + matrix[15],
          row.radius * KNOWLEDGE_UNIVERSE_CROWD.near);
        const reach = (row.radius * this.focal) / depth;
        this.gaps[row.rank] = Math.sqrt((Math.PI * reach * reach) / Math.max(1, row.size));
      }
    },

    /* 포인터 아래의 별(시안 `pickAt`) — 맞힘 반지름 안에서 가깝고 앞선 것. */
    pickAt(x, y) {
      if (this.map === null || this.folding) return -1;
      const S = KNOWLEDGE_UNIVERSE_STAR;
      this.project();
      const proj = this.proj;
      let best = -1;
      let score = Number.POSITIVE_INFINITY;
      for (let at = 0; at < this.layout.count; at += 1) {
        const depth = proj[at * 4 + 3];
        if (depth < 0) continue;
        const dx = proj[at * 4] - x;
        const dy = proj[at * 4 + 1] - y;
        const reach = Math.max(proj[at * 4 + 2] * S.pickShare, S.pickLeast);
        const gap = dx * dx + dy * dy;
        if (gap >= reach * reach) continue;
        const mine = gap / (reach * reach) + depth * S.pickDepth;
        if (mine < score) {
          score = mine;
          best = at;
        }
      }
      return best;
    },

    /* 올림은 한 프레임에 한 번(시안 `schedulePick`). */
    schedulePick() {
      if (this.pickQueued) return;
      this.pickQueued = true;
      requestAnimationFrame(() => {
        this.pickQueued = false;
        if (this.renderer === null || !this.pointer.inside || this.down !== null) return;
        this.setHover(this.pickAt(this.pointer.x, this.pointer.y));
      });
    },

    setHover(seat) {
      if (seat === this.hover) return;
      this.hover = seat;
      this.host?.classList.toggle("is-pointing", seat >= 0);
      this.refreshStates();
      this.showTip(seat);
    },

    /* 올린 별의 팁(시안 `showTip`): 제목, 군집, 연결(선언·추론), 고친 때. 포인터의 오른쪽 위에 서고 판 끝에서는 안으로
     * 든다. 쪽 이름은 그 상자를 비켜 선다(`placeLabels`). */
    showTip(seat) {
      const tip = this.tip;
      if (tip === null) return;
      if (seat < 0 || this.dragging || this.map === null) {
        tip.classList.remove("is-on");
        this.tipBox = null;
        return;
      }
      const layout = this.layout;
      const model = layout.model;
      const row = this.map.rowOf[seat];
      const hue = row >= 0 ? this.map.galaxy[row].hue : -1;
      tip.style.setProperty("--knowledge-universe-ink", hue >= 0 ? `var(--knowledge-hue-${hue})` : "var(--ink-mist)");
      const words = { galaxy: row >= 0 ? knowledgeClusterWord(layout, row) : t("knowledge.rogueStar", "떠돌이 별"),
        links: model.degree[seat], declared: this.declared[seat], inferred: this.inferred[seat] };
      const modified = model.modified[seat];
      writeTextContent(tip.firstChild, model.titles[seat]);
      writeTextContent(tip.lastChild, modified > 0
        ? t("knowledge.starTipAgo", "{{galaxy}} · 연결 {{links}} (선언 {{declared}} · 추론 {{inferred}}) · 고친 때 {{ago}}",
          { ...words, ago: knowledgeUniverseAgo(modified, model.nowMs) })
        : t("knowledge.starTip", "{{galaxy}} · 연결 {{links}} (선언 {{declared}} · 추론 {{inferred}})", words));
      const x = Math.max(0, Math.min(this.width - U.tipRoom, this.pointer.x + U.tipRight));
      const y = Math.max(U.tipTop, this.pointer.y - U.tipUp);
      tip.style.transform = `translate3d(${Math.round(x)}px,${Math.round(y)}px,0)`;
      tip.classList.add("is-on");
      this.tipBox = [x, y, tip.offsetWidth, tip.offsetHeight];
    },

    /* 이름표를 짓는다(시안 `buildTopicCards`) — 큰 군집마다 명판(누르면 그 군집을 고른다), 쪽 이름의 풀, 굵은
     * 필라멘트의 수, 팁. 우주의 그림을 새로 지을 때마다. */
    buildLabels() {
      const layer = this.labels;
      const layout = this.layout;
      const map = this.map;
      layer.textContent = "";
      this.plates = [];
      for (let row = 0; row < map.galaxies; row += 1) {
        const plate = document.createElement("button");
        plate.type = "button";
        plate.className = row < U.plateLarge ? "knowledge-universe-plate" : "knowledge-universe-plate is-minor";
        plate.dataset.knowledgeCluster = String(row);
        plate.tabIndex = -1;
        plate.setAttribute("aria-pressed", "false");
        const hue = map.galaxy[row].hue;
        plate.style.setProperty("--knowledge-universe-ink", hue >= 0 ? `var(--knowledge-hue-${hue})` : "var(--ink-mist)");
        const top = document.createElement("span");
        top.className = "knowledge-universe-plate-top";
        const name = document.createElement("span");
        name.className = "knowledge-universe-plate-name";
        name.textContent = knowledgeClusterWord(layout, row);
        const count = document.createElement("span");
        count.className = "knowledge-universe-plate-count";
        count.textContent = String(layout.communitySize[row]);
        top.append(name, count);
        const flow = document.createElement("span");
        flow.className = "knowledge-universe-plate-flow";
        const mark = document.createElement("i");
        mark.className = "knowledge-universe-plate-mark";
        const said = document.createElement("span");
        const best = knowledgeUniverseStrongest(map, row);
        if (best === null) {
          said.dataset.i18n = "knowledge.flowInside";
          said.textContent = t("knowledge.flowInside", "안에서 이어짐");
        } else {
          const other = document.createElement("em");
          other.textContent = knowledgeClusterWord(layout, best.other);
          said.append(`${best.out ? "→" : "←"} `, other, ` ${best.n}`);
        }
        flow.append(mark, said);
        plate.append(top, flow);
        plate.addEventListener("click", (event) => {
          event.stopPropagation();
          toggleKnowledgeCluster(this.view, row);
        });
        layer.appendChild(plate);
        this.plates.push({ element: plate, row, wide: 0, tall: 0, fullWide: 0, fullTall: 0, full: false, on: false,
          dim: false, pressed: false, x: -1, y: -1 });
      }
      this.names = [];
      for (let slot = 0; slot < U.labels; slot += 1) {
        const name = document.createElement("div");
        name.className = "knowledge-universe-name";
        name.setAttribute("aria-hidden", "true");
        layer.appendChild(name);
        this.names.push({ element: name, seat: -1, on: false, hot: false, x: -1, y: -1 });
      }
      this.flowTags = map.filaments.slice(0, U.flowTags).map((one) => {
        const tag = document.createElement("div");
        tag.className = "knowledge-universe-flow";
        tag.setAttribute("aria-hidden", "true");
        const arrow = document.createElementNS(SVG_NS, "svg");
        arrow.setAttribute("viewBox", "0 0 14 8");
        arrow.setAttribute("class", "knowledge-universe-flow-arrow");
        const path = document.createElementNS(SVG_NS, "path");
        path.setAttribute("d", "M1 4h11M9 1l3 3-3 3");
        arrow.appendChild(path);
        const count = document.createElement("span");
        count.textContent = String(one.n);
        tag.append(arrow, count);
        layer.appendChild(tag);
        const from = map.galaxy[one.from];
        const to = map.galaxy[one.to];
        /* 설 자리 셋 — 가운데, 그 앞, 그 뒤. 화살은 자리마다 흐름 쪽으로 조금 앞선 점을 가리킨다. */
        const lead = U.flowTagAhead - U.flowTagAt;
        const spots = [U.flowTagAt, U.flowTagAt - U.flowTagShift, U.flowTagAt + U.flowTagShift].map((at) => ({
          mid: knowledgeUniverseFilamentPoint(from, to, one.bend, at),
          ahead: knowledgeUniverseFilamentPoint(from, to, one.bend, at + lead) }));
        return { element: tag, arrow, wide: 0, on: false, x: -1, y: -1, turn: Number.NaN, spots };
      });
      const tip = document.createElement("div");
      tip.className = "knowledge-universe-tip";
      tip.setAttribute("aria-hidden", "true");
      const title = document.createElement("span");
      title.className = "knowledge-universe-tip-title";
      const note = document.createElement("span");
      note.className = "knowledge-universe-tip-note";
      tip.append(title, note);
      layer.appendChild(tip);
      this.tip = tip;
      this.tipBox = null;
      const galaxies = map.galaxies;
      this.anchorX = new Float64Array(galaxies);
      this.anchorLow = new Float64Array(galaxies);
      this.anchorHigh = new Float64Array(galaxies);
      this.anchorSeen = new Uint8Array(galaxies);
      this.order = new Int32Array(galaxies);
      this.candidates = new Int32Array(U.labelsCandidates);
      this.candidateScores = new Float64Array(U.labelsCandidates);
      this.boxes = new Float64Array((KNOWLEDGE_UNIVERSE_COVERS.length + galaxies + this.flowTags.length + this.names.length + 1)
        * 4);
      this.nameWidths = new Float32Array(layout.count);
      this.labelVersion = -1;
      this.measured = false;
      this.measureLabels();
    },

    /* 명판의 두 크기(보통·크게)와 필라멘트 수의 넓이, 쪽 이름의 글꼴과 여백 — 판이 보일 때 한 번. 쪽 이름의 넓이는
     * 시안의 어림(글자 수 × 10.6 + 14) 대신 글꼴로 잰다: 한글은 어림보다 넓고 로마자는 좁아, 어림이면 겹치거나 빈다. */
    measureLabels() {
      if (this.host === null || this.host.clientWidth === 0) return false;
      for (const plate of this.plates) {
        const element = plate.element;
        element.classList.remove("is-full");
        let box = element.getBoundingClientRect();
        plate.wide = box.width;
        plate.tall = box.height;
        element.classList.add("is-full");
        box = element.getBoundingClientRect();
        plate.fullWide = box.width;
        plate.fullTall = box.height;
        element.classList.toggle("is-full", plate.full);
      }
      for (const tag of this.flowTags) tag.wide = tag.element.getBoundingClientRect().width;
      const probe = this.names[0]?.element ?? null;
      if (probe !== null) {
        const style = getComputedStyle(probe);
        const pad = (one) => Number.parseFloat(one.paddingLeft) + Number.parseFloat(one.paddingRight)
          + Number.parseFloat(one.borderLeftWidth) + Number.parseFloat(one.borderRightWidth);
        this.nameFont = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        this.namePad = pad(style);
        probe.classList.add("is-hot");
        this.nameHotPad = pad(getComputedStyle(probe));
        probe.classList.remove("is-hot");
      }
      this.measured = true;
      return true;
    },

    /* 쪽 이름의 글 — 평면처럼 `label-max`에서 말줄임으로 자른다(`knowledgeShortWord`). 온 제목은 팁·카드·인스펙터가 든다. */
    nameWord(seat) {
      return knowledgeShortWord(this.layout.model.titles[seat], this.layout.tuning.labelMax);
    },

    /* 쪽 이름 하나의 넓이(CSS px) — 자른 글로, 쪽마다 한 번 잰다. 초점의 이름은 테두리만큼 넓다. */
    nameWide(seat, hot) {
      let wide = this.nameWidths[seat];
      if (wide === 0) {
        this.nameContext ??= document.createElement("canvas").getContext("2d");
        this.nameContext.font = this.nameFont;
        wide = Math.ceil(this.nameContext.measureText(this.nameWord(seat)).width);
        this.nameWidths[seat] = wide;
      }
      return wide + (hot ? this.nameHotPad : this.namePad);
    },

    pushBox(x, y, wide, tall) {
      if ((this.boxCount + 1) * 4 > this.boxes.length) return;
      const at = this.boxCount * 4;
      this.boxes[at] = x;
      this.boxes[at + 1] = y;
      this.boxes[at + 2] = wide;
      this.boxes[at + 3] = tall;
      this.boxCount += 1;
    },

    hit(x, y, wide, tall) {
      const boxes = this.boxes;
      for (let at = 0; at < this.boxCount * 4; at += 4) {
        if (x < boxes[at] + boxes[at + 2] && x + wide > boxes[at] && y < boxes[at + 1] + boxes[at + 3]
          && y + tall > boxes[at + 1]) return true;
      }
      return false;
    },

    /* 무대 위에 늘 서 있는 상자(배율 상자·열린 범례)를 먼저 놓는다 — 프레임의 처음이라 레이아웃은 깨끗하다. */
    readCovers() {
      this.boxCount = 0;
      const host = this.host.getBoundingClientRect();
      for (const selector of KNOWLEDGE_UNIVERSE_COVERS) {
        const element = this.view.querySelector(selector);
        if (element === null || element.hidden) continue;
        const box = element.getBoundingClientRect();
        if (box.width === 0 || box.height === 0) continue;
        this.pushBox(box.left - host.left, box.top - host.top, box.width, box.height);
      }
    },

    /* 세계의 한 점을 화면으로(`project`가 세운 행렬) — 카메라 뒤면 거짓. 답은 `spot`(x, y, 깊이). */
    screenAt(x, y, z) {
      const m = this.viewProjection.elements;
      const depth = m[3] * x + m[7] * y + m[11] * z + m[15];
      if (depth <= KNOWLEDGE_UNIVERSE_SHAPE.behind) return false;
      this.spot[0] = ((m[0] * x + m[4] * y + m[8] * z + m[12]) / depth * 0.5 + 0.5) * this.width;
      this.spot[1] = (1 - ((m[1] * x + m[5] * y + m[9] * z + m[13]) / depth * 0.5 + 0.5)) * this.height;
      this.spot[2] = depth;
      return true;
    },

    /* 군집의 화면 위 원에서 가장 낮은 곳(명판 자리)과 가장 높은 곳(비켜 설 자리) — 나무는 구라서 가운데와 화면
     * 반지름이면 된다. 가운데가 카메라 앞에 있어야 명판이 선다. */
    anchor(row) {
      const k = this.map.galaxy[row];
      const front = this.screenAt(k.x, k.y, k.z);
      const reach = front ? (k.radius * U.plateRing * this.focal) / this.spot[2] : 0;
      this.anchorX[row] = this.spot[0];
      this.anchorLow[row] = this.spot[1] + reach;
      this.anchorHigh[row] = this.spot[1] - reach;
      this.anchorSeen[row] = front ? 1 : 0;
    },

    /* 자리는 `transform` 하나 — 온 픽셀이 바뀐 때만 쓴다. */
    moveLabel(label, x, y) {
      const qx = Math.round(x);
      const qy = Math.round(y);
      if (qx === label.x && qy === label.y) return;
      label.element.style.transform = `translate3d(${qx}px,${qy}px,0)`;
      label.x = qx;
      label.y = qy;
    },

    showLabel(label, on) {
      if (on === label.on) return;
      label.on = on;
      label.element.classList.toggle("is-on", on);
      /* 선 명판은 키보드로도 닿는다(단추) — 숨은 명판은 탭 차례에서 빠진다. */
      if (label.element.tagName === "BUTTON") label.element.tabIndex = on ? 0 : -1;
    },

    hideLabels() {
      if (this.labelVersion === -1) return;
      /* 숨은 명판은 탭 차례에서도 빠진다(`showLabel`). */
      this.tip?.classList.remove("is-on");
      this.tipBox = null;
      for (const plate of this.plates) this.showLabel(plate, false);
      for (const name of this.names) this.showLabel(name, false);
      for (const tag of this.flowTags) this.showLabel(tag, false);
      this.labelVersion = -1;
    },

    /* 이름표를 놓는다(시안 `updateLabels`) — 카메라나 초점이 바뀐 프레임에만. 차례: 무대의 상자 → 군집 명판(고른 것
     * 먼저, 그다음 순위) → 필라멘트 수(멀리서 쉴 때만) → 팁의 상자 → 쪽 이름(쉴 때는 연결이 가장 많은 별들,
     * 가까이 가거나 초점이 있으면 화면에서 크게 보이는 별부터). 먼저 놓인 것을 뒤의 것이 비켜 선다. */
    placeLabels() {
      if (this.labelVersion === this.camVersion || this.map === null) return;
      if (!this.measured && !this.measureLabels()) return;
      this.labelVersion = this.camVersion;
      const layout = this.layout;
      const model = layout.model;
      const map = this.map;
      const wide = this.width;
      const tall = this.height;
      this.readCovers();
      this.project();
      const picked = knowledgeClusterPicked >= 0 && knowledgeClusterPicked < map.rows ? knowledgeClusterPicked : -1;
      const selected = this.selectedSeat;
      const focusSeat = this.hover >= 0 ? this.hover : selected;
      /* 고른 군집(없으면 고른 쪽의 군집) — 크게 서는 명판이고, 「가까이」의 문턱이 그 반지름을 따른다. */
      const keep = picked >= 0 ? picked : selected >= 0 ? map.rowOf[selected] : -1;
      const flown = keep >= 0 ? (map.galaxy[keep].radius * U.flyGalaxyReach + U.flyGalaxyPad) * U.closeInFly : 0;
      const closeIn = this.dist < Math.max(this.home.dist * U.closeIn, flown);
      /* 군집 명판 — 고른 군집이 먼저, 그다음 순위. 고른(또는 고른 쪽의) 군집은 크게 선다. */
      const galaxies = map.galaxies;
      const order = this.order;
      for (let row = 0; row < galaxies; row += 1) {
        this.anchor(row);
        order[row] = row;
      }
      if (picked >= 0 && picked < galaxies) {
        order.copyWithin(1, 0, picked);
        order[0] = picked;
      }
      for (let at = 0; at < galaxies; at += 1) {
        const row = order[at];
        const plate = this.plates[row];
        const full = row === keep;
        if (full !== plate.full) {
          plate.full = full;
          plate.element.classList.toggle("is-full", full);
        }
        const plateWide = full ? plate.fullWide : plate.wide;
        const plateTall = full ? plate.fullTall : plate.tall;
        const x = this.anchorX[row] - plateWide / 2;
        let y = this.anchorLow[row] + U.plateGap;
        let on = this.anchorSeen[row] === 1 && x > U.plateEdge && x < wide - plateWide - U.plateEdge && y > U.plateTop
          && y < tall - plateTall - U.plateBottom && (!closeIn || row === keep);
        if (on && this.hit(x, y, plateWide, plateTall)) {
          const above = this.anchorHigh[row] - plateTall - U.plateGap;
          if (!this.hit(x, above, plateWide, plateTall) && above > U.plateTop) y = above;
          else on = false;
        }
        if (on) {
          this.pushBox(x - U.plateRoom, y - U.plateRoom, plateWide + U.plateRoom * 2, plateTall + U.plateRoom * 2);
          this.moveLabel(plate, x, y);
        }
        this.showLabel(plate, on);
        const dim = (picked >= 0 && row !== picked) || (selected >= 0 && map.rowOf[selected] !== row);
        if (dim !== plate.dim) {
          plate.dim = dim;
          plate.element.classList.toggle("is-dim", dim);
        }
        const pressed = row === picked;
        if (pressed !== plate.pressed) {
          plate.pressed = pressed;
          plate.element.setAttribute("aria-pressed", String(pressed));
        }
      }
      /* 굵은 필라멘트의 링크 수와 흐름 방향(보낸 군집 → 받는 군집) — 멀리서, 아무 초점도 없이 쉴 때만. */
      const flowsShown = !closeIn && this.hover < 0 && selected < 0 && picked < 0 && this.hits === 0;
      for (const tag of this.flowTags) {
        let on = false;
        for (let at = 0; flowsShown && at < tag.spots.length; at += 1) {
          const spot = tag.spots[at];
          if (!this.screenAt(spot.mid[0], spot.mid[1], spot.mid[2])) continue;
          const mx = this.spot[0];
          const my = this.spot[1];
          const turn = this.screenAt(spot.ahead[0], spot.ahead[1], spot.ahead[2])
            ? Math.atan2(this.spot[1] - my, this.spot[0] - mx) : 0;
          const x = mx - tag.wide / 2;
          const y = my - U.flowTagTall / 2;
          if (x <= U.plateEdge || x >= wide - tag.wide - U.plateEdge || y <= U.labelTop || y >= tall - U.labelBottom
            || this.hit(x, y, tag.wide, U.flowTagTall)) continue;
          on = true;
          this.pushBox(x - U.flowTagRoom, y - U.flowTagRoom, tag.wide + U.flowTagRoom * 2, U.flowTagTall + U.flowTagRoom * 2);
          this.moveLabel(tag, x, y);
          const step = Math.round(turn * U.flowTagTurn) / U.flowTagTurn;
          if (step !== tag.turn) {
            tag.arrow.style.transform = `rotate(${step}rad)`;
            tag.turn = step;
          }
          break;
        }
        this.showLabel(tag, on);
      }
      /* 쪽 이름 — 가까이 왔거나 초점이 있으면 화면에서 밝게(크게) 보이는 별부터, 쉬는 먼 자리에서는 가장 밝은 별들. */
      const proj = this.proj;
      const state = this.starState;
      let candidates = this.candidates;
      const scores = this.candidateScores;
      const room = candidates.length;
      let found = 0;
      let most = this.names.length;
      /* 고른 군집 가까이, 쪽 초점도 찾기도 없으면(디자이너 m-12670): 후보는 그 군집의 쪽 전부이고 이름은
       * `labels-close`개까지 — 연결이 많은 차례(`members`)가 곧 화면에서 큰 차례에 가깝다(크기 = 광도 × 깊이). 화면
       * 크기 상위 40만 보면 핵 둘레에 몰린 허브들이 서로를 막아 다섯만 섰다. */
      const galaxyClose = closeIn && picked >= 0 && focusSeat < 0 && this.hits === 0;
      if (galaxyClose) {
        candidates = map.members[picked];
        found = candidates.length;
        most = Math.min(most, U.labelsClose);
      } else if (closeIn || focusSeat >= 0 || this.hits > 0) {
        /* 쪽이 초점이면 그 이웃의 이름은 `labels-focus`개까지 — 링크 수백의 허브를 골라도 이름이 판을 덮지 않는다. */
        if (focusSeat >= 0) most = Math.min(most, U.labelsFocus);
        for (let seat = 0; seat < layout.count; seat += 1) {
          if (proj[seat * 4 + 3] < 0) continue;
          const sx = proj[seat * 4];
          const sy = proj[seat * 4 + 1];
          if (sx < 0 || sx > wide || sy < U.labelScan || sy > tall - U.labelScan) continue;
          let score = proj[seat * 4 + 2];
          if (seat === focusSeat) {
            if (seat === this.hover) continue;
            score = KNOWLEDGE_UNIVERSE_LABEL_FIRST.focus;
          } else if (state[seat] >= 1 && focusSeat >= 0) {
            score += U.labelNeighbour;
          } else if (state[seat] === 3) {
            score += U.labelHit;
          } else if (!closeIn) {
            continue;
          } else if (picked >= 0 && map.rowOf[seat] !== picked) {
            continue;
          }
          if (score < U.labelLeast) continue;
          if (found < room) {
            candidates[found] = seat;
            scores[found] = score;
            found += 1;
          } else {
            let least = 0;
            for (let at = 1; at < room; at += 1) if (scores[at] < scores[least]) least = at;
            if (score > scores[least]) {
              candidates[least] = seat;
              scores[least] = score;
            }
          }
        }
        for (let at = 1; at < found; at += 1) {
          const seat = candidates[at];
          const score = scores[at];
          let back = at - 1;
          while (back >= 0 && scores[back] < score) {
            candidates[back + 1] = candidates[back];
            scores[back + 1] = scores[back];
            back -= 1;
          }
          candidates[back + 1] = seat;
          scores[back + 1] = score;
        }
      } else if (picked < 0) {
        for (const seat of this.bright) {
          if (found >= room) break;
          candidates[found] = seat;
          scores[found] = KNOWLEDGE_UNIVERSE_LABEL_FIRST.bright - found;
          found += 1;
        }
      }
      if (this.hover >= 0 && this.tipBox !== null) {
        const [x, y, tipWide, tipTall] = this.tipBox;
        this.pushBox(x - U.tipGap, y - U.tipGap, tipWide + U.tipGap * 2, tipTall + U.tipGap * 2);
      }
      let used = 0;
      for (let at = 0; at < found && used < most; at += 1) {
        const seat = candidates[at];
        if (proj[seat * 4 + 3] < 0) continue;
        if (galaxyClose && (proj[seat * 4] < 0 || proj[seat * 4] > wide || proj[seat * 4 + 1] < U.labelScan
          || proj[seat * 4 + 1] > tall - U.labelScan || proj[seat * 4 + 2] < U.labelLeast)) continue;
        const hot = seat === focusSeat;
        const nameWide = this.nameWide(seat, hot);
        const off = Math.max(U.labelOffLeast, proj[seat * 4 + 2] * U.labelOff) + U.labelOffPad;
        let x = proj[seat * 4] + off;
        const y = proj[seat * 4 + 1] - U.labelTall / 2;
        if (y < U.labelTop || y > tall - U.labelBottom) continue;
        if (x + nameWide > wide - U.labelEdge) x = proj[seat * 4] - off - nameWide;
        if (x < U.labelEdge) continue;
        if (!hot && this.hit(x, y, nameWide, U.labelTall)) continue;
        this.pushBox(x, y, nameWide, U.labelTall);
        const name = this.names[used];
        used += 1;
        if (name.seat !== seat) {
          name.element.textContent = this.nameWord(seat);
          name.seat = seat;
        }
        this.moveLabel(name, x, y);
        this.showLabel(name, true);
        if (hot !== name.hot) {
          name.hot = hot;
          name.element.classList.toggle("is-hot", hot);
        }
      }
      for (let at = used; at < this.names.length; at += 1) this.showLabel(this.names[at], false);
    },

    /* 초점: 별마다 상태(0 보통 · 1 이웃/고른 군집/시간 창 안 · 2 올림/고름/경로 · 3 찾은 것), 초점 군집, 켜지는
     * 링크(초점 쪽의 것과 경로). 상태는 평면 지도와 같은 자리(고른 쪽·고른 군집·찾기·시간 창·경로)에서 읽는다 —
     * 올림만 우주의 것이다. */
    refreshStates() {
      if (this.map === null || this.starState === null) return;
      const layout = this.layout;
      const model = layout.model;
      const map = this.map;
      const state = this.starState;
      state.fill(0);
      let focus = 0;
      if (knowledgeSlicerCutoff > 0) {
        for (let at = 0; at < layout.count; at += 1) if (layout.sliceMatch[at] === 1) state[at] = 1;
        focus = 1;
      }
      const topic = knowledgeClusterPicked >= 0 && knowledgeClusterPicked < map.rows ? knowledgeClusterPicked : -1;
      if (topic >= 0) {
        for (const at of map.members[topic]) state[at] = 1;
        focus = 1;
      }
      if (layout.pathShown) {
        for (let at = 0; at < layout.count; at += 1) if (layout.pathNode[at] === 1) state[at] = 2;
        focus = 1;
      }
      this.hits = 0;
      if (knowledgeQuery.trim() !== "") {
        for (let at = 0; at < layout.count; at += 1) {
          if (layout.searchMatch[at] !== 1) continue;
          state[at] = 3;
          this.hits += 1;
        }
        focus = 1;
      }
      const selected = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
      this.selectedSeat = selected;
      const focusSeat = this.hover >= 0 ? this.hover : selected;
      if (focusSeat >= 0) {
        if (topic >= 0) for (const at of map.members[topic]) if (state[at] === 1) state[at] = 0;
        for (let slot = model.start[focusSeat]; slot < model.start[focusSeat + 1]; slot += 1) {
          const at = model.neighbour[slot];
          state[at] = Math.max(state[at], 1);
        }
        state[focusSeat] = 2;
        if (selected >= 0 && selected !== focusSeat) state[selected] = 2;
        focus = 1;
      }
      this.uniforms.uFocus.value = focus;
      const galaxy = topic >= 0 ? topic : focusSeat >= 0 ? map.rowOf[focusSeat] : -1;
      this.uniforms.uFocusGal.value = galaxy >= 0 ? galaxy : focusSeat >= 0 ? KNOWLEDGE_UNIVERSE_SHAPE.noGalaxy : -1;
      this.uniforms.uFocusPage.value = focusSeat >= 0 ? 1 : 0;
      /* 켜진 링크 — 초점 쪽의 링크 전부, 그리고 경로. 앞에 켠 것만 끄고 새로 켠다(링크 전부를 다시 쓰지 않는다). */
      const before = this.litCount;
      for (let at = 0; at < before; at += 1) this.lightEdge(this.litList[at], 0);
      this.litCount = 0;
      if (focusSeat >= 0) {
        for (let slot = model.start[focusSeat]; slot < model.start[focusSeat + 1]; slot += 1) {
          this.lightEdge(model.throughEdge[slot], 1);
        }
      }
      if (layout.pathShown) {
        for (let edge = 0; edge < model.edgeCount; edge += 1) if (layout.pathEdge[edge] === 1) this.lightEdge(edge, 2);
      }
      this.uniforms.uLitGain.value = Math.max(U.litLeast, Math.min(1, (U.litFull / Math.max(1, this.litCount)) ** U.litFade));
      /* 굵은 다발에 들지 않은 올은 켜졌거나 제 군집이 초점일 때만 보인다 — 그 밖의 판은 굵은 다발의 올만 싣는다. */
      this.fiberGeometry.instanceCount = topic >= 0 || focusSeat >= 0 || layout.pathShown ? this.edges.fibers : this.edges.thick;
      this.spanLinks();
      if (before > 0 || this.litCount > 0) {
        this.threadLitAttr.needsUpdate = true;
        this.fiberLitAttr.needsUpdate = true;
      }
      this.starStateAttr.needsUpdate = true;
      this.camVersion += 1;
      this.invalidate();
    },

    /* 곧은 링크를 얼마나 실을까 — 현(가지가 아닌 링크)은 켜졌거나 그 끝의 별이 카메라의 `closeFar` 안에 든 판에서만
     * 빛난다. 켜진 링크가 없고 다 떠오르고 다 퍼진 판에서 가장 가까운 별도 그 문턱 밖이면 가지만 싣는다. */
    spanLinks() {
      if (this.threadGeometry === null || this.edges === null) return;
      let all = this.litCount > 0 || this.lift < 1 || this.birth.on;
      if (!all) {
        this.project();
        all = this.nearest < KNOWLEDGE_UNIVERSE_EDGE.closeFar * this.uniforms.uRef.value;
      }
      this.threadGeometry.setDrawRange(0, (all ? this.edges.threads : this.edges.branches) * 2);
    },

    /* 링크 하나를 켜거나 끈다(0 쉼 · 1 켜짐 · 2 경로) — 제 통의 자리에. 켠 것은 적어 둔다. 가려진 링크는 자리가 없다. */
    lightEdge(edge, value) {
      const edges = this.edges;
      const thread = edges.threadOf[edge];
      const fiber = edges.fiberOf[edge];
      if (thread >= 0) {
        this.threadLit[thread * 2] = value;
        this.threadLit[thread * 2 + 1] = value;
      } else if (fiber >= 0) {
        this.fiberLit[fiber] = value;
      } else {
        return;
      }
      if (value > 0 && this.litCount < this.litList.length) {
        this.litList[this.litCount] = edge;
        this.litCount += 1;
      }
    },

    goHome() {
      if (this.folding || this.home === null) return;
      this.lastTouch = performance.now() - U.homeRest;
      this.flyTo(this.home, U.homeMs);
    },

    zoomStep(step) {
      if (this.folding || this.home === null) return;
      this.lastTouch = performance.now();
      this.flyTo({ dist: Math.max(U.zoomMin, Math.min(this.home.dist * U.zoomMax, this.dist * step)) }, U.zoomStepMs);
    },

    turn(yaw, pitch) {
      if (this.folding) return;
      this.lastTouch = performance.now();
      this.tween.on = false;
      this.yaw += yaw;
      this.pitch = this.clampPitch(this.pitch + pitch);
      this.camVersion += 1;
      this.invalidate();
    },

    /* 옷이 바뀌었다 — 평면의 앉기가 끝났으면 자리를 한 번 옮기고, 다음 한 장을 그린다. */
    dress() {
      const layout = this.layout;
      if (layout !== null && layout.left === 0 && this.seatsRevision !== layout.geometryRevision && !this.tween.on) {
        this.placeSeats();
      }
      if (this.map !== null && !this.folding) {
        /* 고름과 군집이 바뀌면 그리로 난다 — 평면의 인스펙터·목록·찾기가 고른 것도 같다(시안 `select`·`focusTopic`). */
        const selected = knowledgeSelectedKey;
        if (selected !== this.seenSelected) {
          this.seenSelected = selected;
          const seat = selected === null ? -1 : layout.model.keys.indexOf(selected);
          if (seat >= 0) this.flyToStar(seat);
        }
        if (knowledgeClusterPicked !== this.seenTopic) {
          this.seenTopic = knowledgeClusterPicked;
          if (knowledgeClusterPicked >= 0 && selected === null) this.flyToGalaxy(knowledgeClusterPicked);
        }
        this.refreshStates();
      }
      this.invalidate();
    },

    /* 이 우주가 사람 앞에 서 있는가 — 가려진 판·숨은 문서·잃은 문맥에서는 그리지 않는다. */
    showing() {
      return this.renderer !== null && !this.lost && this.view.isConnected && !this.view.hidden && !document.hidden
        && this.host?.isConnected === true;
    },

    /* 다음 한 장을 청한다 — 손·비행·옷이 부른다. 쉬는 박자를 기다리던 판은 그 기다림을 거두고 곧바로 그린다. */
    invalidate() {
      if (this.wait !== 0) {
        clearTimeout(this.wait);
        this.wait = 0;
      }
      if (this.raf !== 0 || !this.showing()) return;
      this.raf = requestAnimationFrame((now) => this.frame(now));
    },

    /* 쉬는 동안의 움직임(느린 회전 · 빛의 흐름 · 조준 고리)만 남았다 — 다음 장은 `ambient-gap` 뒤에. */
    linger() {
      if (this.raf !== 0 || this.wait !== 0 || !this.showing()) return;
      this.wait = setTimeout(() => {
        this.wait = 0;
        this.invalidate();
      }, U.ambientGap);
    },

    halt() {
      if (this.raf !== 0) cancelAnimationFrame(this.raf);
      clearTimeout(this.wait);
      this.raf = 0;
      this.wait = 0;
      this.lastNow = 0;
    },

    moving() {
      return this.motion && !knowledgeMotionReduced();
    },

    /* 「움직임」을 켜고 끈다(시안 `setMotion`) — 켜면 이미 쉰 것으로 쳐서(`wake-rest`) 곧 돈다. */
    setMotion(on) {
      this.motion = on;
      if (on) this.lastTouch = performance.now() - U.wakeRest;
      this.invalidate();
    },

    /* 한 장(시안 `frame`). 비행·미끄러짐 중에는 화면의 박자로, 쉬는 동안의 움직임만 남으면 `ambient-gap`의 박자로
     * 다음 장을 청한다 — 쉬면 멈춘다. */
    frame(now) {
      this.raf = 0;
      if (!this.showing()) {
        this.lastNow = 0;
        return;
      }
      const dt = this.lastNow ? Math.min(U.frameMost, (now - this.lastNow) / 1000) : U.frameFirst;
      this.lastNow = now;
      let busy = false;
      let ambient = false;
      if (this.tween.on) {
        const progress = Math.min(1, (performance.now() - this.tween.t0) / this.tween.dur);
        this.applyTween(progress);
        if (progress >= 1) {
          this.tween.on = false;
          const done = this.tween.done;
          this.tween.done = null;
          done?.();
          if (this.renderer === null) return;
        }
        busy = true;
      }
      if (this.birth.on) {
        const progress = Math.min(1, (performance.now() - this.birth.t0) / U.birth);
        this.uniforms.uBirth.value = progress;
        this.birth.on = progress < 1;
        busy = true;
      }
      if (this.glide.on && !this.dragging) {
        this.yaw -= this.glide.vx * dt;
        this.pitch = this.clampPitch(this.pitch + this.glide.vy * dt);
        this.glide.vx *= Math.exp(-dt * U.glideDecay);
        this.glide.vy *= Math.exp(-dt * U.glideDecay);
        if (Math.abs(this.glide.vx) + Math.abs(this.glide.vy) < U.glideStop) this.glide.on = false;
        this.camVersion += 1;
        busy = true;
      }
      if (this.moving() && !this.folding) {
        this.time += dt;
        const resting = !this.tween.on && !this.dragging && knowledgeSelectedKey === null
          && knowledgeClusterPicked < 0 && now - this.lastTouch > U.idleDelay;
        if (resting) {
          this.spin = Math.min(1, this.spin + dt * U.idleRamp);
          this.yaw += dt * U.idleSpin * this.spin;
          this.camVersion += 1;
        } else {
          this.spin = 0;
        }
        ambient = true;
      } else if (this.hits > 0 && !knowledgeMotionReduced() && !this.folding) {
        /* 찾은 별의 조준 고리는 움직임을 끈 판에서도 깜박인다 — 움직임을 줄이라는 판에서만 멈춘다(시안 `frame`). */
        this.time += dt;
        ambient = true;
      }
      this.uniforms.uTime.value = this.time;
      this.uniforms.uFlow.value = this.moving() ? 1 : 0;
      this.uniforms.uLift.value = this.lift;
      this.placeCamera();
      this.spanLinks();
      this.render();
      /* 이름표는 우주가 다 떠오르고 다 퍼진 판에서만(시안 `frame`: 들림 > 0.98) — 떠오르고 내려앉고 퍼지는 동안은 숨는다. */
      if (this.lift > U.labelLift && !this.birth.on) this.placeLabels();
      else this.hideLabels();
      this.paintZoom();
      this.frames += 1;
      if (busy || this.dragging) this.invalidate();
      else if (ambient) this.linger();
      else this.lastNow = 0;
    },

    placeCamera() {
      const cos = Math.cos(this.pitch);
      this.camera.position.set(this.tx + this.dist * cos * Math.sin(this.yaw), this.ty + this.dist * Math.sin(this.pitch),
        this.tz + this.dist * cos * Math.cos(this.yaw));
      this.camera.lookAt(this.tx, this.ty, this.tz);
      this.camera.updateMatrixWorld();
      this.focal = this.height / (2 * Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2));
      this.uniforms.uFocal.value = this.focal;
      this.uniforms.uDpr.value = this.renderer.getPixelRatio();
    },

    /* 한 장(시안 `renderFrame`): 장면을 HDR 목표에 그리고, 빛 번짐 사다리를 내렸다가 올리고, 합성이 톤 매핑과
     * 테마(밝은 테마는 음화 인쇄)를 거쳐 캔버스에 쓴다. */
    render() {
      const renderer = this.renderer;
      const post = this.post;
      if (post.target === null) return;
      renderer.info.reset();
      renderer.setRenderTarget(post.target);
      renderer.setClearColor(0x000000, 1);
      renderer.clear(true, false, false);
      renderer.render(this.scene, this.camera);
      let source = post.target;
      post.quad.material = post.down;
      for (let level = 0; level < post.mips.length; level += 1) {
        const into = post.mips[level];
        post.down.uniforms.tSrc.value = source.texture;
        post.down.uniforms.uTexel.value.set(1 / source.width, 1 / source.height);
        post.down.uniforms.uKaris.value = level === 0 ? 1 : 0;
        renderer.setRenderTarget(into);
        renderer.render(post.scene, post.lens);
        source = into;
      }
      post.quad.material = post.up;
      for (let level = post.mips.length - 1; level > 0; level -= 1) {
        const from = post.mips[level];
        post.up.uniforms.tSrc.value = from.texture;
        post.up.uniforms.uTexel.value.set(1 / from.width, 1 / from.height);
        renderer.setRenderTarget(post.mips[level - 1]);
        renderer.render(post.scene, post.lens);
      }
      const comp = post.comp.uniforms;
      comp.tScene.value = post.target.texture;
      comp.tBloom.value = post.mips[0]?.texture ?? post.target.texture;
      comp.uBloom.value = post.mips.length > 0 ? this.inks.bloom : 0;
      comp.uTime.value = this.uniforms.uTime.value;
      post.quad.material = post.comp;
      renderer.setRenderTarget(null);
      renderer.render(post.scene, post.lens);
    },

    /* 배율의 수 — 처음 자리에 대한 거리의 몫. */
    paintZoom() {
      const word = this.view.querySelector(".knowledge-zoom-label");
      if (word !== null && this.home !== null) writeTextContent(word, `${Math.round((this.home.dist / this.dist) * 100)}%`);
    },

    /* 한 줄 알림 — 우주 위의 판에 선다(평면으로 돌아온 뒤에도 읽히게). */
    say(words) {
      if (this.notice === null) {
        const notice = document.createElement("p");
        notice.className = "knowledge-universe-notice";
        notice.setAttribute("role", "status");
        notice.setAttribute("aria-live", "polite");
        this.view.querySelector(".knowledge-stage")?.appendChild(notice);
        this.notice = notice;
      }
      writeTextContent(this.notice, words);
      this.notice.classList.add("is-on");
      clearTimeout(this.noticeTimer);
      this.noticeTimer = setTimeout(() => this.notice?.classList.remove("is-on"), U.noticeMs);
    },

    /* 문맥을 잃었다: 평면으로 돌아와 한 줄로 말한다. 우주는 그대로 쥐고 있다가 문맥이 돌아오면 다시
     * 선다(시안의 문맥 잃음). 덮여 있는 동안 쉬던 평면의 손은 여기서 한 장을 다시 그린다 — 그리기 버퍼를 돌려준
     * 채로는 빈 판이 드러난다. */
    loseContext() {
      this.lost = true;
      this.halt();
      this.tween.on = false;
      this.view.classList.remove("is-universe");
      this.say(t("knowledge.universeLost", "그래픽 장치가 우주 그림을 놓아 평면 지도로 보여 줍니다 — 돌아오면 다시 섭니다"));
      paintKnowledgeDimension(this.view);
      const layout = knowledgeLayouts.get(this.view);
      if (layout !== undefined) paintKnowledgeFrame(this.view, layout);
    },

    regainContext() {
      this.lost = false;
      for (const geometry of this.built.geometries) {
        for (const attribute of Object.values(geometry.attributes)) attribute.needsUpdate = true;
      }
      this.say(t("knowledge.universeBack", "그래픽 장치가 돌아와 우주를 다시 엽니다"));
      if (knowledgeUniverseWanted(this.view, knowledgeLayouts.get(this.view))) this.enter(true);
      paintKnowledgeDimension(this.view);
    },

    /* 지은 것을 놓는다 — 다시 짓기 전에, 그리고 떠날 때. */
    clearScene() {
      for (const object of this.built.objects) this.scene?.remove(object);
      for (const geometry of this.built.geometries) geometry.dispose();
      this.galTexture?.dispose();
      for (const texture of this.nodeTextures) texture.dispose();
      this.nodeTextures = [];
      this.built.objects.length = 0;
      this.built.geometries.length = 0;
      this.starGeometry = null;
      this.starStateAttr = null;
      this.galTexture = null;
      this.map = null;
      this.node3 = null;
      this.node2 = null;
      this.edges = null;
      this.threadLit = null;
      this.threadLitAttr = null;
      this.fiberLit = null;
      this.fiberLitAttr = null;
      this.fiberGeometry = null;
      this.threadGeometry = null;
      this.litList = null;
      this.litCount = 0;
      if (this.labels !== null) this.labels.textContent = "";
      this.plates = [];
      this.names = [];
      this.flowTags = [];
      this.tip = null;
      this.tipBox = null;
      this.labelVersion = -1;
      this.selectedSeat = -1;
      if (this.uniforms !== null) {
        this.uniforms.tGal.value = null;
        this.uniforms.tNode3.value = null;
        this.uniforms.tNode2.value = null;
      }
    },

    /* 떠난다 — 기하·재질(프로그램)·렌더러·문맥·손·캔버스 전부. 순서가 약속이다: 기하와 재질의
     * `dispose()`가 렌더러의 것보다 먼저다(렌더러가 제 장부를 갈아 끼운 뒤에는 GL 객체를 찾지 못한다). */
    dispose() {
      this.halt();
      clearTimeout(this.noticeTimer);
      this.clearScene();
      for (const material of Object.values(this.materials ?? {})) material.dispose();
      this.dropTargets();
      if (this.post !== null) {
        for (const material of [this.post.down, this.post.up, this.post.comp]) material.dispose();
        this.post.triangle.dispose();
      }
      this.renderer?.renderLists.dispose();
      this.themeWatch?.disconnect();
      this.themeWatch = null;
      for (const stop of this.unlisten) stop();
      this.unlisten.length = 0;
      const context = this.renderer?.getContext() ?? null;
      this.renderer?.dispose();
      context?.getExtension("WEBGL_lose_context")?.loseContext();
      this.host?.remove();
      this.notice?.remove();
      this.renderer = null;
      this.scene = null;
      this.camera = null;
      this.materials = null;
      this.uniforms = null;
      this.layout = null;
      this.pos2 = null;
      this.pos3 = null;
      this.starState = null;
      this.notice = null;
      this.post = null;
      this.probe = null;
      this.inks = null;
      this.folding = false;
    },
  };
}
