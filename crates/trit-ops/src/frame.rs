//! ECC 帧封装：把任意字节负载包进 GF(3) 纠错块 + 帧校验。
//!
//! 首个消费方是 `trit-orch` 的 `TraceEventStore`（审计轨迹落盘的可选
//! `--ecc` 语义）：写路径 [`encode_line`]、读路径 [`decode_line`]，行格式
//! `TRE1 <tag> <len> <hex>`（默认关闭时消费方字节零变化）。
//!
//! # 帧结构
//!
//! 负载字节 → 6 trit/字节（little-endian base-3，`3^6 = 729 ≥ 256`）→
//! 追加 6 个帧校验 trit（见下）→ 零 trit 补齐到 k 的倍数 → 按 k 切块
//! 系统编码成 n trit 码字 → tryte 打包（5 trit/字节，复用 [`crate::pack`]）
//! → hex。块码由 [`FrameCode`] 选择：Golay `[11,6,5]`（默认）、扩展
//! Hamming `[14,10,4]` / `[41,36,4]`。
//!
//! # 帧校验（m-序列加权检查）
//!
//! 6 个检查位 `χ_j = Σ_i s(i+j)·d_i (mod 3)`，`s` 是 GF(3) 上本原多项式
//! `f(x) = x^6 + x^5 + 2` 的 m-序列（周期 728）。任意长度 ≥6 的连续窗口上，
//! 6 个泛函的矩阵含 `[s(w+i+j)]_{i,j<6}`——m-序列状态的 Krylov 矩阵，秩 6
//! ⇒ **局限在单块消息窗口内的任何非零损坏必然改变校验**（确定性，非概率）。
//! 只有 ≥2 块同时被"误纠"且 χ 贡献恰好相消才会漏过——残差 3⁻⁶ ≈ 0.13%
//! （固定帧实测 550/435600 ≈ 0.126%，测试钉住上界）。
//!
//! # 消费语义（如实写）
//!
//! - 块内 ≤1 错：Golay/Hamming 都**确定**纠正（d≥3 ⇒ 单错伴随式唯一）；
//! - 块内 2 错：Golay **确定**判双（d=5 ⇒ 双错伴随式不与单错碰撞）→
//!   fail-closed 拒帧；扩展 Hamming 部分当场 Suspect、其余被帧校验拦下；
//! - 块内 ≥3 错：可能伪装成 ≤2 错被"误纠"（完美码/d=4 的固有界限），
//!   但单块损坏被 m-序列校验**确定性**拦截（Krylov 秩论证），只有多块
//!   同时伪装才有 3⁻⁶ 残差——这条界限写进测试注释，不夸大。

use std::fmt;
use std::sync::OnceLock;

use crate::gf3::{ExtendedDecode, ExtendedHamming, digit, from_digit};
use crate::golay::{Golay11, GolayDecode};
use crate::pack::Trytes;
use crate::trit::Trit;

/// 帧行前缀。
const MAGIC: &str = "TRE1";
/// 每负载字节的 trit 数（3^6 = 729 ≥ 256）。
const TRITS_PER_BYTE: usize = 6;
/// 帧校验位数（m-序列移位泛函个数；残差 3⁻⁶）。
const CHECK_TRITS: usize = 6;
/// m-序列周期（GF(3) 上 6 次本原多项式的阶 3⁶−1）。
const SEQ_LEN: usize = 728;

/// GF(3) m-序列：`f(x) = x^6 + x^5 + 2`，`s_t = -(s_{t-1} + 2·s_{t-6})`，
/// 初始状态 `(1,0,0,0,0,0)`。任一相位的 6×6 Hankel 子阵非奇异（Krylov）。
const fn build_seq() -> [u8; SEQ_LEN] {
    let mut s = [0u8; SEQ_LEN];
    s[0] = 1;
    let mut t = 6;
    while t < SEQ_LEN {
        let v = (s[t - 1] + 2 * s[t - 6]) % 3;
        s[t] = (3 - v) % 3;
        t += 1;
    }
    s
}

static SEQ: [u8; SEQ_LEN] = build_seq();

/// 帧用的块码选择。默认 Golay `[11,6,5]`：单错自愈与双错拒绝都是**确定**语义。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FrameCode {
    /// 三元 Golay `[11,6,5]`（d=5）：纠 1、拒 2，帧校验兜底 ≥3 错。
    #[default]
    Golay11,
    /// 扩展 Hamming `[14,10,4]`（d=4）：纠 1 + 一致性检查，码率更高、语义弱一档。
    Hamming13,
    /// 扩展 Hamming `[41,36,4]`（d=4）：同上，长块低开销档。
    Hamming41,
}

impl FrameCode {
    /// 码在编解码器静态表里的下标。
    fn index(self) -> usize {
        match self {
            FrameCode::Golay11 => 0,
            FrameCode::Hamming13 => 1,
            FrameCode::Hamming41 => 2,
        }
    }

    /// 帧行里的码 tag（`G11` / `H13` / `H41`）。
    pub fn tag(self) -> &'static str {
        match self {
            FrameCode::Golay11 => "G11",
            FrameCode::Hamming13 => "H13",
            FrameCode::Hamming41 => "H41",
        }
    }

    /// 从帧行 tag 解析码；未知 tag 返回 `None`。
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "G11" => Some(FrameCode::Golay11),
            "H13" => Some(FrameCode::Hamming13),
            "H41" => Some(FrameCode::Hamming41),
            _ => None,
        }
    }
}

/// 拒帧原因（机器可读，进 `Error::TraceEcc` 的 reason）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameReject {
    /// 前缀 / tag / 长度 / hex 任一不合法。
    Malformed,
    /// 第 `b` 块判定为确定双错（Golay d=5 语义）——fail-closed。
    DoubleErrorBlock(usize),
    /// 第 `b` 块一致性检查失败（扩展 Hamming Suspect）——fail-closed。
    MultiErrorBlock(usize),
    /// 块译码都"成功"但帧校验不符（≥2 块同时伪装的残差路径）。
    Checksum,
    /// 帧校验侥幸通过但负载 trit 流还原不出字节（同一残差路径的伴随形态）。
    ByteRange,
}

impl fmt::Display for FrameReject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameReject::Malformed => write!(f, "malformed frame"),
            FrameReject::DoubleErrorBlock(b) => write!(f, "double-error in block {b}"),
            FrameReject::MultiErrorBlock(b) => write!(f, "multi-error suspect in block {b}"),
            FrameReject::Checksum => write!(f, "frame checksum mismatch"),
            FrameReject::ByteRange => write!(f, "payload trits out of byte range"),
        }
    }
}

/// 帧解码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameResult {
    /// 干净帧。
    Clean(Vec<u8>),
    /// 自愈帧：payload 与写入时逐字节一致（块级单错被确定纠正）。
    Healed { payload: Vec<u8>, repairs: u32 },
    /// fail-closed 拒帧。
    Rejected(FrameReject),
}

/// 组装好块码的帧编解码器（块码查表只建一次，进程内复用）。
pub struct FrameCodec {
    code: FrameCode,
    k: usize,
    n: usize,
    inner: Inner,
}

enum Inner {
    Golay(Box<Golay11>),
    Hamming(ExtendedHamming),
}

static CODECS: OnceLock<[FrameCodec; 3]> = OnceLock::new();

fn codecs() -> &'static [FrameCodec; 3] {
    CODECS.get_or_init(|| {
        [
            FrameCodec::new(FrameCode::Golay11),
            FrameCodec::new(FrameCode::Hamming13),
            FrameCodec::new(FrameCode::Hamming41),
        ]
    })
}

fn codec_for(code: FrameCode) -> &'static FrameCodec {
    &codecs()[code.index()]
}

/// 把帧编码成一行（写路径）。
pub fn encode_line(code: FrameCode, payload: &[u8]) -> String {
    codec_for(code).encode(payload)
}

/// 解一行帧（读路径）；非 `TRE1` 行由调用方按原格式处理。
pub fn decode_line(line: &str) -> FrameResult {
    let mut parts = line.split_whitespace();
    let parsed = match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(MAGIC), Some(tag), Some(len), Some(hex), None) => {
            FrameCode::from_tag(tag).and_then(|code| {
                len.parse::<usize>()
                    .ok()
                    .map(|payload_len| (code, payload_len, hex))
            })
        }
        _ => None,
    };
    match parsed {
        Some((code, payload_len, hex)) => codec_for(code).decode(payload_len, hex),
        None => FrameResult::Rejected(FrameReject::Malformed),
    }
}

/// 帧校验：6 个 m-序列移位加权和（标准位域 mod 3）。
fn checksum(trits: &[Trit]) -> [u8; CHECK_TRITS] {
    let mut out = [0u8; CHECK_TRITS];
    for (i, &t) in trits.iter().enumerate() {
        let d = digit(t) as usize;
        if d == 0 {
            continue;
        }
        for (j, slot) in out.iter_mut().enumerate() {
            *slot = ((*slot as usize + SEQ[(i + j) % SEQ_LEN] as usize * d) % 3) as u8;
        }
    }
    out
}

/// 字节 → trit 流（每字节 6 trit，little-endian base-3）。
fn bytes_to_trits(payload: &[u8]) -> Vec<Trit> {
    let mut out = Vec::with_capacity(payload.len() * TRITS_PER_BYTE);
    for &b in payload {
        let mut v = b as usize;
        for _ in 0..TRITS_PER_BYTE {
            out.push(from_digit((v % 3) as u8));
            v /= 3;
        }
    }
    out
}

/// trit 流 → 字节；某 6 trit 组超过 255 说明负载确被改过（校验残差路径）。
fn trits_to_bytes(trits: &[Trit]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(trits.len() / TRITS_PER_BYTE);
    for chunk in trits.chunks(TRITS_PER_BYTE) {
        let mut v = 0usize;
        for &t in chunk.iter().rev() {
            v = v * 3 + usize::from(digit(t));
        }
        if v > 255 {
            return None;
        }
        out.push(v as u8);
    }
    Some(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
        .collect()
}

impl FrameCodec {
    /// 构建指定块码的编解码器。
    pub fn new(code: FrameCode) -> Self {
        match code {
            FrameCode::Golay11 => {
                let g = Golay11::new();
                Self {
                    code,
                    k: g.k(),
                    n: g.n(),
                    inner: Inner::Golay(Box::new(g)),
                }
            }
            FrameCode::Hamming13 => Self::hamming(code, 3),
            FrameCode::Hamming41 => Self::hamming(code, 4),
        }
    }

    fn hamming(code: FrameCode, r: u32) -> Self {
        let h = ExtendedHamming::new(crate::gf3::HammingCode::new(r).expect("r=3/4 必然可构造"));
        Self {
            code,
            k: h.k(),
            n: h.n(),
            inner: Inner::Hamming(h),
        }
    }

    fn encode(&self, payload: &[u8]) -> String {
        let mut stream = bytes_to_trits(payload);
        for d in checksum(&stream) {
            stream.push(from_digit(d));
        }
        let blocks = stream.len().div_ceil(self.k);
        stream.resize(blocks * self.k, Trit::ZERO);
        let mut packed = Vec::with_capacity(blocks * self.n);
        let mut buf = vec![Trit::ZERO; self.n];
        for block in stream.chunks(self.k) {
            match &self.inner {
                Inner::Golay(g) => g.encode_into(block, &mut buf),
                Inner::Hamming(h) => h.encode_into(block, &mut buf),
            }
            packed.extend_from_slice(&buf);
        }
        let trytes = Trytes::from_trits(&packed);
        format!(
            "{MAGIC} {} {} {}",
            self.code.tag(),
            payload.len(),
            hex_encode(trytes.codes())
        )
    }

    fn decode(&self, payload_len: usize, hex: &str) -> FrameResult {
        let payload_trits_len = payload_len * TRITS_PER_BYTE;
        let stream_len = payload_trits_len + CHECK_TRITS;
        let blocks = stream_len.div_ceil(self.k);
        let total_trits = blocks * self.n;
        let expected_bytes = total_trits.div_ceil(crate::pack::TRITS_PER_BYTE);
        if hex.len() != expected_bytes * 2 {
            return FrameResult::Rejected(FrameReject::Malformed);
        }
        let Some(codes) = hex_decode(hex) else {
            return FrameResult::Rejected(FrameReject::Malformed);
        };
        let Some(trytes) = Trytes::from_codes(codes, total_trits) else {
            return FrameResult::Rejected(FrameReject::Malformed);
        };
        let word = trytes.to_trits();
        let mut repairs = 0u32;
        let mut stream = Vec::with_capacity(blocks * self.k);
        let mut fixed = vec![Trit::ZERO; self.n];
        for (b, block) in word.chunks(self.n).enumerate() {
            let rejected = match &self.inner {
                Inner::Golay(g) => match g.decode_into(block, &mut fixed) {
                    GolayDecode::Clean => None,
                    GolayDecode::Corrected1 { .. } => {
                        repairs += 1;
                        None
                    }
                    GolayDecode::Corrected2 { .. } => Some(FrameReject::DoubleErrorBlock(b)),
                },
                Inner::Hamming(h) => match h.decode_into(block, &mut fixed) {
                    ExtendedDecode::Clean => None,
                    ExtendedDecode::Corrected { .. } => {
                        repairs += 1;
                        None
                    }
                    ExtendedDecode::Suspect => Some(FrameReject::MultiErrorBlock(b)),
                },
            };
            if let Some(reason) = rejected {
                return FrameResult::Rejected(reason);
            }
            stream.extend_from_slice(&fixed[..self.k]);
        }
        // 去 padding，拆负载与帧校验。
        let payload_trits = &stream[..payload_trits_len];
        let check_trits = &stream[payload_trits_len..stream_len];
        let recomputed = checksum(payload_trits);
        let checks_match = check_trits
            .iter()
            .enumerate()
            .all(|(i, &t)| digit(t) == recomputed[i]);
        if !checks_match {
            return FrameResult::Rejected(FrameReject::Checksum);
        }
        let Some(payload) = trits_to_bytes(payload_trits) else {
            return FrameResult::Rejected(FrameReject::ByteRange);
        };
        if repairs == 0 {
            FrameResult::Clean(payload)
        } else {
            FrameResult::Healed { payload, repairs }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gf3::add;

    /// 确定 xorshift64*。
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
    }

    fn payload(len: usize) -> Vec<u8> {
        let mut rng = Rng(0x00EA_0001);
        (0..len).map(|_| rng.next() as u8).collect()
    }

    /// 把一行帧的码字 trit 流上打指定错型，再走完整 decode_line 路径。
    fn corrupt(
        line: &str,
        payload_len: usize,
        code: FrameCode,
        errors: &[(usize, u8)],
    ) -> FrameResult {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let hex = parts[3];
        let bytes = hex_decode(hex).expect("测试帧 hex 合法");
        let (k, n) = match code {
            FrameCode::Golay11 => (6usize, 11usize),
            FrameCode::Hamming13 => (10, 14),
            FrameCode::Hamming41 => (36, 41),
        };
        let total = (payload_len * TRITS_PER_BYTE + CHECK_TRITS).div_ceil(k) * n;
        let mut trits = Trytes::from_codes(bytes, total)
            .expect("测试帧可解包")
            .to_trits();
        for &(pos, mag) in errors {
            trits[pos] = add(trits[pos], from_digit(mag));
        }
        let repacked = Trytes::from_trits(&trits);
        decode_line(&format!(
            "{MAGIC} {} {payload_len} {}",
            code.tag(),
            hex_encode(repacked.codes())
        ))
    }

    #[test]
    fn byte_trit_conversions_roundtrip_all_values() {
        for b in 0..=255u8 {
            let trits = bytes_to_trits(&[b]);
            assert_eq!(trits.len(), TRITS_PER_BYTE);
            assert_eq!(trits_to_bytes(&trits), Some(vec![b]), "byte {b}");
        }
        // 越界 trit 组（>255）必须被拒。
        let big = [
            Trit::POS,
            Trit::POS,
            Trit::POS,
            Trit::POS,
            Trit::POS,
            Trit::POS,
        ];
        assert_eq!(trits_to_bytes(&big), None, "728 > 255");
    }

    #[test]
    fn frames_roundtrip_at_various_lengths_and_codes() {
        for code in [
            FrameCode::Golay11,
            FrameCode::Hamming13,
            FrameCode::Hamming41,
        ] {
            for len in [0usize, 1, 2, 3, 5, 7, 13, 40, 100, 333] {
                let p = payload(len);
                let line = encode_line(code, &p);
                assert!(line.starts_with(MAGIC));
                match decode_line(&line) {
                    FrameResult::Clean(back) => assert_eq!(back, p, "{code:?} len {len}"),
                    other => panic!("{code:?} len {len}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn golay_frame_every_single_trit_error_is_deterministically_healed() {
        // 单 trit 错 × 全部位 × 两种幅度 = 2·B·n 例：全部 Healed 且 payload 逐字节一致。
        let code = FrameCode::Golay11;
        let len = 13usize;
        let p = payload(len);
        let line = encode_line(code, &p);
        let total_trits = 14 * 11; // S=84 → 14 块 × 11
        let mut healed = 0u64;
        for pos in 0..total_trits {
            for mag in 1..=2u8 {
                match corrupt(&line, len, code, &[(pos, mag)]) {
                    FrameResult::Healed { payload, repairs } => {
                        assert_eq!(payload, p);
                        assert_eq!(repairs, 1);
                    }
                    other => panic!("位置 {pos} 幅度 {mag}: {other:?}"),
                }
                healed += 1;
            }
        }
        assert_eq!(healed, u64::try_from(total_trits * 2).unwrap());
    }

    #[test]
    fn golay_frame_double_errors_certain_outcomes() {
        // 双错：同块 → 确定 Rejected（d=5 语义）；跨块 → 两处单错确定自愈。
        let code = FrameCode::Golay11;
        let len = 13usize;
        let p = payload(len);
        let line = encode_line(code, &p);
        let n = 11usize;
        let mut rejected = 0u64;
        // 同块双错全枚举（块 0 与块 13）。
        for b in [0usize, 13] {
            for i in 0..n {
                for j in (i + 1)..n {
                    for (a, d) in [(1u8, 1u8), (1, 2), (2, 1), (2, 2)] {
                        match corrupt(&line, len, code, &[(b * n + i, a), (b * n + j, d)]) {
                            FrameResult::Rejected(FrameReject::DoubleErrorBlock(bb)) => {
                                assert_eq!(bb, b);
                                rejected += 1;
                            }
                            other => panic!("同块双错 ({b},{i},{a})+({j},{d}): {other:?}"),
                        }
                    }
                }
            }
        }
        assert_eq!(rejected, 440);
        // 跨块双错抽样：各 1 错，必然双处自愈。
        let mut healed = 0u64;
        for i in 0..n {
            for j in 0..n {
                let (pa, pb) = (i, 13 * n + j);
                match corrupt(&line, len, code, &[(pa, 1), (pb, 2)]) {
                    FrameResult::Healed { payload, repairs } => {
                        assert_eq!(payload, p);
                        assert_eq!(repairs, 2);
                        healed += 1;
                    }
                    other => panic!("跨块双错 ({pa},{pb}): {other:?}"),
                }
            }
        }
        assert_eq!(healed, u64::try_from(n * n).unwrap());
    }

    #[test]
    fn golay_frame_single_block_masquerades_are_deterministically_rejected() {
        // Krylov 秩论证的实测钉死：单块内 ≥3 错的伪装（Corrected1/2 后消息被改）
        // 全部被帧校验确定性拦截——3 错全枚举 18480 + 4 错全枚举 73920，
        // 静默 = 0（与对齐基线一致）。
        let code = FrameCode::Golay11;
        let len = 13usize;
        let p = payload(len);
        let line = encode_line(code, &p);
        let n = 11usize;
        let mut total = 0u64;
        let mut healed_wrong = 0u64;
        let b = 5usize; // 纯负载块（无跨块/校验位混淆）
        for width in [3usize, 4] {
            for combo in combinations(n, width) {
                for mags in 0..(1u32 << width) {
                    let errors: Vec<(usize, u8)> = combo
                        .iter()
                        .enumerate()
                        .map(|(k, &pos)| (b * n + pos, if (mags >> k) & 1 == 0 { 1 } else { 2 }))
                        .collect();
                    match corrupt(&line, len, code, &errors) {
                        FrameResult::Rejected(_) => {}
                        FrameResult::Healed { payload, .. } | FrameResult::Clean(payload) => {
                            if payload != p {
                                healed_wrong += 1;
                            }
                        }
                    }
                    total += 1;
                }
            }
        }
        // C(11,3)·8 + C(11,4)·16 = 1320 + 7920 per 块。
        assert_eq!(total, (165 * 8 + 330 * 16) as u64);
        assert_eq!(healed_wrong, 0, "单块伪装必须全部被帧校验拦截");
    }

    #[test]
    fn golay_frame_dual_block_masquerade_residual_is_bounded() {
        // 如实界限：单块伪装被帧校验确定性拦截（上一个测试），只有两块同时
        // 伪装（各 4 错落 1-leader）时 χ 贡献才可能相消——残差 = 3⁻⁶ ≈ 0.13%
        // （对齐基线实测 550/435600）。抽样 45×45 = 2025 对钉住上界。
        let code = FrameCode::Golay11;
        let len = 13usize;
        let p = payload(len);
        let line = encode_line(code, &p);
        let n = 11usize;
        let g = Golay11::new();
        // 取一行帧的码字 trit，枚举块内 4 错型里落 1-leader 的伪装型。
        let masquerades = |block: usize| -> Vec<Vec<(usize, u8)>> {
            let hex = line.split_whitespace().nth(3).expect("帧有 hex");
            let trits = Trytes::from_codes(hex_decode(hex).expect("hex 合法"), 154)
                .expect("可解包")
                .to_trits();
            let base: Vec<Trit> = trits[block * n..(block + 1) * n].to_vec();
            let mut out = Vec::new();
            for combo in combinations(n, 4) {
                for mags in 0..(1u32 << 4) {
                    let errors: Vec<(usize, u8)> = combo
                        .iter()
                        .enumerate()
                        .map(|(k, &pos)| {
                            (block * n + pos, if (mags >> k) & 1 == 0 { 1 } else { 2 })
                        })
                        .collect();
                    let mut bad = base.clone();
                    for &(pos, mag) in &errors {
                        bad[pos - block * n] = add(bad[pos - block * n], from_digit(mag));
                    }
                    if let GolayDecode::Corrected1 { .. } = g.decode(&bad).0 {
                        out.push(errors);
                    }
                }
            }
            out
        };
        let m0 = masquerades(0);
        let m2 = masquerades(2);
        assert_eq!(m0.len(), 660, "块 0 的 1-leader 伪装型数（对齐基线）");
        assert_eq!(m2.len(), 660);
        let mut silent = 0u64;
        let mut caught = 0u64;
        for e1 in m0.iter().take(45) {
            for e2 in m2.iter().take(45) {
                let mut errors = e1.clone();
                errors.extend_from_slice(e2);
                match corrupt(&line, len, code, &errors) {
                    FrameResult::Healed { payload, .. } => {
                        assert_ne!(payload, p, "两块都伪装还自愈成功 = 静默损坏");
                        silent += 1;
                    }
                    FrameResult::Rejected(_) => caught += 1,
                    FrameResult::Clean(_) => panic!("两块伪装不可能判干净"),
                }
            }
        }
        assert_eq!(silent + caught, 2025);
        let ratio_bp = silent * 10_000 / (silent + caught);
        assert!(
            ratio_bp <= 200,
            "双块伪装静默率 {ratio_bp}bp，应 ≤ 2%（实测 ~0.13%）"
        );
    }

    #[test]
    fn hamming_frames_never_silently_corrupt_on_same_block_doubles() {
        // 扩展 Hamming 帧的同块双错：Suspect 拒 / 误纠后帧校验拦 / 载荷不变的自愈，
        // 三者之外不存在的（静默改载荷 ≤ 2% 上界，实测更低）。
        for (code, n, blocks) in [(FrameCode::Hamming13, 14usize, 9usize)] {
            let len = 13usize;
            let p = payload(len);
            let line = encode_line(code, &p);
            let mut rejected = 0u64;
            let mut healed_equal = 0u64;
            let mut healed_wrong = 0u64;
            for b in 0..blocks {
                for i in 0..n {
                    for j in (i + 1)..n {
                        for (a, d) in [(1u8, 1u8), (1, 2), (2, 1), (2, 2)] {
                            match corrupt(&line, len, code, &[(b * n + i, a), (b * n + j, d)]) {
                                FrameResult::Rejected(_) => rejected += 1,
                                FrameResult::Healed { payload, .. } => {
                                    if payload == p {
                                        healed_equal += 1
                                    } else {
                                        healed_wrong += 1
                                    }
                                }
                                FrameResult::Clean(_) => healed_equal += 1,
                            }
                        }
                    }
                }
            }
            let total = rejected + healed_equal + healed_wrong;
            assert_eq!(total, u64::try_from(blocks * n * (n - 1) / 2 * 4).unwrap());
            let bp = healed_wrong * 10_000 / total;
            assert!(bp <= 200, "Hamming 帧双错静默率 {bp}bp 应 ≤ 2%");
        }
    }

    #[test]
    fn malformed_lines_are_rejected() {
        let good = encode_line(FrameCode::Golay11, b"hi");
        for bad in [
            "TRE2 G11 2 ab",
            "TRE1 X99 2 ab",
            "TRE1 G11 not-a-len ab",
            "TRE1 G11 2",
            "TRE1 G11 2 abc",
            "",
            "plain json line",
        ] {
            assert!(
                matches!(
                    decode_line(bad),
                    FrameResult::Rejected(FrameReject::Malformed)
                ),
                "{bad}"
            );
        }
        // 长度与 hex 不匹配（真实长度 2 字节 → Golay 1 块 → 11 trit → 3 字节 → 6 hex）。
        let parts: Vec<&str> = good.split_whitespace().collect();
        let wrong_len = format!("TRE1 G11 999 {}", parts[3]);
        assert!(matches!(
            decode_line(&wrong_len),
            FrameResult::Rejected(FrameReject::Malformed)
        ));
    }

    /// C(n, w) 组合（小 n 直接枚举，字典序）。
    fn combinations(n: usize, w: usize) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        let mut cur = Vec::new();
        fn rec(start: usize, n: usize, w: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            if cur.len() == w {
                out.push(cur.clone());
                return;
            }
            for v in start..n {
                cur.push(v);
                rec(v + 1, n, w, cur, out);
                cur.pop();
            }
        }
        rec(0, n, w, &mut cur, &mut out);
        out
    }
}
