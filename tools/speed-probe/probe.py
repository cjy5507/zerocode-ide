#!/usr/bin/env python3
"""Same-device ABBA probe. Outputs only this fixture's data; never enumerates devices.

Requires an explicitly supplied helper, dedicated device, oracle and output path.
The oracle scores observations; it never chooses the next action.
"""
import argparse
import base64
import io
import json
import math
import os
from pathlib import Path
import selectors
import subprocess
import time

BOARD = json.loads(Path(__file__).with_name("board.json").read_text())
PALETTE = BOARD["colours"]
CENTRES = [(BOARD["x0"] + (i % BOARD["columns"]) * BOARD["dx"],
            BOARD["y0"] + (i // BOARD["columns"]) * BOARD["dy"])
           for i in range(BOARD["rows"] * BOARD["columns"])]


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def order(blocks):
    return ["tap", "touch", "touch", "tap"] * blocks


def classify_board(image):
    image = image.convert("RGB")
    result = []
    for x, y in CENTRES:
        # Avoid the white digit; sample the coloured left half of each tile.
        p = image.getpixel((int((x - BOARD["sampleOffset"]) * image.width), int(y * image.height)))
        distances = [sum((a - b) ** 2 for a, b in zip(p, c)) for c in PALETTE]
        result.append(distances.index(min(distances)) if min(distances) < BOARD["colourDistanceSquared"] else None)
    return result


class Lines:
    def __init__(self, argv):
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.seq = 0

    def line(self, text):
        self.p.stdin.write(text + "\n")
        self.p.stdin.flush()
        with selectors.DefaultSelector() as sel:
            sel.register(self.p.stdout, selectors.EVENT_READ)
            if not sel.select(15):
                raise TimeoutError("probe helper timed out")
        result = json.loads(self.p.stdout.readline())
        if result.get("ok") is False or "error" in result and result["error"]:
            raise RuntimeError("probe helper refused request")
        return result

    def ask(self, kind, **kw):
        self.seq += 1
        return self.line(json.dumps(dict(id=self.seq, kind=kind, **kw)))

    def close(self):
        self.p.stdin.close()
        try:
            self.p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.p.terminate()
            self.p.wait(timeout=3)


def timed(fn):
    start = time.perf_counter_ns()
    result = fn()
    return result, (time.perf_counter_ns() - start) / 1e6


def run(args):
    from PIL import Image
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    oracle = Path(args.oracle)
    helper = Lines([args.helper, args.device])
    ocr = Lines([args.ocr])
    rows = []
    def record(kind, **values):
        row = dict(kind=kind, load=os.getloadavg()[0], **values)
        rows.append(row)
        with (out / "ios.jsonl").open("a") as f:
            f.write(json.dumps(row) + "\n")
    def frame():
        answer = helper.ask("frame", longEdge=960, quality=.9)
        return Image.open(io.BytesIO(base64.b64decode(answer["data"])))
    try:
        helper.ask("ping")
        for index in range(args.samples):
            truth = json.loads(oracle.read_text())
            tree, ax_ms = timed(lambda: helper.ask("ax"))
            shallow, walk_ms = timed(lambda: helper.ask("walk"))
            image, frame_ms = timed(frame)
            board, pixel_ms = timed(lambda: classify_board(image))
            shot = out / "fixture.png"
            _, encode_ms = timed(lambda: image.save(shot))
            words, ocr_wall = timed(lambda: ocr.line(str(shot)))
            ocr_cells = [None] * len(CENTRES)
            for word in words.get("words", []):
                if word["text"] not in ("1", "2", "3", "4"):
                    continue
                near = min(range(len(CENTRES)), key=lambda i: (word["x"]-CENTRES[i][0])**2 + (word["y"]-CENTRES[i][1])**2)
                ocr_cells[near] = int(word["text"]) - 1
            _, rotate_ms = timed(lambda: subprocess.run(["sips", "-r", "270", str(shot), "--out", str(out / "rotated.png")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True))
            _, base64_ms = timed(lambda: base64.b64encode(shot.read_bytes()))
            record("state", index=index, ax_ms=ax_ms, shallow_ms=walk_ms,
                   frame_ms=frame_ms, decode_classify_ms=pixel_ms, png_encode_ms=encode_ms,
                   ocr_ms=words.get("ms"), ocr_wall_ms=ocr_wall, rotate_ms=rotate_ms,
                   base64_ms=base64_ms, correct=sum(a == b for a,b in zip(board,truth["cells"])),
                   ocr_correct=sum(a == b for a,b in zip(ocr_cells,truth["cells"])),
                   tree=json.loads(tree["data"]), ocr_cells=ocr_cells, board=board)
            helper.ask("tap", x=.17, y=.28)
            time.sleep(.12)
        # Both arms share capture, decoder and policy. Only the hand changes.
        for index, arm in enumerate(order(args.blocks)):
            before = json.loads(oracle.read_text())
            start = time.perf_counter_ns()
            image, frame_ms = timed(frame)
            board, pixel_ms = timed(lambda: classify_board(image))
            if None in board:
                raise RuntimeError("pixel policy has no complete board")
            target = board.index(0)
            x, y = CENTRES[target]
            if arm == "tap":
                _, hand_ms = timed(lambda: helper.ask("tap", x=x, y=y))
            else:
                def touch():
                    helper.ask("touch", phase="begin", x=x, y=y)
                    helper.ask("touch", phase="end", x=x, y=y)
                _, hand_ms = timed(touch)
            receipt_ms = (time.perf_counter_ns() - start) / 1e6
            # Completion means both an accepted touch and a changed rendered board.
            polls = 0
            after = before
            visible = False
            while (time.perf_counter_ns() - start) / 1e9 < 2:
                next_board = classify_board(frame())
                after = json.loads(oracle.read_text())
                polls += 1
                if after["turn"] == before["turn"] + 1 and next_board == after["cells"]:
                    visible = True
                    break
            elapsed_ms = (time.perf_counter_ns() - start) / 1e6
            record("reflex", index=index, arm=arm, frame_ms=frame_ms, pixel_ms=pixel_ms,
                   hand_ms=hand_ms, receipt_ms=receipt_ms, visible_ms=elapsed_ms,
                   success=visible, polls=polls, turns=after["turn"]-before["turn"])
    finally:
        helper.close()
        ocr.close()
    print(json.dumps({"rows": len(rows), "output": str(out)}))


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    for name in ("helper", "device", "oracle", "ocr", "output"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--samples", type=int, default=12)
    p.add_argument("--blocks", type=int, default=10)
    run(p.parse_args())
