//! 真机 wire 测试：起一个 `trit-lsp` 子进程，用 LSP 帧走完一次完整会话。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use trit_lsp::framing::{read_message, write_message};

fn send<W: Write>(writer: &mut W, message: Value) {
    write_message(writer, &message).expect("写帧");
}

fn read<R: BufRead>(reader: &mut R) -> Value {
    read_message(reader).expect("读帧").expect("对端提前 EOF")
}

/// 读到指定 id 的响应；途中的通知（诊断）跳过但保留计数。
fn read_until_id<R: BufRead>(reader: &mut R, id: i64) -> Value {
    for _ in 0..32 {
        let message = read(reader);
        if message.get("id").and_then(Value::as_i64) == Some(id) {
            return message;
        }
    }
    panic!("等不到 id={id} 的响应");
}

fn labels(message: &Value) -> Vec<String> {
    message["result"]["items"]
        .as_array()
        .expect("items 数组")
        .iter()
        .filter_map(|item| item.get("label").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[test]
fn speaks_lsp_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 trit-lsp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    // 1) initialize：能力声明 + 语义选择。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"initializationOptions": {"algebra": "bochvar"}},
        }),
    );
    let init = read_until_id(&mut stdout, 1);
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(
        init["result"]["capabilities"]["documentSymbolProvider"],
        true
    );
    assert_eq!(init["result"]["capabilities"]["textDocumentSync"], 1);

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );

    // 2) didOpen：干净文件不报诊断。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///demo.tri", "text": "# demo\n(a & b) | !c\n"}},
        }),
    );
    let diagnostics = read(&mut stdout);
    assert_eq!(diagnostics["method"], "textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["uri"], "file:///demo.tri");
    assert_eq!(diagnostics["params"]["diagnostics"], json!([]));

    // 3) hover 在 `&` 上：Bochvar 语义的真值表（沾 X 即 X）。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": "file:///demo.tri"},
                "position": {"line": 1, "character": 3},
            },
        }),
    );
    let hover = read_until_id(&mut stdout, 2);
    let markdown = hover["result"]["contents"]["value"]
        .as_str()
        .expect("markdown");
    assert!(markdown.contains("与（合取）"), "{markdown}");
    assert!(markdown.contains("bochvar"), "{markdown}");
    // Bochvar：沾 X 即 X；a=0 行是 0 / X / 0。
    assert!(markdown.contains("| 0 | 0 | X | 0 |"), "{markdown}");

    // 4) completion：运算符 / 常量 / 本文件变量都在。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": "file:///demo.tri"},
                "position": {"line": 1, "character": 0},
            },
        }),
    );
    let completion = read_until_id(&mut stdout, 3);
    let labels = labels(&completion);
    for expected in ["&", "|", "!", "X", "a", "b", "c"] {
        assert!(
            labels.iter().any(|l| l == expected),
            "缺 {expected}: {labels:?}"
        );
    }

    // 5) documentSymbol：三个变量。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "textDocument/documentSymbol",
            "params": {"textDocument": {"uri": "file:///demo.tri"}},
        }),
    );
    let symbols = read_until_id(&mut stdout, 4);
    let names: Vec<&str> = symbols["result"]
        .as_array()
        .expect("数组")
        .iter()
        .filter_map(|s| s.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, vec!["a", "b", "c"]);

    // 6) didChange 到坏输入：诊断立刻推回来。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {"uri": "file:///demo.tri", "version": 2},
                "contentChanges": [{"text": "a & )"}],
            },
        }),
    );
    let diagnostics = read(&mut stdout);
    let items = diagnostics["params"]["diagnostics"]
        .as_array()
        .expect("诊断数组");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["source"], "trit-lsp");

    // 7) shutdown → exit，进程正常退出。
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown"}),
    );
    let shutdown = read_until_id(&mut stdout, 5);
    assert_eq!(shutdown["result"], Value::Null);
    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "exit"}));
    drop(stdin);
    let status = child.wait().expect("等待子进程");
    assert!(status.success(), "退出码 {:?}", status.code());
}

#[test]
fn navigation_and_rename_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 trit-lsp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    let init = read_until_id(&mut stdout, 1);
    assert_eq!(init["result"]["capabilities"]["definitionProvider"], true);
    assert_eq!(init["result"]["capabilities"]["referencesProvider"], true);
    assert_eq!(
        init["result"]["capabilities"]["documentHighlightProvider"],
        true
    );
    assert_eq!(init["result"]["capabilities"]["renameProvider"], true);
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );

    // `(alpha & beta) | !alpha`：第 2 行（0 基）列 19 是第二个 alpha。
    let doc = json!({"uri": "file:///nav.tri"});
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///nav.tri", "text": "# demo
(alpha & beta) | !alpha
"}},
        }),
    );
    // didOpen 的诊断通知可能先到，跳过。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/definition",
            "params": {"textDocument": doc, "position": {"line": 1, "character": 19}},
        }),
    );
    let definition = read_until_id(&mut stdout, 2);
    assert_eq!(definition["result"]["uri"], "file:///nav.tri");
    assert_eq!(
        definition["result"]["range"]["start"]["character"], 1,
        "跳到第一个 alpha"
    );

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/references",
            "params": {"textDocument": doc, "position": {"line": 1, "character": 19},
                        "context": {"includeDeclaration": true}},
        }),
    );
    let references = read_until_id(&mut stdout, 3);
    assert_eq!(references["result"].as_array().unwrap().len(), 2);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/rename",
            "params": {"textDocument": doc, "position": {"line": 1, "character": 19}, "newName": "gamma"},
        }),
    );
    let rename = read_until_id(&mut stdout, 4);
    let edits = rename["result"]["changes"]["file:///nav.tri"]
        .as_array()
        .expect("changes 数组");
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "gamma"));

    // 重命名到常量词：返回 null（不是错误）。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/rename",
            "params": {"textDocument": doc, "position": {"line": 1, "character": 19}, "newName": "X"},
        }),
    );
    let rejected = read_until_id(&mut stdout, 5);
    assert!(rejected["result"].is_null());

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 6, "method": "shutdown", "params": null}),
    );
    let _ = read_until_id(&mut stdout, 6);
    drop(stdin);
    let _ = child.wait().expect("等待退出");
}
