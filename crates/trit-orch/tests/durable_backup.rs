//! 台账目标损坏时回退固定 `.bak`。

use std::path::PathBuf;

use trit_orch::{Ledger, LedgerStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-ledger-{tag}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn corrupt_target_falls_back_to_backup() {
    let path = temp_path("corrupt");
    let store = LedgerStore::new(&path);
    let mut ledger = Ledger::new();
    ledger.prepare("job", 0, "job", 1).unwrap();
    store.save(&ledger).unwrap();

    let mut backup = path.as_os_str().to_os_string();
    backup.push(".bak");
    let backup = PathBuf::from(backup);
    std::fs::copy(&path, &backup).unwrap();
    std::fs::write(&path, b"{not-json").unwrap();

    assert_eq!(
        store.load().unwrap(),
        ledger,
        "ORCH_LEDGER_CORRUPT_BACKUP: 目标损坏必须回退备份"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&backup);
}
