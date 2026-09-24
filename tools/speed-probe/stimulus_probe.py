#!/usr/bin/env python3
"""React to an independently ticking fixture; policy never reads the oracle."""
import argparse
import base64
import io
import json
import os
from pathlib import Path
import time

from PIL import Image
from probe import CENTRES, Lines, classify_board


def run(args):
    helper=Lines([args.helper,args.device])
    start=time.monotonic();last=None;actions=0;frames=0;loads=[]
    try:
        while time.monotonic()-start < args.seconds:
            answer=helper.ask("frame",longEdge=960,quality=.9)
            board=classify_board(Image.open(io.BytesIO(base64.b64decode(answer['data']))))
            frames+=1
            if None in board:continue
            if last is not None and board!=last:
                x,y=CENTRES[board.index(0)]
                helper.ask("touch",phase="begin",x=x,y=y)
                helper.ask("touch",phase="end",x=x,y=y)
                actions+=1;loads.append(os.getloadavg()[0])
            last=board
    finally:helper.close()
    elapsed=time.monotonic()-start
    receipts=[json.loads(x) for x in Path(args.receipts).read_text().splitlines()]
    Path(args.output).write_text(json.dumps(dict(elapsed_ms=elapsed*1000,actions=actions,
        frames=frames,apm=actions*60/elapsed,receipts=receipts,load=loads),indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser()
    for name in ('helper','device','receipts','output'):p.add_argument('--'+name,required=True)
    p.add_argument('--seconds',type=float,default=10)
    run(p.parse_args())
