---
title: "Capture, read the status, then print"
source: "zo:session:bench-seed"
tags: [zo, workflow]
---

# Capture, read the status, then print

In an sh script: assign the output, keep `$?` on the next line, and only then
format what you show. Any order but that one loses the status in the
formatting.
