package queue

import "testing"

func TestQueue(t *testing.T) {
	q := New()
	q.Items = append(q.Items, "t")
}
