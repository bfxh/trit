use serde::Deserialize;
use trit_orch::*;
mod finish;
pub(crate) mod stdin;
pub(crate) use finish::{ClaimAuditReport, Execution, FinishInput};

#[derive(Deserialize)]
pub(crate) struct PlanInput {
    pub(crate) revision: u64,
    pub(crate) tasks: Vec<TaskSpec>,
}

#[derive(Deserialize)]
pub(crate) struct SelectInput {
    pub(crate) revision: u64,
    pub(crate) tasks: Vec<TaskSpec>,
    pub(crate) snapshot: Snapshot,
    pub(crate) limits: Limits,
    #[serde(default)]
    pub(crate) patch: Option<PlanPatch>,
    /// patch 的在途保护必须看到**打补丁前**的快照。
    #[serde(default)]
    pub(crate) base_snapshot: Option<Snapshot>,
    #[serde(default)]
    pub(crate) ledger: Ledger,
}

#[derive(Deserialize)]
pub(crate) struct DispatchInput {
    pub(crate) selection: Selection,
    pub(crate) tasks: Vec<TaskSpec>,
    pub(crate) ledger: Ledger,
}

#[derive(Deserialize)]
pub(crate) struct ObligationInput {
    pub(crate) selection: Selection,
    #[serde(default)]
    pub(crate) policy: ObligationPolicy,
    pub(crate) now: u64,
    pub(crate) budget: ObligationBudget,
}

#[derive(Deserialize)]
pub(crate) struct ShadowInput {
    pub(crate) policy: String,
    pub(crate) steps: Vec<TraceStep>,
}

#[derive(Deserialize)]
pub(crate) struct ClaimInput {
    #[serde(default)]
    pub(crate) store: ClaimStore,
    pub(crate) owner: String,
    pub(crate) paths: Vec<String>,
    pub(crate) intent: String,
    pub(crate) now: u64,
    pub(crate) ttl: u64,
}

#[derive(Deserialize)]
pub(crate) struct RenewInput {
    #[serde(default)]
    pub(crate) store: ClaimStore,
    pub(crate) id: String,
    pub(crate) owner: String,
    pub(crate) fencing: u64,
    pub(crate) now: u64,
    pub(crate) ttl: u64,
}

#[derive(Deserialize)]
pub(crate) struct ReleaseInput {
    #[serde(default)]
    pub(crate) store: ClaimStore,
    pub(crate) id: String,
    pub(crate) owner: String,
    pub(crate) fencing: u64,
}

#[derive(Deserialize)]
pub(crate) struct ClaimGateInput {
    #[serde(default)]
    pub(crate) store: ClaimStore,
    pub(crate) owner: String,
    pub(crate) paths: Vec<String>,
    pub(crate) now: u64,
}

#[derive(Deserialize)]
pub(crate) struct ReceiptInput {
    #[serde(default)]
    pub(crate) ledger: ReceiptLedger,
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) state: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct HandoffGateInput {
    pub(crate) packet: HandoffPacket,
    pub(crate) receiver_state_hash: String,
    pub(crate) now: u64,
}

#[derive(Deserialize)]
pub(crate) struct AdmitWithClaimInput {
    pub(crate) dependencies: String,
    pub(crate) authorization: String,
    pub(crate) resources: String,
    pub(crate) freshness: String,
    pub(crate) claim: String,
}
