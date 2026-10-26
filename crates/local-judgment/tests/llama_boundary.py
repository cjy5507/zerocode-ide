import ast
import io
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


with open(sys.argv[1], encoding="utf-8") as source:
    parsed = ast.parse(source.read())
definitions = [node for node in parsed.body if isinstance(node, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))
               or (isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "MAX_FRAME")]
bridge = {}
exec(compile(ast.Module(body=definitions, type_ignores=[]), "owned-native-bridge", "exec"), bridge)

# Windows takes a path without a drive as relative and the native policy refuses it, so the
# fixtures sit at the root of the current drive there; elsewhere at /.
ROOT = Path(Path.cwd().anchor)
EXECUTABLE = str(ROOT / "runtime" / "llama-server")


class Ended(BaseException):
    pass


class Child:
    def __init__(self):
        self.killed = False
        self.waited = False

    def poll(self):
        return -9 if self.killed else None

    def kill(self):
        self.killed = True

    def wait(self, timeout=None):
        self.waited = True
        return -9


class NativeBoundary(unittest.TestCase):
    def setUp(self):
        bridge["native_process"] = None
        bridge["native_address"] = None
        bridge["native_command"] = None
        bridge["native_cwd"] = None
        bridge["native_spawn_allowed"] = False
        bridge["native_lock"] = bridge["threading"].RLock()

    def arguments(self, initial):
        build = bridge.get("native_arguments")
        self.assertTrue(callable(build), "the pinned native command policy is missing")
        return build(initial, 32101, "fixture-capability")

    def permitted(self, event, arguments):
        try:
            bridge["offline_process"](event, arguments)
        except ValueError:
            self.fail("the owned native operation was refused: " + event)

    def test_native_command_disables_jinja_and_does_not_override_template_detection(self):
        initial = {"nativeExecutable": EXECUTABLE, "nativeModelFile": str(ROOT / "models" / "selected.gguf"), "modelId": "fixture", "mode": "model"}
        command = self.arguments(initial)
        self.assertEqual(command[0], initial["nativeExecutable"])
        for flag in ["--no-jinja", "--offline", "--no-webui", "--no-context-shift"]:
            self.assertIn(flag, command)
        self.assertNotIn("--chat-template", command, "a fixed template replaced the model's detected native format")
        self.assertNotIn("--chat-template-file", command)
        self.assertNotIn("--jinja", command)
        self.assertEqual(command[command.index("--host") + 1], "127.0.0.1")
        self.assertEqual(command[command.index("--model") + 1], initial["nativeModelFile"])
        self.assertEqual(command[command.index("--threads") + 1], "2")
        self.assertEqual(command[command.index("--n-gpu-layers") + 1], "0")
        self.assertNotIn("--hf-repo", command)

    def test_model_paths_are_single_arguments_and_cannot_add_engine_options(self):
        name = str(ROOT / "models" / "owned file; --jinja.gguf")
        command = self.arguments({"nativeExecutable": EXECUTABLE, "nativeModelFile": name, "modelId": "fixture", "mode": "model"})
        self.assertEqual(command[command.index("--model") + 1], name)
        self.assertNotIn("--jinja", command)

    def test_runtime_verification_does_not_load_a_model(self):
        command = self.arguments({"nativeExecutable": EXECUTABLE, "mode": "verify"})
        self.assertEqual(command, [EXECUTABLE, "--version"])

    def test_only_the_owned_loopback_connection_is_permitted(self):
        bridge["native_address"] = ("127.0.0.1", 32101)
        self.permitted("socket.connect", (object(), ("127.0.0.1", 32101)))
        self.permitted("socket.getaddrinfo", ("127.0.0.1", 32101, 0, 0, 0))
        for address in [("127.0.0.1", 32102), ("192.0.2.1", 32101)]:
            with self.assertRaises(ValueError):
                bridge["offline_process"]("socket.connect", (object(), address))
        with self.assertRaises(ValueError):
            bridge["offline_process"]("socket.getaddrinfo", ("example.invalid", 32101, 0, 0, 0))

    def test_one_exact_owned_subprocess_is_allowed_and_every_other_command_is_refused(self):
        command = ["/usr/bin/sandbox-exec", "-p", "fixture-profile", "/runtime/llama-server", "--version"]
        bridge["native_command"] = command
        bridge["native_cwd"] = "/runtime"
        bridge["native_spawn_allowed"] = True
        self.permitted("subprocess.Popen", (command[0], command, "/runtime", {}))
        with self.assertRaises(ValueError):
            bridge["offline_process"]("subprocess.Popen", (command[0], command, "/runtime", {}))
        bridge["native_spawn_allowed"] = True
        with self.assertRaises(ValueError):
            bridge["offline_process"]("subprocess.Popen", (command[0], command + ["--help"], "/runtime", {}))
        with self.assertRaises(ValueError):
            bridge["offline_process"]("os.system", ("fixture",))

    def test_parent_eof_reaps_the_native_child_before_exiting(self):
        child = Child()
        bridge["native_process"] = child
        exited = []

        def finish(code):
            exited.append(code)
            raise Ended()

        with patch.object(bridge["sys"], "stdin", SimpleNamespace(buffer=io.BytesIO(b""))), patch.object(bridge["os"], "_exit", finish):
            with self.assertRaises(Ended):
                bridge["read_parent"]()
        self.assertTrue(child.killed, "parent EOF left the owned native child alive")
        self.assertTrue(child.waited, "parent EOF did not reap the owned native child")
        self.assertEqual(exited, [0])

    def test_memory_limit_counts_the_native_child_and_preserves_the_limit_exit_code(self):
        child = Child()
        bridge["native_process"] = child
        process = SimpleNamespace(memory_info=lambda: SimpleNamespace(rss=10), children=lambda recursive: [SimpleNamespace(memory_info=lambda: SimpleNamespace(rss=20))])
        exited = []

        def finish(code):
            exited.append(code)
            raise Ended()

        with patch.object(bridge["os"], "_exit", finish), patch.object(bridge["time"], "sleep", side_effect=AssertionError("native child RSS was not counted")):
            with self.assertRaises(Ended):
                bridge["memory_guard"](process, 25)
        self.assertTrue(child.killed)
        self.assertEqual(exited, [75])

    def test_spawn_publication_is_held_until_the_owned_child_is_recorded(self):
        child = Child()
        active = []
        case = self

        class Guard:
            def __enter__(self):
                active.append(True)

            def __exit__(self, *args):
                case.assertIs(bridge["native_process"], child, "spawn ownership was published after releasing the exit guard")
                active.clear()

        bridge["native_lock"] = Guard()

        def spawn(*args, **kwargs):
            self.assertTrue(active, "the native child was spawned outside the exit guard")
            return child

        with patch.object(bridge["subprocess"], "Popen", spawn):
            self.assertIs(bridge["launch_native"]({"nativeExecutable": EXECUTABLE, "mode": "verify"}), child)

    def test_exit_observes_a_child_published_while_waiting_for_the_spawn_guard(self):
        child = Child()

        class Guard:
            def __enter__(self):
                bridge["native_process"] = child

            def __exit__(self, *args):
                pass

        bridge["native_lock"] = Guard()

        def finish(code):
            raise Ended()

        with patch.object(bridge["os"], "_exit", finish):
            with self.assertRaises(Ended):
                bridge["finish_process"](0)
        self.assertTrue(child.killed, "the exit missed a child created by the pending spawn")
        self.assertTrue(child.waited)

    def test_only_the_private_native_alias_is_forwarded_and_the_public_identity_is_restored(self):
        adapter = bridge["LlamaCppAdapter"].__new__(bridge["LlamaCppAdapter"])
        adapter.model_id = "fixture"
        adapter.native_alias = "fixture-native"
        seen = []

        def exchange(method, resource, body, timeout):
            seen.append((resource, body))
            if resource == "/apply-template":
                return {"prompt": "fixture"}
            if resource == "/tokenize":
                return {"tokens": [1]}
            self.assertEqual(body["model"], adapter.native_alias, "the stable public identity was used as native ownership proof")
            return {"model": adapter.native_alias, "choices": [], "usage": {}}

        adapter.exchange = exchange
        request = {"model": "fixture", "messages": [], "response_format": {"type": "json_schema"}, "temperature": 0, "max_tokens": 10, "stream": False}
        answer = adapter.predict(request)
        self.assertEqual(answer["model"], adapter.model_id)
        self.assertEqual(request["model"], adapter.model_id, "forwarding mutated the caller's request")
        self.assertEqual(len(seen), 3)


unittest.main(argv=["native-boundary"], verbosity=2)
