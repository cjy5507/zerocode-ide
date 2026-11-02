import unittest

from exporters.tsv import export, load
from report_core import Column, Report, Row, Section
from reports_for_tests import rich, stock


class ExportTest(unittest.TestCase):
    maxDiff = None

    def test_the_documented_example(self):
        self.assertEqual(export(stock()), "#title\tStock\nsection\tsku\tqty\tprice\nNorth\tbolt\t12\t0.35\nNorth\tnut\t\\N\t1.10\n")

    def test_every_field_is_escaped(self):
        report = Report("a\tb", [Column("c\nd", "text")], [Section("e\rf", [Row(["g\\h"])])])
        self.assertEqual(export(report), "#title\ta\\tb\nsection\tc\\nd\ne\\rf\tg\\\\h\n")

    def test_unknown_and_a_text_that_looks_like_it(self):
        report = Report("T", [Column("a", "text"), Column("b", "text")], [Section("S", [Row([None, "\\N"])])])
        self.assertEqual(export(report).splitlines()[2], "S\t\\N\t\\\\N")

    def test_decimals_pad_and_round_half_to_even(self):
        cases = {"1.5": "1.50", "1.125": "1.12", "1.135": "1.14", "-0.005": "-0.00", "2": "2.00",
                 "1.1251": "1.13", "12345678901234567890.125": "12345678901234567890.12", "-1.135": "-1.14"}
        for written, shown in cases.items():
            report = Report("T", [Column("d", "decimal", places=2)], [Section("S", [Row([written])])])
            self.assertEqual(export(report).splitlines()[2], f"S\t{shown}", written)

    def test_no_places_means_a_whole_number(self):
        report = Report("T", [Column("d", "decimal", places=0)], [Section("S", [Row(["2.5"]), Row(["3.5"])])])
        self.assertEqual(export(report).splitlines()[2:], ["S\t2", "S\t4"])

    def test_a_section_with_no_rows_adds_no_line(self):
        self.assertEqual(len(export(rich()).splitlines()), 2 + 3 + 1)


class LoadTest(unittest.TestCase):
    def test_it_reads_back_what_was_written(self):
        title, columns, rows = load(export(rich()))
        self.assertEqual(title, "Odd | title")
        self.assertEqual(columns, ["na|me", "n", "d"])
        self.assertEqual(rows, [
            ["A", "x|y", "1000", "1.2"],
            ["A", "two\nlines", "-5", "-1.2"],
            ["A", None, None, None],
            ["B", "é", "7", "0.0"],
        ])

    def test_escapes_come_back_exactly(self):
        text = "a\tb\nc\rd\\e\\N\\t"
        report = Report(text, [Column(text, "text"), Column("b", "text")], [Section(text, [Row([text, "\\N"])])])
        title, columns, rows = load(export(report))
        self.assertEqual((title, columns, rows), (text, [text, "b"], [[text, text, "\\N"]]))

    def test_what_is_not_this_format_is_refused(self):
        bad = {
            "no title": "section\ta\nS\t1\n",
            "too few": "#title\tT\nsection\ta\tb\nS\t1\n",
            "too many": "#title\tT\nsection\ta\nS\t1\t2\n",
            "bad escape": "#title\tT\nsection\ta\nS\t\\x\n",
            "trailing backslash": "#title\tT\nsection\ta\nS\tb\\\n",
        }
        for name, text in bad.items():
            with self.assertRaises(ValueError, msg=name):
                load(text)


if __name__ == "__main__":
    unittest.main()
