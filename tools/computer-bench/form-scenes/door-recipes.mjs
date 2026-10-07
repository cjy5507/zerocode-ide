/* What the computer-use skill teaches an agent to do with its own hands where
 * the door has no verb for it (t-41592) — written once, so the scripted drivers
 * do exactly what the skill says and a test holds the two together
 * (test-drive.mjs reads the skill and finds the recipe in it).
 *
 * A line `끝까지 안 내린 스크롤 상자: <handle> 「…」` names a box with more to
 * read below what it shows (terms to be read to the end before a box turns on):
 * the skill scrolls that box to its end with an `eval`, `String.raw` keeping a
 * handle's backslashes; for a handle with ` >> ` in it the part before is the
 * frame and the part after the box inside it. */

import { FORM_REQUEST } from "../../../ui/tests/browser-scripts.mjs";

/* The recipe's expression for one box's handle. */
export function scrollToEnd(handle) {
  const raw = (text) => `String.raw\`${text}\``;
  if (handle.includes(FORM_REQUEST.frameSeparator)) {
    const [frame, inner] = handle.split(FORM_REQUEST.frameSeparator);
    return `document.querySelector(${raw(frame)}).contentDocument.querySelector(${raw(inner)}).scrollTop = 1e9`;
  }
  return `document.querySelector(${raw(handle)}).scrollTop = 1e9`;
}

/* A button inside a frame is pressed with an `eval` the same way (t-41720): the part of the handle before the separator is the frame, the part after it the
 * button inside the frame — the door's own `click` names a button of the page itself. */
export function pressButton(handle) {
  const raw = (text) => `String.raw\`${text}\``;
  if (handle.includes(FORM_REQUEST.frameSeparator)) {
    const [frame, inner] = handle.split(FORM_REQUEST.frameSeparator);
    return `document.querySelector(${raw(frame)}).contentDocument.querySelector(${raw(inner)}).click()`;
  }
  return `document.querySelector(${raw(handle)}).click()`;
}

