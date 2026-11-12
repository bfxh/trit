use super::header::frame_len_ok;
use super::*;
use std::io::Cursor;

#[test]
fn round_trip() {
    let mut buf = Vec::new();
    let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
    write_message(&mut buf, &msg).expect("写");
    let mut cur = Cursor::new(buf);
    let got = read_message(&mut cur).expect("读").expect("有帧");
    assert_eq!(got, msg);
    assert!(read_message(&mut cur).expect("EOF 正常").is_none());
}

#[test]
fn eof_without_frame_is_none() {
    let mut cur = Cursor::new(Vec::new());
    assert!(read_message(&mut cur).expect("EOF").is_none());
}

#[test]
fn missing_length_is_error() {
    let mut cur = Cursor::new(b"Content-Type: application/json\r\n\r\n{}".to_vec());
    assert!(read_message(&mut cur).is_err());
}

#[test]
fn truncated_body_is_error() {
    let mut cur = Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec());
    assert!(read_message(&mut cur).is_err());
}

#[test]
fn oversized_header_line_is_rejected() {
    let long = "X".repeat(MAX_HEADER_LINE_BYTES + 10);
    let mut cur = Cursor::new(format!("{long}\r\n\r\n").into_bytes());
    let err = read_message(&mut cur).unwrap_err();
    assert!(err.to_string().contains("帧头行超过上限"));
}

#[test]
fn oversized_content_length_is_rejected_before_allocation() {
    // usize::MAX 的 Content-Length 必须在分配前被硬顶拦下。
    let mut cur = Cursor::new(b"Content-Length: 18446744073709551615\r\n\r\n".to_vec());
    let err = read_message(&mut cur).unwrap_err();
    assert!(err.to_string().contains("Content-Length 超过上限"));
}

#[test]
fn frame_len_cap_boundary() {
    // 纯尺寸门：恰好等于上限放行，超一个字节拒绝（不分配 16 MiB 去验边界）。
    assert!(frame_len_ok(0));
    assert!(frame_len_ok(MAX_FRAME_BYTES));
    assert!(!frame_len_ok(MAX_FRAME_BYTES + 1));
    assert!(!frame_len_ok(usize::MAX));
}
