/* ---- 지식 그래프의 두 번째 손 — three.js 위의 인스턴스 페인터 (6차 P2, 09-27 옮김) ----
 *
 * 왜 두 번째 손인가. 5차의 SVG는 점 하나에 요소 셋이고, 그 값이 만 쪽에서
 * 91,754개가 된다(P0의 표). 브라우저가 그것을 굽는 시간은 우리 호출 밖에 있어서
 * 프레임 간격이 841 ms가 되는데, 우리 쪽 처리 시간은 그중 772 ms이고 나머지는
 * 래스터화다 — 어느 쪽도 사람이 지도를 미는 동안 견딜 수 있는 수가 아니다.
 * 이 파일은 같은 그림을 **인스턴스 드로우 넷**으로 그린다.
 *
 * 왜 three.js인가. 처음(09-16)에는 손으로 쓴 WebGL2였다 — 「three.js 코어는
 * min ~680 KB, 여기서 쓰는 것은 직교 투영 하나·버퍼 몇 개·셰이더 넷뿐」이 그때의
 * 이유였고 외부 의존 0 바이트가 그 값이었다. 그 셈은 2026-09-27에 끝났다: 사람이
 * 관계 탭의 3D 보기를 three.js로 옮기기로 정했고(「바꾸는김에 지식그래프도 three.js
 * 쓰던지」), 창은 이미 `ui/vendor/three.js`(r160, IIFE, `window.THREE`)를 싣는다.
 * 그 뒤로 손으로 쓴 렌더러 하나를 따로 두는 것은 두 벌의 GL 살림(문맥·상태 캐시·
 * 버퍼 수명·문맥 잃음)을 두 사람이 따로 고치는 일이다. 그래서 이 손은 같은 렌더러
 * 위에 선다 — 그리는 것은 그대로다: 셰이더 넷은 글자 그대로 `RawShaderMaterial`
 * (GLSL 300 es)이고, 인스턴스는 `InstancedBufferGeometry`이며, 위치는 여전히 부동소수
 * 텍스처 한 장(`DataTexture`)이다. three.js가 맡는 것은 프로그램·VAO·버퍼·텍스처의
 * 수명과 GL 상태의 캐시다 — 문맥은 이 손이 열어 건넨다(`mount`). 필요한 기호는 `ui/vendor/build-three.mjs`의
 * ENTRY가 전부 든다 — 보이지 않는 기호는 거기에 더하고 다시 짓는다.
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
 * 무엇을 하지 않는가: 프러스텀 컬링(만 점의 드로우가 컬링 계산보다 싸다 —
 * 메시마다 `frustumCulled = false`), GPU 픽킹(`readPixels`의 동기 정지가 프레임을
 * 먹는다 — 픽킹은 CPU가 투영한 자리에서, `knowledgePickSeat` 한 벌), 3D(P3의
 * 일이다. 이 페인터는 z=0으로 SVG와 같은 그림을 그린다). three.js의 카메라·행렬은
 * 쓰지 않는다 — 투영은 셰이더의 두 줄(`KNOWLEDGE_GL_PROJECT`)이고, 렌더러에 주는
 * 직교 카메라는 `render(scene, camera)`의 서명을 채우는 것뿐이다. */

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

/* 인스턴스 하나가 덮는 사각형. 네 꼭짓점을 두 삼각형으로 — three.js의 `Mesh`는
 * `TRIANGLES`만 그리므로 스트립 대신 인덱스 여섯이다(같은 두 삼각형). 인덱스는 그릴
 * 꼭짓점 수이기도 하다: 이 기하에는 `position`이 없어서, 인덱스마저 없으면 r160의
 * `renderBufferDirect`가 그릴 끝을 무한으로 두고 말없이 건너뛴다 — 인덱스를 빼면
 * 오류 하나 없이 빈 그림이다. */
const KNOWLEDGE_GL_QUAD = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]);
const KNOWLEDGE_GL_QUAD_INDEX = [0, 1, 2, 2, 1, 3];

/* 셰이더에 `#version`이 없다 — `RawShaderMaterial`의 `glslVersion: GLSL3`이 그 줄을
 * 맨 앞에 붙인다(붙인 뒤에 `#define SHADER_NAME …` 두 줄이 따르고 그다음이 이 글자다).
 * 그 밖에는 원본 그대로다: 정밀도·in/out·`texelFetch`까지 손으로 쓰던 GLSL 300 es다. */

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

const KNOWLEDGE_GL_NODE_VERT = `precision highp float;
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
  return `precision highp float;
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

const KNOWLEDGE_GL_EDGE_VERT = `precision highp float;
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

const KNOWLEDGE_GL_EDGE_FRAG = `precision highp float;
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

const KNOWLEDGE_GL_DISC_VERT = `precision highp float;
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

const KNOWLEDGE_GL_DISC_FRAG = `precision highp float;
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

const KNOWLEDGE_GL_RING_VERT = `precision highp float;
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

const KNOWLEDGE_GL_RING_FRAG = `precision highp float;
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
    /* 렌더러가 실리지 않은 창(`ui/vendor/three.js`가 빠진 판)은 그릴 수 없다 — SVG가 선다. */
    if (typeof THREE !== "object" || typeof THREE.WebGLRenderer !== "function") {
      knowledgeGlAble = false;
      return knowledgeGlAble;
    }
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
  /* 주변 탐색의 페이지는 `is-local` 절의 옷을 입는다 — 군집의 색으로 칠한 점, 그리고 중심은 제 몸의 줄. */
  const local = layout.ring !== null && kind === "page" ? palette.local : null;
  if (local !== null && at === layout.ring.seat) {
    const centre = hue + 1;
    return { fill: local.centreFill, fillAt: centre * 4, stroke: local.centreStroke, strokeAt: centre * 4,
      strokePx: local.centreWidth[centre], dashed: false };
  }
  if (local !== null) {
    return { fill: local.nodeFill, fillAt: row * 4, stroke: local.nodeStroke, strokeAt: row * 4,
      strokePx: local.nodeWidth[row], dashed: false };
  }
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
 * 덧입힌다. 주변 탐색에서는 관계의 가족(종류, 유령), 판의 포커스(`focused` — 판이 `is-focused`인가)와
 * 그 선이 밝혀졌는가, 바퀴살인가가 줄을 고르고, 그 줄의 불투명도가 따로 온다(전체 지도의
 * 불투명도는 잉크에 이미 곱해져 있어 1이다). */
function knowledgeRestingEdgeInk(palette, tuning, layout, model, at, focused = false) {
  const head = model.from[at];
  const tail = model.to[at];
  const code = model.kind[at];
  const word = KNOWLEDGE_EDGE_KINDS[code];
  const typed = code !== KNOWLEDGE_EDGE_CODE.mentions && word !== "merge";
  if (layout.ring !== null) {
    const local = palette.local;
    const family = model.ghost[at] === 1 ? KNOWLEDGE_EDGE_KINDS.length : code;
    const spoke = head === layout.ring.seat || tail === layout.ring.seat;
    const focus = !focused ? 0 : layout.focusEdgeVisited[at] === 1 ? 2 : 4;
    const row = family * 6 + focus + (spoke ? 1 : 0);
    return { ink: local.edgeInk, inkAt: row * 4, width: local.edgeWidth[row], dash: local.edgeDash[row],
      opacity: local.edgeOpacity[row] };
  }
  if (model.ghost[at] === 1) {
    return { ink: palette.ghostEdge, inkAt: 0, width: tuning.edgeMentionsWidth, dash: tuning.edgeGhostDash, opacity: 1 };
  }
  if (typed || word === "merge") {
    return { ink: palette.typed, inkAt: code * 4, width: palette.widths[code],
      dash: palette.widths[KNOWLEDGE_EDGE_KINDS.length + code], opacity: 1 };
  }
  const intra = layout.community[head] === layout.community[tail];
  const hue = intra ? layout.communityHue[layout.community[head]] : -1;
  if (intra && hue >= 0) {
    return { ink: palette.intra, inkAt: hue * 4, width: tuning.edgeMentionsWidth, dash: 0, opacity: 1 };
  }
  return { ink: palette.inter, inkAt: 0, width: tuning.edgeMentionsWidth, dash: 0, opacity: 1 };
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
    /* 주변 탐색의 옷(`.knowledge-picture.is-local` 절) — 제 견본 판에서 읽는다(`knowledgeGlLocalPalette`). */
    local: null,
  };
  const theme = `${document.documentElement.dataset.theme ?? "dark"}`;
  if (palette.swatches !== null && palette.theme === theme) return palette;
  const hues = KNOWLEDGE_HUES;
  const tiers = KNOWLEDGE_TIER_WORD;
  if (palette.swatches === null) {
    const host = document.createElementNS(SVG_NS, "svg");
    host.setAttribute("class", "knowledge-picture knowledge-gl-swatch");
    host.setAttribute("aria-hidden", "true");
    const add = (className, hue, tier, words = {}, into = host) => {
      const group = document.createElementNS(SVG_NS, "g");
      group.setAttribute("class", className);
      if (hue >= 0) group.dataset.hue = String(hue);
      if (tier) group.dataset.tier = tier;
      /* 점이 든 낱말(`knowledgeSupplyDress`) 그대로 — 옷은 그 낱말을 읽는 CSS의 것이다. */
      for (const [name, value] of Object.entries(words)) group.dataset[name] = value;
      const dot = document.createElementNS(SVG_NS, "circle");
      dot.setAttribute("class", "knowledge-dot");
      group.appendChild(dot);
      into.appendChild(group);
      return dot;
    };
    const line = (className, hue, into = host) => {
      const group = document.createElementNS(SVG_NS, "g");
      if (hue >= 0) group.dataset.hue = String(hue);
      const path = document.createElementNS(SVG_NS, "path");
      path.setAttribute("class", className);
      group.appendChild(path);
      into.appendChild(group);
      return path;
    };
    /* 관계 종류의 클래스 — SVG 손(`paintKnowledgeFrame`의 선 옷)이 입히는 이름 그대로. */
    const edgeClass = (kind) => (kind === "mentions"
      ? "knowledge-edge"
      : `knowledge-edge ${kind === "merge" ? "kind-merge" : `is-typed kind-${kind}`}`);
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
      typed: KNOWLEDGE_EDGE_KINDS.map((kind) => line(edgeClass(kind), -1)),
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
    /* 주변 탐색의 옷도 **판**의 클래스(`is-local`)가 정한다 — 그래서 제 판을 따로 가진다. 점은
     * 전체 지도의 견본과 같은 색 칸 × 단이고, 중심(`is-selected`)은 색 칸마다 몸과 후광 하나다.
     * 선은 관계의 가족(종류 + 유령)마다 여섯 줄이다: 포커스 없는 판의 (맥락, 바퀴살), 포커스가 선
     * 판(`is-focused` — 중심이 골라져 있으니 주변 탐색의 보통)에서 밝혀진 (맥락, 바퀴살)과 물러선
     * (맥락, 바퀴살). 그 판의 규칙들이 특이도로 서로를 이기므로(밝혀진 바퀴살은 바퀴살의 잉크가
     * 아니라 주변의 잉크를 입는다) 줄마다 계산값을 읽는다. 포커스 판의 물러선 점 하나가 그
     * 불투명도를 답한다. */
    const localHost = document.createElementNS(SVG_NS, "svg");
    localHost.setAttribute("class", "knowledge-picture is-local knowledge-gl-swatch");
    localHost.setAttribute("aria-hidden", "true");
    const localDots = [];
    for (let hue = 0; hue < hues; hue += 1) {
      for (const tier of tiers) localDots.push(add("knowledge-node is-page", hue, tier, {}, localHost));
    }
    for (const tier of tiers) localDots.push(add("knowledge-node is-page", -1, tier, {}, localHost));
    const centres = [];
    for (let hue = -1; hue < hues; hue += 1) {
      const dot = add("knowledge-node is-page is-selected", hue, "leaf", {}, localHost);
      const halo = document.createElementNS(SVG_NS, "circle");
      halo.setAttribute("class", "knowledge-halo");
      dot.parentNode.appendChild(halo);
      centres.push({ dot, halo });
    }
    const families = [...KNOWLEDGE_EDGE_KINDS.map((kind) => edgeClass(kind)), "knowledge-edge is-ghost"];
    const focusHost = document.createElementNS(SVG_NS, "svg");
    focusHost.setAttribute("class", "knowledge-picture is-local is-focused knowledge-gl-swatch");
    focusHost.setAttribute("aria-hidden", "true");
    const localEdges = [];
    for (const family of families) {
      localEdges.push(line(family, -1, localHost), line(`${family} is-spoke`, -1, localHost),
        line(`${family} is-focus-lit`, -1, focusHost), line(`${family} is-focus-lit is-spoke`, -1, focusHost),
        line(family, -1, focusHost), line(`${family} is-spoke`, -1, focusHost));
    }
    const localNebulaGroup = document.createElementNS(SVG_NS, "g");
    localNebulaGroup.dataset.hue = "0";
    const localNebula = document.createElementNS(SVG_NS, "ellipse");
    localNebula.setAttribute("class", "knowledge-nebula");
    localNebulaGroup.appendChild(localNebula);
    localHost.appendChild(localNebulaGroup);
    const focusDot = add("knowledge-node is-page", 0, "leaf", {}, focusHost);
    palette.swatches.local = { host: localHost, focusHost, dots: localDots, centres, edges: localEdges,
      nebula: localNebula, focusNode: focusDot.parentNode };
    view.append(host, tracing, localHost, focusHost);
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
  palette.local = knowledgeGlLocalPalette(swatches.local, palette.local);
  palette.theme = theme;
  return palette;
}

/* 주변 탐색의 팔레트 — 견본이 입은 `is-local` 절의 계산값. 불투명도는 잉크와 따로 든다: 포커스가
 * 선 판에서는 CSS가 선의 불투명도를 **바꿔 끼우고**(곱하지 않는다), 잉크는 그대로다. 중심의 몸은
 * `transform: scale(…)`로 커지고 테두리·후광의 굵기는 그 배율로 나눠 적혀 있다 — 화면 픽셀로
 * 되돌려 둔다(GL은 자리의 반지름을 키워 그린다). */
function knowledgeGlLocalPalette(swatches, held) {
  const rows = swatches.dots.length;
  const centres = swatches.centres.length;
  const edges = swatches.edges.length;
  const local = held ?? {
    nodeFill: new Float32Array(rows * 4),
    nodeStroke: new Float32Array(rows * 4),
    nodeWidth: new Float32Array(rows),
    centreFill: new Float32Array(centres * 4),
    centreStroke: new Float32Array(centres * 4),
    centreWidth: new Float32Array(centres),
    halo: new Float32Array(centres * 4),
    haloWidth: new Float32Array(centres),
    centreScale: 1,
    edgeInk: new Float32Array(edges * 4),
    edgeOpacity: new Float32Array(edges),
    edgeWidth: new Float32Array(edges),
    edgeDash: new Float32Array(edges),
    discs: true,
    farNode: 1,
  };
  const read = (node) => getComputedStyle(node);
  swatches.dots.forEach((dot, at) => {
    const style = read(dot);
    knowledgeGlColor(style.fill, local.nodeFill, at * 4);
    knowledgeGlColor(style.stroke, local.nodeStroke, at * 4);
    local.nodeWidth[at] = Number.parseFloat(style.strokeWidth) || 0;
  });
  swatches.centres.forEach(({ dot, halo }, at) => {
    const style = read(dot);
    const scale = style.transform === "none" ? 1 : new DOMMatrixReadOnly(style.transform).a;
    local.centreScale = scale;
    knowledgeGlColor(style.fill, local.centreFill, at * 4);
    knowledgeGlColor(style.stroke, local.centreStroke, at * 4);
    local.centreWidth[at] = (Number.parseFloat(style.strokeWidth) || 0) * scale;
    const ring = read(halo);
    const ringScale = ring.transform === "none" ? 1 : new DOMMatrixReadOnly(ring.transform).a;
    knowledgeGlColor(ring.display === "none" ? "none" : ring.stroke, local.halo, at * 4);
    local.halo[at * 4 + 3] *= Number.parseFloat(ring.opacity) || 0;
    local.haloWidth[at] = (Number.parseFloat(ring.strokeWidth) || 0) * ringScale;
  });
  swatches.edges.forEach((path, at) => {
    const style = read(path);
    knowledgeGlColor(style.stroke, local.edgeInk, at * 4);
    local.edgeOpacity[at] = Number.parseFloat(style.opacity);
    local.edgeWidth[at] = Number.parseFloat(style.strokeWidth) || 1;
    local.edgeDash[at] = Number.parseFloat(style.strokeDasharray) || 0;
  });
  local.discs = read(swatches.nebula).display !== "none";
  local.farNode = Number.parseFloat(read(swatches.focusNode).opacity);
  return local;
}

/* 재질 하나 — 손으로 쓰던 셰이더의 글자 그대로를 `RawShaderMaterial`에 싣는다. three.js는
 * 제 `#version`과 `#define` 두 줄만 앞에 붙이고 나머지(정밀도·속성·유니폼·`texelFetch`)는
 * 이 파일의 것이다. 섞기는 손으로 쓰던 것과 같은 식이다: `NormalBlending`을 곱하지 않은
 * 알파로(`premultipliedAlpha: false`) 쓰면 three.js가 부르는 것이
 * `blendFuncSeparate(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA)`다 — 버퍼는
 * 알파를 곱한 색이고(`mount`의 문맥 속성) 셰이더는 곧은 색을 낸다. 깊이는 끈다(2D이고
 * 문맥에 깊이 버퍼도 없다). 양면을 그린다(컬링 없음 — 사각형의 감김을 묻지 않는다).
 * 유니폼은 네 재질이 **한 기록**을 나눈다: 프레임마다 값을 한 번만 쓰고, 셰이더가 쓰지 않는
 * 유니폼은 three.js가 프로그램의 활성 유니폼만 올리므로 그냥 지나간다. */
function knowledgeGlMaterial(name, vertexShader, fragmentShader, uniforms) {
  return new THREE.RawShaderMaterial({
    name: `knowledge-${name}`,
    glslVersion: THREE.GLSL3,
    vertexShader,
    fragmentShader,
    uniforms,
    transparent: true,
    blending: THREE.NormalBlending,
    premultipliedAlpha: false,
    depthTest: false,
    depthWrite: false,
    side: THREE.DoubleSide,
  });
}

/* 한 패스의 기하 — 사각형 하나와 인스턴스 속성들. 배열은 페인터의 통이고(`reserve`가
 * 키운다) 속성은 그 통을 **그대로** 든다: 프레임마다 채우는 것은 통이고, 올리는 것은
 * `addUpdateRange`가 가리킨 앞부분뿐이다(`DYNAMIC_DRAW`의 `bufferSubData`). 정수 속성
 * (`Uint32Array`의 번호)은 three.js가 배열의 형에서 `vertexAttribIPointer`를 고르고,
 * 바이트 속성은 `normalized`로 0..1이 된다 — 손으로 쓰던 판의 포인터와 같다. 통이 자라면
 * 기하를 새로 짓는다(`rebuild`): three.js는 속성을 갈아 끼운 자리의 옛 버퍼를 놓지 않지만,
 * 기하를 놓으면(`dispose`) 그 기하의 버퍼와 VAO를 전부 놓는다. */
function knowledgeGlGeometry(rows) {
  const geometry = new THREE.InstancedBufferGeometry();
  geometry.setAttribute("aCorner", new THREE.BufferAttribute(KNOWLEDGE_GL_QUAD, 2));
  geometry.setIndex(KNOWLEDGE_GL_QUAD_INDEX);
  geometry.instanceCount = 0;
  for (const [name, array, size, normalized] of rows) {
    const attribute = new THREE.InstancedBufferAttribute(array, size, normalized === true);
    attribute.setUsage(THREE.DynamicDrawUsage);
    geometry.setAttribute(name, attribute);
  }
  return geometry;
}

/* 위치 텍스처 — RGBA32F 한 변 `wide`, NEAREST, 밉맵 없음(설계 §9). three.js는 처음 한 번
 * `texStorage2D`로 자리를 잡고 그 뒤 `needsUpdate`마다 `texSubImage2D`로 덮어쓴다 —
 * 손으로 쓰던 판의 두 호출 그대로다. 자료는 통(`seatData`)의 앞부분을 보는 창이라 통을
 * 채우면 텍스처가 그것을 올린다. */
function knowledgeGlSeatTexture(data, wide) {
  const texture = new THREE.DataTexture(data.subarray(0, wide * wide * 4), wide, wide,
    THREE.RGBAFormat, THREE.FloatType);
  texture.magFilter = THREE.NearestFilter;
  texture.minFilter = THREE.NearestFilter;
  texture.wrapS = THREE.ClampToEdgeWrapping;
  texture.wrapT = THREE.ClampToEdgeWrapping;
  texture.generateMipmaps = false;
  texture.flipY = false;
  texture.colorSpace = THREE.NoColorSpace;
  /* 새 텍스처는 첫 쓰임에 올라간다 — `DataTexture`는 스스로 낡았다고 말하지 않는다. */
  texture.needsUpdate = true;
  return texture;
}

/* ---- GL 페인터 -------------------------------------------------------------
 *
 * `KnowledgePainter`의 다섯 손(`mount`·`paintTopology`·`paintFrame`·`pick`·
 * `dispose`)을 그대로 쓴다. 같은 `layout`을 읽고, 같은 카메라를 받고, 같은
 * 이름표 격자를 부른다 — 다른 것은 칠하는 손뿐이다. */
function makeKnowledgeGlPainter() {
  return {
    id: "gl",
    /* 이름표는 캔버스 위의 HTML 층이라 어느 점보다도 위에 선다 — 그래서 격자는 확대한
     * 판에서 이 손의 이름이 남의 점을 덮게 둔다(`placeKnowledgeLabels`). SVG 손은 이름이
     * 제 점의 <g> 안에 있어 뒤에 그려진 점이 글자를 가리므로 이 약속을 하지 않는다. */
    labelsOverPoints: true,
    view: null,
    canvas: null,
    gl: null,
    /* three.js의 살림 — 렌더러 하나, 장면 하나(메시 넷), 서명을 채우는 카메라 하나, 그리고
     * 네 재질이 나누는 유니폼 기록. */
    renderer: null,
    scene: null,
    camera: null,
    uniforms: null,
    palette: null,
    /* 패스 넷(성운·선·점·고리) — 재질·메시·기하와 인스턴스 속성의 이름. */
    passes: null,
    seatTexture: null,
    seatWide: 0,
    /* 텍스처가 잡힌 한 변 — 그림이 자랄 때만 다시 잡는다(`reserve`). */
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
    onTheme: null,

    mount(view) {
      this.view = view;
      const host = view.querySelector(".knowledge-canvas");
      const canvas = document.createElement("canvas");
      canvas.className = "knowledge-gl";
      canvas.setAttribute("aria-hidden", "true");
      /* 그림(<svg>) 바로 앞에 선다 — 이름판과 낱말은 캔버스 위에, 묶음선의 밑층(t-12029)은 캔버스 아래에. */
      host.insertBefore(canvas, host.querySelector(".knowledge-picture"));
      const labels = document.createElement("div");
      labels.className = "knowledge-gl-labels";
      labels.setAttribute("aria-hidden", "true");
      host.appendChild(labels);
      this.canvas = canvas;
      this.labels = labels;
      this.labelPool = [];
      view.querySelector(".knowledge-picture")?.classList.add("is-gl");
      /* 문맥은 **이 손이** 연다. three.js에게 캔버스만 주면 webgl2가 거절된 판에서 webgl1로
       * 물러나 서고, 그 판에서 300 es 셰이더는 서지 못한 채 빈 그림을 그린다 — 여기서 열어
       * 없으면 없다고 말하고(2D로 돌아간다), 있으면 그것을 렌더러에 건넨다. 속성도 여기의
       * 것이다. */
      const gl = typeof THREE !== "object" ? null : canvas.getContext("webgl2", {
        alpha: true,
        antialias: false,
        depth: false,
        stencil: false,
        /* 버퍼는 알파를 곱한 색이다 — 재질의 섞기(`SRC_ALPHA`, `ONE_MINUS_SRC_ALPHA`)가 셰이더의
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
      /* 문맥을 잃으면(잠에서 깬 판·드라이버 재시작) 렌즈를 접고 한 줄 알린다. three.js도
       * 같은 사건을 듣고 제 문맥을 잃은 것으로 적지만, 돌아가는 길은 이 손의 것이다. */
      this.onLost = (event) => {
        event.preventDefault();
        this.lost = true;
        knowledgeGlFellBack(view);
      };
      canvas.addEventListener("webglcontextlost", this.onLost);
      try {
        /* 문맥을 건네므로 three.js는 문맥을 만들지 않는다 — 문맥의 속성은 위의 것이 전부이고,
         * 렌더러가 여기서 읽는 것은 `premultipliedAlpha` 하나다(지우는 색에 알파를 곱할지). 위의
         * 버퍼와 같은 값이어야 한다. */
        const renderer = new THREE.WebGLRenderer({
          canvas,
          context: gl,
          premultipliedAlpha: true,
        });
        this.renderer = renderer;
        /* 크기는 기기 픽셀로 직접 준다(`setSize`, dpr 1) — 캔버스의 CSS 크기는 스타일시트의
         * 것이고(`updateStyle: false`), 셰이더의 자(`uPixelRatio`)는 dpr을 따로 받는다. 정렬은
         * 끈다: 그리는 순서는 장면에 넣은 순서(성운·선·점·고리)다. 색 공간·톤 매핑은 셰이더가
         * 낸 색을 그대로 쓰는 값이다(raw 셰이더라 어차피 지나지 않지만, 기본값에 기대지 않는다). */
        renderer.setPixelRatio(1);
        renderer.autoClear = true;
        renderer.sortObjects = false;
        renderer.setClearColor(0x000000, 0);
        renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
        renderer.toneMapping = THREE.NoToneMapping;
        /* 셰이더가 서지 않으면 three.js는 콘솔에 적고 빈 그림을 그린다 — 이 손은 그 대신
         * 던져서 SVG로 돌아간다(아래 `catch`, 첫 그림이 여기 `render`다). */
        renderer.debug.onShaderError = (context, program, vertexShader, fragmentShader) => {
          throw new Error([context.getProgramInfoLog(program),
            context.getShaderInfoLog(vertexShader), context.getShaderInfoLog(fragmentShader)]
            .filter((word) => word).join(" / "));
        };
        this.uniforms = {
          uSeats: { value: null },
          uCamera: { value: new THREE.Vector3() },
          uViewport: { value: new THREE.Vector2() },
          uPixelRatio: { value: 1 },
          uSeatWide: { value: 1 },
          uFeather: { value: 1 },
        };
        this.scene = new THREE.Scene();
        this.camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
        const uniforms = this.uniforms;
        const pass = (name, vertexShader, fragmentShader) => ({
          material: knowledgeGlMaterial(name, vertexShader, fragmentShader, uniforms),
          mesh: null,
          geometry: null,
          names: [],
        });
        this.passes = {
          disc: pass("disc", KNOWLEDGE_GL_DISC_VERT, KNOWLEDGE_GL_DISC_FRAG),
          edge: pass("edge", KNOWLEDGE_GL_EDGE_VERT, KNOWLEDGE_GL_EDGE_FRAG),
          node: pass("node", KNOWLEDGE_GL_NODE_VERT, knowledgeGlNodeFragment()),
          ring: pass("ring", KNOWLEDGE_GL_RING_VERT, KNOWLEDGE_GL_RING_FRAG),
        };
        /* 통을 하나씩 잡아 메시 넷을 세우고 빈 그림을 한 장 그린다 — 인스턴스 0의 드로우는
         * 없지만 프로그램은 여기서 서고(three.js는 첫 쓰임에서 링크를 검사한다), 서지 않는
         * 셰이더는 여기서 던진다. 손으로 쓰던 판도 셰이더를 세우는 값을 첫 그림 전에 치렀다.
         * 여기서 잡은 한 칸짜리 통은 첫 `paintTopology`가 제 크기로 갈아 끼운다 — 링크 검사를
         * 앞당기려고 일부러 치르는 값이다. */
        this.reserveCounts(1, 1, 1);
        for (const one of Object.values(this.passes)) one.mesh.visible = true;
        renderer.render(this.scene, this.camera);
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
      this.palette = knowledgeGlPalette(view, null);
      /* 캔버스는 테마를 모른다 — 창의 테마 단추(`setTheme`)나 시스템 설정이 뿌리의 `data-theme`를
       * 바꾸면 옷만 다시 입은 프레임 하나를 그린다(자리는 그대로). */
      this.onTheme = new MutationObserver(() => {
        const layout = knowledgeLayouts.get(view);
        if (layout !== undefined) this.paintDress(layout);
      });
      this.onTheme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
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
      if (this.lost || this.renderer === null || this.uploaded === -1) return;
      this.dressStale = true;
      paintKnowledgeFrame(this.view, layout, { cameraOnly: true });
    },

    /* 통을 이 그림의 크기에 맞춘다. 자라기만 한다 — 렌즈가 절반을 숨긴 판이
     * 통을 줄였다가 돌아오면 그 자리에서 다시 짓게 된다. */
    reserve(layout) {
      this.reserveCounts(layout.count, layout.model.edgeCount, layout.namedCount);
    },

    /* 통과 그 통을 든 기하·텍스처. 통이 새로 잡힌 패스만 기하를 다시 짓고(옛 기하는
     * 놓는다), 위치 텍스처는 한 변이 바뀐 프레임에만 다시 잡는다. 처음 부르는 순서
     * (성운·선·점·고리)가 장면의 순서이고 그것이 그리는 순서다. */
    reserveCounts(count, edgeCount, named) {
      /* 떠난 손(문맥을 잃고 놓은 뒤)에는 통도 기하도 없다 — 늦게 온 위상은 그냥 지나간다. */
      if (this.renderer === null) return;
      const wide = Math.max(1, Math.ceil(Math.sqrt(Math.max(1, count))));
      if (this.seatData === null || this.seatData.length < wide * wide * 4) {
        this.seatData = new Float32Array(wide * wide * 4);
        this.uploaded = -1;
        /* 새 통은 새 창으로 봐야 한다 — 한 변이 같아도 텍스처를 다시 잡는다. */
        this.seatStorage = 0;
      }
      this.seatWide = wide;
      if (this.seatStorage !== wide) {
        this.seatTexture?.dispose();
        this.seatTexture = knowledgeGlSeatTexture(this.seatData, wide);
        this.seatStorage = wide;
        this.uniforms.uSeats.value = this.seatTexture;
      }
      if (this.discData === null || this.discData.length < Math.max(1, named) * 4) {
        this.discData = new Float32Array(Math.max(1, named) * 4);
        this.discInk = new Float32Array(Math.max(1, named) * 4);
        this.discRim = new Float32Array(Math.max(1, named) * 2);
        this.rebuild(this.passes.disc, [
          ["aDisc", this.discData, 4],
          ["aInk", this.discInk, 4],
          ["aRim", this.discRim, 2],
        ]);
      }
      if (this.edgeEnds === null || this.edgeEnds.length < edgeCount * 2) {
        this.edgeEnds = new Uint32Array(Math.max(1, edgeCount) * 2);
        this.edgeInk = new Uint8Array(Math.max(1, edgeCount) * 4);
        this.edgeShape = new Uint8Array(Math.max(1, edgeCount) * 4);
        this.rebuild(this.passes.edge, [
          ["aEnds", this.edgeEnds, 2],
          ["aInk", this.edgeInk, 4, true],
          ["aShape", this.edgeShape, 4, true],
        ]);
      }
      if (this.nodeSeat === null || this.nodeSeat.length < count) {
        const room = Math.max(1, count);
        this.nodeSeat = new Uint32Array(room);
        this.nodeFill = new Uint8Array(room * 4);
        this.nodeStroke = new Uint8Array(room * 4);
        this.nodeShape = new Uint8Array(room * 4);
        /* 점 하나에 고리가 둘일 수 있다 — 회상의 고리와 접힘의 고리. */
        this.ringSeat = new Uint32Array(room * 2);
        this.ringInk = new Uint8Array(room * 8);
        this.ringShape = new Uint8Array(room * 8);
        this.rebuild(this.passes.node, [
          ["aSeat", this.nodeSeat, 1],
          ["aFill", this.nodeFill, 4, true],
          ["aStroke", this.nodeStroke, 4, true],
          ["aShape", this.nodeShape, 4, true],
        ]);
        this.rebuild(this.passes.ring, [
          ["aSeat", this.ringSeat, 1],
          ["aInk", this.ringInk, 4, true],
          ["aShape", this.ringShape, 4, true],
        ]);
      }
    },

    /* 패스의 기하를 (다시) 짓는다. 메시는 처음 한 번 장면에 서고 그 뒤로는 기하만 갈아
     * 끼운다 — 장면의 순서가 그리는 순서라 메시를 빼고 다시 넣지 않는다. */
    rebuild(pass, rows) {
      pass.geometry?.dispose();
      pass.geometry = knowledgeGlGeometry(rows);
      pass.names = rows.map((row) => row[0]);
      if (pass.mesh === null) {
        const mesh = new THREE.Mesh(pass.geometry, pass.material);
        /* 컬링도 행렬도 없다 — 자리는 셰이더가 텍스처에서 읽고, 화면은 언제나 전부다. */
        mesh.frustumCulled = false;
        mesh.matrixAutoUpdate = false;
        mesh.visible = false;
        pass.mesh = mesh;
        this.scene.add(mesh);
      } else {
        pass.mesh.geometry = pass.geometry;
      }
    },

    paintFrame(layout, camera, { cameraOnly = false } = {}) {
      const view = this.view;
      if (this.lost || this.renderer === null) return;
      const picture = view.querySelector(".knowledge-picture");
      const inverse = camera.inverse;
      /* 이름판과 관계의 낱말은 여전히 이 <svg> 안에 산다(군집 수·이웃 수만큼이라
       * DOM이 값을 치르지 않는다). 그래서 카메라는 두 손이 함께 쓴다. */
      writeAttribute(picture, "viewBox", `${camera.x} ${camera.y} ${camera.wide} ${camera.tall}`);
      /* 테마가 바뀌면 견본이 새 색을 답한다 — 그 프레임은 카메라만 움직였어도 옷을 다시 올린다
       * (옷은 바이트로 올린 색이라 스타일시트를 따라오지 않는다). */
      const inked = this.palette?.theme;
      this.palette = knowledgeGlPalette(view, this.palette);
      if (this.palette.theme !== inked) this.dressStale = true;
      const tuning = layout.tuning;
      const feather = Number.isFinite(tuning.glFeather) ? tuning.glFeather : 1;
      const wide = view.querySelector(".knowledge-canvas").clientWidth;
      const tall = view.querySelector(".knowledge-canvas").clientHeight;
      const cap = Number.isFinite(tuning.glPixelCap) ? tuning.glPixelCap : 2;
      const dpr = Math.max(1, Math.min(window.devicePixelRatio || 1, cap));
      const pixelWide = Math.max(1, Math.round(wide * dpr));
      const pixelTall = Math.max(1, Math.round(tall * dpr));
      /* 렌더러가 캔버스의 기기 픽셀과 뷰포트를 함께 맞춘다(dpr 1로 세웠으므로 준 수 그대로).
       * 지우기는 렌더러의 몫이다(`autoClear`, 투명한 검정). */
      if (this.canvas.width !== pixelWide || this.canvas.height !== pixelTall) {
        this.renderer.setSize(pixelWide, pixelTall, false);
      }
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
      /* 주변 탐색의 중심은 SVG에서 `transform: scale(…)`로 커진다 — 여기서는 자리의 반지름이다
       * (선은 그 몸의 가장자리에서 끝나고, 후광은 그 몸에서 떨어져 선다). */
      const centre = layout.ring === null ? -1 : layout.ring.seat;
      for (let at = 0; at < count; at += 1) {
        const seat = at * 4;
        data[seat] = x[at];
        data[seat + 1] = drawY[at];
        data[seat + 2] = 0;
        /* 그려지지 않는 점은 반지름 0으로 둔다 — 선이 그 점에서 잘리지 않게. */
        data[seat + 3] = drawn !== null && drawn[at] === 0 ? 0
          : at === centre ? radius[at] * this.palette.local.centreScale : radius[at];
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
      /* 밝힌 군집 — 고른 것 또는 이름판에 올라 선 것(t-12029). */
      const spot = knowledgeSpotRank();
      const selectedSeat = knowledgeSelectedKey === null ? -1
        : model.keys.indexOf(knowledgeSelectedKey);
      /* 주변 탐색: 중심은 제 줄의 옷(고름의 테두리를 덧입지 않는다)과 후광, 포커스가 물린 점은
       * 그 절의 불투명도로 물러선다. */
      const local = layout.ring === null ? null : palette.local;
      const centre = local === null ? -1 : layout.ring.seat;
      const focusFar = local === null ? dim : local.farNode;
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
        if (focused && layout.focusVisited[at] !== 1) alpha = Math.min(alpha, focusFar);
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
        const stated = !ghost && !source && ((selected && at !== centre) || lit || pathLit);
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
        /* 중심의 후광 — SVG의 halo 원(반지름 × `haloScale`, 중심과 같은 배율)과 같은 자리·굵기. */
        if (at === centre && kind === "page") {
          const hue = layout.communityHue[rank] + 1;
          this.ringSeat[rings] = at;
          this.writeInk(this.ringInk, rings * 4, local.halo, hue * 4, alpha);
          this.ringShape[rings * 4] = Math.min(255,
            Math.round(layout.radius[at] * local.centreScale * (tuning.haloScale - 1) * 4));
          this.ringShape[rings * 4 + 1] = Math.min(255, Math.round(local.haloWidth[hue] * 8));
          this.ringShape[rings * 4 + 2] = 0;
          this.ringShape[rings * 4 + 3] = 0;
          rings += 1;
        }
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
      const spot = knowledgeSpotRank();
      const searchOn = knowledgeQuery.trim() !== "";
      const { from, to, edgeCount } = model;
      const drawnEdge = layout.drawnEdge;
      /* 주변 탐색에서 포커스는 쉬는 옷의 줄이 말한다(밝혀진 선도 물러선 선도 그 절의 옷) — 밝힘의
       * 옷은 짚은 선만 입는다. */
      const local = layout.ring !== null;
      let edges = 0;
      for (let at = 0; at < edgeCount; at += 1) {
        if (drawnEdge !== null && drawnEdge[at] === 0) continue;
        /* 묶음 안의 선은 긋지 않는다(t-12029) — 쉬는 전체 지도의 군집 사이 선은 밑층의 줄이 말한다. */
        if (knowledgeEdgeBundled(layout, at)) continue;
        const head = from[at];
        const tail = to[at];
        const lit = layout.lit.has(at);
        const focusLit = layout.focusEdgeVisited[at] === 1;
        const match = layout.searchMatch[head] === 1 && layout.searchMatch[tail] === 1;
        /* SVG의 옷과 같은 규칙 — 밝힌 군집에 닿는 선이 밝다(t-12029). */
        const spotlit = spot >= 0 && (layout.community[head] === spot || layout.community[tail] === spot);
        const pathLit = layout.pathEdge[at] === 1;
        /* 쉬는 옷은 한 함수(`knowledgeRestingEdgeInk`)가 고른다 — 내보내기가 같은 손으로 읽는다. */
        const resting = knowledgeRestingEdgeInk(palette, tuning, layout, model, at, focused);
        let alpha = lit || (pathed && pathLit) ? 1 : resting.opacity;
        if (tracing && !lit) alpha = Math.min(alpha, far);
        if (!local && focused && !focusLit) alpha = Math.min(alpha, dim);
        if (searching && searchOn && !match) alpha = Math.min(alpha, dim);
        if (sliced && (layout.sliceMatch[head] === 0 || layout.sliceMatch[tail] === 0)) {
          alpha = Math.min(alpha, dim);
        }
        if (spotlight && !spotlit) alpha = Math.min(alpha, dim);
        if (pathed && !pathLit) alpha = Math.min(alpha, dim);
        let ink = resting.ink;
        let inkAt = resting.inkAt;
        let width = resting.width;
        const dash = resting.dash;
        if (lit || (!local && focused && focusLit)) {
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
      /* 주변 탐색에는 전체 지도의 성운이 없다 — 견본의 `display`가 그렇게 말하면 한 장도 없다. */
      if (layout.ring !== null && !palette.local.discs) {
        this.counts.discs = 0;
        return;
      }
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

    /* 드로우 넷 — 성운, 선, 점, 고리. 인스턴스가 없는 패스는 호출도 없다(메시를 숨긴다 —
     * 드로우 수는 렌더러가 센 것을 그대로 적는다, `info.render.calls`).
     *
     * 카메라만 움직인 프레임은 **아무것도 올리지 않는다**(설계 §6의 표): 위치도
     * 옷도 그대로이고 바뀐 것은 유니폼 몇뿐이다. 이 한 줄이 만 쪽의 궤도 프레임에서
     * 프레임마다의 163 KB 텍스처 재할당과 570 KB의 버퍼 쓰기를 없앤다. 올리는 프레임도
     * 통 전부가 아니라 채운 앞부분만이다(`addUpdateRange`). */
    draw(layout, camera, pixelWide, pixelTall, feather, dpr, seats, dress) {
      const counts = this.counts;
      const uniforms = this.uniforms;
      /* 카메라는 판 픽셀의 것이고 캔버스는 기기 픽셀이라 배율에 dpr이 든다. */
      uniforms.uCamera.value.set(camera.x, camera.y, camera.scale * dpr);
      uniforms.uViewport.value.set(pixelWide, pixelTall);
      uniforms.uFeather.value = feather * dpr;
      uniforms.uPixelRatio.value = dpr;
      uniforms.uSeatWide.value = this.seatWide;
      /* 자리는 한 번 잡고(`texStorage2D`, `reserve`) 그 뒤로는 덮어쓴다 — `needsUpdate`가
       * `texSubImage2D` 한 번이다. */
      if (seats) this.seatTexture.needsUpdate = true;
      const show = (pass, count, upload) => {
        pass.mesh.visible = count > 0;
        if (count === 0) return;
        pass.geometry.instanceCount = count;
        if (!upload) return;
        for (const name of pass.names) {
          const attribute = pass.geometry.attributes[name];
          /* 지난 범위를 먼저 지운다 — 기하를 새로 지은 프레임에는 three.js가 배열 전체를 처음
           * 올리며 범위를 쓰지 않으므로, 지우지 않으면 그 범위가 다음 옷 프레임에 한 번 더 오른다. */
          attribute.clearUpdateRanges();
          attribute.addUpdateRange(0, count * attribute.itemSize);
          attribute.needsUpdate = true;
        }
      };
      /* 성운만은 프레임마다 올린다 — 군집 수만큼의 짧은 배열이고, 그 자리는
       * 카메라가 움직이면 이름판과 함께 다시 정해진다. 점·선·고리는 위상과 옷이
       * 바뀐 프레임에만 올라간다. */
      show(this.passes.disc, counts.discs, true);
      show(this.passes.edge, counts.edges, dress);
      show(this.passes.node, counts.nodes, dress);
      show(this.passes.ring, counts.rings, dress);
      this.renderer.render(this.scene, this.camera);
      counts.draws = this.renderer.info.render.calls;
    },

    /* 이름표는 HTML이다 — 예산(넓은 판의 배율 1에서 46, 배율에 비례)만큼의 <span>이고
     * 점의 수와 무관하다. 자리는 격자가 **예약한 상자의 한가운데**다: 격자가 그 자리를
     * 지켰으므로 겹치지 않는다는 약속이 그림에서도 참이 된다. */
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
        /* 주변 탐색의 중심 이름 — SVG의 `is-selected .knowledge-label`과 같은 옷(더 크고, 몸 아래로). */
        const centre = layout.ring !== null && at === layout.ring.seat;
        if (word.classList.contains("is-centre") !== centre) word.classList.toggle("is-centre", centre);
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

    /* 놓는다 — 기하(버퍼·VAO)·재질(프로그램)·텍스처·렌더러·문맥·리스너·오버레이 전부.
     * 하나라도 남으면 페인터를 갈아 끼울 때마다 GPU와 힙이 자란다(하네스가 열 번 갈아
     * 끼우고 그 자리를 재고, 렌더러의 장부 `info.memory`가 0으로 돌아오는지도 묻는다).
     * 순서가 약속이다: 기하·재질·텍스처의 `dispose()`가 렌더러의 `dispose()`보다 먼저다.
     * 렌더러는 놓일 때 제 장부(`properties`)를 새로 갈아 끼우므로, 그 뒤에 오는 텍스처·재질의
     * 놓임은 GL 객체를 찾지 못한다 — `deleteTexture`도 프로그램 해제도 없이 `info.memory`와
     * `info.programs`에 남는다. */
    dispose() {
      const view = this.view;
      if (this.passes !== null) {
        for (const pass of Object.values(this.passes)) {
          pass.geometry?.dispose();
          pass.material.dispose();
        }
      }
      this.seatTexture?.dispose();
      this.seatStorage = 0;
      if (this.canvas !== null && this.onLost !== null) {
        this.canvas.removeEventListener("webglcontextlost", this.onLost);
      }
      this.onTheme?.disconnect();
      this.onTheme = null;
      this.renderer?.dispose();
      /* 문맥 자체도 놓는다 — 판마다의 GL 문맥 수는 브라우저가 열여섯 언저리로
       * 묶어 두고, 넘으면 가장 오래된 것을 말없이 잃는다. 렌더러가 서지 못한 판에도
       * 문맥은 열려 있으므로 렌더러가 아니라 문맥에게 말한다. */
      this.gl?.getExtension("WEBGL_lose_context")?.loseContext();
      this.canvas?.remove();
      this.labels?.remove();
      this.palette?.swatches?.host.remove();
      this.palette?.swatches?.tracing?.remove();
      this.palette?.swatches?.local?.host.remove();
      this.palette?.swatches?.local?.focusHost.remove();
      view?.querySelector(".knowledge-picture")?.classList.remove("is-gl");
      this.gl = null;
      this.renderer = null;
      this.scene = null;
      this.camera = null;
      this.uniforms = null;
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
