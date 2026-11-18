//! 声明域上的**完全**验证（与模型大小、权重格式无关）。
//!
//! ## 与 `audit` 的区别
//!
//! | | `audit`（抽样） | 本模块（声明域） |
//! |---|---|---|
//! | 输入从哪来 | 从分布抽样 | **声明**的有限集合 |
//! | 验证方式 | 抽 N 条估计 | **全部**跑一遍 |
//! | `changed == 0` 的含义 | 「这批样本上没差」 | 「**声明域内**完全等价」——是证明 |
//! | 域外 | 不主张 | 不主张 |
//!
//! 强度来自「声明域可穷举」，不是「模型可穷举」。所以它对任意大小、任意权重格式
//! （f32 / 三值 / 量化）的模型都成立——只要模型能 `forward`。
//!
//! ## 用法
//!
//! ```text
//! 1. 声明域：例如「CIFAR-10 test 前 1000 条」；
//! 2. before = 原模型在该域上的输出；
//! 3. 改权重；
//! 4. after = 改后模型在该域上的输出；
//! 5. verify → changed == 0 即「该域上行为完全不变」。
//! ```
//!
//! 报告必须同时写清 `domain.label` 与 `checked`，否则「等价」没有意义。

/// 一个**声明**的输入集合。验证时全部跑，不抽样。
#[derive(Debug, Clone)]
pub struct Domain<I> {
    /// 域的可读描述（写进报告，例如「CIFAR-10 test 前 1000 条，seed 7」）。
    pub label: String,
    inputs: Vec<I>,
}

impl<I> Domain<I> {
    /// 用描述与输入构造。
    pub fn new(label: impl Into<String>, inputs: Vec<I>) -> Self {
        Self {
            label: label.into(),
            inputs,
        }
    }

    /// 输入条数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// 是否为空。
    ///
    /// 公开 API：调用方用它区分「空域」与「没声明域」——空域上 `verify` 会给出
    /// `changed = 0`（vacuously true），所以必须能单独检查。
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// 输入视图。
    #[must_use]
    pub fn inputs(&self) -> &[I] {
        &self.inputs
    }
}

/// 验证结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// 域描述。
    pub domain: String,
    /// 实际检查条数（= 域大小）。
    pub checked: usize,
    /// 输出不同的条数。
    pub changed: usize,
    /// 第一条不同的下标。
    pub first_diff: Option<usize>,
}

impl Verdict {
    /// 声明域内是否**完全**等价。
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.changed == 0
    }
}

/// 在声明域上**全部**比较两个前向函数。
///
/// `before` / `after` 可以是同一个模型的前后状态，也可以是两个不同模型；
/// 只要它们对同一个输入产出可比较的输出。
pub fn verify<I, O, F, G>(before: F, after: G, domain: &Domain<I>) -> Verdict
where
    F: Fn(&I) -> O,
    G: Fn(&I) -> O,
    O: PartialEq,
{
    let mut changed = 0usize;
    let mut first_diff = None;
    for (i, input) in domain.inputs().iter().enumerate() {
        if before(input) != after(input) {
            changed += 1;
            if first_diff.is_none() {
                first_diff = Some(i);
            }
        }
    }
    Verdict {
        domain: domain.label.clone(),
        checked: domain.len(),
        changed,
        first_diff,
    }
}

/// 声明域上的行为指纹：输出逐条哈希成一个 u64。
///
/// 两个模型（或改动前后）指纹相同 ⟺ 在该域上输出完全相同。
/// 用于「改权重后行为是否一格没动」的快速比对——比逐条 diff 省内存，
/// 但指纹相同仍需 `verify` 给出 `changed == 0` 的正式结论。
pub fn fingerprint<I, O, F>(forward: F, domain: &Domain<I>) -> u64
where
    F: Fn(&I) -> O,
    O: std::hash::Hash,
{
    use std::hash::Hasher;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for input in domain.inputs() {
        forward(input).hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> Domain<i32> {
        Domain::new("整数 0..4", vec![0, 1, 2, 3])
    }

    #[test]
    fn unchanged_model_is_exact() {
        let v = verify(|x: &i32| x * 2, |x: &i32| x * 2, &domain());
        assert!(v.is_exact());
        assert_eq!(v.checked, 4);
        assert_eq!(v.changed, 0);
        assert_eq!(v.first_diff, None);
        assert_eq!(v.domain, "整数 0..4");
    }

    #[test]
    fn changed_model_reports_first_diff() {
        // 只在下标 2（值 2）上改行为。
        let v = verify(
            |x: &i32| x * 2,
            |x: &i32| if *x == 2 { 99 } else { x * 2 },
            &domain(),
        );
        assert!(!v.is_exact());
        assert_eq!(v.changed, 1);
        assert_eq!(v.first_diff, Some(2));
    }

    #[test]
    fn fingerprint_matches_iff_outputs_match() {
        let d = domain();
        let a = fingerprint(|x: &i32| x * 2, &d);
        let b = fingerprint(|x: &i32| x * 2, &d);
        let c = fingerprint(|x: &i32| x * 3, &d);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn empty_domain_is_vacuously_exact_but_checked_zero() {
        let d: Domain<i32> = Domain::new("空域", vec![]);
        let v = verify(|x: &i32| *x, |x: &i32| *x + 1, &d);
        assert!(v.is_exact(), "空域上 vacuously true");
        assert_eq!(v.checked, 0, "但必须报告 checked = 0，否则是假保证");
    }
}
