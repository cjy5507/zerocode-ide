#!/usr/bin/env python3
"""Alternate a minimal cloud image decision and a local pixel decision.

The minimal cloud request deliberately omits agent history and reasoning. It
is a measured lower-bound request shape, not the latency of an agent turn.
"""
import argparse
import base64
import io
import json
import os
from pathlib import Path
import subprocess
import time

from PIL import Image
from probe import BOARD, CENTRES, Lines, classify_board, timed
from web_probe import VALUE


def run(args):
    out=Path(args.output); out.mkdir(parents=True,exist_ok=True)
    assert os.environ.get("ZO_CONFIG_HOME"), "use a temporary configuration home"
    helper=Lines([args.helper,args.device])
    oracle=Path(args.oracle)
    # A person's own key for the value seat's chosen row, from the window's
    # key store — never a login (t-10372).
    table=json.loads((VALUE.FIXTURES/"models.json").read_text())
    key=VALUE.row_key(next(row for row in table["rows"] if row["id"]==table["chosen"]))
    assert key, "put an API key in the window's key store for the chosen row"
    road=VALUE.AnthropicRoad(key)
    headers={"x-api-key":key,"Content-Type":"application/json",
             "Accept":"text/event-stream","anthropic-version":"2023-06-01"}
    def ask(image):
        body={"model":args.model,"stream":True,"max_tokens":16,
              "messages":[{"role":"user","content":[
                  {"type":"image","source":{"type":"base64","media_type":"image/png","data":image}},
                  {"type":"text","text":f"This is a {BOARD['rows']} by {BOARD['columns']} game board. Rows and columns start at 0 from the top left. Return ONLY the index (row*{BOARD['columns']}+column) of the FIRST RED tile in reading order. No explanation."}]}]}
        def pick(frame):
            text=(frame.get("delta") or {}).get("text")
            return (text,"value") if text else None
        return road.session.stream("/v1/messages",headers,body,pick)
    def frame():
        answer=helper.ask("frame",longEdge=960,quality=.9)
        return Image.open(io.BytesIO(base64.b64decode(answer["data"])))
    try:
        helper.ask("ping")
        for index,arm in enumerate(["cloud","local","local","cloud"]*args.blocks):
            before=json.loads(oracle.read_text())
            start=time.perf_counter_ns()
            rotate_ms=encode_ms=model_ms=0
            if arm=="cloud":
                path=out/'shot.png'
                def shot():
                    subprocess.run(['zerocode-emulator','screenshot','--platform','ios','--device',args.device,'--out',str(path),'--json'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True,timeout=30)
                _,read_ms=timed(shot)
                # The portrait fixture needs no rotation. Time the reported
                # sips operation separately; never feed a wrong orientation.
                _,rotate_ms=timed(lambda:subprocess.run(['sips','-r','270',str(path),'--out',str(out/'rotated.png')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True))
                encoded,encode_ms=timed(lambda:base64.b64encode(path.read_bytes()).decode())
                answer,model_ms=timed(lambda:ask(encoded))
                try: target=int(answer[2].strip())
                except ValueError: target=-1
                decision_ms=0
            else:
                image,read_ms=timed(frame)
                board,decision_ms=timed(lambda:classify_board(image))
                target=board.index(0) if 0 in board else -1
            correct=0<=target<len(CENTRES) and target==before['cells'].index(0)
            hand_ms=0; success=False
            if correct:
                x,y=CENTRES[target]
                _,hand_ms=timed(lambda:helper.ask('tap',x=x,y=y))
                until=time.monotonic()+2
                while time.monotonic()<until:
                    seen=classify_board(frame())
                    after=json.loads(oracle.read_text())
                    if after['turn']==before['turn']+1 and seen==after['cells']:
                        success=True;break
            row=dict(index=index,arm=arm,read_ms=read_ms,rotate_ms=rotate_ms,
                     base64_ms=encode_ms,model_ms=model_ms,decision_ms=decision_ms,
                     hand_ms=hand_ms,total_ms=(time.perf_counter_ns()-start)/1e6,
                     correct=correct,success=success,load=os.getloadavg()[0],
                     model=args.model if arm=='cloud' else None)
            with (out/'image-loop.jsonl').open('a') as f:f.write(json.dumps(row)+'\n')
    finally:
        road.close();helper.close()


if __name__=='__main__':
    p=argparse.ArgumentParser()
    for name in ('helper','device','oracle','model','output'):p.add_argument('--'+name,required=True)
    p.add_argument('--blocks',type=int,default=4)
    run(p.parse_args())
