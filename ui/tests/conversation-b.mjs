/* The conversation the person passed on 2026-10-29 (t-81525, draft B): the
 * answers read as bare prose in one column, the newest in the fullest ink; the
 * tool work between two answers is one pill; the person's words stand on the
 * right; a code block opens with its own band; the window's own note is a thin
 * line. Read off the laid-out page, on the folded page (the product's
 * default), at seven window widths and in both treatments: nothing pushes the
 * page sideways, the reading steps with the width — the column widens with a
 * wide page (t-81786 P3-16) — every word reads at 4.5:1, and the page builds
 * no more than it shows.
 *
 * `CONVERSATION_B_SHOTS=<folder>` keeps a picture of each treatment at 390,
 * 1440 and 2000 px there; without it nothing is written. */
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { openConversation } from "./conversation-parity.mjs";
import { mockupTurns } from "./conversation-redesign.mjs";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

export const B_WIDTHS = Object.freeze([390, 720, 1000, 1200, 1440, 2000, 2560]);
const SHOT_WIDTHS = Object.freeze([390, 1440, 2000]);

/* The mockup's turns with what draft B adds to them: a second answer before
 * the last (so one answer is not the newest), a fenced block named
 * `typescript` and one named nothing, a long unbroken path in a chip, and the
 * window's note to the agent. */
function draftTurns() {
  const turns = mockupTurns(Date.now() - 60_000);
  const last = turns.pop();
  const path = `/Users/dev/${"very-long-folder-name/".repeat(6)}file.ts`;
  turns.push(
    { role: "assistant", text: `먼저 목록을 찾았습니다. 경로는 \`${path}\` 입니다.`, at_ms: last.at_ms - 900 },
    { role: "window", text: "The window restarted and cut your last turn short. Continue exactly where you left off.", at_ms: last.at_ms - 600 },
    { role: "user", text: "코드도 보여 줘.", at_ms: last.at_ms - 300 },
    { ...last, text: `${last.text}\n\n\`\`\`typescript\nexport const width = (screen: { width: number }) => screen.width - 32;\n\`\`\`\n\n\`\`\`\nplain words in a fence with no language\n\`\`\`` },
  );
  return turns;
}

/* What one width and one treatment look like, read in the page. */
function readPage() {
  const page = document.querySelector("#worker-view");
  const list = page.querySelector(".helper-turns");
  const box = (node) => node.getBoundingClientRect();
  const column = (() => {
    const style = getComputedStyle(list);
    const at = box(list);
    return { left: at.left + parseFloat(style.paddingLeft), right: at.right - parseFloat(style.paddingRight) };
  })();
  const inside = (node) => {
    const at = box(node);
    return at.left >= column.left - 1 && at.right <= column.right + 1;
  };
  // A colour as linear-light luminance, from `rgb()` or `color(srgb …)`.
  const channels = (value) => {
    const srgb = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)/.exec(value);
    if (srgb) return srgb.slice(1, 4).map(Number);
    const rgb = /rgba?\(([\d.]+),? ([\d.]+),? ([\d.]+)/.exec(value);
    return rgb ? rgb.slice(1, 4).map((one) => Number(one) / 255) : null;
  };
  const luminance = (value) => {
    const [r, g, b] = channels(value).map((one) => (one <= 0.04045 ? one / 12.92 : ((one + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (ink, ground) => {
    const [a, b] = [luminance(ink), luminance(ground)].sort((x, y) => y - x);
    return Math.round(((a + 0.05) / (b + 0.05)) * 100) / 100;
  };
  const ground = getComputedStyle(page).backgroundColor;
  const ink = (node) => (node ? contrast(getComputedStyle(node).color, ground) : null);
  const answers = [...list.querySelectorAll(":scope > .helper-turn.is-assistant")];
  const latest = answers.find((row) => row.classList.contains("is-latest"));
  const earlier = answers.find((row) => !row.classList.contains("is-latest"));
  const said = latest?.querySelector(":scope > .helper-said");
  const style = said ? getComputedStyle(said) : null;
  const person = [...list.querySelectorAll(":scope > .helper-turn.is-user")].at(-1)?.querySelector(":scope > .helper-said") ?? null;
  const pills = [...list.querySelectorAll(":scope > .helper-group")];
  const codes = [...list.querySelectorAll(".helper-code")];
  const note = list.querySelector(":scope > .is-window-note .helper-window-note-words");
  // The empty box's sentence, measured in the box's own font against the
  // room inside its padding: one line, never a second one hidden under it.
  const composer = page.querySelector(".worker-composer-box");
  const say = (() => {
    if (!composer) return null;
    const style = getComputedStyle(composer);
    const pen = document.createElement("canvas").getContext("2d");
    pen.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    const room = composer.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    return {
      words: composer.placeholder,
      fits: pen.measureText(composer.placeholder).width <= room,
      oneLine: getComputedStyle(composer, "::placeholder").whiteSpace === "nowrap",
    };
  })();
  const dock = page.querySelector(".chat-dock");
  return {
    pageWidth: Math.round(box(page).width),
    // The column's measure at this page's width, and the base one (t-81786 P3-16).
    measure: window.__READING_MEASURE__(box(page).width),
    baseMeasure: window.__READING_MEASURE__(0),
    dockMax: dock ? parseFloat(getComputedStyle(dock).maxWidth) : null,
    fadeMax: parseFloat(getComputedStyle(page, "::after").maxWidth),
    pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    listOverflow: list.scrollWidth - list.clientWidth,
    columnWidth: Math.round(column.right - column.left),
    proseSize: style ? parseFloat(style.fontSize) : null,
    proseLeading: style ? Math.round((parseFloat(style.lineHeight) / parseFloat(style.fontSize)) * 100) / 100 : null,
    pillsInside: pills.length > 0 && pills.every((pill) => inside(pill.firstElementChild)),
    codesInside: codes.length >= 2 && codes.every(inside),
    // What a block's code runs past its own box — the block's inside scroll,
    // which neither the page's nor the list's width counts.
    codeOverflow: Math.max(...codes.map((code) => {
      const pre = code.querySelector(":scope > pre");
      return pre ? pre.scrollWidth - pre.clientWidth : 0;
    })),
    codeWrap: codes[0] ? getComputedStyle(codes[0].querySelector(":scope > pre")).whiteSpace : null,
    codeLanguages: codes.slice(-2).map((code) => code.querySelector(".helper-code-lang")?.textContent ?? ""),
    codeWord: t("mdview.code", "코드"),
    // The rail's handle names the window's note by its line.
    noteRail: railRowWords(list.querySelector(":scope > .is-window-note")),
    noteWords: t("worker.windowNote", "창 알림 · 창이 다시 시작되어 이어 가라고 알렸습니다"),
    personRight: person ? Math.round(column.right - box(person).right) : null,
    personInside: person ? inside(person) : false,
    answerDots: answers.filter((row) => ["::before", "::after"].some((part) => getComputedStyle(row, part).content !== "none")).length,
    say,
    sayWhole: t("composer.placeholder", "다음 지시를 쓰세요. 일하는 중이면 대기열에 들어갑니다"),
    sayShort: t("composer.placeholderShort", "다음 지시를 쓰세요"),
    rowsInView: list.querySelectorAll(":scope > [data-turn]").length,
    listElements: list.getElementsByTagName("*").length,
    stepRows: list.querySelectorAll(".helper-turn:is(.is-tool, .is-thinking)").length,
    sticky: [...list.querySelectorAll(":scope > .helper-turn")].filter((row) => getComputedStyle(row).position === "sticky").length,
    // The person's last words flow with the rows (t-81786 P3-17): no ground
    // of their own over the rows, no fade hanging under them.
    personFlows: (() => {
      const row = person?.parentElement;
      if (!row) return null;
      const style = getComputedStyle(row);
      return {
        position: style.position,
        ground: style.backgroundColor,
        fade: getComputedStyle(row, "::after").content,
      };
    })(),
    contrast: {
      latest: ink(said),
      earlier: ink(earlier?.querySelector(":scope > .helper-said")),
      pill: ink(pills[0]?.querySelector(".helper-group-words")),
      note: ink(note),
      code: ink(codes[0]?.querySelector(".helper-code-lang")),
      stack: ink(page.querySelector(".chat-stack")),
    },
  };
}

export async function testConversationB(browser, origin, ok, { shots = process.env.CONVERSATION_B_SHOTS ?? "" } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    await openConversation(page, draftTurns());
    if (shots) mkdirSync(shots, { recursive: true });
    const seen = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, theme);
      for (const width of B_WIDTHS) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate(async () => {
          for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
        });
        seen[`${theme}-${width}`] = await page.evaluate(readPage);
        if (shots && SHOT_WIDTHS.includes(width)) {
          await page.screenshot({ path: join(shots, `conversation-b-${theme}-${width}.png`) });
        }
      }
    }
    const all = Object.values(seen);
    ok(`B: at ${B_WIDTHS.join(", ")} px, in both treatments, nothing pushes the page or the column sideways`,
      all.every((one) => one.pageOverflow === 0 && one.listOverflow === 0),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pageOverflow, one.listOverflow]]))));
    ok("B: on a page under 560 px a line of code folds inside its block — nothing to scroll sideways inside it — and a wider page keeps the line whole, to scroll",
      all.every((one) => (one.pageWidth < 560 ? one.codeOverflow === 0 && one.codeWrap === "pre-wrap" : one.codeWrap === "pre")) &&
        all.some((one) => one.pageWidth < 560),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pageWidth, one.codeOverflow, one.codeWrap]]))));
    ok("B: the pills, the code blocks and the person's bubble stay inside the column at every width — the bubble on its right edge",
      all.every((one) => one.pillsInside && one.codesInside && one.personInside && one.personRight <= 1),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pillsInside, one.codesInside, one.personInside, one.personRight]]))));
    const dark = (width) => seen[`dark-${width}`];
    // The steps are the PAGE's width, not the window's: the window's side
    // columns take their share first.
    const step = (one) => (one.pageWidth < 560 ? "narrow" : one.pageWidth >= 1000 ? "wide" : "middle");
    ok("B: the reading steps with the page's own width — a step smaller under 560 px, more air between lines from 1000 px, the column never wider than its measure",
      B_WIDTHS.every((width) => {
        const one = dark(width);
        const want = step(one);
        return one.columnWidth > 0 && one.columnWidth <= one.measure &&
          (want === "narrow" ? one.proseSize === 14 : one.proseSize === 16) &&
          (want === "wide" ? one.proseLeading === 1.6 : one.proseLeading === 1.5);
      }) && new Set(B_WIDTHS.map((width) => step(dark(width)))).size >= 2,
      JSON.stringify(B_WIDTHS.map((width) => [width, dark(width).pageWidth, dark(width).proseSize, dark(width).proseLeading, dark(width).columnWidth])));
    ok("B: a wide page widens its column to the step its width reached — 880, 960, 1040, then at most 1120 px — and the composer and the fade under it follow; under 1000 px the column stays at 760 px (t-81786 P3-16)",
      all.every((one) => one.measure > 0 && one.dockMax === one.measure && one.fadeMax === one.measure &&
        (one.pageWidth >= 1000 ? Math.abs(one.columnWidth - one.measure) <= 1 : one.columnWidth <= one.baseMeasure)) &&
        new Set(B_WIDTHS.map((width) => dark(width).measure)).size >= 3,
      JSON.stringify(B_WIDTHS.map((width) => [width, dark(width).pageWidth, dark(width).measure, dark(width).columnWidth, dark(width).dockMax, dark(width).fadeMax])));
    ok("B: the empty box's sentence stays on its one line at every width — the whole of it where it fits, its first half on a box too narrow for it, cut short rather than folded under the box",
      all.every((one) => one.say?.fits && one.say.oneLine) &&
        dark(390).say.words === dark(390).sayShort && dark(1440).say.words === dark(1440).sayWhole,
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, one.say]))));
    ok("B: the person's last words flow with the rows — no row sticks to the list's top, none paints a ground over the rows or hangs a fade under them (t-81786 P3-17, the person's call of 10-29)",
      all.every((one) => one.sticky === 0 && one.personFlows && !["sticky", "fixed"].includes(one.personFlows.position) &&
        one.personFlows.ground === "rgba(0, 0, 0, 0)" && one.personFlows.fade === "none"),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.sticky, one.personFlows]]))));
    ok("B: an answer wears no dot and the tool work stands as pills with no step built",
      all.every((one) => one.answerDots === 0 && one.stepRows === 0),
      JSON.stringify(dark(1440)));
    ok("B: the rail's handle, standing on the window's note, says the note's line — not nothing, not its door",
      dark(1440).noteRail === dark(1440).noteWords, JSON.stringify([dark(1440).noteRail, dark(1440).noteWords]));
    ok("B: a code block opens with the language its fence names — 「코드」 when it names none",
      dark(1440).codeLanguages[0] === "typescript" && dark(1440).codeLanguages[1] === dark(1440).codeWord,
      JSON.stringify(dark(1440).codeLanguages));
    for (const theme of ["dark", "light"]) {
      const read = seen[`${theme}-1440`].contrast;
      ok(`B: every word reads at 4.5:1 or more in the ${theme} treatment — the newest answer, the ones before it, a pill, the window's note, a code band, the state line`,
        Object.values(read).every((ratio) => ratio === null || ratio >= 4.5) && read.latest !== null && read.earlier !== null && read.pill !== null && read.note !== null,
        JSON.stringify(read));
    }
    ok("B: the page raised no errors", faults.length === 0, faults.join("\n"));
    return seen;
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    const seen = await testConversationB(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
    if (process.argv.includes("--json")) console.log(JSON.stringify(seen));
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
