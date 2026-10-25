/* ---- 디자인 시스템 (설정의 숨은 페이지, t-67484) -------------------------------
 *
 * 디자인 게이트가 읽는 두 파일 — ui/tokens.css와 ui/shell.css — 을 그대로 읽어 그린다.
 * 값은 한 줄도 다시 적지 않는다: 색 띠는 `var(--토큰)`으로 칠하고, 컴포넌트는 창이 쓰는 클래스
 * 그대로이며, 호버·활성·초점은 같은 규칙을 클래스로 붙들어 보인다(`designForceStateRules`).
 *
 * 처음에는 공통 족(색 단계·역할 색·간격·글자·반지름·모션·그림자)과 컴포넌트만 그리고, 화면·
 * 컴포넌트 전용 토큰의 긴 꼬리는 펼칠 때 그린다. 「이 칸에 눈에 보이는 규칙이 있는가」를 재는 일은
 * 이 페이지가 아니라 게이트가 한다(`designStateCoverage`): 그림 시간에 얹지 않는다.
 *
 * 들어가는 길: 설정 「일반」 아래 버전 줄을 5번 누르면 목록에 나타난다. 평소에는 목록이 바뀌지
 * 않는다. 두 테마는 페이지 안 스위치로 한 번에 하나 — 테마 토큰이 `:root`에만 있어서 나란히
 * 그리면 토큰 구조를 바꿔야 한다. 페이지를 떠나면 사람이 고른 테마로 돌아간다.
 */
const DESIGN_UNLOCK_PRESSES = 5;
/* 눌림 사이가 이보다 길면 센 것을 버린다. */
const DESIGN_UNLOCK_WINDOW_MS = 2000;

let designUnlocked = false;
let designPresses = 0;
let designLastPressMs = 0;
let designTokenNames = null;
let designStateRulesForced = 0;
let designPageTheme = null;
const designCoverage = new Map();

/* 공통 족. 이름 모양으로 가르고, 어디에도 들지 않는 것은 긴 꼬리다. 값이 아니라 이름의 규칙이다. */
const DESIGN_FAMILIES = Object.freeze([
  { id: "ramp", title: () => t("design.family.ramp", "색 단계"), note: () => t("design.family.rampNote", "열 단계의 색 띠"), sample: "color",
    test: /^--(gray|blue|red|amber|green|pink|purple|teal|cyan|orange)(-alpha)?-\d+$/ },
  { id: "role", title: () => t("design.family.role", "역할 색"), note: () => t("design.family.roleNote", "화면은 단계가 아니라 역할을 읽습니다"), sample: "color",
    test: /^--(surface|ink|edge|signal|row|background)-/ },
  { id: "space", title: () => t("design.family.space", "간격"), note: () => t("design.family.spaceNote", "간격 눈금"), sample: "space",
    test: /^--space-\d+$/ },
  { id: "type", title: () => t("design.family.type", "글자"), note: () => t("design.family.typeNote", "크기, 이름 붙은 글자 모양, 글꼴"), sample: "type",
    test: /^--(text|type|font)-/ },
  { id: "radius", title: () => t("design.family.radius", "반지름"), note: () => t("design.family.radiusNote", "컨트롤과 떠 있는 것의 모서리"), sample: "radius",
    test: /^--radius/ },
  { id: "motion", title: () => t("design.family.motion", "모션"), note: () => t("design.family.motionNote", "길이와 가속 곡선"), sample: "motion",
    test: /^--(motion|ease)-/ },
  { id: "shadow", title: () => t("design.family.shadow", "그림자와 테두리"), note: () => t("design.family.shadowNote", "1 px 테두리는 그림자 고리로 그립니다"), sample: "shadow",
    test: /^--shadow-/ },
]);

/* 컴포넌트 7종. `on`은 「활성」 상태를 그 컴포넌트가 어떻게 적는지다: 클래스 하나를 붙들거나,
 * 창이 읽는 속성을 켠다. `na`는 의미가 없는 칸 — 눌리지 않는 배지의 호버처럼 규칙이 없는 것이
 * 맞는 칸이며, 이유를 옆에 적는다. 여기에 없는 칸은 눈에 보이는 규칙이 있어야 한다(게이트). */
const DESIGN_COMPONENTS = Object.freeze([
  { id: "button", name: () => t("design.comp.button", "버튼"), cls: ".btn", on: "class",
    html: (attrs) => `<button type="button" class="btn" ${attrs}>${t("design.sample.save", "저장")}</button>`, na: {} },
  { id: "chip", name: () => t("design.comp.chip", "칩"), cls: ".task-board-filter", on: "aria-pressed",
    html: (attrs) => `<button type="button" class="task-board-filter" ${attrs}>${t("design.sample.chip", "질문 3")}</button>`,
    // 걸러내기 칩은 쓸 수 없을 때 꺼지지 않고 줄에서 빠진다.
    na: { disabled: true } },
  { id: "row", name: () => t("design.comp.row", "행"), cls: ".nav-row", on: "aria-current",
    html: (attrs) => `<button type="button" class="nav-row" ${attrs}><span>${t("design.sample.row", "작업")}</span></button>`, na: {} },
  { id: "badge", name: () => t("design.comp.badge", "배지"), cls: ".nav-badge", on: "class",
    html: (attrs) => `<span class="nav-badge" ${attrs}>8</span>`,
    // 배지는 눌리지 않고 꺼지지도 않는다: 호버·눌림·비활성이 낼 것이 없다.
    na: { hover: true, active: true, disabled: true } },
  { id: "input", name: () => t("design.comp.input", "입력"), cls: ".settings-input", on: "class",
    html: (attrs) => `<input class="settings-input" aria-label="${t("design.comp.input", "입력")}" value="acme-web" ${attrs} />`,
    // 입력칸의 눌림은 초점이 말한다: 따로 낼 모습이 없다.
    na: { active: true } },
  { id: "toggle", name: () => t("design.comp.toggle", "토글"), cls: ".auto-row-toggle", on: "aria-checked",
    html: (attrs) => `<button type="button" role="switch" aria-label="${t("design.comp.toggle", "토글")}" class="auto-row-toggle" ${attrs}></button>`, na: {} },
  { id: "tab", name: () => t("design.comp.tab", "탭"), cls: ".board-mode button", on: "aria-pressed",
    html: (attrs) => `<span class="board-mode"><button type="button" ${attrs}>${t("design.sample.row", "작업")}</button></span>`,
    // 모드 묶음의 탭은 꺼지지 않는다: 쓸 수 없는 모드는 묶음에서 빠진다.
    na: { disabled: true } },
]);

const DESIGN_STATES = Object.freeze([
  { id: "base", name: () => t("design.state.base", "기본") },
  { id: "hover", name: () => t("design.state.hover", "호버") },
  { id: "active", name: () => t("design.state.active", "활성") },
  { id: "focus", name: () => t("design.state.focus", "초점") },
  { id: "disabled", name: () => t("design.state.disabled", "비활성") },
  { id: "loading", name: () => t("design.state.loading", "로딩") },
]);

/* 한 칸이 눈에 보이는 차이를 내는지 볼 때 읽는 계산된 값. */
const DESIGN_PROBE = Object.freeze([
  "color", "backgroundColor", "borderTopColor", "boxShadow", "outlineStyle", "outlineColor", "opacity",
  "textDecorationLine", "transform", "fontWeight", "cursor", "animationName",
]);

function designNode(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/* ---- 들어가는 길 ---- */

function designPageAvailable() {
  return designUnlocked;
}

/* 버전 줄 한 번. 5번째에 목록에 나타나고, 한 번 나타나면 이 창이 닫힐 때까지 있다. */
function pressDesignUnlock() {
  if (designUnlocked) return;
  const now = performance.now();
  designPresses = now - designLastPressMs > DESIGN_UNLOCK_WINDOW_MS ? 1 : designPresses + 1;
  designLastPressMs = now;
  if (designPresses < DESIGN_UNLOCK_PRESSES) return;
  designUnlocked = true;
  paintSettingsSearch();
  toast(t("design.unlocked", "설정 목록에 「디자인 시스템」이 나타났습니다"));
}

/* ---- 토큰 ---- */

/* tokens.css에 선언된 이름 전부(중복 없이). 파일이 답이다: 창이 이름 목록을 따로 들고 있지 않다. */
async function designReadTokenNames() {
  if (designTokenNames) return designTokenNames;
  const text = await (await fetch("./tokens.css")).text();
  designTokenNames = [...new Set([...text.matchAll(/^\s*(--[\w-]+)\s*:/gm)].map((hit) => hit[1]))];
  return designTokenNames;
}

function designValueOf(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function designTokenRow(name, kind) {
  const row = designNode("li", "ds-token");
  row.dataset.token = name;
  const sample = designNode("span");
  if (kind === "color") { sample.className = "ds-sw"; sample.style.background = `var(${name})`; }
  else if (kind === "space") { sample.className = "ds-sp"; sample.style.width = `var(${name})`; }
  else if (kind === "radius") { sample.className = "ds-sw"; sample.style.borderRadius = `var(${name})`; }
  else if (kind === "shadow") { sample.className = "ds-sw"; sample.style.boxShadow = `var(${name})`; }
  else if (kind === "motion") sample.className = "ds-mo";
  else if (kind === "type") {
    sample.className = "ds-ty";
    sample.textContent = t("design.sample.glyph", "가");
    if (name.startsWith("--type-")) sample.style.font = `var(${name})`;
    else if (name.startsWith("--text-")) sample.style.fontSize = `var(${name})`;
    else sample.style.fontFamily = `var(${name})`;
  } else sample.className = "ds-ty";
  const words = designNode("div");
  const value = designValueOf(name);
  words.append(designNode("code", "", name), designNode("small", "", value));
  // 이 환경에서 값이 풀리지 않는 토큰: 선언은 있으나 조건부 블록 안에 있다. 누락이 아니다.
  if (value === "") {
    row.dataset.conditional = "";
    words.append(designNode("em", "ds-conditional", t("design.conditional", "조건부")));
  }
  row.append(sample, words);
  return row;
}

function designTokenList(names, kind) {
  const list = designNode("ul", "ds-tokens");
  for (const name of names) list.append(designTokenRow(name, kind));
  return list;
}

/* ---- 상태 칸 ---- */

/* 호버·활성·초점은 밖에서 붙들 수 없으므로, 그 의사 클래스를 가진 위쪽 규칙을 클래스로 바꿔
 * 한 번 복제한다(:hover → .ds-hover). 선언도 명세도 같고, 새로 적은 값은 없다. */
function designForceStateRules() {
  if (designStateRulesForced) return;
  const swaps = [[/:hover/g, ".ds-hover"], [/:active/g, ".ds-active"], [/:focus-visible/g, ".ds-focus"], [/:focus(?![-\w])/g, ".ds-focus"]];
  const forced = [];
  for (const sheet of document.styleSheets) {
    let rules;
    try { rules = sheet.cssRules; } catch { continue; }
    for (const rule of rules) {
      if (rule.type !== CSSRule.STYLE_RULE || !/:(hover|active|focus)/.test(rule.selectorText)) continue;
      let selector = rule.selectorText;
      for (const [from, to] of swaps) selector = selector.replace(from, to);
      forced.push(`${selector} { ${rule.style.cssText} }`);
    }
  }
  let style = document.getElementById("ds-forced");
  if (!style) {
    style = designNode("style");
    style.id = "ds-forced";
    document.head.append(style);
  }
  style.textContent = forced.join("\n");
  designStateRulesForced = forced.length;
}

/* 한 상태를 어떻게 적는가: 클래스를 붙들거나, 창이 읽는 속성을 켠다. 로딩은 컴포넌트마다 다르지
 * 않다 — `aria-busy="true"` 하나(shell.css의 공통 정의). */
function designStateAttrs(component, state) {
  if (state === "hover") return { cls: "ds-hover", attrs: "" };
  if (state === "focus") return { cls: "ds-focus", attrs: "" };
  if (state === "disabled") return { cls: "", attrs: component.id === "badge" ? 'aria-disabled="true"' : "disabled" };
  if (state === "loading") return { cls: "", attrs: 'aria-busy="true"' };
  if (state === "active") {
    if (component.on === "class") return { cls: "ds-active", attrs: "" };
    return { cls: "", attrs: `${component.on}="${component.on === "aria-current" ? "page" : "true"}"` };
  }
  return { cls: "", attrs: "" };
}

function designCell(component, state) {
  const { cls, attrs } = designStateAttrs(component, state);
  const holder = designNode("div");
  holder.innerHTML = component.html(attrs);
  const target = holder.querySelector(component.cls.split(" ").pop()) ?? holder.firstElementChild;
  if (cls) target.classList.add(cls);
  return { holder, target };
}

function designSnapshot(node) {
  const style = getComputedStyle(node);
  const after = getComputedStyle(node, "::after");
  return `${DESIGN_PROBE.map((prop) => style[prop]).join("|")}||${after.transform}${after.backgroundColor}`;
}

/* 게이트가 부른다: 각 칸이 기본과 눈에 보이게 다른가. 테마마다 한 번 재어 둔다. 페이지는 그리는 데
 * 이것을 쓰지 않는다. */
function designStateCoverage() {
  const theme = document.documentElement.dataset.theme ?? "dark";
  if (designCoverage.has(theme)) return designCoverage.get(theme);
  designForceStateRules();
  const found = {};
  const host = designNode("div");
  host.style.cssText = "position:absolute;visibility:hidden;pointer-events:none";
  document.body.append(host);
  for (const component of DESIGN_COMPONENTS) {
    let base = null;
    for (const state of DESIGN_STATES) {
      const { holder, target } = designCell(component, state.id);
      host.append(holder);
      const seen = designSnapshot(target);
      if (state.id === "base") base = seen;
      else found[`${component.id}:${state.id}`] = seen !== base;
    }
  }
  host.remove();
  designCoverage.set(theme, found);
  return found;
}

/* ---- 페이지 ---- */

async function paintDesignPage() {
  const pane = document.querySelector('.settings-pane[data-pane="design-system"]');
  if (!pane) return;
  const names = await designReadTokenNames();
  const body = pane.querySelector(".ds-body");
  const shared = new Map(DESIGN_FAMILIES.map((family) => [family.id, []]));
  const tail = [];
  for (const name of names) {
    const family = DESIGN_FAMILIES.find((one) => one.test.test(name));
    (family ? shared.get(family.id) : tail).push(name);
  }
  const parts = [
    designNode("h3", "ds-h", t("design.tokens", "토큰")),
    designNode("p", "ds-sub", t("design.tokensAbout", "ui/tokens.css의 선언을 그대로 읽어 그립니다. 값은 지금 테마에서 풀린 값입니다.")),
  ];
  for (const family of DESIGN_FAMILIES) {
    const members = shared.get(family.id);
    const box = designNode("section", "ds-fam");
    const head = designNode("h4", "ds-fam-head", family.title());
    head.append(designNode("small", "", t("design.count", "{{n}}개", { n: members.length }) + ` · ${family.note()}`));
    box.append(head, designTokenList(members, family.sample));
    parts.push(box);
  }
  // 긴 꼬리는 펼칠 때 그린다.
  const more = designNode("details", "ds-more");
  more.append(designNode("summary", "", t("design.more", "그 밖 — 화면·컴포넌트 전용 토큰 {{n}}개 (펼치면 그립니다)", { n: tail.length })));
  more.addEventListener("toggle", () => {
    if (!more.open || more.dataset.filled) return;
    more.dataset.filled = "1";
    more.append(designTokenList(tail, "other"));
    paintDesignStats(pane);
  });
  parts.push(more);

  designForceStateRules();
  parts.push(
    designNode("h3", "ds-h", t("design.components", "컴포넌트 7종 × 6상태")),
    designNode("p", "ds-sub", t("design.componentsAbout", "창이 쓰는 클래스 그대로입니다. 호버·활성·초점은 같은 규칙을 클래스로 붙들어 그립니다.")),
  );
  for (const component of DESIGN_COMPONENTS) {
    const box = designNode("section", "ds-comp");
    box.dataset.component = component.id;
    const head = designNode("h4", "ds-comp-head", component.name());
    head.append(designNode("code", "", component.cls));
    const grid = designNode("div", "ds-states");
    for (const state of DESIGN_STATES) {
      const { holder } = designCell(component, state.id);
      const cell = designNode("div", "ds-cell");
      cell.dataset.state = state.id;
      const label = designNode("span", "ds-cell-name", state.name());
      if (component.na[state.id]) {
        cell.dataset.na = "";
        label.append(" ", designNode("i", "", t("design.na", "해당 없음")));
      }
      cell.append(holder, label);
      grid.append(cell);
    }
    box.append(head, grid);
    parts.push(box);
  }
  body.replaceChildren(...parts);
  pane.dataset.tail = String(tail.length);
  pane.dataset.names = String(names.length);
  paintDesignStats(pane);
}

/* 맨 위 줄: 토큰 수, 그림에 든 수, 누락, 조건부, 해당 없음 칸. 그림에서 센다 — 파일의 수와 같은지는
 * 게이트가 본다. */
function paintDesignStats(pane) {
  const stats = pane.querySelector(".ds-stats");
  const total = Number(pane.dataset.names ?? 0);
  const tail = Number(pane.dataset.tail ?? 0);
  const drawn = pane.querySelectorAll(".ds-token").length;
  const pending = pane.querySelector(".ds-more")?.dataset.filled ? 0 : tail;
  const conditional = pane.querySelectorAll(".ds-token[data-conditional]").length;
  const naCells = pane.querySelectorAll(".ds-cell[data-na]").length;
  const facts = [
    ["tokens", t("design.stat.tokens", "토큰 {{n}}개", { n: total })],
    ["drawn", t("design.stat.drawn", "그림에 든 것 {{n}}", { n: drawn })],
    ["missing", t("design.stat.missing", "누락 {{n}}", { n: Math.max(0, total - drawn - pending) })],
    ["conditional", t("design.stat.conditional", "조건부 {{n}}", { n: conditional })],
    ["na", t("design.stat.na", "해당 없음 {{n}}칸", { n: naCells })],
  ];
  stats.replaceChildren(...facts.map(([key, words]) => {
    const fact = designNode("span", "ds-stat", words);
    fact.dataset.dsStat = key;
    return fact;
  }));
}

/* 페이지 안 테마 스위치. 사람이 고른 테마는 건드리지 않는다 — 속성만 바꾸고, 떠날 때 되돌린다. */
function previewDesignTheme(choice) {
  designPageTheme = choice;
  if (choice === "light") document.documentElement.dataset.theme = "light";
  else delete document.documentElement.dataset.theme;
  for (const button of document.querySelectorAll("[data-ds-theme]")) {
    button.setAttribute("aria-pressed", String(button.dataset.dsTheme === choice));
  }
}

function leaveDesignPage() {
  if (designPageTheme === null) return;
  designPageTheme = null;
  applyTheme();
}

function enterDesignPage() {
  const current = document.documentElement.dataset.theme === "light" ? "light" : "dark";
  previewDesignTheme(designPageTheme ?? current);
  void paintDesignPage();
}

for (const button of document.querySelectorAll("[data-ds-theme]")) {
  button.addEventListener("click", () => {
    previewDesignTheme(button.dataset.dsTheme);
    void paintDesignPage();
  });
}
el("general-version").addEventListener("click", pressDesignUnlock);
