//! 门的指纹与错误恢复小件：内容指纹、证据格式、错误后是否恢复。

use trit_core::fingerprint::sha256_hex;

use crate::schema::Trajectory;

/// 内容指纹：任务 id + 每步 tool/args/observation（规范化后）的 SHA-256。
pub fn content_key(t: &Trajectory) -> String {
    let mut s = String::from(&t.task_id);
    for step in &t.steps {
        s.push('\u{1}');
        s.push_str(step.tool.as_deref().unwrap_or(""));
        s.push('\u{1}');
        s.push_str(step.args.as_deref().unwrap_or(""));
        s.push('\u{1}');
        s.push_str(step.observation.as_deref().unwrap_or(""));
    }
    sha256_hex(s.as_bytes())
}

/// 证据必须是 `sha256:<64 hex>`。
pub fn is_sha256(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 最后一次 `error=true` 之后没有任何成功步 ⇒ 没恢复。
pub fn has_unrecovered_error(t: &Trajectory) -> bool {
    let Some(i) = t.steps.iter().rposition(|s| s.error) else {
        return false;
    };
    !t.steps[i + 1..]
        .iter()
        .any(|s| !s.error && !s.observation.as_deref().unwrap_or("").trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_learn::json::Json;

    fn td(evidence: &str) -> Trajectory {
        let json = format!(
            r#"{{"task_id":"k1","model":"m","steps":[{{"role":"assistant","thought":"run","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"a","error":true}},{{"role":"assistant","thought":"retry","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"b"}}],"outcome":{{"verifier":"manual","passed":true,"evidence":["{evidence}"]}}}}"#
        );
        Trajectory::from_json(&Json::parse(&json).unwrap()).unwrap()
    }

    #[test]
    fn evidence_format_is_exact() {
        assert!(is_sha256(&format!("sha256:{}", "a".repeat(64))));
        assert!(!is_sha256("md5:abc"));
        assert!(!is_sha256(&format!("sha256:{}", "g".repeat(64))));
        assert!(!is_sha256(&format!("sha256:{}", "a".repeat(63))));
    }

    #[test]
    fn recovered_error_is_not_flagged_and_key_is_stable() {
        let t = td(&format!("sha256:{}", "a".repeat(64)));
        assert!(!has_unrecovered_error(&t));
        assert_eq!(content_key(&t), content_key(&t));
        assert_eq!(content_key(&t).len(), 64);

        let mut bad = t.clone();
        bad.steps[1].error = true;
        assert!(has_unrecovered_error(&bad));
    }

    #[test]
    fn key_is_order_sensitive_within_steps() {
        let mut a = td(&format!("sha256:{}", "a".repeat(64)));
        let b = a.clone();
        assert_eq!(content_key(&a), content_key(&b));
        a.steps.reverse();
        assert_ne!(content_key(&a), content_key(&b));
    }
}
