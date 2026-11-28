use std::collections::BTreeMap;

use crate::{Error, EvidenceSpec, Progress, Receipt, Snapshot, Summary, TaskSpec};

use super::{Attestation, ReceiptTrust};

/// 宿主签名校验：签名缺失与签名无效分开报，调用方可以按 kind 区分。
pub(super) fn check(task: &str, receipt: &Receipt, key: &[u8]) -> Result<(), Error> {
    if receipt.signature.is_empty() {
        return Err(Error::ReceiptObservationMissing {
            task: task.to_owned(),
            field: "signature",
        });
    }
    if !receipt.signature_valid(key) {
        return Err(Error::ReceiptObservationMismatch {
            task: task.to_owned(),
            field: "signature",
        });
    }
    Ok(())
}

impl Progress {
    /// 与 [`Self::finish_with_receipts`] 相同，但要求每条宿主测量回执都携带
    /// 本次 run 的挑战（`nonce`），防止跨轮重放上一轮的真回执。
    pub fn finish_with_receipts_for_nonce(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
        expected_nonce: &str,
    ) -> Result<Summary, Error> {
        self.finish_inner(
            tasks,
            snapshot,
            Some(Attestation {
                receipts,
                spec,
                trust: ReceiptTrust::HostMeasuredOnly,
                nonce: Some(expected_nonce),
                signature_key: None,
            }),
        )
    }

    /// 宿主测量回执还必须带有效 HMAC-SHA256 签名；缺签名或验签失败都拒绝。
    ///
    /// `expected_nonce` 为 `Some` 时同时检查挑战，等价于
    /// [`Self::finish_with_receipts_for_nonce`] 加签名门。
    pub fn finish_with_receipts_hmac(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
        expected_nonce: Option<&str>,
        key: &[u8],
    ) -> Result<Summary, Error> {
        self.finish_inner(
            tasks,
            snapshot,
            Some(Attestation {
                receipts,
                spec,
                trust: ReceiptTrust::HostMeasuredOnly,
                nonce: expected_nonce,
                signature_key: Some(key),
            }),
        )
    }
}
