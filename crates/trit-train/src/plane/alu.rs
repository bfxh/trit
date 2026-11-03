//! 位平面的按字 ALU：`not` / `and` / `or` / `xor`，以及它们共用的 `combine` 与尾部掩码。
//!
//! 从 `plane.rs` 搬出来只为守住索引文件的行数预算——语义与公开路径都不变：
//! 同一个 crate 里 `impl Planes` 可以分布在多个模块，而 `plane::alu` 是 `plane` 的子模块，
//! 所以结构体的私有字段在这里照样可见。

use trit_core::Algebra;

use super::{Planes, TRITS_PER_WORD};

impl Planes {
    /// 逐位取反：交换两条导轨（`¬`）。
    pub fn not(&self) -> Self {
        Self {
            len: self.len,
            pos: self.neg.clone(),
            neg: self.pos.clone(),
        }
    }

    /// 逻辑与（`∧`），语义按 `alg`。
    pub fn and(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            // Kleene 与 Łukasiewicz 的 `∧` 都是格序 min（X∧X = X）。
            Algebra::Kleene | Algebra::Lukasiewicz => (ap & bp, an | bn),
            // Bochvar：X 是毒值。
            Algebra::Bochvar => {
                let pos = ap & bp;
                (pos, !(pos | ax | bx))
            }
        })
    }

    /// 逻辑或（`∨`），语义按 `alg`。
    pub fn or(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            Algebra::Kleene | Algebra::Lukasiewicz => (ap | bp, an & bn),
            Algebra::Bochvar => {
                let neg = an & bn;
                (!(neg | ax | bx), neg)
            }
        })
    }

    /// 强加法（`⊕`），语义按 `alg`。
    pub fn xor(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            // Kleene / Bochvar：X 传播，两个 +1 才是 +1，否则 -1。
            Algebra::Kleene | Algebra::Bochvar => {
                let pos = ap & bp;
                (pos, !(pos | ax | bx))
            }
            // Łukasiewicz：min(1, a+b)，XX 与 TF 都为 +1。
            Algebra::Lukasiewicz => {
                let neg = an & bn;
                (!((an & (bn | bx)) | (ax & bn)), neg)
            }
        })
    }

    /// 按字组合两个向量；闭包拿到 `(ap, an, bp, bn, ax, bx)`，返回 `(pos, neg)`。
    fn combine(
        &self,
        other: &Self,
        f: impl Fn(u64, u64, u64, u64, u64, u64) -> (u64, u64),
    ) -> Self {
        assert_eq!(self.len, other.len, "位平面长度不一致");
        let mut pos = vec![0u64; self.words()];
        let mut neg = vec![0u64; self.words()];
        for (w, (p, n)) in pos.iter_mut().zip(neg.iter_mut()).enumerate() {
            let valid = self.valid_mask(w);
            let (ap, an) = (self.pos[w], self.neg[w]);
            let (bp, bn) = (other.pos[w], other.neg[w]);
            let ax = !(ap | an) & valid;
            let bx = !(bp | bn) & valid;
            let (out_pos, out_neg) = f(ap, an, bp, bn, ax, bx);
            *p = out_pos & valid;
            *n = out_neg & valid;
        }
        Self {
            len: self.len,
            pos,
            neg,
        }
    }

    /// 最后一字的有效位掩码（长度外的位必须保持 0）。
    fn valid_mask(&self, word: usize) -> u64 {
        let tail = self.len % TRITS_PER_WORD;
        if word + 1 < self.words() || tail == 0 {
            u64::MAX
        } else {
            (1u64 << tail) - 1
        }
    }
}
