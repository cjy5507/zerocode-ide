# Vault pair replay

`seed.py --ledger <vault-pairs.jsonl> --out <private-json>` joins answered
judgments to later explicit `zo vault mark` rows at the same page version.
The output has numeric answers and hashed pair IDs; it contains no page text.
Keep the output outside the repository. An unlabeled pair stays unlabeled.

The ignored Rust replay test in `smart_router/vault_pairs.rs` reads the seed
through `VAULT_PAIR_REPLAY_SEED` and reports proposal precision, the
always-`none` baseline, and input-token cost. Fifty reviewed pairs are
required before precision is used as a promotion argument; this seat never
promotes automatically.
