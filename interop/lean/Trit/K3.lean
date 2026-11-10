namespace TritFormal

inductive K3 where
  | F
  | U
  | T
  deriving DecidableEq, Repr

namespace K3

def not : K3 → K3
  | F => T
  | U => U
  | T => F

def and : K3 → K3 → K3
  | F, _ => F
  | _, F => F
  | T, T => T
  | _, _ => U

def or : K3 → K3 → K3
  | T, _ => T
  | _, T => T
  | F, F => F
  | _, _ => U

theorem not_not (x : K3) : not (not x) = x := by
  cases x <;> rfl

theorem and_comm (a b : K3) : and a b = and b a := by
  cases a <;> cases b <;> rfl

theorem or_comm (a b : K3) : or a b = or b a := by
  cases a <;> cases b <;> rfl

theorem and_assoc (a b c : K3) : and (and a b) c = and a (and b c) := by
  cases a <;> cases b <;> cases c <;> rfl

theorem or_assoc (a b c : K3) : or (or a b) c = or a (or b c) := by
  cases a <;> cases b <;> cases c <;> rfl

theorem and_false (x : K3) : and F x = F := by
  cases x <;> rfl

theorem or_true (x : K3) : or T x = T := by
  cases x <;> rfl

/-! Bit-plane encoding from `interop/spec/v0.md`: F=(0,0), T=(1,0), U=(0,1); (1,1) invalid. -/

def toBits : K3 → Bool × Bool
  | F => (false, false)
  | T => (true, false)
  | U => (false, true)

def ofBits : Bool × Bool → Option K3
  | (false, false) => some F
  | (true, false) => some T
  | (false, true) => some U
  | (true, true) => none

theorem ofBits_toBits (x : K3) : ofBits (toBits x) = some x := by
  cases x <;> rfl

theorem ofBits_invalid : ofBits (true, true) = none := rfl

def falseBit (x : K3) : Bool :=
  !(toBits x).1 && !(toBits x).2

def andBits (a b : K3) : Bool × Bool :=
  ( (toBits a).1 && (toBits b).1,
    !(((toBits a).1 && (toBits b).1) || falseBit a || falseBit b) )

def orBits (a b : K3) : Bool × Bool :=
  ( (toBits a).1 || (toBits b).1,
    !(((toBits a).1 || (toBits b).1) || (falseBit a && falseBit b)) )

def notBits (a : K3) : Bool × Bool :=
  (falseBit a, (toBits a).2)

theorem andBits_correct (a b : K3) : ofBits (andBits a b) = some (and a b) := by
  cases a <;> cases b <;> rfl

theorem orBits_correct (a b : K3) : ofBits (orBits a b) = some (or a b) := by
  cases a <;> cases b <;> rfl

theorem notBits_correct (a : K3) : ofBits (notBits a) = some (not a) := by
  cases a <;> rfl

/-! `KGate` from the spec: required=F -> T, required=U -> U, required=T -> verified. -/

def gate (required verified : K3) : K3 :=
  match required with
  | F => T
  | U => U
  | T => verified

def implies (a b : K3) : K3 :=
  or (not a) b

theorem gate_F (v : K3) : gate F v = T := by
  cases v <;> rfl

theorem gate_U (v : K3) : gate U v = U := by
  cases v <;> rfl

theorem gate_T (v : K3) : gate T v = v := by
  cases v <;> rfl

theorem gate_unknown_true : gate U T = U := rfl

theorem gate_not_implies : gate U T ≠ implies U T := by
  decide

end K3

end TritFormal
