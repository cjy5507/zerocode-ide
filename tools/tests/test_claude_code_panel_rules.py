"""The panel-rules reader finds a module's rule by its hash-free class names,
keeps to the module a landmark names, and merges every declaration of that
rule, and reads the script's own measures by their shapes — the pure part of
`tools/agents/claude_code_panel_rules.py`, without the marketplace."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "agents"))

import claude_code_panel_rules as rules  # noqa: E402

CSS = (
    ".messagesContainer_07S1Yg{padding:20px 20px 40px}"
    ".timelineMessage_07S1Yg{padding-left:30px}"
    ".timelineMessage_07S1Yg:before{content:\"\";left:9px;width:7px;height:7px}"
    ".timelineMessage_07S1Yg.dotSuccess_07S1Yg:before{background-color:#74c991}"
    ".icon_hc5dvw{display:inline-block;width:1.5em;font-family:monospace}"
    ".container_hc5dvw[data-permission-mode=plan] .icon_hc5dvw{color:red}"
    ".icon_zzzzzz{width:16px}"
    ".menuButton_gGYT1w,.sendButton_gGYT1w{width:26px;height:26px}"
    ".sendButton_gGYT1w{border-radius:5px}"
    ".inputFooterV2_gGYT1w{gap:2px}"
    "html{--corner-radius-small:4px;--app-pill-min-height:18px;--nothing:1}"
)

# The shapes the script's measures stand in, minified names and all (the
# 2.1.280 bundle's own spellings, with other names around them).
SCRIPT = (
    'let g=I?F("div",{className:C0.userMessageAttachments,children:b}):null;'
    'return R("div",{className:C0.userMessage,children:[g,F(EV0,{content:A[i]??y,context:Y,maxHeight:60})]})'
    "function WG0($){let J=Math.max(0,Math.ceil($)),Z=Math.min(200,J+20);return{height:Z,truncated:J>Z}}"
    "function cN($){return $.length>250||$.split(`\n`).length>3}class c81 extends p2{}"
    'o(()=>{let B=setInterval(()=>{G((K)=>(K+1)%OU0.length)},120);return()=>clearInterval(B)},[]),'
    "cx(()=>{U(Be(Y))},(B)=>{let K=[2000,3000,5000];return B<K.length?K[B]:5000});"
    "let Q=null,z=0,G=40,q=(U)=>{if(U-z<G){Q=requestAnimationFrame(q);return}z=U};"
    'var wU0=["·","✢","*","✶","✻","✽"],OU0=[...wU0,...[...wU0].reverse()],tD1=["Baking","Pondering"];'
)


class RuleReading(unittest.TestCase):
    def setUp(self) -> None:
        self.rules = rules.rules_of(CSS)

    def test_a_rule_is_found_without_its_hash_and_merged_across_its_lines(self) -> None:
        self.assertEqual(
            rules.rule_named(self.rules, ".sendButton", "inputFooterV2"),
            {"width": "26px", "height": "26px", "border-radius": "5px"},
        )
        self.assertEqual(
            rules.rule_named(self.rules, ".timelineMessage:before", "messagesContainer"),
            {"content": '""', "left": "9px", "width": "7px", "height": "7px"},
        )
        self.assertEqual(
            rules.rule_named(self.rules, ".timelineMessage.dotSuccess:before", "messagesContainer"),
            {"background-color": "#74c991"},
        )

    def test_a_landmark_keeps_a_shared_base_name_to_its_own_module(self) -> None:
        self.assertEqual(
            rules.rule_named(self.rules, ".icon", "container[data-permission-mode"),
            {"display": "inline-block", "width": "1.5em", "font-family": "monospace"},
        )
        self.assertEqual(rules.rule_named(self.rules, ".icon", "nowhere"), {})

    def test_the_snapshot_names_its_source_and_refuses_a_missing_rule(self) -> None:
        with self.assertRaises(SystemExit):
            rules.snapshot(CSS, "0.0.0", "https://example.invalid/vsix")
        wanted, wanted_vars, wanted_constants = rules.WANTED, rules.WANTED_VARS, rules.WANTED_CONSTANTS
        try:
            rules.WANTED = [{"key": "sendButton", "selector": ".sendButton", "landmark": "inputFooterV2"}]
            rules.WANTED_VARS = ["--corner-radius-small"]
            # A snapshot without the script names no script measure: refused.
            with self.assertRaises(SystemExit):
                rules.snapshot(CSS, "0.0.0", "https://example.invalid/vsix")
            written = rules.snapshot(CSS, "0.0.0", "https://example.invalid/vsix", SCRIPT)
            rules.WANTED_CONSTANTS = []
            wanted_words, rules.WANTED_WORDS = rules.WANTED_WORDS, []
            try:
                bare = rules.snapshot(CSS, "0.0.0", "https://example.invalid/vsix")
            finally:
                rules.WANTED_WORDS = wanted_words
        finally:
            rules.WANTED, rules.WANTED_VARS, rules.WANTED_CONSTANTS = wanted, wanted_vars, wanted_constants
        self.assertEqual(written["version"], "0.0.0")
        self.assertEqual(written["source"], "https://example.invalid/vsix")
        self.assertEqual(written["vars"], {"--corner-radius-small": "4px"})
        self.assertEqual(written["rules"]["sendButton"]["border-radius"], "5px")
        self.assertEqual((bare["constants"], bare["words"]), ({}, {}))
        self.assertEqual(written["words"], {"spinnerVerbs": ["Baking", "Pondering"]})
        self.assertTrue(rules.same_measures(written, dict(written, fetched_at="later")))
        self.assertFalse(rules.same_measures(written, dict(written, version="0.0.1")))
        self.assertFalse(rules.same_measures(written, dict(written, constants={"diffMaxHeight": 201})))


class ScriptMeasures(unittest.TestCase):
    def test_each_measure_is_read_by_its_shape_whatever_the_minifier_named(self) -> None:
        self.assertEqual(
            rules.constants_of(SCRIPT),
            {
                "userMessageMaxHeight": 60,
                "diffMaxHeight": 200,
                "diffHeightPad": 20,
                "longTextChars": 250,
                "longTextLines": 3,
                "spinnerVerbAfter1": 2000,
                "spinnerVerbAfter2": 3000,
                "spinnerVerbAfter3": 5000,
                "spinnerVerbEvery": 5000,
                "spinnerRevealStep": 40,
                "spinnerGlyphStep": 120,
            },
        )
        self.assertEqual(rules.words_of(SCRIPT), {"spinnerVerbs": ["Baking", "Pondering"]})
        renamed = SCRIPT.replace("EV0", "Qz9").replace("WG0", "a1$").replace("cN", "zz")
        self.assertEqual(rules.constants_of(renamed)["userMessageMaxHeight"], 60)
        self.assertEqual(rules.constants_of(renamed)["longTextChars"], 250)

    def test_a_shape_found_twice_is_no_measure(self) -> None:
        twice = SCRIPT + "function WG1($){let J=Math.max(0,Math.ceil($)),Z=Math.min(300,J+20);return{height:Z,truncated:J>Z}}"
        self.assertNotIn("diffMaxHeight", rules.constants_of(twice))
        self.assertEqual(rules.constants_of(twice)["userMessageMaxHeight"], 60)


if __name__ == "__main__":
    unittest.main()
