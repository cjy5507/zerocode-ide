"""The panel-rules reader finds a module's rule by its hash-free class names,
keeps to the module a landmark names, and merges every declaration of that
rule — the pure part of `tools/agents/claude_code_panel_rules.py`, without
the marketplace."""
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
        wanted, wanted_vars = rules.WANTED, rules.WANTED_VARS
        try:
            rules.WANTED = [{"key": "sendButton", "selector": ".sendButton", "landmark": "inputFooterV2"}]
            rules.WANTED_VARS = ["--corner-radius-small"]
            written = rules.snapshot(CSS, "0.0.0", "https://example.invalid/vsix")
        finally:
            rules.WANTED, rules.WANTED_VARS = wanted, wanted_vars
        self.assertEqual(written["version"], "0.0.0")
        self.assertEqual(written["source"], "https://example.invalid/vsix")
        self.assertEqual(written["vars"], {"--corner-radius-small": "4px"})
        self.assertEqual(written["rules"]["sendButton"]["border-radius"], "5px")
        self.assertTrue(rules.same_measures(written, dict(written, fetched_at="later")))
        self.assertFalse(rules.same_measures(written, dict(written, version="0.0.1")))


if __name__ == "__main__":
    unittest.main()
