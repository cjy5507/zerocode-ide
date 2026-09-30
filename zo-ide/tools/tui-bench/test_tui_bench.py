"""The bench's own measuring code, pinned on bytes written by hand.

    python3 -m unittest discover -s zo-ide/tools/tui-bench -p 'test_*.py'

A benchmark whose attribution nobody checked would only be a second opinion of
the code it measures; these streams are small enough to count by eye.
"""

import unittest

import tui_bench as bench

ESC = "\x1b"
SYNC_ON, SYNC_OFF = f"{ESC}[?2026h", f"{ESC}[?2026l"
TAIL = f"{ESC}[0m{ESC}[0 q{ESC}[?25h"


def frame(*rows: tuple[int, str], caret: tuple[int, int] = (1, 1)) -> bytes:
    """A frame the way zo writes one: each row a move, a reset, an erase, its
    glyphs and a reset; then the tail that puts the caret back."""
    body = "".join(f"{ESC}[{row + 1};1H{ESC}[0m{ESC}[K{text}{ESC}[0m" for row, text in rows)
    return f"{SYNC_ON}{body}{TAIL}{ESC}[{caret[1]};{caret[0]}H{SYNC_OFF}".encode()


def replay(frames: list[bytes], gap_ns: int = 10_000_000) -> dict:
    raw = [(index * gap_ns, data) for index, data in enumerate(frames)]
    end = len(frames) * gap_ns
    return bench.attribute_windows(raw, {"all": (0, end)}, cols=20, rows=8)["all"]


class Tokens(unittest.TestCase):
    def kinds(self, text: str) -> dict:
        totals: dict = {}
        for kind, size, _row in bench.tokenize(text.encode()):
            totals[kind] = totals.get(kind, 0) + size
        return totals

    def test_a_frame_is_sorted_by_what_its_bytes_do(self) -> None:
        totals = self.kinds(frame((3, "hello"), caret=(6, 4)).decode())
        self.assertEqual(totals["sync"], 16)  # the pair
        self.assertEqual(totals["cursor"], 5 + 6 + len(f"{ESC}[4;6H"))  # shape, show, caret move
        # The last row's own reset and the frame's: nothing is written after either.
        self.assertEqual(totals["tail_sgr"], 8)
        self.assertEqual(totals["addr"], len(f"{ESC}[4;1H"))
        self.assertEqual(totals["erase"], 3)
        self.assertEqual(totals["text"], 5)
        self.assertEqual(totals["sgr"], 4)  # the reset before the erase
        self.assertNotIn("scroll", totals)

    def test_line_motion_and_regions_are_scroll(self) -> None:
        totals = self.kinds(f"{ESC}[1;10r{ESC}[10;1H\r\nx{ESC}M{ESC}[r")
        self.assertEqual(totals["scroll"], len(f"{ESC}[1;10r") + 2 + 2 + len(f"{ESC}[r"))

    def test_every_byte_is_counted_once(self) -> None:
        data = frame((0, "a"), (5, "bcd"), caret=(3, 6))
        self.assertEqual(sum(size for _kind, size, _row in bench.tokenize(data)), len(data))

    def test_a_sequence_cut_at_the_end_of_a_chunk_is_counted_and_does_not_stop_the_run(self) -> None:
        # A CLI that does not bracket its frames is cut into bursts by the gap between reads, and a read can end inside
        # a sequence: Claude Code's did, and the tokenizer died on the missing final byte (2026-09-30, the quiet window).
        for cut in (b"ab\x1b[", b"ab\x1b[1;", b"ab\x1b[38;2;12", b"ab\x1b[?2", b"ab\x1b[1 "):
            tokens = bench.tokenize(cut)
            self.assertEqual(sum(size for _kind, size, _row in tokens), len(cut), cut)
            self.assertEqual(tokens[0], ("text", 2, None), cut)
            self.assertEqual(tokens[-1][0], "other", cut)


class Attribution(unittest.TestCase):
    def test_a_cell_that_changes_costs_a_whole_row_today(self) -> None:
        first = frame((2, "Working-0s"))
        second = frame((2, "Working-1s"))
        found = replay([first, second])
        self.assertEqual(found["frames"], 2)
        self.assertEqual(found["bytes"], len(first) + len(second))
        # The first frame fills the row (10 cells); the second changes one of them.
        self.assertEqual(found["cells_changed"], 10 + 1)
        self.assertEqual(found["rows_written"], 2)
        self.assertEqual(found["rows_changed"], 2)
        self.assertEqual(found["by_row"]["2"]["writes"], 2)

    def test_a_frame_that_changes_nothing_is_silent(self) -> None:
        same = frame((2, "same"))
        found = replay([same, same])
        self.assertEqual(found["silent_frames"], 1)
        self.assertEqual(found["rows_written"], 2)
        self.assertEqual(found["rows_changed"], 1)

    def test_rows_that_repeat_a_row_a_few_rows_away_are_counted_as_shifts(self) -> None:
        before = frame((1, "alpha"), (2, "bravo"), (3, "charlie"), (4, "delta"))
        # The list scrolled by one row: three rows are rewritten with what was one row lower.
        after = frame((1, "bravo"), (2, "charlie"), (3, "delta"), (4, "echo"))
        found = replay([before, after])
        self.assertEqual(found["shift_rows"], 3)
        self.assertGreater(found["shift_bytes"], 0)

    def test_history_rows_are_kept_out_of_the_row_table(self) -> None:
        history = f"{SYNC_ON}{ESC}[1;5r{ESC}[1;1H\r\nline one\r\nline two{ESC}[r{TAIL}{ESC}[8;1H{SYNC_OFF}".encode()
        found = replay([history])
        self.assertEqual(found["scroll_frames"], 1)
        self.assertEqual(found["by_row"], {})

    def test_a_cli_without_the_pair_is_cut_into_bursts(self) -> None:
        raw = [(0, b"\x1b[1;1Hone"), (1_000_000, b"\x1b[2;1Htwo"), (50_000_000, b"\x1b[3;1Hthree")]
        found = bench.attribute_windows(raw, {"all": (0, 100_000_000)}, cols=20, rows=8)["all"]
        self.assertEqual(found["frames"], 2)
        self.assertEqual(found["cells_changed"], len("one") + len("two") + len("three"))


class Echo(unittest.TestCase):
    def test_percentiles_come_off_the_keystrokes(self) -> None:
        keys = [bench.Keystroke("a", 0, int((index + 1) * 1e6)) for index in range(100)]
        stats = bench.echo_stats(keys)
        # Nearest rank: the value at rank ceil(n * q / 100), as the final tables read them.
        self.assertEqual(stats["echo_p50_ms"], 50.0)
        self.assertEqual(stats["echo_p95_ms"], 95.0)
        self.assertEqual(stats["echo_p99_ms"], 99.0)
        self.assertEqual(stats["echo_max_ms"], 100.0)
        thirty = bench.echo_stats([bench.Keystroke("a", 0, int((index + 1) * 1e6)) for index in range(30)])
        self.assertEqual((thirty["echo_p50_ms"], thirty["echo_p95_ms"], thirty["echo_p99_ms"]), (15.0, 29.0, 30.0))


class Screens(unittest.TestCase):
    def test_elapsed_time_and_the_wave_do_not_make_two_screens_differ(self) -> None:
        a = ["• Working (3s • esc to interrupt)", "done in 12.5s"]
        b = ["• Working (4s • esc to interrupt)", "done in 13.1s"]
        self.assertEqual(bench.normalize_screen(a), bench.normalize_screen(b))
        self.assertNotEqual(bench.normalize_screen(a), bench.normalize_screen(["• Working", "other"]))

    def test_the_runs_temporary_folder_does_not_make_two_screens_differ(self) -> None:
        a = ["directory: /private/var/folders/yv/…/before-1/project",
             "claude-opus-5 high · /private/var/folders/yv/23c9/T/tui-bench-runs-4g4a4g… 100% context left"]
        b = ["directory: /private/var/folders/yv/…/after-1/project",
             "claude-opus-5 high · /private/var/folders/yv/23c9/T/tui-bench-runs-9x9x9x… 100% context left"]
        self.assertEqual(bench.normalize_screen(a), bench.normalize_screen(b))
        self.assertNotEqual(bench.normalize_screen(a), bench.normalize_screen(a[:1] + ["claude-opus-5 low"]))


if __name__ == "__main__":
    unittest.main()
