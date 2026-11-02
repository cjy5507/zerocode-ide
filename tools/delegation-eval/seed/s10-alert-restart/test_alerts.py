import unittest

from alerter import Alerter
from outbox import Unavailable
from rules import Rule
from store import Power, PowerLoss, Store

RULES = [
    Rule("cpu", "cpu", 90, for_ticks=2, quiet_ticks=2, escalate_after=3),
    Rule("disk", "disk", 80),
]

# (what to call, its arguments). The wire is down during ticks 3 and 4.
SCRIPT = [
    ("observe", 1, "cpu", 95),
    ("observe", 1, "disk", 85),
    ("observe", 2, "cpu", 96),
    ("tick", 3),
    ("observe", 3, "disk", 10),
    ("tick", 4),
    ("tick", 5),
    ("tick", 6),
    ("observe", 7, "cpu", 10),
    ("observe", 8, "cpu", 95),
    ("observe", 9, "cpu", 10),
    ("tick", 10),
    ("tick", 11),
    ("observe", 12, "cpu", 99),
    ("observe", 13, "cpu", 99),
    ("ack", 14, "cpu"),
    ("tick", 16),
    ("observe", 17, "disk", 90),
]
DOWN = {3, 4}
DELIVERED = {
    "disk:1:fire": 1,
    "cpu:1:fire": 2,
    "cpu:1:escalate": 5,
    "disk:1:resolve": 6,
    "cpu:1:resolve": 11,
    "cpu:2:fire": 13,
    "disk:2:fire": 17,
}
ARRIVAL_ORDER = ["disk:1:fire", "cpu:1:fire", "cpu:1:escalate", "disk:1:resolve", "cpu:1:resolve", "cpu:2:fire", "disk:2:fire"]


class Wire:
    """The receiver: it remembers the first arrival of every key."""

    def __init__(self, power, down=()):
        self.power = power
        self.down = set(down)
        self.now = 0
        self.first = {}
        self.order = []
        self.tries = []
        self.messages = {}

    def send(self, message):
        self.power.step()
        self.tries.append((self.now, message["key"]))
        if self.now in self.down:
            raise Unavailable()
        if message["key"] not in self.first:
            self.first[message["key"]] = self.now
            self.order.append(message["key"])
            self.messages[message["key"]] = dict(message)
        self.power.step()


class World:
    def __init__(self, rules=RULES, down=DOWN):
        self.rules = rules
        self.power = Power()
        self.store = Store(self.power)
        self.wire = Wire(self.power, down)

    def alerter(self):
        return Alerter(self.rules, self.store, self.wire)

    def run(self, script=SCRIPT, losses=(), restart_before=None):
        """Play SCRIPT, repeating a call a power loss cut off, on a new alerter each time."""
        plan = list(losses)
        self.power.arm(plan.pop(0) if plan else None)
        alerter = None
        for position, (call, tick, *rest) in enumerate(script):
            self.wire.now = tick
            if position == restart_before:
                alerter = None
            for _ in range(len(losses) + 2):
                try:
                    if alerter is None:
                        alerter = self.alerter()
                    getattr(alerter, call)(tick, *rest)
                    break
                except PowerLoss:
                    alerter = None
                    self.power.arm(plan.pop(0) if plan else None)
            else:
                raise AssertionError("a call never finished")
        return alerter


def clean_steps():
    world = World()
    world.run()
    return world.power.steps


class CleanRunTest(unittest.TestCase):
    maxDiff = None

    def test_every_message_at_its_tick(self):
        world = World()
        world.run()
        self.assertEqual(world.wire.first, DELIVERED)
        self.assertEqual(world.wire.order, ARRIVAL_ORDER)

    def test_a_message_says_what_it_is(self):
        world = World()
        world.run()
        self.assertEqual(world.wire.messages["cpu:2:fire"],
                         {"key": "cpu:2:fire", "rule": "cpu", "kind": "fire", "incident": 2, "tick": 13})
        self.assertEqual(world.wire.messages["disk:1:resolve"],
                         {"key": "disk:1:resolve", "rule": "disk", "kind": "resolve", "incident": 1, "tick": 3})

    def test_the_state_in_the_store_is_plain_json_and_is_there(self):
        world = World()
        world.run()
        self.assertIsInstance(world.store.load(), (dict, list))


class FiringTest(unittest.TestCase):
    def test_a_break_in_the_readings_starts_the_count_again(self):
        world = World([Rule("r", "m", 5, for_ticks=3)], down=())
        script = [("observe", 1, "m", 9), ("observe", 2, "m", 9), ("observe", 3, "m", 1),
                  ("observe", 4, "m", 9), ("observe", 5, "m", 9)]
        world.run(script)
        self.assertEqual(world.wire.first, {})
        world.run([("observe", 6, "m", 9)])
        self.assertEqual(world.wire.first, {"r:1:fire": 6})

    def test_the_threshold_itself_counts_as_at_it(self):
        world = World([Rule("r", "m", 5)], down=())
        world.run([("observe", 1, "m", 5)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1})

    def test_one_metric_can_drive_two_rules(self):
        world = World([Rule("a", "m", 5), Rule("b", "m", 8)], down=())
        world.run([("observe", 1, "m", 6), ("observe", 2, "m", 9)])
        self.assertEqual(world.wire.first, {"a:1:fire": 1, "b:1:fire": 2})

    def test_cooling_is_noticed_by_a_tick_with_no_readings(self):
        world = World([Rule("r", "m", 5, quiet_ticks=3)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1), ("tick", 4), ("tick", 5)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1, "r:1:resolve": 5})

    def test_cooling_is_noticed_by_a_reading_too(self):
        world = World([Rule("r", "m", 5, quiet_ticks=2)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1), ("observe", 3, "m", 1), ("observe", 4, "m", 1)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1, "r:1:resolve": 4})

    def test_a_later_low_reading_does_not_start_cooling_again(self):
        world = World([Rule("r", "m", 5, quiet_ticks=3)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1), ("observe", 4, "m", 1), ("tick", 5)])
        self.assertEqual(world.wire.first["r:1:resolve"], 5)

    def test_a_reading_already_seen_changes_nothing(self):
        world = World([Rule("r", "m", 5, for_ticks=2)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 1, "m", 9), ("observe", 0, "m", 9)])
        self.assertEqual(world.wire.first, {})
        world.run([("observe", 2, "m", 9)])
        self.assertEqual(world.wire.first, {"r:1:fire": 2})


class EscalationTest(unittest.TestCase):
    RULE = [Rule("r", "m", 5, escalate_after=3)]

    def test_it_comes_once(self):
        world = World(self.RULE, down=())
        world.run([("observe", 1, "m", 9), ("tick", 3), ("tick", 4), ("tick", 5), ("tick", 9)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1, "r:1:escalate": 4})
        self.assertEqual(sum(key == "r:1:escalate" for _, key in world.wire.tries), 1)

    def test_a_reading_notices_it_too(self):
        world = World(self.RULE, down=())
        world.run([("observe", 1, "m", 9), ("observe", 4, "m", 9)])
        self.assertEqual(world.wire.first["r:1:escalate"], 4)

    def test_an_ack_cancels_it_and_lasts_only_for_the_incident(self):
        world = World(self.RULE, down=())
        world.run([("observe", 1, "m", 9), ("ack", 2, "r"), ("tick", 8), ("observe", 9, "m", 1),
                   ("observe", 10, "m", 9), ("tick", 13)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1, "r:1:resolve": 9, "r:2:fire": 10, "r:2:escalate": 13})

    def test_the_end_of_the_incident_cancels_it(self):
        world = World(self.RULE, down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1), ("tick", 9)])
        self.assertEqual(world.wire.first, {"r:1:fire": 1, "r:1:resolve": 2})


class DeliveryTest(unittest.TestCase):
    def test_the_delays_double_up_to_their_ceiling(self):
        world = World([Rule("r", "m", 5)], down=range(1, 60))
        world.run([("observe", 1, "m", 9)] + [("tick", t) for t in range(2, 60)])
        self.assertEqual([tick for tick, _ in world.wire.tries], [1, 2, 4, 8, 16, 32, 48])

    def test_the_delays_carry_on_after_a_restart(self):
        for cut in range(2, 40):
            world = World([Rule("r", "m", 5)], down=range(1, 60))
            script = [("observe", 1, "m", 9)] + [("tick", t) for t in range(2, 60)]
            world.run(script, restart_before=cut)
            self.assertEqual([tick for tick, _ in world.wire.tries], [1, 2, 4, 8, 16, 32, 48], cut)

    def test_a_waiting_message_holds_back_its_own_rule_and_no_other(self):
        world = World([Rule("a", "x", 5), Rule("b", "y", 5)], down={2})
        world.run([("observe", 1, "x", 9), ("observe", 2, "x", 1), ("observe", 2, "x", 9), ("observe", 2, "y", 9),
                   ("observe", 3, "x", 1), ("tick", 4)])
        self.assertEqual(world.wire.first, {"a:1:fire": 1, "a:1:resolve": 3, "b:1:fire": 3})
        self.assertEqual(world.wire.order, ["a:1:fire", "a:1:resolve", "b:1:fire"])

    def test_order_holds_through_a_long_outage(self):
        world = World([Rule("a", "x", 5)], down=range(2, 9))
        script = [("observe", 1, "x", 9), ("observe", 2, "x", 1), ("observe", 3, "x", 9), ("observe", 4, "x", 1), ("observe", 5, "x", 9)]
        world.run(script + [("tick", t) for t in range(6, 20)])
        self.assertEqual(world.wire.order, ["a:1:fire", "a:1:resolve", "a:2:fire", "a:2:resolve", "a:3:fire"])
        attempts = [key for _, key in world.wire.tries]
        self.assertEqual(attempts[:2], ["a:1:fire", "a:1:resolve"])

    def test_nothing_is_tried_again_once_it_got_through(self):
        world = World([Rule("r", "m", 5)], down=())
        world.run([("observe", 1, "m", 9), ("tick", 2), ("tick", 3)])
        self.assertEqual(world.wire.tries, [(1, "r:1:fire")])


class RestartTest(unittest.TestCase):
    maxDiff = None

    def assertSameRun(self, world, where):
        self.assertEqual(world.wire.first, DELIVERED, where)
        self.assertEqual(world.wire.order, ARRIVAL_ORDER, where)

    def test_a_quiet_restart_before_any_call(self):
        for position in range(len(SCRIPT)):
            world = World()
            world.run(restart_before=position)
            self.assertSameRun(world, position)

    def test_a_power_loss_at_every_step(self):
        for step in range(clean_steps()):
            world = World()
            world.run(losses=[step])
            self.assertSameRun(world, step)

    def test_a_second_power_loss_during_the_recovery(self):
        steps = clean_steps()
        for first in range(steps):
            for second in range(0, 12):
                world = World()
                world.run(losses=[first, second])
                self.assertSameRun(world, (first, second))

    def test_incident_numbers_outlive_a_restart(self):
        world = World([Rule("r", "m", 5)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1)])
        world.run([("observe", 3, "m", 9)], restart_before=0)
        self.assertEqual(world.wire.first["r:2:fire"], 3)

    def test_a_breach_count_outlives_a_restart(self):
        world = World([Rule("r", "m", 5, for_ticks=2)], down=())
        world.run([("observe", 1, "m", 9)])
        world.run([("observe", 2, "m", 9)], restart_before=0)
        self.assertEqual(world.wire.first, {"r:1:fire": 2})

    def test_an_ack_outlives_a_restart(self):
        world = World([Rule("r", "m", 5, escalate_after=2)], down=())
        world.run([("observe", 1, "m", 9), ("ack", 1, "r")])
        world.run([("tick", 5)], restart_before=0)
        self.assertEqual(world.wire.first, {"r:1:fire": 1})

    def test_a_cooling_period_outlives_a_restart(self):
        world = World([Rule("r", "m", 5, quiet_ticks=2)], down=())
        world.run([("observe", 1, "m", 9), ("observe", 2, "m", 1)])
        world.run([("tick", 4)], restart_before=0)
        self.assertEqual(world.wire.first["r:1:resolve"], 4)

    def test_a_message_waiting_in_the_outbox_outlives_a_restart(self):
        world = World([Rule("r", "m", 5)], down={1})
        world.run([("observe", 1, "m", 9)])
        self.assertEqual(world.wire.first, {})
        world.run([("tick", 2)], restart_before=0)
        self.assertEqual(world.wire.first, {"r:1:fire": 2})

    def test_a_message_cut_off_after_arriving_comes_again_under_the_same_key(self):
        world = World([Rule("r", "m", 5)], down=())
        world.run([("observe", 1, "m", 9)])
        steps = world.power.steps
        for step in range(steps):
            world = World([Rule("r", "m", 5)], down=())
            world.run([("observe", 1, "m", 9)], losses=[step])
            self.assertEqual(world.wire.first, {"r:1:fire": 1}, step)
            self.assertEqual({key for _, key in world.wire.tries}, {"r:1:fire"}, step)


if __name__ == "__main__":
    unittest.main()
