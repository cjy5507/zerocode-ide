---
title: "set -e does not reach inside a command substitution"
source: "zo:session:bench-seed"
tags: [zo, gotcha]
---

# set -e does not reach inside a command substitution

`set -e` will not stop an sh script at a failure inside `$( … )` when the
substitution's value is being assigned. Read the status explicitly.
