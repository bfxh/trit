//! 供应链完整性：依赖锁定与基线比对（GOAL.md 升级②）。
//!
//! 威胁模型：**依赖投毒**（crate 被换成恶意版本）、**dependency confusion**
//! （内网名被公网抢注）、**锁文件篡改**（构建时注入新依赖）。
//!
//! 内核零网络、零第三方——做确定性可复算的部分：
//!
//! 1. [`parse_lock`]：解析 `Cargo.lock` 的 `[[package]]` 段（name/version/checksum）；
//! 2. [`audit`]：当前锁文件 vs 基线清单 → 三值 findings。裁决协议：
//!    - 新增依赖 → **`-1`**（新攻击面未审计，与"新代码默认 0/未测即不可信"同构）；
//!    - 已锁定包 checksum 变化 → **`-1`**（投毒信号，Dispositive）；
//!    - 依赖消失 → `+1`（攻击面缩减的机械事实——这是审计结论，不是扫描器洗白）；
//!    - 无变化 → 无 finding（管线层 Clean）；
//! 3. [`fingerprint`]：对 Cargo.toml 声明文件做 SHA-256 指纹——上次审计以来的
//!    依赖声明变化可被检测（防"半夜往清单里塞一行"）；
//! 4. 基线清单是纯文本 TSV（[`encode_baseline`]/[`parse_baseline`]），可提交进版本
//!    控制，diff 可读。
//!
//! findings 不自动挂 DAG——调用方（intake 管线/server）拿到三值裁决后走
//! `Delta::Record` 入账，与扫描器同一条日志路径。`-1` finding 挂到对应依赖节点
//! 或仓节点上，门禁自然冻结。

use crate::auth::sha256;
use crate::trit::Trit;

/// 一个被锁定的依赖包（Cargo.lock `[[package]]` 的最小骨架）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub version: String,
    /// registry 来源的包有 checksum；path/git/workspace 成员为 None。
    pub checksum: Option<String>,
}

/// 供应链差异类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyKind {
    /// 基线里没有的 (name, version)——新攻击面。
    Added,
    /// (name, version) 相同但 checksum 变了——投毒信号。
    ChecksumChanged,
    /// 基线里有、当前锁文件没有——攻击面缩减。
    Removed,
}

impl SupplyKind {
    /// 三值裁决（与 GOAL.md "新代码默认 0/未测即不可信" 同构）。
    pub fn verdict(self) -> Trit {
        match self {
            SupplyKind::Added => Trit::Vulnerable,
            SupplyKind::ChecksumChanged => Trit::Vulnerable,
            SupplyKind::Removed => Trit::Hardened,
        }
    }
}

/// 一条供应链差异：包 + 类型 + 三值裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplyFinding {
    pub name: String,
    pub version: String,
    pub kind: SupplyKind,
    pub verdict: Trit,
}

/// 解析 Cargo.lock 文本，抽出全部 `[[package]]` 段的 name/version/checksum。
///
/// 宽容解析：段内只认 `name =`、`version =`、`checksum =` 开头的行，
/// `dependencies = [...]` 多行数组等其余内容忽略；`#` 注释跳过。
pub fn parse_lock(text: &str) -> Vec<Pin> {
    let mut pins: Vec<Pin> = Vec::new();
    let mut in_pkg = false;
    let mut name = String::new();
    let mut version = String::new();
    let mut checksum: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("#") || line.is_empty() {
            continue;
        }
        if line.starts_with("[[package]]") {
            // 新段开始：先落上一段
            if in_pkg && !name.is_empty() {
                pins.push(Pin {
                    name: std::mem::take(&mut name),
                    version: std::mem::take(&mut version),
                    checksum: checksum.take(),
                });
            }
            in_pkg = true;
            name.clear();
            version.clear();
            checksum = None;
            continue;
        }
        if line.starts_with('[') {
            // 其它段（[[patch]]、[metadata] 等）：落上一段并退出包段
            if in_pkg && !name.is_empty() {
                pins.push(Pin {
                    name: std::mem::take(&mut name),
                    version: std::mem::take(&mut version),
                    checksum: checksum.take(),
                });
            }
            in_pkg = false;
            name.clear();
            version.clear();
            checksum = None;
            continue;
        }
        if !in_pkg {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = strip_toml_str(rest) {
                name = v;
            }
        } else if let Some(rest) = line.strip_prefix("version") {
            if let Some(v) = strip_toml_str(rest) {
                version = v;
            }
        } else if let Some(rest) = line.strip_prefix("checksum") {
            if let Some(v) = strip_toml_str(rest) {
                checksum = Some(v);
            }
        }
    }
    if in_pkg && !name.is_empty() {
        pins.push(Pin {
            name,
            version,
            checksum,
        });
    }
    pins
}

/// 剥 TOML 字符串值：` = "0.1.0"` → `0.1.0`。非 `= "..."` 形态返回 None。
fn strip_toml_str(after_key: &str) -> Option<String> {
    let rest = after_key.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 基线 vs 当前锁文件比对。主键 = (name, version)（Cargo.lock 允许同名多版本并存）。
pub fn audit(lock: &[Pin], baseline: &[Pin]) -> Vec<SupplyFinding> {
    let key = |p: &Pin| (p.name.clone(), p.version.clone());
    let mut base: std::collections::BTreeMap<(String, String), &Pin> =
        std::collections::BTreeMap::new();
    for p in baseline {
        base.entry(key(p)).or_insert(p);
    }
    let mut now: std::collections::BTreeMap<(String, String), &Pin> =
        std::collections::BTreeMap::new();
    for p in lock {
        now.entry(key(p)).or_insert(p);
    }

    let mut out = Vec::new();
    for (k, cur) in &now {
        match base.get(k) {
            None => out.push(SupplyFinding {
                name: k.0.clone(),
                version: k.1.clone(),
                kind: SupplyKind::Added,
                verdict: SupplyKind::Added.verdict(),
            }),
            Some(prev) => {
                if prev.checksum != cur.checksum {
                    out.push(SupplyFinding {
                        name: k.0.clone(),
                        version: k.1.clone(),
                        kind: SupplyKind::ChecksumChanged,
                        verdict: SupplyKind::ChecksumChanged.verdict(),
                    });
                }
            }
        }
    }
    for k in base.keys() {
        if !now.contains_key(k) {
            out.push(SupplyFinding {
                name: k.0.clone(),
                version: k.1.clone(),
                kind: SupplyKind::Removed,
                verdict: SupplyKind::Removed.verdict(),
            });
        }
    }
    // 确定性顺序：先按类型再按名字（Added/ChecksumChanged 在前，Removed 在后）
    out.sort_by(|a, b| {
        (a.kind as usize)
            .cmp(&(b.kind as usize))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
    });
    out
}

/// 基线清单编码（TSV，`#` 注释头）。checksum 缺失时第三列为空。
pub fn encode_baseline(pins: &[Pin]) -> String {
    let mut s = String::from("# trit-immune supply baseline v1\n# name\\tversion\\tchecksum\n");
    for p in pins {
        s.push_str(&p.name);
        s.push('\t');
        s.push_str(&p.version);
        s.push('\t');
        s.push_str(p.checksum.as_deref().unwrap_or(""));
        s.push('\n');
    }
    s
}

/// 解析基线清单（[`encode_baseline`] 的逆，宽容：空行/注释跳过，列数不足丢弃）。
pub fn parse_baseline(text: &str) -> Vec<Pin> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut it = line.split('\t');
        let (Some(name), Some(version)) = (it.next(), it.next()) else {
            continue;
        };
        let checksum = it.next().filter(|c| !c.is_empty()).map(|c| c.to_string());
        if name.is_empty() || version.is_empty() {
            continue;
        }
        out.push(Pin {
            name: name.to_string(),
            version: version.to_string(),
            checksum,
        });
    }
    out
}

/// 声明文件指纹：SHA-256 的十六进制小写。调用方保存上次指纹即可检测清单篡改。
pub fn fingerprint(bytes: &[u8]) -> String {
    let d = sha256(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_toml_str_forms() {
        assert_eq!(strip_toml_str(" = \"0.1.0\""), Some("0.1.0".into()));
        assert_eq!(strip_toml_str("=\"x\""), Some("x".into()));
        assert_eq!(strip_toml_str(" = 3"), None);
        assert_eq!(strip_toml_str(""), None);
    }

    #[test]
    fn parse_lock_minimal() {
        let text = "# comment\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nchecksum = \"aa\"\n\n[[package]]\nname = \"b\"\nversion = \"2.0.0\"\n";
        let pins = parse_lock(text);
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "a");
        assert_eq!(pins[0].checksum.as_deref(), Some("aa"));
        assert_eq!(pins[1].name, "b");
        assert_eq!(pins[1].checksum, None);
    }

    #[test]
    fn fingerprint_changes_with_input() {
        assert_eq!(fingerprint(b"x"), fingerprint(b"x"));
        assert_ne!(fingerprint(b"x"), fingerprint(b"y"));
    }
}
