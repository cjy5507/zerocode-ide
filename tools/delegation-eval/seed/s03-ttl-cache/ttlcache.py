"""A bounded cache whose entries may also expire.

Time is whatever number the caller passes as NOW; nothing here reads a clock.

Rules:
  * An entry written with TTL expires at NOW + TTL. It is gone from NOW + TTL on: at that
    very moment it no longer counts as present. TTL None means it never expires, and a TTL
    of zero or less stores nothing (and removes any entry already held under that key).
  * `get` of a live entry makes it the most recently used. `peek` reads without doing so.
  * `put` of a key already held replaces its value and its expiry and makes it the most
    recently used; it never pushes another entry out.
  * When a new key does not fit, entries that have expired by now go first, the one that
    expired earliest before the others; only when none has expired does the least recently
    used entry go, and only as many as needed.
  * `keys(now)` lists the live entries, most recently used first, and removes the expired
    ones it meets. `__len__` counts every entry still stored, expired or not.
  * `stats()` counts: `hits` for `get` of a live entry, `misses` for `get` of a missing or
    expired entry, `evictions` for live entries pushed out to make room, and `expirations`
    for expired entries removed by any means. Each removal counts once.
"""


class _Node:
    __slots__ = ("key", "value", "expires", "older", "newer")

    def __init__(self, key, value, expires):
        self.key = key
        self.value = value
        self.expires = expires
        self.older = None
        self.newer = None


class TTLCache:
    def __init__(self, capacity):
        if capacity < 1:
            raise ValueError("capacity must be at least 1")
        self.capacity = capacity
        self._nodes = {}
        self._newest = None
        self._oldest = None
        self._stats = {"hits": 0, "misses": 0, "evictions": 0, "expirations": 0}

    def __len__(self):
        return len(self._nodes)

    def stats(self):
        return dict(self._stats)

    def _link_newest(self, node):
        node.older = self._newest
        node.newer = None
        if self._newest is not None:
            self._newest.newer = node
        self._newest = node
        if self._oldest is None:
            self._oldest = node

    def _unlink(self, node):
        if node.older is not None:
            node.older.newer = node.newer
        if node.newer is not None:
            node.newer.older = node.older
        if self._newest is node:
            self._newest = node.older
        node.older = node.newer = None

    def _remove(self, node):
        self._unlink(node)
        del self._nodes[node.key]

    @staticmethod
    def _expired(node, now):
        return node.expires is not None and now > node.expires

    def _evict_one(self):
        self._remove(self._oldest)
        self._stats["evictions"] += 1

    def put(self, key, value, now, ttl=None):
        expires = None if ttl is None else now + ttl
        while len(self._nodes) >= self.capacity:
            self._evict_one()
        held = self._nodes.get(key)
        if held is not None:
            held.value = value
            held.expires = expires
            self._unlink(held)
            self._link_newest(held)
            return
        node = _Node(key, value, expires)
        self._nodes[key] = node
        self._link_newest(node)

    def get(self, key, now, default=None):
        node = self._nodes.get(key)
        if node is None:
            self._stats["misses"] += 1
            return default
        if self._expired(node, now):
            self._remove(node)
            self._stats["expirations"] += 1
            self._stats["misses"] += 1
            return default
        self._stats["hits"] += 1
        self._unlink(node)
        self._link_newest(node)
        return node.value

    def peek(self, key, now, default=None):
        node = self._nodes.get(key)
        if node is None or self._expired(node, now):
            return default
        return node.value

    def delete(self, key):
        node = self._nodes.get(key)
        if node is None:
            return False
        self._remove(node)
        return True

    def keys(self, now):
        found = []
        node = self._newest
        while node is not None:
            older = node.older
            if self._expired(node, now):
                self._remove(node)
                self._stats["expirations"] += 1
            else:
                found.append(node.key)
            node = older
        return found

    def _chain(self):
        """The keys walked both ways, for checking that the two walks agree."""
        down, node = [], self._newest
        while node is not None:
            down.append(node.key)
            node = node.older
        up, node = [], self._oldest
        while node is not None:
            up.append(node.key)
            node = node.newer
        return down, up
