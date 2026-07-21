package main

import (
	"fmt"
	"example.test/pira/model"
)

func main() {
	fmt.Println(model.NewUser("Ada").Label())
}
