#!/bin/sh
# The first-run walk a person takes once per Mac (docs/design/
# computer-use-full-operator.md §8): every permission the helper needs is
# requested through the operator itself — the OS prompt and the exact System
# Settings list open — and this script waits, saying which row to switch on,
# until the helper reports it granted. Then it takes the looks that prove the
# eyes work (screenshot, the desktop's windows, OCR, one observe) and names
# the evidence folder. Looks only: no input is injected. Exit 0 on GREEN.
set -u
POLL_SECS=${ONBOARDING_POLL_SECS:-3}      # between permission polls while the person is in System Settings
MAX_SECS=${ONBOARDING_MAX_SECS:-600}     # the person's budget to switch every row on
PERMISSIONS="accessibility screenshots"  # in the order the helper reports them
fail=0

envelope() { # VERB ARGS… -> the JSON line, or empty (a refusal is on stderr)
  zerocode-computer "$@" --json 2>&1 | while IFS= read -r line; do
    case $line in "{"*) printf '%s\n' "$line"; break;; esac
  done
}
field() { # JSON PYTHON-EXPR(result) -> text
  printf '%s' "$1" | python3 -c '
import json, sys
body = json.loads(sys.stdin.read() or "{}")
if not body.get("ok"):
    print("REFUSED " + str(body.get("error", {}).get("message", body.get("error"))))
    sys.exit(0)
result = body.get("result", {})
print(eval(sys.argv[1]))' "$2" 2>/dev/null
}
missing() { # -> permission ids not granted, space-separated
  body=$(envelope permissions)
  field "$body" '" ".join(p["id"] for p in result.get("permissions", []) if p.get("status") != "granted")'
}

echo "onboarding: permissions"
want=$(missing)
case $want in REFUSED*) echo "  permissions: $want"; echo "ONBOARDING RED"; exit 1;; esac
for id in $want; do
  body=$(envelope permissions --id "$id")
  step=$(field "$body" 'result.get("next_step", "")')
  row=$(field "$body" '" ".join(r["name"] + " (" + r["path"] + ")" for r in result.get("judged_rows", []) if r.get("id") == "'"$id"'")')
  echo "  $id: not granted — the OS prompt and its System Settings list are open"
  [ -z "$step" ] || echo "    $step"
  [ -z "$row" ] || echo "    row: $row"
done
waited=0
while [ -n "$want" ]; do
  if [ "$waited" -ge "$MAX_SECS" ]; then
    echo "  still not granted after ${MAX_SECS}s: $want"
    echo "ONBOARDING RED"; exit 1
  fi
  sleep "$POLL_SECS"; waited=$((waited + 1))   # counts polls; MAX_SECS bounds polls when POLL_SECS < 1
  want=$(missing)
  case $want in REFUSED*) echo "  permissions: $want"; echo "ONBOARDING RED"; exit 1;; esac
  [ -z "$want" ] || echo "  waiting for: $want"
done
echo "  both granted"

look() { # NAME EXPR VERB ARGS…
  name=$1; expr=$2; shift 2
  body=$(envelope "$@")
  said=$(field "$body" "$expr")
  case $said in
    ""|REFUSED*|None) echo "  $name: ${said:-no answer}"; fail=1;;
    *) echo "  $name: $said";;
  esac
}
echo "onboarding: looks"
look screenshot '"{}x{} @{}x".format(result["screenshot"]["width"], result["screenshot"]["height"], result["screenshot"].get("scale", 1))' screenshot
look windows '"{} windows".format(len(result.get("windows", [])))' list-all-windows
look ocr '"{} text lines".format(len(result.get("lines", [])))' read --ocr
look observe '"changed share {}, stuck {}".format(result.get("changedShare"), result.get("stuck"))' observe --diff
look evidence '"{} ({} steps, {} frames)".format(result.get("dir"), result.get("count"), result.get("frames"))' evidence --last 5
if [ "$fail" -eq 0 ]; then echo "ONBOARDING GREEN"; else echo "ONBOARDING RED"; fi
exit "$fail"
