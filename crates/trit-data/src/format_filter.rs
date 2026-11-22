//! 格式筛选判定：deny 命中即过滤；仅 allow 时，未命中任何允许格式则过滤。

use crate::label_filter::LabelFilter;

pub fn denied(filter: &LabelFilter, formats: &[&str]) -> bool {
    if !filter.enabled {
        return false;
    }
    if !filter.deny.is_empty() {
        return formats.iter().any(|f| filter.deny.iter().any(|x| x == f));
    }
    if let Some(allow) = &filter.allow {
        return !formats.iter().any(|f| allow.iter().any(|x| x == f));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(enabled: bool, allow: Option<Vec<&str>>, deny: Vec<&str>) -> LabelFilter {
        LabelFilter {
            enabled,
            allow: allow.map(|v| v.into_iter().map(String::from).collect()),
            deny: deny.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn deny_wins_when_present() {
        let filter = f(true, None, vec!["json"]);
        assert!(denied(&filter, &["json"]));
        assert!(!denied(&filter, &["yaml"]));
    }

    #[test]
    fn allow_requires_at_least_one_match() {
        let filter = f(true, Some(vec!["json"]), vec![]);
        assert!(!denied(&filter, &["json"]));
        assert!(denied(&filter, &[]));
        assert!(denied(&filter, &["yaml"]));
    }

    #[test]
    fn disabled_never_filters() {
        assert!(!denied(&f(false, Some(vec!["json"]), vec!["yaml"]), &[]));
    }
}
