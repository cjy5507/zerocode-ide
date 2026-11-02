"""One rule and where it stands."""


class Rule:
    def __init__(self, name, metric, threshold, for_ticks=1, quiet_ticks=0, escalate_after=None):
        self.name = name
        self.metric = metric
        self.threshold = threshold
        self.for_ticks = for_ticks
        self.quiet_ticks = quiet_ticks
        self.escalate_after = escalate_after


class RuleState:
    def __init__(self, rule):
        self.rule = rule
        self.breaches = 0
        self.firing = False
        self.incident = 0

    def observe(self, tick, value):
        """The kinds of message this reading gives rise to."""
        if value >= self.rule.threshold:
            self.breaches += 1
            if not self.firing and self.breaches >= self.rule.for_ticks:
                self.firing = True
                self.incident += 1
                return ["fire"]
            return []
        self.breaches = 0
        if self.firing:
            self.firing = False
            return ["resolve"]
        return []
