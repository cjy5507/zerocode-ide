'use strict';
// What both renderings of the long-form bench (t-37883) share: the state the
// page keeps, which fields show, how an answer is checked and how it is sent.
// The web page (form.js) and the phone screen (phone.js) draw the same spec
// differently; neither keeps a second copy of these rules.
//
// The state is written only by the controls' own events, the way a
// framework-driven form keeps it: a value placed into the DOM without the
// events a person's input makes never reaches what is submitted.

const PASSPORT_SHAPE = /^[A-Z][0-9]{8}$/i;
const EMAIL_SHAPE = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;
// A typed date: day, month and year, the way both renderings' hints spell it.
const DATE_SHAPE = /^(\d{2})\/(\d{2})\/(\d{4})$/;
// The digits a typed date holds at most (DDMMYYYY).
const DATE_DIGITS = 8;
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July',
  'August', 'September', 'October', 'November', 'December'];

const LongForm = {
  spec: null,
  state: {},
  // What a rendering does when a field another field shows on has changed.
  onShownChange: () => {},

  async load() {
    LongForm.spec = await (await fetch('spec')).json();
    return LongForm.spec;
  },

  fields() {
    return LongForm.spec.sections.flatMap((section) => section.fields);
  },

  optionsOf(field) {
    return LongForm.spec.options[field.options] || [];
  },

  labelOf(field, code) {
    const found = LongForm.optionsOf(field).find(([value]) => value === code);
    return found ? found[1] : '';
  },

  initial(field) {
    if (field.kind === 'counter') return field.min ?? 0;
    if (field.kind === 'checkbox') return false;
    if (field.kind === 'file') return null;
    return '';
  },

  shown(field) {
    return !field.shows_when || LongForm.state[field.shows_when.field] === field.shows_when.is;
  },

  // Write one field's value, as a control's event does; a field others show
  // on tells the rendering to re-judge them.
  set(id, value) {
    LongForm.state[id] = value;
    if (LongForm.fields().some((field) => field.shows_when && field.shows_when.field === id)) LongForm.onShownChange();
  },

  isoOf(typed) {
    const match = DATE_SHAPE.exec(typed);
    if (!match) return '';
    const [, day, month, year] = match.map(Number);
    const date = new Date(Date.UTC(year, month - 1, day));
    const real = date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
    return real ? `${match[3]}-${match[2]}-${match[1]}` : '';
  },

  typedOf(iso) {
    const [year, month, day] = iso.split('-');
    return `${day}/${month}/${year}`;
  },

  // The typed date's mask: digits only, slashes placed as they are typed.
  masked(value) {
    const digits = value.replace(/\D/g, '').slice(0, DATE_DIGITS);
    return [digits.slice(0, 2), digits.slice(2, 4), digits.slice(4)].filter(Boolean).join('/');
  },

  blank(field) {
    const value = LongForm.state[field.id];
    return value === '' || value === null || value === undefined || value === false;
  },

  // Every problem with the answer as it stands, in the form's order.
  // `typedButUnread(id)` says whether a typed date's words are there but are
  // no date — each rendering knows its own input.
  errors(typedButUnread) {
    const { state } = LongForm;
    const errors = [];
    const say = (id, message) => errors.push({ id, message });
    for (const field of LongForm.fields().filter(LongForm.shown)) {
      if (field.kind === 'datepick' && typedButUnread(field.id)) say(field.id, `${field.label} must be a real date, like 27/03/2007`);
      else if (field.required && LongForm.blank(field)) say(field.id, `Enter or choose: ${field.label}`);
    }
    const clear = (id) => !errors.some((error) => error.id === id);
    if (clear('email') && state.email && !EMAIL_SHAPE.test(state.email.trim())) say('email', 'Enter an email address like name@example.com');
    if (clear('email_confirm') && state.email_confirm && state.email_confirm.trim().toLowerCase() !== state.email.trim().toLowerCase()) {
      say('email_confirm', 'The email addresses do not match');
    }
    if (clear('passport_number') && state.passport_number && !PASSPORT_SHAPE.test(state.passport_number.trim())) {
      say('passport_number', 'A passport number is one letter and eight digits');
    }
    if (state.passport_issued && state.passport_expires && state.passport_expires <= state.passport_issued) {
      say('passport_expires', 'The expiry date must be after the date of issue');
    }
    if (state.arrival_date && state.departure_date && state.departure_date < state.arrival_date) {
      say('departure_date', 'The departure date must be on or after the arrival date');
    }
    return errors;
  },

  // Send the shown fields; answers the reference the server gave.
  async send() {
    const sent = Object.fromEntries(LongForm.fields().filter(LongForm.shown).map((field) => [field.id, LongForm.state[field.id]]));
    const answer = await (await fetch('submit', { method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ fields: sent }) })).json();
    return answer.reference;
  },
};

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) {
    if (value === false || value === undefined) continue;
    node.setAttribute(name, value === true ? '' : String(value));
  }
  node.append(...children);
  return node;
}
