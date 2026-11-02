"""One record of the log as bytes, and back."""

import json


class CorruptRecord(ValueError):
    """A record that cannot be read back. `seq` is its number, where that is known."""

    def __init__(self, message, seq=None):
        super().__init__(message)
        self.seq = seq


def encode(payload):
    return json.dumps(payload, sort_keys=True).encode() + b"\n"


def decode(line):
    try:
        return json.loads(line)
    except ValueError as err:
        raise CorruptRecord(str(err)) from None
