use serde_json::{Value, json};
use trit_orch::Progress;

use crate::{cli_inputs::FinishInput, parse};

#[path = "nonce.rs"]
mod nonce;

#[path = "finish_flow.rs"]
mod finish_flow;
#[path = "measure_all.rs"]
mod measure_all;
#[path = "receipt_key.rs"]
mod receipt_key;

pub(super) fn flag_present(flag: &str) -> bool {
    std::env::args().skip(2).any(|arg| arg == flag)
}

pub(crate) fn run(input: &str) -> Result<Value, trit_orch::Error> {
    let request: FinishInput = parse(input)?;
    let progress = Progress::from_rounds(request.rounds.clone());
    let key = receipt_key::load(receipt_key::flag_path("--receipt-hmac-key-file")?)?;

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

    finish_flow::run(&request, &progress, key.as_deref())
}
