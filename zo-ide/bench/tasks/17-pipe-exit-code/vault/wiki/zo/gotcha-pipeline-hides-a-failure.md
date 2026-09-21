---
title: "A pipeline reports its last command, so a failure behind one reads green"
source: "zo:session:bench-seed"
tags: [zo, gotcha]
---

# A pipeline reports its last command, so a failure behind one reads green

`something | tail -5` exits with tail's status, which is 0 whatever came
before. Capture the output first, read `$?` on its own line, then trim — or the
verdict you report is the trimmer's, not the thing you ran.
