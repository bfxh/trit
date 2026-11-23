//! `trit-data rewrite` 的端到端门：keep / 保真失败 / 超时三支都必须按 fail-closed 落账。
//!
//! 用平台原生 shell 做外部重写器（Unix `sh`、Windows `powershell`），不改产品代码；
//! 断言读 `rewrite.report.json` 与 `rewrite.failed.jsonl`，不依赖 stdout 文案。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use trit_data::rewrite::to_json;
use trit_data::schema::{Cost, Outcome, Step, Trajectory};
use trit_learn::json::{Json, read_json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("时钟")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("trit-data-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_line() -> String {
    let repeated = "重复句子。".repeat(80);
    let trajectory = Trajectory {
        schema: 1,
        task_id: "rewrite-fixture".to_string(),
        source: Some("fixture".to_string()),
        license: Some("CC0-1.0".to_string()),
        model: "m".to_string(),
        harness: Some("fixture".to_string()),
        steps: vec![
            Step {
                index: 0,
                role: "assistant".to_string(),
                tool: Some("bash".to_string()),
                args: Some("{\"cmd\":\"ls\"}".to_string()),
                observation: Some("列出了三个子目录".to_string()),
                thought: Some("先看目录".to_string()),
                error: false,
            },
            Step {
                index: 1,
                role: "assistant".to_string(),
                tool: Some("finish".to_string()),
                args: Some("{}".to_string()),
                observation: Some("完成".to_string()),
                thought: Some("结尾".to_string()),
                error: false,
            },
        ],
        outcome: Outcome {
            verifier: "manual".to_string(),
            passed: true,
            evidence: vec![format!("sha256:{}", "a".repeat(64))],
            final_text: repeated,
        },
        cost: Cost {
            tokens_in: 100,
            tokens_out: 50,
            tool_calls: 2,
            ms: 10,
        },
        labels: Vec::new(),
    };
    to_json(&trajectory).dump()
}

/// 返回「命令前缀」，真正调用时拼成 `{prefix} {mode}`。
#[cfg(unix)]
fn rewriter(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("rewriter.sh");
    let script = "#!/bin/sh\ncase \"$1\" in\n  keep) cat ;;\n  drop) cat >/dev/null; printf 'x' ;;\n  sleep) cat >/dev/null; sleep 5; printf 'x' ;;\nesac\n";
    fs::write(&path, script).expect("写重写器");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    format!("\"{}\"", path.display())
}

#[cfg(windows)]
fn rewriter(dir: &Path) -> String {
    let path = dir.join("rewriter.ps1");
    let script = "param([string]$Mode)\n$text = [Console]::In.ReadToEnd()\nswitch ($Mode) {\n  'keep' { Write-Output $text }\n  'drop' { Write-Output 'x' }\n  'sleep' { Start-Sleep -Seconds 5; Write-Output 'x' }\n}\n";
    fs::write(&path, script).expect("写重写器");
    format!("powershell -NoProfile -File \"{}\" -Mode", path.display())
}

fn rewrite(input: &Path, out: &Path, prefix: &str, mode: &str, timeout: &str) {
    let status = Command::new(env!("CARGO_BIN_EXE_trit-data"))
        .args([
            "rewrite",
            "--jsonl",
            &input.display().to_string(),
            "--cmd",
            &format!("{prefix} {mode}"),
            "--timeout",
            timeout,
            "--out",
            &out.display().to_string(),
        ])
        .output()
        .expect("启动 trit-data rewrite");
    assert!(
        status.status.success(),
        "rewrite 退出失败：{}",
        String::from_utf8_lossy(&status.stderr)
    )
}

fn count(out: &Path, key: &str) -> u64 {
    let report = read_json(&out.join("rewrite.report.json")).expect("读报告");
    report
        .get("counts")
        .and_then(|counts| counts.get(key))
        .and_then(Json::as_u64)
        .unwrap_or_else(|| panic!("报告缺 counts.{key}"))
}

fn first_failure(out: &Path) -> Json {
    let text = fs::read_to_string(out.join("rewrite.failed.jsonl")).expect("读失败册");
    let line = text.lines().next().expect("失败册为空");
    Json::parse(line).expect("失败册 JSON")
}

fn setup(tag: &str) -> (TempDir, PathBuf, String) {
    let dir = TempDir::new(tag);
    let input = dir.path().join("in.jsonl");
    fs::write(&input, fixture_line() + "\n").expect("写输入");
    let prefix = rewriter(dir.path());
    (dir, input, prefix)
}

#[test]
fn keep_branch_rewrites_and_passes_fidelity() {
    let (dir, input, prefix) = setup("keep");
    let out = dir.path().join("out");
    rewrite(&input, &out, &prefix, "keep", "30");
    assert_eq!(count(&out, "records"), 1);
    assert_eq!(count(&out, "rewritten"), 1);
    assert_eq!(count(&out, "fidelity_failed"), 0);
    assert_eq!(count(&out, "failed"), 0);
}

#[test]
fn drop_branch_fails_closed_on_fidelity() {
    let (dir, input, prefix) = setup("drop");
    let out = dir.path().join("out");
    rewrite(&input, &out, &prefix, "drop", "30");
    assert_eq!(count(&out, "rewritten"), 0);
    assert_eq!(count(&out, "fidelity_failed"), 1);
    assert_eq!(count(&out, "failed"), 0);
    let failed = first_failure(&out);
    let reason = failed
        .get("rewrite")
        .and_then(|rewrite| rewrite.get("reason"))
        .and_then(Json::as_str);
    assert_eq!(reason, Some("fidelity"));
    let ratio_ok = failed
        .get("rewrite")
        .and_then(|rewrite| rewrite.get("length_ratio_ok"))
        .and_then(Json::as_bool);
    assert_eq!(ratio_ok, Some(false));
}

#[test]
fn timeout_branch_fails_closed_and_keeps_original() {
    let (dir, input, prefix) = setup("timeout");
    let out = dir.path().join("out");
    rewrite(&input, &out, &prefix, "sleep", "1");
    assert_eq!(count(&out, "rewritten"), 0);
    assert_eq!(count(&out, "failed"), 1);
    assert_eq!(count(&out, "fidelity_failed"), 0);
    let failed = first_failure(&out);
    let rewrite = failed.get("rewrite").expect("rewrite 字段");
    assert_eq!(
        rewrite.get("reason").and_then(Json::as_str),
        Some("timeout")
    );
    let detail = rewrite.get("detail").and_then(Json::as_str).unwrap_or("");
    assert!(detail.starts_with("timeout:"), "detail={detail}");
}
