//! 一读诊断：当前网络行不行、哪里不行、改哪里有好处。
//!
//! 把行为判据（是否满足）、行级探针（哪些行一碰就坏）、单点编辑实测收益与
//! 修补可行性聚合成一个结论。所有数字都是**真算出来的**，不是打分估计。

use crate::analyze::{EditImpact, analyze};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::probe::{RowImpact, probe_rows_adaptive};
use crate::repair::{RepairOptions, repair};

/// 诊断结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 已满足判据，不需要改。
    Ok,
    /// 不满足，但存在补丁能修好。
    Repairable,
    /// 不满足，且搜索预算内找不到补丁。
    NotRepairable,
}

impl Verdict {
    /// 线格式字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Repairable => "repairable",
            Self::NotRepairable => "not-repairable",
        }
    }
}

/// 诊断选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnoseOptions {
    /// 行探针预算（0 = 全量）。
    pub probe_budget: usize,
    /// 每类候选保留条数（0 = 全部）。
    pub top: usize,
    /// 修补搜索的最大编辑数。
    pub max_edits: usize,
    /// 修补搜索的束宽。
    pub beam_width: usize,
}

impl Default for DiagnoseOptions {
    fn default() -> Self {
        Self {
            probe_budget: 0,
            top: 8,
            max_edits: 8,
            beam_width: 64,
        }
    }
}

/// 修补可行性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairProbe {
    /// 是否找到补丁（已满足判据时视为 `true`、`patch_len = 0`）。
    pub found: bool,
    /// 补丁里的编辑数。
    pub patch_len: usize,
    /// 搜索中评估过的候选数。
    pub evaluated: usize,
}

/// 一次诊断。
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnosis {
    /// 架构。
    pub arch: Vec<usize>,
    /// 权重 + 偏置的 trit 总数。
    pub trits: usize,
    /// 判据样本数。
    pub samples: usize,
    /// 是否已完全满足判据。
    pub exact: bool,
    /// 当前失配数。
    pub loss: usize,
    /// 一碰就坏的行（清零/取反后行为漂移最大）。
    pub rows: Vec<RowImpact>,
    /// 单点编辑的实测收益（不满足判据时才有）。
    pub edits: Vec<EditImpact>,
    /// 修补可行性。
    pub repair: RepairProbe,
    /// 结论。
    pub verdict: Verdict,
}

/// 跑一次诊断。
pub fn diagnose(net: &Net, oracle: &Oracle, options: &DiagnoseOptions) -> Result<Diagnosis> {
    oracle.check_arch(net)?;
    let loss = oracle.loss(net)?;
    let exact = loss == 0;
    let rows = probe_rows_adaptive(net, oracle, options.probe_budget, options.top)?.top;
    let edits = if exact {
        Vec::new()
    } else {
        analyze(net, oracle, options.top)?.candidates
    };
    let repair = if exact {
        RepairProbe {
            found: true,
            patch_len: 0,
            evaluated: 0,
        }
    } else {
        let found = repair(
            net,
            oracle,
            &RepairOptions {
                max_edits: options.max_edits,
                beam_width: options.beam_width,
                ..RepairOptions::default()
            },
        )?;
        RepairProbe {
            found: found.is_some(),
            patch_len: found.as_ref().map_or(0, |r| r.patch.len()),
            evaluated: found.as_ref().map_or(0, |r| r.evaluated),
        }
    };
    let verdict = if exact {
        Verdict::Ok
    } else if repair.found {
        Verdict::Repairable
    } else {
        Verdict::NotRepairable
    };
    Ok(Diagnosis {
        arch: net.arch(),
        trits: net.trit_count(),
        samples: oracle.len(),
        exact,
        loss,
        rows,
        edits,
        repair,
        verdict,
    })
}
