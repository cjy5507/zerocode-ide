# Summons difficulty replay

`seed.py --store <authority.sqlite> --run <run-id> --out <seed.json>` reads
workers, their first dispatch, task words and worker_done messages in one
read-only SQLite transaction. Task wording is a current reconstruction.
The output is a private measurement, never a fixture to commit.

The ignored Rust test
`orchestration::summon_difficulty::tests::replay_recorded_summonses` asks the
production question through its normal Jev door using temporary settings.
Supply `ZEROCODE_DIFFICULTY_REPLAY_SEED`,
`ZEROCODE_DIFFICULTY_REPLAY_OUTPUT`, and a command-scoped `TYPESAFE_API_KEY`.
Never persist the key. Pins are execution context, not answer labels.

To attach actual outcomes to those fixed answers, use the ignored test
`orchestration::summon_difficulty::tests::replay_execution_outcomes` with:

- `ZEROCODE_OUTCOME_SNAPSHOT`: a read-only SQLite backup of the authority store.
- `ZEROCODE_OUTCOME_ANSWERS`: the previous replay's JSON answers.
- `ZEROCODE_OUTCOME_OUTPUT`: the private output JSON.
- `ZEROCODE_OUTCOME_CONFIG`: the account configuration root for usage scans.

This stage sends no Jev requests and writes no operating ledgers. It uses the
same outcome reader as the runtime, including the ledger's attempt/source-bound
coordinator review and existing provider usage scanners. An unknown outcome or
unlinked token count is null. Wall time is dispatch start to end, not review
latency (the ledger has no review timestamp). Usage is the worker's last known
conversation; shared/reused workers are not assigned a fabricated split.

Results include every summons; promotion counts distinct tasks' initial
attempts. Compare actual applied choices against observed coordinator choices
and the always-high subset at the same difficulty and agent. Both baselines
can fail. Shadow predictions never claim the unexecuted choice's success or
savings. Promotion counts each task's total tokens and wall time, including retries and
waits, so a cheap failed first attempt cannot hide expensive rework. It requires
complete matched outcome/cost cohorts, preserved
first-attempt success, and lower mean tokens and wall time. The minimum sample
is the core outcome policy's `MIN_EXECUTIONS`; these observational comparisons
are not a randomized causal estimate. Missing landing facts are not failures.

The editable `smart.summonProfiles` table supplies omitted model/effort choices;
its shipped defaults are `crates/zerocode-core/src/summon-profiles.json`.
Explicit flags win. While recording or without an actionable answer, an omitted
model uses the table's middle difficulty. The settings UI edits the same table.

Run `python3 tools/tests/test_summon_difficulty_replay_seed.py` for extraction
contracts, and the Rust `summon_` tests for execution attribution and promotion.
