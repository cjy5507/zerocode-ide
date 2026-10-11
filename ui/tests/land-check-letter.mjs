import { openWindowTestPage } from "./window-boot.mjs";

/* A landing check's letter on the coordinator's desk, as the window words it (t-42447, t-34501 stage 3).
 *
 * The window runs the check and the ledger writes its evidence as the `land_check` letter. What stands
 * here is the window's half, read off the desk's own functions: the word the kind wears, and the one
 * line each verdict reads in, in every language the window speaks. A verdict the window does not know
 * reads as a merge that did not finish, never as a pass. */

const LOCALES = ["ko", "en", "ja", "zh", "es"];
const VERDICTS = ["passed", "merged", "conflict", "failed", "timed_out", "unstartable", "abandoned", "error"];

export async function testLandCheckLetter({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });

    const words = {};
    for (const locale of LOCALES) {
      words[locale] = await page.evaluate((code) => {
        setLocale(code, { persist: false });
        const now = Date.now();
        const letter = (verdict, over = {}) => deskLetterDetail({ kind: "land_check", task: "t-1", verdict, ...over }, now);
        const mail = DESK_MAIL.land_check;
        return {
          mail: mail ? { state: mail.state, word: t(mail.key, mail.word) } : null,
          passed: letter("passed", { rc: 0 }),
          merged: letter("merged"),
          conflict: letter("conflict", { clashes: 2 }),
          failed: letter("failed", { rc: 3 }),
          timed_out: letter("timed_out"),
          unstartable: letter("unstartable"),
          abandoned: letter("abandoned"),
          error: letter("error"),
          unknown: letter("something_new"),
          noRc: letter("failed", { rc: null }),
        };
      }, locale);
    }

    const ko = words.ko;
    ok(
      "a land_check letter wears the word 착지 전 점검 on the desk, and its verdict says the outcome in Korean with the numbers it carries",
      ko.mail?.word === "착지 전 점검" && ko.mail?.state === "needs-attention" &&
        ko.passed === "점검 통과 (rc 0)" && ko.merged === "충돌 없이 합쳐짐 — 점검 명령은 없음" &&
        ko.conflict === "충돌 2개 — 합치면 겹쳐요" && ko.failed === "점검 실패 (rc 3)" &&
        ko.timed_out === "점검 시간 초과 — 프로세스를 멈췄어요" && ko.unstartable === "점검을 시작하지 못했어요" &&
        ko.abandoned === "창이 끝나 점검이 중간에 끊겼어요" && ko.error === "합치기를 끝내지 못했어요",
      JSON.stringify(ko),
    );

    ok(
      "a verdict the window does not know reads as a merge that did not finish, never as a pass, and a failed run with no exit code says so with a question mark, never a guessed number",
      ko.unknown === ko.error && ko.unknown !== ko.passed && ko.noRc === "점검 실패 (rc ?)",
      JSON.stringify({ unknown: ko.unknown, noRc: ko.noRc }),
    );

    const distinct = LOCALES.every((code) => {
      const heard = VERDICTS.map((verdict) => words[code][verdict]);
      return heard.every((line) => typeof line === "string" && line.length > 0) && new Set(heard).size === heard.length;
    });
    ok(
      "every verdict has its own line in each of the five languages, and no two verdicts in one language share a line",
      distinct,
      JSON.stringify(Object.fromEntries(LOCALES.map((code) => [code, VERDICTS.map((verdict) => words[code][verdict])]))),
    );

    const translated = ["en", "ja", "zh", "es"].every((code) =>
      VERDICTS.every((verdict) => words[code][verdict] !== ko[verdict]) && words[code].mail?.word !== ko.mail?.word);
    ok(
      "the four translated languages say each line in their own words, not the Korean one",
      translated,
      JSON.stringify(Object.fromEntries(["en", "ja", "zh", "es"].map((code) => [code, words[code].passed]))),
    );

    const conflictLines = LOCALES.map((code) => words[code].conflict);
    ok(
      "a conflict names its count in every language, so the desk says how many files clash without a number of its own making",
      LOCALES.every((code, index) => conflictLines[index].includes("2")),
      JSON.stringify(conflictLines),
    );

    await page.evaluate(() => { setLocale("ko", { persist: false }); });
  } finally {
    await page.close();
  }
}
