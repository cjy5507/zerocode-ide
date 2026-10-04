//! The browser door's form pair, window half (t-37883): the page scripts
//! `fields` reads a page's forms with and `fill` writes a bundle with, and
//! the road that runs them — the tables, the bundle's grammar and the words
//! are the core's (`zerocode_core::browser_form`).
//!
//! Every script is synchronous, like every other page script of the door:
//! an engine that does not await a promise (WebKit, WebView2) gets the same
//! answer as one that does, and the clock between the passes of a fill is
//! the window's, never a timer left in the person's signed-in page. Nothing
//! reads a rect or waits for a frame, so a tab nobody is looking at reads and
//! fills as a shown one does.

use super::*;
use zerocode_core::browser_form::{
    BROWSER_FILL_OFF, BROWSER_FILL_ON, BROWSER_FILL_PENDING_MS, BROWSER_FORM_ACTION_CAP,
    BROWSER_FORM_ACTIONS, BROWSER_FORM_CAPTION_DEPTH, BROWSER_FORM_CONTROLS, BROWSER_FORM_DAY_WORDS,
    BROWSER_FORM_DAYS, BROWSER_FORM_FIELD_CAP, BROWSER_FORM_FRAME_DEPTH,
    BROWSER_FORM_FRAME_SEPARATOR, BROWSER_FORM_NOT_FIELDS, BROWSER_FORM_OPTION_CAP,
    BROWSER_FORM_OPTIONS, BROWSER_FORM_SCOPES, FillEntry, FillLedger, FillPass, FillReport,
    FormRead,
};
use zerocode_core::agent_browser::BROWSER_EVAL_FORM_OBJECT;

/// What a read of a page's forms is made of, page side — read only, like the
/// marks helpers it stands on (`BROWSER_MARK_HELPERS`: a field's name, the
/// heading of its region, a structural selector): which elements are
/// fields, the words a field is known by when nothing names it, a handle
/// that finds it again (inside same-origin frames too), what it holds and
/// what it offers. Shared by `fields` and by `fill`'s read-back, so a fill
/// checks a field by the very read the agent was shown.
pub(crate) const BROWSER_FORM_HELPERS: &str = r##"
const zcFormTag = (el) => String((el && el.tagName) || "").toLowerCase();
const zcFormRole = (el) => String((el && el.getAttribute && el.getAttribute("role")) || "").trim().toLowerCase();
const zcFold = (text) => String(text == null ? "" : text).replace(/\s+/g, " ").trim().toLowerCase();
// Whether the page draws an element at all — by its own styles, never by
// where it sits: a field below the fold, or in a tab nobody looks at, is
// still one the form sends.
const zcDrawn = (el) => {
  if (!el || !el.isConnected || el.hidden) return false;
  if (typeof el.checkVisibility === "function") {
    return el.checkVisibility({ visibilityProperty: true, checkVisibilityCSS: true });
  }
  const view = el.ownerDocument.defaultView || window;
  return view.getComputedStyle(el).visibility !== "hidden" && el.getClientRects().length > 0;
};
// A choice a page draws through its label (an input it hides behind a
// styled label) is a choice a person makes, so it counts as drawn.
const zcShown = (el) => zcDrawn(el) || [...(el.labels || [])].some(zcDrawn);
const zcOff = (el) => (el.matches && el.matches(":disabled")) || !!el.closest('[aria-disabled="true"]');
const zcOnly = (el, selector) => {
  try {
    const all = el.ownerDocument.querySelectorAll(selector);
    return all.length === 1 && all[0] === el;
  } catch (_) {
    return false;
  }
};
// The handle a field is found again by: its id, else its name, when either
// names it alone in its document; else its structural path.
const zcHandleOf = (el) => {
  if (el.id && zcOnly(el, "#" + CSS.escape(el.id))) return "#" + CSS.escape(el.id);
  const name = el.getAttribute("name");
  if (name) {
    const by = zcFormTag(el) + '[name="' + CSS.escape(name) + '"]';
    if (zcOnly(el, by)) return by;
  }
  return zcMarkSelector(el);
};
const zcRadios = (radio) => radio.name
  ? [...radio.ownerDocument.querySelectorAll('input[type="radio"]')]
    .filter((other) => other.name === radio.name && other.form === radio.form)
  : [radio];
const zcGroupHandle = (first) => {
  if (!first.name) return zcHandleOf(first);
  const by = 'input[type="radio"][name="' + CSS.escape(first.name) + '"]';
  const all = [...first.ownerDocument.querySelectorAll(by)];
  if (all.every((radio) => radio.form === first.form)) return by;
  return (first.form ? zcHandleOf(first.form) + " " : "") + by;
};
// The words just before an element, in its own box and the few around it,
// after any field that comes between and outside another field's label —
// the caption a page writes beside a field without tying it to the field.
const zcWordsBefore = (el, request) => {
  const doc = el.ownerDocument;
  const controls = request.controls.join(",");
  const skip = "select, textarea, button, option, script, style, template, [role=option], [role=listbox]";
  let scope = el.parentElement;
  for (let level = 0; scope && level < request.captionDepth; level += 1, scope = scope.parentElement) {
    const walker = doc.createTreeWalker(scope, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    let last = "";
    for (let node = walker.nextNode(); node && node !== el; node = walker.nextNode()) {
      if (node.nodeType === 1) {
        if (node.matches(controls) && !node.contains(el)) last = "";
        continue;
      }
      const holder = node.parentElement;
      if (holder && holder.closest(skip)) continue;
      const label = holder && holder.closest("label");
      if (label && label.control && label.control !== el) continue;
      if (/[\p{L}\p{N}]/u.test(node.nodeValue)) last = node.nodeValue;
    }
    if (last.trim()) return last;
  }
  return "";
};
// A field's caption when nothing names it: its table row's header, the term
// its definition follows, the words just before it.
const zcCaption = (el, request) => {
  const cell = el.closest("td, [role=cell], [role=gridcell]");
  const head = cell && cell.parentElement && cell.parentElement.querySelector("th, [role=rowheader]");
  if (head && zcLabelWords(head).trim()) return zcLabelWords(head);
  const definition = el.closest("dd");
  const term = definition && definition.previousElementSibling;
  if (term && zcFormTag(term) === "dt" && zcLabelWords(term).trim()) return zcLabelWords(term);
  return zcWordsBefore(el, request);
};
const zcErrorOf = (el) => {
  if (el.getAttribute("aria-invalid") === "true") {
    const doc = el.ownerDocument;
    const ids = (String(el.getAttribute("aria-errormessage") || "") + " "
      + String(el.getAttribute("aria-describedby") || "")).split(/\s+/).filter(Boolean);
    const said = ids.map((id) => doc.getElementById(id)).filter(Boolean)
      .map((node) => node.textContent).join(" ");
    return said.trim() || el.validationMessage || "aria-invalid";
  }
  if (el.validity && !el.validity.valid && !el.validity.valueMissing) return el.validationMessage || "";
  return "";
};
const zcChoiceWords = (choice, cap) => zcWords(zcNameOf(choice) || choice.innerText || choice.textContent
  || choice.getAttribute("value") || "", cap);
// The choices a dropdown the page draws itself shows now: the options of
// the list it controls, else every drawn option of its document.
const zcDrawnOptions = (el, request) => {
  const doc = el.ownerDocument;
  const typed = zcFormTag(el) === "input" ? el : el.querySelector("input");
  const ids = [el, typed].filter(Boolean)
    .map((node) => String(node.getAttribute("aria-controls") || node.getAttribute("aria-owns") || ""))
    .join(" ").split(/\s+/).filter(Boolean);
  const lists = ids.map((id) => doc.getElementById(id)).filter(Boolean);
  const found = [];
  for (const scope of lists.length ? lists : [doc]) {
    for (const option of scope.querySelectorAll(request.options.join(","))) {
      if (!zcDrawn(option) || option.getAttribute("aria-disabled") === "true") continue;
      found.push({ el: option, words: zcChoiceWords(option, request.wordCap),
        value: String(option.getAttribute("data-value") || ""),
        selected: option.getAttribute("aria-selected") === "true" });
    }
  }
  return found;
};
// The kind of field an element is, or null for one that is no field: HTML's
// input type, a select, a textarea, an editable region, ARIA's widgets. A
// radio input stands for its group; an input inside a combobox, for the
// combobox.
const zcFieldKind = (el, request) => {
  const tag = zcFormTag(el), role = zcFormRole(el);
  if (tag === "input") {
    const type = String(el.type || "text").toLowerCase();
    if (request.notFields.includes(type)) return null;
    if (role === "combobox") return "combobox";
    if (el.parentElement && el.parentElement.closest("[role=combobox]")) return null;
    return type;
  }
  if (tag === "select") return el.multiple ? "select-multiple" : "select";
  if (tag === "textarea") return "textarea";
  if (role === "checkbox" || role === "switch") return "checkbox";
  if (role === "radiogroup") return el.querySelector('input[type="radio"]') ? null : "radio";
  if (role === "combobox") return "combobox";
  return el.isContentEditable ? "text" : null;
};
// One field, read: the element it is pressed and written through, its
// choices (each with its element), what it holds, and its words.
const zcRead = (el, request) => {
  const kind = zcFieldKind(el, request);
  if (!kind) return null;
  const cap = request.wordCap;
  const record = { el, kind, choices: [], value: "", error: zcErrorOf(el),
    required: el.required === true || el.getAttribute("aria-required") === "true",
    disabled: zcOff(el), readOnly: el.readOnly === true || el.getAttribute("aria-readonly") === "true",
    masked: zcSecretField(el),
    maxLength: typeof el.maxLength === "number" && el.maxLength >= 0 ? el.maxLength : null,
    placeholder: String(el.getAttribute("placeholder") || el.getAttribute("aria-placeholder") || "") };
  if (kind === "radio") {
    const group = zcFormTag(el) === "input" ? null : el;
    const radios = group ? [...group.querySelectorAll("[role=radio]")] : zcRadios(el);
    const first = radios[0] || el;
    record.el = group || first;
    record.choices = radios.map((radio) => ({ el: radio, words: zcChoiceWords(radio, cap),
      value: String(radio.getAttribute("value") || ""),
      selected: zcFormTag(radio) === "input" ? radio.checked : radio.getAttribute("aria-checked") === "true" }));
    const picked = record.choices.find((choice) => choice.selected);
    record.value = picked ? picked.words : "";
    record.required = record.required || radios.some((radio) => radio.required);
    record.disabled = radios.length > 0 && radios.every(zcOff);
    record.handle = group ? zcHandleOf(group) : zcGroupHandle(first);
    const legend = first.closest("fieldset") && first.closest("fieldset").querySelector("legend");
    record.caption = (group && zcNameOf(group)) || (legend && legend.textContent) || zcCaption(first, request);
    record.label = record.caption;
    return record;
  }
  record.handle = zcHandleOf(el);
  if (kind === "checkbox") {
    record.value = zcFormTag(el) === "input" ? el.checked : el.getAttribute("aria-checked") === "true";
  } else if (kind.startsWith("select")) {
    record.choices = [...el.options].filter((option) => !option.disabled && option.value !== "")
      .map((option) => ({ el: option, words: zcWords(option.label || option.text, cap),
        value: option.value, selected: option.selected }));
    record.value = record.choices.filter((choice) => choice.selected).map((choice) => choice.words).join(", ");
  } else if (kind === "combobox") {
    const typed = zcFormTag(el) === "input" ? el : el.querySelector("input");
    record.choices = zcDrawnOptions(el, request);
    record.value = (typed && typed.value)
      || zcWords((typed === el ? el.parentElement : el).innerText || "", request.valueCap);
  } else if (!record.masked) {
    record.value = ["input", "textarea"].includes(zcFormTag(el)) ? String(el.value) : String(el.innerText || "");
  }
  record.caption = zcCaption(el, request);
  // A checkbox the page draws itself is named by its own words (ARIA's
  // name from content), before any caption around it.
  const content = kind === "checkbox" && zcFormTag(el) !== "input" ? zcWords(el.innerText || "", cap) : "";
  record.label = zcNameOf(el) || content || record.caption || record.placeholder
    || String(el.getAttribute("name") || "");
  return record;
};
// Fields side by side in one box with one caption — a phone number in three
// boxes, a date in three selects — are that caption's parts, numbered.
const zcNumberRuns = (records) => {
  let at = 0;
  while (at < records.length) {
    const head = records[at];
    let end = at + 1;
    while (end < records.length && records[end].el.parentElement === head.el.parentElement
      && records[end].kind !== "radio"
      && (!zcFold(records[end].caption) || zcFold(records[end].caption) === zcFold(head.caption))
      && !zcNameOf(records[end].el)) end += 1;
    if (end - at > 1 && zcFold(head.label)) {
      for (let part = at; part < end; part += 1) {
        records[part].label = head.label.trim() + " (" + (part - at + 1) + "/" + (end - at) + ")";
      }
    }
    at = end;
  }
};
// The documents a read walks: the page, and every frame inside it whose
// page this one may read (same origin), each with the handle path that
// reaches it; a frame of another origin is named by its host, unread.
const zcDocs = (request) => {
  const open = [{ doc: document, prefix: "", depth: 0 }];
  const sealed = [];
  for (let at = 0; at < open.length; at += 1) {
    const { doc, prefix, depth } = open[at];
    if (depth >= request.frameDepth) continue;
    for (const frame of doc.querySelectorAll("iframe, frame")) {
      if (!zcDrawn(frame)) continue;
      let inner = null;
      try { inner = frame.contentDocument; } catch (_) {}
      if (inner && inner.body) {
        open.push({ doc: inner, prefix: prefix + zcHandleOf(frame) + request.frameSeparator, depth: depth + 1 });
      } else {
        let host = "";
        try { host = new URL(frame.getAttribute("src") || "", location.href).host; } catch (_) {}
        sealed.push(host || zcHandleOf(frame));
      }
    }
  }
  return { open, sealed };
};
const zcFieldOut = (record, request) => {
  const cap = request.wordCap;
  const out = { handle: record.handle, kind: record.kind, label: zcWords(record.label, cap),
    section: zcWords(zcNear(record.el, request.field), cap),
    value: record.masked ? "" : typeof record.value === "boolean" ? record.value
      : zcCut(String(record.value), request.valueCap),
    options: record.choices.slice(0, request.optionCap).map((choice) => choice.words),
    moreOptions: Math.max(0, record.choices.length - request.optionCap),
    required: record.required, disabled: record.disabled, readOnly: record.readOnly,
    masked: record.masked, placeholder: zcWords(record.placeholder, cap), error: zcWords(record.error, cap) };
  if (record.maxLength !== null) out.maxLength = record.maxLength;
  return out;
};
const zcEmpty = (out) => out.value === "" || out.value === false;
// Every field the page draws, in its order, frames after the page, and the
// buttons that stand with them.
const zcFormFields = (request) => {
  const docs = zcDocs(request);
  const records = [];
  const seen = new Set();
  let more = 0;
  for (const { doc, prefix } of docs.open) {
    for (const el of doc.querySelectorAll(request.controls.join(","))) {
      if (!zcShown(el)) continue;
      const record = zcRead(el, request);
      if (!record) continue;
      record.handle = prefix + record.handle;
      if (seen.has(record.handle)) continue;
      seen.add(record.handle);
      if (records.length >= request.fieldCap) {
        more += 1;
        continue;
      }
      records.push(record);
    }
  }
  zcNumberRuns(records);
  const scopes = new Set(records.map((record) => record.el.closest(request.scopes.join(","))
    || record.el.ownerDocument.body));
  const actions = [];
  const named = new Set();
  for (const { doc, prefix } of docs.open) {
    for (const el of doc.querySelectorAll(request.actions.join(","))) {
      if (actions.length >= request.actionCap) break;
      if (!zcDrawn(el) || zcFormRole(el) === "combobox") continue;
      if (![...scopes].some((scope) => scope.contains(el))) continue;
      const label = zcWords(zcMarkName(el), request.wordCap);
      const handle = prefix + zcHandleOf(el);
      if (!label || named.has(handle)) continue;
      named.add(handle);
      actions.push({ handle, label, disabled: zcOff(el) });
    }
  }
  return { records, fields: records.map((record) => zcFieldOut(record, request)), actions,
    more, sealed: docs.sealed };
};
"##;

/// The `fields` read: every field and its buttons, in one synchronous pass
/// that writes nothing.
pub(crate) const BROWSER_FIELDS_BODY: &str = r#"
const read = zcFormFields(request);
return zcEncode({ ok: true, value: { fields: read.fields, actions: read.actions, more: read.more,
  sealedFrames: read.sealed } }, request.answerCap);
"#;

/// What `fill` writes with, page side — standing on the form helpers: each
/// entry of a bundle written in its order — words through the platform's
/// own value setter and the input and change events a page's code listens
/// to, a choice and a checkbox by a press on the choice itself, a dropdown
/// the page draws by opening it and pressing the option whose words are the
/// value, a date the page keeps from being typed by pressing the day of its
/// picker that carries the date's numbers — then every written field read
/// back by the read the agent was shown, and what the form still wants
/// (`zcFill`). A secret is never written (`type --value-stdin` is its road)
/// and a file is the person's turn.
pub(crate) const BROWSER_FILL_HELPERS: &str = r#"
const zcGroups = (text) => (String(text).match(/\d+/g) || []).map(Number);
const zcPad = (number, width) => String(number).padStart(width, "0");
const zcYear = (number) => /^\d{4}$/.test(String(number));
// A date or a time as HTML's own inputs take it, from however it was written.
const zcAsKind = (kind, asked) => {
  const groups = zcGroups(asked);
  if (kind === "date" && groups.length === 3 && zcYear(groups[0])) {
    return zcPad(groups[0], 4) + "-" + zcPad(groups[1], 2) + "-" + zcPad(groups[2], 2);
  }
  if (kind === "month" && groups.length === 2 && zcYear(groups[0])) {
    return zcPad(groups[0], 4) + "-" + zcPad(groups[1], 2);
  }
  if (kind === "time" && (groups.length === 2 || groups.length === 3)) {
    return groups.map((number) => zcPad(number, 2)).join(":");
  }
  return String(asked);
};
// Whether a field holds what was asked: the same words, or — for a value a
// page dresses as it is typed (a phone number's dashes, a date's dots) —
// the same digits, or the same numbers.
const zcDressed = (text) => /^[\d\s()+\-./:]*$/.test(text);
const zcSame = (asked, now) => {
  if (typeof asked === "boolean" || typeof now === "boolean") return asked === now;
  const a = zcFold(asked), b = zcFold(now);
  if (a === b) return true;
  if (!a || !zcDressed(a) || !zcDressed(b)) return false;
  const digits = (text) => text.replace(/\D/g, "");
  if (digits(a) && digits(a) === digits(b)) return true;
  const groups = zcGroups(a);
  return groups.length > 0 && groups.join(",") === zcGroups(b).join(",");
};
// The choice whose words (or value) are the asked value: the exact one,
// else the only one that is the same in numbers, holds the words, or is
// held by them.
const zcPick = (choices, asked) => {
  const want = zcFold(asked);
  if (!want) return null;
  const exact = choices.find((c) => zcFold(c.words) === want || (c.value && zcFold(c.value) === want));
  if (exact) return exact;
  const one = (list) => (list.length === 1 ? list[0] : null);
  return one(choices.filter((c) => zcSame(asked, c.words) || (c.value && zcSame(asked, c.value))))
    || one(choices.filter((c) => zcFold(c.words).includes(want)))
    || one(choices.filter((c) => zcFold(c.words) && want.includes(zcFold(c.words))));
};
const zcFlag = (asked) => {
  if (typeof asked === "boolean") return asked;
  const word = zcFold(asked);
  if (request.on.includes(word)) return true;
  if (request.off.includes(word)) return false;
  return null;
};
const zcEvent = (el, type, init) => {
  const view = el.ownerDocument.defaultView || window;
  const Made = type === "input" && typeof view.InputEvent === "function" ? view.InputEvent : view.Event;
  el.dispatchEvent(new Made(type, Object.assign({ bubbles: true, composed: true }, init || {})));
};
// A press as a person's pointer makes one — down, up, click — for the
// widgets that open on the down and the ones that act on the click.
const zcPress = (el) => {
  const view = el.ownerDocument.defaultView || window;
  const at = { bubbles: true, cancelable: true, composed: true, view, button: 0 };
  const Pointer = typeof view.PointerEvent === "function" ? view.PointerEvent : view.MouseEvent;
  el.dispatchEvent(new Pointer("pointerdown", at));
  el.dispatchEvent(new view.MouseEvent("mousedown", at));
  el.dispatchEvent(new Pointer("pointerup", at));
  el.dispatchEvent(new view.MouseEvent("mouseup", at));
  el.click();
};
// The platform's own value setter — past a page's code that wraps `value`
// on the element (a controlled input), in the element's own realm.
const zcValueSetter = (el) => {
  for (let proto = Object.getPrototypeOf(el); proto; proto = Object.getPrototypeOf(proto)) {
    const held = Object.getOwnPropertyDescriptor(proto, "value");
    if (held && held.set) return held.set;
  }
  return null;
};
const zcWriteText = (el, text, leave) => {
  el.focus({ preventScroll: true });
  if (["input", "textarea"].includes(zcFormTag(el))) {
    const setter = zcValueSetter(el);
    if (setter) setter.call(el, text); else el.value = text;
  } else {
    el.textContent = text;
  }
  zcEvent(el, "input", { data: text, inputType: "insertReplacementText" });
  zcEvent(el, "change");
  if (leave) el.blur();
};
// A field the page keeps from being typed in that takes a date: the day of
// the picker it opens whose own words (its name, title, data) carry the
// asked date's numbers in a date's order.
const zcHasDate = (words, want) => {
  const groups = zcGroups(words);
  const [year, month, day] = want;
  const orders = [[year, month, day], [month, day, year], [day, month, year]];
  return orders.some((order) => groups.some((_, at) =>
    order.every((number, step) => groups[at + step] === number)));
};
const zcDays = (el, want) => {
  const found = [];
  for (const day of el.ownerDocument.querySelectorAll(request.days.join(","))) {
    if (day === el || day.contains(el) || !zcDrawn(day) || zcOff(day)) continue;
    const words = request.dayWords.map((name) => String(day.getAttribute(name) || "")).join(" ");
    if (zcHasDate(words, want)) found.push(day);
  }
  return found.filter((day) => !found.some((other) => other !== day && day.contains(other)));
};
const zcHolds = (record, asked) => {
  if (record.kind === "checkbox") return zcFlag(asked) === record.value;
  if (record.kind === "combobox") {
    const open = record.el.getAttribute("aria-expanded") === "true";
    const now = zcFold(record.value);
    return !open && !!now && (zcSame(asked, record.value) || now.startsWith(zcFold(asked)));
  }
  if (record.choices.length && record.kind !== "combobox") {
    const pick = zcPick(record.choices, asked);
    return pick ? pick.selected : zcSame(asked, record.value);
  }
  return zcSame(zcAsKind(record.kind, asked), record.value);
};
// Write one value; "" when written, else why not.
const zcWrite = (record, asked) => {
  const el = record.el;
  if (record.kind === "checkbox") {
    if (zcFlag(asked) === null) {
      record.offered = request.on.slice(0, 1).concat(request.off.slice(0, 1));
      return "no_option";
    }
    zcPress(el);
    return "";
  }
  if (record.kind === "combobox") {
    const typed = zcFormTag(el) === "input" ? el : el.querySelector("input");
    let options = zcDrawnOptions(el, request);
    if (!options.length) {
      if (typed && zcFold(typed.value) !== zcFold(asked)) zcWriteText(typed, String(asked), false);
      else zcPress(el);
      options = zcDrawnOptions(el, request);
    }
    const pick = zcPick(options, asked);
    if (!pick) {
      record.offered = options.map((choice) => choice.words);
      return "no_option";
    }
    zcPress(pick.el);
    return "";
  }
  if (record.choices.length || record.kind === "radio" || record.kind.startsWith("select")) {
    const pick = zcPick(record.choices, asked);
    if (!pick) {
      record.offered = record.choices.map((choice) => choice.words);
      return "no_option";
    }
    if (record.kind.startsWith("select")) {
      pick.el.selected = true;
      zcEvent(el, "input");
      zcEvent(el, "change");
    } else {
      zcPress(pick.el);
    }
    return "";
  }
  if (record.readOnly) {
    const want = zcGroups(asked);
    if (want.length !== 3) return "read_only";
    let days = zcDays(el, want);
    if (!days.length) {
      zcPress(el);
      days = zcDays(el, want);
    }
    if (days.length !== 1) return "no_option";
    zcPress(days[0]);
    return "";
  }
  const text = zcAsKind(record.kind, asked);
  if (record.maxLength !== null && text.length > record.maxLength) return "too_long";
  zcWriteText(el, text, true);
  return "";
};
// The field a handle names: through its frames, the first one the page draws.
const zcTarget = (handle) => {
  const parts = handle.split(request.frameSeparator);
  let doc = document;
  for (const part of parts.slice(0, -1)) {
    let frame = null;
    try { frame = doc.querySelector(part); } catch (_) { return { code: "invalid_handle" }; }
    let inner = null;
    try { inner = frame && frame.contentDocument; } catch (_) {}
    if (!inner) return { code: "not_found" };
    doc = inner;
  }
  let found;
  try { found = [...doc.querySelectorAll(parts[parts.length - 1])]; } catch (_) { return { code: "invalid_handle" }; }
  const el = found.find(zcShown);
  if (!el) return { code: "not_found" };
  const record = zcRead(el, request);
  return record ? { record } : { code: "not_a_field" };
};
// One pass over a bundle: each entry written in its order, then each
// written field read back, then what the form still wants. With `expect` —
// the fingerprint of the form its agent read — a form that is no longer that
// one is said (`stale`) before anything is written.
const zcFill = (entries, expect) => {
  void expect;
  const passes = entries.map((entry) => {
    const out = { handle: entry.handle, status: "unread", kind: "", label: "", now: "", error: "", options: [] };
    const target = zcTarget(entry.handle);
    if (target.code) return { out, status: target.code };
    const record = target.record;
    out.kind = record.kind;
    out.label = zcWords(record.label, request.wordCap);
    if (record.masked) return { out, status: "secret" };
    if (record.kind === "file") return { out, status: "file" };
    if (record.disabled) return { out, status: "disabled" };
    if (zcHolds(record, entry.value)) return { out, entry, record, wrote: false };
    const refused = zcWrite(record, entry.value);
    if (refused) {
      out.options = (record.offered || []).slice(0, request.optionCap);
      return { out, status: refused };
    }
    return { out, entry, record, wrote: true };
  });
  const results = passes.map((pass) => {
    if (pass.status) {
      pass.out.status = pass.status;
      return pass.out;
    }
    const now = zcRead(pass.record.el, request) || pass.record;
    pass.out.now = now.masked ? "" : typeof now.value === "boolean" ? now.value
      : zcCut(String(now.value), request.valueCap);
    pass.out.error = zcWords(now.error, request.wordCap);
    pass.out.status = zcHolds(now, pass.entry.value) ? (pass.wrote ? "set" : "same") : "mismatch";
    return pass.out;
  });
  const after = zcFormFields(request);
  const left = after.fields.filter((field) => (field.required && zcEmpty(field)) || field.error);
  return { results, left };
};
"#;

/// One pass of a `fill`: the bundle the window hands it, through `zcFill`.
pub(crate) const BROWSER_FILL_BODY: &str = r#"
return zcEncode({ ok: true, value: zcFill(request.entries, request.expect || null) }, request.answerCap);
"#;

/// The form pair inside an `eval` (t-37883): an expression that names
/// [`BROWSER_EVAL_FORM_OBJECT`] (the `const` below is that name) gets
/// `fields()` — the read `fields` answers — and `fill(bundle, read)` — one
/// pass of `fill` over `{handle: value}` or a list of `{handle, value}`,
/// held to the form `read` (a `fields()` answer or its fingerprint) as the
/// door's fill is held to its last read — so one script can read a step,
/// fill it by the words it read, check what is left and press a step's
/// button, in one round trip. Synchronous like every eval: a field that
/// loads later is the next call's.
pub(crate) const BROWSER_EVAL_FORM: &str = r#"
const zerocode = Object.freeze({
  fields: () => {
    const read = zcFormFields(request);
    return { fields: read.fields, actions: read.actions, more: read.more, sealedFrames: read.sealed };
  },
  fill: (bundle, read) => zcFill(Array.isArray(bundle) ? bundle
    : Object.entries(bundle || {}).map(([handle, value]) => ({ handle, value })),
    typeof read === "string" ? read : (read && read.fingerprint) || null),
});
"#;

/// What a form script is handed: the core's tables and caps, and the marks
/// helpers' keys and regions it reads a field's words and section by.
pub(crate) fn form_request() -> serde_json::Value {
    use zerocode_core::agent_browser::{BROWSER_FIELD_HEADINGS, BROWSER_FIELD_REGIONS};
    serde_json::json!({
        "controls": BROWSER_FORM_CONTROLS,
        "notFields": BROWSER_FORM_NOT_FIELDS,
        "actions": BROWSER_FORM_ACTIONS,
        "scopes": BROWSER_FORM_SCOPES,
        "options": BROWSER_FORM_OPTIONS,
        "days": BROWSER_FORM_DAYS,
        "dayWords": BROWSER_FORM_DAY_WORDS,
        "frameSeparator": BROWSER_FORM_FRAME_SEPARATOR,
        "frameDepth": BROWSER_FORM_FRAME_DEPTH,
        "captionDepth": BROWSER_FORM_CAPTION_DEPTH,
        "on": BROWSER_FILL_ON,
        "off": BROWSER_FILL_OFF,
        "field": { "regions": BROWSER_FIELD_REGIONS, "headings": BROWSER_FIELD_HEADINGS },
        "fieldCap": BROWSER_FORM_FIELD_CAP,
        "actionCap": BROWSER_FORM_ACTION_CAP,
        "optionCap": BROWSER_FORM_OPTION_CAP,
        "wordCap": zerocode_core::screen_action::OBSERVED_CHAR_CAP,
        "valueCap": zerocode_core::type_value::asked().value_char_cap,
        "answerCap": BROWSER_CALLBACK_CAP,
    })
}

/// A form script: the observe and marks helpers it stands on (a digest, a
/// field's name, its region's heading), the form helpers, a body.
pub(crate) fn form_script(request: &serde_json::Value, body: &str) -> String {
    automation_script(
        request,
        &format!(
            "{BROWSER_OBSERVE_HELPERS}\n{BROWSER_MARK_HELPERS}\n{BROWSER_FORM_HELPERS}\n{body}"
        ),
    )
}

/// The form a pane's agent last read (`fields`) or was told of by its last
/// fill, by fingerprint — what the next `fill` holds the page to before it
/// writes. Kept per pane label, as the marks table is.
fn form_prints() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static PRINTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    PRINTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Remember the form a pane's agent now knows; an empty fingerprint (a page
/// that answered none) forgets it.
pub(crate) fn remember_form(label: &str, fingerprint: &str) {
    let _ = (form_prints(), label, fingerprint);
}

/// The form a pane's agent last knew, if it read one.
pub(crate) fn known_form(label: &str) -> Option<String> {
    let _ = label;
    None
}

/// A fill script: the form helpers, the writer, and a body that calls it.
pub(crate) fn fill_script(request: &serde_json::Value, body: &str) -> String {
    form_script(request, &format!("{BROWSER_FILL_HELPERS}\n{body}"))
}

/// An `eval`'s script when its expression names the form pair's object
/// ([`BROWSER_EVAL_FORM_OBJECT`]): the fill script's helpers and the object
/// before the expression; any other expression's script is left as it was,
/// so an eval that never reads a form carries none of it.
pub(crate) fn eval_script(expression: &str, body: &str) -> String {
    if expression.contains(&format!("{BROWSER_EVAL_FORM_OBJECT}.")) {
        fill_script(&form_request(), &format!("{BROWSER_EVAL_FORM}\n{body}"))
    } else {
        automation_script(&serde_json::json!({}), body)
    }
}

/// Read every field a pane's page draws, and the buttons beside them.
pub(crate) async fn automate_fields(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<FormRead, String> {
    let pane = browser_pane_of(app, state, label)?;
    let script = form_script(&form_request(), BROWSER_FIELDS_BODY);
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    let read: FormRead = serde_json::from_value(page_value(reply)?)
        .map_err(|_| "브라우저 판의 양식 읽기를 읽을 수 없습니다".to_string())?;
    remember_form(label, &read.fingerprint);
    Ok(read)
}

/// Fill a bundle into a pane's page, in passes ([`fill_passes`]), held to
/// the form the pane's agent last knew; what the fill leaves is the form it
/// knows next.
pub(crate) async fn automate_fill(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    entries: Vec<FillEntry>,
) -> Result<FillReport, String> {
    let pane = browser_pane_of(app, state, label)?;
    let report = fill_passes(
        entries,
        known_form(label),
        Duration::from_millis(BROWSER_FILL_PENDING_MS),
        BROWSER_WAIT_POLL,
        |asked, expect| {
            let pane = pane.clone();
            async move {
                let mut request = form_request();
                request["entries"] = serde_json::to_value(&asked).unwrap_or_default();
                request["expect"] = expect.into();
                let script = fill_script(&request, BROWSER_FILL_BODY);
                let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
                serde_json::from_value::<FillPass>(page_value(reply)?)
                    .map_err(|_| "브라우저 판의 채움 답을 읽을 수 없습니다".to_string())
            }
        },
    )
    .await?;
    if !report.stale {
        remember_form(label, &report.fingerprint);
    }
    Ok(report)
}

/// A fill's passes on the window's clock: the whole bundle first, held to
/// the form its agent read (`expect`, the first pass only — later passes
/// meet fields the fill's own values brought), then — every `poll`, until
/// `pending` has passed since the start — only the fields another pass may
/// still find (the ledger's word), so a field an earlier value brings (a
/// time list a date loads, a box a choice turns on, a dropdown that opens on
/// a press) is written once it is there. A pass that fails ends the fill
/// with its refusal; a stale form ends it with nothing written.
pub(crate) async fn fill_passes<P, F>(
    entries: Vec<FillEntry>,
    expect: Option<String>,
    pending: Duration,
    poll: Duration,
    mut pass: P,
) -> Result<FillReport, String>
where
    P: FnMut(Vec<FillEntry>, Option<String>) -> F,
    F: std::future::Future<Output = Result<FillPass, String>>,
{
    let _ = expect;
    let until = tokio::time::Instant::now() + pending;
    let mut ledger = FillLedger::new(entries);
    loop {
        let asked = ledger.next();
        if asked.is_empty() {
            break;
        }
        let answered = pass(asked.clone(), None).await?;
        ledger.record(&asked, answered);
        if ledger.next().is_empty() || tokio::time::Instant::now() >= until {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    Ok(ledger.report())
}
