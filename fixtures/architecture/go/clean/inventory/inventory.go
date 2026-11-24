package inventory

type Inventory struct {
	Count int
}

func (i *Inventory) M0() int {
	return i.Count + 0
}

func (i *Inventory) M1() int {
	return i.Count + 1
}

func (i *Inventory) M2() int {
	return i.Count + 2
}

func (i *Inventory) M3() int {
	return i.Count + 3
}

func (i *Inventory) M4() int {
	return i.Count + 4
}

func (i *Inventory) M5() int {
	return i.Count + 5
}

func (i *Inventory) M6() int {
	return i.Count + 6
}

func (i *Inventory) M7() int {
	return i.Count + 7
}
