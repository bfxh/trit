//! 案例：网络 + 参考语言。
//!
//! M0 第一版只取一个阈值语言，先把「提取 → 对拍」这条链跑通；
//! 3-bit 加法器、括号匹配、Tomita #3 留到后续。

use trit_wspace::{Layer, Net, Trit};

/// 一个案例：最大长度 `max_len`、网络 `net`、参考语言由 `lang` 判定。
pub struct Case {
    pub name: &'static str,
    pub max_len: usize,
    pub net: Net,
    /// 参考语言：输入是**变长**比特串（`Trit::Pos` = 1，`Trit::Neg` = 0）。
    pub lang: fn(&[Trit]) -> bool,
}

/// 「串里至少 2 个 1」，长度 0..=3。
///
/// 网络吃的是 `word::encode` 的 2 维/位置编码（3 位 ⇒ 6 维输入）：
/// - 层 1：`is_one` 每位复制两份（6 个隐藏单元，权重只取 `is_one` 列），
///   再加 3 个常量单元（bias `+1`，恒 `Pos`）；
/// - 层 2：前 6 项 `+1`、后 3 项 `−1`，bias `0` ⇒ `sign(2·#1 − 3)`。
///
/// `#1=2 → 1 → Pos`、`#1=1 → −1 → Neg`、`#1=0 → −3 → Neg`、`#1=3 → 3 → Pos`，
/// 判决正好是「#1 ≥ 2」；填充位 `is_one = Zero` 不贡献，所以长度无关。
#[must_use]
pub fn at_least_two_ones() -> Case {
    let layer1 = Layer::new(
        9,
        6,
        &[
            1, 0, 0, 0, 0, 0, // pos0 is_one
            1, 0, 0, 0, 0, 0, //
            0, 0, 1, 0, 0, 0, // pos1 is_one
            0, 0, 1, 0, 0, 0, //
            0, 0, 0, 0, 1, 0, // pos2 is_one
            0, 0, 0, 0, 1, 0, //
            0, 0, 0, 0, 0, 0, // 常量
            0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0,
        ],
        &[0, 0, 0, 0, 0, 0, 1, 1, 1],
    )
    .expect("层 1 形状正确");
    let layer2 = Layer::new(1, 9, &[1, 1, 1, 1, 1, 1, -1, -1, -1], &[0]).expect("层 2 形状正确");
    let net = Net::new(vec![layer1, layer2]).expect("层维度连续");
    Case {
        name: "at_least_two_ones",
        max_len: 3,
        net,
        lang: |w| w.iter().filter(|t| **t == Trit::Pos).count() >= 2,
    }
}

/// 全部案例。
#[must_use]
pub fn all() -> Vec<Case> {
    vec![at_least_two_ones()]
}
