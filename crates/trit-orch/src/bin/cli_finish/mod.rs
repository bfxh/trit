use serde_json::{Value, json};
use trit_orch::{EvidenceSpec, Progress};

use crate::{cli_inputs::FinishInput, parse};

fn flag_present(flag: &str) -> bool {
    std::env::args().skip(2).any(|arg| arg == flag)
}

#[allow(deprecated)]
pub(crate) fn run(input: &str) -> Result<Value, trit_orch::Error> {
    let request: FinishInput = parse(input)?;
    let progress = Progress::from_rounds(request.rounds);
    // Fail closed by default: a CheckReport alone is not provenance. Only
    // an explicit legacy flag may accept an unattested snapshot.
    let summary = if flag_present("--allow-unattested") {
        progress.finish(&request.tasks, &request.snapshot)?
    } else {
        progress.finish_with_receipts(
            &request.tasks,
            &request.snapshot,
            &request.receipts,
            EvidenceSpec {
                min_assertions: request.min_assertions,
            },
        )?
    };
    Ok(json!({
        "ok": true,
        "summary": summary,
        "brief": progress.brief(),
    }))
}
