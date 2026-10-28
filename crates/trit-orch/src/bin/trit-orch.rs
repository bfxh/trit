//! `trit-orch` 的宿主桥：stdin 收一份 JSON，stdout 回一份 JSON。
//!
//! 子命令：`plan` / `select` / `dispatch` / `finish` / `recover`。成功退出码 0，
//! 失败退出码 1 并输出 `{"ok":false,"kind":"...","error":"..."}`；成功输出永远带 `"ok":true`。

use std::io::Read;

use serde::Deserialize;
use serde_json::{Value, json};
use trit_orch::{
    Ledger, Limits, Plan, PlanPatch, Progress, RoundReport, Selection, Snapshot, TaskSpec,
    TraceStep, builtin, dispatch_ready, replay,
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
                    "未知策略 {:?}（可用：baseline/longest_first/eager）",
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
        "recover" => {
            let ledger: Ledger = parse(input)?;
            Ok(json!({
                "ok": true,
                "recovery": ledger.recover(),
            }))
        }
        other => Err(trit_orch::Error::InputParse {
            message: format!(
                "未知子命令 {other:?}（可用：plan/select/dispatch/finish/recover/shadow）"
            ),
        }),
    }
}
