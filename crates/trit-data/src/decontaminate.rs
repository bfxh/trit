//! 去污染：候选文本与 benchmark 参考文本的 n-gram 重叠（可复算）。
//!
//! 分词与语言门一致：空白词 + CJK 单字 + ASCII 字母数字串。

use std::collections::BTreeSet;

use crate::language::tokens;

#[derive(Debug, Clone, PartialEq)]
pub struct Overlap {
    pub ngram: usize,
    pub shared: usize,
    pub total: usize,
    pub ratio: f64,
    pub contaminated: bool,
}

pub fn overlap(candidate: &str, reference: &str, n: usize, threshold: f64) -> Overlap {
    let n = n.max(1);
    let a = ngram_set(candidate, n);
    let b = ngram_set(reference, n);
    let total = a.len();
    let shared = a.intersection(&b).count();
    let ratio = if total == 0 {
        0.0
    } else {
        shared as f64 / total as f64
    };
    Overlap {
        ngram: n,
        shared,
        total,
        ratio,
        contaminated: ratio >= threshold,
    }
}

fn ngram_set(text: &str, n: usize) -> BTreeSet<String> {
    let toks = tokens(text);
    if toks.len() < n {
        return BTreeSet::new();
    }
    toks.windows(n)
        .map(|w| w.join("\u{1}"))
        .collect::<BTreeSet<_>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_fully_contaminated() {
        let t = "这是一段用于去污染测试的参考文本，包含若干不同的词语和句子。";
        let o = overlap(t, t, 3, 0.2);
        assert!(o.contaminated);
        assert!((o.ratio - 1.0).abs() < 1e-12);
        assert!(o.shared > 0);
    }

    #[test]
    fn unrelated_text_is_clean() {
        let a = "今天天气平稳，树影缓慢移动，桌面上放着几个杯子。";
        let b = "def solve(x): return x + 1  # a completely different code snippet";
        let o = overlap(a, b, 3, 0.2);
        assert!(!o.contaminated);
        assert_eq!(o.shared, 0);
    }

    #[test]
    fn partial_overlap_is_measured() {
        let a = "第一步读取日志，第二步定位函数，第三步修改代码并运行测试。";
        let b = "第一步读取日志，第二步定位函数，另外还有完全无关的内容。";
        let o = overlap(a, b, 3, 0.2);
        assert!(o.ratio > 0.0 && o.ratio < 1.0, "{o:?}");
    }
}
