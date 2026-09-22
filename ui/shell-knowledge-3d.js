/* ---- 지식 그래프의 두 번째 손 — 손으로 쓴 WebGL2 페인터 (6차 P2) ------------
 *
 * 왜 두 번째 손인가. 5차의 SVG는 점 하나에 요소 셋이고, 그 값이 만 쪽에서
 * 91,754개가 된다(P0의 표). 브라우저가 그것을 굽는 시간은 우리 호출 밖에 있어서
 * 프레임 간격이 841 ms가 되는데, 우리 쪽 처리 시간은 그중 772 ms이고 나머지는
 * 래스터화다 — 어느 쪽도 사람이 지도를 미는 동안 견딜 수 있는 수가 아니다.
 * 이 파일은 같은 그림을 **인스턴스 드로우 넷**으로 그린다.
 *
 * 왜 손으로 쓰는가. three.js 코어는 min ~680 KB이고, 필요한 부분만 잘라도
 * ~250 KB이다. 여기서 쓰는 것은 직교 투영 하나·버퍼 몇 개·셰이더 넷뿐이고 그것은
 * 이 파일의 길이다. 외부 의존 0 바이트, CSP `default-src 'self'` 그대로.
 *
 * 무엇을 그리는가(설계 §5):
 *   1. 성운  — 군집마다 빌보드 원판 하나(방사 알파 + 테두리 링)
 *   2. 선    — 인스턴스 사각형, 두께는 화면 픽셀, 점선은 길이로
 *   3. 점    — 빌보드 사각형에 모양의 SDF(원·사각·마름모·삼각), 테두리·고리의 점선까지
 *   4. 고리  — 회상된 점의 활동 고리(없으면 드로우도 없다)
 *
 * 위치는 **텍스처 하나**다(RGBA32F, NEAREST, `texelFetch`). 점과 선의 버텍스
 * 셰이더가 인덱스로 위치를 읽으므로, 선은 제 좌표를 들지 않고 번호 둘만 든다 —
 * 만 쪽·2만 5천 선에서 프레임마다 올리는 것은 위치 160 KB뿐이다. 필터를 쓰지
 * 않는 것은 WKWebView의 부동소수 텍스처가 필터를 보장하지 않기 때문이고
 * (설계 §9), `texelFetch`는 필터를 묻지 않는다.
 *
 * 색은 **CSS에게 묻는다**. 숨은 견본 하나에 SVG와 똑같은 옷(`knowledge-node
 * is-ghost`, `knowledge-edge is-typed kind-depends_on`…)을 입히고 브라우저가
 * 계산한 `fill`·`stroke`·`stroke-width`를 읽는다. 색의 식을 JS로 옮겨 적으면
 * 그날부터 두 그림이 갈라지고, 토큰을 고친 사람은 한쪽만 고친 것을 모른다.
 *
 * 무엇을 하지 않는가: 프러스텀 컬링(만 점의 드로우가 컬링 계산보다 싸다),
 * GPU 픽킹(`readPixels`의 동기 정지가 프레임을 먹는다 — 픽킹은 CPU가 투영한
 * 자리에서, `knowledgePickSeat` 한 벌), 3D(P3의 일이다. 이 페인터는 z=0으로
 * SVG와 같은 그림을 그린다). */

/* 이 손만의 수. 그림의 나머지 수는 전부 `knowledgeTuning`의 토큰이고 색은 CSS다. */
const KNOWLEDGE_GL_TOKENS = Object.freeze({
  /* SDF 가장자리가 부드러워지는 폭(px). 0이면 계단이 보이고, 크면 점이 흐려진다. */
  feather: "--knowledge-3d-feather",
  /* 활동 고리가 점의 몸에서 떨어지는 거리(px) — SVG의 `--knowledge-halo-scale`이
   * 배수로 말하던 것을 픽셀로. */
  ringGap: "--knowledge-3d-ring-gap",
  /* 기기 픽셀의 상한 — 레티나에서 몇 배까지 그리는가. */
  pixelCap: "--knowledge-3d-pixel-cap",
});

/* 인스턴스 하나가 덮는 사각형. 네 꼭짓점을 두 삼각형으로 — 인덱스 버퍼 없이
 * `TRIANGLE_STRIP`이다. */
const KNOWLEDGE_GL_QUAD = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]);

/* 화면으로 옮기는 두 줄 — 세 셰이더가 같은 글자를 쓴다. 캔버스는 **기기 픽셀**이고(`uCamera.z`에
 * dpr이 든다) 점의 반지름·테두리·선 굵기·점선·고리 틈은 **CSS 픽셀**의 약속이라, 셰이더마다 그 길이에
 * `uPixelRatio`를 곱해 같은 자로 옮긴다. 곱하지 않던 판은 레티나(2배)에서 점과 선을 반쪽으로 그렸다
 * (실측 09-17, 같은 판 36 px 점: 제 모양과의 겹침 SVG 0.975 → GL 0.239). */
const KNOWLEDGE_GL_PROJECT = `
vec2 knowledgeSeatToPx(vec2 seat) { return (seat - uCamera.xy) * uCamera.z; }
vec4 knowledgePxToClip(vec2 px) {
  vec2 ndc = (px / uViewport) * 2.0 - 1.0;
  return vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
`;

/* 위치 텍스처에서 자리 하나 — 번호를 격자 좌표로 풀어 한 텍셀을 읽는다. */
const KNOWLEDGE_GL_FETCH = `
vec4 knowledgeSeatAt(uint seat) {
  int at = int(seat);
  return texelFetch(uSeats, ivec2(at % uSeatWide, at / uSeatWide), 0);
}
`;

const KNOWLEDGE_GL_NODE_VERT = `#version 300 es
precision highp float;
uniform sampler2D uSeats;
uniform vec3 uCamera;
uniform vec2 uViewport;
uniform float uPixelRatio;
uniform int uSeatWide;
uniform float uFeather;
in vec2 aCorner;
in uint aSeat;
in vec4 aFill;
in vec4 aStroke;
in vec4 aShape;
out vec2 vLocal;
out vec4 vFill;
out vec4 vStroke;
out vec2 vBody;
out float vDash;
flat out int vForm;
${KNOWLEDGE_GL_PROJECT}
${KNOWLEDGE_GL_FETCH}
void main() {
  vec4 seat = knowledgeSeatAt(aSeat);
  float radius = seat.w * uPixelRatio;
  float strokePx = aShape.x * 255.0 / 8.0 * uPixelRatio;
  float ringPx = aShape.y * 255.0 / 4.0 * uPixelRatio;
  /* 모양의 번호는 점마다 하나라 조각마다 풀지 않고 여기서 한 번 정수로 푼다. 바이트를
   * 되읽을 때 반올림하는 것은 \`int()\`가 버림이기 때문이다 — 드라이버가 3을 2.9999로
   * 되돌려 주면 버림은 다른 모양을 그린다. */
  vForm = int(aShape.w * 255.0 + 0.5);
  /* 사각형은 몸 + 테두리 + 고리 + 부드러운 가장자리만큼 넓다. 더 넓으면 채우는
   * 픽셀이 늘고, 좁으면 테두리가 잘린다. */
  float reach = radius + strokePx + ringPx + uFeather + 1.0;
  vLocal = aCorner * reach;
  vBody = vec2(radius, strokePx);
  vDash = aShape.z * 255.0 / 4.0 * uPixelRatio;
  vFill = aFill;
  vStroke = aStroke;
  gl_Position = knowledgePxToClip(knowledgeSeatToPx(seat.xy) + aCorner * reach);
}
`;

/* 모양의 거리 — 모양 이름마다 GLSL 한 토막이 외접원 반지름 `radius`에 내접한 모양의
 * 부호 있는 거리(안이 음수)를 `away`에 쓴다. 꼭짓점은 SVG 손의 윤곽
 * (`KNOWLEDGE_SHAPES[].outline`)과 같고, 거리는 바깥에서도 참 거리라 테두리의 바깥
 * 모서리가 SVG의 둥근 이음(`stroke-linejoin: round`)과 같은 곡선으로 닫힌다. 같은 번호를
 * 나누는 모양(원·고리)은 한 토막을 쓴다 — 고리의 점선은 거리가 아니라 옷이다. */
const KNOWLEDGE_GL_DISTANCE = Object.freeze({
  circle: "away = length(vLocal) - radius;",
  /* 반 변이 R/√2인 상자. */
  square: "away = knowledgeBox(vLocal, radius * KNOWLEDGE_HALF_ROOT2);",
  /* 마름모는 45° 돌린 같은 상자다 — 꼭짓점 (R, 0)이 상자의 모서리 (R/√2, −R/√2)로 간다. */
  diamond: "away = knowledgeBox(vec2(vLocal.x + vLocal.y, vLocal.y - vLocal.x) * KNOWLEDGE_HALF_ROOT2,"
    + " radius * KNOWLEDGE_HALF_ROOT2);",
  /* 위를 가리키는 정삼각형(꼭짓점 (0, −R), 밑변 y = R/2). 화면의 y는 아래로 자라므로
   * 뒤집어 재고, 반 변 R√3/2와 무게중심에서 밑변까지 R/2를 옮긴 뒤 한 변으로 접는다. */
  triangle: "vec2 q = vec2(abs(vLocal.x) - radius * KNOWLEDGE_ROOT3 * 0.5, radius * 0.5 - vLocal.y);"
    + " if (q.x + KNOWLEDGE_ROOT3 * q.y > 0.0) {"
    + " q = vec2(q.x - KNOWLEDGE_ROOT3 * q.y, -KNOWLEDGE_ROOT3 * q.x - q.y) * 0.5; }"
    + " q.x -= clamp(q.x, -radius * KNOWLEDGE_ROOT3, 0.0);"
    + " away = -length(q) * sign(q.y);",
  /* 위아래가 평평한 정육각형(꼭짓점 (±R, 0)): 안쪽 반지름 R√3/2의 윗변으로 접어 잰다. */
  hexagon: "vec2 h = abs(vLocal); float inner = radius * KNOWLEDGE_ROOT3 * 0.5;"
    + " vec2 fold = vec2(-KNOWLEDGE_ROOT3 * 0.5, 0.5);"
    + " h -= 2.0 * min(dot(fold, h), 0.0) * fold;"
    + " h -= vec2(clamp(h.x, -inner / KNOWLEDGE_ROOT3, inner / KNOWLEDGE_ROOT3), inner);"
    + " away = length(h) * sign(h.y);",
  /* 십자(반길이 3R/√10, 반폭 R/√10): 한 팔로 접어 잰다. */
  cross: "vec2 c = abs(vLocal); c = (c.y > c.x) ? c.yx : c.xy;"
    + " vec2 arm = vec2(3.0, 1.0) * radius * KNOWLEDGE_INV_ROOT10;"
    + " vec2 q = c - arm; float k = max(q.y, q.x);"
    + " vec2 w = (k > 0.0) ? q : vec2(arm.y - c.x, -k);"
    + " away = sign(k) * length(max(w, 0.0));",
});

/* 점의 조각 셰이더. 모양의 갈래를 **표에서** 짓는다 — 셰이더에 모양의 번호를 적어 두면
 * 표의 번호를 고친 날 두 손이 다른 모양을 그린다. 셰이더의 수는 `#define`으로 든다: 줄머리의
 * `const`는 창의 소스 계약이 스크립트의 최상위 선언으로 읽는다(한 이름 한 선언). 창의 스크립트가 다 선 뒤(`mount`)에
 * 부르므로 뒤에 읽히는 `KNOWLEDGE_SHAPES`를 읽을 수 있다. 갈래의 순서는 표의 순서라 가장
 * 흔한 원이 첫 비교에서 끝난다. 거리 토막이 없는 번호가 남으면 셰이더를 짓지 않는다 —
 * 그 판은 `mount`의 실패 길로 SVG가 그린다. */
function knowledgeGlNodeFragment() {
  const branches = [];
  const drawn = new Set();
  for (const [shape, row] of Object.entries(KNOWLEDGE_SHAPES)) {
    if (drawn.has(row.code) || KNOWLEDGE_GL_DISTANCE[shape] === undefined) continue;
    drawn.add(row.code);
    branches.push({ code: row.code, distance: KNOWLEDGE_GL_DISTANCE[shape] });
  }
  const missing = Object.values(KNOWLEDGE_SHAPES).filter((row) => !drawn.has(row.code));
  if (missing.length > 0) {
    throw new Error(`knowledge gl node: no distance for shape code ${missing[0].code}`);
  }
  const chain = branches.map((one, at) => (at === branches.length - 1
    ? `{ ${one.distance} }`
    : `if (vForm == ${one.code}) { ${one.distance} }`)).join(" else ");
  return `#version 300 es
precision highp float;
uniform float uFeather;
in vec2 vLocal;
in vec4 vFill;
in vec4 vStroke;
in vec2 vBody;
in float vDash;
flat in int vForm;
out vec4 outColor;
#define KNOWLEDGE_HALF_ROOT2 ${Math.SQRT1_2}
#define KNOWLEDGE_ROOT3 ${Math.sqrt(3)}
#define KNOWLEDGE_INV_ROOT10 ${1 / Math.sqrt(10)}
float knowledgeBox(vec2 p, float halfSide) {
  vec2 corner = abs(p) - vec2(halfSide);
  return length(max(corner, 0.0)) + min(max(corner.x, corner.y), 0.0);
}
void main() {
  float radius = vBody.x;
  float strokePx = vBody.y;
  float away;
  ${chain}
  float body = 1.0 - smoothstep(-uFeather, uFeather, away);
  float ink = 1.0 - smoothstep(strokePx * 0.5 - uFeather, strokePx * 0.5 + uFeather,
    abs(away));
  if (vDash > 0.0) {
    /* 고리의 점선은 각도로 긋는다 — 둘레 위의 호 길이가 점선의 자다. 점선이 없는 모양은
     * 점선의 길이가 0으로 온다(\`fillNodes\`). */
    float arc = atan(vLocal.y, vLocal.x) * max(radius, 1.0);
    ink *= step(mod(arc + vDash * 4096.0, vDash * 2.0), vDash);
  }
  /* 테두리를 몸 **위에** 얹는다(over). 두 색을 알파 없이 섞으면 채우지 않은 몸(고리)의 투명한
   * 검정이 테두리의 가장자리를 어둡게 한다 — SVG의 고리보다 가늘어 보였다(실측 09-17: 둘레
   * 띠의 잉크 몫 SVG 0.302, 섞던 GL 0.166, over 0.253). 불투명한 몸에서는 섞기와 같은 값이다. */
  float fillCover = vFill.a * body;
  float strokeCover = ink * vStroke.a;
  float cover = strokeCover + fillCover * (1.0 - strokeCover);
  if (cover <= 0.0) discard;
  outColor = vec4((vStroke.rgb * strokeCover + vFill.rgb * fillCover * (1.0 - strokeCover)) / cover, cover);
}
`;
}

const KNOWLEDGE_GL_EDGE_VERT = `#version 300 es
precision highp float;
uniform sampler2D uSeats;
uniform vec3 uCamera;
uniform vec2 uViewport;
uniform float uPixelRatio;
uniform int uSeatWide;
uniform float uFeather;
in vec2 aCorner;
in uvec2 aEnds;
in vec4 aInk;
in vec4 aShape;
out vec2 vAlong;
out vec4 vInk;
out vec2 vLine;
out float vDash;
${KNOWLEDGE_GL_PROJECT}
${KNOWLEDGE_GL_FETCH}
void main() {
  vec4 from = knowledgeSeatAt(aEnds.x);
  vec4 to = knowledgeSeatAt(aEnds.y);
  vec2 head = knowledgeSeatToPx(from.xy);
  vec2 tail = knowledgeSeatToPx(to.xy);
  float widthPx = aShape.x * 255.0 / 8.0 * uPixelRatio;
  vDash = aShape.y * 255.0 / 4.0 * uPixelRatio;
  vec2 span = tail - head;
  float reach = length(span);
  vec2 way = reach > 0.0001 ? span / reach : vec2(1.0, 0.0);
  /* 선은 점의 몸에서 시작해 점의 몸에서 끝난다 — SVG 페인터가 좌표로 하던 일을
   * 여기서는 셰이더가 한다(그래서 CPU는 선의 좌표를 한 번도 짓지 않는다). */
  /* 두 몸을 합친 것보다 짧은 선은 **자르지 않는다** — 자르면 그 선이 사라지고,
   * 붙어 있는 두 점 사이의 연결이 그림에서 없어진다. SVG 페인터의 같은 갈림길이
   * 그때 가운데에서 가운데로 긋는다(실측: 군집 안의 짧은 선들이 GL에서만 빠졌다). */
  float trim = (from.w + to.w) * uPixelRatio;
  if (reach > trim) {
    head += way * from.w * uPixelRatio;
    tail -= way * to.w * uPixelRatio;
    reach -= trim;
  }
  /* half 는 GLSL의 예약어다(컴파일러가 반정밀도 실수로 읽는다) — 반 폭의 이름은
   * halfPx 이고, 그 이름이 이 셰이더 안에서 겹치지 않는다. */
  float halfPx = widthPx * 0.5 + uFeather;
  vec2 side = vec2(-way.y, way.x);
  float along = aCorner.x * 0.5 + 0.5;
  /* 끝을 반 폭만큼 늘여 둥근 마개를 담는다(SVG의 stroke-linecap: round). */
  vec2 px = head + way * (along * reach + (aCorner.x * halfPx))
    + side * (aCorner.y * halfPx);
  vAlong = vec2(along * reach + aCorner.x * halfPx, aCorner.y * halfPx);
  vLine = vec2(reach, widthPx);
  vInk = aInk;
  gl_Position = knowledgePxToClip(px);
}
`;

const KNOWLEDGE_GL_EDGE_FRAG = `#version 300 es
precision highp float;
uniform float uFeather;
in vec2 vAlong;
in vec4 vInk;
in vec2 vLine;
in float vDash;
out vec4 outColor;
void main() {
  /* 캡슐의 거리 — 둥근 마개가 여기서 나온다. */
  float along = clamp(vAlong.x, 0.0, vLine.x);
  float away = length(vec2(vAlong.x - along, vAlong.y)) - vLine.y * 0.5;
  float ink = 1.0 - smoothstep(-uFeather, uFeather, away);
  if (vDash > 0.0) ink *= step(mod(vAlong.x + vDash * 4096.0, vDash * 2.0), vDash);
  float alpha = vInk.a * ink;
  if (alpha <= 0.0) discard;
  outColor = vec4(vInk.rgb, alpha);
}
`;

const KNOWLEDGE_GL_DISC_VERT = `#version 300 es
precision highp float;
uniform vec3 uCamera;
uniform vec2 uViewport;
uniform float uPixelRatio;
in vec2 aCorner;
in vec4 aDisc;
in vec4 aInk;
in vec2 aRim;
out vec2 vLocal;
out vec4 vInk;
out vec3 vRim;
${KNOWLEDGE_GL_PROJECT}
void main() {
  vec2 radius = aDisc.zw * uCamera.z;
  vec2 reach = radius + vec2(aRim.y * uPixelRatio + 1.0);
  vLocal = aCorner * reach / max(radius, vec2(0.0001));
  vInk = aInk;
  vRim = vec3(aRim.x, aRim.y * uPixelRatio, min(radius.x, radius.y));
  gl_Position = knowledgePxToClip(knowledgeSeatToPx(aDisc.xy) + aCorner * reach);
}
`;

const KNOWLEDGE_GL_DISC_FRAG = `#version 300 es
precision highp float;
uniform float uFeather;
in vec2 vLocal;
in vec4 vInk;
in vec3 vRim;
out vec4 outColor;
void main() {
  float reach = length(vLocal);
  /* SVG의 방사 그라데이션 그대로: 가운데가 토큰의 불투명도, 가장자리가 0. */
  float core = vInk.a * max(0.0, 1.0 - reach);
  float rimWidth = vRim.y / max(vRim.z, 1.0);
  float rim = vRim.x * (1.0 - smoothstep(0.0, rimWidth, abs(reach - 1.0)));
  float alpha = max(core, rim);
  if (alpha <= 0.0) discard;
  outColor = vec4(vInk.rgb, alpha);
}
`;

const KNOWLEDGE_GL_RING_VERT = `#version 300 es
precision highp float;
uniform sampler2D uSeats;
uniform vec3 uCamera;
uniform vec2 uViewport;
uniform float uPixelRatio;
uniform int uSeatWide;
uniform float uFeather;
in vec2 aCorner;
in uint aSeat;
in vec4 aInk;
in vec4 aShape;
out vec2 vLocal;
out vec4 vInk;
out vec3 vRing;
${KNOWLEDGE_GL_PROJECT}
${KNOWLEDGE_GL_FETCH}
void main() {
  vec4 seat = knowledgeSeatAt(aSeat);
  float lift = aShape.x * 255.0 / 4.0 * uPixelRatio;
  float width = aShape.y * 255.0 / 8.0 * uPixelRatio;
  float dash = aShape.z * 255.0 / 4.0 * uPixelRatio;
  float radius = seat.w * uPixelRatio + lift;
  float reach = radius + width + uFeather + 1.0;
  vLocal = aCorner * reach;
  vRing = vec3(radius, width, dash);
  vInk = aInk;
  gl_Position = knowledgePxToClip(knowledgeSeatToPx(seat.xy) + aCorner * reach);
}
`;

const KNOWLEDGE_GL_RING_FRAG = `#version 300 es
precision highp float;
uniform float uFeather;
in vec2 vLocal;
in vec4 vInk;
in vec3 vRing;
out vec4 outColor;
void main() {
  float away = abs(length(vLocal) - vRing.x) - vRing.y * 0.5;
  float ink = 1.0 - smoothstep(-uFeather, uFeather, away);
  if (vRing.z > 0.0) {
    float arc = atan(vLocal.y, vLocal.x) * max(vRing.x, 1.0);
    ink *= step(mod(arc + vRing.z * 4096.0, vRing.z * 2.0), vRing.z);
  }
  float alpha = vInk.a * ink;
  if (alpha <= 0.0) discard;
  outColor = vec4(vInk.rgb, alpha);
}
`;

/* 이 판이 WebGL2를 그릴 수 있는가. 메뉴가 줄을 세우기 **전에** 묻는다 — 죽은
 * 컨트롤을 세우지 않는 것이 이 창의 규칙이고, 이유 없는 비활성은 죽은 컨트롤과
 * 같다. 답은 한 번 캐시한다(문맥 하나를 만들어 보는 값이 있으므로). */
let knowledgeGlAble = null;
function knowledgeGlSupported() {
  if (knowledgeGlAble !== null) return knowledgeGlAble;
  try {
    const probe = document.createElement("canvas");
    const gl = probe.getContext("webgl2");
    knowledgeGlAble = gl !== null && typeof gl.drawArraysInstanced === "function";
    gl?.getExtension("WEBGL_lose_context")?.loseContext();
  } catch {
    knowledgeGlAble = false;
  }
  return knowledgeGlAble;
}

/* 색 하나를 네 실수로. `rgb(..)`·`rgba(..)`·`color(srgb ..)`·`none` 전부 이 문을
 * 지난다 — 브라우저가 계산해 준 값이라 형태는 몇 가지뿐이다. */
function knowledgeGlColor(word, into, at) {
  const numbers = String(word ?? "").match(/[\d.]+(?:e-?\d+)?/gu);
  if (!numbers || numbers.length < 3 || String(word).startsWith("none")) {
    into[at] = 0;
    into[at + 1] = 0;
    into[at + 2] = 0;
    into[at + 3] = 0;
    return;
  }
  const big = String(word).includes("color(") ? 1 : 255;
  into[at] = Number(numbers[0]) / big;
  into[at + 1] = Number(numbers[1]) / big;
  into[at + 2] = Number(numbers[2]) / big;
  into[at + 3] = numbers.length > 3 ? Number(numbers[3]) : 1;
}

/* 점 하나의 쉬는 옷(t-5966 G4에서 따로 나옴): 종류·군집의 색·단으로 고른 몸과 테두리의
 * 색 칸과 굵기, 그리고 모양이 점선인가. GL 페인터의 `fillNodes`가 상태의 테두리를 덧입히기
 * 전의 옷이고, HTML 내보내기가 같은 손으로 읽어 두 그림이 같은 잉크를 입는다. */
function knowledgeRestingNodeInk(palette, tuning, layout, model, at) {
  const kind = model.kinds[at];
  const ghost = kind === "ghost";
  const source = kind === "source";
  const tiers = KNOWLEDGE_TIER_WORD.length;
  const rank = layout.community[at];
  const hue = kind === "page" ? layout.communityHue[rank] : -1;
  const tier = layout.tier[at];
  const row = (hue >= 0 ? hue * tiers : palette.hues * tiers) + tier;
  /* 공급망의 점(P4)은 제 줄의 옷을 입는다. */
  const own = kind === "component" ? palette.component
    : kind === "vulnerability" ? palette.vulnerability : null;
  const ownAt = own === null ? -1 : knowledgeGlSupplyRow(model, at);
  /* 코드 층의 점(t-5970)은 종류마다 한 견본 — 몸 칸 0, 테두리 칸 4. */
  const code = kind === "code_file" ? palette.codeFile
    : kind === "code_symbol" ? palette.codeSymbol : null;
  const plain = ghost ? palette.ghost : source ? palette.source : code;
  return {
    fill: plain ?? (own !== null ? own.fill : palette.nodeFill),
    fillAt: plain !== null ? 0 : own !== null ? ownAt * 4 : row * 4,
    stroke: plain ?? (own !== null ? own.stroke : palette.nodeStroke),
    strokeAt: plain !== null ? 4 : own !== null ? ownAt * 4 : row * 4,
    strokePx: ghost ? tuning.nodeGhostStroke
      : source ? 1
      : code !== null ? tuning.nodeStroke
      : own !== null ? own.width[ownAt]
      : tier === KNOWLEDGE_TIER_CORE ? tuning.nodeSelectedStroke
      : tuning.nodeStroke,
    dashed: KNOWLEDGE_SHAPES[knowledgeShapeOf(kind)].dashed,
  };
}

/* 선 하나의 쉬는 옷: 유령의 선, 이름 있는 관계·merge 물음의 제 잉크와 점선, 군집 안의 본문
 * 링크, 군집을 건너는 본문 링크 — 이 순서로 하나가 이긴다. 밝힘·짚기의 옷은 `fillEdges`가
 * 덧입힌다. */
function knowledgeRestingEdgeInk(palette, tuning, layout, model, at) {
  const head = model.from[at];
  const tail = model.to[at];
  const code = model.kind[at];
  const word = KNOWLEDGE_EDGE_KINDS[code];
  const typed = code !== KNOWLEDGE_EDGE_CODE.mentions && word !== "merge";
  if (model.ghost[at] === 1) {
    return { ink: palette.ghostEdge, inkAt: 0, width: tuning.edgeMentionsWidth, dash: tuning.edgeGhostDash };
  }
  if (typed || word === "merge") {
    return { ink: palette.typed, inkAt: code * 4, width: palette.widths[code],
      dash: palette.widths[KNOWLEDGE_EDGE_KINDS.length + code] };
  }
  const intra = layout.community[head] === layout.community[tail];
  const hue = intra ? layout.communityHue[layout.community[head]] : -1;
  if (intra && hue >= 0) return { ink: palette.intra, inkAt: hue * 4, width: tuning.edgeMentionsWidth, dash: 0 };
  return { ink: palette.inter, inkAt: 0, width: tuning.edgeMentionsWidth, dash: 0 };
}

/* 팔레트 — SVG가 입는 옷을 그대로 입힌 견본에게 물어서 얻는다.
 *
 * 견본은 판 안에 사는 0×0짜리 `<svg class="knowledge-picture">`이고, 그 안의 점과
 * 선은 진짜 점·선과 **같은 클래스와 같은 `data-hue`**를 단다. 그래서 여기서 읽는
 * 것은 우리가 옮겨 적은 색이 아니라 스타일시트가 계산한 색이다: 토큰을 고치면
 * 두 그림이 함께 바뀐다. 테마가 바뀌면 다시 묻는다(견본은 그대로 두고 값만). */
function knowledgeGlPalette(view, held) {
  const palette = held ?? {
    swatches: null,
    theme: "",
    hues: 0,
    /* 색 한 칸은 실수 넷이다. 티어 셋 × 색 칸 + 중립 하나. */
    nodeFill: null,
    nodeStroke: null,
    ghost: new Float32Array(8),
    source: new Float32Array(8),
    codeFile: new Float32Array(8),
    codeSymbol: new Float32Array(8),
    /* 공급망의 점(P4) — 구성요소는 (생태계 없음 + 생태계) × (멤버 아님·멤버), 취약점은 (심각도 없음 +
     * 심각도) × (정보성 아님·정보성) 줄이고, 줄마다 몸·테두리의 색과 테두리 굵기다
     * (`knowledgeGlSupplyRow`). */
    component: null,
    vulnerability: null,
    foldRing: new Float32Array(4),
    intra: null,
    inter: new Float32Array(4),
    typed: null,
    ghostEdge: new Float32Array(4),
    farEdge: new Float32Array(4),
    litEdge: new Float32Array(4),
    highlight: new Float32Array(4),
    tint: null,
    ground: new Float32Array(4),
    ring: new Float32Array(4),
    nebula: null,
    /* 관계 종류마다 굵기 하나와 점선 하나 — 종류의 수가 자리를 정한다. */
    widths: new Float32Array(KNOWLEDGE_EDGE_KINDS.length * 2),
  };
  const theme = `${document.documentElement.dataset.theme ?? "dark"}`;
  if (palette.swatches !== null && palette.theme === theme) return palette;
  const hues = KNOWLEDGE_HUES;
  const tiers = KNOWLEDGE_TIER_WORD;
  if (palette.swatches === null) {
    const host = document.createElementNS(SVG_NS, "svg");
    host.setAttribute("class", "knowledge-picture knowledge-gl-swatch");
    host.setAttribute("aria-hidden", "true");
    const add = (className, hue, tier, words = {}) => {
      const group = document.createElementNS(SVG_NS, "g");
      group.setAttribute("class", className);
      if (hue >= 0) group.dataset.hue = String(hue);
      if (tier) group.dataset.tier = tier;
      /* 점이 든 낱말(`knowledgeSupplyDress`) 그대로 — 옷은 그 낱말을 읽는 CSS의 것이다. */
      for (const [name, value] of Object.entries(words)) group.dataset[name] = value;
      const dot = document.createElementNS(SVG_NS, "circle");
      dot.setAttribute("class", "knowledge-dot");
      group.appendChild(dot);
      host.appendChild(group);
      return dot;
    };
    const line = (className, hue) => {
      const group = document.createElementNS(SVG_NS, "g");
      if (hue >= 0) group.dataset.hue = String(hue);
      const path = document.createElementNS(SVG_NS, "path");
      path.setAttribute("class", className);
      group.appendChild(path);
      host.appendChild(group);
      return path;
    };
    const dots = [];
    for (let hue = 0; hue < hues; hue += 1) {
      for (const tier of tiers) dots.push(add("knowledge-node is-page", hue, tier));
    }
    for (const tier of tiers) dots.push(add("knowledge-node is-page", -1, tier));
    const intra = [];
    for (let hue = 0; hue < hues; hue += 1) {
      intra.push(line("knowledge-edge is-intra", hue));
    }
    const nebulae = [];
    for (let hue = 0; hue < hues; hue += 1) {
      const group = document.createElementNS(SVG_NS, "g");
      group.dataset.hue = String(hue);
      const disc = document.createElementNS(SVG_NS, "ellipse");
      disc.setAttribute("class", "knowledge-nebula");
      group.appendChild(disc);
      host.appendChild(group);
      nebulae.push(disc);
    }
    const halo = document.createElementNS(SVG_NS, "circle");
    halo.setAttribute("class", "knowledge-halo");
    const recalled = document.createElementNS(SVG_NS, "g");
    recalled.setAttribute("class", "knowledge-node is-page is-recalled");
    recalled.appendChild(halo);
    host.appendChild(recalled);
    /* 접힌 점의 점선 고리(`.is-folded .knowledge-halo`) — 주변 탐색의 허브와 공급망의 멤버. */
    const foldHalo = document.createElementNS(SVG_NS, "circle");
    foldHalo.setAttribute("class", "knowledge-halo");
    const folded = document.createElementNS(SVG_NS, "g");
    folded.setAttribute("class", "knowledge-node is-page is-folded");
    folded.appendChild(foldHalo);
    host.appendChild(folded);
    const components = [];
    for (let ecosystem = -1; ecosystem < KNOWLEDGE_SUPPLY_ECOSYSTEMS.length; ecosystem += 1) {
      for (const member of ["false", "true"]) {
        components.push(add("knowledge-node is-component", -1, "leaf", {
          ...(ecosystem < 0 ? {} : { ecosystem: KNOWLEDGE_SUPPLY_ECOSYSTEMS[ecosystem].id }), member }));
      }
    }
    const vulnerabilities = [];
    for (let severity = -1; severity < KNOWLEDGE_SUPPLY_SEVERITIES.length; severity += 1) {
      for (const informational of [false, true]) {
        vulnerabilities.push(add("knowledge-node is-vulnerability", -1, "leaf", {
          ...(severity < 0 ? {} : { severity: KNOWLEDGE_SUPPLY_SEVERITIES[severity].id }),
          ...(informational ? { informational: Object.keys(KNOWLEDGE_SUPPLY_INFORMATIONAL)[0] } : {}) }));
      }
    }
    palette.swatches = {
      host,
      dots,
      intra,
      nebulae,
      halo,
      foldHalo,
      components,
      vulnerabilities,
      ghost: add("knowledge-node is-ghost", -1, "leaf"),
      source: add("knowledge-node is-source", -1, "leaf"),
      codeFile: add("knowledge-node is-code_file", -1, "leaf"),
      codeSymbol: add("knowledge-node is-code_symbol", -1, "leaf"),
      selected: add("knowledge-node is-page is-selected", -1, "leaf"),
      match: add("knowledge-node is-page is-search-match", -1, "leaf"),
      inter: line("knowledge-edge is-inter", -1),
      ghostEdge: line("knowledge-edge is-ghost", -1),
      lit: line("knowledge-edge is-lit", -1),
      typed: KNOWLEDGE_EDGE_KINDS.map((kind) => line(kind === "mentions"
        ? "knowledge-edge"
        : `knowledge-edge ${kind === "merge" ? "kind-merge" : `is-typed kind-${kind}`}`, -1)),
    };
    /* 짚는 중 물러선 선의 잉크는 **판**의 클래스가 정한다(`.is-tracing`). 그래서 그
     * 견본만은 제 판을 따로 가진다 — 한 판에 `is-tracing`을 걸면 그 판의 모든 선
     * 견본이 물러선 잉크로 읽히고(실측: 선이 알파 0.1로 나와 그림에서 사라졌다),
     * 우리는 옅어진 색을 「이 선의 색」으로 외운다. */
    const tracing = document.createElementNS(SVG_NS, "svg");
    tracing.setAttribute("class", "knowledge-picture is-tracing knowledge-gl-swatch");
    tracing.setAttribute("aria-hidden", "true");
    const far = document.createElementNS(SVG_NS, "path");
    far.setAttribute("class", "knowledge-edge");
    tracing.appendChild(far);
    palette.swatches.far = far;
    palette.swatches.tracing = tracing;
    view.append(host, tracing);
    palette.nodeFill = new Float32Array((hues + 1) * tiers.length * 4);
    palette.nodeStroke = new Float32Array((hues + 1) * tiers.length * 4);
    palette.intra = new Float32Array(hues * 4);
    palette.typed = new Float32Array(KNOWLEDGE_EDGE_KINDS.length * 4);
    palette.nebula = new Float32Array(hues * 4);
    palette.hues = hues;
    const rows = (count) => ({ fill: new Float32Array(count * 4), stroke: new Float32Array(count * 4),
      width: new Float32Array(count) });
    palette.component = rows(components.length);
    palette.vulnerability = rows(vulnerabilities.length);
  }
  const swatches = palette.swatches;
  const read = (node) => getComputedStyle(node);
  for (let at = 0; at < swatches.dots.length; at += 1) {
    const style = read(swatches.dots[at]);
    knowledgeGlColor(style.fill, palette.nodeFill, at * 4);
    knowledgeGlColor(style.stroke, palette.nodeStroke, at * 4);
  }
  knowledgeGlColor(read(swatches.ghost).fill, palette.ghost, 0);
  knowledgeGlColor(read(swatches.ghost).stroke, palette.ghost, 4);
  knowledgeGlColor(read(swatches.source).fill, palette.source, 0);
  knowledgeGlColor(read(swatches.source).stroke, palette.source, 4);
  for (const [swatch, into] of [[swatches.codeFile, palette.codeFile],
    [swatches.codeSymbol, palette.codeSymbol]]) {
    knowledgeGlColor(read(swatch).fill, into, 0);
    knowledgeGlColor(read(swatch).stroke, into, 4);
  }
  knowledgeGlColor(read(swatches.selected).stroke, palette.tint ??= new Float32Array(4), 0);
  knowledgeGlColor(read(swatches.match).stroke, palette.highlight, 0);
  for (let hue = 0; hue < hues; hue += 1) {
    knowledgeGlColor(read(swatches.intra[hue]).stroke, palette.intra, hue * 4);
    const disc = read(swatches.nebulae[hue]);
    knowledgeGlColor(disc.fill === "none" || disc.fill.startsWith("url")
      ? getComputedStyle(swatches.nebulae[hue].parentNode).getPropertyValue("--knowledge-tint")
      : disc.fill, palette.nebula, hue * 4);
  }
  knowledgeGlColor(read(swatches.inter).stroke, palette.inter, 0);
  knowledgeGlColor(read(swatches.ghostEdge).stroke, palette.ghostEdge, 0);
  knowledgeGlColor(read(swatches.lit).stroke, palette.litEdge, 0);
  knowledgeGlColor(read(swatches.far).stroke, palette.farEdge, 0);
  knowledgeGlColor(read(swatches.halo).stroke, palette.ring, 0);
  knowledgeGlColor(read(swatches.foldHalo).stroke, palette.foldRing, 0);
  for (const [dots, into] of [[swatches.components, palette.component],
    [swatches.vulnerabilities, palette.vulnerability]]) {
    dots.forEach((dot, at) => {
      const style = read(dot);
      knowledgeGlColor(style.fill, into.fill, at * 4);
      knowledgeGlColor(style.stroke, into.stroke, at * 4);
      into.width[at] = Number.parseFloat(style.strokeWidth) || 0;
    });
  }
  const kinds = KNOWLEDGE_EDGE_KINDS.length;
  for (let at = 0; at < kinds; at += 1) {
    const style = read(swatches.typed[at]);
    knowledgeGlColor(style.stroke, palette.typed, at * 4);
    /* `related`는 옅게 선다 — 그 몫은 색이 아니라 불투명도라 여기서 곱해 둔다. */
    palette.typed[at * 4 + 3] *= Number.parseFloat(style.opacity) || 1;
    palette.widths[at] = Number.parseFloat(style.strokeWidth) || 1;
    palette.widths[kinds + at] = Number.parseFloat(style.strokeDasharray) || 0;
  }
  palette.theme = theme;
  return palette;
}

/* 프로그램 하나 — 짓고, 붙이고, 틀리면 이름을 말한다. */
function knowledgeGlProgram(gl, vertexSource, fragmentSource, name) {
  const compile = (kind, source) => {
    const shader = gl.createShader(kind);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      const why = gl.getShaderInfoLog(shader);
      gl.deleteShader(shader);
      throw new Error(`knowledge gl ${name}: ${why}`);
    }
    return shader;
  };
  const vertex = compile(gl.VERTEX_SHADER, vertexSource);
  const fragment = compile(gl.FRAGMENT_SHADER, fragmentSource);
  const program = gl.createProgram();
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const why = gl.getProgramInfoLog(program);
    gl.deleteProgram(program);
    throw new Error(`knowledge gl ${name}: ${why}`);
  }
  return program;
}

/* 한 패스의 살림: 프로그램, VAO, 인스턴스 버퍼들, 유니폼 자리. */
function knowledgeGlPass(gl, program, spec) {
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  const corner = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, corner);
  gl.bufferData(gl.ARRAY_BUFFER, KNOWLEDGE_GL_QUAD, gl.STATIC_DRAW);
  const cornerAt = gl.getAttribLocation(program, "aCorner");
  gl.enableVertexAttribArray(cornerAt);
  gl.vertexAttribPointer(cornerAt, 2, gl.FLOAT, false, 0, 0);
  const buffers = {};
  for (const row of spec) {
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    const at = gl.getAttribLocation(program, row.name);
    gl.enableVertexAttribArray(at);
    if (row.integer) gl.vertexAttribIPointer(at, row.size, row.type, 0, 0);
    else gl.vertexAttribPointer(at, row.size, row.type, row.normalized === true, 0, 0);
    gl.vertexAttribDivisor(at, 1);
    buffers[row.name] = buffer;
  }
  gl.bindVertexArray(null);
  return {
    program,
    vao,
    corner,
    buffers,
    uniforms: {
      seats: gl.getUniformLocation(program, "uSeats"),
      camera: gl.getUniformLocation(program, "uCamera"),
      viewport: gl.getUniformLocation(program, "uViewport"),
      seatWide: gl.getUniformLocation(program, "uSeatWide"),
      feather: gl.getUniformLocation(program, "uFeather"),
      pixelRatio: gl.getUniformLocation(program, "uPixelRatio"),
    },
  };
}

/* ---- GL 페인터 -------------------------------------------------------------
 *
 * `KnowledgePainter`의 다섯 손(`mount`·`paintTopology`·`paintFrame`·`pick`·
 * `dispose`)을 그대로 쓴다. 같은 `layout`을 읽고, 같은 카메라를 받고, 같은
 * 이름표 격자를 부른다 — 다른 것은 칠하는 손뿐이다. */
function makeKnowledgeGlPainter() {
  return {
    id: "gl",
    view: null,
    canvas: null,
    gl: null,
    palette: null,
    passes: null,
    seatTexture: null,
    seatWide: 0,
    /* 텍스처의 저장소가 잡힌 한 변 — 그림이 자랄 때만 다시 잡는다. */
    seatStorage: 0,
    /* 프레임마다 다시 채우는 통들. 새로 짓지 않는다 — 만 점에서 프레임마다
     * 배열을 짓는 것은 그리는 값보다 비싸다. */
    seatData: null,
    nodeSeat: null,
    nodeFill: null,
    nodeStroke: null,
    nodeShape: null,
    edgeEnds: null,
    edgeInk: null,
    edgeShape: null,
    discData: null,
    discInk: null,
    discRim: null,
    ringSeat: null,
    ringInk: null,
    ringShape: null,
    labels: null,
    labelPool: null,
    /* 위상·옷이 마지막으로 올라간 자리 — 카메라만 움직인 프레임은 아무것도 올리지
     * 않는다(설계 §6의 표). */
    uploaded: -1,
    /* 옷(점·선의 색과 흐림)이 자리와 따로 낡았다 — 호버는 프레임 없이 클래스만
     * 바꾸므로(`paintDress`), 다음 카메라 프레임이 옷만 다시 올린다. */
    dressStale: false,
    counts: { nodes: 0, edges: 0, discs: 0, rings: 0, labels: 0, draws: 0 },
    lost: false,
    onLost: null,

    mount(view) {
      this.view = view;
      const host = view.querySelector(".knowledge-canvas");
      const canvas = document.createElement("canvas");
      canvas.className = "knowledge-gl";
      canvas.setAttribute("aria-hidden", "true");
      host.insertBefore(canvas, host.firstChild);
      const labels = document.createElement("div");
      labels.className = "knowledge-gl-labels";
      labels.setAttribute("aria-hidden", "true");
      host.appendChild(labels);
      this.canvas = canvas;
      this.labels = labels;
      this.labelPool = [];
      view.querySelector(".knowledge-picture")?.classList.add("is-gl");
      const gl = canvas.getContext("webgl2", {
        alpha: true,
        antialias: false,
        depth: false,
        stencil: false,
        /* 버퍼는 알파를 곱한 색이다 — 아래의 섞기(`SRC_ALPHA`, `ONE_MINUS_SRC_ALPHA`)가 셰이더의
         * 곧은 색에 알파를 곱해 쓴다. 곱하지 않은 버퍼(`false`)라고 말하면 합성기가 알파를 한
         * 번 더 곱해 반투명 잉크(원본·흐림·성운)가 SVG보다 옅어진다(실측 09-17, 같은 판: 원본
         * 마름모의 한가운데 SVG (57,69,70) → GL (26,32,34), `true`에서 (58,69,70)). */
        premultipliedAlpha: true,
        /* 그린 버퍼를 붙들지 않는다. 붙들면 브라우저가 캔버스 크기의 색 버퍼 한 벌을
         * 더 들고(1063×1000 판·dpr 2에서 약 17 MB) 프레임마다 그리로 복사한다 —
         * 그 버퍼를 읽는 사람이 없다: 매 프레임 전부 다시 그리고, 스크린샷은 합성된
         * 화면을 찍는다(검증 09-16, 시험 어디에도 픽셀 읽기가 없다). */
        preserveDrawingBuffer: false,
      });
      if (gl === null) {
        /* 여기까지 왔는데 문맥이 없으면 그릴 수 없다 — 2D로 돌아간다. 이미 세운
         * 캔버스·오버레이·견본은 먼저 걷는다: 이 손은 판에 매이지 않으므로(돌아가는
         * 손이 판을 차지한다) 여기서 놓지 않으면 누구도 놓지 않는다. */
        this.lost = true;
        this.dispose();
        knowledgeGlFellBack(view);
        return;
      }
      this.gl = gl;
      /* 문맥을 잃으면(잠에서 깬 판·드라이버 재시작) 렌즈를 접고 한 줄 알린다. */
      this.onLost = (event) => {
        event.preventDefault();
        this.lost = true;
        knowledgeGlFellBack(view);
      };
      canvas.addEventListener("webglcontextlost", this.onLost);
      try {
        this.passes = {
          disc: knowledgeGlPass(gl,
            knowledgeGlProgram(gl, KNOWLEDGE_GL_DISC_VERT, KNOWLEDGE_GL_DISC_FRAG, "disc"), [
              { name: "aDisc", size: 4, type: gl.FLOAT },
              { name: "aInk", size: 4, type: gl.FLOAT },
              { name: "aRim", size: 2, type: gl.FLOAT },
            ]),
          edge: knowledgeGlPass(gl,
            knowledgeGlProgram(gl, KNOWLEDGE_GL_EDGE_VERT, KNOWLEDGE_GL_EDGE_FRAG, "edge"), [
              { name: "aEnds", size: 2, type: gl.UNSIGNED_INT, integer: true },
              { name: "aInk", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
              { name: "aShape", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
            ]),
          node: knowledgeGlPass(gl,
            knowledgeGlProgram(gl, KNOWLEDGE_GL_NODE_VERT, knowledgeGlNodeFragment(), "node"), [
              { name: "aSeat", size: 1, type: gl.UNSIGNED_INT, integer: true },
              { name: "aFill", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
              { name: "aStroke", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
              { name: "aShape", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
            ]),
          ring: knowledgeGlPass(gl,
            knowledgeGlProgram(gl, KNOWLEDGE_GL_RING_VERT, KNOWLEDGE_GL_RING_FRAG, "ring"), [
              { name: "aSeat", size: 1, type: gl.UNSIGNED_INT, integer: true },
              { name: "aInk", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
              { name: "aShape", size: 4, type: gl.UNSIGNED_BYTE, normalized: true },
            ]),
        };
      } catch (trouble) {
        /* 셰이더가 서지 않는 판이 있다(드라이버·판 버전). 그 판은 2D로 그린다 —
         * 창을 멈추는 대신. 무엇이 틀렸는지는 창의 오류 파일이 받는다. */
        this.lost = true;
        invoke("note_webview_error", { text: `knowledge gl: ${trouble}` }).catch(() => {});
        /* 서지 못한 셰이더 앞의 것들(캔버스·문맥·리스너·지어 둔 패스)을 놓는다 —
         * 한 번의 실패가 판에 캔버스와 GPU 문맥을 하나씩 남기지 않게. */
        this.dispose();
        knowledgeGlFellBack(view);
        return;
      }
      this.seatTexture = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, this.seatTexture);
      /* NEAREST만 쓴다 — 부동소수 텍스처의 필터는 판마다 다르고(설계 §9),
       * `texelFetch`는 애초에 필터를 묻지 않는다. */
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.enable(gl.BLEND);
      gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      this.palette = knowledgeGlPalette(view, null);
    },

    /* 위상의 순간. SVG는 여기서 만 개의 <g>를 세우고, 이 손은 통의 크기만 맞춘다. */
    paintTopology(layout) {
      const view = this.view;
      /* 점의 층은 비운다 — 두 그림이 겹쳐 서면 만 쪽에서 SVG의 값을 그대로 치른다. */
      const host = view.querySelector(".knowledge-nodes");
      if (host.firstChild !== null) host.replaceChildren();
      host.dataset.knowledgeSignature = layout.model.signature;
      host.dataset.knowledgeVault = layout.model.vault;
      host.dataset.knowledgeDrawn = layout.drawnStamp;
      const edges = view.querySelector(".knowledge-edges");
      if (edges.firstChild !== null) edges.replaceChildren();
      layout.nodeEls = [];
      layout.edgeEls = [];
      layout.paintedModel = layout.model;
      /* 이름표의 어림 폭은 SVG의 점 페인터가 쓰던 수다 — 여기서도 한 번 채운다
       * (격자가 프레임마다 읽는다). */
      const { titles } = layout.model;
      const tuning = layout.tuning;
      for (let at = 0; at < layout.count; at += 1) {
        const fold = layout.folded === null ? 0 : layout.folded[at];
        layout.labelEm[at] = knowledgeLabelEm(
          knowledgeShortWord(titles[at], tuning.labelMax), tuning,
        ) + (fold > 0 ? knowledgeLabelEm(`+${fold}`, tuning) : 0);
      }
      this.reserve(layout);
      this.uploaded = -1;
    },

    /* 옷만 바뀐 순간(호버). 요소가 없으니 클래스를 입을 자리도 없다 — 옷을 낡음으로
     * 적고 카메라 프레임 하나를 그린다: 자리는 그대로이고 점·선의 색과 흐림,
     * 그리고 그 옷을 따라 우선순위가 바뀐 이름표만 다시 선다. 아직 위상을 올리지
     * 않은 손은 할 일이 없다 — 곧 올 그림이 옷까지 입힌다. */
    paintDress(layout) {
      if (this.lost || this.gl === null || this.uploaded === -1) return;
      this.dressStale = true;
      paintKnowledgeFrame(this.view, layout, { cameraOnly: true });
    },

    /* 통을 이 그림의 크기에 맞춘다. 자라기만 한다 — 렌즈가 절반을 숨긴 판이
     * 통을 줄였다가 돌아오면 그 자리에서 다시 짓게 된다. */
    reserve(layout) {
      const count = layout.count;
      const edgeCount = layout.model.edgeCount;
      const wide = Math.max(1, Math.ceil(Math.sqrt(count)));
      if (this.seatData === null || this.seatData.length < wide * wide * 4) {
        this.seatData = new Float32Array(wide * wide * 4);
        this.seatWide = wide;
        this.uploaded = -1;
      }
      this.seatWide = wide;
      if (this.nodeSeat === null || this.nodeSeat.length < count) {
        this.nodeSeat = new Uint32Array(count);
        this.nodeFill = new Uint8Array(count * 4);
        this.nodeStroke = new Uint8Array(count * 4);
        this.nodeShape = new Uint8Array(count * 4);
        /* 점 하나에 고리가 둘일 수 있다 — 회상의 고리와 접힘의 고리. */
        this.ringSeat = new Uint32Array(count * 2);
        this.ringInk = new Uint8Array(count * 8);
        this.ringShape = new Uint8Array(count * 8);
      }
      if (this.edgeEnds === null || this.edgeEnds.length < edgeCount * 2) {
        this.edgeEnds = new Uint32Array(Math.max(1, edgeCount) * 2);
        this.edgeInk = new Uint8Array(Math.max(1, edgeCount) * 4);
        this.edgeShape = new Uint8Array(Math.max(1, edgeCount) * 4);
      }
      const named = layout.namedCount;
      if (this.discData === null || this.discData.length < Math.max(1, named) * 4) {
        this.discData = new Float32Array(Math.max(1, named) * 4);
        this.discInk = new Float32Array(Math.max(1, named) * 4);
        this.discRim = new Float32Array(Math.max(1, named) * 2);
      }
    },

    paintFrame(layout, camera, { cameraOnly = false } = {}) {
      const view = this.view;
      if (this.lost || this.gl === null) return;
      const picture = view.querySelector(".knowledge-picture");
      const inverse = camera.inverse;
      /* 이름판과 관계의 낱말은 여전히 이 <svg> 안에 산다(군집 수·이웃 수만큼이라
       * DOM이 값을 치르지 않는다). 그래서 카메라는 두 손이 함께 쓴다. */
      writeAttribute(picture, "viewBox", `${camera.x} ${camera.y} ${camera.wide} ${camera.tall}`);
      this.palette = knowledgeGlPalette(view, this.palette);
      const tuning = layout.tuning;
      const feather = Number.isFinite(tuning.glFeather) ? tuning.glFeather : 1;
      const gl = this.gl;
      const wide = view.querySelector(".knowledge-canvas").clientWidth;
      const tall = view.querySelector(".knowledge-canvas").clientHeight;
      const cap = Number.isFinite(tuning.glPixelCap) ? tuning.glPixelCap : 2;
      const dpr = Math.max(1, Math.min(window.devicePixelRatio || 1, cap));
      const pixelWide = Math.max(1, Math.round(wide * dpr));
      const pixelTall = Math.max(1, Math.round(tall * dpr));
      if (this.canvas.width !== pixelWide || this.canvas.height !== pixelTall) {
        this.canvas.width = pixelWide;
        this.canvas.height = pixelTall;
      }
      gl.viewport(0, 0, pixelWide, pixelTall);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      /* 두 가지가 따로 낡는다. 자리(위치 텍스처)는 훑기·위상·판이 바뀐 프레임에서,
       * 옷(점·선의 색과 흐림)은 거기에 더해 프레임 없이 클래스만 바뀐 뒤에
       * (`paintDress`). 카메라만 움직인 프레임은 둘 다 올리지 않는다. */
      const seats = !cameraOnly || this.uploaded !== layout.geometryRevision;
      const dress = seats || this.dressStale;
      if (seats) {
        knowledgeSeatDrawY(layout, camera);
        this.reserve(layout);
        this.fillSeats(layout);
      }
      if (dress) {
        this.fillNodes(layout, picture);
        this.fillEdges(layout, picture);
        this.dressStale = false;
      }
      /* 고리 위 이름표의 자리. 격자가 이 수(`ringLabelAt`)로 상자를 쥐므로 격자보다
       * 먼저 잰다 — SVG 손은 점을 쓰는 고리에서 같은 셈을 부른다. 배율의 역수를
       * 읽으므로 프레임마다이고, 고리의 점은 이웃의 수만큼이다. */
      if (layout.ring !== null) {
        for (let at = 0; at < layout.count; at += 1) {
          if (layout.drawn !== null && layout.drawn[at] === 0) continue;
          knowledgeRingLabelSeat(layout, at);
        }
      }
      /* 성운은 군집 수만큼이라 프레임마다 채운다 — 그 고리가 이름판의 자리를
       * 정하고(`paintKnowledgeClusters`), 카메라만 움직인 프레임도 이름판은 선다. */
      paintKnowledgeClusters(layout, inverse, camera.yScale, camera.middleY);
      this.fillDiscs(layout);
      this.draw(layout, camera, pixelWide, pixelTall, feather, dpr, seats, dress);
      placeKnowledgeLabels(view, layout, camera, inverse);
      this.paintLabels(layout);
      paintKnowledgeEdgeLabels(view, layout);
      const scaleWord = view.querySelector(".knowledge-zoom-label");
      if (scaleWord) writeTextContent(scaleWord, `${Math.round(layout.zoom * 100)}%`);
      if (seats) this.uploaded = layout.geometryRevision;
      layout.paintedGeometry = { revision: layout.geometryRevision, scale: camera.scale,
        zoom: layout.zoom, yScale: camera.yScale, middleY: camera.middleY };
      const counts = this.counts;
      const stats = layout.paintStats;
      stats.draws = counts.draws;
      stats.points = counts.nodes;
      stats.edges = counts.edges;
      stats.labels = counts.labels;
    },

    /* 위치 텍스처 한 장: x, 그린 y, z(2D에서는 0), 화면 반지름. */
    fillSeats(layout) {
      const { count, x, drawY, radius, drawn } = layout;
      const data = this.seatData;
      for (let at = 0; at < count; at += 1) {
        const seat = at * 4;
        data[seat] = x[at];
        data[seat + 1] = drawY[at];
        data[seat + 2] = 0;
        /* 그려지지 않는 점은 반지름 0으로 둔다 — 선이 그 점에서 잘리지 않게. */
        data[seat + 3] = drawn !== null && drawn[at] === 0 ? 0 : radius[at];
      }
    },

    /* 점의 옷 — SVG의 클래스가 말하던 것을 바이트로. 무엇이 흐리고 무엇이 밝은지는
     * 판의 클래스(`is-tracing`·`is-focused`…)가 정하고, 그 판정은 두 손이 같은
     * 자리에서 읽는다. */
    fillNodes(layout, picture) {
      const model = layout.model;
      const tuning = layout.tuning;
      const palette = this.palette;
      const on = picture.classList;
      const tracing = on.contains("is-tracing");
      const focused = on.contains("is-focused");
      const searching = on.contains("is-searching");
      const sliced = on.contains("is-sliced");
      const spotlight = on.contains("is-spotlight");
      const pathed = on.contains("is-path");
      const dim = tuning.dimOpacity;
      const far = tuning.farNodeOpacity;
      const searchOn = knowledgeQuery.trim() !== "";
      const spot = knowledgeClusterPicked;
      const selectedSeat = knowledgeSelectedKey === null ? -1
        : model.keys.indexOf(knowledgeSelectedKey);
      let nodes = 0;
      let rings = 0;
      for (let at = 0; at < layout.count; at += 1) {
        if (layout.drawn !== null && layout.drawn[at] === 0) continue;
        const kind = model.kinds[at];
        const ghost = kind === "ghost";
        const source = kind === "source";
        const rank = layout.community[at];
        const lit = layout.litNodes.has(at);
        const selected = at === selectedSeat;
        const match = layout.searchMatch[at] === 1;
        const pathLit = layout.pathNode[at] === 1;
        /* 가장 물러선 상태가 이긴다 — 겹친 흐림 중 어느 하나라도 물러서라고 하면
         * 그 점은 물러선다. CSS에서는 마지막 규칙이 이기고, 여기서는 최솟값이다. */
        let alpha = 1;
        if (tracing && !lit) alpha = Math.min(alpha, far);
        if (focused && layout.focusVisited[at] !== 1) alpha = Math.min(alpha, dim);
        if (searching && searchOn && !match) alpha = Math.min(alpha, dim);
        if (sliced && layout.sliceMatch[at] === 0) alpha = Math.min(alpha, dim);
        if (spotlight && spot >= 0 && rank !== spot) alpha = Math.min(alpha, dim);
        if (pathed && !pathLit) alpha = Math.min(alpha, dim);
        /* 쉬는 옷은 한 함수(`knowledgeRestingNodeInk`)가 고른다 — 내보내기가 같은 손으로 읽는다.
         * 상태(고름·밝힘·경로·검색)의 테두리만 여기서 덧입힌다; 공급망의 점(P4)도 제 줄의 옷 위에
         * 같은 상태의 테두리를 입는다. */
        const resting = knowledgeRestingNodeInk(palette, tuning, layout, model, at);
        const fill = resting.fill;
        const fillAt = resting.fillAt;
        const stated = !ghost && !source && (selected || lit || pathLit);
        const strokeSource = stated ? (pathLit ? palette.highlight : palette.tint)
          : !ghost && !source && match ? palette.highlight
          : resting.stroke;
        const strokeAt = stated || (!ghost && !source && match) ? 0 : resting.strokeAt;
        const strokePx = stated ? tuning.nodeSelectedStroke
          : !ghost && !source && match ? tuning.nodeMatchStroke
          : resting.strokePx;
        const seat = nodes;
        this.nodeSeat[seat] = at;
        this.writeInk(this.nodeFill, seat * 4, fill, fillAt, alpha);
        this.writeInk(this.nodeStroke, seat * 4, strokeSource, strokeAt, alpha);
        this.nodeShape[seat * 4] = Math.min(255, Math.round(strokePx * 8));
        this.nodeShape[seat * 4 + 1] = 0;
        /* 모양의 번호와 점선 — 둘 다 모양의 표에서 온다(`KNOWLEDGE_SHAPES`). 점선이 없는
         * 모양은 점선의 길이가 0이고, 셰이더는 그 0 하나로 점선을 건너뛴다. */
        const form = KNOWLEDGE_SHAPES[knowledgeShapeOf(kind)];
        this.nodeShape[seat * 4 + 2] = form.dashed
          ? Math.min(255, Math.round(tuning.nodeGhostDash * 4))
          : 0;
        this.nodeShape[seat * 4 + 3] = form.code;
        nodes += 1;
        if (model.recalledNow[at] === 1) {
          this.ringSeat[rings] = at;
          this.writeInk(this.ringInk, rings * 4, palette.ring, 0, alpha);
          this.ringShape[rings * 4] = Math.min(255, Math.round(tuning.glRingGap * 4));
          this.ringShape[rings * 4 + 1] = Math.min(255,
            Math.round(tuning.activityRingWidth * 8));
          this.ringShape[rings * 4 + 2] = Math.min(255,
            Math.round(tuning.activityRingDash * 4));
          this.ringShape[rings * 4 + 3] = 0;
          rings += 1;
        }
        /* 접힌 점의 점선 고리 — SVG의 halo 원(반지름 × `haloScale`)과 같은 자리·굵기·점선. 고리 패스가
         * 재는 틈은 몸에서 떨어진 거리이므로 그 차이를 적는다. */
        if (layout.folded !== null && layout.folded[at] > 0) {
          this.ringSeat[rings] = at;
          this.writeInk(this.ringInk, rings * 4, palette.foldRing, 0, alpha);
          this.ringShape[rings * 4] = Math.min(255,
            Math.round(layout.radius[at] * (tuning.haloScale - 1) * 4));
          this.ringShape[rings * 4 + 1] = Math.min(255, Math.round(tuning.nodeGhostStroke * 8));
          this.ringShape[rings * 4 + 2] = Math.min(255, Math.round(tuning.nodeGhostDash * 4));
          this.ringShape[rings * 4 + 3] = 0;
          rings += 1;
        }
      }
      this.counts.nodes = nodes;
      this.counts.rings = rings;
    },

    writeInk(into, at, from, fromAt, alpha) {
      into[at] = Math.round(from[fromAt] * 255);
      into[at + 1] = Math.round(from[fromAt + 1] * 255);
      into[at + 2] = Math.round(from[fromAt + 2] * 255);
      into[at + 3] = Math.round(Math.max(0, Math.min(1, from[fromAt + 3] * alpha)) * 255);
    },

    /* 선의 옷. 프레임의 SVG 페인터가 `measured`에 쓰던 판정을 같은 규칙으로 읽되,
     * 좌표는 짓지 않는다 — 선은 제 끝점의 **번호** 둘만 든다. */
    fillEdges(layout, picture) {
      const model = layout.model;
      const tuning = layout.tuning;
      const palette = this.palette;
      const on = picture.classList;
      const tracing = on.contains("is-tracing");
      const focused = on.contains("is-focused");
      const searching = on.contains("is-searching");
      const sliced = on.contains("is-sliced");
      const spotlight = on.contains("is-spotlight");
      const pathed = on.contains("is-path");
      const dim = tuning.dimEdgeOpacity;
      const far = tuning.farEdgeOpacity;
      const spot = knowledgeClusterPicked;
      const searchOn = knowledgeQuery.trim() !== "";
      const { from, to, edgeCount } = model;
      const drawnEdge = layout.drawnEdge;
      let edges = 0;
      for (let at = 0; at < edgeCount; at += 1) {
        if (drawnEdge !== null && drawnEdge[at] === 0) continue;
        const head = from[at];
        const tail = to[at];
        const lit = layout.lit.has(at);
        const focusLit = layout.focusEdgeVisited[at] === 1;
        const match = layout.searchMatch[head] === 1 && layout.searchMatch[tail] === 1;
        const spotlit = spot >= 0 && layout.community[head] === spot
          && layout.community[tail] === spot;
        const pathLit = layout.pathEdge[at] === 1;
        let alpha = 1;
        if (tracing && !lit) alpha = Math.min(alpha, far);
        if (focused && !focusLit) alpha = Math.min(alpha, dim);
        if (searching && searchOn && !match) alpha = Math.min(alpha, dim);
        if (sliced && (layout.sliceMatch[head] === 0 || layout.sliceMatch[tail] === 0)) {
          alpha = Math.min(alpha, dim);
        }
        if (spotlight && !spotlit) alpha = Math.min(alpha, dim);
        if (pathed && !pathLit) alpha = Math.min(alpha, dim);
        /* 쉬는 옷은 한 함수(`knowledgeRestingEdgeInk`)가 고른다 — 내보내기가 같은 손으로 읽는다. */
        const resting = knowledgeRestingEdgeInk(palette, tuning, layout, model, at);
        let ink = resting.ink;
        let inkAt = resting.inkAt;
        let width = resting.width;
        const dash = resting.dash;
        if (lit || (focused && focusLit)) {
          ink = palette.litEdge;
          inkAt = 0;
          width = tuning.edgeLitWidth;
        } else if (tracing) {
          ink = palette.farEdge;
          inkAt = 0;
        }
        this.edgeEnds[edges * 2] = head;
        this.edgeEnds[edges * 2 + 1] = tail;
        this.writeInk(this.edgeInk, edges * 4, ink, inkAt, alpha);
        this.edgeShape[edges * 4] = Math.min(255, Math.round(width * 8));
        this.edgeShape[edges * 4 + 1] = Math.min(255, Math.round(dash * 4));
        this.edgeShape[edges * 4 + 2] = 0;
        this.edgeShape[edges * 4 + 3] = 0;
        edges += 1;
      }
      this.counts.edges = edges;
    },

    /* 성운 — 군집마다 원판 하나. 자리와 반지름은 `paintKnowledgeClusters`가 방금
     * 쓴 것을 읽는다(두 손이 같은 원반을 그린다). */
    fillDiscs(layout) {
      const palette = this.palette;
      const tuning = layout.tuning;
      let discs = 0;
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        if (layout.clusterTally[rank] === 0) continue;
        const hue = layout.communityHue[rank];
        if (hue < 0) continue;
        const reach = layout.clusterReach[rank];
        this.discData[discs * 4] = layout.clusterX[rank];
        this.discData[discs * 4 + 1] = layout.clusterY[rank];
        this.discData[discs * 4 + 2] = reach;
        this.discData[discs * 4 + 3] = reach;
        this.discInk[discs * 4] = palette.nebula[hue * 4];
        this.discInk[discs * 4 + 1] = palette.nebula[hue * 4 + 1];
        this.discInk[discs * 4 + 2] = palette.nebula[hue * 4 + 2];
        this.discInk[discs * 4 + 3] = tuning.nebulaCore;
        this.discRim[discs * 2] = tuning.nebulaEdgeWeight / 100;
        this.discRim[discs * 2 + 1] = tuning.nebulaEdgeWidth;
        discs += 1;
      }
      this.counts.discs = discs;
    },

    /* 드로우 넷 — 성운, 선, 점, 고리. 인스턴스가 없는 패스는 호출도 없다.
     *
     * 카메라만 움직인 프레임은 **아무것도 올리지 않는다**(설계 §6의 표): 위치도
     * 옷도 그대로이고 바뀐 것은 행렬 하나뿐이다. 이 한 줄이 만 쪽의 궤도 프레임에서
     * 프레임마다의 163 KB 텍스처 재할당과 570 KB의 버퍼 쓰기를 없앤다. */
    draw(layout, camera, pixelWide, pixelTall, feather, dpr, seats, dress) {
      const gl = this.gl;
      const counts = this.counts;
      counts.draws = 0;
      /* 카메라는 판 픽셀의 것이고 캔버스는 기기 픽셀이라 배율에 dpr이 든다. */
      const cameraX = camera.x;
      const cameraY = camera.y;
      const scale = camera.scale * dpr;
      const seatWide = this.seatWide;
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, this.seatTexture);
      if (seats) {
        /* 자리는 한 번 잡고(`texStorage2D`) 그 뒤로는 덮어쓴다 — 프레임마다 저장소를
         * 다시 잡으면 드라이버가 옛 텍스처를 놓을 때까지 기다린다. */
        if (this.seatStorage !== seatWide) {
          gl.deleteTexture(this.seatTexture);
          this.seatTexture = gl.createTexture();
          gl.bindTexture(gl.TEXTURE_2D, this.seatTexture);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
          gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA32F, seatWide, seatWide);
          this.seatStorage = seatWide;
        }
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, seatWide, seatWide,
          gl.RGBA, gl.FLOAT, this.seatData);
      }
      const run = (pass, count, upload, uploads) => {
        if (count === 0) return;
        gl.useProgram(pass.program);
        gl.bindVertexArray(pass.vao);
        if (upload) {
          for (const [name, data] of uploads) {
            gl.bindBuffer(gl.ARRAY_BUFFER, pass.buffers[name]);
            gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
          }
        }
        if (pass.uniforms.seats !== null) gl.uniform1i(pass.uniforms.seats, 0);
        if (pass.uniforms.seatWide !== null) gl.uniform1i(pass.uniforms.seatWide, seatWide);
        gl.uniform3f(pass.uniforms.camera, cameraX, cameraY, scale);
        gl.uniform2f(pass.uniforms.viewport, pixelWide, pixelTall);
        if (pass.uniforms.feather !== null) gl.uniform1f(pass.uniforms.feather, feather * dpr);
        if (pass.uniforms.pixelRatio !== null) gl.uniform1f(pass.uniforms.pixelRatio, dpr);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
        counts.draws += 1;
      };
      /* 성운만은 프레임마다 올린다 — 군집 수만큼의 짧은 배열이고, 그 자리는
       * 카메라가 움직이면 이름판과 함께 다시 정해진다. 점·선·고리는 위상과 옷이
       * 바뀐 프레임에만 올라간다. */
      run(this.passes.disc, counts.discs, true, [
        ["aDisc", this.discData.subarray(0, counts.discs * 4)],
        ["aInk", this.discInk.subarray(0, counts.discs * 4)],
        ["aRim", this.discRim.subarray(0, counts.discs * 2)],
      ]);
      run(this.passes.edge, counts.edges, dress, [
        ["aEnds", this.edgeEnds.subarray(0, counts.edges * 2)],
        ["aInk", this.edgeInk.subarray(0, counts.edges * 4)],
        ["aShape", this.edgeShape.subarray(0, counts.edges * 4)],
      ]);
      run(this.passes.node, counts.nodes, dress, [
        ["aSeat", this.nodeSeat.subarray(0, counts.nodes)],
        ["aFill", this.nodeFill.subarray(0, counts.nodes * 4)],
        ["aStroke", this.nodeStroke.subarray(0, counts.nodes * 4)],
        ["aShape", this.nodeShape.subarray(0, counts.nodes * 4)],
      ]);
      run(this.passes.ring, counts.rings, dress, [
        ["aSeat", this.ringSeat.subarray(0, counts.rings)],
        ["aInk", this.ringInk.subarray(0, counts.rings * 4)],
        ["aShape", this.ringShape.subarray(0, counts.rings * 4)],
      ]);
      gl.bindVertexArray(null);
    },

    /* 이름표는 HTML이다 — 예산(≤ 46)만큼의 <span>이고 점의 수와 무관하다. 자리는
     * 격자가 **예약한 상자의 한가운데**다: 격자가 그 자리를 지켰으므로 겹치지
     * 않는다는 약속이 그림에서도 참이 된다. */
    paintLabels(layout) {
      const tuning = layout.tuning;
      const titles = layout.model.titles;
      const pool = this.labelPool;
      let shown = 0;
      for (let at = 0; at < layout.count; at += 1) {
        if (layout.labelShown[at] !== 1) continue;
        const where = layout.labelWhere[at];
        if (where < 0) continue;
        let word = pool[shown];
        if (word === undefined) {
          word = document.createElement("span");
          word.className = "knowledge-gl-label";
          pool.push(word);
          this.labels.appendChild(word);
        }
        /* 격자가 쥔 상자의 한가운데(`labelAtX/Y`). 자리를 번호에서 다시 셈하지
         * 않는다 — 고리 위에서는 번호가 네 후보를 모두 0으로 말하므로 다시 센
         * 자리가 격자가 지킨 상자와 어긋난다(검증 09-16). */
        const seatX = layout.labelAtX[at];
        const seatY = layout.labelAtY[at];
        const fold = layout.folded === null ? 0 : layout.folded[at];
        const said = knowledgeShortWord(titles[at], tuning.labelMax)
          + (fold > 0 ? ` +${fold}` : "");
        if (word.textContent !== said) word.textContent = said;
        /* 이름을 누르면 그 점이 골라진다 — SVG에서 이름이 점의 <g> 안에 있는 것과
         * 같은 손(`knowledgeUnder`가 이 열쇠를 읽는다). */
        const key = layout.model.keys[at];
        if (word.dataset.graphKey !== key) word.dataset.graphKey = key;
        writeStyleValue(word, "transform",
          `translate(${Math.round(seatX)}px, ${Math.round(seatY)}px) translate(-50%, -50%)`);
        if (word.hidden) word.hidden = false;
        shown += 1;
      }
      for (let at = shown; at < pool.length; at += 1) {
        if (!pool[at].hidden) pool[at].hidden = true;
      }
      this.counts.labels = shown;
    },

    pick(px, py) {
      const layout = knowledgeLayouts.get(this.view);
      return layout === undefined ? -1 : knowledgePickSeat(layout, px, py);
    },

    /* 놓는다 — 버퍼·텍스처·프로그램·VAO·리스너·오버레이 전부. 하나라도 남으면
     * 페인터를 갈아 끼울 때마다 GPU와 힙이 자란다(하네스가 열 번 갈아 끼우고
     * 그 자리를 잰다). */
    dispose() {
      const view = this.view;
      const gl = this.gl;
      if (gl !== null && this.passes !== null) {
        for (const pass of Object.values(this.passes)) {
          for (const buffer of Object.values(pass.buffers)) gl.deleteBuffer(buffer);
          gl.deleteBuffer(pass.corner);
          gl.deleteVertexArray(pass.vao);
          gl.deleteProgram(pass.program);
        }
      }
      if (gl !== null && this.seatTexture !== null) gl.deleteTexture(this.seatTexture);
      this.seatStorage = 0;
      if (this.canvas !== null && this.onLost !== null) {
        this.canvas.removeEventListener("webglcontextlost", this.onLost);
      }
      /* 문맥 자체도 놓는다 — 판마다의 GL 문맥 수는 브라우저가 열여섯 언저리로
       * 묶어 두고, 넘으면 가장 오래된 것을 말없이 잃는다. */
      gl?.getExtension("WEBGL_lose_context")?.loseContext();
      this.canvas?.remove();
      this.labels?.remove();
      this.palette?.swatches?.host.remove();
      this.palette?.swatches?.tracing?.remove();
      view?.querySelector(".knowledge-picture")?.classList.remove("is-gl");
      this.gl = null;
      this.passes = null;
      this.seatTexture = null;
      this.canvas = null;
      this.labels = null;
      this.labelPool = null;
      this.palette = null;
      this.seatData = null;
      this.nodeSeat = null;
      this.nodeFill = null;
      this.nodeStroke = null;
      this.nodeShape = null;
      this.edgeEnds = null;
      this.edgeInk = null;
      this.edgeShape = null;
      this.discData = null;
      this.discInk = null;
      this.discRim = null;
      this.ringSeat = null;
      this.ringInk = null;
      this.ringShape = null;
      this.onLost = null;
      this.view = null;
      this.uploaded = -1;
    },
  };
}

/* 공급망 점의 견본 줄(P4) — `knowledgeGlPalette`가 지은 순서 그대로: 구성요소는
 * (생태계 번호 + 1) × 2 + 멤버, 취약점은 (심각도 번호 + 1) × 2 + 정보성. 번호가 없는 점(답 밖에서 온
 * 점)은 생태계·심각도 없는 줄이다. */
function knowledgeGlSupplyRow(model, at) {
  if (model.kinds[at] === "component") {
    return ((model.ecosystem?.[at] ?? -1) + 1) * 2 + (model.member?.[at] === 1 ? 1 : 0);
  }
  return ((model.severity?.[at] ?? -1) + 1) * 2 + (model.informational?.[at] === 1 ? 1 : 0);
}
