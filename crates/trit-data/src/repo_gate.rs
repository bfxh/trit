//! 仓库级质量门：对 `RepoUnit` 做硬门（reject）与软门（defer）判定。
//!
//! 与轨迹门（`gate`）分开：代码文件的证据是"可解析、有许可证、有测试、非生成、
//! 不重复"，而不是 steps / tool calls。决策沿用 [`crate::gate::Decision`]。

use std::collections::BTreeSet;

use crate::gate::{Decision, Verdict};
use crate::repo::Unit;
use crate::repo::dup::Dup;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub require_license: bool,
    pub reject_missing_license: bool,
    pub require_tests: bool,
    pub reject_generated: bool,
    pub reject_duplicates: bool,
    pub reject_unknown_lang: bool,
    pub max_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            require_license: false,
            reject_missing_license: false,
            require_tests: false,
            reject_generated: true,
            reject_duplicates: true,
            reject_unknown_lang: true,
            max_bytes: 1 << 20,
        }
    }
}

/// 重复组里除第一份之外的路径（组内路径已排序，保留字典序第一份）。
pub fn duplicate_paths(dup: &Dup) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for g in &dup.exact {
        out.extend(g.paths.iter().skip(1).cloned());
    }
    for g in &dup.near {
        out.extend(g.paths.iter().skip(1).cloned());
    }
    out
}

pub fn evaluate(u: &Unit, cfg: &Config, dup: &BTreeSet<String>) -> Verdict {
    let (mut reject, mut defer) = (Vec::new(), Vec::new());
    if u.path.is_empty() || u.lang.is_empty() || u.bytes == 0 {
        reject.push("schema");
    }
    if cfg.reject_unknown_lang && u.lang == "unknown" {
        reject.push("unknown_lang");
    }
    if cfg.reject_generated && u.generated {
        reject.push("generated");
    }
    if cfg.reject_duplicates && dup.contains(&u.path) {
        reject.push("duplicate");
    }
    if cfg.reject_missing_license && u.license.is_none() {
        reject.push("license");
    }
    if cfg.require_license && u.license.is_none() {
        defer.push("license");
    }
    if !u.imports_known {
        defer.push("imports_unknown");
    }
    if cfg.require_tests && !u.is_test && u.tests.is_empty() && !u.inline_tests {
        defer.push("no_test");
    }
    if u.bytes > cfg.max_bytes {
        defer.push("too_large");
    }
    if !reject.is_empty() {
        Verdict {
            decision: Decision::Reject,
            reasons: reject,
        }
    } else if !defer.is_empty() {
        Verdict {
            decision: Decision::Defer,
            reasons: defer,
        }
    } else {
        Verdict {
            decision: Decision::Accept,
            reasons: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(path: &str, lang: &str) -> Unit {
        Unit {
            repo: "r".into(),
            path: path.into(),
            lang: lang.into(),
            bytes: 10,
            imports_known: true,
            ..Default::default()
        }
    }

    #[test]
    fn hard_gates_reject_generated_duplicate_and_unknown() {
        let cfg = Config::default();
        let mut generated = unit("a.rs", "rust");
        generated.generated = true;
        assert_eq!(
            evaluate(&generated, &cfg, &BTreeSet::new()).decision,
            Decision::Reject
        );
        let dup = BTreeSet::from(["b.rs".to_string()]);
        assert_eq!(evaluate(&unit("b.rs", "rust"), &cfg, &dup).decision, Decision::Reject);
        assert_eq!(
            evaluate(&unit("c.txt", "unknown"), &cfg, &BTreeSet::new()).decision,
            Decision::Reject
        );
    }

    #[test]
    fn soft_gates_defer_without_rejecting() {
        let cfg = Config {
            require_license: true,
            require_tests: true,
            ..Default::default()
        };
        let mut u = unit("a.rs", "rust");
        u.license = Some("MIT".into());
        assert_eq!(evaluate(&u, &cfg, &BTreeSet::new()).decision, Decision::Defer);
        u.tests.push("tests/a.rs".into());
        assert_eq!(evaluate(&u, &cfg, &BTreeSet::new()).decision, Decision::Accept);
    }

    #[test]
    fn strict_license_becomes_a_reject() {
        let cfg = Config {
            reject_missing_license: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate(&unit("a.rs", "rust"), &cfg, &BTreeSet::new()).decision,
            Decision::Reject
        );
    }

    #[test]
    fn duplicate_paths_keep_the_first_of_each_group() {
        let dup = Dup {
            exact: vec![crate::repo::dup::ExactGroup {
                hash: "h".into(),
                paths: vec!["a.rs".into(), "b.rs".into()],
                bytes: 10,
            }],
            near: vec![crate::repo::dup::NearGroup {
                paths: vec!["c.rs".into(), "d.rs".into(), "e.rs".into()],
                similarity: 0.9,
            }],
            ..Default::default()
        };
        let paths = duplicate_paths(&dup);
        assert!(paths.contains("b.rs") && paths.contains("d.rs") && paths.contains("e.rs"));
        assert!(!paths.contains("a.rs") && !paths.contains("c.rs"));
    }
}
