import unittest

from ttlcache import TTLCache


class ChainChecked(unittest.TestCase):
    def assertSound(self, cache):
        down, up = cache._chain()
        self.assertEqual(down, list(reversed(up)), "the two walks disagree")
        self.assertEqual(sorted(down), sorted(cache._nodes), "the chain and the map disagree")
        self.assertEqual(len(down), len(cache))
        self.assertLessEqual(len(cache), cache.capacity)


class RecencyTest(ChainChecked):
    def test_the_least_recently_used_goes_first(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0)
        cache.put("b", 2, now=0)
        cache.get("a", now=0)
        cache.put("c", 3, now=0)
        self.assertEqual(cache.keys(now=0), ["c", "a"])
        self.assertSound(cache)

    def test_peek_does_not_touch_recency(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0)
        cache.put("b", 2, now=0)
        self.assertEqual(cache.peek("a", now=0), 1)
        cache.put("c", 3, now=0)
        self.assertEqual(cache.keys(now=0), ["c", "b"])

    def test_the_oldest_can_be_used_again_and_again(self):
        cache = TTLCache(3)
        for key in "abc":
            cache.put(key, key, now=0)
        for key in "abcabc":
            self.assertEqual(cache.get(key, now=0), key)
            self.assertSound(cache)
        cache.put("d", "d", now=0)
        self.assertEqual(cache.keys(now=0), ["d", "c", "b"])
        self.assertSound(cache)

    def test_a_long_churn_keeps_the_chain_sound(self):
        cache = TTLCache(4)
        state = 7
        for step in range(400):
            state = (state * 31 + 11) % 257
            key = state % 9
            if state % 3 == 0:
                cache.get(key, now=step)
            elif state % 3 == 1:
                cache.put(key, step, now=step)
            else:
                cache.delete(key)
            self.assertSound(cache)

    def test_deleting_the_only_entry_leaves_room_for_more(self):
        cache = TTLCache(1)
        cache.put("a", 1, now=0)
        self.assertTrue(cache.delete("a"))
        self.assertFalse(cache.delete("a"))
        cache.put("b", 2, now=0)
        self.assertEqual(cache.keys(now=0), ["b"])
        self.assertSound(cache)


class ReplaceTest(ChainChecked):
    def test_writing_a_held_key_pushes_nothing_out(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0)
        cache.put("b", 2, now=0)
        cache.put("a", 3, now=0)
        self.assertEqual(cache.keys(now=0), ["a", "b"])
        self.assertEqual(cache.peek("a", now=0), 3)
        self.assertEqual(cache.stats()["evictions"], 0)
        self.assertSound(cache)

    def test_writing_the_newest_key_again_changes_nothing_else(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0)
        cache.put("a", 2, now=0)
        cache.put("a", 3, now=0)
        self.assertEqual(cache.keys(now=0), ["a"])
        self.assertSound(cache)

    def test_writing_replaces_the_expiry_too(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0, ttl=5)
        cache.put("a", 2, now=1, ttl=None)
        self.assertEqual(cache.get("a", now=100), 2)
        cache.put("a", 3, now=100, ttl=5)
        self.assertIsNone(cache.get("a", now=105))


class ExpiryTest(ChainChecked):
    def test_an_entry_is_gone_at_the_very_moment_it_expires(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=10, ttl=5)
        self.assertEqual(cache.get("a", now=14), 1)
        self.assertIsNone(cache.get("a", now=15))
        self.assertEqual(len(cache), 0)

    def test_peek_and_keys_agree_on_that_moment(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=10, ttl=5)
        self.assertEqual(cache.peek("a", now=14), 1)
        self.assertIsNone(cache.peek("a", now=15))
        self.assertEqual(cache.keys(now=15), [])
        self.assertEqual(cache.stats()["expirations"], 1)

    def test_a_ttl_of_zero_or_less_stores_nothing(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0, ttl=0)
        cache.put("b", 1, now=0, ttl=-3)
        self.assertEqual(len(cache), 0)
        self.assertEqual(cache.stats()["evictions"], 0)

    def test_a_ttl_of_zero_removes_what_the_key_held(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0)
        cache.put("a", 2, now=0, ttl=0)
        self.assertEqual(len(cache), 0)
        self.assertIsNone(cache.get("a", now=0))
        self.assertSound(cache)

    def test_a_ttl_of_zero_pushes_nothing_out_of_a_full_cache(self):
        cache = TTLCache(1)
        cache.put("a", 1, now=0)
        cache.put("b", 2, now=0, ttl=0)
        self.assertEqual(cache.keys(now=0), ["a"])


class RoomTest(ChainChecked):
    def test_expired_entries_go_before_any_live_one(self):
        cache = TTLCache(3)
        cache.put("old", 1, now=0)
        cache.put("short", 2, now=0, ttl=5)
        cache.put("new", 3, now=0)
        cache.put("next", 4, now=6)
        self.assertEqual(cache.keys(now=6), ["next", "new", "old"])
        self.assertEqual(cache.stats()["evictions"], 0)
        self.assertEqual(cache.stats()["expirations"], 1)
        self.assertSound(cache)

    def test_the_earliest_expired_goes_first_and_no_more_than_needed(self):
        cache = TTLCache(3)
        cache.put("later", 1, now=0, ttl=8)
        cache.put("earlier", 2, now=0, ttl=4)
        cache.put("live", 3, now=0)
        cache.put("next", 4, now=9)
        self.assertEqual(sorted(cache._nodes), ["later", "live", "next"])
        self.assertEqual(cache.stats()["expirations"], 1)

    def test_with_nothing_expired_the_least_recently_used_goes(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0, ttl=50)
        cache.put("b", 2, now=0, ttl=50)
        cache.put("c", 3, now=1)
        self.assertEqual(cache.keys(now=1), ["c", "b"])
        self.assertEqual(cache.stats()["evictions"], 1)
        self.assertEqual(cache.stats()["expirations"], 0)
        self.assertSound(cache)


class StatsTest(ChainChecked):
    def test_every_way_of_counting(self):
        cache = TTLCache(2)
        cache.put("a", 1, now=0, ttl=5)
        cache.get("a", now=1)
        cache.get("nothing", now=1)
        cache.get("a", now=5)
        cache.get("a", now=6)
        self.assertEqual(cache.stats(), {"hits": 1, "misses": 3, "evictions": 0, "expirations": 1})

    def test_removals_found_by_keys_count_once(self):
        cache = TTLCache(3)
        cache.put("a", 1, now=0, ttl=1)
        cache.put("b", 2, now=0, ttl=1)
        cache.keys(now=2)
        cache.keys(now=2)
        self.assertEqual(cache.stats()["expirations"], 2)
        self.assertSound(cache)


if __name__ == "__main__":
    unittest.main()
