//! 网络面检查：地址分类 + 外联 / 监听面 / 外联扇出 / 连接失败形态。
//!
//! 边界（不猜）：
//! - **DNS 未采集** ⇒ 只能对 IP 字面量分类，回答不了「连的是哪个域名」；
//! - TCP 表是**瞬时快照**（每个采样间隔重拍一次）——看见 LISTEN 不代表一直在听，
//!   同一条连接会在多个采样里重复出现，本模块先按 (host, port, state) 去重再判定；
//! - finding 只陈述证据形态（连了公网 / 监听所有网卡 / 扇出 / SYN_SENT 堆积），
//!   不下「恶意」结论。
//!
//! 分类口径：`outbound` 只算 **Public 与 CGNAT**（运营商网络对企业同样是外部）；
//! Loopback / Private / LinkLocal / Multicast / Unspecified 不算外联。

use std::collections::{BTreeMap, BTreeSet};

use crate::analyze::Finding;
use crate::event::Event;

/// 扇出判定的不同主机数阈值。
const SCAN_FANOUT_HOSTS: usize = 8;
/// SYN_SENT 堆积判定的不同目标数阈值。
const SYN_SENT_PILEUP: usize = 4;

/// 地址类别（RFC 作用域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddrClass {
    /// `127.0.0.0/8`、`::1`。
    Loopback,
    /// `10/8`、`172.16/12`、`192.168/16`、`fc00::/7`（ULA）。
    Private,
    /// `100.64.0.0/10`（运营商级 NAT）。
    Cgnat,
    /// `169.254/16`、`fe80::/10`。
    LinkLocal,
    /// `224/4`、`ff00::/8`。
    Multicast,
    /// `0.0.0.0/8`、`::`。
    Unspecified,
    /// 公网。
    Public,
    /// 解析失败（正常情况下 `Get-NetTCPConnection` 只给 IP 字面量）。
    Invalid,
}

/// 一条 remote 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// 作用域类别。
    pub class: AddrClass,
    /// 端口（解析失败为 0）。
    pub port: u16,
    /// 主机部分（剥掉 IPv6 方括号后）。
    pub host: String,
}

/// 解析采集端的 remote 拼法 `{RemoteAddress}:{RemotePort}`（IPv6 不带方括号，
/// 但带方括号的写法也顺手剥掉）。host 含 `:` 视为 IPv6。
pub fn parse_remote(remote: &str) -> Remote {
    let Some((host, port_str)) = remote.rsplit_once(':') else {
        return Remote {
            class: AddrClass::Invalid,
            port: 0,
            host: remote.to_string(),
        };
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let Ok(port) = port_str.parse::<u16>() else {
        return Remote {
            class: AddrClass::Invalid,
            port: 0,
            host,
        };
    };
    let class = if host.contains(':') {
        classify_ipv6(&host)
    } else {
        classify_ipv4(&host)
    };
    Remote { class, port, host }
}

/// IPv4 作用域分类（严格十进制四段，每段 0–255）。
pub fn classify_ipv4(host: &str) -> AddrClass {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return AddrClass::Invalid;
    }
    let mut octets = [0u8; 4];
    for (slot, part) in octets.iter_mut().zip(parts.iter()) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return AddrClass::Invalid;
        }
        match part.parse::<u8>() {
            Ok(value) => *slot = value,
            Err(_) => return AddrClass::Invalid,
        }
    }
    match octets {
        [127, ..] => AddrClass::Loopback,
        [10, ..] => AddrClass::Private,
        [172, 16..=31, ..] => AddrClass::Private,
        [192, 168, ..] => AddrClass::Private,
        [100, 64..=127, ..] => AddrClass::Cgnat,
        [169, 254, ..] => AddrClass::LinkLocal,
        [224..=239, ..] => AddrClass::Multicast,
        [0, ..] => AddrClass::Unspecified,
        _ => AddrClass::Public,
    }
}

/// IPv6 作用域分类（本工具只做前缀级判断，不做完整地址解析）。
pub fn classify_ipv6(host: &str) -> AddrClass {
    let lower = host.to_ascii_lowercase();
    if lower == "::" {
        return AddrClass::Unspecified;
    }
    if lower == "::1" {
        return AddrClass::Loopback;
    }
    // IPv4-mapped：::ffff:a.b.c.d → 按 v4 再分类。
    if let Some(mapped) = lower.strip_prefix("::ffff:") {
        return classify_ipv4(mapped);
    }
    if lower.starts_with("fe80")
        || lower.starts_with("fe9")
        || lower.starts_with("fea")
        || lower.starts_with("feb")
    {
        return AddrClass::LinkLocal;
    }
    if lower.starts_with("ff") {
        return AddrClass::Multicast;
    }
    if lower.starts_with("fc") || lower.starts_with("fd") {
        return AddrClass::Private;
    }
    AddrClass::Public
}

/// 监听绑定在所有网卡上（`0.0.0.0` / `::` / `*`）。
fn is_wildcard_host(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "*")
}

/// 网络面全部发现（确定性：pid 升序、证据去重）。
///
/// 检查项：
/// - `outbound-connection`（high）：连到 Public / CGNAT；
/// - `listen-all-interfaces`（high）：`0.0.0.0` / `::` 上的监听（面向所有网卡）；
/// - `egress-fanout`（medium）：同一 pid 对 ≥ [`SCAN_FANOUT_HOSTS`] 个不同非本机主机
///   发起连接（扫描 / 信标重试的常见形态）；
/// - `connect-failures`（medium）：同一 pid ≥ [`SYN_SENT_PILEUP`] 个目标停在 SYN_SENT。
pub fn network_findings(events: &[Event]) -> Vec<Finding> {
    // pid → (class, host, port, state) 去重集合（同一连接会被重复采样）。
    let mut by_pid: BTreeMap<u32, BTreeSet<(AddrClass, String, u16, String)>> = BTreeMap::new();
    for event in events {
        if let Event::Connection {
            pid, remote, state, ..
        } = event
        {
            let parsed = parse_remote(remote);
            by_pid.entry(*pid).or_default().insert((
                parsed.class,
                parsed.host,
                parsed.port,
                state.to_ascii_lowercase(),
            ));
        }
    }

    let mut outbound: Vec<String> = Vec::new();
    let mut listeners: Vec<String> = Vec::new();
    let mut fanouts: Vec<(u32, Vec<String>)> = Vec::new();
    let mut failures: Vec<(u32, Vec<String>)> = Vec::new();

    for (pid, rows) in &by_pid {
        let mut hosts_reachable: BTreeSet<String> = BTreeSet::new();
        let mut synsent: BTreeSet<(String, u16)> = BTreeSet::new();
        for (class, host, port, state) in rows {
            let endpoint = format!("{host}:{port}");
            match class {
                AddrClass::Public | AddrClass::Cgnat => {
                    outbound.push(format!("pid {pid} → {endpoint} ({state})"));
                }
                _ => {}
            }
            let is_listen_all = *state == "listen" && is_wildcard_host(host);
            if is_listen_all {
                listeners.push(format!("pid {pid} listen {endpoint}（所有网卡）"));
            }
            if !matches!(
                class,
                AddrClass::Loopback | AddrClass::Invalid | AddrClass::Unspecified
            ) {
                hosts_reachable.insert(host.clone());
            }
            if state == "synsent" {
                synsent.insert((host.clone(), *port));
            }
        }
        if hosts_reachable.len() >= SCAN_FANOUT_HOSTS {
            let evidence: Vec<String> = hosts_reachable
                .iter()
                .map(|host| format!("pid {pid} → {host}"))
                .collect();
            fanouts.push((*pid, evidence.into_iter().take(8).collect::<Vec<_>>()));
        }
        if synsent.len() >= SYN_SENT_PILEUP {
            let evidence: Vec<String> = synsent
                .iter()
                .map(|(host, port)| format!("pid {pid} → {host}:{port} (SynSent)"))
                .collect();
            failures.push((*pid, evidence.into_iter().take(8).collect::<Vec<_>>()));
        }
    }

    let mut findings = Vec::new();
    if !outbound.is_empty() {
        findings.push(Finding {
            id: "outbound-connection",
            severity: "high",
            detail: format!("有 {} 条公网/运营商网络的连接", outbound.len()),
            evidence: outbound.into_iter().take(8).collect(),
        });
    }
    if !listeners.is_empty() {
        findings.push(Finding {
            id: "listen-all-interfaces",
            severity: "high",
            detail: format!(
                "有 {} 个监听绑定在所有网卡上（而不只是本机回环）",
                listeners.len()
            ),
            evidence: listeners.into_iter().take(8).collect(),
        });
    }
    for (pid, evidence) in fanouts {
        findings.push(Finding {
            id: "egress-fanout",
            severity: "medium",
            detail: format!(
                "pid {pid} 在采样窗口内对 ≥{SCAN_FANOUT_HOSTS} 个不同主机发起连接（扫描/信标重试的常见形态）"
            ),
            evidence,
        });
    }
    for (pid, evidence) in failures {
        findings.push(Finding {
            id: "connect-failures",
            severity: "medium",
            detail: format!(
                "pid {pid} 有 ≥{SYN_SENT_PILEUP} 个目标停在 SYN_SENT（连不上：重试或扫描）"
            ),
            evidence,
        });
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(pid: u32, remote: &str, state: &str) -> Event {
        Event::Connection {
            ts_ms: 1,
            pid,
            remote: remote.into(),
            state: state.into(),
        }
    }

    #[test]
    fn ipv4_classes_match_rfc_scopes() {
        for (remote, want) in [
            ("127.0.0.1:3425", AddrClass::Loopback),
            ("10.1.2.3:443", AddrClass::Private),
            ("172.16.5.5:80", AddrClass::Private),
            ("172.31.9.9:80", AddrClass::Private),
            ("172.32.1.1:80", AddrClass::Public),
            ("192.168.1.1:1900", AddrClass::Private),
            ("100.64.0.1:443", AddrClass::Cgnat),
            ("100.127.255.1:443", AddrClass::Cgnat),
            ("100.128.0.1:443", AddrClass::Public),
            ("169.254.3.3:0", AddrClass::LinkLocal),
            ("224.0.0.251:5353", AddrClass::Multicast),
            ("0.0.0.0:445", AddrClass::Unspecified),
            ("8.8.8.8:53", AddrClass::Public),
        ] {
            assert_eq!(parse_remote(remote).class, want, "{remote}");
        }
    }

    #[test]
    fn malformed_remotes_are_invalid() {
        for remote in [
            "example.com:443",
            "1.2.3:443",
            "1.2.3.4:99999",
            "1.2.3.4:x",
            "1.2.3.4",
        ] {
            assert_eq!(parse_remote(remote).class, AddrClass::Invalid, "{remote}");
        }
        // 八位组越界（256）与带符号也是 Invalid。
        assert_eq!(parse_remote("256.1.1.1:80").class, AddrClass::Invalid);
        assert_eq!(parse_remote("+1.2.3.4:80").class, AddrClass::Invalid);
    }

    #[test]
    fn ipv6_classes_and_brackets() {
        for (remote, want) in [
            ("::1:443", AddrClass::Loopback),   // 采集端拼法（无括号）
            ("[::1]:443", AddrClass::Loopback), // 带括号也能剥
            ("::ffff:93.184.216.34:80", AddrClass::Public), // v4 映射
            ("::ffff:10.0.0.1:80", AddrClass::Private),
            ("fe80::1:445", AddrClass::LinkLocal),
            ("fd00::1:80", AddrClass::Private),
            ("ff02::1:5353", AddrClass::Multicast),
            (":::445", AddrClass::Unspecified), // 0.0.0.0/:: 的 v6 监听行
            ("2606:4700:4700::1111:443", AddrClass::Public),
        ] {
            assert_eq!(parse_remote(remote).class, want, "{remote}");
        }
    }

    #[test]
    fn public_egress_is_flagged_private_is_not() {
        let events = vec![
            conn(1, "93.184.216.34:443", "Established"),
            conn(1, "192.168.1.9:445", "Established"),
            conn(1, "10.1.2.3:443", "Established"), // 10/8 也算私网（变异门曾暴露漏覆盖）
            conn(1, "100.64.1.1:443", "Established"), // CGNAT 算外部
        ];
        let findings = network_findings(&events);
        let outbound = findings
            .iter()
            .find(|f| f.id == "outbound-connection")
            .expect("公网连接必须报");
        assert_eq!(outbound.evidence.len(), 2, "{:?}", outbound.evidence);
        // 证据按 (类别, 主机, 端口) 排序——断言与顺序无关。
        assert!(
            outbound
                .evidence
                .iter()
                .any(|e| e.contains("93.184.216.34")),
            "{:?}",
            outbound.evidence
        );
        assert!(
            outbound.evidence.iter().any(|e| e.contains("100.64.1.1")),
            "{:?}",
            outbound.evidence
        );
        for private in ["192.168.1.9", "10.1.2.3"] {
            assert!(
                !outbound.evidence.iter().any(|e| e.contains(private)),
                "内网 {private} 不应进外联：{:?}",
                outbound.evidence
            );
        }
    }

    #[test]
    fn listen_all_interfaces_is_flagged_loopback_is_not() {
        let events = vec![
            conn(5, "0.0.0.0:445", "Listen"),
            conn(5, ":::3389", "Listen"),
            conn(5, "127.0.0.1:8080", "Listen"), // 只听回环：不报
            conn(5, "192.168.1.5:80", "Listen"), // 只听内网单卡：也不报「所有网卡」
        ];
        let findings = network_findings(&events);
        let listen = findings
            .iter()
            .find(|f| f.id == "listen-all-interfaces")
            .expect("0.0.0.0 / :: 监听必须报");
        assert_eq!(listen.evidence.len(), 2, "{:?}", listen.evidence);
    }

    #[test]
    fn egress_fanout_needs_eight_distinct_hosts() {
        let mut events = Vec::new();
        for i in 0..8u32 {
            events.push(conn(7, &format!("23.{}.0.1:443", 100 + i), "Established"));
        }
        events.push(conn(7, "23.200.0.1:443", "Established")); // 同 pid 第 9 个
        let findings = network_findings(&events);
        assert!(
            findings.iter().any(|f| f.id == "egress-fanout"),
            "{findings:?}"
        );

        // 只有 7 个主机：不报。
        let mut short = Vec::new();
        for i in 0..7u32 {
            short.push(conn(8, &format!("23.{}.0.1:443", 100 + i), "Established"));
        }
        assert!(
            !network_findings(&short)
                .iter()
                .any(|f| f.id == "egress-fanout")
        );
    }

    #[test]
    fn repeated_samples_deduplicate_before_thresholds() {
        // 同一条连接被采样 10 次 ≠ 扇出 10 主机。
        let mut events = Vec::new();
        for _ in 0..10u32 {
            events.push(conn(9, "23.1.0.1:443", "Established"));
        }
        assert!(
            !network_findings(&events)
                .iter()
                .any(|f| f.id == "egress-fanout")
        );
    }

    #[test]
    fn syn_sent_pileup_is_flagged() {
        let mut events = Vec::new();
        for i in 0..4u32 {
            events.push(conn(3, &format!("23.{}.9.9:80", 100 + i), "SynSent"));
        }
        let findings = network_findings(&events);
        assert!(
            findings.iter().any(|f| f.id == "connect-failures"),
            "{findings:?}"
        );
        // 单个目标重试（同 host:port）：是采样去重后的一条，不报。
        let mut single = Vec::new();
        for _ in 0..6u32 {
            single.push(conn(4, "23.9.9.9:80", "SynSent"));
        }
        assert!(
            !network_findings(&single)
                .iter()
                .any(|f| f.id == "connect-failures")
        );
    }

    #[test]
    fn no_connections_no_findings() {
        assert!(network_findings(&[]).is_empty());
        let events = vec![conn(1, "127.0.0.1:3425", "Established")];
        assert!(network_findings(&events).is_empty());
    }
}
