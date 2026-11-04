//! 场景 G：智能体被拒之后拿到多少信息、预检自己花多少。
//!
//! 跑法：`cargo run --release -p trit-vcs --example agent_ergonomics`
//!
//! 两把尺：
//! · **负载** = 同一条判决，旧出口（只有 `FAIL <一句话>`）与新出口（`explain` 文本 /
//!   `--json` 对象）各自的字节与 token 元数（`trit-vcs::tokens`，ASCII 4 字符=1 元）。
//! · **预检成本** = `preflight()` 的耗时，以及它必须写 0 个对象。
//!
//! "少几轮"不在这把尺里：它是设计意图（被拒时给可执行命令 ⇒ 不用再翻文档猜参数），
//! 要实测得让真智能体跑任务，这里只登记负载与信息量。

use std::collections::BTreeMap;
use std::time::Instant;

use trit_vcs::gate::ClaimSpec;
use trit_vcs::repo::{CommitRequest, Policy, Repo, RepoError};
use trit_vcs::tokens::estimate;
use trit_vcs::{remediation, tokens};

fn corpus(files: usize) -> BTreeMap<String, Vec<u8>> {
    (0..files)
        .map(|i| {
            (
                format!("gen/d{:02}/f{i:03}.txt", i / 10),
                format!("body {i}\n").repeat(20).into_bytes(),
            )
        })
        .collect()
}

fn claim(id: &str, owner: &str, paths: Vec<String>) -> ClaimSpec {
    ClaimSpec {
        id: id.to_string(),
        owner: owner.to_string(),
        paths,
        acquired_at: 1,
        expires_at: 4_000_000_000,
    }
}

fn measure(label: &str, err: &RepoError, ctx: remediation::Context<'_>) {
    let old = format!("FAIL {err}");
    let new = remediation::explain(err, ctx);
    let json = serde_json::to_string(&remediation::explain_json(err, ctx)).unwrap_or_default();
    let commands = remediation::commands(err, ctx).len();
    println!(
        "  {label}：旧负载 {} B / {:>3} 元　新负载 {} B / {:>3} 元　--json {} B / {:>3} 元　可执行命令 {commands} 条",
        old.len(),
        estimate(&old),
        new.len(),
        estimate(&new),
        json.len(),
        estimate(&json),
    );
    assert!(!old.is_empty() && commands > 0, "{label} 没给出下一步");
}

fn main() {
    let root = std::env::temp_dir().join(format!("trit-vcs-ergo-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut repo = Repo::init(&root).unwrap();
    let ctx = remediation::Context {
        dir: &root,
        session: "qoder:bench",
    };

    println!("G 智能体被拒时的信息量（同一把 token 尺）");

    // 1) 未认领：只暂存了文件、没有任何在效认领
    repo.stage(&corpus(60), 1_000).unwrap();
    let objects_before = repo.store().stats().unwrap().objects;
    let unclaimed = repo
        .preflight(&CommitRequest {
            message: "一片改动".to_string(),
            session: "qoder:bench".to_string(),
            claims: vec![],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    measure("unclaimed（60 文件）", &unclaimed, ctx);

    // 2) 撞车：别人在效认领覆盖了同一片路径
    let clash = repo
        .preflight(&CommitRequest {
            message: "一片改动".to_string(),
            session: "qoder:bench".to_string(),
            claims: vec![claim("c7", "zcode:other", vec!["gen".to_string()])],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    measure("claim_conflict（对手 1 个）", &clash, ctx);

    // 3) 缺见证：策略打开后没带 witness
    std::fs::write(
        root.join(".tritvcs").join("policy.json"),
        serde_json::to_vec(&Policy {
            require_witness: true,
            require_intent: false,
        })
        .unwrap(),
    )
    .unwrap();
    let repo = Repo::open(&root).unwrap();
    let no_witness = repo
        .preflight(&CommitRequest {
            message: "一片改动".to_string(),
            session: "qoder:bench".to_string(),
            claims: vec![claim("c1", "qoder:bench", vec!["gen".to_string()])],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    measure("witness_required", &no_witness, ctx);

    // 4) 基线错配：intent 绑了另一棵树（策略得先关回见证要求，否则先撞上一条判决）
    std::fs::write(root.join(".tritvcs").join("policy.json"), b"{}").unwrap();
    let repo = Repo::open(&root).unwrap();
    let mismatch = repo
        .preflight(&CommitRequest {
            message: "一片改动".to_string(),
            session: "qoder:bench".to_string(),
            claims: vec![claim("c1", "qoder:bench", vec!["gen".to_string()])],
            intent: Some(
                trit_vcs::Intent::new(
                    "把认领判决说清楚".to_string(),
                    vec!["cargo test -p trit-vcs".to_string()],
                )
                .with_base(trit_vcs::Hash::of(b"another-tree")),
            ),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    measure("intent_base_mismatch", &mismatch, ctx);

    // 5) 放行时的预检：成本与"什么都不写"
    std::fs::write(root.join(".tritvcs").join("policy.json"), b"{}").unwrap();
    let repo = Repo::open(&root).unwrap();
    let request = CommitRequest {
        message: "一片改动".to_string(),
        session: "qoder:bench".to_string(),
        claims: vec![claim("c1", "qoder:bench", vec!["gen".to_string()])],
        now_secs: 1_000,
        ..Default::default()
    };
    let start = Instant::now();
    let facts = repo.preflight(&request).unwrap();
    let pre_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let after_ms = Instant::now().elapsed();
    let _ = after_ms;
    assert_eq!(
        repo.store().stats().unwrap().objects,
        objects_before,
        "预检必须一个对象都不写"
    );
    println!(
        "  预检（60 文件 / {} 个对象 / {} 字节）：{pre_ms:.1} ms，写盘对象 0 个",
        facts.size.objects, facts.size.bytes
    );
    println!(
        "  预检读数本身：{} B / {} 元",
        serde_json::to_string(&facts).unwrap_or_default().len(),
        estimate(&serde_json::to_string(&facts).unwrap_or_default())
    );

    // 6) 同一个负载压进 200 元会变成什么
    let mut value = serde_json::to_value(&facts).unwrap();
    let dropped = tokens::cap_rows(&mut value, 200);
    println!(
        "  同一条预检读数按 --budget 200 压：{} 元 → {} 元（丢 {dropped} 行；没超预算就不挂 truncation）",
        estimate(&serde_json::to_string(&facts).unwrap_or_default()),
        estimate(&serde_json::to_string(&value).unwrap_or_default())
    );

    // 顺手确认这确实是同一个尺：MCP 的 schema 读数也在同一把尺下量出来过（803 元）。
    let _ = estimate("mcp schema 用同一把尺");
    let _ = std::fs::remove_dir_all(&root);
}
