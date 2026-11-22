//! 从环境变量构造 pgwire 选项：`TRIT_PG_PASSWORD`、`TRIT_PG_TLS_CERT`、`TRIT_PG_TLS_KEY`、
//! `TRIT_PG_ALLOW_PLAINTEXT`（无 TLS 时允许明文密码，仅本地测试用）。

use std::sync::Arc;

use tritdb_pgwire::{AuthMode, ScramVerifier, ServerOptions};

use crate::Error;

/// SCRAM-SHA-256 迭代次数（与 pgwire 内部默认一致）。
const SCRAM_ITERATIONS: u32 = 4096;

pub(super) fn server_options() -> Result<Arc<ServerOptions>, Error> {
    let mut options = ServerOptions::default();
    if let Ok(password) = std::env::var("TRIT_PG_PASSWORD") {
        let verifier = ScramVerifier::from_password(&password, SCRAM_ITERATIONS)
            .map_err(|error| io(&error.to_string()))?;
        options.auth = AuthMode::ScramSha256Verifier(verifier);
        options.allow_plaintext_password = std::env::var("TRIT_PG_ALLOW_PLAINTEXT").is_ok();
    }
    if let (Ok(cert), Ok(key)) = (
        std::env::var("TRIT_PG_TLS_CERT"),
        std::env::var("TRIT_PG_TLS_KEY"),
    ) {
        options.tls_cert = Some(cert.into());
        options.tls_key = Some(key.into());
    }
    Ok(Arc::new(options))
}

fn io(message: &str) -> Error {
    Error::TraceIo {
        path: "pgwire options".into(),
        message: message.to_string(),
    }
}
