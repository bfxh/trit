// Package ternary provides balanced-ternary integers, Kleene K3 logic,
// and a 5-trits-per-byte wire format.
//
// A balanced-ternary digit (trit) is -1, 0, or +1 with weight 3^k.
// Arithmetic zero and logical Unknown are deliberately separate types:
// a zero trit is a value, not missing evidence.
//
// This is a reference implementation for other languages, not a stable API
// and not part of the default trit workspace. See ../README.md.
package ternary

import (
	"errors"
	"fmt"
	"math/big"
	"strings"
)

// Trit is one balanced-ternary digit: Neg (-1), Zero (0), or Pos (+1).
type Trit int8

const (
	Neg  Trit = -1
	Zero Trit = 0
	Pos  Trit = 1
)

var (
	ErrTrit      = errors.New("ternary: trit out of range")
	ErrOverflow  = errors.New("ternary: value does not fit int64")
	ErrPackRange = errors.New("ternary: packed byte is not a valid 5-trit group")
	ErrShortData = errors.New("ternary: not enough packed trits")
)

func (t Trit) Valid() bool { return t >= Neg && t <= Pos }

func (t Trit) String() string {
	switch t {
	case Neg:
		return "-"
	case Zero:
		return "0"
	case Pos:
		return "+"
	default:
		return "?"
	}
}

// Format renders the most-significant trit first, e.g. "+--" for 5.
func Format(ts []Trit) string {
	if len(ts) == 0 {
		return "0"
	}
	var b strings.Builder
	b.Grow(len(ts))
	for i := len(ts) - 1; i >= 0; i-- {
		b.WriteString(ts[i].String())
	}
	return b.String()
}

// EncodeInt64 returns the balanced-ternary digits of n, least significant first.
func EncodeInt64(n int64) []Trit {
	if n == 0 {
		return []Trit{Zero}
	}
	out := make([]Trit, 0, 41)
	for n != 0 {
		r := n % 3
		n /= 3
		switch r {
		case 0:
			out = append(out, Zero)
		case 1:
			out = append(out, Pos)
		case 2:
			out = append(out, Neg)
			n++
		case -1:
			out = append(out, Neg)
		case -2:
			out = append(out, Pos)
			n--
		}
	}
	return trim(out)
}

// DecodeInt64 converts balanced-ternary digits to an int64.
// It accepts any length and reports overflow instead of wrapping.
func DecodeInt64(ts []Trit) (int64, error) {
	var n, three, d big.Int
	three.SetInt64(3)
	for i := len(ts) - 1; i >= 0; i-- {
		t := ts[i]
		if !t.Valid() {
			return 0, ErrTrit
		}
		n.Mul(&n, &three)
		d.SetInt64(int64(t))
		n.Add(&n, &d)
	}
	if !n.IsInt64() {
		return 0, ErrOverflow
	}
	return n.Int64(), nil
}

// Negate flips every trit. In balanced ternary this is integer negation with
// no separate sign bit or two's-complement fixup.
func Negate(ts []Trit) []Trit {
	out := make([]Trit, len(ts))
	for i, t := range ts {
		out[i] = -t
	}
	return trim(out)
}

// Add returns a+b in balanced ternary. Inputs must contain only valid trits.
func Add(a, b []Trit) []Trit {
	n := len(a)
	if len(b) > n {
		n = len(b)
	}
	out := make([]Trit, 0, n+1)
	carry := 0
	for i := 0; i < n; i++ {
		x, y := 0, 0
		if i < len(a) {
			x = int(a[i])
		}
		if i < len(b) {
			y = int(b[i])
		}
		s := x + y + carry
		var digit int
		switch s {
		case -3:
			digit, carry = 0, -1
		case -2:
			digit, carry = 1, -1
		case -1:
			digit, carry = -1, 0
		case 0:
			digit, carry = 0, 0
		case 1:
			digit, carry = 1, 0
		case 2:
			digit, carry = -1, 1
		case 3:
			digit, carry = 0, 1
		default:
			panic("ternary: invalid trit in Add")
		}
		out = append(out, Trit(digit))
	}
	if carry != 0 {
		out = append(out, Trit(carry))
	}
	return trim(out)
}

func trim(ts []Trit) []Trit {
	if len(ts) == 0 {
		return []Trit{Zero}
	}
	i := len(ts)
	for i > 1 && ts[i-1] == Zero {
		i--
	}
	return ts[:i]
}

// Kleene is the three-valued logic domain: False, Unknown, True.
// Unknown means missing evidence, not probability 0.5.
type Kleene uint8

const (
	KFalse Kleene = iota
	KUnknown
	KTrue
)

func (k Kleene) Valid() bool { return k <= KTrue }

func (k Kleene) String() string {
	switch k {
	case KFalse:
		return "F"
	case KUnknown:
		return "U"
	case KTrue:
		return "T"
	default:
		return "?"
	}
}

func (k Kleene) Not() Kleene {
	switch k {
	case KFalse:
		return KTrue
	case KTrue:
		return KFalse
	default:
		return KUnknown
	}
}

// KAnd is Kleene strong conjunction: a decisive False wins, otherwise
// both operands must be True.
func KAnd(a, b Kleene) Kleene {
	if a == KFalse || b == KFalse {
		return KFalse
	}
	if a == KTrue && b == KTrue {
		return KTrue
	}
	return KUnknown
}

// KOr is Kleene strong disjunction: a decisive True wins, otherwise
// both operands must be False.
func KOr(a, b Kleene) Kleene {
	if a == KTrue || b == KTrue {
		return KTrue
	}
	if a == KFalse && b == KFalse {
		return KFalse
	}
	return KUnknown
}

// KAll folds KAnd over ks. An empty input is True.
func KAll(ks ...Kleene) Kleene {
	out := KTrue
	for _, k := range ks {
		out = KAnd(out, k)
	}
	return out
}

// KGate is the dependency-admission table:
//
//	required \ verified | T       | U       | F
//	F (not needed)      | T       | T       | T
//	U (not confirmed)   | U       | U       | U
//	T (required)        | T       | U       | F
//
// It is intentionally not Kleene implication (!required OR verified):
// with required=U and verified=T, implication would admit, while this
// table keeps the task suspended because the demand itself is unknown.
func KGate(required, verified Kleene) Kleene {
	if !required.Valid() || !verified.Valid() {
		return KUnknown
	}
	switch required {
	case KFalse:
		return KTrue
	case KUnknown:
		return KUnknown
	default: // KTrue
		return verified
	}
}

// TritsPerByte is the packing density of the wire format: 3^5 = 243,
// so five trits fit in one byte with 13 unused code points.
const TritsPerByte = 5

// Pack5 encodes trits as five per byte, least significant trit first.
// Missing trits in the final group are padded with Zero.
func Pack5(ts []Trit) ([]byte, error) {
	out := make([]byte, 0, (len(ts)+TritsPerByte-1)/TritsPerByte)
	for i := 0; i < len(ts); i += TritsPerByte {
		v, pow := 0, 1
		for j := 0; j < TritsPerByte; j++ {
			t := Zero
			if i+j < len(ts) {
				t = ts[i+j]
			}
			if !t.Valid() {
				return nil, ErrTrit
			}
			v += (int(t) + 1) * pow
			pow *= 3
		}
		out = append(out, byte(v))
	}
	return out, nil
}

// Unpack5 decodes exactly n trits from data produced by Pack5.
func Unpack5(data []byte, n int) ([]Trit, error) {
	if n < 0 {
		return nil, fmt.Errorf("ternary: negative trit count %d", n)
	}
	if n > len(data)*TritsPerByte {
		return nil, ErrShortData
	}
	out := make([]Trit, 0, n)
	for i := 0; i < n; i++ {
		b := data[i/TritsPerByte]
		if b >= 243 {
			return nil, ErrPackRange
		}
		v := int(b)
		for j := 0; j < i%TritsPerByte; j++ {
			v /= 3
		}
		out = append(out, Trit(v%3-1))
	}
	return out, nil
}
