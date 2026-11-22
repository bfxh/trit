//! 语言门：可复算的文本特征与三值分类（Good / Uncertain / Bad）。
//!
//! 这是 M1 的**规则基线**，不是训练出来的教育性分类器。判据全部可重算：
//! 字符构成、控制字符、4-gram 重复、词汇多样性、教育性结构词、代码/URL/引用。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangClass {
    Good,
    Uncertain,
    Bad,
}

impl LangClass {
    pub fn as_str(self) -> &'static str {
        match self {
            LangClass::Good => "good",
            LangClass::Uncertain => "uncertain",
            LangClass::Bad => "bad",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Features {
    pub chars: usize,
    pub nonspace_ratio: f64,
    pub control_ratio: f64,
    pub repeat4_ratio: f64,
    pub alpha_ratio: f64,
    pub digit_ratio: f64,
    pub unique_word_ratio: f64,
    pub avg_word_len: f64,
    pub educational_hits: usize,
    pub code_blocks: usize,
    pub urls: usize,
    pub refs: usize,
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub bad_min_chars: usize,
    pub min_chars: usize,
    pub min_nonspace: f64,
    pub max_control: f64,
    pub max_repeat4: f64,
    pub warn_repeat4: f64,
    pub min_alpha: f64,
    pub min_unique_words: f64,
    pub min_educational: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            bad_min_chars: 40,
            min_chars: 200,
            min_nonspace: 0.6,
            max_control: 0.01,
            max_repeat4: 0.4,
            warn_repeat4: 0.25,
            min_alpha: 0.2,
            min_unique_words: 0.3,
            min_educational: 2,
        }
    }
}

pub fn features(text: &str) -> Features {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len().max(1) as f64;
    let nonspace = chars.iter().filter(|c| !c.is_whitespace()).count() as f64;
    let control = chars
        .iter()
        .filter(|c| {
            let u = (**c) as u32;
            u < 0x20 && !matches!(**c, '\n' | '\r' | '\t')
        })
        .count() as f64;
    let alpha = chars.iter().filter(|c| c.is_alphabetic()).count() as f64;
    let digit = chars.iter().filter(|c| c.is_numeric()).count() as f64;
    let toks = tokens(text);
    let words = toks.len().max(1) as f64;
    let unique = toks.iter().collect::<std::collections::BTreeSet<_>>().len() as f64;
    let repeat4 = if toks.len() >= 8 {
        let mut seen = std::collections::BTreeSet::new();
        let mut dup = 0usize;
        let mut all = 0usize;
        for w in toks.windows(4) {
            all += 1;
            if !seen.insert(w.join("\u{1}")) {
                dup += 1;
            }
        }
        dup as f64 / all.max(1) as f64
    } else {
        0.0
    };
    let lower = text.to_lowercase();
    let hits = [
        "定义",
        "因为",
        "所以",
        "例如",
        "步骤",
        "结论",
        "公式",
        "证明",
        "注意",
        "总结",
        "首先",
        "其次",
        "最后",
        "definition",
        "because",
        "therefore",
        "example",
        "step",
        "conclusion",
        "formula",
        "proof",
        "note",
        "summary",
        "first",
        "finally",
    ]
    .iter()
    .map(|k| lower.matches(k).count())
    .sum();
    Features {
        chars: chars.len(),
        nonspace_ratio: nonspace / total,
        control_ratio: control / total,
        repeat4_ratio: repeat4,
        alpha_ratio: alpha / total,
        digit_ratio: digit / total,
        unique_word_ratio: unique / words,
        avg_word_len: toks.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / words,
        educational_hits: hits,
        code_blocks: lower.matches("```").count() / 2,
        urls: lower.matches("http://").count() + lower.matches("https://").count(),
        refs: count_refs(text),
    }
}

pub fn classify(f: &Features, l: &Limits) -> LangClass {
    if f.chars < l.bad_min_chars
        || f.nonspace_ratio < l.min_nonspace
        || f.control_ratio > l.max_control
        || f.repeat4_ratio > l.max_repeat4
        || f.alpha_ratio < l.min_alpha
    {
        return LangClass::Bad;
    }
    if f.chars < l.min_chars
        || f.educational_hits < l.min_educational
        || f.unique_word_ratio < l.min_unique_words
        || f.repeat4_ratio > l.warn_repeat4
    {
        return LangClass::Uncertain;
    }
    LangClass::Good
}

pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if is_cjk(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(c.to_string());
        } else if c.is_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn count_refs(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut n = 0;
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
                n += 1;
            }
        }
        i += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_structural_signals() {
        let text = "定义：这是一个例子。例如 `pytest -q` 会运行测试，见 https://example.com/doc 与 [1][^2]。";
        let f = features(text);
        assert_eq!(f.code_blocks, 0);
        assert_eq!(f.urls, 1);
        assert_eq!(f.refs, 2);
        assert!(f.educational_hits >= 2);
        assert!(f.alpha_ratio > 0.3);
    }

    #[test]
    fn three_way_classification_is_deciding() {
        let l = Limits::default();
        let good = "定义：这份数据用于训练智能体。因为任务需要先读日志再改代码，所以每一步都要留下观察。例如先运行测试，再根据失败信息定位到具体函数并修复。最后给出结论，附上测试输出与证据哈希，便于复核。整个过程要记录工具调用、观察结果和失败后的恢复步骤，避免只给最终答案。还需要检查语言是否清晰、数字与代码是否保留，以及是否存在重复或空转。如果样本通过验证但过程不完整，应标记为 defer 而不是直接接受，交给重写或人工复核。这样既能保住可验证的硬事实，也能让不确定的样本进入下一轮处理。";
        assert_eq!(classify(&features(good), &l), LangClass::Good);

        assert_eq!(classify(&features("太短"), &l), LangClass::Bad);

        let repeated = "重复句子。".repeat(80);
        assert_eq!(classify(&features(&repeated), &l), LangClass::Bad);

        let plain = "今天下午的天气比较平稳，窗口外的树影缓慢移动。桌面上放着几个不同大小的杯子，旁边有一本翻开的册子。走廊里偶尔传来脚步声，随后又恢复安静。记录里写下了时间、地点和简单的观察，没有额外的解释。远处有一辆小车经过，颜色和上午看到的那辆不同。人们按自己的节奏处理手头的事情，偶尔停下来看看窗外。这个场景没有特别的说明，只是把看到的内容按顺序写下来，供以后核对。文件中还包含若干编号和日期，便于定位到具体的那一行记录。";
        assert_eq!(classify(&features(plain), &l), LangClass::Uncertain);
    }

    #[test]
    fn cjk_characters_are_tokens() {
        let f = features("中文测试");
        assert!(f.unique_word_ratio > 0.9);
        assert!(f.avg_word_len >= 1.0);
    }
}
