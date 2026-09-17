# vault-lint fixture

`vault/` is a second-brain vault holding exactly one of each lint finding, and
`expected.json` is the table `zerocode vault-lint` answers for it. Two tests
pin the two together and the same file feeds the window's vault health card, so
the recipe's numbers and the card's numbers cannot drift:

- `crates/zerocode-core/src/second_brain_lint.rs` — the producer, against
  `expected.json`;
- `crates/zerocode-app/tests/vault_lint.rs` — the recipe's exit code and text;
- `ui/tests/test-knowledge-graph.mjs` — the card, fed `expected.json`.

| row | witness |
|---|---|
| index gaps (2) | `index-gap.md`, `orphan.md` are not in `wiki/index.md` |
| ghost links (1) | `hub.md` links `[[never-written]]` |
| orphans (1) | `orphan.md` — nothing links to it and the index does not list it |
| missing frontmatter (1) | `no-source.md` has no `source:` |
| undeclared relations (1) | `hub.md` links `[[linked-target]]` in prose with no key naming it |
| unlogged raw (1) | `raw/unlogged.md` — no `source:` names it and no page mentions its path |
| contradictions (1) | `linked-target.md` declares `contradicts: [[indexed-clean]]` |
| superseded (1) | `hub.md` declares `supersedes: [[old-note]]` |
