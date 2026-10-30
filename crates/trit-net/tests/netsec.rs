//! `netsec` 的合成夹具测试：pcap 读取、报文解码、流聚合、三值判决、CLI 端到端。
//!
//! 所有报文都是**手搓字节**，不依赖任何真实抓包文件。

use std::process::Command;

use trit_net::netsec::{
    decision::{Verdict, decide},
    features::{DEFAULT_RULES, encode, ternarize},
    flow::{FlowTable, N, NAMES},
    packet::{self, Skip},
    pcap::{self, LinkType, PcapError},
};
use trit_net::{Layer, Model};

fn write_u16(out: &mut Vec<u8>, value: u16, big: bool) {
    if big {
        out.extend_from_slice(&value.to_be_bytes());
    } else {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn write_u32(out: &mut Vec<u8>, value: u32, big: bool) {
    if big {
        out.extend_from_slice(&value.to_be_bytes());
    } else {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn pcap_file(records: &[(&[u8], u32, u32)], big: bool, nanos: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(match (big, nanos) {
        (false, false) => &[0xd4, 0xc3, 0xb2, 0xa1],
        (true, false) => &[0xa1, 0xb2, 0xc3, 0xd4],
        (false, true) => &[0x4d, 0x3c, 0xb2, 0xa1],
        (true, true) => &[0xa1, 0xb2, 0x3c, 0x4d],
    });
    write_u16(&mut out, 2, big);
    write_u16(&mut out, 4, big);
    write_u32(&mut out, 0, big);
    write_u32(&mut out, 0, big);
    write_u32(&mut out, 65_535, big);
    write_u32(&mut out, 1, big);
    for (frame, sec, frac) in records {
        write_u32(&mut out, *sec, big);
        write_u32(&mut out, *frac, big);
        write_u32(&mut out, frame.len() as u32, big);
        write_u32(&mut out, frame.len() as u32, big);
        out.extend_from_slice(frame);
    }
    out
}

fn eth_ipv4_tcp(
    src: [u8; 4],
    dst: [u8; 4],
    sport: u16,
    dport: u16,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let total = 20 + 20 + payload.len();
    let mut frame = Vec::with_capacity(14 + total);
    frame.extend_from_slice(&[0u8; 6]);
    frame.extend_from_slice(&[0u8; 6]);
    frame.extend_from_slice(&0x0800u16.to_be_bytes());
    frame.push(0x45);
    frame.push(0);
    frame.extend_from_slice(&(total as u16).to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&0x4000u16.to_be_bytes());
    frame.push(64);
    frame.push(6);
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&src);
    frame.extend_from_slice(&dst);
    frame.extend_from_slice(&sport.to_be_bytes());
    frame.extend_from_slice(&dport.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.push(5 << 4);
    frame.push(flags);
    frame.extend_from_slice(&0xffffu16.to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn judge_model(hot: Option<usize>, bias: i8) -> Model {
    let mut weights = vec![vec![0i8; N]];
    if let Some(index) = hot {
        weights[0][index] = 1;
    }
    Model {
        layers: vec![Layer {
            inputs: N,
            outputs: 1,
            weights,
            bias: vec![bias],
        }],
    }
}

#[test]
fn little_and_big_endian_read_the_same_records() {
    let frame = eth_ipv4_tcp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 80, 0x02, &[]);
    let le = pcap_file(&[(&frame, 7, 250_000)], false, false);
    let be = pcap_file(&[(&frame, 7, 250_000)], true, false);
    let mut readers = [
        pcap::Reader::new(&le).expect("小端文件头"),
        pcap::Reader::new(&be).expect("大端文件头"),
    ]
    .into_iter()
    .map(|mut reader| {
        assert_eq!(reader.link_type(), LinkType::Ethernet);
        assert!(!reader.nanos());
        let record = reader.next().expect("一条记录").expect("记录合法");
        assert!(reader.next().is_none());
        (record.timestamp_seconds(false), record.data.to_vec())
    });
    let (le_ts, le_data) = readers.next().unwrap();
    let (be_ts, be_data) = readers.next().unwrap();
    assert_eq!(le_ts, 7.25);
    assert_eq!(be_ts, 7.25);
    assert_eq!(le_data, frame);
    assert_eq!(be_data, frame);
}

#[test]
fn nanosecond_magic_uses_nano_scale() {
    let frame = eth_ipv4_tcp([10, 0, 0, 1], [10, 0, 0, 2], 1, 2, 0x10, &[]);
    let file = pcap_file(&[(&frame, 1, 500_000_000)], false, true);
    let mut reader = pcap::Reader::new(&file).expect("纳秒文件头");
    assert!(reader.nanos());
    let record = reader.next().unwrap().unwrap();
    assert!((record.timestamp_seconds(true) - 1.5).abs() < 1e-9);
}

#[test]
fn pcapng_and_bad_magic_are_rejected() {
    let mut ng = vec![0x0a, 0x0d, 0x0d, 0x0a];
    ng.extend_from_slice(&[0u8; 20]);
    assert_eq!(
        pcap::Reader::new(&ng).unwrap_err(),
        PcapError::PcapNgUnsupported
    );

    let mut bad = vec![0xde, 0xad, 0xbe, 0xef];
    bad.extend_from_slice(&[0u8; 20]);
    assert_eq!(
        pcap::Reader::new(&bad).unwrap_err(),
        PcapError::BadMagic([0xde, 0xad, 0xbe, 0xef])
    );
}

#[test]
fn unsupported_link_type_is_named() {
    let mut file = pcap_file(&[], false, false);
    file[20..24].copy_from_slice(&113u32.to_le_bytes());
    assert_eq!(
        pcap::Reader::new(&file).unwrap_err(),
        PcapError::UnsupportedLinkType(113)
    );
}

#[test]
fn truncated_record_is_reported_not_silently_dropped() {
    let frame = eth_ipv4_tcp([10, 0, 0, 1], [10, 0, 0, 2], 1, 2, 0x10, b"abc");
    let mut file = pcap_file(&[(&frame, 1, 0)], false, false);
    file.truncate(file.len() - 2);
    let mut reader = pcap::Reader::new(&file).expect("文件头合法");
    let error = reader.next().unwrap().unwrap_err();
    assert_eq!(
        error,
        PcapError::TruncatedRecord {
            index: 1,
            need: frame.len(),
            remaining: frame.len() - 2,
        }
    );
}

#[test]
fn vlan_tag_is_stripped_once() {
    let inner = eth_ipv4_tcp([10, 0, 0, 1], [10, 0, 0, 2], 1000, 443, 0x18, b"xy");
    let mut frame = Vec::new();
    frame.extend_from_slice(&inner[..12]);
    frame.extend_from_slice(&0x8100u16.to_be_bytes());
    frame.extend_from_slice(&0x0064u16.to_be_bytes());
    frame.extend_from_slice(&inner[12..]);
    let decoded = packet::decode(LinkType::Ethernet, &frame).expect("VLAN 报文应能解码");
    assert_eq!(decoded.payload_len, 2);
    assert_eq!(decoded.transport.ports(), (1000, 443));
}

#[test]
fn non_first_fragment_is_skipped_with_reason() {
    let mut frame = eth_ipv4_tcp([10, 0, 0, 1], [10, 0, 0, 2], 1, 2, 0x10, b"abc");
    frame[20] = 0x00;
    frame[21] = 0x10;
    assert_eq!(
        packet::decode(LinkType::Ethernet, &frame).unwrap_err(),
        Skip::Fragment
    );
}

#[test]
fn flow_aggregates_both_directions() {
    let a = [192, 168, 1, 5];
    let b = [192, 168, 1, 9];
    let mut table = FlowTable::new();
    let packets = [
        (0.0, eth_ipv4_tcp(a, b, 5555, 80, 0x02, &[])),
        (0.01, eth_ipv4_tcp(b, a, 80, 5555, 0x12, &[])),
        (0.02, eth_ipv4_tcp(a, b, 5555, 80, 0x10, &[0u8; 10])),
        (0.05, eth_ipv4_tcp(b, a, 80, 5555, 0x18, &[0u8; 20])),
    ];
    for (ts, frame) in &packets {
        let decoded = packet::decode(LinkType::Ethernet, frame).expect("合成报文应可解码");
        table.ingest(*ts, &decoded);
    }
    assert_eq!(table.len(), 1);
    let flows = table.finish();
    assert_eq!(flows.len(), 1);
    let flow = &flows[0];
    assert_eq!(flow.src().port, 5555, "首包发送方是正向");
    assert_eq!(flow.dst().port, 80);
    assert!((flow.features.duration_s - 0.05).abs() < 1e-9);
    assert_eq!(flow.features.fwd_pkts, 2.0);
    assert_eq!(flow.features.bwd_pkts, 2.0);
    assert_eq!(flow.features.fwd_bytes, 10.0);
    assert_eq!(flow.features.bwd_bytes, 20.0);
    assert_eq!(flow.features.syn, 2.0);
    assert_eq!(flow.features.fin, 0.0);
    assert_eq!(flow.features.fwd_pkt_ratio, 0.5);
}

#[test]
fn dead_band_becomes_unknown() {
    let mut values = [0.0; N];
    // fwd_pkts 落在 (10, 1000) 死区；bwd_pkts 走 Low 方向的死区。
    values[1] = 100.0;
    values[2] = 3.0;
    let inputs = ternarize(&values, &DEFAULT_RULES);
    assert_eq!(inputs.len(), N);
    use trit_core::value::Ternary;
    assert_eq!(inputs[1], Ternary::Unknown);
    assert_eq!(inputs[2], Ternary::Unknown);
    // 未被规则覆盖的维度固定 False。
    assert_eq!(inputs[0], Ternary::False);
    assert_eq!(encode(&inputs).len(), N);
    assert_eq!(inputs.iter().filter(|v| **v == Ternary::Unknown).count(), 2);
}

#[test]
fn definite_output_is_malicious_and_unknown_input_abstains() {
    use trit_core::value::Ternary;

    let model = judge_model(None, 1);
    let inputs = vec![Ternary::False; N];
    let decision = decide(&model, &inputs, trit_net::DEFAULT_BUDGET).expect("全确定输入");
    assert_eq!(decision.verdict, Verdict::Malicious);
    assert_eq!(decision.reason, "definite-positive");

    // 输出神经元自己弃权（累加正好为 0）时不能冒充良性。
    let model = judge_model(None, 0);
    let decision = decide(&model, &inputs, trit_net::DEFAULT_BUDGET).expect("全确定输入");
    assert_eq!(decision.verdict, Verdict::Undetermined);
    assert_eq!(decision.reason, "output-zero");

    let model = judge_model(Some(1), 0);
    let mut inputs = vec![Ternary::False; N];
    inputs[1] = Ternary::Unknown;
    let decision = decide(&model, &inputs, trit_net::DEFAULT_BUDGET).expect("一个未知输入");
    assert_eq!(decision.verdict, Verdict::Undetermined);
    assert_eq!(decision.reason, "ambiguous");
    assert!(decision.exact);
    assert_eq!(decision.unknown_inputs, vec![1]);
}

#[test]
fn cli_flow_emits_one_json_line_per_flow() {
    let frame = eth_ipv4_tcp([10, 1, 2, 3], [10, 1, 2, 4], 4444, 53, 0x02, &[]);
    let file = pcap_file(&[(&frame, 3, 0)], false, false);
    let path = std::env::temp_dir().join(format!("trit-netsec-{}.pcap", std::process::id()));
    std::fs::write(&path, &file).expect("写临时 pcap");
    let output = Command::new(env!("CARGO_BIN_EXE_trit-net"))
        .arg("flow")
        .arg(&path)
        .output()
        .expect("运行 trit-net flow");
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout 是 UTF-8");
    let lines: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    let value: serde_json::Value = serde_json::from_str(lines[0]).expect("每行都是合法 JSON");
    assert_eq!(value["src"], "10.1.2.3:4444");
    assert_eq!(value["dst"], "10.1.2.4:53");
    assert_eq!(value["proto"], 6);
    assert_eq!(value["inputs"].as_str().map(str::len), Some(NAMES.len()));
}
