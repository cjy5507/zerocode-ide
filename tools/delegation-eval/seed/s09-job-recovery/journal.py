"""An append-only file of JSON events, one per line."""

import json


class Journal:
    def __init__(self, disk, name):
        self.disk = disk
        self.name = name

    def append(self, event):
        self.disk.append(self.name, json.dumps(event, sort_keys=True).encode() + b"\n")

    def events(self):
        data = self.disk.read(self.name) or b""
        return [json.loads(line) for line in data.splitlines()]

    def clear(self):
        self.disk.remove(self.name)

    def size(self):
        return len(self.disk.read(self.name) or b"")
