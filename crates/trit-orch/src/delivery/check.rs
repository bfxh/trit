//! `delivery check`：只读预检，同时回答“交接门”和“账本是否还能续”。
//!
//! 与 `gate` 的区别：`gate` 在门为 `True` 时补 `sent + delivered`；
//! `check` 一个字节都不写，适合宿主在动手前做一次组合判断。
use std::path::Path;

use serde::Serialize;

use super::{DeliveryVerify, now_secs, read_packet, verify};
use crate::{Error, Ternary, handoff_gate};

/// 只读预检结果；`ready` 只表示“门通过且账本没有坏掉”，不表示四态已走完。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeliveryCheck {
    pub gate: Ternary,
    pub receipt: DeliveryVerify,
    pub ready: bool,
}

pub fn check(
    root: &Path,
    packet_path: &Path,
    receiver_state_hash: &str,
    at: u64,
) -> Result<DeliveryCheck, Error> {
    let at = if at == 0 { now_secs() } else { at };
    let (packet, _) = read_packet(packet_path)?;
    let gate = handoff_gate(&packet, receiver_state_hash, at);
    let receipt = match verify(root, packet.id.trim(), Some(packet_path)) {
        Ok(report) => report,
        Err(error) => DeliveryVerify {
            status: "invalid".to_owned(),
            complete: false,
            reason: error.to_string(),
            journal: None,
        },
    };
    let ready = gate == Ternary::True && receipt.status != "invalid";
    Ok(DeliveryCheck {
        gate,
        receipt,
        ready,
    })
}
