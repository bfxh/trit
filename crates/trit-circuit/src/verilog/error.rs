//! Verilog 子集的误差模型：类别、行号与构造助手。
//!
//! 从 `mod.rs` 拆出来——编排文件不该同时兼任错误类型的定义处。调用方拿 `VerilogErrorKind`
//! 按根因分类，不必解析错误文本。公开路径不变：`verilog::VerilogError` / `VerilogErrorKind` 仍由 `mod.rs` 转发。

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

pub(in crate::verilog) fn message(text: impl Into<String>) -> VerilogError {
    VerilogError::syntax(text)
}
