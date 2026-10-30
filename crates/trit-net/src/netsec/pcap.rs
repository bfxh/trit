//! 经典 pcap 读取（libpcap 老格式）。
//!
//! 只认 24 字节全局头 + 16 字节记录头。pcapng、未知链路层类型一律报错——
//! 静默读错一个字段，后面所有流特征都是假的，而且看起来还很像真的。

use std::fmt;

const GLOBAL_HEADER: usize = 24;
const RECORD_HEADER: usize = 16;
/// 单条记录的上限，挡住损坏头里的天文数字（`incl_len`）。
const MAX_RECORD: u32 = 256 * 1024;

/// 读取失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcapError {
    /// 文件短于 24 字节全局头。
    ShortGlobalHeader(usize),
    /// 首 4 字节不是任何已知 magic。
    BadMagic([u8; 4]),
    /// pcapng 尚未支持（`0a 0d 0d 0a`）。
    PcapNgUnsupported,
    /// 主版本不是 2。
    UnsupportedVersion(u16),
    /// 链路层类型不在支持列表里。
    UnsupportedLinkType(u32),
    /// 记录头或记录体越界。
    TruncatedRecord {
        /// 记录序号（从 0 起）。
        index: usize,
        /// 还需要的字节数。
        need: usize,
        /// 文件剩余字节数。
        remaining: usize,
    },
    /// `incl_len` 超过 256 KiB 上限。
    OversizedRecord {
        /// 记录序号。
        index: usize,
        /// 头里声明的长度。
        incl_len: u32,
    },
}

impl fmt::Display for PcapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortGlobalHeader(len) => write!(f, "pcap 全局头不足 24 字节（实际 {len}）"),
            Self::BadMagic(magic) => write!(f, "未识别的 pcap magic：{magic:02x?}"),
            Self::PcapNgUnsupported => f.write_str("pcapng 尚未支持（M0 只读经典 pcap）"),
            Self::UnsupportedVersion(major) => write!(f, "不支持的 pcap 主版本 {major}（只认 2）"),
            Self::UnsupportedLinkType(code) => {
                write!(f, "不支持的链路层类型 {code}（只认 1=Ethernet、101=RawIP）")
            }
            Self::TruncatedRecord {
                index,
                need,
                remaining,
            } => write!(
                f,
                "第 {index} 条记录被截断：需要 {need} 字节，只剩 {remaining}"
            ),
            Self::OversizedRecord { index, incl_len } => write!(
                f,
                "第 {index} 条记录声明 {incl_len} 字节，超过上限 {MAX_RECORD}"
            ),
        }
    }
}

impl std::error::Error for PcapError {}

/// 多字节字段的字节序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// 小端。
    Little,
    /// 大端。
    Big,
}

/// 支持的数据链路层类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    /// `LINKTYPE_ETHERNET = 1`。
    Ethernet,
    /// `LINKTYPE_RAW = 101`，报文从 IP 头开始。
    RawIp,
}

impl LinkType {
    /// pcap 全局头里的数值。
    pub const fn code(self) -> u32 {
        match self {
            Self::Ethernet => 1,
            Self::RawIp => 101,
        }
    }

    fn from_code(code: u32) -> Result<Self, PcapError> {
        match code {
            1 => Ok(Self::Ethernet),
            101 => Ok(Self::RawIp),
            other => Err(PcapError::UnsupportedLinkType(other)),
        }
    }
}

/// 一条记录（借用文件缓冲）。
#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    /// 秒部分。
    pub ts_sec: u32,
    /// 小数部分：微秒或纳秒，取决于文件头 magic。
    pub ts_frac: u32,
    /// 抓包时的原始长度，可能大于 [`Self::data`] 的长度。
    pub orig_len: u32,
    /// 实际抓到的字节。
    pub data: &'a [u8],
}

impl Record<'_> {
    /// 时间戳（秒，浮点）。`nanos` 取自文件头。
    pub fn timestamp_seconds(&self, nanos: bool) -> f64 {
        let scale = if nanos { 1e-9 } else { 1e-6 };
        f64::from(self.ts_sec) + f64::from(self.ts_frac) * scale
    }
}

/// 逐条记录迭代器。
#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
    index: usize,
    endian: Endian,
    nanos: bool,
    link_type: LinkType,
    snaplen: u32,
}

impl<'a> Reader<'a> {
    /// 解析全局头；失败时不消费任何东西。
    pub fn new(data: &'a [u8]) -> Result<Self, PcapError> {
        if data.len() < GLOBAL_HEADER {
            return Err(PcapError::ShortGlobalHeader(data.len()));
        }
        let magic: [u8; 4] = data[..4].try_into().expect("长度已校验：4 字节 magic");
        let (endian, nanos) = match magic {
            [0xd4, 0xc3, 0xb2, 0xa1] => (Endian::Little, false),
            [0xa1, 0xb2, 0xc3, 0xd4] => (Endian::Big, false),
            [0x4d, 0x3c, 0xb2, 0xa1] => (Endian::Little, true),
            [0xa1, 0xb2, 0x3c, 0x4d] => (Endian::Big, true),
            [0x0a, 0x0d, 0x0d, 0x0a] => return Err(PcapError::PcapNgUnsupported),
            other => return Err(PcapError::BadMagic(other)),
        };
        let major = read_u16(data, 4, endian).expect("长度已校验：版本号");
        if major != 2 {
            return Err(PcapError::UnsupportedVersion(major));
        }
        let snaplen = read_u32(data, 16, endian).expect("长度已校验：snaplen");
        let network = read_u32(data, 20, endian).expect("长度已校验：linktype");
        Ok(Self {
            data,
            offset: GLOBAL_HEADER,
            index: 0,
            endian,
            nanos,
            link_type: LinkType::from_code(network)?,
            snaplen,
        })
    }

    /// 链路层类型。
    pub const fn link_type(&self) -> LinkType {
        self.link_type
    }

    /// 时间戳小数部分是纳秒吗。
    pub const fn nanos(&self) -> bool {
        self.nanos
    }

    /// 文件头声明的抓包上限（0 表示未限制）。
    pub const fn snaplen(&self) -> u32 {
        self.snaplen
    }

    /// 已读记录数。
    pub const fn index(&self) -> usize {
        self.index
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Record<'a>, PcapError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }
        let remaining = self.data.len() - self.offset;
        if remaining < RECORD_HEADER {
            let index = self.index;
            self.offset = self.data.len();
            return Some(Err(PcapError::TruncatedRecord {
                index,
                need: RECORD_HEADER,
                remaining,
            }));
        }
        let index = self.index + 1;
        let ts_sec = read_u32(self.data, self.offset, self.endian).expect("长度已校验：记录头");
        let ts_frac =
            read_u32(self.data, self.offset + 4, self.endian).expect("长度已校验：记录头");
        let incl_len =
            read_u32(self.data, self.offset + 8, self.endian).expect("长度已校验：记录头");
        let orig_len =
            read_u32(self.data, self.offset + 12, self.endian).expect("长度已校验：记录头");
        self.index = index;
        self.offset += RECORD_HEADER;
        if incl_len > MAX_RECORD {
            self.offset = self.data.len();
            return Some(Err(PcapError::OversizedRecord { index, incl_len }));
        }
        let need = incl_len as usize;
        let available = self.data.len() - self.offset;
        if need > available {
            self.offset = self.data.len();
            return Some(Err(PcapError::TruncatedRecord {
                index,
                need,
                remaining: available,
            }));
        }
        let body = &self.data[self.offset..self.offset + need];
        self.offset += need;
        Some(Ok(Record {
            ts_sec,
            ts_frac,
            orig_len,
            data: body,
        }))
    }
}

fn read_u16(data: &[u8], at: usize, endian: Endian) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
    Some(match endian {
        Endian::Little => u16::from_le_bytes(bytes),
        Endian::Big => u16::from_be_bytes(bytes),
    })
}

fn read_u32(data: &[u8], at: usize, endian: Endian) -> Option<u32> {
    let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
    Some(match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    })
}
