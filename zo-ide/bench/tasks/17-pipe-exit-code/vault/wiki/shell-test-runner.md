---
title: "run_tests.sh is the visible suite"
tags: [shell, tests]
---

# run_tests.sh is the visible suite

When `run_tests.sh` fails, fix the script it names. Do not change
`run_tests.sh` itself: it is the check, and a suite you can fix so it passes is
not a check. Run `./run_tests.sh` again after changing anything.
