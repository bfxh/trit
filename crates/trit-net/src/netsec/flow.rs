//! 双向五元组流聚合。
//!
//! 键是无向的：`(地址, 端口)` 两个端点排序后加协议号。方向由**首个报文**决定，
//! 之后 `fwd_*` 一律指「和首包同向」，不随后续包变化。

use std::collections::HashMap;
use std::fmt;

use super::packet::{Addr, Packet, Transport};

/// 一个端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint {
    /// 地址。
    pub addr: Addr,
    /// 端口。
    pub port: u16,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.addr, self.port)
    }
}

/// 无向流键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FlowKey {
    /// 排序靠前的端点。
    pub lo: Endpoint,
    /// 排序靠后的端点。
    pub hi: Endpoint,
    /// IP 协议号（6 / 17）。
    pub proto: u8,
}

impl FlowKey {
    /// 规范化两个端点。
    pub fn new(a: Endpoint, b: Endpoint, proto: u8) -> Self {
        if a <= b {
            Self {
                lo: a,
                hi: b,
                proto,
            }
        } else {
            Self {
                lo: b,
                hi: a,
                proto,
            }
        }
    }
}

/// 特征维数，与 [`FlowFeatures::values`] 和 [`NAMES`] 对齐。
pub const N: usize = 24;

/// 特征名，顺序即 [`FlowFeatures::values`] 的下标。
pub const NAMES: [&str; N] = [
    "duration_s",
    "fwd_pkts",
    "bwd_pkts",
    "fwd_bytes",
    "bwd_bytes",
    "fwd_pkt_len_mean",
    "fwd_pkt_len_std",
    "fwd_pkt_len_min",
    "fwd_pkt_len_max",
    "bwd_pkt_len_mean",
    "bwd_pkt_len_std",
    "bwd_pkt_len_min",
    "bwd_pkt_len_max",
    "iat_mean_ms",
    "iat_std_ms",
    "iat_min_ms",
    "iat_max_ms",
    "pkt_rate",
    "byte_rate",
    "fwd_pkt_ratio",
    "syn",
    "rst",
    "fin",
    "psh",
];

/// 一条流的特征向量。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FlowFeatures {
    /// 首包到末包的秒数。
    pub duration_s: f64,
    /// 正向包数。
    pub fwd_pkts: f64,
    /// 反向包数。
    pub bwd_pkts: f64,
    /// 正向载荷字节。
    pub fwd_bytes: f64,
    /// 反向载荷字节。
    pub bwd_bytes: f64,
    /// 正向包长均值。
    pub fwd_pkt_len_mean: f64,
    /// 正向包长标准差（样本标准差）。
    pub fwd_pkt_len_std: f64,
    /// 正向最小包长。
    pub fwd_pkt_len_min: f64,
    /// 正向最大包长。
    pub fwd_pkt_len_max: f64,
    /// 反向包长均值。
    pub bwd_pkt_len_mean: f64,
    /// 反向包长标准差。
    pub bwd_pkt_len_std: f64,
    /// 反向最小包长。
    pub bwd_pkt_len_min: f64,
    /// 反向最大包长。
    pub bwd_pkt_len_max: f64,
    /// 包间隔均值（毫秒）。
    pub iat_mean_ms: f64,
    /// 包间隔标准差（毫秒）。
    pub iat_std_ms: f64,
    /// 最小包间隔（毫秒）。
    pub iat_min_ms: f64,
    /// 最大包间隔（毫秒）。
    pub iat_max_ms: f64,
    /// 包速率（包/秒）。
    pub pkt_rate: f64,
    /// 字节速率（载荷字节/秒）。
    pub byte_rate: f64,
    /// 正向包占比。
    pub fwd_pkt_ratio: f64,
    /// SYN 包数。
    pub syn: f64,
    /// RST 包数。
    pub rst: f64,
    /// FIN 包数。
    pub fin: f64,
    /// PSH 包数。
    pub psh: f64,
}

impl FlowFeatures {
    /// 按 [`NAMES`] 顺序取值。
    pub fn values(&self) -> [f64; N] {
        [
            self.duration_s,
            self.fwd_pkts,
            self.bwd_pkts,
            self.fwd_bytes,
            self.bwd_bytes,
            self.fwd_pkt_len_mean,
            self.fwd_pkt_len_std,
            self.fwd_pkt_len_min,
            self.fwd_pkt_len_max,
            self.bwd_pkt_len_mean,
            self.bwd_pkt_len_std,
            self.bwd_pkt_len_min,
            self.bwd_pkt_len_max,
            self.iat_mean_ms,
            self.iat_std_ms,
            self.iat_min_ms,
            self.iat_max_ms,
            self.pkt_rate,
            self.byte_rate,
            self.fwd_pkt_ratio,
            self.syn,
            self.rst,
            self.fin,
            self.psh,
        ]
    }
}

/// 流式均值 / 方差（Welford），避免先存整个序列。
#[derive(Debug, Clone, Copy, Default)]
struct Stats {
    n: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

impl Stats {
    fn push(&mut self, value: f64) {
        self.n += 1;
        let delta = value - self.mean;
        self.mean += delta / self.n as f64;
        self.m2 += delta * (value - self.mean);
        if self.n == 1 {
            self.min = value;
            self.max = value;
        } else {
            if value < self.min {
                self.min = value;
            }
            if value > self.max {
                self.max = value;
            }
        }
    }

    fn std(&self) -> f64 {
        if self.n > 1 {
            (self.m2 / (self.n - 1) as f64).sqrt()
        } else {
            0.0
        }
    }
}

/// 一条流。
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    /// 流键。
    pub key: FlowKey,
    /// 首包发送方。
    pub initiator: Endpoint,
    /// 首包时间（秒）。
    pub first_ts: f64,
    /// 特征。
    pub features: FlowFeatures,
}

impl Flow {
    /// 首包发送方（正向）。
    pub const fn src(&self) -> Endpoint {
        self.initiator
    }

    /// 首包接收方（反向）。
    pub fn dst(&self) -> Endpoint {
        if self.initiator == self.key.lo {
            self.key.hi
        } else {
            self.key.lo
        }
    }
}

#[derive(Debug, Clone)]
struct FlowState {
    initiator: Endpoint,
    first_ts: f64,
    last_ts: f64,
    packets: u64,
    fwd_pkts: u64,
    bwd_pkts: u64,
    fwd_bytes: u64,
    bwd_bytes: u64,
    fwd_len: Stats,
    bwd_len: Stats,
    iat: Stats,
    syn: u64,
    rst: u64,
    fin: u64,
    psh: u64,
}

impl FlowState {
    fn new(ts: f64, initiator: Endpoint) -> Self {
        Self {
            initiator,
            first_ts: ts,
            last_ts: ts,
            packets: 0,
            fwd_pkts: 0,
            bwd_pkts: 0,
            fwd_bytes: 0,
            bwd_bytes: 0,
            fwd_len: Stats::default(),
            bwd_len: Stats::default(),
            iat: Stats::default(),
            syn: 0,
            rst: 0,
            fin: 0,
            psh: 0,
        }
    }

    fn push(&mut self, ts: f64, forward: bool, packet: &Packet) {
        if self.packets > 0 {
            // 乱序抓包会出现负间隔；钳到 0，不让它污染方差。
            self.iat.push((ts - self.last_ts).max(0.0) * 1000.0);
        }
        if ts > self.last_ts {
            self.last_ts = ts;
        }
        self.packets += 1;
        let length = packet.payload_len as f64;
        if forward {
            self.fwd_pkts += 1;
            self.fwd_bytes += packet.payload_len as u64;
            self.fwd_len.push(length);
        } else {
            self.bwd_pkts += 1;
            self.bwd_bytes += packet.payload_len as u64;
            self.bwd_len.push(length);
        }
        if let Transport::Tcp { flags, .. } = packet.transport {
            self.syn += u64::from(flags & 0x02 != 0);
            self.rst += u64::from(flags & 0x04 != 0);
            self.fin += u64::from(flags & 0x01 != 0);
            self.psh += u64::from(flags & 0x08 != 0);
        }
    }

    fn finish(self, key: FlowKey) -> Flow {
        let duration = (self.last_ts - self.first_ts).max(0.0);
        let total_pkts = self.fwd_pkts + self.bwd_pkts;
        let total_bytes = self.fwd_bytes + self.bwd_bytes;
        let (pkt_rate, byte_rate) = if duration > 0.0 {
            (total_pkts as f64 / duration, total_bytes as f64 / duration)
        } else {
            (0.0, 0.0)
        };
        let ratio = if total_pkts > 0 {
            self.fwd_pkts as f64 / total_pkts as f64
        } else {
            0.0
        };
        Flow {
            key,
            initiator: self.initiator,
            first_ts: self.first_ts,
            features: FlowFeatures {
                duration_s: duration,
                fwd_pkts: self.fwd_pkts as f64,
                bwd_pkts: self.bwd_pkts as f64,
                fwd_bytes: self.fwd_bytes as f64,
                bwd_bytes: self.bwd_bytes as f64,
                fwd_pkt_len_mean: self.fwd_len.mean,
                fwd_pkt_len_std: self.fwd_len.std(),
                fwd_pkt_len_min: self.fwd_len.min,
                fwd_pkt_len_max: self.fwd_len.max,
                bwd_pkt_len_mean: self.bwd_len.mean,
                bwd_pkt_len_std: self.bwd_len.std(),
                bwd_pkt_len_min: self.bwd_len.min,
                bwd_pkt_len_max: self.bwd_len.max,
                iat_mean_ms: self.iat.mean,
                iat_std_ms: self.iat.std(),
                iat_min_ms: self.iat.min,
                iat_max_ms: self.iat.max,
                pkt_rate,
                byte_rate,
                fwd_pkt_ratio: ratio,
                syn: self.syn as f64,
                rst: self.rst as f64,
                fin: self.fin as f64,
                psh: self.psh as f64,
            },
        }
    }
}

/// 流表。`finish` 之前不做超时切分——pcap 是离线数据，切分规则要显式给。
#[derive(Debug, Clone, Default)]
pub struct FlowTable {
    flows: HashMap<FlowKey, FlowState>,
}

impl FlowTable {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 投递一条报文。
    pub fn ingest(&mut self, ts: f64, packet: &Packet) {
        let (sport, dport) = packet.transport.ports();
        let src = Endpoint {
            addr: packet.src,
            port: sport,
        };
        let dst = Endpoint {
            addr: packet.dst,
            port: dport,
        };
        let key = FlowKey::new(src, dst, packet.transport.proto());
        let state = self
            .flows
            .entry(key)
            .or_insert_with(|| FlowState::new(ts, src));
        state.push(ts, src == state.initiator, packet);
    }

    /// 当前流数。
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// 收尾，按首包时间排序（同一时刻按流键，保证输出确定）。
    pub fn finish(self) -> Vec<Flow> {
        let mut flows: Vec<Flow> = self
            .flows
            .into_iter()
            .map(|(key, state)| state.finish(key))
            .collect();
        flows.sort_by(|a, b| {
            a.first_ts
                .total_cmp(&b.first_ts)
                .then_with(|| a.key.cmp(&b.key))
        });
        flows
    }
}
