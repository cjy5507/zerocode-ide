"""The log file: one record per line, numbered by where the line stands."""

from pathlib import Path

import frames


class Log:
    def __init__(self, path):
        self.path = Path(path)

    def _lines(self):
        if not self.path.exists():
            return []
        return self.path.read_bytes().splitlines()

    def append(self, payload):
        """Add a record; its number."""
        number = len(self._lines())
        with open(self.path, "ab") as out:
            out.write(frames.encode(payload))
        return number

    def records(self, start=0):
        """(number, payload) of every record from number START on, oldest first."""
        return [(number, frames.decode(line)) for number, line in enumerate(self._lines()) if number >= start]
