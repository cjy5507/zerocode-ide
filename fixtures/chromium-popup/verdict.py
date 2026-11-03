#!/usr/bin/env python3
"""What each page really got, as one line per case. Exit 1 if any case fails."""

from __future__ import annotations

import json
import sys

POST = "synthetic_order=fixture-order-7"


def rows(path: str) -> list[dict]:
    try:
        with open(path, encoding="utf-8") as stream:
            return [json.loads(line) for line in stream if line.strip()]
    except FileNotFoundError:
        return []


def of_case(events: list[dict], case: str) -> list[dict]:
    """The events between this case's start and the next one's."""
    inside, found = False, []
    for row in events:
        if row.get("ev") == "case":
            inside = row.get("case") == case
        elif inside:
            found.append(row)
    return found


def main() -> int:
    pages, events = rows(sys.argv[1]), rows(sys.argv[2])
    mode = sys.argv[3] if len(sys.argv) > 3 else "real"
    of = lambda case, via: [row for row in pages if row.get("case") == case and row.get("via") == via]
    target = lambda case: of(case, "target")
    back = lambda case: bool(of(case, "opener-got"))
    posted = lambda case: any(row.get("post") == POST for row in target(case))
    guest = lambda case: any(row.get("guest") == "true" for row in of(case, "target-late"))
    header = lambda case: any(row.get("header") == "worn" for row in target(case))
    no_opener = lambda case: bool(target(case)) and all(row.get("opener") == "false" for row in target(case))
    refused = any(row.get("ev") == "before_browse" and row.get("url", "").startswith("zc-probe-refused:") and not row.get("allowed") for row in events)
    checks = [
        ("window.open('') then location: page shows, opener answers", bool(target("open-empty-location")) and back("open-empty-location")),
        ("window.open('about:blank', name) then document.write: opener answers", back("open-blank-write")),
        ("form target=name POST: body arrives, opener answers", posted("form-post-named") and back("form-post-named")),
        ("form target=_blank POST: body arrives (the web gives such a page no opener)", posted("form-post-blank") and no_opener("form-post-blank")),
        ("redirect then window.opener callback answers", back("opener-callback")),
        ("window.close() from a popup reaches the close callback", any(row.get("ev") == "before_close" and row.get("role") == "popup" for row in events)),
        ("<a target=_blank>: page shows, no opener", no_opener("link-blank")),
        ("window.open(url, _blank, noopener): page shows, no opener", no_opener("open-noopener")),
        ("form rel=noopener POST: body arrives, no opener", posted("form-noopener") and no_opener("form-noopener")),
        ("a window that turns out to be a file saves it, then closes", any(row.get("ev") == "download_done" and row.get("state") == "complete" for row in events) and any(row.get("ev") == "download_popup_closed" for row in events)),
        ("data: popup never runs", not target("refused-data")),
        ("a later move to a refused scheme is stopped by on_before_browse", refused),
        ("guest script stands in a direct popup's first document", guest("link-blank") and guest("form-post-blank")),
        ("user agent is worn by a direct popup's first request", header("opener-callback") and header("link-blank") and header("form-post-blank")),
        ("window.open with no gesture opens nothing: Chromium stops it before on_before_popup",
         any(row.get("case") == "no-gesture" and row.get("via") == "handle" and row.get("value") == "false" for row in pages)
         and not any(row.get("ev") == "before_popup" for row in of_case(events, "no-gesture"))
         and not target("no-gesture")),
    ]
    if mode in ("real", "settled"):
        slow = of_case(events, "slow-download-closed")
        checks += [
            ("closing a window mid-download cuts the file and CEF says nothing: the window must say it failed",
             any(row.get("ev") == "before_close" and row.get("role") == "popup" for row in slow)
             and of("slow-download-closed", "slow-file-cut") != []
             and not any(row.get("ev") == "download_done" for row in slow)),
            ("a window that turns out to be a file has no document when its download begins, and hears no load end",
             any(row.get("ev") == "download_started" and row.get("role") == "popup" and row.get("frame") in ("", "about:blank") for row in slow)
             and not any(row.get("ev") == "load_end" and row.get("role") == "popup" for row in slow)),
        ]
    if mode == "settled":
        refused = of_case(events, "refused-settle")
        checks += [
            ("a refused popup's POST never leaves", not of("refused-settle", "post-arrived") and not target("refused-settle")),
            ("a refused popup closes, and the opener stays", any(row.get("ev") == "settled" and row.get("refused") for row in refused)
             and any(row.get("ev") == "before_close" and row.get("role") == "popup" for row in refused)
             and not any(row.get("ev") == "before_close" and row.get("role") == "opener" for row in refused)),
        ]
    failed = 0
    for name, passed in checks:
        failed += not passed
        print(f"{'ok  ' if passed else 'FAIL'} {name}")
    print(f"{len(checks) - failed} of {len(checks)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
