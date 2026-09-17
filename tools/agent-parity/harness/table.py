#!/usr/bin/env python3
"""The axis × subject table, formatted from the analysers rather than by hand.

One formatter per axis, so a cell in `docs/analysis/zo-agent-parity-<date>.md`
is never a number somebody retyped. `min/median of N` axes take several runs;
the rest take one.
"""
import json, os, statistics, sys

ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")
sys.path.insert(0, os.path.join(ROOT, "harness"))
import analyze  # noqa: E402


def load(runs):
    out = []
    for run in runs:
        d = run if os.path.isabs(run) else os.path.join(ROOT, "runs", run)
        if not os.path.isdir(d):
            continue
        axis, drv, reqs, m = analyze.analyse(d)
        out.append((os.path.basename(d), drv, m))
    return out


def mm(rows, key, unit="ms"):
    vals = [m[key] for _, _, m in rows if isinstance(m.get(key), (int, float))]
    if not vals:
        return "—"
    return "**%.1f / %.1f %s**" % (min(vals), statistics.median(vals), unit)


def yes(flag):
    return "예" if flag else "아니오"


def cell_A(rows):
    return mm(rows, "spawn_to_child_request_ms")


def cell_Aprime(rows):
    return mm(rows, "spawn_to_first_tool_ms")


def cell_K(rows):
    _, _, m = rows[0]
    return "**%s 턴 · 와이어 %s개(`Agent` %s)**" % (
        m["parent_turns_before_child"], m["wire_tool_count"],
        "있음" if m["agent_advertised_on_wire"] else "없음")


def cell_B(rows):
    _, _, m = rows[0]
    return "**%d/%d 정답 · %s(첫 요청 폭 %.3f s) · 부모 폴링 %d회**" % (
        m["answers_matched"], m["answers_expected"],
        "병렬" if m["parallel"] else "직렬", m["first_request_span_s"],
        m["parent_polls"])


def cell_C(rows):
    _, _, m = rows[0]
    return "**유실 %d (%d/%d) · 중앙 %.3f s**" % (
        m["missing"], m["delivered"], m["expected"], m["delivery_delay_median_s"])


def cell_D(rows):
    _, _, m = rows[0]
    kinds = m["kinds_seen"]
    return "**%d/%d** — %s" % (len(kinds), m["kinds_expected"],
                               "·".join("`%s`" % k for k in kinds) or "이름 붙은 영수증 없음")


def cell_F(rows):
    _, _, m = rows[0]
    return "**%d/%d · 중앙 %.3f s · 본문 %d/%d 인라인**" % (
        m["notified"], m["expected"], m["delay_median_s"],
        m["output_inlined"], m["expected"])


def cell_G(rows):
    _, _, m = rows[0]
    return "**재실행 %d · 부수 호출 %d**" % (m["completed_steps_rerun"],
                                            m["extra_calls_needed"])


def cell_H1(rows):
    _, _, m = rows[0]
    if not m["cron_fired"]:
        return "**0/1 — 안 뜬다**(`%s`, %s)" % (m["cron_expr"], m["cron_zone"])
    return "**1/1, 표현식의 분 대비 %+.2f s**(`%s`, %s)" % (
        m["cron_error_s"], m["cron_expr"], m["cron_zone"])


def cell_H2(rows):
    _, _, m = rows[0]
    if not m["wakeup_fired"]:
        return "**0/1 — 안 뜬다**"
    return "**1/1 · 무장 뒤 %.2f s**(45 s 요청은 둘 다 60 s 로 clamp)" % m["wakeup_delay_s"]


def cell_H3(rows):
    """Ticks in the window, and the error of the STEADY intervals.

    The first interval is not one: a loop that schedules through cron waits for
    the next minute boundary, so gap #1 is a phase artifact of when the run
    started, not of the interval. It is named rather than averaged in.
    """
    _, _, m = rows[0]
    errs = m["interval_error_s"]
    steady = errs[1:] if len(errs) > 1 else errs
    span = ("%+.2f s(전부)" % steady[0] if steady and len(set(steady)) == 1
            else "%+.2f ~ %+.2f s" % (min(steady), max(steady)) if steady
            else "간격 없음")
    first = ("첫 간격 %.1f s" % m["interval_s"][0]) if m["interval_s"] else "간격 없음"
    return "**%d회(창 %.0f s, ≥%d 필요) · %s · %s**" % (
        m["ticks"], m["window_s"], m["expected"], span, first)


def cell_I(rows):
    _, _, m = rows[0]
    dirty = [ln for ln in (m["main_status_raw"] or "").splitlines()
             if ln[:2].strip() and ln[:2] in ("??", " M", "M ", "A ", " D")]
    return "**%s, main %s**%s" % ("통과" if m["pass"] else "실패",
                                  "깨끗" if m["main_clean"] else "더러움",
                                  "" if m["main_clean"] else " — `%s`" % dirty[0])


def cell_J(rows):
    _, _, m = rows[0]
    return "**%d/%d · 도구 %d회**" % (m["fork_correct"], m["fork_expected"],
                                      m["fork_tool_calls"])


def cell_M(rows):
    _, _, m = rows[0]
    return ("**해제 %.3f s · 넷 다 `lane_done` %s · `StopAgent` %d회 · "
            "대조군 자리 지킴 %s**" % (m["release_s"], yes(m["all_lane_done"]),
                                      m["stop_agent_calls"],
                                      yes(m["control_still_seated"])))


CELLS = {"A": cell_A, "A'": cell_Aprime, "K": cell_K, "B": cell_B, "C": cell_C,
         "D": cell_D, "F": cell_F, "G": cell_G, "H1": cell_H1, "H2": cell_H2,
         "H3": cell_H3, "I": cell_I, "J": cell_J, "M": cell_M}


def main():
    axis, runs = sys.argv[1], sys.argv[2:]
    rows = load(runs)
    if not rows:
        print("no runs")
        return
    print(CELLS[axis](rows))


if __name__ == "__main__":
    main()
