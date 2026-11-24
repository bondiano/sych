use crate::inventory::Inventory;
use crate::queue::Queue;

pub fn run(inventory: &Inventory) -> usize {
    let mut q = Queue::new();
    q.push("job".into());
    let n = q.len() + q.limit;
    let mut total = n;
    total += inventory.m0();
    total += inventory.m1();
    total += inventory.m2();
    total
}
