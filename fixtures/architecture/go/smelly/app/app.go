package app

import (
	"example.com/smelly/inventory"
	"example.com/smelly/queue"
)

func Run(inv *inventory.Inventory) int {
	q := queue.New()
	q.Items = append(q.Items, "job")
	n := len(q.View()) + q.Limit
	total := n
	total += inv.M0()
	total += inv.M1()
	total += inv.M2()
	total += inv.M3()
	total += inv.M4()
	total += inv.M5()
	total += inv.M6()
	total += inv.M7()
	return total
}
