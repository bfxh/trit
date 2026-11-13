//! 多分类评测指标（混淆矩阵 / 准确率 / 逐类与 macro P-R-F1）。
//!
//! 神经训练的评测部件：与 [`crate::loss::cross_entropy`] 的 `label: usize`
//! 多类口径配套。二分类场景同样适用（`classes = 2`，正类取 1）。
//!
//! 设计约束：不依赖 serde/JSON（库层保持零多余依赖），序列化由调用方做。

/// 多类混淆矩阵：`counts[pred][label]`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confusion {
    classes: usize,
    counts: Vec<usize>,
}

impl Confusion {
    /// `classes` 类的空混淆矩阵。
    pub fn new(classes: usize) -> Self {
        assert!(classes > 0, "classes 必须非零");
        Self {
            classes,
            counts: vec![0; classes * classes],
        }
    }

    /// 记录一个样本：`pred` 是argmax 类，`label` 是真值。
    pub fn observe(&mut self, pred: usize, label: usize) {
        assert!(pred < self.classes && label < self.classes, "类越界");
        self.counts[pred * self.classes + label] += 1;
    }

    /// 类数。
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// `pred` 类、`label` 类格子的计数。
    pub fn count(&self, pred: usize, label: usize) -> usize {
        self.counts[pred * self.classes + label]
    }

    /// 总样本数。
    pub fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    /// 准确率。
    pub fn accuracy(&self) -> f32 {
        let diag: usize = (0..self.classes).map(|c| self.count(c, c)).sum();
        diag as f32 / self.total().max(1) as f32
    }

    /// 逐类 `(precision, recall, f1)`；某类无样本时三项均为 0。
    pub fn per_class(&self) -> Vec<(f32, f32, f32)> {
        (0..self.classes)
            .map(|c| {
                let tp = self.count(c, c);
                let fp: usize = (0..self.classes).map(|p| self.count(p, c)).sum::<usize>() - tp;
                let fn_: usize = (0..self.classes).map(|l| self.count(c, l)).sum::<usize>() - tp;
                let precision = tp as f32 / (tp + fp).max(1) as f32;
                let recall = tp as f32 / (tp + fn_).max(1) as f32;
                let f1 = 2.0 * precision * recall / (precision + recall).max(1e-12);
                (precision, recall, f1)
            })
            .collect()
    }

    /// macro 平均 F1（类别不均衡时的主指标；空类记 0）。
    pub fn macro_f1(&self) -> f32 {
        let per = self.per_class();
        let nonempty: Vec<&(f32, f32, f32)> = per
            .iter()
            .enumerate()
            .filter(|(c, _)| (0..self.classes).any(|p| self.count(p, *c) > 0))
            .map(|(_, v)| v)
            .collect();
        if nonempty.is_empty() {
            return 0.0;
        }
        nonempty.iter().map(|(_, _, f1)| f1).sum::<f32>() / nonempty.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_confusion_and_rates() {
        // pred=1 视为正类：tp=3（count(1,1)）、fp=1（count(1,0)）、fn=1、tn=1。
        let mut c = Confusion::new(2);
        c.observe(1, 1);
        c.observe(1, 1);
        c.observe(1, 1);
        c.observe(1, 0);
        c.observe(0, 1);
        c.observe(0, 0);
        assert_eq!(c.total(), 6);
        assert!((c.accuracy() - 4.0 / 6.0).abs() < 1e-6);
        let per = c.per_class();
        assert!((per[1].0 - 0.75).abs() < 1e-6, "precision {}", per[1].0);
        assert!((per[1].1 - 0.75).abs() < 1e-6, "recall {}", per[1].1);
        assert!((c.macro_f1() - (per[0].2 + per[1].2) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn empty_class_is_excluded_from_macro() {
        let mut c = Confusion::new(3);
        c.observe(0, 0);
        c.observe(1, 1);
        // 类 2 从未出现：macro 只平均出现过的类。
        assert!((c.macro_f1() - 1.0).abs() < 1e-6);
        assert!(c.per_class()[2].2 == 0.0);
    }

    #[test]
    fn perfect_prediction_is_all_correct() {
        let mut c = Confusion::new(4);
        for label in 0..4 {
            for _ in 0..3 {
                c.observe(label, label);
            }
        }
        assert!((c.accuracy() - 1.0).abs() < 1e-6);
        assert!((c.macro_f1() - 1.0).abs() < 1e-6);
    }
}
