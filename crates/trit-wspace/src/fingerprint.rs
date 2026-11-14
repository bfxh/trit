//! 挑战集行为指纹：对一组输入记录输出签名与摘要，用于「同一模型」的快速判定。
//!
//! 与 audit 的行为链同源：`Oracle::outputs_of(net)` 给出逐样本输出；指纹把它固化成
//! 可传输的 JSON（含 FNV 摘要），`check` 逐样本比对并列出失配下标。三值输出没有容差，
//! 所以判定是精确的：任何一位变化都会让 `matched` 下降、`digest_match` 变假。

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 一份挑战集指纹。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// 样本数。
    pub samples: usize,
    /// 逐样本输出。
    pub signature: Vec<Vec<Trit>>,
    /// 签名摘要（FNV-1a 64）。
    pub digest: u64,
}

/// 比对结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// 样本数。
    pub samples: usize,
    /// 逐样本完全一致的条数。
    pub matched: usize,
    /// 失配的样本下标（最多 `limit` 条）。
    pub mismatched: Vec<usize>,
    /// 摘要是否一致。
    pub digest_match: bool,
}

impl CheckReport {
    /// 是否完全一致（摘要 + 全部样本）。
    #[must_use]
    pub fn passed(&self) -> bool {
        self.digest_match && self.matched == self.samples
    }

    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "samples": self.samples,
            "matched": self.matched,
            "mismatched": self.mismatched,
            "digest_match": self.digest_match,
            "passed": self.passed(),
        })
    }
}

fn trit_i8(value: Trit) -> i8 {
    value.to_i8()
}

fn trit_of(value: i8) -> Result<Trit> {
    match value {
        -1 => Ok(Trit::Neg),
        0 => Ok(Trit::Zero),
        1 => Ok(Trit::Pos),
        other => Err(Error::InvalidTrit(other)),
    }
}

fn digest_of(signature: &[Vec<Trit>]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for row in signature {
        for trit in row {
            hash ^= u64::from(trit_i8(*trit) as u8);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl Fingerprint {
    /// → JSON（`signature` 用 `-1/0/+1` 整数）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.fingerprint.v1",
            "samples": self.samples,
            "digest": format!("{:016x}", self.digest),
            "signature": self.signature.iter().map(|row| row.iter().map(|trit| trit_i8(*trit)).collect::<Vec<_>>()).collect::<Vec<_>>(),
        })
    }

    /// 从 JSON 还原；schema、样本数、三值合法性与摘要都会校验。
    pub fn from_json(value: &Value) -> Result<Self> {
        let schema = value
            .get("schema")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Wire("指纹缺 schema".into()))?;
        if schema != "trit-wspace.fingerprint.v1" {
            return Err(Error::Wire(format!("指纹 schema `{schema}` 不受支持")));
        }
        let samples = value
            .get("samples")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Wire("指纹缺 samples".into()))?;
        let samples =
            usize::try_from(samples).map_err(|_| Error::Wire("samples 超出 usize".into()))?;
        let entries = value
            .get("signature")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Wire("指纹缺 signature".into()))?;
        if entries.len() != samples {
            return Err(Error::Wire(format!(
                "指纹 samples={samples} 与 signature {} 条不一致",
                entries.len()
            )));
        }
        let mut signature = Vec::with_capacity(entries.len());
        for (index, row) in entries.iter().enumerate() {
            let values = row
                .as_array()
                .ok_or_else(|| Error::Wire(format!("signature[{index}] 不是数组")))?;
            let mut trits = Vec::with_capacity(values.len());
            for value in values {
                let raw = value
                    .as_i64()
                    .ok_or_else(|| Error::Wire(format!("signature[{index}] 含非整数")))?;
                let raw = i8::try_from(raw).map_err(|_| Error::InvalidTrit(0))?;
                trits.push(trit_of(raw)?);
            }
            signature.push(trits);
        }
        let digest = digest_of(&signature);
        if let Some(text) = value.get("digest").and_then(Value::as_str)
            && let Ok(expected) = u64::from_str_radix(text, 16)
            && expected != digest
        {
            return Err(Error::Wire(format!(
                "指纹摘要不符：记录 {expected:016x}，重算 {digest:016x}"
            )));
        }
        Ok(Self {
            samples,
            signature,
            digest,
        })
    }
}

/// 对挑战集生成指纹。
pub fn emit(net: &Net, oracle: &Oracle) -> Result<Fingerprint> {
    let signature = oracle.outputs_of(net)?;
    Ok(Fingerprint {
        samples: signature.len(),
        digest: digest_of(&signature),
        signature,
    })
}

/// 用指纹校验工件行为；`limit` 限制返回的失配下标条数。
pub fn check(
    net: &Net,
    oracle: &Oracle,
    fingerprint: &Fingerprint,
    limit: usize,
) -> Result<CheckReport> {
    let signature = oracle.outputs_of(net)?;
    if signature.len() != fingerprint.signature.len() {
        return Err(Error::Wire(format!(
            "样本数不一致：工件 {}，指纹 {}",
            signature.len(),
            fingerprint.signature.len()
        )));
    }
    let mut matched = 0usize;
    let mut mismatched = Vec::new();
    for (index, (left, right)) in signature.iter().zip(&fingerprint.signature).enumerate() {
        if left == right {
            matched += 1;
        } else if mismatched.len() < limit {
            mismatched.push(index);
        }
    }
    Ok(CheckReport {
        samples: signature.len(),
        matched,
        mismatched,
        digest_match: digest_of(&signature) == fingerprint.digest,
    })
}
