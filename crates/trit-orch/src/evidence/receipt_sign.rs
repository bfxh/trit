use crate::{digest, Exit, Receipt, ReceiptProvenance, Ternary};

/// 签名域前缀：版本化，避免未来字段/编码变化后旧签名被重新解释。
const HMAC_DOMAIN: &[u8] = b"trit-orch-receipt-hmac-v1\0";

fn put_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

fn put_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_be_bytes());
}

fn put_i32(buf: &mut Vec<u8>, value: i32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

fn put_bytes(buf: &mut Vec<u8>, value: &[u8]) {
    put_u64(buf, value.len() as u64);
    buf.extend_from_slice(value);
}

fn put_str(buf: &mut Vec<u8>, value: &str) {
    put_bytes(buf, value.as_bytes());
}

fn put_ternary(buf: &mut Vec<u8>, value: Ternary) {
    put_u8(
        buf,
        match value {
            Ternary::False => 0,
            Ternary::Unknown => 1,
            Ternary::True => 2,
        },
    );
}

fn put_opt_ternary(buf: &mut Vec<u8>, value: Option<Ternary>) {
    match value {
        Some(inner) => {
            put_u8(buf, 1);
            put_ternary(buf, inner);
        }
        None => put_u8(buf, 0),
    }
}

impl Receipt {
    /// 参与签名的规范化字节：`signature` 自身不参与，其余字段按固定顺序、
    /// 定长整数 + 长度前缀字符串编码，不依赖 `serde_json` 的字段序或数字格式。
    ///
    /// 这是跨语言的字节契约：任何语言只要按同一顺序与同一长度前缀写字段，
    /// 就能算出同一个 HMAC。域前缀带版本号，编码规则变化时不会把旧签名
    /// 误读成新语义。
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(HMAC_DOMAIN);
        put_u64(&mut buf, self.epoch);
        match self.exit {
            Exit::Completed { code } => {
                put_u8(&mut buf, 0);
                put_i32(&mut buf, code);
            }
            Exit::Signalled => put_u8(&mut buf, 1),
            Exit::TimedOut => put_u8(&mut buf, 2),
            Exit::SpawnFailed => put_u8(&mut buf, 3),
        }
        put_u32(&mut buf, self.assertions_passed);
        put_u32(&mut buf, self.assertions_failed);
        put_u8(
            &mut buf,
            match self.provenance {
                ReceiptProvenance::HostMeasured => 0,
                ReceiptProvenance::SelfReported => 1,
            },
        );
        put_str(&mut buf, &self.nonce);
        put_u64(&mut buf, self.command.len() as u64);
        for arg in &self.command {
            put_str(&mut buf, arg);
        }
        put_opt_ternary(&mut buf, self.authorization);
        put_opt_ternary(&mut buf, self.freshness);
        match self.usage {
            Some(usage) => {
                put_u8(&mut buf, 1);
                put_u64(&mut buf, usage.fresh);
                put_u64(&mut buf, usage.cached);
                put_u64(&mut buf, usage.output);
            }
            None => put_u8(&mut buf, 0),
        }
        match &self.test_digest {
            Some(test_digest) => {
                put_u8(&mut buf, 1);
                put_str(&mut buf, test_digest);
            }
            None => put_u8(&mut buf, 0),
        }
        match &self.plan {
            Some(plan) => {
                put_u8(&mut buf, 1);
                put_u64(&mut buf, plan.revision);
                put_str(&mut buf, &plan.acceptance_digest);
            }
            None => put_u8(&mut buf, 0),
        }
        buf
    }

    /// 用宿主 HMAC-SHA256 签名（小写 hex）。
    pub fn sign(&mut self, key: &[u8]) {
        self.signature = digest::hex(&digest::hmac_sha256(key, &self.signing_bytes()));
    }

    /// 验签；空签名一律无效。签名只证明字段未被改动且签名方持有密钥，
    /// 不证明签名方真的执行了测量。
    pub fn signature_valid(&self, key: &[u8]) -> bool {
        !self.signature.is_empty()
            && self.signature == digest::hex(&digest::hmac_sha256(key, &self.signing_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exit, ReceiptProvenance, TokenUsage};

    fn receipt() -> Receipt {
        Receipt::host_measured(7, Exit::Completed { code: 0 }, 2, 0).with_observation(
            Ternary::True,
            Ternary::True,
            TokenUsage {
                fresh: 10,
                cached: 2,
                output: 5,
            },
        )
    }

    #[test]
    fn signing_bytes_are_stable_and_domain_separated() {
        let receipt = receipt();
        let bytes = receipt.signing_bytes();
        assert!(bytes.starts_with(HMAC_DOMAIN));
        assert_eq!(bytes, receipt.signing_bytes());
    }

    #[test]
    fn signature_does_not_feed_back_into_signing_bytes() {
        let mut receipt = receipt();
        let before = receipt.signing_bytes();
        receipt.sign(b"0123456789abcdef");
        assert_eq!(before, receipt.signing_bytes());
    }

    #[test]
    fn every_signed_field_changes_the_bytes() {
        let base = receipt().signing_bytes();
        let mut nonce = receipt();
        nonce.nonce = "n-1".to_owned();
        assert_ne!(base, nonce.signing_bytes());

        let mut command = receipt();
        command.command = vec!["python".to_owned(), "-c".to_owned()];
        assert_ne!(base, command.signing_bytes());

        let mut provenance = receipt();
        provenance.provenance = ReceiptProvenance::SelfReported;
        assert_ne!(base, provenance.signing_bytes());

        let mut usage = receipt();
        usage.usage = Some(TokenUsage {
            fresh: 10,
            cached: 3,
            output: 5,
        });
        assert_ne!(base, usage.signing_bytes());

        let mut plan = receipt();
        plan.plan = Some(crate::ReceiptPlan {
            revision: 2,
            acceptance_digest: "digest".to_owned(),
        });
        assert_ne!(base, plan.signing_bytes());
    }
}
