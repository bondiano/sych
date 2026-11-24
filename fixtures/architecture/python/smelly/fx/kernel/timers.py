from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from fx.kernel import Tasks


class Timers:
    def __init__(self) -> None:
        self.armed = 0

    def arm(self) -> None:
        self.armed += 1

    def fire(self, tasks: Tasks) -> None:
        tasks.count += 1
