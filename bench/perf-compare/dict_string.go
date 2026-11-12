package main

import (
	"fmt"
	"strconv"
)

func main() {
	d := make(map[string]int64)
	for i := int64(0); i < 200000; i++ {
		d["k"+strconv.FormatInt(i, 10)] = i
	}
	var s int64 = 0
	for i := int64(0); i < 200000; i++ {
		s += d["k"+strconv.FormatInt(i, 10)]
	}
	fmt.Println(s)
}
