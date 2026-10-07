/* The scripted hands' eyes on the long-form phone (t-37883): where the
 * phone shows a thing, read from its page — the stand-in for what a model
 * reads off the picture. Shared by the bench's self-test and its scripted
 * drivers; the hand itself only ever gets display points or words.
 */

// How long the eyes give the phone after the mirror's lag: its sheets and
// keyboard slide for 300 ms.
export const SLIDE_SETTLE_MS = 400;

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// What the eyes look for on the phone, each a body run in the page that
// answers an element (`arg` is its argument).
export const SEES = {
  // The control that reads `arg` — in an open sheet or keyboard first, else
  // on the screen behind them.
  words: `const hits = [...document.querySelectorAll("button, span, div, input")].filter((element) =>
      [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim() === arg) || element.placeholder === arg);
    const live = hits.filter((element) => element.closest(".sheet.up, .keyboard.up"));
    return (live.length ? live : hits.filter((element) => !element.closest(".sheet, .keyboard")))[0];`,
  field: "return document.getElementById(arg);",
  // A row's button (segment, check, stepper, switch) by the row's own words.
  choice: `const row = [...document.querySelectorAll(".question, .label, .consent")].find((each) => each.textContent === arg.label).closest(".row");
    return arg.words === null ? row.querySelector(".switch") :
      [...row.querySelectorAll("button")].find((each) => each.textContent.replace("✓", "") === arg.words);`,
};

/* The display point where the phone shows what `find` locates, after the
 * phone has answered every input sent: near the start of a control (a
 * finger presses a row's words, not its middle). With `scroll`, the eyes
 * scroll it into view first (the self-test's shortcut for the scrolling an
 * agent does). With `words`, the point is the middle of the element's own
 * run of text — where an OCR line of those words has its centre, which a
 * press by words lands on before its nudge. `hit` says what stands at the
 * point, for a trace. */
export async function pointOf(desk, find, arg, { scroll = true, words = false } = {}) {
  await desk.settled();
  await sleep(desk.scene.lagMs + SLIDE_SETTLE_MS);
  const box = await desk.content().evaluate(({ source, arg, scroll, words }) => {
    const element = new Function("arg", source)(arg);
    if (!element) return null;
    if (scroll) element.scrollIntoView({ block: "center" });
    let x, y;
    const run = words && [...element.childNodes].find((node) => node.nodeType === 3 && node.textContent.trim());
    if (run) {
      const range = document.createRange();
      range.selectNodeContents(run);
      const rect = range.getBoundingClientRect();
      [x, y] = [rect.x + rect.width / 2, rect.y + rect.height / 2];
    } else {
      const rect = element.getBoundingClientRect();
      [x, y] = [rect.x + Math.min(rect.width / 2, 40), rect.y + rect.height / 2];
    }
    // In view is what a person could press there: the element itself on top
    // at that point, not one a list clips or the keyboard covers.
    const hit = document.elementFromPoint(x, y);
    const said = hit ? `${hit.tagName.toLowerCase()}.${String(hit.className || "").split(" ")[0]} "${String(hit.textContent || hit.value || "").trim().slice(0, 30)}"` : "nothing";
    return { x, y, inView: Boolean(hit) && (element.contains(hit) || hit.contains(element)), hit: said };
  }, { source: find, arg, scroll, words });
  if (!box) throw new Error(`nothing on the phone answers ${find} (${JSON.stringify(arg)})`);
  return { ...toDisplay(desk, box), inView: box.inView, hit: box.hit };
}

/* What the page says about a run, read before the desk closes: its own
 * errors, the alert shown, and the shown required fields still blank — for
 * a run the oracle failed. */
export async function pageSays(desk) {
  return desk.content().evaluate(() => ({
    errors: LongForm.errors(() => false).map((error) => error.message),
    alert: document.getElementById("alert")?.textContent || null,
    unset: LongForm.fields().filter(LongForm.shown).filter((field) => field.required && LongForm.blank(field)).map((field) => field.id),
  }));
}

/* A point in the phone's own CSS pixels, on the display. */
export function toDisplay(desk, point) {
  const { window: win, screen } = desk.scene;
  const scale = win.width / screen.width;
  return { x: win.x + point.x * scale, y: win.y + screen.top + point.y * scale };
}

/* Where things stand once the phone has answered every input sent. */
export async function settledLayout(desk) {
  await desk.settled();
  await sleep(desk.scene.lagMs + SLIDE_SETTLE_MS);
  return layout(desk);
}

/* What changes where things stand on the phone: a sheet or the keyboard up,
 * how far the form is scrolled, how far an open sheet's lists are. */
export function layout(desk) {
  return desk.content().evaluate(() => JSON.stringify({
    sheet: document.querySelector(".sheet.up .bar span")?.textContent || null,
    picking: Boolean(document.querySelector(".sheet.up .years")),
    keyboard: Boolean(document.querySelector(".keyboard.up")),
    scrollY: Math.round(scrollY),
    lists: [...document.querySelectorAll(".sheet.up .list, .sheet.up .col")].map((list) => Math.round(list.scrollTop)),
    alert: Boolean(document.querySelector(".alert.up")),
  }));
}
