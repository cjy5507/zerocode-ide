/* The bench's watch over its own records (t-41720): the values a run typed as secrets, each with the field it went into, and the places — a text a road sent, an answer of the door,
 * a line of the trace — that carry one. Apart from the roads: a road tells it what it typed and never asks it anything; the person's hands tell it the code. Only counts of places
 * leave it, never a value. */

// A value shorter than this is no mark to look for in a text: two digits stand in every number.
export const LEAK_MIN = 3;
// A value this long is looked for whole, in any text. A shorter one — a part of a card number, a code of three digits, a PIN — is one that another field may hold as well (a phone number's
// parts are four digits too), so it is looked for only where it belongs to the field it was typed into.
export const LEAK_WHOLE = 6;

export class Watch {
  constructor() {
    this.secrets = [];
    // Every value typed as a secret, whatever its length, with the field it went into: what a row's result is read against (`keptResult`).
    this.typed = [];
  }

  add(value, handle = "") {
    const text = typeof value === "string" ? value.trim() : "";
    if (text) this.typed.push({ value: text, handle: String(handle) });
    if (text.length >= LEAK_MIN) this.secrets.push({ value: text, handle: String(handle) });
  }

  /* Whether a text a road sent, or a line of the door's words, carries a watched value: a long one anywhere; a short one as the value of the field it was typed into — that field's handle as a key of a bundle,
   * or its own line of words showing the value. */
  carriesText(text) {
    const body = String(text);
    return this.secrets.some(({ value, handle }) => {
      if ((value.length >= LEAK_WHOLE || !handle) && body.includes(value)) return true;
      if (!handle) return false;
      if (body.includes(`${JSON.stringify(handle)}:`)) return true;
      const quoted = JSON.stringify(value);
      return body.split("\n").some((line) => line.includes(handle) && line.includes(quoted));
    });
  }

  /* Whether the JSON of an answer of the door carries one: a long value anywhere, a short one in the object of the field it was typed into, where that field says what it holds. */
  carriesJson(text) {
    const body = String(text);
    let parsed;
    try { parsed = JSON.parse(body); } catch (_) { return this.carriesText(body); }
    if (this.secrets.some(({ value, handle }) => (value.length >= LEAK_WHOLE || !handle) && body.includes(value))) return true;
    const shows = (node) => {
      if (Array.isArray(node)) return node.some(shows);
      if (!node || typeof node !== "object") return false;
      if (typeof node.handle === "string" && this.secrets.some(({ value, handle }) => handle === node.handle
        && [node.value, node.now].some((shown) => typeof shown === "string" && shown.includes(value)))) return true;
      return Object.values(node).some(shows);
    };
    return shows(parsed);
  }

  /* Whether a line of the trace carries one: what was sent, and the words of the door. */
  carriesNote(note) {
    return Object.entries(note).some(([key, value]) => {
      if (key === "sent" && value && typeof value === "object") return this.carriesText(JSON.stringify(value));
      if (typeof value === "string") return this.carriesText(value);
      return Array.isArray(value) && value.some((one) => typeof one === "string" && this.carriesText(one));
    });
  }

  /* The indexes of the texts of a list that carry one. */
  where(texts, carries = (text) => this.carriesText(text)) {
    const at = [];
    texts.forEach((text, index) => { if (carries(text)) at.push(index); });
    return at;
  }
}
