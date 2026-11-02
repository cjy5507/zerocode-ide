"""The messages on their way out."""


class Unavailable(Exception):
    """The receiver cannot take a message now."""


class Outbox:
    def __init__(self):
        self.messages = []

    def add(self, message):
        self.messages.append(message)

    def deliver(self, transport):
        """Send everything waiting; what fails stays for next time."""
        for message in list(self.messages):
            try:
                transport.send(message)
            except Unavailable:
                continue
            self.messages.remove(message)
