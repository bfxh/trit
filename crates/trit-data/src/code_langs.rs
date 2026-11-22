//! 编程语言识别。

use crate::code_langs_data::PROBES;

pub fn code_languages(text: &str) -> Vec<&'static str> {
    let t = text.to_lowercase();
    PROBES
        .iter()
        .filter(|(_, keys)| keys.iter().any(|k| t.contains(k)))
        .map(|(lang, _)| *lang)
        .collect()
}
