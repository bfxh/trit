//! `trit` CLI 的回归：账本初始化/追加/日志重放、分支与合并门禁、同步与锁文件的命令行形状。
//!
//! 从 `bin/trit.rs` 拆出来——入口文件该只剩"参数怎么分派"，
//! 而这些测试讲的是每条子命令的**行为**。子模块用 `use super::*`，可见性与原来一致。

use super::*;
use std::fs;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trit-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn s(v: impl Into<String>) -> String {
    v.into()
}

#[test]
fn init_creates_missing_directories_and_refuses_overwrite() {
    let base = tmpdir("init-missing");
    let target = base.join("nested").join("brand-new");

    run(&[s("init"), s(target.to_string_lossy().to_string())])
        .expect("init 应能把不存在的父目录一起建出来");
    assert!(
        target.join(DEFAULT_LEDGER).is_file(),
        "init 后账本文件必须存在"
    );

    let again = run(&[s("init"), s(target.to_string_lossy().to_string())]);
    assert!(
        matches!(again, Err(CliError::Runtime(_))),
        "重复 init 不该静默覆盖已有账本：{again:?}"
    );
}

#[test]
fn full_git_style_workflow() {
    let dir = tmpdir("wf");
    let ledger = dir.join(DEFAULT_LEDGER);

    // init → branch → commit(0 默认) → commit(+1) → status → gate → checkout
    run(&[s("init"), s(dir.to_string_lossy().to_string())]).unwrap();
    assert!(ledger.is_file(), "init 应产生账本");

    run(&[
        s("branch"),
        s("feat-login"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("commit"),
        s("-m"),
        s("登录模块完成"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("commit"),
        s("-m"),
        s("回归通过"),
        s("-v"),
        s("+1"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();

    // status 与 gate 不炸且输出非空（走 std::io 捕获太重，只验证 Ok）
    run(&[
        s("status"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[s("gate"), s("-f"), s(ledger.to_string_lossy().to_string())]).unwrap();
    run(&[
        s("suspects"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("checkout"),
        s("1"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("log"),
        s("-n"),
        s("3"),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();

    // clone：副本内容一致
    let dst = dir.join("clone.tritj");
    run(&[
        s("clone"),
        s(ledger.to_string_lossy().to_string()),
        s(dst.to_string_lossy().to_string()),
    ])
    .unwrap();
    assert_eq!(
        fs::read_to_string(&ledger).unwrap(),
        fs::read_to_string(&dst).unwrap(),
        "克隆应逐字节一致"
    );

    // merge：克隆侧追加再合回（δ-CRDT 无冲突）
    run(&[
        s("commit"),
        s("-m"),
        s("克隆侧增量"),
        s("-v"),
        s("+1"),
        s("-f"),
        s(dst.to_string_lossy().to_string()),
    ])
    .unwrap();
    run(&[
        s("merge"),
        s(dst.to_string_lossy().to_string()),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .unwrap();

    // 合并后主账本应含克隆侧增量（Record 不增节点数，追加进 #1 的历史）
    let j = Journal::open_as(&ledger, 0).unwrap();
    let n1 = j.dag().node(NodeId(1));
    assert!(
        n1.history.iter().any(|e| e.note.contains("克隆侧增量")),
        "δ-CRDT 合并应把克隆侧裁决带进来：{:?}",
        n1.history
    );
    drop(j.into_dag().unwrap());

    // sync-git：非 git 目录在 spawn 之前被拒（错误路径，零进程创建）
    let missing = dir.join("not-a-repo");
    assert!(matches!(
        run(&[s("sync-git"), s(missing.to_string_lossy().to_string())]),
        Err(CliError::Runtime(_))
    ));

    // 用法错误路径
    assert!(matches!(
        run(&[s("commit"), s("-m")]),
        Err(CliError::Usage(_))
    ));
    assert!(matches!(run(&[s("nope")]), Err(CliError::Usage(_))));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lock_accepts_ledger_option_after_positional_argument() {
    // 文档口径是 `lock <Cargo.lock> [-f file]`：`-f` 排在位置参数之后也必须生效，
    // 否则坐标静默写进默认账本——同一命令行写到了另一个文件。
    let dir = tmpdir("lock-opt");
    run(&[s("init"), s(dir.to_string_lossy().to_string())]).unwrap();
    let ledger = dir.join(DEFAULT_LEDGER);
    let lockfile = dir.join("Cargo.lock");
    fs::write(
        &lockfile,
        "[[package]]\nname = \"shared-lib\"\nversion = \"1.2.0\"\nchecksum = \"aaa\"\n",
    )
    .unwrap();

    run(&[
        s("lock"),
        s(lockfile.to_string_lossy().to_string()),
        s("-f"),
        s(ledger.to_string_lossy().to_string()),
    ])
    .expect("lock 应接受位置参数之后的 -f");

    let j = Journal::open_as(&ledger, 0).unwrap();
    let titles: Vec<String> = j
        .dag()
        .nodes()
        .iter()
        .filter(|n| n.title.starts_with("COORD "))
        .map(|n| n.title.clone())
        .collect();
    assert_eq!(
        titles.len(),
        1,
        "坐标节点必须落在 -f 指定的账本里：{titles:?}"
    );
    drop(j.into_dag().unwrap());
    fs::remove_dir_all(&dir).unwrap();
}
