//! 命令行端到端：MCP stdio 循环（initialize / tools/list / add / commit / log）。

use std::io::Write;
use std::process::{Command, Stdio};

use trit_vcs::repo::Repo;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-mcpcli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-vcs")
}

#[test]
fn serve_loop_answers_initialize() {
    let root = temp("loop");
    let _repo = Repo::init(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"hello").unwrap();

    let mut child = Command::new(bin())
        .arg("mcp")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 mcp 失败");
    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add","arguments":{"paths":["a.txt"]}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"commit","arguments":{"message":"first","session":"qoder:it","allow_unclaimed":true}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"log","arguments":{}}}"#,
        ] {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 5, "通知不应产生响应");

    assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
    let tools = lines[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
    assert!(
        lines[1]["result"]["_meta"]["usage"]["tokens_est"]
            .as_u64()
            .unwrap()
            > 0
    );

    for result in lines.iter().take(5).skip(2).map(|line| &line["result"]) {
        assert_eq!(result["isError"], false, "{result}");
        assert!(result["_meta"]["usage"]["tokens_est"].as_u64().unwrap() > 0);
    }
    let log: serde_json::Value =
        serde_json::from_str(lines[4]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(log["count"], 1);
    let _ = std::fs::remove_dir_all(&root);
}
