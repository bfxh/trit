use std::collections::BTreeMap;

use serde_json::{Value, json};
use trit_orch::{Error, EvidenceSpec, Progress, Receipt, ReceiptProvenance};

use crate::cli_inputs::FinishInput;

use super::{flag_present, measure_all, nonce::host_nonce};

pub(super) fn run(
    request: &FinishInput,
    progress: &Progress,
    key: Option<&[u8]>,
) -> Result<Value, Error> {
    let allow_self_reported = flag_present("--allow-self-reported");
    let spec = EvidenceSpec {
        min_assertions: request.min_assertions.unwrap_or(0),
    };

    // 外部传入的 receipts 一律降级为 SelfReported：JSON 里的 `provenance`
    // 字段是调用方写的，不能自己给自己发可信证书。默认路径直接拒绝它们；
    // 只有显式 --allow-self-reported 才采信。
    let mut receipts: BTreeMap<String, Receipt> = BTreeMap::new();
    let mut self_reported_tasks = Vec::new();
    for (task, receipt) in &request.receipts {
        let mut receipt = receipt.clone();
        let signed_host = key.is_some_and(|key| {
            receipt.provenance == ReceiptProvenance::HostMeasured && receipt.signature_valid(key)
        });
        if !signed_host {
            receipt.provenance = ReceiptProvenance::SelfReported;
        }
        if allow_self_reported {
            receipts.insert(task.clone(), receipt);
        } else {
            self_reported_tasks.push(task.clone());
        }
    }
    if let Some(task) = self_reported_tasks.first() {
        return Err(trit_orch::Error::ReceiptSelfReported { task: task.clone() });
    }
    // 宿主自己执行验收命令，回执的每个数字都来自真实进程。
    let nonce = request.challenge.clone().unwrap_or_else(host_nonce);
    measure_all::host_measure(request, &mut receipts, &nonce)?;
    if let Some(key) = key {
        for receipt in receipts.values_mut() {
            receipt.sign(key);
        }
    }

    let summary = if allow_self_reported {
        progress.finish_with_self_reported_receipts(
            &request.tasks,
            &request.snapshot,
            &receipts,
            spec,
        )?
    } else if let Some(key) = key {
        progress.finish_with_receipts_hmac(
            &request.tasks,
            &request.snapshot,
            &receipts,
            spec,
            Some(&nonce),
            key,
        )?
    } else {
        progress.finish_with_receipts_for_nonce(
            &request.tasks,
            &request.snapshot,
            &receipts,
            spec,
            &nonce,
        )?
    };
    let test_digests: BTreeMap<&str, &str> = receipts
        .iter()
        .filter_map(|(task, receipt)| {
            receipt
                .test_digest
                .as_deref()
                .map(|digest| (task.as_str(), digest))
        })
        .collect();
    Ok(json!({
        "ok": true,
        "summary": summary,
        "all_passed": summary.all_passed(),
        "brief": progress.brief(),
        "provenance": if allow_self_reported { "self-reported-allowed" } else { "host-measured" },
        "nonce": nonce,
        "test_digests": test_digests,
    }))
}
