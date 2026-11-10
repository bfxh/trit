package main

import (
	"fmt"

	t "github.com/bfxh/trit/other-languages/go"
)

func main() {
	for _, n := range []int64{0, 1, 2, 3, 4, 5, -5, 9, 10} {
		fmt.Printf("%d = %s\n", n, t.Format(t.EncodeInt64(n)))
	}

	a, b := int64(123456789), int64(-987654321)
	sum, err := t.DecodeInt64(t.Add(t.EncodeInt64(a), t.EncodeInt64(b)))
	if err != nil {
		panic(err)
	}
	fmt.Printf("%d + %d = %d\n", a, b, sum)

	trits := []t.Trit{t.Pos, t.Neg, t.Zero, t.Pos, t.Pos, t.Neg, t.Zero}
	packed, err := t.Pack5(trits)
	if err != nil {
		panic(err)
	}
	unpacked, err := t.Unpack5(packed, len(trits))
	if err != nil {
		panic(err)
	}
	fmt.Printf("pack %s -> %v -> %s\n", t.Format(trits), packed, t.Format(unpacked))

	fmt.Println("KGate(required, verified):")
	for _, r := range []t.Kleene{t.KFalse, t.KUnknown, t.KTrue} {
		for _, v := range []t.Kleene{t.KFalse, t.KUnknown, t.KTrue} {
			fmt.Printf("  %v,%v -> %v\n", r, v, t.KGate(r, v))
		}
	}
}
