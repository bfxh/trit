//! 权重空间投影：用三值基把一层压到低维。
//!
//! 这是 `examples/dimsweep` 那条实验线的库化：那边证明了「投影到判据方向」是唯一有效的
//! 压缩方式（`k=1` 对线性判据全过、`k=64` 对 STS 排序保住 88.7%）。这里把同一件事做成
//! 一等权重空间操作——基是一层三值权重，可以被 `patch` / `repair` / `certify` 直接处理。
//!
//! 语义：`out[r][i] = sign(Σ_j basis[i][j] · self[r][j])`，`bias` 逐行保留。
//! 输入维度从 `cols` 降到 `basis.rows()`；输出维度（行数）不变。

use crate::error::{Error, Result};
use crate::net::Layer;

impl Layer {
    /// 用三值基 `basis`（`k × cols`）投影本层，返回 `rows × k` 的新层。
    ///
    /// 基的列数必须与本层列数一致；`bias` 不动（它作用在输出行上，与输入维度无关）。
    pub fn project(&self, basis: &Layer) -> Result<Self> {
        if basis.cols() != self.cols() {
            return Err(Error::Shape {
                what: "project basis cols",
                expected: self.cols(),
                got: basis.cols(),
            });
        }
        let (rows, cols) = (self.rows(), self.cols());
        let k = basis.rows();
        let mut trits = vec![0i8; rows * k];
        for (r, chunk) in trits.chunks_mut(k).enumerate() {
            for (i, slot) in chunk.iter_mut().enumerate() {
                let mut acc: i32 = 0;
                for j in 0..cols {
                    acc += i32::from(basis.trit(i, j).expect("basis trit").to_i8())
                        * i32::from(self.trit(r, j).expect("layer trit").to_i8());
                }
                *slot = acc.signum() as i8;
            }
        }
        let bias: Vec<i8> = self.bias().iter().map(|t| t.to_i8()).collect();
        Self::new(rows, k, &trits, &bias)
    }
}
