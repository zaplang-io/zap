package main

import "fmt"

func main() {
	d := make(map[int64]int64)
	for i := int64(0); i < 200000; i++ {
		d[i] = i
	}
	var s int64 = 0
	for i := int64(0); i < 200000; i++ {
		s += d[i]
	}
	fmt.Println(s)
}
