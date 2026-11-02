"""The durable part of the alerter's world: a store, and the power that can fail."""

import json


class PowerLoss(BaseException):
    """The power went. Nothing in memory survives this."""


class Power:
    """Counts the steps that touch the outside world, and can fail on one of them."""

    def __init__(self):
        self.steps = 0
        self._fail_at = None

    def arm(self, step):
        self.steps = 0
        self._fail_at = step

    def step(self):
        here = self.steps
        self.steps += 1
        if here == self._fail_at:
            self._fail_at = None
            raise PowerLoss()


class Store:
    """One JSON value, replaced all at once. A failed save leaves the old value."""

    def __init__(self, power):
        self.power = power
        self._text = None

    def save(self, state):
        text = json.dumps(state, sort_keys=True)
        self.power.step()
        self._text = text

    def load(self):
        return None if self._text is None else json.loads(self._text)
