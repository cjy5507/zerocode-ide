/* The form bench's scripted drivers (t-37883): what one booking or
 * registration flow takes in model round trips on each road the browser
 * door offers, counted without asking a model.
 *
 *   node drive.mjs --scene DIR [--scene DIR …] --out FILE.json
 *
 * A scene folder holds `scene.html` (served over loopback http, so a frame
 * beside it is the same origin), `card.json` (what the person gives the
 * agent: the task, each fact under the words a person sees beside its field,
 * the person's own turns) and `expected.json` (what the page must take).
 * The page sets `window.__sceneResult` when it takes the booking and
 * `window.__personPhone` when a code goes to the person's phone.
 *
 * A model's round trip is one tool call. The roads:
 * - `verbs`: `fields`, one `fill` (its passes inside the call), `click` a
 *   button by the handle the read gave (a button inside a frame by an
 *   `eval`), a `handoff` for the person's code, and one read of the page
 *   after the submit;
 * - `script`: one `eval` a step that reads the fields, fills them by the
 *   words it read and presses the step's own "next" — never a send or a
 *   submit, which go by `click` — then the same hand-off and read;
 * - `today`: not run but counted from the same pages: the door before this
 *   change, a look (`marks`) per screenful and a call per field — `type`,
 *   `click` for a choice or a box, an `eval` for a select and for a field
 *   inside a frame, an open + a look + a press for a dropdown or a calendar.
 *
 * What the numbers know (the report says it beside each one): the card's
 * facts; a stand-in for the model's reading — a fact goes to the field whose
 * words are the fact's words or hold them (parts `(i/n)` take the value's
 * number groups), a button's intent is read from a short list of words; and
 * nothing about any scene — no handle, label, order or step is written here.
 * Between two round trips the driver waits THINK_MS: a model's turn is never
 * shorter, and a page's own late fields land inside it.
 *
 * What the door cannot do, the driver does by hand as a model would — open,
 * look, press, a round trip each: a date a fill answered `no_option` with
 * the calendar it saw, a choice it answered `no_option` for, and a fact no
 * field takes whose words are beside a thing the read could not name
 * (`unknowns`).
 */

import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import { FORM_REQUEST, clickScript, evalFormScript, fieldsScript, fillPasses } from "../../../ui/tests/browser-scripts.mjs";

// The floor of one model round trip, and what one costs in the person's
// session (m-38845: about 17 s — 55 min, 122 requests, 80 % model time).
const THINK_MS = 1000;
const SECONDS_PER_ROUND_TRIP = 17;
// The most round trips a road is given before it is called stuck.
const ROUND_TRIPS_MAX = 40;
// The most presses a calendar is given by hand before the driver moves on.
const HAND_PRESSES = 12;
// A button's intent, read from its words — the model's reading, in the two
// languages the bench's pages are written in.
const INTENT = {
  code: ["인증", "코드", "code", "verify", "otp", "발송", "전송"],
  next: ["다음", "next", "continue", "계속"],
  submit: ["예약", "결제", "신청", "제출", "등록", "완료", "확인", "submit", "book", "pay", "confirm", "finish", "register"],
};
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".json": "application/json" };

const fold = (text) => String(text ?? "").replace(/\s+/g, " ").trim().toLowerCase();
const PART = / \((\d+)\/(\d+)\)$/;
const words = (label) => fold(label).replace(PART, "").replace(/\s*\*$/, "");
const took = (status) => status === "set" || status === "same";
const intentOf = (label, intent) => INTENT[intent].some((word) => fold(label).includes(word));

function args(argv) {
  const parsed = { scene: [] };
  for (let at = 0; at < argv.length; at += 2) {
    const key = argv[at].replace(/^--/, "");
    if (key === "scene") parsed.scene.push(argv[at + 1]); else parsed[key] = argv[at + 1];
  }
  if (!parsed.scene.length || !parsed.out) throw new Error("--scene DIR [--scene DIR …] --out FILE.json");
  return parsed;
}

/* Serve one folder over loopback, nothing outside it. */
async function serve(folder) {
  const root = resolve(folder);
  const server = createServer(async (request, response) => {
    const path = resolve(root, "." + decodeURIComponent(new URL(request.url, "http://x").pathname));
    if (path !== root && !path.startsWith(root + sep)) return response.writeHead(403).end();
    try {
      const body = await readFile(path.endsWith(sep) || path === root ? join(path, "scene.html") : path);
      response.writeHead(200, { "content-type": TYPES[extname(path)] || "application/octet-stream" }).end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return { url: `http://127.0.0.1:${server.address().port}/scene.html`, close: () => server.close() };
}

/* The model's reading, played by one rule: each fact to the field whose words
 * are its words, else the one field whose words hold them or are held by
 * them; a value split across `(i/n)` parts by its number groups. */
function plan(fields, facts) {
  const bundle = {};
  const unplaced = [];
  const byWords = new Map();
  for (const field of fields) {
    const key = words(field.label);
    if (!byWords.has(key)) byWords.set(key, []);
    byWords.get(key).push(field);
  }
  for (const fact of facts) {
    const said = fold(fact.says);
    let group = byWords.get(said);
    if (!group) {
      const near = [...byWords.entries()].filter(([key]) => key && (key.includes(said) || said.includes(key)));
      if (near.length === 1) group = near[0][1];
    }
    if (!group) {
      unplaced.push(fact);
      continue;
    }
    if (group.length === 1) {
      const [field] = group;
      if (field.value !== fact.value) bundle[field.handle] = fact.value;
      continue;
    }
    const groups = String(fact.value).match(/\d+/g) || [];
    const pieces = groups.length === group.length ? groups : [];
    group.forEach((field, at) => {
      const piece = pieces[at] ?? (at === 0 ? String(fact.value) : "");
      if (piece && field.value !== piece) bundle[field.handle] = piece;
    });
  }
  return { bundle, unplaced };
}

/* The one thing whose words are these words, else the one that holds them
 * or is held by them — the reading `plan` gives a fact, given to a press. */
function thingOf(things, said) {
  const wanted = fold(said);
  const same = things.filter((thing) => [thing.label, thing.caption].some((text) => text && fold(text) === wanted));
  if (same.length) return same[0];
  const near = things.filter((thing) => [thing.label, thing.caption]
    .some((text) => text && (fold(text).includes(wanted) || wanted.includes(fold(text)))));
  return near.length === 1 ? near[0] : null;
}

class Road {
  constructor(page, card, name) {
    this.page = page;
    this.name = name;
    this.facts = card.facts.map((fact) => ({ ...fact }));
    this.codeTurn = (card.personTurns || []).includes("code");
    this.codeSent = false;
    this.count = { roundTrips: 0, fields: 0, fill: 0, click: 0, eval: 0, handoff: 0, read: 0, fillPasses: 0, stale: 0 };
    this.form = null;
    this.byHandTried = new Set();
    this.scriptMs = [];
    this.trail = [];
  }

  async call(kind, run) {
    if (this.count.roundTrips >= ROUND_TRIPS_MAX) throw new Error("stuck: the round trips ran out");
    if (this.count.roundTrips) await this.page.waitForTimeout(THINK_MS);
    this.count.roundTrips += 1;
    this.count[kind] += 1;
    const began = performance.now();
    const answer = await run();
    this.scriptMs.push(performance.now() - began);
    return answer;
  }

  run(source) {
    return this.page.evaluate(source).then((raw) => JSON.parse(raw));
  }

  /* The window keeps the form a pane's agent last read, and holds the
   * next fill to it (`known_form`): so does the driver. */
  async fields() {
    const read = await this.call("fields", () => this.run(fieldsScript()));
    if (!read.ok) throw new Error(`fields refused: ${JSON.stringify(read)}`);
    this.form = read.value.fingerprint;
    return read.value;
  }

  async fill(bundle) {
    const filled = await this.call("fill", () => fillPasses((source) => this.run(source), bundle, this.form));
    if (filled.stale) {
      this.count.stale += 1;
      this.trail.push({ fill: "form_stale" });
      return { ...filled, left: [] };
    }
    this.form = filled.fingerprint;
    this.count.fillPasses += filled.passes;
    this.trail.push({ fill: filled.results.map((result) => `${result.label}:${result.status}`) });
    for (const result of filled.results) {
      if (result?.status !== "no_option") continue;
      if (result.widget) await this.byHand(result, bundle[result.handle]);
      else await this.openAndPress(result, bundle[result.handle]);
    }
    return filled;
  }

  /* A thing the door could not fill — a field answered `no_option`, a thing
   * of no kind beside a fact's words — opened, looked at and pressed by the
   * value's words, each a round trip: a press, a read, a press by handle when
   * the read names it, else a look by words (`marks`) and a press. Tried
   * once a thing. */
  async openAndPress(thing, value) {
    if (typeof value !== "string" || this.byHandTried.has(thing.handle)) return false;
    this.byHandTried.add(thing.handle);
    await this.press({ handle: thing.handle, label: thing.label });
    const read = await this.fields();
    const named = thingOf([...(read.unknowns || []), ...read.actions].filter((one) => one.handle !== thing.handle), value);
    if (named) {
      await this.press(named);
      return true;
    }
    const spot = await this.call("read", () => this.page.evaluate(([wanted]) => {
      const fold = (text) => String(text ?? "").replace(/\s+/g, " ").trim().toLowerCase();
      const all = [...document.querySelectorAll("body *")];
      return all.findIndex((el) => el.getClientRects().length && fold(el.innerText) === fold(wanted)
        && ![...el.children].some((child) => fold(child.innerText) === fold(wanted)));
    }, [value]));
    this.trail.push({ look: thing.label, found: spot >= 0 });
    if (spot < 0) return false;
    await this.call("click", () => this.page.evaluate(([at]) => document.querySelectorAll("body *")[at].click(), [spot]));
    return true;
  }

  /* The facts no field took, each to the thing of no kind its words sit
   * beside: answers true when one was pressed, so the step is read again. */
  async unknownRoad(unknowns, unplaced) {
    let pressed = false;
    for (const fact of unplaced) {
      const thing = thingOf(unknowns || [], fact.says);
      if (thing && (await this.openAndPress(thing, fact.value))) pressed = true;
    }
    return pressed;
  }

  /* A date the fill could not pick, finished by hand from the calendar its
   * answer showed, as a model reads it: open the field, page and look until
   * the heading reads the month, press the day — each a round trip, at most
   * HAND_PRESSES of them. */
  async byHand(result, asked) {
    const [year, month, day] = String(asked).match(/\d+/g).map(Number);
    const target = year * 12 + month;
    const look = () => this.call("eval", () => this.page.evaluate(([box]) => {
      const days = document.querySelector(box);
      let root = days;
      for (let level = 0; root && level < 3 && !/\d{4}/.test(root.innerText || ""); level += 1) root = root.parentElement;
      return root ? String(root.innerText || "") : "";
    }, [result.widget.days]));
    const monthOf = (text) => {
      const year4 = (text.match(/\d{4}/) || [])[0];
      const rest = year4 ? text.replace(year4, " ") : "";
      const number = rest.match(/(?:^|\D)(\d{1,2})(?:\D|$)/);
      return year4 && number ? Number(year4) * 12 + Number(number[1]) : null;
    };
    // Open it only when the look shows no calendar: a press on a field
    // under its own open calendar is refused as covered.
    if (monthOf(await look()) === null) await this.press({ handle: result.handle, label: result.label });
    for (let presses = 0, pager = 0; presses < HAND_PRESSES; presses += 1) {
      const now = monthOf(await look());
      if (now === target) break;
      if (now === null || pager >= result.widget.pagers.length) return;
      const before = now;
      await this.press({ handle: result.widget.pagers[pager], label: "pager" });
      const after = monthOf(await look());
      if (after === null || Math.sign(after - before) !== Math.sign(target - before)) pager += 1;
    }
    await this.call("eval", () => this.page.evaluate(([box, wanted]) => {
      const cells = [...document.querySelector(box).querySelectorAll("button, td, [role=gridcell]")]
        .filter((cell) => !cell.querySelector("button, td, [role=gridcell]") && /^\d{1,2}$/.test(cell.innerText.trim()));
      const numbers = cells.map((cell) => Number(cell.innerText.trim()));
      const first = numbers.indexOf(1);
      const end = numbers.indexOf(1, first + 1);
      const cell = cells.slice(first, end < 0 ? undefined : end)[numbers.slice(first, end < 0 ? undefined : end).indexOf(wanted)];
      if (cell) cell.click();
      return Boolean(cell);
    }, [result.widget.days, day]));
  }

  async press(action) {
    this.trail.push({ press: action.label });
    if (action.handle.includes(FORM_REQUEST.frameSeparator)) {
      const [frame, inner] = action.handle.split(FORM_REQUEST.frameSeparator);
      return this.call("eval", () => this.page.evaluate(([outer, button]) =>
        document.querySelector(outer).contentDocument.querySelector(button).click(), [frame, inner]));
    }
    const pressed = await this.call("click", () => this.run(clickScript(action.handle)));
    if (!pressed.ok) throw new Error(`click refused: ${JSON.stringify(pressed)}`);
    return pressed;
  }

  async handoff() {
    const code = await this.call("handoff", () => this.page.evaluate(() => window.__personPhone || null));
    if (!code) throw new Error("stuck: the person's phone got no code");
    this.codeTurn = false;
    return code;
  }

  async done() {
    const result = await this.call("read", () => this.page.evaluate(() => window.__sceneResult || null));
    return result;
  }

  /* The verbs road: read, fill, then the step's one press. A press the page
   * refuses (a field it brought late is still empty) is read again. */
  async verbs() {
    for (;;) {
      const read = await this.fields();
      const { bundle, unplaced } = plan(read.fields, this.facts);
      if (this.price) this.priced += await this.today(read, bundle);
      if (await this.unknownRoad(read.unknowns, unplaced)) continue;
      let left = read.fields.filter((field) => field.required && (field.value === "" || field.value === false));
      if (Object.keys(bundle).length) {
        const filled = await this.fill(bundle);
        if (filled.stale) continue;
        left = filled.left;
      }
      const owed = await this.code(left);
      if (owed && this.price) this.priced += 1;
      if (owed && (await this.fill(owed)).stale) continue;
      if (await this.step(read.actions)) return this.done();
    }
  }

  /* After a send: the person reads the code off the phone, and it goes into
   * the required field no fact fills — the field the send brought. Answers
   * the bundle that writes it, or null. */
  async code(left) {
    if (!this.codeSent || !this.codeTurn) return null;
    const empty = left.filter((field) => !plan([field], this.facts).bundle[field.handle]);
    if (!empty.length) return null;
    const code = await this.handoff();
    this.facts.push({ says: empty[0].label, value: code });
    return { [empty[0].handle]: code };
  }

  /* The step's press: a code to send while one is owed, else "next", else
   * the submit — true when the submit was pressed. */
  async step(actions) {
    const live = actions.filter((action) => !action.disabled);
    if (this.codeTurn && !this.codeSent) {
      const send = live.find((action) => intentOf(action.label, "code"));
      if (send) {
        await this.press(send);
        this.codeSent = true;
        return false;
      }
    }
    const next = live.find((action) => intentOf(action.label, "next"));
    if (next) {
      await this.press(next);
      return false;
    }
    const submit = live.find((action) => intentOf(action.label, "submit") && !intentOf(action.label, "code"));
    if (!submit) throw new Error(`stuck: no button moves the form on (${live.map((a) => a.label).join(", ")})`);
    await this.press(submit);
    return true;
  }

  /* The script road: one eval a step reads, fills by the words it read and
   * presses the step's own "next"; sends and submits go by click. */
  async script() {
    const STEP = `(() => {
      const fold = ${fold.toString()};
      const PART = ${PART.toString()};
      const words = ${words.toString()};
      const plan = ${plan.toString()};
      const took = ${took.toString()};
      const INTENT = ${JSON.stringify(INTENT)};
      const intentOf = ${intentOf.toString()};
      const facts = __FACTS__;
      const read = zerocode.fields();
      const { bundle, unplaced } = plan(read.fields, facts);
      const filled = Object.keys(bundle).length ? zerocode.fill(bundle, read) : { results: [], left: read.fields
        .filter((field) => field.required && (field.value === "" || field.value === false)) };
      const clean = filled.results.every((result) => took(result.status)) && !filled.left.length;
      const next = read.actions.find((action) => !action.disabled && intentOf(action.label, "next"));
      const pressNext = clean && !!next && !next.handle.includes(${JSON.stringify(FORM_REQUEST.frameSeparator)});
      if (pressNext) document.querySelector(next.handle).click();
      const hand = filled.results.filter((result) => result.status === "no_option")
        .map((result) => ({ ...result, asked: bundle[result.handle] }));
      return { results: filled.results.map((result) => result.label + ":" + result.status), left: filled.left,
        actions: read.actions, pressedNext: pressNext, hand, unknowns: read.unknowns, unplaced };
    })()`;
    for (;;) {
      const source = evalFormScript(STEP.replace("__FACTS__", () => JSON.stringify(this.facts)));
      const answer = await this.call("eval", () => this.run(source));
      if (!answer.ok) throw new Error(`eval refused: ${JSON.stringify(answer)}`);
      const said = answer.value;
      this.trail.push({ script: said.results, pressedNext: said.pressedNext });
      for (const result of said.hand || []) {
        if (result.widget) await this.byHand(result, result.asked);
        else await this.openAndPress(result, result.asked);
      }
      if (said.pressedNext) continue;
      if (await this.unknownRoad(said.unknowns, said.unplaced)) continue;
      if (await this.code(said.left)) continue;
      if (said.left.length || said.results.some((result) => !/:(set|same)$/.test(result))) continue;
      if (await this.step(said.actions)) return this.done();
    }
  }

  /* Today's road, priced at one read: a look (`marks`) per screenful the
   * fields to write span, a scroll between two, and per field a call — an
   * open, a look and a press for a dropdown or a calendar. */
  async today(read, bundle) {
    const touched = read.fields.filter((field) => field.handle in bundle);
    const height = await this.page.evaluate(() => window.innerHeight);
    const tops = await this.page.evaluate(([handles, separator]) => handles.map((handle) => {
      if (handle.includes(separator)) return null;
      const element = document.querySelector(handle);
      return element ? element.getBoundingClientRect().top + window.scrollY : null;
    }), [touched.map((field) => field.handle), FORM_REQUEST.frameSeparator]);
    const placed = tops.filter((top) => top !== null);
    const screens = placed.length ? new Set(placed.map((top) => Math.floor(top / height))).size : 1;
    let calls = screens * 2 - 1;
    for (const field of touched) calls += field.kind === "combobox" || field.readOnly ? 3 : 1;
    return calls;
  }
}

async function drive(browser, folder, roadName) {
  const card = JSON.parse(await readFile(join(folder, "card.json"), "utf8"));
  const expected = JSON.parse(await readFile(join(folder, "expected.json"), "utf8"));
  const site = await serve(folder);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const road = new Road(page, card, roadName);
  const began = performance.now();
  let result = null, stuck = null;
  try {
    await page.goto(site.url);
    result = roadName === "verbs" ? await road.verbs() : await road.script();
  } catch (error) {
    stuck = String(error.message || error);
  } finally {
    await page.close();
    site.close();
  }
  const ok = JSON.stringify(result) === JSON.stringify(expected);
  const scriptMs = road.scriptMs.slice().sort((a, b) => a - b);
  return {
    scene: folder.split(sep).filter(Boolean).pop(), road: roadName, ok, stuck,
    wrong: result ? Object.keys(expected).filter((key) => JSON.stringify(result[key]) !== JSON.stringify(expected[key])) : null,
    ...road.count, projectedSeconds: road.count.roundTrips * SECONDS_PER_ROUND_TRIP,
    callMsP50: Math.round(scriptMs[Math.floor(scriptMs.length / 2)] ?? 0), callMsMax: Math.round(scriptMs.at(-1) ?? 0),
    wallMs: Math.round(performance.now() - began), trail: road.trail,
  };
}

/* Today's count walks the same steps as the verbs road and prices each
 * read the old way; its presses, hand-offs and the last read are the same. */
async function countToday(browser, folder) {
  const card = JSON.parse(await readFile(join(folder, "card.json"), "utf8"));
  const site = await serve(folder);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const road = new Road(page, card, "today");
  road.price = true;
  road.priced = 0;
  let stuck = null;
  try {
    await page.goto(site.url);
    await road.verbs();
  } catch (error) {
    stuck = String(error.message || error);
  } finally {
    await page.close();
    site.close();
  }
  const roundTrips = road.priced + road.count.click + road.count.eval + road.count.handoff + road.count.read;
  return { scene: folder.split(sep).filter(Boolean).pop(), road: "today", counted: true, stuck, roundTrips,
    projectedSeconds: roundTrips * SECONDS_PER_ROUND_TRIP };
}

const options = args(process.argv.slice(2));
const browser = await chromium.launch();
const rows = [];
try {
  for (const folder of options.scene) {
    for (const road of ["verbs", "script"]) {
      const row = await drive(browser, folder, road);
      rows.push(row);
      console.log(`DRIVE ${row.scene} ${row.road} ok=${row.ok} roundTrips=${row.roundTrips} fields=${row.fields} fill=${row.fill} click=${row.click} eval=${row.eval} handoff=${row.handoff} passes=${row.fillPasses} callMs p50=${row.callMsP50} max=${row.callMsMax}${row.stuck ? " stuck=" + row.stuck : ""}${row.wrong?.length ? " wrong=" + row.wrong.join(",") : ""}`);
    }
    const today = await countToday(browser, folder);
    rows.push(today);
    console.log(`DRIVE ${today.scene} today counted roundTrips=${today.roundTrips}${today.stuck ? " stuck=" + today.stuck : ""}`);
  }
} finally {
  await browser.close();
}
await writeFile(options.out, JSON.stringify(rows, null, 2) + "\n");
process.exit(rows.every((row) => row.counted || row.ok) ? 0 : 1);
