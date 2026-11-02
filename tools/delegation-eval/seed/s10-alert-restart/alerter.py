"""Rules watching metrics and telling someone: see ALERTS.md."""

from outbox import Outbox
from rules import RuleState


class Alerter:
    def __init__(self, rules, store, transport):
        self.rules = rules
        self.store = store
        self.transport = transport
        self.states = {rule.name: RuleState(rule) for rule in rules}
        self.outbox = Outbox()
        self.sent = 0

    def _message(self, rule, kind, tick):
        self.sent += 1
        state = self.states[rule.name]
        return {"key": f"{rule.name}:{self.sent}:{kind}", "rule": rule.name, "kind": kind,
                "incident": state.incident, "tick": tick}

    def observe(self, tick, metric, value):
        """A reading of METRIC at TICK."""
        for rule in self.rules:
            if rule.metric == metric:
                for kind in self.states[rule.name].observe(tick, value):
                    self.outbox.add(self._message(rule, kind, tick))
        self.outbox.deliver(self.transport)
        self.store.save({"sent": self.sent})

    def tick(self, now):
        """Time has reached NOW."""
        self.outbox.deliver(self.transport)
        self.store.save({"sent": self.sent})

    def ack(self, now, rule_name):
        """A person has seen the open incident of the rule."""
