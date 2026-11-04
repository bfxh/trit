//! 认领门：没有在效认领覆盖的路径，**默认拒绝提交**。
//!
//! 判据直接复用 `trit-orch` 的 `ClaimStore`（不另写一套规则，避免第二个"共享表"分叉）：
//! - `True`  ⇒ 我有在效认领覆盖全部路径；
//! - `Unknown` ⇒ 没人声明过 ⇒ 这里按"未认领"处理，默认拒；
//! - `False` ⇒ 别人在效且相交 ⇒ 冲突，拒，并点名对方。

use serde::{Deserialize, Serialize};
use trit_orch::{Claim, ClaimStore, paths_overlap};

/// 认领规格（与 `CLAIMS.md` 的字段一一对应；时间用 Unix 秒）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimSpec {
    pub id: String,
    pub owner: String,
    #[serde(default)]
    pub paths: Vec<String>,
    pub acquired_at: u64,
    pub expires_at: u64,
}

impl ClaimSpec {
    pub fn as_claim(&self) -> Claim {
        Claim {
            id: self.id.clone(),
            owner: self.owner.clone(),
            paths: self.paths.clone(),
            intent: String::new(),
            fencing: 0,
            acquired_at: self.acquired_at,
            expires_at: self.expires_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 我有在效认领覆盖全部路径。
    Allowed,
    /// 没人声明过（或只覆盖了一部分）。默认拒，除非显式放行。
    Unclaimed,
    /// 别人在效且相交。
    Conflict { others: Vec<String> },
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Unclaimed => "unclaimed",
            Self::Conflict { .. } => "conflict",
        }
    }

    pub fn allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// 判定一次提交能不能落。
pub fn check(claims: &[ClaimSpec], owner: &str, paths: &[String], now: u64) -> Verdict {
    let store = match ClaimStore::from_claims(claims.iter().map(ClaimSpec::as_claim).collect()) {
        Ok(store) => store,
        Err(_) => {
            // 认领表本身不合法（同名 id 等）⇒ 不敢放行
            return Verdict::Unclaimed;
        }
    };
    let mut others: Vec<String> = Vec::new();
    for claim in claims {
        if claim.expires_at <= now || claim.owner == owner {
            continue;
        }
        let overlaps = claim
            .paths
            .iter()
            .any(|scope| paths.iter().any(|path| paths_overlap(scope, path)));
        if overlaps && !others.contains(&claim.owner) {
            others.push(claim.owner.clone());
        }
    }
    if !others.is_empty() {
        others.sort();
        return Verdict::Conflict { others };
    }
    match store.gate(paths, owner, now) {
        Ok(trit_core::Ternary::True) => Verdict::Allowed,
        _ => Verdict::Unclaimed,
    }
}

/// 从 JSON 文件读认领表（数组）。文件不存在返回空表 + 原因。
pub fn load_claims(path: &std::path::Path) -> Result<Vec<ClaimSpec>, String> {
    if !path.is_file() {
        return Err(format!("认领表读不到：{}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("认领表不是合法 JSON 数组：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: &str, owner: &str, paths: &[&str], expires_at: u64) -> ClaimSpec {
        ClaimSpec {
            id: id.to_string(),
            owner: owner.to_string(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            acquired_at: 1,
            expires_at,
        }
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn own_live_claim_allows() {
        let claims = vec![claim("c1", "qoder:s1", &["crates/trit-vcs"], 100)];
        assert_eq!(
            check(
                &claims,
                "qoder:s1",
                &paths(&["crates/trit-vcs/src/x.rs"]),
                50
            ),
            Verdict::Allowed
        );
    }

    #[test]
    fn no_claim_is_unclaimed_not_allowed() {
        assert_eq!(
            check(&[], "qoder:s1", &paths(&["a.rs"]), 50),
            Verdict::Unclaimed,
            "没人声明过不许当成可以写"
        );
    }

    #[test]
    fn partial_coverage_is_unclaimed() {
        let claims = vec![claim("c1", "qoder:s1", &["a.rs"], 100)];
        assert_eq!(
            check(&claims, "qoder:s1", &paths(&["a.rs", "b.rs"]), 50),
            Verdict::Unclaimed
        );
    }

    #[test]
    fn other_live_claim_conflicts_and_is_named() {
        let claims = vec![claim("c1", "zcode:zoder", &["crates/trit-vcs"], 100)];
        assert_eq!(
            check(
                &claims,
                "qoder:s1",
                &paths(&["crates/trit-vcs/src/lib.rs"]),
                50
            ),
            Verdict::Conflict {
                others: vec!["zcode:zoder".to_string()]
            }
        );
    }

    #[test]
    fn expired_other_claim_does_not_conflict() {
        let claims = vec![claim("c1", "zcode:zoder", &["a.rs"], 40)];
        assert_eq!(
            check(&claims, "qoder:s1", &paths(&["a.rs"]), 50),
            Verdict::Unclaimed
        );
    }

    #[test]
    fn duplicate_ids_fail_closed() {
        let claims = vec![
            claim("c1", "qoder:s1", &["a.rs"], 100),
            claim("c1", "qoder:s2", &["a.rs"], 100),
        ];
        assert_eq!(
            check(&claims, "qoder:s1", &paths(&["a.rs"]), 50),
            Verdict::Unclaimed,
            "认领表自身不合法时不许放行"
        );
    }

    #[test]
    fn boundary_expiry_is_exclusive() {
        let claims = vec![claim("c1", "qoder:s1", &["a.rs"], 50)];
        assert_eq!(
            check(&claims, "qoder:s1", &paths(&["a.rs"]), 50),
            Verdict::Unclaimed
        );
    }
}
