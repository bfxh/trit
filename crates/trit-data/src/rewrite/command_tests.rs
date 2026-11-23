//! 外部命令重写器的用例：argv 拆分、stdin→stdout、超时 fail-closed。

use super::command::{CommandRewriter, run_command, split_command};
use std::time::Duration;

#[test]
fn split_command_keeps_quoted_spaces() {
    assert_eq!(
        split_command("srv --style formal").unwrap(),
        vec!["srv", "--style", "formal"]
    );
    assert_eq!(
        split_command(r#"pwsh -NoProfile -Command "Write-Output 'a b'""#).unwrap(),
        vec!["pwsh", "-NoProfile", "-Command", "Write-Output 'a b'"]
    );
    assert!(split_command("").unwrap_err().contains("命令为空"));
    assert!(split_command(r#"srv "unterminated"#).is_err());
    // 空引号也算一个参数，不能被空白吃掉
    assert_eq!(split_command(r#"srv """#).unwrap(), vec!["srv", ""]);
}

/// 平台各取一个「把 stdin 原样吐到 stdout」的最小命令。
fn echo_argv() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "[Console]::In.ReadToEnd()".into(),
            ],
        )
    } else {
        ("sh".into(), vec!["-c".into(), "cat".into()])
    }
}

/// 「睡过头」的命令，用来量超时分支。
fn sleep_argv() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 5".into(),
            ],
        )
    } else {
        ("sh".into(), vec!["-c".into(), "sleep 5".into()])
    }
}

#[test]
fn command_rewriter_pipes_stdin_to_stdout() {
    let (prog, args) = echo_argv();
    let out = run_command(
        &prog,
        &args,
        "pytest -q got 42 passed",
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(out, "pytest -q got 42 passed");
}

#[test]
fn command_rewriter_fails_closed_on_timeout() {
    let (prog, args) = sleep_argv();
    let err = run_command(&prog, &args, "x", Duration::from_millis(300)).unwrap_err();
    assert!(err.starts_with("timeout:"), "{err}");
    assert!(CommandRewriter::parse("  ", Duration::from_secs(5)).is_err());
    assert!(CommandRewriter::parse("srv", Duration::from_secs(0)).is_err());
}
