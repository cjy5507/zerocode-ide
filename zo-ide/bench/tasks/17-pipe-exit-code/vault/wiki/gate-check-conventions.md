---
title: "gate_check.sh wraps the gate"
tags: [shell, gate]
---

# gate_check.sh wraps the gate

`gate_check.sh` runs `gate.sh` and reports it. `run_tests.sh` checks that
`gate_check.sh` passes on a green gate and fails on a red one, and that it
prints five lines either way.
