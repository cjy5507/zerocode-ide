/* 공급망 렌즈의 계약 (지식 그래프 P4·P5, 2026-09-17).
 *
 * 사람의 말: 「네모 세모 그리고 취약점까지 보여줘 … 의미를 넣어서 SCA와 SBOM 요소도」.
 * 설계는 docs/design/knowledge-supply-chain-20260917.md — 창이 읽는 것은 §5.3의 답 하나이고,
 * 이 파일은 그 답이 그림에 서는 방식을 묻는다:
 *   렌즈   기본 꺼짐, 켜면 `supply_chain_graph`를 한 번 묻고, 묻는 동안·실패·지난 답은 카드의 한
 *          줄이 **답의 낱말 그대로** 말한다.
 *   점     구성요소는 사각(생태계의 잉크, 멤버는 두꺼운 테두리), 취약점은 삼각(심각도의 잉크).
 *   선     구성요소 → 구성요소는 볼트의 `depends_on`과 같은 선, 취약점 → 구성요소는 새 종류 `affects`.
 *   접기   멤버 + 직접 의존 + 취약점이 닿는 경로(와 볼트가 이름으로 가리키는 구성요소)만 서고,
 *          나머지는 그것에 먼저 닿는 멤버의 「+N」이다(G7).
 *   유령   유령 링크의 이름이 구성요소의 이름과 글자 그대로 같을 때만 유령 대신 그 구성요소로.
 *   카드   구성요소(생태계·판·출처·의존 수·취약점), 취약점(id·별칭·심각도·점수·요약·고친 판·영향·경로·OSV).
 *   검색   `kind:component`·`kind:vulnerability`·`sev:<심각도>`.
 *
 * 기대값은 제품의 셈이 아니라 **답에서 곧장** 센다(너비 우선 한 번) — 제품의 함수로 제품을 재면
 * 무엇도 묻지 않는다. 판(`evaluate`)이 던지면 그 던짐이 FAIL 한 줄의 근거가 되고 하네스는 끝까지
 * 달린다(렌즈가 없는 제품에서도 뒤의 측정이 선다). */

/* 의미를 재는 작은 답: Cargo 멤버 둘과 npm 루트, 레지스트리 마흔, 아무도 의존하지 않는 하나,
 * 심각도 여섯 낱말을 두 번 도는 취약점 열둘(열두째는 정보성 권고). 이름을 준 둘은 유령 링크 시험의 짝이다. */
export const SUPPLY_SMALL = Object.freeze({
  root: "/repo",
  members: ["app-core", "app-shell", "npm:ui"],
  deps: 40,
  direct: 2,
  fanout: 2,
  unreachable: 1,
  vulnerabilities: 12,
  names: { 3: "tauri", 9: "serde" },
});

/* 천 개의 구성요소 — 설계 §2 G7의 잣대(「1,000 구성요소에서 첫 그림」). 이 저장소의 실측
 * (구성요소 1,102, 멤버 둘레 수십, RustSec 22)과 같은 몫으로. */
export const SUPPLY_THOUSAND = Object.freeze({
  root: "/repo",
  members: ["core", "shell", "tools", "zo", "zo-cli", "zo-tui", "zo-ide", "npm:ui"],
  deps: 992,
  direct: 12,
  fanout: 2,
  unreachable: 0,
  vulnerabilities: 22,
});

/* 답에서 곧장 센 접기 — 제품과 같은 규칙, 제품과 다른 손. 멤버를 답의 순서로 줄 세워 너비
 * 우선으로 걷고(선은 답의 순서: kind → from → to), 먼저 닿은 멤버가 그 구성요소의 임자다. */
export const supplyReferenceSource = `(${String((answer, { unfolded = [], named = [] } = {}) => {
  const count = answer.components.length;
  const out = Array.from({ length: count }, () => []);
  const affected = new Set();
  for (const edge of answer.edges) {
    if (edge.kind === "depends_on") out[edge.from].push(edge.to);
    if (edge.kind === "affects") affected.add(edge.to);
  }
  const root = new Array(count).fill(-1);
  const parent = new Array(count).fill(-1);
  const hops = new Array(count).fill(-1);
  const queue = [];
  answer.components.forEach((row, at) => {
    if (!row.member) return;
    root[at] = at;
    hops[at] = 0;
    queue.push(at);
  });
  for (let head = 0; head < queue.length; head += 1) {
    const at = queue[head];
    for (const next of out[at]) {
      if (hops[next] >= 0) continue;
      hops[next] = hops[at] + 1;
      root[next] = root[at];
      parent[next] = at;
      queue.push(next);
    }
  }
  const visible = new Set();
  answer.components.forEach((row, at) => {
    if (row.member || hops[at] === 1 || named.includes(row.name)) visible.add(at);
  });
  const paths = new Map();
  for (const at of affected) {
    const chain = [];
    for (let step = at; step >= 0; step = parent[step]) chain.unshift(step);
    if (root[at] < 0) chain.splice(0, chain.length, at);
    paths.set(at, chain);
    for (const one of chain) visible.add(one);
  }
  const unfoldedSeats = new Set(unfolded.map((id) => answer.components.findIndex((row) => row.id === id)));
  answer.components.forEach((row, at) => {
    if (root[at] >= 0 && unfoldedSeats.has(root[at])) visible.add(at);
  });
  const folded = new Map();
  answer.components.forEach((row, at) => {
    if (visible.has(at) || root[at] < 0) return;
    const member = answer.components[root[at]].id;
    folded.set(member, (folded.get(member) ?? 0) + 1);
  });
  const dependsOn = answer.edges.filter((edge) => edge.kind === "depends_on"
    && visible.has(edge.from) && visible.has(edge.to)).length;
  return {
    visible: [...visible].map((at) => answer.components[at].id).sort(),
    folded: Object.fromEntries(folded),
    hidden: count - visible.size,
    dependsOn,
    affects: answer.edges.filter((edge) => edge.kind === "affects").length,
    paths: Object.fromEntries([...paths].map(([at, chain]) => [answer.components[at].id,
      chain.map((one) => answer.components[one].id)])),
    unreachable: root.filter((one) => one < 0).length,
  };
})})`;

/* 판의 살림을 같은 자리로 — 앞 케이스가 남긴 렌즈·검색·고른 점·못박은 손은 이 케이스의 그림을
 * 바꾼다. 판 안에서 부르는 글자라 문자열로 싣는다. */
const RESET = `(() => {
  knowledgeQuery = "";
  knowledgeTagsPicked.clear();
  knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
  knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
  knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
  knowledgeSlicerCutoff = 0;
  knowledgeClusterPicked = -1;
  knowledgeEntryPending = false;
  knowledgeRevealKey = null;
  knowledgeRevealMode = null;
  knowledgeMode = "global";
  knowledgePainterKind = null;
  knowledgeShowSources = false;
  const find = document.querySelector(".knowledge-view:not([hidden]) .knowledge-query");
  if (find) find.value = "";
})`;

const SMALL_VAULT = Object.freeze({ pages: 12, linksPer: 2, ghosts: 2, tags: ["core", "reading"] });

export async function testKnowledgeSupply(page, ok) {
  await page.setViewportSize({ width: 1280, height: 860 });
  /* 1. 기본 꺼짐 — 문을 열어도 공급망을 묻지 않고, 「보기」 메뉴에 렌즈가 눌리지 않은 채 선다. */
  const opened = await page.evaluate(async ({ reset, vault, supply }) => {
    try {
      eval(reset)();
      window.__VAULT__ = vault;
      window.__SUPPLY__ = supply;
      delete window.__SUPPLY_HOLD__;
      delete window.__SUPPLY_FAIL__;
      knowledgeReport = null;
      knowledgeAskedAt = 0;
      dropTab("knowledge");
      document.getElementById("nav-knowledge").click();
      for (let round = 0; round < 240; round += 1) {
        await new Promise((done) => requestAnimationFrame(done));
        const view = document.querySelector(".knowledge-view:not([hidden])");
        if (view && (knowledgeLayouts.get(view)?.model.count ?? 0) > 0) break;
      }
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const layout = knowledgeLayouts.get(view);
      const flag = view.querySelector('.knowledge-lens-popover [data-knowledge-flag="supply"]');
      return {
        shown: typeof knowledgeSupplyShown === "undefined" ? "missing" : knowledgeSupplyShown,
        asked: (window.__SUPPLY_ASKED__ ?? []).length,
        kinds: [...new Set(layout.model.kinds)].sort(),
        flag: flag !== null,
        button: flag?.localName ?? "",
        pressed: flag?.getAttribute("aria-pressed") ?? "",
        named: flag?.getAttribute("aria-label") ?? "",
        overview: view.querySelector(".knowledge-overview-supply")?.hidden ?? "missing",
        chip: view.querySelector('.knowledge-token-chip[data-token="sev:"]')?.hidden ?? "missing",
      };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reset: RESET, vault: SMALL_VAULT, supply: SUPPLY_SMALL });
  ok("supply lens: off by default — opening the graph asks nothing, the 보기 menu carries an unpressed 공급망 button, and no supply word or token helper stands",
    !opened.thrown && opened.shown === false && opened.asked === 0
      && opened.kinds.every((kind) => ["page", "ghost"].includes(kind))
      && opened.flag && opened.button === "button" && opened.pressed === "false" && opened.named !== ""
      && opened.overview === true && opened.chip === true,
    JSON.stringify(opened));

  /* 2. 켠다 — 한 번 묻고(인자는 계약의 둘), 묻는 동안은 카드의 줄이 「확인 중」이고 그림은 아직
   * 볼트뿐이다. 답이 오면 구성요소·취약점이 모양과 옷을 입고 선다. */
  const lensOn = await page.evaluate(async ({ reference }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const workspaceWas = activeWorktreePath;
      activeWorktreePath = "/repo";
      window.__SUPPLY_HOLD__ = true;
      view.querySelector(".knowledge-lens-toggle").click();
      view.querySelector('[data-knowledge-flag="supply"]').click();
      for (let round = 0; round < 30 && !window.__SUPPLY_RELEASE__; round += 1) await frame();
      const line = () => view.querySelector(".knowledge-supply-lookup");
      const asking = {
        asked: (window.__SUPPLY_ASKED__ ?? []).map((args) => JSON.stringify(args)),
        pressed: view.querySelector('[data-knowledge-flag="supply"]').getAttribute("aria-pressed"),
        overviewHidden: view.querySelector(".knowledge-overview-supply")?.hidden,
        state: line()?.dataset.lookupState ?? "",
        said: line()?.textContent ?? "",
        kinds: [...new Set(knowledgeLayouts.get(view).model.kinds)].sort(),
      };
      delete window.__SUPPLY_HOLD__;
      window.__SUPPLY_RELEASE__?.();
      delete window.__SUPPLY_RELEASE__;
      for (let round = 0; round < 120; round += 1) {
        await frame();
        if (knowledgeLayouts.get(view).model.kinds.includes("component")) break;
      }
      for (let round = 0; round < 600 && (knowledgeLayouts.get(view)?.left ?? 0) > 0; round += 1) await frame();
      view.querySelector(".knowledge-lens-toggle").click();
      activeWorktreePath = workspaceWas;
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const answer = knowledgeSupplyAnswer;
      const expected = eval(reference)(answer);
      const componentOf = new Map(answer.components.map((row) => [row.id, row]));
      const vulnerabilityOf = new Map(answer.vulnerabilities.map((row) => [row.id, row]));
      const nodes = [];
      for (let at = 0; at < model.count; at += 1) {
        const kind = model.kinds[at];
        if (kind !== "component" && kind !== "vulnerability") continue;
        const node = layout.nodeEls[at];
        const dot = node?.querySelector(".knowledge-dot");
        nodes.push({
          key: model.keys[at], kind, title: model.titles[at],
          shape: dot?.dataset.shape ?? "",
          ecosystem: node?.dataset.ecosystem ?? "", member: node?.dataset.member ?? "",
          severity: node?.dataset.severity ?? "", informational: node?.dataset.informational ?? "",
          strokeWidth: dot ? Number.parseFloat(getComputedStyle(dot).strokeWidth) : 0,
          fill: dot ? getComputedStyle(dot).fill : "",
        });
      }
      const keyOf = (kind, id) => `${KNOWLEDGE_SUPPLY_KEYS[kind]}${id}`;
      const rows = nodes.map((node) => {
        const component = node.kind === "component"
          ? componentOf.get(node.key.slice(KNOWLEDGE_SUPPLY_KEYS.component.length)) : null;
        const vulnerability = node.kind === "vulnerability"
          ? vulnerabilityOf.get(node.key.slice(KNOWLEDGE_SUPPLY_KEYS.vulnerability.length)) : null;
        return {
          ...node,
          known: component !== null ? component !== undefined : vulnerability !== undefined,
          wantedShape: node.kind === "component" ? "square" : "triangle",
          wantedTitle: component ? (component.version === "" ? component.name : `${component.name}@${component.version}`)
            : vulnerability?.id ?? "",
          wantedEcosystem: component?.ecosystem ?? "",
          wantedMember: component ? String(component.member) : "",
          wantedSeverity: vulnerability?.severity ?? "",
          wantedInformational: vulnerability?.informational ?? "",
        };
      });
      const member = rows.filter((row) => row.kind === "component" && row.member === "true");
      const plain = rows.filter((row) => row.kind === "component" && row.member !== "true");
      const inkOf = (kind, attribute, value) => rows.find((row) => row.kind === kind
        && row[attribute] === value)?.fill ?? "";
      return {
        asking,
        after: {
          asked: (window.__SUPPLY_ASKED__ ?? []).length,
          visibleKeys: rows.filter((row) => row.kind === "component").map((row) => row.key).sort(),
          wantedKeys: expected.visible.map((id) => keyOf("component", id)).sort(),
          vulnerabilities: rows.filter((row) => row.kind === "vulnerability").length,
          wantedVulnerabilities: answer.vulnerabilities.length,
          mismatched: rows.filter((row) => !row.known || row.shape !== row.wantedShape || row.title !== row.wantedTitle
            || row.ecosystem !== row.wantedEcosystem || row.member !== row.wantedMember
            || row.severity !== row.wantedSeverity || row.informational !== row.wantedInformational),
          memberStroke: Math.min(...member.map((row) => row.strokeWidth)),
          plainStroke: Math.max(...plain.map((row) => row.strokeWidth)),
          cargoInk: inkOf("component", "wantedEcosystem", "cargo"),
          npmInk: plain.find((row) => row.ecosystem === "npm")?.fill ?? "",
          cargoPlainInk: plain.find((row) => row.ecosystem === "cargo")?.fill ?? "",
          severityInks: Object.fromEntries(["critical", "high", "medium", "low", "none", "unknown"]
            .map((word) => [word, rows.find((row) => row.severity === word && row.informational === "")?.fill ?? ""])),
        },
      };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reference: supplyReferenceSource });
  const lensDetail = JSON.stringify(lensOn);
  ok("supply lens: pressing 공급망 asks supply_chain_graph once for the active workspace (root, refresh false) and, while it answers, the card line says it is asking and the picture is still the vault",
    !lensOn.thrown && lensOn.asking.asked.length === 1
      && lensOn.asking.asked[0] === JSON.stringify({ root: "/repo", refresh: false })
      && lensOn.asking.pressed === "true" && lensOn.asking.overviewHidden === false
      && lensOn.asking.state === "asking" && lensOn.asking.said.trim() !== ""
      && lensOn.asking.kinds.every((kind) => ["page", "ghost"].includes(kind)),
    lensDetail);
  ok("supply lens: every component stands as a square named name@version in its ecosystem's ink, members with the thicker border, every vulnerability as a triangle wearing its severity and informational words",
    !lensOn.thrown && lensOn.after.asked === 1 && lensOn.after.mismatched.length === 0
      && lensOn.after.vulnerabilities === lensOn.after.wantedVulnerabilities
      && lensOn.after.memberStroke > lensOn.after.plainStroke
      && lensOn.after.npmInk !== "" && lensOn.after.cargoPlainInk !== "" && lensOn.after.npmInk !== lensOn.after.cargoPlainInk,
    lensDetail);
  ok("supply lens: the severity ramp gives critical, high, medium and low four different inks, and none shares unknown's ink by declaration",
    !lensOn.thrown && Object.values(lensOn.after.severityInks).every((ink) => ink !== "")
      && new Set(["critical", "high", "medium", "low", "unknown"].map((word) => lensOn.after.severityInks[word])).size === 5
      && lensOn.after.severityInks.none === lensOn.after.severityInks.unknown,
    lensDetail);
  /* G7 — 서는 구성요소는 답에서 곧장 센 집합과 같고, 접힌 것은 멤버의 「+N」이다. */
  ok("supply lens G7: only members, their direct dependencies and each vulnerability's path from a member stand — the drawn component set is the one counted straight from the answer",
    !lensOn.thrown && lensOn.after.visibleKeys.length > 0
      && lensOn.after.visibleKeys.length < SUPPLY_SMALL.deps + SUPPLY_SMALL.members.length + SUPPLY_SMALL.unreachable
      && JSON.stringify(lensOn.after.visibleKeys) === JSON.stringify(lensOn.after.wantedKeys),
    lensDetail);

  /* 3. 선과 접기의 수, 범례. */
  const wiring = await page.evaluate(async ({ reference }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const answer = knowledgeSupplyAnswer;
      const expected = eval(reference)(answer);
      const supplyKind = (at) => model.kinds[at] === "component" || model.kinds[at] === "vulnerability";
      let dependsOn = 0;
      let affects = 0;
      let affectsWrong = 0;
      for (let at = 0; at < model.edgeCount; at += 1) {
        const word = KNOWLEDGE_EDGE_KINDS[model.kind[at]];
        if (word === "depends_on" && model.kinds[model.from[at]] === "component" && model.kinds[model.to[at]] === "component") dependsOn += 1;
        if (word === "affects") {
          affects += 1;
          if (model.kinds[model.from[at]] !== "vulnerability" || model.kinds[model.to[at]] !== "component") affectsWrong += 1;
        }
      }
      const lines = [...view.querySelectorAll(".knowledge-edge.kind-affects")];
      const folded = {};
      const foldWords = {};
      for (let at = 0; at < model.count; at += 1) {
        const count = layout.folded === null ? 0 : layout.folded[at];
        if (!supplyKind(at) || count === 0) continue;
        const id = model.keys[at].slice(KNOWLEDGE_SUPPLY_KEYS.component.length);
        folded[id] = count;
        const node = layout.nodeEls[at];
        foldWords[id] = { tail: node?.querySelector(".knowledge-fold")?.textContent ?? "",
          classed: node?.classList.contains("is-folded") ?? false };
      }
      const legendEdge = view.querySelector('.knowledge-legend [data-edge-kind="affects"]');
      const legendKinds = [...view.querySelectorAll(".knowledge-legend [data-node-kind]")]
        .filter((row) => !row.hidden).map((row) => row.dataset.nodeKind);
      return {
        order: [...KNOWLEDGE_EDGE_KINDS],
        dependsOn, wantedDependsOn: expected.dependsOn,
        affects, wantedAffects: expected.affects, affectsWrong,
        affectsLines: lines.length,
        arrows: lines.every((line) => line.getAttribute("marker-end") === "url(#knowledge-arrow-affects)")
          && view.querySelector("#knowledge-arrow-affects") !== null,
        folded, wantedFolded: expected.folded, foldWords,
        legendEdge: legendEdge !== null && !legendEdge.hidden
          && legendEdge.querySelector(".knowledge-legend-edge.kind-affects") !== null
          && (legendEdge.querySelector(".knowledge-legend-why")?.textContent ?? "") !== "",
        legendKinds,
      };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reference: supplyReferenceSource });
  const wiringDetail = JSON.stringify(wiring);
  ok("supply lens: depends_on joins components with the vault's own relation code, and affects is a new kind appended after the seven — vulnerability to component, drawn with its arrow and taught by the legend",
    !wiring.thrown
      && wiring.order.join(",") === "mentions,related,implements,depends_on,supersedes,contradicts,merge,affects"
      && wiring.dependsOn === wiring.wantedDependsOn && wiring.affects === wiring.wantedAffects
      && wiring.affects > 0 && wiring.affectsWrong === 0 && wiring.affectsLines === wiring.affects && wiring.arrows
      && wiring.legendEdge && wiring.legendKinds.includes("component") && wiring.legendKinds.includes("vulnerability"),
    wiringDetail);
  ok("supply lens G7: what stays folded is counted on the member that reaches it first — its +N, in the hub fold's words and ring",
    !wiring.thrown && Object.keys(wiring.wantedFolded).length > 0
      && JSON.stringify(Object.entries(wiring.folded).sort()) === JSON.stringify(Object.entries(wiring.wantedFolded).sort())
      && Object.entries(wiring.foldWords).every(([id, words]) => words.tail === `+${wiring.folded[id]}` && words.classed),
    wiringDetail);

  /* 4. 카드 — 구성요소와 취약점. 페이지의 문(열기·작업 요청·연결 추가)은 서지 않는다. */
  const cards = await page.evaluate(async ({ reference }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const layout = knowledgeLayouts.get(view);
      const answer = knowledgeSupplyAnswer;
      const expected = eval(reference)(answer);
      const fact = (name) => view.querySelector(`.knowledge-inspector-supply [data-supply-fact="${name}"]`);
      const said = (name) => fact(name)?.textContent.trim() ?? null;
      const listKeys = (selector) => [...view.querySelectorAll(`${selector} [data-knowledge-key]`)]
        .map((row) => row.dataset.knowledgeKey);
      const shown = (selector) => {
        const held = view.querySelector(selector);
        return held !== null && !held.hidden && held.getClientRects().length > 0;
      };
      const affectsEdge = answer.edges.find((edge) => edge.kind === "affects");
      const vulnerability = answer.vulnerabilities[affectsEdge.from];
      const target = answer.components[affectsEdge.to];
      const componentKey = `${KNOWLEDGE_SUPPLY_KEYS.component}${target.id}`;
      selectKnowledgeNode(view, componentKey);
      await new Promise((done) => requestAnimationFrame(done));
      const component = {
        section: shown(".knowledge-inspector-supply"),
        title: view.querySelector(".knowledge-card .knowledge-inspector-title")?.textContent ?? "",
        ecosystem: said("ecosystem"), version: said("version"), origin: said("origin"),
        dependencies: said("dependencies"), lockfiles: said("lockfiles"),
        vulnerabilities: listKeys(".knowledge-supply-vulnerabilities"),
        wantedVulnerabilities: answer.edges.filter((edge) => edge.kind === "affects" && edge.to === affectsEdge.to)
          .map((edge) => `${KNOWLEDGE_SUPPLY_KEYS.vulnerability}${answer.vulnerabilities[edge.from].id}`),
        wantedDependencies: answer.edges.filter((edge) => edge.kind === "depends_on" && edge.from === affectsEdge.to).length,
        pageDoors: [".knowledge-inspector-open", ".knowledge-inspector-request", ".knowledge-inspector-link"]
          .filter((selector) => shown(selector)),
      };
      const memberRow = answer.components.find((row) => row.member && (expected.folded[row.id] ?? 0) > 0);
      selectKnowledgeNode(view, `${KNOWLEDGE_SUPPLY_KEYS.component}${memberRow.id}`);
      await new Promise((done) => requestAnimationFrame(done));
      const member = {
        folded: said("folded"),
        wantedFolded: expected.folded[memberRow.id],
        unfold: view.querySelector(".knowledge-inspector-supply [data-knowledge-supply-unfold]")?.dataset.knowledgeSupplyUnfold ?? "",
      };
      const vulnerabilityKey = `${KNOWLEDGE_SUPPLY_KEYS.vulnerability}${vulnerability.id}`;
      selectKnowledgeNode(view, vulnerabilityKey);
      await new Promise((done) => requestAnimationFrame(done));
      const routed = [];
      const openWas = window.__ANSWER__.open_url;
      window.__ANSWER__.open_url = (args) => { routed.push(args.url); return null; };
      const browserWas = browserPrefs.open_links_in_app;
      browserPrefs.open_links_in_app = false;
      view.querySelector(".knowledge-supply-osv")?.click();
      await new Promise((done) => setTimeout(done, 20));
      browserPrefs.open_links_in_app = browserWas;
      window.__ANSWER__.open_url = openWas;
      const paths = [...view.querySelectorAll(".knowledge-supply-paths .knowledge-supply-path")]
        .map((row) => [...row.querySelectorAll("[data-knowledge-key]")].map((one) => one.dataset.knowledgeKey));
      const severityRow = KNOWLEDGE_SUPPLY_SEVERITIES.find((row) => row.id === vulnerability.severity);
      const card = {
        section: shown(".knowledge-inspector-supply"),
        title: view.querySelector(".knowledge-card .knowledge-inspector-title")?.textContent ?? "",
        aliases: said("aliases"), severity: said("severity"), summary: view.querySelector(".knowledge-supply-summary")?.textContent ?? "",
        fixed: said("fixed"),
        affected: listKeys(".knowledge-supply-affected"),
        wantedAffected: answer.edges.filter((edge) => edge.kind === "affects" && edge.from === affectsEdge.from)
          .map((edge) => `${KNOWLEDGE_SUPPLY_KEYS.component}${answer.components[edge.to].id}`),
        paths,
        wantedPaths: answer.edges.filter((edge) => edge.kind === "affects" && edge.from === affectsEdge.from)
          .map((edge) => expected.paths[answer.components[edge.to].id].map((id) => `${KNOWLEDGE_SUPPLY_KEYS.component}${id}`)),
        routed,
        severityWord: severityRow ? t(severityRow.key, severityRow.word) : "",
        vulnerability,
        pageDoors: [".knowledge-inspector-open", ".knowledge-inspector-request", ".knowledge-inspector-link"]
          .filter((selector) => shown(selector)),
      };
      selectKnowledgeNode(view, null);
      return { component, member, card, target };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reference: supplyReferenceSource });
  const cardDetail = JSON.stringify(cards);
  ok("supply card: a component names its ecosystem's registry, version, origin, dependency count, lockfiles and the vulnerabilities that affect it, and no page door stands",
    !cards.thrown && cards.component.section
      && cards.component.title === `${cards.target.name}@${cards.target.version}`
      && cards.component.ecosystem.includes("crates.io") && cards.component.version === cards.target.version
      && cards.component.origin !== "" && cards.component.dependencies.includes(String(cards.component.wantedDependencies))
      && cards.component.lockfiles.includes("Cargo.lock")
      && JSON.stringify(cards.component.vulnerabilities.sort()) === JSON.stringify(cards.component.wantedVulnerabilities.sort())
      && cards.component.pageDoors.length === 0,
    cardDetail);
  ok("supply card: a folded member says how many components it folds and carries the unfold door",
    !cards.thrown && cards.member.folded !== null && cards.member.folded.includes(String(cards.member.wantedFolded))
      && cards.member.unfold !== "",
    cardDetail);
  ok("supply card: a vulnerability names its id, aliases, severity word and score, summary, fixed versions, affected components and each path from a member, and its OSV link goes through the window's link door",
    !cards.thrown && cards.card.section && cards.card.title === cards.card.vulnerability.id
      && cards.card.vulnerability.aliases.every((alias) => cards.card.aliases.includes(alias))
      && cards.card.severity.includes(cards.card.severityWord)
      && (cards.card.vulnerability.score === null || cards.card.severity.includes(String(cards.card.vulnerability.score)))
      && cards.card.summary === cards.card.vulnerability.summary
      && cards.card.vulnerability.fixed.every((row) => cards.card.fixed.includes(row.version))
      && JSON.stringify(cards.card.affected.sort()) === JSON.stringify(cards.card.wantedAffected.sort())
      && JSON.stringify(cards.card.paths) === JSON.stringify(cards.card.wantedPaths)
      && cards.card.routed.length === 1 && cards.card.routed[0] === cards.card.vulnerability.url
      && cards.card.pageDoors.length === 0,
    cardDetail);

  /* 5. 펼치기 — 멤버의 접힌 구성요소가 서고, 그 멤버의 「+N」이 사라진다. */
  const unfold = await page.evaluate(async ({ reference }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const answer = knowledgeSupplyAnswer;
      const before = eval(reference)(answer);
      const memberRow = answer.components.find((row) => row.member && (before.folded[row.id] ?? 0) > 0);
      const key = `${KNOWLEDGE_SUPPLY_KEYS.component}${memberRow.id}`;
      selectKnowledgeNode(view, key);
      await frame();
      view.querySelector(`.knowledge-inspector-supply [data-knowledge-supply-unfold]`).click();
      for (let round = 0; round < 60; round += 1) {
        await frame();
        const model = knowledgeLayouts.get(view).model;
        if (model.kinds.filter((kind) => kind === "component").length > before.visible.length) break;
      }
      const after = eval(reference)(answer, { unfolded: [memberRow.id] });
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const seat = model.keys.indexOf(key);
      const drawn = model.keys.filter((one, at) => model.kinds[at] === "component").sort();
      const result = {
        drawn: drawn.length,
        wanted: after.visible.length,
        same: JSON.stringify(drawn) === JSON.stringify(after.visible.map((id) => `${KNOWLEDGE_SUPPLY_KEYS.component}${id}`).sort()),
        grew: drawn.length - before.visible.length,
        foldedBefore: before.folded[memberRow.id],
        foldedNow: layout.folded === null ? 0 : layout.folded[seat],
        stillSelected: knowledgeSelectedKey === key,
      };
      /* 되접는다 — 같은 문이 다시 접는다. */
      view.querySelector(`.knowledge-inspector-supply [data-knowledge-supply-unfold]`)?.click();
      for (let round = 0; round < 60; round += 1) {
        await frame();
        if (knowledgeLayouts.get(view).model.kinds.filter((kind) => kind === "component").length === before.visible.length) break;
      }
      result.refolded = knowledgeLayouts.get(view).model.kinds.filter((kind) => kind === "component").length === before.visible.length;
      selectKnowledgeNode(view, null);
      return result;
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reference: supplyReferenceSource });
  ok("supply lens G7: 펼치기 on a member stands exactly the components it folded, its +N leaves, and the same door folds them again",
    !unfold.thrown && unfold.same && unfold.grew === unfold.foldedBefore && unfold.foldedNow === 0
      && unfold.stillSelected && unfold.refolded,
    JSON.stringify(unfold));

  /* 6. 유령 링크 — 이름이 글자 그대로 같을 때만 구성요소로. */
  const ghosts = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const answer = window.__buildVaultGraph__({ path: "/vault", sources: false },
        { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"] });
      const ghostAt = answer.graph.nodes.map((node, at) => [node, at]).filter(([node]) => node.kind === "ghost");
      const rename = (at, name, kind) => {
        answer.graph.nodes[at].id = `ghost:${name}`;
        answer.graph.nodes[at].title = name;
        for (const edge of answer.graph.edges) if (edge.to === at && kind) edge.kind = kind;
      };
      rename(ghostAt[0][1], "tauri", "depends_on");
      rename(ghostAt[1][1], "Serde", null);
      rename(ghostAt[2][1], "serde ", null);
      const linkedFrom = answer.graph.edges.filter((edge) => edge.to === ghostAt[0][1])
        .map((edge) => answer.graph.nodes[edge.from].id);
      knowledgeReport = answer;
      knowledgeAskedAt = Date.now();
      await paintKnowledgeView();
      const model = knowledgeLayouts.get(view).model;
      const tauri = knowledgeSupplyAnswer.components.filter((row) => row.name === "tauri");
      const tauriKeys = tauri.map((row) => `${KNOWLEDGE_SUPPLY_KEYS.component}${row.id}`);
      const joined = [];
      for (let at = 0; at < model.edgeCount; at += 1) {
        if (tauriKeys.includes(model.keys[model.to[at]]) && model.kinds[model.from[at]] === "page") {
          joined.push({ from: model.keys[model.from[at]], kind: KNOWLEDGE_EDGE_KINDS[model.kind[at]] });
        }
      }
      return {
        redirectedGhost: model.keys.includes("ghost:tauri"),
        tauriDrawn: tauriKeys.every((key) => model.keys.includes(key)),
        tauriCount: tauri.length,
        joined,
        linkedFrom,
        caseGhost: model.keys.includes("ghost:Serde"),
        spaceGhost: model.keys.includes("ghost:serde "),
      };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  ok("supply lens: a ghost link whose name is exactly a component's name stands as that component, keeping the page's relation, while a near miss (case, space) stays a ghost",
    !ghosts.thrown && ghosts.tauriCount === 1 && !ghosts.redirectedGhost && ghosts.tauriDrawn
      && ghosts.joined.length === ghosts.linkedFrom.length && ghosts.joined.length > 0
      && ghosts.joined.every((row) => row.kind === "depends_on" && ghosts.linkedFrom.includes(row.from))
      && ghosts.caseGhost && ghosts.spaceGhost,
    JSON.stringify(ghosts));

  /* 7. 검색 토큰 — 기존 문법의 `kind:`와 새 `sev:`. 디밍만 하고 자리는 그대로다. */
  const tokens = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const find = view.querySelector(".knowledge-query");
      const lit = async (query) => {
        find.value = query;
        find.dispatchEvent(new Event("input", { bubbles: true }));
        await paintKnowledgeView();
        const layout = knowledgeLayouts.get(view);
        const seats = [];
        for (let at = 0; at < layout.count; at += 1) if (layout.searchMatch[at] === 1) seats.push(layout.model.keys[at]);
        return seats.sort();
      };
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const x = Float32Array.from(layout.x);
      const ofKind = (kind) => model.keys.filter((key, at) => model.kinds[at] === kind).sort();
      const answer = knowledgeSupplyAnswer;
      const severe = (word) => answer.vulnerabilities.filter((row) => row.severity === word)
        .map((row) => `${KNOWLEDGE_SUPPLY_KEYS.vulnerability}${row.id}`).sort();
      const result = {
        component: JSON.stringify(await lit("kind:component")) === JSON.stringify(ofKind("component")),
        vulnerability: JSON.stringify(await lit("kind:vulnerability")) === JSON.stringify(ofKind("vulnerability")),
        high: await lit("sev:high"),
        wantedHigh: severe("high"),
        critical: await lit("SEV:Critical"),
        wantedCritical: severe("critical"),
        mixed: await lit("sev:medium 2026-0008"),
        wantedMixed: severe("medium").filter((key) => key.endsWith("2026-0008")),
        chip: (() => {
          const chip = view.querySelector('.knowledge-token-chip[data-token="sev:"]');
          return chip !== null && !chip.hidden;
        })(),
      };
      result.unmoved = knowledgeLayouts.get(view).x.every((value, at) => value === x[at]);
      find.value = "";
      find.dispatchEvent(new Event("input", { bubbles: true }));
      await paintKnowledgeView();
      return result;
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  ok("supply search: kind:component and kind:vulnerability light their kind, sev:<severity> (any case) lights the vulnerabilities of that severity, free words still narrow it, and nothing moves",
    !tokens.thrown && tokens.component && tokens.vulnerability && tokens.high.length > 0
      && JSON.stringify(tokens.high) === JSON.stringify(tokens.wantedHigh)
      && JSON.stringify(tokens.critical) === JSON.stringify(tokens.wantedCritical)
      && tokens.wantedMixed.length === 1 && JSON.stringify(tokens.mixed) === JSON.stringify(tokens.wantedMixed)
      && tokens.chip && tokens.unmoved,
    JSON.stringify(tokens));

  /* 8. 조회 상태 — 답의 낱말(`state`·`reason`)을 그대로 읽는 한 줄, 문의 거절은 그 문장 그대로,
   * 「다시 확인」은 `refresh: true`. */
  const lookups = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const line = () => view.querySelector(".knowledge-supply-lookup");
      const spec = window.__SUPPLY__;
      const rows = [];
      for (const lookup of [
        { state: "fresh", reason: null, checkedAt: 1789628477294 },
        { state: "cached", reason: null, checkedAt: 1789628477294 },
        { state: "failed", reason: "offline", checkedAt: null },
        { state: "failed", reason: "timeout", checkedAt: 1789628477294 },
        { state: "failed", reason: "http_503", checkedAt: null },
        { state: "failed", reason: "bad_answer", checkedAt: null },
      ]) {
        window.__SUPPLY__ = { ...spec, lookup };
        await refreshKnowledgeSupply({ force: true });
        rows.push({ lookup, state: line()?.dataset.lookupState ?? "", reason: line()?.dataset.lookupReason ?? "",
          said: line()?.textContent ?? "", when: lookup.checkedAt === null ? "" : knowledgeWhen(lookup.checkedAt) });
      }
      window.__SUPPLY__ = spec;
      window.__SUPPLY_FAIL__ = "공급망은 워크스페이스 폴더에서 읽습니다";
      await refreshKnowledgeSupply({ force: true });
      const refused = { state: line()?.dataset.lookupState ?? "", said: line()?.textContent ?? "",
        components: knowledgeLayouts.get(view).model.kinds.includes("component") };
      delete window.__SUPPLY_FAIL__;
      const asked = (window.__SUPPLY_ASKED__ ?? []).length;
      view.querySelector(".knowledge-supply-recheck")?.click();
      for (let round = 0; round < 60 && (window.__SUPPLY_ASKED__ ?? []).length === asked; round += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      for (let round = 0; round < 60 && knowledgeSupplyAsking; round += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const recheck = (window.__SUPPLY_ASKED__ ?? []).at(-1) ?? {};
      return { rows, refused, recheck, final: line()?.dataset.lookupState ?? "" };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const lookupDetail = JSON.stringify(lookups);
  ok("supply lookup: the card line reads the answer's own state and reason tokens — fresh, cached, offline, timeout, http_<status>, bad_answer — each in its own words, with the status number and the last check time when the answer has them",
    !lookups.thrown && lookups.rows.length === 6
      && lookups.rows.every((row) => row.state === row.lookup.state && row.reason === (row.lookup.reason ?? "")
        && row.said.trim() !== "" && (row.when === "" || row.said.includes(row.when)))
      && new Set(lookups.rows.map((row) => row.said)).size === 6
      && lookups.rows.find((row) => row.reason === "http_503").said.includes("503"),
    lookupDetail);
  ok("supply lookup: a refused call is said in the backend's own sentence without dropping the components already standing, and 다시 확인 asks again with refresh true",
    !lookups.thrown && lookups.refused.state === "error"
      && lookups.refused.said.includes("공급망은 워크스페이스 폴더에서 읽습니다") && lookups.refused.components
      && lookups.recheck.refresh === true && lookups.final === "fresh",
    lookupDetail);

  /* 9. 시야와 언어 — 렌즈는 시야에 실리고, 판의 새 낱말은 다섯 언어에서 한국어로 새지 않는다. */
  const scenesAndWords = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const layout = knowledgeLayouts.get(view);
      const scene = captureCurrentKnowledgeScene("supply", view, layout);
      const offScene = { ...scene, lens: { ...scene.lens, supply: false } };
      await restoreKnowledgeScene(view, offScene);
      const off = { shown: knowledgeSupplyShown, kinds: [...new Set(knowledgeLayouts.get(view).model.kinds)] };
      const asked = (window.__SUPPLY_ASKED__ ?? []).length;
      await restoreKnowledgeScene(view, scene);
      for (let round = 0; round < 60 && !knowledgeLayouts.get(view).model.kinds.includes("component"); round += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const on = { shown: knowledgeSupplyShown, askedAgain: (window.__SUPPLY_ASKED__ ?? []).length > asked,
        kinds: [...new Set(knowledgeLayouts.get(view).model.kinds)] };
      const hangul = /[ᄀ-ᇿ㄰-㆏가-힯]/u;
      const localeWas = locale;
      const spoken = {};
      const supplyNode = knowledgeLayouts.get(view).model.keys.find((key) => key.startsWith(KNOWLEDGE_SUPPLY_KEYS.vulnerability));
      try {
        for (const code of ["en", "ja", "zh", "es"]) {
          locale = code;
          applyLocale();
          await paintKnowledgeView();
          const words = [];
          const read = (root) => {
            if (!root) return;
            if (hangul.test(root.textContent)) words.push(`text:${root.className}:${root.textContent.slice(0, 80)}`);
            for (const one of [root, ...root.querySelectorAll("*")]) {
              for (const name of ["aria-label", "data-tip", "title"]) {
                const value = one.getAttribute?.(name);
                if (value && hangul.test(value)) words.push(`${name}:${value}`);
              }
            }
          };
          read(view.querySelector(".knowledge-overview-supply"));
          read(view.querySelector('[data-knowledge-flag="supply"]'));
          for (const row of view.querySelectorAll(".knowledge-legend [data-legend-supply], .knowledge-legend [data-edge-kind=\"affects\"]")) read(row);
          selectKnowledgeNode(view, supplyNode);
          read(view.querySelector(".knowledge-inspector-supply"));
          selectKnowledgeNode(view, null);
          spoken[code] = words;
        }
      } finally {
        locale = localeWas;
        applyLocale();
        await paintKnowledgeView();
      }
      return { lens: scene.lens.supply, off, on, spoken };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  ok("supply scenes: a saved view carries the lens, restoring it off leaves the vault alone and restoring it on asks again and stands the supply chain",
    !scenesAndWords.thrown && scenesAndWords.lens === true && scenesAndWords.off.shown === false
      && !scenesAndWords.off.kinds.includes("component") && scenesAndWords.on.shown === true
      && scenesAndWords.on.askedAgain && scenesAndWords.on.kinds.includes("component"),
    JSON.stringify(scenesAndWords));
  ok("supply words: the lens button, the card's supply section, the legend rows and the component and vulnerability cards speak English, Japanese, Chinese and Spanish without a Korean word",
    !scenesAndWords.thrown && Object.keys(scenesAndWords.spoken).length === 4
      && Object.values(scenesAndWords.spoken).every((words) => words.length === 0),
    JSON.stringify(scenesAndWords.spoken ?? scenesAndWords));

  /* 10. 대비와 키보드·움직임. */
  const reach = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
      const channels = (color) => {
        const sentinel = "#010203";
        paint.fillStyle = sentinel;
        paint.fillStyle = color;
        if (paint.fillStyle === sentinel) return null;
        paint.clearRect(0, 0, 1, 1);
        paint.fillRect(0, 0, 1, 1);
        const held = paint.getImageData(0, 0, 1, 1).data;
        return [held[0] / 255, held[1] / 255, held[2] / 255];
      };
      const luminance = (color) => {
        const held = channels(color);
        if (!held) return null;
        return held.map((one) => (one <= 0.04045 ? one / 12.92 : ((one + 0.055) / 1.055) ** 2.4))
          .reduce((sum, one, at) => sum + one * [0.2126, 0.7152, 0.0722][at], 0);
      };
      const ratio = (left, right) => {
        const a = luminance(left);
        const b = luminance(right);
        return a === null || b === null ? 0 : (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      };
      const measure = () => {
        const ground = getComputedStyle(view.querySelector(".knowledge-canvas")).backgroundColor;
        const style = getComputedStyle(view);
        const inks = {};
        for (const row of KNOWLEDGE_SUPPLY_ECOSYSTEMS) inks[`ecosystem-${row.id}`] = style.getPropertyValue(`--knowledge-ecosystem-${row.id}`).trim();
        for (const row of KNOWLEDGE_SUPPLY_SEVERITIES) inks[`severity-${row.id}`] = style.getPropertyValue(`--knowledge-severity-${row.id}`).trim();
        const affects = view.querySelector(".knowledge-edge.kind-affects");
        inks.affects = affects ? getComputedStyle(affects).stroke : "";
        return Object.entries(inks).map(([name, ink]) => ({ name, ink, ratio: Math.round(ratio(ink, ground) * 100) / 100 }));
      };
      const dark = measure();
      setTheme("light");
      await new Promise((done) => setTimeout(done, 700));
      const light = measure();
      setTheme("dark");
      await new Promise((done) => setTimeout(done, 700));
      /* 키보드: 카드의 공급망 목록은 진짜 단추이고 ↓가 줄을 옮긴다. */
      const model = knowledgeLayouts.get(view).model;
      const vulnerable = model.keys.find((key, at) => model.kinds[at] === "component"
        && knowledgeSupplyAnswer.edges.some((edge) => edge.kind === "affects"
          && `${KNOWLEDGE_SUPPLY_KEYS.component}${knowledgeSupplyAnswer.components[edge.to].id}` === key));
      const memberKey = model.keys.find((key, at) => model.kinds[at] === "component" && model.member?.[at] === 1);
      selectKnowledgeNode(view, memberKey);
      const outgoing = [...view.querySelectorAll(".knowledge-relations-outgoing .knowledge-inspector-row")];
      outgoing[0]?.focus();
      outgoing[0]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
      const moved = outgoing.length > 1 && document.activeElement === outgoing[1];
      selectKnowledgeNode(view, vulnerable);
      const vulnerabilityRows = [...view.querySelectorAll(".knowledge-supply-vulnerabilities .knowledge-inspector-row")];
      const buttons = vulnerabilityRows.length > 0 && vulnerabilityRows.every((row) => row.localName === "button");
      selectKnowledgeNode(view, null);
      return { dark, light, moved, buttons };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  ok("supply inks: both ecosystems, the six severity words and the affects line keep three-to-one contrast on the graph ground in both themes",
    !reach.thrown && [reach.dark, reach.light].every((theme) => theme.length === 2 + 6 + 1
      && theme.every((row) => row.ink !== "" && row.ratio >= 3)),
    JSON.stringify(reach));
  ok("supply keyboard: a component card's rows are real buttons that ↓ walks, like every other inspector list",
    !reach.thrown && reach.moved && reach.buttons,
    JSON.stringify({ moved: reach.moved, buttons: reach.buttons }));

  await page.emulateMedia({ reducedMotion: "reduce" });
  const still = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      view.querySelector(".knowledge-lens-toggle").click();
      view.querySelector('[data-knowledge-flag="supply"]').click();
      await new Promise((done) => requestAnimationFrame(done));
      const offAnimations = document.getAnimations().filter((one) => one.playState === "running").length;
      view.querySelector('[data-knowledge-flag="supply"]').click();
      for (let round = 0; round < 60 && !knowledgeLayouts.get(view).model.kinds.includes("component"); round += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const onAnimations = document.getAnimations().filter((one) => one.playState === "running").length;
      view.querySelector(".knowledge-lens-toggle").click();
      return { offAnimations, onAnimations, left: knowledgeLayouts.get(view).left };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  await page.emulateMedia({ reducedMotion: null });
  ok("supply motion: with reduced motion the lens turns off and on without a running animation and the picture seats at once",
    !still.thrown && still.offAnimations === 0 && still.onAnimations === 0 && still.left === 0,
    JSON.stringify(still));

  /* 끝: 렌즈를 끄고 볼트만 남긴다 — 뒤의 케이스는 볼트의 그림을 잰다. */
  await page.evaluate(async () => {
    try {
      knowledgeSupplyShown = false;
      delete window.__SUPPLY__;
      await paintKnowledgeView();
    } catch {
      // 렌즈가 없는 제품 — 되돌릴 것이 없다.
    }
  });
}

/* 천 개의 구성요소의 첫 그림 (설계 §2 G7). 게이트의 판(크로미엄 SVG·SwiftShader GL)에서는 수를
 * 적고 계약(겹침 0·접힘의 합)을 묻는다; 진짜 GPU의 시간은 `knowledge-gpu.mjs`가 WebKit에서 잰다.
 * 볼트는 볼트 모양 장면(359쪽·유령 53) 그대로 — 이 저장소의 실제 모양(볼트 412·구성요소 1,102). */
export const SUPPLY_SCENE = Object.freeze({ name: "vault+supply-1k", pages: 359, ghosts: 53, linksPer: 3 });

export async function measureKnowledgeSupplyScene(page, ok, { painter = "svg", rounds = 1, gate = null } = {}) {
  const seatWas = page.viewportSize();
  await page.setViewportSize({ width: 1998, height: 1069 });
  const row = await page.evaluate(async ({ reset, scene, supply, hand, rounds: count, reference }) => {
    try {
      eval(reset)();
      const pinned = knowledgePainterKind;
      knowledgePainterKind = hand;
      const view = document.querySelector(".knowledge-view:not([hidden])");
      setKnowledgeMode(view, "global", { paint: false });
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const vault = window.__buildVaultGraph__({ path: `/scene/${scene.name}`, sources: false },
        { pages: scene.pages, ghosts: scene.ghosts, linksPer: scene.linksPer, tags: ["core", "reading", "tools"] });
      const answer = window.__buildSupplyAnswer__({ root: "/repo" }, supply);
      const expected = eval(reference)(answer);
      const samples = [];
      let last = null;
      for (let round = 0; round < count; round += 1) {
        knowledgeLayouts.delete(view);
        const host = view.querySelector(".knowledge-nodes");
        host.dataset.knowledgeSignature = "";
        host.dataset.knowledgeVault = "";
        host.replaceChildren();
        view.querySelector(".knowledge-edges").replaceChildren();
        knowledgeReport = vault;
        knowledgeAskedAt = Date.now();
        knowledgeSupplyAnswer = answer;
        knowledgeSupplyShown = true;
        knowledgeSupplyAskedAt = Date.now();
        knowledgeSupplyAskedRoot = activeWorktreePath ?? null;
        const began = performance.now();
        await paintKnowledgeView();
        await frame();
        samples.push(performance.now() - began);
        for (let wait = 0; wait < 1200 && (knowledgeLayouts.get(view)?.left ?? 0) > 0; wait += 1) await frame();
        last = knowledgeLayouts.get(view);
      }
      const layout = last;
      const model = layout.model;
      const boxes = window.__knowledgeLabelBoxes__(view);
      let folded = 0;
      for (let at = 0; at < layout.count; at += 1) folded += layout.folded === null ? 0 : layout.folded[at];
      const result = {
        name: "vault+supply-1k",
        painter: knowledgePainterFor(view).id,
        components: answer.components.length,
        drawnComponents: model.kinds.filter((kind) => kind === "component").length,
        wantedDrawn: expected.visible.length,
        vulnerabilities: model.kinds.filter((kind) => kind === "vulnerability").length,
        folded,
        wantedFolded: Object.values(expected.folded).reduce((sum, one) => sum + one, 0),
        nodes: layout.count,
        edges: model.edgeCount,
        firstPaintMs: Math.round(samples.slice().sort((a, b) => a - b)[Math.floor(samples.length / 2)]),
        firstPaintSamples: samples.map((one) => Math.round(one)),
        labelsShown: boxes.length,
        labelOverlaps: window.__knowledgeOverlapPairs__(boxes, 0.5),
        draws: knowledgePainterFor(view).id === "svg"
          ? view.querySelectorAll(".knowledge-clusters *, .knowledge-edges *, .knowledge-nodes *").length
          : layout.paintStats.draws,
      };
      knowledgeSupplyShown = false;
      knowledgeSupplyAnswer = null;
      knowledgePainterKind = pinned;
      knowledgeReport = null;
      knowledgeAskedAt = 0;
      await paintKnowledgeView();
      return result;
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reset: RESET, scene: SUPPLY_SCENE, supply: SUPPLY_THOUSAND, hand: painter, rounds, reference: supplyReferenceSource });
  if (seatWas) await page.setViewportSize(seatWas);
  ok(`${painter}: a thousand components stand folded — members, direct dependencies and vulnerable paths only, the rest counted on the members, and no two labels overlap`,
    !row.thrown && row.painter === painter && row.components >= 1000
      && row.drawnComponents === row.wantedDrawn && row.drawnComponents < row.components
      && row.folded === row.wantedFolded && row.vulnerabilities === SUPPLY_THOUSAND.vulnerabilities
      && row.labelOverlaps === 0 && row.labelsShown > 0,
    JSON.stringify(row));
  if (gate !== null) {
    ok(`${painter}: the first picture of the vault with a thousand components takes at most ${gate} ms`,
      !row.thrown && row.firstPaintMs <= gate, JSON.stringify(row));
  }
  console.log(`METRIC knowledge supply scene (${painter}): ${JSON.stringify(row)}`);
  return row;
}

/* 두 손의 대조 — GL이 서는 판에서. 같은 답·같은 자리에서 SVG와 GL이 같은 점을 세우고, 같은 이름표
 * 자리를 쥐고, 같은 「+N」을 말하고, 구성요소·취약점의 한가운데를 같은 색으로 칠하는가. 칠은 점을
 * 키워 한 줄로 세우고 찍어서 읽는다(P1 G3 픽셀 대조와 같은 길, 한가운데 색만). */
const SUPPLY_PIXELS = Object.freeze({ radius: 30, pitch: 90, centreLevels: 3 });

export async function measureKnowledgeSupplyParity(glPage, ok) {
  await glPage.emulateMedia({ reducedMotion: "reduce" });
  const hands = await glPage.evaluate(async ({ reset, supply, tune }) => {
    try {
      if (!knowledgeGlSupported()) return { skip: "this browser has no WebGL2 context" };
      eval(reset)();
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      setKnowledgeMode(view, "global", { paint: false });
      window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 2, tags: ["core", "reading"] };
      knowledgeReport = window.__buildVaultGraph__({ path: "/vault", sources: false }, window.__VAULT__);
      knowledgeAskedAt = Date.now();
      knowledgeSupplyAnswer = window.__buildSupplyAnswer__({ root: "/repo" }, supply);
      knowledgeSupplyShown = true;
      knowledgeSupplyAskedAt = Date.now();
      knowledgeSupplyAskedRoot = activeWorktreePath ?? null;
      const read = async (hand) => {
        knowledgePainterKind = hand;
        await paintKnowledgeView();
        for (let round = 0; round < 1200 && (knowledgeLayouts.get(view)?.left ?? 0) > 0; round += 1) await frame();
        await frame();
        const layout = knowledgeLayouts.get(view);
        const model = layout.model;
        const drawn = [];
        const named = [];
        const folds = {};
        for (let at = 0; at < layout.count; at += 1) {
          if (layout.drawn !== null && layout.drawn[at] === 0) continue;
          drawn.push(model.keys[at]);
          if (layout.labelShown[at] === 1) named.push(model.keys[at]);
          const fold = layout.folded === null ? 0 : layout.folded[at];
          if (fold > 0) folds[model.keys[at]] = fold;
        }
        const words = hand === "gl"
          ? [...view.querySelectorAll(".knowledge-gl-label:not([hidden])")].map((one) => one.textContent)
          : [...view.querySelectorAll(".knowledge-node.is-named .knowledge-label")].map((one) => one.textContent);
        return { hand: knowledgePainterFor(view).id, drawn: drawn.sort(), named: named.sort(), folds,
          points: layout.paintStats.points, words: words.sort(), elements: view.querySelectorAll(".knowledge-nodes .knowledge-node").length };
      };
      const svg = await read("svg");
      const gl = await read("gl");
      return { svg, gl };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { reset: RESET, supply: SUPPLY_SMALL, tune: SUPPLY_PIXELS });
  if (hands.skip) {
    console.log(`SKIP supply lens painter comparison: ${hands.skip}`);
    await glPage.emulateMedia({ reducedMotion: null });
    return;
  }
  const centreCase = "supply two hands: each ecosystem and member state and each severity fills its centre with the same colour in SVG and GL";
  ok("supply two hands: SVG and GL stand the same supply points, give the grid the same named seats and say the same +N folds",
    !hands.thrown && hands.svg.hand === "svg" && hands.gl.hand === "gl"
      && JSON.stringify(hands.svg.drawn) === JSON.stringify(hands.gl.drawn)
      && JSON.stringify(hands.svg.named) === JSON.stringify(hands.gl.named)
      && JSON.stringify(hands.svg.folds) === JSON.stringify(hands.gl.folds) && Object.keys(hands.gl.folds).length > 0
      && hands.gl.elements === 0 && hands.gl.points === hands.gl.drawn.length
      && Object.keys(hands.gl.folds).every((key) => hands.gl.named.includes(key))
      && Object.values(hands.gl.folds).every((fold) => hands.gl.words.some((word) => word.endsWith(` +${fold}`))
        && hands.svg.words.some((word) => word.endsWith(`+${fold}`))),
    JSON.stringify(hands));

  /* 한가운데의 색: 구성요소 넷(Cargo·npm × 멤버·아님)과 심각도 여섯. */
  if (hands.thrown) {
    ok(centreCase, false, JSON.stringify(hands));
    await glPage.emulateMedia({ reducedMotion: null });
    return;
  }
  /* 자리 잡기도 판 안의 이름을 읽으므로 감싼다 — 렌즈가 없는 제품(전·후를 짝지어 재는 판의 「전」)에서
   * 이 블록이 던지면 하네스가 통째로 멈춘다. */
  const seatAll = (hand) => glPage.evaluate(async ({ next, tune }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      knowledgePainterKind = next;
      await paintKnowledgeView();
      const layout = knowledgeLayouts.get(view);
      layout.left = 0;
      const model = layout.model;
      const pick = [];
      const wanted = [
        ...["cargo", "npm"].flatMap((ecosystem) => [true, false].map((member) => ({ kind: "component", ecosystem, member }))),
        ...KNOWLEDGE_SUPPLY_SEVERITIES.map((row) => ({ kind: "vulnerability", severity: row.id })),
      ];
      for (const want of wanted) {
        const at = model.keys.findIndex((key, seat) => model.kinds[seat] === want.kind
          && (want.kind === "component"
            ? KNOWLEDGE_SUPPLY_ECOSYSTEMS[model.ecosystem[seat]]?.id === want.ecosystem && (model.member[seat] === 1) === want.member
            : KNOWLEDGE_SUPPLY_SEVERITIES[model.severity[seat]]?.id === want.severity && model.informational[seat] === 0));
        pick.push({ ...want, at });
      }
      const placed = pick.filter((one) => one.at >= 0);
      /* 고른 점만 줄에 세우고 나머지는 멀리 — 한가운데를 남의 선이 지나가지 않게. */
      for (let at = 0; at < layout.count; at += 1) {
        layout.x[at] = -100000 - at;
        layout.y[at] = -100000;
      }
      placed.forEach((one, index) => {
        layout.x[one.at] = index * tune.pitch;
        layout.y[one.at] = 0;
      });
      layout.geometryRevision += 1;
      knowledgeBounds(layout);
      layout.bounds.minX = -tune.pitch;
      layout.bounds.maxX = placed.length * tune.pitch;
      layout.bounds.minY = -tune.pitch;
      layout.bounds.maxY = tune.pitch;
      fitKnowledgeGraph(view, layout);
      await frame();
      await frame();
      const box = view.querySelector(".knowledge-canvas").getBoundingClientRect();
      return placed.map((one) => ({ ...one, x: box.left + layout.project.screenX(one.at),
        y: box.top + layout.project.screenY(one.at) }));
    } catch {
      return [];
    }
  }, { next: hand, tune: SUPPLY_PIXELS });
  const setup = await glPage.evaluate(({ tune }) => {
    const view = document.querySelector(".knowledge-view:not([hidden])");
    for (const name of ["--knowledge-node-radius-min", "--knowledge-node-radius-max",
      "--knowledge-core-radius", "--knowledge-major-radius"]) view.style.setProperty(name, String(tune.radius));
    knowledgeTunings.delete(view);
    knowledgeLayouts.delete(view);
    const style = document.createElement("style");
    style.id = "knowledge-supply-pixels";
    style.textContent = ".knowledge-view .knowledge-label, .knowledge-view .knowledge-gl-label, .knowledge-view .knowledge-edges,"
      + " .knowledge-view .knowledge-clusters { visibility: hidden !important; }";
    document.head.appendChild(style);
    return true;
  }, { tune: SUPPLY_PIXELS });
  const shots = {};
  const seats = {};
  if (setup) {
    for (const hand of ["svg", "gl"]) {
      seats[hand] = await seatAll(hand);
      const png = await glPage.screenshot({ type: "png" });
      shots[hand] = png.toString("base64");
    }
  }
  const centres = await glPage.evaluate(async ({ shots, seats, tune }) => {
    try {
      const decode = async (png) => {
        const bytes = Uint8Array.from(atob(png), (glyph) => glyph.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }),
          { colorSpaceConversion: "none", premultiplyAlpha: "none" });
        const surface = new OffscreenCanvas(bitmap.width, bitmap.height);
        const brush = surface.getContext("2d", { willReadFrequently: true });
        brush.drawImage(bitmap, 0, 0);
        return { wide: bitmap.width, scale: bitmap.width / window.innerWidth,
          data: brush.getImageData(0, 0, bitmap.width, bitmap.height).data };
      };
      const images = { svg: await decode(shots.svg), gl: await decode(shots.gl) };
      const at = (image, x, y) => {
        const seat = (Math.floor(y * image.scale) * image.wide + Math.floor(x * image.scale)) * 4;
        return [image.data[seat], image.data[seat + 1], image.data[seat + 2]];
      };
      if (seats.svg.length === 0 || seats.gl.length === 0) return { rows: [], seated: false };
      const rows = seats.svg.map((one) => {
        const twin = seats.gl.find((other) => other.at === one.at);
        const svg = at(images.svg, one.x, one.y);
        const gl = twin ? at(images.gl, twin.x, twin.y) : [0, 0, 0];
        return { kind: one.kind, ecosystem: one.ecosystem ?? "", member: one.member ?? "", severity: one.severity ?? "",
          svg, gl, same: svg.every((value, channel) => Math.abs(value - gl[channel]) <= tune.centreLevels) };
      });
      return { rows };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    } finally {
      document.getElementById("knowledge-supply-pixels")?.remove();
      const view = document.querySelector(".knowledge-view:not([hidden])");
      for (const name of ["--knowledge-node-radius-min", "--knowledge-node-radius-max",
        "--knowledge-core-radius", "--knowledge-major-radius"]) view?.style.removeProperty(name);
      if (view) {
        knowledgeTunings.delete(view);
        knowledgeLayouts.delete(view);
      }
      knowledgeSupplyShown = false;
      knowledgeSupplyAnswer = null;
      knowledgePainterKind = null;
      knowledgeReport = null;
      knowledgeAskedAt = 0;
      await paintKnowledgeView();
    }
  }, { shots, seats, tune: SUPPLY_PIXELS });
  await glPage.emulateMedia({ reducedMotion: null });
  ok(centreCase,
    !centres.thrown && centres.rows.length === 4 + 6 && centres.rows.every((row) => row.same),
    JSON.stringify(centres));
  console.log(`METRIC knowledge supply centres: ${JSON.stringify(centres.rows ?? centres)}`);
}
