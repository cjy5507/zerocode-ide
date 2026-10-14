import ast
import hashlib
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


source = ast.parse(json.loads(sys.argv[1]))
definitions = [
    node for node in source.body
    if isinstance(node, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))
]
bridge = {}
exec(compile(ast.Module(body=definitions, type_ignores=[]), "owned-bridge", "exec"), bridge)


class BridgeBoundary(unittest.TestCase):
    def test_loaders_force_offline_mode_and_replace_untrusted_templates(self):
        received = []
        tokenizer = SimpleNamespace(chat_template="untrusted template")

        def config_loader(*args, **kwargs):
            received.append(kwargs)
            return SimpleNamespace()

        def tokenizer_loader(*args, **kwargs):
            received.append(kwargs)
            return tokenizer

        transformers = SimpleNamespace(AutoConfig=SimpleNamespace(from_pretrained=config_loader), AutoTokenizer=SimpleNamespace(from_pretrained=tokenizer_loader))
        with patch.dict(sys.modules, {"transformers": transformers}):
            bridge["configure_loaders"]({"engine": "transformers_system_one", "chatTemplate": "reviewed template"})
            transformers.AutoConfig.from_pretrained("owned", trust_remote_code=True, local_files_only=False)
            loaded = transformers.AutoTokenizer.from_pretrained("owned", trust_remote_code=True, local_files_only=False)
        self.assertEqual(loaded.chat_template, "reviewed template")
        self.assertEqual(received, [{"trust_remote_code": False, "local_files_only": True}] * 2)

    def test_reviewed_runtime_refuses_model_code_and_hub_resolution(self):
        runtime = SimpleNamespace()
        with patch.dict(sys.modules, {"d3_runtime": runtime}):
            loaded = bridge["reviewed_system_one_runtime"]()
            self.assertIs(loaded, runtime)
            self.assertIsNone(sys.modules["d3_fast"])
            for operation in (loaded.load_file_module, loaded.resolve_dir, loaded._download):
                with self.assertRaisesRegex(ValueError, "unreviewed_code_disabled"):
                    operation("untrusted")
            self.assertIsNone(loaded.fast_module("untrusted"))
            self.assertFalse(loaded.vision_weights_present("untrusted"))

    def test_data_verification_rejects_corruption_and_directory_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            owned = root / "owned"
            owned.mkdir()
            payload = b"weights"
            (owned / "model.safetensors").write_bytes(payload)
            descriptor = {"path": "model.safetensors", "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}
            initial = {"modelDirectory": str(owned), "modelFiles": [descriptor]}
            self.assertEqual(bridge["verify_model_data"](initial), owned.resolve(strict=True))
            (owned / "model.safetensors").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "integrity_failed"):
                bridge["verify_model_data"](initial)
            (root / "outside.safetensors").write_bytes(payload)
            descriptor["path"] = "../outside.safetensors"
            with self.assertRaisesRegex(ValueError, "unsafe_path"):
                bridge["verify_model_data"](initial)

    def test_offline_process_refuses_network_and_subprocess_events(self):
        for event in ("socket.connect", "socket.getaddrinfo", "subprocess.Popen", "os.system", "os.posix_spawn"):
            with self.assertRaisesRegex(ValueError, "offline_process"):
                bridge["offline_process"](event, ())

    def test_pickle_is_refused_instead_of_deserializing_a_payload(self):
        with self.assertRaisesRegex(ValueError, "pickle_disabled"):
            bridge["refuse_pickle"](b"untrusted")

    def test_memory_guard_exits_when_measurement_fails(self):
        def failed_measurement():
            raise OSError("fixture")

        process = SimpleNamespace(memory_info=failed_measurement)
        with patch.object(bridge["os"], "_exit", side_effect=SystemExit(1)):
            with self.assertRaises(SystemExit):
                bridge["memory_guard"](process, 1024)


unittest.main(argv=["bridge-boundary"], verbosity=2)
