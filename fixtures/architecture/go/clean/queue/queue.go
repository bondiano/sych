package queue

type Queue struct {
	Items  []string
	Limit  int
	served int
}

func New() *Queue {
	return &Queue{Limit: 3}
}

func (q *Queue) Full() bool {
	return len(q.Items)+q.served >= q.Limit
}

func (q *Queue) View() []string { return q.Items }

func (q *Queue) Push(item string) {
	if !q.Full() {
		q.Items = append(q.Items, item)
	}
}

func (q *Queue) Len() int {
	return len(q.Items)
}
