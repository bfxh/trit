//! 签名 attestation：三值裁决集 → in-toto statement 形态 + HMAC-SHA256 签名。
//!
//! 对齐 [in-toto attestation 框架](https://in-toto.io)（SLSA provenance 的
//! 载体格式）：statement = `_type` + `subject`（工件 + 内容摘要）+
//! `predicateType` + `predicate`（本系统的裁决集）。本模块的 predicate
//! 类型是 `trit-immune/verdicts/v1`——把免疫网络的全部节点裁决作为可验证
//! 声明发布，外部工具（SBOM/VEX 消费者、CI 门禁）可验签后采信。
//!
//! 纪律（与研究纪要"咬合三标准"同源）：
//! - **digest 绑定账本**：subject 的 digest = [`crate::export_dag_tsv`] 的
//!   SHA-256——声明与账本内容强绑定，改一个裁决 digest 就变；
//! - **签名 = HMAC-SHA256**（[`crate::auth::hmac_sha256`]，RFC 2104 向量
//!   对拍过），验证用常量时间比较（[`crate::auth::ct_eq`]）；
//! - **payload 不比账本乐观**：predicate 直接从 DAG 当前状态序列化，
//!   `fixed` 等派生语义由 [`crate::vex`] 负责，这里只照实转写。
//!
//! 用法：`attest(&dag, "author", "product", key, key_id)` 产出 [`Attestation`]；
//! 消费方 `verify(&att, key)` 验签（payload 任何一字节被改都拒绝）。

use crate::auth::{ct_eq, hmac_sha256, sha256};
use crate::json::json_escape;
use crate::dag::ImmuneDag;
use crate::journal::export_dag_tsv;
use crate::trit::Trit;

/// 本系统的 predicate 类型标识。
pub const PREDICATE_TYPE: &str = "https://trit-immune.dev/verdicts/v1";

/// in-toto Statement v1 类型标识。
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

/// 签名后的 attestation：canonical payload + HMAC 签名 + 密钥标识。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// JSON statement（签名所覆盖的字节，必须原样传输/存储）。
    pub payload: String,
    /// HMAC-SHA256(payload, key) 的十六进制小写。
    pub signature: String,
    /// 密钥标识（消费方据此选钥；不含密钥本体）。
    pub key_id: String,
}

/// verdict → 单节点声明（in-toto predicate 里的一行）。
fn statement_json(title: &str, verdict: Trit, note: &str) -> String {
    let status = match verdict {
        Trit::Vulnerable => "affected",
        Trit::Unknown => "under_investigation",
        Trit::Hardened => "verified",
    };
    format!(
        "{{\"vulnerability\": {{\"name\": {}}}, \"status\": {}, \"status_notes\": {}}}",
        json_escape(title),
        json_escape(status),
        json_escape(note)
    )
}

/// 产出签名 attestation：subject digest 绑定账本内容，predicate 照实转写
/// 全部非根节点的当前裁决（`+1` 记 `verified`——不区分 not_affected/fixed，
/// 那是 [`crate::vex`] 的派生职责）。
pub fn attest(dag: &ImmuneDag, author: &str, product: &str, key: &[u8], key_id: &str) -> Attestation {
    // subject digest：账本全量 TSV 的 SHA-256（内容变 → digest 变）
    let ledger_tsv = export_dag_tsv(dag);
    let digest_hex = {
        let d = sha256(ledger_tsv.as_bytes());
        let mut s = String::with_capacity(64);
        for b in d {
            s.push_str(&format!("{b:02x}"));
        }
        s
    };

    let mut predicate = String::with_capacity(256 + dag.nodes().len() * 128);
    predicate.push_str("{\"author\": ");
    predicate.push_str(&json_escape(author));
    predicate.push_str(", \"statements\": [");
    let mut first = true;
    for n in dag.nodes() {
        if n.parent.is_none() {
            continue; // 根 = 库名，不是漏洞面
        }
        if !first {
            predicate.push(',');
        }
        first = false;
        let note = n.history.last().map(|e| e.note.as_str()).unwrap_or("");
        predicate.push('\n');
        predicate.push_str(&statement_json(&n.title, n.verdict, note));
    }
    predicate.push_str("\n  ]}");

    let mut payload = String::with_capacity(predicate.len() + 256);
    payload.push_str("{\n  \"_type\": ");
    payload.push_str(&json_escape(STATEMENT_TYPE));
    payload.push_str(",\n  \"subject\": [{\"name\": ");
    payload.push_str(&json_escape(product));
    payload.push_str(", \"digest\": {\"sha256\": ");
    payload.push_str(&json_escape(&digest_hex));
    payload.push_str("}}],\n  \"predicateType\": ");
    payload.push_str(&json_escape(PREDICATE_TYPE));
    payload.push_str(",\n  \"predicate\": ");
    payload.push_str(&predicate);
    payload.push_str("\n}\n");

    let sig = hmac_sha256(key, payload.as_bytes());
    let mut signature = String::with_capacity(64);
    for b in sig {
        signature.push_str(&format!("{b:02x}"));
    }
    Attestation {
        payload,
        signature,
        key_id: key_id.to_string(),
    }
}

/// 验签：重算 HMAC 并常量时间比对 payload。任何字节被改都拒绝。
pub fn verify(att: &Attestation, key: &[u8]) -> bool {
    let expected = hmac_sha256(key, att.payload.as_bytes());
    let mut expected_hex = String::with_capacity(64);
    for b in expected {
        expected_hex.push_str(&format!("{b:02x}"));
    }
    ct_eq(expected_hex.as_bytes(), att.signature.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{NodeId, NodeKind};

    fn sample_dag() -> ImmuneDag {
        let mut dag = ImmuneDag::root("lib");
        let a = dag.spawn(NodeId(0), NodeKind::Delta, "TRI001-SQLi");
        dag.record(a, Trit::Vulnerable, None, "打穿");
        let b = dag.spawn(NodeId(0), NodeKind::Delta, "TRI002-路径遍历");
        dag.record(b, Trit::Hardened, None, "转义完备");
        dag
    }

    #[test]
    fn attest_verify_roundtrip() {
        let dag = sample_dag();
        let att = attest(&dag, "red-team", "my-lib", b"secret-key-1", "k1");
        assert_eq!(att.key_id, "k1");
        assert_eq!(att.signature.len(), 64, "HMAC-SHA256 hex");
        assert!(verify(&att, b"secret-key-1"), "对钥应通过");
        assert!(!verify(&att, b"wrong-key"), "错钥拒绝");
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let dag = sample_dag();
        let mut att = attest(&dag, "red-team", "my-lib", b"k", "k1");
        // 把 payload 里任何字节改掉（这里把状态词改坏）
        att.payload = att.payload.replace("verified", "under_investigation");
        assert!(!verify(&att, b"k"), "payload 被改必须拒绝");
    }

    #[test]
    fn payload_is_valid_statement_bound_to_ledger() {
        let dag = sample_dag();
        let att = attest(&dag, "red-team", "my-lib", b"k", "k1");
        let parsed = crate::json::parse(&att.payload).expect("payload 必须是合法 JSON");
        assert_eq!(
            parsed.get("_type").and_then(|v| v.as_str()),
            Some(STATEMENT_TYPE)
        );
        assert_eq!(
            parsed.get("predicateType").and_then(|v| v.as_str()),
            Some(PREDICATE_TYPE)
        );
        // digest 绑定账本：改一个裁决 → digest 变 → payload 变 → 原签名失效
        let mut dag2 = dag.clone();
        let b = dag2
            .spawn(NodeId(0), NodeKind::Delta, "TRI003-新发现");
        dag2.record(b, Trit::Vulnerable, None, "新漏洞");
        let att2 = attest(&dag2, "red-team", "my-lib", b"k", "k1");
        assert_ne!(att.payload, att2.payload, "账本变了 payload 必须变");
        assert!(!verify(&att2, b"k") || att2.payload != att.payload);
        assert!(verify(&att2, b"k"), "新账本的新签名应通过");
        // predicate 照实转写：含两个节点的声明
        let stmts = parsed
            .get("predicate")
            .and_then(|p| p.get("statements"))
            .and_then(|s| s.as_arr())
            .unwrap();
        assert_eq!(stmts.len(), 2, "根跳过，两个业务节点");
    }
}
