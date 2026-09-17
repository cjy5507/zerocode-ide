#!/bin/sh
# Looks-only smoke of the desktop operator through the pane's own shim
# (docs/design/computer-use-full-operator.md §5): no input is injected, so it
# is safe to run while a person is at the keyboard. Every answer is --json and
# is checked for `ok` and the field the verb promises. Exit 0 only when every
# look answered.
set -u
fail=0
check() {
  name=$1; shift
  out=$(zerocode-computer "$@" --json 2>&1)
  if printf '%s' "$out" | python3 -c '
import json, sys
name = sys.argv[1]; field = sys.argv[2]
line = next((l for l in sys.stdin.read().splitlines() if l.startswith("{")), "")
try:
    body = json.loads(line)
except Exception:
    print(f"{name}: no envelope"); sys.exit(1)
if not body.get("ok"):
    print(name + ": refused " + str(body.get("error"))); sys.exit(1)
result = body.get("result", {})
if field and field not in result:
    print(f"{name}: result lacks {field!r}: {list(result)[:8]}"); sys.exit(1)
print(f"{name}: ok" + (f" ({field} present)" if field else ""))
' "$name" "${FIELD:-}"; then :; else fail=1; fi
}
FIELD=platform check capabilities capabilities
FIELD=displays check displays displays
FIELD=screenshot check screenshot screenshot
FIELD=x check cursor-position cursor-position
FIELD=text check read-ocr read --ocr
FIELD=screenshot check observe-1 observe --diff
FIELD=changed check observe-2 observe --diff
FIELD=hotkey check status status
FIELD=dir check evidence evidence --last 5
FIELD=recipes check recipe-list recipe-list
if [ "$fail" -eq 0 ]; then echo "SMOKE GREEN"; else echo "SMOKE RED"; fi
exit "$fail"
