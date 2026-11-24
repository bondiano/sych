class Queue:
    def __init__(self) -> None:
        self.items: list[str] = []
        self.limit = 3
        self._served = 0

    def full(self) -> bool:
        return len(self.items) + self._served >= self.limit

    @property
    def view(self) -> list[str]:
        return self.items

    def push(self, item: str) -> None:
        if not self.full():
            self.items.append(item)

    def size(self) -> int:
        return len(self.items)
