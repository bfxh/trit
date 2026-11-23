//! 语言门的三支映射：谁算软门、谁算硬门。
//!
//! `Defer` 是历史行为（Bad → defer，Uncertain 放过）；`Strict` 把 Uncertain 也
//! 转人工、把 Bad 判 reject；`Off` 只保留语言识别（allow/deny），不做质量判断。

use crate::language::LangClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangPolicy {
    Off,
    Defer,
    Strict,
}

impl LangPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            LangPolicy::Off => "off",
            LangPolicy::Defer => "defer",
            LangPolicy::Strict => "strict",
        }
    }
}

/// 硬门：只有 `Strict` + `Bad` 才 reject。
pub fn hard_reason(policy: LangPolicy, class: Option<LangClass>) -> Option<&'static str> {
    matches!((policy, class), (LangPolicy::Strict, Some(LangClass::Bad)))
        .then_some("language_bad")
}

/// 软门：`Defer` 下 Bad 转人工；`Strict` 下 Uncertain 也转人工。
pub fn soft_reason(policy: LangPolicy, class: Option<LangClass>) -> Option<&'static str> {
    match (policy, class) {
        (LangPolicy::Defer, Some(LangClass::Bad)) => Some("language"),
        (LangPolicy::Strict, Some(LangClass::Uncertain)) => Some("language_uncertain"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_policies_map_bad_and_uncertain_differently() {
        let bad = Some(LangClass::Bad);
        let uncertain = Some(LangClass::Uncertain);
        let good = Some(LangClass::Good);

        assert_eq!(hard_reason(LangPolicy::Strict, bad), Some("language_bad"));
        assert_eq!(hard_reason(LangPolicy::Defer, bad), None);
        assert_eq!(hard_reason(LangPolicy::Off, bad), None);
        assert_eq!(hard_reason(LangPolicy::Strict, good), None);

        assert_eq!(soft_reason(LangPolicy::Defer, bad), Some("language"));
        assert_eq!(soft_reason(LangPolicy::Strict, bad), None);
        assert_eq!(
            soft_reason(LangPolicy::Strict, uncertain),
            Some("language_uncertain")
        );
        assert_eq!(soft_reason(LangPolicy::Defer, uncertain), None);
        assert_eq!(soft_reason(LangPolicy::Off, uncertain), None);
        // 关掉语言门时分类是 None，两个方向的映射都必须静默
        assert_eq!(hard_reason(LangPolicy::Strict, None), None);
        assert_eq!(soft_reason(LangPolicy::Strict, None), None);
    }
}
