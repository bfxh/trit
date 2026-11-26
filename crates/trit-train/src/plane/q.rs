//! `PlaneMatrix` 的惰性 `q` 展开：首次调用 `q()` 时从双导轨解码并缓存。
use super::{PlaneMatrix, TRITS_PER_WORD};
impl PlaneMatrix {
    /// 展开后的三值权重（行主序）；首次调用时从双导轨解码并缓存。
    pub fn q(&self) -> &[i8] {
        self.q.get_or_init(|| {
            let mut out = vec![0i8; self.rows * self.cols];
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let word = r * self.bpr + c / TRITS_PER_WORD;
                    let bit = 1u64 << (c % TRITS_PER_WORD);
                    if self.pos[word] & bit != 0 {
                        out[r * self.cols + c] = 1;
                    } else if self.neg[word] & bit != 0 {
                        out[r * self.cols + c] = -1;
                    }
                }
            }
            out
        })
    }
}
#[cfg(test)]
mod tests;
