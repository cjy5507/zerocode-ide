/* A Rust constant's value, read from the source the window is built from —
 * one reader for every node script that must agree with a Rust table (the
 * browser door's page scripts, the long-form bench's fake desk), so none
 * restates a number the Rust owns. Each answers null when the source has no
 * such constant, so a missing one fails its own check, not the whole file. */

/* A `&str` constant's text, raw (`r#"…"#`) or plain. */
export const rustText = (source, name) => {
  const raw = source.match(new RegExp(`const ${name}: &str = r(#+)"([\\s\\S]*?)"\\1;`));
  if (raw) return raw[2];
  const plain = source.match(new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`));
  return plain ? JSON.parse(`"${plain[1]}"`) : null;
};

/* A `&[&str]` or `[&str; N]` constant's words. */
export const rustList = (source, name) => {
  const held = source.match(new RegExp(`const ${name}: (?:&\\[&str\\]|\\[&str; \\d+\\]) = &?\\[([\\s\\S]*?)\\];`));
  return held ? [...held[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((hit) => JSON.parse(`"${hit[1]}"`)) : null;
};

/* A numeric constant (`u64`, `f64`, …), underscores and all. */
export const rustNumber = (source, name) => {
  const held = source.match(new RegExp(`const ${name}: \\w+ = ([\\d_.]+);`));
  return held ? Number(held[1].replace(/_/g, "")) : null;
};
