/* The long-form bench's scripted drivers (t-37883): what filling the phone
 * form takes in model round trips, counted without asking a model — so the
 * levers' before and after can be read without spending a login.
 *
 *   node drive.mjs --policy today|levers --out DIR [--words-choose PATH]
 *                  [--window-source computer_use.rs]
 *
 * `--window-source` is the core file the window's numbers are read from —
 * for `today`, the base commit's (its look's settle cap is 1 s).
 *
 * A model's round trip is one tool call (a batch, a look of its own, a
 * hand-off) plus the final answer. Each driver plays the best a model could
 * do with one set of tools, and the oracle scores what it filled:
 *
 * - `today`: the installed window and zo. Presses go by coordinates, which
 *   come only from a picture taken since the layout last changed (a sheet or
 *   the keyboard opened or closed, the form or a list scrolled). A batch runs
 *   until a step needs such a picture; zo's look after the batch (the whole
 *   desktop) is the next picture, and when it was taken mid-motion
 *   (`settled: false`) the driver looks at the window again, as the person's
 *   session did. No waits and no activates: the habits that cost the session
 *   401 s and 114 steps are left out, so this is the floor of today's road.
 * - `levers`: this branch. Presses go by the words the phone shows (`click
 *   --ocr`), each value is checked in the batch (`wait-for --ocr`), and a
 *   picture is needed only when the words to press next are not yet known: at
 *   the start, after a page scroll, and the first time each kind of screen
 *   opens (a choice sheet, a searchable sheet, the date sheet, its year list,
 *   the keyboard). zo's look after is the app's window, settled.
 *
 * Where a model reads a position or a nudge off the picture, the drivers read
 * it off the page (`phone-eyes.mjs`) — only after a picture was taken.
 */

import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { startDesk } from "./desk.mjs";
import { SEES, pageSays, pointOf, settledLayout, toDisplay } from "./phone-eyes.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SPEC = JSON.parse(await readFile(join(HERE, "spec.json"), "utf8"));
const EXPECTED = SPEC.card.expected;
const KLM = JSON.parse(spawnSync("python3", [join(HERE, "klm.py"), "--json"], { encoding: "utf8" }).stdout);
const PERSON_ATTACH_MS = Math.round(KLM.rows.find((row) => row.id === "passport_scan").seconds * 1000);
// One model round trip in the person's session: about 17 s (coordinator
// m-38845: "한 번 약 17초" — 55 min, 122 requests, 80 % of it model time).
const SECONDS_PER_ROUND_TRIP = 17;
const PHONE = "iPhone Mirroring";
// How long a check waits for the words it expects, at most.
const CHECK_MS = 3000;
// A wheel line in CSS pixels (Chromium's line).
const PIXELS_PER_LINE = 40;
// A page scroll moves the form by this share of the phone's height.
const PAGE_SHARE = 0.7;
// The most wheel turns a driver gives one target before it says it is lost.
const SCROLLS_MAX = 12;

const option = (field, code) => SPEC.options[field.options].find(([value]) => value === code)[1];
const monthName = (month) => new Date(Date.UTC(2000, Number(month) - 1, 1)).toLocaleString("en", { month: "long", timeZone: "UTC" });

function args(argv) {
  const parsed = {};
  for (let at = 0; at < argv.length; at += 2) parsed[argv[at].replace(/^--/, "").replace(/-(\w)/g, (_, c) => c.toUpperCase())] = argv[at + 1];
  if (!["today", "levers"].includes(parsed.policy) || !parsed.out) throw new Error("--policy today|levers --out DIR");
  return parsed;
}

class Driver {
  constructor(desk, policy) {
    this.desk = desk;
    this.policy = policy;
    this.count = { roundTrips: 0, batches: 0, ownLooks: 0, steps: 0, lookAfters: 0, unsettledLookAfters: 0, refused: 0 };
    this.open = false;
    this.lookedAt = null;
    this.known = new Set();
  }

  async call(argv) {
    const { body } = await this.desk.doors.computer([...argv, "--json"]);
    if (!body.ok) {
      this.count.refused += 1;
      throw new Error(`${argv.join(" ")}: ${body.error.code}: ${body.error.message}`);
    }
    return body.result;
  }

  /* One step of the open batch (or of a new one). */
  async step(argv) {
    if (!this.open) {
      this.open = true;
      this.count.roundTrips += 1;
      this.count.batches += 1;
    }
    this.count.steps += 1;
    return this.call(argv);
  }

  /* A picture before the next step: the open batch ends and zo's look
   * after it is that picture — or, with no batch open, a look of the
   * model's own. */
  async look() {
    if (this.open) {
      this.open = false;
      this.count.lookAfters += 1;
      const app = this.policy === "levers" ? ["--app", PHONE] : [];
      const seen = await this.call(["observe", "--diff", "--settle", ...app]);
      if (seen.settle && seen.settle.settled === false) {
        this.count.unsettledLookAfters += 1;
        await this.ownLook();
      }
    } else {
      await this.ownLook();
    }
    this.lookedAt = await settledLayout(this.desk);
  }

  async ownLook() {
    this.count.roundTrips += 1;
    this.count.ownLooks += 1;
    await this.call(["observe", "--diff", "--app", PHONE]);
  }

  /* `today`: a press at a point read off the picture, the picture taken
   * since the layout last changed — scrolling first when it is off screen. */
  async pressAt(find, arg) {
    if (this.lookedAt === null || this.lookedAt !== (await settledLayout(this.desk))) await this.look();
    let point = await pointOf(this.desk, find, arg, { scroll: false });
    for (let turns = 0; !point.inView; turns += 1) {
      if (turns === SCROLLS_MAX) throw new Error(`${JSON.stringify(arg)} never came into view`);
      const { lines, at } = await this.scrollTo(find, arg);
      await this.step(["mouse-scroll", "--x", String(at.x), "--y", String(at.y), "--dy", String(-lines)]);
      await this.look();
      point = await pointOf(this.desk, find, arg, { scroll: false });
    }
    await this.step(["mouse-click", "--x", String(point.x), "--y", String(point.y), "--mouse-button", "left", "--click-count", "1"]);
  }

  /* `levers`: a press by the words the phone shows, a picture taken only
   * when those words are not known yet. */
  async pressWords(kind, words, { after = null, nudge = null, find = SEES.words, arg = words, list = false } = {}) {
    await this.reveal(kind, find, arg, list);
    if (!this.known.has(kind)) {
      await this.look();
      this.known.add(kind);
    }
    let offset = [];
    if (nudge) {
      // From where a press by those words lands — its OCR line's centre —
      // to the control beside them, as a model reads both off the picture.
      const [from, to] = await Promise.all([pointOf(this.desk, SEES.words, nudge, { scroll: false, words: true }),
        pointOf(this.desk, find, arg, { scroll: false })]);
      offset = ["--dx", String(Math.round(to.x - from.x)), "--dy", String(Math.round(to.y - from.y))];
    }
    const said = nudge || words;
    await this.step(["click", "--app", PHONE, "--ocr", "--text", said, ...(after ? ["--after-text", after] : []), ...offset]);
  }

  /* `levers`: scroll until `find` shows — the form a page at a time, each
   * new page a screen whose words a picture teaches; a `list` (a sheet's
   * column) by as far as it is, its words (a year) known before they show. */
  async reveal(kind, find, arg, list) {
    for (let turns = 0; ; turns += 1) {
      const point = await pointOf(this.desk, find, arg, { scroll: false });
      if (point.inView) return;
      if (turns === SCROLLS_MAX) throw new Error(`${JSON.stringify(arg)} never came into view`);
      const page = Math.round((this.desk.scene.screen.height * PAGE_SHARE) / PIXELS_PER_LINE);
      const wheel = list ? await this.scrollTo(find, arg) : { lines: page, at: { x: point.x, y: this.middle() } };
      await this.step(["mouse-scroll", "--x", String(wheel.at.x), "--y", String(wheel.at.y), "--dy", String(-wheel.lines)]);
      if (!list) this.known.delete(kind);
    }
  }

  async check(words) {
    if (this.policy === "levers") await this.step(["wait-for", "--app", PHONE, "--ocr", "--text", words, "--timeout-ms", String(CHECK_MS)]);
  }

  async type(text) {
    await this.step(["type", "--text", text]);
  }

  middle() {
    const { window: win } = this.desk.scene;
    return win.y + win.height / 2;
  }

  /* How far, in wheel lines, `find` stands from the middle of what scrolls
   * it (a sheet's column or list, else the form), and the display point the
   * wheel turns at — what a model reads off the picture. */
  async scrollTo(find, arg) {
    const away = await this.desk.content().evaluate(({ source, arg }) => {
      const element = new Function("arg", source)(arg);
      const box = (element.closest(".col, .list") || document.documentElement).getBoundingClientRect();
      const view = { top: Math.max(0, box.top), bottom: Math.min(innerHeight, box.bottom), left: box.left, right: box.right };
      const rect = element.getBoundingClientRect();
      const middle = (view.top + view.bottom) / 2;
      return { by: rect.top + rect.height / 2 - middle, at: { x: (view.left + view.right) / 2, y: middle } };
    }, { source: find, arg });
    const lines = Math.sign(away.by) * Math.max(1, Math.round(Math.abs(away.by) / PIXELS_PER_LINE));
    return { lines, at: toDisplay(this.desk, away.at) };
  }
}

/* One field, the way each driver presses it. */
async function fill(driver, field) {
  const value = EXPECTED[field.id];
  const levers = driver.policy === "levers";
  const press = (kind, words, options = {}) => (levers ? driver.pressWords(kind, words, options)
    : driver.pressAt(options.find || SEES.words, options.arg ?? words));
  const [year, month, day] = String(value).split("-");
  if (["text", "email", "tel", "textarea", "datepick"].includes(field.kind)) {
    if (!value) return;
    await press("page", field.label, { nudge: field.label, find: SEES.field, arg: field.id });
    const typed = field.kind === "datepick" ? `${day}${month}${year}` : value;
    await driver.type(typed);
    await press("keyboard", "Done");
    await driver.check(field.kind === "datepick" ? `${day}/${month}/${year}` : value);
  } else if (field.kind === "select" || field.kind === "combobox") {
    const words = option(field, value);
    await press("page", field.label);
    if (SPEC.options[field.options].length > 12) {
      await driver.check("Cancel");
      await press("sheet:search", "Search");
      // All but its last letter: the field then shows other words than the row.
      await driver.type(words.slice(0, -1));
    } else {
      await driver.check("Cancel");
    }
    await press(SPEC.options[field.options].length > 12 ? "sheet:search" : "sheet:choice", words);
    await driver.check(words);
  } else if (field.kind === "date") {
    const now = new Date();
    await press("page", field.label);
    await driver.check("Done");
    await press("sheet:date", `${monthName(now.getUTCMonth() + 1)} ${now.getUTCFullYear()} ›`);
    await press("sheet:date-years", year, { list: true });
    await press("sheet:date-years", monthName(month));
    await press("sheet:date-years", `${monthName(month)} ${year} ⌃`);
    await press("sheet:date", String(Number(day)));
    await press("sheet:date", "Done");
    await driver.check(`${day}/${month}/${year}`);
  } else if (field.kind === "radio" || field.kind === "yesno") {
    await press("page", option(field, value), { after: field.label, find: SEES.choice, arg: { label: field.label, words: option(field, value) } });
  } else if (field.kind === "counter") {
    for (let count = 0; count < value; count += 1) await press("page", "+", { after: field.label });
  } else if (field.kind === "checkbox" && value) {
    await press("page", field.label, { nudge: field.label, find: SEES.choice, arg: { label: field.label, words: null } });
  } else if (field.kind === "file") {
    driver.open = false;
    driver.count.roundTrips += 1;
    await driver.call(["handoff", "--reason", "attach the passport photo"]);
    driver.lookedAt = null;
  }
}

async function main() {
  const options = args(process.argv.slice(2));
  const out = resolve(options.out);
  await mkdir(out, { recursive: true });
  const server = spawn("python3", [join(HERE, "server.py"), "--out", out], { stdio: ["ignore", "pipe", "inherit"] });
  const url = await new Promise((done) => server.stdout.on("data", (chunk) => {
    const said = String(chunk).match(/listening (\S+)/);
    if (said) done(said[1]);
  }));
  const desk = await startDesk({ url, out, personAttachMs: PERSON_ATTACH_MS, scene: "phone", window: options.policy,
    wordsChoose: options.wordsChoose ? resolve(options.wordsChoose) : null,
    windowSource: options.windowSource ? resolve(options.windowSource) : undefined });
  const driver = new Driver(desk, options.policy);
  const started = Date.now();
  let failed = null;
  try {
    for (const field of SPEC.sections.flatMap((section) => section.fields)) await fill(driver, field);
    await (driver.policy === "levers" ? driver.pressWords("page", "Submit registration") : driver.pressAt(SEES.words, "Submit registration"));
    await driver.check("Registration received");
    await driver.look();
    driver.count.roundTrips += 1; // the answer that says the reference
  } catch (error) {
    failed = String(error.message || error);
  }
  await desk.settled();
  const said = failed ? await pageSays(desk).catch(() => null) : null;
  await new Promise((done) => setTimeout(done, 500));
  const tally = desk.tally;
  await desk.close();
  server.kill();
  const oracle = JSON.parse(spawnSync("python3", [join(HERE, "oracle.py"), "--out", out, "--json"], { encoding: "utf8" }).stdout);
  const handSeconds = (tally.handMs + tally.settleMs + tally.waitMs + tally.ocrMs) / 1000;
  const result = {
    policy: options.policy, failed, said, oracle: { pass: oracle.pass, ok: oracle.ok_fields, of: oracle.total_fields, reasons: oracle.reasons },
    count: driver.count,
    hand: { verbs: tally.verbs, waitMs: tally.waitMs, activates: tally.verbs.activate || 0, handMs: tally.handMs, settleMs: tally.settleMs,
      ocrReads: tally.ocrReads, ocrMs: tally.ocrMs, personMs: tally.personMs, pictures: tally.pngBytes.length,
      pictureBytes: tally.pngBytes.reduce((sum, bytes) => sum + bytes, 0) },
    projected: {
      secondsPerRoundTrip: SECONDS_PER_ROUND_TRIP,
      seconds: Math.round(driver.count.roundTrips * SECONDS_PER_ROUND_TRIP + handSeconds + tally.personMs / 1000),
      personSeconds: KLM.total_seconds,
    },
    driveSeconds: (Date.now() - started) / 1000,
  };
  await writeFile(join(out, "drive.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(`DRIVE ${JSON.stringify(result)}`);
  process.exit(oracle.pass && !failed ? 0 : 1);
}

await main();
