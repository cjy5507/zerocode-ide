---
title: "tail and head trim what a check prints"
tags: [shell, gate]
---

# tail and head trim what a check prints

`tail -5` prints the last five lines and `head -5` the first. `gate_check.sh`
uses one of them to keep its output to five lines while `run_tests.sh` checks
that it passes. Both are filters: they read standard input and write standard
output, and changing which one you use changes only what is shown.
