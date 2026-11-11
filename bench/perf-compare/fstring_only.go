package main

import (
	"fmt"
	"strconv"
)

func main() {
	var total int64 = 0
	for pass := 0; pass < 2; pass++ {
		for i := int64(0); i < 200000; i++ {
			s := "k" + strconv.FormatInt(i, 10)
			total += int64(len(s))
		}
	}
	fmt.Println(total)
}
