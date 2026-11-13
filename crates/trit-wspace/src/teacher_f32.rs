//! 受控合成教师：给分类实验提供**可复现**的标签。
//!
//! 这里的标签都是人工构造的（不是真实标注语料），存在的意义是让"能否学到一个
//! 指定映射"变成可度量的问题。所有函数都只用 [`Lcg`] 的确定性随机流，同一 seed
//! 必然给出同一批标签。
//!
//! 抽取顺序是契约：调用方先用同一个 `Lcg` 取教师标签，再用它初始化下游参数，
//! 这样读数是可复现的。

/// 确定性 LCG（Knuth MMIX 乘子），取高位映射到 `[-1, 1]`。
#[derive(Clone, Debug, PartialEq)]
pub struct Lcg(u64);

impl Lcg {
    /// 以种子建流。
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// 下一个 `[-1, 1]` 内的均匀数。
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = ((self.0 >> 40) as f32) / ((1u32 << 24) as f32);
        unit * 2.0 - 1.0
    }
}

/// 最大值下标（并列取靠前的一个）。
#[must_use]
pub fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

/// 行主序 `rows × cols` 权重乘输入，得到 `rows` 个得分。
#[must_use]
pub fn project(weights: &[f32], input: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    (0..rows)
        .map(|row| {
            weights[row * cols..(row + 1) * cols]
                .iter()
                .zip(input)
                .map(|(weight, value)| weight * value)
                .sum()
        })
        .collect()
}

/// 单位化（零向量原样返回）。
#[must_use]
pub fn unit(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 {
        vector.to_vec()
    } else {
        vector.iter().map(|value| value / norm).collect()
    }
}

/// 按得分排名切档：保证每类样本数尽量相同。
#[must_use]
pub fn rank_labels(scores: &[f32], classes: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|left, right| {
        scores[*left]
            .partial_cmp(&scores[*right])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut labels = vec![0usize; scores.len()];
    let bucket = scores.len().div_ceil(classes).max(1);
    for (rank, index) in order.iter().enumerate() {
        labels[*index] = (rank / bucket).min(classes - 1);
    }
    labels
}

/// 线性教师：`argmax(R · x)`，`R` 是 `classes × dim` 的随机投影。
///
/// 块输出本就可线性解码，线性头单独就能学会——只用于验证管线。
pub fn linear_labels(rng: &mut Lcg, tokens: &[Vec<f32>], classes: usize) -> Vec<usize> {
    let dim = tokens.first().map_or(0, Vec::len);
    let weights: Vec<f32> = (0..classes * dim).map(|_| rng.next_f32() * 0.1).collect();
    tokens
        .iter()
        .map(|token| argmax(&project(&weights, token, classes, dim)))
        .collect()
}

/// 二次型教师：`s = Σ (a_d · x̂_d)²`（输入先单位化），再按分位切 `classes` 档。
///
/// 二次型不是块输出的线性函数，设计意图是"线性头做不到、块必须参与"。
/// 注意：标签虽然均衡，但当样本数远小于特征维时，线性头仍可**记住**任意标注。
pub fn quad_labels(rng: &mut Lcg, tokens: &[Vec<f32>], classes: usize) -> Vec<usize> {
    let dim = tokens.first().map_or(0, Vec::len);
    let axis: Vec<f32> = (0..dim).map(|_| rng.next_f32() * 0.1).collect();
    let scores: Vec<f32> = tokens
        .iter()
        .map(|token| {
            let normalized = unit(token);
            normalized
                .iter()
                .zip(&axis)
                .map(|(value, weight)| {
                    let scaled = value * weight;
                    scaled * scaled
                })
                .sum()
        })
        .collect();
    rank_labels(&scores, classes)
}
