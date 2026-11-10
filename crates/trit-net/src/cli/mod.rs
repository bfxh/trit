//! `trit-net` 的子命令实现面：凡是"要跑完整回路"（而不止三行分派）的命令，各占一个文件。
//!
//! 分派表留在 `main.rs`，实现搬到这里——二进制入口文件不该同时是校准回路的定义处。

pub(crate) mod calibrate;

pub(crate) use calibrate::calibrate_cmd;
