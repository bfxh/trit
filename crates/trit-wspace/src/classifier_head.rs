//! 线性分类头：`logits = weight · x + bias`，两种求法。
//!
//! - **闭式**：[`ClassifierHead::fit_ridge`] 解 `(XᵗX + λI)W = XᵗY`，不需要训练循环，
//!   适合量"冻结特征的可解码上限"。
//! - **SGD**：[`crate::transformer_f32_cls::train_classify_with`] 把交叉熵梯度同时
//!   回传到头与 transformer 块。
//!
//! 两者产生同一种工件（[`ClassifierHead`]），便于在留出集上直接对比。

use crate::error::{Error, Result};

/// 线性分类头，`weight` 行主序 `classes × dim`。
#[derive(Clone, Debug, PartialEq)]
pub struct ClassifierHead {
    weight: Vec<f32>,
    bias: Vec<f32>,
    classes: usize,
    dim: usize,
}

impl ClassifierHead {
    /// 全零初始化（权重与偏置都是 0）。
    pub fn zeros(classes: usize, dim: usize) -> Result<Self> {
        if classes == 0 || dim == 0 {
            return Err(Error::EmptyLayer {
                rows: classes,
                cols: dim,
            });
        }
        Ok(Self {
            weight: vec![0.0; classes * dim],
            bias: vec![0.0; classes],
            classes,
            dim,
        })
    }

    /// 直接给权重（`weight` 行主序 `classes × dim`，`bias` 长度 `classes`）。
    pub fn new(weight: Vec<f32>, bias: Vec<f32>, classes: usize, dim: usize) -> Result<Self> {
        if weight.len() != classes * dim {
            return Err(Error::Shape {
                what: "classifier weight",
                expected: classes * dim,
                got: weight.len(),
            });
        }
        if bias.len() != classes {
            return Err(Error::Shape {
                what: "classifier bias",
                expected: classes,
                got: bias.len(),
            });
        }
        Ok(Self {
            weight,
            bias,
            classes,
            dim,
        })
    }

    /// 类别数。
    #[must_use]
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// 特征维数。
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 权重（行主序 `classes × dim`）。
    #[must_use]
    pub fn weight(&self) -> &[f32] {
        &self.weight
    }

    /// 偏置。
    #[must_use]
    pub fn bias(&self) -> &[f32] {
        &self.bias
    }

    /// 可写权重（训练用）。
    pub(crate) fn weight_mut(&mut self) -> &mut [f32] {
        &mut self.weight
    }

    /// 可写偏置（训练用）。
    pub(crate) fn bias_mut(&mut self) -> &mut [f32] {
        &mut self.bias
    }

    /// 一个样本的预测类别。
    pub fn predict(&self, features: &[f32]) -> Result<usize> {
        Ok(argmax(&self.logits(features)?))
    }

    /// 一批样本的准确率。
    pub fn accuracy(&self, features: &[Vec<f32>], labels: &[usize]) -> Result<f32> {
        if features.len() != labels.len() {
            return Err(Error::Shape {
                what: "accuracy labels",
                expected: features.len(),
                got: labels.len(),
            });
        }
        if features.is_empty() {
            return Err(Error::Shape {
                what: "accuracy samples",
                expected: 1,
                got: 0,
            });
        }
        let mut hits = 0usize;
        for (row, label) in features.iter().zip(labels) {
            if self.predict(row)? == *label {
                hits += 1;
            }
        }
        Ok(hits as f32 / labels.len() as f32)
    }

    /// 闭式岭回归：`(XᵗX + λI)W = XᵗY`（特征末列补 1 作偏置）。
    ///
    /// `λ > 0` 时正规方程必然正定、可解；返回 `None` 只表示数值退化（主元塌成 0）。
    #[must_use]
    pub fn fit_ridge(features: &[Vec<f32>], labels: &[usize], lambda: f32) -> Option<Self> {
        let classes = labels.iter().copied().max().map_or(0, |value| value + 1);
        let dim = features.first().map_or(0, Vec::len);
        if classes == 0 || dim == 0 || features.len() != labels.len() {
            return None;
        }
        let rows = dim + 1;
        let mut gram = vec![vec![0.0f64; rows]; rows];
        let mut cross = vec![vec![0.0f64; classes]; rows];
        for (feature, label) in features.iter().zip(labels) {
            if feature.len() != dim {
                return None;
            }
            let values: Vec<f64> = feature
                .iter()
                .map(|value| f64::from(*value))
                .chain(std::iter::once(1.0))
                .collect();
            for (left, left_value) in values.iter().enumerate() {
                for right in left..rows {
                    gram[left][right] += left_value * values[right];
                }
                cross[left][*label] += left_value;
            }
        }
        // 上三角镜像到下三角，再在主对角线加 λ。
        let upper = gram.clone();
        for (left, row) in gram.iter_mut().enumerate() {
            for (right, slot) in row.iter_mut().enumerate() {
                if right < left {
                    *slot = upper[right][left];
                }
                if right == left {
                    *slot += f64::from(lambda);
                }
            }
        }
        let solved = solve(gram, cross)?;
        let mut weight = vec![0.0f32; classes * dim];
        let mut bias = vec![0.0f32; classes];
        for (index, row) in solved.iter().enumerate() {
            for (class, value) in row.iter().enumerate() {
                if index == dim {
                    bias[class] = *value as f32;
                } else {
                    weight[class * dim + index] = *value as f32;
                }
            }
        }
        Self::new(weight, bias, classes, dim).ok()
    }

    /// 原始得分（内部与训练共用）。
    pub(crate) fn logits(&self, features: &[f32]) -> Result<Vec<f32>> {
        if features.len() != self.dim {
            return Err(Error::Shape {
                what: "classifier input",
                expected: self.dim,
                got: features.len(),
            });
        }
        Ok((0..self.classes)
            .map(|class| {
                let line = &self.weight[class * self.dim..(class + 1) * self.dim];
                line.iter()
                    .zip(features)
                    .map(|(weight, value)| weight * value)
                    .sum::<f32>()
                    + self.bias[class]
            })
            .collect())
    }
}

/// 解 `A W = B`（高斯-约当 + 部分主元）；`A` 是 `d×d`、`B` 是 `d×c`。
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let d = a.len();
    for pivot in 0..d {
        let mut best = pivot;
        for row in pivot + 1..d {
            if a[row][pivot].abs() > a[best][pivot].abs() {
                best = row;
            }
        }
        if a[best][pivot].abs() < 1e-12 {
            return None;
        }
        a.swap(pivot, best);
        b.swap(pivot, best);
        let diagonal = a[pivot][pivot];
        // 主元行/右端项先取出来，避免同一行既读又写的借用冲突。
        let mut pivot_row = std::mem::take(&mut a[pivot]);
        let mut pivot_b = std::mem::take(&mut b[pivot]);
        for value in pivot_row.iter_mut().skip(pivot) {
            *value /= diagonal;
        }
        for value in pivot_b.iter_mut() {
            *value /= diagonal;
        }
        for row in 0..d {
            if row == pivot {
                continue;
            }
            let factor = a[row][pivot];
            if factor == 0.0 {
                continue;
            }
            for (slot, value) in a[row][pivot..].iter_mut().zip(&pivot_row[pivot..]) {
                *slot -= factor * value;
            }
            for (slot, value) in b[row].iter_mut().zip(&pivot_b) {
                *slot -= factor * value;
            }
        }
        a[pivot] = pivot_row;
        b[pivot] = pivot_b;
    }
    Some(b)
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}
