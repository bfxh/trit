//! TLS 传输层（`tls` feature flag 启用，默认关闭保持零依赖）。
//!
//! 生产部署推荐：**反向代理（nginx/caddy）终结 TLS**，内核只看到明文 TCP + HMAC。
//! 本模块供**不经代理的直连场景**使用（内网点对点同步）。
//!
//! 证书：生产用 CA 签发证书；测试用 `generate_self_signed` 自签。
#![cfg(feature = "tls")]

/// TLS 配置：证书路径 + 私钥路径。
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// 生成自签名证书（测试/开发用；生产用 CA 签发）。
pub fn generate_self_signed(hostname: &str) -> Result<(String, String), String> {
    let cert = rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .map_err(|e| format!("证书生成失败：{e}"))?;
    Ok((cert.cert.pem(), cert.key_pair.serialize_pem()))
}

/// 创建 rustls 服务端配置。
pub fn server_config(cert_pem: &str, key_pem: &str) -> Result<rustls::ServerConfig, String> {
    let mut certs = Vec::new();
    for chunk in rustls_pemfile::certs(&mut cert_pem.as_bytes()) {
        certs.push(chunk.map_err(|e| format!("证书解析失败：{e}"))?);
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| format!("私钥解析失败：{e}"))?
        .ok_or("私钥为空")?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("服务端 TLS 配置失败：{e}"))?;
    Ok(config)
}

/// 创建 rustls 客户端配置（信任自签证书或跳过校验——仅测试用）。
pub fn client_config_skip_verify() -> Result<rustls::ClientConfig, String> {
    use std::sync::Arc;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();
    Ok(config)
}

#[derive(Debug)]
struct SkipVerify;

impl rustls::client::danger::ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}
