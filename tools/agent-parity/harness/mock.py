#!/usr/bin/env python3
"""Scripted Anthropic service for the t-2877 agent-parity measurement.

Deterministic lane: no provider spend. Both `zo` and the `claude` CLI point
ANTHROPIC_BASE_URL here; the script that answers is chosen from markers the
prompt carries, so the SAME script drives both subjects.
"""
import http.server, json, os, re, sys, threading, time, uuid

ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")
RUN = os.environ.get("PARITY_RUN", "adhoc")
# Seconds a prelude (tool-less probe) reply is held before it is sent, so a
# run can show what a probe on the parent's critical path costs the first
# token; 0 (the default) answers at once.
PRELUDE_DELAY_S = float(os.environ.get("PARITY_PRELUDE_DELAY_S", "0"))
OUT = os.path.join(ROOT, "runs", RUN)
os.makedirs(OUT, exist_ok=True)
REQLOG = os.path.join(OUT, "requests.jsonl")
LOCK = threading.Lock()
COUNTERS = {}
MODEL = "claude-sonnet-4-6"


def now():
    return time.time()


def log(rec):
    with LOCK:
        with open(REQLOG, "a") as f:
            f.write(json.dumps(rec) + "\n")


# ---------------------------------------------------------------- SSE builders
def sse(event, payload):
    return "event: %s\ndata: %s\n\n" % (event, json.dumps(payload))


def usage(i=12, o=8):
    return {"input_tokens": i, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": o}


def msg_start(mid):
    return sse("message_start", {"type": "message_start", "message": {
        "id": mid, "type": "message", "role": "assistant", "content": [],
        "model": MODEL, "stop_reason": None, "stop_sequence": None,
        "usage": usage(12, 0)}})


def text_sse(mid, text):
    b = msg_start(mid)
    b += sse("content_block_start", {"type": "content_block_start", "index": 0,
                                     "content_block": {"type": "text", "text": ""}})
    b += sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                     "delta": {"type": "text_delta", "text": text}})
    b += sse("content_block_stop", {"type": "content_block_stop", "index": 0})
    b += sse("message_delta", {"type": "message_delta",
                               "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                               "usage": usage()})
    b += sse("message_stop", {"type": "message_stop"})
    return b


def tools_sse(mid, calls):
    """calls: list of (tool_id, name, input_dict)"""
    b = msg_start(mid)
    for idx, (tid, name, inp) in enumerate(calls):
        b += sse("content_block_start", {"type": "content_block_start", "index": idx,
                                         "content_block": {"type": "tool_use", "id": tid,
                                                           "name": name, "input": {}}})
        b += sse("content_block_delta", {"type": "content_block_delta", "index": idx,
                                         "delta": {"type": "input_json_delta",
                                                   "partial_json": json.dumps(inp)}})
        b += sse("content_block_stop", {"type": "content_block_stop", "index": idx})
    b += sse("message_delta", {"type": "message_delta",
                               "delta": {"stop_reason": "tool_use", "stop_sequence": None},
                               "usage": usage()})
    b += sse("message_stop", {"type": "message_stop"})
    return b


def nonstream(mid, text):
    return {"id": mid, "type": "message", "role": "assistant", "model": MODEL,
            "content": [{"type": "text", "text": text}], "stop_reason": "end_turn",
            "stop_sequence": None, "usage": usage()}


# ------------------------------------------------------------- request reading
def user_text(body):
    out = []
    for m in body.get("messages", []):
        c = m.get("content")
        if isinstance(c, str):
            out.append(c)
        elif isinstance(c, list):
            for b in c:
                if b.get("type") == "text":
                    out.append(b.get("text", ""))
                elif b.get("type") == "tool_result":
                    cc = b.get("content")
                    if isinstance(cc, str):
                        out.append(cc)
                    elif isinstance(cc, list):
                        for x in cc:
                            if x.get("type") == "text":
                                out.append(x.get("text", ""))
    sysm = body.get("system")
    if isinstance(sysm, str):
        out.append(sysm)
    elif isinstance(sysm, list):
        for b in sysm:
            if isinstance(b, dict) and b.get("type") == "text":
                out.append(b.get("text", ""))
    return "\n".join(out)


def user_prompt_text(body):
    """Only role=user text blocks — what a person (or a scheduler) submitted.

    Excludes tool_results and system blocks, which echo a scheduled prompt
    back and would score an arming receipt as a firing.
    """
    out = []
    for m in body.get("messages", []):
        if m.get("role") != "user":
            continue
        c = m.get("content")
        if isinstance(c, str):
            out.append(c)
        elif isinstance(c, list):
            for b in c:
                if b.get("type") == "text":
                    out.append(b.get("text", ""))
    return "\n".join(out)


def assistant_tool_uses(body):
    """[(name, id, input)] the assistant has already emitted in this conversation."""
    out = []
    for m in body.get("messages", []):
        if m.get("role") != "assistant":
            continue
        c = m.get("content")
        if isinstance(c, list):
            for b in c:
                if b.get("type") == "tool_use":
                    out.append((b.get("name"), b.get("id"), b.get("input")))
    return out


def tool_results(body):
    """[(tool_use_id, text, is_error)]"""
    out = []
    for m in body.get("messages", []):
        c = m.get("content")
        if isinstance(c, list):
            for b in c:
                if b.get("type") == "tool_result":
                    cc = b.get("content")
                    if isinstance(cc, str):
                        txt = cc
                    elif isinstance(cc, list):
                        txt = "\n".join(x.get("text", json.dumps(x)) for x in cc)
                    else:
                        txt = json.dumps(cc)
                    out.append((b.get("tool_use_id"), txt, bool(b.get("is_error"))))
    return out


def marker(text, key):
    m = re.search(key + r":([A-Za-z0-9_.\-]+)", text)
    return m.group(1) if m else None


def bump(key):
    with LOCK:
        COUNTERS[key] = COUNTERS.get(key, 0) + 1
        return COUNTERS[key]


def peek(key):
    with LOCK:
        return COUNTERS.get(key, 0)


# ------------------------------------------------------------------- scripting
SCRIPTS = {}


def script(name):
    def deco(fn):
        SCRIPTS[name] = fn
        return fn
    return deco


def load_scripts():
    # scripts.py does `from mock import ...`; alias this running module so the
    # decorator registers into THIS SCRIPTS dict, not a second copy.
    sys.modules.setdefault("mock", sys.modules[__name__])
    if sys.modules["mock"] is not sys.modules[__name__]:
        sys.modules["mock"] = sys.modules[__name__]
    import scripts  # noqa: F401  (registers via @script)


def dispatch(body, meta):
    text = user_text(body)
    axis = marker(text, "PARITYAXIS")
    child = marker(text, "PARITYCHILD")
    if child and "." in child:
        axis, child = child.split(".", 1)
    meta["axis"] = axis
    meta["child"] = child
    if not body.get("tools"):
        # prelude / classifier / dreamer probes: keep them silent and cheap.
        meta["kind"] = "prelude"
        return None
    meta["kind"] = "child" if child else "parent"
    fn = SCRIPTS.get(axis)
    if fn is None:
        return text_sse("msg_noscript", "no script for axis %s\n" % axis)
    meta["prompts"] = user_prompt_text(body)
    return fn(body, text, child, meta)


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _send(self, status, ctype, payload, extra=()):
        data = payload.encode()
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)
        self.wfile.flush()

    def do_POST(self):
        t_in = now()
        n = int(self.headers.get("content-length", 0))
        raw = self.rfile.read(n).decode("utf-8", "replace")
        try:
            body = json.loads(raw)
        except Exception:
            self._send(400, "application/json", '{"type":"error"}')
            return
        meta = {"t_in": t_in, "path": self.path, "model": body.get("model"),
                "stream": bool(body.get("stream")),
                "tools": [t.get("name") for t in body.get("tools", [])],
                "n_messages": len(body.get("messages", [])),
                "tool_uses": [u[0] for u in assistant_tool_uses(body)],
                "tool_results": [r[1][:4000] for r in tool_results(body)]}
        try:
            out = dispatch(body, meta)
        except Exception as e:
            meta["error"] = repr(e)
            out = text_sse("msg_err", "script error: %r\n" % e)
        if out is None:
            if PRELUDE_DELAY_S > 0:
                time.sleep(PRELUDE_DELAY_S)
            out = text_sse("msg_prelude", "ok\n")
        if body.get("stream"):
            payload, ctype = out, "text/event-stream"
        else:
            payload, ctype = json.dumps(nonstream("msg_ns", "ok")), "application/json"
        m = re.search(r"PARENT DONE [A-Z]", out or "")
        if m:
            meta["final_marker"] = out[m.start():m.start() + 13].split("\\n")[0].strip()
        meta["t_out"] = now()
        meta["raw_len"] = len(raw)
        meta["raw"] = raw if os.environ.get("PARITY_RAW") else None
        log(meta)
        self._send(200, ctype, payload, extra=[("x-request-id", "req_" + uuid.uuid4().hex[:12])])

    def do_GET(self):
        self._send(404, "application/json", '{"type":"error"}')


def main():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    load_scripts()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    print("PORT=%d" % srv.server_address[1], flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
