//! 受控探测（M3）的测试：授权纪律（默认拒绝/字面 IP/实验室范围/硬上限）、
//! 连接分类纯函数、环回真连接。**所有真连接都只打 127.0.0.1**。
//!
//! CLI 子命令无进程级 e2e：`std::process::Command` 被 Mimosa 硬拦截（仓纪律：
//! spawn 留给部署层），CLI 覆盖由库级测试承担——arg 解析是薄层。

use std::io::ErrorKind;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::time::Duration;

use trit_net::netsec::attack::Evidence;
use trit_net::netsec::probe::{
    Allowlist, ProbeConfig, ProbeError, ProbeFinding, classify_connect, is_lab_address, run,
};

fn fast_config() -> ProbeConfig {
    ProbeConfig {
        connect_timeout: Duration::from_millis(800),
        banner_timeout: Duration::from_millis(300),
        delay: Duration::ZERO,
        deadline: Duration::from_secs(10),
    }
}

fn allowlist_json(ip: &str, ports: &[u16]) -> String {
    let list: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
    format!(
        "{{\"targets\":[{{\"ip\":\"{ip}\",\"ports\":[{}]}}]}}",
        list.join(",")
    )
}

fn ipv4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn allowlist_default_denies_everything() {
    // 空 allowlist：没有任何目标可探测。
    let empty = Allowlist::from_json("{\"targets\":[]}").unwrap();
    assert!(empty.is_empty());
    // 未列入 allowlist 的 (ip, port) 不许探测：主机在列表、端口不在 ⇒ 拒。
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[9])).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let open_port = listener.local_addr().unwrap().port();
    assert!(!allow.permits(IpAddr::V4(Ipv4Addr::LOCALHOST), open_port));
    assert!(!allow.permits(IpAddr::V4(Ipv4Addr::LOCALHOST), 59999));
}

#[test]
fn allowlist_rejects_non_literal_addresses() {
    // DNS 名不许——不做解析。
    assert_eq!(
        Allowlist::from_json(&allowlist_json("example.com", &[443])),
        Err(ProbeError::NonLiteralAddress("example.com".to_string()))
    );
    // 坏串不许。
    assert!(matches!(
        Allowlist::from_json(&allowlist_json("not-an-ip", &[80])),
        Err(ProbeError::NonLiteralAddress(_))
    ));
}

#[test]
fn allowlist_rejects_targets_outside_the_lab() {
    // 公网不许。
    assert_eq!(
        Allowlist::from_json(&allowlist_json("8.8.8.8", &[53])),
        Err(ProbeError::OutsideLab("8.8.8.8".to_string()))
    );
    // CGNAT 100.64/10 与基准测试 198.18/15 刻意不在实验室范围。
    assert!(!is_lab_address(ipv4(100, 64, 0, 1)));
    assert!(!is_lab_address(ipv4(198, 18, 0, 1)));
    // 组播不许。
    assert!(!is_lab_address(ipv4(224, 0, 0, 1)));
}

#[test]
fn lab_scope_covers_loopback_private_and_v6_ula() {
    // v4：环回 / 三段私网 / 链路本地。
    assert!(is_lab_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(is_lab_address(ipv4(127, 255, 0, 1)));
    assert!(is_lab_address(ipv4(10, 0, 0, 1)));
    assert!(is_lab_address(ipv4(172, 16, 0, 1)));
    assert!(is_lab_address(ipv4(172, 31, 255, 254)));
    assert!(is_lab_address(ipv4(192, 168, 1, 1)));
    assert!(is_lab_address(ipv4(169, 254, 1, 1)));
    // 172.32 越界。
    assert!(!is_lab_address(ipv4(172, 32, 0, 1)));
    // v6：环回 / ULA / 链路本地；全球单播不许。
    assert!(is_lab_address(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert!(is_lab_address(IpAddr::V6("fd00::1".parse().unwrap())));
    assert!(is_lab_address(IpAddr::V6("fe80::1".parse().unwrap())));
    assert!(!is_lab_address(IpAddr::V6("2001:db8::1".parse().unwrap())));
}

#[test]
fn allowlist_enforces_hard_caps_and_shape() {
    // 65 个目标 = 扫描的量纲，拒绝。
    let targets: Vec<String> = (0..65)
        .map(|i| format!("{{\"ip\":\"127.0.0.1\",\"ports\":[{0}]}}", 20000 + i))
        .collect();
    let text = format!("{{\"targets\":[{}]}}", targets.join(","));
    assert!(matches!(
        Allowlist::from_json(&text),
        Err(ProbeError::ConfigTooHot(_))
    ));
    // 单目标 33 个端口，拒绝（端口段是扫描的形状）。
    let ports: Vec<String> = (0..33).map(|i| (30000 + i).to_string()).collect();
    let text = format!(
        "{{\"targets\":[{{\"ip\":\"127.0.0.1\",\"ports\":[{}]}}]}}",
        ports.join(",")
    );
    assert!(matches!(
        Allowlist::from_json(&text),
        Err(ProbeError::ConfigTooHot(_))
    ));
    // 空端口列表，拒绝。
    assert!(matches!(
        Allowlist::from_json("{\"targets\":[{\"ip\":\"127.0.0.1\",\"ports\":[]}]}"),
        Err(ProbeError::Json(_))
    ));
    // 重复端口去重排序。
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[9, 9, 8])).unwrap();
    assert_eq!(allow.len(), 1);
}

#[test]
fn config_hot_limits_are_refused_not_clamped() {
    let mut config = fast_config();
    config.connect_timeout = Duration::from_millis(5_001);
    assert_eq!(
        config.validate(),
        Err(ProbeError::ConfigTooHot(
            "connect_timeout 必须在 1..=5000ms"
        ))
    );
    config.connect_timeout = Duration::from_millis(0);
    assert!(matches!(
        config.validate(),
        Err(ProbeError::ConfigTooHot(_))
    ));
    config.connect_timeout = Duration::from_millis(1_000);
    config.banner_timeout = Duration::from_millis(2_001);
    assert!(matches!(
        config.validate(),
        Err(ProbeError::ConfigTooHot(_))
    ));
    config.banner_timeout = Duration::from_millis(500);
    config.deadline = Duration::ZERO;
    assert!(matches!(
        config.validate(),
        Err(ProbeError::ConfigTooHot(_))
    ));
}

#[test]
fn classify_connect_maps_results_to_evidence() {
    // 纯函数：成功=T，明确拒绝=F，其余=X。真实连接在环回测试里覆盖。
    let (evidence, note) =
        classify_connect(&Err(std::io::Error::from(ErrorKind::ConnectionRefused)));
    assert_eq!(evidence, Evidence::RuledOut);
    assert_eq!(note, "refused");
    let (evidence, _) = classify_connect(&Err(std::io::Error::from(ErrorKind::TimedOut)));
    assert_eq!(evidence, Evidence::Unverified);
    let (evidence, _) = classify_connect(&Err(std::io::Error::from(ErrorKind::PermissionDenied)));
    assert_eq!(evidence, Evidence::Unverified);
    // 成功分支：真实环回连接。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
    });
    let (evidence, note) = classify_connect(&TcpStream::connect(("127.0.0.1", port)));
    server.join().unwrap();
    assert_eq!(evidence, Evidence::Confirmed);
    assert_eq!(note, "connected");
}

#[test]
fn loopback_open_port_with_banner() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nServer: lab\r\n\r\n")
            .ok();
    });
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[port])).unwrap();
    let report = run(&allow, &fast_config()).unwrap();
    server.join().unwrap();
    assert_eq!(report.planned, 1);
    assert_eq!(report.probed, 1);
    assert!(!report.truncated);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.evidence, Evidence::Confirmed);
    assert_eq!(finding.source, "tcp-connect");
    assert_eq!(finding.port, port);
    assert!(finding.note.contains("connected"), "note={}", finding.note);
    // banner 捕获进 note。
    assert!(
        finding.note.contains("banner:HTTP/1.1"),
        "note={}",
        finding.note
    );
}

#[test]
fn loopback_closed_port_is_never_confirmed() {
    // 占住一个端口再放开，得到一个确定没人监听的环回端口。
    // 平台现实：Linux 明确拒绝（RuledOut/refused）；Windows 的 connect_timeout
    // 常把拒绝表面化成超时（Unverified/"timed out"）。两种都诚实——钉死的
    // 不变量是：**关闭端口永不绝不判 Confirmed**，note 与 evidence 一致。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap().port();
    drop(listener);
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[closed])).unwrap();
    let report = run(&allow, &fast_config()).unwrap();
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    match finding.evidence {
        Evidence::RuledOut => assert_eq!(finding.note, "refused"),
        Evidence::Unverified => assert_eq!(finding.note, "timed out"),
        Evidence::Confirmed => panic!("关闭端口被判 Confirmed：{}", finding.note),
    }
}

#[test]
fn deadline_truncates_and_reports_honestly() {
    // 两个端口（加载时升序排序 ⇒ 先探测端口 1），deadline 在第一次探测后耗尽
    // ⇒ 只报已跑的，truncated=true。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let open_port = listener.local_addr().unwrap().port();
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[open_port, 1])).unwrap();
    let config = ProbeConfig {
        connect_timeout: Duration::from_millis(800),
        banner_timeout: Duration::from_millis(300),
        delay: Duration::from_millis(2_000),
        deadline: Duration::from_millis(1_500),
    };
    let report = run(&allow, &config).unwrap();
    assert!(report.truncated);
    assert_eq!(report.probed, 1);
    assert_eq!(report.planned, 2);
    assert_eq!(report.findings[0].port, 1);
}

#[test]
fn report_jsonl_roundtrip() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap().port();
    drop(listener);
    let allow = Allowlist::from_json(&allowlist_json("127.0.0.1", &[closed])).unwrap();
    let report = run(&allow, &fast_config()).unwrap();
    let jsonl = report.to_jsonl();
    let lines: Vec<&str> = jsonl.lines().collect();
    assert_eq!(lines.len(), 1);
    let back: ProbeFinding = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(back, report.findings[0]);
}
