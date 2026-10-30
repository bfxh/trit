//! `t3-format` — zero-copy, panic-free container triage.
//!
//! Turns a byte buffer into [`t3_core::Facts`]: a fixed table of tri-state
//! observations. Two properties are held above all others:
//!
//! 1. **Malformed input is an answer, not an error.** A truncated file parses and
//!    reports which facts became unmeasurable. Nothing panics; nothing silently
//!    defaults to `false`.
//! 2. **No allocation on the decision path.** The section table is a fixed array;
//!    the fact table is a fixed array; import names are borrowed from the buffer.
//!    Only the region list allocates, because its length depends on the file.
//!
//! Scope is metadata only: headers, tables, names, entropy. There is no
//! disassembly here, and consequently no claim about what the code *does* — the
//! facts describe an attack surface, not behaviour.

#![deny(missing_docs)]
#![warn(clippy::pedantic)]
// 解析/特征层是数值密集代码：所有标量都在 u64/usize 与 f32/f64 之间搬运，
// 量级由格式字段宽度界定（偏移/长度 ≤ 4 GiB，打分 ∈ [0,1]），这里显式接受精度转换。
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "解析层数值转换：来源是 u8/u16/u32/偏移，目标 f32 仅用于打分，量级受控"
)]
// 字节解析用小写单字母是惯例（a/b/c 依次取字节），改名反而难对照规范。
#![allow(clippy::many_single_char_names, reason = "字节解析惯例")]
// 解析函数按格式字段顺序线性展开，拆分会把「偏移 → 字段」的对照关系打散。
#![allow(
    clippy::too_many_lines,
    reason = "PE/ELF 单表解析：字段顺序与规范一一对应，拆开反而难核对偏移"
)]

pub mod detect;
pub mod elf;
pub mod pe;
pub mod reader;
pub mod region;

pub use detect::{analyze, calibration, presence, Analysis, Kind};
pub use elf::{ElfError, ElfInfo};
pub use pe::{PeError, PeInfo};
pub use reader::{Endian, Reader};
pub use region::{Region, RegionKind};
