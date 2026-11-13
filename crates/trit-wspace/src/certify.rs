//! 保持集副作用证书：编辑前后逐样本比对，给“没改坏”一个离散证据。
//!
//! 连续权重空间里的模型编辑（ROME / MEMIT / AlphaEdit）没法给出可复现的判等：
//! 只能比 logits 的相对漂移，保持集（preservation set）上的副作用只能用容差说话。
//! 三值权重把编辑变成逐位替换，于是同一件事可以写成**证书**：
//!
//! - 保持集：[`Oracle`]（输入 + 期望输出），与本次编辑无关。编辑后必须逐样本仍然
//!   正确；任何“编辑前对、编辑后错”的样本记一条 violation；
//! - 编辑集（可选）：本次编辑**必须**达成的样本。编辑后必须全部正确，否则补丁没
//!   达成目的，证书同样不成立。
//!
//! 判定是精确的：`Trit` 三选一，比较用 `==`，没有容差、没有浮点。分类固定为五种：
//!
//! | 判定 | 编辑前 | 编辑后 | 是否违规 |
//! |---|---|---|---|
//! | [`Verdict::Preserved`] | 对 | 对 | 否 |
//! | [`Verdict::Fixed`] | 错 | 对 | 否（保持集里本不该发生，单独计数） |
//! | [`Verdict::Regressed`] | 对 | 错 | **是** |
//! | [`Verdict::Drifted`] | 错 | 错（输出变了） | 否（本来就是错样本） |
//! | [`Verdict::Stale`] | 错 | 错（逐位相同） | 否 |
//!
//! 证书里还记补丁自身的事实：条数、成本，以及**空操作**（目标值已等于当前值）与
//! **重复目标**（同一位置被写多次）——它们不改变行为，但会让“编辑了几位”的读数虚高。
//!

use std::collections::BTreeMap;

mod verdicts;

use serde_json::{Value, json};

use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;
use crate::wire;

/// 报告里默认保留的明细行数。
pub const DEFAULT_ROWS: usize = 64;

/// 证书参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertifyOptions {
    /// 允许的 violation 数（默认 0：保持集必须一位不差）。
    pub allow: usize,
    /// 明细行上限（计数永远是全量，只有 `rows` 会截断）。
    pub rows: usize,
}

impl Default for CertifyOptions {
    fn default() -> Self {
        Self {
            allow: 0,
            rows: DEFAULT_ROWS,
        }
    }
}

/// 单个样本的判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 编辑前后都符合期望。
    Preserved,
    /// 编辑前错、编辑后对。
    Fixed,
    /// 编辑前对、编辑后错：唯一的违规类。
    Regressed,
    /// 编辑前后都错，但输出变了。
    Drifted,
    /// 编辑前后都错，且输出逐位相同。
    Stale,
}

impl Verdict {
    /// 稳定字符串（报告/过滤用）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Preserved => "preserved",
            Self::Fixed => "fixed",
            Self::Regressed => "regressed",
            Self::Drifted => "drifted",
            Self::Stale => "stale",
        }
    }

    /// 是否算“编辑引入的副作用”。
    #[must_use]
    pub const fn is_violation(self) -> bool {
        matches!(self, Self::Regressed)
    }
    pub(crate) const fn classify(before_ok: bool, after_ok: bool, same: bool) -> Self {
        match (before_ok, after_ok) {
            (true, true) => Self::Preserved,
            (false, true) => Self::Fixed,
            (true, false) => Self::Regressed,
            (false, false) => {
                if same {
                    Self::Stale
                } else {
                    Self::Drifted
                }
            }
        }
    }
}

/// 一条样本级证据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRow {
    /// 样本下标（与保持集/编辑集顺序一致）。
    pub sample: usize,
    /// 判定。
    pub verdict: Verdict,
    /// 输入。
    pub input: Vec<Trit>,
    /// 期望输出。
    pub want: Vec<Trit>,
    /// 编辑前输出。
    pub before: Vec<Trit>,
    /// 编辑后输出。
    pub after: Vec<Trit>,
}

impl SampleRow {
    /// → JSON（三值用 `-0+` 紧凑串）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "sample": self.sample,
            "verdict": self.verdict.label(),
            "input": wire::trits_text(&self.input),
            "want": wire::trits_text(&self.want),
            "before": wire::trits_text(&self.before),
            "after": wire::trits_text(&self.after),
        })
    }
}

/// 一个样本集（保持集或编辑集）的体检结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepReport {
    /// 样本数。
    pub samples: usize,
    /// 编辑前失配数。
    pub before_loss: usize,
    /// 编辑后失配数。
    pub after_loss: usize,
    /// `Preserved` 数。
    pub preserved: usize,
    /// `Fixed` 数。
    pub fixed: usize,
    /// `Regressed` 数（violation）。
    pub regressed: usize,
    /// `Drifted` 数。
    pub drifted: usize,
    /// `Stale` 数。
    pub stale: usize,
    /// 非 `Preserved` 样本明细（最多 `options.rows` 条）。
    pub rows: Vec<SampleRow>,
    /// 明细是否被截断。
    pub truncated: bool,
}

impl KeepReport {
    /// violation 数（`Regressed`）。
    #[must_use]
    pub const fn violations(&self) -> usize {
        self.regressed
    }

    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "samples": self.samples,
            "before_loss": self.before_loss,
            "after_loss": self.after_loss,
            "preserved": self.preserved,
            "fixed": self.fixed,
            "regressed": self.regressed,
            "drifted": self.drifted,
            "stale": self.stale,
            "rows": self.rows.iter().map(SampleRow::to_json).collect::<Vec<_>>(),
            "truncated": self.truncated,
        })
    }
}

/// 补丁自身的事实（与行为无关的部分）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchFacts {
    /// 编辑条数。
    pub edits: usize,
    /// `SetTrit` 条数。
    pub trits: usize,
    /// `SetBias` 条数。
    pub biases: usize,
    /// `SetScale` 条数。
    pub scales: usize,
    /// 空操作条数：目标值已经等于当前值。
    pub noop: usize,
    /// 重复目标条数：同一位置被写第二次及以后。
    pub repeated: usize,
    /// 碰到的三值位置数。
    pub touched: usize,
}

impl PatchFacts {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "edits": self.edits,
            "trits": self.trits,
            "biases": self.biases,
            "scales": self.scales,
            "noop": self.noop,
            "repeated": self.repeated,
            "touched": self.touched,
        })
    }
}

/// 副作用证书。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// 保持集体检。
    pub keep: KeepReport,
    /// 编辑集体检（没给编辑集时为 `None`）。
    pub edit: Option<KeepReport>,
    /// 补丁事实（直接比两个工件时为 `None`）。
    pub patch: Option<PatchFacts>,
    /// 允许的 violation 数。
    pub allow: usize,
    /// 编辑前工件状态哈希。
    pub base_state: u64,
    /// 编辑后工件状态哈希。
    pub edited_state: u64,
}

impl Certificate {
    /// 证书是否成立：保持集 violation 不超过 `allow`，且编辑集（若有）全部达成。
    #[must_use]
    pub fn passed(&self) -> bool {
        self.keep.violations() <= self.allow
            && self.edit.as_ref().is_none_or(|edit| edit.after_loss == 0)
    }

    /// → JSON（可直接落盘存档）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.certificate.v1",
            "certified": self.passed(),
            "allow": self.allow,
            "keep": self.keep.to_json(),
            "edit": self.edit.as_ref().map(KeepReport::to_json),
            "patch": self.patch.as_ref().map(PatchFacts::to_json),
            "state": {
                "base": format!("{:016x}", self.base_state),
                "edited": format!("{:016x}", self.edited_state),
            },
        })
    }
}

/// 直接给“编辑前 / 编辑后”两个同架构工件出证书。
///
/// `keep` 是保持集（输入 + 期望输出），`edit` 是编辑集（可选，必须全部达成）。
pub fn certify_nets(
    base: &Net,
    edited: &Net,
    keep: &Oracle,
    edit: Option<&Oracle>,
    options: &CertifyOptions,
) -> Result<Certificate> {
    if base.arch() != edited.arch() {
        return Err(crate::error::Error::ArchMismatch);
    }
    let keep_report = compare(keep, base, edited, options.rows)?;
    let edit_report = edit
        .map(|set| compare(set, base, edited, options.rows))
        .transpose()?;
    Ok(Certificate {
        keep: keep_report,
        edit: edit_report,
        patch: None,
        allow: options.allow,
        base_state: base.state_hash(),
        edited_state: edited.state_hash(),
    })
}

/// 应用补丁再出证书：多记一份补丁事实（空操作/重复目标）。
pub fn certify(
    base: &Net,
    patch: &Patch,
    keep: &Oracle,
    edit: Option<&Oracle>,
    options: &CertifyOptions,
) -> Result<Certificate> {
    Ok(apply_and_certify(base, patch, keep, edit, options)?.0)
}

/// 应用补丁 → 出证书，同时返回编辑后的工件（CLI 要落盘时用）。
pub fn apply_and_certify(
    base: &Net,
    patch: &Patch,
    keep: &Oracle,
    edit: Option<&Oracle>,
    options: &CertifyOptions,
) -> Result<(Certificate, Net)> {
    let mut edited = base.clone();
    patch.apply(&mut edited)?;
    let mut certificate = certify_nets(base, &edited, keep, edit, options)?;
    certificate.patch = Some(patch_facts(base, patch));
    Ok((certificate, edited))
}

/// 从网络自身行为采样保持集：随机三值输入 + 当前输出。
///
/// 期望输出就是网络当前行为，所以任何输出漂移都会记成 [`Verdict::Regressed`]：
/// 这份保持集回答的是“行为一字未变”，不需要外部标签。宽输入也能用
/// （[`Oracle::sampled_behavior`] 受穷举域上限约束，这里不受）。
pub fn behavior_set(net: &Net, samples: usize, seed: u64) -> Result<Oracle> {
    if samples == 0 {
        return Err(crate::error::Error::EmptyOracle);
    }
    let width = net.input_dim();
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 1;
    }
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut inputs = Vec::with_capacity(samples);
    for _ in 0..samples {
        let row = (0..width)
            .map(|_| Trit::ALL[(next() % 3) as usize])
            .collect::<Vec<_>>();
        inputs.push(row);
    }
    let outputs = inputs
        .iter()
        .map(|row| net.forward(row))
        .collect::<Result<Vec<_>>>()?;
    Oracle::new(inputs, outputs)
}

/// 补丁事实：条数、成本、空操作与重复目标。
#[must_use]
pub fn patch_facts(base: &Net, patch: &Patch) -> PatchFacts {
    let cost = patch.cost(base);
    let mut seen: BTreeMap<(usize, u8, usize, usize), usize> = BTreeMap::new();
    let mut noop = 0usize;
    let mut repeated = 0usize;
    for edit in patch.edits() {
        if is_noop(base, edit) {
            noop += 1;
        }
        if let Some(key) = target_key(edit) {
            let count = seen.entry(key).or_insert(0);
            if *count > 0 {
                repeated += 1;
            }
            *count += 1;
        }
    }
    PatchFacts {
        edits: cost.edits,
        trits: cost.trits,
        biases: cost.biases,
        scales: cost.scales,
        noop,
        repeated,
        touched: cost.touched,
    }
}

fn is_noop(net: &Net, edit: &Edit) -> bool {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
            ..
        } => net
            .layer(layer)
            .and_then(|one| one.trit(row, col))
            .is_some_and(|current| current == to),
        Edit::SetBias { layer, row, to, .. } => net
            .layer(layer)
            .and_then(|one| one.bias_trit(row))
            .is_some_and(|current| current == to),
        Edit::SetScale { layer, row, to, .. } => net
            .layer(layer)
            .and_then(|one| one.scale(row))
            .is_some_and(|current| current == to),
        Edit::SwapRows { a, b, .. } => a == b,
        _ => false,
    }
}

fn target_key(edit: &Edit) -> Option<(usize, u8, usize, usize)> {
    match *edit {
        Edit::SetTrit {
            layer, row, col, ..
        } => Some((layer, 0, row, col)),
        Edit::SetBias { layer, row, .. } => Some((layer, 1, row, 0)),
        Edit::SetScale { layer, row, .. } => Some((layer, 2, row, 0)),
        _ => None,
    }
}

fn compare(set: &Oracle, before: &Net, after: &Net, rows: usize) -> Result<KeepReport> {
    let was = set.outputs_of(before)?;
    let now = set.outputs_of(after)?;
    let mut report = KeepReport {
        samples: set.len(),
        before_loss: 0,
        after_loss: 0,
        preserved: 0,
        fixed: 0,
        regressed: 0,
        drifted: 0,
        stale: 0,
        rows: Vec::new(),
        truncated: false,
    };
    for (index, ((want, left), right)) in set.outputs().iter().zip(&was).zip(&now).enumerate() {
        let before_ok = left == want;
        let after_ok = right == want;
        if !before_ok {
            report.before_loss += 1;
        }
        if !after_ok {
            report.after_loss += 1;
        }
        let verdict = Verdict::classify(before_ok, after_ok, left == right);
        match verdict {
            Verdict::Preserved => report.preserved += 1,
            Verdict::Fixed => report.fixed += 1,
            Verdict::Regressed => report.regressed += 1,
            Verdict::Drifted => report.drifted += 1,
            Verdict::Stale => report.stale += 1,
        }
        if verdict == Verdict::Preserved {
            continue;
        }
        if report.rows.len() < rows {
            report.rows.push(SampleRow {
                sample: index,
                verdict,
                input: set.inputs()[index].clone(),
                want: want.clone(),
                before: left.clone(),
                after: right.clone(),
            });
        } else {
            report.truncated = true;
        }
    }
    Ok(report)
}
