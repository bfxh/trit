//! 链路 / 网络 / 传输层解码，只取流特征需要的字段。
//!
//! 解不出来时返回 [`Skip`] 并让调用方计数——「跳过」必须可见，
//! 否则一份解不动的抓包会被读成「零流量的干净样本」。

use std::fmt;

use super::pcap::LinkType;

/// IP 地址。`Ord` 用于流键的规范化排序，不表示任何路由语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Addr {
    /// IPv4。
    V4([u8; 4]),
    /// IPv6。
    V6([u8; 16]),
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(bytes) => write!(f, "{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3]),
            Self::V6(bytes) => {
                for (index, chunk) in bytes.chunks(2).enumerate() {
                    if index > 0 {
                        f.write_str(":")?;
                    }
                    write!(f, "{:x}", u16::from_be_bytes([chunk[0], chunk[1]]))?;
                }
                Ok(())
            }
        }
    }
}

/// 传输层头部的必要字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// TCP。
    Tcp {
        /// 源端口。
        sport: u16,
        /// 目的端口。
        dport: u16,
        /// 标志位字节（`FIN=0x01 … URG=0x20`）。
        flags: u8,
    },
    /// UDP。
    Udp {
        /// 源端口。
        sport: u16,
        /// 目的端口。
        dport: u16,
    },
}

impl Transport {
    /// 源 / 目的端口。
    pub const fn ports(self) -> (u16, u16) {
        match self {
            Self::Tcp { sport, dport, .. } | Self::Udp { sport, dport } => (sport, dport),
        }
    }

    /// IP 协议号。
    pub const fn proto(self) -> u8 {
        match self {
            Self::Tcp { .. } => 6,
            Self::Udp { .. } => 17,
        }
    }
}

/// 解出来的一层报文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    /// 源地址。
    pub src: Addr,
    /// 目的地址。
    pub dst: Addr,
    /// 传输层。
    pub transport: Transport,
    /// 传输层载荷长度（不含 TCP/UDP 头）。
    pub payload_len: usize,
}

/// 跳过一条报文的原因。每一种都要计数上报。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// 不是 IPv4/IPv6。
    NotIp,
    /// IP 分片的非首片——没有传输层头。
    Fragment,
    /// 传输层不是 TCP/UDP。
    NotTcpUdp,
    /// 头部长度不足或自相矛盾。
    Malformed(&'static str),
}

impl Skip {
    /// 稳定字符串，用于计数与 JSON 输出。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotIp => "not-ip",
            Self::Fragment => "fragment",
            Self::NotTcpUdp => "not-tcp-udp",
            Self::Malformed(what) => what,
        }
    }
}

/// 按链路层类型解码。
pub fn decode(link: LinkType, frame: &[u8]) -> Result<Packet, Skip> {
    match link {
        LinkType::Ethernet => {
            let (ethertype, payload) = ethernet(frame)?;
            match ethertype {
                0x0800 => ipv4(payload),
                0x86dd => ipv6(payload),
                _ => Err(Skip::NotIp),
            }
        }
        LinkType::RawIp => match frame.first().map(|byte| byte >> 4) {
            Some(4) => ipv4(frame),
            Some(6) => ipv6(frame),
            _ => Err(Skip::NotIp),
        },
    }
}

fn ethernet(frame: &[u8]) -> Result<(u16, &[u8]), Skip> {
    if frame.len() < 14 {
        return Err(Skip::Malformed("以太网头"));
    }
    let mut ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    let mut offset = 14;
    let mut tags = 0;
    while ethertype == 0x8100 || ethertype == 0x88a8 {
        if frame.len() < offset + 4 {
            return Err(Skip::Malformed("vlan-tag"));
        }
        ethertype = u16::from_be_bytes([frame[offset + 2], frame[offset + 3]]);
        offset += 4;
        tags += 1;
        if tags > 2 {
            return Err(Skip::Malformed("vlan-depth"));
        }
    }
    Ok((ethertype, &frame[offset..]))
}

fn ipv4(payload: &[u8]) -> Result<Packet, Skip> {
    if payload.len() < 20 {
        return Err(Skip::Malformed("ipv4-header"));
    }
    if payload[0] >> 4 != 4 {
        return Err(Skip::Malformed("ipv4-version"));
    }
    let header_len = usize::from(payload[0] & 0x0f) * 4;
    if header_len < 20 || payload.len() < header_len {
        return Err(Skip::Malformed("ipv4-ihl"));
    }
    let total = usize::from(u16::from_be_bytes([payload[2], payload[3]]));
    if total < header_len {
        return Err(Skip::Malformed("ipv4-total-length"));
    }
    let end = total.min(payload.len());
    let fragment = u16::from_be_bytes([payload[6], payload[7]]);
    if fragment & 0x1fff != 0 {
        return Err(Skip::Fragment);
    }
    let src = Addr::V4([payload[12], payload[13], payload[14], payload[15]]);
    let dst = Addr::V4([payload[16], payload[17], payload[18], payload[19]]);
    transport(payload[9], src, dst, &payload[header_len..end])
}

fn ipv6(payload: &[u8]) -> Result<Packet, Skip> {
    if payload.len() < 40 {
        return Err(Skip::Malformed("ipv6-header"));
    }
    let declared = usize::from(u16::from_be_bytes([payload[4], payload[5]]));
    let end = (40 + declared).min(payload.len());
    let src = Addr::V6(payload[8..24].try_into().expect("长度已校验：IPv6 源地址"));
    let dst = Addr::V6(
        payload[24..40]
            .try_into()
            .expect("长度已校验：IPv6 目的地址"),
    );
    let mut next = payload[6];
    let mut offset = 40;
    let mut hops = 0;
    loop {
        match next {
            0 | 43 | 60 => {
                if payload.len() < offset + 2 {
                    return Err(Skip::Malformed("ipv6-extension"));
                }
                let length = (usize::from(payload[offset + 1]) + 1) * 8;
                if payload.len() < offset + length {
                    return Err(Skip::Malformed("ipv6-extension"));
                }
                next = payload[offset];
                offset += length;
            }
            51 => {
                if payload.len() < offset + 2 {
                    return Err(Skip::Malformed("ipv6-ah"));
                }
                let length = (usize::from(payload[offset + 1]) + 2) * 4;
                if payload.len() < offset + length {
                    return Err(Skip::Malformed("ipv6-ah"));
                }
                next = payload[offset];
                offset += length;
            }
            44 => {
                if payload.len() < offset + 8 {
                    return Err(Skip::Malformed("ipv6-fragment"));
                }
                let fragment = u16::from_be_bytes([payload[offset + 2], payload[offset + 3]]);
                if fragment & 0xfff8 != 0 {
                    return Err(Skip::Fragment);
                }
                next = payload[offset];
                offset += 8;
            }
            _ => break,
        }
        hops += 1;
        if hops > 8 {
            return Err(Skip::Malformed("ipv6-extension-chain"));
        }
    }
    let end = end.max(offset).min(payload.len());
    transport(next, src, dst, &payload[offset..end])
}

fn transport(proto: u8, src: Addr, dst: Addr, l4: &[u8]) -> Result<Packet, Skip> {
    match proto {
        6 => {
            if l4.len() < 20 {
                return Err(Skip::Malformed("tcp-header"));
            }
            let header_len = usize::from(l4[12] >> 4) * 4;
            if header_len < 20 || l4.len() < header_len {
                return Err(Skip::Malformed("tcp-data-offset"));
            }
            let transport = Transport::Tcp {
                sport: u16::from_be_bytes([l4[0], l4[1]]),
                dport: u16::from_be_bytes([l4[2], l4[3]]),
                flags: l4[13],
            };
            Ok(Packet {
                src,
                dst,
                transport,
                payload_len: l4.len() - header_len,
            })
        }
        17 => {
            if l4.len() < 8 {
                return Err(Skip::Malformed("udp-header"));
            }
            let length = usize::from(u16::from_be_bytes([l4[4], l4[5]])).min(l4.len());
            if length < 8 {
                return Err(Skip::Malformed("udp-length"));
            }
            let transport = Transport::Udp {
                sport: u16::from_be_bytes([l4[0], l4[1]]),
                dport: u16::from_be_bytes([l4[2], l4[3]]),
            };
            Ok(Packet {
                src,
                dst,
                transport,
                payload_len: length - 8,
            })
        }
        _ => Err(Skip::NotTcpUdp),
    }
}
