import { Inventory } from "./inventory";
import { Queue } from "./queue";

export function run(inventory: Inventory): number {
  const q = new Queue();
  q.push("job");
  const n = q.size() + q.limit;
  let total = n;
  total += inventory.m0();
  total += inventory.m1();
  total += inventory.m2();
  return total;
}
