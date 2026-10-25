//! unsafe 站点识别 + 三值判定。
//!
//! 判定用 K3（复用 `trit-core` 的 `Algebra::Kleene`）：
//! `Safe = False`（未发现风险证据）、`Unknown = Unknown`、`Risky = True`。
//! 合成用 K3 的「或」（取证据最强的那个）：`F ∨ U = U`、`U ∨ T = T`。
//!
//! **这不是证明**：它只检查"有没有常见的防护证据"（`// SAFETY:` 说明、断言/边界检查）。
//! `Safe` 的含义是"该有的证据都在"，不是"证过了没 UB"。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use trit_core::Algebra;
use trit_core::value::Ternary;

use crate::lexer::scrub;

/// 常见的高危 unsafe API / 形态。
const RISKY_APIS: [(&str, &str); 14] = [
    ("transmute", "类型双关"),
    ("from_raw_parts", "裸指针造切片"),
    ("get_unchecked", "越界检查被跳过"),
    ("set_len", "手动改长度"),
    ("assume_init", "未初始化内存当已初始化"),
    ("copy_nonoverlapping", "裸内存拷贝"),
    ("ptr::read", "裸指针读"),
    ("ptr::write", "裸指针写"),
    ("static mut", "可变静态"),
    ("extern \"C\"", "FFI 边界"),
    ("unreachable_unchecked", "承诺不可达"),
    ("new_unchecked", "跳过有效性检查"),
    ("align_to", "对齐相关的重解释"),
    ("offset_from", "指针距离（同分配约束）"),
];

const GUARDS: [&str; 16] = [
    "assert",
    "debug_assert",
    "is_null",
    "checked_",
    "< len",
    "<= len",
    "> len",
    ">= len",
    "len()",
    "align",
    "try_into",
    "get(0..",
    "if let Some",
    "matches!",
    "is_empty",
    "saturating_",
];

#[derive(Debug, Clone)]
pub struct Site {
    pub file: String,
    /// src / tests（测试、示例、基准里的 unsafe 单列，不参与 strict 判定）
    pub context: &'static str,
    pub line: usize,
    pub kind: &'static str,
    pub apis: Vec<&'static str>,
    pub documented: bool,
    pub guarded: bool,
    pub verdict: Ternary,
    pub snippet: String,
}

impl Site {
    pub fn reasons(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.apis.is_empty() {
            out.push(format!(
                "高危形态：{}",
                self.apis
                    .iter()
                    .map(|api| RISKY_APIS
                        .iter()
                        .find(|(name, _)| name == api)
                        .map(|(_, why)| format!("{api}（{why}）"))
                        .unwrap_or_else(|| api.to_string()))
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if !self.documented {
            out.push("没有 // SAFETY: 说明".to_string());
        }
        if !self.guarded {
            out.push("附近没有断言/边界/空指针检查".to_string());
        }
        out
    }
}

fn find_word(line: &str, word: &str) -> Option<usize> {
    let mut start = 0usize;
    while let Some(position) = line[start..].find(word) {
        let at = start + position;
        let before_ok = at == 0
            || !line[..at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        let after = at + word.len();
        let after_ok = after >= line.len()
            || !line[after..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        if before_ok && after_ok {
            return Some(at);
        }
        start = at + word.len();
    }
    None
}

fn classify_kind(rest: &str) -> Option<&'static str> {
    let trimmed = rest.trim_start();
    if trimmed.starts_with('{') {
        Some("block")
    } else if trimmed.starts_with("fn ") || trimmed.starts_with("fn\t") {
        Some("fn")
    } else if trimmed.starts_with("impl") {
        Some("impl")
    } else if trimmed.starts_with("trait") {
        Some("trait")
    } else {
        None
    }
}

/// K3「或」：取证据更强的那个。
pub fn join(a: Ternary, b: Ternary) -> Ternary {
    Algebra::Kleene.or(a, b)
}

pub fn verdict_of(documented: bool, guarded: bool, apis: &[&'static str], kind: &str) -> Ternary {
    // 签名级 unsafe（fn/impl/trait）只是「承诺面」，本工具不会把它判成 Risky；
    // 未文档化的签名单独计数（undocumented_signatures），不混进风险数。
    if matches!(kind, "fn" | "impl" | "trait") {
        return Ternary::Unknown;
    }
    if apis.is_empty() {
        return Ternary::Unknown;
    }
    if documented && guarded {
        return Ternary::False; // 证据齐
    }
    if !documented && !guarded {
        return Ternary::True; // 高危形态 + 两条证据都没有
    }
    Ternary::Unknown
}
pub fn scan_source(text: &str, label: &str) -> Vec<Site> {
    let cleaned = scrub(text);
    let raw: Vec<&str> = text.lines().collect();
    let mut sites = Vec::new();
    for (index, line) in cleaned.iter().enumerate() {
        let Some(offset) = find_word(line, "unsafe") else {
            continue;
        };
        let rest = &line[offset + "unsafe".len()..];
        let Some(kind) = classify_kind(rest) else {
            continue;
        };
        // 高危形态：本行 + 后 3 行（调用常跨行）
        let window_end = (index + 4).min(cleaned.len());
        let window = cleaned[index..window_end].join("\n");
        let apis: Vec<&'static str> = RISKY_APIS
            .iter()
            .filter(|(name, _)| window.contains(name))
            .map(|(name, _)| *name)
            .collect();
        // 防护：前 6 行 ~ 后 12 行
        let guard_start = index.saturating_sub(15);
        let guard_end = (index + 21).min(cleaned.len());
        let guard_window = cleaned[guard_start..guard_end].join("\n");
        let guarded = GUARDS.iter().any(|guard| guard_window.contains(guard));
        // 说明：本行及往上 8 行的原始注释里找 SAFETY:
        let doc_start = index.saturating_sub(30);
        let documented = raw[doc_start..=index.min(raw.len().saturating_sub(1))]
            .iter()
            .map(|line| line.trim_start())
            .filter(|line| {
                line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
            })
            .any(|line| line.contains("SAFETY") || line.contains("Safety"));
        let verdict = verdict_of(documented, guarded, &apis, kind);
        let lower = label.to_ascii_lowercase();
        let context = if lower.contains("/tests/")
            || lower.contains("/benches/")
            || lower.contains("/examples/")
            || lower.contains("\\tests\\")
            || lower.contains("\\benches\\")
            || lower.contains("\\examples\\")
        {
            "tests"
        } else {
            "src"
        };
        sites.push(Site {
            file: label.to_string(),
            context,
            line: index + 1,
            kind,
            apis,
            documented,
            guarded,
            verdict,
            snippet: raw
                .get(index)
                .map(|line| line.trim().chars().take(120).collect())
                .unwrap_or_default(),
        });
    }
    sites
}

pub fn scan_file(path: &Path, label: &str) -> Result<Vec<Site>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读 {} 失败（非 UTF-8？）", path.display()))?;
    Ok(scan_source(&text, label))
}

fn is_skipped(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        name == "target" || name == ".git" || name == "node_modules" || name == ".qoder"
    })
}

pub fn collect_rust_files(root: &Path, max_files: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max_files {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_skipped(&path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
                if out.len() >= max_files {
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

/// 从 Cargo.lock 取出（name, version）列表。
pub fn lock_crates(lock: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if let (Some(name), Some(version)) = (name.take(), version.take()) {
                out.push((name, version));
            }
        } else if let Some(rest) = line.strip_prefix("name = \"") {
            name = Some(rest.trim_end_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("version = \"") {
            version = Some(rest.trim_end_matches('"').to_string());
        }
    }
    if let (Some(name), Some(version)) = (name, version) {
        out.push((name, version));
    }
    out
}

pub struct Rollup {
    pub files: usize,
    pub sites: Vec<Site>,
    /// crate → (K3 合成, 站点数, risky 数)
    pub per_crate: BTreeMap<String, (Ternary, usize, usize)>,
}

fn crate_label(path: &Path, root: &Path) -> String {
    let parts: Vec<String> = path
        .strip_prefix(root)
        .ok()
        .map(|rest| {
            rest.components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    if let Some(position) = parts.iter().position(|part| part == "crates")
        && let Some(next) = parts.get(position + 1)
    {
        return next.clone();
    }
    parts.first().cloned().unwrap_or_else(|| "root".to_string())
}
pub fn scan_tree(root: &Path, max_files: usize) -> Result<Rollup> {
    let files = collect_rust_files(root, max_files);
    let mut rollup = Rollup {
        files: files.len(),
        sites: Vec::new(),
        per_crate: BTreeMap::new(),
    };
    for path in files {
        let label = crate_label(&path, root);
        let sites = scan_file(&path, &path.display().to_string())?;
        if !sites.is_empty() {
            let verdict = sites
                .iter()
                .fold(Ternary::False, |acc, site| join(acc, site.verdict));
            let risky = sites
                .iter()
                .filter(|site| site.verdict == Ternary::True)
                .count();
            let entry = rollup
                .per_crate
                .entry(label)
                .or_insert((Ternary::False, 0, 0));
            entry.0 = join(entry.0, verdict);
            entry.1 += sites.len();
            entry.2 += risky;
        }
        rollup.sites.extend(sites);
    }
    rollup
        .sites
        .sort_by_key(|site| std::cmp::Reverse(site.verdict));
    Ok(rollup)
}
/// （src 的 safe/unknown/risky，tests 的 risky，未文档化的签名面）
pub fn counts(rollup: &Rollup) -> (usize, usize, usize, usize, usize) {
    let mut safe = 0;
    let mut unknown = 0;
    let mut risky = 0;
    let mut risky_tests = 0;
    let mut undoc_signatures = 0;
    for site in &rollup.sites {
        if site.kind != "block" && !site.documented {
            undoc_signatures += 1;
        }
        match (site.context, site.verdict) {
            ("tests", Ternary::True) => risky_tests += 1,
            (_, Ternary::False) => safe += 1,
            (_, Ternary::Unknown) => unknown += 1,
            (_, Ternary::True) => risky += 1,
        }
    }
    (safe, unknown, risky, risky_tests, undoc_signatures)
}

pub fn verdict_name(value: Ternary) -> &'static str {
    match value {
        Ternary::False => "safe",
        Ternary::Unknown => "unknown",
        Ternary::True => "risky",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undocumented_transmute_is_risky() {
        let source = "fn f() {\n    unsafe { std::mem::transmute::<u32, f32>(1u32) }\n}\n";
        let sites = scan_source(source, "t.rs");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, "block");
        assert_eq!(sites[0].verdict, Ternary::True, "{:?}", sites[0]);
        assert!(sites[0].apis.contains(&"transmute"));
    }

    #[test]
    fn documented_and_guarded_is_safe() {
        let source = "fn f(p: *const u8, n: usize) {\n    // SAFETY: 调用方保证 p 是 n 字节的有效指针\n    assert!(!p.is_null());\n    unsafe { std::slice::from_raw_parts(p, n) };\n}\n";
        let sites = scan_source(source, "t.rs");
        assert_eq!(sites.len(), 1);
        assert!(sites[0].documented && sites[0].guarded, "{:?}", sites[0]);
        assert_eq!(sites[0].verdict, Ternary::False, "{:?}", sites[0]);
    }

    #[test]
    fn documented_but_unguarded_is_unknown() {
        let source = "fn f(p: *const u8, n: usize) {\n    // SAFETY: 相信调用方\n    unsafe { std::slice::from_raw_parts(p, n) };\n}\n";
        let sites = scan_source(source, "t.rs");
        assert_eq!(sites[0].verdict, Ternary::Unknown, "{:?}", sites[0]);
    }

    #[test]
    fn unsafe_signatures_are_unknown_not_risky() {
        let source = "unsafe fn g() {}\nunsafe impl Send for X {}\nunsafe trait T {}\n";
        let sites = scan_source(source, "t.rs");
        let kinds: Vec<&str> = sites.iter().map(|site| site.kind).collect();
        assert_eq!(kinds, vec!["fn", "impl", "trait"], "{kinds:?}");
        for site in &sites {
            assert_eq!(site.verdict, Ternary::Unknown, "{site:?}");
            assert!(!site.documented, "这些签名没有文档");
        }
    }

    #[test]
    fn documented_unsafe_fn_is_unknown_not_risky() {
        let source = "/// # Safety\n/// 调用方保证 id 存在。\npub unsafe fn get(id: usize) {}\n";
        let sites = scan_source(source, "t.rs");
        assert_eq!(sites.len(), 1);
        assert!(sites[0].documented, "{sites:?}");
        assert_eq!(sites[0].verdict, Ternary::Unknown, "{sites:?}");
    }

    #[test]
    fn comments_and_strings_do_not_create_sites() {
        let source = "// unsafe { transmute(x) }\nlet s = \"unsafe { transmute(y) }\";\n";
        assert!(scan_source(source, "t.rs").is_empty());
    }

    #[test]
    fn rollup_uses_k3_join() {
        // 一个 safe + 一个 unknown ⇒ unknown；再加一个 risky ⇒ risky
        assert_eq!(join(Ternary::False, Ternary::Unknown), Ternary::Unknown);
        assert_eq!(join(Ternary::Unknown, Ternary::True), Ternary::True);
        assert_eq!(join(Ternary::False, Ternary::True), Ternary::True);
    }

    #[test]
    fn parses_cargo_lock_packages() {
        let lock = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"2.0.0\"\n";
        let crates = lock_crates(lock);
        assert_eq!(
            crates,
            vec![
                ("a".to_string(), "1.0.0".to_string()),
                ("b".to_string(), "2.0.0".to_string())
            ]
        );
    }
}
