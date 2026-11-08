#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

[ -f textutil.py ] || { echo "FAIL: textutil.py is missing"; exit 1; }
grep -q "textutil" docs_site.py || { echo "FAIL: docs_site.py does not use textutil"; exit 1; }
python3 - <<'PY' || { echo "FAIL: behavior"; exit 1; }
from textutil import slugify
from docs_site import page_url
cases = {"Hello, World!": "hello-world", "  Rust  2024 -- Edition ": "rust-2024-edition", "---": "", "already-a-slug": "already-a-slug", "MiXeD_case99": "mixed-case99"}
for text, want in cases.items():
    got = slugify(text)
    assert got == want, (text, got, want)
assert page_url("guides", "Hello, World!") == "/guides/hello-world/", page_url("guides", "Hello, World!")
PY
echo PASS
