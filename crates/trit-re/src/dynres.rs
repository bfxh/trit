//! 动态解析链与栈串证据的格式无关接线层（从 triage.rs 拆出，守预算）。
//!
//! 两条证据链（N1 首批读数记录的漏报边界的闭合面）：
//! 1. **dynres**：解析器（GetProcAddress/dlsym）在表 ∧ 敏感 API 名以
//!    NUL 串藏于非可执行节且不在导入表/符号表——动态解析形态；
//! 2. **栈串**：可执行节立即数流里出现敏感名——逐字节写栈形态
//!    （数据段扫描全盲）。
//!
//! 三值纪律：证据不完整的维度如实 `Unknown`/`False`，绝不参与 False
//! 结论（误对齐不等于"没有"）。

use crate::functions::stackstrings::{stack_string_hits, stack_string_hits_arm64};
use crate::imports::{self, LINUX_SENSITIVE_APIS, windows_sensitive_vocab};
use crate::witness::Facts;
use std::collections::BTreeSet;
use trit_core::value::Ternary;

/// 按平台给词汇表（调用方传 `linux` 旗标）。
fn vocab(linux: bool) -> Vec<&'static str> {
    if linux {
        LINUX_SENSITIVE_APIS.to_vec()
    } else {
        windows_sensitive_vocab()
    }
}

/// 写入 dynres 三事实（解析器 / 隐藏串 / 栈串）+ 注记带命中名单。
/// `arm64` = AArch64 栈串形态（MOVZ/MOVK 半字流）；x86-64 = C6/C7/B8+r。
#[allow(clippy::too_many_arguments)]
pub fn set_facts(
    facts: &mut Facts,
    notes: &mut Vec<String>,
    all_imports: &[String],
    imported_apis: &BTreeSet<String>,
    resolvers: &[&str],
    nonexec_slices: &[&[u8]],
    exec_slices: &[&[u8]],
    linux: bool,
    arm64: bool,
) {
    let table = if linux {
        "动态符号表"
    } else {
        "导入表"
    };
    let resolver = imports::resolver_imported(all_imports, resolvers);
    let resolver_note = if linux {
        "dlsym 在动态符号表（动态解析链前半段）"
    } else {
        "GetProcAddress 在导入表（动态解析链前半段）"
    };
    facts.set(
        "dynres.resolver_imported",
        Ternary::from(resolver),
        resolver_note,
    );

    let v = vocab(linux);
    let hidden = imports::hidden_api_strings(nonexec_slices, imported_apis, &v);
    let hidden_note = if hidden.is_empty() {
        "（无）".to_string()
    } else {
        hidden.join(", ")
    };
    facts.set(
        "dynres.hidden_api_strings",
        match hidden.len() {
            0 => Ternary::False,
            1..=2 => Ternary::Unknown,
            _ => Ternary::True,
        },
        format!(
            "非可执行节里发现 {} 个不在{table}的敏感 API 名：{hidden_note}（阈值 ≥3 = True）",
            hidden.len()
        ),
    );

    // 栈串提取按机器派发：AArch64 = MOVZ/MOVK 半字流；x86-64 = C6/C7/B8+r。
    let stack = if arm64 {
        stack_string_hits_arm64(exec_slices, &v)
    } else {
        stack_string_hits(exec_slices, &v)
    };
    let stack_note = if stack.is_empty() {
        "（无）".to_string()
    } else {
        stack.join(", ")
    };
    facts.set(
        "dynres.stack_strings",
        bool3(!stack.is_empty()),
        format!(
            "可执行节立即数流命中 {} 个敏感名：{stack_note}（栈串形态高信号，≥1 = True）",
            stack.len()
        ),
    );
    notes.push(format!(
        "dynres：解析器={}，隐藏串 {}，栈串 {}",
        resolver,
        hidden.len(),
        stack.len()
    ));
}

fn bool3(b: bool) -> Ternary {
    if b { Ternary::True } else { Ternary::False }
}
