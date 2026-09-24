import importlib.util
import json
import tempfile
from pathlib import Path


SOURCE = Path(__file__).resolve().parents[1] / "skill-pick-replay" / "seed.py"
SPEC = importlib.util.spec_from_file_location("skill_pick_seed", SOURCE)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_no_gold_does_not_turn_agent_loads_into_accuracy():
    rows = [{"id": "one", "firstLoad": "skill-a", "skillCalls": 1},
            {"id": "two", "firstLoad": None, "skillCalls": 0}]
    result = MODULE.evaluate(rows, {}, {})
    assert result["covered"] == 0
    assert result["uncovered"] == 0
    assert result["completionMet"] is False


def test_paired_gold_counts_fixes_and_harms():
    rows = [{"id": "one", "firstLoad": "wrong", "skillCalls": 1},
            {"id": "two", "firstLoad": None, "skillCalls": 0},
            {"id": "three", "firstLoad": "good", "skillCalls": 1}]
    gold = {"one": {"skill": "good"}, "two": {"skill": None},
            "three": {"skill": "good"}}
    assisted = {"one": {"firstLoad": "good"}, "two": {"firstLoad": None},
                "three": {"firstLoad": "wrong"}}
    result = MODULE.evaluate(rows, gold, assisted)
    assert (result["fixed"], result["harmed"]) == (1, 1)
    assert result["completionMet"] is False


def test_ledger_reads_disagreement_and_unused_proxy_separately():
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        # The suggestion's own ledger, and the search's it shared before (t-6877).
        own = root / "project" / "state" / "smart-router" / "skill-suggestion.jsonl"
        own.parent.mkdir(parents=True)
        own.write_text("\n".join(json.dumps(row) for row in [
            {"at": 100, "agreed": False, "unusedLoad": True, "baselineUnusedLoad": True},
        ]) + "\n")
        shared = own.with_name("skill-search.jsonl")
        shared.write_text("\n".join(json.dumps(row) for row in [
            {"at": 101, "agreed": True, "unusedLoad": False},
        ]) + "\n")
        result = MODULE.observed_labels(root, 100)
        assert result["compared"] == 2
        assert result["disagreed"] == 1
        assert result["unusedLoad"] == 1
        assert result["baselineUnusedObserved"] == 1


if __name__ == "__main__":
    test_no_gold_does_not_turn_agent_loads_into_accuracy()
    test_paired_gold_counts_fixes_and_harms()
    test_ledger_reads_disagreement_and_unused_proxy_separately()
