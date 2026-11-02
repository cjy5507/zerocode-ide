"""Version numbers and the ranges that select them.

A version is MAJOR.MINOR.PATCH, optionally followed by -PRERELEASE and +BUILD:

    1.4.0   2.0.0-rc.1   1.0.0+20261102   3.1.0-beta.2+exp-7

Precedence:
  * MAJOR, MINOR and PATCH compare numerically.
  * A version with a prerelease comes before the same version without one.
  * Prereleases compare identifier by identifier, left to right. Identifiers made
    only of digits compare numerically; others compare as text; a numeric identifier
    comes before a non-numeric one. When all shared identifiers are equal, the version
    with fewer identifiers comes first.
  * BUILD never takes part in precedence, and may itself hold hyphens.

A range is one or more alternatives joined by `||`. An alternative is one or more
comparators separated by spaces, and a version must satisfy all of them:

    >=1.2.3  >1.2.3  <=1.2.3  <1.2.3  =1.2.3  1.2.3   exact comparisons
    1.2.x  1.x  *                                     any version with those leading numbers
    ~1.2.3                                            >=1.2.3 and below the next minor
    ^1.2.3                                            >=1.2.3 and below the next change of
                                                      the left-most non-zero number:
                                                      ^1.2.3 <2.0.0, ^0.2.3 <0.3.0,
                                                      ^0.0.3 <0.0.4

A version with a prerelease satisfies an alternative only when that alternative also
names a prerelease of the very same MAJOR.MINOR.PATCH in one of its comparators:
`>=1.2.3-rc.1 <2.0.0` takes 1.2.3-rc.2 but not 1.3.0-rc.1, and `^1.2.3` takes neither.
"""

import re

_VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([^+]*))?(?:\+(.*))?$")


class Version:
    def __init__(self, major, minor, patch, prerelease=(), build=""):
        self.major = major
        self.minor = minor
        self.patch = patch
        self.prerelease = tuple(prerelease)
        self.build = build

    def numbers(self):
        return (self.major, self.minor, self.patch)

    def __repr__(self):
        text = ".".join(str(n) for n in self.numbers())
        if self.prerelease:
            text += "-" + ".".join(self.prerelease)
        if self.build:
            text += "+" + self.build
        return text


def parse(text):
    """The Version TEXT spells; ValueError when it spells none."""
    head, _, build = text.partition("-")
    head, plus, tail = head.partition("+")
    prerelease = ""
    if plus:
        build = tail + ("-" + build if build else "")
    else:
        prerelease, _, build = build.partition("+")
    found = _VERSION.match(head)
    if not found:
        raise ValueError(f"not a version: {text!r}")
    identifiers = prerelease.split(".") if prerelease else []
    return Version(int(found[1]), int(found[2]), int(found[3]), identifiers, build)


def _key(version):
    return (version.numbers(), version.prerelease)


def compare(a, b):
    """Negative when A comes before B, zero when they tie, positive when A comes after."""
    left, right = _key(a), _key(b)
    return (left > right) - (left < right)


def _bound(op, version):
    tests = {
        ">=": lambda c: c >= 0,
        ">": lambda c: c > 0,
        "<=": lambda c: c <= 0,
        "<": lambda c: c < 0,
        "=": lambda c: c == 0,
    }
    test = tests[op]
    return (version, lambda candidate: test(compare(candidate, version)))


def _next_after(numbers, index):
    bumped = list(numbers)
    bumped[index] += 1
    for later in range(index + 1, 3):
        bumped[later] = 0
    return Version(*bumped)


def _expand(token):
    """The comparators one written TOKEN of a range stands for."""
    for op in (">=", "<=", ">", "<", "="):
        if token.startswith(op):
            return [_bound(op, parse(token[len(op):]))]
    if token == "*":
        return [_bound(">=", Version(0, 0, 0))]
    if token.startswith("~"):
        low = parse(token[1:])
        return [_bound(">=", low), _bound("<", _next_after(low.numbers(), 1))]
    if token.startswith("^"):
        low = parse(token[1:])
        return [_bound(">=", low), _bound("<", _next_after(low.numbers(), 0))]
    parts = token.split(".")
    if "x" in parts:
        held = [int(p) for p in parts[: parts.index("x")]]
        low = Version(*(held + [0] * (3 - len(held))))
        return [_bound(">=", low), _bound("<", _next_after(low.numbers(), len(held) - 1))]
    return [_bound("=", parse(token))]


def _alternative_takes(comparators, version):
    return all(test(version) for _, test in comparators)


def matches(text, range_text):
    """Whether the version TEXT lies in the range RANGE_TEXT."""
    version = parse(text)
    for alternative in range_text.split("||"):
        comparators = [c for token in alternative.split() for c in _expand(token)]
        if _alternative_takes(comparators, version):
            return True
    return False


def best(texts, range_text):
    """The highest of the version TEXTS in the range, or None. The first written wins a tie."""
    found = None
    for text in texts:
        if matches(text, range_text) and (found is None or compare(parse(text), parse(found)) > 0):
            found = text
    return found
