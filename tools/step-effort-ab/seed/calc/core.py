"""A small calculation toolkit used by the step-effort A/B tasks."""

from __future__ import annotations

import re


def mean(values: list[float]) -> float:
    if not values:
        raise ValueError("mean of an empty list")
    return sum(values) / len(values)


_DURATION = re.compile(r"(\d+)([hms])")


def parse_duration(text: str) -> int:
    """Seconds in a duration written like ``1h30m`` or ``45s``."""
    total = 0
    for amount, unit in _DURATION.findall(text):
        if unit == "h":
            total += int(amount) * 60
        elif unit == "m":
            total += int(amount) * 60
        else:
            total += int(amount)
    return total


def word_count(text: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for word in text.split():
        counts[word] = counts.get(word, 0) + 1
    return counts


def paginate(items: list, page: int, per_page: int) -> list:
    """The ``page``-th page (1-based) of ``items``."""
    start = page * per_page
    return items[start : start + per_page]
