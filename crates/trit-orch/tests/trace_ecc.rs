//! `TraceEventStore` 可选 ECC 帧封装的行为测试（消费点语义）。
//!
//! 口径：默认构造字节级零变化；`with_ecc` 后单 trit 错自愈、同块双错
//! fail-closed 拒（`trace_ecc_rejected`）；帧行自带码 tag，未开 ECC 的存储
//! 也能读 ECC 文件。

use std::path::PathBuf;

use trit_ops::gf3::{add, from_digit};
use trit_ops::pack::Trytes;
use trit_ops::{FrameCode, FrameResult, decode_line};
use trit_orch::{Error, TraceEvent, TraceEventKind, TraceEventStore, TracePayload};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-orch-ecc-{tag}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t1", seq, TraceEventKind::LimitsCaptured, "ecc", 100).with_payload(
        TracePayload::Limits {
            limits: trit_orch::Limits {
                workers: 2,
                capacity: trit_orch::Resources::new(2, 4),
                tokens: 500,
            },
        },
    )
}

/// 把文件里第 `line_no`（1 起）行改成 ECC 帧 hex 部分被翻转 trit 后的版本。
fn corrupt_line(path: &std::path::Path, line_no: usize, errors: &[(usize, u8)]) {
    let text = std::fs::read_to_string(path).expect("文件存在");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let old = &lines[line_no - 1];
    let parts: Vec<&str> = old.split_whitespace().collect();
    assert_eq!(parts[0], "TRE1", "被改行必须是 ECC 帧");
    let code = FrameCode::from_tag(parts[1]).expect("tag 合法");
    let payload_len: usize = parts[2].parse().unwrap();
    let bytes: Vec<u8> = (0..parts[3].len() / 2)
        .map(|i| u8::from_str_radix(&parts[3][2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let (k, n) = match code {
        FrameCode::Golay11 => (6usize, 11usize),
        FrameCode::Hamming13 => (10, 14),
        FrameCode::Hamming41 => (36, 41),
    };
    let total = (payload_len * 6 + 6).div_ceil(k) * n;
    let mut trits = Trytes::from_codes(bytes, total).expect("可解包").to_trits();
    for &(pos, mag) in errors {
        trits[pos] = add(trits[pos], from_digit(mag));
    }
    let repacked = Trytes::from_trits(&trits);
    let hex: String = repacked
        .codes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    lines[line_no - 1] = format!("TRE1 {} {payload_len} {hex}", code.tag());
    std::fs::write(path, lines.join("\n") + "\n").expect("回写");
}

#[test]
fn default_store_stays_byte_identical() {
    // 默认关闭：文件就是逐行 JSON，无任何帧前缀（现状零变化）。
    let path = temp_path("plain");
    let store = TraceEventStore::new(&path);
    assert_eq!(store.ecc(), None);
    store.append(&event(1)).unwrap();
    store.append(&event(2)).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for line in text.lines() {
        assert!(line.starts_with('{'), "纯 JSON 行，实际 {line}");
        assert!(!line.contains("TRE1"));
    }
    assert_eq!(store.load().unwrap().len(), 2);
}

#[test]
fn ecc_store_roundtrips_and_self_heals_single_trit_error() {
    let path = temp_path("heal");
    let store = TraceEventStore::with_ecc(&path);
    assert_eq!(store.ecc(), Some(FrameCode::Golay11));
    let want: Vec<TraceEvent> = (1..=4).map(event).collect();
    for e in &want {
        store.append(e).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    for line in text.lines() {
        assert!(line.starts_with("TRE1 "), "ECC 帧行，实际 {line}");
    }
    // 干净读回。
    assert_eq!(store.load().unwrap(), want);
    // 单错自愈是确定语义：trit-ops 帧层已逐 trit 穷举（含端点），这里在
    // 消费点抽样走完整读写路径（每 17 个位置 × 两种幅度 + 首尾端点）。
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let parts: Vec<&str> = lines[1].split_whitespace().collect();
    let payload_len: usize = parts[2].parse().unwrap();
    let bytes: Vec<u8> = (0..parts[3].len() / 2)
        .map(|i| u8::from_str_radix(&parts[3][2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let total_trits = Trytes::from_codes(bytes, (payload_len * 6 + 6).div_ceil(6) * 11)
        .expect("帧可解包")
        .len();
    assert!(total_trits > 0);
    let mut positions: Vec<usize> = (0..total_trits).step_by(17).collect();
    positions.push(0);
    positions.push(total_trits - 1);
    positions.sort_unstable();
    positions.dedup();
    for &pos in &positions {
        for mag in 1..=2u8 {
            std::fs::write(&path, &text).unwrap();
            corrupt_line(&path, 2, &[(pos, mag)]);
            let store = TraceEventStore::with_ecc(&path);
            assert_eq!(
                store.load().unwrap(),
                want,
                "单错 位置{pos} 幅度{mag} 必须自愈"
            );
        }
    }
}

#[test]
fn ecc_store_rejects_same_block_double_error_fail_closed() {
    let path = temp_path("double");
    // Golay d=5：同块双错伴随式不与单错碰撞 → 确定 DoubleErrorBlock 拒帧。
    for (i, j) in [(0usize, 1usize), (3, 7), (9, 10)] {
        let _ = std::fs::remove_file(&path);
        let store = TraceEventStore::with_ecc(&path);
        store.append(&event(1)).unwrap();
        corrupt_line(&path, 1, &[(i, 1), (j, 2)]);
        let store = TraceEventStore::with_ecc(&path);
        let error = store.load().unwrap_err();
        assert_eq!(error.kind(), "trace_ecc_rejected");
        assert!(matches!(error, Error::TraceEcc { line: 1, .. }));
        let line = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        match decode_line(&line) {
            FrameResult::Rejected(trit_ops::FrameReject::DoubleErrorBlock(_)) => {}
            other => panic!("({i},{j}) 应拒帧，得到 {other:?}"),
        }
    }
}

#[test]
fn plain_store_still_reads_ecc_lines_via_tag() {
    // 帧行自带 tag：没开 ECC 的存储读 ECC 文件同样校验与自愈。
    let path = temp_path("tag");
    let writer = TraceEventStore::with_ecc(&path);
    let want: Vec<TraceEvent> = (1..=3).map(event).collect();
    for e in &want {
        writer.append(e).unwrap();
    }
    let reader = TraceEventStore::new(&path);
    assert_eq!(reader.ecc(), None);
    assert_eq!(reader.load().unwrap(), want);
}

#[test]
fn mixed_lines_load_with_ecc_and_plain() {
    // 兼容读写：一文件里 ECC 行与普通 JSON 行并存，逐行按前缀分流。
    let path = temp_path("mixed");
    let store = TraceEventStore::with_ecc(&path);
    store.append(&event(1)).unwrap();
    let plain_line = serde_json::to_string(&event(2)).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&plain_line);
    text.push('\n');
    std::fs::write(&path, text).unwrap();
    let store = TraceEventStore::with_ecc(&path);
    assert_eq!(store.load().unwrap(), vec![event(1), event(2)]);
}

#[test]
fn hamming_ecc_frames_roundtrip() {
    // 备选块码：扩展 Hamming [14,10,4]，tag H13。
    let path = temp_path("h13");
    let store = TraceEventStore::with_ecc_code(&path, FrameCode::Hamming13);
    let want: Vec<TraceEvent> = (1..=3).map(event).collect();
    for e in &want {
        store.append(e).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.lines().next().unwrap().starts_with("TRE1 H13 "));
    assert_eq!(store.load().unwrap(), want);
}
