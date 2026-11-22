//! 教育性分数：FineWeb-Edu 风格的**规则近似**（0..5，≥3 视为高质量）。
//!
//! 不使用分类器权重；只看教育词、结构词、长度与重复率，结果可重算。

use crate::language::features;

pub fn score(text: &str) -> f64 {
    let f = features(text);
    let lower = text.to_lowercase();
    let structure = [
        "定义", "步骤", "例如", "结论", "公式", "证明", "首先", "其次", "最后", "definition",
        "step", "example", "conclusion", "formula", "proof",
    ]
    .iter()
    .map(|k| lower.matches(k).count())
    .sum::<usize>() as f64;
    let length = (f.chars as f64 / 800.0).min(1.0);
    let raw = (f.educational_hits as f64 / 4.0).min(1.0) * 0.4
        + (structure / 4.0).min(1.0) * 0.3
        + length * 0.3
        - f.repeat4_ratio * 0.5;
    (raw * 5.0).clamp(0.0, 5.0)
}

pub fn passes(text: &str, min: f64) -> bool {
    score(text) >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn educational_text_scores_high() {
        let text = "定义：三值逻辑有三种真值。步骤：首先把规则写出来，其次逐行求值，最后统计结果。例如，NULL 参与比较得到未知。结论：未知不是错误，而是需要显式处理的状态。公式与证明见后续章节。";
        let s = score(text);
        assert!(s >= 3.0, "{s}");
    }

    #[test]
    fn repeated_or_short_text_scores_low() {
        let repeated = "重复句子。".repeat(100);
        assert!(score(&repeated) < 2.0);
        assert!(score("太短") < 3.0);
    }
}
