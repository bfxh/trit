package ternary

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

type vectorFile struct {
	SpecVersion string   `json:"spec_version"`
	Profile     string   `json:"profile"`
	Encoding    string   `json:"encoding"`
	Vectors     []vector `json:"vectors"`
}

type vector struct {
	ID          string          `json:"id"`
	Op          string          `json:"op"`
	Input       json.RawMessage `json:"input"`
	Expected    json.RawMessage `json:"expected"`
	TritCount   int             `json:"trit_count"`
	ExpectError string          `json:"expect_error"`
}

func loadVectorFile(t *testing.T, name string) vectorFile {
	t.Helper()
	path := filepath.Join("..", "..", "interop", "vectors", name)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	var f vectorFile
	if err := json.Unmarshal(data, &f); err != nil {
		t.Fatalf("parse %s: %v", path, err)
	}
	if f.SpecVersion != "0.1.0" {
		t.Fatalf("%s: spec_version = %q", path, f.SpecVersion)
	}
	return f
}

func mustUnmarshal(t *testing.T, raw json.RawMessage, dst any) {
	t.Helper()
	if err := json.Unmarshal(raw, dst); err != nil {
		t.Fatalf("unmarshal %s: %v", raw, err)
	}
}

func parseKleene(s string) (Kleene, bool) {
	switch s {
	case "F":
		return KFalse, true
	case "U":
		return KUnknown, true
	case "T":
		return KTrue, true
	default:
		return KUnknown, false
	}
}

func kleeneString(k Kleene) string {
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

func TestK3Conformance(t *testing.T) {
	f := loadVectorFile(t, "k3.json")
	for _, v := range f.Vectors {
		v := v
		t.Run(v.ID, func(t *testing.T) {
			var got Kleene
			switch v.Op {
			case "not":
				var in string
				mustUnmarshal(t, v.Input, &in)
				a, ok := parseKleene(in)
				if !ok {
					t.Fatalf("bad input %q", in)
				}
				got = a.Not()
			case "and", "or":
				var in []string
				mustUnmarshal(t, v.Input, &in)
				if len(in) != 2 {
					t.Fatalf("input length %d", len(in))
				}
				a, okA := parseKleene(in[0])
				b, okB := parseKleene(in[1])
				if !okA || !okB {
					t.Fatalf("bad input %v", in)
				}
				if v.Op == "and" {
					got = KAnd(a, b)
				} else {
					got = KOr(a, b)
				}
			default:
				t.Fatalf("unknown op %q", v.Op)
			}
			var want string
			mustUnmarshal(t, v.Expected, &want)
			if kleeneString(got) != want {
				t.Fatalf("got %s, want %s", kleeneString(got), want)
			}
		})
	}
}

func TestKGateConformance(t *testing.T) {
	f := loadVectorFile(t, "kgate.json")
	for _, v := range f.Vectors {
		v := v
		t.Run(v.ID, func(t *testing.T) {
			if v.Op != "gate" {
				t.Fatalf("unknown op %q", v.Op)
			}
			var in []string
			mustUnmarshal(t, v.Input, &in)
			if len(in) != 2 {
				t.Fatalf("input length %d", len(in))
			}
			required, okR := parseKleene(in[0])
			verified, okV := parseKleene(in[1])
			if !okR || !okV {
				t.Fatalf("bad input %v", in)
			}
			var want string
			mustUnmarshal(t, v.Expected, &want)
			if got := kleeneString(KGate(required, verified)); got != want {
				t.Fatalf("got %s, want %s", got, want)
			}
		})
	}
}

func decodeTrits(t *testing.T, raw json.RawMessage) []Trit {
	t.Helper()
	var in []int
	mustUnmarshal(t, raw, &in)
	out := make([]Trit, len(in))
	for i, n := range in {
		out[i] = Trit(n)
	}
	return out
}

func decodeByteInput(t *testing.T, raw json.RawMessage) []byte {
	t.Helper()
	var single int
	if err := json.Unmarshal(raw, &single); err == nil {
		return []byte{byte(single)}
	}
	var many []int
	mustUnmarshal(t, raw, &many)
	out := make([]byte, len(many))
	for i, n := range many {
		out[i] = byte(n)
	}
	return out
}

func TestPack5Conformance(t *testing.T) {
	f := loadVectorFile(t, "pack5.json")
	for _, v := range f.Vectors {
		v := v
		t.Run(v.ID, func(t *testing.T) {
			switch v.Op {
			case "pack":
				trits := decodeTrits(t, v.Input)
				got, err := Pack5(trits)
				switch v.ExpectError {
				case "":
					if err != nil {
						t.Fatalf("unexpected error: %v", err)
					}
					var want int
					mustUnmarshal(t, v.Expected, &want)
					if len(got) != 1 || int(got[0]) != want {
						t.Fatalf("got %v, want %d", got, want)
					}
				case "invalid_trit":
					if !errors.Is(err, ErrTrit) {
						t.Fatalf("got error %v, want ErrTrit", err)
					}
				default:
					t.Fatalf("unknown expected error %q", v.ExpectError)
				}
			case "unpack":
				bytes := decodeByteInput(t, v.Input)
				got, err := Unpack5(bytes, v.TritCount)
				switch v.ExpectError {
				case "":
					if err != nil {
						t.Fatalf("unexpected error: %v", err)
					}
					var want []int
					mustUnmarshal(t, v.Expected, &want)
					if len(got) != len(want) {
						t.Fatalf("got len %d, want %d", len(got), len(want))
					}
					for i := range want {
						if int(got[i]) != want[i] {
							t.Fatalf("at %d: got %d, want %d", i, got[i], want[i])
						}
					}
				case "invalid_byte":
					if !errors.Is(err, ErrPackRange) {
						t.Fatalf("got error %v, want ErrPackRange", err)
					}
				case "short_input":
					if !errors.Is(err, ErrShortData) {
						t.Fatalf("got error %v, want ErrShortData", err)
					}
				default:
					t.Fatalf("unknown expected error %q", v.ExpectError)
				}
			default:
				t.Fatalf("unknown op %q", v.Op)
			}
		})
	}
}
