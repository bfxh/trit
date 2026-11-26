//! 末态统计：非 `F` 占比与三符号香农熵。

use trit_core::value::Ternary;

/// 非 `F` 占比。
pub fn density(cells: &[Ternary]) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let alive = cells
        .iter()
        .filter(|value| **value != Ternary::False)
        .count();
    alive as f64 / cells.len() as f64
}

/// 三符号香农熵。
pub fn entropy(cells: &[Ternary]) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let total = cells.len() as f64;
    [Ternary::False, Ternary::Unknown, Ternary::True]
        .into_iter()
        .map(|symbol| {
            let count = cells.iter().filter(|value| **value == symbol).count() as f64;
            if count == 0.0 {
                0.0
            } else {
                -(count / total) * (count / total).log2()
            }
        })
        .sum()
}
