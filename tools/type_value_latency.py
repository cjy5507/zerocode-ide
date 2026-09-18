#!/usr/bin/env python3
"""Which small model writes the value a browser walk has to type — measured.

A goal-based browser walk that lands on a `type` step needs a STRING, and the
judgment that chose the step cannot write one (Jev answers a closed choice, not
free text). So one model has to, and the only thing that settles which is the
wait it costs: the walk is already inside a stopped step's budget.

This calls the real providers. It asks every candidate the SAME question — the
one in `crates/zerocode-core/fixtures/type-value/question.json`, which the
product's own table reads, so the measurement and the product cannot drift —
and reports each candidate's p10/p50/p90 with the machine's load beside them.

    tools/type_value_latency.py --rounds 7                  # every reachable road
    tools/type_value_latency.py --only haiku,diffusiongemma
    tools/type_value_latency.py --list                      # roads, no calls

THE DISCIPLINE, because one fast reading proves nothing:

1. WARM FIRST. Every candidate gets one uncounted call. A cold TLS handshake
   and a cold serving slot are not what the walk will pay.
2. ONE SESSION. Each candidate keeps ONE kept-alive connection for the whole
   run, which is what a walking process has.
3. ALTERNATE. Candidates go round-robin, one call each per round, so a slow
   minute lands on all of them rather than on whoever went first.
4. AT LEAST FIVE. `--rounds` counted calls each (7 by default), and the
   report is the median and the p10 — never the best reading.
5. LOAD IS PART OF THE READING. The 1-minute load average is stamped on every
   call and the report carries its range; a number taken under a running gate
   is not the same number.
6. ROTATE THE QUESTION. Round n asks case n % len(cases), so no candidate is
   scored on one lucky field, and every candidate is asked each case the same
   number of times.

Credentials are read from the stores zo and the window already keep — the
keychain and `~/.zo/credentials.json` — and are never printed, logged or
written to the ledger.

Rows land in `~/.zo/projects/<project>/state/request-timings/type-value.jsonl`,
beside the request timings the ledger already keeps, so a later reading can be
compared with this one.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import pathlib
import plistlib
import subprocess
import sys
import time
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parent.parent
FIXTURES = REPO / "crates" / "zerocode-core" / "fixtures" / "type-value"
QUESTION_FILE = FIXTURES / "question.json"
MODELS_FILE = FIXTURES / "models.json"
CREDENTIALS = pathlib.Path.home() / ".zo" / "credentials.json"
LEDGER_NAME = "type-value.jsonl"

# The most tokens one value may cost. A field's value is a line; a model that
# wants more than this is answering the wrong question, and cutting it off is
# cheaper than waiting for it.
MAX_TOKENS = 32


# --------------------------------------------------------------------------
# what the machine was doing while it read
# --------------------------------------------------------------------------


def load_now() -> float:
    """The 1-minute load average, or -1 where the platform has none."""
    try:
        return round(os.getloadavg()[0], 2)
    except (OSError, AttributeError):
        return -1.0


# --------------------------------------------------------------------------
# credentials — read, never printed
# --------------------------------------------------------------------------


def credentials() -> dict:
    try:
        return json.loads(CREDENTIALS.read_text())
    except (OSError, ValueError):
        return {}


def keychain(service: str) -> str | None:
    try:
        done = subprocess.run(
            ["security", "find-generic-password", "-s", service, "-w"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    key = done.stdout.strip()
    return key or None


def claude_cli_version() -> str:
    try:
        done = subprocess.run(
            ["claude", "--version"], capture_output=True, text=True, timeout=5
        )
        word = done.stdout.split()[0]
        return word if word[0].isdigit() else "2.1.276"
    except (OSError, subprocess.SubprocessError, IndexError):
        return "2.1.276"


# The Antigravity identity the Code Assist backend gates personal access on —
# the same three headers `gemini_code_assist.rs` sends, and the same floor
# version, because a lower one is served a shorter model registry.
ANTIGRAVITY_FLOOR = "2.9.1"
ANTIGRAVITY_API_CLIENT = "google-cloud-sdk vscode_cloudshelleditor/0.1"
CLIENT_METADATA = json.dumps(
    {"ideType": "ANTIGRAVITY", "platform": "DARWIN_ARM64", "pluginType": "GEMINI"}
)


def antigravity_user_agent() -> str:
    def installed() -> str | None:
        for root in ("/Applications", str(pathlib.Path.home() / "Applications")):
            info = pathlib.Path(root) / "Antigravity.app/Contents/Info.plist"
            try:
                version = plistlib.loads(info.read_bytes()).get(
                    "CFBundleShortVersionString"
                )
            except (OSError, ValueError):
                continue
            if version:
                return str(version)
        return None

    def parts(version: str) -> list[int]:
        return [int(p) for p in version.split(".") if p.isdigit()]

    here = installed()
    newest = here if here and parts(here) > parts(ANTIGRAVITY_FLOOR) else ANTIGRAVITY_FLOOR
    return f"antigravity/{newest} darwin/arm64"


# --------------------------------------------------------------------------
# the wire: one kept-alive connection, timed to the first content byte
# --------------------------------------------------------------------------


class Session:
    """One origin, one connection, reopened only when the peer drops it."""

    def __init__(self, base: str, timeout: float = 60.0):
        split = urllib.parse.urlsplit(base)
        self.scheme = split.scheme
        self.host = split.hostname
        self.port = split.port
        self.prefix = split.path.rstrip("/")
        self.timeout = timeout
        self.conn: http.client.HTTPConnection | None = None

    def _connection(self) -> http.client.HTTPConnection:
        if self.conn is None:
            maker = (
                http.client.HTTPSConnection
                if self.scheme == "https"
                else http.client.HTTPConnection
            )
            self.conn = maker(self.host, self.port, timeout=self.timeout)
        return self.conn

    def stream(self, path: str, headers: dict, body: dict, pick):
        """POST and read the stream. Returns (first_ms, done_ms, text, kind).

        `pick(frame)` answers `(text, kind)` for one decoded frame, where kind
        is "value" for the answer's own words and "thought" for a model that
        narrates before it answers. Both are timed: a thought is the wait a
        walk pays too, and a candidate that spends its budget thinking is one
        this seat cannot use.
        """
        payload = json.dumps(body).encode()
        # A kept-alive connection the peer dropped while another candidate was
        # being read is a LOST call, not a slow one: the request never left. It
        # is sent once more on a fresh connection, and only a second failure is
        # a reading. Nothing after the first byte is ever retried — that would
        # hide a real stall behind a second attempt's clock.
        for attempt in (0, 1):
            conn = self._connection()
            began = time.perf_counter()
            try:
                conn.request("POST", self.prefix + path, body=payload, headers=headers)
                answer = conn.getresponse()
                break
            except Exception:
                self.close()
                if attempt:
                    raise
        if answer.status != 200:
            detail = answer.read()[:200]
            self.close()
            raise RuntimeError(f"HTTP {answer.status}: {detail!r}")
        first_ms: float | None = None
        first_kind: str | None = None
        value: list[str] = []
        pending = b""
        while True:
            chunk = answer.read1(65536)
            if not chunk:
                break
            pending += chunk
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                frame = _decode(line.strip())
                if frame is None:
                    continue
                said = pick(frame)
                if not said or not said[0]:
                    continue
                text, kind = said
                if first_ms is None:
                    first_ms, first_kind = (time.perf_counter() - began) * 1000, kind
                if kind == "value":
                    value.append(text)
        done_ms = (time.perf_counter() - began) * 1000
        return first_ms, done_ms, "".join(value), first_kind

    def close(self) -> None:
        if self.conn is not None:
            try:
                self.conn.close()
            except OSError:
                pass
            self.conn = None


def _decode(line: bytes):
    """One SSE or JSON-array line as an object, or None when it carries none."""
    if not line:
        return None
    if line.startswith(b"data:"):
        line = line[5:].strip()
        if line in (b"[DONE]", b""):
            return None
    line = line.lstrip(b"[,").rstrip(b"]")
    try:
        return json.loads(line)
    except ValueError:
        return None


# --------------------------------------------------------------------------
# the roads
# --------------------------------------------------------------------------


def render(question: dict, case: dict) -> str:
    """The state, exactly as the question's table spells it.

    The keys, their labels and the closing line all come from the fixture, so
    this renders the same bytes `zerocode_core::type_value::render` does. A
    probe that asked a differently worded question would be measuring a
    different seat.
    """
    said = {"goal": case["goal"], **case["field"]}
    lines = [f"{row['label']}: {said.get(row['key'], '')}" for row in question["state"]]
    lines.append(question["valueLine"])
    return "\n".join(lines)


class OpenAiCompatRoad:
    """Every gateway that speaks `/chat/completions` — ours and a person's own."""

    def __init__(self, base: str, key: str | None, user_agent: str | None = None):
        self.session = Session(base)
        self.key = key
        self.user_agent = user_agent

    def ask(self, model: str, question: dict, case: dict):
        headers = {"Content-Type": "application/json", "Accept": "text/event-stream"}
        if self.key:
            headers["Authorization"] = f"Bearer {self.key}"
        if self.user_agent:
            headers["User-Agent"] = self.user_agent
        body = {
            "model": model,
            "stream": True,
            "temperature": 0,
            "max_tokens": MAX_TOKENS,
            "messages": [
                {"role": "system", "content": question["instructions"]},
                {"role": "user", "content": render(question, case)},
            ],
        }

        def pick(frame):
            for choice in frame.get("choices") or []:
                delta = choice.get("delta") or {}
                if delta.get("content"):
                    return delta["content"], "value"
                if delta.get("reasoning_content") or delta.get("reasoning"):
                    return delta.get("reasoning_content") or delta.get("reasoning"), "thought"
            return None

        return self.session.stream("/chat/completions", headers, body, pick)

    def close(self):
        self.session.close()


class AnthropicRoad:
    """The subscription road zo and the window already hold a token for."""

    IDENTITY = "You are Claude Code, Anthropic's official CLI for Claude."

    def __init__(self, token: str):
        self.session = Session("https://api.anthropic.com")
        self.token = token
        self.user_agent = f"claude-cli/{claude_cli_version()} (external, cli)"

    def ask(self, model: str, question: dict, case: dict):
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "anthropic-version": "2023-06-01",
            "anthropic-beta": "oauth-2025-04-20",
            "User-Agent": self.user_agent,
        }
        body = {
            "model": model,
            "stream": True,
            "temperature": 0,
            "max_tokens": MAX_TOKENS,
            "system": [
                {"type": "text", "text": self.IDENTITY},
                {"type": "text", "text": question["instructions"]},
            ],
            "messages": [{"role": "user", "content": render(question, case)}],
        }

        def pick(frame):
            if frame.get("type") != "content_block_delta":
                return None
            delta = frame.get("delta") or {}
            if delta.get("text"):
                return delta["text"], "value"
            if delta.get("thinking"):
                return delta["thinking"], "thought"
            return None

        return self.session.stream("/v1/messages", headers, body, pick)

    def close(self):
        self.session.close()


class CodeAssistRoad:
    """zo's Gemini road: Code Assist `streamGenerateContent`, Antigravity identity.

    `thinking` is the `thinkingLevel` the request asks for — the one field that
    separates `gemini-3.8-flash` the name from what the backend serves, which is
    why the roster spells it per row instead of leaving it to a default.
    """

    def __init__(self, token: str, project: str, host: str, thinking: str | None):
        self.session = Session(f"https://{host}")
        self.token = token
        self.project = project
        self.thinking = thinking
        self.user_agent = antigravity_user_agent()

    def ask(self, model: str, question: dict, case: dict):
        generation = {"temperature": 0, "maxOutputTokens": MAX_TOKENS}
        if self.thinking:
            generation["thinkingConfig"] = {
                "thinkingLevel": self.thinking,
                "includeThoughts": True,
            }
        body = {
            "model": model,
            "project": self.project,
            "userAgent": "antigravity",
            "requestId": f"type-value-{int(time.time() * 1000)}",
            "request": {
                "contents": [
                    {"role": "user", "parts": [{"text": render(question, case)}]}
                ],
                "systemInstruction": {"parts": [{"text": question["instructions"]}]},
                "generationConfig": generation,
            },
        }
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "User-Agent": self.user_agent,
            "X-Goog-Api-Client": ANTIGRAVITY_API_CLIENT,
            "Client-Metadata": CLIENT_METADATA,
        }

        def pick(frame):
            answer = frame.get("response") or frame
            for candidate in answer.get("candidates") or []:
                for part in (candidate.get("content") or {}).get("parts") or []:
                    if part.get("text"):
                        return part["text"], ("thought" if part.get("thought") else "value")
            return None

        return self.session.stream(
            "/v1internal:streamGenerateContent?alt=sse", headers, body, pick
        )

    def close(self):
        self.session.close()


def build_road(row: dict, creds: dict):
    """The road a roster row names, or None with the word for why not."""
    kind = row["road"]
    if kind == "anthropic":
        token = (creds.get("oauth") or {}).get("accessToken")
        return (AnthropicRoad(token), None) if token else (None, "no anthropic login")
    if kind == "code-assist":
        oauth = creds.get("google_code_assist_oauth") or {}
        project = (creds.get("google_code_assist_project") or {}).get("project")
        token = oauth.get("accessToken")
        if not token or not project:
            return None, "no google login"
        return CodeAssistRoad(token, project, row["host"], row.get("thinkingLevel")), None
    if kind == "openai-compat":
        key = None
        if row.get("keychainService"):
            key = keychain(row["keychainService"])
        elif row.get("credentialKey"):
            key = (creds.get("openai_compat_api_keys") or {}).get(row["credentialKey"])
        if row.get("needsKey", True) and not key:
            return None, "no key"
        agent = None
        if row.get("clientFingerprint") == "claude-code":
            agent = f"claude-cli/{claude_cli_version()} (external, cli)"
        return OpenAiCompatRoad(row["baseUrl"], key, agent), None
    return None, f"unknown road {kind}"


# --------------------------------------------------------------------------
# reading the answers
# --------------------------------------------------------------------------


def accepted(case: dict, said: str, cap: int) -> bool:
    """Whether the answer is a value this field could take.

    One line, inside the cap, and one of the words the case accepts — a fast
    model that answers the wrong question is not a candidate for this seat.
    """
    value = said.strip().strip('"').strip("'")
    if not value or "\n" in value or len(value) > cap:
        return False
    folded = value.casefold()
    return any(folded == word or folded.startswith(word) for word in case["accepts"])


def percentile(values: list[float], share: float) -> float:
    if not values:
        return float("nan")
    ordered = sorted(values)
    spot = (len(ordered) - 1) * share
    low, high = int(spot), min(int(spot) + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (spot - low)


def project_slug_stem(cwd: pathlib.Path) -> str:
    """The readable half of zo's project slug for `cwd` (`config::project_slug`).

    The other half is a hash of the path under Rust's own hasher, which nothing
    outside Rust can reproduce — so a row is filed by the stem, which is the
    whole path with every character zo replaces replaced.
    """
    sanitized = "".join(
        ch if (ch.isascii() and (ch.isalnum() or ch in "-_.")) else "-" for ch in str(cwd)
    )
    return sanitized[max(0, len(sanitized) - 80) :].strip("-")


def shown(line: dict, key: str) -> str:
    """A cell, with an em dash where a candidate answered too seldom to have one."""
    return "—" if line[key] is None else str(line[key])


def ledger_path() -> pathlib.Path | None:
    """Where this reading is filed: beside the PROJECT's request timings.

    A worktree is not a project — its readings belong with the timings they will
    be read against, which live under the main checkout's slug. Git knows which
    checkout that is (`--git-common-dir`), so a run from any worktree files in
    the same place.
    """
    projects = pathlib.Path.home() / ".zo" / "projects"
    if not projects.is_dir():
        return None
    try:
        common = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            cwd=REPO,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None
    if not common:
        return None
    stem = project_slug_stem(pathlib.Path(common).parent)
    for home in sorted(projects.iterdir()):
        if home.name.startswith(stem + "-") and (home / "state" / "request-timings").is_dir():
            return home / "state" / "request-timings" / LEDGER_NAME
    return None


# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rounds", type=int, default=7, help="counted calls per candidate")
    parser.add_argument("--only", default="", help="comma-separated roster ids")
    parser.add_argument("--list", action="store_true", help="print the roads and stop")
    parser.add_argument("--roster", default=str(MODELS_FILE))
    parser.add_argument("--no-ledger", action="store_true")
    parser.add_argument("--note", default="", help="one word filed with the rows (e.g. a load condition)")
    args = parser.parse_args()

    question = json.loads(QUESTION_FILE.read_text())
    roster = json.loads(pathlib.Path(args.roster).read_text())
    rows = roster["rows"]
    if args.only:
        wanted = {word.strip() for word in args.only.split(",") if word.strip()}
        rows = [row for row in rows if row["id"] in wanted]
    if not rows:
        print("no roster rows selected", file=sys.stderr)
        return 2

    creds = credentials()
    live, missing = [], []
    for row in rows:
        road, why = build_road(row, creds)
        (live if road else missing).append((row, road or why))
    for row, why in missing:
        print(f"{row['id']:26} skipped — {why}")
    if args.list:
        for row, _ in live:
            print(f"{row['id']:26} {row['road']:14} {row['model']}")
        return 0
    if not live:
        return 2

    cases = question["cases"]
    cap = question["valueCharCap"]
    readings: dict[str, list[dict]] = {row["id"]: [] for row, _ in live}

    # 1. warm: one uncounted call each, so no candidate is read cold.
    for row, road in live:
        try:
            road.ask(row["model"], question, cases[0])
        except Exception as err:
            print(f"{row['id']:26} warm-up failed — {str(err)[:110]}")

    # 2..6. round-robin, rotating the case, load stamped on every call.
    for round_no in range(args.rounds):
        case = cases[round_no % len(cases)]
        for row, road in live:
            load = load_now()
            began = time.time()
            try:
                first_ms, done_ms, said, kind = road.ask(row["model"], question, case)
                readings[row["id"]].append(
                    {
                        "case": case["id"],
                        "firstMs": None if first_ms is None else round(first_ms, 1),
                        "doneMs": round(done_ms, 1),
                        "firstKind": kind,
                        "ok": accepted(case, said, cap),
                        "load1": load,
                        "at": round(began, 3),
                    }
                )
            except Exception as err:
                readings[row["id"]].append(
                    {"case": case["id"], "error": str(err)[:140], "load1": load, "at": round(began, 3)}
                )
    for _, road in live:
        road.close()

    # the report
    print()
    print(f"question rubric v{question['rubricVersion']}, {args.rounds} counted rounds, "
          f"{len(cases)} cases rotating, one warm-up each")
    print(f"{'candidate':26}{'n':>3}{'p10':>7}{'p50':>7}{'p90':>7}{'first':>8}{'ok':>6}  load")
    report = []
    for row, _ in live:
        taken = readings[row["id"]]
        good = [r for r in taken if "error" not in r and r["doneMs"] is not None]
        waits = [r["doneMs"] for r in good]
        loads = [r["load1"] for r in taken if r.get("load1", -1) >= 0]
        okays = sum(1 for r in good if r["ok"])
        thoughts = sum(1 for r in good if r["firstKind"] == "thought")
        line = {
            "id": row["id"],
            "model": row["model"],
            "road": row["road"],
            "n": len(good),
            "errors": len(taken) - len(good),
            "p10Ms": round(percentile(waits, 0.1)) if waits else None,
            "p50Ms": round(percentile(waits, 0.5)) if waits else None,
            "p90Ms": round(percentile(waits, 0.9)) if waits else None,
            "firstByteP50Ms": round(percentile([r["firstMs"] for r in good if r["firstMs"]], 0.5))
            if any(r["firstMs"] for r in good)
            else None,
            "accepted": okays,
            "thoughtFirst": thoughts,
            "load1Min": min(loads) if loads else None,
            "load1Max": max(loads) if loads else None,
            "firstError": next((r["error"] for r in taken if "error" in r), None),
        }
        report.append(line)
        print(
            f"{row['id']:26}{line['n']:>3}{shown(line, 'p10Ms'):>7}{shown(line, 'p50Ms'):>7}"
            f"{shown(line, 'p90Ms'):>7}{shown(line, 'firstByteP50Ms'):>8}"
            f"{f'{okays}/{len(good)}':>6}  {line['load1Min']}–{line['load1Max']}"
            + (f"   ({line['errors']} failed: {line['firstError']})" if line["errors"] else "")
            + (f"   [{thoughts} thought first]" if thoughts else "")
        )
    print()
    print("p10/p50/p90 are milliseconds to the WHOLE one-line value; `first` is to the")
    print("first byte of any kind. A candidate whose first byte is a thought is spending")
    print("the walk's budget narrating.")

    if not args.no_ledger:
        path = ledger_path()
        if path:
            stamp = int(time.time())
            with path.open("a") as ledger:
                for line in report:
                    row = {"recorded_at": stamp, "rounds": args.rounds, **line}
                    if args.note:
                        row["note"] = args.note
                    ledger.write(json.dumps(row) + "\n")
                    ledger.flush()
            print(f"\nrows appended to {path}")
        else:
            print("\nno project ledger resolved — rows not filed", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
