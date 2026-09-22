#!/usr/bin/env python3
"""Hold what the codegraph index answers against rust-analyzer (t-5970).

The index links names by spelling alone. rust-analyzer's LSIF knows, for every
identifier in a Rust workspace, the definition it resolves to — so it is the
ground truth for two questions the index's users act on:

- **links** (`index_cost links`): when the index says file F uses file G, or
  test T uses F, does F really depend on G (some identifier in F resolves to a
  definition in G)? Precision over every link, and over the eight per list a
  neighbour line can show.
- **neighbours** (`runtime --test neighbours_measure`): of the imports and
  tests a read's neighbour line lists — the text's own, then the index's
  under the same per-relation cap — how many are real, and how many real
  dependencies each finds.
- **references** (`index_cost sample`): of the occurrences `find_references`
  answers for a definition, how many resolve to that definition — and what
  would two cheap filters keep: an occurrence whose file's imports spell the
  name (or that sits in the defining file), and the same but keeping every
  occurrence of a name only one file defines.

    rust-analyzer lsif <snapshot>/zo-ide > truth.lsif
    index_cost links  --workspace <snapshot> --cache-dir <c> > links.json
    index_cost sample --workspace <snapshot> --cache-dir <c> --under zo-ide > sample.json
    ZO_MEASURE_NEIGHBOURS_ROOT=<snapshot> ZO_MEASURE_NEIGHBOURS_FILES=<list> \\
    ZO_MEASURE_NEIGHBOURS_OUT=neighbours.json \\
        cargo test -p runtime --test neighbours_measure -- --ignored
    tools/codegraph-bench/truth.py --lsif truth.lsif --root <snapshot> \\
        --links links.json --neighbours neighbours.json --sample sample.json

Only Rust files the LSIF covers are judged; everything else is counted as not
judged. LSIF columns are UTF-16 code units, the index's are bytes: each
occurrence is converted through its own source line.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from urllib.parse import unquote, urlparse

# How many of one relation a neighbour line can show (runtime
# file_neighbours: MAX_PER_RELATION) — the cut the "shown" precision uses.
SHOWN_PER_RELATION = 8
# Buckets of how many files define a sampled name: the one number that
# explains an exact-name answer's precision (a name one file defines is
# almost always meant for it; `new` is defined in hundreds).
DEFINER_BUCKETS = ((1, 1, "1"), (2, 9, "2-9"), (10, None, "10+"))


def definer_bucket(definers: int) -> str:
    for low, high, label in DEFINER_BUCKETS:
        if definers >= low and (high is None or definers <= high):
            return label
    return DEFINER_BUCKETS[0][2]


class Lsif:
    """The part of an LSIF dump this script reads: which definition every
    range resolves to, and where every range starts."""

    def __init__(self) -> None:
        self.documents: dict[int, str] = {}
        self.range_start: dict[int, tuple[int, int]] = {}
        self.range_document: dict[int, int] = {}
        self.range_result: dict[int, int] = {}
        self.result_definition: dict[int, int] = {}
        self.definition_results: set[int] = set()
        self.definition_items: dict[int, list[int]] = defaultdict(list)

    @classmethod
    def read(cls, lines, root: Path) -> "Lsif":
        lsif = cls()
        for line in lines:
            lsif.take(json.loads(line), root)
        return lsif

    def take(self, element: dict, root: Path) -> None:
        label = element.get("label")
        if element.get("type") == "vertex":
            if label == "document":
                path = Path(unquote(urlparse(element["uri"]).path))
                try:
                    self.documents[element["id"]] = path.relative_to(root).as_posix()
                except ValueError:
                    pass
            elif label == "range":
                start = element["start"]
                self.range_start[element["id"]] = (start["line"], start["character"])
            elif label == "definitionResult":
                self.definition_results.add(element["id"])
            return
        if label == "contains":
            for range_id in element["inVs"]:
                self.range_document[range_id] = element["outV"]
        elif label == "next":
            self.range_result[element["outV"]] = element["inV"]
        elif label == "textDocument/definition":
            self.result_definition[element["outV"]] = element["inV"]
        elif label == "item" and element["outV"] in self.definition_results:
            self.definition_items[element["outV"]].extend(element["inVs"])

    def location(self, range_id: int) -> tuple[str, int, int] | None:
        document = self.documents.get(self.range_document.get(range_id))
        if document is None:
            return None
        line, character = self.range_start[range_id]
        return document, line, character

    def definitions(self, range_id: int) -> set[tuple[str, int, int]]:
        result = self.result_definition.get(self.range_result.get(range_id))
        found = set()
        for target in self.definition_items.get(result, ()):
            location = self.location(target)
            if location is not None:
                found.add(location)
        return found

    def ranges_by_start(self) -> dict[tuple[str, int, int], int]:
        by_start = {}
        for range_id in self.range_start:
            location = self.location(range_id)
            if location is not None:
                by_start[location] = range_id
        return by_start

    def referrers(self, targets: set[tuple[str, int, int]]) -> dict[tuple, set[int]]:
        """Every range resolving to one of `targets`, but the target's own."""
        found: dict[tuple, set[int]] = defaultdict(set)
        for range_id in self.range_start:
            location = self.location(range_id)
            for target in self.definitions(range_id) & targets:
                if target != location:
                    found[target].add(range_id)
        return found

    def dependencies(self) -> dict[str, set[str]]:
        """File → the other files some identifier in it resolves into."""
        depends: dict[str, set[str]] = defaultdict(set)
        for range_id in self.range_start:
            location = self.location(range_id)
            if location is None:
                continue
            for target, _, _ in self.definitions(range_id):
                if target != location[0]:
                    depends[location[0]].add(target)
        return depends


class Columns:
    """Byte columns → UTF-16 columns, reading each source line once."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.lines: dict[str, list[bytes]] = {}

    def utf16(self, file: str, row: int, byte_column: int) -> int | None:
        if file not in self.lines:
            try:
                self.lines[file] = (self.root / file).read_bytes().split(b"\n")
            except OSError:
                self.lines[file] = []
        lines = self.lines[file]
        if row >= len(lines):
            return None
        try:
            prefix = lines[row][:byte_column].decode("utf-8")
        except UnicodeDecodeError:
            return None
        return len(prefix.encode("utf-16-le")) // 2


def precision(confirmed: int, total: int) -> float | None:
    return None if total == 0 else confirmed / total


def judge_links(links: dict, lsif: Lsif) -> dict:
    """Precision of `uses` links and of test `used_by` links, all and shown,
    and the share of each file's real dependencies its `uses` found."""
    covered = set(lsif.documents.values())
    depends = lsif.dependencies()
    tally = defaultdict(int)
    files_with = defaultdict(int)
    judged_files = 0
    dependencies = found_dependencies = 0
    for entry in links["files"]:
        source = entry["file"]
        if source not in covered:
            continue
        judged_files += 1
        uses = [linked for linked in entry["uses"] if linked["file"] in covered]
        tests = [linked for linked in entry["used_by"]
                 if linked["test"] and linked["file"] in covered]
        files_with["uses"] += bool(uses)
        files_with["tests"] += bool(tests)
        real = depends.get(source, set()) & covered
        dependencies += len(real)
        found_dependencies += len(real & {linked["file"] for linked in uses})
        for rank, linked in enumerate(uses):
            hit = linked["file"] in depends.get(source, ())
            tally["uses", "all", hit] += 1
            if rank < SHOWN_PER_RELATION:
                tally["uses", "shown", hit] += 1
        for rank, linked in enumerate(tests):
            hit = source in depends.get(linked["file"], ())
            tally["tests", "all", hit] += 1
            if rank < SHOWN_PER_RELATION:
                tally["tests", "shown", hit] += 1
    report = {
        "judged_files": judged_files,
        "files_with": dict(files_with),
        "uses_recall": precision(found_dependencies, dependencies),
    }
    for relation in ("uses", "tests"):
        for cut in ("all", "shown"):
            confirmed = tally[relation, cut, True]
            total = confirmed + tally[relation, cut, False]
            report[f"{relation}_{cut}"] = {
                "links": total,
                "confirmed": confirmed,
                "precision": precision(confirmed, total),
            }
    return report


def judge_neighbours(texts: dict, links: dict, lsif: Lsif) -> dict:
    """The neighbour line before (the text's own imports and tests) and after
    (then the index's, up to the per-relation cap), against LSIF: precision,
    recall of each file's real dependencies, and files the index added a
    real neighbour to."""
    covered = set(lsif.documents.values())
    depends = lsif.dependencies()
    index_by_file = {entry["file"]: entry for entry in links["files"]}
    tally = defaultdict(int)
    for entry in texts["files"]:
        source = entry["file"]
        if source not in covered:
            continue
        tally["files"] += 1
        real = depends.get(source, set()) & covered
        texts_imports = [n["path"] for n in entry["neighbours"] if n["relation"] == "imports"]
        texts_tests = [n["path"] for n in entry["neighbours"] if n["relation"] == "tests"]
        indexed = index_by_file.get(source, {"uses": [], "used_by": []})
        added_imports = [linked["file"] for linked in indexed["uses"]
                         if linked["file"] not in texts_imports]
        added_tests = [linked["file"] for linked in indexed["used_by"]
                       if linked["test"] and linked["file"] not in texts_tests]
        stages = {
            "before": (texts_imports, texts_tests),
            "after": (
                (texts_imports + added_imports)[:max(SHOWN_PER_RELATION, len(texts_imports))],
                (texts_tests + added_tests)[:max(SHOWN_PER_RELATION, len(texts_tests))],
            ),
        }
        for stage, (imports, tests) in stages.items():
            judged_imports = [path for path in imports if path in covered]
            judged_tests = [path for path in tests if path in covered]
            tally[stage, "imports"] += len(judged_imports)
            tally[stage, "imports_true"] += sum(path in real for path in judged_imports)
            tally[stage, "tests"] += len(judged_tests)
            tally[stage, "tests_true"] += sum(
                source in depends.get(path, set()) for path in judged_tests
            )
            tally[stage, "dependencies"] += len(real)
            tally[stage, "dependencies_found"] += len(real & set(judged_imports))
        before_true = {path for path in stages["before"][0] if path in real}
        after_true = {path for path in stages["after"][0] if path in real}
        after_tests_true = {
            path for path in stages["after"][1] if source in depends.get(path, set())
        }
        before_tests_true = {
            path for path in stages["before"][1] if source in depends.get(path, set())
        }
        tally["files_gaining"] += bool(after_true - before_true or after_tests_true - before_tests_true)
    report = {"judged_files": tally["files"], "files_gaining_a_real_neighbour": tally["files_gaining"]}
    for stage in ("before", "after"):
        report[stage] = {
            "imports": tally[stage, "imports"],
            "imports_precision": precision(tally[stage, "imports_true"], tally[stage, "imports"]),
            "dependency_recall": precision(
                tally[stage, "dependencies_found"], tally[stage, "dependencies"]
            ),
            "tests": tally[stage, "tests"],
            "tests_precision": precision(tally[stage, "tests_true"], tally[stage, "tests"]),
            "real_tests": tally[stage, "tests_true"],
        }
    return report


# The filters an occurrence is weighed against: whether each keeps it.
FILTERS = {
    "exact_name": lambda sample, occurrence: True,
    "imports_spell": lambda sample, occurrence: (
        occurrence["imports_spell"] or occurrence["file"] == sample["definition"]["file"]
    ),
    "unique_or_imports_spell": lambda sample, occurrence: (
        sample["definers"] == 1
        or occurrence["imports_spell"]
        or occurrence["file"] == sample["definition"]["file"]
    ),
    # A method's container type imported counts too. Measured and not
    # adopted (t-5970): over three seeds it moved recall 96.4% → 97.8% and
    # precision 57.1% → 55.0% — within what one heavy name moves.
    "unique_or_imports_spell_name_or_container": lambda sample, occurrence: (
        sample["definers"] == 1
        or occurrence["imports_spell"]
        or occurrence.get("imports_spell_container", False)
        or occurrence["file"] == sample["definition"]["file"]
    ),
}


def judge_references(sample: dict, lsif: Lsif, root: Path) -> dict:
    """Per filter: pooled and per-definition precision and recall of the
    occurrences it keeps, against the definitions rust-analyzer resolves —
    overall and per bucket of how many files define the name."""
    covered = set(lsif.documents.values())
    by_start = lsif.ranges_by_start()
    columns = Columns(root)
    tallies = {name: defaultdict(int) for name in FILTERS}
    per_definition = {name: {"precision": [], "recall": []} for name in FILTERS}
    buckets = {name: defaultdict(lambda: defaultdict(int)) for name in FILTERS}
    bucket_sizes = defaultdict(int)
    judged = unresolved = skipped = 0
    targets = {}
    for position, drawn in enumerate(sample["samples"]):
        definition = drawn["definition"]
        if definition["file"] not in covered:
            continue
        start = definition["range"]["start"]
        column = columns.utf16(definition["file"], start["row"], start["column"])
        target = (definition["file"], start["row"], column)
        if target in by_start:
            targets[position] = target
    # Every place rust-analyzer resolves to each definition but the
    # definition itself: the references a perfect answer would list.
    referrers = lsif.referrers(set(targets.values()))
    for position, drawn in enumerate(sample["samples"]):
        target = targets.get(position)
        if target is None:
            skipped += 1
            continue
        judged += 1
        bucket = definer_bucket(drawn["definers"])
        bucket_sizes[bucket] += 1
        truth = referrers.get(target, set())
        verdicts = []
        for occurrence in drawn["occurrences"]:
            if occurrence["file"] not in covered:
                continue
            column = columns.utf16(occurrence["file"], occurrence["row"], occurrence["column"])
            range_id = by_start.get((occurrence["file"], occurrence["row"], column))
            if range_id is None:
                unresolved += 1
                continue
            verdicts.append((occurrence, target in lsif.definitions(range_id)))
        for name, keeps in FILTERS.items():
            kept = [hit for occurrence, hit in verdicts if keeps(drawn, occurrence)]
            for tally in (tallies[name], buckets[name][bucket]):
                tally["kept"] += len(kept)
                tally["kept_true"] += sum(kept)
                tally["true"] += len(truth)
            if kept:
                per_definition[name]["precision"].append(sum(kept) / len(kept))
            if truth:
                per_definition[name]["recall"].append(sum(kept) / len(truth))
    report = {"judged_definitions": judged, "skipped": skipped,
              "unresolved_occurrences": unresolved, "filters": {}}
    for name, tally in tallies.items():
        precisions = per_definition[name]["precision"]
        recalls = per_definition[name]["recall"]
        report["filters"][name] = {
            "kept": tally["kept"],
            "kept_true": tally["kept_true"],
            "pooled_precision": precision(tally["kept_true"], tally["kept"]),
            "pooled_recall": precision(tally["kept_true"], tally["true"]),
            "median_precision": statistics.median(precisions) if precisions else None,
            "median_recall": statistics.median(recalls) if recalls else None,
            "by_definers": {
                label: {
                    "definitions": bucket_sizes[label],
                    "kept": buckets[name][label]["kept"],
                    "precision": precision(
                        buckets[name][label]["kept_true"], buckets[name][label]["kept"]
                    ),
                    "recall": precision(
                        buckets[name][label]["kept_true"], buckets[name][label]["true"]
                    ),
                }
                for _, _, label in DEFINER_BUCKETS
            },
        }
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lsif", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path,
                        help="the snapshot the index and the LSIF were both made from")
    parser.add_argument("--links", type=Path, help="`index_cost links` output")
    parser.add_argument("--neighbours", type=Path,
                        help="`runtime --test neighbours_measure` output (needs --links)")
    parser.add_argument("--sample", type=Path, action="append", default=[],
                        help="`index_cost sample` output; repeat to pool several seeds")
    parser.add_argument("--out", type=Path, help="write the report as JSON")
    arguments = parser.parse_args(argv)
    root = arguments.root.resolve()
    with arguments.lsif.open() as lines:
        lsif = Lsif.read(lines, root)
    report = {"lsif_documents": len(lsif.documents)}
    if arguments.links:
        links = json.loads(arguments.links.read_text())
        report["links"] = judge_links(links, lsif)
        if arguments.neighbours:
            report["neighbours"] = judge_neighbours(
                json.loads(arguments.neighbours.read_text()), links, lsif
            )
    if arguments.sample:
        pooled = {"samples": []}
        for path in arguments.sample:
            pooled["samples"].extend(json.loads(path.read_text())["samples"])
        report["references"] = judge_references(pooled, lsif, root)
    text = json.dumps(report, indent=2, ensure_ascii=False)
    if arguments.out:
        arguments.out.write_text(text + "\n")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
