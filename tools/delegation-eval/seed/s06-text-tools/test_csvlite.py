import unittest

from csvlite import ParseError, dump, parse


class ParseTest(unittest.TestCase):
    def test_plain_records(self):
        self.assertEqual(parse("a,b\nc,d\n"), [["a", "b"], ["c", "d"]])
        self.assertEqual(parse("a,b\nc,d"), [["a", "b"], ["c", "d"]])
        self.assertEqual(parse(""), [])

    def test_windows_line_ends(self):
        self.assertEqual(parse("a,b\r\nc\r\n"), [["a", "b"], ["c"]])

    def test_a_lone_carriage_return_is_text(self):
        self.assertEqual(parse("a\rb,c\n"), [["a\rb", "c"]])

    def test_empty_fields_and_blank_lines(self):
        self.assertEqual(parse(",a,\n"), [["", "a", ""]])
        self.assertEqual(parse("a\n\nb\n"), [["a"], [""], ["b"]])
        self.assertEqual(parse("\n"), [[""]])
        self.assertEqual(parse("\n\n"), [[""], [""]])

    def test_quoted_fields(self):
        self.assertEqual(parse('"a,b",c\n'), [["a,b", "c"]])
        self.assertEqual(parse('"say ""hi""",x\n'), [['say "hi"', "x"]])
        self.assertEqual(parse('""\n'), [[""]])
        self.assertEqual(parse('a,""\n'), [["a", ""]])

    def test_newlines_inside_quotes_are_kept_as_written(self):
        self.assertEqual(parse('"a\nb",c\n'), [["a\nb", "c"]])
        self.assertEqual(parse('"a\r\nb"\r\nc\r\n'), [["a\r\nb"], ["c"]])

    def test_a_quote_inside_an_unquoted_field_is_plain(self):
        self.assertEqual(parse('a"b,c"\n'), [['a"b', 'c"']])
        self.assertEqual(parse(' "a"\n'), [[' "a"']])

    def test_text_after_a_closing_quote_is_named(self):
        with self.assertRaises(ParseError) as caught:
            parse('ok,fine\nx,"ab"c,d\n')
        self.assertEqual((caught.exception.line, caught.exception.column), (2, 7))

    def test_text_after_a_closing_quote_on_a_later_line_of_the_field(self):
        with self.assertRaises(ParseError) as caught:
            parse('"a\nbc"d\n')
        self.assertEqual((caught.exception.line, caught.exception.column), (2, 4))

    def test_a_quote_never_closed_is_named_where_it_opened(self):
        with self.assertRaises(ParseError) as caught:
            parse('ok\nab,"never\nclosed\n')
        self.assertEqual((caught.exception.line, caught.exception.column), (2, 4))

    def test_a_doubled_quote_at_the_end_does_not_close(self):
        with self.assertRaises(ParseError) as caught:
            parse('"abc""')
        self.assertEqual((caught.exception.line, caught.exception.column), (1, 1))


class DumpTest(unittest.TestCase):
    def test_only_what_needs_quotes_gets_them(self):
        self.assertEqual(dump([["a", "b c"]]), "a,b c\n")
        self.assertEqual(dump([["a,b", 'c"d', "e\nf", "g\rh"]]), '"a,b","c""d","e\nf","g\rh"\n')

    def test_spaces_at_the_edges_get_quotes(self):
        self.assertEqual(dump([[" a", "b ", "c d"]]), '" a","b ",c d\n')

    def test_an_empty_only_field_gets_quotes_and_others_do_not(self):
        self.assertEqual(dump([[""]]), '""\n')
        self.assertEqual(dump([["", ""], ["a", ""]]), ",\na,\n")

    def test_nothing_is_lost_on_the_way_back(self):
        rows = [
            [""], ["", ""], ["a"], ['"'], ['""'], [","], ["\n"], ["\r\n"], ["\r"],
            ["a b", " c", "d "], ['x"y', "z,w", "line\nbreak"], ['"quoted"', ""],
            ["é", "日本語"],
        ]
        for one in rows:
            self.assertEqual(parse(dump([one])), [one], one)
        self.assertEqual(parse(dump(rows)), rows)


if __name__ == "__main__":
    unittest.main()
