use crate::{Receipt, digest};

impl Receipt {
    /// 参与签名的规范化字节：`signature` 自身不参与，其余字段按结构体顺序序列化。
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut copy = self.clone();
        copy.signature.clear();
        serde_json::to_vec(&copy).expect("receipt is always serializable")
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
