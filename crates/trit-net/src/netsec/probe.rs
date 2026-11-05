//! 受控探测（M3）：allowlist 默认拒绝的端口/服务指纹，只对实验室目标。
//!
//! `docs/SECURITY-NET.md` 授权边界的落点：**只对自有实验室**（环回、私网、
//! 本机虚拟机）发包；任何主动发包必须走显式 allowlist，**默认拒绝**；
//! 不做 DoS/压测式打击、大规模扫描、规避检测、向第三方投递载荷。
//!
//! 机械化的纪律（每条都可测）：
//!
//! 1. **只吃字面 IP**：allowlist 里写 DNS 名一律拒绝——不做 DNS 解析，
//!    SSRF/DNS 重绑定面从协议层排除（与 trit-immune `net` 出网守卫同一硬约束）。
//! 2. **实验室范围校验**：环回 / v4 私网 / v4 链路本地 / `::1` / ULA / v6 链路本地
//!    之外一律拒绝（公网、组播、CGNAT 100.64/10、基准测试 198.18/15 都不在内）。
//! 3. **默认拒绝**：`run` 只探测 allowlist 里显式列出的 (ip, port)；allowlist 之外
//!    的目标不存在"顺带扫一下"。范围外/未列出的目标进 `refused` 清单，不静默。
//! 4. **硬上限防扫全段**：目标数 / 每目标端口数 / 总探测数三级上限，配置超过
//!    硬顶直接拒绝运行（`ConfigTooHot`）——这不是性能调优，是把"大规模扫描"
//!    从量纲上排除。端口必须是**显式列表**，不支持端口段（段 = 扫描的形状）。
//! 5. **限速串行**：单线程顺序连接 + 每次探测间隔 `delay` + 全局 deadline。
//! 6. **结果只分类不升格**：连接成功 = `T`；明确拒绝（RST）= `F`；超时/不可达 =
//!    `X`（分不清 filtered 还是 down，不许替网络下结论）。
//!    平台现实：Windows 的 `connect_timeout` 常把拒绝表面化成超时——那时落 `X`，
//!    宁可少报信息也不多报结论（`F` 只在拿到明确 RST 时给）。
//!    截断时只少报（缺失的探测不出现在 findings），已报的每条都带
//!    `source = "tcp-connect"` 可追溯。
//!
//! banner 捕获与知名端口提示只是**指纹线索**（`note` 字段），不是判决；
//! 判决只有上面三种。

use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::attack::{AttackError, AttackGraph, Evidence};

/// 允许的连接超时上限（毫秒）。
pub const MAX_CONNECT_TIMEOUT_MS: u64 = 5_000;
/// 允许的单次 banner 读取超时上限（毫秒）。
pub const MAX_BANNER_TIMEOUT_MS: u64 = 2_000;
/// 允许的单次探测间隔上限（毫秒）——间隔本身不是能力，顶住即可。
pub const MAX_DELAY_MS: u64 = 60_000;
/// 单次运行的目标数硬顶。
pub const HARD_MAX_TARGETS: usize = 64;
/// 每目标端口数硬顶。
pub const HARD_MAX_PORTS_PER_TARGET: usize = 32;
/// 单次运行总探测数硬顶。
pub const HARD_MAX_TOTAL_PROBES: usize = 512;
/// banner 捕获字节数上限。
pub const MAX_BANNER_BYTES: usize = 256;

/// 探测错误：**授权与配置类**错误拒绝运行；单个目标的网络结果不走这里
/// （它们被分类成证据，见 [`classify_connect`]）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// allowlist 里的地址不是字面 IP（DNS 名/解析失败）。
    #[error("「{0}」不是字面 IP：不做 DNS 解析，allowlist 只接受字面地址")]
    NonLiteralAddress(String),
    /// 地址在实验室范围（环回/私网/链路本地）之外。
    #[error("{0} 不在实验室范围（环回/私网/链路本地/ULA）之外只对自有实验室发包")]
    OutsideLab(String),
    /// 配置超过硬上限——拒绝运行而不是缩放。
    #[error("配置超硬上限：{0}")]
    ConfigTooHot(&'static str),
    /// allowlist JSON 解析失败。
    #[error("allowlist 解析失败：{0}")]
    Json(String),
    /// 登记进攻击图失败（出处为空/端点缺失——桥自己的不变量被破坏时才可能）。
    #[error("登记进攻击图失败：{0}")]
    Attach(#[from] AttackError),
}

/// 探测配置；上限用 [`ProbeConfig::validate`] 校验，超过硬顶拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    /// 单次 TCP 连接超时。
    pub connect_timeout: Duration,
    /// 连接成功后 banner 读取超时。
    pub banner_timeout: Duration,
    /// 相邻两次探测的间隔（限速）。
    pub delay: Duration,
    /// 全局 deadline。
    pub deadline: Duration,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_millis(1_500),
            banner_timeout: Duration::from_millis(500),
            delay: Duration::from_millis(100),
            deadline: Duration::from_secs(60),
        }
    }
}

impl ProbeConfig {
    /// 校验硬上限；超了就报 [`ProbeError::ConfigTooHot`]，不静默缩放。
    pub fn validate(&self) -> Result<(), ProbeError> {
        if self.connect_timeout.is_zero()
            || self.connect_timeout > Duration::from_millis(MAX_CONNECT_TIMEOUT_MS)
        {
            return Err(ProbeError::ConfigTooHot(
                "connect_timeout 必须在 1..=5000ms",
            ));
        }
        if self.banner_timeout > Duration::from_millis(MAX_BANNER_TIMEOUT_MS) {
            return Err(ProbeError::ConfigTooHot("banner_timeout 必须 <= 2000ms"));
        }
        if self.delay > Duration::from_millis(MAX_DELAY_MS) {
            return Err(ProbeError::ConfigTooHot("delay 必须 <= 60000ms"));
        }
        if self.deadline.is_zero() {
            return Err(ProbeError::ConfigTooHot("deadline 必须为正"));
        }
        Ok(())
    }
}

/// 一个实验室目标：字面 IP + 显式端口列表（不支持端口段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// 字面 IP。
    pub ip: String,
    /// 显式端口列表。
    pub ports: Vec<u16>,
}

/// allowlist：显式列出的 (ip, port) 全集。**默认拒绝**——没列的都不探测。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowlist {
    targets: Vec<(IpAddr, Vec<u16>)>,
}

impl Allowlist {
    /// 从 JSON 文本解析：`{"targets":[{"ip":"127.0.0.1","ports":[80,443]}]}`。
    ///
    /// 解析时即校验：字面 IP、实验室范围、端口列表非空且去重排序；
    /// 目标数/每目标端口数超硬顶在此拒绝。
    pub fn from_json(text: &str) -> Result<Self, ProbeError> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            targets: Vec<Target>,
        }
        let raw: Raw =
            serde_json::from_str(text).map_err(|error| ProbeError::Json(error.to_string()))?;
        if raw.targets.len() > HARD_MAX_TARGETS {
            return Err(ProbeError::ConfigTooHot(
                "目标数超过硬顶（64）——这个量纲就是扫描",
            ));
        }
        let mut targets = Vec::new();
        for entry in raw.targets {
            let ip: IpAddr = entry
                .ip
                .parse()
                .map_err(|_| ProbeError::NonLiteralAddress(entry.ip.clone()))?;
            if !is_lab_address(ip) {
                return Err(ProbeError::OutsideLab(entry.ip.clone()));
            }
            if entry.ports.len() > HARD_MAX_PORTS_PER_TARGET {
                return Err(ProbeError::ConfigTooHot(
                    "单目标端口数超过硬顶（32）——端口段是扫描的形状，用显式列表",
                ));
            }
            let mut ports = entry.ports;
            ports.sort_unstable();
            ports.dedup();
            if ports.is_empty() {
                return Err(ProbeError::Json(format!("{} 的端口列表为空", entry.ip)));
            }
            targets.push((ip, ports));
        }
        Ok(Self { targets })
    }

    /// 是否允许探测这个 (ip, port)。
    pub fn permits(&self, ip: IpAddr, port: u16) -> bool {
        self.targets
            .iter()
            .any(|(target_ip, ports)| *target_ip == ip && ports.contains(&port))
    }

    /// 目标数。
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// 环回 / v4 私网 / v4 链路本地 / `::1` / ULA / v6 链路本地。
///
/// 刻意**不包含**：CGNAT 100.64/10、基准测试 198.18/15、组播、公网——
/// 「自有实验室」宁窄勿宽。
pub fn is_lab_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            let segments = v6.segments();
            // fc00::/7（ULA，含 fd00::/8 常用段）
            if segments[0] & 0xfe00 == 0xfc00 {
                return true;
            }
            // fe80::/10（链路本地）
            segments[0] & 0xffc0 == 0xfe80
        }
    }
}

/// 连接结果 → 证据与备注。纯函数，单测钉死。
///
/// - 成功 = `T`（端口开放，直接证据）
/// - 明确拒绝（RST）= `F`
/// - 其余（超时/不可达/被过滤/其他 IO 错误）= `X`——分不清就不下结论
pub fn classify_connect(result: &std::io::Result<TcpStream>) -> (Evidence, String) {
    match result {
        Ok(_) => (Evidence::Confirmed, "connected".to_string()),
        Err(error) => match error.kind() {
            std::io::ErrorKind::ConnectionRefused => (Evidence::RuledOut, "refused".to_string()),
            _ => (Evidence::Unverified, format!("{}", error.kind())),
        },
    }
}

/// 知名端口提示（只是线索，写进 note；不是判决）。
fn port_hint(port: u16) -> Option<&'static str> {
    Some(match port {
        22 => "ssh?",
        80 => "http?",
        443 => "https?",
        445 => "smb?",
        3306 => "mysql?",
        3389 => "rdp?",
        5432 => "postgres?",
        6379 => "redis?",
        8080 => "http-alt?",
        9200 => "elasticsearch?",
        _ => return None,
    })
}

/// banner 捕获：连接成功后读一小段，只留可打印 ASCII（其余转 `.`）。
fn grab_banner(stream: &TcpStream, timeout: Duration) -> String {
    let mut buf = [0u8; MAX_BANNER_BYTES];
    stream.set_read_timeout(Some(timeout)).ok();
    let mut limited = stream.take(MAX_BANNER_BYTES as u64);
    match limited.read(&mut buf) {
        Ok(0) | Err(_) => String::new(),
        Ok(n) => buf[..n]
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) || b == b'\r' || b == b'\n' {
                    b as char
                } else {
                    '.'
                }
            })
            .collect(),
    }
}

/// 单条探测发现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeFinding {
    /// 字面 IP。
    pub ip: String,
    /// 端口。
    pub port: u16,
    /// 证据等级（见 [`classify_connect`]）。
    pub evidence: Evidence,
    /// 证据出处——固定 `tcp-connect`，进攻击图的 `source` 字段。
    pub source: String,
    /// 线索：连接备注 + 知名端口提示 + banner 摘录（可空）。
    pub note: String,
}

/// 一次受控探测的完整报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// 逐条发现（按执行序）。
    pub findings: Vec<ProbeFinding>,
    /// 被拒绝执行的目标（allowlist 校验在加载时已挡；这里是运行期兜底，
    /// 目前恒空——保留给调用方在自定义目标源时复用）。
    pub refused: Vec<String>,
    /// allowlist 里计划的总探测数。
    pub planned: usize,
    /// 实际执行的探测数（deadline 截断时 < planned）。
    pub probed: usize,
    /// 是否因 deadline 提前收工（**只少报**：没跑的不出现在 findings）。
    pub truncated: bool,
}

impl ProbeReport {
    /// 导出 JSONL（每条 finding 一行），可直接进台账。
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for finding in &self.findings {
            out.push_str(&serde_json::to_string(finding).expect("finding 序列化不会失败"));
            out.push('\n');
        }
        out
    }
}

/// 执行受控探测：顺序遍历 allowlist，逐 (ip, port) 连接并分类。
///
/// 目标顺序按 allowlist 声明序；端口按升序（加载时已排序）。
/// 每次探测之间 `delay` 限速；全局 `deadline` 到点即收工并置 `truncated`。
pub fn run(allowlist: &Allowlist, config: &ProbeConfig) -> Result<ProbeReport, ProbeError> {
    config.validate()?;
    let started = Instant::now();
    let planned: usize = allowlist.targets.iter().map(|(_, ports)| ports.len()).sum();
    let mut report = ProbeReport {
        planned,
        ..ProbeReport::default()
    };
    'outer: for (index, (ip, ports)) in allowlist.targets.iter().enumerate() {
        let _ = index;
        for &port in ports {
            if started.elapsed() >= config.deadline {
                report.truncated = true;
                break 'outer;
            }
            let addr = SocketAddr::new(*ip, port);
            let attempt = TcpStream::connect_timeout(&addr, config.connect_timeout);
            let (evidence, mut note) = classify_connect(&attempt);
            if evidence == Evidence::Confirmed {
                let stream = attempt.expect("Confirmed 必有流");
                let banner = grab_banner(&stream, config.banner_timeout);
                if let Some(hint) = port_hint(port) {
                    note.push_str(&format!(" {hint}"));
                }
                if !banner.is_empty() {
                    note.push_str(&format!(" banner:{}", banner.replace('\n', "\\n")));
                }
            }
            report.findings.push(ProbeFinding {
                ip: ip.to_string(),
                port,
                evidence,
                source: "tcp-connect".to_string(),
                note,
            });
            report.probed += 1;
            if config.delay > Duration::ZERO {
                std::thread::sleep(config.delay);
            }
        }
    }
    Ok(report)
}

/// 登记桥的计数：这次登记保证了多少节点与边存在（幂等重跑数字不变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AttachSummary {
    /// 主机节点数。
    pub hosts: usize,
    /// 服务节点数（每条 finding 一个）。
    pub services: usize,
    /// 登记的边数（station→host 每主机一条 + host→service 每探测一条）。
    pub edges: usize,
}

/// 从 findings JSONL（每行一个 [`ProbeFinding`]）复原清单；坏行显式报错。
pub fn findings_from_jsonl(text: &str) -> Result<Vec<ProbeFinding>, ProbeError> {
    let mut findings = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let finding: ProbeFinding = serde_json::from_str(line)
            .map_err(|error| ProbeError::Json(format!("第 {} 行：{error}", number + 1)))?;
        findings.push(finding);
    }
    Ok(findings)
}

/// probe 报告 → 攻击图的**半自动登记桥**。
///
/// 分工：桥机械地登记**探测证据**（出处直接用 finding 的 `source`，即
/// `tcp-connect`）；攻击语义（哪个利用、哪条路径）由人随后用 LWW 重新登记
/// 改写——同端点/同 label 后写覆盖是攻击图的既定纪律，桥不会锁死语义。
///
/// 结构（就这三层，不猜更多）：
///
/// - `station_label` 节点：`T`（probe 从这里发出，确定存在）；
/// - 每主机一个节点：label = 字面 IP，证据 = **主机应答**——任一探测拿到
///   `T` 或 `F` 就算主机在网（`F`/refused 也是主机回的 RST），全 `X` 才是
///   `X`（分不清 filtered 还是没这主机）；
/// - 每服务一个节点：label = `ip:port`，证据 = finding 的证据，备注 =
///   finding 的 note（banner 线索跟过去）；
/// - 边 station→host（应答证据）与 host→service（finding 证据），出处都是
///   `tcp-connect`。
///
/// 幂等：同 label 节点 LWW 覆盖、同端点边 LWW 覆盖 ⇒ 同一份报告重复登记图不变；
/// 换一份新报告重登记（端口状态变了）⇒ 节点与边都被刷新成最新证据。
pub fn attach_probe(
    graph: &mut AttackGraph,
    findings: &[ProbeFinding],
    station_label: &str,
) -> Result<AttachSummary, ProbeError> {
    let station = graph.add_node(station_label, Evidence::Confirmed, "probe 起点");
    let mut summary = AttachSummary::default();
    // 按主机分组，保持报告顺序。
    let mut groups: Vec<(String, Vec<&ProbeFinding>)> = Vec::new();
    for finding in findings {
        match groups.iter_mut().find(|(ip, _)| *ip == finding.ip) {
            Some((_, group)) => group.push(finding),
            None => groups.push((finding.ip.clone(), vec![finding])),
        }
    }
    for (ip, group) in &groups {
        let answered = group
            .iter()
            .any(|finding| finding.evidence != Evidence::Unverified);
        let host_evidence = if answered {
            Evidence::Confirmed
        } else {
            Evidence::Unverified
        };
        let note = format!("probe 探得 {} 项", group.len());
        let host = graph.add_node(ip, host_evidence, &note);
        graph.add_edge(station, host, host_evidence, "tcp-connect")?;
        summary.hosts += 1;
        summary.edges += 1;
        for finding in group {
            let service_label = format!("{}:{}", finding.ip, finding.port);
            let service = graph.add_node(&service_label, finding.evidence, &finding.note);
            graph.add_edge(host, service, finding.evidence, &finding.source)?;
            summary.services += 1;
            summary.edges += 1;
        }
    }
    Ok(summary)
}
