/* A form's page half: find the fields a saved address can fill, and write into
 * them when the person picks one (t-79331). One expression, evaluated by the
 * window in the pane's own page (`browser_form_fill.rs` hands it `op` and
 * `args`); it keeps no global and plants nothing.
 *
 *   detect — answers which kinds of field the page's fullest form holds, and
 *            the names of its other empty text fields. It never reads a
 *            field's value.
 *   fill   — writes the values it is given into that form's empty fields and
 *            answers only HOW MANY fields took a value. The values are an
 *            argument of this call and leave with it: they are not returned,
 *            logged, or kept in a variable the page can reach.
 *
 * A field's kind is HTML's own autocomplete word — the word the import stores
 * an address under (`browser_profile_data.rs`). The platform's mark decides
 * first (`autocomplete`), then the field's type (`email`, `tel`), then a word
 * in its name, id, placeholder or label, in the five languages the window
 * speaks. A card, password or one-time-code field is never a kind: nothing is
 * written there.
 *
 * The generic hands — which field counts as visible, the words a field carries,
 * and the write through the native setter that a page's framework hears — are
 * the sign-in strip scaffold's (t-79326), kept here as their one copy.
 */
(op, args) => {
  const visible = (element) => {
    if (!element || !element.isConnected || element.hidden || element.disabled || element.readOnly) return false;
    const style = getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const marks = (element) => String(element.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/).filter(Boolean);
  const wordsOf = (element) => [
    element.name, element.id, element.placeholder, element.getAttribute("aria-label"),
    element.labels && element.labels[0] ? element.labels[0].textContent : "",
  ].map((word) => String(word || "").trim()).filter(Boolean);

  // HTML's autocomplete words this fill knows, and the two it reads as one of them.
  const KINDS = new Set([
    "name", "given-name", "additional-name", "family-name", "organization",
    "street-address", "address-line1", "address-line2", "address-level1", "address-level2",
    "postal-code", "country", "email", "tel",
  ]);
  const ALIASES = { "tel-national": "tel", "country-name": "country" };
  // A field marked with one of these is never filled at all.
  const REFUSED = /^(cc-|current-password$|new-password$|one-time-code$|username$)/;
  // A word in the field's own text, first rule first: the more particular words
  // stand above the ones that would also match them ("email address" is an
  // email, "company name" an organization, "address line 2" a second line).
  const WORDS = [
    ["email", /e-?mail|이메일|메일|メール|邮箱|电子邮件|correo/i],
    ["tel", /phone|^tel|mobile|휴대\s*폰|휴대\s*전화|전화|연락처|電話|携帯|电话|手机|tel[eé]fono|m[oó]vil|celular/i],
    ["postal-code", /zip|postal|post.?code|우편\s*번호|郵便番号|邮编|邮政编码|c[oó]digo\s*postal/i],
    ["address-line2", /address.?(line)?.?2|addr2|apt|suite|상세\s*주소|나머지\s*주소|建物名|详细地址|piso|departamento/i],
    ["address-level1", /state|province|^region|시\s*\/?\s*도|都道府県|省份|provincia|estado/i],
    ["address-level2", /city|town|시\s*\/?\s*군\s*\/?\s*구|市区町村|城市|ciudad|localidad|municipio/i],
    ["organization", /company|organi[sz]ation|회사|직장|会社|勤務先|公司|empresa/i],
    ["street-address", /street|address|addr|주소|住所|地址|direcci[oó]n|calle|domicilio/i],
    ["family-name", /last.?name|family.?name|surname|^성$|^姓$|apellido/i],
    ["given-name", /first.?name|given.?name|^名$/i],
    ["name", /^name$|full.?name|your.?name|성명|이름|お名前|氏名|姓名|^nombre/i],
    ["country", /country|국가|나라|^国$|国家|pa[ií]s/i],
  ];
  const TEXTUAL = new Set(["text", "email", "tel", ""]);
  const fillable = (element) => {
    if (!visible(element)) return false;
    if (element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement) return true;
    return element instanceof HTMLInputElement && TEXTUAL.has(String(element.type).toLowerCase());
  };
  const refused = (element) => marks(element).some((mark) => REFUSED.test(mark));
  const kindOf = (element) => {
    const said = marks(element);
    for (const mark of said) {
      const kind = ALIASES[mark] ?? mark;
      if (KINDS.has(kind)) return kind;
    }
    const type = String(element.type || "").toLowerCase();
    if (type === "email" || type === "tel") return type;
    const words = wordsOf(element);
    for (const [kind, pattern] of WORDS) {
      if (words.some((word) => pattern.test(word))) return kind;
    }
    return null;
  };
  const empty = (element) => element instanceof HTMLSelectElement || element.value === "";

  // Every fillable field, grouped by its form (fields outside any form are one
  // group); the group with the most kinds is the form this page is about.
  const groups = new Map();
  for (const element of document.querySelectorAll("input, textarea, select")) {
    // A card, password or one-time-code field takes neither an address nor a past entry.
    if (!fillable(element) || refused(element)) continue;
    const kind = kindOf(element);
    const group = groups.get(element.form) ?? { kinds: new Set(), fields: [] };
    if (kind) group.kinds.add(kind);
    group.fields.push({ element, kind });
    groups.set(element.form, group);
  }
  let best = null;
  for (const group of groups.values()) {
    if (!best || group.kinds.size > best.kinds.size) best = group;
  }
  const others = (group) => group.fields
    .filter((field) => !field.kind && field.element.name && field.element instanceof HTMLInputElement && empty(field.element));

  if (op === "detect") {
    return {
      kinds: best ? [...best.kinds] : [],
      names: best ? [...new Set(others(best).map((field) => field.element.name))] : [],
    };
  }

  if (op === "fill") {
    const write = (element, value) => {
      if (typeof value !== "string" || value === "" || !empty(element)) return false;
      if (element instanceof HTMLSelectElement) {
        const wanted = value.trim().toLowerCase();
        const option = [...element.options].find((one) =>
          one.value.trim().toLowerCase() === wanted || one.text.trim().toLowerCase() === wanted);
        if (!option || option.selected) return false;
        element.value = option.value;
      } else if (element instanceof HTMLTextAreaElement) {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(element, value);
      } else {
        // A one-line field would drop a street's line break and weld its lines.
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(element, value.replace(/\s*\n\s*/g, " "));
      }
      element.dispatchEvent(new Event("input", { bubbles: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    };
    const values = (args && args.values) || {};
    const entries = (args && args.entries) || {};
    let fields = 0;
    let typed = 0;
    for (const field of best ? best.fields : []) {
      if (field.kind) {
        if (write(field.element, values[field.kind])) fields += 1;
      } else if (field.element.name && write(field.element, entries[field.element.name])) {
        typed += 1;
      }
    }
    return { fields, entries: typed };
  }
  return { kinds: [], names: [] };
}
