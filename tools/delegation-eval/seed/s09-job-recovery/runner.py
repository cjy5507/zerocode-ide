"""Claiming jobs, running them and recording what they did."""

from journal import Journal

EFFECTS = "effects.journal"


class JobError(Exception):
    """The job itself failed; it may be tried again."""


class Effects:
    """What jobs have done to the outside world, in order."""

    def __init__(self, disk):
        self.journal = Journal(disk, EFFECTS)

    def apply(self, job, result):
        self.journal.append({"job": job, "result": result})

    def applied(self):
        return [(event["job"], event["result"]) for event in self.journal.events()]


def run_until_idle(store, handler, effects, checkpoint_every=0):
    """Run HANDLER over the queued jobs until none is left, checkpointing every so many jobs."""
    finished = 0
    while True:
        claimed = store.claim()
        if claimed is None:
            return
        job, payload = claimed
        try:
            result = handler(payload)
        except JobError:
            store.fail(job)
            continue
        effects.apply(job, result)
        store.complete(job, result)
        finished += 1
        if checkpoint_every and finished % checkpoint_every == 0:
            store.checkpoint()
