//! `trit-orch` 的宿主桥：stdin 收一份 JSON，stdout 回一份 JSON。
//!
//! 子命令：`plan` / `select` / `dispatch` / `finish` / `recover` / `shadow` / `compact-gate`
//! 以及协调面 `claim` / `renew` / `release` / `claim-gate` / `receipt` / `handoff-gate` /
//! `admit-with-claim`。成功退出码 0，失败退出码 1 并输出
//! `{"ok":false,"kind":"...","error":"..."}`；成功输出永远带 `"ok":true`。

use std::io::Read;

use serde::Deserialize;
use serde_json::{Value, json};
use trit_orch::{
    ClaimStore, ContextFacts, HandoffPacket, Ledger, Limits, Plan, PlanPatch, Progress,
    ReceiptLedger, ReceiptState, RoundReport, Selection, Snapshot, TaskSpec, Ternary, TraceStep,
    TraceStore, admit_with_claim, builtin, compaction_gate, dispatch_ready, handoff_gate, replay,
};

#[derive(Deserialize)]
struct PlanInput {
    revision: u64,
    tasks: Vec<TaskSpec>,
}

#[derive(Deserialize)]
struct SelectInput {
    revision: u64,
    tasks: Vec<TaskSpec>,
    snapshot: Snapshot,
    limits: Limits,
    #[serde(default)]
    patch: Option<PlanPatch>,
    /// patch 的在途保护必须看到**打补丁前**的快照。
    #[serde(default)]
    base_snapshot: Option<Snapshot>,
}

#[derive(Deserialize)]
struct DispatchInput {
    selection: Selection,
    tasks: Vec<TaskSpec>,
    ledger: Ledger,
}

#[derive(Deserialize)]
struct ShadowInput {
    policy: String,
    steps: Vec<TraceStep>,
}

#[derive(Deserialize)]
struct FinishInput {
    tasks: Vec<TaskSpec>,
    snapshot: Snapshot,
    #[serde(default)]
    rounds: Vec<RoundReport>,
}

#[derive(Deserialize)]
struct ClaimInput {
    #[serde(default)]
    store: ClaimStore,
    owner: String,
    paths: Vec<String>,
    intent: String,
    now: u64,
    ttl: u64,
}

#[derive(Deserialize)]
struct RenewInput {
    #[serde(default)]
    store: ClaimStore,
    id: String,
    owner: String,
    fencing: u64,
    now: u64,
    ttl: u64,
}

#[derive(Deserialize)]
struct ReleaseInput {
    #[serde(default)]
    store: ClaimStore,
    id: String,
    owner: String,
    fencing: u64,
}

#[derive(Deserialize)]
struct ClaimGateInput {
    #[serde(default)]
    store: ClaimStore,
    owner: String,
    paths: Vec<String>,
    now: u64,
}

#[derive(Deserialize)]
struct ReceiptInput {
    #[serde(default)]
    ledger: ReceiptLedger,
    action: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct HandoffGateInput {
    packet: HandoffPacket,
    receiver_state_hash: String,
    now: u64,
}

#[derive(Deserialize)]
struct AdmitWithClaimInput {
    dependencies: String,
    authorization: String,
    resources: String,
    freshness: String,
    claim: String,
}

fn parse_ternary(raw: &str) -> Result<Ternary, trit_orch::Error> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "+1" | "1" => Ok(Ternary::True),
        "unknown" | "u" | "0" => Ok(Ternary::Unknown),
        "false" | "f" | "-1" => Ok(Ternary::False),
        other => Err(trit_orch::Error::InputParse {
            message: format!("非法三值 {other:?}（True/Unknown/False 或 +1/0/-1）"),
        }),
    }
}

fn parse_receipt_state(raw: &str) -> Result<ReceiptState, trit_orch::Error> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sent" => Ok(ReceiptState::Sent),
        "delivered" => Ok(ReceiptState::Delivered),
        "read" => Ok(ReceiptState::Read),
        "acted" => Ok(ReceiptState::Acted),
        other => Err(trit_orch::Error::InputParse {
            message: format!("未知回执状态 {other:?}（sent/delivered/read/acted）"),
        }),
    }
}

fn main() {
    let command = std::env::args().nth(1).unwrap_or_default();
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        fail(&trit_orch::Error::InputParse {
            message: error.to_string(),
        });
    }
    match run(&command, &input) {
        Ok(value) => println!("{value}"),
        Err(error) => fail(&error),
    }
}

fn fail(error: &trit_orch::Error) -> ! {
    println!(
        "{}",
        json!({"ok": false, "kind": error.kind(), "error": error.to_string()})
    );
    std::process::exit(1);
}

/// `select --capture <path>`：把这一步追加进轨迹文件（路径由宿主给，模型无法注入）。
fn capture_path() -> Option<String> {
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == "--capture" {
            return args.next();
        }
    }
    None
}

/// `shadow-jsonl <policy> <path>`。
fn jsonl_args() -> (String, String) {
    let mut args = std::env::args().skip(2);
    (
        args.next().unwrap_or_default(),
        args.next().unwrap_or_default(),
    )
}

fn parse<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, trit_orch::Error> {
    serde_json::from_str(input).map_err(|error| trit_orch::Error::InputParse {
        message: error.to_string(),
    })
}

fn run(command: &str, input: &str) -> Result<Value, trit_orch::Error> {
    match command {
        "plan" => {
            let request: PlanInput = parse(input)?;
            let plan = Plan::new(request.revision, request.tasks)?;
            Ok(json!({
                "ok": true,
                "revision": plan.revision(),
                "tasks": plan.tasks().len(),
            }))
        }
        "select" => {
            let request: SelectInput = parse(input)?;
            let capture = capture_path();
            let plan = Plan::new(request.revision, request.tasks)?;
            let plan = match request.patch {
                Some(patch) => {
                    let base = request.base_snapshot.as_ref().ok_or_else(|| {
                        trit_orch::Error::InputParse {
                            message: "带 patch 时必须提供 base_snapshot（打补丁前的快照）".into(),
                        }
                    })?;
                    plan.patch(&patch, base)?
                }
                None => plan,
            };
            let selection = plan.select(&request.snapshot, &request.limits)?;
            if let Some(path) = &capture {
                TraceStore::new(path).append(&TraceStep {
                    revision: selection.revision,
                    // 记录**补丁后**的计划，这样单步回放不依赖 patch。
                    tasks: plan.tasks().to_vec(),
                    snapshot: request.snapshot.clone(),
                    limits: request.limits.clone(),
                    ledger: Ledger::new(),
                })?;
            }
            Ok(json!({
                "ok": true,
                "brief": selection.brief(),
                "selection": selection,
            }))
        }
        "dispatch" => {
            let request: DispatchInput = parse(input)?;
            let attempts = dispatch_ready(&request.selection, &request.tasks, &request.ledger)?;
            Ok(json!({
                "ok": true,
                "ready": attempts,
            }))
        }
        "finish" => {
            let request: FinishInput = parse(input)?;
            let progress = Progress::from_rounds(request.rounds);
            let summary = progress.finish(&request.tasks, &request.snapshot)?;
            Ok(json!({
                "ok": true,
                "summary": summary,
                "brief": progress.brief(),
            }))
        }
        "shadow" => {
            let request: ShadowInput = parse(input)?;
            let policy = builtin(&request.policy).ok_or_else(|| trit_orch::Error::InputParse {
                message: format!(
                    "未知策略 {:?}（可用：baseline/longest_first/eager/critical_path）",
                    request.policy
                ),
            })?;
            let report = replay(policy.as_ref(), &request.steps)?;
            Ok(json!({
                "ok": true,
                "safe": report.is_safe(),
                "report": report,
            }))
        }
        "shadow-jsonl" => {
            let (policy_name, path) = jsonl_args();
            let policy = builtin(&policy_name).ok_or_else(|| trit_orch::Error::InputParse {
                message: format!("未知策略 {policy_name:?}"),
            })?;
            let steps = TraceStore::new(&path).load()?;
            let report = replay(policy.as_ref(), &steps)?;
            Ok(json!({
                "ok": true,
                "safe": report.is_safe(),
                "trace": path,
                "report": report,
            }))
        }
        "recover" => {
            let ledger: Ledger = parse(input)?;
            Ok(json!({
                "ok": true,
                "recovery": ledger.recover(),
            }))
        }
        "claim" => {
            let mut request: ClaimInput = parse(input)?;
            let outcome = request.store.claim(
                &request.owner,
                &request.paths,
                &request.intent,
                request.now,
                request.ttl,
            )?;
            Ok(json!({
                "ok": true,
                "store": request.store,
                "outcome": outcome,
            }))
        }
        "renew" => {
            let mut request: RenewInput = parse(input)?;
            let claim = request.store.renew(
                &request.id,
                &request.owner,
                request.fencing,
                request.now,
                request.ttl,
            )?;
            Ok(json!({
                "ok": true,
                "store": request.store,
                "claim": claim,
            }))
        }
        "release" => {
            let mut request: ReleaseInput = parse(input)?;
            let claim = request
                .store
                .release(&request.id, &request.owner, request.fencing)?;
            Ok(json!({
                "ok": true,
                "store": request.store,
                "claim": claim,
            }))
        }
        "claim-gate" => {
            let request: ClaimGateInput = parse(input)?;
            let gate = request
                .store
                .gate(&request.paths, &request.owner, request.now)?;
            Ok(json!({
                "ok": true,
                "gate": format!("{gate:?}"),
            }))
        }
        "receipt" => {
            let mut request: ReceiptInput = parse(input)?;
            let receipt = match request.action.as_str() {
                "record" => request.ledger.record(&request.id, &request.key)?,
                "advance" => {
                    let state = parse_receipt_state(request.state.as_deref().unwrap_or(""))?;
                    request.ledger.advance(&request.key, state)?
                }
                "reject" => {
                    let reason = request.reason.as_deref().unwrap_or("");
                    request.ledger.reject(&request.key, reason)?
                }
                "expire" => request.ledger.expire(&request.key)?,
                other => {
                    return Err(trit_orch::Error::InputParse {
                        message: format!(
                            "未知 receipt action {other:?}（record/advance/reject/expire）"
                        ),
                    });
                }
            };
            Ok(json!({
                "ok": true,
                "ledger": request.ledger,
                "receipt": receipt,
            }))
        }
        "handoff-gate" => {
            let request: HandoffGateInput = parse(input)?;
            let gate = handoff_gate(&request.packet, &request.receiver_state_hash, request.now);
            Ok(json!({
                "ok": true,
                "gate": format!("{gate:?}"),
            }))
        }
        "admit-with-claim" => {
            let request: AdmitWithClaimInput = parse(input)?;
            let gate = admit_with_claim(
                parse_ternary(&request.dependencies)?,
                parse_ternary(&request.authorization)?,
                parse_ternary(&request.resources)?,
                parse_ternary(&request.freshness)?,
                parse_ternary(&request.claim)?,
            );
            Ok(json!({
                "ok": true,
                "gate": format!("{gate:?}"),
            }))
        }
        "compact-gate" => {
            let facts: ContextFacts = parse(input)?;
            Ok(json!({
                "ok": true,
                "decision": compaction_gate(&facts)?,
            }))
        }
        other => Err(trit_orch::Error::InputParse {
            message: format!(
                "未知子命令 {other:?}（可用：plan/select/dispatch/finish/recover/shadow/shadow-jsonl/compact-gate/claim/renew/release/claim-gate/receipt/handoff-gate/admit-with-claim）"
            ),
        }),
    }
}
