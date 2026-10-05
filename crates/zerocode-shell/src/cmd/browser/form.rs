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
use zerocode_core::agent_browser::BROWSER_EVAL_FORM_OBJECT;
use zerocode_core::browser_form::{
    BROWSER_FILL_OFF, BROWSER_FILL_ON, BROWSER_FILL_PENDING_MS, BROWSER_FORM_ACTION_CAP,
    BROWSER_FORM_ACTIONS, BROWSER_FORM_CAPTION_DEPTH, BROWSER_FORM_CONTROLS,
    BROWSER_FORM_DAY_WORDS, BROWSER_FORM_DAYS, BROWSER_FORM_FIELD_CAP, BROWSER_FORM_FRAME_DEPTH,
    BROWSER_FORM_FRAME_SEPARATOR, BROWSER_FORM_LIST_ITEMS, BROWSER_FORM_LISTS,
    BROWSER_FORM_MONTH_DAYS, BROWSER_FORM_MONTH_PAGES, BROWSER_FORM_NOT_FIELDS,
    BROWSER_FORM_OPTION_CAP, BROWSER_FORM_OPTIONS, BROWSER_FORM_PRESSABLES, BROWSER_FORM_SCAN_CAP,
    BROWSER_FORM_SCOPES, FillEntry, FillLedger, FillPass, FillReport, FormAction, FormRead,
};

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
// The words a field has to itself under a header's cell or a term's
// definition: those before it in the outermost box inside the cell that
// holds no other field. A field straight in the cell has none.
const zcOwnWords = (el, holder, request) => {
  const controls = request.controls.join(",");
  let depth = 0;
  for (let up = el.parentElement, level = 1; up && up !== holder; up = up.parentElement, level += 1) {
    if ([...up.querySelectorAll(controls)].some((other) => other !== el && zcFieldKind(other, request))) break;
    depth = level;
  }
  return depth ? zcWordsBefore(el, { ...request, captionDepth: depth }) : "";
};
// The header over a table cell's column — HTML's own table meaning: the
// head cell, of the table's `thead` (its last row) or of a first row that is
// nothing but headers, that covers the column the cell stands in (a head cell
// spanning several columns covers each of them).
const zcColumnHead = (cell) => {
  const row = cell.parentElement;
  const table = cell.closest("table");
  if (!row || !table || !row.cells) return "";
  const first = table.rows[0];
  const heads = table.tHead && table.tHead.rows.length ? table.tHead.rows[table.tHead.rows.length - 1]
    : first && first !== row && [...first.cells].every((one) => one.tagName === "TH") ? first : null;
  if (!heads || heads === row) return "";
  let column = 0;
  for (const before of row.cells) {
    if (before === cell) break;
    column += before.colSpan || 1;
  }
  let at = 0;
  for (const head of heads.cells) {
    at += head.colSpan || 1;
    if (column < at) return zcLabelWords(head);
  }
  return "";
};
// A field's caption when nothing names it: the words it has to itself in a
// box of its own, else its table row's header, else the header over its
// column, the term its definition follows (shared by the fields with no words
// of their own), the words just before it.
const zcCaption = (el, request) => {
  const holder = el.closest("td, [role=cell], [role=gridcell], dd");
  const own = holder ? zcOwnWords(el, holder, request) : "";
  if (own.trim()) return own;
  const cell = el.closest("td, [role=cell], [role=gridcell]");
  const head = cell && cell.parentElement && cell.parentElement.querySelector("th, [role=rowheader]");
  if (head && zcLabelWords(head).trim()) return zcLabelWords(head);
  const column = cell && cell.tagName === "TD" ? zcColumnHead(cell) : "";
  if (column.trim()) return column;
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
// The list a focusable box keeps beside it — a dropdown drawn with no ARIA:
// a sibling list in the box's own parent.
const zcListBeside = (el, request) => el.parentElement
  ? [...el.parentElement.children].find((child) => child !== el && child.matches(request.lists.join(","))) || null
  : null;
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
  if (el.isContentEditable) return "text";
  return el.hasAttribute("tabindex") && zcListBeside(el, request) ? "dropdown" : null;
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
  } else if (kind === "dropdown") {
    const list = zcListBeside(el, request);
    record.choices = [...list.children].filter((item) => item.matches(request.listItems.join(",")))
      .map((item) => ({ el: item, words: zcChoiceWords(item, cap), value: String(item.getAttribute("data-value") || ""),
        selected: false }));
    record.value = zcWords(el.innerText || el.textContent || "", request.valueCap);
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
// The words between two fields side by side, and after the last of them in
// their box — where a value's parts carry their units (시 · 분, 년 · 월 · 일).
const zcWordsBetween = (from, to) => {
  const range = from.ownerDocument.createRange();
  range.setStartAfter(from);
  if (to) range.setEndBefore(to); else range.setEndAfter(from.parentElement.lastChild);
  return zcFold(range.toString());
};
// Whether a field continues the run before it as one more part: no caption
// of its own, the head's caption, or — for parts of one kind whose last is
// followed by words too — only the unit between it and the part before.
const zcContinues = (head, prev, next) => {
  const caption = zcFold(next.caption);
  if (!caption || caption === zcFold(head.caption)) return true;
  return next.kind === head.kind && caption === zcWordsBetween(prev.el, next.el)
    && zcWordsBetween(next.el, null) !== "";
};
// Fields side by side in one box with one caption — a phone number in three
// boxes, a date in three selects, an hour and a minute — are that caption's
// parts, numbered.
const zcNumberRuns = (records) => {
  let at = 0;
  while (at < records.length) {
    const head = records[at];
    let end = at + 1;
    while (end < records.length && records[end].el.parentElement === head.el.parentElement
      && records[end].kind !== "radio" && zcContinues(head, records[end - 1], records[end])
      && !zcNameOf(records[end].el)) end += 1;
    if (end - at > 1 && zcFold(head.label)) {
      // The caption read before the first part is renamed, for every part.
      const caption = head.label.trim();
      for (let part = at; part < end; part += 1) {
        records[part].label = caption + " (" + (part - at + 1) + "/" + (end - at) + ")";
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
// The words the page gives about a field that is off or cannot be written,
// besides its label: what its aria-describedby names, else its title, else
// the words of the nearest box around it that holds no other field, without
// the field's own labels — so an agent learns what switches it on.
const zcHintOf = (record, request) => {
  const el = record.el;
  const doc = el.ownerDocument;
  const named = String(el.getAttribute("aria-describedby") || "").split(/\s+/).filter(Boolean)
    .map((id) => doc.getElementById(id)).filter(Boolean).map(zcLabelWords).join(" ");
  if (named.trim()) return named;
  const title = String(el.getAttribute("title") || "");
  if (title.trim() && zcFold(title) !== zcFold(record.label)) return title;
  const own = [...(el.labels || [])];
  for (const id of String(el.getAttribute("aria-labelledby") || "").split(/\s+/).filter(Boolean)) {
    const node = doc.getElementById(id);
    if (node) own.push(node);
  }
  const skip = "select, textarea, button, option, script, style, template, [role=option], [role=listbox]";
  const controls = request.controls.join(",");
  let box = el.parentElement;
  for (let level = 0; box && level < request.captionDepth; level += 1, box = box.parentElement) {
    if (box.matches("form, body, main")) break;
    if ([...box.querySelectorAll(controls)].some((other) => other !== el && !el.contains(other)
      && zcFieldKind(other, request))) break;
    const walker = doc.createTreeWalker(box, NodeFilter.SHOW_TEXT);
    const words = [];
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const holder = node.parentElement;
      if (!holder || holder.closest(skip) || own.some((label) => label.contains(holder))) continue;
      words.push(node.nodeValue);
    }
    const said = words.join(" ").replace(/\s+/g, " ").trim();
    if (said) return said;
  }
  return "";
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
  if (record.disabled || record.readOnly) {
    const hint = zcWords(zcHintOf(record, request), cap);
    if (hint) out.hint = hint;
  }
  return out;
};
const zcEmpty = (out) => out.value === "" || out.value === false;
// A net under every rule: what a person can press or focus inside the boxes
// the fields stand in that no field, choice or button stands for — an
// element that takes focus or a click of its own, one of ARIA's pressable
// roles, a box that shows a pointer its parent does not — said once,
// outermost, with its words and the caption beside it. A link, a label and
// HTML's own controls are named elsewhere or are no step of a form.
const zcUnknowns = (records, pressed, scopes, docs, request) => {
  const known = [...pressed];
  for (const record of records) {
    known.push(record.el, ...record.choices.map((choice) => choice.el).filter(Boolean));
  }
  const said = [];
  const unknowns = [];
  const pointer = (el) => {
    const view = el.ownerDocument.defaultView || window;
    return view.getComputedStyle(el).cursor === "pointer";
  };
  // A page whose every control is drawn by hand has no field to find a box
  // by: its whole body is looked through.
  const roots = scopes.size ? [...scopes] : docs.open.map(({ doc }) => doc.body).filter(Boolean);
  for (const { doc, prefix } of docs.open) {
    for (const scope of roots) {
      if (scope.ownerDocument !== doc) continue;
      let scanned = 0;
      for (const el of scope.querySelectorAll("*")) {
        if (unknowns.length >= request.actionCap || (scanned += 1) > request.scanCap) break;
        if (said.some((one) => one.contains(el))) continue;
        const own = el.matches(request.pressables.join(","))
          || (pointer(el) && !(el.parentElement && pointer(el.parentElement)));
        if (!own || el.matches("input, select, textarea, button, option, label, a[href]")) continue;
        if (el.closest("label, a[href]") || !zcDrawn(el)) continue;
        if (known.some((one) => one === el || one.contains(el) || el.contains(one))) continue;
        const label = zcWords(el.getAttribute("aria-label") || el.innerText || el.textContent || "",
          request.wordCap);
        const caption = zcWords(zcCaption(el, request), request.wordCap);
        if (!label && !caption) continue;
        said.push(el);
        unknowns.push({ handle: prefix + zcHandleOf(el), label, caption });
      }
    }
  }
  return unknowns;
};
// The boxes the page lets scroll that have more to read below what is shown:
// drawn, with text, holding no field or button, `overflow` auto or scroll, a
// scroll height past the box, and not yet read to its end — outermost, with
// its first words. Attributes, styles and sizes only, so it is the same for
// any widget; a box read to its end is said no more.
const zcScrollBoxes = (scopes, docs, request) => {
  const roots = scopes.size ? [...scopes] : docs.open.map(({ doc }) => doc.body).filter(Boolean);
  const holds = request.controls.concat(request.actions).join(",");
  const said = [];
  const boxes = [];
  for (const { doc, prefix } of docs.open) {
    const view = doc.defaultView || window;
    for (const scope of roots) {
      if (scope.ownerDocument !== doc) continue;
      let scanned = 0;
      for (const el of scope.querySelectorAll("*")) {
        if (boxes.length >= request.actionCap || (scanned += 1) > request.scanCap) break;
        if (el.scrollHeight <= el.clientHeight + 1 || said.some((one) => one.contains(el))) continue;
        if (!/^(auto|scroll)$/.test(view.getComputedStyle(el).overflowY)) continue;
        if (el.scrollTop + el.clientHeight >= el.scrollHeight - 1) continue;
        if (!zcDrawn(el) || el.matches("textarea, select") || el.querySelector(holds)) continue;
        const label = zcWords(el.innerText || el.textContent || "", request.wordCap);
        if (!label) continue;
        said.push(el);
        boxes.push({ handle: prefix + zcHandleOf(el), label });
      }
    }
  }
  return boxes;
};
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
  const pressed = [];
  for (const { doc, prefix } of docs.open) {
    for (const el of doc.querySelectorAll(request.actions.join(","))) {
      if (actions.length >= request.actionCap) break;
      if (!zcDrawn(el) || zcFormRole(el) === "combobox") continue;
      if (![...scopes].some((scope) => scope.contains(el))) continue;
      const label = zcWords(zcMarkName(el), request.wordCap);
      const handle = prefix + zcHandleOf(el);
      if (!label || named.has(handle)) continue;
      named.add(handle);
      pressed.push(el);
      actions.push({ handle, label, disabled: zcOff(el) });
    }
  }
  const unknowns = zcUnknowns(records, pressed, scopes, docs, request);
  const scrollBoxes = zcScrollBoxes(scopes, docs, request);
  // The form's fingerprint: every field's handle, kind and words, in order —
  // never a value, which a fill is there to change.
  const print = zcDigest(records.map((record) => [record.handle, record.kind, zcFold(record.label)]
    .join("\u001f")).join("\u001e"));
  return { records, fields: records.map((record) => zcFieldOut(record, request)), actions,
    more, sealed: docs.sealed, print, unknowns, scrollBoxes };
};
"##;

/// The `fields` read: every field and its buttons, in one synchronous pass
/// that writes nothing.
pub(crate) const BROWSER_FIELDS_BODY: &str = r#"
const read = zcFormFields(request);
return zcEncode({ ok: true, value: { fields: read.fields, actions: read.actions, more: read.more,
  sealedFrames: read.sealed, fingerprint: read.print, unknowns: read.unknowns, scrollBoxes: read.scrollBoxes } }, request.answerCap);
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
  // A number asked of choices that dress their numbers in words ("4" of
  // "4일", "04" of "4 Dec") is the one choice holding just those numbers.
  const numbers = zcDressed(want) ? zcGroups(want).join(",") : "";
  return one(choices.filter((c) => zcSame(asked, c.words) || (c.value && zcSame(asked, c.value))))
    || (numbers && one(choices.filter((c) => zcGroups(c.words).join(",") === numbers)))
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
// ---- a date, written the way the field takes it ----
// The asked date as [year, month, day], when it is one (year first).
const zcDateOf = (asked) => {
  const groups = zcGroups(asked);
  return groups.length === 3 && zcYear(groups[0]) ? groups : null;
};
// The form a field shows its dates in, from its own placeholder or title:
// a year, a month and a day token (YYYY / YY, MM / M, DD / D, either case)
// with whatever stands between them — "YYYY.MM.DD", "mm/dd/yyyy".
const zcDateTokens = /y{2,4}|m{1,2}|d{1,2}/gi;
const zcDateFormat = (el) => {
  for (const hint of [el.getAttribute("placeholder"), el.getAttribute("aria-placeholder"), el.getAttribute("title")]) {
    const tokens = String(hint || "").match(zcDateTokens) || [];
    const kinds = new Set(tokens.map((token) => token[0].toLowerCase()));
    if (tokens.length === 3 && kinds.size === 3) return String(hint);
  }
  return null;
};
const zcInFormat = (format, [year, month, day]) => format.replace(zcDateTokens, (token) => {
  const kind = token[0].toLowerCase();
  if (kind === "y") return token.length === 2 ? zcPad(year % 100, 2) : zcPad(year, 4);
  const number = kind === "m" ? month : day;
  return token.length === 2 ? zcPad(number, 2) : String(number);
});
// ---- a date, picked on the page's own calendar ----
// The months' names the platform knows for the page's language and English,
// longest first — no table of names of our own.
const zcMonthNames = () => {
  const names = [];
  for (const lang of [document.documentElement.lang, navigator.language, "en"].filter(Boolean)) {
    for (const month of ["long", "short"]) {
      try {
        const said = new Intl.DateTimeFormat(lang, { month, timeZone: "UTC" });
        for (let at = 0; at < 12; at += 1) names.push([zcFold(said.format(new Date(Date.UTC(2000, at, 1)))), at + 1]);
      } catch (_) {}
    }
  }
  return names.filter(([name]) => name && !/^\d+$/.test(name)).sort((a, b) => b[0].length - a[0].length);
};
// The month a calendar's heading shows: a four-digit year, and beside it the
// month's number or its name.
const zcMonthIn = (text) => {
  const words = zcFold(text);
  const year = (words.match(/\d{4}/) || [])[0];
  if (!year) return null;
  const rest = " " + words.replace(year, " ") + " ";
  const number = rest.match(/\D(\d{1,2})\D/);
  if (number && Number(number[1]) >= 1 && Number(number[1]) <= 12) return [Number(year), Number(number[1])];
  const named = zcMonthNames().find(([name]) => rest.includes(name));
  return named ? [Number(year), named[1]] : null;
};
// The drawn cells whose own words are a day's number, innermost first.
const zcDayCells = (doc) => [...doc.querySelectorAll(request.days.join(","))]
  .filter((cell) => /^\d{1,2}$/.test(zcFold(cell.innerText || cell.textContent)) && zcDrawn(cell)
    && !cell.querySelector(request.days.join(",")));
// The calendars a page shows: the closest box around a month of day cells,
// each with the month its heading shows — the box's own name, else the
// first words above its days that read as a year and a month.
const zcCalendars = (doc) => {
  const cells = zcDayCells(doc);
  const grids = [];
  for (const cell of cells) {
    if (grids.some((grid) => grid.box.contains(cell))) continue;
    let box = cell.parentElement;
    while (box && box !== doc.body && cells.filter((other) => box.contains(other)).length < request.monthDays) {
      box = box.parentElement;
    }
    if (!box || box === doc.body) continue;
    grids.push({ box, cells: cells.filter((other) => box.contains(other)) });
  }
  for (const grid of grids) {
    grid.month = zcMonthIn(grid.box.getAttribute("aria-label") || "");
    grid.root = grid.box;
    let near = grid.box;
    for (let level = 0; !grid.month && near && level < request.captionDepth; level += 1) {
      const walker = doc.createTreeWalker(near, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node && !grid.month; node = walker.nextNode()) {
        if (grid.cells.some((cell) => cell.contains(node))) continue;
        const holder = node.parentElement;
        const around = holder && String(holder.textContent || "").length <= request.wordCap ? holder.textContent : "";
        const said = zcMonthIn(node.nodeValue) || zcMonthIn(around);
        if (said) {
          grid.month = said;
          grid.heading = holder;
          grid.root = near;
        }
      }
      near = near.parentElement;
    }
    // A calendar with no heading to read keeps its controls beside its days.
    if (!grid.month && grid.box.parentElement) grid.root = grid.box.parentElement;
  }
  return grids;
};
// The day of a calendar showing that month: its run of days runs from its
// first "1" to the day before the next — the days of the months around it
// it also draws are outside the run — and an off day is no choice.
const zcDayIn = (grid, day) => {
  const numbers = grid.cells.map((cell) => Number(zcFold(cell.innerText || cell.textContent)));
  const first = numbers.indexOf(1);
  if (first < 0) return null;
  let end = numbers.indexOf(1, first + 1);
  if (end < 0) end = numbers.length;
  const at = numbers.slice(first, end).indexOf(day);
  if (at < 0) return null;
  const cell = grid.cells[first + at];
  return zcOff(cell) || cell.disabled ? null : cell;
};
// What a control says it is beyond the words it draws: its ARIA name or
// title, an input button's value, and the names of the picture it is drawn
// with — an image's alt, an svg's title, a part's ARIA name.
const zcSaysItIs = (el) => {
  const parts = [...el.querySelectorAll("img[alt], svg title, [aria-label]")]
    .map((part) => part.getAttribute("alt") || part.getAttribute("aria-label") || part.textContent);
  return zcFold([zcNameOf(el), zcFormTag(el) === "input" ? el.value : "", ...parts].join(" "));
};
// A press the window leaves to a person — a payment, a transfer, a deletion,
// a send or a submit (`request.holds`, the one table of them) — is never made
// on a guess. A pager is learnt by pressing it, so a control that says it is
// one of those is no pager candidate.
const zcHeld = (el) => {
  const says = zcSaysItIs(el);
  return request.holds.some((word) => says.includes(word));
};
// A control that says in its own markup it submits its form — `type="submit"`
// on a button or an input, an input of type image — is no pager, whatever it is
// named: pressed on a guess it would send the form. A button with no type at
// all is still a candidate (calendar libraries draw their pagers so).
const zcSubmits = (el) => {
  const type = String(el.getAttribute("type") || "").toLowerCase();
  return type === "submit" || (zcFormTag(el) === "input" && type === "image");
};
// The controls that page a calendar: the drawn buttons around its heading
// that are no day, say no words (an arrow, an icon), submit nothing and name
// no press that cannot be taken back — and which way each pages is learnt by
// pressing it and reading the heading again.
const zcPagers = (grid) => [...grid.root.querySelectorAll(request.actions.join(","))]
  .filter((button) => zcDrawn(button) && !zcOff(button) && !grid.cells.includes(button)
    && !grid.cells.some((cell) => button.contains(cell) || cell.contains(button))
    && !/[\p{L}\p{N}]/u.test(String(button.innerText || button.textContent || ""))
    && !zcSubmits(button) && !zcHeld(button));
const zcMonthNumber = ([year, month]) => year * 12 + month;
// The calendars nearest a field — those sharing the deepest box with it —
// and how deep that box is: a calendar another field left open is not this
// field's.
const zcDepth = (node) => {
  let depth = 0;
  for (; node; node = node.parentElement) depth += 1;
  return depth;
};
const zcNearest = (grids, el) => {
  const around = new Set();
  for (let node = el; node; node = node.parentElement) around.add(node);
  const depths = grids.map((grid) => {
    let shared = grid.box;
    while (shared && !around.has(shared)) shared = shared.parentElement;
    return zcDepth(shared);
  });
  const best = Math.max(0, ...depths);
  return { grids: grids.filter((_, at) => depths[at] === best), depth: best };
};
// Page a calendar to the asked month and press its day; "" when pressed,
// else what the page shows of the calendar for the agent to finish by hand.
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
// What a calendar shows the agent when the fill could not pick its day: the
// heading's words and month, the controls that may page it, the box of its
// days — enough to finish it in a few presses.
const zcWidget = (grids) => {
  const grid = grids.find((each) => each.month) || grids[0];
  if (!grid) return null;
  return { heading: zcWords(grid.heading ? grid.heading.textContent : "", request.wordCap),
    month: grid.month ? grid.month[0] + "-" + zcPad(grid.month[1], 2) : "",
    pagers: zcPagers(grid).map(zcHandleOf), days: zcHandleOf(grid.box) };
};
// Pick a date on the page's own calendar: a day that says its whole date
// (its name, title, data) first; else the calendar showing a month, paged
// with its own controls — each learnt by pressing it and reading the
// heading again — to the asked month, and the day of that month pressed.
// Null when a day was pressed, else what the calendar shows.
const zcPickDate = (record, date) => {
  const el = record.el;
  const doc = el.ownerDocument;
  const near = () => zcNearest(zcCalendars(doc), el);
  // The field's own calendar shares at least the field's own box with it;
  // anything further is opened by pressing the field.
  let found = near();
  if ((!found.grids.length || found.depth < zcDepth(el.parentElement)) && !zcDays(el, date).length) {
    zcPress(el);
    found = near();
  }
  let grids = found.grids;
  const whole = zcDays(el, date);
  if (whole.length === 1) {
    zcPress(whole[0]);
    return null;
  }
  const target = zcMonthNumber(date);
  // A calendar draws itself anew on each page, so a pager is known by its
  // handle, not by the element pressed.
  const tried = new Set();
  let toward = null;
  for (let press = 0; press <= request.monthPages; press += 1) {
    const shown = grids.find((grid) => grid.month && zcMonthNumber(grid.month) === target);
    if (shown) {
      const cell = zcDayIn(shown, date[2]);
      if (!cell) return zcWidget(grids);
      zcPress(cell);
      return null;
    }
    const known = grids.find((grid) => grid.month);
    if (!known) return zcWidget(grids);
    const now = zcMonthNumber(known.month);
    const pagers = zcPagers(known);
    const pager = (toward && pagers.find((each) => zcHandleOf(each) === toward))
      || pagers.find((each) => !tried.has(zcHandleOf(each)));
    if (!pager) return zcWidget(grids);
    const handle = zcHandleOf(pager);
    tried.add(handle);
    zcPress(pager);
    grids = near().grids;
    if (!grids.length) {
      // That control closed the calendar: open it again, that one tried.
      zcPress(el);
      grids = near().grids;
      continue;
    }
    const after = grids.find((grid) => grid.month);
    if (!after) return zcWidget(grids);
    const step = zcMonthNumber(after.month) - now;
    toward = step !== 0 && Math.sign(step) === Math.sign(target - now) ? handle : null;
  }
  return zcWidget(grids);
};
// Whether what a field shows is the asked date: its numbers in a date's
// order, or the year, the day and the month's name.
const zcShowsDate = (shown, date) => {
  if (zcHasDate(String(shown), date)) return true;
  const groups = zcGroups(shown);
  const words = zcFold(shown);
  return groups.includes(date[0]) && groups.includes(date[2])
    && zcMonthNames().some(([name, month]) => month === date[1] && words.includes(name));
};
const zcHolds = (record, asked) => {
  if (record.kind === "checkbox") return zcFlag(asked) === record.value;
  const date = record.kind !== "select" && zcDateOf(asked);
  if (date && record.value !== "" && zcShowsDate(record.value, date)) return true;
  if (record.kind === "dropdown") {
    const pick = zcPick(record.choices, asked);
    const now = zcFold(record.value);
    return !!pick && (now === zcFold(pick.words) || now.startsWith(zcFold(pick.words)));
  }
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
  if (record.kind === "dropdown") {
    const pick = zcPick(record.choices, asked);
    if (!pick) {
      record.offered = record.choices.map((choice) => choice.words);
      return "no_option";
    }
    if (!zcDrawn(pick.el)) zcPress(el);
    zcPress(pick.el);
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
  const date = zcDateOf(asked);
  if (record.readOnly) {
    if (!date) return "read_only";
    const shown = zcPickDate(record, date);
    if (!shown) return "";
    record.widget = shown;
    return "no_option";
  }
  const format = date && zcDateFormat(el);
  const text = format ? zcInFormat(format, date) : zcAsKind(record.kind, asked);
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
// One pass over a bundle, in two halves the window runs apart so the page can
// settle between them: the write, then the read-back.
//
// The write: each entry written in its order — with `expect`, the fingerprint
// of the form its agent read, a form that is no longer that one is said
// (`stale`) before anything is written. It answers what each entry held (the
// refusals, the words about the field, what was written), whether anything
// was written, and the document and the page's own clock at the last write:
// what a settle counts stillness from. A page that updates after its input
// handler has returned (a microtask, a debounced check) has not yet shown what
// the write did when this half ends. `watch`, when given, is the settle's: it
// stands before the first write, so the pass's own changes are the first it sees.
const zcFillWrite = (entries, expect, watch) => {
  if (expect) {
    const now = zcFormFields(request).print;
    if (now !== expect) return { stale: true, fingerprint: now, wrote: false, held: [] };
  }
  if (watch) zcSettleWatch(watch);
  const held = entries.map((entry) => {
    const out = { handle: entry.handle, status: "unread", kind: "", label: "", now: "", error: "", options: [] };
    // The value goes back to the window only with an entry the read-back will
    // check it against — never with a secret or any entry that was refused.
    const base = { handle: entry.handle, out };
    const target = zcTarget(entry.handle);
    if (target.code) return { ...base, status: target.code, wrote: false };
    const record = target.record;
    out.kind = record.kind;
    out.label = zcWords(record.label, request.wordCap);
    if (record.masked) return { ...base, status: "secret", wrote: false };
    if (record.kind === "file") return { ...base, status: "file", wrote: false };
    if (record.disabled) {
      const hint = zcWords(zcHintOf(record, request), request.wordCap);
      if (hint) out.hint = hint;
      return { ...base, status: "disabled", wrote: false };
    }
    if (zcHolds(record, entry.value)) return { ...base, value: entry.value, status: null, wrote: false };
    const refused = zcWrite(record, entry.value);
    if (refused) {
      out.options = (record.offered || []).slice(0, request.optionCap);
      if (record.widget) out.widget = record.widget;
      return { ...base, status: refused, wrote: false };
    }
    return { ...base, value: entry.value, status: null, wrote: true };
  });
  return { stale: false, fingerprint: "", wrote: held.some((one) => one.wrote), epoch: zcEpoch(),
    at: performance.now(), held };
};
// The read-back, once the page has settled: each written field read again
// where its handle finds it now (a page may have drawn it anew), then what the
// form still wants and the buttons it has. A page that is no longer the
// document the write was made in answers each written field `replaced` — what
// it held is gone — and the form of the document that stands now.
const zcFillRead = (held, epoch) => {
  const replaced = zcEpoch() !== epoch;
  const results = held.map((one) => {
    const out = one.out;
    if (one.status) {
      out.status = one.status;
      return out;
    }
    if (replaced) {
      out.status = "replaced";
      return out;
    }
    const target = zcTarget(one.handle);
    if (target.code) {
      out.status = target.code;
      return out;
    }
    const now = target.record;
    out.now = now.masked ? "" : typeof now.value === "boolean" ? now.value
      : zcCut(String(now.value), request.valueCap);
    out.error = zcWords(now.error, request.wordCap);
    out.status = zcHolds(now, one.value) ? (one.wrote ? "set" : "same") : "mismatch";
    return out;
  });
  const after = zcFormFields(request);
  const left = after.fields.filter((field) => (field.required && zcEmpty(field)) || field.error);
  return { results, left, stale: false, fingerprint: after.print, actions: after.actions };
};
// One whole pass in one synchronous run, as an eval's `zerocode.fill` has it:
// a field the page loads or turns on later is the next call's.
const zcFill = (entries, expect) => {
  const written = zcFillWrite(entries, expect, null);
  return written.stale
    ? { results: [], left: [], stale: true, fingerprint: written.fingerprint }
    : zcFillRead(written.held, written.epoch);
};
"#;

/// One half of a pass of a `fill` (`request.phase`): the write of the bundle
/// the window hands it (`zcFillWrite`), or — after the page has settled — the
/// read-back of what the write held (`zcFillRead`).
pub(crate) const BROWSER_FILL_BODY: &str = r#"
const value = request.phase === "read"
  ? zcFillRead(request.held, request.epoch)
  : zcFillWrite(request.entries, request.expect || null, request.watch);
return zcEncode({ ok: true, value }, request.answerCap);
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
    return { fields: read.fields, actions: read.actions, more: read.more, sealedFrames: read.sealed,
      fingerprint: read.print, unknowns: read.unknowns, scrollBoxes: read.scrollBoxes };
  },
  fill: (bundle, read) => zcFill(Array.isArray(bundle) ? bundle
    : Object.entries(bundle || {}).map(([handle, value]) => ({ handle, value })),
    typeof read === "string" ? read : (read && read.fingerprint) || null),
});
"#;

/// What a form script is handed: the core's tables and caps, the words of
/// the presses that cannot be taken back (`holds`: the window's one table of
/// them, which a script holds a control back by), and the marks helpers' keys
/// and regions it reads a field's words and section by.
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
        "lists": BROWSER_FORM_LISTS,
        "pressables": BROWSER_FORM_PRESSABLES,
        "holds": zerocode_core::guarded::HELD_ROWS.concat(),
        "scanCap": BROWSER_FORM_SCAN_CAP,
        "watch": settle_watch(),
        "listItems": BROWSER_FORM_LIST_ITEMS,
        "monthDays": BROWSER_FORM_MONTH_DAYS,
        "monthPages": BROWSER_FORM_MONTH_PAGES,
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
    static PRINTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::OnceLock::new();
    PRINTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Remember the form a pane's agent now knows; an empty fingerprint (a page
/// that answered none) forgets it.
pub(crate) fn remember_form(label: &str, fingerprint: &str) {
    let mut held = form_prints()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if fingerprint.is_empty() {
        held.remove(label);
    } else {
        held.insert(label.to_string(), fingerprint.to_string());
    }
}

/// The form a pane's agent last knew, if it read one.
pub(crate) fn known_form(label: &str) -> Option<String> {
    form_prints()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(label)
        .cloned()
}

/// The buttons of the form a pane's agent last knew — what a fill's answer
/// says went away. Kept per pane label beside the fingerprint.
fn form_buttons() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<FormAction>>> {
    static BUTTONS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<FormAction>>>,
    > = std::sync::OnceLock::new();
    BUTTONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Remember the buttons of the form a pane's agent now knows.
pub(crate) fn remember_buttons(label: &str, buttons: &[FormAction]) {
    form_buttons()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(label.to_string(), buttons.to_vec());
}

/// The buttons the form a pane's agent last knew had.
pub(crate) fn known_buttons(label: &str) -> Vec<FormAction> {
    form_buttons()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(label)
        .cloned()
        .unwrap_or_default()
}

/// What the write half of a fill pass says: whether the form was the one read
/// (`stale`), whether anything was written, the document and the page's own
/// clock when the last value was written — what the settle counts stillness
/// from — and the outcomes the read half finishes.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct FillWritten {
    pub(crate) stale: bool,
    pub(crate) fingerprint: String,
    pub(crate) wrote: bool,
    pub(crate) epoch: String,
    pub(crate) at: Option<f64>,
    pub(crate) held: serde_json::Value,
}

/// One pass of a fill: the write, then — when something was written — the
/// page's settle (the one a press by number waits with, in the document the
/// write was made in, counting from the page's clock at the last write), then
/// the read-back of what the write held. A page that did not stand still in
/// the settle's time, or went to another document, is said to be still
/// changing (`moving`); a pass that wrote nothing waited for nothing and says
/// nothing of it. A form that was not the one read is answered at once.
pub(crate) async fn settled_pass<W, WF, S, SF, R, RF>(
    write: W,
    settle: S,
    read: R,
) -> Result<FillPass, String>
where
    W: FnOnce() -> WF,
    WF: std::future::Future<Output = Result<FillWritten, String>>,
    S: FnOnce(String, Option<f64>) -> SF,
    SF: std::future::Future<Output = SettleReport>,
    R: FnOnce(serde_json::Value, String) -> RF,
    RF: std::future::Future<Output = Result<FillPass, String>>,
{
    let written = write().await?;
    if written.stale {
        return Ok(FillPass {
            stale: true,
            fingerprint: written.fingerprint,
            ..FillPass::default()
        });
    }
    let settled = if written.wrote {
        Some(settle(written.epoch.clone(), written.at).await)
    } else {
        None
    };
    let mut pass = read(written.held, written.epoch).await?;
    pass.moving = settled.map(|settled| settled.state != Settle::Ready);
    Ok(pass)
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
    remember_buttons(label, &read.actions);
    Ok(read)
}

/// Fill a bundle into a pane's page, in passes ([`fill_passes`]), held to
/// the form the pane's agent last knew; what the fill leaves is the form it
/// knows next, and the report says whether that is the form it read and what
/// the buttons are now.
pub(crate) async fn automate_fill(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    entries: Vec<FillEntry>,
) -> Result<FillReport, String> {
    let pane = browser_pane_of(app, state, label)?;
    let known = known_form(label);
    let buttons = known_buttons(label);
    let report = fill_passes(
        entries,
        known.clone(),
        Duration::from_millis(BROWSER_FILL_PENDING_MS),
        BROWSER_WAIT_POLL,
        |asked, expect| {
            let pane = pane.clone();
            async move { fill_pass_on(&pane, asked, expect).await }
        },
    )
    .await?;
    if report.stale {
        return Ok(report);
    }
    let report = report.against(known.as_deref(), &buttons);
    remember_form(label, &report.fingerprint);
    remember_buttons(label, &report.actions);
    Ok(report)
}

/// One pass of a fill on a pane's page: the write; the settle after a press
/// ([`settle_after_press`], the one road — nothing new waits for a page here)
/// when it wrote; the read-back ([`settled_pass`]).
async fn fill_pass_on(
    pane: &BrowserPane,
    asked: Vec<FillEntry>,
    expect: Option<String>,
) -> Result<FillPass, String> {
    let unreadable = |_| "브라우저 판의 채움 답을 읽을 수 없습니다".to_string();
    settled_pass(
        || async move {
            let mut request = form_request();
            request["entries"] = serde_json::to_value(&asked).unwrap_or_default();
            request["expect"] = expect.into();
            request["phase"] = "write".into();
            let script = fill_script(&request, BROWSER_FILL_BODY);
            let reply = page_json(pane, script, BROWSER_CALLBACK_DEADLINE).await?;
            serde_json::from_value::<FillWritten>(page_value(reply)?).map_err(unreadable)
        },
        |epoch, at| async move { settle_after_press(pane, &epoch, at).await },
        |held, epoch| async move {
            let mut request = form_request();
            request["held"] = held;
            request["epoch"] = epoch.into();
            request["phase"] = "read".into();
            let script = fill_script(&request, BROWSER_FILL_BODY);
            let reply = page_json(pane, script, BROWSER_CALLBACK_DEADLINE).await?;
            serde_json::from_value::<FillPass>(page_value(reply)?).map_err(unreadable)
        },
    )
    .await
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
    mut expect: Option<String>,
    pending: Duration,
    poll: Duration,
    mut pass: P,
) -> Result<FillReport, String>
where
    P: FnMut(Vec<FillEntry>, Option<String>) -> F,
    F: std::future::Future<Output = Result<FillPass, String>>,
{
    let until = tokio::time::Instant::now() + pending;
    let mut ledger = FillLedger::new(entries);
    loop {
        let asked = ledger.next();
        if asked.is_empty() {
            break;
        }
        let answered = pass(asked.clone(), expect.take()).await?;
        ledger.record(&asked, answered);
        if ledger.next().is_empty() || tokio::time::Instant::now() >= until {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    Ok(ledger.report())
}
