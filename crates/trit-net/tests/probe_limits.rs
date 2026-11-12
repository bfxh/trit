//! 受控探测的资源上限测试（从 probe.rs 拆出，纯搬家不改断言）：聚合探测数硬顶、
//! 限速睡眠被 deadline 夹住。**所有真连接都只打 127.0.0.1**。

use std::net::TcpListener;
use std::time::Duration;

use trit_net::netsec::probe::{Allowlist, ProbeConfig, ProbeError, run};

fn allowlist_json(ip: &str, ports: &[u16]) -> String {
    let list: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
    format!(
        "{{\"targets\":[{{\"ip\":\"{ip}\",\"ports\":[{}]}}]}}",
        list.join(",")
    )
}

/// 构造多目标 allowlist JSON：每个目标一段不重叠的端口，IP 走 127.0.0.x 环回。
fn multi_target_json(port_counts: &[usize]) -> String {
    let targets: Vec<String> = port_counts
        .iter()
        .enumerate()
        .map(|(t, &count)| {
            let ports: Vec<String> = (0..count)
                .map(|p| (20000 + t * 100 + p).to_string())
                .collect();
            format!(
                "{{\"ip\":\"127.0.0.{}\",\"ports\":[{}]}}",
                t + 1,
                ports.join(",")
            )
        })
        .collect();
    format!("{{\"targets\":[{}]}}", targets.join(","))
}

#[test]
fn allowlist_enforces_aggregate_probe_cap() {
    // 16 目标 × 32 端口 = 512，恰好到顶 ⇒ 放行（目标数 16≤64、每目标 32≤32 都合规）。
    let at_cap = vec![32usize; 16];
    let ok = Allowlist::from_json(&multi_target_json(&at_cap)).unwrap();
    assert_eq!(ok.len(), 16);
    // 再加一个 1 端口目标 ⇒ 513：每目标与目标数仍合规，但聚合超硬顶 ⇒ 拒绝。
    let mut over = at_cap.clone();
    over.push(1);
    assert!(matches!(
        Allowlist::from_json(&multi_target_json(&over)),
        Err(ProbeError::ConfigTooHot(msg)) if msg.contains("总探测数")
    ));
}

#[test]
fn delay_sleep_is_clamped_to_deadline() {
    // delay=3s 远大于 deadline=200ms。第一次探测打开放端口（无 accept、无数据），
    // banner 读到 300ms 超时 ⇒ 探测本身已耗 ~300ms（>deadline，单次阻塞的固有粒度）。
    // 关键：之后的限速睡眠必须被夹到 deadline 剩余（此处为 0），绝不睡满 3s。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let open_port = listener.local_addr().unwrap().port();
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[open_port, 1])).unwrap();
    let config = ProbeConfig {
        connect_timeout: Duration::from_millis(800),
        banner_timeout: Duration::from_millis(300),
        delay: Duration::from_millis(3_000),
        deadline: Duration::from_millis(200),
    };
    let started = std::time::Instant::now();
    let report = run(&allow, &config).unwrap();
    let elapsed = started.elapsed();
    assert!(report.truncated);
    assert_eq!(report.probed, 1);
    // 旧实现睡满 delay ⇒ 总耗时 ~3.3s；夹到剩余后 ~0.3s。1.5s 阈值干净区分两者。
    assert!(
        elapsed < Duration::from_millis(1_500),
        "限速睡眠 overshoot 了 deadline：elapsed={elapsed:?}"
    );
}
