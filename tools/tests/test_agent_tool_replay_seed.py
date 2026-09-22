#!/usr/bin/env python3
"""The agent tool seed extracts, never calculates, and its constants are the
core table's own."""
import importlib.util
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('agent_tool_seed', REPO / 'tools/agent-tool-replay/seed.py')
seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seed)

CORE_TABLE = REPO / 'crates/zerocode-core/src/jev.rs'


def core_const(name: str) -> int:
    """`pub const NAME: usize = 2_000;` as the core table spells it."""
    found = re.search(rf'pub const {name}: usize = ([0-9_]+);', CORE_TABLE.read_text(encoding='utf-8'))
    assert found, name
    return int(found.group(1).replace('_', ''))


class ConstantsMatchTheirSource(unittest.TestCase):
    def test_the_text_cap_is_the_tables_own(self):
        source = CORE_TABLE.read_text(encoding='utf-8')
        self.assertIn('pub const AGENT_TOOL_TEXT_CHAR_CAP: usize = ROUTING_TASK_CHAR_CAP;', source)
        self.assertEqual(seed.TEXT_CHAR_CAP, core_const('ROUTING_TASK_CHAR_CAP'))

    def test_the_option_cap_is_the_tables_own_and_the_topics_fit_it(self):
        self.assertEqual(seed.OPTION_CAP, core_const('AGENT_TOOL_OPTION_CAP'))
        self.assertLessEqual(len(seed.TOPIC_TAGS), seed.OPTION_CAP)
        self.assertGreaterEqual(len(seed.TOPIC_TAGS), 2, 'a choose needs at least two options')
        self.assertEqual(len({tag for tag, _ in seed.TOPIC_TAGS}), len(seed.TOPIC_TAGS), 'a topic is offered once')


def page(tags: str, title: str, body: str) -> str:
    return f'---\ntitle: "{title}"\ntags: [{tags}]\n---\n# {title}\n\n{body}\n'


class Extraction(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.vault = Path(self.tmp.name)
        (self.vault / 'wiki').mkdir()

    def write(self, name: str, text: str):
        (self.vault / 'wiki' / name).write_text(text, encoding='utf-8')

    def test_a_page_belongs_when_it_carries_exactly_one_topic(self):
        self.write('one.md', page('zerocode, jev, measurement', 'One seat', 'About a seat.'))
        self.write('two.md', page('jev, terminal', 'Two topics', 'Ambiguous.'))
        self.write('none.md', page('zerocode, measurement', 'No topic', 'Unlabelled.'))
        self.write('raw.md', 'no frontmatter at all\n')
        items = seed.extract(self.vault)
        self.assertEqual([item['file'] for item in items], ['one.md'])
        self.assertEqual(items[0]['label'], 'jev')
        self.assertEqual(items[0]['context'], 'One seat\n\nAbout a seat.')

    def test_the_context_is_the_title_and_the_head_cut_to_the_cap_without_the_frontmatter(self):
        long_body = 'x' * (seed.TEXT_CHAR_CAP + 500)
        self.write('long.md', page('terminal', 'A long page', long_body))
        items = seed.extract(self.vault)
        self.assertEqual(len(items[0]['context']), seed.TEXT_CHAR_CAP)
        self.assertTrue(items[0]['context'].startswith('A long page\n\nxxx'))
        self.assertNotIn('tags:', items[0]['context'])
        self.assertNotIn('# A long page', items[0]['context'], 'the heading is the title, not the head')

    def test_the_seed_extracts_and_never_calculates(self):
        self.write('one.md', page('release', 'Ship it', 'The lane.'))
        out = self.vault / 'seed.json'
        with patch.object(sys, 'argv', ['seed', '--vault', str(self.vault), '--out', str(out)]):
            self.assertEqual(seed.main(), 0)
        built = json.loads(out.read_text(encoding='utf-8'))
        self.assertEqual(set(built), {'question', 'options', 'textCharCap', 'extractedAt', 'items'})
        self.assertEqual(built['question'], seed.QUESTION)
        self.assertEqual([option['id'] for option in built['options']], [tag for tag, _ in seed.TOPIC_TAGS])
        self.assertEqual(built['textCharCap'], seed.TEXT_CHAR_CAP)
        self.assertEqual(built['items'], [{'file': 'one.md', 'label': 'release', 'context': 'Ship it\n\nThe lane.'}])
        for key in ('agreement', 'agreed', 'wilson', 'tokens', 'elapsedMs'):
            self.assertNotIn(key, json.dumps(built), f'the seed calculated {key}')


if __name__ == '__main__':
    unittest.main()
