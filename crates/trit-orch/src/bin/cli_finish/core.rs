use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use trit_orch::{EvidenceSpec, Progress, Receipt, ReceiptProvenance};

use crate::{cli_inputs::FinishInput, parse};

use super::measure;

fn flag_present(flag: &str) -> bool {
    std::env::args().skip(2).any(|arg| arg == flag)
}

/// 宿主本次调用的挑战。模型可以读到输入，但读不到调用开始后才生成的值，
/// 因此上一轮的真回执无法在本轮重放。
fn host_nonce() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "host-{}-{}-{}",
        std::process::id(),
        now.as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn run(input: &str) -> Result<Value, trit_orch::Error> {
    let request: FinishInput = parse(input)?;
    let progress = Progress::from_rounds(request.rounds);

    // 显式 legacy 逃生舱：只从快照计事实，不要求任何 provenance。
    if flag_present("--allow-unattested") {
        #[allow(deprecated)]
        let summary = progress.finish(&request.tasks, &request.snapshot)?;
        return Ok(json!({
            "ok": true,
            "summary": summary,
            "all_passed": summary.all_passed(),
            "brief": progress.brief(),
            "provenance": "unattested",
        }));
    }

    let allow_self_reported = flag_present("--allow-self-reported");
    let spec = EvidenceSpec {
        min_assertions: request.min_assertions,
    };

    // 外部传入的 receipts 一律降级为 SelfReported：JSON 里的 `provenance`
    // 字段是调用方写的，不能自己给自己发可信证书。默认路径直接拒绝它们；
    // 只有显式 --allow-self-reported 才采信。
    let mut receipts: BTreeMap<String, Receipt> = BTreeMap::new();
    let mut self_reported_tasks = Vec::new();
    for (task, receipt) in &request.receipts {
        let mut receipt = receipt.clone();
        receipt.provenance = ReceiptProvenance::SelfReported;
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
    for execution in &request.executions {
        let task_spec = request
            .tasks
            .iter()
            .find(|spec| spec.task.id == execution.task)
            .ok_or_else(|| trit_orch::Error::ReceiptUnknownTask {
                task: execution.task.clone(),
            })?;
        let receipt = measure::run_execution(execution, task_spec.epoch, &nonce)?;
        receipts.insert(execution.task.clone(), receipt);
    }

    let summary = if allow_self_reported {
        progress.finish_with_self_reported_receipts(
            &request.tasks,
            &request.snapshot,
            &receipts,
            spec,
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
    Ok(json!({
        "ok": true,
        "summary": summary,
        "all_passed": summary.all_passed(),
        "brief": progress.brief(),
        "provenance": if allow_self_reported { "self-reported-allowed" } else { "host-measured" },
        "nonce": nonce,
    }))
}
