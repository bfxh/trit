//! `trit-orch capability`：把一条动作判定成三值结论（stdin 一份 JSON，stdout 一份 JSON）。

use serde::Deserialize;
use serde_json::{Value, json};
use trit_orch::{ActionSpec, CapabilityPolicy, Error, Ternary, decide};

#[derive(Deserialize)]
struct Request {
    policy: CapabilityPolicy,
    action: ActionSpec,
}

pub fn run(input: &str) -> Result<Value, Error> {
    let request: Request = serde_json::from_str(input).map_err(|error| Error::InputParse {
        message: format!("capability: {error}"),
    })?;
    let decision = decide(&request.policy, &request.action);
    Ok(json!({
        "ok": true,
        "allow": decision.verdict == Ternary::True,
        "decision": decision,
    }))
}
