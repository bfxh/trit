//! 第三轮 R1/R2 回归：`adopt` 先校验后落盘，并拒绝超出本地时钟容差的未来戳。

use std::path::PathBuf;

use trit_immune::NodeId;
use trit_immune::delta::{Delta, Stamped};
use trit_immune::hlc::Hlc;
use trit_immune::journal::Journal;
use trit_immune::trit::Trit;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-adopt-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建目录");
    dir.join("journal.log")
}

fn record(id: u32) -> Delta {
    Delta::Record {
        id: NodeId(id),
        verdict: Trit::Vulnerable,
        evidence: None,
        note: "回归".into(),
    }
}

#[test]
fn adopt_rejects_future_stamp_without_touching_the_journal() {
    let path = scratch("future");
    let mut journal = Journal::create(&path, "root").expect("建日志");
    let before = std::fs::read(&path).expect("读旧日志");

    let poison = Stamped {
        ts: Hlc::new(u64::MAX, 0, 1),
        delta: record(1),
    };
    let error = journal.adopt(vec![poison]).expect_err("未来戳必须被拒");
    assert!(
        error.to_string().contains("容差"),
        "错误应点明时钟容差，实际：{error}"
    );
    assert_eq!(
        std::fs::read(&path).expect("读日志"),
        before,
        "被拒绝时不应改动现有日志"
    );
}

#[test]
fn adopt_rejects_broken_delta_without_touching_the_journal() {
    let path = scratch("broken");
    let mut journal = Journal::create(&path, "root").expect("建日志");
    let before = std::fs::read(&path).expect("读旧日志");

    // 引用不存在的节点：重放必然失败
    let broken = Stamped {
        ts: Hlc::new(Hlc::physical_now(), 0, 1),
        delta: record(999),
    };
    assert!(journal.adopt(vec![broken]).is_err(), "坏 Delta 必须被拒");
    assert_eq!(
        std::fs::read(&path).expect("读日志"),
        before,
        "校验失败时不应先把本地账本写坏"
    );
}
