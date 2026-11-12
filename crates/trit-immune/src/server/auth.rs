//! HTTP 写端点的 PSK 认证（审计 H1/H5）。
//!
//! 读端点（render/gate/suspects/board/attacks…）公开；**写端点**（改裁决/隔离/合并：
//! `/v1/deltas`、`/v1/diag`、`/v1/lsp`、`/v1/scan`、`/v1/taskmd` 及 fleet 的 record/links/
//! sync/lock）**默认禁用**：须在 `Server`/`FleetServer` 上用 `.with_psk(k)` 配置共享密钥，
//! 且请求带 `Authorization: Bearer <hex(psk)>`。强制自定义头本身也挡掉浏览器跨站——简单
//! 请求带不了 `Authorization`，跨域会触发预检被挡（防 CSRF）。

use crate::auth::ct_eq;

/// 会改状态的写端点（POST）。读端点不在此列，保持公开。
pub(crate) fn is_write(method: &str, path: &str) -> bool {
    method == "POST"
        && matches!(
            path,
            "/v1/taskmd"
                | "/v1/deltas"
                | "/v1/diag"
                | "/v1/scan"
                | "/v1/lsp"
                | "/v1/fleet/links"
                | "/v1/fleet/record"
                | "/v1/fleet/sync"
                | "/v1/fleet/lock"
        )
}

/// 写请求授权：写端点未配 PSK ⇒ 403（默认禁用）；配了 ⇒ 必须带正确 Bearer，否则 401。
pub(crate) fn authorize_write(
    psk: Option<&[u8]>,
    method: &str,
    path: &str,
    headers: &[(String, String)],
) -> Result<(), (u16, String)> {
    if !is_write(method, path) {
        return Ok(());
    }
    let Some(psk) = psk else {
        return Err((403, "写端点未配置 PSK（默认禁用写操作）\n".into()));
    };
    let expected = to_hex(psk);
    let provided = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, v)| v.strip_prefix("Bearer "))
        .map(str::trim);
    match provided {
        Some(tok) if tok.len() == expected.len() && ct_eq(tok.as_bytes(), expected.as_bytes()) => {
            Ok(())
        }
        _ => Err((
            401,
            "认证失败：写端点需要 `Authorization: Bearer <hex(psk)>`\n".into(),
        )),
    }
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_need_no_auth() {
        assert!(authorize_write(None, "GET", "/v1/render", &[]).is_ok());
        // 非写 POST（intake 只分类、不落账）不拦
        assert!(authorize_write(None, "POST", "/v1/intake", &[]).is_ok());
    }

    #[test]
    fn write_without_configured_psk_is_forbidden() {
        assert_eq!(
            authorize_write(None, "POST", "/v1/deltas", &[])
                .unwrap_err()
                .0,
            403
        );
        assert_eq!(
            authorize_write(None, "POST", "/v1/fleet/record", &[])
                .unwrap_err()
                .0,
            403
        );
    }

    #[test]
    fn write_with_right_bearer_ok() {
        let psk = b"secret";
        let h = vec![(
            "authorization".to_string(),
            format!("Bearer {}", to_hex(psk)),
        )];
        assert!(authorize_write(Some(psk), "POST", "/v1/deltas", &h).is_ok());
        // 头名大小写不敏感
        let h2 = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", to_hex(psk)),
        )];
        assert!(authorize_write(Some(psk), "POST", "/v1/lsp", &h2).is_ok());
    }

    #[test]
    fn write_with_wrong_or_missing_bearer_is_unauthorized() {
        let wrong = vec![("Authorization".to_string(), "Bearer deadbeef".to_string())];
        assert_eq!(
            authorize_write(Some(b"secret"), "POST", "/v1/deltas", &wrong)
                .unwrap_err()
                .0,
            401
        );
        assert_eq!(
            authorize_write(Some(b"secret"), "POST", "/v1/deltas", &[])
                .unwrap_err()
                .0,
            401
        );
    }
}
