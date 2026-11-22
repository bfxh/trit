//! 位宽 / 多值 / 数据单位识别。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BitInfo {
    pub widths: BTreeMap<String, usize>,
    pub units: BTreeMap<String, usize>,
    pub bit_ops: bool,
    pub multi_valued: bool,
}

pub fn analyze(text: &str) -> BitInfo {
    let lower = text.to_lowercase();
    let mut info = BitInfo::default();
    for w in ["1", "2", "4", "8", "16", "32", "64"] {
        let pats = [
            format!("{w}-bit"),
            format!("{w} bit"),
            format!("{w}bits"),
            format!("{w} bits"),
            format!("int{w}"),
            format!("uint{w}"),
            format!("float{w}"),
            format!("fp{w}"),
            format!("bf{w}"),
        ];
        let n: usize = pats.iter().map(|p| lower.matches(p).count()).sum();
        if n > 0 {
            info.widths.insert(format!("{w}bit"), n);
        }
    }
    for (key, pats) in [
        ("bit", &["bit ", "bits "][..]),
        ("byte", &["byte ", "bytes "][..]),
        ("kb", &["kb", "kib"][..]),
        ("mb", &["mb", "mib"][..]),
        ("gb", &["gb", "gib"][..]),
    ] {
        let n: usize = pats.iter().map(|p| lower.matches(p).count()).sum();
        if n > 0 {
            info.units.insert(key.to_string(), n);
        }
    }
    info.bit_ops = ["<<", ">>", " & ", " | ", " ^ ", "~"]
        .iter()
        .any(|op| text.contains(op));
    info.multi_valued = [
        "ternary",
        "trit",
        "quaternary",
        "multi-valued",
        "multivalued",
        "3-value",
        "4-value",
        "bitnet",
        "int4",
        "int8",
        "nf4",
    ]
    .iter()
    .any(|k| lower.contains(k));
    info
}

/// 可用于筛选的标签：`1bit`..`64bit`、`multi-valued`。
pub fn labels(text: &str) -> Vec<&'static str> {
    let info = analyze(text);
    let mut out = Vec::new();
    for w in ["1bit", "2bit", "4bit", "8bit", "16bit", "32bit", "64bit"] {
        if info.widths.contains_key(w) {
            out.push(w);
        }
    }
    if info.multi_valued {
        out.push("multi-valued");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_widths_units_and_multi_value() {
        let info = analyze("使用 int8 与 4-bit 权重，对比 16 bit 激活；内存 512 MB，吞吐 2 GB/s。");
        assert!(info.widths.contains_key("8bit"));
        assert!(info.widths.contains_key("4bit"));
        assert!(info.widths.contains_key("16bit"));
        assert!(info.units.contains_key("mb"));
        assert!(info.units.contains_key("gb"));
        assert!(info.multi_valued);
    }

    #[test]
    fn bit_ops_and_labels() {
        let info = analyze("x = a << 2 | b & mask;");
        assert!(info.bit_ops);
        assert_eq!(labels("8-bit 量化与 ternary 权重"), vec!["8bit", "multi-valued"]);
        assert!(labels("普通文本").is_empty());
    }
}
