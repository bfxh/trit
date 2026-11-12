use super::*;

fn open(server: &mut Server, text: &str) -> Value {
    let notifications = server.notification(
        "textDocument/didOpen",
        &json!({"textDocument": {"uri": "file:///a.tri", "text": text}}),
    );
    assert_eq!(notifications.len(), 1);
    notifications[0].clone()
}

#[test]
fn initialize_reports_capabilities() {
    let mut server = Server::new();
    let result = server
        .request(
            "initialize",
            &json!({"initializationOptions": {"algebra": "b3"}}),
        )
        .expect("初始化");
    assert_eq!(result["capabilities"]["hoverProvider"], true);
    assert_eq!(result["capabilities"]["textDocumentSync"], 1);
    assert_eq!(server.algebra, Algebra::Bochvar);
}

#[test]
fn did_open_publishes_diagnostics() {
    let mut server = Server::new();
    let notification = open(&mut server, "a & )");
    let diagnostics = &notification["params"]["diagnostics"];
    assert_eq!(diagnostics.as_array().expect("数组").len(), 1);
}

#[test]
fn hover_and_completion() {
    let mut server = Server::new();
    open(&mut server, "a & b");
    let hover = server
        .request(
            "textDocument/hover",
            &json!({
                "textDocument": {"uri": "file:///a.tri"},
                "position": {"line": 0, "character": 2},
            }),
        )
        .expect("悬停");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .expect("md")
            .contains("与")
    );
    let completion = server
            .request(
                "textDocument/completion",
                &json!({"textDocument": {"uri": "file:///a.tri"}, "position": {"line": 0, "character": 0}}),
            )
            .expect("补全");
    assert!(!completion["items"].as_array().expect("数组").is_empty());
}

#[test]
fn unknown_request_is_method_not_found() {
    let mut server = Server::new();
    let error = server
        .request("textDocument/formatting", &json!({}))
        .expect_err("应报错");
    assert_eq!(error.code, -32601);
}

#[test]
fn did_close_clears_diagnostics() {
    let mut server = Server::new();
    open(&mut server, "a & )");
    let notifications = server.notification(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": "file:///a.tri"}}),
    );
    assert_eq!(notifications[0]["params"]["diagnostics"], json!([]));
}

#[test]
fn retention_problem_boundaries() {
    // 尺寸：恰好到顶放行，超一字节拒绝。
    assert_eq!(retention_problem(0, false, MAX_DOC_BYTES), None);
    assert_eq!(
        retention_problem(0, false, MAX_DOC_BYTES + 1),
        Some("文档超过尺寸上限")
    );
    assert_eq!(
        retention_problem(0, false, usize::MAX),
        Some("文档超过尺寸上限")
    );
    // 数量：新文档到顶拒绝；已存在文档到顶仍可更新。
    assert_eq!(
        retention_problem(MAX_OPEN_DOCS, false, 0),
        Some("打开的文档数超过上限")
    );
    assert_eq!(retention_problem(MAX_OPEN_DOCS, true, 0), None);
    assert_eq!(retention_problem(MAX_OPEN_DOCS - 1, false, 0), None);
}

#[test]
fn did_open_rejects_oversized_text() {
    let mut server = Server::new();
    let huge = "a".repeat(MAX_DOC_BYTES + 1);
    let notifications = server.notification(
        "textDocument/didOpen",
        &json!({"textDocument": {"uri": "file:///big.tri", "text": huge}}),
    );
    // 超尺寸文档在保留前被拒：不发布诊断，也不进 texts。
    assert!(notifications.is_empty());
    assert!(server.texts.is_empty());
}

#[test]
fn did_open_rejects_over_count() {
    let mut server = Server::new();
    for i in 0..MAX_OPEN_DOCS {
        let uri = format!("file:///{i}.tri");
        let notifications = server.notification(
            "textDocument/didOpen",
            &json!({"textDocument": {"uri": uri, "text": "a & b"}}),
        );
        assert_eq!(notifications.len(), 1);
    }
    assert_eq!(server.texts.len(), MAX_OPEN_DOCS);
    // 第 257 个新文档被拒。
    let notifications = server.notification(
        "textDocument/didOpen",
        &json!({"textDocument": {"uri": "file:///overflow.tri", "text": "a"}}),
    );
    assert!(notifications.is_empty());
    assert_eq!(server.texts.len(), MAX_OPEN_DOCS);
    assert!(!server.texts.contains_key("file:///overflow.tri"));
}

#[test]
fn did_change_rejects_over_count_for_new_uri() {
    let mut server = Server::new();
    for i in 0..MAX_OPEN_DOCS {
        let uri = format!("file:///{i}.tri");
        server.notification(
            "textDocument/didOpen",
            &json!({"textDocument": {"uri": uri, "text": "a & b"}}),
        );
    }
    // 到顶后，didChange 引入新 uri 同样被数量守卫挡下。
    server.notification(
        "textDocument/didChange",
        &json!({
            "textDocument": {"uri": "file:///new.tri"},
            "contentChanges": [{"text": "a | b"}],
        }),
    );
    assert_eq!(server.texts.len(), MAX_OPEN_DOCS);
    assert!(!server.texts.contains_key("file:///new.tri"));
}
