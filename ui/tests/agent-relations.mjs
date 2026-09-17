import { mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { chromium, createWindowServer, openWindowTestPage } from './window-boot.mjs';
import { taskBoardFixture } from './task-board.mjs';
import { installBoardWaits } from './board-waits.mjs';
import { relationsFixture } from './agent-relations-adversarial.mjs';

export async function testAgentRelations(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(taskBoardFixture);
    await page.waitForSelector('.task-board-row');
    await page.evaluate(async () => {
      const now = Date.now();
      window.__PANES__ = [101, 102, 103, 104, 105].map(term => ({term, agent:'codex', state:'working', at:now, resumable:false}));
      window.__LEDGER__ = [101, 102, 103].map(term => ({term, worker:`w-${term}`, run:'run-test', task_id:`t-${term}`,
        dispatch_id:`dp-${term}`, dispatch_started_ms:now-60000, reported:false, task:'같은 이름',
        state:'working', agent:'codex', ledger:'active', checkout:term === 102 ? '/repos/acme' : '/repos/zerocode', at:now}));
      window.__OVERLAYS__ = { mail:[{from:'term:101', to:'term:103', count:2, unread:1, at:now,
        last_message:{id:'m-evidence',run:'run-test',from:'worker:w-101',to:'worker:w-103',kind:'status',created_ms:now}}],
        dependencies:[{from:'term:101',to:'term:103',count:1,at:now-90000}],
        task_dependencies:[{run:'run-test',task:'t-103',dependency:'t-101',task_state:'dispatched',dependency_state:'completed',
          task_created_ms:now-90000,from:'term:101',to:'term:103'}] };
      agentBoardMode = 'graph'; agentGraphScopeKey = ''; agentGraphSelectedEdgeKey = null; agentGraphInspectorTab = 'relations';
      await paintBoardView(undefined, {force:true});
    });
    const identities = await page.evaluate(() => {
      const view = document.querySelector('#board-view');
      const model = agentGraphFullModel(view);
      return {ids:[101,103].map(n=>model.source.places.get(`term:${n}`).taskId),
        groups:agentGraphTasksFor(view,model).groups.filter(g=>g.key.includes('run-test')).length};
    });
    ok('task identity survives the board round trip and distinguishes equal names', identities.ids.join() === 't-101,t-103' && identities.groups === 3, JSON.stringify(identities));

    await page.evaluate(() => setAgentGraphScope(document.querySelector('#board-view'), JSON.stringify(['run-test','t-101'])));
    ok('task scope preserves the full inventory and shows only its participants', await page.evaluate(() => {
      const view=document.querySelector('#board-view'); return agentGraphModels.get(view).agents.length===1 && agentGraphFullModel(view).agents.length===6;
    }));
    ok('scoped project folds work without changing the full inventory', await page.evaluate(() => {
      const view=document.querySelector('#board-view');const key=agentGraphModels.get(view).bands[0].key;
      agentGraphFoldProject(view,key);const folded=agentGraphModels.get(view).bands[0].folded;
      agentGraphFoldProject(view,key);return folded && !agentGraphModels.get(view).bands[0].folded && agentGraphFullModel(view).agents.length===6;
    }));
    await page.evaluate(() => selectAgentGraphEntity(document.querySelector('#board-view'),'agent:term:101'));
    ok('scoped inspector exposes cross-task mail and dependencies', await page.locator('.agent-relation-row').count() >= 2);
    await page.locator('.agent-relation-row').filter({hasText:'메일'}).click();
    ok('relation inspection displays actual message provenance', (await page.locator('.agent-relation-evidence').textContent()).includes('m-evidence'));
    await page.locator('[data-relation-endpoint="agent:term:103"]').click();
    ok('cross-task endpoint navigation reveals its real destination', await page.evaluate(() => agentGraphScopeKey==='' && agentGraphSelectedKey==='agent:term:103'));

    await page.evaluate(() => selectAgentGraphRelation(document.querySelector('#board-view'),'overlay:dependency:agent:term:101>agent:term:103'));
    ok('dependency evidence distinguishes task completion from agent state', (await page.locator('.agent-relation-evidence').textContent()).includes('t-101 · 과업 완료'));
    await page.locator('[data-agent-inspector-tab="activity"]').click();
    ok('activity tab subscribes only to the selected live terminal',await page.evaluate(() => [...previewingTerms()].join() === '103'));
    await page.locator('[data-agent-inspector-tab="details"]').click();
    ok('execution tab shows real task and dispatch identities', (await page.locator('.agent-inspector-pane.is-details').textContent()).includes('dp-103'));
    await page.locator('[data-agent-inspector-tab="details"]').press('Home');
    ok('inspector tabs support keyboard navigation', await page.locator('[data-agent-inspector-tab="relations"]').getAttribute('aria-selected') === 'true');
    ok('relations tab retires the unused terminal screen subscription',await page.evaluate(() => previewingTerms().size===0));

    await page.evaluate(() => selectAgentGraphEntity(document.querySelector('#board-view'),'agent:term:101'));
    await page.fill('.board-ask-field','검증 중인 답변');
    const retained = await page.evaluate(async () => {
      const view=document.querySelector('#board-view'); const input=view.querySelector('.board-ask-field');
      input.dispatchEvent(new CompositionEvent('compositionstart',{data:'검',bubbles:true}));
      input.setSelectionRange(2,4);
      window.__COLUMNS__[0].cards[0].at=Date.now(); window.__COLUMNS__[0].cards[0].said='답을 기다리는 동안 진행 중인 도구';
      paneActivities.set('term:101',[{at:Date.now(),activity:{verb:'bash',target:'own activity while composing',phase:'started'}}]);
      window.__COLUMNS__[1].cards[2].said='새 활동';
      await paintBoardView(undefined,{force:true});
      const result=input===view.querySelector('.board-ask-field') && input===document.activeElement && input.selectionStart===2 && input.value==='검증 중인 답변';
      input.dispatchEvent(new CompositionEvent('compositionend',{data:'검',bubbles:true})); return result;
    });
    ok('live updates preserve the focused response node, IME and selection',retained);

    const performance = await page.evaluate(async () => {
      // Settle the preceding activity change before measuring selection alone.
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const view=document.querySelector('#board-view'); paintAgentGraphEdges(view);
      const before=[agentGraphNodeCreations,agentGraphLayoutRuns,agentGraphEdgeMeasureRuns];
      for(let i=0;i<30;i++) selectAgentGraphEntity(view, `agent:term:${i%2 ? 101 : 103}`);
      await new Promise(requestAnimationFrame);
      return {before,after:[agentGraphNodeCreations,agentGraphLayoutRuns,agentGraphEdgeMeasureRuns]};
    });
    ok('selection reuses DOM, layout and measured edge ports',performance.before.join()===performance.after.join(),JSON.stringify(performance));

    const clone=await page.evaluate(() => {
      const original=document.querySelector('#board-view'); const copy=original.cloneNode(true);copy.removeAttribute('id');
      original.parentNode.append(copy);paintAgentGraph(copy,agentGraphFullModel(original));
      copy.querySelector('[data-agent-inspector-tab="details"]').click();
      const pass=copy.querySelector('.agent-inspector-pane.is-details').hidden===false && copy.querySelector('.agent-graph-scope').onchange!==null;
      copy.remove();return pass;
    });
    ok('cloned views wire their own controls and inspector',clone);

    ok('selected worker follows its actual replacement terminal seat',await page.evaluate(async () => {
      const view=document.querySelector('#board-view');selectAgentGraphEntity(view,'agent:term:103');
      window.__LEDGER__.find(row=>row.term===103).term=113;
      window.__PANES__.find(row=>row.term===103).term=113;
      window.__COLUMNS__[1].cards.find(card=>card.pane==='term:103').pane='term:113';
      await paintBoardView(undefined,{force:true});return agentGraphSelectedKey==='agent:term:113';
    }));
    ok('returning from tasks follows the newly selected task scope',await page.evaluate(() => {
      const view=document.querySelector('#board-view');setAgentGraphScope(view,JSON.stringify(['run-test','t-101']));
      view.querySelector('[data-board-mode="tasks"]').click();selectTaskBoardMember(view,'agent:term:113');
      view.querySelector('[data-board-mode="graph"]').click();
      return agentGraphScopeKey===JSON.stringify(['run-test','t-103']) && agentGraphSelectedKey==='agent:term:113';
    }));
    ok('a vanished scope explains its own absence while retaining other executions',await page.evaluate(async () => {
      const view=document.querySelector('#board-view');
      window.__LEDGER__=window.__LEDGER__.filter(row=>row.term!==113);
      window.__PANES__=window.__PANES__.filter(row=>row.term!==113);
      window.__COLUMNS__=window.__COLUMNS__.map(column=>({...column,cards:column.cards.filter(card=>card.pane!=='term:113')}));
      await paintBoardView(undefined,{force:true});
      const correct=agentGraphScopeKey===JSON.stringify(['run-test','t-103']) && agentGraphFullModel(view).agents.length>0
        && !view.querySelector('.agent-graph-empty').hidden && view.querySelector('.agent-graph-empty').textContent.includes('이 범위');
      setAgentGraphScope(view,'');return correct;
    }));
    await page.emulateMedia({reducedMotion:'reduce'});
    await mkdir('output/playwright/agent-relations',{recursive:true});
    for(const width of [1440,900,560,360]) {
      await page.setViewportSize({width,height:960});
      await page.evaluate(() => { const view=document.querySelector('#board-view');setAgentGraphInspectorOpen(view,false);paintAgentGraphEdges(view); });
      const layout=await page.evaluate(() => {
        const view=document.querySelector('#board-view'); const toolbar=view.querySelector('.agent-relations-toolbar');
        return {overflow:view.scrollWidth>view.clientWidth+1,toolbar:toolbar.getBoundingClientRect().height};
      });
      ok(`relations controls fit at ${width}px`,!layout.overflow && layout.toolbar>0,JSON.stringify(layout));
      await page.screenshot({path:`output/playwright/agent-relations/relations-${width}.png`});
    }
    ok('relations raise no browser errors',faults.length===0,faults.join('\n'));
  } finally { await page.close(); }
}
/* t-4145 · 승인된 시안(docs/design/agent-relations-preview-20260914)의 형태.
 *
 * 동작·온톨로지는 위의 사례가 지키고, 여기는 **배치와 모습**만 고정한다: 구역이
 * 시안의 차례로 서는가, 레일이 범위 선택과 같은 답을 내는가, 인스펙터가 티어를
 * 따라 곁판·서랍·시트를 지나는가, 어느 티어에서도 2레인 그림이 맞춤 바닥에서
 * 가로로 넘치지 않는가, 두 테마에서 글자가 제 바탕 위에 읽히는가. 폭은 토큰에서
 * 읽고 어떤 수도 여기 적지 않는다. */
export async function testAgentRelationsForm(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.evaluate(relationsFixture);
    await page.evaluate(() => window.__BOARD_SETTLED__());
    const ascending = (list) => list.every((at, index) => at >= 0 && (index === 0 || at > list[index - 1]));

    const regions = await page.evaluate(() => {
      const view = document.querySelector('#board-view');
      const order = (root, selectors) => selectors.map((one) => [...(root?.children ?? [])].findIndex((child) => child.matches(one)));
      const head = view.querySelector('.agent-graph-head');
      const toolbar = view.querySelector('.agent-relations-toolbar');
      const surface = view.querySelector('.agent-graph-surface');
      const inspector = view.querySelector('.agent-inspector');
      return {
        view: order(view, ['.agent-relations-rail', '.agent-graph-head', '.workbench-nav', '.agent-relations-toolbar', '.agent-graph-layout']),
        railItems: view.querySelectorAll('.agent-relations-rail-item').length,
        railPressed: view.querySelector('.agent-relations-rail-item[aria-pressed="true"]')?.dataset.scope ?? null,
        head: order(head, ['.agent-graph-heading', '.board-mode', '.agent-graph-summary', '.agent-graph-head-tools']),
        headTools: [...(head.querySelector('.agent-graph-head-tools')?.children ?? [])].map((one) => one.className),
        toolbar: order(toolbar, ['.agent-graph-scope-label', '.agent-graph-overlays', '.board-search', '.agent-graph-density']),
        overlays: toolbar.querySelectorAll('[data-graph-overlay]').length,
        surface: order(surface, ['.agent-graph-canvas-head', '.agent-graph-scroll', '.agent-graph-canvas-foot']),
        canvasHead: ['.agent-graph-run', '.agent-graph-scope-title', '.agent-graph-scope-copy', '.agent-graph-scope-count']
          .map((one) => surface.querySelector(`.agent-graph-canvas-head ${one}`) !== null),
        foot: order(surface.querySelector('.agent-graph-canvas-foot'), ['.agent-graph-legend', '.agent-graph-gesture-hint', '.agent-graph-zoom']),
        inspector: order(inspector, ['.agent-inspector-head', '.agent-inspector-tabs', '.agent-inspector-body', '.agent-inspector-actions']),
        inspectorHead: order(inspector.querySelector('.agent-inspector-head'),
          ['.agent-inspector-close', '.agent-inspector-kind', '.agent-inspector-icon', '.agent-inspector-title', '.agent-inspector-meta']),
        tabs: [...inspector.querySelectorAll('[data-agent-inspector-tab]')].map((one) => one.dataset.agentInspectorTab).join(),
        scopeTitle: view.querySelector('.agent-graph-scope-title')?.textContent ?? '',
      };
    });
    ok('F1 the relations view stands in the preview\'s regions, in the preview\'s order',
      ascending(regions.view) && ascending(regions.head) && ascending(regions.toolbar) && ascending(regions.surface)
        && ascending(regions.foot) && ascending(regions.inspector) && ascending(regions.inspectorHead)
        && regions.headTools.some((one) => one.includes('agent-graph-follow')) && regions.headTools.some((one) => one.includes('agent-inspector-toggle'))
        && regions.headTools.some((one) => one.includes('board-popout')) && regions.overlays === 4 && regions.canvasHead.every(Boolean)
        && regions.railItems >= 2 && regions.railPressed === '' && regions.tabs === 'relations,activity,details' && regions.scopeTitle === '전체 실행',
      JSON.stringify(regions));

    const rail = await page.evaluate(async () => {
      const view = document.querySelector('#board-view');
      const key = JSON.stringify(['run-1', 't-A']);
      const item = view.querySelector(`.agent-relations-rail-item[data-scope='${key}']`);
      item?.click();
      await window.__BOARD_SETTLED__();
      const scoped = { key: agentGraphScopeKey, select: view.querySelector('.agent-graph-scope').value,
        pressed: view.querySelector('.agent-relations-rail-item[aria-pressed="true"]')?.dataset.scope ?? null,
        title: view.querySelector('.agent-graph-scope-title')?.textContent ?? '', shown: agentGraphModels.get(view).agents.length,
        count: view.querySelector('.agent-graph-scope-count')?.textContent ?? '', small: item?.querySelector('small')?.textContent ?? '',
        mark: item?.querySelector('.agent-relations-rail-mark')?.className ?? '' };
      view.querySelector('.agent-relations-rail-item[data-scope=""]')?.click();
      await window.__BOARD_SETTLED__();
      const attention = view.querySelector(`.agent-relations-rail-item[data-scope='${JSON.stringify(['run-1', 't-E'])}'] small`)?.textContent ?? '';
      return { scoped, attention, back: { key: agentGraphScopeKey, title: view.querySelector('.agent-graph-scope-title')?.textContent ?? '',
        pressed: view.querySelector('.agent-relations-rail-item[aria-pressed="true"]')?.dataset.scope ?? null } };
    });
    ok('F2 the task rail scopes the picture, agrees with the scope select, and says each task\'s count and state',
      rail.scoped.key === JSON.stringify(['run-1', 't-A']) && rail.scoped.select === rail.scoped.key && rail.scoped.pressed === rail.scoped.key
        && rail.scoped.title === '구현' && rail.scoped.shown === 2 && rail.scoped.count.includes('2') && rail.scoped.small.includes('2')
        && rail.scoped.small.includes('작업 중') && rail.scoped.mark.includes('is-working') && rail.attention.includes('확인 필요 1')
        && rail.back.key === '' && rail.back.pressed === '' && rail.back.title === '전체 실행',
      JSON.stringify(rail));

    const inspectorHead = await page.evaluate(async () => {
      const view = document.querySelector('#board-view');
      selectAgentGraphEntity(view, 'agent:term:202', { focus: false });
      await window.__BOARD_SETTLED__();
      const agent = { kind: view.querySelector('.agent-inspector-kind')?.textContent ?? '',
        icon: view.querySelector('.agent-inspector-icon')?.querySelector('svg, .agent-ico') !== null,
        hidden: view.querySelector('.agent-inspector-icon')?.hidden ?? true };
      selectAgentGraphRelation(view, 'overlay:mail:agent:term:202>agent:term:203');
      await window.__BOARD_SETTLED__();
      const relation = { kind: view.querySelector('.agent-inspector-kind')?.textContent ?? '',
        icon: view.querySelector('.agent-inspector-icon use')?.getAttribute('href') ?? '' };
      selectAgentGraphEntity(view, 'agent:term:202', { focus: false });
      await window.__BOARD_SETTLED__();
      return { agent, relation };
    });
    ok('F3 the inspector head carries the subject\'s glyph under its kind line, for an agent and for a relation',
      inspectorHead.agent.icon && !inspectorHead.agent.hidden && inspectorHead.relation.kind === '선택한 관계' && inspectorHead.relation.icon === '#i-link',
      JSON.stringify(inspectorHead));

    await page.evaluate(() => {
      const view = document.querySelector('#board-view');
      view.style.position = 'fixed'; view.style.inset = '0'; view.style.height = '100vh'; view.style.zIndex = '100';
    });
    const widths = await page.evaluate(() => {
      const dress = getComputedStyle(document.querySelector('#board-view'));
      return { side: Number.parseFloat(dress.getPropertyValue('--agent-graph-tier-side')),
        list: Number.parseFloat(dress.getPropertyValue('--agent-graph-tier-list')),
        tight: Number.parseFloat(dress.getPropertyValue('--agent-graph-tier-tight')) };
    });
    const at = async (width) => {
      await page.setViewportSize({ width: Math.max(width, 320), height: 960 });
      return page.evaluate(async (width) => {
        const view = document.querySelector('#board-view');
        view.style.width = `${width}px`;
        setAgentGraphInspectorOpen(view, false);
        await paintBoardView(boardTab(), { force: true });
        await window.__BOARD_SETTLED__();
        // The resize watcher schedules the fit after this frame's callbacks; give it the tier case's settle.
        await new Promise((done) => setTimeout(done, 260));
        await window.__BOARD_SETTLED__();
        const shown = (selector) => { const held = view.querySelector(selector); return held ? getComputedStyle(held).display !== 'none' : null; };
        const railBox = view.querySelector('.agent-relations-rail')?.getBoundingClientRect();
        const dress = getComputedStyle(view);
        const surface = getComputedStyle(view.querySelector('.agent-graph-surface'));
        const px = (name, from = dress) => Number.parseFloat(from.getPropertyValue(name));
        const scale = px('--agent-graph-density', surface) * agentGraphTuning(view).fitFloor;
        const lanes = 2;
        const need = (AGENT_GRAPH_CONTEXT_LAYERS.length * px('--agent-graph-context-width')
          + lanes * px('--agent-graph-agent-width') + (AGENT_GRAPH_CONTEXT_LAYERS.length + lanes - 1) * px('--agent-graph-layer-gap')) * scale;
        const scroll = view.querySelector('.agent-graph-scroll');
        const inner = scroll.clientWidth - Number.parseFloat(getComputedStyle(scroll).paddingLeft) - Number.parseFloat(getComputedStyle(scroll).paddingRight);
        const toggle = view.querySelector('.agent-inspector-toggle');
        const before = view.classList.contains('is-inspector-open');
        toggle?.click();
        const after = { open: view.classList.contains('is-inspector-open'), expanded: toggle?.getAttribute('aria-expanded') ?? null };
        setAgentGraphInspectorOpen(view, false);
        const fits = (node) => node.scrollWidth <= node.clientWidth + 1;
        return { width, rail: shown('.agent-relations-rail'), railWidth: Math.round(railBox?.width ?? 0),
          railToken: px('--agent-relations-rail-width'), railDrawerToken: px('--agent-relations-rail-width-drawer'),
          inspectorPlace: getComputedStyle(view.querySelector('.agent-inspector')).position === 'static' ? 'beside' : 'floating',
          toggle: shown('.agent-inspector-toggle'), toggled: !before && after.open && after.expanded === 'true',
          list: view.classList.contains('is-graph-list'), need: Math.round(need), inner: Math.round(inner),
          fits: fits(view) && fits(view.querySelector('.agent-graph-head')) && fits(view.querySelector('.agent-relations-toolbar')) && fits(scroll) };
      }, width);
    };
    const tiers = { wide: await at(1280), side: await at(widths.side), drawer: await at(900), list: await at(widths.list), sheet: await at(560), tight: await at(360) };
    ok('F4 the rail is the preview\'s sidebar: beside the picture at the side tier, narrower in the drawer tier, folded below it; the inspector toggle opens the drawer',
      tiers.wide.rail && tiers.wide.railWidth === tiers.wide.railToken && tiers.wide.inspectorPlace === 'beside' && tiers.wide.toggle === false
        && tiers.side.rail && tiers.side.inspectorPlace === 'beside'
        && tiers.drawer.rail && tiers.drawer.railWidth === tiers.drawer.railDrawerToken && tiers.drawer.inspectorPlace === 'floating' && tiers.drawer.toggle && tiers.drawer.toggled
        && tiers.list.rail && tiers.list.railWidth === tiers.list.railDrawerToken && tiers.list.toggle
        && tiers.sheet.rail === false && tiers.sheet.list && tiers.tight.rail === false,
      JSON.stringify(tiers));
    ok('F5 at every tier a two-lane picture fits the canvas at the fit floor, and nothing scrolls sideways',
      ['wide', 'side', 'drawer', 'list'].every((key) => tiers[key].inner >= tiers[key].need)
        && Object.values(tiers).every((one) => one.fits),
      JSON.stringify(tiers));

    const tasksMode = await page.evaluate(async () => {
      const view = document.querySelector('#board-view');
      view.style.width = '1280px';
      view.querySelector('[data-board-mode="tasks"]').click();
      await window.__BOARD_SETTLED__();
      const shown = (selector) => { const held = view.querySelector(selector); return held ? held.getClientRects().length > 0 : null; };
      const seen = { query: shown('.board-query'), rail: shown('.agent-relations-rail'), scope: shown('.agent-graph-scope'), density: shown('.agent-graph-density'), toggle: shown('.agent-inspector-toggle') };
      view.querySelector('[data-board-mode="graph"]').click();
      await window.__BOARD_SETTLED__();
      seen.back = shown('.agent-relations-rail') && shown('.agent-graph-scope');
      return seen;
    });
    ok('F6 the task view keeps the search and folds the relations-only controls and the rail',
      tasksMode.query && tasksMode.rail === false && tasksMode.scope === false && tasksMode.density === false && tasksMode.toggle === false && tasksMode.back,
      JSON.stringify(tasksMode));

    await page.setViewportSize({ width: 1440, height: 960 });
    const readInk = () => page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 400));
      const view = document.querySelector('#board-view');
      view.style.width = '1440px';
      const channel = (value) => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
      // color-mix results come back as `color(srgb r g b)` on a 0..1 scale; plain tokens as `rgb(r, g, b)`.
      const luminance = (color) => { const parts = (color.match(/[\d.]+/g) ?? [0, 0, 0]).map(Number); const [r, g, b] = color.startsWith('color(srgb') ? parts.map((one) => one * 255) : parts; return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b); };
      const contrast = (ink, ground) => { const a = luminance(ink); const b = luminance(ground); return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05); };
      const pair = (inkSelector, groundSelector) => {
        const ink = view.querySelector(inkSelector); const ground = view.querySelector(groundSelector);
        if (!ink || !ground) return null;
        return Math.round(contrast(getComputedStyle(ink).color, getComputedStyle(ground).backgroundColor) * 100) / 100;
      };
      return { theme: document.documentElement.dataset.theme ?? 'dark',
        scopeTitle: pair('.agent-graph-scope-title', '.agent-graph-surface'), scopeCopy: pair('.agent-graph-scope-copy', '.agent-graph-surface'),
        railTitle: pair('.agent-relations-rail-item b', '.agent-relations-rail'), railSmall: pair('.agent-relations-rail-item small', '.agent-relations-rail'),
        inspectorKind: pair('.agent-inspector-kind', '.agent-inspector'), inspectorTitle: pair('.agent-inspector-title', '.agent-inspector'),
        hint: pair('.agent-graph-gesture-hint', '.agent-graph-canvas-foot'), stat: pair('.agent-graph-stat.is-attention .agent-graph-stat-count', '.agent-graph-head') };
    });
    const readable = (ink) => ink && ink.scopeTitle >= 4.5 && ink.railTitle >= 4.5 && ink.inspectorTitle >= 4.5 && ink.stat >= 4.5
      && ink.scopeCopy >= 3 && ink.railSmall >= 3 && ink.inspectorKind >= 3 && ink.hint >= 3;
    const dark = await readInk();
    await page.evaluate(() => { theme = 'light'; applyTheme(); });
    const light = await readInk();
    await page.evaluate(() => { theme = 'dark'; applyTheme(); });
    ok('F7 the new words read on their own grounds in both treatments', dark.theme === 'dark' && light.theme === 'light' && readable(dark) && readable(light),
      JSON.stringify({ dark, light }));
    await mkdir('output/playwright/agent-relations', { recursive: true });
    await page.screenshot({ path: 'output/playwright/agent-relations/form-1440.png' });
    ok('relations form raises no browser errors', faults.length === 0, faults.join('\n'));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin }=await createWindowServer();const browser=await chromium.launch({headless:true});let failures=0;
  const report=(name,pass,detail='')=>{console.log(`${pass?'PASS':'FAIL'} ${name}${!pass&&detail?'\n'+detail:''}`);if(!pass)failures++;};
  try { await testAgentRelations(browser,origin,report); await testAgentRelationsForm(browser,origin,report); }
  finally {await browser.close();files.close();} if(failures)process.exitCode=1;
}
