//! `server::auth` 的鉴权单测（从 auth.rs 拆出，控制行数预算）。

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
    // 第四轮：coord-declare / coord-sync 之前漏在白名单外，必须同样默认禁用
    assert_eq!(
        authorize_write(None, "POST", "/v1/fleet/coord-declare", &[])
            .unwrap_err()
            .0,
        403
    );
    assert_eq!(
        authorize_write(None, "POST", "/v1/fleet/coord-sync", &[])
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
