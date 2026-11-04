//! 命令行端到端：智能体被拒之后，一次调用就能拿到可直接执行的下一步；读数能压预算。
//!
//! 这里的每条断言都对着"少一轮往返/少猜参数"这个目的，不是为了让覆盖率好看。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-vcs")
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(root)
        .output()
        .expect("启动 trit-vcs 失败")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-ergo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 写一份只覆盖 `a.txt` 的认领表（JSON 数组），owner 是谁由参数决定。
fn claims_file(dir: &Path, owner: &str) -> PathBuf {
    let path = dir.join("claims.json");
    fs::write(
        &path,
        format!(
            r#"[{{"id":"c1","owner":"{owner}","paths":["a.txt","b.txt"],"acquired_at":1,"expires_at":4000000000}}]"#
        ),
    )
    .unwrap();
    path
}

fn seed(root: &Path) {
    assert!(run(root, &["init"]).status.success());
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    assert!(run(root, &["add", "a.txt", "b.txt"]).status.success());
}

#[test]
fn rejection_ships_the_command_that_clears_it() {
    let root = temp("remediation");
    seed(&root);

    // 别人占着 a.txt ⇒ 提交被判冲突
    let claims = claims_file(&root, "zcode:other");
    let claims_text = claims.to_string_lossy().to_string();
    let blocked = run(
        &root,
        &[
            "commit",
            "-m",
            "改一点",
            "--owner",
            "qoder:me",
            "--claims",
            &claims_text,
            "--json",
        ],
    );
    assert!(!blocked.status.success(), "撞车必须判红");
    let err_text = stderr(&blocked);
    assert!(err_text.contains("FAIL"), "失败要带 FAIL 前缀：{err_text}");
    assert!(
        err_text.contains("下一步 1."),
        "必须给编号的下一步：{err_text}"
    );
    assert!(
        err_text.contains("--who qoder:me"),
        "下一步里要带上我自己的会话名：{err_text}"
    );

    let payload: serde_json::Value = serde_json::from_str(&stdout(&blocked)).unwrap_or_else(|e| {
        panic!(
            "--json 下失败也必须是纯 JSON 对象：{e}，{}",
            stdout(&blocked)
        )
    });
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["kind"], "claim_conflict");
    let fixes = payload["remediation"].as_array().unwrap();
    assert!(!fixes.is_empty(), "被拒时必须给至少一条可执行动作");
    assert!(
        fixes[0]
            .as_str()
            .unwrap()
            .contains("agent_claims.py gate --who qoder:me"),
        "第一条应当是先去看谁挡着：{fixes:?}"
    );

    // 同一个请求换我自己认领（不冲突）⇒ 预检放行，然后真提交也放行
    let mine = claims_file(&root, "qoder:me");
    let mine_text = mine.to_string_lossy().to_string();
    let pre = run(
        &root,
        &[
            "preflight",
            "-m",
            "改一点",
            "--owner",
            "qoder:me",
            "--claims",
            &mine_text,
            "--json",
        ],
    );
    assert!(pre.status.success(), "{}", stderr(&pre));
    let facts: serde_json::Value = serde_json::from_str(&stdout(&pre)).unwrap();
    assert_eq!(facts["verdict"], "allowed");
    assert_eq!(facts["facts"]["size"]["files"], 2);
    assert_eq!(facts["facts"]["staged_paths"], 2);
    assert!(
        run(&root, &["log", "--json"]).stdout.to_vec() == b"[]\n".to_vec(),
        "预检不该留下任何提交"
    );

    // commit --dry-run 与 preflight 是同一条判决（同样放行、同样不落盘）
    let dry = run(
        &root,
        &[
            "commit",
            "--dry-run",
            "-m",
            "改一点",
            "--owner",
            "qoder:me",
            "--claims",
            &mine_text,
            "--json",
        ],
    );
    assert!(dry.status.success(), "{}", stderr(&dry));
    let dry_facts: serde_json::Value = serde_json::from_str(&stdout(&dry)).unwrap();
    assert_eq!(dry_facts["verdict"], "allowed");
    assert_eq!(
        dry_facts["facts"]["tree"], facts["facts"]["tree"],
        "两个入口必须指同一棵树"
    );
    assert!(
        run(&root, &["log", "--json"]).stdout.to_vec() == b"[]\n".to_vec(),
        "--dry-run 也不能落提交"
    );

    let committed = run(
        &root,
        &[
            "commit",
            "-m",
            "改一点",
            "--owner",
            "qoder:me",
            "--claims",
            &mine_text,
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn budget_caps_rows_and_says_how_many_it_dropped() {
    let root = temp("budget");
    seed(&root);
    for i in 0..30 {
        fs::write(root.join("a.txt"), format!("a {i}\n")).unwrap();
        let commit = run(&root, &["add", "a.txt", "b.txt"]);
        assert!(commit.status.success(), "{}", stderr(&commit));
        let committed = run(
            &root,
            &[
                "commit",
                "-m",
                &format!("提交 {i}：这条说明写得稍长一点，方便看预算效果"),
                "--owner",
                "qoder:me",
                "--allow-unclaimed",
            ],
        );
        assert!(committed.status.success(), "{}", stderr(&committed));
    }

    let full: serde_json::Value =
        serde_json::from_str(&stdout(&run(&root, &["log", "--json"]))).unwrap();
    assert_eq!(full.as_array().unwrap().len(), 30);

    let capped = run(&root, &["log", "--json", "--budget", "300"]);
    assert!(capped.status.success(), "{}", stderr(&capped));
    let rows: serde_json::Value = serde_json::from_str(&stdout(&capped)).unwrap();
    let kept = rows.as_array().unwrap().len();
    assert!(kept < 30, "预算内必须丢行：留了 {kept}");
    let note = stderr(&capped);
    assert!(
        note.contains("note:") && note.contains(&format!("丢了 {}/30 行", 30 - kept)),
        "顶层数组没地方写 truncation ⇒ 必须在 stderr 喊出来：{note}"
    );

    // 对象型读数把丢了多少写进 JSON 本身（前提是真丢了行）
    for i in 0..24 {
        fs::write(
            root.join(format!("extra{i:02}.txt")),
            format!("多出来的文件 {i}\n"),
        )
        .unwrap();
    }
    let status = run(&root, &["status", "--json", "--budget", "120"]);
    assert!(status.status.success(), "{}", stderr(&status));
    let value: serde_json::Value = serde_json::from_str(&stdout(&status)).unwrap();
    let dropped = value["truncation"]["dropped"].as_u64().unwrap_or(0);
    assert!(dropped >= 1, "24 条改动压不进 120 元：{}", stdout(&status));
    assert_eq!(value["truncation"]["budget"], 120);
    assert!(
        value["changes"].as_array().unwrap().len() + dropped as usize == 24,
        "只丢整行，不该改行数"
    );

    let zero = run(&root, &["log", "--json", "--budget", "0"]);
    assert!(!zero.status.success(), "--budget 0 是歧义，必须判失败");
    let _ = fs::remove_dir_all(&root);
}
