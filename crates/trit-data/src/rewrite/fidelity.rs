//! 保真检查：重写版本必须保留原文的硬事实（数字/URL/代码/引用），长度不许塌。

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fidelity {
    pub ok: bool,
    pub missing: Vec<String>,
    pub length_ratio_ok: bool,
}

/// 从原文提取必须保留的硬事实：数字、URL、反引号代码、引用编号。
pub fn anchors(text: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut num = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            if num.chars().filter(|x| x.is_ascii_digit()).count() >= 2 {
                out.insert(num.clone());
            }
            num.clear();
        }
    }
    if num.chars().filter(|x| x.is_ascii_digit()).count() >= 2 {
        out.insert(num);
    }
    for w in text.split_whitespace() {
        if w.starts_with("http://") || w.starts_with("https://") {
            out.insert(w.trim_end_matches([',', '。', ')', ']', '，']).to_string());
        }
    }
    let mut in_code = false;
    for part in text.split('`') {
        if in_code {
            let p = part.trim();
            if !p.is_empty() {
                out.insert(p.to_string());
            }
        }
        in_code = !in_code;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'^' {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start && j < bytes.len() && bytes[j] == b']' {
                out.insert(text[i..=j].to_string());
            }
        }
        i += 1;
    }
    out.into_iter().collect()
}

pub fn fidelity(original: &str, rewritten: &str) -> Fidelity {
    let missing: Vec<String> = anchors(original)
        .into_iter()
        .filter(|a| !rewritten.contains(a))
        .collect();
    let a = original.chars().count().max(1) as f64;
    let b = rewritten.chars().count() as f64;
    // 长文本用比例（截断/灌水都拦得住）；短文本（<100 字符）比例没有意义——
    // 一句 13 字的 thought 加一句结构提示就会超 2 倍，所以改用绝对上下限。
    let length_ratio_ok = if a < 100.0 {
        b >= a * 0.5 && b <= a * 4.0 + 40.0
    } else {
        (0.5..=2.0).contains(&(b / a))
    };
    Fidelity {
        ok: missing.is_empty() && length_ratio_ok,
        missing,
        length_ratio_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_extract_hard_facts() {
        let text = "运行 `pytest -q` 得到 42 passed，见 https://example.com/a 与 [1][^2]。";
        let a = anchors(text);
        assert!(a.iter().any(|x| x == "42"), "{a:?}");
        assert!(a.iter().any(|x| x == "https://example.com/a"), "{a:?}");
        assert!(a.iter().any(|x| x == "pytest -q"), "{a:?}");
        assert!(a.iter().any(|x| x == "[1]"), "{a:?}");
        assert!(a.iter().any(|x| x == "[^2]"), "{a:?}");
    }

    #[test]
    fn fidelity_detects_missing_anchor_and_truncation() {
        let original = "运行 `pytest -q` 得到 42 passed，见 https://example.com/a";
        assert!(fidelity(original, original).ok);
        let bad = fidelity(original, "完全不同的文本，没有保留任何硬事实");
        assert!(!bad.ok && !bad.missing.is_empty());
        let truncated = fidelity(original, "");
        assert!(!truncated.length_ratio_ok);
    }

    #[test]
    fn short_texts_get_absolute_room_but_not_unlimited() {
        // 13 字的 thought 加一句结构提示：比例 6 倍在长文本上是超限，短文本允许
        let short = "先看当前目录里有什么文件";
        let padded = format!("定义：这份记录说明了背景与过程。因为要保留可核对的事实，所以按步骤整理。例如先写现象，再写结果。最后给出结论。{short}");
        let f = fidelity(short, &padded);
        assert!(f.length_ratio_ok, "{f:?}");
        // 缩到 1 个字仍然算截断
        assert!(!fidelity(short, "短").length_ratio_ok);
        // 短文本也不是无上限：1 个字灌到 200 字判红
        assert!(!fidelity("短", &"重复句子。".repeat(40)).length_ratio_ok);
    }
}
