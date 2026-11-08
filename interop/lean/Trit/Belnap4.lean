namespace TritFormal

inductive B4 where
  | N
  | T
  | F
  | B
  deriving DecidableEq, Repr

namespace B4

def not : B4 → B4
  | N => N
  | T => F
  | F => T
  | B => B

def and : B4 → B4 → B4
  | N, N => N
  | N, T => N
  | N, F => F
  | N, B => F
  | T, N => N
  | T, T => T
  | T, F => F
  | T, B => B
  | F, _ => F
  | B, N => F
  | B, T => B
  | B, F => F
  | B, B => B

def or : B4 → B4 → B4
  | N, N => N
  | N, T => T
  | N, F => N
  | N, B => T
  | T, _ => T
  | F, N => N
  | F, T => T
  | F, F => F
  | F, B => B
  | B, N => T
  | B, T => T
  | B, F => B
  | B, B => B

theorem not_involutive (x : B4) : not (not x) = x := by
  cases x <;> rfl

theorem and_comm (a b : B4) : and a b = and b a := by
  cases a <;> cases b <;> rfl

theorem or_comm (a b : B4) : or a b = or b a := by
  cases a <;> cases b <;> rfl

theorem not_B : not B = B := rfl

theorem and_N_B : and N B = F := rfl

theorem or_N_B : or N B = T := rfl

theorem and_B_B : and B B = B := rfl

theorem or_B_B : or B B = B := rfl

/-! Bit-plane encoding: N=(0,0), T=(1,0), F=(0,1), B=(1,1). Unlike K3, (1,1) is valid. -/

def toBits : B4 → Bool × Bool
  | N => (false, false)
  | T => (true, false)
  | F => (false, true)
  | B => (true, true)

def ofBits : Bool × Bool → B4
  | (false, false) => N
  | (true, false) => T
  | (false, true) => F
  | (true, true) => B

theorem ofBits_toBits (x : B4) : ofBits (toBits x) = x := by
  cases x <;> rfl

def andBits (a b : Bool × Bool) : Bool × Bool :=
  (a.1 && b.1, a.2 || b.2)

def orBits (a b : Bool × Bool) : Bool × Bool :=
  (a.1 || b.1, a.2 && b.2)

def notBits (a : Bool × Bool) : Bool × Bool :=
  (a.2, a.1)

theorem andBits_correct (a b : B4) :
    ofBits (andBits (toBits a) (toBits b)) = and a b := by
  cases a <;> cases b <;> rfl

theorem orBits_correct (a b : B4) :
    ofBits (orBits (toBits a) (toBits b)) = or a b := by
  cases a <;> cases b <;> rfl

theorem notBits_correct (a : B4) : ofBits (notBits (toBits a)) = not a := by
  cases a <;> rfl

end B4

end TritFormal
