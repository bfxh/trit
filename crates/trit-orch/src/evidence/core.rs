use serde::{Deserialize, Serialize};

#[path = "acceptance_digest.rs"]
pub(crate) mod acceptance_digest;
#[path = "digest.rs"]
mod digest;
#[path = "exit.rs"]
mod exit;
#[path = "plan.rs"]
mod plan;
#[path = "receipt.rs"]
mod receipt;
#[path = "receipt_ctor.rs"]
mod receipt_ctor;
#[path = "receipt_plan.rs"]
mod receipt_plan;
#[path = "receipt_sign.rs"]
mod receipt_sign;
#[path = "task_digest.rs"]
mod task_digest;

pub use digest::test_files_digest;
pub use exit::Exit;
pub use plan::ReceiptPlan;
#[path = "provenance.rs"]
mod provenance;
pub use provenance::ReceiptProvenance;
pub use task_digest::task_spec_digest;

#[path = "receipt_core.rs"]
mod receipt_core;
pub use receipt_core::Receipt;

/// 验收规格：至少要通过多少条断言才算覆盖命题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpec {
    pub min_assertions: u32,
}
