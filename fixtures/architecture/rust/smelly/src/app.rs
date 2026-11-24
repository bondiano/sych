use crate::inventory::Inventory;
use crate::queue::Queue;

pub fn run(inventory: &Inventory) -> usize {
    let mut q = Queue::new();
    q.items.push("job".into());
    let n = q.items().len() + q.limit;
    let mut total = n;
    total += inventory.m0();
    total += inventory.m1();
    total += inventory.m2();
    total += inventory.m3();
    total += inventory.m4();
    total += inventory.m5();
    total += inventory.m6();
    total += inventory.m7();
    total
}
