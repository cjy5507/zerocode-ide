import ast
import hashlib
import importlib.metadata
import json
import pickle
import sys
import time

import torch


source = ast.parse(sys.argv[1])
definitions = [node for node in source.body
               if isinstance(node, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))]
bridge = {}
exec(compile(ast.Module(body=definitions, type_ignores=[]), "owned-bridge", "exec"), bridge)
initial = json.loads(sys.argv[2])
for package, version in initial["packages"].items():
    if importlib.metadata.version(package) != version:
        raise ValueError("runtime_version")
bridge["configure_loaders"](initial)
torch.set_num_threads(2)
torch.set_num_interop_threads(1)
torch.load = bridge["refuse_pickle"]
torch.jit.load = bridge["refuse_pickle"]
pickle.load = bridge["refuse_pickle"]
pickle.loads = bridge["refuse_pickle"]
sys.addaudithook(bridge["offline_process"])
started = time.perf_counter()
adapter = bridge["LayaAdapter"](initial)
startup_ms = (time.perf_counter() - started) * 1000
native_predict = adapter.agent.predict
calls = []


def counted_predict(state, questions):
    started = time.perf_counter()
    result = native_predict(state, questions)
    calls.append({"questions": len(questions), "elapsed_ms": (time.perf_counter() - started) * 1000})
    return result


adapter.agent.predict = counted_predict
rows = []
for fixture in json.load(sys.stdin):
    for repetition in range(3):
        calls.clear()
        started = time.perf_counter()
        result = adapter.predict(fixture["request"])
        elapsed_ms = (time.perf_counter() - started) * 1000
        result["model"] = initial["modelId"]
        rows.append({
            "shape": fixture["shape"], "repetition": repetition,
            "request_sha256": hashlib.sha256(json.dumps(fixture["request"], sort_keys=True).encode()).hexdigest(),
            "elapsed_ms": elapsed_ms, "calls": list(calls), "result": result,
        })
print(json.dumps({
    "startup_ms": startup_ms, "rows": rows, "runtime_executable": sys.executable,
    "model_directory": initial["modelDirectory"], "loaded_limits": adapter.limits,
    "bridge_sha256": hashlib.sha256(sys.argv[1].encode()).hexdigest(),
}))
