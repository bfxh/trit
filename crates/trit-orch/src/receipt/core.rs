//! 四态回执与交接包：Sent → Delivered → Read → Acted。
//!
//! - 正常路径只有四个状态；`Rejected`/`Expired` 是终止失败，单独放在 `failure`。
//! - 同一 `idempotency_key` 重复投递只保留一条回执；重复 advance 是幂等的。
//! - 交接包只做结构/过期/状态哈希校验；是否执行由调度器决定。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;

/// 回执的正常四态。顺序固定，不能跳级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptState {
    Sent,
    Delivered,
    Read,
    Acted,
}

impl ReceiptState {
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Sent => Some(Self::Delivered),
            Self::Delivered => Some(Self::Read),
            Self::Read => Some(Self::Acted),
            Self::Acted => None,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Acted => "acted",
        }
    }
}

/// 终止失败原因。失败后不能再推进四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptFailure {
    Rejected,
    Expired,
}

/// 一条回执。`failure` 与四态互斥：失败后 `state` 保留最后到达的位置，便于审计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub id: String,
    pub idempotency_key: String,
    pub state: ReceiptState,
    pub failure: Option<ReceiptFailure>,
    pub reason: Option<String>,
}

impl DeliveryReceipt {
    pub fn sent(id: &str, idempotency_key: &str) -> Result<Self, Error> {
        if id.trim().is_empty() {
            return Err(Error::ReceiptIdBlank);
        }
        if idempotency_key.trim().is_empty() {
            return Err(Error::ReceiptKeyBlank);
        }
        Ok(Self {
            id: id.trim().to_owned(),
            idempotency_key: idempotency_key.trim().to_owned(),
            state: ReceiptState::Sent,
            failure: None,
            reason: None,
        })
    }

    /// 推进到下一态。重复推进到当前态是幂等的；跳级/回退判红。
    pub fn advance(&mut self, next: ReceiptState) -> Result<(), Error> {
        if self.failure.is_some() {
            return Err(Error::ReceiptFailed {
                key: self.idempotency_key.clone(),
            });
        }
        if next == self.state {
            return Ok(());
        }
        if self.state.next() != Some(next) {
            return Err(Error::ReceiptSkip {
                key: self.idempotency_key.clone(),
                from: self.state.kind().to_owned(),
                to: next.kind().to_owned(),
            });
        }
        self.state = next;
        Ok(())
    }

    /// 拒绝。已 Acted 不能拒绝；重复拒绝幂等。
    pub fn reject(&mut self, reason: &str) -> Result<(), Error> {
        if self.state == ReceiptState::Acted {
            return Err(Error::ReceiptAlreadyActed {
                key: self.idempotency_key.clone(),
            });
        }
        if self.failure == Some(ReceiptFailure::Rejected) {
            return Ok(());
        }
        if self.failure.is_some() {
            return Err(Error::ReceiptFailed {
                key: self.idempotency_key.clone(),
            });
        }
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(Error::ReceiptReasonBlank);
        }
        self.failure = Some(ReceiptFailure::Rejected);
        self.reason = Some(reason.to_owned());
        Ok(())
    }

    /// 过期。已 Acted 不能过期；重复过期幂等。
    pub fn expire(&mut self) -> Result<(), Error> {
        if self.state == ReceiptState::Acted {
            return Err(Error::ReceiptAlreadyActed {
                key: self.idempotency_key.clone(),
            });
        }
        if self.failure == Some(ReceiptFailure::Expired) {
            return Ok(());
        }
        if self.failure.is_some() {
            return Err(Error::ReceiptFailed {
                key: self.idempotency_key.clone(),
            });
        }
        self.failure = Some(ReceiptFailure::Expired);
        self.reason = None;
        Ok(())
    }

    pub fn is_acted(&self) -> bool {
        self.state == ReceiptState::Acted && self.failure.is_none()
    }

    pub fn is_failed(&self) -> bool {
        self.failure.is_some()
    }
}

/// 按 idempotency key 去重的回执表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLedger {
    receipts: BTreeMap<String, DeliveryReceipt>,
}

impl ReceiptLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条初始回执；同一 key 重复记录返回已有条目（不覆盖 id）。
    ///
    /// 同 key 但 id 不同不是“幂等重放”，而是身份混淆：静默返回旧条目会让审计
    /// 把两次不同的投递当成同一条。这里按冲突拒绝。
    pub fn record(&mut self, id: &str, idempotency_key: &str) -> Result<DeliveryReceipt, Error> {
        let key = idempotency_key.trim();
        let id = id.trim();
        if id.is_empty() {
            return Err(Error::ReceiptIdBlank);
        }
        if key.is_empty() {
            return Err(Error::ReceiptKeyBlank);
        }
        if let Some(existing) = self.receipts.get(key) {
            if existing.id != id {
                return Err(Error::ReceiptIdConflict {
                    key: key.to_owned(),
                    existing: existing.id.clone(),
                    incoming: id.to_owned(),
                });
            }
            return Ok(existing.clone());
        }
        let receipt = DeliveryReceipt::sent(id, key)?;
        self.receipts.insert(key.to_owned(), receipt.clone());
        Ok(receipt)
    }

    pub fn advance(&mut self, key: &str, next: ReceiptState) -> Result<DeliveryReceipt, Error> {
        let receipt = self
            .receipts
            .get_mut(key)
            .ok_or_else(|| Error::ReceiptNotFound {
                key: key.to_owned(),
            })?;
        receipt.advance(next)?;
        Ok(receipt.clone())
    }

    pub fn reject(&mut self, key: &str, reason: &str) -> Result<DeliveryReceipt, Error> {
        let receipt = self
            .receipts
            .get_mut(key)
            .ok_or_else(|| Error::ReceiptNotFound {
                key: key.to_owned(),
            })?;
        receipt.reject(reason)?;
        Ok(receipt.clone())
    }

    pub fn expire(&mut self, key: &str) -> Result<DeliveryReceipt, Error> {
        let receipt = self
            .receipts
            .get_mut(key)
            .ok_or_else(|| Error::ReceiptNotFound {
                key: key.to_owned(),
            })?;
        receipt.expire()?;
        Ok(receipt.clone())
    }

    pub fn get(&self, key: &str) -> Option<&DeliveryReceipt> {
        self.receipts.get(key)
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
}
