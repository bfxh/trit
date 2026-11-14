//! 分组结构的判等证书：L0–L4 分级判据 + 可复核证据。
//!
//! 设计见 `docs/WSP-GROUPED-CERTIFY.md`。口径先钉死：
//!
//! - 只有 L0–L3 能声明 `equivalent = true`；
//! - L4 是 **inconclusive**，不是「行为不同」——穷举有域上限，没证出来不等于否证；
//! - L0–L2 是**结构证明**（覆盖任意实数输入），L3 只覆盖有界 trit 域（`vars ≤ 8`，
//!   其余维为 0）；
//! - 形状不同单列 `ShapeMismatch`（定义域不同 ⇒ 确定不等价）。
//!
//! `Layer` 的「正 scale 不改行为」在分组结构下失效：分组是跨行的，同一行内不同列可以
//! 落到不同 scale。能恢复的判据按强度分五级，每一级都给出可被第三方重算的证据字段。

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;

/// 穷举行为签名时允许的最大输入维数（`3^8 = 6561`）。
pub const MAX_VARS: usize = 8;

/// L1/L2 判「同一个比例」的数值容差；写进证书供复核者重算。
pub const DEFAULT_TOLERANCE: f32 = 1e-6;

/// 证书判据（从强到弱）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Witness {
    /// L0：逐位相同。
    Identical,
    /// L1：全体 scale 成同一正比例（canonical 相等）。
    Canonical,
    /// L2：逐行正比例（`B[r,:] = k_r·A[r,:]`，`k_r > 0`）。
    RowProportional,
    /// L3：有界 trit 域上穷举签名相同。
    Behavior,
    /// 形状不同：定义域不同 ⇒ 确定不等价。
    ShapeMismatch,
    /// L4：以上都不成立，**不是**「行为不同」。
    Inconclusive,
}

impl Witness {
    /// 证据标签（落进证书 JSON）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Canonical => "canonical",
            Self::RowProportional => "row_proportional",
            Self::Behavior => "behavior",
            Self::ShapeMismatch => "shape_mismatch",
            Self::Inconclusive => "inconclusive",
        }
    }

    /// 该判据能覆盖的输入范围。
    #[must_use]
    pub const fn implies(self) -> &'static str {
        match self {
            Self::Identical | Self::Canonical | Self::RowProportional => "all_inputs",
            Self::Behavior => "bounded_trit_domain",
            Self::ShapeMismatch | Self::Inconclusive => "none",
        }
    }

    /// 是否等价。
    #[must_use]
    pub const fn equivalent(self) -> bool {
        matches!(
            self,
            Self::Identical | Self::Canonical | Self::RowProportional | Self::Behavior
        )
    }
}

/// 穷举签名的证据（L3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BehaviorProof {
    /// 穷举的输入维数。
    pub vars: usize,
    /// 穷举的输入数（`3^vars`）。
    pub samples: usize,
    /// 每行的符号数（= 行数）。
    pub rows: usize,
    /// 签名摘要（FNV-1a 64，两层相同才相等）。
    pub signature_fnv: u64,
}

impl BehaviorProof {
    fn to_json(self) -> Value {
        json!({
            "vars": self.vars,
            "samples": self.samples,
            "rows": self.rows,
            "trit_inputs": true,
            "signature_fnv": format!("{:016x}", self.signature_fnv),
        })
    }
}

/// 一次分组等价判定的证书。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupedCertificate {
    /// 命中的判据。
    pub witness: Witness,
    /// 行数。
    pub rows: usize,
    /// 列数。
    pub cols: usize,
    /// 判「同一比例」的容差（复核者可自行收紧）。
    pub tolerance: f32,
    /// L1 的全体比例（`other.scales = ratio · self.scales`）。
    pub scale_ratio: Option<f32>,
    /// L2 的逐行比例。
    pub row_ratios: Vec<f32>,
    /// L3 的穷举证据。
    pub behavior: Option<BehaviorProof>,
}

impl GroupedCertificate {
    /// 证书是否成立（等价）。
    #[must_use]
    pub const fn equivalent(&self) -> bool {
        self.witness.equivalent()
    }

    /// 该判据覆盖的输入范围（`all_inputs` / `bounded_trit_domain` / `none`）。
    #[must_use]
    pub const fn implies(&self) -> &'static str {
        self.witness.implies()
    }

    /// → JSON（可直接落盘存档）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.grouped-certificate.v1",
            "scope": "structural",
            "witness": self.witness.as_str(),
            "equivalent": self.equivalent(),
            "implies": self.implies(),
            "tolerance": self.tolerance,
            "shape": { "rows": self.rows, "cols": self.cols },
            "scale_ratio": self.scale_ratio,
            "row_ratios": self.row_ratios,
            "behavior": self.behavior.as_ref().map(|proof| proof.to_json()),
        })
    }
}

fn sign_i8(value: f32) -> i8 {
    if value > 1e-9 {
        1
    } else if value < -1e-9 {
        -1
    } else {
        0
    }
}

fn fnv_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn signature_fnv(signature: &[Vec<i8>]) -> u64 {
    let mut bytes = Vec::new();
    for row in signature {
        bytes.extend(row.iter().map(|value| *value as u8));
    }
    fnv_bytes(&bytes)
}

fn close(left: f32, right: f32, tolerance: f32) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= tolerance * scale
}

impl GroupedLayer {
    /// 行为签名：穷举 `3^vars` 个 trit 输入（其余维为 0），返回每个输入下每行输出的符号。
    ///
    /// 这是**域内完全判定**：`vars` 不得超过 [`MAX_VARS`]，否则返回 `DomainTooLarge`。
    pub fn behavior(&self, vars: usize) -> Result<Vec<Vec<i8>>> {
        if vars > MAX_VARS {
            return Err(Error::DomainTooLarge {
                vars,
                limit: MAX_VARS,
            });
        }
        if vars > self.cols() {
            return Err(Error::Shape {
                what: "behavior vars",
                expected: self.cols(),
                got: vars,
            });
        }
        let total = 3usize.pow(vars as u32);
        let mut signature = Vec::with_capacity(total);
        for code in 0..total {
            let mut input = vec![0.0f32; self.cols()];
            let mut rest = code;
            for slot in input.iter_mut().take(vars) {
                *slot = match rest % 3 {
                    0 => -1.0,
                    1 => 0.0,
                    _ => 1.0,
                };
                rest /= 3;
            }
            let output = self.forward(&input)?;
            signature.push(output.iter().map(|value| sign_i8(*value)).collect());
        }
        Ok(signature)
    }

    /// 在 `vars` 维输入上穷举比较两层的行为是否完全相同。
    pub fn equivalent(&self, other: &Self, vars: usize) -> Result<bool> {
        if self.rows() != other.rows() || self.cols() != other.cols() {
            return Ok(false);
        }
        Ok(self.behavior(vars)? == other.behavior(vars)?)
    }

    /// 规范化：所有 scale 除以 `scales[0]`。
    ///
    /// 全体 scale 同乘一个正数不改变符号行为，所以这个商是唯一的 scale 冗余自由度。
    #[must_use]
    pub fn canonical(&self) -> Self {
        let mut normalized = self.clone();
        let base = self.scales().first().copied().unwrap_or(1.0);
        if base > 0.0 && base.is_finite() {
            for scale in normalized.scales_mut() {
                *scale /= base;
            }
        }
        normalized
    }

    /// 是否「每行自成一组」——此时退化为 [`crate::net::Layer`] 的语义。
    #[must_use]
    pub fn is_row_grouped(&self) -> bool {
        let cols = self.cols();
        for row in 0..self.rows() {
            let first = match self.group_of(row, 0) {
                Some(group) => group,
                None => return false,
            };
            for col in 1..cols {
                if self.group_of(row, col) != Some(first) {
                    return false;
                }
            }
            for row_other in 0..self.rows() {
                if row_other == row {
                    continue;
                }
                for col_other in 0..cols {
                    if self.group_of(row_other, col_other) == Some(first) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// L1 的比例：`trits/groups/bias` 完全一致，且 `other.scales = ratio · self.scales`。
    fn canonical_ratio(&self, other: &Self) -> Option<f32> {
        if self.trits != other.trits
            || self.groups != other.groups
            || self.bias_f32 != other.bias_f32
        {
            return None;
        }
        let (a, b) = (self.scales(), other.scales());
        if a.len() != b.len() || a.is_empty() {
            return None;
        }
        let ratio = match (a[0], b[0]) {
            (left, right) if left > 0.0 && right > 0.0 => right / left,
            (left, right) if left == 0.0 && right == 0.0 => 1.0,
            _ => return None,
        };
        if !(ratio > 0.0 && ratio.is_finite()) {
            return None;
        }
        for (left, right) in a.iter().zip(b) {
            if *left == 0.0 || *right == 0.0 {
                if left != right {
                    return None;
                }
                continue;
            }
            if !close(right / left, ratio, DEFAULT_TOLERANCE) {
                return None;
            }
        }
        Some(ratio)
    }

    /// 若 `other` 的每一行都与 `self` 的对应行**成正比例**（比例 > 0），返回各行的比例。
    ///
    /// 这是**充分条件**：比例存在 ⇒ 两层符号行为完全相同。
    #[must_use]
    pub fn row_proportional(&self, other: &Self) -> Option<Vec<f32>> {
        if self.rows() != other.rows() || self.cols() != other.cols() {
            return None;
        }
        if self.bias_f32 != other.bias_f32 {
            return None;
        }
        let mut ratios = Vec::with_capacity(self.rows());
        for row in 0..self.rows() {
            let mut ratio: Option<f32> = None;
            for col in 0..self.cols() {
                let left = self.effective(row, col)?;
                let right = other.effective(row, col)?;
                if left == 0.0 {
                    if right != 0.0 {
                        return None;
                    }
                    continue;
                }
                let candidate = right / left;
                if candidate <= 0.0 || !candidate.is_finite() {
                    return None;
                }
                match ratio {
                    Some(existing) if !close(existing, candidate, DEFAULT_TOLERANCE) => {
                        return None;
                    }
                    None => ratio = Some(candidate),
                    _ => {}
                }
            }
            ratios.push(ratio.unwrap_or(1.0));
        }
        Some(ratios)
    }

    /// 两层的**每一行**是否恰为同一比例（整体缩放的更粗特例）。
    #[must_use]
    pub fn globally_proportional(&self, other: &Self) -> bool {
        match self.row_proportional(other) {
            Some(ratios) => {
                let first = ratios.first().copied().unwrap_or(1.0);
                ratios
                    .iter()
                    .all(|ratio| close(*ratio, first, DEFAULT_TOLERANCE))
            }
            None => false,
        }
    }

    /// 出分级判等证书（L0 → L4）；`vars` 用于最后的穷举一级。
    pub fn certify_equivalence(&self, other: &Self, vars: usize) -> Result<GroupedCertificate> {
        if vars > MAX_VARS {
            return Err(Error::DomainTooLarge {
                vars,
                limit: MAX_VARS,
            });
        }
        let base = |witness: Witness| GroupedCertificate {
            witness,
            rows: self.rows(),
            cols: self.cols(),
            tolerance: DEFAULT_TOLERANCE,
            scale_ratio: None,
            row_ratios: Vec::new(),
            behavior: None,
        };
        if self.rows() != other.rows() || self.cols() != other.cols() {
            return Ok(base(Witness::ShapeMismatch));
        }
        if self == other {
            return Ok(base(Witness::Identical));
        }
        if let Some(ratio) = self.canonical_ratio(other) {
            let mut certificate = base(Witness::Canonical);
            certificate.scale_ratio = Some(ratio);
            return Ok(certificate);
        }
        if let Some(ratios) = self.row_proportional(other) {
            let mut certificate = base(Witness::RowProportional);
            certificate.row_ratios = ratios;
            return Ok(certificate);
        }
        let left = self.behavior(vars)?;
        let right = other.behavior(vars)?;
        if left == right {
            let mut certificate = base(Witness::Behavior);
            certificate.behavior = Some(BehaviorProof {
                vars,
                samples: left.len(),
                rows: self.rows(),
                signature_fnv: signature_fnv(&left),
            });
            return Ok(certificate);
        }
        Ok(base(Witness::Inconclusive))
    }
}
