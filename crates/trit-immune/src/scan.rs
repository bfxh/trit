//! 静态扫描路线：把传统 SAST/规则引擎的原始告警接成三值裁决源。
//!
//! 校准协议只有一条不对称规则：**扫描器永远不产生 `+1`**。
//!
//! - `Dispositive`（规则即漏洞本体：硬编码凭据、关闭 TLS 校验……）→ `-1`，直接证伪；
//! - `Heuristic`（可疑模式：eval、XSS sink、SQL 拼接……）→ `0`，冻结并进红队队列；
//! - 未命中 → 不产生任何记录：清白的扫描只是"没发现"，不等于"安全"；
//!   `+1` 只能来自红队拦截、沙盒通过或人工裁定。
//!
//! 多条命中按 K3 AND 折叠（最坏者胜），产出 [`Delta::Record`](crate::Delta) 走
//! `apply`/日志同一条路径。内置匹配器是演示级的确定性子串规则；接真实引擎
//! （Semgrep/CodeQL 等）时替换 `Rule` 的匹配函数即可，校准协议不变。

use crate::dag::NodeId;
use crate::delta::Delta;
use crate::trit::Trit;
use std::collections::BTreeMap;

/// 证据等级：规则命中是否即漏洞本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// 命中即证伪（硬编码凭据、关闭 TLS 校验……），无需利用。
    Dispositive,
    /// 可疑模式，只值一个 `0`，交给红队/沙盒升级。
    Heuristic,
}

/// 一条扫描规则（函数指针不做相等比较，故不派生 `PartialEq`）。
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub id: &'static str,
    pub evidence: Evidence,
    pub matches: fn(code: &str) -> bool,
}

/// 一条校准后的发现。`calibrated` 只会是 `-1` 或 `0`，永不 `+1`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub node: NodeId,
    pub excerpt: String,
    pub calibrated: Trit,
}

/// 一次扫描的报告。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanReport {
    pub findings: Vec<Finding>,
    pub scanned: usize,
    pub suppressed: usize,
}

/// 按节点折叠后的裁决（K3 AND：最坏者胜；无命中的节点不在表里——扫描器不发 `+1`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFold {
    pub node: NodeId,
    pub verdict: Trit,
    pub rules: Vec<&'static str>,
}

/// 内置演示规则集：确定性子串匹配（真实部署换 AST/污点引擎，校准协议不变）。
/// 多语言覆盖：Python / JavaScript / Go / Java / SQL / Shell / Rust。
pub const RULES: &[Rule] = &[
    Rule {
        id: "TRI001-HARDCODED-SECRET",
        evidence: Evidence::Dispositive,
        matches: r_hardcoded_secret,
    },
    Rule {
        id: "TRI002-INSECURE-TLS",
        evidence: Evidence::Dispositive,
        matches: r_insecure_tls,
    },
    Rule {
        id: "TRI003-EVAL",
        evidence: Evidence::Heuristic,
        matches: r_eval,
    },
    Rule {
        id: "TRI004-XSS-SINK",
        evidence: Evidence::Heuristic,
        matches: r_xss_sink,
    },
    Rule {
        id: "TRI005-SQL-CONCAT",
        evidence: Evidence::Heuristic,
        matches: r_sql_concat,
    },
    Rule {
        id: "TRI006-COMMAND-INJECTION",
        evidence: Evidence::Heuristic,
        matches: r_command_injection,
    },
    Rule {
        id: "TRI007-DESERIALIZE",
        evidence: Evidence::Heuristic,
        matches: r_deserialize,
    },
];

fn contains_any(code: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| code.contains(n))
}

fn r_hardcoded_secret(code: &str) -> bool {
    contains_any(
        code,
        &[
            "password = \"",
            "api_key = \"",
            "secret = \"",
            "PRIVATE KEY",
        ],
    )
}

fn r_insecure_tls(code: &str) -> bool {
    contains_any(
        code,
        &["InsecureSkipVerify: true", "verify = False", "CERT_NONE"],
    )
}

fn r_eval(code: &str) -> bool {
    contains_any(code, &["eval(", "exec("])
}

fn r_xss_sink(code: &str) -> bool {
    contains_any(code, &["innerHTML =", "dangerouslySetInnerHTML"])
}

fn r_sql_concat(code: &str) -> bool {
    code.contains("SELECT") && (code.contains('+') || code.contains("format!"))
}

fn r_command_injection(code: &str) -> bool {
    contains_any(
        code,
        &[
            "os.system(",
            "subprocess.call(",
            "subprocess.Popen(",
            "child_process.exec",
            "Command::new(",
            "Runtime.getRuntime().exec(",
            "os/exec.Run(",
            "exec.Command(",
        ],
    )
}

fn r_deserialize(code: &str) -> bool {
    contains_any(
        code,
        &[
            "pickle.loads(",
            "yaml.load(",
            "deserialize(",
            "ObjectInputStream",
            "unsafe.Deserialize",
        ],
    )
}

fn clip(code: &str) -> String {
    let t = code.trim();
    let mut s: String = t.chars().take(120).collect();
    if t.chars().count() > 120 {
        s.push('…');
    }
    s
}

/// 扫描一组代码片段（调用方持有每个节点的代码）。
/// `suppressed` 里的节点整点跳过（范围外/已豁免）——跳过不是"洗白"，不产生任何记录。
pub fn run_scan(snippets: &[(NodeId, &str)], rules: &[Rule], suppressed: &[NodeId]) -> ScanReport {
    let mut findings = Vec::new();
    let mut suppressed_count = 0usize;
    for (node, code) in snippets {
        if suppressed.contains(node) {
            suppressed_count += 1;
            continue;
        }
        for rule in rules {
            if (rule.matches)(code) {
                findings.push(Finding {
                    rule: rule.id,
                    node: *node,
                    excerpt: clip(code),
                    calibrated: match rule.evidence {
                        Evidence::Dispositive => Trit::Vulnerable,
                        Evidence::Heuristic => Trit::Unknown,
                    },
                });
            }
        }
    }
    ScanReport {
        scanned: snippets.len() - suppressed_count,
        suppressed: suppressed_count,
        findings,
    }
}

impl ScanReport {
    /// 按节点折叠：K3 AND（任一 Dispositive → `-1`；否则任一 Heuristic → `0`）。
    pub fn fold_by_node(&self) -> Vec<NodeFold> {
        let mut acc: BTreeMap<u32, (Trit, Vec<&'static str>)> = BTreeMap::new();
        for f in &self.findings {
            let e = acc.entry(f.node.0).or_insert((Trit::Hardened, Vec::new()));
            e.0 = e.0.and(f.calibrated);
            e.1.push(f.rule);
        }
        acc.into_iter()
            .map(|(id, (verdict, rules))| NodeFold {
                node: NodeId(id),
                verdict,
                rules,
            })
            .collect()
    }

    /// 折叠成 `Delta::Record` 序列——走 `apply`/日志同一条代码路径。
    pub fn to_deltas(&self, source: &str) -> Vec<Delta> {
        self.fold_by_node()
            .into_iter()
            .map(|f| Delta::Record {
                id: f.node,
                verdict: f.verdict,
                evidence: None,
                note: format!("静态扫描[{source}]：{}", f.rules.join(" + ")),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_never_emits_hardened() {
        let report = run_scan(&[(NodeId(1), "let x = compute(a, b);")], RULES, &[]);
        assert!(report.findings.is_empty());
        assert_eq!(report.scanned, 1);
        assert_eq!(report.suppressed, 0);
        assert!(report.fold_by_node().is_empty(), "无命中的节点不该出现");
    }
}
