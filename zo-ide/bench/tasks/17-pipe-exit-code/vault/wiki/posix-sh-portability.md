---
title: "sh here means dash, not bash"
tags: [shell, portability]
---

# sh here means dash, not bash

Every `.sh` in this tree runs under `/bin/sh`. Check that each construct passes
under dash before changing a script: no `local`, no `[[`, no arrays.
