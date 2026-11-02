import json
import unittest
import zlib

from exporters.jsonl import export
from report_core import Column, Report, Row, Section
from reports_for_tests import rich, stock


class JsonlTest(unittest.TestCase):
    maxDiff = None

    def assertChecked(self, text):
        lines = text.splitlines(keepends=True)
        said = json.loads(lines[-1])["check"]
        self.assertEqual(said, f"{zlib.crc32(''.join(lines[:-1]).encode()):08x}")

    def test_the_layout(self):
        lines = export(stock()).splitlines()
        self.assertEqual(lines[0], '{"kind":"report","columns":["sku","qty","price"],"sections":1,"title":"Stock"}')
        self.assertEqual(lines[1], '{"kind":"row","price":"0.35","qty":12,"section":"North","sku":"bolt"}')
        self.assertEqual(lines[2], '{"kind":"row","price":"1.10","qty":null,"section":"North","sku":"nut"}')
        self.assertTrue(lines[3].startswith('{"kind":"end","rows":2,"sum":{"price":"1.45","qty":12},"check":"'))
        self.assertTrue(lines[3].endswith('"}'))
        self.assertChecked(export(stock()))

    def test_empty_sections_count_and_text_is_written_as_it_is(self):
        text = export(rich())
        lines = text.splitlines()
        self.assertIn('"sections":3', lines[0])
        self.assertEqual(lines[4], '{"kind":"row","d":"0.1","n":7,"na|me":"é","section":"B"}')
        self.assertTrue(lines[5].startswith('{"kind":"end","rows":4,"sum":{"d":"0.1","n":1002},"check":"'))
        self.assertChecked(text)

    def test_decimals_round_half_up_in_their_digits(self):
        report = Report("T", [Column("d", "decimal", places=1)], [Section("S", [Row(["1.25"]), Row(["-1.25"]), Row(["2"])])])
        lines = export(report).splitlines()
        self.assertIn('"d":"1.3"', lines[1])
        self.assertIn('"d":"-1.3"', lines[2])
        self.assertIn('"d":"2.0"', lines[3])

    def test_a_sum_is_worked_out_exactly(self):
        report = Report("T", [Column("d", "decimal", places=2)], [Section("S", [Row(["0.10"]), Row(["0.20"]), Row(["0.005"]), Row(["12345678901234567890.10"])])])
        self.assertIn('"sum":{"d":"12345678901234567890.41"}', export(report).splitlines()[-1])

    def test_column_names_that_collide_move_aside(self):
        report = Report("T", [Column("section", "text"), Column("kind", "text"), Column("z", "int")], [Section("S", [Row(["a", "b", 1])])])
        self.assertEqual(export(report).splitlines()[1],
                         '{"kind":"row","column.kind":"b","column.section":"a","section":"S","z":1}')

    def test_a_numeric_column_with_nothing_known_sums_to_null(self):
        report = Report("T", [Column("n", "int"), Column("d", "decimal", places=1)], [Section("S", [Row([None, None])])])
        self.assertIn('"sum":{"d":null,"n":null}', export(report).splitlines()[-1])

    def test_a_report_with_nothing_in_it(self):
        text = export(Report("T", [Column("n", "int")], []))
        lines = text.splitlines()
        self.assertEqual(lines[0], '{"kind":"report","columns":["n"],"sections":0,"title":"T"}')
        self.assertTrue(lines[1].startswith('{"kind":"end","rows":0,"sum":{"n":null},"check":"'))
        self.assertChecked(text)

    def test_every_line_is_json(self):
        for line in export(rich()).splitlines():
            self.assertIn("kind", json.loads(line))


if __name__ == "__main__":
    unittest.main()
