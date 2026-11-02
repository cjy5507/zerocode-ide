import unittest

from disk import Disk, PowerLoss
from jobs import JobStore
from runner import Effects, JobError, run_until_idle

JOBS = [("a", {"n": 1}), ("b", {"n": 2}), ("c", {"n": 3}), ("d", {"n": 4}), ("e", {"n": 5})]
EXPECTED = sorted((job, payload["n"] * 10) for job, payload in JOBS)


def work(payload):
    return payload["n"] * 10


def boot(disk, jobs=JOBS, checkpoint_every=2):
    store = JobStore(disk)
    effects = Effects(disk)
    for job, payload in jobs:
        store.submit(job, payload)
    run_until_idle(store, work, effects, checkpoint_every)
    return store, effects


def drive(disk, losses):
    """Boot again after every power loss until a boot runs to its end."""
    plan = list(losses)
    for _ in range(len(plan) + 2):
        step, torn = plan.pop(0) if plan else (None, False)
        disk.arm(step, torn)
        try:
            return boot(disk)
        except PowerLoss:
            continue
    raise AssertionError("no boot ran to its end")


def clean_steps():
    disk = Disk()
    drive(disk, [])
    return disk.steps


class CleanRunTest(unittest.TestCase):
    def test_everything_runs_once(self):
        store, effects = drive(Disk(), [])
        self.assertEqual(sorted(effects.applied()), EXPECTED)
        for job, _ in JOBS:
            self.assertEqual(store.status(job)["state"], "done", job)

    def test_a_later_boot_finds_all_of_it(self):
        disk = Disk()
        drive(disk, [])
        store, effects = drive(disk, [])
        self.assertEqual(sorted(effects.applied()), EXPECTED)
        for job, payload in JOBS:
            held = store.status(job)
            self.assertEqual((held["state"], held["result"], held["attempts"]), ("done", payload["n"] * 10, 1), job)


class PowerLossTest(unittest.TestCase):
    def check(self, disk, store, effects, where):
        self.assertEqual(sorted(effects.applied()), EXPECTED, where)
        for job, payload in JOBS:
            held = store.status(job)
            self.assertEqual((held["state"], held["result"]), ("done", payload["n"] * 10), (where, job))
        again = JobStore(disk)
        self.assertEqual({job: again.status(job) for job, _ in JOBS}, {job: store.status(job) for job, _ in JOBS}, where)

    def test_one_loss_at_every_write(self):
        for torn in (False, True):
            for step in range(clean_steps()):
                disk = Disk()
                store, effects = drive(disk, [(step, torn)])
                self.check(disk, store, effects, (step, torn))

    def test_a_second_loss_during_the_recovery(self):
        steps = clean_steps()
        for torn in (False, True):
            for first in range(steps):
                for second in range(steps + 6):
                    disk = Disk()
                    store, effects = drive(disk, [(first, torn), (second, torn)])
                    self.check(disk, store, effects, (first, second, torn))

    def test_attempts_count_each_claim_once(self):
        for step in range(clean_steps()):
            disk = Disk()
            store, _ = drive(disk, [(step, False)])
            attempts = [store.status(job)["attempts"] for job, _ in JOBS]
            self.assertEqual(sorted(attempts)[:-1], [1] * (len(JOBS) - 1), step)
            self.assertIn(max(attempts), (1, 2), step)


class SubmitTest(unittest.TestCase):
    def test_submitting_again_changes_nothing(self):
        disk = Disk()
        store = JobStore(disk)
        store.submit("a", {"n": 1})
        self.assertEqual(store.claim(), ("a", {"n": 1}))
        store.submit("a", {"n": 99})
        self.assertEqual(store.status("a")["state"], "running")
        store.complete("a", 7)
        store.submit("a", {"n": 99})
        self.assertEqual(store.status("a")["state"], "done")
        self.assertEqual(store.status("a")["payload"], {"n": 1})
        self.assertIsNone(store.claim())


class TornRecordTest(unittest.TestCase):
    def test_half_a_record_is_dropped_and_nothing_joins_it(self):
        disk = Disk()
        store = JobStore(disk)
        store.submit("a", {"n": 1})
        disk.arm(0, torn=True)
        with self.assertRaises(PowerLoss):
            store.submit("b", {"n": 2})
        disk.arm(None)
        store = JobStore(disk)
        self.assertIsNone(store.jobs.get("b"))
        store.submit("c", {"n": 3})
        self.assertEqual(list(JobStore(disk).jobs), ["a", "c"])

    def test_half_an_effect_is_dropped_too(self):
        disk = Disk()
        effects = Effects(disk)
        effects.apply("a", 1)
        disk.arm(0, torn=True)
        with self.assertRaises(PowerLoss):
            effects.apply("b", 2)
        disk.arm(None)
        effects = Effects(disk)
        self.assertEqual(effects.applied(), [("a", 1)])
        effects.apply("c", 3)
        self.assertEqual(Effects(disk).applied(), [("a", 1), ("c", 3)])


class AttemptsTest(unittest.TestCase):
    def test_a_job_that_fails_is_tried_until_it_runs_out(self):
        calls = []

        def always_fails(payload):
            calls.append(payload)
            raise JobError()

        disk = Disk()
        store = JobStore(disk, max_attempts=3)
        effects = Effects(disk)
        store.submit("a", {"n": 1})
        run_until_idle(store, always_fails, effects)
        self.assertEqual(len(calls), 3)
        held = JobStore(disk, max_attempts=3).status("a")
        self.assertEqual((held["state"], held["attempts"]), ("dead", 3))
        self.assertEqual(effects.applied(), [])

    def test_a_job_that_recovers_is_done(self):
        left = [JobError(), JobError()]

        def twice(payload):
            if left:
                raise left.pop()
            return "fine"

        disk = Disk()
        store = JobStore(disk, max_attempts=3)
        store.submit("a", {"n": 1})
        run_until_idle(store, twice, Effects(disk))
        held = store.status("a")
        self.assertEqual((held["state"], held["attempts"], held["result"]), ("done", 3, "fine"))

    def test_attempts_outlive_a_checkpoint_and_a_boot(self):
        def fails(payload):
            raise JobError()

        disk = Disk()
        store = JobStore(disk, max_attempts=2)
        store.submit("a", {"n": 1})
        store.claim()
        store.fail("a")
        store.checkpoint()
        store = JobStore(disk, max_attempts=2)
        self.assertEqual(store.status("a")["attempts"], 1)
        run_until_idle(store, fails, Effects(disk))
        self.assertEqual(JobStore(disk, max_attempts=2).status("a")["state"], "dead")

    def test_a_job_cut_off_by_power_is_queued_again_with_its_attempt_counted(self):
        disk = Disk()
        store = JobStore(disk, max_attempts=2)
        store.submit("a", {"n": 1})
        store.claim()
        store = JobStore(disk, max_attempts=2)
        held = store.status("a")
        self.assertEqual((held["state"], held["attempts"]), ("queued", 1))
        self.assertEqual(store.claim(), ("a", {"n": 1}))

    def test_a_job_cut_off_on_its_last_attempt_is_dead_and_its_effect_never_happens(self):
        def cut(payload):
            raise PowerLoss()

        disk = Disk()
        for _ in range(2):
            store = JobStore(disk, max_attempts=2)
            store.submit("a", {"n": 1})
            with self.assertRaises(PowerLoss):
                run_until_idle(store, cut, Effects(disk))
        store = JobStore(disk, max_attempts=2)
        held = store.status("a")
        self.assertEqual((held["state"], held["attempts"]), ("dead", 2))
        run_until_idle(store, work, Effects(disk))
        self.assertEqual(Effects(disk).applied(), [])
        self.assertEqual(JobStore(disk, max_attempts=2).status("a")["state"], "dead")


class OrderTest(unittest.TestCase):
    def test_a_job_queued_again_keeps_its_place(self):
        disk = Disk()
        store = JobStore(disk)
        for job in "abc":
            store.submit(job, {})
        self.assertEqual(store.claim()[0], "a")
        store = JobStore(disk)
        self.assertEqual([store.claim()[0], store.claim()[0], store.claim()[0]], ["a", "b", "c"])

    def test_order_comes_through_a_checkpoint(self):
        disk = Disk()
        store = JobStore(disk)
        for job in ("z", "m", "a"):
            store.submit(job, {})
        store.checkpoint()
        store = JobStore(disk)
        self.assertEqual([store.claim()[0], store.claim()[0], store.claim()[0]], ["z", "m", "a"])


class CheckpointTest(unittest.TestCase):
    def test_it_leaves_the_journal_empty(self):
        disk = Disk()
        store = JobStore(disk)
        store.submit("a", {})
        self.assertGreater(store.journal.size(), 0)
        store.checkpoint()
        self.assertEqual(store.journal.size(), 0)

    def test_every_cut_reads_back_the_same_jobs(self):
        for torn in (False, True):
            for step in range(8):
                disk = Disk()
                store = JobStore(disk)
                store.submit("a", {"n": 1})
                store.submit("b", {"n": 2})
                store.claim()
                store.complete("a", 10)
                store.claim()
                store.fail("b")
                expected = {job: store.status(job) for job in "ab"}
                disk.arm(step, torn)
                try:
                    store.checkpoint()
                except PowerLoss:
                    pass
                disk.arm(None)
                again = JobStore(disk)
                self.assertEqual({job: again.status(job) for job in "ab"}, expected, (step, torn))

    def test_a_second_checkpoint_after_a_cut_one_still_works(self):
        for step in range(8):
            disk = Disk()
            store = JobStore(disk)
            store.submit("a", {"n": 1})
            disk.arm(step, True)
            try:
                store.checkpoint()
            except PowerLoss:
                pass
            disk.arm(None)
            store = JobStore(disk)
            store.submit("b", {"n": 2})
            store.checkpoint()
            self.assertEqual(sorted(JobStore(disk).jobs), ["a", "b"], step)
            self.assertEqual(store.journal.size(), 0, step)


if __name__ == "__main__":
    unittest.main()
