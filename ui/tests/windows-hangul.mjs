/* Hangul as Windows draws it, on any machine.
 *
 * The window's type chain (`--font-ui` in tokens.css) names "Apple SD Gothic Neo" for Hangul, and Geist leaves
 * those letters out. On macOS they land on Apple SD Gothic Neo, whose syllables are 0.865 em wide; on Windows no
 * face in the chain carries them and Chromium draws them in Malgun Gothic, a full 1 em. The same Korean words are
 * 16% wider there: the Windows CI leg measured 「main과 겹침 1 · 커밋 없음 · 저장 안 한 변경」 19.3 px longer at 11 px —
 * 13 syllables of 1.49 px each, (1 - 0.865) x 11 — and cut chips, wrapped rows and an obscured button followed
 * that only a Windows run could see (t-90089 r3).
 *
 * This puts a face of that width under the macOS name for Hangul alone: AppleGothic (macOS, 1 em), or Malgun
 * Gothic itself where it is installed. Elsewhere neither loads and the chain goes on as before. One weight range
 * for every weight, so 500 and 600 draw the same advances and nothing is synthesised. A layout check that must
 * hold for every person who reads Korean runs under it, so a Mac run fails where a Windows person's window would. */
const HANGUL = "U+1100-11FF, U+3130-318F, U+A960-A97F, U+AC00-D7AF, U+D7B0-D7FF";

export const WINDOWS_HANGUL_FACE = `@font-face {
  font-family: "Apple SD Gothic Neo";
  src: local("Malgun Gothic"), local("AppleGothic");
  font-weight: 100 900;
  font-style: normal;
  unicode-range: ${HANGUL};
}`;

/** Draws the page's Hangul at the width Windows draws it, waits until the face is in use, and refuses to go on
 * when it is not: a syllable in the window's own type must measure a whole em, or the check after it would pass
 * on the narrower face it exists to rule out. */
export async function drawHangulAsWindows(page) {
  await page.addStyleTag({ content: WINDOWS_HANGUL_FACE });
  const em = await page.evaluate(async () => {
    await document.fonts.load('500 11px "Apple SD Gothic Neo"', "가");
    await document.fonts.ready;
    const probe = document.createElement("span");
    probe.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap;font:500 100px var(--font-ui)";
    probe.textContent = "가나다라마바사아자차";
    document.body.append(probe);
    const width = probe.getBoundingClientRect().width / 1000;
    probe.remove();
    return width;
  });
  if (Math.abs(em - 1) > 0.02) {
    throw new Error(`Hangul did not take Windows' width: a syllable measured ${em.toFixed(3)} em, not 1 (no Malgun Gothic or AppleGothic here)`);
  }
}
