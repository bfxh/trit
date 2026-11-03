//! 结构化 Verilog 子集：组合逻辑（原语 + `assign`，支持多比特端口）。
//!
//! 子模块：`expr`（右值）、`parse`（模块/语句编排）、`decl`（端口/线网声明）、
//! `build`（表达式到门）、`instance`（模块实例语句）、`export`（导出与向量重组）。

/// 单个向量声明或常量字面量的位宽上限。
const MAX_VECTOR_WIDTH: usize = 4096;

/// Verilog 错误类别，调用方可以按根因分类，不必解析错误文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerilogErrorKind {
    /// 语法/格式错误。
    Syntax,
    /// 合法 Verilog，但本库的子集不支持。
    Unsupported,
    /// 资源预算超限（文本、位宽、网络/门数、嵌套深度）。
    Limit,
    /// 导出/结构错误。
    Export,
}

impl VerilogErrorKind {
    /// 稳定短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Unsupported => "unsupported",
            Self::Limit => "limit",
            Self::Export => "export",
        }
    }
}

/// Verilog 子集解析/生成错误：类别 + 可选行号 + 消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerilogError {
    /// 错误类别。
    pub kind: VerilogErrorKind,
    /// 1 起的行号；导出或无法定位时为 `None`。
    pub line: Option<usize>,
    /// 人类可读消息。
    pub message: String,
}

impl VerilogError {
    pub(crate) fn syntax(text: impl Into<String>) -> Self {
        Self {
            kind: VerilogErrorKind::Syntax,
            line: None,
            message: text.into(),
        }
    }

    pub(crate) fn unsupported(text: impl Into<String>) -> Self {
        Self {
            kind: VerilogErrorKind::Unsupported,
            line: None,
            message: text.into(),
        }
    }

    pub(crate) fn limit(text: impl Into<String>) -> Self {
        Self {
            kind: VerilogErrorKind::Limit,
            line: None,
            message: text.into(),
        }
    }

    pub(crate) fn export(text: impl Into<String>) -> Self {
        Self {
            kind: VerilogErrorKind::Export,
            line: None,
            message: text.into(),
        }
    }

    /// 在尚未定位行号时补上行号。
    pub(crate) fn with_line(mut self, line: usize) -> Self {
        if self.line.is_none() {
            self.line = Some(line);
        }
        self
    }
}

impl std::fmt::Display for VerilogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "第 {line} 行：{}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for VerilogError {}

fn message(text: impl Into<String>) -> VerilogError {
    VerilogError::syntax(text)
}

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
mod export;
mod expr;
mod hierarchy;
mod instance;
mod params;
mod parse;
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
