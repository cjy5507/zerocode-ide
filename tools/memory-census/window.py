#!/usr/bin/env python3
"""Follow a running window's memory over hours, the main process and each helper apart (t-80604).

usage: window.py --out <fresh folder> [--pid <zerocode-shell pid>] [--minutes 120]
                 [--interval-s 60] [--vmmap-every 5] [--heap-at 2,30,120]
                 [--label before|after]

`grid.py` reads a test process it started; this script reads a window that is
already open — the person's own, or one started for the scenario — and never
starts or stops one. Every `--interval-s` it writes one JSON line to
`samples.jsonl`:

  ps rss             the main process (`zerocode-shell`) and every child whose
                     command is a `ZeroCode Helper` (renderer, gpu-process and
                     each utility service by its sub-type); the pane programs
                     the window runs (agent CLIs, shells) are counted apart,
                     never inside the window's sum
  footprint          physical footprint, dirty and swapped bytes, and the
                     MALLOC share, for the main process and each helper
                     (`footprint_of`, the same reading grid.py takes); with
                     the peak, the graphics share and the largest categories
                     for the main process and each WebKit service
  webkit             the window's own UI is a WKWebView, so its WebContent,
                     GPU and Networking services are launchd's children, not
                     the window's: they are found by the process responsible
                     for them (`responsibility_get_pid_responsible_for_pid`)
                     and read like a helper, WebContent and GPU also by
                     vmmap -summary on the vmmap beat
  vmmap -summary     every `--vmmap-every` samples, the main process only: the
                     MALLOC zones and the CoreGraphics, IOKit and WebKit
                     regions, with the raw text kept beside the line
  heap --showSizes   only at the minutes `--heap-at` names (the main process):
                     the size histogram says which structure grew; it pauses
                     the process for a moment, so it is never taken each minute
  machine            one-minute load against the core count, power source

Nothing it reads holds what a pane or a page shows: sizes and region names only.
`--minutes 0` takes one sample and stops. `report` prints the table:

  window.py report <folder> [<folder after>]
"""
import argparse
import ctypes
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from grid import footprint_of, heap_of  # noqa: E402

MIB = 1024 * 1024
MAIN = "zerocode-shell"
HELPER = "ZeroCode Helper"
WEBKIT = re.compile(r"/com\.apple\.WebKit\.(\w+)\.xpc/")
# The categories a footprint reading names the largest of, per process.
TOP_CATEGORIES = 6
# The vmmap -summary rows worth a column: the allocator's zones and the
# graphics and web regions the task names. Each is summed over its rows.
# Chromium tags the pages it maps itself: 253 is PartitionAlloc, 254 Chromium,
# 255 V8 (`PageTag`, partition_alloc page_allocator_constants.h). The window's
# browser process runs PartitionAlloc as malloc, so tag 253 holds every malloc
# of the main process — the Rust side's history and ledger included — and the
# MALLOC zones are nearly empty there (first reading, 2026-10-29: tag 253
# 484 MiB dirty against 1.9 MiB of MALLOC). WebKit names its own regions (an
# artifact's WKWebView lives in this process).
REGION_GROUPS = {
    "malloc": re.compile(r"^MALLOC(?! ZONE)"),
    "partition_alloc": re.compile(r"^Memory Tag 253(?! \(reserved\))"),
    "chromium_v8": re.compile(r"^Memory Tag 25[45](?! \(reserved\))"),
    "vm_allocate": re.compile(r"^VM_ALLOCATE(?! \(reserved\))"),
    "coregraphics": re.compile(r"^CG\w*|^CoreGraphics|^CoreAnimation|^CoreUI"),
    "iokit": re.compile(r"^IOKit|^IOSurface|^IOAccelerator"),
    "webkit": re.compile(r"^WebKit|^JS |^JavaScript"),
    # IOSurfaces a process owns and has no mapping of: a WebContent's layer
    # backing stores and canvases, a GPU process's textures and targets.
    "graphics": re.compile(r"^owned unmapped \(graphics\)"),
}
# "REGION TYPE   VIRTUAL  RESIDENT  DIRTY  SWAPPED ..." — sizes like 12.5M, 640K, 1.2G.
VMMAP_ROW = re.compile(r"^(\S.*?)\s{2,}(\d[\d.]*[KMGB])\s+(\d[\d.]*[KMGB])\s+(\d[\d.]*[KMGB])\s+(\d[\d.]*[KMGB])")
UNIT = {"B": 1, "K": 1024, "M": MIB, "G": 1024 * MIB}


def size_of(text):
    return float(text[:-1]) * UNIT[text[-1]]


def processes():
    """Every process: pid, parent, resident bytes, elapsed and command line."""
    said = subprocess.run(["ps", "-axo", "pid=,ppid=,rss=,args="], check=True,
                          capture_output=True, text=True).stdout
    rows = []
    for line in said.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            rows.append({"pid": int(parts[0]), "ppid": int(parts[1]),
                         "rss": int(parts[2]) * 1024, "args": parts[3]})
    return rows


def find_window(rows, pid):
    if pid:
        return next((row for row in rows if row["pid"] == pid), None)
    mains = [row for row in rows if row["args"].split(" ")[0].endswith("/" + MAIN)]
    if len(mains) != 1:
        raise SystemExit(f"{len(mains)} windows open; name one with --pid")
    return mains[0]


def responsible_for(pid):
    """The process macOS holds responsible for `pid` — the app for its XPC services."""
    try:
        ask = ctypes.CDLL("/usr/lib/libSystem.B.dylib").responsibility_get_pid_responsible_for_pid
    except (OSError, AttributeError):
        return None
    ask.restype = ctypes.c_int
    ask.argtypes = [ctypes.c_int]
    return ask(pid)


def webkit_services(rows, window_pid):
    """The WebKit services working for the window: WebContent, GPU, Networking."""
    found = []
    for row in rows:
        kind = WEBKIT.search(row["args"])
        if kind and responsible_for(row["pid"]) == window_pid:
            found.append({**row, "kind": kind.group(1)})
    return found


def footprint_detail(pid, path):
    """`footprint_of`, and from the same reading the peak, the graphics share
    and the largest categories by dirty bytes."""
    reading = footprint_of(pid, path)
    with open(path) as held:
        process = json.load(held)["processes"][0]
    categories = process.get("categories", {})
    reading["peak"] = process.get("auxiliary", {}).get("phys_footprint_peak")
    reading["graphics_dirty"] = sum(one.get("dirty", 0) for name, one in categories.items()
                                    if "(graphics)" in name)
    reading["reclaimable"] = sum(one.get("reclaimable", 0) for one in categories.values())
    largest = sorted(categories.items(), key=lambda item: -item[1].get("dirty", 0))[:TOP_CATEGORIES]
    reading["largest"] = {name: {"dirty": one.get("dirty", 0), "reclaimable": one.get("reclaimable", 0)}
                          for name, one in largest}
    return reading


def helper_kind(args):
    kind = re.search(r"--type=(\S+)", args)
    sub = re.search(r"--utility-sub-type=([\w.]+)", args)
    if not kind:
        return "helper"
    if sub:
        return sub.group(1).split(".")[0]
    return kind.group(1)


def vmmap_groups(pid, path):
    said = subprocess.run(["vmmap", "-summary", str(pid)], capture_output=True, text=True).stdout
    with open(path, "w") as held:
        held.write(said)
    groups = {name: {"resident": 0, "dirty": 0, "swapped": 0} for name in REGION_GROUPS}
    footprint = re.search(r"^Physical footprint:\s+(\d[\d.]*[KMGB])", said, re.M)
    for line in said.splitlines():
        # The region table ends at its TOTAL; the MALLOC ZONE table after it
        # names WebKit's zones again and would count them twice.
        if line.startswith("TOTAL"):
            break
        row = VMMAP_ROW.match(line)
        if not row:
            continue
        for name, pattern in REGION_GROUPS.items():
            if pattern.match(row.group(1)):
                groups[name]["resident"] += size_of(row.group(3))
                groups[name]["dirty"] += size_of(row.group(4))
                groups[name]["swapped"] += size_of(row.group(5))
                break
    return {"physical_footprint": size_of(footprint.group(1)) if footprint else None, **groups}


def machine():
    load = os.getloadavg()[0]
    power = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True).stdout
    source = re.search(r"from '([^']+)'", power)
    return {"load1": round(load, 2), "cores": os.cpu_count(),
            "power": source.group(1) if source else None}


def sample(out, window_pid, index, minute, vmmap_every, heap_at):
    rows = processes()
    window = find_window(rows, window_pid)
    if window is None:
        return None
    children = [row for row in rows if row["ppid"] == window["pid"]]
    helpers = [row for row in children if HELPER in row["args"]]
    programs = [row for row in children if HELPER not in row["args"]]
    line = {"at_ms": int(time.time() * 1000), "minute": round(minute, 2), "sample": index,
            "machine": machine(),
            "main": {"pid": window["pid"], "rss": window["rss"],
                     "footprint": footprint_detail(window["pid"], os.path.join(out, "footprint.main.json"))},
            "helpers": [], "webkit": [],
            "programs": {"count": len(programs), "rss": sum(row["rss"] for row in programs)}}
    for row in helpers:
        try:
            reading = footprint_of(row["pid"], os.path.join(out, "footprint.helper.json"))
        except (subprocess.CalledProcessError, KeyError, IndexError):
            reading = None  # A helper that ended between ps and footprint.
        line["helpers"].append({"pid": row["pid"], "kind": helper_kind(row["args"]),
                                "rss": row["rss"], "footprint": reading})
    for row in webkit_services(rows, window["pid"]):
        try:
            reading = footprint_detail(row["pid"], os.path.join(out, f"footprint.webkit-{row['kind']}.json"))
        except (subprocess.CalledProcessError, KeyError, IndexError):
            reading = None
        service = {"pid": row["pid"], "kind": row["kind"], "rss": row["rss"], "footprint": reading}
        if index % vmmap_every == 0 and row["kind"] in ("WebContent", "GPU"):
            service["vmmap"] = vmmap_groups(
                row["pid"], os.path.join(out, f"vmmap.webkit-{row['kind']}-{row['pid']}.{index:04d}.txt"))
        line["webkit"].append(service)
    if index % vmmap_every == 0:
        line["main"]["vmmap"] = vmmap_groups(window["pid"], os.path.join(out, f"vmmap.{index:04d}.txt"))
    due = [at for at in heap_at if at <= minute and at not in sample.heaps_taken]
    if due:
        sample.heaps_taken.update(due)
        line["main"]["heap"] = heap_of(window["pid"], os.path.join(out, f"heap.{index:04d}.txt"))
    with open(os.path.join(out, "samples.jsonl"), "a") as held:
        held.write(json.dumps(line) + "\n")
    return line


sample.heaps_taken = set()


def follow(asked):
    if not os.path.isabs(asked.out) or os.path.exists(asked.out):
        raise SystemExit("--out must be an absolute path that does not exist yet")
    os.makedirs(asked.out)
    heap_at = [float(at) for at in asked.heap_at.split(",") if at]
    window_pid = asked.pid or find_window(processes(), None)["pid"]
    with open(os.path.join(asked.out, "scenario.json"), "w") as held:
        json.dump({"label": asked.label, "pid": window_pid, "minutes": asked.minutes,
                   "interval_s": asked.interval_s, "note": asked.note}, held, indent=1)
    began = time.monotonic()
    index = 0
    while True:
        minute = (time.monotonic() - began) / 60
        line = sample(asked.out, window_pid, index, minute, asked.vmmap_every, heap_at)
        if line is None:
            print(f"window {window_pid} ended at minute {minute:.1f}", file=sys.stderr)
            return 1
        print(summary_row(line), flush=True)
        index += 1
        if minute >= asked.minutes:
            return 0
        time.sleep(max(0.0, began + index * asked.interval_s - time.monotonic()))


def helpers_mib(line, field):
    total = 0
    for helper in line["helpers"]:
        value = helper["rss"] if field == "rss" else (helper["footprint"] or {}).get("footprint", 0)
        total += value
    return total / MIB


def webkit_mib(line, kind, field):
    total = 0
    for service in line.get("webkit", []):
        if service["kind"] == kind:
            total += (service["footprint"] or {}).get(field) or 0
    return total / MIB


def summary_row(line):
    main = line["main"]
    return (f"{line['minute']:>7.1f} {main['rss'] / MIB:>9.1f} {main['footprint']['footprint'] / MIB:>9.1f} "
            f"{main['footprint']['malloc_dirty_and_swapped'] / MIB:>9.1f} {len(line['helpers']):>3} "
            f"{helpers_mib(line, 'rss'):>9.1f} {helpers_mib(line, 'footprint'):>9.1f} "
            f"{webkit_mib(line, 'WebContent', 'footprint'):>9.1f} "
            f"{webkit_mib(line, 'WebContent', 'graphics_dirty'):>9.1f} "
            f"{webkit_mib(line, 'GPU', 'footprint'):>9.1f} "
            f"{line['machine']['load1']:>5}")


HEADER = (f"{'minute':>7} {'main RSS':>9} {'main fp':>9} {'malloc':>9} {'hlp':>3} "
          f"{'hlp RSS':>9} {'hlp fp':>9} {'ui fp':>9} {'ui gfx':>9} {'wk gpu':>9} {'load':>5}   (MiB)")


def vmmap_dirty(regions):
    return " ".join(f"{name}={regions.get(name, {}).get('dirty', 0) / MIB:.1f}" for name in REGION_GROUPS)


def report(folders):
    for folder in folders:
        with open(os.path.join(folder, "scenario.json")) as held:
            print(f"# {folder} {json.load(held)}")
        print(HEADER)
        with open(os.path.join(folder, "samples.jsonl")) as held:
            lines = [json.loads(one) for one in held if one.strip()]
        marks = {0, len(lines) - 1}
        marks.update(at for at, line in enumerate(lines) if "vmmap" in line["main"] or "heap" in line["main"])
        for at in sorted(marks):
            print(summary_row(lines[at]))
            # A folder taken before a group existed has no column for it.
            regions = lines[at]["main"].get("vmmap")
            if regions:
                print("        vmmap dirty MiB: " + vmmap_dirty(regions))
            for service in lines[at].get("webkit", []):
                if "vmmap" in service:
                    print(f"        {service['kind']} vmmap dirty MiB: " + vmmap_dirty(service["vmmap"]))
    return 0


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "report":
        return report(sys.argv[2:])
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True)
    parser.add_argument("--pid", type=int)
    parser.add_argument("--minutes", type=float, default=120)
    parser.add_argument("--interval-s", type=float, default=60)
    parser.add_argument("--vmmap-every", type=int, default=5)
    parser.add_argument("--heap-at", default="2,30,120")
    parser.add_argument("--label", default="")
    parser.add_argument("--note", default="")
    asked = parser.parse_args()
    print(HEADER, flush=True)
    return follow(asked)


if __name__ == "__main__":
    sys.exit(main())
