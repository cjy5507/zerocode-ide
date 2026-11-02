"""A consumer's place in the log, kept in a small file of its own."""

import json
from pathlib import Path


class Cursor:
    def __init__(self, log, path):
        self.log = log
        self.path = Path(path)
        self.next = json.loads(self.path.read_text())["next"] if self.path.exists() else 0
        self._polled = self.next

    def poll(self, limit):
        """Up to LIMIT records after the committed place, as (number, payload)."""
        batch = self.log.records()[self.next:self.next + limit]
        self._polled = self.next + len(batch)
        return batch

    def commit(self):
        """Everything the last poll returned has been handled."""
        self.next = self._polled
        self.path.write_text(json.dumps({"next": self.next}))
