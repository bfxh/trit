//! 命令行端到端：publish 到中央仓 → 另一侧只 pull 一个目录 → 重复 pull 传 0 字节 → 本地改动挡路。

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
    let dir = std::env::temp_dir().join(format!("trit-vcs-central-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn publish_then_sparse_pull() {
    let root = temp("flow");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    assert!(run(&src, &["init"]).status.success());
    for d in 0..10 {
        for f in 0..3 {
            let dir = src.join(format!("gen/d{d:02}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("f{f}.txt")),
                format!("dir {d} file {f}\n").repeat(6),
            )
            .unwrap();
        }
    }
    let added = run(&src, &["add", "gen"]);
    assert!(added.status.success(), "{}", stderr(&added));
    let committed = run(
        &src,
        &[
            "commit",
            "-m",
            "central",
            "--owner",
            "qoder:it",
            "--allow-unclaimed",
        ],
    );
    assert!(committed.status.success(), "{}", stderr(&committed));

    // 中央仓是个空目录：publish 应当自己把它建起来
    let central = root.join("central");
    fs::create_dir_all(&central).unwrap();
    let central_text = central.to_string_lossy().to_string();
    let published = run(&src, &["publish", "main", "--central", &central_text]);
    assert!(published.status.success(), "{}", stderr(&published));
    assert!(
        stdout(&published).contains("ref main"),
        "应报告写了哪个 ref：{}",
        stdout(&published)
    );

    let client = root.join("client");
    fs::create_dir_all(&client).unwrap();
    let pulled = run(
        &client,
        &[
            "pull",
            "main",
            "--central",
            &central_text,
            "--prefix",
            "gen/d00",
        ],
    );
    assert!(pulled.status.success(), "{}", stderr(&pulled));
    assert!(
        stdout(&pulled).contains("物化 3 个文件"),
        "只该物化一个目录：{}",
        stdout(&pulled)
    );
    assert!(client.join("gen/d00/f0.txt").is_file());
    assert!(
        !client.join("gen/d01").exists(),
        "范围外不该出现文件（稀疏物化没生效）"
    );

    let repeated = run(
        &client,
        &[
            "pull",
            "main",
            "--central",
            &central_text,
            "--prefix",
            "gen/d00",
        ],
    );
    assert!(
        repeated.status.success(),
        "{}",
        stderr(&repeated).to_string()
    );
    assert!(
        stdout(&repeated).contains("字节 0/"),
        "第二次 pull 不该再传字节：{}",
        stdout(&repeated)
    );

    // 本地改动挡住 pull，且报错时不动工作树；--force 才覆盖
    fs::write(client.join("gen/d00/f0.txt"), b"local edit\n").unwrap();
    let blocked = run(
        &client,
        &[
            "pull",
            "main",
            "--central",
            &central_text,
            "--prefix",
            "gen/d00",
        ],
    );
    assert!(!blocked.status.success(), "脏工作树必须判失败");
    assert!(
        stderr(&blocked).contains("FAIL"),
        "失败要带 FAIL 前缀：{}",
        stderr(&blocked)
    );
    assert_eq!(
        fs::read(client.join("gen/d00/f0.txt")).unwrap(),
        b"local edit\n"
    );
    let forced = run(
        &client,
        &[
            "pull",
            "main",
            "--central",
            &central_text,
            "--prefix",
            "gen/d00",
            "--force",
        ],
    );
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert_ne!(
        fs::read(client.join("gen/d00/f0.txt")).unwrap(),
        b"local edit\n",
        "--force 后应还原成仓里的内容"
    );

    // 不传 --prefix 就是全量物化
    let full = root.join("full");
    fs::create_dir_all(&full).unwrap();
    let whole = run(&full, &["pull", "main", "--central", &central_text]);
    assert!(whole.status.success(), "{}", stderr(&whole));
    assert!(
        stdout(&whole).contains("物化 30 个文件"),
        "全量 pull 该写 30 个：{}",
        stdout(&whole)
    );

    // ref 只能往前推：退回旧提交判红，--force 才改写
    fs::write(src.join("gen/d00/extra.txt"), b"extra\n").unwrap();
    assert!(run(&src, &["add", "gen"]).status.success());
    assert!(
        run(
            &src,
            &[
                "commit",
                "-m",
                "second",
                "--owner",
                "qoder:it",
                "--allow-unclaimed"
            ],
        )
        .status
        .success()
    );
    let logged = run(&src, &["log", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&logged)).unwrap();
    let newest = rows[0]["id"].as_str().unwrap().to_string();
    let oldest = rows[1]["id"].as_str().unwrap().to_string();
    let forwards = run(
        &src,
        &[
            "publish",
            "main",
            "--central",
            &central_text,
            "--commit",
            &newest,
        ],
    );
    assert!(
        forwards.status.success(),
        "快进应当放行：{}",
        stderr(&forwards)
    );
    let backwards = run(
        &src,
        &[
            "publish",
            "main",
            "--central",
            &central_text,
            "--commit",
            &oldest,
        ],
    );
    assert!(!backwards.status.success(), "把 ref 退回旧提交必须判失败");
    assert!(
        stderr(&backwards).contains("FAIL"),
        "失败要带 FAIL 前缀：{}",
        stderr(&backwards)
    );
    let rewrote = run(
        &src,
        &[
            "publish",
            "main",
            "--central",
            &central_text,
            "--commit",
            &oldest,
            "--force",
        ],
    );
    assert!(
        rewrote.status.success(),
        "--force 才允许改写：{}",
        stderr(&rewrote)
    );

    let bad_ref = run(&client, &["pull", "a/b", "--central", &central_text]);
    assert!(!bad_ref.status.success(), "非法 ref 名必须判失败");
    let _ = fs::remove_dir_all(&root);
}
