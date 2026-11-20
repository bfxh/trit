//! MCP stdio 的帧行字节顶：超过 16 MiB 的行在解析前被拒绝并丢弃到换行。
//!
//! 变异放行后，16 MB 的 `x` 会进 `serde_json`，回的是 `-32700` 而不是 `-32600`。

use trit_wspace::mcp::{MAX_LINE_BYTES, serve};

#[test]
fn oversize_line_is_rejected_without_parsing_it() {
    let mut input = vec![b'x'; MAX_LINE_BYTES + 16];
    input.push(b'\n');
    input.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    input.push(b'\n');
    let mut out = Vec::new();
    serve(std::io::Cursor::new(input), &mut out).expect("serve 不应失败");
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("帧行超过上限"), "实得: {text}");
    assert!(text.contains("-32600"), "应回无效请求错误: {text}");
    assert!(text.contains("\"result\""), "后续正常帧仍应被处理: {text}");
}
