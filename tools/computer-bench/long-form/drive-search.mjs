/* The short phone errand's scripted drivers (t-76055): what "connect, open
 * an app, press its search field, type, Return, wait for the results, open
 * one" takes in model round trips on a mirrored phone, counted without
 * asking a model — the yardstick for zo's road on the person's 10-26 session
 * (15 actions, 19 requests, 234 s).
 *
 *   node drive-search.mjs --policy session|today|levers --out DIR [--words-choose PATH]
 *
 * A model's round trip is one tool call (a batch, a single act, a look of its
 * own) plus the final answer. The drivers share drive.mjs's rules:
 *
 * - `session`: the habit the person's session showed — one act a call, each
 *   press at a point read off the picture the act before it left.
 * - `today`: the floor of the coordinate road — acts batched until a press
 *   needs a picture taken since the layout last changed; zo's look after the
 *   batch is that picture, and one taken mid-motion is looked at again.
 * - `levers`: presses by the words the phone shows (`click --ocr`), checks
 *   in the batch (`wait-for --ocr`); a picture only when the words to press
 *   next are not known — from the task (the app, the query, the place, what
 *   the place shows) or from a picture already taken of that kind of screen.
 *
 * Where a model reads a position off the picture, the drivers read it off
 * the page (`phone-eyes.mjs`) — only after a picture was taken.
 */

import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { startDesk } from "./desk.mjs";
import { SEES, pointOf, settledLayout } from "./phone-eyes.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const PHONE = "iPhone Mirroring";
const PAGE = "search.html";
// What the task tells the model: the app, the query, the place, and what
// it should read there.
const TASK = { app: "Atlas", query: "blue", place: "Blue Bottle Coffee", reads: "Opening hours" };
// The words a picture of each kind of screen teaches.
const SCREEN_WORDS = { connect: "Continue", app: "Search places" };
// One zo request in the person's session, on average: the requests of
// session-1791959770709-3 iterations 5–20 took 225 s together, 16 of them
// (ttfb 1.1–2.9 s, stream 2.3–46 s; state/request-timings, 10-26).
const SECONDS_PER_ROUND_TRIP = 14.1;
// What that session's road took, as its steps and timings say.
const SESSION_MEASURED = { actions: 15, requests: 19, seconds: 234 };
// How long a check waits for the words it expects, at most.
const CHECK_MS = 5000;

function args(argv) {
  const parsed = {};
  for (let at = 0; at < argv.length; at += 2) parsed[argv[at].replace(/^--/, "").replace(/-(\w)/g, (_, c) => c.toUpperCase())] = argv[at + 1];
  if (!["session", "today", "levers"].includes(parsed.policy) || !parsed.out) throw new Error("--policy session|today|levers --out DIR");
  return parsed;
}

/* Where things stand: which screen, the keyboard, the results shown. */
function layoutOf(desk) {
  return desk.content().evaluate(() => JSON.stringify({
    screen: document.querySelector(".screen.on")?.id || null,
    keyboard: Boolean(document.querySelector(".keyboard.up")),
    status: document.getElementById("status")?.textContent || null,
    connecting: document.getElementById("connecting")?.textContent || null,
  }));
}

class Driver {
  constructor(desk, policy) {
    this.desk = desk;
    this.policy = policy;
    this.count = { roundTrips: 0, batches: 0, ownLooks: 0, steps: 0, lookAfters: 0, unsettledLookAfters: 0, refused: 0 };
    this.open = false;
    this.lookedAt = null;
    this.known = new Set(Object.values(TASK));
  }

  async call(argv) {
    const { body } = await this.desk.doors.computer([...argv, "--json"]);
    if (!body.ok) {
      this.count.refused += 1;
      throw new Error(`${argv.join(" ")}: ${body.error.code}: ${body.error.message}`);
    }
    return body.result;
  }

  /* One step of the open batch (or of a new one); `session` closes it at
   * once — one act, one call. */
  async step(argv) {
    if (!this.open) {
      this.open = true;
      this.count.roundTrips += 1;
      this.count.batches += 1;
    }
    this.count.steps += 1;
    const said = await this.call(argv);
    if (this.policy === "session") await this.look();
    return said;
  }

  /* A picture before the next step: zo's look after the open batch, or a
   * look of the model's own. */
  async look() {
    if (this.open) {
      this.open = false;
      this.count.lookAfters += 1;
      const seen = await this.call(["observe", "--diff", "--settle", "--app", PHONE]);
      if (seen.settle && seen.settle.settled === false) {
        this.count.unsettledLookAfters += 1;
        await this.ownLook();
      }
    } else {
      await this.ownLook();
    }
    await settledLayout(this.desk);
    this.lookedAt = await layoutOf(this.desk);
    const screen = JSON.parse(this.lookedAt).screen;
    if (SCREEN_WORDS[screen]) this.known.add(SCREEN_WORDS[screen]);
  }

  async ownLook() {
    this.count.roundTrips += 1;
    this.count.ownLooks += 1;
    await this.call(["observe", "--diff", "--app", PHONE]);
  }

  async press(words) {
    if (this.policy === "levers") {
      if (!this.known.has(words)) await this.look();
      await this.step(["click", "--app", PHONE, "--ocr", "--text", words]);
      return;
    }
    await settledLayout(this.desk);
    if (this.lookedAt === null || this.lookedAt !== (await layoutOf(this.desk))) await this.look();
    const point = await pointOf(this.desk, SEES.words, words, { scroll: false });
    if (!point.inView) throw new Error(`${words} is not on the phone`);
    await this.step(["mouse-click", "--x", String(point.x), "--y", String(point.y), "--mouse-button", "left", "--click-count", "1"]);
  }

  /* The wait for what an act brings: the levers check the words in the
   * batch; the coordinate roads see it in the next picture. */
  async check(words) {
    if (this.policy === "levers") await this.step(["wait-for", "--app", PHONE, "--ocr", "--text", words, "--timeout-ms", String(CHECK_MS)]);
  }
}

async function errand(driver) {
  await driver.look();
  await driver.press("Continue");
  await driver.check(TASK.app);
  await driver.press(TASK.app);
  await driver.press("Search places");
  await driver.step(["type", "--text", TASK.query]);
  await driver.step(["key", "--key", "Return"]);
  await driver.check(TASK.place);
  await driver.press(TASK.place);
  await driver.check(TASK.reads);
  await driver.look();
  driver.count.roundTrips += 1; // the answer that reads the hours
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
  const desk = await startDesk({ url, out, personAttachMs: 0, scene: "phone", overrides: { page: PAGE },
    window: options.policy === "levers" ? "levers" : "today",
    wordsChoose: options.wordsChoose ? resolve(options.wordsChoose) : null });
  const driver = new Driver(desk, options.policy);
  const started = Date.now();
  let failed = null;
  try {
    await errand(driver);
  } catch (error) {
    failed = String(error.message || error);
  }
  await desk.settled();
  const outcome = await desk.content().evaluate(() => ({ ...LongForm.outcome,
    reads: document.querySelector("#place.on") ? document.getElementById("place").innerText : null })).catch(() => null);
  const tally = desk.tally;
  await desk.close();
  server.kill();
  const pass = !failed && outcome?.query === TASK.query && outcome?.opened === TASK.place && String(outcome?.reads).includes(TASK.reads);
  const handSeconds = (tally.handMs + tally.settleMs + tally.waitMs + tally.ocrMs) / 1000;
  const projected = Math.round(driver.count.roundTrips * SECONDS_PER_ROUND_TRIP + handSeconds);
  const actions = driver.count.steps;
  const result = {
    policy: options.policy, failed, oracle: { pass, outcome },
    count: driver.count,
    hand: { verbs: tally.verbs, handMs: tally.handMs, settleMs: tally.settleMs, waitMs: tally.waitMs, ocrReads: tally.ocrReads,
      ocrMs: tally.ocrMs, pictures: tally.pngBytes.length, pictureBytes: tally.pngBytes.reduce((sum, bytes) => sum + bytes, 0) },
    projected: { secondsPerRoundTrip: SECONDS_PER_ROUND_TRIP, seconds: projected, actions,
      secondsPerAction: Math.round((projected / Math.max(1, actions)) * 10) / 10 },
    sessionMeasured: SESSION_MEASURED,
    driveSeconds: (Date.now() - started) / 1000,
  };
  await writeFile(join(out, "drive.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(`DRIVE ${JSON.stringify(result)}`);
  process.exit(pass ? 0 : 1);
}

await main();
