#!/bin/sh
# The project's real gate. It prints a lot and it may fail.
# $1 is "green" or "red"; anything else is green.
i=1
while [ "$i" -le 20 ]; do
  echo "gate line $i"
  i=$((i + 1))
done
if [ "${1:-green}" = "red" ]; then
  echo "gate: FAILED"
  exit 1
fi
echo "gate: ok"
exit 0
