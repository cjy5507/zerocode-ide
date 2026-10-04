'use strict';
// The long-form bench as a phone app (t-37883): the same spec, drawn the way
// an iOS form is — grouped rows, sheets for choices, a calendar sheet for a
// date, segmented controls, a stepper, switches, an on-screen keyboard that
// covers the lower screen while a field is being typed in, and an alert for
// a refused submit. The rules (state, what shows, checks, sending) are
// form-core.js's.

// A choice list this long gets a search field at the top of its sheet.
const SEARCH_FROM = 12;
// How long a sheet or the keyboard takes to slide (its CSS transition).
const SLIDE_MS = 300;
// The years the date sheet offers, counted back from this year.
const YEARS_BACK = 100;
const KEY_ROWS = ['1234567890', 'qwertyuiop', 'asdfghjkl', 'zxcvbnm'];

const rows = {};
const sheet = () => document.getElementById('sheet');
const shade = () => document.getElementById('shade');
const keyboard = () => document.getElementById('keyboard');

function card(...children) {
  return el('div', { class: 'card' }, ...children);
}

function shownValue(field) {
  const value = LongForm.state[field.id];
  if (field.kind === 'date') return value ? LongForm.typedOf(value) : 'Select';
  return LongForm.labelOf(field, value) || 'Select';
}

// ---- the keyboard: up while a text field has the focus -------------------

function keyboardUp(up) {
  keyboard().classList.toggle('up', up);
  document.body.classList.toggle('typing', up);
}

function drawKeyboard() {
  const done = el('button', { type: 'button' }, 'Done');
  done.addEventListener('mousedown', (event) => event.preventDefault());
  done.addEventListener('click', () => document.activeElement && document.activeElement.blur());
  const press = (words) => {
    const target = document.activeElement;
    if (!target || !('value' in target)) return;
    if (words === '⌫') document.execCommand('delete');
    else document.execCommand('insertText', false, words);
  };
  const keyRow = (keys, extra = []) => el('div', { class: 'keys' }, ...[...keys].map((key) => {
    const button = el('button', { type: 'button' }, key);
    button.addEventListener('mousedown', (event) => event.preventDefault());
    button.addEventListener('click', () => press(key));
    return button;
  }), ...extra);
  const wide = el('button', { type: 'button', class: 'wide' }, 'space');
  wide.addEventListener('mousedown', (event) => event.preventDefault());
  wide.addEventListener('click', () => press(' '));
  const back = el('button', { type: 'button' }, '⌫');
  back.addEventListener('mousedown', (event) => event.preventDefault());
  back.addEventListener('click', () => press('⌫'));
  keyboard().replaceChildren(el('div', { class: 'accessory' }, done),
    ...KEY_ROWS.slice(0, 3).map((keys) => keyRow(keys)), keyRow(KEY_ROWS[3], [back]), el('div', { class: 'keys' }, wide));
}

function typingField(input) {
  input.addEventListener('focus', () => {
    keyboardUp(true);
    setTimeout(() => input.scrollIntoView({ block: 'center' }), SLIDE_MS);
  });
  input.addEventListener('blur', () => setTimeout(() => {
    if (!document.activeElement || !('value' in document.activeElement)) keyboardUp(false);
  }, 0));
  return input;
}

// ---- sheets ---------------------------------------------------------------

function openSheet(title, body, onCancel) {
  const cancel = el('button', { type: 'button' }, 'Cancel');
  cancel.addEventListener('click', () => { closeSheet(); if (onCancel) onCancel(); });
  sheet().replaceChildren(el('div', { class: 'grab' }), el('div', { class: 'bar' }, cancel, el('span', {}, title), el('span', {})), ...body);
  shade().classList.add('up');
  sheet().classList.add('up');
}

function closeSheet() {
  if (document.activeElement) document.activeElement.blur();
  keyboardUp(false);
  shade().classList.remove('up');
  sheet().classList.remove('up');
}

function choiceSheet(field, onChoose) {
  const list = el('div', { class: 'list' });
  const draw = (typed) => {
    const words = typed.trim().toLowerCase();
    list.replaceChildren(...LongForm.optionsOf(field).filter(([, label]) => label.toLowerCase().includes(words)).map(([code, label]) => {
      const mark = LongForm.state[field.id] === code ? '✓' : '';
      const button = el('button', { type: 'button' }, el('span', {}, label), el('span', { class: 'mark' }, mark));
      button.addEventListener('click', () => { onChoose(code); closeSheet(); });
      return button;
    }));
  };
  const parts = [];
  if (LongForm.optionsOf(field).length > SEARCH_FROM) {
    const search = typingField(el('input', { class: 'search', type: 'search', placeholder: 'Search', autocomplete: 'off' }));
    search.addEventListener('input', () => draw(search.value));
    parts.push(search);
  }
  draw('');
  openSheet(field.label, [...parts, list]);
}

function dateSheet(field, onChoose) {
  const chosen = LongForm.state[field.id];
  const today = new Date();
  let year = chosen ? Number(chosen.slice(0, 4)) : today.getUTCFullYear();
  let month = chosen ? Number(chosen.slice(5, 7)) - 1 : today.getUTCMonth();
  let picking = false;
  const body = el('div', { class: 'cal' });
  const draw = () => {
    const header = el('button', { type: 'button' }, `${MONTHS[month]} ${year} ${picking ? '⌃' : '›'}`);
    header.addEventListener('click', () => { picking = !picking; draw(); });
    const back = el('button', { type: 'button', 'aria-label': 'Previous month' }, '‹');
    const next = el('button', { type: 'button', 'aria-label': 'Next month' }, '›');
    back.addEventListener('click', () => { month -= 1; if (month < 0) { month = 11; year -= 1; } draw(); });
    next.addEventListener('click', () => { month += 1; if (month > 11) { month = 0; year += 1; } draw(); });
    const top = el('div', { class: 'top' }, header, el('span', {}, back, next));
    if (picking) {
      const years = el('div', { class: 'col' });
      for (let each = today.getUTCFullYear(); each >= today.getUTCFullYear() - YEARS_BACK; each -= 1) {
        const button = el('button', { type: 'button', class: each === year ? 'on' : '' }, String(each));
        button.addEventListener('click', () => { year = each; draw(); });
        years.append(button);
      }
      const months = el('div', { class: 'col' }, ...MONTHS.map((name, at) => {
        const button = el('button', { type: 'button', class: at === month ? 'on' : '' }, name);
        button.addEventListener('click', () => { month = at; draw(); });
        return button;
      }));
      body.replaceChildren(top, el('div', { class: 'years' }, years, months));
      const on = years.querySelector('.on');
      if (on) years.scrollTop = on.offsetTop - years.clientHeight / 2;
      return;
    }
    const first = new Date(Date.UTC(year, month, 1)).getUTCDay();
    const days = new Date(Date.UTC(year, month + 1, 0)).getUTCDate();
    const grid = el('div', { class: 'grid' }, ...['S', 'M', 'T', 'W', 'T', 'F', 'S'].map((day) => el('span', {}, day)));
    for (let blank = 0; blank < first; blank += 1) grid.append(el('span'));
    for (let day = 1; day <= days; day += 1) {
      const iso = `${year}-${String(month + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
      const button = el('button', { type: 'button', class: iso === LongForm.state[field.id] ? 'on' : '' }, String(day));
      button.addEventListener('click', () => { onChoose(iso); draw(); });
      grid.append(button);
    }
    body.replaceChildren(top, grid);
  };
  draw();
  openSheet(field.label, [body]);
  const done = el('button', { type: 'button' }, 'Done');
  done.addEventListener('click', closeSheet);
  sheet().querySelector('.bar').lastChild.replaceWith(done);
}

// ---- rows -----------------------------------------------------------------

function textRow(field) {
  const input = typingField(field.kind === 'textarea'
    ? el('textarea', { id: field.id, rows: 2 })
    : el('input', { id: field.id, type: field.kind === 'datepick' ? 'text' : field.kind, autocomplete: 'off',
      inputmode: field.kind === 'datepick' ? 'numeric' : undefined, placeholder: field.kind === 'datepick' ? 'DD/MM/YYYY' : (field.hint || '') }));
  input.addEventListener('input', () => {
    if (field.kind === 'datepick') {
      const shaped = LongForm.masked(input.value);
      if (shaped !== input.value) input.value = shaped;
      LongForm.set(field.id, LongForm.isoOf(shaped));
    } else {
      LongForm.set(field.id, input.value);
    }
  });
  return el('div', { class: 'row' }, el('span', { class: 'label' }, field.label), input);
}

function pickRow(field, open) {
  const value = el('span', { class: 'value' });
  const draw = () => {
    value.replaceChildren(shownValue(field), el('span', { class: 'chevron' }, '›'));
    value.classList.toggle('set', Boolean(LongForm.state[field.id]));
  };
  draw();
  const row = el('div', { class: 'row inline' }, el('span', { class: 'label' }, field.label), value);
  row.addEventListener('click', () => {
    if (document.activeElement) document.activeElement.blur();
    open(field, (code) => { LongForm.set(field.id, code); draw(); });
  });
  return row;
}

function segmentsRow(field) {
  const buttons = LongForm.optionsOf(field).map(([code, label]) => {
    const button = el('button', { type: 'button' }, label);
    button.addEventListener('click', () => {
      LongForm.set(field.id, code);
      buttons.forEach((each) => each.classList.toggle('on', each === button));
    });
    return button;
  });
  const question = field.kind === 'yesno' ? el('div', { class: 'question' }, field.label) : el('span', { class: 'label' }, field.label);
  return el('div', { class: 'row' }, question, el('div', { class: 'segments' }, ...buttons));
}

function checksRow(field) {
  const buttons = LongForm.optionsOf(field).map(([code, label]) => {
    const button = el('button', { type: 'button' }, el('span', {}, label), el('span', { class: 'mark' }, '✓'));
    button.addEventListener('click', () => {
      LongForm.set(field.id, code);
      buttons.forEach((each) => each.classList.toggle('on', each === button));
    });
    return button;
  });
  return el('div', { class: 'row' }, el('span', { class: 'label' }, field.label), el('div', { class: 'checks' }, ...buttons));
}

function stepperRow(field) {
  const count = el('span', { class: 'count' }, String(LongForm.state[field.id]));
  const step = (by) => {
    const next = Math.max(field.min ?? 0, Math.min(field.max ?? 9, LongForm.state[field.id] + by));
    LongForm.set(field.id, next);
    count.textContent = String(next);
  };
  const less = el('button', { type: 'button' }, '−');
  const more = el('button', { type: 'button' }, '+');
  less.addEventListener('click', () => step(-1));
  more.addEventListener('click', () => step(1));
  return el('div', { class: 'row inline' }, el('span', { class: 'label' }, field.label), el('div', { class: 'stepper' }, less, count, more));
}

function switchRow(field) {
  const toggle = el('button', { type: 'button', class: 'switch' });
  toggle.addEventListener('click', () => {
    LongForm.set(field.id, !LongForm.state[field.id]);
    toggle.classList.toggle('on', LongForm.state[field.id]);
  });
  return el('div', { class: 'row inline' }, el('span', { class: 'consent' }, field.label), toggle);
}

// The upload is the person's: the button opens the phone's own photo choice,
// which no agent drives — a hand-off attaches the file (window.longFormAttach).
function uploadRow(field) {
  const name = el('span', { class: 'value' }, 'No photo');
  const add = el('button', { type: 'button', class: 'link' }, 'Add photo of passport page');
  add.addEventListener('click', () => {
    const library = el('button', { type: 'button' }, el('span', {}, 'Choose from Library'));
    const camera = el('button', { type: 'button' }, el('span', {}, 'Take Photo'));
    const hint = el('p', { class: 'notice' }, 'Your photos are only shown to you. Ask the phone’s owner to choose one.');
    openSheet('Add photo', [el('div', { class: 'list' }, camera, library), hint]);
  });
  window.longFormAttach = (file) => {
    LongForm.set(field.id, file);
    name.textContent = file.name;
    name.classList.add('set');
    closeSheet();
  };
  return el('div', { class: 'row' }, el('span', { class: 'label' }, field.label), add, name);
}

const ROWS = {
  text: textRow, email: textRow, tel: textRow, textarea: textRow, datepick: textRow,
  date: (field) => pickRow(field, dateSheet),
  select: (field) => pickRow(field, choiceSheet), combobox: (field) => pickRow(field, choiceSheet),
  radio: (field) => (LongForm.optionsOf(field).length <= 3 ? segmentsRow(field) : checksRow(field)),
  yesno: segmentsRow, counter: stepperRow, checkbox: switchRow, file: uploadRow,
};

function showAlert(title, words) {
  const box = document.getElementById('alert');
  const ok = el('button', { type: 'button' }, 'OK');
  ok.addEventListener('click', () => { box.classList.remove('up'); shade().classList.remove('up'); });
  box.replaceChildren(el('h2', {}, title), el('p', {}, words), ok);
  shade().classList.add('up');
  box.classList.add('up');
}

async function submit() {
  if (document.activeElement) document.activeElement.blur();
  const errors = LongForm.errors((id) => Boolean(document.getElementById(id).value) && !LongForm.state[id]);
  if (errors.length) {
    const more = errors.length > 3 ? ` (+${errors.length - 3} more)` : '';
    showAlert('There is a problem', `${errors.slice(0, 3).map((error) => error.message).join('. ')}${more}.`);
    return;
  }
  const reference = await LongForm.send();
  document.getElementById('app').replaceChildren(el('div', { class: 'done' }, el('div', { class: 'tick' }, '✓'),
    el('h2', {}, 'Registration received'), el('p', {}, 'Your reference number is'), el('h1', {}, reference)));
  window.scrollTo(0, 0);
}

async function main() {
  const spec = await LongForm.load();
  for (const field of LongForm.fields()) LongForm.state[field.id] = LongForm.initial(field);
  const app = document.getElementById('app');
  app.append(el('h1', {}, spec.title), el('p', { class: 'notice' }, spec.notice));
  for (const section of spec.sections) {
    const rowsOf = section.fields.map((field) => {
      rows[field.id] = ROWS[field.kind](field);
      return rows[field.id];
    });
    app.append(el('div', { class: 'head' }, section.title.replace(/^\d+\.\s*/, '')), card(...rowsOf));
  }
  const send = el('button', { type: 'button', class: 'primary' }, 'Submit registration');
  send.addEventListener('click', submit);
  app.append(send);
  LongForm.onShownChange = () => {
    for (const field of LongForm.fields()) rows[field.id].hidden = !LongForm.shown(field);
  };
  LongForm.onShownChange();
  drawKeyboard();
}

main();
