//! 攻击链枚举的资源上限测试（从 tests/crossrepo.rs 拆出，控制单文件行数预算）。

use trit_immune::*;

fn tmp(tag: &str, n: u64) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-cross-{tag}-{}-{n}.journal",
        std::process::id()
    ))
}

fn chain_repo(path: &std::path::Path, writer: u32, verdicts: &[Trit]) -> Journal {
    let _ = std::fs::remove_file(path);
    let mut j = Journal::create_as(path, "chain", writer).unwrap();
    let mut parent = NodeId(0);
    for (i, v) in verdicts.iter().enumerate() {
        let id = j.spawn(parent, NodeKind::Delta, &format!("N{i}")).unwrap();
        j.record(id, *v, None, "链上节点").unwrap();
        parent = id;
    }
    drop(j.into_dag().unwrap());
    Journal::open_as(path, writer).unwrap()
}

#[test]
fn attack_chain_depth_cap_bounds_huge_max_depth() {
    // H6：调用方（HTTP / 库）传超大的 max_depth，attack_chains 必须夹到 MAX_ATTACK_DEPTH
    //（既防递归爆栈、又防深链枚举）。构造 100 跳链、头部 -1：夹到 64 后到不了源 ⇒ 截断且空。
    let p = tmp("cap-depth", 8);
    let mut verdicts = vec![Trit::Unknown; 100];
    verdicts[0] = Trit::Vulnerable;
    let mut fleet = Fleet::new();
    fleet.add("r", chain_repo(&p, 0, &verdicts)).unwrap();
    // target = 链尾（NodeId(100)），源在 NodeId(1)：需要 99 跳，远超 64 的硬顶。
    let res = fleet.attack_chains("r", NodeId(100), 64, usize::MAX);
    assert!(res.truncated, "超大 max_depth 必须被夹到硬顶并如实截断");
    assert!(
        res.chains.is_empty(),
        "深度被夹到 64 后到不了 depth-99 的源"
    );
    let _ = std::fs::remove_file(&p);
}

#[test]
fn attack_chain_visit_budget_bounds_dense_graph() {
    use std::sync::mpsc;
    use std::time::Duration;
    // H6：满图 + 大深度 + 无源（全 0）时原实现按 n! 枚举（n=12 即百亿级）。访问预算必须让
    // 它快速截断；用子线程 + 超时把「挂死」变成断言失败（变异门可判「抓住」而非「超时」）。
    let p = tmp("cap-dense", 9);
    let mut fleet = Fleet::new();
    fleet
        .add("r", chain_repo(&p, 0, &[Trit::Unknown; 12]))
        .unwrap();
    let mut rows = String::new();
    for i in 1..=12 {
        for j in 1..=12 {
            if i != j {
                rows.push_str(&format!("FLEETLINK\tr\t{i}\tr\t{j}\t-1\tk\n"));
            }
        }
    }
    let ledger = std::env::temp_dir().join(format!("trit-dense-{}.tsv", std::process::id()));
    std::fs::write(&ledger, &rows).unwrap();
    fleet.load_links(&ledger).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = fleet.attack_chains("r", NodeId(1), usize::MAX, usize::MAX);
        let _ = tx.send(res.truncated);
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(truncated) => assert!(truncated, "满图必须被访问预算截断"),
        Err(_) => panic!("满图枚举挂死（访问预算回归）"),
    }
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(&ledger);
}
