//! 受控探测（M3）的测试：授权纪律（默认拒绝/字面 IP/实验室范围/硬上限）、
//! 连接分类纯函数、环回真连接。**所有真连接都只打 127.0.0.1**。
//!
//! CLI 子命令无进程级 e2e：`std::process::Command` 被 Mimosa 硬拦截（仓纪律：
//! spawn 留给部署层），CLI 覆盖由库级测试承担——arg 解析是薄层。

use std::io::ErrorKind;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::time::Duration;

use trit_net::netsec::attack::{AttackGraph, Evidence};
use trit_net::netsec::probe::{
    Allowlist, ProbeConfig, ProbeError, ProbeFinding, attach_probe, classify_connect,
    findings_from_jsonl, is_lab_address, run,
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

fn finding(ip: &str, port: u16, evidence: Evidence, note: &str) -> ProbeFinding {
    ProbeFinding {
        ip: ip.to_string(),
        port,
        evidence,
        source: "tcp-connect".to_string(),
        note: note.to_string(),
    }
}

#[test]
fn attach_builds_station_host_service_with_evidence() {
    let findings = vec![
        finding(
            "10.0.0.5",
            443,
            Evidence::Confirmed,
            "connected banner:HTTP",
        ),
        finding("10.0.0.5", 22, Evidence::Unverified, "timed out"),
        finding("10.0.0.9", 445, Evidence::RuledOut, "refused"),
    ];
    let mut graph = AttackGraph::new();
    let summary = attach_probe(&mut graph, &findings, "lab-station").unwrap();
    assert_eq!(summary.hosts, 2);
    assert_eq!(summary.services, 3);
    assert_eq!(summary.edges, 5);
    // 节点：station=T、主机=应答证据、服务=finding 证据。
    let station = graph.node_id("lab-station").unwrap();
    assert_eq!(graph.node(station).unwrap().evidence, Evidence::Confirmed);
    let host_a = graph.node_id("10.0.0.5").unwrap();
    assert_eq!(graph.node(host_a).unwrap().evidence, Evidence::Confirmed);
    let host_b = graph.node_id("10.0.0.9").unwrap();
    // 主机 B 只有 refused——refused 也是主机回的 RST ⇒ 在网（Confirmed）。
    assert_eq!(graph.node(host_b).unwrap().evidence, Evidence::Confirmed);
    let svc_t = graph.node_id("10.0.0.5:443").unwrap();
    let svc_x = graph.node_id("10.0.0.5:22").unwrap();
    let svc_f = graph.node_id("10.0.0.9:445").unwrap();
    assert_eq!(graph.node(svc_t).unwrap().evidence, Evidence::Confirmed);
    assert_eq!(graph.node(svc_x).unwrap().evidence, Evidence::Unverified);
    assert_eq!(graph.node(svc_f).unwrap().evidence, Evidence::RuledOut);
    assert!(graph.node(svc_t).unwrap().note.contains("banner:HTTP"));
    // 边证据与出处。
    assert_eq!(
        graph.edge(host_a, svc_x).map(|edge| edge.evidence),
        Some(Evidence::Unverified)
    );
    assert_eq!(
        graph.edge(host_a, svc_x).map(|edge| edge.source.as_str()),
        Some("tcp-connect")
    );
    // reach 贯通：T 服务已证实、X 服务待验证（见证点名 host→svc 边与服务节点）、
    // F 服务已排除。
    let limits = trit_net::netsec::attack::Limits::default();
    let reach_t = graph.reach(station, svc_t, limits).unwrap();
    assert_eq!(reach_t.verdict, Evidence::Confirmed);
    let reach_x = graph.reach(station, svc_x, limits).unwrap();
    assert_eq!(reach_x.verdict, Evidence::Unverified);
    let open = &reach_x.paths[0].open;
    assert!(open.contains(&trit_net::netsec::attack::OpenStep {
        node: host_a,
        edge_to: Some(svc_x),
    }));
    assert!(open.contains(&trit_net::netsec::attack::OpenStep {
        node: svc_x,
        edge_to: None,
    }));
    let reach_f = graph.reach(station, svc_f, limits).unwrap();
    assert_eq!(reach_f.verdict, Evidence::RuledOut);
}

#[test]
fn attach_all_timeout_host_stays_unverified() {
    // 全 X 的主机：应答证据 = X（分不清 filtered 还是没有这台主机）。
    let findings = vec![finding("10.0.0.7", 80, Evidence::Unverified, "timed out")];
    let mut graph = AttackGraph::new();
    attach_probe(&mut graph, &findings, "lab-station").unwrap();
    let host = graph.node_id("10.0.0.7").unwrap();
    assert_eq!(graph.node(host).unwrap().evidence, Evidence::Unverified);
}

#[test]
fn attach_is_idempotent_and_lww_refreshes() {
    let findings = vec![finding("10.0.0.5", 443, Evidence::Confirmed, "connected")];
    let mut graph = AttackGraph::new();
    attach_probe(&mut graph, &findings, "lab-station").unwrap();
    let snapshot = graph.clone();
    // 同一份报告重登记 ⇒ 图不变。
    attach_probe(&mut graph, &findings, "lab-station").unwrap();
    assert_eq!(graph, snapshot);
    // 端口状态变了（服务被关）⇒ 新报告 LWW 刷新节点与边。
    let newer = vec![finding("10.0.0.5", 443, Evidence::RuledOut, "refused")];
    attach_probe(&mut graph, &newer, "lab-station").unwrap();
    let svc = graph.node_id("10.0.0.5:443").unwrap();
    assert_eq!(graph.node(svc).unwrap().evidence, Evidence::RuledOut);
    let host = graph.node_id("10.0.0.5").unwrap();
    assert_eq!(graph.edge(host, svc).unwrap().evidence, Evidence::RuledOut);
    // 攻击语义的人写改写同样走 LWW：人给这条边登记攻击步骤。
    graph
        .add_edge(host, svc, Evidence::Confirmed, "smb 利用已复现（人工）")
        .unwrap();
    assert_eq!(
        graph.edge(host, svc).unwrap().source,
        "smb 利用已复现（人工）"
    );
}

#[test]
fn findings_from_jsonl_roundtrip_and_rejects_bad_lines() {
    let findings = vec![
        finding("10.0.0.5", 443, Evidence::Confirmed, "connected"),
        finding("10.0.0.9", 445, Evidence::RuledOut, "refused"),
    ];
    let text = findings
        .iter()
        .map(|f| serde_json::to_string(f).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let back = findings_from_jsonl(&text).unwrap();
    assert_eq!(back, findings);
    // 坏行显式报错，不静默丢。
    assert!(matches!(
        findings_from_jsonl(
            "{\"ip\":\"10.0.0.5\",\"port\":443,\"evidence\":\"Q\",\"source\":\"x\",\"note\":\"\"}"
        ),
        Err(ProbeError::Json(_))
    ));
    assert!(matches!(
        findings_from_jsonl("not-json"),
        Err(ProbeError::Json(_))
    ));
    // 空行跳过。
    let with_blank = format!("{text}\n\n");
    assert_eq!(findings_from_jsonl(&with_blank).unwrap().len(), 2);
}
