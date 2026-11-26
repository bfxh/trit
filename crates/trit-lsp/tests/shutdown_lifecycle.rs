//! 第 40 轮 S7-2 回归：`shutdown` 之后服务端必须拒绝/丢弃后续请求与通知。
//!
//! 帧管道：`initialize → shutdown → didOpen → hover`。
//! - `shutdown` 正常应答 `null`；
//! - 关停后的 `didOpen` 不得回推 `publishDiagnostics`，文档不得被插入；
//! - 关停后的 `hover` 请求返回 JSON-RPC `-32600`（Invalid Request），协议形状不变；
//! - 最后 `exit` 正常退出。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use trit_lsp::framing::{read_message, write_message};
use trit_lsp::server::Server;

fn send<W: Write>(writer: &mut W, message: Value) {
    write_message(writer, &message).expect("写帧");
}

fn read<R: BufRead>(reader: &mut R) -> Value {
    read_message(reader).expect("读帧").expect("对端提前 EOF")
}

/// 读到指定 id 的响应；途中收集通知帧用于断言。
fn read_until_id<R: BufRead>(reader: &mut R, id: i64, notifications: &mut Vec<Value>) -> Value {
    for _ in 0..32 {
        let message = read(reader);
        if message.get("id").and_then(Value::as_i64) == Some(id) {
            return message;
        }
        notifications.push(message);
    }
    panic!("等不到 id={id} 的响应");
}

#[test]
fn post_shutdown_requests_and_notifications_are_rejected() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 trit-lsp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    // 1) initialize。
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    let mut notifications = Vec::new();
    let init = read_until_id(&mut stdout, 1, &mut notifications);
    assert!(init["result"].is_object(), "initialize 应答异常");

    // 2) shutdown：应答 result=null。
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
    );
    let shutdown = read_until_id(&mut stdout, 2, &mut notifications);
    assert_eq!(shutdown["result"], Value::Null);

    // 3) 关停后的 didOpen：通知被丢弃，不得回推诊断，文档不得入表。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///post-shutdown.tri", "text": "a & b"}},
        }),
    );

    // 4) 关停后的 hover：必须返回错误，而不是按已插入的文档查真值表。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": "file:///post-shutdown.tri"},
                "position": {"line": 0, "character": 2},
            },
        }),
    );
    let mut later_notifications = Vec::new();
    let hover = read_until_id(&mut stdout, 3, &mut later_notifications);

    let code = hover["error"]["code"]
        .as_i64()
        .expect("hover 必须是错误响应");
    assert_eq!(code, -32600, "关停后请求应返回 Invalid Request");
    assert!(hover["result"].is_null() || hover.get("result").is_none());

    // 关停后这条 didOpen 没有产生任何诊断通知。
    let diagnostics: Vec<&Value> = later_notifications
        .iter()
        .filter(|n| n["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert!(
        diagnostics.is_empty(),
        "关停后的 didOpen 不得回推诊断: {later_notifications:?}"
    );

    // 5) exit：收到过 shutdown，进程正常退出。
    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "exit"}));
    drop(stdin);
    let status = child.wait().expect("等待子进程");
    assert!(status.success(), "退出码 {:?}", status.code());
}

/// API 层直测：关停后通知被丢弃（返回空列表、不回诊断），其余请求返回 `-32600`。
#[test]
fn post_shutdown_notification_dropped_and_request_rejected_at_api_level() {
    let mut server = Server::new();
    server.request("initialize", &json!({})).unwrap();
    server.request("shutdown", &json!({})).unwrap();

    // didOpen 被丢弃：不回推诊断（基线会立刻发一条 publishDiagnostics）。
    let notifications = server.notification(
        "textDocument/didOpen",
        &json!({"textDocument": {"uri": "file:///x.tri", "text": "a & b"}}),
    );
    assert!(notifications.is_empty(), "关停后 didOpen 不得回推诊断");

    // didChange 同样被丢弃。
    server.notification(
        "textDocument/didChange",
        &json!({
            "textDocument": {"uri": "file:///x.tri"},
            "contentChanges": [{"text": "b & c"}],
        }),
    );

    let error = server
        .request(
            "textDocument/hover",
            &json!({"textDocument": {"uri": "file:///x.tri"}, "position": {"line": 0, "character": 2}}),
        )
        .expect_err("关停后 hover 必须拒绝");
    assert_eq!(error.code, -32600);

    // `shutdown` 请求本身不在拒绝之列。
    assert!(server.request("shutdown", &json!({})).is_ok());
}
