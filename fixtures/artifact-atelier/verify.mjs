// 결 아틀리에 픽스처 검증 — 헤드리스 브라우저로 조작·초기화·내보내기·키보드·모션 줄임·좁은 폭을 확인한다.
// 사용: node fixtures/artifact-atelier/verify.mjs [--engine chromium|webkit] [--out <dir>]
// Playwright 는 설치하지 않는다. PLAYWRIGHT_MODULE 또는 알려진 기존 설치를 쓴다.
import { createRequire } from "node:module";
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const page_ = join(here, "index.html");
const args = process.argv.slice(2);
const flag = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback; };
const engine = flag("--engine", "chromium");
const out = flag("--out", mkdtempSync(join(tmpdir(), "atelier-")));

const require = createRequire(import.meta.url);
const candidates = [process.env.PLAYWRIGHT_MODULE, "playwright"].filter(Boolean);
let pw;
for (const c of candidates) { try { pw = require(c); break; } catch { /* 다음 후보 */ } }
if (!pw) { console.error("playwright 를 찾지 못함 — PLAYWRIGHT_MODULE 을 지정하세요"); process.exit(2); }

const results = [];
const check = (name, ok, detail = "") => { results.push({ name, ok: Boolean(ok), detail }); };

// 정적 검사: 색 리터럴은 토큰 블록(:root { … }) 밖에 없어야 한다.
const src = readFileSync(page_, "utf8");
const core = src.slice(src.indexOf('<style id="studio-core">'), src.indexOf("</style>"));
const tokenBlockEnd = core.indexOf("\n}\n");
const outsideTokens = core.slice(tokenBlockEnd) + src.slice(src.indexOf('<style id="studio-chrome">'), src.indexOf('<style id="studio-choices">'));
const stray = outsideTokens.match(/#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(/g) || [];
check("색 리터럴은 토큰 블록에만", stray.length === 0, stray.join(" "));
check("외부 자원 없음", !/(src|href)=["']https?:|@import|url\(["']?https?:/.test(src));

const browser = await pw[engine].launch();
const errors = [];
const watch = (p) => { p.on("pageerror", (e) => errors.push(String(e))); p.on("console", (m) => m.type() === "error" && errors.push(m.text())); };

const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
watch(page);
await page.goto(pathToFileURL(page_).href);
await page.evaluate(() => localStorage.clear());
await page.reload();

// 시안·보기 전환은 View Transition 안에서 반영되므로 속성이 바뀔 때까지 기다린다.
const attrIs = (p, name, value) => p.waitForFunction(([n, v]) => document.documentElement.getAttribute(n) === v, [name, value], { timeout: 3000 }).then(() => true, () => false);
// 스크린샷 전에 전환 애니메이션이 끝나기를 기다린다.
const settle = (p) => p.waitForFunction(() => document.getAnimations().every((a) => a.playState !== "running"), null, { timeout: 3000 }).catch(() => {});
const shot = async (p, name) => { await settle(p); await p.screenshot({ path: join(out, `${engine}-${name}.png`), fullPage: true }); };
const visibleFrames = () => page.$$eval(".variant-frame", (els) => els.filter((e) => !e.hidden).map((e) => e.dataset.frame));
check("세 시안이 같은 원고에서 그려짐", (await page.$$eval(".variant", (e) => e.length)) === 3);
check("하나씩 보기는 한 시안만", JSON.stringify(await visibleFrames()) === '["editorial"]');
await shot(page, `editorial-light`);

// 키보드: 숫자 키와 C
await page.keyboard.press("2");
check("2 키 → 공간 시안", await attrIs(page, "data-variant", "spatial"));
await shot(page, `spatial`);
await page.keyboard.press("c");
check("C 키 → 나란히 보기", (await attrIs(page, "data-view", "compare")) && (await visibleFrames()).length === 3);
await shot(page, `compare`);
await page.keyboard.press("c");
await attrIs(page, "data-view", "single");

// 탭으로 첫 선택지까지 가서 화살표로 시안 바꾸기
await page.focus('input[name="variant"]:checked');
await page.keyboard.press("ArrowDown");
check("화살표로 선택지 이동", await attrIs(page, "data-variant", "index"));

// 조절: 밀도·글자 → #studio-choices 와 실제 글자 크기
const bodyPx = () => page.$eval('.variant-frame:not([hidden]) .variant', (e) => parseFloat(getComputedStyle(e).fontSize));
const before = await bodyPx();
const chromeBefore = await page.$eval(".direction", e => parseFloat(getComputedStyle(e).fontSize));
const setRange = (name, v) => page.$eval(`input[name="${name}"]`, (el, val) => { el.value = val; el.dispatchEvent(new Event("input", { bubbles: true })); }, v);
await setRange("density", "1.2");
await setRange("type", "1.2");
const choices = await page.$eval("#studio-choices", (e) => e.textContent);
check("선택값이 CSS 규칙으로 들어감", choices.includes("--density: 1.2") && choices.includes("--type-scale: 1.2"), choices);
const after = await bodyPx();
check("글자 크기 조절이 실제 본문을 바꿈", Math.abs(after / before - 1.2) < 0.01, `${before}px → ${after}px`);
const chromePx = await page.$eval(".direction", (e) => parseFloat(getComputedStyle(e).fontSize));
check("조절은 작업대 글자를 건드리지 않음", Math.abs(chromePx - chromeBefore) < 0.01, `${chromePx}px`);

// 테마
const bg = () => page.$eval("body", (e) => getComputedStyle(e).backgroundColor);
const lightBg = await bg();
await page.check('input[name="theme"][value="dark"]');
check("다크 선택 → data-theme", (await page.getAttribute("html", "data-theme")) === "dark");
check("다크 바탕이 실제로 바뀜", (await bg()) !== lightBg, `${lightBg} → ${await bg()}`);
await page.keyboard.press("2");
await attrIs(page, "data-variant", "spatial");
await shot(page, `spatial-dark`);

// 원칙 강조
await page.click('.variant-frame:not([hidden]) [data-pick="rhythm"]');
check("원칙 강조 → data-focus", (await page.getAttribute("html", "data-focus")) === "rhythm");
check("강조가 모든 시안에 공유됨", (await page.$$eval('[data-pick="rhythm"]', (els) => els.every((e) => (e.getAttribute("aria-pressed") || e.getAttribute("aria-expanded")) === "true"))));
await shot(page, `spatial-dark-focus`);

// 내보내기
await page.click("#export");
const exportedHTML = await page.$eval("#export-code", (e) => e.value);
check("내보내기 대화상자 열림", await page.$eval("#export-dialog", (d) => d.open));
await page.keyboard.press("Escape");
check("Esc 로 닫힘", !(await page.$eval("#export-dialog", (d) => d.open)));
const exportPath = join(out, "exported.html");
writeFileSync(exportPath, exportedHTML);

// 저장 후 새로고침: 선택 복원
await page.reload();
check("새로고침 뒤 선택 복원", (await page.getAttribute("html", "data-variant")) === "spatial" && (await page.getAttribute("html", "data-theme")) === "dark");

// 초기화: 원본 토큰으로
await page.click("#reset");
check("초기화 → 시안·테마·강조가 원본으로", (await attrIs(page, "data-variant", "editorial")) && (await attrIs(page, "data-theme", null)) && (await attrIs(page, "data-focus", null)));
check("초기화 → 선택값 규칙 비움", (await page.$eval("#studio-choices", (e) => e.textContent)) === "");
check("초기화 → 슬라이더가 토큰 원본값", (await page.$eval('input[name="density"]', (e) => e.value)) === "1");

// 내보낸 파일을 다른 컨텍스트(빈 저장소)에서 연다
const ctx2 = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const ex = await ctx2.newPage();
watch(ex);
await ex.goto(pathToFileURL(exportPath).href);
check("내보낸 파일: 선택한 테마 유지", (await ex.getAttribute("html", "data-theme")) === "dark");
check("내보낸 파일: 선택값 CSS 유지", (await ex.$eval("#studio-choices", (e) => e.textContent)).includes("--density: 1.2"));
check("내보낸 파일: 시안 하나·조절판 없음", (await ex.$$eval(".variant", (e) => e.length)) === 1 && !(await ex.$("#controls")));
check("내보낸 파일: 글자 크기 유지", Math.abs((await ex.$eval(".variant", (e) => parseFloat(getComputedStyle(e).fontSize))) - after) < 0.01);
await ex.click('[data-pick="contrast"]');
check("내보낸 파일: 원칙 강조 동작", (await ex.getAttribute("html", "data-focus")) === "contrast");
await shot(ex, `exported`);

// 모션 줄임
const ctx3 = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce" });
const rm = await ctx3.newPage();
watch(rm);
await rm.goto(pathToFileURL(page_).href);
check("reduced-motion → --motion 0", (await rm.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--motion").trim())) === "0");
await rm.keyboard.press("2");
await attrIs(rm, "data-variant", "spatial");
check("reduced-motion → 카드 전환 0초", (await rm.$eval(".sp-card", (e) => getComputedStyle(e).transitionDuration.split(",").every((d) => parseFloat(d) === 0))));
check("모션 줄임에서도 원고가 모두 보임", (await rm.$$eval(".variant-frame:not([hidden]) .sp-card", (e) => e.length)) === 4);

// 좁은 폭
for (const width of [390, 768, 1205]) {
  const c = await browser.newContext({ viewport: { width, height: 900 } });
  const p = await c.newPage();
  watch(p);
  await p.goto(pathToFileURL(page_).href);
  for (const key of ["1", "2", "3"]) {
    await p.keyboard.press(key);
    await attrIs(p, "data-variant", ["editorial", "spatial", "index"][Number(key) - 1]);
    const over = await p.evaluate(() => document.documentElement.scrollWidth - innerWidth);
    check(`${width}px 폭 시안 ${key}: 가로 넘침 없음`, over <= 0, `${over}px`);
    if (key === "1") {
      const headings = await p.$$eval(".ed-pick", nodes => nodes.every(node => {
        if (node.scrollWidth > node.clientWidth) return false;
        return [...node.querySelectorAll(".ed-no,.ed-name,.ed-en")].every(word => {
          const range = document.createRange(); range.selectNodeContents(word);
          return range.getClientRects().length === 1;
        });
      }));
      check(`${width}px 편집 시안의 원칙 이름·영문·번호가 끊기지 않음`, headings);
    }
  }
  await p.keyboard.press("2");
  await attrIs(p, "data-variant", "spatial");
  await shot(p, `${width}-spatial`);
  await c.close();
}

check("콘솔·페이지 오류 없음", errors.length === 0, errors.join(" | "));
await browser.close();

const failed = results.filter((r) => !r.ok);
for (const r of results) console.log(`${r.ok ? "ok  " : "FAIL"} ${r.name}${r.detail ? ` — ${r.detail}` : ""}`);
console.log(`\n${engine}: ${results.length - failed.length}/${results.length} 통과 · 스크린샷 ${out}`);
process.exit(failed.length ? 1 : 0);
