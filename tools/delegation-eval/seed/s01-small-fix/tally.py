"""A tiny word tally: the seed of the 1×1 sample (scenario s01-small-fix)."""


def tally(text):
    """Each word of TEXT and how many times it occurs, case kept."""
    counts = {}
    for word in text.split(" "):
        counts[word] = counts.get(word, 0) + 1
    return counts


def top(text, n):
    """The N most frequent words of TEXT, most frequent first."""
    counts = tally(text)
    return sorted(counts, key=lambda word: counts[word])[:n]
