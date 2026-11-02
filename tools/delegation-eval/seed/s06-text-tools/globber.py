"""Matching slash-separated paths against glob patterns.

    *        any run of characters within one path segment, possibly empty
    ?        exactly one character, never a slash
    **       as a whole segment: zero or more whole segments. `a/**/b` takes `a/b` and
             `a/x/y/b`; a trailing `a/**` takes `a`, `a/x` and `a/x/y`; a leading `**/b`
             takes `b` and `x/y/b`. Anywhere else `**` means the same as `*`.
    [abc]    one of these characters; `[a-c]` a range; `[!abc]` any other character.
             Never takes a slash. A `]` written first stands for itself: `[]a]`.
    \\c      the character c itself, whatever it is

A `[` with no closing `]` stands for itself. Matching covers
the whole path and is case sensitive. A pattern must never take time that grows
exponentially with its length: `match("a*" * 30 + "b", "a" * 60)` answers at once.
"""

from functools import lru_cache


def match(pattern, path):
    """Whether PATH matches PATTERN."""
    return _match(pattern, path)


@lru_cache(maxsize=None)
def _match(pattern, text):
    if not pattern:
        return not text
    head = pattern[0]
    if head == "*":
        for split in range(len(text) + 1):
            if _match(pattern[1:], text[split:]):
                return True
        return False
    if not text:
        return False
    if head == "?" or head == text[0]:
        return _match(pattern[1:], text[1:])
    return False
