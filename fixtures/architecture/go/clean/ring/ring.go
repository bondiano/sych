package ring

type T0 struct {
	n int
}

func (t *T0) Next(o *T1) int {
	return t.n + o.n
}

type T1 struct {
	n int
}

func (t *T1) Next(o *T2) int {
	return t.n + o.n
}

type T2 struct {
	n int
}

func (t *T2) Next(o *T3) int {
	return t.n + o.n
}

type T3 struct {
	n int
}

func (t *T3) Next(o *T4) int {
	return t.n + o.n
}

type T4 struct {
	n int
}

func (t *T4) Next(o *T5) int {
	return t.n + o.n
}

type T5 struct {
	n int
}

func (t *T5) Next(o *T6) int {
	return t.n + o.n
}

type T6 struct {
	n int
}

func (t *T6) Next(o *T0) int {
	return t.n + o.n
}
