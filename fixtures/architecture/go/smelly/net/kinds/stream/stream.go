package stream

import "example.com/fx/net/socket/addr"

type Stream struct{}

func Fits(n int) bool {
	return n < addr.Limit
}
