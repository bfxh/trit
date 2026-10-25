//! 三进制语言服务器（`.tri` / TTIR-0）的库面。
//!
//! 分层：
//! - [`framing`]：LSP 的 `Content-Length` 帧；
//! - [`text`]：LSP 位置（UTF-16 码元）与字节偏移；
//! - [`lex`]：`.tri` 的最小词法扫描与运算符表（拼写/优先级/结合性）；
//! - [`analysis`]：诊断、悬停（真值表）、补全、文档符号；
//! - [`server`]：JSON-RPC 分发与文档状态。
//!
//! 语义不在本 crate 复制：解析用 [`trit_core::Parser`]，真值表用 [`trit_eval::eval::eval`]。
//! 这样 `.tri` 的语法一改，LSP 自动跟着变。

pub mod analysis;
pub mod framing;
pub mod lex;
pub mod server;
pub mod text;
