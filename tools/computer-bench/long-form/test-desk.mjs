/* The long-form bench end to end without a model (t-37883): the pages in
 * Chromium, the fake desk, the fake shim and the oracle. A scripted hand that
 * fills the card through the desktop verbs an agent uses passes on both
 * scenes — on the phone by pixels alone, through the mirror's lag; a run
 * that does nothing fails; a value written without the events a person's
 * input makes is scored wrong, because the pages keep their own state.
 *
 *   NODE_PATH=…/node_modules node tools/computer-bench/long-form/test-desk.mjs
 */

import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { startDesk } from "./desk.mjs";
import { SEES, pageSays, pointOf, settledLayout } from "./phone-eyes.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SPEC = JSON.parse(await readFile(join(HERE, "spec.json"), "utf8"));
const EXPECTED = SPEC.card.expected;
const FIELDS = SPEC.sections.flatMap((section) => section.fields);
const PERSON_ATTACH_MS = 10;
const PHONE = "iPhone Mirroring";
const CHROME = "Google Chrome";
// How long a pressed submit takes to post its answer to the local server.
const SUBMIT_SETTLE_MS = 500;
let failures = 0;

function check(name, held, detail = "") {
  console.log(`${held ? "PASS" : "FAIL"}  ${name}${held ? "" : `  ${detail}`}`);
  if (!held) failures += 1;
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const option = (field, code) => SPEC.options[field.options].find(([value]) => value === code)[1];
const monthName = (month) => new Date(Date.UTC(2000, Number(month) - 1, 1)).toLocaleString("en", { month: "long", timeZone: "UTC" });

async function standing(scene, fn, deskOptions = {}) {
  const out = await mkdtemp(join(tmpdir(), "long-form-"));
  const server = spawn("python3", [join(HERE, "server.py"), "--out", out], { stdio: ["ignore", "pipe", "inherit"] });
  const url = await new Promise((done) => server.stdout.on("data", (chunk) => {
    const said = String(chunk).match(/listening (\S+)/);
    if (said) done(said[1]);
  }));
  const desk = await startDesk({ url, out, personAttachMs: PERSON_ATTACH_MS, scene, ...deskOptions });
  let said = null;
  try {
    await fn({ desk, out });
    await desk.settled();
    // A submit's post leaves after the press reached the page.
    await sleep(SUBMIT_SETTLE_MS);
    said = await pageSays(desk).catch(() => null);
  } finally {
    await desk.close();
    server.kill();
  }
  const scored = JSON.parse(spawnSync("python3", [join(HERE, "oracle.py"), "--out", out, "--json"], { encoding: "utf8" }).stdout);
  await rm(out, { recursive: true, force: true });
  return { ...scored, said };
}

/* The fake shim, called as an agent calls it — never synchronously: the
 * desk answering it runs in this same process. */
function shim(desk, argv) {
  return new Promise((done) => {
    const child = spawn(join(HERE, "bin", "zerocode-computer"), argv, {
      env: { ...process.env, LONG_FORM_DESK: `http://127.0.0.1:${desk.port}` },
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("close", (code) => done({ code, out, err }));
  });
}

const click = (desk, [x, y]) => shim(desk, ["mouse-click", "--x", String(x), "--y", String(y), "--mouse-button", "left", "--click-count", "1", "--json"]);
const type = (desk, text) => shim(desk, ["type", "--text", text, "--json"]);

/* ---- the web page, through its accessibility tree ----------------------- */

function chromeSteps() {
  const steps = [];
  const byLabel = (label, ...more) => ["click", "--app", CHROME, "--label", label, ...more, "--json"];
  for (const field of FIELDS.filter((each) => !each.shows_when && each.kind !== "yesno")) {
    const value = EXPECTED[field.id];
    const focus = byLabel(field.label);
    const typed = (text) => ["type", "--text", text, "--json"];
    const [year, month, day] = String(value).split("-");
    if (["text", "email", "tel", "textarea"].includes(field.kind)) { if (value) steps.push(focus, typed(value)); }
    else if (field.kind === "date") steps.push(focus, typed(`${month}${day}${year}`));
    else if (field.kind === "datepick") steps.push(focus, typed(`${day}/${month}/${year}`));
    else if (field.kind === "select") steps.push(focus, typed(option(field, value)));
    else if (field.kind === "combobox") steps.push(focus, typed(option(field, value).slice(0, 8)), ["key", "--key", "Return", "--json"]);
    else if (field.kind === "radio") steps.push(byLabel(option(field, value), "--role", "radio"));
    else if (field.kind === "counter") for (let count = 0; count < value; count += 1) steps.push(byLabel(`Increase: ${field.label}`));
    else if (field.kind === "checkbox" && value) steps.push(byLabel(field.label, "--role", "checkbox"));
  }
  return steps;
}

async function fillChrome(desk) {
  const answers = [];
  for (const step of chromeSteps()) answers.push(await shim(desk, step));
  // The yes/no questions all read "Yes" and "No": pressed where they stand.
  for (const field of FIELDS.filter((each) => each.kind === "yesno")) {
    const radio = desk.page.locator(`#${field.id}-${EXPECTED[field.id]}`);
    await radio.scrollIntoViewIfNeeded();
    const box = await radio.boundingBox();
    answers.push(await click(desk, [box.x + 2, box.y + 2]));
  }
  const other = FIELDS.find((each) => each.shows_when);
  answers.push(await shim(desk, ["click", "--app", CHROME, "--label", other.label, "--json"]));
  answers.push(await type(desk, option(other, EXPECTED[other.id])));
  answers.push(await shim(desk, ["handoff", "--reason", "attach the passport scan", "--json"]));
  answers.push(await shim(desk, ["click", "--app", CHROME, "--label", "Submit registration", "--json"]));
  return answers;
}

/* ---- the phone, by pixels alone ------------------------------------------ */

/* Every tap of the pixel road, and what stood where it landed — said when
 * the road's run fails. */
const taps = [];
async function tap(desk, find, arg, options) {
  const { x, y, hit } = await pointOf(desk, find, arg, options);
  // What the form held when this tap was aimed — after the last one landed.
  const held = await desk.content().evaluate(() => JSON.stringify(LongForm.state)).catch(() => "{}");
  taps.push({ arg, hit, held: JSON.parse(held) });
  return click(desk, [x, y]);
}

/* The pixel road's taps, each with what the one before it changed in the
 * form's state — said when the road's run fails. */
function tapTrace() {
  return taps.map((each, at) => {
    const before = at ? taps[at - 1].held : {};
    const changed = Object.keys(each.held).filter((key) => JSON.stringify(each.held[key]) !== JSON.stringify(before[key]));
    return `${JSON.stringify(each.arg).slice(0, 40)} → ${each.hit}${at ? ` | before it: ${changed.map((key) => `${key}=${JSON.stringify(each.held[key])}`).join(", ")}` : ""}`;
  });
}

async function fillPhone(desk) {
  const answers = [];
  for (const field of FIELDS) {
    const value = EXPECTED[field.id];
    const [year, month, day] = String(value).split("-");
    if (["text", "email", "tel", "textarea", "datepick"].includes(field.kind)) {
      if (!value) continue;
      answers.push(await tap(desk, SEES.field, field.id));
      answers.push(await type(desk, field.kind === "datepick" ? `${day}${month}${year}` : value));
      answers.push(await tap(desk, SEES.words, "Done"));
    } else if (field.kind === "select" || field.kind === "combobox") {
      answers.push(await tap(desk, SEES.words, field.label));
      if (SPEC.options[field.options].length > 12) {
        answers.push(await tap(desk, SEES.words, "Search"));
        answers.push(await type(desk, option(field, value)));
      }
      answers.push(await tap(desk, SEES.words, option(field, value)));
    } else if (field.kind === "date") {
      const now = new Date();
      answers.push(await tap(desk, SEES.words, field.label));
      answers.push(await tap(desk, SEES.words, `${monthName(now.getUTCMonth() + 1)} ${now.getUTCFullYear()} ›`));
      answers.push(await tap(desk, SEES.words, year));
      answers.push(await tap(desk, SEES.words, monthName(month)));
      answers.push(await tap(desk, SEES.words, `${monthName(month)} ${year} ⌃`));
      answers.push(await tap(desk, SEES.words, String(Number(day))));
      answers.push(await tap(desk, SEES.words, "Done"));
    } else if (field.kind === "radio" || field.kind === "yesno") {
      answers.push(await tap(desk, SEES.choice, { label: field.label, words: option(field, value) }));
    } else if (field.kind === "counter") {
      for (let count = 0; count < value; count += 1) answers.push(await tap(desk, SEES.words, "+"));
    } else if (field.kind === "checkbox" && value) {
      answers.push(await tap(desk, SEES.choice, { label: field.label, words: null }));
    } else if (field.kind === "file") {
      // The person takes the phone once the road's last tap has reached it:
      // their attach closes whatever sheet is open, and a tap still in the
      // mirror's lag would land on the form behind it (the second
      // nationality's choice did, with this test's 10 ms person).
      await desk.settled();
      answers.push(await shim(desk, ["handoff", "--reason", "attach the passport photo", "--json"]));
    }
  }
  answers.push(await tap(desk, SEES.words, "Submit registration"));
  return answers;
}

/* ---- the checks ------------------------------------------------------------ */

for (const scene of ["phone", "chrome"]) {
  const idle = await standing(scene, async () => {});
  check(`${scene}: a run that did nothing fails`, idle.pass === false && idle.submits === 0, JSON.stringify(idle.reasons));
}

const silent = await standing("chrome", async ({ desk }) => {
  await desk.page.evaluate(() => { document.getElementById("family_name").value = "SAMPLE"; });
  await desk.page.evaluate(() => document.getElementById("form").requestSubmit());
});
check("chrome: a value written without its events is not what the page holds", silent.submits === 0 || silent.fields.family_name !== "ok",
  JSON.stringify(silent));

const chrome = await standing("chrome", async ({ desk }) => {
  const refused = (await fillChrome(desk)).filter((answer) => answer.code !== 0);
  check("chrome: every scripted step was answered", refused.length === 0, refused.map((answer) => answer.err).slice(0, 3).join(" | "));
});
check("chrome: the card filled through the desk passes the oracle", chrome.pass === true, JSON.stringify([chrome.reasons, chrome.said]));

let lastHeld = null;
const phone = await standing("phone", async ({ desk }) => {
  const refused = (await fillPhone(desk)).filter((answer) => answer.code !== 0);
  await desk.settled();
  lastHeld = { ...(await desk.content().evaluate(() => ({ second: LongForm.state.second_nationality, dual: LongForm.state.decl_dual }))),
    landed: desk.tally.landed.slice(-6) };
  check("phone: every scripted tap was answered", refused.length === 0, refused.map((answer) => answer.err).slice(0, 3).join(" | "));
});
check("phone: the card tapped in by pixels passes the oracle", phone.pass === true,
  JSON.stringify([phone.reasons, phone.said, tapTrace().slice(-8), lastHeld]));

await standing("phone", async ({ desk }) => {
  const names = ["iPhone 미러링", "com.apple.ScreenContinuity", PHONE];
  const looks = [];
  for (const name of names) looks.push(JSON.parse((await shim(desk, ["observe", "--diff", "--app", name, "--json"])).out));
  check("phone: the window answers to its English name, its Korean name and its bundle id",
    looks.every((look) => look.ok && look.result.screenshot.width === 436 && look.result.screenshot.height === 958),
    JSON.stringify(looks.map((look) => look.result && look.result.screenshot)));
  check("phone: the mirror has no tree beyond its window", looks[0].result.tree.elementCount === 1, JSON.stringify(looks[0].result.tree));
  const unknown = await shim(desk, ["observe", "--app", "Notes", "--json"]);
  check("phone: an app that is not there is refused", unknown.code === 1 && JSON.parse(unknown.err).error.code === "app_not_found", unknown.err);
  const before = await desk.content().evaluate(() => LongForm.state.companions);
  const plus = await pointOf(desk, SEES.words, "+");
  const sent = Date.now();
  await click(desk, [plus.x, plus.y]);
  const early = await desk.content().evaluate(() => LongForm.state.companions);
  await desk.settled();
  const late = await desk.content().evaluate(() => LongForm.state.companions);
  check("phone: a tap reaches the phone only after the mirror's lag", early === before && late === before + 1 && Date.now() - sent >= desk.scene.lagMs,
    `${before} → ${early} → ${late}`);
});

// The window with t-37883's levers (needs the core's `words_choose` example,
// named by WORDS_CHOOSE): a press by the words the phone shows, a wait after
// it that ends once the screen holds still, and looks that say who they saw.
if (process.env.WORDS_CHOOSE) {
  await standing("phone", async ({ desk }) => {
    const ask = (argv) => shim(desk, argv);
    const opened = await ask(["click", "--app", PHONE, "--ocr", "--text", "Title", "--json"]);
    check("levers: a click by the words the phone shows presses them", opened.code === 0, opened.err);
    const sheet = await ask(["wait-for", "--app", PHONE, "--ocr", "--text", "Cancel", "--timeout-ms", "3000", "--json"]);
    check("levers: an OCR wait sees the sheet the press opened", sheet.code === 0, sheet.err);
    await ask(["click", "--app", PHONE, "--ocr", "--text", "Mr", "--json"]);
    // The sheet slides away after the press reaches the phone: what the eyes
    // wait out before they read the screen behind it.
    await settledLayout(desk);
    check("levers: the words chose the option", (await desk.content().evaluate(() => LongForm.state.title)) === "mr");
    // "name" is held by three labels on the first screen and read exactly by none.
    const twice = await ask(["click", "--app", PHONE, "--ocr", "--text", "name", "--json"]);
    const shown = twice.code === 1 && JSON.parse(twice.err).error.code === "ambiguous_target" ? ""
      : JSON.parse((await ask(["read", "--app", PHONE, "--ocr", "--json"])).out || "{}").result?.text;
    check("levers: words that recur are refused by name", !shown, `${twice.err} | read: ${shown}`);
    const batch = JSON.parse((await ask(["batch", "--commands", JSON.stringify([
      ["click", "--app", PHONE, "--ocr", "--text", "Male", "--json"], ["wait", "--ms", "4000", "--json"]]), "--json"])).out);
    const waited = batch.result.steps[1].result;
    check("levers: a batch's wait after a press ends once the phone holds still", waited.settled === true && waited.waitedMs < 4000,
      JSON.stringify(waited));
    const look = JSON.parse((await ask(["observe", "--diff", "--app", "iPhone 미러링", "--json"])).out);
    check("levers: a look says the app the window resolved", look.result.app.bundleId === "com.apple.ScreenContinuity", JSON.stringify(look.result.app));
  }, { window: "levers", wordsChoose: process.env.WORDS_CHOOSE });
} else {
  console.log("SKIP  levers: WORDS_CHOOSE names no words_choose example");
}

await standing("chrome", async ({ desk }) => {
  const commands = JSON.stringify([["wait", "--ms", "1", "--json"], ["click", "--app", CHROME, "--label", "No such control", "--json"], ["wait", "--ms", "1", "--json"]]);
  const answer = await shim(desk, ["batch", "--commands", commands, "--json"]);
  const said = JSON.parse(answer.err || "{}");
  check("a batch stops at its first refusal and says where", answer.code === 1 && said.result.refusedAt === 2 && said.result.ran === 1, answer.err);
});

console.log(failures ? `${failures} FAILED` : "all passed");
process.exit(failures ? 1 : 0);
