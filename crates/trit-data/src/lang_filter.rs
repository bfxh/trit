//! 语言过滤配置：批量/单次、gate/decontaminate/mix 共用。

use crate::script::primary_language;

#[derive(Debug, Clone)]
pub struct LangFilter {
    pub enabled: bool,
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
}

impl Default for LangFilter {
    fn default() -> Self {
        Self {
            enabled: true,
            allow: None,
            deny: Vec::new(),
        }
    }
}

impl LangFilter {
    pub fn denied(&self, lang: &str) -> bool {
        if !self.enabled {
            return false;
        }
        self.deny.iter().any(|x| x == lang)
            || self
                .allow
                .as_ref()
                .is_some_and(|a| !a.iter().any(|x| x == lang))
    }

    /// 返回 `(主语言, 是否被过滤)`。
    pub fn eval(&self, text: &str) -> (String, bool) {
        let lang = primary_language(text).to_string();
        let denied = self.denied(&lang);
        (lang, denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allows_everything() {
        let f = LangFilter::default();
        assert!(!f.denied("zh"));
        assert!(!f.denied("en"));
    }

    #[test]
    fn deny_and_allow_lists_decide() {
        let f = LangFilter {
            deny: vec!["en".into()],
            ..LangFilter::default()
        };
        assert!(f.denied("en"));
        assert!(!f.denied("zh"));

        let f = LangFilter {
            allow: Some(vec!["zh".into()]),
            ..LangFilter::default()
        };
        assert!(f.denied("en"));
        assert!(!f.denied("zh"));
    }

    #[test]
    fn disabled_filter_never_denies() {
        let f = LangFilter {
            enabled: false,
            deny: vec!["en".into()],
            allow: Some(vec!["zh".into()]),
        };
        assert!(!f.denied("en"));
        assert!(!f.denied("ja"));
    }
}
