package app

import (
	"example.com/fx/inventory"
	"example.com/fx/queue"
)

func Run(inv *inventory.Inventory) int {
	q := queue.New()
	q.Push("job")
	n := q.Len() + q.Limit
	total := n
	total += inv.M0()
	total += inv.M1()
	total += inv.M2()
	return total
}
