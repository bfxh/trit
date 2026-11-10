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
  cases a <;> cases b <;> cases c <;> cases d <;> cases e <;> decide

theorem pack_lt_243 (a b c d e : PackTrit) : pack a b c d e < 243 := by
  cases a <;> cases b <;> cases c <;> cases d <;> cases e <;> decide

/-! Tail-inclusive group API: missing positions are padded with `zero`. -/

inductive PackError where
  | invalidByte
  | shortInput
  | invalidTrit
  deriving DecidableEq, Repr

def packGroup (ts : List PackTrit) : Nat :=
  digit (ts.getD 0 .zero) +
  3 * digit (ts.getD 1 .zero) +
  9 * digit (ts.getD 2 .zero) +
  27 * digit (ts.getD 3 .zero) +
  81 * digit (ts.getD 4 .zero)

def unpackGroup (n count : Nat) : Except PackError (List PackTrit) :=
  if 243 ≤ n then
    .error .invalidByte
  else if 5 < count then
    .error .shortInput
  else
    .ok ((List.range count).map fun i => ofDigit ((n / Nat.pow 3 i) % 3))

theorem unpackGroup_packGroup1 (a : PackTrit) :
    unpackGroup (packGroup [a]) 1 = .ok [a] := by
  cases a <;> rfl

theorem unpackGroup_packGroup2 (a b : PackTrit) :
    unpackGroup (packGroup [a, b]) 2 = .ok [a, b] := by
  cases a <;> cases b <;> rfl

theorem unpackGroup_packGroup3 (a b c : PackTrit) :
    unpackGroup (packGroup [a, b, c]) 3 = .ok [a, b, c] := by
  cases a <;> cases b <;> cases c <;> rfl

theorem unpackGroup_packGroup4 (a b c d : PackTrit) :
    unpackGroup (packGroup [a, b, c, d]) 4 = .ok [a, b, c, d] := by
  cases a <;> cases b <;> cases c <;> cases d <;> rfl

theorem unpackGroup_packGroup5 (a b c d e : PackTrit) :
    unpackGroup (packGroup [a, b, c, d, e]) 5 = .ok [a, b, c, d, e] := by
  cases a <;> cases b <;> cases c <;> cases d <;> cases e <;> rfl

theorem unpackGroup_invalidByte (n count : Nat) (h : 243 ≤ n) :
    unpackGroup n count = .error .invalidByte := by
  simp [unpackGroup, h]

theorem unpackGroup_shortInput (n count : Nat) (hn : n < 243) (hc : 5 < count) :
    unpackGroup n count = .error .shortInput := by
  simp [unpackGroup, Nat.not_le.mpr hn, hc]

def ofInt (n : Int) : Except PackError PackTrit :=
  if n = -1 then .ok .neg
  else if n = 0 then .ok .zero
  else if n = 1 then .ok .pos
  else .error .invalidTrit

theorem ofInt_neg_one : ofInt (-1) = .ok .neg := by
  rfl

theorem ofInt_two : ofInt 2 = .error .invalidTrit := by
  rfl

end Pack5

end TritFormal
