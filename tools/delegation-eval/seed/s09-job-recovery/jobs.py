"""A durable job queue: the jobs are rebuilt from a snapshot and a journal at every boot."""

import json

from journal import Journal

JOURNAL = "jobs.journal"
SNAPSHOT = "jobs.snapshot"


class JobStore:
    def __init__(self, disk, max_attempts=3):
        self.disk = disk
        self.max_attempts = max_attempts
        self.journal = Journal(disk, JOURNAL)
        self.jobs = {}
        snapshot = disk.read(SNAPSHOT)
        if snapshot:
            self.jobs = json.loads(snapshot)
        for event in self.journal.events():
            self._apply(event)

    def _apply(self, event):
        kind, job = event["kind"], event["job"]
        if kind == "submit":
            self.jobs[job] = {"payload": event["payload"], "state": "queued", "attempts": 0, "result": None}
            return
        held = self.jobs[job]
        if kind == "claim":
            held["state"] = "running"
            held["attempts"] += 1
        elif kind == "done":
            held["state"] = "done"
            held["result"] = event["result"]
        elif kind == "fail":
            held["state"] = "dead" if held["attempts"] >= self.max_attempts else "queued"

    def _record(self, event):
        self.journal.append(event)
        self._apply(event)

    def submit(self, job, payload):
        self._record({"kind": "submit", "job": job, "payload": payload})

    def claim(self):
        """The (id, payload) of the first job queued, now running; None when none is."""
        for job, held in self.jobs.items():
            if held["state"] == "queued":
                self._record({"kind": "claim", "job": job})
                return job, held["payload"]
        return None

    def complete(self, job, result):
        self._record({"kind": "done", "job": job, "result": result})

    def fail(self, job):
        self._record({"kind": "fail", "job": job})

    def status(self, job):
        return dict(self.jobs[job])

    def checkpoint(self):
        """Move what the journal holds into the snapshot."""
        self.journal.clear()
        self.disk.write(SNAPSHOT, json.dumps(self.jobs).encode())
