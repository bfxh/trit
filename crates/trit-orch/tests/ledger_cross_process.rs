//! 跨进程并发保存：两个进程同时写同一台账，结果必须是某一方的完整写入。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use trit_orch::{Ledger, LedgerStore};

const CHILD_ENV: &str = "TRIT_LEDGER_CHILD_PATH";

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-ledger-xproc-{tag}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn ledger_with(prefix: &str, count: u32) -> Ledger {
    let mut ledger = Ledger::new();
    for index in 0..count {
        let name = format!("{prefix}-{index}");
        ledger.prepare(&name, 0, &name, 1).unwrap();
    }
    ledger
}

/// 子进程侧：只做保存。父进程运行时没有该环境变量，直接返回。
#[test]
fn child_save() {
    let Ok(path) = std::env::var(CHILD_ENV) else {
        return;
    };
    let store = LedgerStore::new(path);
    for _ in 0..50 {
        store.save(&ledger_with("child", 20)).unwrap();
    }
}

#[test]
fn cross_process_saves_do_not_corrupt_the_ledger() {
    let path = temp_path("shared");
    let child = Command::new(std::env::current_exe().expect("current exe"))
        .args(["--exact", "child_save"])
        .env(CHILD_ENV, &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");

    let store = LedgerStore::new(&path);
    for _ in 0..50 {
        store.save(&ledger_with("parent", 20)).unwrap();
    }

    let status = child.wait_with_output().expect("wait child");
    assert!(status.status.success(), "子进程保存失败");

    let loaded = store.load().expect("load 必须成功");
    let has_parent = loaded.attempt("parent-0").is_some();
    let has_child = loaded.attempt("child-0").is_some();
    assert!(
        has_parent ^ has_child,
        "台账必须是某一方的完整写入，不能混合"
    );

    let mut lock = path.as_os_str().to_os_string();
    lock.push(".ledger.lock");
    assert!(!Path::new(&lock).exists(), "锁文件必须已释放");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}.bak", path.display()));
}
