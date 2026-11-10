#![cfg(feature = "full")]
//! MD → 任务栏：调度器有界 drain 消费任务栏 + 服务器端点。

use std::net::SocketAddr;
use trit_immune::scheduler::{drain_plan, JournalHandler, Scheduler};
use trit_immune::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

fn bind_server(journal: Journal) -> SocketAddr {
    let server = Server::new(journal);
    server
        .serve("127.0.0.1:0".parse().unwrap())
        .expect("起服务器")
}

#[test]
fn scheduler_drains_taskbar_bounded() {
    // 任务栏 → 调度器有界循环：MD 任务被 Review 消费，固定点到达
    let pa = temp("sched-md");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let inbox = j.spawn(NodeId(0), NodeKind::Message, "收件箱").unwrap();
    j.into_dag().unwrap();

    let mut j = Journal::open_as(&pa, 0).unwrap();
    ingest_md(
        &mut j,
        inbox,
        "daily.md",
        "- [ ] 任务甲\n- [ ] 任务乙\n- [ ] 任务丙\n",
        false,
    )
    .unwrap();
    assert_eq!(taskbar(&j, inbox).len(), 3);

    // 有界 drain 循环：Review(甲) → Review(乙) → Review(丙) → 空计划
    let sched = Scheduler::new(2);
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds <= 4, "有界终止");
        let plan = drain_plan(j.dag(), 10);
        if plan.is_empty() {
            break;
        }
        let mut h = JournalHandler { journal: &mut j };
        let report = sched.run(plan, &mut h);
        assert_eq!(report.failed, 0, "{report:?}");
    }
    assert!(taskbar(&j, inbox).is_empty(), "任务栏清空 = 固定点");
    assert!((j.dag().convergence().ratio - 1.0).abs() < 1e-9);

    j.into_dag().unwrap();
    let _ = std::fs::remove_file(&pa);
}

#[test]
fn server_taskmd_and_taskbar_endpoints() {
    let pa = temp("srv-md");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let _ = j.spawn(NodeId(0), NodeKind::Message, "收件箱").unwrap();
    j.into_dag().unwrap();

    let addr = bind_server(Journal::open_as(&pa, 0).unwrap());

    let md = "- [ ] 修登录模块 #p0\n- [x] 写日报\n";
    let (s, b) = server::client::post(addr, "/v1/taskmd?parent=1&source=daily.md", md).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("created=2"), "{b}");
    assert!(b.contains("zero_state_only=true"), "{b}");

    let (s, b) = server::client::get(addr, "/v1/taskbar?parent=1").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("修登录模块"), "{b}");
    assert!(b.contains("写日报"), "{b}");

    // 幂等：重复导入只跳过
    let (s, b) = server::client::post(addr, "/v1/taskmd?parent=1&source=daily.md", md).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("created=0 skipped=2"), "{b}");

    // 隔离导入：外部 MD 自动隔离
    let (s, b) = server::client::post(
        addr,
        "/v1/taskmd?parent=1&source=外部.md&quarantine=1",
        "- [ ] 外部投喂的任务\n",
    )
    .unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("quarantined=1"), "{b}");
    let (_, b) = server::client::get(addr, "/v1/taskbar?parent=1").unwrap();
    assert!(!b.contains("外部投喂的任务"), "隔离任务应退出任务栏：{b}");

    let _ = std::fs::remove_file(&pa);
}
