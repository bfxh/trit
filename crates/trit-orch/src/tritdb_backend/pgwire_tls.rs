//! TLS 选项：`TRIT_PG_TLS_CERT` / `TRIT_PG_TLS_KEY` / `TRIT_PG_TLS_CLIENT_CA` /
//! `TRIT_PG_TLS_CLIENT_REVOKED` / `TRIT_PG_TLS_REQUIRE`。

use tritdb_pgwire::ServerOptions;

pub(super) fn apply(options: &mut ServerOptions) {
    if let (Ok(cert), Ok(key)) = (
        std::env::var("TRIT_PG_TLS_CERT"),
        std::env::var("TRIT_PG_TLS_KEY"),
    ) {
        options.tls_cert = Some(cert.into());
        options.tls_key = Some(key.into());
    }
    if let Ok(ca) = std::env::var("TRIT_PG_TLS_CLIENT_CA") {
        options.tls_client_ca = Some(ca.into());
    }
    if let Ok(revoked) = std::env::var("TRIT_PG_TLS_CLIENT_REVOKED") {
        options.tls_client_revoked = Some(revoked.into());
    }
    if std::env::var("TRIT_PG_TLS_REQUIRE").is_ok() {
        options.require_tls = true;
    }
}
