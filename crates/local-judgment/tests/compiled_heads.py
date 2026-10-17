import ast
import hashlib
import json
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import patch

from laya import Agent, common
from transformers import AutoTokenizer


source = ast.parse(sys.argv[1])
definitions = [
    node for node in source.body
    if isinstance(node, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))
]
bridge = {}
exec(compile(ast.Module(body=definitions, type_ignores=[]), "owned-bridge", "exec"), bridge)
model = Path(sys.argv[2])
config = json.loads((model / "rl_agent_config.json").read_text())
tokenizer = AutoTokenizer.from_pretrained(model / "tokenizer", local_files_only=True, trust_remote_code=False)
budget = config["head_max_len"]
native_build_head = common.build_head
measured_common = SimpleNamespace(build_head=native_build_head)


class MeasuredAgent:
    _to_internal = staticmethod(Agent._to_internal)

    def __init__(self, directory, device, compile):
        self.cfg = config
        self.tok = tokenizer
        self.submitted = []

    def predict(self, state, questions):
        self.submitted.append((state, questions))
        answers = {}
        for name, question in questions.items():
            measured_common.build_head(self.tok, self._to_internal(question), budget)
            if question["type"] == "choice":
                labels = list(question["criteria"])
                winner = labels[-1]
                answers[name] = {
                    "type": "choice", "choice": winner,
                    "probabilities": {label: float(label == winner) for label in labels},
                    "confidence": 1.0, "answer_confidence": 1.0,
                }
            else:
                answers[name] = {"type": "noul", "noul": 0.0, "confidence": 1.0}
        return {"answers": answers, "usage": {"input_tokens": 0, "output_tokens": 0}}


def measure(question):
    internal = Agent._to_internal(question)
    head, whole = bridge["measured_head"](native_build_head, tokenizer, internal, budget)
    counted = bridge["EncodingRecord"](tokenizer)
    native_build_head(counted, internal, budget)
    return {
        "whole": whole, "tokens": len(head[0]),
        "instruction_tokens": len(counted.whole[0]), "instruction_kept": head[1][0] - 2,
    }


rows = []
for fixture in json.load(sys.stdin):
    request = fixture["request"]
    before = {name: measure(question) for name, question in request["questions"].items()}
    measured_common.build_head = native_build_head
    with patch.dict(sys.modules, {"laya": SimpleNamespace(Agent=MeasuredAgent, common=measured_common)}):
        adapter = bridge["LayaAdapter"]({"modelDirectory": str(model), "modelId": "loaded", "runtime": "loaded"})
    result = adapter.predict(request)
    submitted = [question for _, batch in adapter.agent.submitted for question in batch.values()]
    after = [measure(question) for question in submitted]
    seen = set()
    intact = True
    guards = {name: 0 for name, question in request["questions"].items() if question["type"] == "noul"}
    for state, batch in adapter.agent.submitted:
        intact = intact and state == request["state"]
        for question in batch.values():
            originals = [original for original in request["questions"].values()
                         if original["instructions"] == question["instructions"]]
            intact = intact and bool(originals)
            for name in guards:
                if question == request["questions"][name]:
                    guards[name] += 1
            if question["type"] == "choice":
                seen.update(question["criteria"])
                intact = intact and all(
                    request["questions"]["action"]["criteria"].get(label) == description
                    for label, description in question["criteria"].items()
                )
    expected = set(request["questions"]["action"]["criteria"])
    rows.append({
        "shape": fixture["shape"], "before": before, "after": after,
        "calls": len(adapter.agent.submitted),
        "pass": all(head["whole"] for head in after)
                and not result["usage"].get("truncated_questions") and intact and seen == expected
                and set(result["answers"]) == set(request["questions"])
                and all(count == 1 for count in guards.values()),
    })
print(json.dumps({
    "head_max_len": budget, "rows": rows, "runtime_executable": sys.executable,
    "model_directory": str(model), "bridge_sha256": hashlib.sha256(sys.argv[1].encode()).hexdigest(),
}))
