//! 二进制逆向台账：把 PE/ELF 的结构观测变成**三值事实**，再把假设求成
//! 三值结论；证据两边都有时用 Belnap 四值的 `B` 明确表示冲突。
//!
//! 设计边界：
//! - **不做反汇编**，不下"恶意"结论；只回答结构性问题（加壳迹象、注入能力、装载能力）。
//! - 解析拿不到的数据一律 `Unknown`，不默认成 `False`。
//! - 导出是 append-only 的 JSONL 台账（见 [`ledger`]），采样哈希是主键。
//!
//! ```no_run
//! let bytes = std::fs::read("sample.exe").unwrap();
//! let triage = trit_re::analyze(&bytes);
//! for judgment in &triage.judgments {
//!     println!("{} = {:?} ({})", judgment.id, judgment.value, judgment.four);
//! }
//! ```

#![forbid(unsafe_code)]

pub mod elf;
pub mod entropy;
pub mod hash;
pub mod imports;
pub mod json;
pub mod ledger;
pub mod pe;
pub mod triage;
pub mod verdict;
pub mod witness;

pub use triage::{Triage, analyze};
pub use verdict::{Hypothesis, Judgment, default_hypotheses, evaluate};
pub use witness::{Facts, Witness};
