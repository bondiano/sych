import { Inventory } from "./inventory";
import { Queue } from "./queue";

export function run(inventory: Inventory): number {
  const q = new Queue();
  q.items.push("job");
  const n = q.view.length + q.limit;
  let total = n;
  total += inventory.m0();
  total += inventory.m1();
  total += inventory.m2();
  total += inventory.m3();
  total += inventory.m4();
  total += inventory.m5();
  total += inventory.m6();
  total += inventory.m7();
  return total;
}
