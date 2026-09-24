#!/usr/bin/env python3
"""Measure two-rAF completion on the caller's own fixture tab; keep censoring."""
import argparse
import json
import os
from pathlib import Path
import time

from web_probe import browser, unframe


def run(args):
    rows=[]
    for i in range(args.rounds):
        browser(args.pane, "eval", "(()=>{const p={start:performance.now(),end:0};window.paintProbe=p;requestAnimationFrame(()=>requestAnimationFrame(()=>p.end=performance.now()));return true})()")
        start=time.monotonic()
        while time.monotonic()-start < args.deadline:
            value=unframe(browser(args.pane,"eval","JSON.stringify({start:paintProbe.start,end:paintProbe.end,hidden:document.hidden})"))
            if value["end"]>value["start"]:break
        rows.append(dict(index=i,wall_ms=(time.monotonic()-start)*1000,
                         raf_ms=value["end"]-value["start"] if value["end"] else None,
                         censored=not bool(value["end"]),hidden=value["hidden"],load=os.getloadavg()[0]))
    Path(args.output).write_text(json.dumps(rows,indent=2))


if __name__=="__main__":
    p=argparse.ArgumentParser()
    p.add_argument("--pane",required=True)
    p.add_argument("--output",required=True)
    p.add_argument("--rounds",type=int,default=6)
    p.add_argument("--deadline",type=float,default=7)
    run(p.parse_args())
