import Trit.K3

/-!
# `TriVec`：位切片三值向量的语义模型

主库 `crates/trit-core/src/bulk.rs` 的 `TriVec` 用两个位平面
`(ones, unknowns)` 表示三值向量：`(0,0)=F`、`(1,0)=T`、`(0,1)=U`，
`(1,1)` 在合法状态（`wf`）下不出现。这里把位平面抽象成
`Nat → Bool`（第 `i` 位就是代码里的 `(word i/64 >> i%64) & 1`），
只关心语义，不建模 64 位字的打包细节——打包层的细化由 Verus 锚点
（`verus/src/bulk.rs`）负责，两边证的是同一句话。

本文件证三类东西：

1. **单点操作**：`fromTernary` / `get` / `set` 的视图律与 `wf` 维持，
   对应主库 `from_ternary` / `try_get` / `try_set`；
2. **位平面公式 ≡ 标量 **：`andv` / `orv` / `notv` 的视图逐位等于
   `K3.and` / `K3.or` / `K3.not`（公式与 `bulk.rs` 的 `o_out/f_out` 逐字同形）；
3. **B3 毒值**：`andvB3` / `orvB3` 的视图逐位等于 Bochvar 运算。
-/

namespace TritFormal

open K3

/-- 位平面：位索引 → 位值。 -/
abbrev Plane := Nat → Bool

/-- 位切片三值向量：长度 + 两个位平面。 -/
structure TVec where
  /-- 元素个数（`len` 位）。 -/
  len : Nat
  /-- `True` 平面（1 = T）。 -/
  ones : Plane
  /-- `Unknown` 平面（1 = U）。 -/
  unk : Plane

/-- 第 `i` 位的 `(ones, unknowns)` 对。 -/
def bits (v : TVec) (i : Nat) : Bool × Bool := (v.ones i, v.unk i)

/-- `(1,1)` 非法时按主库 `try_get` 的 `_ => F` 分支取 `F`。 -/
def ofBitsView (p : Bool × Bool) : K3 :=
  match K3.ofBits p with
  | some t => t
  | none => K3.F

/-- 第 `i` 个三值（主库 `TriVec::get` 的语义）。 -/
def view (v : TVec) (i : Nat) : K3 := ofBitsView (bits v i)

/-- 合法状态：`len` 位内两个平面互斥（主库 `from_ternary`/`try_set` 维持）。 -/
def wf (v : TVec) : Prop := ∀ i, i < v.len → ¬ (v.ones i && v.unk i)

/-- `T` 判定（主库 `set` 里的 `Ternary::True` 分支）。 -/
def isT : K3 → Bool
  | K3.T => true
  | _ => false

/-- `U` 判定。 -/
def isU : K3 → Bool
  | K3.U => true
  | _ => false

theorem not_both_isT_isU (t : K3) : ¬ (isT t && isU t) := by
  cases t <;> simp [isT, isU]

theorem ofBitsView_encode (t : K3) : ofBitsView (isT t, isU t) = t := by
  cases t <;> rfl

theorem ofBitsView_toBits (t : K3) : ofBitsView (K3.toBits t) = t := by
  cases t <;> rfl

theorem ofBitsView_eq {p : Bool × Bool} {t : K3} (h : K3.ofBits p = some t) :
    ofBitsView p = t := by
  simp [ofBitsView, h]

theorem toBits_ofBits {p : Bool × Bool} (h : ¬ (p.1 && p.2)) :
    K3.toBits (ofBitsView p) = p := by
  rcases p with ⟨o, u⟩
  cases o <;> cases u <;> simp_all [ofBitsView, K3.ofBits, K3.toBits]

/-- `wf` 下，第 `i` 位的原始位对就是该元素编码：`bits = toBits (view)`。 -/
theorem bits_of_wf {v : TVec} (h : wf v) {i : Nat} (hi : i < v.len) :
    bits v i = K3.toBits (view v i) := by
  exact (toBits_ofBits (h i hi)).symm

/-! ## 单点操作：`fromTernary` / `get` / `set` -/

/-- 从标量三值列表打包（主库 `from_ternary`）。 -/
def fromTernary (xs : List K3) : TVec where
  len := xs.length
  ones := fun i => match xs[i]? with
    | some t => isT t
    | none => false
  unk := fun i => match xs[i]? with
    | some t => isU t
    | none => false

theorem fromTernary_wf (xs : List K3) : wf (fromTernary xs) := by
  intro i hi
  have hget : xs[i]? = some xs[i] := List.getElem?_eq_getElem hi
  simpa [fromTernary, hget] using not_both_isT_isU (xs[i])

theorem fromTernary_view (xs : List K3) {i : Nat} (hi : i < xs.length) :
    view (fromTernary xs) i = xs[i] := by
  have hget : xs[i]? = some xs[i] := List.getElem?_eq_getElem hi
  show ofBitsView (bits (fromTernary xs) i) = xs[i]
  rw [show bits (fromTernary xs) i = (isT xs[i], isU xs[i]) by
    simp [bits, fromTernary, hget]]
  exact ofBitsView_encode xs[i]

/-- 写第 `i` 位（主库 `try_set` 的位运算：先清两平面，再按值置位）。 -/
def set (v : TVec) (i : Nat) (t : K3) : TVec where
  len := v.len
  ones := fun j => if j = i then isT t else v.ones j
  unk := fun j => if j = i then isU t else v.unk j

theorem set_len (v : TVec) (i : Nat) (t : K3) : (set v i t).len = v.len := rfl

theorem set_bits_self (v : TVec) (i : Nat) (t : K3) :
    bits (set v i t) i = K3.toBits t := by
  simp [bits, set, isT, isU]
  cases t <;> rfl

theorem set_view_self (v : TVec) (i : Nat) (t : K3) (_hi : i < v.len) :
    view (set v i t) i = t := by
  show ofBitsView (bits (set v i t) i) = t
  rw [set_bits_self, ofBitsView_toBits]

theorem set_view_other (v : TVec) (i j : Nat) (t : K3) (h : j ≠ i) :
    view (set v i t) j = view v j := by
  have hji : ¬ (j = i) := h
  simp [view, bits, set, hji]

theorem set_wf {v : TVec} (h : wf v) {i : Nat} (hi : i < v.len) (t : K3) :
    wf (set v i t) := by
  intro j hj
  by_cases hji : j = i
  · subst hji
    simpa [set] using not_both_isT_isU t
  · have hne : j ≠ i := hji
    have hv := h j hj
    simpa [set, hne] using hv

theorem set_view_fromTernary (xs : List K3) {i : Nat} (hi : i < xs.length) (t : K3) :
    view (set (fromTernary xs) i t) i = t := by
  exact set_view_self (fromTernary xs) i t hi

theorem set_view_fromTernary_other (xs : List K3) {i j : Nat} (_hi : i < xs.length)
    (h : j ≠ i) (hj : j < xs.length) (t : K3) :
    view (set (fromTernary xs) i t) j = xs[j] := by
  rw [set_view_other _ _ _ _ h]
  exact fromTernary_view xs hj

theorem set_view_fromTernary_out (xs : List K3) {i j : Nat} (_hi : i < xs.length)
    (h : j ≠ i) (hj : ¬ j < xs.length) (t : K3) :
    view (set (fromTernary xs) i t) j = K3.F := by
  rw [set_view_other _ _ _ _ h]
  have hnone : xs[j]? = none := List.getElem?_eq_none (Nat.le_of_not_lt hj)
  simp [view, bits, fromTernary, ofBitsView, hnone, K3.ofBits]

/-- 带边界检查的读（主库 `try_get`）。 -/
def get? (v : TVec) (i : Nat) : Option K3 :=
  if i < v.len then some (view v i) else none

/-- 带边界检查的写（主库 `try_set`）。 -/
def set? (v : TVec) (i : Nat) (t : K3) : Option TVec :=
  if i < v.len then some (set v i t) else none

theorem get?_set?_same {v : TVec} {i : Nat} (hi : i < v.len) (t : K3) :
    get? (set v i t) i = some t := by
  simp [get?, set_len, hi, set_view_self v i t hi]

theorem get?_set?_other {v : TVec} {i j : Nat} (_hi : i < v.len) (h : j ≠ i) (t : K3) :
    get? (set v i t) j = get? v j := by
  by_cases hj : j < v.len
  · simp [get?, set_len, hj, set_view_other v i j t h]
  · have hjn : ¬ j < v.len := hj
    simp [get?, set_len, hjn]

theorem set?_out_of_range {v : TVec} {i : Nat} (t : K3) (h : ¬ i < v.len) :
    set? v i t = none := by
  simp [set?, h]

/-! ## 位平面公式 ≡ 标量：K3（Kleene/Ł3 格序） -/

theorem andBits_valid (a b : K3) :
    ¬ ((K3.andBits a b).1 && (K3.andBits a b).2) := by
  cases a <;> cases b <;> simp [K3.andBits, K3.falseBit, K3.toBits]

theorem orBits_valid (a b : K3) :
    ¬ ((K3.orBits a b).1 && (K3.orBits a b).2) := by
  cases a <;> cases b <;> simp [K3.orBits, K3.falseBit, K3.toBits]

theorem notBits_valid (a : K3) : ¬ ((K3.notBits a).1 && (K3.notBits a).2) := by
  cases a <;> simp [K3.notBits, K3.falseBit, K3.toBits]

/-- `falseBit` 与主库 `!(ones | unknowns)` 同值（逐元素形式）。 -/
theorem falseBit_eq_or (t : K3) :
    K3.falseBit t = !((K3.toBits t).1 || (K3.toBits t).2) := by
  cases t <;> rfl

/-- 位平面 `and`（与 `bulk.rs` 的 `o_out = o1 & o2`、`f_out = (...) | (...)` 同形）。 -/
def andv (a b : TVec) : TVec where
  len := a.len
  ones := fun i => (K3.andBits (view a i) (view b i)).1
  unk := fun i => (K3.andBits (view a i) (view b i)).2

theorem andv_view (a b : TVec) (i : Nat) :
    view (andv a b) i = K3.and (view a i) (view b i) := by
  show ofBitsView (bits (andv a b) i) = K3.and (view a i) (view b i)
  rw [show bits (andv a b) i = K3.andBits (view a i) (view b i) from rfl]
  exact ofBitsView_eq (K3.andBits_correct (view a i) (view b i))

theorem andv_wf (_ha : wf a) (_hb : wf b) : wf (andv a b) := by
  intro i _
  exact andBits_valid (view a i) (view b i)

/-- 位平面 `or`。 -/
def orv (a b : TVec) : TVec where
  len := a.len
  ones := fun i => (K3.orBits (view a i) (view b i)).1
  unk := fun i => (K3.orBits (view a i) (view b i)).2

theorem orv_view (a b : TVec) (i : Nat) :
    view (orv a b) i = K3.or (view a i) (view b i) := by
  show ofBitsView (bits (orv a b) i) = K3.or (view a i) (view b i)
  rw [show bits (orv a b) i = K3.orBits (view a i) (view b i) from rfl]
  exact ofBitsView_eq (K3.orBits_correct (view a i) (view b i))

theorem orv_wf (_ha : wf a) (_hb : wf b) : wf (orv a b) := by
  intro i _
  exact orBits_valid (view a i) (view b i)

/-- 位平面取反。 -/
def notv (v : TVec) : TVec where
  len := v.len
  ones := fun i => (K3.notBits (view v i)).1
  unk := fun i => (K3.notBits (view v i)).2

theorem notv_view (v : TVec) (i : Nat) : view (notv v) i = K3.not (view v i) := by
  show ofBitsView (bits (notv v) i) = K3.not (view v i)
  rw [show bits (notv v) i = K3.notBits (view v i) from rfl]
  exact ofBitsView_eq (K3.notBits_correct (view v i))

theorem notv_wf (_h : wf v) : wf (notv v) := by
  intro i _
  exact notBits_valid (view v i)

/-- 取反后 `unknowns` 平面不变（主库 `not` 不改 unknowns）。 -/
theorem notv_unk_eq {v : TVec} (h : wf v) {i : Nat} (hi : i < v.len) :
    (notv v).unk i = v.unk i := by
  have hb := bits_of_wf h hi
  have hu : (K3.toBits (view v i)).2 = v.unk i := (congrArg Prod.snd hb).symm
  have h1 : (notv v).unk i = (K3.toBits (view v i)).2 := rfl
  rw [h1, hu]

/-- 取反的 `ones` 平面在 `wf` 下等于主库的 `!(ones | unknowns)`。 -/
theorem notv_ones_eq_code {v : TVec} (h : wf v) {i : Nat} (hi : i < v.len) :
    (notv v).ones i = !(v.ones i || v.unk i) := by
  have hb := bits_of_wf h hi
  have ho : (K3.toBits (view v i)).1 = v.ones i := (congrArg Prod.fst hb).symm
  have hu : (K3.toBits (view v i)).2 = v.unk i := (congrArg Prod.snd hb).symm
  have h1 : (notv v).ones i = K3.falseBit (view v i) := rfl
  rw [h1, falseBit_eq_or, ho, hu]

/-! ## 位平面公式 ≡ 标量：B3（Bochvar 毒值） -/

/-- Bochvar 与：任一 `U` → `U`，否则 `T ∧ T`。 -/
def poisonAnd : K3 → K3 → K3
  | K3.U, _ => K3.U
  | _, K3.U => K3.U
  | K3.T, K3.T => K3.T
  | _, _ => K3.F

/-- Bochvar 或。 -/
def poisonOr : K3 → K3 → K3
  | K3.U, _ => K3.U
  | _, K3.U => K3.U
  | K3.T, _ => K3.T
  | _, K3.T => K3.T
  | _, _ => K3.F

/-- B3 位平面 `and`：`x_out = x1 | x2`、`ones = (o1 & o2) & !x_out`（与 `bulk.rs` 同形）。 -/
def andvB3 (a b : TVec) : TVec where
  len := a.len
  ones := fun i => (a.ones i && b.ones i) && !(a.unk i || b.unk i)
  unk := fun i => a.unk i || b.unk i

theorem andvB3_view (a b : TVec) (ha : wf a) (hb : wf b) {i : Nat}
    (hia : i < a.len) (hib : i < b.len) :
    view (andvB3 a b) i = poisonAnd (view a i) (view b i) := by
  have hoa : a.ones i = (K3.toBits (view a i)).1 := congrArg Prod.fst (bits_of_wf ha hia)
  have hua : a.unk i = (K3.toBits (view a i)).2 := congrArg Prod.snd (bits_of_wf ha hia)
  have hob : b.ones i = (K3.toBits (view b i)).1 := congrArg Prod.fst (bits_of_wf hb hib)
  have hub : b.unk i = (K3.toBits (view b i)).2 := congrArg Prod.snd (bits_of_wf hb hib)
  show ofBitsView (bits (andvB3 a b) i) = poisonAnd (view a i) (view b i)
  rw [show bits (andvB3 a b) i
      = (((a.ones i && b.ones i) && !(a.unk i || b.unk i)), a.unk i || b.unk i) from rfl,
    hoa, hua, hob, hub]
  cases view a i <;> cases view b i <;> simp [poisonAnd, K3.ofBits, K3.toBits, ofBitsView]

theorem andvB3_wf (_ha : wf a) (_hb : wf b) : wf (andvB3 a b) := by
  intro i _
  simp [andvB3]

/-- B3 位平面 `or`。 -/
def orvB3 (a b : TVec) : TVec where
  len := a.len
  ones := fun i => (a.ones i || b.ones i) && !(a.unk i || b.unk i)
  unk := fun i => a.unk i || b.unk i

theorem orvB3_view (a b : TVec) (ha : wf a) (hb : wf b) {i : Nat}
    (hia : i < a.len) (hib : i < b.len) :
    view (orvB3 a b) i = poisonOr (view a i) (view b i) := by
  have hoa : a.ones i = (K3.toBits (view a i)).1 := congrArg Prod.fst (bits_of_wf ha hia)
  have hua : a.unk i = (K3.toBits (view a i)).2 := congrArg Prod.snd (bits_of_wf ha hia)
  have hob : b.ones i = (K3.toBits (view b i)).1 := congrArg Prod.fst (bits_of_wf hb hib)
  have hub : b.unk i = (K3.toBits (view b i)).2 := congrArg Prod.snd (bits_of_wf hb hib)
  show ofBitsView (bits (orvB3 a b) i) = poisonOr (view a i) (view b i)
  rw [show bits (orvB3 a b) i
      = (((a.ones i || b.ones i) && !(a.unk i || b.unk i)), a.unk i || b.unk i) from rfl,
    hoa, hua, hob, hub]
  cases view a i <;> cases view b i <;> simp [poisonOr, K3.ofBits, K3.toBits, ofBitsView]

theorem orvB3_wf (_ha : wf a) (_hb : wf b) : wf (orvB3 a b) := by
  intro i _
  simp [orvB3]

end TritFormal
