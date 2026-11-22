//! 语言/文字系统检测：按 Unicode 区块统计比例（可复算），供语言过滤使用。

use crate::schema::Trajectory;

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptMix {
    pub han: f64,
    pub latin: f64,
    pub kana: f64,
    pub hangul: f64,
    pub cyrillic: f64,
    pub arabic: f64,
    pub other: f64,
}

pub fn script_mix(text: &str) -> ScriptMix {
    let (mut han, mut latin, mut kana, mut hangul, mut cyrillic, mut arabic, mut other) =
        (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for c in text.chars() {
        let u = c as u32;
        if (0x4E00..=0x9FFF).contains(&u) || (0x3400..=0x4DBF).contains(&u) {
            han += 1.0;
        } else if (0x3040..=0x30FF).contains(&u) {
            kana += 1.0;
        } else if (0xAC00..=0xD7AF).contains(&u) {
            hangul += 1.0;
        } else if (0x0400..=0x04FF).contains(&u) {
            cyrillic += 1.0;
        } else if (0x0600..=0x06FF).contains(&u) {
            arabic += 1.0;
        } else if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&u) {
            latin += 1.0;
        } else if c.is_alphabetic() {
            other += 1.0;
        }
    }
    let total = (han + latin + kana + hangul + cyrillic + arabic + other).max(1.0);
    ScriptMix {
        han: han / total,
        latin: latin / total,
        kana: kana / total,
        hangul: hangul / total,
        cyrillic: cyrillic / total,
        arabic: arabic / total,
        other: other / total,
    }
}

/// 主语言：`zh` / `ja` / `ko` / `ru` / `ar` / `en` / `other`。
pub fn primary_language(text: &str) -> &'static str {
    let m = script_mix(text);
    if m.kana > 0.05 && m.kana + m.han >= m.latin {
        return "ja";
    }
    if m.hangul > 0.2 {
        return "ko";
    }
    if m.han > m.latin && m.han > 0.1 {
        return "zh";
    }
    if m.cyrillic > 0.2 {
        return "ru";
    }
    if m.arabic > 0.2 {
        return "ar";
    }
    if m.latin > 0.2 {
        return "en";
    }
    "other"
}

pub fn trajectory_text(t: &Trajectory) -> String {
    let mut out = String::new();
    for s in &t.steps {
        if let Some(x) = &s.thought {
            out.push_str(x);
            out.push('\n');
        }
        if let Some(x) = &s.observation {
            out.push_str(x);
            out.push('\n');
        }
    }
    out.push_str(&t.outcome.final_text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        assert_eq!(
            primary_language("这是中文文本，包含一些英文 code 与数字 123。"),
            "zh"
        );
        assert_eq!(
            primary_language("This is an English sentence with some 中文。"),
            "en"
        );
        assert_eq!(primary_language("これは日本語のテキストです。"), "ja");
        assert_eq!(primary_language("안녕하세요 한국어 텍스트입니다."), "ko");
        assert_eq!(primary_language("Это русский текст."), "ru");
        assert_eq!(primary_language("هذا نص عربي."), "ar");
        assert_eq!(primary_language("123 !!!"), "other");
    }

    #[test]
    fn mix_sums_to_one_for_letters() {
        let m = script_mix("中文 English 日本語");
        let sum = m.han + m.latin + m.kana + m.hangul + m.cyrillic + m.arabic + m.other;
        assert!((sum - 1.0).abs() < 1e-9, "{m:?}");
    }
}
