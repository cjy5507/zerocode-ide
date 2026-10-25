/* ---- ZeroCode's own window as a Computer Use target (t-73906) ----
 *
 * An agent's request that names ZeroCode itself never reaches the helper: the
 * backend asks this page (`computer:self-act`) and the page answers by id
 * (`computer_self_answer`). Three requests:
 *
 *  - `look`: the visible controls, each with a role, a name, a value (never a
 *    secret field's) and the facts the window already marks it with;
 *  - `facts`: one control's facts again, read now, right before a press;
 *  - `do`: press that control — click it, fill it, or scroll it — only when
 *    its facts are still the facts the backend judged.
 *
 * Judging is the backend's (core `self_surface::judge`); this file reads
 * marks and presses. It presses with the document's own calls — `click()`, a
 * value and its input event, a scroll — so no pointer moves, no focus moves
 * and no key is sent: the person keeps the window while an agent works in it.
 * The person's own keys are told apart by `isTrusted`; a press is dropped if
 * the person typed in that control after its facts were read. */

/* Looks the page still answers presses for, newest last: an older look's
 * index names nothing any more. */
const SELF_ACT_LOOKS_KEPT = 4;
/* At most this many controls in one look, front to back. */
const SELF_ACT_MAX_CONTROLS = 600;
/* How long a pressed control wears its outline, so the person sees where an
 * agent pressed. */
const SELF_ACT_MARK_MS = 600;

const SELF_ACT_CONTROLS = [
  "button", "a[href]", "input", "select", "textarea", "summary",
  "[role=button]", "[role=tab]", "[role=link]", "[role=menuitem]", "[role=menuitemcheckbox]",
  "[role=menuitemradio]", "[role=option]", "[role=checkbox]", "[role=switch]", "[role=radio]",
  "[role=treeitem]", "[role=textbox]", "[role=combobox]", "[role=searchbox]",
  "[contenteditable=true]", "[contenteditable='']",
].join(",");
const SELF_ACT_SECRET = [
  "input[type=password]",
  "[autocomplete~=current-password]",
  "[autocomplete~=new-password]",
  "[autocomplete~=one-time-code]",
].join(",");
/* The window's own marks, each a mark it already draws: the one popup every
 * question to the person stands in, a dialog or a settings pane that holds a
 * secret, a terminal, a person-only container, a settings pane's body. */
const SELF_ACT_ASKING = "#ask-scrim";
const SELF_ACT_SECRET_HOLDERS = ".settings-pane, dialog, [role=dialog], .scrim";
const SELF_ACT_TERMINAL = ".terminal";
const SELF_ACT_PERSON_ONLY = "[data-person-only]";
const SELF_ACT_SETTING = ".settings-pane";

const selfActLooks = new Map();
let selfActNextLook = 0;
/* When the person last typed in a control (performance.now()), by control. */
const selfActTyped = new WeakMap();
/* When the backend last read a control's facts, by look and control. */
const selfActFactsRead = new Map();

for (const kind of ["keydown", "input", "paste"]) {
  document.addEventListener(kind, (event) => {
    if (event.isTrusted && event.target instanceof Element) selfActTyped.set(event.target, performance.now());
  }, true);
}

function selfActVisible(node) {
  if (!node.isConnected || node.closest("[hidden], [inert], [aria-hidden=true]")) return false;
  if (node.getClientRects().length === 0) return false;
  const style = getComputedStyle(node);
  return style.visibility !== "hidden" && style.display !== "none";
}

function selfActTextEntry(node) {
  if (node.isContentEditable) return true;
  if (node instanceof HTMLTextAreaElement) return true;
  if (!(node instanceof HTMLInputElement)) return ["textbox", "searchbox", "combobox"].includes(node.getAttribute("role"));
  return !["button", "submit", "reset", "checkbox", "radio", "range", "color", "file", "image", "hidden"].includes(node.type);
}

/* The role a control speaks, in the helper's vocabulary (core `render`). */
function selfActRole(node) {
  const role = node.getAttribute("role");
  const byRole = {
    button: "AXButton", tab: "AXTab", link: "AXLink", menuitem: "AXMenuItem",
    menuitemcheckbox: "AXMenuItem", menuitemradio: "AXMenuItem", option: "AXRow",
    checkbox: "AXCheckBox", switch: "AXCheckBox", radio: "AXRadioButton", treeitem: "AXRow",
    textbox: "AXTextField", searchbox: "AXTextField", combobox: "AXComboBox",
  };
  if (role && byRole[role]) return byRole[role];
  if (node instanceof HTMLAnchorElement) return "AXLink";
  if (node instanceof HTMLSelectElement) return "AXPopUpButton";
  if (node instanceof HTMLTextAreaElement || node.isContentEditable) return "AXTextArea";
  if (node instanceof HTMLInputElement) {
    if (node.type === "checkbox") return "AXCheckBox";
    if (node.type === "radio") return "AXRadioButton";
    if (node.type === "range") return "AXSlider";
    if (["button", "submit", "reset", "image"].includes(node.type)) return "AXButton";
    return "AXTextField";
  }
  return "AXButton";
}

function selfActName(node) {
  const said = (text) => (text ?? "").replace(/\s+/g, " ").trim().slice(0, 120);
  const aria = said(node.getAttribute("aria-label"));
  if (aria) return aria;
  const by = node.getAttribute("aria-labelledby");
  if (by) {
    const text = said(by.split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? "").join(" "));
    if (text) return text;
  }
  const label = said(node.labels?.[0]?.textContent);
  if (label) return label;
  const title = said(node.getAttribute("title") ?? node.dataset.tip);
  if (!(node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement)) {
    const text = said(node.textContent);
    if (text) return text;
  }
  return title || said(node.getAttribute("placeholder"));
}

function selfActFacts(node) {
  const holder = node.closest(SELF_ACT_SECRET_HOLDERS);
  const typed = selfActTyped.get(node);
  const focused = document.activeElement;
  return {
    asking: node.closest(SELF_ACT_ASKING) !== null,
    secret: node.matches(SELF_ACT_SECRET),
    besideSecret: holder !== null && holder.querySelector(SELF_ACT_SECRET) !== null,
    terminal: node.closest(SELF_ACT_TERMINAL) !== null,
    personOnly: node.closest(SELF_ACT_PERSON_ONLY) !== null,
    setting: node.closest(SELF_ACT_SETTING) !== null,
    personFocus: selfActTextEntry(node) && focused !== null && (focused === node || node.contains(focused)),
    typedMsAgo: typed === undefined ? null : Math.max(0, Math.round(performance.now() - typed)),
  };
}

function selfActValue(node, facts) {
  if (facts.secret) return null;
  if (node instanceof HTMLInputElement && (node.type === "checkbox" || node.type === "radio")) {
    return node.checked ? "checked" : "unchecked";
  }
  if ("value" in node && typeof node.value === "string") return node.value.slice(0, 200);
  if (node.isContentEditable) return (node.textContent ?? "").slice(0, 200);
  return null;
}

function selfActLook() {
  const lookId = ++selfActNextLook;
  const nodes = [...document.querySelectorAll(SELF_ACT_CONTROLS)]
    .filter(selfActVisible)
    .slice(0, SELF_ACT_MAX_CONTROLS);
  selfActLooks.set(lookId, nodes);
  for (const old of [...selfActLooks.keys()].slice(0, -SELF_ACT_LOOKS_KEPT)) selfActLooks.delete(old);
  const focused = nodes.indexOf(document.activeElement);
  return {
    lookId,
    title: document.title || "ZeroCode",
    focused: focused < 0 ? null : focused + 1,
    controls: nodes.map((node, id) => {
      const facts = selfActFacts(node);
      return { id, role: selfActRole(node), name: selfActName(node), value: selfActValue(node, facts), facts };
    }),
  };
}

/* The control a look showed under `id`, while it is still in the document. */
function selfActNode(lookId, id) {
  const node = selfActLooks.get(lookId)?.[id];
  return node && node.isConnected ? node : null;
}

function selfActFactsOf(lookId, id) {
  const node = selfActNode(lookId, id);
  if (!node || !selfActVisible(node)) return { gone: true };
  selfActFactsRead.set(`${lookId}:${id}`, performance.now());
  return { role: selfActRole(node), name: selfActName(node), facts: selfActFacts(node) };
}

/* The marks that decide a press, without the clock: the backend judged these. */
function selfActMarks(facts) {
  const { typedMsAgo: _clock, ...marks } = facts ?? {};
  return JSON.stringify(Object.keys(marks).sort().map((key) => [key, marks[key]]));
}

function selfActFill(node, value) {
  if (node.isContentEditable) {
    node.textContent = value;
  } else if (node instanceof HTMLSelectElement) {
    if (![...node.options].some((option) => option.value === value)) return false;
    node.value = value;
  } else if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
    if (!selfActTextEntry(node)) return false;
    // The prototype's own setter, so a field that watches its value hears it.
    const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(node), "value")?.set;
    if (setter) setter.call(node, value);
    else node.value = value;
  } else {
    return false;
  }
  node.dispatchEvent(new Event("input", { bubbles: true }));
  node.dispatchEvent(new Event("change", { bubbles: true }));
  return true;
}

function selfActScroll(node, direction, pages) {
  let box = node;
  while (box && box !== document.documentElement) {
    const style = getComputedStyle(box);
    if (/(auto|scroll)/.test(style.overflowY + style.overflowX)
        && (box.scrollHeight > box.clientHeight || box.scrollWidth > box.clientWidth)) break;
    box = box.parentElement;
  }
  box ??= document.scrollingElement;
  const count = Number.isFinite(Number(pages)) ? Number(pages) : 1;
  const across = direction === "left" || direction === "right";
  const sign = direction === "up" || direction === "left" ? -1 : 1;
  const step = (across ? box.clientWidth : box.clientHeight) * count * sign;
  box.scrollBy(across ? { left: step } : { top: step });
  return true;
}

function selfActOutline(node) {
  node.classList.add("self-act-pressed");
  setTimeout(() => node.classList.remove("self-act-pressed"), SELF_ACT_MARK_MS);
}

/* A press: on the control the look showed, only while its marks are the ones
 * judged and the person has not typed in it since they were read. */
function selfActDo(request) {
  const node = selfActNode(request.lookId, request.id);
  const read = selfActFactsRead.get(`${request.lookId}:${request.id}`);
  if (!node || !selfActVisible(node) || read === undefined) return { done: false };
  const now = selfActFacts(node);
  if (selfActMarks(now) !== selfActMarks(request.facts)) return { done: false };
  const typed = selfActTyped.get(node);
  if (typed !== undefined && typed > read) return { done: false };
  selfActFactsRead.delete(`${request.lookId}:${request.id}`);
  let done = false;
  if (request.act === "click") {
    node.click();
    done = true;
  } else if (request.act === "setValue") {
    done = selfActFill(node, String(request.value ?? ""));
    if (!done) {
      return { error: { code: "value_not_settable", message: "that control of ZeroCode's window takes no typed value; click it instead" } };
    }
  } else if (request.act === "scroll") {
    done = selfActScroll(node, request.direction, request.pages);
  }
  if (done) selfActOutline(node);
  return { done };
}

function selfActAnswer(request) {
  switch (request?.kind) {
    case "look": return selfActLook();
    case "facts": return selfActFactsOf(request.lookId, request.id);
    case "do": return selfActDo(request);
    default: return { error: { code: "invalid_argument", message: `the page does not answer ${request?.kind}` } };
  }
}

listen("computer:self-act", (event) => {
  const { id, request } = event.payload ?? {};
  let answer;
  try {
    const said = selfActAnswer(request);
    answer = said?.error ? said : { ok: said };
  } catch (error) {
    answer = { error: { code: "accessibility_error", message: String(error?.message ?? error) } };
  }
  void invoke("computer_self_answer", { id, answer }).catch(() => {});
});
