from fx.inventory import Inventory
from fx.queue import Queue


def run(inventory: Inventory) -> int:
    q = Queue()
    q.items.append("job")
    n = len(q.view) + q.limit
    total = n
    total += inventory.m0()
    total += inventory.m1()
    total += inventory.m2()
    total += inventory.m3()
    total += inventory.m4()
    total += inventory.m5()
    total += inventory.m6()
    total += inventory.m7()
    return total
