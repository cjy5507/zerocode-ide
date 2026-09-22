/* ---- 지식 그래프의 코드 층 (t-5970, Graphify G2) ----------------------------------------
 *
 * 창의 몫은 셋이다: 렌즈를 켜고 끄는 것, 켜졌을 때 열린 워크스페이스의 층을 백엔드에 묻는 것
 * (`second_brain_graph { code, project }`), 백엔드가 접붙인 그림과 그 답의 수·거절을 그대로 서게
 * 하는 것. 언급을 풀고 인덱스를 읽는 일은 zo(`zo vault code`)와 core(`second_brain_code::graft`)의
 * 것이라 여기에 없다 — 픽스처(`knowledge-fixture.mjs`)는 그 답의 모양만 짓는다. */

const WORKSPACE = "/workspace/acme";

export async function testKnowledgeCode(page, ok) {
  const seen = await page.evaluate(async (workspace) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted) => {
        for (let round = 0; round < 300; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const codeCount = () => knowledgeLayouts.get(view).model.kinds
        .filter((kind) => kind === "code_file" || kind === "code_symbol").length;
      const heldWorktree = activeWorktreePath;
      const heldPainter = knowledgePainterKind;
      const heldLocale = locale;
      /* 점의 모양은 요소로 읽는다 — SVG 손을 못박는다(끝에서 놓는다). 한 줄은 카탈로그의 문장과
       * 견준다 — 폴백이 아니라 번역 표가 서는 언어로(끝에서 되돌린다). */
      knowledgePainterKind = "svg";
      locale = "en";
      knowledgeQuery = "";
      knowledgeTagsPicked.clear();
      knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
      knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
      knowledgeShowSources = false;
      knowledgeShowCode = false;
      knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
      setKnowledgeMode(view, "global", { paint: false });
      activeWorktreePath = workspace;
      const vault = { pages: 8, linksPer: 2, ghosts: 0, tags: ["core"], code: { files: 2, symbols: 3 } };
      window.__VAULT__ = vault;
      knowledgeAskedAt = 0;
      await refreshKnowledgeGraph({ force: true });
      const off = {
        asked: { ...window.__GRAPH_ASKED__ },
        code: codeCount(),
        legendHidden: [...view.querySelectorAll(".knowledge-legend [data-legend-code]")]
          .every((row) => row.hidden),
      };

      const flag = view.querySelector('[data-knowledge-flag="code"]');
      const asks = window.__GRAPH_ASKS__ ?? 0;
      flag.click();
      await until(() => (window.__GRAPH_ASKS__ ?? 0) > asks && knowledgeReport?.code !== undefined);
      await paintKnowledgeView();
      const layout = knowledgeLayouts.get(view);
      const shapeOf = (kind) => {
        const at = layout.model.kinds.indexOf(kind);
        return at < 0 ? "" : layout.nodeEls[at]?.querySelector(".knowledge-dot")?.dataset.shape ?? "";
      };
      const graph = knowledgeReport.graph;
      const codeEdges = graph.edges.filter((edge) => graph.nodes[edge.from].kind.startsWith("code_"));
      const answer = knowledgeReport.code;
      const on = {
        asked: { ...window.__GRAPH_ASKED__ },
        pressed: flag.getAttribute("aria-pressed"),
        files: layout.model.kinds.filter((kind) => kind === "code_file").length,
        symbols: layout.model.kinds.filter((kind) => kind === "code_symbol").length,
        fileShape: shapeOf("code_file"),
        symbolShape: shapeOf("code_symbol"),
        measured: codeEdges.length > 0 && codeEdges.every((edge) => edge.provenance === "measured"),
        legendShown: [...view.querySelectorAll(".knowledge-legend [data-legend-code]")]
          .every((row) => !row.hidden),
        tip: flag.dataset.tip ?? "",
        /* 한 줄은 답의 수를 그대로 싣는다 — 창이 센 수가 아니다. */
        wantTip: t("knowledge.codeStatus", "", {
          nodes: answer.grafted.nodes, edges: answer.grafted.edges,
          mentions: answer.mentions, resolved: answer.resolved,
        }),
      };

      /* 거절은 문장 그대로 선다. */
      const refusal = "zo를 찾을 수 없습니다";
      window.__VAULT__ = { ...vault, code: { error: refusal } };
      knowledgeAskedAt = 0;
      await refreshKnowledgeGraph({ force: true });
      await paintKnowledgeView();
      const refused = { tip: flag.dataset.tip ?? "", code: codeCount(), refusal };

      /* 열린 워크스페이스가 없으면 묻지 않고, 그 까닭을 말한다. */
      activeWorktreePath = null;
      window.__VAULT__ = vault;
      knowledgeAskedAt = 0;
      await refreshKnowledgeGraph({ force: true });
      await paintKnowledgeView();
      const bare = {
        project: window.__GRAPH_ASKED__?.project,
        code: codeCount(),
        tip: flag.dataset.tip ?? "",
        wantTip: t("knowledge.codeNoProject", ""),
      };

      /* 다시 끄면 층이 그림을 떠난다. */
      activeWorktreePath = workspace;
      flag.click();
      await until(() => window.__GRAPH_ASKED__?.code === false);
      await paintKnowledgeView();
      const offAgain = { code: codeCount(), pressed: flag.getAttribute("aria-pressed") };

      activeWorktreePath = heldWorktree;
      knowledgePainterKind = heldPainter;
      locale = heldLocale;
      applyLocale();
      knowledgeShowCode = false;
      await paintKnowledgeView();
      return { off, on, refused, bare, offAgain };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, WORKSPACE);
  const detail = JSON.stringify(seen);
  ok("t-5970 G2: the code lens is off by default — no code is asked for or drawn, and its legend rows stay hidden",
    !seen.thrown && seen.off.asked.code === false && seen.off.code === 0 && seen.off.legendHidden,
    detail);
  ok("t-5970 G2: turned on, the lens asks for the open workspace's layer and stands files as hexagons and definitions as crosses, every line of theirs measured",
    !seen.thrown && seen.on.asked.code === true && seen.on.asked.project === WORKSPACE
      && seen.on.pressed === "true" && seen.on.files === 2 && seen.on.symbols === 3
      && seen.on.fileShape === "hexagon" && seen.on.symbolShape === "cross"
      && seen.on.measured && seen.on.legendShown,
    detail);
  ok("t-5970 G2: the flag's sentence carries the backend's counts, its refusal verbatim, or why nothing was asked",
    !seen.thrown && seen.on.tip === seen.on.wantTip && seen.on.tip !== ""
      && seen.refused.tip.includes(seen.refused.refusal) && seen.refused.code === 0
      && seen.bare.project === null && seen.bare.code === 0 && seen.bare.tip === seen.bare.wantTip,
    detail);
  ok("t-5970 G2: turned off again, the layer leaves the picture",
    !seen.thrown && seen.offAgain.code === 0 && seen.offAgain.pressed === "false",
    detail);
}

/* 실측(t-5970): 이 기계의 볼트 규모(511쪽)와, 이 저장소에서 `zo vault code`가 답한 층의 규모(상한
 * 600점 — 파일 170·정의 430)를 픽스처로 세우고, 층을 켜지 않은 그림·켠 그림, 그리고 켠 그림과 점의
 * 수가 같은 페이지만의 그림(대조군)의 첫 그림과 가라앉는 동안의 최악 프레임 간격을 잰다. 세 장면을
 * 번갈아 `ROUNDS`번, 중앙값으로. 계약은 둘이다: 코드의 점은 같은 수의 페이지보다 비싸지 않고(첫 그림이
 * 대조군의 `SLACK` 안), 최악 간격은 천 쪽의 그림과 같은 예산(12 × 8 ms) 안이다. */
const CODE_SCENE = Object.freeze({ pages: 511, linksPer: 3, ghosts: 20, files: 170, symbols: 430 });
const ROUNDS = 3;
const SLACK = 1.25;
const FRAME_GAP_BUDGET_MS = 12 * 8;

export async function measureKnowledgeCodeScene(page, ok) {
  const seatWas = page.viewportSize();
  await page.setViewportSize({ width: 1998, height: 1069 });
  const rows = await page.evaluate(async ({ scene, workspace, rounds }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const heldPainter = knowledgePainterKind;
      knowledgePainterKind = "svg";
      setKnowledgeMode(view, "global", { paint: false });
      const code = scene.files + scene.symbols;
      const scenes = {
        vaultOnly: { pages: scene.pages, withCode: false },
        withCode: { pages: scene.pages, withCode: true },
        /* 켠 그림과 점의 수가 같은, 페이지만의 그림. */
        control: { pages: scene.pages + code, withCode: false },
      };
      const measure = async (name, one) => {
        const answer = window.__buildVaultGraph__(
          { path: `/scene/code-${name}`, sources: false, code: one.withCode, project: one.withCode ? workspace : null },
          { pages: one.pages, linksPer: scene.linksPer, ghosts: scene.ghosts, tags: ["core", "reading", "tools"],
            code: { files: scene.files, symbols: scene.symbols } });
        knowledgeLayouts.delete(view);
        const host = view.querySelector(".knowledge-nodes");
        host.dataset.knowledgeSignature = "";
        host.dataset.knowledgeVault = "";
        host.replaceChildren();
        view.querySelector(".knowledge-edges").replaceChildren();
        knowledgeShowCode = one.withCode;
        knowledgeReport = answer;
        knowledgeAskedAt = Date.now();
        const began = performance.now();
        await paintKnowledgeView();
        await frame();
        const firstPaint = performance.now() - began;
        const stamps = [performance.now()];
        for (let round = 0; round < 600; round += 1) {
          await frame();
          stamps.push(performance.now());
          if ((knowledgeLayouts.get(view)?.left ?? 0) === 0) break;
        }
        let worstGap = 0;
        for (let at = 1; at < stamps.length; at += 1) worstGap = Math.max(worstGap, stamps[at] - stamps[at - 1]);
        return {
          nodes: knowledgeLayouts.get(view).model.count,
          edges: answer.graph.edges.length,
          code: answer.graph.nodes.filter((node) => node.kind.startsWith("code_")).length,
          firstPaint,
          worstGap,
          settleMs: stamps.at(-1) - stamps[0],
        };
      };
      const samples = Object.fromEntries(Object.keys(scenes).map((name) => [name, []]));
      for (let round = 0; round < rounds; round += 1) {
        /* 차례를 돌린다 — 앞 장면의 데운 캐시가 늘 같은 장면에 가지 않게. */
        const order = Object.keys(scenes);
        for (let turn = 0; turn < order.length; turn += 1) {
          const name = order[(turn + round) % order.length];
          samples[name].push(await measure(name, scenes[name]));
        }
      }
      const median = (values) => values.slice().sort((one, two) => one - two)[Math.floor(values.length / 2)];
      const summary = Object.fromEntries(Object.entries(samples).map(([name, runs]) => [name, {
        nodes: runs[0].nodes, edges: runs[0].edges, code: runs[0].code,
        firstPaint: Math.round(median(runs.map((run) => run.firstPaint))),
        worstGap: Math.round(median(runs.map((run) => run.worstGap))),
        settleMs: Math.round(median(runs.map((run) => run.settleMs))),
      }]));
      knowledgePainterKind = heldPainter;
      knowledgeShowCode = false;
      return summary;
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { scene: CODE_SCENE, workspace: WORKSPACE, rounds: ROUNDS });
  await page.setViewportSize(seatWas);
  console.log(`METRIC knowledge code layer (${CODE_SCENE.pages} pages + ${CODE_SCENE.files} files + ${CODE_SCENE.symbols} definitions, median of ${ROUNDS}): ${JSON.stringify(rows)}`);
  ok(`t-5970 G2: grafting ${CODE_SCENE.files + CODE_SCENE.symbols} code nodes onto ${CODE_SCENE.pages} pages costs no more than as many pages would, and keeps the thousand-point frame gap`,
    !rows.thrown && rows.withCode.code === CODE_SCENE.files + CODE_SCENE.symbols
      && rows.withCode.nodes === rows.control.nodes
      && rows.withCode.firstPaint <= rows.control.firstPaint * SLACK
      && rows.withCode.worstGap < FRAME_GAP_BUDGET_MS,
    JSON.stringify(rows));
}
