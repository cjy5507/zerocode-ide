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
 * - `verbs`: `fields`, one `fill` (its passes inside the call), a `type …
 *   --value-stdin` for each secret field the fill names, `click` a button by
 *   the handle the read gave and the state the fill's answer ended with (a
 *   button inside a frame by an `eval`), a `handoff` for the person's code,
 *   and one read of the page after the submit;
 * - `script`: one `eval` a step that reads the fields, fills them by the
 *   words it read and presses the step's own "next" — never a send or a
 *   submit, which go by `click` — by the buttons the fill's answer ended
 *   with, a `type` for a secret field, then the same hand-off and read;
 * - `today`: not run but counted from the same pages: the door before this
 *   change, a look (`marks`) per screenful and a call per field — `type`,
 *   `click` for a choice or a box, an `eval` for a select and for a field
 *   inside a frame, an open + a look + a press for a dropdown or a calendar.
 *
 * What the numbers know (the report says it beside each one): the card's
 * facts; a stand-in for the model's reading — a fact goes to the field whose
 * words are the fact's words or hold them (parts `(i/n)` take the value's
 * number groups), a button's intent is read from a short list of words, and
 * the person's code goes, after a send, to the field whose words name a
 * one-time code or that the send brought, required or not; and nothing
 * about any scene — no handle, label, order or step is written here.
 * Between two round trips the driver waits THINK_MS: a model's turn is never
 * shorter, and a page's own late fields land inside it.
 *
 * What the door cannot do, the driver does by hand as a model would — open,
 * look, press, a round trip each: a date a fill answered `no_option` with
 * the calendar it saw, a choice it answered `no_option` for, and a fact no
 * field takes whose words are beside a thing the read could not name
 * (`unknowns`).
 */

import { readFile, writeFile } from "node:fs/promises";
import { join, sep } from "node:path";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import { FORM_REQUEST, clickScript, evalFormScript, fieldsScript, fillPasses, typeScript } from "../../../ui/tests/browser-scripts.mjs";
import { serveScene as serve, wrongKeys } from "./scene-kit.mjs";

// The floor of one model round trip, and what one costs in the person's
// session (m-38845: about 17 s — 55 min, 122 requests, 80 % model time).
const THINK_MS = 1000;
const SECONDS_PER_ROUND_TRIP = 17;
// The most round trips a road is given before it is called stuck.
const ROUND_TRIPS_MAX = 40;
// The most presses a calendar is given by hand before the driver moves on.
const HAND_PRESSES = 12;
// A button's intent, read from its words — the model's reading, in the two
// languages the bench's pages are written in: a code to send, the step's
// "next", the submit, and (the whole words) a code's own confirm button.
// `oneTime` is a field's: what only the person's phone knows.
const INTENT = {
  code: ["인증", "코드", "code", "verify", "otp", "발송", "전송"],
  next: ["다음", "next", "continue", "계속"],
  submit: ["예약", "결제", "신청", "제출", "등록", "완료", "확인", "submit", "book", "pay", "confirm", "finish", "register"],
  confirm: ["확인", "인증", "인증하기", "verify", "confirm"],
  oneTime: ["인증번호", "인증 번호", "인증코드", "일회용", "otp", "verification code", "one-time", "passcode"],
};

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
    this.codeField = null;
    this.codeConfirmed = false;
    this.seen = [];
    this.beforeSend = new Set();
    this.count = { roundTrips: 0, fields: 0, fill: 0, click: 0, eval: 0, type: 0, handoff: 0, read: 0, fillPasses: 0, stale: 0 };
    // The secret fields typed on the setter road: a fill never reads one back,
    // so a field already typed is no longer asked of it nor owed by it.
    this.typed = new Set();
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
    this.seen = read.value.fields;
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
      if (result?.status === "secret") await this.typeSecret(result.handle, bundle[result.handle]);
      if (result?.status !== "no_option") continue;
      if (result.widget) await this.byHand(result, bundle[result.handle]);
      else await this.openAndPress(result, bundle[result.handle]);
    }
    return filled;
  }

  /* A secret field: `fill` never writes one and says so (`secret`), naming
   * the road — `type <label> <handle> --value-stdin`, the setter. A model
   * takes it as the answer says: one `type` a field, with the fact's value,
   * once. A field inside a frame has no selector to type by. */
  async typeSecret(handle, value) {
    if (typeof value !== "string" || this.typed.has(handle) || handle.includes(FORM_REQUEST.frameSeparator)) return false;
    this.typed.add(handle);
    this.trail.push({ type: handle });
    const typed = await this.call("type", () => this.run(typeScript(handle, value, "setter")));
    if (!typed.ok) throw new Error(`type refused: ${JSON.stringify(typed)}`);
    return true;
  }

  /* What a read still asks of the fields: those not yet typed as secrets. */
  untyped(fields) {
    return fields.filter((field) => !this.typed.has(field.handle));
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

  /* The verbs road: read, fill, then the step's one press — by the buttons
   * the last fill's answer ended with (a button that turns on once the fields
   * are right is on there), else by the read's. A press the page refuses (a
   * field it brought late is still empty) is read again. */
  async verbs() {
    for (;;) {
      const read = await this.fields();
      const { bundle, unplaced } = plan(this.untyped(read.fields), this.facts);
      if (this.price) this.priced += await this.today(read, bundle);
      if (await this.unknownRoad(read.unknowns, unplaced)) continue;
      let actions = read.actions;
      if (Object.keys(bundle).length) {
        const filled = await this.fill(bundle);
        if (filled.stale) continue;
        if (filled.actions.length) actions = filled.actions;
      }
      const owed = await this.code(read.fields, bundle);
      if (owed && this.price) this.priced += 1;
      if (owed) {
        const filled = await this.fill(owed);
        if (filled.stale) continue;
        if (filled.actions.length) actions = filled.actions;
      }
      if (await this.step(actions)) return this.done();
    }
  }

  /* After a send: the person reads the code off the phone, and it goes into
   * the empty field no fact fills that asks for it — whether or not the page
   * marks it required: the one whose words name a one-time code, else the one
   * the send brought, else the one the page requires. A field the card has
   * no value for is never made up. Answers the bundle that writes it, or
   * null. */
  async code(fields, bundle) {
    if (!this.codeSent || !this.codeTurn) return null;
    const open = fields.filter((field) => field.value === "" && !(field.handle in bundle)
      && !plan([field], this.facts).bundle[field.handle]);
    const asks = open.find((field) => INTENT.oneTime.some((word) => fold(field.label).includes(word)))
      || open.find((field) => !this.beforeSend.has(field.handle))
      || open.find((field) => field.required);
    if (!asks) return null;
    const code = await this.handoff();
    this.codeField = asks.handle;
    this.facts.push({ says: asks.label, value: code });
    return { [asks.handle]: code };
  }

  /* The step's press: a code to send while one is owed, a code written
   * confirmed by its own button once when the page has one, else "next",
   * else the submit — true when the submit was pressed. */
  async step(actions) {
    const live = actions.filter((action) => !action.disabled);
    if (this.codeTurn && !this.codeSent) {
      const send = live.find((action) => intentOf(action.label, "code"));
      if (send) {
        this.beforeSend = new Set(this.seen.map((field) => field.handle));
        await this.press(send);
        this.codeSent = true;
        return false;
      }
    }
    if (this.codeField && !this.codeConfirmed) {
      this.codeConfirmed = true;
      const confirm = live.find((action) => INTENT.confirm.includes(fold(action.label)));
      if (confirm) {
        await this.press(confirm);
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
   * presses the step's own "next" — or, while the person's code is owed and
   * the step can send one, holds it; with a code just written, presses the
   * code's own confirm button first. Sends and submits go by click. */
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
      const turn = __TURN__;
      const typed = __TYPED__;
      const read = zerocode.fields();
      const fields = read.fields.filter((field) => !typed.includes(field.handle));
      const { bundle, unplaced } = plan(fields, facts);
      const filled = Object.keys(bundle).length ? zerocode.fill(bundle, read) : { results: [], left: fields
        .filter((field) => field.required && (field.value === "" || field.value === false)) };
      const left = filled.left.filter((field) => !typed.includes(field.handle));
      const secrets = filled.results.filter((result) => result.status === "secret")
        .map((result) => ({ handle: result.handle, value: bundle[result.handle] }));
      const clean = filled.results.every((result) => took(result.status)) && !left.length;
      const actions = filled.actions && filled.actions.length ? filled.actions : read.actions;
      const live = actions.filter((action) => !action.disabled);
      const inFrame = (action) => action.handle.includes(${JSON.stringify(FORM_REQUEST.frameSeparator)});
      const confirm = turn.confirm && clean
        ? live.find((action) => !inFrame(action) && INTENT.confirm.includes(fold(action.label))) : null;
      if (confirm) document.querySelector(confirm.handle).click();
      const hold = turn.owed && live.some((action) => intentOf(action.label, "code"));
      const next = live.find((action) => intentOf(action.label, "next"));
      const pressNext = clean && !confirm && !hold && !!next && !inFrame(next);
      if (pressNext) document.querySelector(next.handle).click();
      const hand = filled.results.filter((result) => result.status === "no_option")
        .map((result) => ({ ...result, asked: bundle[result.handle] }));
      return { results: filled.results.map((result) => result.label + ":" + result.status), left,
        actions, pressedNext: pressNext, hand, secrets, unknowns: read.unknowns, unplaced, clean,
        confirmed: !!confirm, bundle, fields: read.fields.map((field) => ({ handle: field.handle, label: field.label,
          value: field.value, required: field.required })) };
    })()`;
    for (;;) {
      const turn = { owed: this.codeTurn, confirm: Boolean(this.codeField) && !this.codeConfirmed };
      const source = evalFormScript(STEP.replace("__FACTS__", () => JSON.stringify(this.facts))
        .replace("__TURN__", () => JSON.stringify(turn))
        .replace("__TYPED__", () => JSON.stringify([...this.typed])));
      const answer = await this.call("eval", () => this.run(source));
      if (!answer.ok) throw new Error(`eval refused: ${JSON.stringify(answer)}`);
      const said = answer.value;
      this.seen = said.fields;
      if (turn.confirm && said.clean) this.codeConfirmed = true;
      this.trail.push({ script: said.results, pressedNext: said.pressedNext, confirmed: said.confirmed });
      for (const result of said.hand || []) {
        if (result.widget) await this.byHand(result, result.asked);
        else await this.openAndPress(result, result.asked);
      }
      for (const secret of said.secrets || []) await this.typeSecret(secret.handle, secret.value);
      if (said.pressedNext || said.confirmed) continue;
      if (await this.unknownRoad(said.unknowns, said.unplaced)) continue;
      if (await this.code(said.fields, said.bundle)) continue;
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
  const wrong = result ? wrongKeys(result, expected) : null;
  const ok = wrong !== null && wrong.length === 0;
  const scriptMs = road.scriptMs.slice().sort((a, b) => a - b);
  return {
    scene: folder.split(sep).filter(Boolean).pop(), road: roadName, ok, stuck, wrong,
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
      console.log(`DRIVE ${row.scene} ${row.road} ok=${row.ok} roundTrips=${row.roundTrips} fields=${row.fields} fill=${row.fill} click=${row.click} eval=${row.eval} type=${row.type} handoff=${row.handoff} passes=${row.fillPasses} callMs p50=${row.callMsP50} max=${row.callMsMax}${row.stuck ? " stuck=" + row.stuck : ""}${row.wrong?.length ? " wrong=" + row.wrong.join(",") : ""}`);
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
