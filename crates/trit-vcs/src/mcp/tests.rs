use super::*;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn server(root: &Path) -> Mcp {
    Mcp::new(Repo::init(root).unwrap())
}

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    let response = server
        .dispatch(&rpc(
            1,
            "tools/call",
            json!({"name": name, "arguments": args}),
        ))
        .unwrap();
    response["result"].clone()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn tools_list_has_thirteen_tools_and_usage() {
    let root = temp("tools");
    let mut server = server(&root);
    let response = server.dispatch(&rpc(1, "tools/list", json!({}))).unwrap();
    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13, "工具数必须 ≤13 且与测试一致");
    let names: std::collections::BTreeSet<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 13, "工具名不能重复");
    for tool in tools {
        assert!(tool["description"].as_str().unwrap().len() > 4);
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
    let usage = &response["result"]["_meta"]["usage"];
    assert!(usage["bytes"].as_u64().unwrap() > 0);
    assert!(usage["tokens_est"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn initialize_reports_protocol() {
    let root = temp("init");
    let mut server = server(&root);
    let response = server.dispatch(&rpc(7, "initialize", json!({}))).unwrap();
    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(response["result"]["serverInfo"]["name"], "trit-vcs");
    assert_eq!(
        server.dispatch(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        None,
        "通知不应有响应"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn add_commit_log_and_status_roundtrip() {
    let root = temp("flow");
    std::fs::write(root.join("a.txt"), b"one").unwrap();
    let mut server = server(&root);

    let added = call(&mut server, "add", json!({"paths": ["a.txt"]}));
    assert_eq!(added["isError"], false, "{}", text_of(&added));
    let committed = call(
        &mut server,
        "commit",
        json!({"message": "first", "session": "qoder:it", "tags": ["vcs"],
               "allow_unclaimed": true}),
    );
    assert_eq!(committed["isError"], false, "{}", text_of(&committed));
    assert!(committed["_meta"]["usage"]["tokens_est"].as_u64().unwrap() > 0);

    let logged = call(&mut server, "log", json!({"tag": "vcs"}));
    let payload: Value = serde_json::from_str(&text_of(&logged)).unwrap();
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["commits"][0]["message"], "first");

    let status = call(&mut server, "status", json!({}));
    let payload: Value = serde_json::from_str(&text_of(&status)).unwrap();
    assert_eq!(payload["changes"].as_array().unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn commit_without_allowance_is_a_tool_error() {
    let root = temp("denied");
    std::fs::write(root.join("a.txt"), b"one").unwrap();
    let mut server = server(&root);
    call(&mut server, "add", json!({"paths": ["a.txt"]}));
    let denied = call(
        &mut server,
        "commit",
        json!({"message": "x", "session": "qoder:it"}),
    );
    assert_eq!(denied["isError"], true);
    assert!(text_of(&denied).contains("未认领"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unknown_method_and_tool_are_reported() {
    let root = temp("unknown");
    let mut server = server(&root);
    let response = server.dispatch(&rpc(2, "no/such", json!({}))).unwrap();
    assert_eq!(response["error"]["code"], -32601);
    let tool = call(&mut server, "nope", json!({}));
    assert_eq!(tool["isError"], true);
    assert!(text_of(&tool).contains("未知工具"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn where_and_tests_use_techmap() {
    let root = temp("techmap");
    let fixture = root.join("techmap.json");
    std::fs::write(
        &fixture,
        r#"{"entries":[{"id":"demo","title":"demo","tags":["vcs"],"paths":["a.txt"],
            "tests":["crates/trit-vcs/tests/mcp_cli.rs::serve_loop_answers_initialize"],
            "status":"landed"}]}"#,
    )
    .unwrap();
    let mut server = server(&root);
    let hit = call(
        &mut server,
        "where",
        json!({"needle": "a.txt", "techmap": fixture.to_string_lossy()}),
    );
    let payload: Value = serde_json::from_str(&text_of(&hit)).unwrap();
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["entries"][0]["id"], "demo");

    let tests = call(
        &mut server,
        "tests",
        json!({"needle": "demo", "techmap": fixture.to_string_lossy()}),
    );
    let payload: Value = serde_json::from_str(&text_of(&tests)).unwrap();
    assert_eq!(
        payload["commands"][0],
        "cargo test -p trit-vcs serve_loop_answers_initialize"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn token_estimate_is_ascii_quarter_and_cjk_one() {
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcdefgh"), 2);
    assert_eq!(estimate_tokens("中文"), 2);
    assert!(estimate_tokens("混合 mixed") >= 2);
}
