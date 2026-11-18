//! 修补预算与候选作用域（从 `repair.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;
use crate::oracle::Oracle;

/// 修补预算。
pub struct RepairOptions {
    /// 最多允许碰多少个三值位置。
    pub max_edits: usize,
    /// 束宽。
    pub beam_width: usize,
    /// 半径 2 穷举的候选对上限（超过就跳过，直接进束搜索）。
    pub exhaustive_pairs: usize,
    /// 状态空间预算：`3^权重数` 不超过它就改用完备 BFS（能给出最小编辑数）。
    pub exhaustive_states: u64,
    /// 候选编辑作用域；`None` 表示全权重穷举。给定时束搜索只在其中挑选。
    pub scope: Option<Vec<Edit>>,
    /// 是否把行级原语（整行取反、相邻行交换）也放进候选。
    pub row_ops: bool,
    /// 保持集：给定时只接受不破坏它的补丁。
    pub keep: Option<Oracle>,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            max_edits: 8,
            beam_width: 64,
            exhaustive_pairs: 20_000,
            exhaustive_states: 200_000,
            scope: None,
            row_ops: false,
            keep: None,
        }
    }
}
