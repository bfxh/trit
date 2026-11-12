//! 网络与层：权重本身就是工件，前向只做整数内积 + 符号判决。
//!
//! 语义约定（本 crate 的 M0 边界）：
//!
//! - 每层 `y[r] = sign(scale[r] · (Σ_c w[r,c]·x[c] + b[r]))`；
//! - 激活是**符号**，所以 `scale` 只要为正就**不影响行为**（见 [`crate`] 文档的规范差）；
//! - `Σ = 0` 输出 `Zero`（弃权），不是随机 tie-break。

use crate::error::{Error, Result};
use crate::trit::Trit;

/// 一层三值全连接权重。
///
/// 权重、偏置都是三值；`scales` 是 f32 的**规范自由度**（只影响 f32 读数，
/// 不影响符号判决）。改 scale 不改行为，这一点由 `scale_gauge_is_behavioral_noop` 钉住。
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    rows: usize,
    cols: usize,
    trits: Vec<Trit>,
    bias: Vec<Trit>,
    scales: Vec<f32>,
    /// 原始 f32 偏置（可选）：只影响读数和导出，行为判据仍是三值 `bias`。
    bias_f32: Vec<f32>,
}

impl Layer {
    /// 新建一层；scale 全部取 `1.0`。
    ///
    /// `trits` 是行主序的 `rows × cols`，`bias` 长度是 `rows`。
    pub fn new(rows: usize, cols: usize, trits: &[i8], bias: &[i8]) -> Result<Self> {
        Self::with_scales(rows, cols, trits, bias, vec![1.0; rows])
    }

    /// 全零层（偏置也零）。
    pub fn zeros(rows: usize, cols: usize) -> Result<Self> {
        Self::new(rows, cols, &vec![0; rows * cols], &vec![0; rows])
    }

    /// 带 scale 新建一层。
    pub fn with_scales(
        rows: usize,
        cols: usize,
        trits: &[i8],
        bias: &[i8],
        scales: Vec<f32>,
    ) -> Result<Self> {
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        if trits.len() != rows * cols {
            return Err(Error::Shape {
                what: "trits",
                expected: rows * cols,
                got: trits.len(),
            });
        }
        if bias.len() != rows {
            return Err(Error::Shape {
                what: "bias",
                expected: rows,
                got: bias.len(),
            });
        }
        if scales.len() != rows {
            return Err(Error::Shape {
                what: "scales",
                expected: rows,
                got: scales.len(),
            });
        }
        for (row, &scale) in scales.iter().enumerate() {
            if !scale.is_finite() || scale <= 0.0 {
                return Err(Error::InvalidScale { row, value: scale });
            }
        }
        let mut out_trits = Vec::with_capacity(trits.len());
        for &value in trits {
            out_trits.push(Trit::from_i8(value).ok_or(Error::InvalidTrit(value))?);
        }
        let mut out_bias = Vec::with_capacity(bias.len());
        for &value in bias {
            out_bias.push(Trit::from_i8(value).ok_or(Error::InvalidTrit(value))?);
        }
        Ok(Self {
            rows,
            cols,
            trits: out_trits,
            bias: out_bias,
            scales,
            bias_f32: Vec::new(),
        })
    }

    /// 从 f32 权重构造：复用 `trit-learn` 的 absmean 三值化口径。
    ///
    /// 全零行会得到 `scale = 1.0`（而不是 0），保证“scale 必须为正”的层不变量。
    pub fn from_f32(rows: usize, cols: usize, weights: &[f32], bias: &[f32]) -> Result<Self> {
        if weights.len() != rows * cols {
            return Err(Error::Shape {
                what: "trits",
                expected: rows * cols,
                got: weights.len(),
            });
        }
        if bias.len() != rows {
            return Err(Error::Shape {
                what: "bias",
                expected: rows,
                got: bias.len(),
            });
        }
        let mut trits = Vec::with_capacity(rows * cols);
        let mut scales = Vec::with_capacity(rows);
        for row in 0..rows {
            let q = trit_learn::ternarize(&weights[row * cols..(row + 1) * cols]);
            trits.extend_from_slice(&q.q);
            scales.push(if q.scale == 0.0 { 1.0 } else { q.scale });
        }
        let bias_trits: Vec<i8> = bias
            .iter()
            .map(|&value| {
                if value > 0.0 {
                    1
                } else if value < 0.0 {
                    -1
                } else {
                    0
                }
            })
            .collect();
        let layer = Self::with_scales(rows, cols, &trits, &bias_trits, scales)?;
        if bias.iter().all(|value| *value == 0.0) {
            Ok(layer)
        } else {
            layer.with_bias_f32(bias.to_vec())
        }
    }

    /// 附加原始 f32 偏置（长度 `rows`、全部有限）；`bias` 仍是行为判据。
    pub fn with_bias_f32(mut self, values: Vec<f32>) -> Result<Self> {
        if values.is_empty() {
            self.bias_f32.clear();
            return Ok(self);
        }
        if values.len() != self.rows {
            return Err(Error::Shape {
                what: "bias_f32",
                expected: self.rows,
                got: values.len(),
            });
        }
        if let Some((row, value)) = values
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
        {
            return Err(Error::Wire(format!(
                "bias_f32 第 {row} 行是 {value}（非有限值）"
            )));
        }
        self.bias_f32 = values;
        Ok(self)
    }

    /// 行数（输出维）。
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// 列数（输入维）。
    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// 全部三值权重（行主序）。
    #[must_use]
    pub fn trits(&self) -> &[Trit] {
        &self.trits
    }

    /// 偏置（三值）。
    #[must_use]
    pub fn bias(&self) -> &[Trit] {
        &self.bias
    }

    /// 原始 f32 偏置；空表示没有保存幅度。
    #[must_use]
    pub fn bias_f32(&self) -> &[f32] {
        &self.bias_f32
    }

    /// 每行 scale。
    #[must_use]
    pub fn scales(&self) -> &[f32] {
        &self.scales
    }

    /// 取一个权重。
    #[must_use]
    pub fn trit(&self, row: usize, col: usize) -> Option<Trit> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.trits.get(row * self.cols + col).copied()
    }

    /// 取一个偏置。
    #[must_use]
    pub fn bias_trit(&self, row: usize) -> Option<Trit> {
        self.bias.get(row).copied()
    }

    /// 取一行 scale。
    #[must_use]
    pub fn scale(&self, row: usize) -> Option<f32> {
        self.scales.get(row).copied()
    }

    /// 取一行权重。
    #[must_use]
    pub fn row(&self, row: usize) -> Option<&[Trit]> {
        if row >= self.rows {
            return None;
        }
        self.trits.get(row * self.cols..(row + 1) * self.cols)
    }

    /// 三值前向：`out[r] = sign(Σ w·x + b)`。调用方保证 `input.len() == cols`。
    fn activate(&self, input: &[Trit], out: &mut Vec<Trit>) {
        out.clear();
        out.reserve(self.rows);
        for (row, &bias) in self.bias.iter().enumerate() {
            let weights = &self.trits[row * self.cols..(row + 1) * self.cols];
            let mut acc = i32::from(bias.to_i8());
            for (&w, &x) in weights.iter().zip(input.iter()) {
                acc += i32::from(w.to_i8()) * i32::from(x.to_i8());
            }
            out.push(Trit::sign_of(acc));
        }
    }

    /// f32 读数：不取符号，保留 scale 与幅值（给“置信度/裕量”用）。
    fn raw(&self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.rows);
        for (row, &bias) in self.bias.iter().enumerate() {
            let weights = &self.trits[row * self.cols..(row + 1) * self.cols];
            let mut acc = f32::from(bias.to_i8());
            for (&w, &x) in weights.iter().zip(input.iter()) {
                acc += f32::from(w.to_i8()) * x;
            }
            out.push(self.scales[row] * acc);
        }
        out
    }

    /// 与 [`Layer::raw`] 相同，但输入是三值。
    fn raw_trit(&self, input: &[Trit]) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.rows);
        for (row, &bias) in self.bias.iter().enumerate() {
            let weights = &self.trits[row * self.cols..(row + 1) * self.cols];
            let mut acc = f32::from(bias.to_i8());
            for (&w, &x) in weights.iter().zip(input.iter()) {
                acc += f32::from(w.to_i8()) * f32::from(x.to_i8());
            }
            out.push(self.scales[row] * acc);
        }
        out
    }

    pub(crate) fn set_trit(&mut self, row: usize, col: usize, to: Trit) {
        self.trits[row * self.cols + col] = to;
    }

    pub(crate) fn set_bias(&mut self, row: usize, to: Trit) {
        self.bias[row] = to;
        // 三值偏置一旦被编辑，旧的 f32 幅度就不再对应。
        self.bias_f32.clear();
    }

    pub(crate) fn set_scale(&mut self, row: usize, to: f32) {
        self.scales[row] = to;
    }

    pub(crate) fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        for col in 0..self.cols {
            self.trits.swap(a * self.cols + col, b * self.cols + col);
        }
        self.bias.swap(a, b);
        if !self.bias_f32.is_empty() {
            self.bias_f32.swap(a, b);
        }
        self.scales.swap(a, b);
    }

    pub(crate) fn negate_row(&mut self, row: usize) {
        for col in 0..self.cols {
            let index = row * self.cols + col;
            self.trits[index] = self.trits[index].neg();
        }
        self.bias[row] = self.bias[row].neg();
    }
}

/// 前馈三值网络。
#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    layers: Vec<Layer>,
}

impl Net {
    /// 从层列表新建；相邻层必须首尾相接。
    pub fn new(layers: Vec<Layer>) -> Result<Self> {
        if layers.is_empty() {
            return Err(Error::EmptySizes);
        }
        for pair in layers.windows(2) {
            if pair[0].rows != pair[1].cols {
                return Err(Error::Shape {
                    what: "layer",
                    expected: pair[0].rows,
                    got: pair[1].cols,
                });
            }
        }
        Ok(Self { layers })
    }

    /// 按 `sizes` 建全零网络（至少两个维度）。
    pub fn zeros(sizes: &[usize]) -> Result<Self> {
        if sizes.len() < 2 {
            return Err(Error::EmptySizes);
        }
        let mut layers = Vec::with_capacity(sizes.len() - 1);
        for pair in sizes.windows(2) {
            layers.push(Layer::zeros(pair[1], pair[0])?);
        }
        Self::new(layers)
    }

    /// 按 `sizes` 建确定性伪随机网络（xorshift64*；同 seed 同结果）。
    pub fn random(sizes: &[usize], seed: u64) -> Result<Self> {
        if sizes.len() < 2 {
            return Err(Error::EmptySizes);
        }
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        if state == 0 {
            state = 1;
        }
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let mut layers = Vec::with_capacity(sizes.len() - 1);
        for pair in sizes.windows(2) {
            let (rows, cols) = (pair[1], pair[0]);
            let mut trits = Vec::with_capacity(rows * cols);
            for _ in 0..rows * cols {
                trits.push(match next() % 3 {
                    0 => -1,
                    1 => 0,
                    _ => 1,
                });
            }
            let bias: Vec<i8> = (0..rows)
                .map(|_| match next() % 3 {
                    0 => -1,
                    1 => 0,
                    _ => 1,
                })
                .collect();
            layers.push(Layer::new(rows, cols, &trits, &bias)?);
        }
        Self::new(layers)
    }

    /// 从 f32 权重矩阵构造；`weights[i]` 是第 i 层的行主序矩阵。
    pub fn from_f32(sizes: &[usize], weights: &[Vec<f32>], biases: &[Vec<f32>]) -> Result<Self> {
        if sizes.len() < 2 || weights.len() != sizes.len() - 1 || biases.len() != weights.len() {
            return Err(Error::ArchMismatch);
        }
        let mut layers = Vec::with_capacity(weights.len());
        for (i, (w, b)) in weights.iter().zip(biases.iter()).enumerate() {
            layers.push(Layer::from_f32(sizes[i + 1], sizes[i], w, b)?);
        }
        Self::new(layers)
    }

    /// 输入维。
    #[must_use]
    pub fn input_dim(&self) -> usize {
        self.layers[0].cols()
    }

    /// 输出维。
    #[must_use]
    pub fn output_dim(&self) -> usize {
        self.layers[self.layers.len() - 1].rows()
    }

    /// 层数（不含输入）。
    #[must_use]
    pub fn depth(&self) -> usize {
        self.layers.len()
    }

    /// 全部层。
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// 取一层。
    #[must_use]
    pub fn layer(&self, index: usize) -> Option<&Layer> {
        self.layers.get(index)
    }

    /// 架构：`[输入维, 各层输出维...]`。
    #[must_use]
    pub fn arch(&self) -> Vec<usize> {
        let mut sizes = Vec::with_capacity(self.layers.len() + 1);
        sizes.push(self.input_dim());
        sizes.extend(self.layers.iter().map(Layer::rows));
        sizes
    }

    /// 权重总数（含偏置）。
    #[must_use]
    pub fn trit_count(&self) -> usize {
        self.layers
            .iter()
            .map(|layer| layer.trits.len() + layer.bias.len())
            .sum()
    }

    /// 三值前向。输入/输出都是三值。
    pub fn forward(&self, input: &[Trit]) -> Result<Vec<Trit>> {
        if input.len() != self.input_dim() {
            return Err(Error::Shape {
                what: "input",
                expected: self.input_dim(),
                got: input.len(),
            });
        }
        let mut current = input.to_vec();
        let mut next = Vec::new();
        for layer in &self.layers {
            layer.activate(&current, &mut next);
            std::mem::swap(&mut current, &mut next);
        }
        Ok(current)
    }

    /// 批量前向；逐行等价于 [`Net::forward`]。
    pub fn forward_batch(&self, batch: &[Vec<Trit>]) -> Result<Vec<Vec<Trit>>> {
        batch.iter().map(|row| self.forward(row)).collect()
    }

    /// 每层每行的 **pre-activation** 整数累加值（不取符号）。
    ///
    /// 用于探针选行：行内累加值非零的次数与幅值能廉价地说明“这一行在不在工作”。
    pub fn pre_activations(&self, input: &[Trit]) -> Result<Vec<Vec<i32>>> {
        if input.len() != self.input_dim() {
            return Err(Error::Shape {
                what: "input",
                expected: self.input_dim(),
                got: input.len(),
            });
        }
        let mut current = input.to_vec();
        let mut all = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            let mut pre = Vec::with_capacity(layer.rows());
            let mut next = Vec::with_capacity(layer.rows());
            for (row, &bias) in layer.bias.iter().enumerate() {
                let weights = &layer.trits[row * layer.cols..(row + 1) * layer.cols];
                let mut acc = i32::from(bias.to_i8());
                for (&weight, &value) in weights.iter().zip(current.iter()) {
                    acc += i32::from(weight.to_i8()) * i32::from(value.to_i8());
                }
                pre.push(acc);
                next.push(Trit::sign_of(acc));
            }
            all.push(pre);
            current = next;
        }
        Ok(all)
    }

    /// f32 读数：隐藏层按三值激活，最后一层保留 scale 后的原始分数。
    pub fn readout(&self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.input_dim() {
            return Err(Error::Shape {
                what: "input",
                expected: self.input_dim(),
                got: input.len(),
            });
        }
        if self.layers.len() == 1 {
            return Ok(self.layers[0].raw(input));
        }
        let first = self.layers[0].raw(input);
        let mut current: Vec<Trit> = first.iter().copied().map(sign_f32).collect();
        let mut next = Vec::new();
        for layer in &self.layers[1..self.layers.len() - 1] {
            layer.activate(&current, &mut next);
            std::mem::swap(&mut current, &mut next);
        }
        Ok(self.layers[self.layers.len() - 1].raw_trit(&current))
    }

    /// 行为状态哈希：只覆盖权重与偏置（scale 是规范自由度，故意排除）。
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        hash = fnv(hash, &(self.layers.len() as u64).to_le_bytes());
        for layer in &self.layers {
            hash = fnv(hash, &(layer.rows as u64).to_le_bytes());
            hash = fnv(hash, &(layer.cols as u64).to_le_bytes());
            for trit in &layer.trits {
                hash = fnv(hash, &[trit.to_i8() as u8]);
            }
            for trit in &layer.bias {
                hash = fnv(hash, &[trit.to_i8() as u8]);
            }
        }
        hash
    }

    /// 对隐藏层做行置换：`perm[i]` 是“变成第 `i` 行”的旧行号。
    ///
    /// 同时置换下一层的列（隐藏单元重命名）。输出层没有这种对称性，会报错。
    pub fn permute_rows(&self, layer: usize, perm: &[usize]) -> Result<Self> {
        if layer >= self.layers.len() {
            return Err(Error::OutOfBounds {
                layer,
                row: 0,
                col: 0,
            });
        }
        if layer + 1 == self.layers.len() {
            return Err(Error::OutputLayerPermutation { layer });
        }
        let rows = self.layers[layer].rows();
        if perm.len() != rows {
            return Err(Error::Shape {
                what: "perm",
                expected: rows,
                got: perm.len(),
            });
        }
        let mut seen = vec![false; rows];
        for &p in perm {
            if p >= rows || seen[p] {
                return Err(Error::NotPermutation { layer });
            }
            seen[p] = true;
        }
        let mut layers = self.layers.clone();
        {
            let source = &self.layers[layer];
            let cols = source.cols();
            let target = &mut layers[layer];
            if !source.bias_f32.is_empty() {
                target.bias_f32 = vec![0.0; rows];
            }
            for (new_row, &old_row) in perm.iter().enumerate() {
                let src = &source.trits[old_row * cols..(old_row + 1) * cols];
                let dst = &mut target.trits[new_row * cols..(new_row + 1) * cols];
                dst.copy_from_slice(src);
                target.bias[new_row] = source.bias[old_row];
                target.scales[new_row] = source.scales[old_row];
                if !source.bias_f32.is_empty() {
                    target.bias_f32[new_row] = source.bias_f32[old_row];
                }
            }
        }
        {
            let source = &self.layers[layer + 1];
            let next_cols = source.cols();
            let target = &mut layers[layer + 1];
            for row in 0..source.rows() {
                for (new_col, &old_col) in perm.iter().enumerate() {
                    target.trits[row * next_cols + new_col] =
                        source.trits[row * next_cols + old_col];
                }
            }
        }
        Self::new(layers)
    }

    pub(crate) fn layer_mut(&mut self, index: usize) -> &mut Layer {
        &mut self.layers[index]
    }
}

fn sign_f32(value: f32) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{Layer, Net};
    use crate::trit::Trit;

    #[test]
    fn layer_rejects_bad_shapes() {
        assert!(Layer::new(0, 2, &[], &[]).is_err());
        assert!(Layer::new(2, 2, &[0; 3], &[0; 2]).is_err());
        assert!(Layer::new(2, 2, &[0; 4], &[0; 3]).is_err());
        assert!(Layer::new(2, 2, &[2, 0, 0, 0], &[0; 2]).is_err());
    }

    #[test]
    fn forward_is_sign_of_integer_accumulation() {
        // 2x2 恒等：y = sign(x)；零和时输出弃权。
        let net = Net::new(vec![Layer::new(2, 2, &[1, 0, 0, 1], &[0, 0]).unwrap()]).unwrap();
        assert_eq!(
            net.forward(&[Trit::Neg, Trit::Pos]).unwrap(),
            vec![Trit::Neg, Trit::Pos]
        );
        assert_eq!(
            net.forward(&[Trit::Pos, Trit::Neg]).unwrap(),
            vec![Trit::Pos, Trit::Neg]
        );
        let anti = Net::new(vec![Layer::new(1, 2, &[1, 1], &[0]).unwrap()]).unwrap();
        assert_eq!(
            anti.forward(&[Trit::Pos, Trit::Neg]).unwrap(),
            vec![Trit::Zero]
        );
    }

    #[test]
    fn pre_activations_match_manual_sums() {
        let net = Net::new(vec![
            Layer::new(2, 2, &[1, -1, -1, 1], &[1, 0]).unwrap(),
            Layer::new(1, 2, &[1, 1], &[-1]).unwrap(),
        ])
        .unwrap();
        let pre = net.pre_activations(&[Trit::Pos, Trit::Pos]).unwrap();
        assert_eq!(pre, vec![vec![1, 0], vec![0]]);
    }

    #[test]
    fn batch_matches_single() {
        let net = Net::random(&[3, 4, 2], 7).unwrap();
        let batch = vec![
            vec![Trit::Neg, Trit::Zero, Trit::Pos],
            vec![Trit::Pos, Trit::Pos, Trit::Pos],
        ];
        let batched = net.forward_batch(&batch).unwrap();
        for (row, expected) in batch.iter().zip(batched.iter()) {
            assert_eq!(&net.forward(row).unwrap(), expected);
        }
    }

    #[test]
    fn zero_scale_is_rejected() {
        let err = Layer::with_scales(1, 1, &[1], &[0], vec![0.0]).unwrap_err();
        assert!(matches!(err, crate::error::Error::InvalidScale { .. }));
    }
}
