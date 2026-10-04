'use strict';
// The long-form bench as a web page (t-37883): the same spec drawn the way a
// government site draws it — labels above controls, native selects, an ARIA
// combobox, masked dates with a calendar, radios, a counter, a file picker.
// The rules (state, what shows, checks, sending) are form-core.js's.

// How long a combobox keeps its list open after its input lost focus, so a
// press on an option lands before the list closes.
const COMBO_CLOSE_MS = 150;

const rows = {};

// One labelled row: the label, the hint, the control and its error line.
function row(field, control, labelTag = 'label') {
  const label = labelTag === 'label'
    ? el('label', { for: field.id, id: `${field.id}-label` }, field.label)
    : el('div', { class: 'label', id: `${field.id}-label` }, field.label);
  const parts = [label];
  if (field.hint) parts.push(el('span', { class: 'hint', id: `${field.id}-hint` }, field.hint));
  parts.push(el('span', { class: 'field-error', id: `${field.id}-error`, hidden: true }));
  parts.push(control);
  return el('div', { class: 'row', 'data-field': field.id }, ...parts);
}

function textControl(field, type) {
  const input = el('input', { id: field.id, name: field.id, type, autocomplete: 'off',
    'aria-describedby': field.hint ? `${field.id}-hint` : undefined });
  input.addEventListener('input', () => LongForm.set(field.id, input.value));
  return row(field, input);
}

function textareaControl(field) {
  const area = el('textarea', { id: field.id, name: field.id, rows: 3 });
  area.addEventListener('input', () => LongForm.set(field.id, area.value));
  return row(field, area);
}

function selectControl(field) {
  const select = el('select', { id: field.id, name: field.id }, el('option', { value: '' }, 'Select…'));
  for (const [code, label] of LongForm.optionsOf(field)) select.append(el('option', { value: code }, label));
  select.addEventListener('change', () => LongForm.set(field.id, select.value));
  return row(field, select);
}

// An ARIA combobox (the list filters as you type; a choice is made by Enter or
// a press on an option, never by the typed words alone).
function comboboxControl(field) {
  const listId = `${field.id}-list`;
  const input = el('input', { id: field.id, type: 'text', role: 'combobox', autocomplete: 'off',
    'aria-autocomplete': 'list', 'aria-expanded': 'false', 'aria-controls': listId });
  const list = el('ul', { id: listId, role: 'listbox', 'aria-label': field.label, hidden: true });
  let shown = [];
  let active = -1;
  const close = () => { list.hidden = true; input.setAttribute('aria-expanded', 'false'); input.removeAttribute('aria-activedescendant'); active = -1; };
  const mark = (index) => {
    active = index;
    [...list.children].forEach((item, at) => item.setAttribute('aria-selected', String(at === index)));
    if (index >= 0) input.setAttribute('aria-activedescendant', list.children[index].id);
  };
  const choose = ([code, label]) => { input.value = label; LongForm.set(field.id, code); close(); };
  const open = () => {
    const typed = input.value.trim().toLowerCase();
    shown = LongForm.optionsOf(field).filter(([, label]) => label.toLowerCase().includes(typed));
    list.replaceChildren(...shown.map(([code, label], at) => {
      const item = el('li', { id: `${listId}-${code}`, role: 'option', 'aria-selected': 'false' }, label);
      item.addEventListener('mousedown', (event) => { event.preventDefault(); choose(shown[at]); });
      return item;
    }));
    list.hidden = shown.length === 0;
    input.setAttribute('aria-expanded', String(!list.hidden));
    mark(shown.length ? 0 : -1);
  };
  input.addEventListener('input', () => { LongForm.set(field.id, ''); open(); });
  input.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (list.hidden) { open(); return; }
      const step = event.key === 'ArrowDown' ? 1 : -1;
      mark(Math.max(0, Math.min(shown.length - 1, active + step)));
    } else if (event.key === 'Enter' && !list.hidden && active >= 0) {
      event.preventDefault();
      choose(shown[active]);
    } else if (event.key === 'Escape') {
      close();
    }
  });
  input.addEventListener('blur', () => setTimeout(close, COMBO_CLOSE_MS));
  return row(field, el('div', { class: 'combo' }, input, list));
}

function radioControl(field) {
  const set = el('fieldset', { id: field.id, 'data-field': field.id, class: 'row' }, el('legend', { id: `${field.id}-label` }, field.label),
    el('span', { class: 'field-error', id: `${field.id}-error`, hidden: true }));
  for (const [code, label] of LongForm.optionsOf(field)) {
    const radio = el('input', { type: 'radio', name: field.id, id: `${field.id}-${code}`, value: code });
    radio.addEventListener('change', () => { if (radio.checked) LongForm.set(field.id, code); });
    set.append(el('label', { for: radio.id }, radio, label));
  }
  return set;
}

function nativeDateControl(field) {
  const input = el('input', { id: field.id, name: field.id, type: 'date' });
  const take = () => LongForm.set(field.id, input.value);
  input.addEventListener('input', take);
  input.addEventListener('change', take);
  return row(field, input);
}

// A text date with a mask and a calendar beside it.
function datePickControl(field) {
  const hinted = { ...field, hint: field.hint || 'For example, 27/03/2007' };
  const input = el('input', { id: field.id, name: field.id, type: 'text', inputmode: 'numeric', placeholder: 'DD/MM/YYYY',
    autocomplete: 'off', 'aria-describedby': `${field.id}-hint` });
  const toggle = el('button', { type: 'button', 'aria-expanded': 'false', 'aria-controls': `${field.id}-calendar`,
    'aria-label': `Choose ${field.label.toLowerCase()} from a calendar` }, 'Choose date');
  const calendar = el('div', { class: 'calendar', id: `${field.id}-calendar`, role: 'dialog', 'aria-label': `${field.label} calendar`, hidden: true });
  input.addEventListener('input', () => {
    const shaped = LongForm.masked(input.value);
    if (shaped !== input.value) input.value = shaped;
    LongForm.set(field.id, LongForm.isoOf(shaped));
  });
  const today = new Date();
  let shownYear = today.getUTCFullYear();
  let shownMonth = today.getUTCMonth();
  const draw = () => {
    const first = new Date(Date.UTC(shownYear, shownMonth, 1)).getUTCDay();
    const days = new Date(Date.UTC(shownYear, shownMonth + 1, 0)).getUTCDate();
    const back = el('button', { type: 'button', 'aria-label': 'Previous month' }, '‹');
    const next = el('button', { type: 'button', 'aria-label': 'Next month' }, '›');
    back.addEventListener('click', () => { shownMonth -= 1; if (shownMonth < 0) { shownMonth = 11; shownYear -= 1; } draw(); });
    next.addEventListener('click', () => { shownMonth += 1; if (shownMonth > 11) { shownMonth = 0; shownYear += 1; } draw(); });
    const body = el('tbody');
    let week = el('tr');
    for (let blank = 0; blank < first; blank += 1) week.append(el('td'));
    for (let day = 1; day <= days; day += 1) {
      const iso = `${shownYear}-${String(shownMonth + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
      const pick = el('button', { type: 'button', 'aria-label': `${day} ${MONTHS[shownMonth]} ${shownYear}` }, String(day));
      pick.addEventListener('click', () => {
        input.value = LongForm.typedOf(iso);
        LongForm.set(field.id, iso);
        calendar.hidden = true;
        toggle.setAttribute('aria-expanded', 'false');
        input.focus();
      });
      week.append(el('td', {}, pick));
      if ((first + day) % 7 === 0) { body.append(week); week = el('tr'); }
    }
    if (week.children.length) body.append(week);
    calendar.replaceChildren(el('div', { class: 'nav' }, back, el('span', {}, `${MONTHS[shownMonth]} ${shownYear}`), next),
      el('table', {}, body));
  };
  toggle.addEventListener('click', () => {
    const iso = LongForm.state[field.id];
    if (iso) { shownYear = Number(iso.slice(0, 4)); shownMonth = Number(iso.slice(5, 7)) - 1; }
    calendar.hidden = !calendar.hidden;
    toggle.setAttribute('aria-expanded', String(!calendar.hidden));
    if (!calendar.hidden) draw();
  });
  return row(hinted, el('div', {}, el('div', { class: 'datepick' }, input, toggle), calendar));
}

// A counter: a spinbutton between two buttons, keyboard-operable the APG way.
function counterControl(field) {
  const min = field.min ?? 0;
  const max = field.max ?? 9;
  const value = el('span', { id: field.id, role: 'spinbutton', tabindex: 0, 'aria-valuenow': min, 'aria-valuemin': min,
    'aria-valuemax': max, 'aria-labelledby': `${field.id}-label` }, String(min));
  const set = (next) => {
    const clamped = Math.max(min, Math.min(max, next));
    value.textContent = String(clamped);
    value.setAttribute('aria-valuenow', clamped);
    LongForm.set(field.id, clamped);
  };
  const less = el('button', { type: 'button', 'aria-label': `Decrease: ${field.label}` }, '−');
  const more = el('button', { type: 'button', 'aria-label': `Increase: ${field.label}` }, '+');
  less.addEventListener('click', () => set(LongForm.state[field.id] - 1));
  more.addEventListener('click', () => set(LongForm.state[field.id] + 1));
  const steps = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 };
  value.addEventListener('keydown', (event) => {
    if (event.key in steps) { event.preventDefault(); set(LongForm.state[field.id] + steps[event.key]); }
    if (event.key === 'Home') { event.preventDefault(); set(min); }
    if (event.key === 'End') { event.preventDefault(); set(max); }
  });
  return row(field, el('div', { class: 'counter' }, less, value, more), 'div');
}

function checkboxControl(field) {
  const box = el('input', { type: 'checkbox', id: field.id, name: field.id });
  box.addEventListener('change', () => LongForm.set(field.id, box.checked));
  return el('div', { class: 'row', 'data-field': field.id },
    el('span', { class: 'field-error', id: `${field.id}-error`, hidden: true }),
    el('label', { for: field.id }, box, ' ', field.label));
}

function fileControl(field) {
  const input = el('input', { type: 'file', id: field.id, name: field.id, class: 'visually-hidden', accept: '.jpg,.jpeg,.png,.pdf' });
  const name = el('span', { class: 'file-name', id: `${field.id}-name` }, 'No file chosen');
  input.addEventListener('change', () => {
    const file = input.files && input.files[0];
    LongForm.set(field.id, file ? { name: file.name, size: file.size } : null);
    name.textContent = file ? file.name : 'No file chosen';
  });
  const pick = el('label', { for: field.id, class: 'file-pick' }, el('span', { role: 'button' }, 'Choose file'));
  return row(field, el('div', {}, input, pick, name), 'div');
}

const CONTROLS = {
  text: (field) => textControl(field, 'text'),
  email: (field) => textControl(field, 'email'),
  tel: (field) => textControl(field, 'tel'),
  textarea: textareaControl,
  select: selectControl,
  combobox: comboboxControl,
  radio: radioControl,
  yesno: radioControl,
  date: nativeDateControl,
  datepick: datePickControl,
  counter: counterControl,
  checkbox: checkboxControl,
  file: fileControl,
};

function showErrors(errors) {
  for (const field of LongForm.fields()) {
    const line = document.getElementById(`${field.id}-error`);
    const error = errors.find((each) => each.id === field.id);
    line.hidden = !error;
    line.textContent = error ? error.message : '';
  }
  const box = document.getElementById('errors');
  box.replaceChildren(el('h2', {}, 'There is a problem'),
    el('ul', {}, ...errors.map((error) => el('li', {}, el('a', { href: `#${error.id}` }, error.message)))));
  box.hidden = errors.length === 0;
  if (errors.length) box.focus();
}

async function submit(event) {
  event.preventDefault();
  const errors = LongForm.errors((id) => Boolean(document.getElementById(id).value) && !LongForm.state[id]);
  showErrors(errors);
  if (errors.length) return;
  const reference = await LongForm.send();
  document.getElementById('form').hidden = true;
  const done = document.getElementById('done');
  done.replaceChildren(el('h2', {}, 'Registration received'), el('p', {}, 'Your reference number is ', el('strong', { id: 'reference' }, reference)));
  done.hidden = false;
  done.focus();
}

async function main() {
  const spec = await LongForm.load();
  for (const field of LongForm.fields()) LongForm.state[field.id] = LongForm.initial(field);
  document.getElementById('notice').textContent = spec.notice;
  const form = document.getElementById('form');
  for (const section of spec.sections) {
    const part = el('section', { 'aria-labelledby': `${section.id}-title` }, el('h2', { id: `${section.id}-title` }, section.title));
    for (const field of section.fields) {
      rows[field.id] = CONTROLS[field.kind](field);
      part.append(rows[field.id]);
    }
    form.append(part);
  }
  form.append(el('div', { class: 'row' }, el('button', { type: 'submit', class: 'primary' }, 'Submit registration')));
  form.addEventListener('submit', submit);
  LongForm.onShownChange = () => {
    for (const field of LongForm.fields()) rows[field.id].hidden = !LongForm.shown(field);
  };
  LongForm.onShownChange();
}

main();
