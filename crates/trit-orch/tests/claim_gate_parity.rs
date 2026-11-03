//! 认领闸门的行为对拍：Rust `ClaimStore::gate` 与 `scripts/agent_claims.py` 的 `gate()`
//! 读同一张表（`tests/fixtures/claim_gate_cases.json`）。
//!
//! 为什么要一张共享表：这套口径有两个消费者——库（`trit-orch`）和智能体每天敲的 CLI
//! （`agent_claims.py`）。两边各自写各自的测试时，同一个形状能给出两种答案
//! （已经发生过：两份 `agent_claims.py` 副本对「同仓路径不相交」一个拦一个只警）。
//! 表在仓库里，改口径必须同时改表、Rust 实现和 Python 实现；只改一边就在这里判红。

use std::collections::BTreeMap;

use serde::Deserialize;
use trit_core::Ternary;
use trit_orch::{Claim, ClaimStore};

const FIXTURE: &str = include_str!("fixtures/claim_gate_cases.json");

#[derive(Deserialize)]
struct Doc {
    spec: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    claims: Vec<RawClaim>,
    query: Query,
    expect: String,
    #[allow(dead_code)]
    why: String,
}

#[derive(Deserialize)]
struct RawClaim {
    id: String,
    owner: String,
    paths: Vec<String>,
    acquired_at: u64,
    expires_at: u64,
}

#[derive(Deserialize)]
struct Query {
    owner: String,
    paths: Vec<String>,
    now: u64,
}

fn ternary(label: &str) -> Ternary {
    match label {
        "true" => Ternary::True,
        "false" => Ternary::False,
        "unknown" => Ternary::Unknown,
        other => panic!("表里的 expect 不是三态之一：{other}"),
    }
}

fn store_of(case: &Case) -> ClaimStore {
    let claims = case
        .claims
        .iter()
        .map(|raw| Claim {
            id: raw.id.clone(),
            owner: raw.owner.clone(),
            paths: raw.paths.clone(),
            intent: String::new(),
            fencing: 0,
            acquired_at: raw.acquired_at,
            expires_at: raw.expires_at,
        })
        .collect();
    ClaimStore::from_claims(claims)
        .unwrap_or_else(|err| panic!("{} 的声明表建不起来：{err}", case.name))
}

/// 共享表逐条对账。
#[test]
fn claim_gate_matches_shared_table() {
    let doc: Doc = serde_json::from_str(FIXTURE).expect("共享表必须能解析");
    assert_eq!(
        doc.spec, "bfxh/trit claim-gate parity v1",
        "表版本变了要同步改两侧实现"
    );
    assert!(
        doc.cases.len() >= 12,
        "共享表只剩 {} 条，门在空转",
        doc.cases.len()
    );

    let mut mismatches = Vec::new();
    for case in &doc.cases {
        let want = ternary(&case.expect);
        let got = store_of(case)
            .gate(&case.query.paths, &case.query.owner, case.query.now)
            .unwrap_or_else(|err| panic!("{}：gate 直接报错 {err}", case.name));
        if got != want {
            mismatches.push(format!("{}：期望 {want:?}，实到 {got:?}", case.name));
        }
    }
    assert!(
        mismatches.is_empty(),
        "gate 与共享表不一致：\n{}",
        mismatches.join("\n")
    );
}

/// 表里的 `expect` 不能全是一种答案——那样任何实现都能过。
#[test]
fn shared_table_covers_all_three_verdicts() {
    let doc: Doc = serde_json::from_str(FIXTURE).expect("共享表必须能解析");
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for case in &doc.cases {
        *tally.entry(case.expect.clone()).or_default() += 1;
    }
    for verdict in ["true", "false", "unknown"] {
        assert!(
            tally.get(verdict).copied().unwrap_or_default() > 0,
            "共享表里一条 {verdict} 都没有，判据覆盖不到这一态"
        );
    }
    assert!(
        doc.cases.iter().all(|case| !case.why.trim().is_empty()),
        "每条用例都得写清它在钉什么"
    );
}
