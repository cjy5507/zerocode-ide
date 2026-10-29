#!/usr/bin/env python3
"""The tables of one window census, read out of its data folder (t-82914).

usage: census-summary.py <data folder written by window-census.sh>

Prints markdown: the boot phases per level, each steady scene's memory and
idle cost by process, the thirty-minute curve, the person's window, and the
page harness table `profile.mjs report` wrote. Medians over the runs a stage
kept; run 1 of a measure.sh folder is the ledger's cutover start and is shown
apart, never in a median. A repeat taken with the load over the core count is
left out and counted. Every number names its unit.
"""
import glob
import json
import os
import re
import statistics
import sys

MIB = 1024 * 1024
BOOT_PHASES = ["settings_read", "ledger_ready", "webview_created", "renderer_up", "page_script_loaded",
               "first_paint", "chromium_ready", "stage_restored", "layouts_read", "panes_restored",
               "browser_tabs_restored", "terminals_resumed"]


def median(values):
    kept = [one for one in values if one is not None]
    return statistics.median(kept) if kept else None


def fmt(value, digits=1):
    return "—" if value is None else f"{value:,.{digits}f}"


def run_number(path):
    found = re.search(r"-(\d+)(?:\.json)?$", path.rstrip("/"))
    return int(found.group(1)) if found else 0


def read_json(path):
    with open(path) as held:
        return json.load(held)


def read_lines(path):
    with open(path) as held:
        return [json.loads(one) for one in held if one.strip()]


def conditions_note(data, name):
    path = os.path.join(data, f"{name}.conditions.json")
    if not os.path.exists(path):
        return ""
    said = read_json(path)
    return (f"조건: {said.get('power')} {said.get('charge')}%, 저전력 {'켬' if said.get('low_power_mode') else '끔'}, "
            f"시작 때 1분 부하 {said.get('load1', 0):.1f}/{said.get('cores')}, 설치본 {said.get('installed')}")


def boot(data):
    out = ["## 시작 (boot) — 단계별 ms, 프로세스 시작부터", ""]
    levels = [level for level in ("normal", "low") if os.path.isdir(os.path.join(data, f"boot-{level}"))]
    if not levels:
        return []
    timelines = {}
    for level in levels:
        rows = sorted(glob.glob(os.path.join(data, f"boot-{level}", "timeline-*.json")), key=run_number)
        timelines[level] = [(run_number(path), read_json(path)) for path in rows]
    out.append("| 단계 | " + " | ".join(f"{level} 중앙값 (첫 회 제외 {sum(1 for at, _ in timelines[level] if at > 1)}회)"
                                         f" | {level} 첫 회(원장 이관)" for level in levels) + " |")
    out.append("|---|" + "---:|---:|" * len(levels))
    for phase in BOOT_PHASES:
        cells = []
        for level in levels:
            warm = [one.get("phases", {}).get(phase) for at, one in timelines[level] if at > 1]
            first = next((one.get("phases", {}).get(phase) for at, one in timelines[level] if at == 1), None)
            cells += [fmt(median(warm), 0), fmt(first, 0)]
        out.append(f"| {phase} | " + " | ".join(cells) + " |")
    for level in levels:
        incomplete = sum(1 for _, one in timelines[level] if not one.get("complete"))
        out.append(f"\n{level}: {conditions_note(data, f'boot-{level}')}; 끝나지 않은 시작 {incomplete}회")
    return out + [""]


def family_memory(line):
    """One window.py sample, by process, MiB."""
    main = line["main"]
    webkit = {}
    for service in line.get("webkit", []):
        footprint = service.get("footprint") or {}
        held = webkit.setdefault(service["kind"], {"footprint": 0, "graphics": 0})
        held["footprint"] += footprint.get("footprint") or 0
        held["graphics"] += footprint.get("graphics_dirty") or 0
    helpers = sum((one.get("footprint") or {}).get("footprint", 0) for one in line.get("helpers", []))
    vmmap = main.get("vmmap") or {}
    row = {
        "main": main["footprint"]["footprint"] / MIB,
        "main_partition": (vmmap.get("partition_alloc") or {}).get("dirty", 0) / MIB if vmmap else None,
        "helpers": helpers / MIB, "helpers_n": len(line.get("helpers", [])),
        "ui": webkit.get("WebContent", {}).get("footprint", 0) / MIB,
        "ui_graphics": webkit.get("WebContent", {}).get("graphics", 0) / MIB,
        "gpu": webkit.get("GPU", {}).get("footprint", 0) / MIB,
        "network": webkit.get("Networking", {}).get("footprint", 0) / MIB,
        "programs_n": line.get("programs", {}).get("count"),
        "programs_rss": line.get("programs", {}).get("rss", 0) / MIB,
    }
    row["total"] = row["main"] + row["helpers"] + row["ui"] + row["gpu"] + row["network"]
    return row


def idle_cost(line):
    """One idle.py repeat, by role, CPU % of one core and wake-ups per second."""
    row = {"loud": line["conditions"]["loud"], "pane_programs": line["conditions"].get("pane_programs")}
    for one in line["rows"]:
        if "cpu_pct" not in one:
            continue
        key = {"main": "main", "panes": "panes"}.get(one["role"]) or (
            "ui" if one["kind"] == "WebContent" else "gpu" if one["kind"] == "GPU" else
            "network" if one["kind"] == "Networking" else "helpers")
        held = row.setdefault(key, {"cpu": 0, "idle_w": 0, "intr_w": 0, "children": 0})
        held["cpu"] += one["cpu_pct"]
        held["idle_w"] += one["idle_wakeups_per_s"]
        held["intr_w"] += one["interrupt_wakeups_per_s"]
        held["children"] += one.get("children_cpu_pct", 0)
    window = [row[key] for key in ("main", "helpers", "ui", "gpu", "network") if key in row]
    row["window"] = {name: sum(one[name] for one in window) for name in ("cpu", "idle_w", "intr_w", "children")}
    threads = line.get("threads", {})
    row["main_threads"] = next((rows for owner, rows in threads.items() if owner.startswith("zerocode-shell:")), [])
    return row


MEMORY_COLUMNS = [("main", "본체"), ("main_partition", "본체 PartitionAlloc dirty"), ("helpers", "CEF 도우미"),
                  ("ui", "창 UI WebContent"), ("ui_graphics", "└ 그래픽"), ("gpu", "WebKit GPU"),
                  ("network", "WebKit 네트워크"), ("total", "창 가족 합")]
IDLE_COLUMNS = [("main", "본체"), ("helpers", "CEF 도우미"), ("ui", "창 UI WebContent"), ("gpu", "WebKit GPU"),
                ("window", "창 가족 합"), ("panes", "판 안 프로세스 합")]


def steady(data):
    scenes = sorted(glob.glob(os.path.join(data, "steady", "*")))
    scenes = [one for one in scenes if os.path.isdir(one)]
    if not scenes:
        return []
    out = ["## 판 수·탭별 정상 상태 (격리 창, 첫 회 제외 중앙값)", "",
           "메모리 footprint, MiB:", "",
           "| 장면 | 회 | " + " | ".join(label for _, label in MEMORY_COLUMNS) + " | 판 프로그램 수 |",
           "|---|---:|" + "---:|" * (len(MEMORY_COLUMNS) + 1)]
    idle_rows = []
    for scene in scenes:
        name = os.path.basename(scene)
        memory = [family_memory(read_lines(path)[0]) for path in glob.glob(os.path.join(scene, "memory-*", "samples.jsonl"))
                  if run_number(os.path.dirname(path)) > 1]
        if memory:
            out.append(f"| {name} | {len(memory)} | " + " | ".join(fmt(median(row[key] for row in memory))
                                                              for key, _ in MEMORY_COLUMNS)
                       + f" | {fmt(median(row['programs_n'] for row in memory), 0)} |")
        idle = [idle_cost(read_lines(path)[0]) for path in glob.glob(os.path.join(scene, "idle-*", "idle.jsonl"))
                if run_number(os.path.dirname(path)) > 1]
        idle_rows.append((name, idle))
    out += ["", "쉬는 비용 — CPU % (한 코어 기준) · idle 깨어남/초 · interrupt 깨어남/초, 본체 칸의 「자식」은 본체가 띄웠다 끝난 짧은 명령의 CPU %:", "",
            "| 장면 | 조용한 회 | " + " | ".join(label for _, label in IDLE_COLUMNS) + " |",
            "|---|---:|" + "---|" * len(IDLE_COLUMNS)]
    for name, idle in idle_rows:
        quiet = [row for row in idle if not row["loud"]]
        cells = []
        for key, _ in IDLE_COLUMNS:
            picked = [row[key] for row in quiet if key in row]
            if not picked:
                cells.append("—")
                continue
            cell = (f"{fmt(median(one['cpu'] for one in picked), 2)} % · {fmt(median(one['idle_w'] for one in picked))}"
                    f" · {fmt(median(one['intr_w'] for one in picked))}")
            if key == "main":
                cell += f" (자식 {fmt(median(one['children'] for one in picked), 2)} %)"
            cells.append(cell)
        out.append(f"| {name} | {len(quiet)}/{len(idle)} | " + " | ".join(cells) + " |")
    out += ["", "본체에서 CPU를 가장 많이 쓴 스레드 (장면별, 회 중앙값 상위 5):", ""]
    for name, idle in idle_rows:
        spent = {}
        for row in idle:
            for thread in row["main_threads"]:
                spent.setdefault(thread["thread"], []).append(thread["cpu_pct"])
        top = sorted(spent.items(), key=lambda item: -median(item[1]))[:5]
        out.append(f"- {name}: " + ", ".join(f"{thread} {median(values):.2f} %" for thread, values in top))
    return out + [""]


def curve(lines, minutes):
    """The sample nearest each named minute."""
    picked = []
    for minute in minutes:
        near = min(lines, key=lambda line: abs(line["minute"] - minute))
        if abs(near["minute"] - minute) <= max(1.5, minute * 0.1):
            picked.append((minute, near))
    return picked


def follow_section(title, memory_path, idle_path, minutes):
    out = []
    if os.path.exists(memory_path):
        lines = read_lines(memory_path)
        out += [f"## {title}", "", "메모리 footprint, MiB (가까운 표본):", "",
                "| 분 | " + " | ".join(label for _, label in MEMORY_COLUMNS) + " | 판 프로그램 수 | 1분 부하 |",
                "|---:|" + "---:|" * (len(MEMORY_COLUMNS) + 2)]
        for minute, line in curve(lines, minutes):
            row = family_memory(line)
            out.append(f"| {line['minute']:.0f} | " + " | ".join(fmt(row[key]) for key, _ in MEMORY_COLUMNS)
                       + f" | {row['programs_n']} | {line['machine']['load1']} |")
        out.append("")
    if os.path.exists(idle_path):
        rows = [idle_cost(line) for line in read_lines(idle_path)]
        quiet = [row for row in rows if not row["loud"]]
        out += [f"쉬는 비용 ({len(quiet)}/{len(rows)} 조용한 분의 중앙값):", ""]
        for key, label in IDLE_COLUMNS:
            picked = [row[key] for row in quiet if key in row]
            if picked:
                out.append(f"- {label}: CPU {fmt(median(one['cpu'] for one in picked), 2)} %, idle 깨어남 "
                           f"{fmt(median(one['idle_w'] for one in picked))}/초, interrupt 깨어남 "
                           f"{fmt(median(one['intr_w'] for one in picked))}/초"
                           + (f", 자식 {fmt(median(one['children'] for one in picked), 2)} %" if key == "main" else ""))
        out.append("")
    return out


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    data = sys.argv[1]
    out = [f"# 창 측정 요약 — {data}", ""]
    if os.path.exists(os.path.join(data, "ui.ref")):
        with open(os.path.join(data, "ui.ref")) as held:
            out += [f"하네스 UI: {held.read().strip()}", ""]
    out += boot(data)
    out += steady(data)
    for level in ("normal", "low"):
        out += follow_section(f"idle-30m {level} (격리 창 판 10, 30분 손대지 않음)",
                              os.path.join(data, f"idle-30m-{level}", "memory", "samples.jsonl"),
                              os.path.join(data, f"idle-30m-{level}", "idle", "idle.jsonl"), [0, 10, 20, 30])
    for level in ("normal", "low"):
        dense = sorted(glob.glob(os.path.join(data, f"dense-{level}", "idle-*", "idle.jsonl")))
        if dense:
            rows = [idle_cost(read_lines(path)[0]) for path in dense if run_number(os.path.dirname(path)) > 1]
            out += [f"## dense {level}", ""] + [
                f"- {label}: CPU {fmt(median(row[key]['cpu'] for row in rows if key in row), 2)} %"
                for key, label in IDLE_COLUMNS if any(key in row for row in rows)] + [""]
    started = sorted(glob.glob(os.path.join(data, "person-boot-timeline-v*.json")))
    if started:
        out += ["## 사람 창의 실제 시작 — 단계별 ms (창이 쓴 boot-timeline.json, 한 번씩)", "",
                "| 단계 | " + " | ".join(os.path.basename(path)[len("person-boot-timeline-"):-5] for path in started) + " |",
                "|---|" + "---:|" * len(started)]
        read = [read_json(path).get("phases", {}) for path in started]
        for phase in BOOT_PHASES + ["first_browser_tab"]:
            out.append(f"| {phase} | " + " | ".join(fmt(one.get(phase), 0) for one in read) + " |")
        out.append("")
    out += follow_section("사람 창 (읽기만)", os.path.join(data, "person", "memory", "samples.jsonl"),
                          os.path.join(data, "person", "idle", "idle.jsonl"), [2, 30, 60, 120])
    # harness/ served <data>/ui; harness-<name>/ served the release prep --ui <name> unpacked into <data>/<name>/ui.
    for table in sorted(glob.glob(os.path.join(data, "harness*", "table.md"))):
        folder = os.path.basename(os.path.dirname(table))
        ref = os.path.join(data, "ui.ref" if folder == "harness" else os.path.join(folder[len("harness-"):], "ui.ref"))
        served = ""
        if os.path.exists(ref):
            with open(ref) as held:
                served = f" — UI {held.read().split()[0]}"
        with open(table) as held:
            out += [f"## 페이지 하네스{served} (profile.mjs report: 탐침 · 수 · 수준별 5회 중앙값)", "", held.read()]
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
