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
 * number groups; a group of chips takes the options a fact names, or lists), the
 * button that moves a form on is chosen by what the door said of the buttons
 * (the one the page declares a submit, else the one the fill turned on, else the
 * page's last), a code's own buttons are read from a short list of words, and
 * the person's code goes, after a send, to the field whose words name a
 * one-time code or that the send brought, required or not; and nothing
 * about any scene — no handle, label, order or step is written here.
 * With `--door-text PATH` (the built `door_text` example) each run also keeps a
 * `trace`: what the door said to each `fields` and `fill` in the core's own words, and what
 * the driver pressed and typed — the chain a failed run is read from.
 *
 * What a model takes from the door's words and the skill, the driver does the same way: a value the page
 * splits across fields is cut at the symbols the door says stand between the parts (`joint`) — the last part,
 * when it is a list that has the rest among its options, takes it; a date or a time goes to parts the page
 * names by units (년 · 월 · 일, 시 · 분, 오전 · 오후; year, month, day, hour, minute, am, pm); a fact that
 * names two fields is told by the kind of the field the value fits, and a card that does not tell is stopped on;
 * a field the door says has new text after a press, with one button of its own beside it (`beside`), has that button
 * pressed once and the form read again; a field whose read-back the door cannot tell (`unseen`) is not written
 * again. A fill
 * whose answer says the form changed is followed by a read of the form before anything is
 * pressed; a button that moves the form on and is off while the door has said new text (a check
 * the page is still running) is waited for, a fixed number of times, and the form read again; a field the read says the page keeps off, with a box the read names as not yet read to
 * its end, is switched on by scrolling that box with an `eval` the way the skill teaches
 * (door-recipes.mjs), then the form is read again; a press in a form that was read is made as the
 * door makes it (`pressInForm`) — the page waited for and read after it.
 *
 * Between two round trips the driver waits THINK_MS: a model's turn is never
 * shorter, and a page's own late fields land inside it.
 *
 * What the door cannot do, the driver does by hand as a model would — open,
 * look, press, a round trip each: a date a fill answered `no_option` with
 * the calendar it saw, a choice it answered `no_option` for, and a fact no
 * field takes whose words are beside a thing the read could not name
 * (`unknowns`).
 */

import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve, sep } from "node:path";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import { FORM_REQUEST, clickScript, evalFormScript, evalScript, fieldsScript, fillPasses, pressInForm, typeScript } from "../../../ui/tests/browser-scripts.mjs";
import { pressButton, scrollToEnd } from "./door-recipes.mjs";
import { readScene, serveScene as serve, verdictWords, verdicts } from "./scene-kit.mjs";

// The floor of one model round trip, and what one costs in the person's
// session (m-38845: about 17 s — 55 min, 122 requests, 80 % model time).
const THINK_MS = 1000;
const SECONDS_PER_ROUND_TRIP = 17;
// The most round trips a road is given before it is called stuck.
const ROUND_TRIPS_MAX = 40;
// The most presses a calendar is given by hand before the driver moves on.
const HAND_PRESSES = 12;
// How long one wait lasts and how many times a button that is off is waited for: a page's own
// check takes a second or two, and a model that waits more than a few times has stopped reading.
const WAIT_MS = 1500;
const WAITS_MAX = 3;
// What is left of a code's buttons' words, in the two languages the bench's pages are written in: the plainest word a button that
// sends a code has, and (the whole word) the plainest a code's own confirm button has. They are used only when the door says no place
// (`codeSender`, `codeConfirmer`): the buttons of a code are chosen by the place the door gave them. `oneTime` is a field's: what
// only the person's phone knows. The button that moves a form on is no word list's either: `advancing` chooses it by what the door
// said of the buttons.
const INTENT = {
  code: ["인증", "code"],
  confirm: ["확인", "verify", "confirm"],
  oneTime: ["인증번호", "인증 번호", "인증코드", "일회용", "otp", "verification code", "one-time", "passcode"],
};

// The words a part of one value names what it holds by, read as units — the model's reading, a short list in each of the
// two languages the bench's pages are written in. A part is told by the words after its caption (`<caption> — <words>`).
const UNITS = {
  year: ["년", "year", "yyyy"],
  month: ["월", "month", "mm"],
  day: ["일", "day", "dd"],
  hour: ["시", "hour", "hh"],
  minute: ["분", "minute", "min"],
  meridiem: ["오전", "오후", "am", "pm"],
};

const fold = (text) => String(text ?? "").replace(/\s+/g, " ").trim().toLowerCase();
const PART = / \((\d+)\/(\d+)\)$/;
const words = (label) => fold(label).replace(PART, "").replace(/\s*\*$/, "");
const took = (status) => status === "set" || status === "same";
const intentOf = (label, intent) => INTENT[intent].some((word) => fold(label).includes(word));
/* A date the card gives whole, year first (`2026-11-27`). */
const dateLike = (value) => typeof value === "string" && /^\d{4}[-./]\d{1,2}[-./]\d{1,2}$/.test(value.trim());

/* The button that sends the code of the person, chosen by the place the door gave it: the one button that stands beside the phone
 * field the driver wrote — a field of kind `tel` with a value, or one in the bundle just written. Words come after, and only when no
 * one button stands there (the door says none, or two). */
function codeSender(actions, fields, written = []) {
  const live = actions.filter((action) => !action.disabled);
  const phones = new Set((fields || []).filter((field) => field.kind === "tel" && (field.value !== "" || written.includes(field.handle)))
    .map((field) => field.handle));
  const beside = live.filter((action) => phones.has(action.beside));
  if (beside.length === 1) return beside[0];
  return live.find((action) => intentOf(action.label, "code")) || null;
}

/* The button that confirms the code written, chosen the same way: the one button the door says stands beside the field the code went
 * into; else the whole words that are left. */
function codeConfirmer(actions, codeField) {
  const live = actions.filter((action) => !action.disabled);
  const beside = live.filter((action) => action.beside === codeField);
  if (beside.length === 1) return beside[0];
  return live.find((action) => INTENT.confirm.includes(fold(action.label))) || null;
}

/* Whether a label holds the card's words whole: the words stand in it with no number before them and no letter or number after them (`North Gate 9` is held by
 * `Gate 9 · open` and not by `North Gate 90`; `25` is not held by `125`). A tag text a page draws against the front of the words — `SpotNorth Gate 9` — has no space in the label
 * and is no part of the words: a letter before them does not break them, a number does. */
function holdsWhole(label, value) {
  const want = fold(value);
  if (!want) return false;
  const text = fold(label);
  const word = /[\p{L}\p{N}]/u, number = /\p{N}/u;
  for (let from = text.indexOf(want); from >= 0; from = text.indexOf(want, from + 1)) {
    const before = text[from - 1], after = text[from + want.length];
    if (!(before && number.test(before)) && !(after && word.test(after))) return true;
  }
  return false;
}

/* The button that moves a form on, chosen by what the door said of the buttons and by no word: the one the
 * page declares a submit (`(제출 단추)`), else the one that was off before the fields were written and is on
 * now, else the page's last — a step's own button stands after the ones that go back, apply or search. */
function advancing(actions, before = [], written = []) {
  if (!Array.isArray(before)) before = [];
  if (!Array.isArray(written)) written = [];
  const submits = actions.filter((action) => action.submit);
  if (submits.length) return submits[submits.length - 1];
  // A button the value of a field the fill wrote turned on — the "Fewer" beside a count that is now more than one — is that field's own: the
  // door says whose it is (`beside`). It is not the step's button.
  const turned = actions.filter((action) => !action.disabled && !written.includes(action.beside)
    && before.some((one) => one.handle === action.handle && one.disabled));
  if (turned.length) return turned[turned.length - 1];
  return actions.length ? actions[actions.length - 1] : null;
}

/* The unit a part's own words name, when they name exactly one. */
function unitOf(own) {
  const tokens = fold(own).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const hit = Object.entries(UNITS).filter(([, names]) => tokens.some((token) => names.includes(token))).map(([unit]) => unit);
  return hit.length === 1 ? hit[0] : null;
}

/* The fields the door names by a caption and a unit — `<caption> — <unit>` — grouped by caption: the parts of a date or a
 * time, two or more units of one. */
function unitGroups(fields) {
  const groups = new Map();
  for (const field of fields) {
    const label = String(field.label || "").replace(/\s*\*$/, "");
    const at = label.lastIndexOf(" — ");
    if (at < 0) continue;
    const unit = unitOf(label.slice(at + 3));
    if (!unit) continue;
    const key = fold(label.slice(0, at)).replace(/\s*\*$/, "");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push({ field, unit });
  }
  for (const [key, parts] of groups) if (new Set(parts.map((part) => part.unit)).size < 2) groups.delete(key);
  return groups;
}

/* The fields the door names by a caption and their own short words — `<caption> — <words>` — grouped by caption: the parts of one value
 * a caption names, two or more. */
function captionGroups(fields) {
  const groups = new Map();
  for (const field of fields) {
    const label = String(field.label || "").replace(/\s*\*$/, "");
    const at = label.lastIndexOf(" — ");
    if (at < 0) continue;
    const key = fold(label.slice(0, at)).replace(/\s*\*$/, "");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(field);
  }
  for (const [key, parts] of groups) if (parts.length < 2) groups.delete(key);
  return groups;
}

/* A date or a time the card gives, in its numbers: a year first makes a date (an hour and a minute after it make a time too), else
 * an hour and a minute; `pm` is which half of the day the words say, or null. */
function momentOf(value) {
  const text = String(value);
  const numbers = (text.match(/\d+/g) || []).map(Number);
  const words = fold(text);
  const pm = /오후|\bpm\b|p\.m\./.test(words) ? true : /오전|\bam\b|a\.m\./.test(words) ? false : null;
  const moment = { pm };
  let rest = numbers;
  if (numbers.length >= 3 && /^\d{4}$/.test((text.match(/\d+/) || [""])[0])) {
    [moment.year, moment.month, moment.day] = numbers;
    rest = numbers.slice(3);
  }
  if (rest.length) {
    moment.hour = rest[0];
    moment.minute = rest[1] ?? 0;
  }
  return moment;
}

/* The parts of one value cut at the symbols the door says stand between them (each part's `joint` is the symbol after it): the text
 * between two symbols goes to the part before the second; parts with no symbol between them share the text up to the next symbol — the
 * last of them, when it is a list that has the text among its options, takes it (the page writes the parts before it), else the first.
 * A Map from part to its text; null when the door said no symbol, or the text lacks one it said. */
function splitAtJoints(parts, text) {
  if (!parts.slice(0, -1).some((part) => part.joint)) return null;
  const chunks = [];
  let rest = text;
  let run = [];
  for (let at = 0; at < parts.length; at += 1) {
    run.push(parts[at]);
    const joint = at + 1 < parts.length ? parts[at].joint : "";
    if (at + 1 < parts.length && !joint) continue;
    let chunk = rest;
    if (joint) {
      const found = rest.indexOf(joint);
      if (found < 0) return null;
      chunk = rest.slice(0, found);
      rest = rest.slice(found + joint.length);
    }
    chunks.push({ run, chunk });
    run = [];
  }
  const cut = new Map();
  for (const { run: held, chunk } of chunks) {
    const last = held[held.length - 1];
    const option = (last.options || []).find((one) => fold(one) === fold(chunk));
    if (held.length > 1 && option) cut.set(last, option);
    else cut.set(held[0], chunk);
  }
  return cut;
}

/* Whether a value fits the kind of a field: a true or false is a checkbox's, any other value is not. */
function kindFits(field, value) {
  return typeof value === "boolean" ? field.kind === "checkbox" : field.kind !== "checkbox";
}

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
 * them; a value split across `(i/n)` parts by its number groups. A group of
 * chips takes the facts that name its options — true for an option to be
 * pressed, false for one to be let go — as the list of the options wanted, and a
 * fact that gives the group its words and a text as the text, which the door
 * reads as a list. */
function plan(fields, facts) {
  const bundle = {};
  const unplaced = [];
  const ambiguous = [];
  const dated = unitGroups(fields);
  const captioned = captionGroups(fields);
  const sameNumbers = (a, b) => {
    const left = String(a ?? "").match(/\d+/g), right = String(b ?? "").match(/\d+/g);
    return Boolean(left && right) && left.map(Number).join() === right.map(Number).join();
  };
  const byWords = new Map();
  for (const field of fields) {
    const key = words(field.label);
    if (!byWords.has(key)) byWords.set(key, []);
    byWords.get(key).push(field);
  }
  // The words of every fact: a field whose words are another fact's words, whole, is that fact's — a fact whose words only some fields' words hold does not take it.
  const owned = new Set(facts.map((fact) => fold(fact.says)));
  const chips = fields.filter((field) => field.kind === "chips");
  const wanted = new Map();
  const optionOf = (said) => {
    const hits = [];
    for (const field of chips) {
      for (const option of field.options || []) {
        const key = fold(option);
        if (key === said || (said && key.includes(said)) || (key && said.includes(key))) hits.push({ field, option, exact: key === said });
      }
    }
    const exact = hits.filter((hit) => hit.exact);
    const found = exact.length ? exact : hits;
    return found.length === 1 ? found[0] : null;
  };
  for (const fact of facts) {
    const said = fold(fact.says);
    const parts = dated.get(said);
    if (parts && typeof fact.value !== "boolean") {
      // A date or a time, written into the parts the page names by units.
      const moment = momentOf(fact.value);
      const halves = parts.some((part) => part.unit === "meridiem");
      for (const { field, unit } of parts) {
        let piece = null;
        if (unit === "year" && moment.year !== undefined) piece = String(moment.year);
        if (unit === "month" && moment.month !== undefined) piece = String(moment.month);
        if (unit === "day" && moment.day !== undefined) piece = String(moment.day);
        if (unit === "hour" && moment.hour !== undefined) piece = String(halves ? moment.hour % 12 || 12 : moment.hour);
        if (unit === "minute" && moment.hour !== undefined) piece = String(moment.minute);
        if (unit === "meridiem" && moment.hour !== undefined) {
          const names = (moment.pm ?? moment.hour >= 12) ? ["오후", "pm"] : ["오전", "am"];
          piece = (field.options || []).find((option) => names.some((name) => fold(option).includes(name))) ?? null;
        }
        if (piece !== null && !sameNumbers(field.value, piece) && fold(field.value) !== fold(piece)) bundle[field.handle] = piece;
      }
      continue;
    }
    const named = captioned.get(said);
    if (named && typeof fact.value !== "boolean") {
      // The parts a caption names: a value cut at the symbols the door says stand between them.
      const cut = splitAtJoints(named, String(fact.value));
      if (cut) {
        for (const [field, piece] of cut) if (piece && field.value !== piece) bundle[field.handle] = piece;
        continue;
      }
    }
    let group = byWords.get(said);
    if (!group) {
      const near = [...byWords.entries()].filter(([key]) => key && (key.includes(said) || said.includes(key)) && !owned.has(key));
      if (near.length === 1) group = near[0][1];
      else if (near.length > 1) {
        // One item that two fields answer to: a number goes to the field that takes numbers, a true or false to a
        // checkbox; what still fits two fields is not told by the card.
        let fits = near.flatMap(([, list]) => list).filter((field) => kindFits(field, fact.value));
        if (fits.length > 1 && /^\d+$/.test(String(fact.value))) {
          const numeric = fits.filter((field) => field.kind === "number" || field.kind === "range" || /^\d+$/.test(String(field.value ?? "")));
          if (numeric.length) fits = numeric;
        }
        if (fits.length === 1) group = fits;
        else if (fits.length > 1) {
          ambiguous.push({ says: fact.says, handles: fits.map((field) => field.handle) });
          continue;
        }
      }
    }
    if (!group) {
      const hit = typeof fact.value === "boolean" ? optionOf(said) : null;
      if (hit) {
        if (!wanted.has(hit.field.handle)) wanted.set(hit.field.handle, { field: hit.field, on: new Set() });
        if (fact.value) wanted.get(hit.field.handle).on.add(hit.option);
        continue;
      }
      // A fact no field's words name, whose value is one option of one group of chips — a time among slots the
      // page lists under other titles — is that group's.
      const holders = typeof fact.value === "string"
        ? chips.filter((field) => (field.options || []).some((option) => fold(option) === fold(fact.value))) : [];
      if (holders.length === 1) {
        const [holder] = holders;
        if (!(holder.value || []).some((word) => fold(word) === fold(fact.value))) bundle[holder.handle] = fact.value;
        continue;
      }
      unplaced.push(fact);
      continue;
    }
    if (group.length === 1) {
      const [field] = group;
      if (field.kind === "chips" && typeof fact.value === "string") {
        if (fold((field.value || []).join(", ")) !== fold(fact.value)) bundle[field.handle] = fact.value;
      } else if (field.value !== fact.value) {
        bundle[field.handle] = fact.value;
      }
      continue;
    }
    // A value the page splits across parts, cut at the symbols the door says stand between them.
    const cut = splitAtJoints(group, String(fact.value));
    if (cut) {
      for (const [field, piece] of cut) if (piece && field.value !== piece) bundle[field.handle] = piece;
      continue;
    }
    const groups = String(fact.value).match(/\d+/g) || [];
    const pieces = groups.length === group.length ? groups : [];
    group.forEach((field, at) => {
      const piece = pieces[at] ?? (at === 0 ? String(fact.value) : "");
      if (piece && field.value !== piece) bundle[field.handle] = piece;
    });
  }
  for (const { field, on } of wanted.values()) {
    const list = (field.options || []).filter((option) => on.has(option));
    if (JSON.stringify(list) !== JSON.stringify(field.value || [])) bundle[field.handle] = list;
  }
  return { bundle, unplaced, ambiguous };
}

/* What a stop on a card that does not tell says. */
function ambiguousWords(ambiguous) {
  return `stuck: ${ambiguous.map((one) => `"${one.says}" answers to ${one.handles.length} fields (${one.handles.join(", ")})`).join("; ")} and the card does not say which`;
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
    this.count = { roundTrips: 0, fields: 0, fill: 0, click: 0, eval: 0, type: 0, handoff: 0, read: 0, wait: 0, fillPasses: 0, stale: 0 };
    // What the door last said was new in the form — the page's own words after a write or a press —
    // and how many times in a row the button that moves the form on has been waited for.
    this.fresh = [];
    this.waits = 0;
    // The buttons pressed in a form that stayed, by the form's fingerprint and the button's handle, the button
    // pressed last, and the answer the script road read last: what a driver that has nothing more to give stops on.
    this.pressed = new Map();
    this.justPressed = null;
    this.lastSig = null;
    // Every button the driver pressed, and what the door said after the last press: whether the form stayed, the fields that
    // have something to say and the buttons — what a field's own button is found by.
    this.everPressed = new Set();
    this.after = null;
    // The secret fields typed on the setter road: a fill never reads one back,
    // so a field already typed is no longer asked of it nor owed by it.
    this.typed = new Set();
    this.form = null;
    // The door's own words, when asked for: the core's `door_text` says them.
    this.ask = null;
    this.said = [];
    this.buttons = [];
    this.byHandTried = new Set();
    this.scrolled = new Set();
    this.scriptMs = [];
    // How long each kind of round trip took on the wall, by kind — a fill's is
    // what the door spends reading the page after it writes.
    this.kindMs = {};
    this.trail = [];
  }

  async call(kind, run) {
    if (this.count.roundTrips >= ROUND_TRIPS_MAX) throw new Error("stuck: the round trips ran out");
    if (this.count.roundTrips) await this.page.waitForTimeout(THINK_MS);
    this.count.roundTrips += 1;
    this.count[kind] += 1;
    const began = performance.now();
    const answer = await run();
    const took = performance.now() - began;
    // A wait is the page's time, not the door's: it stays out of the calls' own timing.
    if (kind !== "wait") this.scriptMs.push(took);
    (this.kindMs[kind] ||= []).push(took);
    return answer;
  }

  run(source) {
    return this.page.evaluate(source).then((raw) => JSON.parse(raw));
  }

  /* The window keeps the form a pane's agent last read, and holds the
   * next fill to it (`known_form`): so does the driver. */
  /* A line of the trace, with the round trip it came at. */
  note(entry) {
    if (this.ask) this.said.push({ at: this.count.roundTrips, ...entry });
  }

  async fields() {
    const read = await this.call("fields", () => this.run(fieldsScript()));
    if (!read.ok) throw new Error(`fields refused: ${JSON.stringify(read)}`);
    this.form = read.value.fingerprint;
    this.seen = read.value.fields;
    this.buttons = read.value.actions || [];
    if (this.ask) this.note({ verb: "fields", words: this.ask({ op: "fields", label: "browser-1", json: false, read: read.value }).words });
    return read.value;
  }

  async fill(bundle) {
    const known = this.form, buttons = this.buttons;
    const filled = await this.call("fill", () => fillPasses((source) => this.run(source), bundle, this.form));
    if (this.ask) {
      // What was sent is recorded without the value of a field the fill left to `type`: that value goes by `type --value-stdin` alone and is in no record, as it is in no answer of the door.
      const sent = { ...bundle };
      for (const result of filled.results || []) if (result?.status === "secret" && result.handle in sent) sent[result.handle] = "(비밀 칸 — type으로)";
      this.note({ verb: "fill", sent, words: this.ask({ op: "fill", label: "browser-1", text: JSON.stringify(bundle), passes: filled.rounds, known, buttons }).words });
    }
    if (!filled.stale) this.buttons = filled.actions || [];
    if (filled.stale) {
      this.count.stale += 1;
      this.trail.push({ fill: "form_stale" });
      return { ...filled, left: [] };
    }
    this.form = filled.fingerprint;
    this.count.fillPasses += filled.passes;
    this.fresh = [...(filled.outside || []), ...(filled.alerts || []), ...filled.results.flatMap((result) => result?.fresh || [])];
    this.trail.push({ fill: filled.results.map((result) => `${result.label}:${result.status}`) });
    // What the driver does by hand after the fill's answer can turn a button on, so
    // the buttons that answer ended with are no longer the page's: the step is read again.
    const before = this.count.roundTrips;
    for (const result of filled.results) {
      if (result?.status === "secret") await this.typeSecret(result.handle, bundle[result.handle]);
      if (result?.status !== "no_option") continue;
      if (result.widget) await this.byHand(result, bundle[result.handle]);
      else await this.openAndPress(result, bundle[result.handle]);
    }
    // The form the fill's answer ended with is not the one read before it: the answer says so
    // (`양식 바뀜`), and a model reads the form again before it presses.
    return { ...filled, byHand: this.count.roundTrips > before, changed: known !== null && filled.fingerprint !== known };
  }

  /* A secret field: `fill` never writes one and says so (`secret`), naming
   * the road — `type <label> <handle> --value-stdin`, the setter. A model
   * takes it as the answer says: one `type` a field, with the fact's value,
   * once. A field inside a frame of the page's own origin is typed by its handle too (t-41720), held to the form the driver read last. */
  async typeSecret(handle, value, expect = this.form) {
    if (typeof value !== "string" || this.typed.has(handle)) return false;
    this.typed.add(handle);
    this.note({ verb: "type", handle });
    this.trail.push({ type: handle });
    const held = handle.includes(FORM_REQUEST.frameSeparator) ? expect : null;
    let typed = await this.call("type", () => this.run(typeScript(handle, value, "setter", held)));
    // The form is no longer the one that was read (the field typed before this one brought a field of its own): the window refuses, and the form is read again — as a model does — before the type is made once more.
    if (!typed.ok && typed.code === "form_stale") {
      const read = await this.fields();
      typed = await this.call("type", () => this.run(typeScript(handle, value, "setter", read.fingerprint)));
    }
    if (!typed.ok) throw new Error(`type refused: ${JSON.stringify(typed)}`);
    return true;
  }

  /* What a read still asks of the fields: those not yet typed as secrets. */
  untyped(fields) {
    return fields.filter((field) => !this.typed.has(field.handle));
  }

  /* The plan of a read: the facts to the fields of the whole form — the fields already typed as secrets are part of the form a value is cut across (a number in four boxes, two of them typed, is still cut across four) — and,
   * of what is to be written, not the fields already typed. */
  planOf(fields) {
    const planned = plan(fields, this.facts);
    return { ...planned, bundle: Object.fromEntries(Object.entries(planned.bundle).filter(([handle]) => !this.typed.has(handle))) };
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

  /* A field the door says the page keeps from being written and names the button that opens it (`opens`) is filled from the window that button opens, as a model
   * does with a pick from a list the page searches (t-41720): the button is pressed, the form read — a window with a frame may need a moment, waited for a fixed
   * number of times —, the card's words written into the one text field the window brought, the button that starts the search pressed, and the result whose words hold
   * the card's words whole pressed. Once for each button. True when it pressed anything, so the step is read again. */
  async openRoad(results, bundle, actions) {
    let pressed = false;
    for (const result of results || []) {
      if (!result || result.status !== "read_only" || !result.opens) continue;
      const value = bundle[result.handle];
      if (typeof value !== "string" || !value.trim() || this.byHandTried.has(result.opens.handle)) continue;
      this.byHandTried.add(result.opens.handle);
      const waits = this.waits;
      pressed = (await this.searchAndPick(result.opens, value, actions || this.buttons)) || pressed;
      this.waits = waits;
    }
    return pressed;
  }

  async searchAndPick(opener, value, actions) {
    const separator = FORM_REQUEST.frameSeparator;
    const known = new Set(this.seen.map((field) => field.handle));
    const knownButtons = new Set(actions.map((action) => action.handle));
    await this.press({ handle: opener.handle, label: opener.label });
    const brought = async () => (await this.fields()).fields.filter((field) => !known.has(field.handle) && field.kind === "text");
    let box = await brought();
    for (let waits = 0; !box.length && waits < WAITS_MAX; waits += 1) {
      await this.wait();
      box = await brought();
    }
    if (box.length !== 1) return true;
    await this.fill({ [box[0].handle]: value });
    const before = new Set(this.buttons.map((action) => action.handle));
    const starters = this.buttons.filter((action) => !knownButtons.has(action.handle) && !action.disabled);
    const start = starters.find((action) => action.submit) || starters.find((action) => action.beside === box[0].handle)
      || (starters.length === 1 ? starters[0] : null);
    if (!start) return true;
    await this.press(start);
    let found = [];
    for (let waits = 0; ; waits += 1) {
      await this.fields();
      found = this.buttons.filter((action) => !before.has(action.handle) && !action.disabled);
      if (found.length || waits >= WAITS_MAX) break;
      await this.wait();
    }
    const holding = found.filter((action) => holdsWhole(action.label, value));
    if (holding.length === 1) {
      await this.press(holding[0]);
      await this.fields();
    }
    return true;
  }

  /* Dates the card gives that no field takes, and buttons the door says open a dialog (`dialog`, the dialog's name) (t-41720): the facts whose words begin with the dialog's name are its dates.
   * Each is given to one of its buttons — the one whose words hold what the fact says besides the name, else the next in the page's order — by a fill, in the card's order, which opens the dialog,
   * pages its calendar and presses the day; and once two are picked the button of the dialog that turned on (the one that applies the range) is pressed. Once for each dialog. True when it
   * pressed anything, so the step is read again. */
  async dialogRoad(unplaced, actions) {
    const dates = (unplaced || []).filter((fact) => dateLike(fact.value));
    const names = [...new Set((actions || []).filter((action) => action.dialog && !action.disabled).map((action) => action.dialog))];
    if (!dates.length || !names.length) return false;
    const tokens = (text) => fold(text).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
    const lead = (said, own) => {
      let run = 0;
      while (run < said.length && run < own.length && said[run] === own[run]) run += 1;
      return run;
    };
    let pressed = false;
    for (const name of names) {
      if (this.byHandTried.has(`dialog:${name}`)) continue;
      const own = tokens(name);
      const mine = dates.filter((fact) => {
        const said = tokens(fact.says);
        const here = lead(said, own);
        return here >= 1 && names.every((other) => lead(said, tokens(other)) <= here);
      });
      const buttons = (actions || []).filter((action) => action.dialog === name && !action.disabled);
      if (!mine.length || mine.length > buttons.length) continue;
      const left = [...buttons];
      const given = new Map();
      const rest = (fact) => tokens(fact.says).filter((word) => !own.includes(word));
      for (const fact of [...mine].sort((a, b) => Number(!rest(a).length) - Number(!rest(b).length))) {
        const words = rest(fact);
        const fits = left.filter((button) => words.length && words.some((word) => tokens(button.label).includes(word)));
        const button = fits.length === 1 ? fits[0] : (!words.length ? left[0] : null);
        if (!button) break;
        left.splice(left.indexOf(button), 1);
        given.set(fact, button);
      }
      if (given.size !== mine.length) continue;
      this.byHandTried.add(`dialog:${name}`);
      await this.fields();
      let before = null;
      let last = null;
      for (const fact of mine) {
        before = last;
        last = await this.fill({ [given.get(fact).handle]: fact.value.trim() });
      }
      pressed = true;
      // The buttons the second pick turned on: the one that applies the range, and the one that goes back a month when the second day stood in a later month than the first. The one
      // that applies stands after the one that goes back in the dialog — the page's last of them, as the button that moves a form on is taken.
      const turned = before && last ? (last.actions || []).filter((action) => !action.disabled
        && (before.actions || []).some((one) => one.handle === action.handle && one.disabled)) : [];
      if (turned.length) await this.press(turned[turned.length - 1]);
      await this.fields();
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
    this.everPressed.add(action.handle);
    this.after = null;
    if (action.handle.includes(FORM_REQUEST.frameSeparator)) {
      this.note({ verb: "eval", press: action.handle, label: action.label });
      const pressed = await this.call("eval", () => this.run(evalScript(pressButton(action.handle))));
      if (!pressed.ok) throw new Error(`press refused: ${JSON.stringify(pressed)}`);
      return pressed;
    }
    // A pane whose form was never read is pressed as it always was.
    if (this.form === null) {
      this.note({ verb: "click", handle: action.handle, label: action.label });
      const pressed = await this.call("click", () => this.run(clickScript(action.handle)));
      if (!pressed.ok) throw new Error(`click refused: ${JSON.stringify(pressed)}`);
      return pressed;
    }
    // One in a form that was read is pressed in it: the page waited for and read, set against that form.
    const known = this.form, buttons = this.buttons;
    const done = await this.call("click", () => pressInForm((source) => this.run(source), action.handle));
    const entry = { verb: "click", handle: action.handle, label: action.label };
    if (this.ask) entry.words = this.ask({ op: "press", label: "browser-1", read: done.read, known, buttons, moving: done.moving }).words;
    this.note(entry);
    this.fresh = [...(done.read.outside || []), ...(done.read.alerts || []), ...(done.read.noted || []).flatMap((one) => one.fresh || [])];
    this.after = { stayed: done.read.fingerprint === known, noted: done.read.noted || [], actions: done.read.actions || [] };
    if (done.read.fingerprint === known) {
      this.form = done.read.fingerprint;
      this.buttons = done.read.actions || this.buttons;
    }
    return done.pressed;
  }

  /* A field's own button (the door says which button stands beside which field, `beside`): after a press that left the form as it was, when
   * the door says a field that holds a value has new text — the page's answer about it — and one button on beside that field has not been
   * pressed, it is pressed once and the form is read again, as a model does with the "apply" of a code the page asked to be applied. True
   * when pressed. */
  async ownButton() {
    const after = this.after;
    if (!after || !after.stayed) return false;
    for (const field of after.noted) {
      const holds = field.value !== "" && field.value !== false && !(Array.isArray(field.value) && !field.value.length);
      if (!holds || !(field.fresh || []).length) continue;
      const own = after.actions.filter((action) => action.beside === field.handle && !action.disabled && !this.everPressed.has(action.handle));
      if (own.length !== 1) continue;
      this.trail.push({ own: own[0].label, beside: field.handle });
      await this.press(own[0]);
      // The page answers the buttons pressed so far differently now: the count of the same press in the same form starts again.
      this.pressed.clear();
      this.waits = 0;
      return true;
    }
    return false;
  }

  /* A field the page keeps off until a box is read to its end: the read says why in the page's
   * words (`hint`) and names the box (`scrollBoxes`), and the skill teaches scrolling it with an
   * `eval`. Each box once; true when one was scrolled, so the form is read again. */
  async scrollRoad(read) {
    if (!(read.fields || []).some((field) => field.disabled && field.hint)) return false;
    let scrolled = false;
    for (const box of read.scrollBoxes || []) {
      if (this.scrolled.has(box.handle)) continue;
      this.scrolled.add(box.handle);
      this.note({ verb: "eval", scroll: box.handle });
      this.trail.push({ scroll: box.handle });
      const answer = await this.call("eval", () => this.run(evalScript(scrollToEnd(box.handle))));
      if (!answer.ok) throw new Error(`scroll refused: ${JSON.stringify(answer)}`);
      scrolled = true;
    }
    return scrolled;
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

  /* Whether the page has taken what was given: the harness reads the page's own result — no round trip of
   * the model's; the model reads the page's words after a press. */
  async finished() {
    return Boolean(await this.page.evaluate(() => window.__sceneResult || null));
  }

  /* A wait, as a model makes one when the page is still at work: a round trip that lets the page's own
   * timers run; the form is read again after it. */
  async wait() {
    this.waits += 1;
    this.note({ verb: "wait" });
    this.trail.push({ wait: this.waits });
    await this.call("wait", () => this.page.waitForTimeout(WAIT_MS));
  }

  /* The verbs road: read, fill, then the step's one press — by the buttons
   * the last fill's answer ended with (a button that turns on once the fields
   * are right is on there), else by the read's. A press the page refuses (a
   * field it brought late is still empty) is read again. */
  async verbs() {
    for (;;) {
      const read = await this.fields();
      if (await this.finished()) return this.done();
      if (await this.scrollRoad(read)) continue;
      const { bundle, unplaced, ambiguous } = this.planOf(read.fields);
      if (ambiguous.length) throw new Error(ambiguousWords(ambiguous));
      if (this.price) this.priced += await this.today(read, bundle);
      if (await this.unknownRoad(read.unknowns, unplaced)) continue;
      if (await this.dialogRoad(unplaced, read.actions)) continue;
      let actions = read.actions;
      if (Object.keys(bundle).length) {
        const filled = await this.fill(bundle);
        if (filled.stale || filled.byHand || filled.changed) continue;
        if (await this.openRoad(filled.results, bundle, filled.actions.length ? filled.actions : read.actions)) continue;
        if (filled.actions.length) actions = filled.actions;
      }
      const owed = await this.code(read.fields, bundle);
      if (owed && this.price) this.priced += 1;
      if (owed) {
        const filled = await this.fill(owed);
        if (filled.stale || filled.byHand || filled.changed) continue;
        if (filled.actions.length) actions = filled.actions;
      }
      if (await this.step(actions, read.actions, [...Object.keys(bundle), ...Object.keys(owed || {})])) return this.done();
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

  /* The step's press: a code to send while one is owed (`codeSender`), a code written
   * confirmed by its own button once when the page has one (`codeConfirmer`) — both chosen by the
   * place the door gave the button — else the button that moves
   * the form on (`advancing`, by what the door said of the buttons) — when that one is off
   * while the door has said new text, waited for a fixed number of times and the form read again —
   * true when the page has taken what was given. */
  async step(actions, before = [], written = []) {
    const live = actions.filter((action) => !action.disabled);
    if (this.codeTurn && !this.codeSent) {
      const send = codeSender(live, this.seen, written);
      if (send) {
        this.beforeSend = new Set(this.seen.map((field) => field.handle));
        await this.press(send);
        this.codeSent = true;
        return false;
      }
    }
    if (this.codeField && !this.codeConfirmed) {
      this.codeConfirmed = true;
      const confirm = codeConfirmer(live, this.codeField);
      if (confirm) {
        await this.press(confirm);
        return false;
      }
    }
    const advance = advancing(actions, before, written);
    // A page with no button at all, or whose button — the one just pressed — went off, is still at work, or
    // has finished and shows nothing to press: it is waited for, then read again.
    if (!advance || (advance.disabled && (this.fresh.length || advance.handle === this.justPressed))) {
      if (this.waits < WAITS_MAX) {
        await this.wait();
        return false;
      }
      if (!advance) throw new Error("stuck: the form has no button");
    }
    if (advance.disabled) throw new Error(`stuck: no button moves the form on (${live.map((a) => a.label).join(", ")})`);
    const key = `${this.form}|${advance.handle}`;
    if ((this.pressed.get(key) || 0) >= 2) throw new Error(`stuck: ${advance.label} was pressed twice and the form stayed — nothing is left to give it`);
    this.pressed.set(key, (this.pressed.get(key) || 0) + 1);
    this.waits = 0;
    this.fresh = [];
    this.justPressed = advance.handle;
    await this.press(advance);
    if (await this.finished()) return true;
    await this.ownButton();
    return false;
  }

  /* The script road: one eval a step reads, fills by the words it read and
   * presses the button that moves the step on (`advancing`) — or, while the person's code is owed and
   * the step can send one, holds it; with a code just written, presses the
   * code's own confirm button first. Sends, and a button the page declares a submit, go by click. */
  async script() {
    const STEP = `(() => {
      const fold = ${fold.toString()};
      const PART = ${PART.toString()};
      const words = ${words.toString()};
      const UNITS = ${JSON.stringify(UNITS)};
      const unitOf = ${unitOf.toString()};
      const unitGroups = ${unitGroups.toString()};
      const captionGroups = ${captionGroups.toString()};
      const momentOf = ${momentOf.toString()};
      const splitAtJoints = ${splitAtJoints.toString()};
      const kindFits = ${kindFits.toString()};
      const plan = ${plan.toString()};
      const took = ${took.toString()};
      const INTENT = ${JSON.stringify(INTENT)};
      const intentOf = ${intentOf.toString()};
      const codeSender = ${codeSender.toString()};
      const codeConfirmer = ${codeConfirmer.toString()};
      const advancing = ${advancing.toString()};
      const dateLike = ${dateLike.toString()};
      const facts = __FACTS__;
      const turn = __TURN__;
      const typed = __TYPED__;
      const read = zerocode.fields();
      const fields = read.fields.filter((field) => !typed.includes(field.handle));
      // The fields already typed are part of the form a value is cut across, and no part of what is written again.
      const planned = plan(read.fields, facts);
      const bundle = Object.fromEntries(Object.entries(planned.bundle).filter(([handle]) => !typed.includes(handle)));
      const { unplaced, ambiguous } = planned;
      // A card that does not tell which of two fields a fact is for stops the step before anything is written or pressed.
      const stop = ambiguous.length > 0;
      const filled = !stop && Object.keys(bundle).length ? zerocode.fill(bundle, read) : { results: [], left: fields
        .filter((field) => field.required && (field.value === "" || field.value === false)) };
      const left = filled.left.filter((field) => !typed.includes(field.handle));
      // Which fields the fill named for typing — the handles only: the page's answer
      // redacts any key that looks like a secret, and no value goes back through it.
      const viaType = filled.results.filter((result) => result.status === "secret").map((result) => result.handle);
      const clean = filled.results.every((result) => took(result.status)) && !left.length;
      // The fill's own values brought a field: the form is not the one read, and it is read again before a press.
      const moved = Object.keys(bundle).length > 0 && Boolean(filled.fingerprint) && filled.fingerprint !== read.fingerprint;
      const actions = filled.actions && filled.actions.length ? filled.actions : read.actions;
      const live = actions.filter((action) => !action.disabled);
      const inFrame = (action) => action.handle.includes(${JSON.stringify(FORM_REQUEST.frameSeparator)});
      const confirm = turn.confirm && clean && !moved && !stop ? codeConfirmer(live.filter((action) => !inFrame(action)), turn.codeField) : null;
      if (confirm) document.querySelector(confirm.handle).click();
      // The code is owed while the step can send one, and once it was sent until it is written.
      const hold = turn.owed && (turn.sent || codeSender(live, read.fields, Object.keys(bundle)) !== null);
      // The dates of the card that no field takes are the buttons' that open a dialog (the driver gives them once and then says the dialog's name in turn.dialogs): the step's own button waits for them.
      const owesDates = unplaced.some((fact) => dateLike(fact.value)) && actions.some((action) => action.dialog && !action.disabled && !turn.dialogs.includes(action.dialog));
      const next = advancing(actions, read.actions, Object.keys(bundle));
      const pressNext = clean && !moved && !stop && !confirm && !hold && !owesDates && !!next && !next.disabled && !next.submit && !inFrame(next);
      if (pressNext) document.querySelector(next.handle).click();
      const hand = filled.results.filter((result) => result.status === "no_option")
        .map((result) => ({ ...result, asked: bundle[result.handle] }));
      const fresh = [].concat(filled.outside || [], filled.alerts || [], ...filled.results.map((result) => result.fresh || []));
      return { results: filled.results.map((result) => result.label + ":" + result.status), left,
        actions, before: read.actions.map((action) => ({ handle: action.handle, disabled: action.disabled })), fresh, pressedNext: pressNext, pressedHandle: pressNext ? next.handle : null,
        print: read.fingerprint, hand, viaType, unknowns: read.unknowns, unplaced, ambiguous, clean, moved,
        readOnly: filled.results.filter((result) => result.status === "read_only" && result.opens)
          .map((result) => ({ handle: result.handle, status: result.status, opens: result.opens })),
        scrollBoxes: read.scrollBoxes || [],
        confirmed: !!confirm, bundle, fields: read.fields.map((field) => ({ handle: field.handle, kind: field.kind, label: field.label,
          value: field.value, required: field.required, disabled: field.disabled, hint: field.hint })) };
    })()`;
    for (;;) {
      const turn = { owed: this.codeTurn, sent: this.codeSent, confirm: Boolean(this.codeField) && !this.codeConfirmed, codeField: this.codeField,
        dialogs: [...this.byHandTried].filter((key) => key.startsWith("dialog:")).map((key) => key.slice("dialog:".length)) };
      const source = evalFormScript(STEP.replace("__FACTS__", () => JSON.stringify(this.facts))
        .replace("__TURN__", () => JSON.stringify(turn))
        .replace("__TYPED__", () => JSON.stringify([...this.typed])));
      const answer = await this.call("eval", () => this.run(source));
      if (!answer.ok) throw new Error(`eval refused: ${JSON.stringify(answer)}`);
      const said = answer.value;
      this.seen = said.fields;
      if (await this.finished()) return this.done();
      if (said.ambiguous.length) throw new Error(ambiguousWords(said.ambiguous));
      if (turn.confirm && said.clean) this.codeConfirmed = true;
      this.trail.push({ script: said.results, pressedNext: said.pressedNext, confirmed: said.confirmed });
      this.note({ verb: "eval", results: said.results, left: said.left.map((field) => `${field.handle} ${field.label}`),
        buttons: said.actions.map((action) => `${action.handle} ${action.label}${action.disabled ? " (꺼짐)" : ""}`), pressedNext: said.pressedNext });
      for (const result of said.hand || []) {
        if (result.widget) await this.byHand(result, result.asked);
        else await this.openAndPress(result, result.asked);
      }
      // The value is the card's: the driver reads it where it stands, not from the page's answer.
      const owedTyping = this.planOf(said.fields).bundle;
      for (const handle of said.viaType || []) await this.typeSecret(handle, owedTyping[handle], said.print);
      if (await this.scrollRoad({ fields: said.fields, scrollBoxes: said.scrollBoxes })) continue;
      if (said.moved) continue;
      if (said.pressedNext || said.confirmed) {
        if (said.pressedNext) {
          const key = `${said.print}|${said.pressedHandle}`;
          this.pressed.set(key, (this.pressed.get(key) || 0) + 1);
          if (this.pressed.get(key) > 2) throw new Error(`stuck: ${said.pressedHandle} was pressed twice and the form stayed — nothing is left to give it`);
          this.waits = 0;
          this.justPressed = said.pressedHandle;
        }
        continue;
      }
      if (await this.openRoad(said.readOnly, said.bundle || {}, said.actions)) continue;
      if (await this.dialogRoad(said.unplaced, said.actions)) continue;
      if (await this.unknownRoad(said.unknowns, said.unplaced)) continue;
      if (await this.code(said.fields, said.bundle)) continue;
      // What is left or did not take is tried again once; the same answer twice is the page's, and the step goes on.
      const sig = JSON.stringify([said.results, said.left.map((field) => field.handle)]);
      const again = sig === this.lastSig;
      this.lastSig = sig;
      if (!again && (said.left.length || said.results.some((result) => !/:(set|same|unseen)$/.test(result)))) continue;
      this.fresh = said.fresh;
      this.form = said.print;
      if (await this.step(said.actions, said.before, Object.keys(said.bundle || {}))) return this.done();
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
  const { card, expected, made, shapes } = await readScene(folder);
  const site = await serve(folder);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const road = new Road(page, card, roadName);
  road.ask = doorWords;
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
  // The verdicts side by side — the first rule as it was (`wholeRaw`), the whole result but for the
  // keys the scene says its page makes itself (`whole`), the expected keys alone (`ok`) — and the
  // keys only the page recorded: a key no card names and no declaration excuses is a field an agent
  // could have written. A pass is told only when all hold. The page's whole result is kept in the row.
  const v = verdicts(result, expected, made, shapes);
  const scriptMs = road.scriptMs.slice().sort((a, b) => a - b);
  const fillMs = (road.kindMs.fill || []).slice().sort((a, b) => a - b);
  // How long each kind of round trip took, by kind, as count / median / longest in ms: what the
  // door spends reading a page (`fields`), writing it (`fill`) and pressing in it (`click`).
  const ms = Object.fromEntries(Object.entries(road.kindMs).map(([kind, list]) => {
    const sorted = list.slice().sort((a, b) => a - b);
    return [kind, { n: sorted.length, p50: Math.round(sorted[Math.floor(sorted.length / 2)]), max: Math.round(sorted.at(-1)) }];
  }));
  return {
    scene: folder.split(sep).filter(Boolean).pop(), road: roadName, pass: v.pass, ok: v.ok, wholeRaw: v.wholeRaw, whole: v.whole, sameSet: v.sameSet,
    extra: v.extra, made: v.made, madeProblems: v.madeProblems, stuck, wrong: result ? v.wrong : null, result,
    ...road.count, projectedSeconds: road.count.roundTrips * SECONDS_PER_ROUND_TRIP,
    callMsP50: Math.round(scriptMs[Math.floor(scriptMs.length / 2)] ?? 0), callMsMax: Math.round(scriptMs.at(-1) ?? 0),
    fillMsP50: Math.round(fillMs[Math.floor(fillMs.length / 2)] ?? 0), fillMsMax: Math.round(fillMs.at(-1) ?? 0), ms,
    wallMs: Math.round(performance.now() - began), trail: road.trail,
    ...(doorWords ? { trace: road.said } : {}),
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
/* What the core says to a read and to a fill, for a run's trace. */
const doorWords = options["door-text"]
  ? (request) => JSON.parse(spawnSync(resolve(options["door-text"]), [], { input: JSON.stringify(request), encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }).stdout)
  : null;
const browser = await chromium.launch();
const rows = [];
try {
  for (const folder of options.scene) {
    for (const road of ["verbs", "script"]) {
      const row = await drive(browser, folder, road);
      rows.push(row);
      console.log(`DRIVE ${row.scene} ${row.road} pass=${row.pass} ${verdictWords(row)} roundTrips=${row.roundTrips} fields=${row.fields} fill=${row.fill} click=${row.click} eval=${row.eval} type=${row.type} handoff=${row.handoff} passes=${row.fillPasses} callMs p50=${row.callMsP50} max=${row.callMsMax} fillMs p50=${row.fillMsP50} max=${row.fillMsMax} ms ${Object.entries(row.ms).map(([kind, one]) => `${kind}=${one.n}/${one.p50}/${one.max}`).join(" ")}${row.stuck ? " stuck=" + row.stuck : ""}${row.wrong?.length ? " wrong=" + row.wrong.join(",") : ""}`);
    }
    const today = await countToday(browser, folder);
    rows.push(today);
    console.log(`DRIVE ${today.scene} today counted roundTrips=${today.roundTrips}${today.stuck ? " stuck=" + today.stuck : ""}`);
  }
} finally {
  await browser.close();
}
await writeFile(options.out, JSON.stringify(rows, null, 2) + "\n");
process.exit(rows.every((row) => row.counted || row.pass) ? 0 : 1);
