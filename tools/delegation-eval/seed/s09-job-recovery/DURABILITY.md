# What the job queue promises across a power loss

Everything durable goes through `disk.py`, which models a disk that can lose power during any
write. A power loss raises `PowerLoss` out of whatever was running, and all memory is gone. The
next boot makes new `JobStore` and `Effects` objects on the same disk. Clients do not know which
of their submissions landed, so they submit everything again.

1. **Nothing is lost.** Every submitted job ends `done` or `dead`, however many power losses
   happen and wherever they fall, including during the boot after an earlier power loss.
2. **Effects happen once.** A job's effect appears exactly once in `Effects.applied()`, whatever
   happens. The recorded result of a `done` job is the result of that effect.
3. **Submitting is idempotent.** Submitting a job id already known changes nothing, whatever state
   the job is in.
4. **A cut write is not an error.** A power loss can leave half a record at the end of a journal
   file. Opening it discards that half record, and later records are not joined onto it.
5. **Attempts are counted durably.** `attempts` counts each claim, across restarts and
   checkpoints alike, exactly once each. A failed job is queued again until it has used
   `max_attempts`, and is then `dead`. A job left `running` by a power loss has had an attempt: at
   the next boot it is queued again, or `dead` when that attempt was its last. A dead job's effect
   never happens.
6. **Order holds.** Queued jobs are claimed in the order they were first submitted. A job queued
   again keeps its original place in that order.
7. **A checkpoint is safe at any instant.** `checkpoint()` moves the journal's content into the
   snapshot and leaves the journal empty. Every state it can be cut in must read back to the same
   jobs, with no event applied twice and no snapshot half written.

`run_until_idle` drives a store until nothing is queued.
