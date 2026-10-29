#!/usr/bin/env python3
"""What a window costs while nobody touches it: CPU and wake-ups of every process in its family (t-82914).

usage: idle.py --out <fresh folder> [--pid <zerocode-shell pid>] [--seconds 60] [--repeat 5]
               [--threads 6] [--label <scene>] [--note <text>]
       idle.py --out <fresh folder> --process <role>=<pid> [--process ...] [...]
       idle.py report <folder> [<folder> ...]

`window.py` follows memory; this reads the quiet cost. Each repeat reads every
process twice, `--seconds` apart, through `proc_pid_rusage` — the kernel's own
counters, readable for the caller's own processes with no sudo, no powermetrics
and nothing paused:

  cpu                 user + system time over the stretch, as a share of one core (%)
  idle wake-ups       times the process brought the package out of idle
                      (ri_pkg_idle_wkups), per second
  interrupt wake-ups  timer and interrupt wake-ups (ri_interrupt_wkups), per second
  energy              the kernel's billed energy (ri_billed_energy), mW
  footprint           phys_footprint at the end of the stretch, and its lifetime peak

for the main process, each Chromium helper by its kind, the WebKit services that
work for the window (found the way window.py finds them), and the pane programs:
every process under the window that is not a helper, with all its descendants,
summed into one row — the agents' own cost, never counted as the window's.
`--process role=pid` reads named processes instead of a window's family (a
harness's own browser processes).

The busiest threads of the main process and of each WebContent are read as well
(`proc_pidinfo` by thread id): the `--threads` largest by CPU over the stretch,
with their names. The thread that wakes is the first lead to the beat that wakes it.

Every repeat writes its conditions beside it: one-minute load against the core
count, power source, Low Power Mode, the CPU speed limit, the window's age and its
pane-program count. A repeat that began with the load over the core count is
flagged `loud` and the report keeps it out of the median.

Nothing it reads holds what a pane or a page shows: counters and thread names only.
"""
import ctypes
import json
import os
import re
import statistics
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from window import HELPER, find_window, helper_kind, machine, processes, webkit_services  # noqa: E402

MIB = 1024 * 1024
# proc_pid_rusage flavour 4: the first that carries billed energy and instructions.
RUSAGE_INFO_V4 = 4
# proc_pidinfo flavours: the thread ids of a task, and one thread's info by id.
PROC_PIDLISTTHREADIDS = 28
PROC_PIDTHREADID64INFO = 15
# Thread ids read per process at most; the window's main process runs a few hundred.
THREAD_ID_ROOM = 4096


class RusageInfoV4(ctypes.Structure):
    _fields_ = [("ri_uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in (
        "ri_user_time", "ri_system_time", "ri_pkg_idle_wkups", "ri_interrupt_wkups", "ri_pageins",
        "ri_wired_size", "ri_resident_size", "ri_phys_footprint", "ri_proc_start_abstime",
        "ri_proc_exit_abstime", "ri_child_user_time", "ri_child_system_time", "ri_child_pkg_idle_wkups",
        "ri_child_interrupt_wkups", "ri_child_pageins", "ri_child_elapsed_abstime", "ri_diskio_bytesread",
        "ri_diskio_byteswritten", "ri_cpu_time_qos_default", "ri_cpu_time_qos_maintenance",
        "ri_cpu_time_qos_background", "ri_cpu_time_qos_utility", "ri_cpu_time_qos_legacy",
        "ri_cpu_time_qos_user_initiated", "ri_cpu_time_qos_user_interactive", "ri_billed_system_time",
        "ri_serviced_system_time", "ri_logical_writes", "ri_lifetime_max_phys_footprint", "ri_instructions",
        "ri_cycles", "ri_billed_energy", "ri_serviced_energy", "ri_interval_max_phys_footprint",
        "ri_runnable_time")]


class ThreadInfo(ctypes.Structure):
    _fields_ = [("pth_user_time", ctypes.c_uint64), ("pth_system_time", ctypes.c_uint64)] + [
        (name, ctypes.c_int32) for name in ("pth_cpu_usage", "pth_policy", "pth_run_state", "pth_flags",
                                            "pth_sleep_time", "pth_curpri", "pth_priority", "pth_maxpriority")
    ] + [("pth_name", ctypes.c_char * 64)]


class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
LIBPROC.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.POINTER(RusageInfoV4)]
LIBPROC.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
LIBSYSTEM = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
LIBSYSTEM.mach_absolute_time.restype = ctypes.c_uint64
_TIMEBASE = Timebase()
LIBSYSTEM.mach_timebase_info(ctypes.byref(_TIMEBASE))


def ticks_ns(ticks):
    """rusage times are mach ticks on Apple silicon (1 tick = 125/3 ns on M-series), nanoseconds on Intel."""
    return ticks * _TIMEBASE.numer / _TIMEBASE.denom


def rusage(pid):
    """The kernel's counters for one process, or None when it is gone or not ours."""
    info = RusageInfoV4()
    if LIBPROC.proc_pid_rusage(pid, RUSAGE_INFO_V4, ctypes.byref(info)) != 0:
        return None
    return {
        "cpu_ns": ticks_ns(info.ri_user_time + info.ri_system_time),
        "idle_wakeups": info.ri_pkg_idle_wkups,
        "interrupt_wakeups": info.ri_interrupt_wkups,
        "energy_nj": info.ri_billed_energy,
        "footprint": info.ri_phys_footprint,
        "peak": info.ri_lifetime_max_phys_footprint,
        "age_s": ticks_ns(LIBSYSTEM.mach_absolute_time() - info.ri_proc_start_abstime) / 1e9,
    }


def threads(pid):
    """Every thread of a process: id → (name, cpu ns). Thread times are nanoseconds."""
    ids = (ctypes.c_uint64 * THREAD_ID_ROOM)()
    got = LIBPROC.proc_pidinfo(pid, PROC_PIDLISTTHREADIDS, 0, ids, ctypes.sizeof(ids))
    found = {}
    for at in range(max(0, got) // ctypes.sizeof(ctypes.c_uint64)):
        info = ThreadInfo()
        if LIBPROC.proc_pidinfo(pid, PROC_PIDTHREADID64INFO, ids[at], ctypes.byref(info), ctypes.sizeof(info)) <= 0:
            continue  # A thread that ended between the list and its read.
        name = info.pth_name.decode("utf-8", "replace") or f"(unnamed {ids[at]})"
        found[ids[at]] = (name, info.pth_user_time + info.pth_system_time)
    return found


def descendants(rows, roots):
    """The pids under `roots`, the roots included."""
    children = {}
    for row in rows:
        children.setdefault(row["ppid"], []).append(row["pid"])
    seen, todo = set(), list(roots)
    while todo:
        pid = todo.pop()
        if pid not in seen:
            seen.add(pid)
            todo.extend(children.get(pid, []))
    return seen


def window_family(window_pid):
    """The window's processes by role. A role names one process, or a sum (`panes`)."""
    rows = processes()
    window = find_window(rows, window_pid)
    if window is None:
        return None
    family = [{"role": "main", "kind": "zerocode-shell", "pids": [window["pid"]]}]
    children = [row for row in rows if row["ppid"] == window["pid"]]
    for row in children:
        if HELPER in row["args"]:
            family.append({"role": "helper", "kind": helper_kind(row["args"]), "pids": [row["pid"]]})
    for row in webkit_services(rows, window["pid"]):
        family.append({"role": "webkit", "kind": row["kind"], "pids": [row["pid"]]})
    programs = [row["pid"] for row in children if HELPER not in row["args"]]
    family.append({"role": "panes", "kind": f"{len(programs)} pane programs, with descendants",
                   "pids": sorted(descendants(rows, programs)), "programs": len(programs)})
    return family


def named_family(pairs):
    family = []
    for pair in pairs:
        role, _, pid = pair.partition("=")
        if not pid.isdigit():
            raise SystemExit(f"--process wants role=pid, not {pair}")
        family.append({"role": role, "kind": role, "pids": [int(pid)]})
    return family


def conditions(family):
    """The machine the stretch ran on, and the window it read."""
    said = machine()
    settings = subprocess.run(["pmset", "-g"], capture_output=True, text=True).stdout
    therm = subprocess.run(["pmset", "-g", "therm"], capture_output=True, text=True).stdout
    limit = re.search(r"CPU_Speed_Limit\s*=\s*(\d+)", therm)
    main = next((one for one in family if one["role"] == "main"), None)
    reading = rusage(main["pids"][0]) if main else None
    panes = next((one for one in family if one["role"] == "panes"), None)
    return {**said,
            # Newer macOS says `powermode 1` (0 automatic, 1 low power, 2 high power); older `lowpowermode 1`.
            "low_power_mode": bool(re.search(r"^\s*(?:low)?powermode\s+1\b", settings, re.M)),
            "cpu_speed_limit": int(limit.group(1)) if limit else None,
            "loud": said["load1"] > said["cores"],
            "window_age_s": round(reading["age_s"]) if reading else None,
            "pane_programs": panes["programs"] if panes else None}


def read_all(family, thread_roles):
    at = time.monotonic()
    counters = {}
    for one in family:
        for pid in one["pids"]:
            counters[pid] = rusage(pid)
    held = {}
    for one in family:
        if one["role"] in thread_roles or one["kind"] in thread_roles:
            for pid in one["pids"]:
                held[pid] = threads(pid)
    return at, counters, held


def delta_row(one, before, after, wall_s):
    """One role's cost over the stretch, from the processes that stood at both ends."""
    both = [pid for pid in one["pids"] if before.get(pid) and after.get(pid)]
    row = {"role": one["role"], "kind": one["kind"], "pids": both[:8], "processes": len(both),
           "gone": len([pid for pid in one["pids"] if not (before.get(pid) and after.get(pid))])}
    if not both:
        return row
    take = lambda key: sum(after[pid][key] - before[pid][key] for pid in both)  # noqa: E731
    row.update({
        "cpu_pct": round(take("cpu_ns") / (wall_s * 1e9) * 100, 3),
        "idle_wakeups_per_s": round(take("idle_wakeups") / wall_s, 2),
        "interrupt_wakeups_per_s": round(take("interrupt_wakeups") / wall_s, 2),
        "energy_mw": round(take("energy_nj") / wall_s / 1e6, 2),
        "footprint": sum(after[pid]["footprint"] for pid in both),
        "peak": max(after[pid]["peak"] for pid in both),
    })
    return row


def busiest_threads(before, after, wall_s, keep):
    rows = []
    for tid, (name, cpu) in after.items():
        spent = cpu - before.get(tid, (name, 0))[1]
        if spent > 0:
            rows.append({"thread": name, "cpu_pct": round(spent / (wall_s * 1e9) * 100, 3), "new": tid not in before})
    rows.sort(key=lambda row: -row["cpu_pct"])
    return rows[:keep]


def stretch(family, seconds, keep_threads):
    thread_roles = {"main", "WebContent", "page"}
    t0, before, threads0 = read_all(family, thread_roles)
    time.sleep(seconds)
    t1, after, threads1 = read_all(family, thread_roles)
    wall_s = t1 - t0
    rows = [delta_row(one, before, after, wall_s) for one in family]
    busy = {}
    for one in family:
        for pid in one["pids"]:
            if pid in threads0 and pid in threads1:
                busy[f"{one['kind']}:{pid}"] = busiest_threads(threads0[pid], threads1[pid], wall_s, keep_threads)
    return {"wall_s": round(wall_s, 2), "rows": rows, "threads": busy}


def fmt_row(row):
    if "cpu_pct" not in row:
        return f"{row['role']:<7} {row['kind'][:34]:<34} gone"
    return (f"{row['role']:<7} {row['kind'][:34]:<34} {row['processes']:>3} {row['cpu_pct']:>8.2f} "
            f"{row['idle_wakeups_per_s']:>9.1f} {row['interrupt_wakeups_per_s']:>9.1f} "
            f"{row['energy_mw']:>8.1f} {row['footprint'] / MIB:>9.1f}")


HEADER = (f"{'role':<7} {'kind':<34} {'n':>3} {'cpu %':>8} {'idle w/s':>9} {'intr w/s':>9} "
          f"{'mW':>8} {'fp MiB':>9}")


def follow(asked):
    if not os.path.isabs(asked.out) or os.path.exists(asked.out):
        raise SystemExit("--out must be an absolute path that does not exist yet")
    os.makedirs(asked.out)
    with open(os.path.join(asked.out, "scenario.json"), "w") as held:
        json.dump({"label": asked.label, "note": asked.note, "seconds": asked.seconds, "repeat": asked.repeat,
                   "pid": asked.pid, "process": asked.process}, held, indent=1)
    for index in range(asked.repeat):
        family = named_family(asked.process) if asked.process else window_family(asked.pid)
        if family is None:
            print(f"window {asked.pid} is gone", file=sys.stderr)
            return 1
        line = {"label": asked.label, "repeat": index, "at_ms": int(time.time() * 1000),
                "conditions": conditions(family), **stretch(family, asked.seconds, asked.threads)}
        with open(os.path.join(asked.out, "idle.jsonl"), "a") as held:
            held.write(json.dumps(line) + "\n")
        state = line["conditions"]
        print(f"# repeat {index}: {line['wall_s']} s, load {state['load1']}/{state['cores']}"
              f"{' LOUD' if state['loud'] else ''}, {state['power']}{' LPM' if state['low_power_mode'] else ''},"
              f" window age {state['window_age_s']} s, pane programs {state['pane_programs']}")
        print(HEADER)
        for row in line["rows"]:
            print(fmt_row(row))
        for owner, rows in line["threads"].items():
            print(f"  threads {owner}: " + ", ".join(f"{one['thread']} {one['cpu_pct']:.2f}%" for one in rows))
        sys.stdout.flush()
    return 0


SUMMED = ("cpu_pct", "idle_wakeups_per_s", "interrupt_wakeups_per_s", "energy_mw", "footprint")


def by_kind(line):
    """One repeat's rows summed per role and kind: five renderers are one `renderer` row."""
    summed = {}
    for row in line["rows"]:
        if "cpu_pct" not in row:
            continue
        kind = row["kind"] if row["role"] != "panes" else "pane programs, with descendants"
        held = summed.setdefault((row["role"], kind), {"role": row["role"], "kind": kind, "processes": 0,
                                                      **{key: 0 for key in SUMMED}})
        held["processes"] += row["processes"]
        for key in SUMMED:
            held[key] += row[key]
    return summed


def report(folders):
    """Per folder, the median over the quiet repeats per role and kind; loud repeats are counted, not used."""
    for folder in folders:
        with open(os.path.join(folder, "idle.jsonl")) as held:
            lines = [json.loads(one) for one in held if one.strip()]
        quiet = [line for line in lines if not line["conditions"]["loud"]]
        print(f"# {folder}: {len(quiet)} quiet of {len(lines)} repeats "
              f"({', '.join(sorted({line['conditions']['power'] or '?' for line in lines}))})")
        print(HEADER)
        summed = [by_kind(line) for line in quiet]
        for key in dict.fromkeys(key for one in summed for key in one):
            picked = [one[key] for one in summed if key in one]
            median = {name: statistics.median(row[name] for row in picked) for name in SUMMED}
            print(fmt_row({"role": key[0], "kind": key[1],
                           "processes": statistics.median(row["processes"] for row in picked), **median}))
        threads = {}
        for line in quiet:
            for owner, rows in line["threads"].items():
                for one in rows:
                    threads.setdefault((owner.split(":")[0], one["thread"]), []).append(one["cpu_pct"])
        top = sorted(threads.items(), key=lambda item: -statistics.median(item[1]))[:10]
        for (owner, name), values in top:
            print(f"  thread {owner} {name}: median {statistics.median(values):.2f}% over {len(values)} repeats")
    return 0


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "report":
        return report(sys.argv[2:])
    import argparse
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True)
    parser.add_argument("--pid", type=int)
    parser.add_argument("--process", action="append", default=[], help="role=pid; repeatable")
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--threads", type=int, default=6)
    parser.add_argument("--label", default="")
    parser.add_argument("--note", default="")
    return follow(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
