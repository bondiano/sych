from clean.inventory import Inventory
from clean.queue import Queue


def run(inventory: Inventory) -> int:
    q = Queue()
    q.push("job")
    n = q.size() + q.limit
    total = n
    total += inventory.m0()
    total += inventory.m1()
    total += inventory.m2()
    return total
