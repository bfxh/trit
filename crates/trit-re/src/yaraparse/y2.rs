//! YARA 完整语义前端（v2）——接缝占位。
//!
//! 规范与任务卡见 `docs/YARA-EXPR/`。实现前一律返回 `Err`：回放 harness
//! 按「拒绝」计（官方编译错误一致，其余弃权），错判必须保持 0。

use super::ParsedFile;

/// 解析 YARA 规则文件（v2 完整语义）。占位阶段拒绝一切输入。
pub fn parse_rules_v2(_text: &str) -> Result<ParsedFile, String> {
    Err("y2 前端尚未实现（接缝占位，见 docs/YARA-EXPR/05-TASKS.md）".to_string())
}
