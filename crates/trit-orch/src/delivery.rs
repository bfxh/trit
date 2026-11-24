//! 交接包的持久化 DeliveryReceipt 账本：Sent → Delivered → Read → Acted。
//!
//! `receipt::DeliveryReceipt` 是内存状态机；这里补的是宿主侧可审计的落盘账本：
//! 每个状态绑定 actor、时间和证据，包字节用 SHA-256 钉住。账本默认写在
//! `<Root>/.agent/delivery/<packet-id>.json`，与 handoff 包的 state_hash 无关。
//!
//! 诚实边界：这是本地自报账本，不是密码学签名；能写这个目录的人也能伪造状态。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::digest;
use crate::receipt::{HandoffPacket, ReceiptState};
use crate::{Error, Ternary, handoff_gate};
use serde::{Deserialize, Serialize};

/// 账本格式标识。
pub const TOOL: &str = "trit-delivery/1";
const MAX_TEXT: usize = 2000;
const DEFAULT_DIR: &str = ".agent/delivery";
/// 账本里的四态（小写落盘；与 `ReceiptState` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    Sent,
    Delivered,
    Read,
    Acted,
}
impl DeliveryState {
    fn from_receipt(state: ReceiptState) -> Self {
        match state {
            ReceiptState::Sent => Self::Sent,
            ReceiptState::Delivered => Self::Delivered,
            ReceiptState::Read => Self::Read,
            ReceiptState::Acted => Self::Acted,
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Sent => Some(Self::Delivered),
            Self::Delivered => Some(Self::Read),
            Self::Read => Some(Self::Acted),
            Self::Acted => None,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Acted => "acted",
        }
    }
}

/// 一条状态记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryEntry {
    pub state: DeliveryState,
    pub at: u64,
    pub actor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// 一个交接包的完整账本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryJournal {
    pub tool: String,
    pub packet_id: String,
    pub packet_sha256: String,
    pub from: String,
    pub to: String,
    pub goal: String,
    pub created_at: u64,
    pub states: Vec<DeliveryEntry>,
}

impl DeliveryJournal {
    pub fn path(root: &Path, packet_id: &str) -> PathBuf {
        root.join(DEFAULT_DIR).join(format!("{packet_id}.json"))
    }

    pub fn load(root: &Path, packet_id: &str) -> Result<Self, Error> {
        if !valid_id(packet_id) {
            return Err(invalid("packet id is not a valid id"));
        }
        let path = Self::path(root, packet_id);
        if !path.is_file() {
            return Err(Error::ReceiptNotFound {
                key: packet_id.to_owned(),
            });
        }
        let bytes = fs::read(&path).map_err(|error| Error::InputParse {
            message: format!("read {}: {error}", path.display()),
        })?;
        let journal: Self = serde_json::from_slice(&bytes).map_err(|error| Error::InputParse {
            message: format!("parse {}: {error}", path.display()),
        })?;
        if journal.packet_id != packet_id {
            return Err(invalid("ledger packet_id does not match the requested id"));
        }
        journal.validate()?;
        Ok(journal)
    }

    pub fn save(&self, root: &Path) -> Result<PathBuf, Error> {
        self.validate()?;
        let path = Self::path(root, &self.packet_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| Error::InputParse {
                message: format!("create {}: {error}", parent.display()),
            })?;
        }
        let text = serde_json::to_vec_pretty(self).map_err(|error| Error::InputParse {
            message: format!("serialize ledger: {error}"),
        })?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, text).map_err(|error| Error::InputParse {
            message: format!("write {}: {error}", temp.display()),
        })?;
        if fs::rename(&temp, &path).is_err() {
            // Windows does not replace an existing file with rename; the
            // ledger is single-writer and this is still a whole-file swap.
            let _ = fs::remove_file(&path);
            fs::rename(&temp, &path).map_err(|error| Error::InputParse {
                message: format!("rename {}: {error}", path.display()),
            })?;
        }
        Ok(path)
    }

    pub fn state_names(&self) -> Vec<&'static str> {
        self.states.iter().map(|entry| entry.state.kind()).collect()
    }

    pub fn complete(&self) -> bool {
        self.states.len() == 4
    }

    fn validate(&self) -> Result<(), Error> {
        if self.tool != TOOL {
            return Err(invalid(format!("tool must be {TOOL}")));
        }
        if !valid_id(&self.packet_id) {
            return Err(invalid("packet_id is not a valid id"));
        }
        if self.packet_sha256.len() != 64
            || !self
                .packet_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(invalid("packet_sha256 must be 64 lowercase hex characters"));
        }
        check_text(&self.from, "from", false)?;
        check_text(&self.to, "to", false)?;
        check_text(&self.goal, "goal", false)?;
        if self.states.is_empty() || self.states.len() > 4 {
            return Err(invalid("states must contain one to four entries"));
        }
        let expected = [
            DeliveryState::Sent,
            DeliveryState::Delivered,
            DeliveryState::Read,
            DeliveryState::Acted,
        ];
        for (index, entry) in self.states.iter().enumerate() {
            if entry.state != expected[index] {
                return Err(invalid(format!(
                    "state {index} must be {}",
                    expected[index].kind()
                )));
            }
            if entry.at < self.created_at && index == 0 {
                return Err(invalid("sent timestamp precedes created_at"));
            }
            if index > 0 && entry.at < self.states[index - 1].at {
                return Err(invalid("state timestamps must not go backwards"));
            }
            let expected_actor = if entry.state == DeliveryState::Sent {
                &self.from
            } else {
                &self.to
            };
            if entry.actor.trim() != expected_actor.trim() {
                return Err(invalid(format!(
                    "{} actor must match the packet endpoint",
                    entry.state.kind()
                )));
            }
            check_text(&entry.actor, "actor", false)?;
            check_text(&entry.evidence, "evidence", true)?;
            check_text(&entry.note, "note", true)?;
            if entry.state == DeliveryState::Acted && entry.evidence.trim().is_empty() {
                return Err(Error::ReceiptReasonBlank);
            }
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InputParse {
        message: message.into(),
    }
}

fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn check_text(value: &str, label: &str, allow_empty: bool) -> Result<(), Error> {
    let trimmed = value.trim();
    if !allow_empty && trimmed.is_empty() {
        return Err(invalid(format!("{label} must be non-empty")));
    }
    if value.chars().count() > MAX_TEXT {
        return Err(invalid(format!(
            "{label} is longer than {MAX_TEXT} characters"
        )));
    }
    if has_machine_path(value) {
        return Err(invalid(format!("{label} must not contain a machine path")));
    }
    Ok(())
}

fn has_machine_path(value: &str) -> bool {
    value.split_whitespace().any(|part| {
        let part = part.trim_matches(|character| matches!(character, '"' | '\'' | '`'));
        part.starts_with("\\\\")
            || (part.len() >= 2
                && part.as_bytes()[0].is_ascii_alphabetic()
                && part.as_bytes()[1] == b':')
    })
}

fn read_packet(path: &Path) -> Result<(HandoffPacket, Vec<u8>), Error> {
    let bytes = fs::read(path).map_err(|error| Error::InputParse {
        message: format!("read packet {}: {error}", path.display()),
    })?;
    let packet: HandoffPacket =
        serde_json::from_slice(&bytes).map_err(|error| Error::InputParse {
            message: format!("parse packet {}: {error}", path.display()),
        })?;
    Ok((packet, bytes))
}

/// 当前 Unix 秒；CLI 的 `at=0` 走这里，而不是把 0 当真实时间。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn packet_digest(bytes: &[u8]) -> String {
    digest::hex(&digest::sha256(bytes))
}

/// 登记 sent；同一包字节重复登记幂等，包字节变化拒绝续账。
pub fn sent(
    root: &Path,
    packet_path: &Path,
    actor: &str,
    at: u64,
) -> Result<DeliveryJournal, Error> {
    let (packet, bytes) = read_packet(packet_path)?;
    let at = if at == 0 { now_secs() } else { at };
    packet.validate(at)?;
    let packet_id = packet.id.trim().to_owned();
    if !valid_id(&packet_id) {
        return Err(invalid("packet id is not a valid id"));
    }
    let packet_sha256 = packet_digest(&bytes);
    let path = DeliveryJournal::path(root, &packet_id);
    if path.is_file() {
        let journal = DeliveryJournal::load(root, &packet_id)?;
        if journal.packet_sha256 != packet_sha256 {
            return Err(invalid("packet bytes changed since sent"));
        }
        return Ok(journal);
    }
    let actor = if actor.trim().is_empty() {
        packet.from.trim().to_owned()
    } else {
        actor.trim().to_owned()
    };
    if actor != packet.from.trim() {
        return Err(invalid("sent actor must equal the packet from"));
    }
    let journal = DeliveryJournal {
        tool: TOOL.to_owned(),
        packet_id: packet_id.clone(),
        packet_sha256,
        from: packet.from.trim().to_owned(),
        to: packet.to.trim().to_owned(),
        goal: packet.goal.trim().to_owned(),
        created_at: at,
        states: vec![DeliveryEntry {
            state: DeliveryState::Sent,
            at,
            actor,
            evidence: String::new(),
            note: String::new(),
        }],
    };
    journal.save(root)?;
    Ok(journal)
}

/// 交接门通过后的落账：只补 sent → delivered，不自动写 read/acted。
///
/// 返回 `(账本, 是否新推进了 delivered)`；已到中间态时幂等。
pub fn attach(root: &Path, packet_path: &Path, at: u64) -> Result<(DeliveryJournal, bool), Error> {
    let mut journal = sent(root, packet_path, "", at)?;
    if journal.states.len() != 1 {
        return Ok((journal, false));
    }
    let at = if at == 0 { now_secs() } else { at };
    journal.states.push(DeliveryEntry {
        state: DeliveryState::Delivered,
        at,
        actor: journal.to.clone(),
        evidence: String::new(),
        note: "handoff gate passed".to_owned(),
    });
    journal.save(root)?;
    Ok((journal, true))
}

/// 显式推进到下一态；顺序、actor、acted 证据都在这里 fail-closed。
pub fn advance(
    root: &Path,
    packet_id: &str,
    next: ReceiptState,
    actor: &str,
    evidence: &str,
    note: &str,
    at: u64,
) -> Result<DeliveryJournal, Error> {
    let mut journal = DeliveryJournal::load(root, packet_id)?;
    let next = DeliveryState::from_receipt(next);
    let current = journal.states.last().expect("validated non-empty").state;
    if next == current {
        return Ok(journal);
    }
    if current.next() != Some(next) {
        return Err(Error::ReceiptSkip {
            key: packet_id.to_owned(),
            from: current.kind().to_owned(),
            to: next.kind().to_owned(),
        });
    }
    let expected_actor = if next == DeliveryState::Sent {
        &journal.from
    } else {
        &journal.to
    };
    if actor.trim() != expected_actor.trim() {
        return Err(invalid(format!(
            "{} actor must equal the packet endpoint",
            next.kind()
        )));
    }
    check_text(actor, "actor", false)?;
    check_text(evidence, "evidence", true)?;
    check_text(note, "note", true)?;
    if next == DeliveryState::Acted && evidence.trim().is_empty() {
        return Err(Error::ReceiptReasonBlank);
    }
    let at = if at == 0 { now_secs() } else { at };
    if at < journal.states.last().expect("validated non-empty").at {
        return Err(invalid("state timestamp goes backwards"));
    }
    journal.states.push(DeliveryEntry {
        state: next,
        at,
        actor: actor.trim().to_owned(),
        evidence: evidence.trim().to_owned(),
        note: note.trim().to_owned(),
    });
    journal.save(root)?;
    Ok(journal)
}

/// 校验结果：完整体 / 未完成 / 缺失 / 包字节变化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeliveryVerify {
    pub status: String,
    pub complete: bool,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<DeliveryJournal>,
}

pub fn verify(
    root: &Path,
    packet_id: &str,
    packet_path: Option<&Path>,
) -> Result<DeliveryVerify, Error> {
    if !valid_id(packet_id) {
        return Err(invalid("packet id is not a valid id"));
    }
    let path = DeliveryJournal::path(root, packet_id);
    if !path.is_file() {
        return Ok(DeliveryVerify {
            status: "missing".to_owned(),
            complete: false,
            reason: "no delivery ledger".to_owned(),
            journal: None,
        });
    }
    let journal = DeliveryJournal::load(root, packet_id)?;
    if let Some(packet_path) = packet_path {
        let (_, bytes) = read_packet(packet_path)?;
        if packet_digest(&bytes) != journal.packet_sha256 {
            return Ok(DeliveryVerify {
                status: "invalid".to_owned(),
                complete: false,
                reason: "packet bytes changed since sent".to_owned(),
                journal: Some(journal),
            });
        }
    }
    let complete = journal.complete();
    Ok(DeliveryVerify {
        status: if complete { "complete" } else { "incomplete" }.to_owned(),
        complete,
        reason: String::new(),
        journal: Some(journal),
    })
}

/// 纯 Rust 交接门 + attach：gate 不通过就不落账。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeliveryGate {
    pub gate: Ternary,
    pub attached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<DeliveryJournal>,
}

pub fn gate(
    root: &Path,
    packet_path: &Path,
    receiver_state_hash: &str,
    at: u64,
) -> Result<DeliveryGate, Error> {
    let at = if at == 0 { now_secs() } else { at };
    let (packet, _) = read_packet(packet_path)?;
    let gate = handoff_gate(&packet, receiver_state_hash, at);
    if gate != Ternary::True {
        return Ok(DeliveryGate {
            gate,
            attached: false,
            journal: None,
        });
    }
    let (journal, attached) = attach(root, packet_path, at)?;
    Ok(DeliveryGate {
        gate,
        attached,
        journal: Some(journal),
    })
}
