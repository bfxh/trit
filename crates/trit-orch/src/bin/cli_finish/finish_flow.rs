use std::collections::BTreeMap;

use serde_json::{Value, json};
use trit_orch::{Error, EvidenceSpec, Progress, Receipt, ReceiptProvenance};

use crate::cli_inputs::{ClaimAuditReport, FinishInput};

use super::{flag_present, measure_all, nonce::host_nonce};

const MAX_DETAIL_CHARS: usize = 160;

fn truncate_detail(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "claim audit did not pass".to_owned();
    }
    if text.chars().count() <= MAX_DETAIL_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(MAX_DETAIL_CHARS).collect();
    format!("{head}...")
}

/// `claim_audit.py -Json` 的第二证据链：报告必须自洽，且每条 claim 都是 VERIFIED。
/// `ok` 是报告自己的结论；summary 与逐条 verdict 是第二道自洽检查，防止手写
/// `ok:true` 但明细仍是 UNVERIFIED/FABRICATED 的报告混过去。
fn check_claim_audit(report: &ClaimAuditReport) -> Result<(), Error> {
    if report.summary.total == 0 {
        return Err(Error::ClaimAuditInvalid {
            message: "report contains no claims".to_owned(),
        });
    }
    if report.claims.len() != report.summary.total as usize {
        return Err(Error::ClaimAuditInvalid {
            message: format!(
                "summary.total={} but {} claim row(s)",
                report.summary.total,
                report.claims.len()
            ),
        });
    }
    if !report.ok
        || report.summary.verified != report.summary.total
        || report.summary.unverified != 0
        || report.summary.fabricated != 0
    {
        return Err(Error::ClaimAuditRefused {
            detail: truncate_detail(&report.verdict),
            total: report.summary.total,
            unverified: report.summary.unverified,
            fabricated: report.summary.fabricated,
        });
    }
    if let Some(claim) = report
        .claims
        .iter()
        .find(|claim| claim.verdict != "VERIFIED")
    {
        return Err(Error::ClaimAuditRefused {
            detail: truncate_detail(&format!(
                "{}: {} ({})",
                claim.kind, claim.claim, claim.verdict
            )),
            total: report.summary.total,
            unverified: report.summary.unverified,
            fabricated: report.summary.fabricated,
        });
    }
    Ok(())
}

pub(super) fn run(
    request: &FinishInput,
    progress: &Progress,
    key: Option<&[u8]>,
) -> Result<Value, Error> {
    if let Some(audit) = &request.claim_audit {
        check_claim_audit(audit)?;
    }
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
