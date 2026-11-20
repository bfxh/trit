//! 审计链的带密钥封印。
//!
//! 背景（第七轮 Y1–Y3）：`verify_chain` 的期望值由条目自身算出，无密钥 FNV 只能检测损坏，
//! 挡不住「改完重算」。这里不动线格式，给**链头**加 HMAC-SHA256 封印：篡改并重算后
//! 链头必然变化，只要封印来自可信渠道（提交、公告、CI 产物），伪造就过不了校验。
//!
//! SHA-256 / HMAC 实现与 `trit-immune/src/auth.rs` 同源（RFC 4231 向量对拍过）；
//! 自带一份是为了避免 `trit-wspace` 反向依赖 `trit-immune`。

use crate::audit::AuditLog;
use crate::error::{Error, Result};

/// 链头封印：`count` + 链头 + `mac = HMAC-SHA256(key, "trit-audit-v1|count|head")`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedHead {
    /// 封印时的条目数。
    pub count: usize,
    /// 链头（`chain` 的 16 位十六进制；空日志为 `0`）。
    pub head: String,
    /// 64 位十六进制 MAC。
    pub mac: String,
}

const SCHEME: &str = "trit-audit-v1";

/// 计算链头的封印。
#[must_use]
pub fn seal_head(log: &AuditLog, key: &[u8]) -> SealedHead {
    let count = log.entries().len();
    let head = format!("{:016x}", log.head());
    let mac = hmac_sha256(key, format!("{SCHEME}|{count}|{head}").as_bytes());
    SealedHead {
        count,
        head,
        mac: hex32(&mac),
    }
}

/// 校验封印与当前日志是否一致；`key` 不对、条目数或链头变化都会报错。
pub fn verify_sealed(log: &AuditLog, sealed: &SealedHead, key: &[u8]) -> Result<()> {
    let count = log.entries().len();
    let head = format!("{:016x}", log.head());
    if sealed.count != count || sealed.head != head {
        return Err(Error::Audit(format!(
            "审计链头与封印不符（封印 {} 条 / {}，当前 {count} 条 / {head}）",
            sealed.count, sealed.head
        )));
    }
    let want = hmac_sha256(key, format!("{SCHEME}|{count}|{head}").as_bytes());
    let Ok(got) = hex_to_32(&sealed.mac) else {
        return Err(Error::Audit("封印的 mac 不是 64 位十六进制".into()));
    };
    if !ct_eq(&got, &want) {
        return Err(Error::Audit("封印验证失败：密钥不对或链头被改写".into()));
    }
    Ok(())
}

fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_32(text: &str) -> std::result::Result<[u8; 32], ()> {
    if text.len() != 64 {
        return Err(());
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| ())?;
    }
    Ok(out)
}

/// 常量时间比较。
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in [a, b, c, d, e, f, g, hh].into_iter().enumerate() {
            h[slot] = h[slot].wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer = Vec::with_capacity(96);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&ih);
    sha256(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditLog;
    use crate::edit::Patch;
    use crate::net::Net;

    fn log_with(entries: usize) -> AuditLog {
        let mut log = AuditLog::new();
        let net = Net::zeros(&[2, 2]).expect("zeros");
        for i in 0..entries {
            log.record(
                format!("edit-{i}"),
                &net,
                &net,
                Patch::new(Vec::new()),
                true,
                false,
            )
            .expect("record");
        }
        log
    }

    #[test]
    fn seal_round_trips_and_rejects_rekey() {
        let log = log_with(2);
        let sealed = seal_head(&log, b"secret");
        verify_sealed(&log, &sealed, b"secret").expect("通过");
        assert!(verify_sealed(&log, &sealed, b"other").is_err());
    }

    #[test]
    fn tampering_with_recompute_breaks_the_seal() {
        let log = log_with(2);
        let sealed = seal_head(&log, b"secret");
        let text = log.to_jsonl().replace("edit-0", "innocuous");
        let mut forged = AuditLog::from_jsonl(&text).expect("parse forged");
        forged.recompute_chain();
        // 链自洽（无密钥 FNV 挡不住重算），但链头已变，封印过不了
        forged.verify_chain().expect("重算后链自洽");
        assert!(verify_sealed(&forged, &sealed, b"secret").is_err());
    }

    #[test]
    fn rfc4231_vector() {
        // RFC 4231 Test Case 2
        let tag = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex32(&tag),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
