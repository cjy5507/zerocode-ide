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

# A stand-in for the laya runtime, for the bridge's own logic only: it proves nothing about real
# tokens, which the contract test against the manifest-pinned runtime measures. Its head keeps
# laya's layout and its three silent cuts, at this file's sizes: each option is capped at CAP
# tokens, the options are re-cut to RECUT tokens each when fewer than FLOOR would be left for the
# instruction, and the instruction keeps what the options leave.
CAP, FLOOR, RECUT = 48, 16, 4


class Words:
    """One token per word, only the first `max_length` of them when a call truncates."""

    mask_token = "[MASK]"
    cls_token_id, sep_token_id, mask_token_id = 1, 2, 3

    def __call__(self, text, add_special_tokens=True, truncation=False, max_length=None):
        ids = [10 + len(piece) for piece in self.pieces(text)]
        return {"input_ids": ids[:max_length] if truncation else ids}

    def pieces(self, text):
        return text.split()


class Letters(Words):
    """One token per letter, so the same text costs more."""

    def pieces(self, text):
        return [letter for letter in text if not letter.isspace()]


def stand_in_build_head(tok, q, head_max_len, option_order=None):
    instruction = tok("%s question: %s" % (q["t"], q["ins"]), add_special_tokens=False)["input_ids"]
    options = [
        [tok.mask_token_id] + tok(" " + text, add_special_tokens=False, truncation=True, max_length=CAP)["input_ids"]
        for text in q["crit"].values()
    ]
    if head_max_len - sum(map(len, options)) < FLOOR:
        options = [option[:RECUT] for option in options]
    ids = [tok.cls_token_id] + instruction[:max(1, head_max_len - sum(map(len, options)))] + [tok.sep_token_id]
    markers = []
    for option in options:
        markers.append(len(ids))
        ids.extend(option)
    ids.append(tok.sep_token_id)
    return ids, markers, {"options": len(options), "options_distinct": len({tuple(option) for option in options})}


def stand_in_laya(through_module=True):
    """The stand-in runtime as `import laya` finds it. Its agent builds every head through
    `laya.common.build_head`, as laya's `build_sequence` does, or through a reference of its own
    that nothing outside it can see when `through_module` is false."""
    common = SimpleNamespace(build_head=stand_in_build_head)

    class Agent:
        def __init__(self, directory, device, compile):
            self.cfg = json.loads((Path(directory) / "rl_agent_config.json").read_text())
            self.tok = {"words": Words(), "letters": Letters()}[self.cfg.get("tokenizer", "words")]
            self.batches = 0

        @staticmethod
        def _to_internal(question):
            return {"t": question["type"], "ins": question["instructions"], "crit": dict(question["criteria"])}

        def predict(self, state, questions):
            self.batches += 1
            build_head = common.build_head if through_module else stand_in_build_head
            for question in questions.values():
                build_head(self.tok, self._to_internal(question), self.cfg.get("head_max_len", 64))
            return {
                "answers": {name: {"type": question["type"]} for name, question in questions.items()},
                "usage": {"input_tokens": 1, "output_tokens": 0, "truncated": False, "truncated_questions": []},
            }

    return SimpleNamespace(Agent=Agent, common=common)


def stand_in_d3(loaded, answer=None):
    """The stand-in D3 runtime: its model holds `loaded(the max_length it was asked for)` as the
    `max_length` it loaded, and answers every request with `answer`."""

    class D3:
        def __init__(self, root, device, batch_size, max_length, model_name):
            self.max_length = loaded(max_length)

        def system_one(self, state, questions):
            return answer

    return SimpleNamespace(D3=D3)


def choice(words, options=("yes", "no")):
    """A choice question with `words` words of instruction and one option per text."""
    return {
        "type": "choice",
        "instructions": " ".join(["word"] * words),
        "criteria": {"option-%d" % index: text for index, text in enumerate(options)},
    }


def sized(instruction, options):
    """A question whose head costs, under `Words`, `instruction` tokens of instruction and the
    given tokens per option, each option's [MASK] included."""
    return choice(instruction - 2, [" ".join(["word"] * (size - 1)) for size in options])


class BridgeBoundary(unittest.TestCase):
    def laya(self, config, stand_in=None, start=None):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        Path(folder.name, "rl_agent_config.json").write_text(json.dumps(config))
        initial = {"modelDirectory": folder.name, "modelId": "fixture-model", "runtime": "fixture-runtime"}
        initial.update(start or {})
        with patch.dict(sys.modules, {"laya": stand_in or stand_in_laya()}):
            return bridge["LayaAdapter"](initial), Path(folder.name)

    def d3(self, loaded, answer=None):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        initial = {"modelDirectory": folder.name, "modelFiles": [], "modelId": "fixture-model", "runtime": "fixture-runtime"}
        with patch.dict(sys.modules, {"d3_runtime": stand_in_d3(loaded, answer)}):
            return bridge["TransformersSystemOneAdapter"](initial)

    def named(self, adapter, questions):
        return adapter.predict({"state": "fixture state", "questions": questions})["usage"].get("truncated_questions")

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

    def test_memory_guard_ends_the_runtime_with_its_own_code_past_the_limit(self):
        process = SimpleNamespace(memory_info=lambda: SimpleNamespace(rss=2048))
        with patch.object(bridge["os"], "_exit", side_effect=SystemExit) as ended:
            with self.assertRaises(SystemExit):
                bridge["memory_guard"](process, 1024)
        ended.assert_called_once_with(75)

    def test_a_question_whose_instruction_the_runtime_cuts_is_named(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 24})
        self.assertEqual(
            self.named(adapter, {"goal": choice(40), "other": choice(4)}),
            ["goal"],
            "an instruction the runtime cut was answered as whole",
        )

    def test_a_question_whose_option_the_runtime_caps_is_named(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 256})
        capped = choice(4, ("yes", " ".join(["detail"] * (CAP + 12))))
        self.assertEqual(
            self.named(adapter, {"goal": capped, "other": choice(4)}),
            ["goal"],
            "an option the runtime capped was answered as whole",
        )

    def test_a_question_whose_options_the_runtime_recuts_is_named(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 40})
        crowded = choice(2, ("a b c d e f", "g h i j k l", "m n o p q r", "s t u v w x"))
        self.assertEqual(
            self.named(adapter, {"goal": crowded, "other": choice(2)}),
            ["goal"],
            "options the runtime re-cut were answered as whole",
        )

    def test_the_named_questions_follow_the_loaded_budget_and_the_tokenizer(self):
        questions = {"goal": choice(30), "other": choice(2)}
        for config, expected in [
            ({"max_len": 1024, "head_max_len": 24}, ["goal"]),
            ({"max_len": 1024, "head_max_len": 48}, None),
            ({"max_len": 1024, "head_max_len": 48, "tokenizer": "letters"}, ["goal"]),
        ]:
            adapter, _ = self.laya(config)
            self.assertEqual(self.named(adapter, questions), expected, "the named questions ignored %s" % config)

    def test_a_question_the_runtime_never_measured_is_not_answered(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 256}, stand_in_laya(through_module=False))
        with self.assertRaisesRegex(ValueError, "head_unmeasured", msg="a head nobody measured was answered"):
            adapter.predict({"state": "fixture state", "questions": {"goal": choice(4)}})

    def test_the_original_cut_heads_are_named_and_their_normal_contrasts_are_not(self):
        # The heads of reports/t-58268/head-budget-offline-20261016.json under head_max_len 256, as
        # the installed tokenizer counted them: the instruction tokens of the action, walled and
        # instructed questions of main's request (baseline) and t-58268's (candidate), without and
        # with the navigation options, and the tokens of each option, its [MASK] included.
        options = [
            {"action": [20, 20, 19, 19], "walled": [26, 26], "instructed": [27, 27]},
            {"action": [22, 22, 22, 22, 22, 21, 21], "walled": [26, 26], "instructed": [27, 27]},
        ]
        instructions = {
            "baseline": {goal: [(145, 55, 62), (150, 55, 62)] for goal in ("recorded_trial", "english_400", "korean_400")},
            "candidate": {
                "recorded_trial": [(168, 78, 62), (173, 78, 62)],
                "english_400": [(233, 143, 62), (238, 143, 62)],
                "korean_400": [(366, 276, 62), (371, 276, 62)],
            },
        }
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 256})
        named = {}
        for shape, goals in instructions.items():
            for goal, variants in goals.items():
                for navigation, sizes in enumerate(variants):
                    questions = {
                        name: sized(size, options[navigation][name])
                        for name, size in zip(("action", "walled", "instructed"), sizes)
                    }
                    named[shape, goal, bool(navigation)] = self.named(adapter, questions)
        cut = {("baseline", goal, True): ["action"] for goal in ("recorded_trial", "english_400", "korean_400")}
        cut.update({
            ("candidate", "recorded_trial", True): ["action"],
            ("candidate", "english_400", False): ["action"],
            ("candidate", "english_400", True): ["action"],
            ("candidate", "korean_400", False): ["action", "walled"],
            ("candidate", "korean_400", True): ["action", "walled"],
        })
        self.assertEqual(
            named,
            {request: cut.get(request) for request in named},
            "the original cut heads and their whole contrasts were named otherwise",
        )

    def test_every_question_of_every_batch_is_measured(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 24})
        questions = {"q0": choice(40), "q1": choice(41), "q2": choice(4), "q3": choice(42), "q4": choice(5), "q5": choice(43)}
        self.assertEqual(
            self.named(adapter, questions),
            ["q0", "q1", "q3", "q5"],
            "a cut head went unnamed within or across the batches of four",
        )
        self.assertEqual(adapter.agent.batches, 2)

    def test_two_question_ids_with_the_same_content_are_named_alike(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 24})
        cut, whole = choice(40), choice(4)
        questions = {
            "goal": cut,
            "goal-again": dict(cut),
            "plain": whole,
            "plain-again": dict(whole),
            "goal-later": dict(cut),
            "plain-later": dict(whole),
        }
        self.assertEqual(
            self.named(adapter, questions),
            ["goal", "goal-again", "goal-later"],
            "two ids with one content were named apart",
        )

    def test_the_declared_limits_are_the_loaded_config_not_a_later_read(self):
        for max_len, head_max_len in [(1024, 256), (2048, 300)]:
            adapter, folder = self.laya({"max_len": max_len, "head_max_len": head_max_len})
            (folder / "rl_agent_config.json").write_text(json.dumps({"max_len": 4096, "head_max_len": 512}))
            adapter.agent.cfg.update(max_len=4096, head_max_len=512)
            self.assertEqual(
                getattr(adapter, "limits", None),
                {"model": "fixture-model", "runtime": "fixture-runtime", "tokens": {"max_len": max_len, "head_max_len": head_max_len}},
                "the declaration is not the config the agent loaded",
            )

    def test_each_load_declares_its_own_model_and_runtime(self):
        first, _ = self.laya({"max_len": 1024, "head_max_len": 256}, start={"modelId": "model-a", "runtime": "runtime-1"})
        second, _ = self.laya({"max_len": 2048, "head_max_len": 300}, start={"modelId": "model-b", "runtime": "runtime-2"})
        self.assertEqual(
            [getattr(first, "limits", None), getattr(second, "limits", None)],
            [
                {"model": "model-a", "runtime": "runtime-1", "tokens": {"max_len": 1024, "head_max_len": 256}},
                {"model": "model-b", "runtime": "runtime-2", "tokens": {"max_len": 2048, "head_max_len": 300}},
            ],
            "a load declared another load's model, runtime or numbers",
        )

    def test_a_config_without_two_positive_numbers_declares_nothing(self):
        whole = {"max_len": 1024, "head_max_len": 256}
        cases = [
            ({}, {}),
            ({"max_len": 1024}, {}),
            ({"head_max_len": 256}, {}),
            (dict(whole, max_len=0), {}),
            (dict(whole, head_max_len=-256), {}),
            (dict(whole, max_len=True), {}),
            (dict(whole, max_len="1024"), {}),
            (dict(whole, head_max_len=256.0), {}),
            (dict(whole, max_len=None), {}),
            (whole, {"runtime": ""}),
            (whole, {"runtime": None}),
            (whole, {"modelId": ""}),
        ]
        for config, start in cases:
            adapter, _ = self.laya(config, start=start)
            self.assertIn("limits", vars(adapter), "the load of %s %s never decided what to declare" % (config, start))
            self.assertIsNone(adapter.limits, "the load of %s %s declared a number" % (config, start))
        unreadable = stand_in_laya()

        class Unreadable(unreadable.Agent):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, **kwargs)
                del self.cfg

        unreadable.Agent = Unreadable
        adapter, _ = self.laya(whole, unreadable)
        self.assertIn("limits", vars(adapter), "an agent without a config never decided what to declare")
        self.assertIsNone(adapter.limits, "an agent without a config declared a number")

    def test_the_d3_adapter_declares_its_loaded_max_length_and_no_head_budget(self):
        for loaded, tokens in [(lambda asked: asked, {"max_len": 1024}), (lambda asked: 4096, {"max_len": 4096})]:
            self.assertEqual(
                getattr(self.d3(loaded), "limits", None),
                {"model": "fixture-model", "runtime": "fixture-runtime", "tokens": tokens},
                "D3 did not declare the max_length it loaded",
            )
        for unusable in (None, 0, -1, True, "1024"):
            adapter = self.d3(lambda asked: unusable)
            self.assertIn("limits", vars(adapter), "D3 holding %r never decided what to declare" % (unusable,))
            self.assertIsNone(adapter.limits, "D3 holding %r declared a number" % (unusable,))

    def test_the_measure_changes_nothing_the_runtime_computes(self):
        capped = choice(4, ("yes", " ".join(["detail"] * (CAP + 12))))
        crowded = choice(2, ("a b c d e f", "g h i j k l", "m n o p q r", "s t u v w x"))
        to_internal = stand_in_laya().Agent._to_internal
        for tokenizer in (Words(), Letters()):
            for question, budget in [(choice(40), 24), (capped, 256), (crowded, 40), (choice(4), 256)]:
                internal = to_internal(question)
                plain = stand_in_build_head(tokenizer, internal, budget, option_order=[1, 0])
                head, _ = bridge["measured_head"](stand_in_build_head, tokenizer, internal, budget, option_order=[1, 0])
                self.assertEqual(head, plain, "the measure changed the ids, markers or stats build_head computes")

    def test_a_head_that_fits_is_not_named(self):
        adapter, _ = self.laya({"max_len": 1024, "head_max_len": 256})
        answered = adapter.predict({"state": "fixture state", "questions": {"goal": choice(40), "other": choice(4)}})
        self.assertNotIn("truncated_questions", answered["usage"], "a whole head was named as cut")
        self.assertEqual(sorted(answered["answers"]), ["goal", "other"])

    def test_a_d3_length_refusal_is_never_named_as_a_cut(self):
        refused = {
            "model": "fixture-model",
            "answers": {"goal": {"type": "choice", "error": "max_length_exceeded", "message": "nothing was truncated"}},
            "usage": {"input_tokens": 0, "output_tokens": 0},
        }
        adapter = self.d3(lambda asked: asked, refused)
        answered = adapter.predict({"state": "fixture state", "questions": {"goal": choice(4)}})
        self.assertNotIn("truncated_questions", answered["usage"], "a D3 length refusal was named as a cut")
        self.assertEqual(answered["answers"], refused["answers"])


unittest.main(argv=["bridge-boundary"], verbosity=2)
