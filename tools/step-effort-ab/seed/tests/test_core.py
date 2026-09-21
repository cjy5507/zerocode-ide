import unittest

from calc.core import mean, paginate, parse_duration, word_count


class MeanTests(unittest.TestCase):
    def test_mean_of_three(self):
        self.assertEqual(mean([1, 2, 3]), 2)

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            mean([])


class DurationTests(unittest.TestCase):
    def test_seconds(self):
        self.assertEqual(parse_duration("45s"), 45)

    def test_minutes(self):
        self.assertEqual(parse_duration("2m"), 120)


class WordCountTests(unittest.TestCase):
    def test_counts_words(self):
        self.assertEqual(word_count("a b a"), {"a": 2, "b": 1})


class PaginateTests(unittest.TestCase):
    def test_second_page(self):
        self.assertEqual(paginate(list(range(10)), 1, 3), [3, 4, 5])


if __name__ == "__main__":
    unittest.main()
