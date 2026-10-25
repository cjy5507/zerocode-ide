import unittest

from tally import tally, top


class TallyTest(unittest.TestCase):
    def test_words_are_split_on_any_whitespace(self):
        self.assertEqual(tally("a  b\nb\ta"), {"a": 2, "b": 2})

    def test_case_does_not_make_two_words(self):
        self.assertEqual(tally("The the THE"), {"the": 3})

    def test_top_is_most_frequent_first_and_ties_keep_first_seen(self):
        self.assertEqual(top("c b b a a a c", 2), ["a", "c"])


if __name__ == "__main__":
    unittest.main()
