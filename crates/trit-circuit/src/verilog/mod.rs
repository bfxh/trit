//! 结构化 Verilog 子集：组合逻辑（原语 + `assign`，支持多比特端口）。
//!
//! 子模块：`expr`（右值）、`parse`（模块/语句编排）、`decl`（端口/线网声明）、
//! `build`（表达式到门）、`instance`（模块实例语句）、`export`（导出与向量重组）。

/// 单个向量声明或常量字面量的位宽上限。
const MAX_VECTOR_WIDTH: usize = 4096;

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

/// 信号引用：标识符或位选 `name[3]`。
fn is_signal_ref(token: &str) -> bool {
    if is_identifier(token) {
        return true;
    }
    let Some(open) = token.find('[') else {
        return false;
    };
    let base = &token[..open];
    let Some(inner) = token[open + 1..].strip_suffix(']') else {
        return false;
    };
    is_identifier(base) && !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_digit())
}

mod build;
mod decl;
mod error;
mod export;
mod expr;
mod hierarchy;
mod instance;
mod params;
mod parse;
pub(in crate::verilog) use error::message;
pub use error::{VerilogError, VerilogErrorKind};
pub use export::to_verilog;
pub use parse::parse_verilog;

#[cfg(test)]
mod assign_tests;
#[cfg(test)]
mod hierarchy_tests;
#[cfg(test)]
mod parameter_tests;
#[cfg(test)]
mod tests;
