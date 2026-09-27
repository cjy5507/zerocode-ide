# Summons difficulty replay

`seed.py --store <authority.sqlite> --run <run-id> --out <seed.json>` reads
workers, their first dispatch, task words and worker_done messages in one
read-only SQLite transaction. It writes only the requested measurement seed.
Task wording and review are current reconstructions; attempt/failure context
is restricted to dispatch facts known at that summons. Review facts require a
coordinator author. No session or checkout paths are extracted.

The ignored Rust test
`orchestration::summon_difficulty::tests::replay_recorded_summonses` asks the
production question through its normal Jev door using temporary settings.
Supply `ZEROCODE_DIFFICULTY_REPLAY_SEED`,
`ZEROCODE_DIFFICULTY_REPLAY_OUTPUT`, and a command-scoped `TYPESAFE_API_KEY`.
The key must never be saved in a seed, result, log or command-line argument.

One output row pairs the pinned effort with the difficulty answer and the
agent's translated effort. Comparison marks use only the inverse pin as
teacher, against always-high difficulty. Rework rounds and explicit retry
counts are totals for that task. A worker_done receipt means a structured
commit head or a report path was supplied; it is not verification of the
contents. Review facts remain separate. Lower suggested effort and observed
rework are descriptive; they do not prove the unexecuted effort would have
succeeded or saved tokens. Token attribution and runtime outcome notes are a
separate second stage. Replay never promotes the operating seat.

Seeds and results contain private task text or measurements: keep them out of
git. Run `python3 tools/tests/test_summon_difficulty_replay_seed.py` for the
retry/receipt/attempt attribution contract without a service call.
