from clean.kernel.timers import Timers


class Tasks:
    def __init__(self) -> None:
        self.count = 0

    def spawn(self, timers: Timers) -> None:
        timers.arm()
        self.count += 1


def tick(tasks: Tasks, timers: Timers) -> None:
    timers.fire()
    tasks.count += 1
