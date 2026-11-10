package ternary

import (
	"errors"
	"math"
	"math/rand"
	"testing"
)

func TestEncodeDecodeRoundTrip(t *testing.T) {
	values := []int64{
		0, 1, -1, 2, -2, 3, -3, 4, -4, 5, -5,
		9, -9, 10, -10, 27, -27, 28, -28,
		1 << 20, -(1 << 20),
		math.MaxInt64, math.MinInt64,
	}
	for n := int64(-20000); n <= 20000; n++ {
		values = append(values, n)
	}
	for _, want := range values {
		got, err := DecodeInt64(EncodeInt64(want))
		if err != nil {
			t.Fatalf("DecodeInt64(EncodeInt64(%d)): %v", want, err)
		}
		if got != want {
			t.Fatalf("round trip %d: got %d", want, got)
		}
	}
}

func TestDecodeOverflow(t *testing.T) {
	ts := make([]Trit, 41)
	for i := range ts {
		ts[i] = Pos
	}
	if _, err := DecodeInt64(ts); !errors.Is(err, ErrOverflow) {
		t.Fatalf("41 positive trits: got %v, want ErrOverflow", err)
	}
	if _, err := DecodeInt64([]Trit{Pos, 2}); !errors.Is(err, ErrTrit) {
		t.Fatalf("invalid trit: got %v, want ErrTrit", err)
	}
}

func TestAdd(t *testing.T) {
	rng := rand.New(rand.NewSource(1))
	for i := 0; i < 20000; i++ {
		a := rng.Int63n(2e12) - 1e12
		b := rng.Int63n(2e12) - 1e12
		got, err := DecodeInt64(Add(EncodeInt64(a), EncodeInt64(b)))
		if err != nil {
			t.Fatalf("Add(%d, %d): %v", a, b, err)
		}
		if got != a+b {
			t.Fatalf("Add(%d, %d) = %d", a, b, got)
		}
	}
}

func TestAddIdentityAndNegate(t *testing.T) {
	a := EncodeInt64(123456789)
	got, err := DecodeInt64(Add(a, []Trit{Zero}))
	if err != nil || got != 123456789 {
		t.Fatalf("a+0 = %d, %v", got, err)
	}
	got, err = DecodeInt64(Negate(a))
	if err != nil || got != -123456789 {
		t.Fatalf("-a = %d, %v", got, err)
	}
	if s := Format(a); s != "+00--++0-+-+0-0-00" {
		t.Fatalf("Format(123456789) = %q", s)
	}
}

func TestKleeneTables(t *testing.T) {
	all := []Kleene{KFalse, KUnknown, KTrue}
	for _, a := range all {
		for _, b := range all {
			wantAnd, wantOr := KUnknown, KUnknown
			switch {
			case a == KFalse || b == KFalse:
				wantAnd = KFalse
			case a == KTrue && b == KTrue:
				wantAnd = KTrue
			}
			switch {
			case a == KTrue || b == KTrue:
				wantOr = KTrue
			case a == KFalse && b == KFalse:
				wantOr = KFalse
			}
			if got := KAnd(a, b); got != wantAnd {
				t.Fatalf("KAnd(%v, %v) = %v, want %v", a, b, got, wantAnd)
			}
			if got := KOr(a, b); got != wantOr {
				t.Fatalf("KOr(%v, %v) = %v, want %v", a, b, got, wantOr)
			}
		}
	}
	if KAnd(KUnknown, KFalse) != KFalse || KOr(KUnknown, KTrue) != KTrue {
		t.Fatal("decisive operand must win")
	}
	if KAll() != KTrue || KAll(KTrue, KUnknown) != KUnknown || KAll(KTrue, KFalse) != KFalse {
		t.Fatal("KAll fold is wrong")
	}
}

func TestKGate(t *testing.T) {
	// Columns are verified = F, U, T.
	table := [3][3]Kleene{
		{KTrue, KTrue, KTrue},          // required = False
		{KUnknown, KUnknown, KUnknown}, // required = Unknown
		{KFalse, KUnknown, KTrue},      // required = True
	}
	for r := 0; r < 3; r++ {
		for v := 0; v < 3; v++ {
			got := KGate(Kleene(r), Kleene(v))
			if got != table[r][v] {
				t.Fatalf("KGate(%d, %d) = %v, want %v", r, v, got, table[r][v])
			}
		}
	}
	// The reason KGate is not implication: unknown demand must not be
	// admitted just because the predecessor succeeded.
	if KGate(KUnknown, KTrue) != KUnknown {
		t.Fatal("unknown demand was admitted")
	}
	if KGate(KTrue, KTrue) != KTrue || KGate(KTrue, KFalse) != KFalse {
		t.Fatal("known demand must follow verified verdict")
	}
	if KGate(Kleene(9), KTrue) != KUnknown {
		t.Fatal("invalid Kleene value must be Unknown")
	}
}

func TestPack5(t *testing.T) {
	rng := rand.New(rand.NewSource(2))
	for n := 0; n <= 200; n++ {
		ts := make([]Trit, n)
		for i := range ts {
			ts[i] = Trit(rng.Intn(3) - 1)
		}
		data, err := Pack5(ts)
		if err != nil {
			t.Fatalf("Pack5(len=%d): %v", n, err)
		}
		for _, b := range data {
			if b >= 243 {
				t.Fatalf("Pack5 produced invalid byte %d", b)
			}
		}
		got, err := Unpack5(data, n)
		if err != nil {
			t.Fatalf("Unpack5(len=%d): %v", n, err)
		}
		if len(got) != n {
			t.Fatalf("Unpack5 len = %d, want %d", len(got), n)
		}
		for i := range ts {
			if got[i] != ts[i] {
				t.Fatalf("round trip at %d: %v != %v", i, got[i], ts[i])
			}
		}
	}
	if _, err := Unpack5([]byte{243}, 1); !errors.Is(err, ErrPackRange) {
		t.Fatalf("invalid packed byte: got %v", err)
	}
	if _, err := Unpack5(nil, 1); !errors.Is(err, ErrShortData) {
		t.Fatalf("short data: got %v", err)
	}
	if _, err := Unpack5(nil, -1); err == nil {
		t.Fatal("negative count must fail")
	}
	if _, err := Pack5([]Trit{Pos, 2}); !errors.Is(err, ErrTrit) {
		t.Fatalf("invalid trit in Pack5: got %v", err)
	}
}

func TestFormat(t *testing.T) {
	if Format(nil) != "0" || Format([]Trit{Zero}) != "0" {
		t.Fatal("zero formatting")
	}
	if Format(EncodeInt64(5)) != "+--" {
		t.Fatalf("5 = %q", Format(EncodeInt64(5)))
	}
	if Format(EncodeInt64(-5)) != "-++" {
		t.Fatalf("-5 = %q", Format(EncodeInt64(-5)))
	}
}
