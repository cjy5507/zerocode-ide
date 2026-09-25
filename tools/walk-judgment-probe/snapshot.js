// A stand-in for the page's snapshot a look will carry once the browser door
// reads one (t-6721 U4): given the marks answer's own [number, selector]
// pairs, what the page holds beside them in the same pass — its document,
// its form fields, and its containers, images and rows. Measurement only:
// the harness times this call on its own line and never counts it as the
// product's. The page is only read.
(function (pairs) {
  const selectorOf = (element) => {
    if (element.id) return "#" + CSS.escape(element.id);
    const parts = [];
    let node = element;
    while (node && node.nodeType === 1 && parts.length < 20) {
      if (node.id) { parts.unshift("#" + CSS.escape(node.id)); break; }
      let segment = node.tagName.toLowerCase();
      const parent = node.parentElement;
      if (parent) {
        const siblings = [...parent.children].filter((child) => child.tagName === node.tagName);
        if (siblings.length > 1) segment += ":nth-of-type(" + (siblings.indexOf(node) + 1) + ")";
      }
      parts.unshift(segment);
      if (!parent || node === document.body) break;
      node = parent;
    }
    return parts.join(" > ");
  };
  const words = (text) => String(text || "").replace(/\s+/g, " ").trim();
  const fields = [];
  for (const [mark, selector] of pairs) {
    const element = selector && document.querySelector(selector);
    if (!element) continue;
    const tag = element.tagName.toLowerCase();
    const editable = element.isContentEditable;
    if (tag !== "input" && tag !== "textarea" && tag !== "select" && !editable) continue;
    const kind = tag === "input" ? String(element.type || "text").toLowerCase()
      : editable ? "contenteditable" : tag;
    const secret = kind === "password"
      || String(element.autocomplete || "").toLowerCase() === "current-password";
    const label = element.labels && element.labels[0]
      ? words([...element.labels[0].childNodes].filter((node) => node.nodeType === 3).map((node) => node.textContent).join(" "))
      : words(element.getAttribute("aria-label"));
    const around = element.closest("form, section, fieldset, main") || document.body;
    const heading = around.querySelector("h1, h2, legend") || document.querySelector("h1");
    fields.push({ mark, kind, secret, label,
      placeholder: words(element.getAttribute("placeholder")),
      near: heading ? words(heading.textContent) : "",
      value: secret ? "" : String(element.value || "") });
  }
  const lists = [...document.querySelectorAll("[role=list]")];
  const containers = lists.map((list) => ({
    label: words(list.getAttribute("aria-label")), role: "list",
    count: list.querySelectorAll("[role=listitem]").length, selector: selectorOf(list) }));
  const images = [...document.querySelectorAll("img")].map((image) => ({
    alt: words(image.alt), width: image.width, height: image.height, selector: selectorOf(image) }));
  const rows = [...document.querySelectorAll("[role=listitem]")].map((row) => ({
    text: words(row.textContent), selector: selectorOf(row) }));
  return JSON.stringify({ documentEpoch: String(performance.timeOrigin), fields, containers, images, rows });
})
