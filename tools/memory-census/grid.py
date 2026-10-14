#!/usr/bin/env python3
"""Read the terminal grid's scenarios from outside the process that holds them (t-54317).

usage: grid.py --binary <grid_memory test binary> --out <fresh folder>
               [--scenarios empty,panes-1,...] [--low-spec]

The binary is `cargo test -p zerocode-pty --test grid_memory --no-run`'s
executable. Each scenario runs in a process of its own — a scenario dropped
inside one process leaves pages the allocator keeps, and the next scenario's
footprint would carry them — and is read while it holds its state:

  heap --showSizes   live allocations and their size histogram (the honest
                     instrument for a grid: which size class grew says which
                     structure grew)
  footprint          the physical footprint and its dirty and swapped parts
  vmmap -summary     the same regions as the VM sees them, kept for the record

The resident size alone is never the verdict. `--low-spec` runs the process
under `taskpolicy -b` (efficiency cores, low I/O priority), the profile the
window has to stay usable on. Only processes this script started are read.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

SCENARIOS = ["empty", "panes-1", "panes-10", "panes-30", "scrollback-5000", "scrollback-50000"]
READY_PATIENCE_S = 600
# The heap tool's two summary lines: the size histogram, and the totals.
SIZES = re.compile(r"^All zones: (\d+) nodes malloced - Sizes: (.*)$", re.M)
TOTALS = re.compile(r"^All zones: (\d+) nodes \((\d+) bytes\)", re.M)


def footprint_of(pid, path):
    """The process's footprint, its dirty and swapped bytes, and the malloc share."""
    subprocess.run(["footprint", "-j", path, "-f", "bytes", "--swapped", "-p", str(pid)],
                   check=True, capture_output=True, text=True)
    with open(path) as held:
        process = json.load(held)["processes"][0]
    categories = process.get("categories", {})
    dirty = sum(one.get("dirty", 0) for one in categories.values())
    swapped = sum(one.get("swapped", 0) for one in categories.values())
    malloc = sum(one.get("dirty", 0) + one.get("swapped", 0)
                 for name, one in categories.items() if name.startswith("MALLOC"))
    return {"footprint": process["footprint"], "dirty": dirty, "swapped": swapped,
            "malloc_dirty_and_swapped": malloc}


def heap_of(pid, path):
    """Live malloc nodes and bytes, and the size histogram line as the tool prints it."""
    said = subprocess.run(["heap", "-q", "--showSizes", str(pid)],
                          check=True, capture_output=True, text=True).stdout
    with open(path, "w") as held:
        held.write(said)
    sizes = SIZES.search(said)
    totals = TOTALS.search(said)
    return {
        "nodes": int(totals.group(1)) if totals else None,
        "bytes": int(totals.group(2)) if totals else None,
        "histogram": sizes.group(2).strip() if sizes else None,
    }


def read_scenario(binary, out, name, low_spec):
    ready = os.path.join(out, f"{name}.ready")
    go = os.path.join(out, f"{name}.go")
    env = dict(os.environ, ZEROCODE_GRID_CENSUS=out, ZEROCODE_GRID_CENSUS_SCENARIOS=name)
    prefix = ["taskpolicy", "-b"] if low_spec else []
    began = time.monotonic()
    child = subprocess.Popen(prefix + [binary, "--ignored", "--exact", "census", "--test-threads=1"],
                             env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    while not os.path.exists(ready):
        if child.poll() is not None:
            raise SystemExit(f"{name}: the census process ended before it was ready: {child.stderr.read()}")
        if time.monotonic() - began > READY_PATIENCE_S:
            child.kill()
            raise SystemExit(f"{name}: not ready after {READY_PATIENCE_S} s")
        time.sleep(0.05)
    built_s = time.monotonic() - began
    # The note is written whole before the file is closed; read it once it parses.
    note = None
    while note is None:
        try:
            with open(ready) as held:
                note = json.load(held)
        except (json.JSONDecodeError, OSError):
            time.sleep(0.02)
    pid = note["pid"]
    row = {"scenario": name, "low_spec": low_spec, "built_s": round(built_s, 2), **note}
    row["heap"] = heap_of(pid, os.path.join(out, f"{name}.heap.txt"))
    row["footprint"] = footprint_of(pid, os.path.join(out, f"{name}.footprint.json"))
    with open(os.path.join(out, f"{name}.vmmap.txt"), "w") as held:
        held.write(subprocess.run(["vmmap", "-summary", str(pid)], capture_output=True, text=True).stdout)
    with open(go, "w") as held:
        held.write("read\n")
    child.wait(timeout=120)
    row["exit"] = child.returncode
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--scenarios", default=",".join(SCENARIOS))
    parser.add_argument("--low-spec", action="store_true")
    asked = parser.parse_args()
    if not os.path.isabs(asked.out) or os.path.exists(asked.out):
        raise SystemExit("--out must be an absolute path that does not exist yet")
    os.makedirs(asked.out)
    rows = [read_scenario(asked.binary, asked.out, name, asked.low_spec)
            for name in asked.scenarios.split(",") if name]
    with open(os.path.join(asked.out, "grid-census.json"), "w") as held:
        json.dump(rows, held, indent=1)
    mib = 1024 * 1024
    print(f"{'scenario':<18} {'low':>3} {'alloc MiB':>9} {'heap MiB':>8} {'nodes':>9} "
          f"{'footprint MiB':>13} {'dirty MiB':>9} {'malloc MiB':>10}")
    for row in rows:
        print(f"{row['scenario']:<18} {'yes' if row['low_spec'] else 'no':>3} "
              f"{row['live_bytes'] / mib:>9.1f} {(row['heap']['bytes'] or 0) / mib:>8.1f} "
              f"{row['heap']['nodes'] or 0:>9} {row['footprint']['footprint'] / mib:>13.1f} "
              f"{row['footprint']['dirty'] / mib:>9.1f} "
              f"{row['footprint']['malloc_dirty_and_swapped'] / mib:>10.1f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
