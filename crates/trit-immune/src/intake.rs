//! 自动化 intake 管线：外部代码/PR 进来 → **自动扫（安全 + 死依赖）→ 自动隔离 →
//! 等智能体看见**。全程本地确定性，无 LLM。
//!
//! 数据源可插拔：GitHub webhook / 手动粘贴 / 文件 / CI 产物——都走同一条管线。
//! HTTPS 拉取需 TLS（诚实边界：接 rustls 时源接口不变）；本模块只做**管线**，
//! 不做网络 I/O。
//!
//! 死依赖检测：Cargo.toml `[dependencies]` 里的名字在代码 `.rs` 文件中
//! 找不到 `use` / `::` 引用 → `Dispositive`（不用就是不用，无需利用即证伪）。

use crate::journal::Journal;
use crate::scan;
use crate::trit::Trit;
use crate::NodeId;
use std::io;
use std::path::Path;

/// 一条死依赖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadDep {
    pub name: String,
    pub declared_at: String,
}

/// 从 Cargo.toml 提取 `[dependencies]` 段的依赖名。
pub fn parse_cargo_deps(toml_text: &str) -> Vec<(String, String)> {
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in toml_text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]";
            continue;
        }
        if in_deps && !t.is_empty() && !t.starts_with('#') {
            if let Some((name, _)) = t.split_once('=') {
                let name = name.trim().trim_matches('"').to_string();
                if !name.is_empty() {
                    out.push((name.clone(), line.trim().to_string()));
                }
            }
        }
    }
    out
}

/// 扫描代码片段，检测哪些依赖从未被引用。
pub fn find_dead_deps(cargo_toml: &str, rs_sources: &[(&str, &str)]) -> Vec<DeadDep> {
    let deps = parse_cargo_deps(cargo_toml);
    let all_code = rs_sources
        .iter()
        .map(|(_, code)| *code)
        .collect::<Vec<&str>>()
        .join("\n");
    deps.into_iter()
        .filter(|(name, _)| {
            !all_code.contains(&format!("use {name}"))
                && !all_code.contains(&format!("{name}::"))
                && !all_code.contains(&format!("extern crate {name}"))
        })
        .map(|(name, declared)| DeadDep {
            name,
            declared_at: declared,
        })
        .collect()
}

/// 自动 intake 管线的综合报告。
#[derive(Debug, Clone, Default)]
pub struct IntakeReport {
    pub scan_findings: usize,
    pub dead_deps: usize,
    pub created: usize,
    pub quarantined: usize,
}

/// 全自动管线：
/// ① 安全扫描（永不 +1）→ ② 死依赖检测（Dispositive）→ ③ 隔离入账 →
/// ④ 调度器可在 `suspects()` 看见 → 智能体处理。
pub fn intake_pr(
    journal: &mut Journal,
    parent: NodeId,
    source: &str,
    code_snippets: &[(u32, &str)],
    cargo_toml: Option<&str>,
    quarantine: bool,
) -> io::Result<IntakeReport> {
    let refs: Vec<(NodeId, &str)> = code_snippets
        .iter()
        .map(|(n, c)| (NodeId(*n), *c))
        .collect();
    let scan_report = scan::run_scan(&refs, scan::RULES, &[]);
    for d in scan_report.to_deltas(source) {
        journal
            .append_delta(&d)
            .map_err(|e| io::Error::other(e.to_string()))?;
    }

    let dead_deps = if let Some(toml) = cargo_toml {
        let rs: Vec<(&str, &str)> = code_snippets
            .iter()
            .map(|(n, c)| (format!("file-{n}").leak() as &str, *c))
            .collect();
        find_dead_deps(toml, &rs)
    } else {
        Vec::new()
    };

    for dd in &dead_deps {
        let node = journal
            .spawn(
                parent,
                crate::NodeKind::Message,
                &format!("死依赖: {}", dd.name),
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        journal
            .record(
                node,
                Trit::Vulnerable,
                None,
                &format!(
                    "Cargo.toml 声明了 {} 但代码未引用——{}",
                    dd.name, dd.declared_at
                ),
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
    }

    let mut q = 0usize;
    if quarantine {
        let to_quarantine: Vec<NodeId> = journal
            .dag()
            .nodes()
            .iter()
            .filter(|n| {
                n.id != parent
                    && !n.quarantined
                    && n.verdict != Trit::Hardened
                    && n.parent == Some(parent)
            })
            .map(|n| n.id)
            .collect();
        for id in to_quarantine {
            journal
                .quarantine(id, "外部代码自动隔离（intake 管线）")
                .map_err(|e| io::Error::other(e.to_string()))?;
            q += 1;
        }
    }

    Ok(IntakeReport {
        scan_findings: scan_report.findings.len(),
        dead_deps: dead_deps.len(),
        created: scan_report.findings.len() + dead_deps.len(),
        quarantined: q,
    })
}

/// 供应链审计入账：基线 vs 当前锁文件（[`crate::supply::audit`]），
/// 每条 finding 在 `parent` 下立一个**锚定攻击分支**（`NodeKind::AttackVector`
/// ——merge_gate 会折叠因果链上节点的锚定攻击分支，`-1` 因此收紧门禁；
/// 兄弟分支不殃及的语义不受影响）并按三值裁决入账。
/// `-1`（新依赖/checksum 变化）冻结门禁，`+1`（依赖移除）留痕。
/// 返回立账的 finding 条数。
pub fn intake_supply(
    journal: &mut Journal,
    parent: NodeId,
    source: &str,
    lock_now: &[crate::supply::Pin],
    baseline: &[crate::supply::Pin],
) -> io::Result<usize> {
    let mut n = 0usize;
    for f in crate::supply::audit(lock_now, baseline) {
        let node = journal
            .spawn(
                parent,
                crate::NodeKind::AttackVector,
                &format!("供应链: {} @ {}", f.name, f.version),
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        journal
            .record(node, f.verdict, None, &format!("{source}: {:?}", f.kind))
            .map_err(|e| io::Error::other(e.to_string()))?;
        n += 1;
    }
    Ok(n)
}

/// 扫描一个目录的 Cargo.toml + .rs 文件做死依赖检测。
pub fn scan_dir_for_dead_deps(dir: &Path) -> io::Result<Vec<DeadDep>> {
    let toml_path = dir.join("Cargo.toml");
    if !toml_path.exists() {
        return Ok(Vec::new());
    }
    let toml = std::fs::read_to_string(&toml_path)?;
    let mut rs_sources: Vec<(String, String)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if rs_sources.len() > 1000 {
            break;
        }
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                if name == "target" || name == ".git" || name.starts_with('.') {
                    continue;
                }
                stack.push(p);
            } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                rs_sources.push((
                    p.to_string_lossy().to_string(),
                    std::fs::read_to_string(&p)?,
                ));
            }
        }
    }
    let refs: Vec<(&str, &str)> = rs_sources
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    Ok(find_dead_deps(&toml, &refs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;

    #[test]
    fn finds_dead_dependencies() {
        let toml = "[dependencies]\nserde = \"1.0\"\ntokio = { version = \"1\" }\nunused-crate = \"0.1\"\n";
        let code = "use serde::Serialize;\nfn main() { tokio::runtime::Runtime::new(); }";
        let dead = find_dead_deps(toml, &[("main.rs", code)]);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].name, "unused-crate");
    }

    #[test]
    fn all_deps_used_means_no_dead() {
        let toml = "[dependencies]\nserde = \"1.0\"\n";
        let code = "use serde::Serialize;\n";
        assert!(find_dead_deps(toml, &[("m.rs", code)]).is_empty());
    }

    #[test]
    fn intake_pipeline_end_to_end() {
        let pa =
            std::env::temp_dir().join(format!("trit-immune-intake-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&pa);
        let mut j = Journal::create_as(&pa, "main", 0).unwrap();
        let inbox = j
            .spawn(NodeId(0), crate::NodeKind::Message, "收件箱")
            .unwrap();
        j.into_dag().unwrap();

        let mut j = Journal::open_as(&pa, 0).unwrap();
        let snippets = vec![
            (1u32, "use serde::Serialize;\ndb.password = \"hunter2\";"),
            (2u32, "use tokio::runtime;\nfn clean() {}"),
        ];
        let toml =
            "[dependencies]\nserde = \"1.0\"\ntokio = { version = \"1\" }\nphantom-dep = \"0.1\"\n";
        let report = intake_pr(&mut j, inbox, "github/pr/42", &snippets, Some(toml), true).unwrap();
        assert!(report.scan_findings > 0, "硬编码凭据应被扫到");
        assert_eq!(report.dead_deps, 1, "phantom-dep 应被判死");
        assert!(report.quarantined > 0, "外部代码应自动隔离");

        // 死依赖节点存在且为 -1（Dispositive）
        let dead_node = j
            .dag()
            .nodes()
            .iter()
            .find(|n| n.title.contains("phantom-dep"))
            .unwrap();
        assert_eq!(dead_node.verdict, Trit::Vulnerable);

        // 门禁 BLOCK（收件箱子树有 -1）
        assert_eq!(j.dag().merge_gate(inbox), crate::GateDecision::Block);

        j.into_dag().unwrap();
        let _ = std::fs::remove_file(&pa);
    }
}
