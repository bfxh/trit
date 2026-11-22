use super::{DeliveryReceipt, ReceiptFailure, ReceiptState};
use crate::{AdmissionObligation, Error, ObligationKind, Ternary};
use serde::{Deserialize, Serialize};

/// 跨会话交接包。字段都是“让接收方不追问就能接住”的最小集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffPacket {
    pub id: String,
    pub from: String,
    pub to: String,
    pub goal: String,
    pub base_commit: String,
    pub state_hash: String,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
    pub claims: Vec<String>,
    pub evidence: Vec<String>,
    pub next_action: String,
    pub idempotency_key: String,
    pub expires_at: u64,
}

impl HandoffPacket {
    /// 结构校验 + 过期校验。空字段/过期分别给出稳定 kind。
    pub fn validate(&self, now: u64) -> Result<(), Error> {
        for (field, value) in [
            ("id", self.id.as_str()),
            ("from", self.from.as_str()),
            ("to", self.to.as_str()),
            ("goal", self.goal.as_str()),
            ("base_commit", self.base_commit.as_str()),
            ("state_hash", self.state_hash.as_str()),
            ("next_action", self.next_action.as_str()),
            ("idempotency_key", self.idempotency_key.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(Error::HandoffFieldBlank {
                    field: field.to_owned(),
                });
            }
        }
        for (field, values) in [
            ("decisions", &self.decisions),
            ("open_questions", &self.open_questions),
            ("claims", &self.claims),
            ("evidence", &self.evidence),
        ] {
            if values.iter().any(|value| value.trim().is_empty()) {
                return Err(Error::HandoffFieldBlank {
                    field: field.to_owned(),
                });
            }
        }
        if self.expires_at <= now {
            return Err(Error::HandoffExpired {
                id: self.id.trim().to_owned(),
            });
        }
        Ok(())
    }
}

/// 交接门：True=可接；Unknown=过期/状态哈希不一致，先对账；False=结构不合法。
pub fn handoff_gate(packet: &HandoffPacket, receiver_state_hash: &str, now: u64) -> Ternary {
    match packet.validate(now) {
        Ok(()) if packet.state_hash == receiver_state_hash => Ternary::True,
        Ok(()) => Ternary::Unknown,
        Err(Error::HandoffExpired { .. }) => Ternary::Unknown,
        Err(_) => Ternary::False,
    }
}

/// 交接门未通过时的可执行动作。
///
/// `Unknown`（过期/哈希不一致）先对账；`False`（结构不合法）重新生成交接包。
/// 两种情况都不能靠“再发一次同样的包”解决。
pub fn handoff_obligation(
    packet: &HandoffPacket,
    receiver_state_hash: &str,
    now: u64,
) -> Option<AdmissionObligation> {
    match handoff_gate(packet, receiver_state_hash, now) {
        Ternary::True => None,
        Ternary::Unknown => Some(AdmissionObligation::new(
            ObligationKind::Handoff,
            &packet.id,
            &packet.id,
            "交接包过期或状态哈希不一致，先对账",
        )),
        Ternary::False => Some(AdmissionObligation::new(
            ObligationKind::Handoff,
            &packet.id,
            &packet.id,
            "交接包结构不合法，重新生成后再交接",
        )),
    }
}

/// 通讯回执未到 `Acted` 时，发送方还不能把“交接”当完成。
pub fn receipt_obligation(receipt: &DeliveryReceipt) -> Option<AdmissionObligation> {
    if receipt.is_acted() {
        return None;
    }
    let detail = match receipt.failure {
        Some(ReceiptFailure::Rejected) => "对方明确拒绝，重新交接或升级",
        Some(ReceiptFailure::Expired) => "回执过期，重新交接或对账",
        None => match receipt.state {
            ReceiptState::Sent => "已发出但未送达，继续跟踪投递",
            ReceiptState::Delivered => "已送达但未读，继续跟踪",
            ReceiptState::Read => "已读但未行动，等待或升级",
            ReceiptState::Acted => return None,
        },
    };
    Some(AdmissionObligation::new(
        ObligationKind::Handoff,
        &receipt.id,
        &receipt.idempotency_key,
        detail,
    ))
}
