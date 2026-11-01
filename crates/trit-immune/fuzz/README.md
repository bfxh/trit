//! Fuzz 目标索引：五个解析器 = 外部输入的第一道墙。
//!
//! 不变量（与 tests/fuzz_smoke.rs 一致）：**任何输入不 panic、不挂起、
//! 只返回 Ok 或显式 Err**。确定性版（随机字节流，进常规测试套件）在
//! `tests/fuzz_smoke.rs`；本目录是 libFuzzer 版（覆盖率引导，触发条件：
//! nightly + cargo-fuzz）。
//!
//! ```text
//! rustup install nightly && cargo install cargo-fuzz
//! cargo fuzz run json_parse -- -max_len=4096
//! ```
