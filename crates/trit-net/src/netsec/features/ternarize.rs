//! `ternarize`（拆出 `features.rs` 守模块行数；第 37 轮 S3 修规则下标越界）。

use trit_core::value::Ternary;

use crate::netsec::flow::N;

use super::Rule;

/// 把特征值按规则转成三值输入。
///
/// 没有被任何规则覆盖的维度固定为 `False`——这是**显式选择**：
/// 模型只该看被声明过的证据，没声明的维度不参与判断。
pub fn ternarize(values: &[f64; N], rules: &[Rule]) -> Vec<Ternary> {
    let mut inputs = vec![Ternary::False; N];
    for rule in rules {
        if rule.feature >= N {
            continue; // 规则下标来自文件，越界就跳过（第 37 轮 S3）
        }
        inputs[rule.feature] = rule.apply(values[rule.feature]);
    }
    inputs
}
