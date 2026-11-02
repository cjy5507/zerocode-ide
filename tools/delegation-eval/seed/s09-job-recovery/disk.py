"""A disk kept in memory, which can lose power during any write.

Every write is one step. `arm(step)` makes that step the one the power fails on: the write does
not happen, or with `torn=True` part of it does, and `PowerLoss` is raised. Reads never fail.
"""


class PowerLoss(BaseException):
    """The power went. Nothing in memory survives this."""


class Disk:
    def __init__(self):
        self._files = {}
        self.steps = 0
        self._fail_at = None
        self._torn = False

    def arm(self, step, torn=False):
        """Count writes from zero, and fail the one numbered STEP (None for none)."""
        self.steps = 0
        self._fail_at = step
        self._torn = torn

    def _step(self, partly=None):
        here = self.steps
        self.steps += 1
        if here == self._fail_at:
            self._fail_at = None
            if self._torn and partly:
                partly()
            raise PowerLoss()

    def read(self, name):
        """The bytes of file NAME, or None when there is no such file."""
        return self._files.get(name)

    def names(self):
        return sorted(self._files)

    def append(self, name, data):
        def part():
            self._files[name] = self._files.get(name, b"") + data[: len(data) // 2]

        self._step(part)
        self._files[name] = self._files.get(name, b"") + data

    def write(self, name, data):
        """Replace the content of NAME. This is not atomic: a torn write leaves part of DATA."""

        def part():
            self._files[name] = data[: len(data) // 2]

        self._step(part)
        self._files[name] = data

    def rename(self, source, target):
        """Move SOURCE over TARGET, all at once."""
        self._step()
        self._files[target] = self._files.pop(source)

    def truncate(self, name, size):
        """Cut NAME down to SIZE bytes, all at once."""
        self._step()
        self._files[name] = self._files[name][:size]

    def remove(self, name):
        """Delete NAME, all at once. A file already gone is no error."""
        self._step()
        self._files.pop(name, None)
