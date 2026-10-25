// 설정의 숨은 페이지 「디자인 시스템」 (t-67484, 시안 p-d3d9af654f2f1c47).
//
// 「일반」의 버전 줄을 5번 누르면 설정 목록에 나타난다. 페이지는 ui/tokens.css와 ui/shell.css를
// 그대로 읽어 그린다: 토큰 전수(누락 0 — 값이 안 풀리는 토큰은 「조건부」로 따로 표시하고 누락으로
// 세지 않는다), 컴포넌트 7종 × 6상태(다크·라이트는 페이지 안 스위치). 값은 다시 적지 않는다.
//
// 그리는 시간은 `DESIGN_PAGE_REPORT=1`일 때 9번 그려 중앙값을 밀리초로 내고 5 ms 이하를 요구한다.
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPORT_RUNS = 9;
const COMPONENTS = 7;
const STATES = 6;

export async function testDesignSystem(browser, origin, standBackend, ok) {
  const tokensText = await readFile(resolve(HERE, "../tokens.css"), "utf8");
  const declared = [...new Set([...tokensText.matchAll(/^\s*(--[\w-]+)\s*:/gm)].map((hit) => hit[1]))];

  const page = await browser.newPage();
  await standBackend(page);
  await page.goto(`${origin}/index.html`);
  await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  await page.evaluate(() => { setSettingsOpen(true); showSettingsPane("general"); });

  const railItem = () => page.evaluate(() => {
    const item = document.querySelector('.settings-rail-item[data-pane="design-system"]');
    return item ? { hidden: item.hidden, boxHidden: item.closest(".settings-rail-group-box")?.hidden ?? null } : null;
  });

  // 평소에는 목록이 바뀌지 않는다.
  const before = await railItem();
  ok("the design page is not in the settings list until it is asked for",
    before !== null && before.hidden === true, JSON.stringify(before));
  const version = await page.evaluate(() => document.getElementById("general-version")?.tagName ?? null);
  ok("「일반」 ends in a version line that can be pressed", version === "BUTTON", `tag=${version}`);
  if (version !== "BUTTON" || before === null) { await page.close(); return; }

  for (let press = 0; press < 4; press += 1) await page.click("#general-version");
  const four = await railItem();
  ok("four presses do not show it", four.hidden === true, JSON.stringify(four));
  await page.click("#general-version");
  const five = await railItem();
  ok("the fifth press puts 「디자인 시스템」 in the list", five.hidden === false && five.boxHidden === false, JSON.stringify(five));

  // 열면 토큰 전수: 처음엔 공통 족만, 긴 꼬리는 펼칠 때.
  const baseTheme = await page.evaluate(() => document.documentElement.dataset.theme ?? "dark");
  await page.click('.settings-rail-item[data-pane="design-system"]');
  await page.waitForSelector('.settings-pane[data-pane="design-system"] .ds-comp', { timeout: 5000 });
  const first = await page.evaluate(() => ({
    tokens: document.querySelectorAll('.settings-pane[data-pane="design-system"] .ds-token').length,
    longTail: document.querySelector(".ds-more")?.open ?? null,
    stat: Object.fromEntries([...document.querySelectorAll("[data-ds-stat]")].map((node) => [node.dataset.dsStat, node.textContent])),
  }));
  ok("at first only the shared families are drawn", first.tokens > 0 && first.tokens < declared.length / 2 && first.longTail === false,
    JSON.stringify({ drawn: first.tokens, declared: declared.length }));

  await page.evaluate(() => { document.querySelector(".ds-more").open = true; });
  await page.waitForFunction((count) => document.querySelectorAll('.settings-pane[data-pane="design-system"] .ds-token').length >= count, declared.length);
  const all = await page.evaluate(() => {
    const drawn = [...document.querySelectorAll('.settings-pane[data-pane="design-system"] .ds-token')];
    return {
      names: drawn.map((node) => node.dataset.token),
      conditional: drawn.filter((node) => node.dataset.conditional !== undefined).map((node) => node.dataset.token),
      stat: Object.fromEntries([...document.querySelectorAll("[data-ds-stat]")].map((node) => [node.dataset.dsStat, node.textContent])),
    };
  });
  const missing = declared.filter((name) => !all.names.includes(name));
  ok("every token tokens.css declares is drawn once, none missing", missing.length === 0 && new Set(all.names).size === declared.length,
    JSON.stringify({ declared: declared.length, drawn: all.names.length, missing: missing.slice(0, 8) }));
  const unresolved = await page.evaluate((names) => names.filter((name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() === ""), declared);
  ok("a token with no value here is marked 「조건부」 and not counted missing",
    JSON.stringify([...all.conditional].sort()) === JSON.stringify([...unresolved].sort()) && /(\d+)\s*$/.exec(all.stat.missing)?.[1] === "0",
    JSON.stringify({ conditional: all.conditional, unresolved, stat: all.stat }));

  // 컴포넌트 7종 × 6상태. 눈에 보이는 차이는 페이지가 아니라 게이트가 잰다(designStateCoverage).
  const grid = await page.evaluate(() => {
    const found = designStateCoverage();
    const comps = [...document.querySelectorAll(".ds-comp")];
    return {
      comps: comps.length,
      cells: comps.map((comp) => [...comp.querySelectorAll(".ds-cell")].map((cell) => ({
        component: comp.dataset.component, state: cell.dataset.state, na: cell.dataset.na !== undefined,
        differs: cell.dataset.state === "base" ? null : found[`${comp.dataset.component}:${cell.dataset.state}`],
      }))),
    };
  });
  ok("seven components wear six states each", grid.comps === COMPONENTS && grid.cells.every((cells) => cells.length === STATES),
    JSON.stringify({ comps: grid.comps, cells: grid.cells.map((cells) => cells.length) }));
  const flat = grid.cells.flat().filter((cell) => cell.state !== "base");
  const gaps = flat.filter((cell) => !cell.na && cell.differs === false).map((cell) => `${cell.component}:${cell.state}`);
  ok("every state has a look or is written down as 해당 없음 in the source", gaps.length === 0, JSON.stringify(gaps));
  const stale = flat.filter((cell) => cell.na && cell.differs === true).map((cell) => `${cell.component}:${cell.state}`);
  ok("a cell written down as 해당 없음 really has no look", stale.length === 0, JSON.stringify(stale));
  const loading = flat.filter((cell) => cell.state === "loading").map((cell) => cell.differs);
  ok("loading is one definition for all seven: aria-busy", loading.length === COMPONENTS && loading.every((differs) => differs === true), JSON.stringify(loading));

  // 두 테마는 페이지 안 스위치: 한 번에 하나, 페이지를 떠나면 원래대로.
  const swatch = () => page.evaluate(() => getComputedStyle(document.querySelector(".ds-token[data-token='--background-100'] .ds-sw")).backgroundColor);
  const darkSwatch = await swatch();
  const other = baseTheme === "light" ? "dark" : "light";
  await page.click(`[data-ds-theme="${other}"]`);
  const switched = await page.evaluate(() => document.documentElement.dataset.theme ?? "dark");
  const otherSwatch = await swatch();
  ok("the switch shows the other theme, one at a time", switched === other && otherSwatch !== darkSwatch, JSON.stringify({ switched, darkSwatch, otherSwatch }));
  const otherGaps = await page.evaluate(() => {
    const found = designStateCoverage();
    const na = new Set([...document.querySelectorAll(".ds-cell[data-na]")].map((cell) => `${cell.closest(".ds-comp").dataset.component}:${cell.dataset.state}`));
    return Object.entries(found).filter(([key, differs]) => !differs && !na.has(key)).map(([key]) => key);
  });
  ok("the other theme has the same looks", otherGaps.length === 0, JSON.stringify(otherGaps));
  await page.evaluate(() => showSettingsPane("general"));
  const restored = await page.evaluate(() => document.documentElement.dataset.theme ?? "dark");
  ok("leaving the page puts the person's theme back", restored === baseTheme, `restored=${restored} wanted=${baseTheme}`);

  if (process.env.DESIGN_PAGE_REPORT) {
    const times = await page.evaluate(async (runs) => {
      const out = [];
      for (let n = 0; n < runs; n += 1) {
        showSettingsPane("general");
        const started = performance.now();
        await paintDesignPage();
        out.push(performance.now() - started);
      }
      return out.sort((a, b) => a - b);
    }, REPORT_RUNS);
    const median = times[Math.floor(times.length / 2)];
    console.log(`MEASURE design_page_paint_median_ms=${median.toFixed(1)} max_ms=${times.at(-1).toFixed(1)}`);
    ok("the page draws in 5 ms or less", median <= 5, `median=${median.toFixed(1)} ms`);
  }
  await page.close();
}
