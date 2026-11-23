//! #627 回归：UDP 长度字段与捕获载荷对账（fail-closed）。
//!
//! 全部为手搓字节夹具，直接调用公开 API（[`packet::decode`] / [`pcap::Reader`]），
//! 不经过 CLI 参数解析；断言区分「修前通过 / 修后拒绝或记账」。

use trit_net::netsec::packet::{self, Skip};
use trit_net::netsec::pcap::{self, PcapError};

const SRC: [u8; 4] = [10, 0, 0, 1];
const DST: [u8; 4] = [10, 0, 0, 2];

/// 手搓 Ethernet + IPv4(UDP) 帧；`udp_length` 可谎报，IP 总长按实际载荷写。
fn udp_frame(payload: &[u8], udp_length: u16) -> Vec<u8> {
    let ip_total = 20 + 8 + payload.len();
    let mut frame = Vec::with_capacity(14 + ip_total);
    frame.extend_from_slice(&[0u8; 6]);
    frame.extend_from_slice(&[0u8; 6]);
    frame.extend_from_slice(&0x0800u16.to_be_bytes());
    frame.push(0x45);
    frame.push(0);
    frame.extend_from_slice(&(ip_total as u16).to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&0x4000u16.to_be_bytes());
    frame.push(64);
    frame.push(17);
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(&SRC);
    frame.extend_from_slice(&DST);
    frame.extend_from_slice(&1234u16.to_be_bytes());
    frame.extend_from_slice(&53u16.to_be_bytes());
    frame.extend_from_slice(&udp_length.to_be_bytes());
    frame.extend_from_slice(&0u16.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_udp(payload: &[u8], udp_length: u16) -> Result<packet::Packet, Skip> {
    packet::decode(pcap::LinkType::Ethernet, &udp_frame(payload, udp_length))
}

#[test]
fn declared_length_smaller_than_captured_is_rejected() {
    // F-1 / #627：声明 8（零载荷），实际带 20 字节载荷。
    // 修前：解码成零载荷报文（payload_len=0）且无跳过计数；修后：可见跳过。
    let result = decode_udp(&[0x75u8; 20], 8);
    assert_eq!(result.unwrap_err(), Skip::Malformed("udp-length"));
}

#[test]
fn declared_length_zero_is_rejected() {
    let result = decode_udp(&[0x75u8; 20], 0);
    assert_eq!(result.unwrap_err(), Skip::Malformed("udp-length"));
}

#[test]
fn declared_length_below_header_is_rejected() {
    let result = decode_udp(b"abc", 7);
    assert_eq!(result.unwrap_err(), Skip::Malformed("udp-length"));
}

#[test]
fn declared_length_exactly_matches_captured_is_decoded() {
    let packet = decode_udp(&[0x75u8; 20], 28).expect("长度一致应解码");
    assert_eq!(packet.payload_len, 20);
    assert_eq!(packet.transport.ports(), (1234, 53));
}

#[test]
fn declared_length_oversized_is_clamped_to_captured() {
    // 偏大方向沿用既有口径：抓包截断时按捕获长度钳取，不拒绝。
    let packet = decode_udp(b"xy", 65_535).expect("u16 上限声明应按捕获钳取");
    assert_eq!(packet.payload_len, 2);
}

#[test]
fn zero_payload_datagram_with_consistent_length_decodes() {
    // 长度=8 且捕获也只有 8 字节：合法零载荷，不能被新检查误伤。
    let packet = decode_udp(&[], 8).expect("合法零载荷 UDP 应解码");
    assert_eq!(packet.payload_len, 0);
}

#[test]
fn garbage_pcap_global_header_is_rejected() {
    // 非 ASCII / 畸形 pcap 包头：首 4 字节不是任何已知 magic。
    let garbage: Vec<u8> = (0..24u8).map(|byte| byte.wrapping_mul(0x5b)).collect();
    let error = pcap::Reader::new(&garbage).unwrap_err();
    assert!(matches!(error, PcapError::BadMagic(_)));

    let short: Vec<u8> = garbage[..10].to_vec();
    assert_eq!(
        pcap::Reader::new(&short).unwrap_err(),
        PcapError::ShortGlobalHeader(10)
    );
}
