#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

python3 - <<'PY' || { echo "FAIL: settings.ini"; exit 1; }
import configparser
raw = open("settings.ini", "rb").read()
assert raw.count(b"\n") == raw.count(b"\r\n") and b"\r\n" in raw, "line endings changed"
lines = raw.decode().split("\r\n")
at = lines.index("timeout = 45")
assert lines[at + 1].replace(" ", "") == "retries=3", lines[at + 1]
config = configparser.ConfigParser()
config.read_string(raw.decode())
assert config["network"]["timeout"] == "45" and config["network"]["retries"] == "3"
assert config["server"]["port"] == "8080" and config["network"]["proxy"] == "none" and config["logging"]["level"] == "info"
PY
echo PASS
