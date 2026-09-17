#!/bin/bash
# Every cell of the 09-07 table, in the order the document lists them.
set -u
export PARITY_ROOT=${PARITY_ROOT:-/tmp/zo-agent-parity-20260907}
T="python3 $PARITY_ROOT/harness/table.py"
row() { printf '%-10s %-8s ' "$1" "$2"; shift 2; $T "$@" 2>&1 | tail -1; }
row A zo       A  A1-zo A2-zo A3-zo A4-zo A5-zo
row A claude   A  A1-claude A2-claude A3-claude A4-claude A5-claude
row A-pane zo  A  AP1-zo AP2-zo AP3-zo AP4-zo AP5-zo
row A\' zo     A\' A1-zo A2-zo A3-zo A4-zo A5-zo
row A\' claude A\' A1-claude A2-claude A3-claude A4-claude A5-claude
row A\'-pane zo A\' AP1-zo AP2-zo AP3-zo AP4-zo AP5-zo
for ax in K B C D F G I J; do
  row $ax zo     $ax $ax-zo
  row $ax claude $ax $ax-claude
done
for ax in H1 H2 H3; do
  case $ax in H3) r=L ;; *) r=H ;; esac
  row $ax zo     $ax $r-zo
  row $ax claude $ax $r-claude
done
row H1-utc zo H1 H-zo-utc
row M zo M M-zo
