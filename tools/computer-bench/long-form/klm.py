#!/usr/bin/env python3
"""tools/computer-bench/long-form/klm.py — how long a person takes to fill the
long-form bench page, by the Keystroke-Level Model (t-37883).

    klm.py [--json]

The yardstick the agent's run is held to. Its assumptions, all written here:

- Operator times are Card, Moran & Newell's (The Psychology of Human-Computer
  Interaction, 1983, table 8.1; Kieras, "Using the Keystroke-Level Model",
  2001): K a keystroke of an average non-secretarial typist (~40 wpm),
  P pointing with the mouse, B a button press or release (a click is BB),
  H moving a hand between keyboard and mouse, M a mental act (find the field,
  read its label, recall or look up the value).
- The person has the page open and the card's facts at hand (the passport on
  the desk); one M per field covers finding and recalling its value.
- Keyboard fields (text, dates, selects, the combobox) are typed and left with
  Tab; a native select is chosen by type-ahead of the shortest prefix that
  names its option alone, the combobox by that prefix and Enter, a masked date
  by its eight digits. Choice controls (radios, yes/no, boxes, the counter,
  the file picker, the submit button) are clicked; each change of device costs
  one H.
- A section the mouse works in below the first screen costs one scroll S.
- System response R: none on the page itself; the file dialog opening and the
  submit answering are named below.
"""
import argparse
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
SPEC = HERE / "spec.json"

# Seconds per operator (Card, Moran & Newell 1983; Kieras 2001).
K = 0.28
P = 1.1
B = 0.1
H = 0.4
M = 1.35
# One wheel flick and finding one's place again: KLM names no scroll operator,
# so this is the bench's own assumption, kept separate so it can be weighed.
S = 1.0
# The system's answers the person waits for: the file dialog opening, picking
# the file in it, and the page answering the submit.
R_FILE_DIALOG = 1.0
R_SUBMIT = 0.5
# The sections the first screen shows; the mouse works below them after a scroll.
FIRST_SCREEN_SECTIONS = 2

KEYBOARD = {"text", "textarea", "email", "tel", "select", "combobox", "date", "datepick"}
MOUSE = {"radio", "yesno", "checkbox", "counter", "file"}
# The digits a masked or native date takes (DDMMYYYY / MMDDYYYY).
DATE_KEYS = 8


def shortest_prefix(label, labels):
    """How many characters of `label` a person types before no other option
    starts the same way (type-ahead, case-blind)."""
    label = label.casefold()
    others = [other.casefold() for other in labels if other.casefold() != label]
    for length in range(1, len(label) + 1):
        if not any(other.startswith(label[:length]) for other in others):
            return length
    return len(label)


def option_label(spec, field, code):
    return next(label for value, label in spec["options"][field["options"]] if value == code)


def field_seconds(spec, field, value):
    """The operators one field costs, without the device change before it."""
    kind = field["kind"]
    if kind in {"text", "textarea", "email", "tel"}:
        return M + K * len(str(value)) + K
    if kind in {"select", "combobox"}:
        labels = [label for _code, label in spec["options"][field["options"]]]
        typed = shortest_prefix(option_label(spec, field, value), labels)
        confirm = K if kind == "combobox" else 0.0
        return M + K * typed + confirm + K
    if kind in {"date", "datepick"}:
        return M + K * DATE_KEYS + K
    if kind in {"radio", "yesno"}:
        return M + P + 2 * B
    if kind == "checkbox":
        return M + (P + 2 * B if value else 0.0)
    if kind == "counter":
        return M + P + 2 * B * (int(value) - field.get("min", 0))
    if kind == "file":
        return M + P + 2 * B + R_FILE_DIALOG + M + P + 4 * B
    raise ValueError(f"no recipe for a {kind} field")


def estimate(spec=None):
    spec = spec or json.loads(SPEC.read_text())
    expected = spec["card"]["expected"]
    rows = []
    hand = "mouse"
    for index, section in enumerate(spec["sections"]):
        scrolled = index >= FIRST_SCREEN_SECTIONS and any(field["kind"] in MOUSE for field in section["fields"])
        if scrolled:
            rows.append({"id": f"scroll:{section['id']}", "seconds": S})
        for field in section["fields"]:
            device = "keyboard" if field["kind"] in KEYBOARD else "mouse"
            switch = H if device != hand else 0.0
            if switch and device == "keyboard":
                # Back on the keyboard after a click: the field is clicked into.
                switch += P + 2 * B
            hand = device
            rows.append({"id": field["id"], "seconds": round(switch + field_seconds(spec, field, expected[field["id"]]), 3)})
    hand_to_mouse = H if hand == "keyboard" else 0.0
    rows.append({"id": "submit", "seconds": round(hand_to_mouse + M + P + 2 * B + R_SUBMIT + M, 3)})
    total = round(sum(row["seconds"] for row in rows), 1)
    return {"total_seconds": total, "rows": rows,
            "operators": {"K": K, "P": P, "B": B, "H": H, "M": M, "S": S,
                          "R_file_dialog": R_FILE_DIALOG, "R_submit": R_SUBMIT}}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    result = estimate()
    if args.json:
        print(json.dumps(result))
        return
    for row in result["rows"]:
        print(f"{row['id']:<22} {row['seconds']:6.2f} s")
    print(f"{'total':<22} {result['total_seconds']:6.1f} s ({result['total_seconds'] / 60:.1f} min)")


if __name__ == "__main__":
    main()
