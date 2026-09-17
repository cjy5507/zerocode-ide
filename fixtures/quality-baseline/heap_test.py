import unittest
from heap import histogram


class HeapHistogram(unittest.TestCase):
    def test_exact_size_classes_fold_object_types_and_ignore_physical_rss(self):
        text = '''Physical footprint (peak): 999999K
All zones: 4 nodes malloced - Sizes: 1KB[1] 16[3]
All zones: 4 nodes (1072 bytes)
  2 32 16.0 non-object
  1 1024 1024.0 non-object
  1 16 16.0 CFString ObjC
'''
        self.assertEqual(histogram(text), {'live_bytes': 1072, 'classes': [
            {'size_bytes': 16, 'count': 3}, {'size_bytes': 1024, 'count': 1}]})

    def test_rejects_missing_or_incomplete_histograms(self):
        for text in ['Physical footprint: 10M', 'All zones: 3 nodes (48 bytes)\n  2 32 16.0 non-object']:
            with self.assertRaises(ValueError):
                histogram(text)


if __name__ == '__main__':
    unittest.main()
