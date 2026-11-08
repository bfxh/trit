import Lean

namespace TritFormal

inductive PackTrit where
  | neg
  | zero
  | pos
  deriving DecidableEq, Repr

namespace Pack5

def digit : PackTrit → Nat
  | .neg => 0
  | .zero => 1
  | .pos => 2

def ofDigit (n : Nat) : PackTrit :=
  if n = 0 then .neg else if n = 1 then .zero else .pos

def pack (a b c d e : PackTrit) : Nat :=
  digit a + 3 * digit b + 9 * digit c + 27 * digit d + 81 * digit e

def unpack (n : Nat) : PackTrit × PackTrit × PackTrit × PackTrit × PackTrit :=
  ( ofDigit (n % 3),
    ofDigit ((n / 3) % 3),
    ofDigit ((n / 9) % 3),
    ofDigit ((n / 27) % 3),
    ofDigit ((n / 81) % 3) )

theorem unpack_pack (a b c d e : PackTrit) :
    unpack (pack a b c d e) = (a, b, c, d, e) := by
  cases a <;> cases b <;> cases c <;> cases d <;> cases e <;> native_decide

theorem pack_lt_243 (a b c d e : PackTrit) : pack a b c d e < 243 := by
  cases a <;> cases b <;> cases c <;> cases d <;> cases e <;> native_decide

end Pack5

end TritFormal
