use super::*;
use crate::lsp::LspBridge;
use crate::{Journal, NodeId, Trit};
use std::io::Cursor;

fn frame(body: &str) -> Vec<u8> {
    encode_frame(body)
}

#[test]
fn frame_roundtrip_and_eof() {
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(&frame("{\"a\":1}"));
    wire.extend_from_slice(&frame("second"));
    let mut r = Cursor::new(wire);
    assert_eq!(read_frame(&mut r).unwrap(), Some("{\"a\":1}".to_string()));
    assert_eq!(read_frame(&mut r).unwrap(), Some("second".to_string()));
    assert_eq!(read_frame(&mut r).unwrap(), None, "EOF 干净结束");
}

#[test]
fn bad_content_length_is_rejected() {
    let mut r = Cursor::new(b"Content-Length: abc\r\n\r\n{}".to_vec());
    assert!(read_frame(&mut r).is_err());
    let mut r = Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec());
    assert!(read_frame(&mut r).is_err(), "缺 Content-Length 应拒绝");
}

#[test]
fn oversized_header_line_is_rejected() {
    let long = "X".repeat(MAX_HEADER_LINE_BYTES + 10);
    let mut r = Cursor::new(format!("{long}\r\n\r\n").into_bytes());
    let err = read_frame(&mut r).unwrap_err();
    assert!(err.to_string().contains("帧头行超过上限"));
}

#[test]
fn oversized_content_length_is_rejected_before_allocation() {
    // usize::MAX 的 Content-Length 必须在分配前被硬顶拦下。
    let mut r = Cursor::new(b"Content-Length: 18446744073709551615\r\n\r\n".to_vec());
    let err = read_frame(&mut r).unwrap_err();
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

#[test]
fn json_escape_covers_specials() {
    assert_eq!(json_escape("a\"b\\c\nd\te"), "\"a\\\"b\\\\c\\nd\\te\"");
    assert_eq!(json_escape("\u{1}"), "\"\\u0001\"");
}

/// 假 LSP 服务器：预录 initialize 应答 + 两条诊断通知。
fn fake_server_wire() -> Vec<u8> {
    let mut wire = Vec::new();
    wire.extend_from_slice(&frame(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"capabilities\":{\"textDocumentSync\":1}}}",
    ));
    wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"file:///w/src/lib.rs\",\"diagnostics\":[{\"range\":{},\"severity\":1,\"message\":\"borrow checker 报错\"}]}}",
        ));
    wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"file:///w/src/other.rs\",\"diagnostics\":[]}}",
        ));
    wire
}

#[test]
fn session_full_flow_initialize_open_pump_and_ledger() {
    let wire = fake_server_wire();

    let mut session = LspSession::new(Cursor::new(wire.clone()), Vec::new());
    let resp = session.initialize(None).unwrap();
    assert!(resp.contains("capabilities"), "应答应含 capabilities");
    session
        .did_open("file:///w/src/lib.rs", "fn main() {}", "rust")
        .unwrap();
    let frames = session.pump(10).unwrap();
    assert_eq!(frames.len(), 2, "两条诊断通知都应泵到");
    assert!(frames[0].contains("publishDiagnostics"));

    // 全链路落账：pump 帧 → LspBridge → journal
    let p = std::env::temp_dir().join(format!("trit-daemon-{}.journal", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let mut j = Journal::create_as(&p, "daemon", 0).unwrap();
    let lib = j
        .spawn(NodeId(0), crate::NodeKind::Delta, "lib.rs")
        .unwrap();
    j.record(lib, Trit::Unknown, None, "打开待检").unwrap();
    {
        let mut bridge = LspBridge {
            journal: &mut j,
            uri_map: Default::default(),
        };
        bridge.map_uri("file:///w/src/lib.rs", lib);
        for f in &frames {
            bridge.process(f).unwrap();
        }
    }
    assert_eq!(j.dag().node(lib).verdict, Trit::Vulnerable, "E1 → -1");
    drop(j.into_dag().unwrap());
    let _ = std::fs::remove_file(&p);
}

#[test]
fn request_buffers_intermediate_notifications() {
    let mut wire = Vec::new();
    wire.extend_from_slice(&frame(
        "{\"jsonrpc\":\"2.0\",\"method\":\"window/logMessage\",\"params\":{\"message\":\"hi\"}}",
    ));
    wire.extend_from_slice(&frame(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}",
    ));
    let mut session = LspSession::new(Cursor::new(wire), Vec::new());
    let resp = session.request("foo/bar", "{}").unwrap();
    assert!(resp.contains("\"ok\":true"));
    let drained = session.pump(10).unwrap();
    assert_eq!(drained.len(), 1, "中途通知应缓存并在 pump 时吐出");
    assert!(drained[0].contains("logMessage"));
}

#[test]
fn eof_during_request_is_broken_pipe() {
    let mut session = LspSession::new(Cursor::new(Vec::new()), Vec::new());
    assert!(session.request("foo/bar", "{}").is_err());
}
