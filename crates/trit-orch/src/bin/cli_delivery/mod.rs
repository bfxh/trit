//! `trit-orch delivery` 的 JSON 桥：纯 Rust 账本的 CLI 面。
//!
//! 输入沿用宿主桥的 stdin JSON：
//! `{"action":"sent|attach|deliver|read|act|advance|verify|show|gate",
//!  "root":".","packet":"...","packet_id":"...","actor":"...","evidence":"...",
//!  "note":"...","at":0,"receiver_state_hash":"..."}`
//!
//! `gate` 是唯一会同时跑 handoff gate 的动作：gate 为 True 才 attach
//! sent+delivered；Unknown/False 不落账。
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use trit_orch::delivery::{self, DeliveryJournal};
use trit_orch::{Error, ReceiptState};
#[derive(Deserialize)]
struct DeliveryInput {
    #[serde(default)]
    root: String,
    #[serde(default)]
    packet: String,
    action: String,
    #[serde(default)]
    packet_id: String,
    #[serde(default)]
    actor: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    at: u64,
    #[serde(default)]
    receiver_state_hash: String,
}

pub(crate) fn run(input: &str) -> Result<Value, Error> {
    let request: DeliveryInput =
        serde_json::from_str(input).map_err(|error| Error::InputParse {
            message: format!("parse delivery input: {error}"),
        })?;
    let root = if request.root.trim().is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(request.root.trim())
    };
    let at = if request.at == 0 {
        delivery::now_secs()
    } else {
        request.at
    };
    let packet = Path::new(request.packet.trim());
    match request.action.as_str() {
        "sent" => {
            let journal = delivery::sent(&root, packet, &request.actor, at)?;
            Ok(journal_value("sent", &journal, &root))
        }
        "attach" => {
            let (journal, attached) = delivery::attach(&root, packet, at)?;
            Ok(json!({
                "ok": true,
                "action": "attach",
                "attached": attached,
                "journal": journal,
                "ledger": DeliveryJournal::path(&root, &journal.packet_id)
                    .display()
                    .to_string(),
            }))
        }
        "deliver" | "read" | "act" | "advance" => {
            let next = match request.action.as_str() {
                "deliver" => ReceiptState::Delivered,
                "read" => ReceiptState::Read,
                "act" => ReceiptState::Acted,
                _ => super::parse_receipt_state(&request.state)?,
            };
            let journal = delivery::advance(
                &root,
                request.packet_id.trim(),
                next,
                &request.actor,
                &request.evidence,
                &request.note,
                at,
            )?;
            Ok(journal_value(&request.action, &journal, &root))
        }
        "verify" => {
            let packet_path =
                (!request.packet.trim().is_empty()).then(|| Path::new(request.packet.trim()));
            let report = delivery::verify(&root, request.packet_id.trim(), packet_path)?;
            Ok(json!({"ok": true, "action": "verify", "verify": report}))
        }
        "show" => {
            let journal = DeliveryJournal::load(&root, request.packet_id.trim())?;
            Ok(journal_value("show", &journal, &root))
        }
        "gate" => {
            let report = delivery::gate(&root, packet, request.receiver_state_hash.trim(), at)?;
            Ok(json!({
                "ok": true,
                "action": "gate",
                "gate": format!("{:?}", report.gate),
                "attached": report.attached,
                "states": report.journal.as_ref().map(|journal| journal.state_names()),
                "journal": report.journal,
            }))
        }
        other => Err(Error::InputParse {
            message: format!(
                "unknown delivery action {other:?} (sent/attach/deliver/read/act/advance/verify/show/gate)"
            ),
        }),
    }
}

fn journal_value(action: &str, journal: &DeliveryJournal, root: &Path) -> Value {
    json!({
        "ok": true,
        "action": action,
        "packet_id": &journal.packet_id,
        "ledger": DeliveryJournal::path(root, &journal.packet_id)
            .display()
            .to_string(),
        "states": journal.state_names(),
        "journal": journal,
    })
}
