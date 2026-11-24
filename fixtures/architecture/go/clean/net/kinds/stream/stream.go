package stream

const Limit = 8

type Stream struct{}

func Fits(n int) bool {
	return n < Limit
}
