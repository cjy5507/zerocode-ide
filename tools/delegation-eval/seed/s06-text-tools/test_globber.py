import unittest

from globber import match


class GlobTest(unittest.TestCase):
    def check(self, pattern, takes, refuses=()):
        for path in takes:
            self.assertTrue(match(pattern, path), f"{pattern} should take {path}")
        for path in refuses:
            self.assertFalse(match(pattern, path), f"{pattern} should refuse {path}")

    def test_plain_text_covers_the_whole_path(self):
        self.check("a/b", ["a/b"], ["a/b/c", "x/a/b", "A/b"])

    def test_a_star_stays_inside_its_segment(self):
        self.check("*.py", ["a.py", ".py"], ["d/a.py", "a.pyc"])
        self.check("a/*/c", ["a/b/c", "a//c"], ["a/b/d/c"])
        self.check("a*", ["a", "abc"], ["a/b"])

    def test_a_question_mark_is_one_character_and_never_a_slash(self):
        self.check("a?c", ["abc", "a.c"], ["ac", "abbc", "a/c"])

    def test_double_star_in_the_middle(self):
        self.check("a/**/b", ["a/b", "a/x/b", "a/x/y/b"], ["ab", "a/xb", "a/b/c", "x/a/b"])

    def test_double_star_at_the_ends(self):
        self.check("a/**", ["a", "a/x", "a/x/y"], ["ab", "b/a"])
        self.check("**/b", ["b", "x/b", "x/y/b"], ["xb", "b/x"])
        self.check("**", ["", "a", "a/b/c"])

    def test_double_star_that_is_not_a_whole_segment_is_one_star(self):
        self.check("a**b", ["ab", "axxb"], ["a/b", "a/x/b"])
        self.check("a/**b", ["a/b", "a/xb"], ["a/x/b"])

    def test_two_double_stars(self):
        self.check("**/a/**/b", ["a/b", "x/a/y/b", "a/a/b/b"], ["b/a", "x/b"])

    def test_character_sets(self):
        self.check("[abc]x", ["ax", "cx"], ["dx", "x"])
        self.check("[a-c]", ["a", "b", "c"], ["d", "-"])
        self.check("[!a-c]", ["d", "-"], ["a", "b", "/"])
        self.check("[]a]", ["]", "a"], ["b"])
        self.check("[a/]", ["a"], ["/"])
        self.check("x[", ["x["], ["xa"])

    def test_a_backslash_makes_anything_plain(self):
        self.check(r"a\*", ["a*"], ["ab"])
        self.check(r"\[a\]", ["[a]"], ["a"])
        self.check(r"a\?", ["a?"], ["ab"])
        self.check(r"a\\b", ["a\\b"], ["ab"])
        self.check(r"[\]]", ["]"], ["\\"])

    def test_long_patterns_answer_at_once(self):
        self.assertFalse(match("a*" * 30 + "b", "a" * 60))
        self.assertFalse(match("**/a/" * 12 + "b", "a/" * 40 + "c"))
        self.assertTrue(match("a*" * 30, "a" * 60))


if __name__ == "__main__":
    unittest.main()
