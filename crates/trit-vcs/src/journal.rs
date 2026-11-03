//! 哈希链事件日志：追加式，每条事件带前一条的哈希。
//!
//! 为什么这条比 Git 强：Git 的 reflog 是**本地、可改写、`gc` 会丢**的东西；
//! 这里的每条记录（提交、认领、门禁结论、回执）串成链，改中间一条会让后面全部对不上。
//! `verify` 就是那条尺子。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::Hash;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: String,
    pub payload: Value,
    pub prev: Option<Hash>,
    pub hash: Hash,
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("日志行不是合法 JSON（第 {line} 条）：{detail}")]
    BadLine { line: usize, detail: String },
    #[error("链断了（第 {line} 条）：记录的 hash 与内容对不上")]
    Broken { line: usize },
    #[error("prev 指错了（第 {line} 条）：期望 {expected:?}，实际 {actual:?}")]
    WrongPrev {
        line: usize,
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("seq 不连续（第 {line} 条）：期望 {expected}，实际 {actual}")]
    SeqGap {
        line: usize,
        expected: u64,
        actual: u64,
    },
}

pub(crate) fn event_hash(
    seq: u64,
    at_ms: u64,
    kind: &str,
    payload: &Value,
    prev: Option<&Hash>,
) -> Hash {
    let prev_hex = prev.map(|h| h.to_hex()).unwrap_or_else(|| "-".to_string());
    let body = format!(
        "{seq}\n{at_ms}\n{kind}\n{}\n{prev_hex}",
        serde_json::to_string(payload).unwrap_or_default()
    );
    Hash::of(body.as_bytes())
}

#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    events: Vec<Event>,
}

impl Journal {
    /// 打开并**先验一遍链**：链断了直接拒绝工作，不在坏账上继续追加。
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let events = Self::read(&path)?;
        Ok(Self { path, events })
    }

    fn read(path: &Path) -> Result<Vec<Event>, JournalError> {
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(path)?;
        let mut events: Vec<Event> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let line_no = index + 1;
            let event: Event = serde_json::from_str(line).map_err(|e| JournalError::BadLine {
                line: line_no,
                detail: e.to_string(),
            })?;
            let expected_hash = event_hash(
                event.seq,
                event.at_ms,
                &event.kind,
                &event.payload,
                event.prev.as_ref(),
            );
            if expected_hash != event.hash {
                return Err(JournalError::Broken { line: line_no });
            }
            let expected_prev = events.last().map(|e| e.hash);
            if event.prev != expected_prev && !(events.is_empty() && event.prev.is_none()) {
                return Err(JournalError::WrongPrev {
                    line: line_no,
                    expected: expected_prev.map(|h| h.to_hex()),
                    actual: event.prev.map(|h| h.to_hex()),
                });
            }
            let expected_seq = events.len() as u64;
            if event.seq != expected_seq {
                return Err(JournalError::SeqGap {
                    line: line_no,
                    expected: expected_seq,
                    actual: event.seq,
                });
            }
            events.push(event);
        }
        Ok(events)
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn head(&self) -> Option<Hash> {
        self.events.last().map(|e| e.hash)
    }

    pub fn append(
        &mut self,
        kind: &str,
        payload: Value,
        at_ms: u64,
    ) -> Result<Event, JournalError> {
        let seq = self.events.len() as u64;
        let prev = self.head();
        let hash = event_hash(seq, at_ms, kind, &payload, prev.as_ref());
        let event = Event {
            seq,
            at_ms,
            kind: kind.to_string(),
            payload,
            prev,
            hash,
        };
        let line = serde_json::to_string(&event).map_err(|e| JournalError::BadLine {
            line: seq as usize + 1,
            detail: e.to_string(),
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        file.sync_data()?;
        self.events.push(event.clone());
        Ok(event)
    }

    /// 只验不追加：返回事件条数。
    pub fn verify(path: impl AsRef<Path>) -> Result<usize, JournalError> {
        Ok(Self::read(path.as_ref())?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trit-vcs-journal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("journal.jsonl")
    }

    #[test]
    fn append_and_verify() {
        let path = temp("append");
        let mut journal = Journal::open(&path).unwrap();
        journal
            .append("commit", json!({"tree": "abc"}), 1000)
            .unwrap();
        journal
            .append("claim", json!({"owner": "qoder:s1"}), 1001)
            .unwrap();
        journal
            .append("gate", json!({"verdict": "true"}), 1002)
            .unwrap();
        assert_eq!(journal.events().len(), 3);
        assert_eq!(Journal::verify(&path).unwrap(), 3);
        // 链：后一条的 prev 必须等于前一条的 hash
        let events = journal.events();
        assert_eq!(events[1].prev, Some(events[0].hash));
        assert_eq!(events[2].prev, Some(events[1].hash));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn tampering_with_a_middle_event_is_detected() {
        let path = temp("tamper");
        let mut journal = Journal::open(&path).unwrap();
        for i in 0..3 {
            journal.append("commit", json!({"i": i}), 1000 + i).unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        lines[1] = lines[1].replace("\"i\":1", "\"i\":9");
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert!(matches!(
            Journal::verify(&path),
            Err(JournalError::Broken { .. })
        ));
    }

    #[test]
    fn dropping_an_event_breaks_prev_links() {
        let path = temp("drop");
        let mut journal = Journal::open(&path).unwrap();
        for i in 0..3 {
            journal.append("commit", json!({"i": i}), 1000 + i).unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(Journal::verify(&path).is_err(), "删掉中间一条必须现形");
    }

    #[test]
    fn refusing_to_append_on_a_broken_chain() {
        let path = temp("broken-open");
        let mut journal = Journal::open(&path).unwrap();
        journal.append("commit", json!({"i": 0}), 1000).unwrap();
        let mut text = fs::read_to_string(&path).unwrap();
        text = text.replace("\"i\":0", "\"i\":1");
        fs::write(&path, text).unwrap();
        assert!(Journal::open(&path).is_err(), "坏链上不许继续追加");
    }

    /// 模拟"写坏的写入方"：prev 指错但把自己的 hash 算对了。
    /// 哈希校验抓不到（hash 自洽），只有 prev 链接检查能抓——这条专钉那一段。
    #[test]
    fn wrong_prev_with_self_consistent_hash_is_caught() {
        let path = temp("wrong-prev");
        let first = Event {
            seq: 0,
            at_ms: 1,
            kind: "commit".to_string(),
            payload: json!({"i": 0}),
            prev: None,
            hash: event_hash(0, 1, "commit", &json!({"i": 0}), None),
        };
        let bogus_prev = Hash::of(b"somewhere-else");
        let second = Event {
            seq: 1,
            at_ms: 2,
            kind: "commit".to_string(),
            payload: json!({"i": 1}),
            prev: Some(bogus_prev),
            hash: event_hash(1, 2, "commit", &json!({"i": 1}), Some(&bogus_prev)),
        };
        let body = format!(
            "{}
{}
",
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        fs::write(&path, body).unwrap();
        assert!(matches!(
            Journal::verify(&path),
            Err(JournalError::WrongPrev { .. })
        ));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn payload_order_is_stable_across_runs() {
        let a = temp("stable-a");
        let b = temp("stable-b");
        let mut ja = Journal::open(&a).unwrap();
        let mut jb = Journal::open(&b).unwrap();
        let payload = json!({"z": 1, "a": {"y": 2, "b": 3}});
        let ea = ja.append("x", payload.clone(), 7).unwrap();
        let eb = jb.append("x", payload, 7).unwrap();
        assert_eq!(ea.hash, eb.hash, "同样的 payload 必须得到同一个 hash");
        let _ = fs::remove_dir_all(a.parent().unwrap());
        let _ = fs::remove_dir_all(b.parent().unwrap());
    }
}
