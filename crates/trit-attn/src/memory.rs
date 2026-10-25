//! 三值关联记忆：编码器把「键 → 值」写进一个整数矩阵，解码器用查询去读。
//!
//! 这是「快权重 FFN」的静态版：权重不是训练出来的，而是**现场由数据生成**
//! （`W = Σ_j v_j k_jᵀ`），宽度随条目数增长。键和值都是三值向量，
//! 所以 `W` 的元素是整数，编码/解码全程没有浮点（只有实验对照用 f32）。

/// 一次召回实验的报告。
#[derive(Debug, Clone, PartialEq)]
pub struct RecallReport {
    /// 条目数（= 快权重的「隐层宽度」）。
    pub pairs: usize,
    /// 键/值维度。
    pub dim: usize,
    /// 三值整数记忆的 top-1 召回率。
    pub ternary_accuracy: f32,
    /// f32 快权重记忆的召回率（对照，**未做能量对齐**）。
    ///
    /// 三值键的 `|k|² ≈ 2d/3`，f32 均匀键只有 `d/3`，所以这个数不能直接和三值比高低。
    pub float_accuracy: f32,
    /// softmax 注意力的召回率（对照，精确到 softmax）。
    pub attention_accuracy: f32,
    /// 「注意力」与「FFN 视角」在本次查询上的最大逐元素差。
    pub attention_ffn_max_diff: f32,
}

/// 三值关联记忆（编码器 / 解码器）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernaryMemory {
    dim: usize,
    weights: Vec<i32>,
}

/// 三值记忆的输入错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// 键/值/查询长度与 `dim` 不符。
    Dimension {
        /// 记忆维数。
        expected: usize,
        /// 传入的键（或查询）长度。
        key: usize,
        /// 传入的值长度；`decode` 时为 0。
        value: usize,
    },
    /// 权重累加溢出 `i32`。
    Overflow,
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dimension {
                expected,
                key,
                value,
            } => write!(
                f,
                "维度不符：记忆 {expected}×{expected}，键 {key}，值 {value}"
            ),
            Self::Overflow => write!(f, "权重累加溢出 i32"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl TernaryMemory {
    /// 建一个 `dim × dim` 的零矩阵。
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            weights: vec![0; dim * dim],
        }
    }

    /// 编码一条「键 → 值」：`W += v kᵀ`（键、值都必须已三值化）。
    ///
    /// 复审 M2：长度必须等于 `dim`，累加用 `checked_*` 防 `i32` 溢出。
    pub fn encode(&mut self, key: &[i32], value: &[i32]) -> Result<(), MemoryError> {
        if key.len() != self.dim || value.len() != self.dim {
            return Err(MemoryError::Dimension {
                expected: self.dim,
                key: key.len(),
                value: value.len(),
            });
        }
        for (row, value) in value.iter().enumerate() {
            for (col, key) in key.iter().enumerate() {
                let index = row * self.dim + col;
                let delta = value.checked_mul(*key).ok_or(MemoryError::Overflow)?;
                self.weights[index] = self.weights[index]
                    .checked_add(delta)
                    .ok_or(MemoryError::Overflow)?;
            }
        }
        Ok(())
    }

    /// 解码：`out = W q`，再取符号。查询长度必须等于 `dim`。
    pub fn decode(&self, query: &[i32]) -> Result<Vec<i32>, MemoryError> {
        if query.len() != self.dim {
            return Err(MemoryError::Dimension {
                expected: self.dim,
                key: query.len(),
                value: 0,
            });
        }
        Ok((0..self.dim)
            .map(|row| {
                // 中间量用 i64，避免 dim 很大时 i32 乘法先溢出。
                let acc: i64 = (0..self.dim)
                    .map(|col| {
                        i64::from(self.weights[row * self.dim + col]) * i64::from(query[col])
                    })
                    .sum();
                sign(acc.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
            })
            .collect())
    }

    /// 维数。
    pub fn dim(&self) -> usize {
        self.dim
    }
}

/// 跑一次召回实验：`pairs` 条三值键值对，查询用原键。
pub fn recall_experiment(seed: u64, pairs: usize, dim: usize) -> RecallReport {
    let mut rng = Rng(seed | 1);
    let keys: Vec<Vec<i32>> = (0..pairs)
        .map(|_| (0..dim).map(|_| rng.ternary()).collect())
        .collect();
    // 值取 ±1（不含 0），这样「解出来的符号」和「目标」在同一集合里，精度可比。
    let values: Vec<Vec<i32>> = (0..pairs)
        .map(|_| (0..dim).map(|_| rng.bipolar()).collect())
        .collect();

    let mut ternary = TernaryMemory::new(dim);
    for (key, value) in keys.iter().zip(values.iter()) {
        ternary.encode(key, value).expect("内部键值长度与 dim 一致");
    }
    // 逐分量精度：整向量精确匹配在 dim=16 时会被 (1-ε)^16 稀释到看不出差异。
    let mut ternary_hits = 0usize;
    for (key, value) in keys.iter().zip(values.iter()) {
        let decoded = ternary.decode(key).expect("内部查询长度与 dim 一致");
        ternary_hits += decoded
            .iter()
            .zip(value.iter())
            .filter(|(a, b)| a == b)
            .count();
    }

    // f32 快权重对照：同样的构造，但键/值是高斯 f32。
    let mut float_w = vec![0.0f32; dim * dim];
    let float_keys: Vec<Vec<f32>> = (0..pairs)
        .map(|_| (0..dim).map(|_| rng.f32()).collect())
        .collect();
    let float_values: Vec<Vec<f32>> = (0..pairs)
        .map(|_| (0..dim).map(|_| rng.f32()).collect())
        .collect();
    for (key, value) in float_keys.iter().zip(float_values.iter()) {
        for (row, value) in value.iter().enumerate() {
            for (col, key) in key.iter().enumerate() {
                float_w[row * dim + col] += value * key;
            }
        }
    }
    let mut float_hits = 0usize;
    for (key, value) in float_keys.iter().zip(float_values.iter()) {
        let decoded: Vec<f32> = (0..dim)
            .map(|row| {
                let acc: f32 = (0..dim)
                    .map(|col| float_w[row * dim + col] * key[col])
                    .sum();
                if acc >= 0.0 { 1.0 } else { -1.0 }
            })
            .collect();
        let target: Vec<f32> = value
            .iter()
            .map(|v| if *v >= 0.0 { 1.0 } else { -1.0 })
            .collect();
        float_hits += decoded
            .iter()
            .zip(target.iter())
            .filter(|(a, b)| a == b)
            .count();
    }

    // softmax 注意力对照 + 与 FFN 视角的数值一致性。
    let attention_keys: Vec<Vec<f32>> = keys
        .iter()
        .map(|key| key.iter().map(|value| *value as f32).collect())
        .collect();
    let attention_values: Vec<Vec<f32>> = values
        .iter()
        .map(|value| value.iter().map(|v| *v as f32).collect())
        .collect();
    let mut attention_hits = 0usize;
    let mut max_diff = 0.0f32;
    for (key, value) in keys.iter().zip(values.iter()) {
        let query: Vec<f32> = key.iter().map(|v| *v as f32).collect();
        let out = crate::scores::attention(&query, &attention_keys, &attention_values);
        let ffn = crate::scores::ffn_view(&query, &attention_keys, &attention_values);
        for (a, b) in out.iter().zip(ffn.iter()) {
            max_diff = max_diff.max((a - b).abs());
        }
        let decoded: Vec<i32> = out.iter().map(|v| if *v >= 0.0 { 1 } else { -1 }).collect();
        attention_hits += decoded
            .iter()
            .zip(value.iter())
            .filter(|(a, b)| a == b)
            .count();
    }

    RecallReport {
        pairs,
        dim,
        ternary_accuracy: ternary_hits as f32 / (pairs * dim) as f32,
        float_accuracy: float_hits as f32 / (pairs * dim) as f32,
        attention_accuracy: attention_hits as f32 / (pairs * dim) as f32,
        attention_ffn_max_diff: max_diff,
    }
}

fn sign(value: i32) -> i32 {
    match value.cmp(&0) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn ternary(&mut self) -> i32 {
        (self.next() % 3) as i32 - 1
    }
    fn bipolar(&mut self) -> i32 {
        if self.next().is_multiple_of(2) { 1 } else { -1 }
    }
    fn f32(&mut self) -> f32 {
        (self.next() % 2000) as f32 / 1000.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orthogonal_keys_recall_exactly() {
        let mut memory = TernaryMemory::new(3);
        let keys = [vec![1, 0, 0], vec![0, 1, 0], vec![0, 0, 1]];
        let values = [vec![1, -1, 0], vec![0, 1, -1], vec![-1, 0, 1]];
        for (key, value) in keys.iter().zip(values.iter()) {
            memory.encode(key, value).expect("测试维度一致");
        }
        for (key, value) in keys.iter().zip(values.iter()) {
            assert_eq!(memory.decode(key).expect("测试维度一致"), *value);
        }
    }

    #[test]
    fn report_is_deterministic_and_bounded() {
        let first = recall_experiment(7, 16, 8);
        let second = recall_experiment(7, 16, 8);
        assert_eq!(first, second);
        assert!(first.attention_ffn_max_diff < 1e-5);
        for accuracy in [
            first.ternary_accuracy,
            first.float_accuracy,
            first.attention_accuracy,
        ] {
            assert!((0.0..=1.0).contains(&accuracy));
        }
    }

    #[test]
    fn dimension_mismatch_is_an_error() {
        let mut memory = TernaryMemory::new(8);
        assert!(matches!(
            memory.encode(&[0; 5], &[1; 9]),
            Err(MemoryError::Dimension {
                expected: 8,
                key: 5,
                value: 9
            })
        ));
        assert!(matches!(
            memory.decode(&[0; 3]),
            Err(MemoryError::Dimension {
                expected: 8,
                key: 3,
                value: 0
            })
        ));
        assert!(memory.encode(&[0; 8], &[1; 8]).is_ok());
        assert!(memory.decode(&[0; 8]).is_ok());
    }

    #[test]
    fn sign_maps_through_zero() {
        assert_eq!(sign(-3), -1);
        assert_eq!(sign(0), 0);
        assert_eq!(sign(2), 1);
    }
}
