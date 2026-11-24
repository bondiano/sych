class Timers:
    def __init__(self) -> None:
        self.armed = 0

    def arm(self) -> None:
        self.armed += 1

    def fire(self) -> None:
        self.armed -= 1
