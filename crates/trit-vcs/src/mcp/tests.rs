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
    // schema 是每个智能体每次会话的固定开销 ⇒ 上界当门钉住（实测 4,146 B / 1,111 元，2026-11-05）。
    // 加参数可以，别把同一段说明抄 13 份：那条中文抄开一次就是 +192 元。
    assert!(
        usage["tokens_est"].as_u64().unwrap() <= 1_300,
        "tools/list 的 schema 涨到 {} 元，超过上界：把重复说明收进一处",
        usage["tokens_est"].as_u64().unwrap()
    );
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
    assert_eq!(estimate("abcd"), 1);
    assert_eq!(estimate("abcdefgh"), 2);
    assert_eq!(estimate("中文"), 2);
    assert!(estimate("混合 mixed") >= 2);
}

#[test]
fn every_tool_advertises_budget_and_commit_advertises_dry_run() {
    let root = temp("schema-budget");
    let mut server = server(&root);
    let response = server
        .dispatch(&rpc(1, "tools/list", json!({})))
        .expect("tools/list");
    let tools = response["result"]["tools"].as_array().unwrap();
    for tool in tools {
        assert_eq!(
            tool["inputSchema"]["properties"]["budget"]["type"], "integer",
            "{} 没声明 budget 参数",
            tool["name"]
        );
    }
    let commit = tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some("commit"))
        .unwrap();
    assert_eq!(
        commit["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dry_run_writes_nothing_and_budget_caps_the_reading() {
    let root = temp("budget-run");
    let mut server = server(&root);
    std::fs::write(root.join("a.txt"), "一行\n").unwrap();
    let added = call(&mut server, "add", json!({"paths": ["a.txt"]}));
    assert_eq!(added["isError"], json!(false), "{added}");

    // 预检：说放行，但一条提交都不许落
    let dry = call(
        &mut server,
        "commit",
        json!({"message": "预检一下", "session": "qoder:mcp", "allow_unclaimed": true, "dry_run": true}),
    );
    assert_eq!(dry["isError"], json!(false), "{}", text_of(&dry));
    let text = text_of(&dry);
    assert!(text.contains("\"dry_run\":true"), "{text}");
    // 这个工具不接受 claims ⇒ 放行只能走 allow_unclaimed，判决面如实写 unclaimed
    assert!(text.contains("\"verdict\":\"unclaimed\""), "{text}");
    assert!(text.contains("\"files\":1"), "预检必须带上规模读数：{text}");
    let log = call(&mut server, "log", json!({}));
    let rows: Value = serde_json::from_str(&text_of(&log)).unwrap();
    assert_eq!(
        rows["commits"].as_array().unwrap().len(),
        0,
        "dry_run 不该落提交"
    );

    // 真提交 6 条，再用小预算读回来：必须真压进预算，并说丢了几行
    for i in 0..6 {
        std::fs::write(
            root.join("a.txt"),
            format!("第 {i} 版内容，稍微长一点好让预算有得压\n"),
        )
        .unwrap();
        assert_eq!(
            call(&mut server, "add", json!({"paths": ["a.txt"]}))["isError"],
            json!(false)
        );
        let committed = call(
            &mut server,
            "commit",
            json!({"message": format!("提交 {i}"), "session": "qoder:mcp", "allow_unclaimed": true}),
        );
        assert_eq!(
            committed["isError"],
            json!(false),
            "{}",
            text_of(&committed)
        );
    }

    let capped = call(&mut server, "log", json!({"budget": 120}));
    let payload: Value = serde_json::from_str(&text_of(&capped)).unwrap();
    assert!(
        payload["truncation"]["dropped"].as_u64().unwrap() >= 1,
        "120 元装不下 6 条提交：{}",
        text_of(&capped)
    );
    let kept = payload["commits"].as_array().unwrap().len();
    assert!(kept < 6, "压完必须少几行：留了 {kept}");
    assert!(
        estimate(&text_of(&capped)) <= 120,
        "读数本身必须真在预算内：{}",
        estimate(&text_of(&capped))
    );
    let _ = std::fs::remove_dir_all(&root);
}
