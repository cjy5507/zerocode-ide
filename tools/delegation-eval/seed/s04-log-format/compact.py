"""Drop the records a newer record has replaced."""

import os

import frames


def compact(log, key):
    """Keep, for each value of payload[KEY], only the newest record, and every record without KEY."""
    records = log.records()
    latest = {}
    for number, payload in records:
        latest[payload.get(key, ("unkeyed", number))] = number
    keep = set(latest.values())
    body = b"".join(frames.encode(payload) for number, payload in records if number in keep)
    temp = log.path.with_name(log.path.name + ".compacting")
    temp.write_bytes(body)
    os.replace(temp, log.path)
