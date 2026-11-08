//! 三值守恒门禁：防止「三值被静默折成布尔」的用法回潮。
//!
//! 口径（扫描 `crates/**/src/**/*.rs` 与 `crates/**/tests/**/*.rs`）：
//!
//! 1. **禁止**把 `Unknown` 直接写成布尔字面量（`Unknown => true` / `Unknown => false`）；
//! 2. `.is_true()` / `.is_false()` 只能出现在白名单文件里——它们对 `Unknown` 返回 `false`，
//!    在决策路径上使用等于把「未知」静默折成「非真」；
//! 3. `to_bool_strict()` / `to_bool_loose()` 只能出现在白名单文件里，
//!    新代码必须走 `Ternary::collapse(BinaryCollapse)`，把策略写在调用点上。
//!
//! 白名单必须写理由；理由为空或文件不存在都会让 `whitelist_entries_are_documented` 失败。

use std::fs;
use std::path::{Path, PathBuf};

/// 允许 `.is_true()` / `.is_false()` 的文件（附理由）。
const IS_TRUE_WHITELIST: &[(&str, &str)] = &[
    ("trit-core/src/value.rs", "定义本身与其单测"),
    (
        "trit-core/src/algebra.rs",
        "Kleene / Łukasiewicz 代数的 True 行定义（不是降级）",
    ),
    (
        "trit-core/src/bits.rs",
        "all(is_true)/all(is_false) 精确谓词：问的是「是否全部确定」",
    ),
    (
        "trit-core/src/bulk.rs",
        "to_bools() 里 is_binary() 保护下解包，含 Unknown 已提前返回 None",
    ),
    (
        "trit-bdd/src/equiv.rs",
        "「精确为假」的等价性判定（Unknown ≠ False，语义正确）",
    ),
    (
        "trit-bdd/src/quantify.rs",
        "「精确为假」的可满足性判定（Unknown ≠ False，语义正确）",
    ),
    (
        "trit-circuit/src/circuit/types.rs",
        "XorClassic 先判 is_unknown()，之后才用 is_true() 比较",
    ),
];

/// 允许 `to_bool_strict()` / `to_bool_loose()` 的文件（附理由）。
const TO_BOOL_WHITELIST: &[(&str, &str)] = &[(
    "trit-core/src/value.rs",
    "定义与单测；对外已由 collapse(BinaryCollapse) 取代",
)];

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().and_then(|s| s.to_str()) == Some("target") {
                continue;
            }
            if p.file_name().and_then(|s| s.to_str()) == Some(".git") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("trit-core 应位于 crates/ 下")
        .to_path_buf()
}

#[test]
fn no_implicit_ternary_degradation() {
    let crates = crates_root();
    let mut files = Vec::new();
    collect_rs(&crates, &mut files);
    assert!(
        files.len() > 30,
        "只扫到 {} 个 rs 文件，路径可能不对（{}）",
        files.len(),
        crates.display()
    );

    // 用 concat! 拼模式，避免本文件自身命中。
    let pat_unknown_true = concat!("Unknown => ", "true");
    let pat_unknown_false = concat!("Unknown => ", "false");
    let pat_is_true = concat!(".is_", "true()");
    let pat_is_false = concat!(".is_", "false()");
    let pat_strict = concat!("to_bool_", "strict");
    let pat_loose = concat!("to_bool_", "loose");

    let mut violations: Vec<String> = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with("tests/tristate_audit.rs") {
            continue;
        }
        let Ok(text) = fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if line.contains(pat_unknown_true) || line.contains(pat_unknown_false) {
                violations.push(format!("{rel}:{n} 把 Unknown 直接折成布尔字面量"));
            }
            let uses_is = line.contains(pat_is_true) || line.contains(pat_is_false);
            if uses_is && !IS_TRUE_WHITELIST.iter().any(|(p, _)| rel == *p) {
                violations.push(format!(
                    "{rel}:{n} 用 is_true/is_false 做布尔判断（应改用 collapse(BinaryCollapse)）"
                ));
            }
            let uses_to_bool = line.contains(pat_strict) || line.contains(pat_loose);
            if uses_to_bool && !TO_BOOL_WHITELIST.iter().any(|(p, _)| rel == *p) {
                violations.push(format!(
                    "{rel}:{n} 直接用 to_bool_*（应改用 collapse(BinaryCollapse)）"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "发现 {} 处三值隐式降级：\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[test]
fn whitelist_entries_are_documented() {
    let crates = crates_root();
    for (p, why) in IS_TRUE_WHITELIST.iter().chain(TO_BOOL_WHITELIST.iter()) {
        assert!(!why.is_empty(), "{p} 的白名单理由不能为空");
        let full = crates.join(p);
        assert!(full.exists(), "白名单里的文件不存在：{p}");
    }
}

#[test]
fn collapse_is_the_documented_funnel() {
    // 三值 → 二值必须只经 collapse：三个策略都要能被表达。
    use trit_core::{BinaryCollapse, Ternary};
    assert_eq!(Ternary::Unknown.collapse(BinaryCollapse::ToFalse), Some(false));
    assert_eq!(Ternary::Unknown.collapse(BinaryCollapse::ToTrue), Some(true));
    assert_eq!(Ternary::Unknown.collapse(BinaryCollapse::Reject), None);
}
