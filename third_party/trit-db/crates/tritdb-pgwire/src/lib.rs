#![forbid(unsafe_code)]
//! Minimal PostgreSQL wire protocol (v3) server for trit-db.
//!
//! Supported: startup handshake, `SSLRequest` rejection, simple `Query`
//! messages, `Terminate`, and `Sync`. Extended query protocol
//! (`Parse`/`Bind`/`Execute`) is not implemented; clients must use the simple
//! query protocol (most `psql`/driver defaults do).
//!
//! Connections are handled on their own threads and share one [`SqlEngine`]
//! behind a mutex, so statements from different clients serialize on the
//! engine.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use md5::{Digest as _, Md5};

use tritdb_exec::{DataType, Table, Value};
use tritdb_sql::{parse_script, QueryResult, SqlEngine, SqlError, Statement};
use x509_cert::ext::pkix::CrlReason;

const PROTOCOL_V3: i32 = 196_608;
/// Default limit on concurrently served connections (`0` disables the limit).
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;
/// Default milliseconds a connection waits for the shared engine lock.
pub const DEFAULT_ENGINE_LOCK_TIMEOUT_MS: u64 = 30_000;
/// Upper bound on the SQL text produced by `Bind` parameter substitution.
const MAX_SUBSTITUTED_SQL: usize = 16 * 1024 * 1024;
/// Upper bound on prepared statements kept per connection.
const MAX_PREPARED_STATEMENTS: usize = 1024;
/// Upper bound on SQL bytes held by prepared statements per connection.
const MAX_PREPARED_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on bound portals kept per connection.
const MAX_PORTALS: usize = 1024;
/// Upper bound on rows one `COPY` may insert.
const MAX_COPY_ROWS: usize = 1 << 20;
/// Upper bound on one batched `INSERT` string assembled by `COPY ... FROM
/// STDIN`. Without it, 1024 rows of up to 16 MiB each could build a
/// multi-gigabyte statement before it is executed.
const MAX_COPY_BATCH_BYTES: usize = 8 * 1024 * 1024;
/// SCRAM-SHA-256 iteration count used for new challenges.
const SCRAM_ITERATIONS: u32 = 4_096;
const SSL_REQUEST: i32 = 80_877_103;

/// Server errors.
#[derive(Debug)]
pub enum PgError {
    /// Network or file I/O error.
    Io(std::io::Error),
    /// Malformed client message.
    Protocol(String),
    /// SQL engine failed to open.
    Sql(String),
}

impl fmt::Display for PgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgError::Io(error) => write!(formatter, "pgwire io error: {error}"),
            PgError::Protocol(reason) => write!(formatter, "pgwire protocol error: {reason}"),
            PgError::Sql(reason) => write!(formatter, "pgwire sql error: {reason}"),
        }
    }
}

impl std::error::Error for PgError {}

impl From<std::io::Error> for PgError {
    fn from(error: std::io::Error) -> Self {
        PgError::Io(error)
    }
}

/// Byte stream the protocol runs on: a plain `TcpStream` or a TLS stream.
trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// A persisted SCRAM-SHA-256 verifier, in PostgreSQL's on-disk form:
/// `SCRAM-SHA-256$<iterations>:<base64 salt>$<base64 stored key>:<base64 server key>`.
///
/// Holding only the verifier means the server never needs the cleartext
/// password at runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScramVerifier {
    /// PBKDF2 iteration count.
    pub iterations: u32,
    /// PBKDF2 salt.
    pub salt: Vec<u8>,
    /// `SHA256(HMAC(SaltedPassword, "Client Key"))`.
    pub stored_key: [u8; 32],
    /// `HMAC(SaltedPassword, "Server Key")`.
    pub server_key: [u8; 32],
}

impl ScramVerifier {
    /// Derives a verifier from a cleartext password with a fresh random salt.
    pub fn from_password(password: &str, iterations: u32) -> Result<Self, PgError> {
        if iterations == 0 {
            return Err(PgError::Protocol(
                "SCRAM iteration count must be greater than 0".to_string(),
            ));
        }
        let salt = random_bytes::<16>()?;
        let salted = pbkdf2_sha256(password.as_bytes(), &salt, iterations);
        let client_key = tritdb_cas::hmac_sha256(&salted, b"Client Key");
        Ok(Self {
            iterations,
            salt: salt.to_vec(),
            stored_key: tritdb_cas::sha256(&client_key),
            server_key: tritdb_cas::hmac_sha256(&salted, b"Server Key"),
        })
    }

    /// Serializes the verifier in PostgreSQL's `SCRAM-SHA-256$...` form.
    pub fn encode(&self) -> String {
        format!(
            "SCRAM-SHA-256${}:{}${}:{}",
            self.iterations,
            BASE64.encode(&self.salt),
            BASE64.encode(self.stored_key),
            BASE64.encode(self.server_key)
        )
    }

    /// Parses a `SCRAM-SHA-256$...` verifier (one trailing newline is allowed).
    pub fn parse(text: &str) -> Result<Self, PgError> {
        let rest = text.trim().strip_prefix("SCRAM-SHA-256$").ok_or_else(|| {
            PgError::Protocol("verifier must start with SCRAM-SHA-256$".to_string())
        })?;
        let (params, keys) = rest
            .split_once('$')
            .ok_or_else(|| PgError::Protocol("verifier is missing its key section".to_string()))?;
        let (iterations, salt) = params
            .split_once(':')
            .ok_or_else(|| PgError::Protocol("verifier is missing iterations:salt".to_string()))?;
        let (stored, server) = keys.split_once(':').ok_or_else(|| {
            PgError::Protocol("verifier is missing stored:server keys".to_string())
        })?;
        let iterations: u32 = iterations
            .parse()
            .map_err(|_| PgError::Protocol("bad verifier iteration count".to_string()))?;
        if iterations == 0 {
            return Err(PgError::Protocol(
                "verifier iteration count must be greater than 0".to_string(),
            ));
        }
        let decode_key = |value: &str, label: &str| -> Result<[u8; 32], PgError> {
            let bytes = BASE64
                .decode(value)
                .map_err(|error| PgError::Protocol(format!("bad {label}: {error}")))?;
            bytes
                .try_into()
                .map_err(|_| PgError::Protocol(format!("{label} must be 32 bytes")))
        };
        Ok(Self {
            iterations,
            salt: BASE64
                .decode(salt)
                .map_err(|error| PgError::Protocol(format!("bad verifier salt: {error}")))?,
            stored_key: decode_key(stored, "verifier stored key")?,
            server_key: decode_key(server, "verifier server key")?,
        })
    }
}

/// How the server authenticates clients.
#[derive(Clone, Debug, Default)]
pub enum AuthMode {
    /// Accept any client without a password (development default).
    #[default]
    Trust,
    /// Ask for a cleartext password and compare it with this value.
    Password(String),
    /// `AuthenticationMD5Password`: `md5(md5(password + user) + salt)`.
    Md5(String),
    /// SASL `SCRAM-SHA-256` derived from a cleartext password at connect time.
    ScramSha256(String),
    /// SASL `SCRAM-SHA-256` backed by a persisted verifier (no cleartext
    /// password needed at runtime).
    ScramSha256Verifier(ScramVerifier),
}

/// Server-side options. `Default` is trust auth with plain text connections,
/// matching the original behaviour.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// Authentication mode.
    pub auth: AuthMode,
    /// PEM certificate chain; requires `tls_key` to enable TLS.
    pub tls_cert: Option<PathBuf>,
    /// PEM private key; requires `tls_cert` to enable TLS.
    pub tls_key: Option<PathBuf>,
    /// PEM CA bundle used to verify client certificates (mTLS).
    pub tls_client_ca: Option<PathBuf>,
    /// File of revoked client-certificate SHA-256 fingerprints, one hex digest
    /// per line (`#` starts a comment), or a PEM/DER X.509 CRL.
    pub tls_client_revoked: Option<PathBuf>,
    /// DER-encoded OCSP response (pre-fetched) used to revoke client
    /// certificates.
    pub tls_client_ocsp: Option<PathBuf>,
    /// OCSP responder URL (`http://` or `https://`) used to check client
    /// certificates online. Requires `tls_client_ca`.
    pub tls_client_ocsp_url: Option<String>,
    /// Extra PEM certificate trusted for HTTPS OCSP responders (e.g. a
    /// private CA). System roots are always used as well.
    pub tls_client_ocsp_ca: Option<PathBuf>,
    /// Allow client certificates that an OCSP responder put on
    /// `certificateHold` (only that reason).
    pub tls_client_ocsp_allow_hold: bool,
    /// Reject clients that do not request TLS.
    pub require_tls: bool,
    /// Permit cleartext password authentication over a plaintext connection.
    pub allow_plaintext_password: bool,
    /// Maximum number of connections served at once; `0` means unlimited.
    pub max_connections: usize,
    /// How long a connection waits for the shared engine lock before it
    /// reports a timeout; `None` waits forever.
    pub engine_lock_timeout: Option<Duration>,
    /// Wall-clock budget for one query's row scan; `None` means unlimited.
    pub query_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            auth: AuthMode::default(),
            tls_cert: None,
            tls_key: None,
            tls_client_ca: None,
            tls_client_revoked: None,
            tls_client_ocsp: None,
            tls_client_ocsp_url: None,
            tls_client_ocsp_ca: None,
            tls_client_ocsp_allow_hold: false,
            require_tls: false,
            allow_plaintext_password: false,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            engine_lock_timeout: Some(Duration::from_millis(DEFAULT_ENGINE_LOCK_TIMEOUT_MS)),
            query_timeout: None,
        }
    }
}

impl ServerOptions {
    /// True when both halves of a TLS key pair are configured.
    pub fn tls_enabled(&self) -> bool {
        self.tls_cert.is_some() && self.tls_key.is_some()
    }
}

/// What the client sent before the startup packet was accepted.
enum Startup {
    /// The client disconnected cleanly.
    Closed,
    /// A plain protocol-v3 startup packet (with its `user` parameter).
    Plain { user: Option<String> },
    /// The client asked to negotiate TLS first.
    SslRequest,
}

/// Extracts the `user` parameter from a startup packet body.
fn parse_startup_user(body: &[u8]) -> Option<String> {
    let mut fields = body[4..].split(|byte| *byte == 0);
    while let Some(key) = fields.next() {
        let value = fields.next()?;
        if key.is_empty() {
            return None;
        }
        if key == b"user" {
            return Some(String::from_utf8_lossy(value).to_string());
        }
    }
    None
}

/// Compares two byte strings without short-circuiting on the first mismatch.
///
/// The length difference is folded into the accumulator and the loop runs
/// over the shorter input, so a wrong-length guess takes the same path as a
/// wrong value instead of returning early.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    for (a, b) in left.iter().zip(right) {
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

fn random_bytes<const N: usize>() -> Result<[u8; N], PgError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|error| PgError::Protocol(format!("random bytes: {error}")))?;
    Ok(bytes)
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("write hex");
    }
    out
}

fn md5_hex(bytes: &[u8]) -> String {
    hex_lower(&Md5::digest(bytes))
}

/// PBKDF2-HMAC-SHA256 with a 32-byte output (a single block).
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut block = Vec::with_capacity(salt.len() + 4);
    block.extend_from_slice(salt);
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = tritdb_cas::hmac_sha256(password, &block);
    let mut acc = u;
    for _ in 1..iterations {
        u = tritdb_cas::hmac_sha256(password, &u);
        for (left, right) in acc.iter_mut().zip(u) {
            *left ^= right;
        }
    }
    acc
}

fn read_password_message(stream: &mut dyn ReadWrite) -> Result<Vec<u8>, PgError> {
    let Some((kind, payload)) = read_message(stream)? else {
        return Err(PgError::Protocol(
            "client closed the connection during authentication".to_string(),
        ));
    };
    if kind != b'p' {
        write_error(stream, "expected a PasswordMessage")?;
        return Err(PgError::Protocol(format!(
            "expected PasswordMessage, got {}",
            kind as char
        )));
    }
    Ok(payload)
}

fn auth_failed(stream: &mut dyn ReadWrite) -> Result<(), PgError> {
    write_error(stream, "password authentication failed")?;
    Err(PgError::Protocol(
        "password authentication failed".to_string(),
    ))
}

fn authenticate_md5(
    stream: &mut dyn ReadWrite,
    password: &str,
    user: Option<&str>,
) -> Result<(), PgError> {
    let Some(user) = user else {
        write_error(stream, "MD5 authentication requires a username")?;
        return Err(PgError::Protocol("missing startup username".to_string()));
    };
    let salt = random_bytes::<4>()?;
    let mut request = Vec::with_capacity(8);
    request.extend_from_slice(&5i32.to_be_bytes());
    request.extend_from_slice(&salt);
    write_message(stream, b'R', &request)?;

    let payload = read_password_message(stream)?;
    let supplied = String::from_utf8_lossy(&payload)
        .trim_end_matches('\0')
        .to_string();
    let inner = md5_hex(format!("{password}{user}").as_bytes());
    let mut outer_input = inner.into_bytes();
    outer_input.extend_from_slice(&salt);
    let expected = md5_hex(&outer_input);
    if constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        write_message(stream, b'R', &0i32.to_be_bytes())
    } else {
        auth_failed(stream)
    }
}

fn authenticate_scram(stream: &mut dyn ReadWrite, verifier: &ScramVerifier) -> Result<(), PgError> {
    // AuthenticationSASL (10) advertising SCRAM-SHA-256.
    let mut payload = Vec::new();
    payload.extend_from_slice(&10i32.to_be_bytes());
    payload.extend_from_slice(b"SCRAM-SHA-256\0\0");
    write_message(stream, b'R', &payload)?;

    let initial = read_password_message(stream)?;
    let (mechanism, cursor) = read_cstring(&initial, 0)?;
    if mechanism != "SCRAM-SHA-256" {
        write_error(stream, &format!("unsupported SASL mechanism {mechanism}"))?;
        return Err(PgError::Protocol(format!(
            "unsupported SASL mechanism {mechanism}"
        )));
    }
    let length = read_i32(&initial, cursor)?;
    if length < 0 {
        write_error(stream, "bad SASL initial response")?;
        return Err(PgError::Protocol("bad SASL initial response".to_string()));
    }
    let start = cursor + 4;
    let end = start + length as usize;
    let client_first = initial
        .get(start..end)
        .ok_or_else(|| PgError::Protocol("truncated SASL initial response".to_string()))?;
    let client_first = String::from_utf8_lossy(client_first).to_string();

    // Skip the GS2 header ("n,," / "y,,"); the rest is the bare message.
    let bare_start = client_first
        .find(",,")
        .map(|index| index + 2)
        .ok_or_else(|| PgError::Protocol("malformed SCRAM client-first-message".to_string()))?;
    let client_first_bare = client_first[bare_start..].to_string();
    let client_nonce = client_first_bare
        .split(',')
        .find_map(|part| part.strip_prefix("r="))
        .ok_or_else(|| PgError::Protocol("missing SCRAM client nonce".to_string()))?
        .to_string();

    let nonce = format!("{client_nonce}{}", hex_lower(&random_bytes::<18>()?));
    let server_first = format!(
        "r={nonce},s={},i={}",
        BASE64.encode(&verifier.salt),
        verifier.iterations
    );
    let mut payload = Vec::new();
    payload.extend_from_slice(&11i32.to_be_bytes());
    payload.extend_from_slice(server_first.as_bytes());
    write_message(stream, b'R', &payload)?;

    let final_message = read_password_message(stream)?;
    let client_final = String::from_utf8_lossy(&final_message)
        .trim_end_matches('\0')
        .to_string();
    let Some(proof_at) = client_final.rfind(",p=") else {
        write_error(stream, "malformed SCRAM client-final-message")?;
        return Err(PgError::Protocol(
            "malformed SCRAM client-final-message".to_string(),
        ));
    };
    let client_final_without_proof = client_final[..proof_at].to_string();
    let proof = BASE64
        .decode(&client_final[proof_at + 3..])
        .map_err(|error| PgError::Protocol(format!("bad SCRAM proof: {error}")))?;
    if proof.len() != 32 {
        write_error(stream, "bad SCRAM proof length")?;
        return Err(PgError::Protocol("bad SCRAM proof length".to_string()));
    }

    let auth_message = format!("{client_first_bare},{server_first},{client_final_without_proof}");
    let client_signature = tritdb_cas::hmac_sha256(&verifier.stored_key, auth_message.as_bytes());
    let mut recovered = [0u8; 32];
    for (index, value) in recovered.iter_mut().enumerate() {
        *value = proof[index] ^ client_signature[index];
    }
    if !constant_time_eq(&tritdb_cas::sha256(&recovered), &verifier.stored_key) {
        return auth_failed(stream);
    }
    let server_signature = tritdb_cas::hmac_sha256(&verifier.server_key, auth_message.as_bytes());
    let mut payload = Vec::new();
    payload.extend_from_slice(&12i32.to_be_bytes());
    payload.extend_from_slice(format!("v={}", BASE64.encode(server_signature)).as_bytes());
    write_message(stream, b'R', &payload)?;
    write_message(stream, b'R', &0i32.to_be_bytes())
}

/// Runs the authentication exchange configured on the server.
fn authenticate(
    stream: &mut dyn ReadWrite,
    auth: &AuthMode,
    user: Option<&str>,
) -> Result<(), PgError> {
    match auth {
        AuthMode::Trust => write_message(stream, b'R', &0i32.to_be_bytes()),
        AuthMode::Password(expected) => {
            write_message(stream, b'R', &3i32.to_be_bytes())?;
            let payload = read_password_message(stream)?;
            let supplied = String::from_utf8_lossy(&payload)
                .trim_end_matches('\0')
                .to_string();
            if constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
                write_message(stream, b'R', &0i32.to_be_bytes())
            } else {
                auth_failed(stream)
            }
        }
        AuthMode::Md5(password) => authenticate_md5(stream, password, user),
        AuthMode::ScramSha256(password) => {
            let verifier = ScramVerifier::from_password(password, SCRAM_ITERATIONS)?;
            authenticate_scram(stream, &verifier)
        }
        AuthMode::ScramSha256Verifier(verifier) => authenticate_scram(stream, verifier),
    }
}

/// Wraps a client-certificate verifier and rejects certificates whose SHA-256
/// fingerprint appears in a revocation list.
#[derive(Debug)]
struct RevocationVerifier {
    inner: Arc<dyn rustls::server::danger::ClientCertVerifier>,
    fingerprints: Vec<[u8; 32]>,
    serials: Vec<Vec<u8>>,
    online: Option<Arc<OcspOnline>>,
    allow_hold: bool,
}

impl rustls::server::danger::ClientCertVerifier for RevocationVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        let fingerprint = tritdb_cas::sha256(end_entity.as_ref());
        let revoked = self
            .fingerprints
            .iter()
            .any(|revoked| constant_time_eq(revoked, &fingerprint))
            || (!self.serials.is_empty()
                && x509_parser::parse_x509_certificate(end_entity.as_ref())
                    .map(|(_, certificate)| {
                        let serial = certificate.raw_serial();
                        self.serials
                            .iter()
                            .any(|revoked| revoked.as_slice() == serial)
                    })
                    .unwrap_or(false));
        if revoked {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Revoked,
            ));
        }
        if let Some(online) = &self.online {
            match online.check_status(end_entity.as_ref()) {
                Ok(OcspStatus::Revoked(reason)) => {
                    if self.allow_hold && matches!(reason, Some(CrlReason::CertificateHold)) {
                        eprintln!("pgwire: client certificate is on hold (allowed by policy)");
                    } else {
                        eprintln!("pgwire: client certificate revoked (reason: {reason:?})");
                        return Err(rustls::Error::InvalidCertificate(
                            rustls::CertificateError::Revoked,
                        ));
                    }
                }
                Ok(OcspStatus::Good) => {}
                Err(error) => {
                    eprintln!("pgwire: OCSP check failed: {error}");
                    return Err(rustls::Error::General(format!(
                        "OCSP check failed: {error}"
                    )));
                }
            }
        }
        self.inner
            .verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Result of an OCSP lookup for one certificate.
#[derive(Clone, Copy, Debug)]
enum OcspStatus {
    /// The responder says the certificate is valid.
    Good,
    /// The responder revoked it; the reason may be absent.
    Revoked(Option<CrlReason>),
}

/// Online OCSP checker backed by a preconfigured responder URL.
#[derive(Debug)]
struct OcspOnline {
    url: String,
    ca_der: Vec<u8>,
    extra_root: Option<Vec<u8>>,
    cache: Mutex<HashMap<Vec<u8>, (OcspStatus, Instant)>>,
}

impl OcspOnline {
    fn new(url: String, ca_der: Vec<u8>, extra_root: Option<Vec<u8>>) -> Self {
        Self {
            url,
            ca_der,
            extra_root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the revocation reason when the responder revokes this
    /// certificate. Unknown or failed checks are treated as revoked.
    fn check_status(&self, cert_der: &[u8]) -> Result<OcspStatus, PgError> {
        let (_, certificate) = x509_parser::parse_x509_certificate(cert_der)
            .map_err(|error| PgError::Protocol(format!("OCSP: bad client certificate: {error}")))?;
        let serial = certificate.raw_serial().to_vec();
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((status, at)) = cache.get(&serial) {
                if at.elapsed() < Duration::from_secs(60) {
                    return Ok(*status);
                }
            }
        }
        let (request, nonce) = self.request_der(&serial)?;
        let response = http_post(&self.url, &request, self.extra_root.as_deref())?;
        let status = ocsp_revoked(&response, Some(&self.ca_der), &serial, Some(&nonce))?;
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Bound the cache: a CA with many clients must not grow it forever.
        if cache.len() >= 4096 {
            let ttl = Duration::from_secs(60);
            cache.retain(|_, (_, at)| at.elapsed() < ttl);
            if cache.len() >= 4096 {
                cache.clear();
            }
        }
        cache.insert(serial, (status, Instant::now()));
        Ok(status)
    }

    fn request_der(&self, serial: &[u8]) -> Result<(Vec<u8>, Vec<u8>), PgError> {
        use der::{Decode, Encode};
        use sha1::{Digest, Sha1};
        let ca = x509_cert::Certificate::from_der(&self.ca_der)
            .map_err(|error| PgError::Protocol(format!("OCSP: bad CA certificate: {error}")))?;
        let name_hash = Sha1::digest(
            ca.tbs_certificate
                .subject
                .to_der()
                .map_err(|error| PgError::Protocol(format!("OCSP: CA subject: {error}")))?,
        );
        let key_hash = Sha1::digest(
            ca.tbs_certificate
                .subject_public_key_info
                .to_der()
                .map_err(|error| PgError::Protocol(format!("OCSP: CA key: {error}")))?,
        );
        let cert_id = x509_ocsp::CertId {
            hash_algorithm: spki::AlgorithmIdentifierOwned {
                oid: der::asn1::ObjectIdentifier::new_unwrap("1.3.14.3.2.26"),
                parameters: Some(der::Any::null()),
            },
            issuer_name_hash: der::asn1::OctetString::new(name_hash.to_vec())
                .map_err(|error| PgError::Protocol(format!("OCSP: name hash: {error}")))?,
            issuer_key_hash: der::asn1::OctetString::new(key_hash.to_vec())
                .map_err(|error| PgError::Protocol(format!("OCSP: key hash: {error}")))?,
            serial_number: x509_cert::serial_number::SerialNumber::new(serial)
                .map_err(|error| PgError::Protocol(format!("OCSP: serial: {error}")))?,
        };
        let nonce_bytes = random_bytes::<16>()?;
        let nonce = x509_ocsp::ext::Nonce::new(nonce_bytes.to_vec())
            .map_err(|error| PgError::Protocol(format!("OCSP nonce: {error}")))?;
        let nonce_extension = x509_cert::ext::Extension {
            extn_id: <x509_ocsp::ext::Nonce as const_oid::AssociatedOid>::OID,
            critical: false,
            extn_value: der::asn1::OctetString::new(
                nonce
                    .to_der()
                    .map_err(|error| PgError::Protocol(format!("OCSP nonce DER: {error}")))?,
            )
            .map_err(|error| PgError::Protocol(format!("OCSP nonce extension: {error}")))?,
        };
        let request = x509_ocsp::OcspRequest {
            tbs_request: x509_ocsp::TbsRequest {
                version: Default::default(),
                requestor_name: None,
                request_list: vec![x509_ocsp::Request {
                    req_cert: cert_id,
                    single_request_extensions: None,
                }],
                request_extensions: Some(vec![nonce_extension]),
            },
            optional_signature: None,
        };
        Ok((
            request
                .to_der()
                .map_err(|error| PgError::Protocol(format!("OCSP request: {error}")))?,
            nonce_bytes.to_vec(),
        ))
    }
}

/// Minimal HTTP/1.1 POST for `http://` OCSP responders.
fn http_post(url: &str, body: &[u8], extra_root: Option<&[u8]>) -> Result<Vec<u8>, PgError> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        PgError::Protocol("OCSP responder URL needs an http:// or https:// scheme".to_string())
    })?;
    let (host_port, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| PgError::Protocol(format!("bad OCSP responder port {port}")))?,
        ),
        None => (host_port, if scheme == "https" { 443 } else { 80 }),
    };
    // A plain `connect` can hang for the OS SYN-retry budget; a hostile or
    // unreachable responder must not block the TLS handshake that long.
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|error| PgError::Protocol(format!("OCSP responder address: {error}")))?
        .next()
        .ok_or_else(|| {
            PgError::Protocol("OCSP responder address resolved to nothing".to_string())
        })?;
    let tcp = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/ocsp-request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    match scheme {
        "http" => post_stream(tcp, &request),
        "https" => {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if let Some(der) = extra_root {
                roots
                    .add(der.to_vec().into())
                    .map_err(|error| PgError::Protocol(format!("OCSP extra root: {error}")))?;
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|error| PgError::Protocol(format!("OCSP TLS versions: {error}")))?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|error| PgError::Protocol(format!("OCSP TLS name: {error}")))?;
            let connection = rustls::ClientConnection::new(Arc::new(config), server_name)
                .map_err(|error| PgError::Protocol(format!("OCSP TLS client: {error}")))?;
            let stream = rustls::StreamOwned::new(connection, tcp);
            post_stream(stream, &request)
        }
        other => Err(PgError::Protocol(format!(
            "unsupported OCSP responder scheme {other}"
        ))),
    }
}

fn post_stream<S: Read + Write>(mut stream: S, request: &[u8]) -> Result<Vec<u8>, PgError> {
    stream.write_all(request)?;
    let mut response = Vec::new();
    // OCSP responses are tiny; cap the read so a hostile responder cannot
    // exhaust memory.
    const MAX_OCSP_RESPONSE: u64 = 1024 * 1024;
    let mut limited = (&mut stream).take(MAX_OCSP_RESPONSE);
    // Responders may close the connection without a TLS `close_notify`; keep
    // whatever was already received.
    if let Err(error) = limited.read_to_end(&mut response) {
        if response.is_empty() {
            return Err(PgError::Io(error));
        }
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| PgError::Protocol("OCSP: malformed HTTP response".to_string()))?;
    let head = String::from_utf8_lossy(&response[..header_end]);
    let status = head.lines().next().unwrap_or("");
    if !status.contains(" 200 ") {
        return Err(PgError::Protocol(format!(
            "OCSP responder returned {status}"
        )));
    }
    Ok(response[header_end + 4..].to_vec())
}

/// Returns whether `serial` is revoked according to an OCSP response.
fn ocsp_revoked(
    bytes: &[u8],
    ca_der: Option<&[u8]>,
    serial: &[u8],
    expected_nonce: Option<&[u8]>,
) -> Result<OcspStatus, PgError> {
    use der::Decode;
    let response = x509_ocsp::OcspResponse::from_der(bytes)
        .map_err(|error| PgError::Protocol(format!("bad OCSP response: {error}")))?;
    if !matches!(
        response.response_status,
        x509_ocsp::OcspResponseStatus::Successful
    ) {
        return Err(PgError::Protocol(
            "OCSP responder returned a non-success status".to_string(),
        ));
    }
    let Some(response_bytes) = response.response_bytes else {
        return Err(PgError::Protocol(
            "OCSP response has no responseBytes".to_string(),
        ));
    };
    let basic = x509_ocsp::BasicOcspResponse::from_der(response_bytes.response.as_bytes())
        .map_err(|error| PgError::Protocol(format!("bad OCSP basic response: {error}")))?;
    verify_ocsp_signature(&basic, ca_der)?;
    if let Some(expected) = expected_nonce {
        match basic.nonce() {
            Some(nonce) if nonce.0.as_bytes() == expected => {}
            Some(_) => {
                return Err(PgError::Protocol("OCSP nonce mismatch".to_string()));
            }
            None => {
                return Err(PgError::Protocol(
                    "OCSP response is missing the nonce".to_string(),
                ));
            }
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    for single in &basic.tbs_response_data.responses {
        if single.cert_id.serial_number.as_bytes() != serial {
            continue;
        }
        let updated = single.this_update.0.to_unix_duration();
        if updated.as_secs() > now + 300 {
            return Err(PgError::Protocol(
                "OCSP thisUpdate is in the future".to_string(),
            ));
        }
        if let Some(next) = &single.next_update {
            let expiry = next.0.to_unix_duration();
            if expiry.as_secs() < now {
                return Err(PgError::Protocol("OCSP response has expired".to_string()));
            }
        }
        return Ok(match &single.cert_status {
            x509_ocsp::CertStatus::Revoked(info) => OcspStatus::Revoked(info.revocation_reason),
            _ => OcspStatus::Good,
        });
    }
    Err(PgError::Protocol(
        "OCSP response does not mention the client certificate".to_string(),
    ))
}

/// Parses a pre-fetched OCSP response, verifies its signature, and returns
/// the serial numbers the responder marked as revoked.
fn load_ocsp(path: &Path, ca_der: Option<&[u8]>) -> Result<Vec<Vec<u8>>, PgError> {
    use der::Decode;
    let bytes = std::fs::read(path)?;
    let response = x509_ocsp::OcspResponse::from_der(&bytes)
        .map_err(|error| PgError::Protocol(format!("bad OCSP response: {error}")))?;
    if !matches!(
        response.response_status,
        x509_ocsp::OcspResponseStatus::Successful
    ) {
        return Err(PgError::Protocol(format!(
            "OCSP response status is {:?}",
            response.response_status
        )));
    }
    let Some(response_bytes) = response.response_bytes else {
        return Err(PgError::Protocol(
            "OCSP response has no responseBytes".to_string(),
        ));
    };
    let basic = x509_ocsp::BasicOcspResponse::from_der(response_bytes.response.as_bytes())
        .map_err(|error| PgError::Protocol(format!("bad OCSP basic response: {error}")))?;
    verify_ocsp_signature(&basic, ca_der)?;
    let mut revoked = Vec::new();
    for single in &basic.tbs_response_data.responses {
        if matches!(single.cert_status, x509_ocsp::CertStatus::Revoked(_)) {
            revoked.push(single.cert_id.serial_number.as_bytes().to_vec());
        }
    }
    Ok(revoked)
}

fn verify_ocsp_signature(
    basic: &x509_ocsp::BasicOcspResponse,
    ca_der: Option<&[u8]>,
) -> Result<(), PgError> {
    use der::Encode;
    let signer_der = if let Some(certs) = &basic.certs {
        let certificate = certs.first().ok_or_else(|| {
            PgError::Protocol("OCSP response has no signer certificate".to_string())
        })?;
        certificate
            .to_der()
            .map_err(|error| PgError::Protocol(format!("OCSP signer encode: {error}")))?
    } else if let Some(ca) = ca_der {
        ca.to_vec()
    } else {
        return Err(PgError::Protocol(
            "OCSP response has no signer certificate and --tls-client-ca is missing".to_string(),
        ));
    };
    let (_, signer) = x509_parser::parse_x509_certificate(&signer_der)
        .map_err(|error| PgError::Protocol(format!("bad OCSP signer: {error}")))?;
    if basic.certs.is_some() {
        let eku = signer
            .extended_key_usage()
            .map_err(|error| PgError::Protocol(format!("OCSP signer EKU: {error}")))?
            .map(|extension| extension.value);
        let Some(eku) = eku else {
            return Err(PgError::Protocol(
                "OCSP signer certificate has no extended key usage".to_string(),
            ));
        };
        if !eku.any && !eku.ocsp_signing {
            return Err(PgError::Protocol(
                "OCSP signer certificate is not valid for OCSP signing (missing id-kp-OCSPSigning)"
                    .to_string(),
            ));
        }
    }
    let now = x509_parser::time::ASN1Time::now();
    if !signer.validity().is_valid_at(now) {
        return Err(PgError::Protocol(
            "OCSP signer certificate is not valid now".to_string(),
        ));
    }
    if let Some(ca) = ca_der {
        let (_, ca_cert) = x509_parser::parse_x509_certificate(ca)
            .map_err(|error| PgError::Protocol(format!("bad client CA: {error}")))?;
        if !ca_cert.validity().is_valid_at(now) {
            return Err(PgError::Protocol(
                "client CA certificate is not valid now".to_string(),
            ));
        }
        if signer_der != ca {
            signer
                .verify_signature(Some(ca_cert.public_key()))
                .map_err(|error| {
                    PgError::Protocol(format!(
                        "OCSP signer is not issued by the client CA: {error}"
                    ))
                })?;
        }
    }
    let signature_oid = basic.signature_algorithm.oid.to_string();
    let algorithm_oid = <x509_parser::asn1_rs::Oid as std::str::FromStr>::from_str(&signature_oid)
        .map_err(|error| PgError::Protocol(format!("bad OCSP signature OID: {error}")))?;
    let signature = basic
        .signature
        .as_bytes()
        .ok_or_else(|| PgError::Protocol("OCSP signature is not byte aligned".to_string()))?;
    let signature_bits = x509_parser::asn1_rs::BitString::new(0, signature);
    let algorithm = x509_parser::x509::AlgorithmIdentifier {
        algorithm: algorithm_oid,
        parameters: None,
    };
    let message = basic
        .tbs_response_data
        .to_der()
        .map_err(|error| PgError::Protocol(format!("OCSP tbs encode: {error}")))?;
    x509_parser::verify::verify_signature(
        signer.public_key(),
        &algorithm,
        &signature_bits,
        &message,
    )
    .map_err(|error| PgError::Protocol(format!("OCSP signature: {error}")))
}

/// Revocation data loaded from a fingerprint list, a CRL, or an OCSP
/// response.
#[derive(Default)]
struct RevocationSource {
    fingerprints: Vec<[u8; 32]>,
    serials: Vec<Vec<u8>>,
}

/// Loads a revocation source: either a PEM/DER X.509 CRL or a list of
/// SHA-256 fingerprints. CRL signatures are verified against the client CA.
fn load_revocation(path: &Path, ca_der: Option<&[u8]>) -> Result<RevocationSource, PgError> {
    let bytes = std::fs::read(path)?;
    let crl_der = match std::str::from_utf8(&bytes) {
        Ok(text) if text.contains("BEGIN X509 CRL") => {
            let (_, pem) = x509_parser::pem::parse_x509_pem(&bytes)
                .map_err(|error| PgError::Protocol(format!("bad CRL PEM: {error}")))?;
            pem.contents
        }
        Ok(text) => {
            return Ok(RevocationSource {
                fingerprints: parse_revoked_fingerprints(text)?,
                serials: Vec::new(),
            })
        }
        Err(_) => bytes,
    };
    let (_, crl) = x509_parser::parse_x509_crl(&crl_der)
        .map_err(|error| PgError::Protocol(format!("bad CRL: {error}")))?;
    let Some(ca_der) = ca_der else {
        return Err(PgError::Protocol(
            "CRL revocation needs --tls-client-ca to verify its signature".to_string(),
        ));
    };
    let (_, ca) = x509_parser::parse_x509_certificate(ca_der)
        .map_err(|error| PgError::Protocol(format!("bad client CA: {error}")))?;
    crl.verify_signature(ca.public_key())
        .map_err(|error| PgError::Protocol(format!("CRL signature: {error}")))?;
    let serials = crl
        .iter_revoked_certificates()
        .map(|entry| entry.raw_serial().to_vec())
        .collect();
    Ok(RevocationSource {
        fingerprints: Vec::new(),
        serials,
    })
}

/// Parses one SHA-256 hex fingerprint per line; `#` starts a comment,
/// `:`/`-`/spaces inside a fingerprint are ignored.
fn parse_revoked_fingerprints(text: &str) -> Result<Vec<[u8; 32]>, PgError> {
    let mut revoked = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let hex: String = line
            .chars()
            .filter(|character| !matches!(character, ':' | '-' | ' '))
            .collect();
        if hex.len() != 64 {
            return Err(PgError::Protocol(format!(
                "revoked fingerprint must be 64 hex characters: {line}"
            )));
        }
        let mut bytes = [0u8; 32];
        for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(chunk)
                .map_err(|_| PgError::Protocol(format!("bad hex in {line}")))?;
            bytes[index] = u8::from_str_radix(pair, 16)
                .map_err(|_| PgError::Protocol(format!("bad hex in {line}")))?;
        }
        revoked.push(bytes);
    }
    Ok(revoked)
}

fn load_tls_config(options: &ServerOptions) -> Result<Arc<rustls::ServerConfig>, PgError> {
    let (Some(cert_path), Some(key_path)) = (&options.tls_cert, &options.tls_key) else {
        return Err(PgError::Protocol(
            "TLS needs both tls_cert and tls_key".to_string(),
        ));
    };
    let cert_bytes = std::fs::read(cert_path)?;
    let key_bytes = std::fs::read(key_path)?;
    let certs = rustls_pemfile::certs(&mut cert_bytes.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PgError::Protocol(format!("bad certificate PEM: {error}")))?;
    if certs.is_empty() {
        return Err(PgError::Protocol(
            "certificate file contains no certificates".to_string(),
        ));
    }
    let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
        .map_err(|error| PgError::Protocol(format!("bad private key PEM: {error}")))?
        .ok_or_else(|| PgError::Protocol("private key file contains no key".to_string()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| PgError::Protocol(format!("TLS versions: {error}")))?;
    let builder = match &options.tls_client_ca {
        Some(ca_path) => {
            let ca_bytes = std::fs::read(ca_path)?;
            let ca_certs = rustls_pemfile::certs(&mut ca_bytes.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| PgError::Protocol(format!("bad client CA PEM: {error}")))?;
            let mut roots = rustls::RootCertStore::empty();
            for cert in &ca_certs {
                roots
                    .add(cert.clone())
                    .map_err(|error| PgError::Protocol(format!("client CA: {error}")))?;
            }
            let verifier_provider = Arc::new(rustls::crypto::ring::default_provider());
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                verifier_provider,
            )
            .build()
            .map_err(|error| PgError::Protocol(format!("client verifier: {error}")))?;
            let mut revocation = match &options.tls_client_revoked {
                Some(path) => load_revocation(path, ca_certs.first().map(|cert| cert.as_ref()))?,
                None => RevocationSource::default(),
            };
            if let Some(path) = &options.tls_client_ocsp {
                revocation
                    .serials
                    .extend(load_ocsp(path, ca_certs.first().map(|cert| cert.as_ref()))?);
            }
            let extra_root = match &options.tls_client_ocsp_ca {
                Some(path) => {
                    let bytes = std::fs::read(path)?;
                    let certs = rustls_pemfile::certs(&mut bytes.as_slice())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| PgError::Protocol(format!("bad OCSP CA PEM: {error}")))?;
                    certs.first().map(|cert| cert.as_ref().to_vec())
                }
                None => None,
            };
            let online = match (&options.tls_client_ocsp_url, ca_certs.first()) {
                (Some(url), Some(ca)) => Some(Arc::new(OcspOnline::new(
                    url.clone(),
                    ca.as_ref().to_vec(),
                    extra_root,
                ))),
                (Some(_), None) => {
                    return Err(PgError::Protocol(
                        "--tls-client-ocsp-url needs --tls-client-ca".to_string(),
                    ))
                }
                (None, _) => None,
            };
            let verifier: Arc<dyn rustls::server::danger::ClientCertVerifier> =
                if revocation.fingerprints.is_empty()
                    && revocation.serials.is_empty()
                    && online.is_none()
                {
                    verifier
                } else {
                    Arc::new(RevocationVerifier {
                        inner: verifier,
                        fingerprints: revocation.fingerprints,
                        serials: revocation.serials,
                        online,
                        allow_hold: options.tls_client_ocsp_allow_hold,
                    })
                };
            builder.with_client_cert_verifier(verifier)
        }
        None => builder.with_no_client_auth(),
    };
    let config = builder
        .with_single_cert(certs, key)
        .map_err(|error| PgError::Protocol(format!("TLS certificate: {error}")))?;
    Ok(Arc::new(config))
}

/// Reloads the TLS configuration when its files change on disk.
struct TlsReloader {
    options: ServerOptions,
    cached: Mutex<CachedTls>,
}

struct CachedTls {
    fingerprint: [u8; 32],
    config: Arc<rustls::ServerConfig>,
}

impl TlsReloader {
    fn new(options: &ServerOptions) -> Result<Self, PgError> {
        let fingerprint = tls_fingerprint(options)?;
        let config = load_tls_config(options)?;
        Ok(Self {
            options: options.clone(),
            cached: Mutex::new(CachedTls {
                fingerprint,
                config,
            }),
        })
    }

    /// Returns the current configuration, reloading it when the PEM files
    /// changed. A reload failure keeps the previous certificate.
    fn snapshot(&self) -> Result<Arc<rustls::ServerConfig>, PgError> {
        let fingerprint = tls_fingerprint(&self.options)?;
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached.fingerprint != fingerprint {
            match load_tls_config(&self.options) {
                Ok(config) => {
                    cached.config = config;
                    cached.fingerprint = fingerprint;
                    eprintln!("pgwire: reloaded TLS certificate");
                }
                Err(error) => eprintln!(
                    "pgwire: TLS reload failed, keeping the previous certificate: {error}"
                ),
            }
        }
        Ok(Arc::clone(&cached.config))
    }
}

fn tls_fingerprint(options: &ServerOptions) -> Result<[u8; 32], PgError> {
    let mut input = Vec::new();
    for path in [
        options.tls_cert.as_ref(),
        options.tls_key.as_ref(),
        options.tls_client_ca.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        input.extend_from_slice(&std::fs::read(path)?);
    }
    Ok(tritdb_cas::sha256(&input))
}

/// Serves connections until the listener is closed or fails.
///
/// Connections are accepted and handled serially; each one opens its own
/// [`SqlEngine`] on `db`.
pub fn serve(db: PathBuf, listener: TcpListener) -> Result<(), PgError> {
    serve_with_options(db, listener, Arc::new(ServerOptions::default()))
}

/// Serves connections with explicit authentication and TLS options.
pub fn serve_with_options(
    db: PathBuf,
    listener: TcpListener,
    options: Arc<ServerOptions>,
) -> Result<(), PgError> {
    if matches!(options.auth, AuthMode::Password(_))
        && !options.tls_enabled()
        && !options.allow_plaintext_password
    {
        return Err(PgError::Protocol(
            "cleartext password authentication over a plaintext connection needs \
             allow_plaintext_password (or enable TLS)"
                .to_string(),
        ));
    }
    let tls = if options.tls_enabled() {
        Some(Arc::new(TlsReloader::new(&options)?))
    } else {
        None
    };
    let engine = Arc::new(Mutex::new(
        SqlEngine::open(&db).map_err(|error| PgError::Sql(error.to_string()))?,
    ));
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("pgwire: accept failed: {error}");
                continue;
            }
        };
        let current = active.fetch_add(1, Ordering::AcqRel) + 1;
        if options.max_connections != 0 && current > options.max_connections {
            active.fetch_sub(1, Ordering::AcqRel);
            eprintln!(
                "pgwire: refusing a connection: {} of {} slots are in use",
                current - 1,
                options.max_connections
            );
            let _ = write_error(&mut stream, "too many connections");
            continue;
        }
        let slot = ConnectionSlot(Arc::clone(&active));
        let engine = Arc::clone(&engine);
        let options = Arc::clone(&options);
        let tls = tls.clone();
        std::thread::spawn(move || {
            let _slot = slot;
            if let Err(error) = handle_connection(engine, stream, options, tls) {
                eprintln!("pgwire: {error}");
            }
        });
    }
    Ok(())
}

/// Releases a connection slot when its handler thread ends.
struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn lock_engine(
    engine: &Arc<Mutex<SqlEngine>>,
    timeout: Option<Duration>,
) -> Result<std::sync::MutexGuard<'_, SqlEngine>, PgError> {
    let Some(timeout) = timeout else {
        return Ok(engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
    };
    let deadline = Instant::now() + timeout;
    loop {
        match engine.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(PgError::Protocol(format!(
                        "timed out after {} ms waiting for the query engine",
                        timeout.as_millis()
                    )));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

/// Acquires the shared engine lock, reporting a timeout to the client instead
/// of blocking this connection behind a long-running query.
fn acquire_engine<'a>(
    stream: &mut dyn ReadWrite,
    engine: &'a Arc<Mutex<SqlEngine>>,
    options: &ServerOptions,
    tx_status: TxStatus,
) -> Result<Option<std::sync::MutexGuard<'a, SqlEngine>>, PgError> {
    match lock_engine(engine, options.engine_lock_timeout) {
        Ok(guard) => Ok(Some(guard)),
        Err(error) => {
            write_error(stream, &error.to_string())?;
            write_message(stream, b'Z', &[tx_status.byte()])?;
            Ok(None)
        }
    }
}

fn handle_connection(
    engine: Arc<Mutex<SqlEngine>>,
    mut stream: TcpStream,
    options: Arc<ServerOptions>,
    tls: Option<Arc<TlsReloader>>,
) -> Result<(), PgError> {
    // Postgres-style clients send one small message per round trip; leaving
    // Nagle on makes every response wait for the peer's delayed ACK (~40 ms on
    // Linux). Disable it per connection.
    stream.set_nodelay(true)?;

    let user: Option<String>;
    let mut conn: Box<dyn ReadWrite> = match read_startup(&mut stream)? {
        Startup::Closed => return Ok(()),
        Startup::Plain { user: name } => {
            user = name;
            if options.require_tls {
                write_error(&mut stream, "TLS is required by this server")?;
                return Err(PgError::Protocol("TLS is required".to_string()));
            }
            Box::new(stream)
        }
        Startup::SslRequest => match tls.as_ref() {
            Some(reloader) => {
                stream.write_all(b"S")?;
                let config = reloader.snapshot()?;
                let server = rustls::ServerConnection::new(config)
                    .map_err(|error| PgError::Protocol(format!("TLS handshake: {error}")))?;
                let mut tls_stream = rustls::StreamOwned::new(server, stream);
                match read_startup(&mut tls_stream)? {
                    Startup::Plain { user: name } => user = name,
                    Startup::Closed => return Ok(()),
                    Startup::SslRequest => {
                        return Err(PgError::Protocol(
                            "client repeated SSLRequest inside TLS".to_string(),
                        ))
                    }
                }
                Box::new(tls_stream)
            }
            None => {
                stream.write_all(b"N")?;
                match read_startup(&mut stream)? {
                    Startup::Plain { user: name } => {
                        user = name;
                        if options.require_tls {
                            write_error(&mut stream, "TLS is required by this server")?;
                            return Err(PgError::Protocol("TLS is required".to_string()));
                        }
                        Box::new(stream)
                    }
                    Startup::Closed => return Ok(()),
                    Startup::SslRequest => {
                        return Err(PgError::Protocol(
                            "client repeated SSLRequest after it was refused".to_string(),
                        ))
                    }
                }
            }
        },
    };

    let stream: &mut dyn ReadWrite = conn.as_mut();
    authenticate(stream, &options.auth, user.as_deref())?;
    // A few ParameterStatus values that clients expect.
    write_parameter(stream, "server_version", "14.0")?;
    write_parameter(stream, "server_encoding", "UTF8")?;
    write_parameter(stream, "client_encoding", "UTF8")?;
    write_parameter(stream, "DateStyle", "ISO, MDY")?;
    // BackendKeyData
    let mut key = Vec::with_capacity(8);
    key.extend_from_slice(&1i32.to_be_bytes());
    key.extend_from_slice(&0i32.to_be_bytes());
    write_message(stream, b'K', &key)?;
    write_message(stream, b'Z', b"I")?;

    let mut prepared: BTreeMap<String, Prepared> = BTreeMap::new();
    let mut portals: BTreeMap<String, Bound> = BTreeMap::new();
    let mut tx_status = TxStatus::Idle;

    loop {
        let Some((kind, payload)) = read_message(stream)? else {
            return Ok(());
        };
        match kind {
            b'Q' => {
                let sql = String::from_utf8_lossy(&payload)
                    .trim_end_matches('\0')
                    .to_string();
                let Some(mut guard) = acquire_engine(stream, &engine, &options, tx_status)? else {
                    continue;
                };
                guard.set_deadline(options.query_timeout.map(|budget| Instant::now() + budget));
                handle_query(stream, &mut guard, &sql, &mut tx_status)?;
                guard.set_deadline(None);
            }
            b'S' => write_message(stream, b'Z', &[tx_status.byte()])?,
            b'X' => return Ok(()),
            b'P' => parse_prepared(stream, &mut prepared, &payload)?,
            b'B' => bind_portal(stream, &prepared, &mut portals, &payload)?,
            b'D' => {
                let Some(mut guard) = acquire_engine(stream, &engine, &options, tx_status)? else {
                    continue;
                };
                describe(stream, &mut guard, &prepared, &portals, &payload)?;
            }
            b'E' => {
                let Some(mut guard) = acquire_engine(stream, &engine, &options, tx_status)? else {
                    continue;
                };
                guard.set_deadline(options.query_timeout.map(|budget| Instant::now() + budget));
                execute_portal(stream, &mut guard, &mut portals, &payload, &mut tx_status)?;
                guard.set_deadline(None);
            }
            b'C' => close_portal(stream, &mut prepared, &mut portals, &payload)?,
            b'H' => {}
            other => {
                write_error(
                    stream,
                    &format!("unsupported message type {}", other as char),
                )?;
                write_message(stream, b'Z', &[tx_status.byte()])?;
            }
        }
    }
}

/// Reads the startup message (handling `SSLRequest`), returning `false` when
/// the client disconnected cleanly.
fn read_startup(stream: &mut dyn ReadWrite) -> Result<Startup, PgError> {
    let mut length = [0u8; 4];
    if stream.read_exact(&mut length).is_err() {
        return Ok(Startup::Closed);
    }
    let length = i32::from_be_bytes(length);
    if !(8..=1_000_000).contains(&length) {
        return Err(PgError::Protocol(format!("bad startup length {length}")));
    }
    let mut body = vec![0u8; (length - 4) as usize];
    stream.read_exact(&mut body)?;
    let protocol = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    if protocol == SSL_REQUEST {
        return Ok(Startup::SslRequest);
    }
    if protocol != PROTOCOL_V3 {
        return Err(PgError::Protocol(format!(
            "unsupported protocol version {protocol}"
        )));
    }
    // The rest is key\0value\0...\0; keep the `user` parameter.
    let user = parse_startup_user(&body);
    Ok(Startup::Plain { user })
}

/// Reads one backend message, returning `None` on clean EOF.
fn read_message(stream: &mut dyn ReadWrite) -> Result<Option<(u8, Vec<u8>)>, PgError> {
    let mut kind = [0u8; 1];
    if stream.read_exact(&mut kind).is_err() {
        return Ok(None);
    }
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = i32::from_be_bytes(length);
    if !(4..=16_000_000).contains(&length) {
        return Err(PgError::Protocol(format!("bad message length {length}")));
    }
    let mut payload = vec![0u8; (length - 4) as usize];
    stream.read_exact(&mut payload)?;
    Ok(Some((kind[0], payload)))
}

fn write_message(stream: &mut dyn ReadWrite, kind: u8, payload: &[u8]) -> Result<(), PgError> {
    let length = (payload.len() as i32) + 4;
    stream.write_all(&[kind])?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    Ok(())
}

fn write_parameter(stream: &mut dyn ReadWrite, key: &str, value: &str) -> Result<(), PgError> {
    let mut payload = Vec::with_capacity(key.len() + value.len() + 2);
    payload.extend_from_slice(key.as_bytes());
    payload.push(0);
    payload.extend_from_slice(value.as_bytes());
    payload.push(0);
    write_message(stream, b'S', &payload)
}

/// Connection-level transaction state used by `ReadyForQuery`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TxStatus {
    Idle,
    InTransaction,
    Failed,
}

impl TxStatus {
    fn byte(self) -> u8 {
        match self {
            TxStatus::Idle => b'I',
            TxStatus::InTransaction => b'T',
            TxStatus::Failed => b'E',
        }
    }
}

enum TxCommand {
    Begin,
    Commit,
    Rollback,
}

fn transaction_command(sql: &str) -> Option<TxCommand> {
    let normalized = sql.trim().trim_end_matches(';').trim().to_ascii_lowercase();
    match normalized.as_str() {
        "begin" | "start transaction" => Some(TxCommand::Begin),
        "commit" | "end" => Some(TxCommand::Commit),
        "rollback" | "abort" => Some(TxCommand::Rollback),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum CopyDirection {
    In,
    Out,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyFormat {
    Text,
    Csv,
    Binary,
}

#[derive(Clone)]
struct CopyOptions {
    format: CopyFormat,
    delimiter: u8,
    null: String,
    header: bool,
    quote: u8,
    escape: u8,
    force_null: Vec<String>,
    force_not_null: Vec<String>,
    encoding: String,
}

impl CopyOptions {
    fn defaults(format: CopyFormat) -> Self {
        match format {
            CopyFormat::Csv => Self {
                format,
                delimiter: b',',
                null: String::new(),
                header: false,
                quote: b'"',
                escape: b'"',
                force_null: Vec::new(),
                force_not_null: Vec::new(),
                encoding: "UTF8".to_string(),
            },
            _ => Self {
                format,
                delimiter: b'\t',
                null: "\\N".to_string(),
                header: false,
                quote: b'"',
                escape: b'"',
                force_null: Vec::new(),
                force_not_null: Vec::new(),
                encoding: "UTF8".to_string(),
            },
        }
    }
}

fn split_copy_options(inner: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut depth = 0usize;
    for character in inner.chars() {
        match character {
            '\'' => {
                in_quote = !in_quote;
                current.push(character);
            }
            '(' if !in_quote => {
                depth += 1;
                current.push(character);
            }
            ')' if !in_quote => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if !in_quote && depth == 0 => {
                items.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items
        .into_iter()
        .filter_map(|item| {
            let mut parts = item.splitn(2, char::is_whitespace);
            let key = parts.next()?.to_ascii_uppercase();
            let value = parts
                .next()
                .map(|value| value.trim().to_string())
                .unwrap_or_else(|| "true".to_string());
            Some((key, value))
        })
        .collect()
}

fn copy_option_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_string()
    }
}

fn copy_option_columns(value: &str) -> Vec<String> {
    let value = copy_option_value(value);
    let value = value.trim().trim_start_matches('(').trim_end_matches(')');
    if value == "*" {
        return vec!["*".to_string()];
    }
    value
        .split(',')
        .map(|column| column.trim().trim_matches('"').to_ascii_lowercase())
        .filter(|column| !column.is_empty())
        .collect()
}

/// Maps a PostgreSQL encoding name to the matching WHATWG codec.
///
/// `LATIN1` is deliberately not routed through here: WHATWG labels
/// `iso-8859-1` as an alias of windows-1252, while PostgreSQL's `LATIN1`
/// really is ISO-8859-1, so that one keeps its byte-for-byte path.
fn copy_encoding_codec(encoding: &str) -> Option<&'static encoding_rs::Encoding> {
    let label = match encoding
        .to_ascii_uppercase()
        .replace(['-', '_'], "")
        .as_str()
    {
        "LATIN2" => "iso-8859-2",
        "LATIN3" => "iso-8859-3",
        "LATIN4" => "iso-8859-4",
        "LATIN5" => "iso-8859-9",
        "LATIN6" => "iso-8859-10",
        "LATIN7" => "iso-8859-13",
        "LATIN8" => "iso-8859-14",
        "LATIN9" => "iso-8859-15",
        "LATIN10" => "iso-8859-16",
        "ISO88592" => "iso-8859-2",
        "ISO88593" => "iso-8859-3",
        "ISO88594" => "iso-8859-4",
        "ISO88595" => "iso-8859-5",
        "ISO88596" => "iso-8859-6",
        "ISO88597" => "iso-8859-7",
        "ISO88598" => "iso-8859-8",
        "ISO88599" => "iso-8859-9",
        "ISO885910" => "iso-8859-10",
        "ISO885913" => "iso-8859-13",
        "ISO885914" => "iso-8859-14",
        "ISO885915" => "iso-8859-15",
        "ISO885916" => "iso-8859-16",
        "WIN1250" | "CP1250" => "windows-1250",
        "WIN1251" | "CP1251" => "windows-1251",
        "WIN1252" | "CP1252" => "windows-1252",
        "WIN1253" | "CP1253" => "windows-1253",
        "WIN1254" | "CP1254" => "windows-1254",
        "WIN1255" | "CP1255" => "windows-1255",
        "WIN1256" | "CP1256" => "windows-1256",
        "WIN1257" | "CP1257" => "windows-1257",
        "WIN1258" | "CP1258" => "windows-1258",
        "WIN866" | "CP866" => "ibm866",
        "WIN874" | "CP874" => "windows-874",
        "KOI8R" => "koi8-r",
        "KOI8U" => "koi8-u",
        "SJIS" => "shift_jis",
        "BIG5" => "big5",
        "EUCJP" => "euc-jp",
        "EUCKR" | "UHC" => "euc-kr",
        "EUCCN" | "GBK" => "gbk",
        "GB18030" => "gb18030",
        _ => return None,
    };
    encoding_rs::Encoding::for_label(label.as_bytes())
}

fn copy_encoding_supported(encoding: &str) -> bool {
    let normalized = encoding.to_ascii_uppercase().replace(['-', '_'], "");
    matches!(
        normalized.as_str(),
        "UTF8" | "LATIN1" | "ISO88591" | "ASCII" | "UTF16LE" | "UTF16BE"
    ) || copy_encoding_codec(encoding).is_some()
}

/// Decodes one COPY line using the declared encoding.
///
/// Invalid or incomplete byte sequences are an error, not a silent
/// replacement: a truncated UTF-16 unit or a broken multi-byte character used
/// to be dropped, quietly corrupting the row and shifting later fields.
fn decode_copy_line(bytes: &[u8], encoding: &str) -> Result<String, String> {
    let normalized = encoding.to_ascii_uppercase().replace(['-', '_'], "");
    match normalized.as_str() {
        "LATIN1" | "ISO88591" => Ok(bytes.iter().map(|byte| *byte as char).collect()),
        "ASCII" => {
            if bytes.iter().any(|byte| *byte >= 0x80) {
                return Err("COPY data is not valid ASCII".to_string());
            }
            Ok(bytes.iter().map(|byte| *byte as char).collect())
        }
        "UTF16LE" => decode_utf16(bytes, true),
        "UTF16BE" => decode_utf16(bytes, false),
        _ => match copy_encoding_codec(encoding) {
            Some(codec) => {
                let (text, had_errors) = codec.decode_without_bom_handling(bytes);
                if had_errors {
                    Err(format!("COPY data is not valid {encoding}"))
                } else {
                    Ok(text.into_owned())
                }
            }
            None => String::from_utf8(bytes.to_vec())
                .map_err(|_| format!("COPY data is not valid {encoding}")),
        },
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    let (chunks, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("COPY data has an incomplete UTF-16 code unit".to_string());
    }
    let units: Vec<u16> = chunks
        .iter()
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|_| "COPY data has an unpaired UTF-16 surrogate".to_string())
}

/// Encodes one COPY line as UTF-8 into the declared encoding.
///
/// PostgreSQL refuses to write characters the target encoding cannot
/// represent, so an unrepresentable character is an error rather than a
/// substitution.
fn encode_copy_line(text: &str, encoding: &str) -> Result<Vec<u8>, String> {
    let normalized = encoding.to_ascii_uppercase().replace(['-', '_'], "");
    match normalized.as_str() {
        "LATIN1" | "ISO88591" => {
            let mut out = Vec::with_capacity(text.len());
            for character in text.chars() {
                let code = character as u32;
                if code > 0xFF {
                    return Err(format!(
                        "character U+{code:04X} has no equivalent in encoding {encoding}"
                    ));
                }
                out.push(code as u8);
            }
            Ok(out)
        }
        "ASCII" => {
            let mut out = Vec::with_capacity(text.len());
            for character in text.chars() {
                let code = character as u32;
                if code >= 0x80 {
                    return Err(format!(
                        "character U+{code:04X} has no equivalent in encoding {encoding}"
                    ));
                }
                out.push(code as u8);
            }
            Ok(out)
        }
        "UTF16LE" => Ok(encode_utf16(text, true)),
        "UTF16BE" => Ok(encode_utf16(text, false)),
        _ => match copy_encoding_codec(encoding) {
            Some(codec) => {
                let (bytes, _, had_errors) = codec.encode(text);
                if had_errors {
                    return Err(format!(
                        "characters have no equivalent in encoding {encoding}"
                    ));
                }
                Ok(bytes.into_owned())
            }
            None => Ok(text.as_bytes().to_vec()),
        },
    }
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        let bytes = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        out.extend_from_slice(&bytes);
    }
    out
}

fn copy_option_byte(value: &str, default: u8) -> u8 {
    copy_option_value(value)
        .as_bytes()
        .first()
        .copied()
        .unwrap_or(default)
}

/// Parses the `(FORMAT csv, HEADER true, DELIMITER ';', NULL 'NULL', ...)`
/// clause that follows `FROM STDIN` / `TO STDOUT`.
fn parse_copy_options(tail: &str) -> CopyOptions {
    let (Some(open), Some(close)) = (tail.find('('), tail.rfind(')')) else {
        return CopyOptions::defaults(CopyFormat::Text);
    };
    if close <= open {
        return CopyOptions::defaults(CopyFormat::Text);
    }
    let pairs = split_copy_options(&tail[open + 1..close]);
    let format = pairs
        .iter()
        .find(|(key, _)| key == "FORMAT")
        .map(
            |(_, value)| match copy_option_value(value).to_ascii_lowercase().as_str() {
                "csv" => CopyFormat::Csv,
                "binary" => CopyFormat::Binary,
                _ => CopyFormat::Text,
            },
        )
        .unwrap_or(CopyFormat::Text);
    let mut options = CopyOptions::defaults(format);
    for (key, value) in pairs {
        match key.as_str() {
            "FORMAT" => {}
            "HEADER" => {
                let value = copy_option_value(&value).to_ascii_lowercase();
                options.header = !matches!(value.as_str(), "false" | "off" | "0");
            }
            "DELIMITER" => options.delimiter = copy_option_byte(&value, options.delimiter),
            "NULL" => options.null = copy_option_value(&value),
            "QUOTE" => options.quote = copy_option_byte(&value, options.quote),
            "ESCAPE" => options.escape = copy_option_byte(&value, options.escape),
            "FORCE_NULL" => options.force_null = copy_option_columns(&value),
            "FORCE_NOT_NULL" => options.force_not_null = copy_option_columns(&value),
            "ENCODING" => options.encoding = copy_option_value(&value),
            _ => {}
        }
    }
    options
}

struct CopyPlan {
    table: String,
    columns: Option<Vec<String>>,
    direction: CopyDirection,
    options: CopyOptions,
}

/// Recognises `COPY <table> [(<cols>)] FROM STDIN` / `TO STDOUT`.
fn parse_copy(sql: &str) -> Option<CopyPlan> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.len() < 5 || !trimmed[..4].eq_ignore_ascii_case("copy") {
        return None;
    }
    let rest = trimmed[4..].trim_start();
    let upper = rest.to_ascii_uppercase();
    let (head, direction, tail) = if let Some(at) = upper.rfind(" FROM STDIN") {
        (&rest[..at], CopyDirection::In, &rest[at..])
    } else {
        let at = upper.rfind(" TO STDOUT")?;
        (&rest[..at], CopyDirection::Out, &rest[at..])
    };
    let options = parse_copy_options(tail);
    let head = head.trim();
    let (table, columns) = match head.find('(') {
        Some(open) => {
            let close = head.rfind(')')?;
            let table = head[..open].trim().trim_matches('"').to_ascii_lowercase();
            let columns: Vec<String> = head[open + 1..close]
                .split(',')
                .map(|column| column.trim().trim_matches('"').to_ascii_lowercase())
                .filter(|column| !column.is_empty())
                .collect();
            (table, Some(columns))
        }
        None => (head.trim().trim_matches('"').to_ascii_lowercase(), None),
    };
    if !is_plain_identifier(&table) {
        return None;
    }
    if let Some(columns) = &columns {
        if columns.iter().any(|column| !is_plain_identifier(column)) {
            return None;
        }
    }
    Some(CopyPlan {
        table,
        columns,
        direction,
        options,
    })
}

/// Identifier-shaped names only: COPY must never smuggle SQL text.
fn is_plain_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

fn copy_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Unescapes one COPY text-format value (`\\`, `\t`, `\n`, `\r`).
fn copy_unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn copy_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Builds a SQL literal for one COPY value, using the target column's type.
fn copy_literal_value(value: Option<&str>, data_type: DataType) -> String {
    match value {
        None => "NULL".to_string(),
        Some(text) => copy_literal(text, data_type),
    }
}

fn copy_literal(value: &str, data_type: DataType) -> String {
    let value = copy_unescape(value);
    match data_type {
        // Integer columns accept integers only: `1e999` parses as `f64::INFINITY`
        // and used to be spliced in verbatim.
        DataType::Int | DataType::Lukasiewicz => {
            if value.parse::<i64>().is_ok() {
                value
            } else {
                copy_quote(&value)
            }
        }
        DataType::Nat => {
            if value.parse::<u64>().is_ok() {
                value
            } else {
                copy_quote(&value)
            }
        }
        DataType::Bool | DataType::Logic | DataType::Belnap => {
            let lowered = value.to_ascii_lowercase();
            match lowered.as_str() {
                "t" | "true" => "TRUE".to_string(),
                "f" | "false" => "FALSE".to_string(),
                "both" | "neither" => value.to_ascii_uppercase(),
                _ => copy_quote(&value),
            }
        }
        DataType::Text => copy_quote(&value),
    }
}

fn copy_failed(stream: &mut dyn ReadWrite, message: &str) -> Result<bool, PgError> {
    write_error(stream, message)?;
    Ok(false)
}

/// COPY failure carrying the SQLSTATE and hint of the SQL error.
fn copy_failed_sql(stream: &mut dyn ReadWrite, error: &SqlError) -> Result<bool, PgError> {
    write_sql_error(stream, error)?;
    Ok(false)
}

fn handle_copy(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    plan: &CopyPlan,
) -> Result<bool, PgError> {
    if !copy_encoding_supported(&plan.options.encoding) {
        return copy_failed(
            stream,
            &format!("unsupported COPY encoding {}", plan.options.encoding),
        );
    }
    let Some(schema) = engine.table_schema(&plan.table) else {
        return copy_failed(stream, &format!("unknown table {}", plan.table));
    };
    let columns = match &plan.columns {
        Some(columns) => columns.clone(),
        None => schema.iter().map(|(name, _)| name.clone()).collect(),
    };
    let mut types = Vec::with_capacity(columns.len());
    for column in &columns {
        let Some((_, data_type)) = schema.iter().find(|(name, _)| name == column) else {
            return copy_failed(stream, &format!("unknown column {column}"));
        };
        types.push(*data_type);
    }
    match (plan.direction, plan.options.format) {
        (CopyDirection::In, CopyFormat::Text) => {
            copy_in(stream, engine, &plan.table, &columns, &types, &plan.options)
        }
        (CopyDirection::In, CopyFormat::Csv) => {
            copy_in_csv(stream, engine, &plan.table, &columns, &types, &plan.options)
        }
        (CopyDirection::In, CopyFormat::Binary) => {
            copy_in_binary(stream, engine, &plan.table, &columns, &types)
        }
        (CopyDirection::Out, CopyFormat::Text) => {
            let Some((table, indices)) = copy_out_source(engine, &plan.table, &columns) else {
                return copy_failed(stream, &format!("unknown table {}", plan.table));
            };
            copy_out(stream, table, &indices, &columns, &plan.options)
        }
        (CopyDirection::Out, CopyFormat::Csv) => {
            let Some((table, indices)) = copy_out_source(engine, &plan.table, &columns) else {
                return copy_failed(stream, &format!("unknown table {}", plan.table));
            };
            copy_out_csv(stream, table, &indices, &columns, &plan.options)
        }
        (CopyDirection::Out, CopyFormat::Binary) => {
            let Some((table, indices)) = copy_out_source(engine, &plan.table, &columns) else {
                return copy_failed(stream, &format!("unknown table {}", plan.table));
            };
            copy_out_binary(stream, table, &indices, &types)
        }
    }
}

/// Resolves the table and column indexes for a streaming COPY OUT.
fn copy_out_source<'a>(
    engine: &'a SqlEngine,
    name: &str,
    columns: &[String],
) -> Option<(&'a Table, Vec<usize>)> {
    let table = engine.copy_source(name)?;
    let mut indices = Vec::with_capacity(columns.len());
    for column in columns {
        indices.push(table.schema.index_of(column)?);
    }
    Some((table, indices))
}

/// Parses one CSV line into fields. An unquoted empty field is `NULL`.
fn parse_csv_line(line: &str, options: &CopyOptions) -> (Vec<(bool, String)>, bool) {
    let quote = options.quote as char;
    let escape = options.escape as char;
    let delimiter = options.delimiter as char;
    let mut fields = Vec::new();
    let mut unterminated = false;
    let mut chars = line.chars().peekable();
    loop {
        let mut field = String::new();
        let mut quoted = false;
        if chars.peek() == Some(&quote) {
            quoted = true;
            chars.next();
            let mut closed = false;
            while let Some(character) = chars.next() {
                if character == escape && escape == quote {
                    if chars.peek() == Some(&quote) {
                        chars.next();
                        field.push(quote);
                    } else {
                        // A lone quote closes the field; `""` above escapes one.
                        closed = true;
                        break;
                    }
                } else if character == quote {
                    closed = true;
                    break;
                } else {
                    field.push(character);
                }
            }
            if !closed {
                unterminated = true;
            }
            while matches!(chars.peek(), Some(' ') | Some('\t')) {
                chars.next();
            }
        } else {
            while let Some(&character) = chars.peek() {
                if character == delimiter {
                    break;
                }
                field.push(character);
                chars.next();
            }
        }
        fields.push((quoted, field));
        if chars.next() != Some(delimiter) {
            break;
        }
    }
    (fields, unterminated)
}

fn csv_field(value: Option<&str>, options: &CopyOptions) -> String {
    match value {
        None => options.null.clone(),
        Some(text) => {
            let quote = options.quote as char;
            let needs_quote = text.is_empty()
                || text.contains(options.delimiter as char)
                || text.contains(quote)
                || text.contains('\n')
                || text.contains('\r')
                || text == options.null;
            if needs_quote {
                format!(
                    "{quote}{}{quote}",
                    text.replace(quote, &format!("{quote}{quote}"))
                )
            } else {
                text.to_string()
            }
        }
    }
}

fn copy_in_csv(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    table: &str,
    columns: &[String],
    types: &[DataType],
    options: &CopyOptions,
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + columns.len() * 2);
    payload.push(0);
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for _ in columns {
        payload.extend_from_slice(&0i16.to_be_bytes());
    }
    write_message(stream, b'G', &payload)?;

    let mut inserted = 0usize;
    let mut batch = String::new();
    let mut batched = 0usize;
    let mut header_pending = options.header;
    loop {
        let Some((kind, payload)) = read_message(stream)? else {
            return Err(PgError::Protocol(
                "client disconnected during COPY".to_string(),
            ));
        };
        match kind {
            b'd' => {
                if header_pending {
                    header_pending = false;
                    continue;
                }
                let line = match decode_copy_line(&payload, &options.encoding) {
                    Ok(line) => line,
                    Err(message) => return copy_failed(stream, &message),
                };
                let line = line.trim_end_matches(['\r', '\n']);
                let (parsed, unterminated) = parse_csv_line(line, options);
                if unterminated {
                    return copy_failed(stream, "COPY CSV has an unterminated quoted field");
                }
                if parsed.len() != columns.len() {
                    return copy_failed(
                        stream,
                        &format!(
                            "COPY expected {} columns, got {}",
                            columns.len(),
                            parsed.len()
                        ),
                    );
                }
                let values: Vec<Option<String>> = parsed
                    .into_iter()
                    .enumerate()
                    .map(|(index, (quoted, field))| {
                        let column = columns[index].to_ascii_lowercase();
                        let force_null = options
                            .force_null
                            .iter()
                            .any(|name| name == "*" || name == &column);
                        let force_not_null = options
                            .force_not_null
                            .iter()
                            .any(|name| name == "*" || name == &column);
                        if force_not_null && field.is_empty() {
                            return Some(field);
                        }
                        if field == options.null && (force_null || !quoted) {
                            return None;
                        }
                        Some(field)
                    })
                    .collect();
                if batched == 0 {
                    batch.push_str(&format!(
                        "INSERT INTO {table} ({}) VALUES ",
                        columns.join(", ")
                    ));
                } else {
                    batch.push(',');
                }
                batch.push('(');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        batch.push(',');
                    }
                    batch.push_str(&copy_literal_value(value.as_deref(), types[index]));
                }
                batch.push(')');
                batched += 1;
                if inserted + batched > MAX_COPY_ROWS {
                    return copy_failed(
                        stream,
                        &format!("COPY inserts more than {MAX_COPY_ROWS} rows"),
                    );
                }
                if batched >= 1024 || batch.len() >= MAX_COPY_BATCH_BYTES {
                    batch.push(';');
                    if let Err(error) = engine.execute_script(&batch) {
                        return copy_failed_sql(stream, &error);
                    }
                    inserted += batched;
                    batch.clear();
                    batched = 0;
                }
            }
            b'c' => break,
            b'f' => {
                let message = String::from_utf8_lossy(&payload)
                    .trim_end_matches('\0')
                    .to_string();
                return copy_failed(stream, &message);
            }
            other => {
                return Err(PgError::Protocol(format!(
                    "unexpected message {} during COPY",
                    other as char
                )));
            }
        }
    }
    if batched > 0 {
        batch.push(';');
        if let Err(error) = engine.execute_script(&batch) {
            return copy_failed_sql(stream, &error);
        }
        inserted += batched;
    }
    let tag = format!("COPY {inserted}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

fn copy_out_csv(
    stream: &mut dyn ReadWrite,
    table: &Table,
    indices: &[usize],
    columns: &[String],
    options: &CopyOptions,
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + columns.len() * 2);
    payload.push(0);
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for _ in columns {
        payload.extend_from_slice(&0i16.to_be_bytes());
    }
    write_message(stream, b'H', &payload)?;
    if options.header {
        let mut header = String::new();
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                header.push(options.delimiter as char);
            }
            header.push_str(column);
        }
        header.push('\n');
        let header = match encode_copy_line(&header, &options.encoding) {
            Ok(bytes) => bytes,
            Err(message) => return copy_failed(stream, &message),
        };
        write_message(stream, b'd', &header)?;
    }
    let mut sent = 0usize;
    for row in &table.rows {
        let mut line = String::new();
        for (position, &index) in indices.iter().enumerate() {
            if position > 0 {
                line.push(options.delimiter as char);
            }
            let value = &row[index];
            let text = if value.is_null() {
                None
            } else {
                Some(value_text(value))
            };
            line.push_str(&csv_field(text.as_deref(), options));
        }
        line.push('\n');
        let bytes = match encode_copy_line(&line, &options.encoding) {
            Ok(bytes) => bytes,
            Err(message) => return copy_failed(stream, &message),
        };
        write_message(stream, b'd', &bytes)?;
        sent += 1;
    }
    write_message(stream, b'c', &[])?;
    let tag = format!("COPY {sent}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

const COPY_BINARY_SIGNATURE: &[u8] = b"PGCOPY\n\xff\r\n\0";

fn decode_copy_binary_value(bytes: &[u8], data_type: DataType) -> Result<String, PgError> {
    match data_type {
        DataType::Int | DataType::Nat => {
            let value = match bytes.len() {
                2 => i16::from_be_bytes([bytes[0], bytes[1]]) as i64,
                4 => i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
                8 => i64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]),
                _ => {
                    return Err(PgError::Protocol(format!(
                        "COPY binary integer has {} bytes",
                        bytes.len()
                    )))
                }
            };
            Ok(value.to_string())
        }
        DataType::Bool => {
            let [flag] = bytes else {
                return Err(PgError::Protocol(
                    "COPY binary bool must be 1 byte".to_string(),
                ));
            };
            Ok(if *flag == 0 {
                "f".to_string()
            } else {
                "t".to_string()
            })
        }
        _ => Ok(String::from_utf8_lossy(bytes).to_string()),
    }
}

fn encode_copy_binary_value(value: &Value, data_type: DataType) -> Vec<u8> {
    match (data_type, value) {
        (DataType::Int | DataType::Nat, Value::Int(number)) => number.to_be_bytes().to_vec(),
        (DataType::Bool, Value::Bool(flag)) => vec![u8::from(*flag)],
        _ => value_text(value).into_bytes(),
    }
}

/// Pulls complete binary rows out of `buffer`, leaving a partial row behind.
fn take_binary_rows(
    buffer: &mut Vec<u8>,
    header_done: &mut bool,
    types: &[DataType],
) -> Result<Vec<Vec<Option<String>>>, PgError> {
    let mut rows = Vec::new();
    loop {
        if !*header_done {
            if buffer.len() < 19 {
                break;
            }
            if &buffer[..11] != COPY_BINARY_SIGNATURE {
                return Err(PgError::Protocol("bad COPY binary signature".to_string()));
            }
            let extension = i32::from_be_bytes([buffer[15], buffer[16], buffer[17], buffer[18]]);
            if extension < 0 {
                return Err(PgError::Protocol(
                    "bad COPY binary extension length".to_string(),
                ));
            }
            let total = 19 + extension as usize;
            if buffer.len() < total {
                break;
            }
            buffer.drain(..total);
            *header_done = true;
            continue;
        }
        if buffer.len() < 2 {
            break;
        }
        let count = i16::from_be_bytes([buffer[0], buffer[1]]);
        if count == -1 {
            buffer.drain(..2);
            break;
        }
        if count < 0 {
            return Err(PgError::Protocol(format!(
                "bad COPY binary field count {count}"
            )));
        }
        let count = count as usize;
        if count != types.len() {
            return Err(PgError::Protocol(format!(
                "COPY binary row has {count} fields, expected {}",
                types.len()
            )));
        }
        let mut cursor = 2;
        let mut values = Vec::with_capacity(count);
        let mut complete = true;
        for data_type in types {
            if buffer.len() < cursor + 4 {
                complete = false;
                break;
            }
            let length = i32::from_be_bytes([
                buffer[cursor],
                buffer[cursor + 1],
                buffer[cursor + 2],
                buffer[cursor + 3],
            ]);
            cursor += 4;
            if length < 0 {
                values.push(None);
                continue;
            }
            let length = length as usize;
            if buffer.len() < cursor + length {
                complete = false;
                break;
            }
            let bytes = &buffer[cursor..cursor + length];
            values.push(Some(decode_copy_binary_value(bytes, *data_type)?));
            cursor += length;
        }
        if !complete {
            break;
        }
        buffer.drain(..cursor);
        rows.push(values);
    }
    Ok(rows)
}

fn copy_in_binary(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    table: &str,
    columns: &[String],
    types: &[DataType],
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + columns.len() * 2);
    payload.push(1);
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for _ in columns {
        payload.extend_from_slice(&1i16.to_be_bytes());
    }
    write_message(stream, b'G', &payload)?;

    let mut buffer: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut inserted = 0usize;
    let mut batch = String::new();
    let mut batched = 0usize;

    loop {
        let Some((kind, payload)) = read_message(stream)? else {
            return Err(PgError::Protocol(
                "client disconnected during COPY".to_string(),
            ));
        };
        match kind {
            b'd' => buffer.extend_from_slice(&payload),
            b'c' => {}
            b'f' => {
                let message = String::from_utf8_lossy(&payload)
                    .trim_end_matches('\0')
                    .to_string();
                return copy_failed(stream, &message);
            }
            other => {
                return Err(PgError::Protocol(format!(
                    "unexpected message {} during COPY",
                    other as char
                )));
            }
        }
        let rows = take_binary_rows(&mut buffer, &mut header_done, types)?;
        for values in rows {
            if batched == 0 {
                batch.push_str(&format!(
                    "INSERT INTO {table} ({}) VALUES ",
                    columns.join(", ")
                ));
            } else {
                batch.push(',');
            }
            batch.push('(');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    batch.push(',');
                }
                batch.push_str(&copy_literal_value(value.as_deref(), types[index]));
            }
            batch.push(')');
            batched += 1;
            if inserted + batched > MAX_COPY_ROWS {
                return copy_failed(
                    stream,
                    &format!("COPY inserts more than {MAX_COPY_ROWS} rows"),
                );
            }
            if batched >= 1024 || batch.len() >= MAX_COPY_BATCH_BYTES {
                batch.push(';');
                if let Err(error) = engine.execute_script(&batch) {
                    return copy_failed_sql(stream, &error);
                }
                inserted += batched;
                batch.clear();
                batched = 0;
            }
        }
        if kind == b'c' {
            break;
        }
    }
    if batched > 0 {
        batch.push(';');
        if let Err(error) = engine.execute_script(&batch) {
            return copy_failed_sql(stream, &error);
        }
        inserted += batched;
    }
    let tag = format!("COPY {inserted}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

fn copy_out_binary(
    stream: &mut dyn ReadWrite,
    table: &Table,
    indices: &[usize],
    types: &[DataType],
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + types.len() * 2);
    payload.push(1);
    payload.extend_from_slice(&(types.len() as i16).to_be_bytes());
    for _ in types {
        payload.extend_from_slice(&1i16.to_be_bytes());
    }
    write_message(stream, b'H', &payload)?;

    let mut header = Vec::with_capacity(19);
    header.extend_from_slice(COPY_BINARY_SIGNATURE);
    header.extend_from_slice(&0i32.to_be_bytes());
    header.extend_from_slice(&0i32.to_be_bytes());
    write_message(stream, b'd', &header)?;

    let mut sent = 0usize;
    for row in &table.rows {
        let mut message = Vec::new();
        message.extend_from_slice(&(indices.len() as i16).to_be_bytes());
        for (position, &index) in indices.iter().enumerate() {
            let value = &row[index];
            if value.is_null() {
                message.extend_from_slice(&(-1i32).to_be_bytes());
                continue;
            }
            let bytes = encode_copy_binary_value(value, types[position]);
            message.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            message.extend_from_slice(&bytes);
        }
        write_message(stream, b'd', &message)?;
        sent += 1;
    }
    write_message(stream, b'd', &(-1i16).to_be_bytes())?;
    write_message(stream, b'c', &[])?;
    let tag = format!("COPY {sent}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

fn copy_in(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    table: &str,
    columns: &[String],
    types: &[DataType],
    options: &CopyOptions,
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + columns.len() * 2);
    payload.push(0); // text format
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for _ in columns {
        payload.extend_from_slice(&0i16.to_be_bytes());
    }
    write_message(stream, b'G', &payload)?;

    let mut inserted = 0usize;
    let mut batch = String::new();
    let mut batched = 0usize;
    let mut header_pending = options.header;
    loop {
        let Some((kind, payload)) = read_message(stream)? else {
            return Err(PgError::Protocol(
                "client disconnected during COPY".to_string(),
            ));
        };
        match kind {
            b'd' => {
                if header_pending {
                    header_pending = false;
                    continue;
                }
                let line = match decode_copy_line(&payload, &options.encoding) {
                    Ok(line) => line,
                    Err(message) => return copy_failed(stream, &message),
                };
                let line = line.trim_end_matches(['\r', '\n']);
                let values: Vec<&str> = line.split(options.delimiter as char).collect();
                if values.len() != columns.len() {
                    return copy_failed(
                        stream,
                        &format!(
                            "COPY expected {} columns, got {}",
                            columns.len(),
                            values.len()
                        ),
                    );
                }
                if batched == 0 {
                    batch.push_str(&format!(
                        "INSERT INTO {table} ({}) VALUES ",
                        columns.join(", ")
                    ));
                } else {
                    batch.push(',');
                }
                batch.push('(');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        batch.push(',');
                    }
                    if *value == options.null.as_str() {
                        batch.push_str("NULL");
                    } else {
                        batch.push_str(&copy_literal(value, types[index]));
                    }
                }
                batch.push(')');
                batched += 1;
                if inserted + batched > MAX_COPY_ROWS {
                    return copy_failed(
                        stream,
                        &format!("COPY inserts more than {MAX_COPY_ROWS} rows"),
                    );
                }
                if batched >= 1024 || batch.len() >= MAX_COPY_BATCH_BYTES {
                    batch.push(';');
                    if let Err(error) = engine.execute_script(&batch) {
                        return copy_failed_sql(stream, &error);
                    }
                    inserted += batched;
                    batch.clear();
                    batched = 0;
                }
            }
            b'c' => break,
            b'f' => {
                let message = String::from_utf8_lossy(&payload)
                    .trim_end_matches('\0')
                    .to_string();
                return copy_failed(stream, &message);
            }
            other => {
                return Err(PgError::Protocol(format!(
                    "unexpected message {} during COPY",
                    other as char
                )));
            }
        }
    }
    if batched > 0 {
        batch.push(';');
        if let Err(error) = engine.execute_script(&batch) {
            return copy_failed_sql(stream, &error);
        }
        inserted += batched;
    }
    let tag = format!("COPY {inserted}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

fn copy_out(
    stream: &mut dyn ReadWrite,
    table: &Table,
    indices: &[usize],
    columns: &[String],
    options: &CopyOptions,
) -> Result<bool, PgError> {
    let mut payload = Vec::with_capacity(3 + columns.len() * 2);
    payload.push(0);
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for _ in columns {
        payload.extend_from_slice(&0i16.to_be_bytes());
    }
    write_message(stream, b'H', &payload)?;
    if options.header {
        let mut header = String::new();
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                header.push(options.delimiter as char);
            }
            header.push_str(column);
        }
        header.push('\n');
        let header = match encode_copy_line(&header, &options.encoding) {
            Ok(bytes) => bytes,
            Err(message) => return copy_failed(stream, &message),
        };
        write_message(stream, b'd', &header)?;
    }
    let mut sent = 0usize;
    for row in &table.rows {
        let mut line = String::new();
        for (position, &index) in indices.iter().enumerate() {
            if position > 0 {
                line.push(options.delimiter as char);
            }
            let value = &row[index];
            if value.is_null() {
                line.push_str(&options.null);
            } else {
                line.push_str(&copy_escape(&value_text(value)));
            }
        }
        line.push('\n');
        let bytes = match encode_copy_line(&line, &options.encoding) {
            Ok(bytes) => bytes,
            Err(message) => return copy_failed(stream, &message),
        };
        write_message(stream, b'd', &bytes)?;
        sent += 1;
    }
    write_message(stream, b'c', &[])?;
    let tag = format!("COPY {sent}\0");
    write_message(stream, b'C', tag.as_bytes())?;
    Ok(true)
}

fn handle_query(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    sql: &str,
    tx_status: &mut TxStatus,
) -> Result<(), PgError> {
    if sql.trim().is_empty() {
        write_message(stream, b'I', &[])?;
        write_message(stream, b'Z', &[tx_status.byte()])?;
        return Ok(());
    }
    if let Some(command) = transaction_command(sql) {
        match command {
            TxCommand::Begin => {
                if *tx_status == TxStatus::Failed {
                    write_error(
                        stream,
                        "current transaction is aborted, commands ignored until end of transaction block",
                    )?;
                } else {
                    *tx_status = TxStatus::InTransaction;
                    write_message(stream, b'C', b"BEGIN\0")?;
                }
            }
            TxCommand::Commit => {
                if *tx_status == TxStatus::Failed {
                    *tx_status = TxStatus::Idle;
                    write_message(stream, b'C', b"ROLLBACK\0")?;
                } else {
                    *tx_status = TxStatus::Idle;
                    write_message(stream, b'C', b"COMMIT\0")?;
                }
            }
            TxCommand::Rollback => {
                *tx_status = TxStatus::Idle;
                write_message(stream, b'C', b"ROLLBACK\0")?;
            }
        }
        write_message(stream, b'Z', &[tx_status.byte()])?;
        return Ok(());
    }
    if *tx_status == TxStatus::Failed {
        write_error(
            stream,
            "current transaction is aborted, commands ignored until end of transaction block",
        )?;
        write_message(stream, b'Z', &[tx_status.byte()])?;
        return Ok(());
    }
    let success = match parse_copy(sql) {
        Some(plan) => handle_copy(stream, engine, &plan)?,
        None => execute_and_write(stream, engine, sql, &[])?,
    };
    if !success && *tx_status == TxStatus::InTransaction {
        *tx_status = TxStatus::Failed;
    }
    write_message(stream, b'Z', &[tx_status.byte()])?;
    Ok(())
}

fn execute_and_write(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    sql: &str,
    result_formats: &[i16],
) -> Result<bool, PgError> {
    match engine.execute_script(sql) {
        Ok(results) => {
            for result in results {
                match result {
                    QueryResult::Rows { columns, rows } => {
                        let oids = column_oids(&rows, columns.len());
                        write_row_description(stream, &columns, &oids, result_formats)?;
                        for row in &rows {
                            write_data_row(stream, row, &oids, result_formats)?;
                        }
                        let tag = format!("SELECT {}\0", rows.len());
                        write_message(stream, b'C', tag.as_bytes())?;
                    }
                    QueryResult::Message(message) => {
                        let tag = format!("{}\0", command_tag(&message));
                        write_message(stream, b'C', tag.as_bytes())?;
                    }
                }
            }
            Ok(true)
        }
        Err(error) => {
            write_sql_error(stream, &error)?;
            Ok(false)
        }
    }
}

struct Prepared {
    sql: String,
    param_types: Vec<i32>,
}

struct Bound {
    sql: String,
    result_formats: Vec<i16>,
    /// Rows buffered for a paged `Execute` (`max_rows > 0`).
    paged: Option<PagedResult>,
}

/// Rows buffered for a paged `Execute`, with how many were already sent.
struct PagedResult {
    columns: Vec<String>,
    oids: Vec<i32>,
    rows: Vec<Vec<Value>>,
    sent: usize,
}

fn read_cstring(payload: &[u8], offset: usize) -> Result<(String, usize), PgError> {
    let end = payload[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| PgError::Protocol("unterminated string".to_string()))?
        + offset;
    Ok((
        String::from_utf8_lossy(&payload[offset..end]).to_string(),
        end + 1,
    ))
}

fn read_i16(payload: &[u8], offset: usize) -> Result<i16, PgError> {
    let bytes: [u8; 2] = payload
        .get(offset..offset + 2)
        .ok_or_else(|| PgError::Protocol("truncated int16".to_string()))?
        .try_into()
        .map_err(|_| PgError::Protocol("truncated int16".to_string()))?;
    Ok(i16::from_be_bytes(bytes))
}

fn read_i32(payload: &[u8], offset: usize) -> Result<i32, PgError> {
    let bytes: [u8; 4] = payload
        .get(offset..offset + 4)
        .ok_or_else(|| PgError::Protocol("truncated int32".to_string()))?
        .try_into()
        .map_err(|_| PgError::Protocol("truncated int32".to_string()))?;
    Ok(i32::from_be_bytes(bytes))
}

fn parse_prepared(
    stream: &mut dyn ReadWrite,
    prepared: &mut BTreeMap<String, Prepared>,
    payload: &[u8],
) -> Result<(), PgError> {
    let (name, cursor) = read_cstring(payload, 0)?;
    let (sql, cursor) = read_cstring(payload, cursor)?;
    let count = read_i16(payload, cursor)?;
    let mut cursor = cursor + 2;
    let mut param_types = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count.max(0) {
        param_types.push(read_i32(payload, cursor)?);
        cursor += 4;
    }
    let held: usize = prepared.values().map(|item| item.sql.len()).sum();
    if !prepared.contains_key(&name)
        && (prepared.len() >= MAX_PREPARED_STATEMENTS
            || held.saturating_add(sql.len()) > MAX_PREPARED_BYTES)
    {
        return Err(PgError::Protocol(format!(
            "too many prepared statements (limit {MAX_PREPARED_STATEMENTS})"
        )));
    }
    prepared.insert(name, Prepared { sql, param_types });
    write_message(stream, b'1', &[])
}

fn bind_portal(
    stream: &mut dyn ReadWrite,
    prepared: &BTreeMap<String, Prepared>,
    portals: &mut BTreeMap<String, Bound>,
    payload: &[u8],
) -> Result<(), PgError> {
    let (portal, cursor) = read_cstring(payload, 0)?;
    let (statement, mut cursor) = read_cstring(payload, cursor)?;
    let format_count = read_i16(payload, cursor)?;
    cursor += 2;
    let mut param_formats = Vec::with_capacity(format_count.max(0) as usize);
    for _ in 0..format_count.max(0) {
        param_formats.push(read_i16(payload, cursor)?);
        cursor += 2;
    }
    let param_count = read_i16(payload, cursor)?;
    cursor += 2;
    let prepared = prepared
        .get(&statement)
        .ok_or_else(|| PgError::Protocol(format!("unknown prepared statement {statement:?}")))?;
    let mut params = Vec::with_capacity(param_count.max(0) as usize);
    for index in 0..param_count.max(0) {
        let length = read_i32(payload, cursor)?;
        cursor += 4;
        if length < 0 {
            params.push(None);
            continue;
        }
        let length = length as usize;
        let value = payload
            .get(cursor..cursor + length)
            .ok_or_else(|| PgError::Protocol("truncated parameter".to_string()))?;
        cursor += length;
        let format = match param_formats.len() {
            0 => 0,
            1 => param_formats[0],
            _ => param_formats.get(index as usize).copied().unwrap_or(0),
        };
        let oid = prepared
            .param_types
            .get(index as usize)
            .copied()
            .unwrap_or(0);
        let text = if format == 1 {
            decode_binary_param(value, oid)?
        } else {
            String::from_utf8_lossy(value).to_string()
        };
        params.push(Some(text));
    }
    let sql = substitute_params(&prepared.sql, &prepared.param_types, &params)?;
    let result_format_count = read_i16(payload, cursor)?;
    cursor += 2;
    let mut result_formats = Vec::with_capacity(result_format_count.max(0) as usize);
    for _ in 0..result_format_count.max(0) {
        result_formats.push(read_i16(payload, cursor)?);
        cursor += 2;
    }
    if !portals.contains_key(&portal) && portals.len() >= MAX_PORTALS {
        return Err(PgError::Protocol(format!(
            "too many portals (limit {MAX_PORTALS})"
        )));
    }
    portals.insert(
        portal,
        Bound {
            sql,
            result_formats,
            paged: None,
        },
    );
    write_message(stream, b'2', &[])
}

fn describe(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    prepared: &BTreeMap<String, Prepared>,
    portals: &BTreeMap<String, Bound>,
    payload: &[u8],
) -> Result<(), PgError> {
    let kind = payload
        .first()
        .copied()
        .ok_or_else(|| PgError::Protocol("empty Describe".to_string()))?;
    let (name, _) = read_cstring(payload, 1)?;
    match kind {
        b'S' => {
            let prepared = prepared
                .get(&name)
                .ok_or_else(|| PgError::Protocol(format!("unknown prepared statement {name:?}")))?;
            let mut p = Vec::new();
            p.extend_from_slice(&(prepared.param_types.len() as i16).to_be_bytes());
            for oid in &prepared.param_types {
                p.extend_from_slice(&oid.to_be_bytes());
            }
            write_message(stream, b't', &p)?;
            describe_result(stream, engine, &prepared.sql, &prepared.param_types, &[])?;
        }
        b'P' => {
            let bound = portals
                .get(&name)
                .ok_or_else(|| PgError::Protocol(format!("unknown portal {name:?}")))?;
            describe_result(stream, engine, &bound.sql, &[], &bound.result_formats)?;
        }
        other => {
            return Err(PgError::Protocol(format!(
                "bad Describe kind {}",
                other as char
            )));
        }
    }
    Ok(())
}

/// Writes a `RowDescription` when the statement is a `SELECT`; otherwise
/// `NoData`.
fn describe_result(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    sql: &str,
    param_types: &[i32],
    result_formats: &[i16],
) -> Result<(), PgError> {
    match describe_columns(engine, sql, param_types) {
        Ok(Some(meta)) => write_row_description(stream, &meta.columns, &meta.oids, result_formats),
        Ok(None) => write_message(stream, b'n', &[]),
        // `describe_columns` already returns `PgError`; keep its message.
        Err(error) => write_error(stream, &error.to_string()),
    }
}

/// Result column names plus the PostgreSQL type OIDs inferred from a sample
/// row.
struct ResultMeta {
    columns: Vec<String>,
    oids: Vec<i32>,
}

/// Prepares a query with `LIMIT 1` to learn the result column names and types.
/// Unbound parameters are substituted with `NULL`.
fn describe_columns(
    engine: &mut SqlEngine,
    sql: &str,
    param_types: &[i32],
) -> Result<Option<ResultMeta>, PgError> {
    let placeholders = vec![None; param_types.len()];
    let sql = substitute_params(sql, param_types, &placeholders)?;
    // Fast path: bind only. `Describe` of a grouped query used to run
    // `SELECT ... LIMIT 1`, which aggregates the whole table.
    if let Some(columns) = engine.describe_select(&sql) {
        let names = columns.iter().map(|(name, _)| name.clone()).collect();
        let oids = columns
            .iter()
            .map(|(_, data_type)| match data_type {
                DataType::Int => 20,
                DataType::Bool => 16,
                _ => 25,
            })
            .collect();
        return Ok(Some(ResultMeta {
            columns: names,
            oids,
        }));
    }
    let statements = parse_script(&sql).map_err(|error| PgError::Sql(error.to_string()))?;
    let Some(Statement::Select(select)) = statements.first() else {
        return Ok(None);
    };
    let mut limited = select.clone();
    limited.limit = Some(1);
    match engine
        .execute_statement(&Statement::Select(limited))
        .map_err(|error| PgError::Sql(error.to_string()))?
    {
        QueryResult::Rows { columns, rows } => {
            let oids = column_oids(&rows, columns.len());
            Ok(Some(ResultMeta { columns, oids }))
        }
        QueryResult::Message(_) => Ok(None),
    }
}

/// Infers PostgreSQL type OIDs from the result rows: `INT` → `int8`(20),
/// `BOOL` → `bool`(16), everything else (including `NULL`-only columns) →
/// `text`(25).
fn column_oids(rows: &[Vec<Value>], column_count: usize) -> Vec<i32> {
    let mut oids = vec![25; column_count];
    let mut known = vec![false; column_count];
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            if known[index] || value.is_null() {
                continue;
            }
            oids[index] = match value {
                Value::Int(_) => 20,
                Value::Bool(_) => 16,
                _ => 25,
            };
            known[index] = true;
        }
    }
    oids
}

fn execute_portal(
    stream: &mut dyn ReadWrite,
    engine: &mut SqlEngine,
    portals: &mut BTreeMap<String, Bound>,
    payload: &[u8],
    tx_status: &mut TxStatus,
) -> Result<(), PgError> {
    let (portal, cursor) = read_cstring(payload, 0)?;
    let max_rows = read_i32(payload, cursor)?;
    let bound = portals
        .get_mut(&portal)
        .ok_or_else(|| PgError::Protocol(format!("unknown portal {portal:?}")))?;
    if *tx_status == TxStatus::Failed {
        write_error(
            stream,
            "current transaction is aborted, commands ignored until end of transaction block",
        )?;
        return Ok(());
    }
    if max_rows <= 0 {
        // One-shot execution: the previous behaviour.
        bound.paged = None;
        let success = execute_and_write(stream, engine, &bound.sql, &bound.result_formats)?;
        if !success && *tx_status == TxStatus::InTransaction {
            *tx_status = TxStatus::Failed;
        }
        return Ok(());
    }
    if bound.paged.is_none() {
        let results = match engine.execute_script(&bound.sql) {
            Ok(results) => results,
            Err(error) => {
                write_sql_error(stream, &error)?;
                bound.paged = None;
                if *tx_status == TxStatus::InTransaction {
                    *tx_status = TxStatus::Failed;
                }
                return Ok(());
            }
        };
        match results.into_iter().next() {
            Some(QueryResult::Rows { columns, rows }) => {
                let oids = column_oids(&rows, columns.len());
                bound.paged = Some(PagedResult {
                    columns,
                    oids,
                    rows,
                    sent: 0,
                });
            }
            Some(QueryResult::Message(message)) => {
                let tag = format!("{}\0", command_tag(&message));
                write_message(stream, b'C', tag.as_bytes())?;
                return Ok(());
            }
            None => {
                write_message(stream, b'C', b"SELECT 0\0")?;
                return Ok(());
            }
        }
    }
    let formats = bound.result_formats.clone();
    let mut finished = false;
    {
        let Some(paged) = bound.paged.as_mut() else {
            return Ok(());
        };
        let remaining = paged.rows.len() - paged.sent;
        let take = remaining.min(max_rows as usize);
        if paged.sent == 0 {
            write_row_description(stream, &paged.columns, &paged.oids, &formats)?;
        }
        for row in &paged.rows[paged.sent..paged.sent + take] {
            write_data_row(stream, row, &paged.oids, &formats)?;
        }
        paged.sent += take;
        if paged.sent < paged.rows.len() {
            // PortalSuspended: more rows are waiting for the next Execute.
            write_message(stream, b's', &[])?;
        } else {
            let tag = format!("SELECT {}\0", paged.rows.len());
            write_message(stream, b'C', tag.as_bytes())?;
            finished = true;
        }
    }
    if finished {
        bound.paged = None;
    }
    Ok(())
}

fn close_portal(
    stream: &mut dyn ReadWrite,
    prepared: &mut BTreeMap<String, Prepared>,
    portals: &mut BTreeMap<String, Bound>,
    payload: &[u8],
) -> Result<(), PgError> {
    let kind = payload
        .first()
        .copied()
        .ok_or_else(|| PgError::Protocol("empty Close".to_string()))?;
    let (name, _) = read_cstring(payload, 1)?;
    match kind {
        b'S' => {
            prepared.remove(&name);
        }
        b'P' => {
            portals.remove(&name);
        }
        other => {
            return Err(PgError::Protocol(format!(
                "bad Close kind {}",
                other as char
            )));
        }
    }
    write_message(stream, b'3', &[])
}

/// Decodes a binary-format parameter into the text form the SQL layer uses.
fn decode_binary_param(bytes: &[u8], oid: i32) -> Result<String, PgError> {
    match oid {
        16 => {
            let [flag] = bytes else {
                return Err(PgError::Protocol(
                    "binary bool parameter must be 1 byte".to_string(),
                ));
            };
            Ok(if *flag == 0 {
                "f".to_string()
            } else {
                "t".to_string()
            })
        }
        20 | 21 | 23 => {
            let value = match bytes.len() {
                2 => i16::from_be_bytes([bytes[0], bytes[1]]) as i64,
                4 => i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
                8 => i64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]),
                _ => {
                    return Err(PgError::Protocol(format!(
                        "binary integer parameter has {} bytes",
                        bytes.len()
                    )))
                }
            };
            Ok(value.to_string())
        }
        700 => {
            let value: [u8; 4] = bytes.try_into().map_err(|_| {
                PgError::Protocol("binary float4 parameter must be 4 bytes".to_string())
            })?;
            Ok(f32::from_be_bytes(value).to_string())
        }
        701 => {
            let value: [u8; 8] = bytes.try_into().map_err(|_| {
                PgError::Protocol("binary float8 parameter must be 8 bytes".to_string())
            })?;
            Ok(f64::from_be_bytes(value).to_string())
        }
        25 | 1043 | 0 => Ok(String::from_utf8_lossy(bytes).to_string()),
        1700 => decode_numeric_param(bytes),
        1082 => decode_date_param(bytes),
        1114 => decode_timestamp_param(bytes),
        2950 => decode_uuid_param(bytes),
        other => Err(PgError::Protocol(format!(
            "unsupported binary parameter type oid {other}"
        ))),
    }
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

/// PostgreSQL `date` binary format: days since 2000-01-01.
fn decode_date_param(bytes: &[u8]) -> Result<String, PgError> {
    let days: [u8; 4] = bytes
        .try_into()
        .map_err(|_| PgError::Protocol("binary date parameter must be 4 bytes".to_string()))?;
    let days = i32::from_be_bytes(days) + 10_957;
    let (year, month, day) = civil_from_days(days);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// PostgreSQL `timestamp` binary format: microseconds since 2000-01-01.
fn decode_timestamp_param(bytes: &[u8]) -> Result<String, PgError> {
    let micros: [u8; 8] = bytes
        .try_into()
        .map_err(|_| PgError::Protocol("binary timestamp parameter must be 8 bytes".to_string()))?;
    let micros = i64::from_be_bytes(micros) + 10_957 * 86_400_000_000;
    let days = micros.div_euclid(86_400_000_000) as i32;
    let remainder = micros.rem_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days);
    let seconds = remainder / 1_000_000;
    let fraction = remainder % 1_000_000;
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    if fraction == 0 {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        ))
    } else {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{fraction:06}"
        ))
    }
}

/// PostgreSQL `uuid` binary format: 16 raw bytes.
fn decode_uuid_param(bytes: &[u8]) -> Result<String, PgError> {
    if bytes.len() != 16 {
        return Err(PgError::Protocol(
            "binary uuid parameter must be 16 bytes".to_string(),
        ));
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// PostgreSQL `numeric` binary format: base-10000 digits.
fn decode_numeric_param(bytes: &[u8]) -> Result<String, PgError> {
    if bytes.len() < 8 || !bytes.len().is_multiple_of(2) {
        return Err(PgError::Protocol(
            "bad binary numeric parameter".to_string(),
        ));
    }
    let ndigits = i16::from_be_bytes([bytes[0], bytes[1]]);
    let weight = i16::from_be_bytes([bytes[2], bytes[3]]);
    let sign = u16::from_be_bytes([bytes[4], bytes[5]]);
    let dscale = i16::from_be_bytes([bytes[6], bytes[7]]);
    match sign {
        0xC000 => return Ok("NaN".to_string()),
        0xD000 => return Ok("Infinity".to_string()),
        0xF000 => return Ok("-Infinity".to_string()),
        _ => {}
    }
    if ndigits < 0 || dscale < 0 {
        return Err(PgError::Protocol("bad binary numeric header".to_string()));
    }
    let ndigits = ndigits as usize;
    if bytes.len() != 8 + ndigits * 2 {
        return Err(PgError::Protocol(
            "binary numeric length mismatch".to_string(),
        ));
    }
    let mut digits = Vec::with_capacity(ndigits);
    for index in 0..ndigits {
        let at = 8 + index * 2;
        digits.push(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
    }
    // Every base-10000 digit is four decimal places wide.
    let mut raw = String::with_capacity(ndigits * 4);
    for digit in &digits {
        raw.push_str(&format!("{digit:04}"));
    }
    let point = (weight as i64 + 1) * 4;
    let mut integer = String::new();
    let mut fraction = String::new();
    if point <= 0 {
        integer.push('0');
        for _ in 0..(-point) {
            fraction.push('0');
        }
        fraction.push_str(&raw);
    } else if point as usize >= raw.len() {
        integer.push_str(&raw);
        for _ in 0..(point as usize - raw.len()) {
            integer.push('0');
        }
    } else {
        integer.push_str(&raw[..point as usize]);
        fraction.push_str(&raw[point as usize..]);
    }
    let trimmed = integer.trim_start_matches('0');
    let integer = if trimmed.is_empty() { "0" } else { trimmed };
    let mut text = String::new();
    if sign == 0x4000 {
        text.push('-');
    }
    text.push_str(integer);
    if dscale > 0 {
        fraction.truncate(dscale as usize);
        while fraction.len() < dscale as usize {
            fraction.push('0');
        }
        text.push('.');
        text.push_str(&fraction);
    }
    Ok(text)
}

fn substitute_params(
    sql: &str,
    param_types: &[i32],
    params: &[Option<String>],
) -> Result<String, PgError> {
    let mut output = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut block_depth = 0usize;
    while let Some(character) = chars.next() {
        if in_line_comment {
            output.push(character);
            if character == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if block_depth > 0 {
            output.push(character);
            if character == '/' && chars.peek() == Some(&'*') {
                chars.next();
                output.push('*');
                block_depth += 1;
            } else if character == '*' && chars.peek() == Some(&'/') {
                chars.next();
                output.push('/');
                block_depth -= 1;
            }
            continue;
        }
        if in_string {
            output.push(character);
            if character == '\'' {
                if chars.peek() == Some(&'\'') {
                    // `''` is an escaped quote, not the end of the literal.
                    chars.next();
                    output.push('\'');
                } else {
                    in_string = false;
                }
            }
            continue;
        }
        if character == '\'' {
            in_string = true;
            output.push(character);
            continue;
        }
        if character == '-' && chars.peek() == Some(&'-') {
            chars.next();
            output.push_str("--");
            in_line_comment = true;
            continue;
        }
        if character == '/' && chars.peek() == Some(&'*') {
            chars.next();
            output.push_str("/*");
            block_depth = 1;
            continue;
        }
        if character == '$' {
            let mut digits = String::new();
            while let Some(next) = chars.peek() {
                if next.is_ascii_digit() {
                    digits.push(*next);
                    chars.next();
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                output.push('$');
                continue;
            }
            let index: usize = digits
                .parse()
                .map_err(|_| PgError::Protocol("bad parameter index".to_string()))?;
            if index == 0 || index > params.len() {
                return Err(PgError::Protocol(format!(
                    "parameter ${index} out of range ({} supplied)",
                    params.len()
                )));
            }
            let oid = param_types.get(index - 1).copied().unwrap_or(0);
            let literal = param_literal(&params[index - 1], oid)?;
            if output.len().saturating_add(literal.len()) > MAX_SUBSTITUTED_SQL {
                return Err(PgError::Protocol(format!(
                    "substituted SQL exceeds {MAX_SUBSTITUTED_SQL} bytes"
                )));
            }
            output.push_str(&literal);
            continue;
        }
        output.push(character);
    }
    Ok(output)
}

fn param_literal(value: &Option<String>, oid: i32) -> Result<String, PgError> {
    let Some(text) = value else {
        return Ok("NULL".to_string());
    };
    match oid {
        20 | 21 | 23 => text
            .parse::<i64>()
            .map(|_| text.clone())
            .map_err(|_| PgError::Protocol(format!("invalid integer parameter {text:?}"))),
        16 => Ok(if text == "t" || text.eq_ignore_ascii_case("true") {
            "TRUE".to_string()
        } else {
            "FALSE".to_string()
        }),
        25 | 1043 => Ok(format!("'{}'", text.replace('\'', "''"))),
        _ => Ok(if text.parse::<i64>().is_ok() {
            text.clone()
        } else if text.eq_ignore_ascii_case("true") || text == "t" {
            "TRUE".to_string()
        } else if text.eq_ignore_ascii_case("false") || text == "f" {
            "FALSE".to_string()
        } else {
            format!("'{}'", text.replace('\'', "''"))
        }),
    }
}

fn write_row_description(
    stream: &mut dyn ReadWrite,
    columns: &[String],
    oids: &[i32],
    formats: &[i16],
) -> Result<(), PgError> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for (index, name) in columns.iter().enumerate() {
        let oid = oids.get(index).copied().unwrap_or(25);
        let size: i16 = match oid {
            20 => 8, // int8
            16 => 1, // bool
            _ => -1, // text
        };
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&0i32.to_be_bytes()); // table oid
        payload.extend_from_slice(&0i16.to_be_bytes()); // attribute number
        payload.extend_from_slice(&oid.to_be_bytes());
        payload.extend_from_slice(&size.to_be_bytes());
        payload.extend_from_slice(&(-1i32).to_be_bytes()); // type modifier
        payload.extend_from_slice(&format_for_column(formats, index).to_be_bytes());
    }
    write_message(stream, b'T', &payload)
}

/// PostgreSQL result format rule: zero codes means text, one code applies to
/// every column, otherwise each column has its own code.
fn format_for_column(formats: &[i16], index: usize) -> i16 {
    match formats.len() {
        0 => 0,
        1 => formats[0],
        _ => formats.get(index).copied().unwrap_or(0),
    }
}

fn value_binary(value: &Value, oid: i32) -> Vec<u8> {
    match (oid, value) {
        (20, Value::Int(number)) => number.to_be_bytes().to_vec(),
        (16, Value::Bool(flag)) => vec![u8::from(*flag)],
        _ => value_text(value).into_bytes(),
    }
}

fn write_data_row(
    stream: &mut dyn ReadWrite,
    row: &[Value],
    oids: &[i32],
    formats: &[i16],
) -> Result<(), PgError> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(row.len() as i16).to_be_bytes());
    for (index, value) in row.iter().enumerate() {
        if value.is_null() {
            payload.extend_from_slice(&(-1i32).to_be_bytes());
            continue;
        }
        if format_for_column(formats, index) == 1 {
            let bytes = value_binary(value, oids.get(index).copied().unwrap_or(25));
            payload.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            payload.extend_from_slice(&bytes);
        } else {
            let text = value_text(value);
            payload.extend_from_slice(&(text.len() as i32).to_be_bytes());
            payload.extend_from_slice(text.as_bytes());
        }
    }
    write_message(stream, b'D', &payload)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Int(value) => value.to_string(),
        Value::Text(value) => value.clone(),
        Value::Bool(value) => {
            if *value {
                "t".to_string()
            } else {
                "f".to_string()
            }
        }
        Value::Logic(value) => value.as_str().to_string(),
        Value::Lukasiewicz(value) => format!("{:.3}", f64::from(*value) / 1000.0),
        Value::Nat(value) => value.to_string(),
    }
}

fn write_error(stream: &mut dyn ReadWrite, message: &str) -> Result<(), PgError> {
    write_error_fields(stream, "XX000", message, None)
}

/// ErrorResponse with an explicit SQLSTATE and optional hint.
///
/// The `C` (SQLSTATE) and `H` (hint) fields are what a client or an agent can
/// branch on without parsing prose: `42703` means "fix the column name",
/// `42P01` means "fix the table name", `23505` means "pick another value".
fn write_error_fields(
    stream: &mut dyn ReadWrite,
    code: &str,
    message: &str,
    hint: Option<&str>,
) -> Result<(), PgError> {
    let mut payload = Vec::new();
    payload.push(b'S');
    payload.extend_from_slice(b"ERROR\0");
    payload.push(b'V');
    payload.extend_from_slice(b"ERROR\0");
    payload.push(b'C');
    payload.extend_from_slice(code.as_bytes());
    payload.push(0);
    payload.push(b'M');
    payload.extend_from_slice(message.as_bytes());
    payload.push(0);
    if let Some(hint) = hint {
        payload.push(b'H');
        payload.extend_from_slice(hint.as_bytes());
        payload.push(0);
    }
    payload.push(0);
    write_message(stream, b'E', &payload)
}

/// Writes a SQL error with its SQLSTATE and repair hint.
fn write_sql_error(stream: &mut dyn ReadWrite, error: &SqlError) -> Result<(), PgError> {
    write_error_fields(
        stream,
        error.code(),
        &error.to_string(),
        error.hint().as_deref(),
    )
}

/// Maps an engine acknowledgement to a PostgreSQL command tag.
fn command_tag(message: &str) -> String {
    if message.starts_with("inserted ") {
        let count = message
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        return format!("INSERT 0 {count}");
    }
    if message.starts_with("updated ") {
        let count = message
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        return format!("UPDATE {count}");
    }
    if message.starts_with("deleted ") {
        let count = message
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        return format!("DELETE {count}");
    }
    if message.starts_with("created table") {
        return "CREATE TABLE".to_string();
    }
    if message.starts_with("dropped table") {
        return "DROP TABLE".to_string();
    }
    if message.starts_with("added column") || message.starts_with("dropped column") {
        return "ALTER TABLE".to_string();
    }
    if message.starts_with("renamed") || message.starts_with("altered column") {
        return "ALTER TABLE".to_string();
    }
    if message.starts_with("analyzed") {
        return "ANALYZE".to_string();
    }
    "OK".to_string()
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn random_csv_lines_never_panic() {
        let options = CopyOptions::defaults(CopyFormat::Csv);
        let mut state = 0x1234_5678u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let len = (next() % 200) as usize;
            let line: String = (0..len)
                .map(|_| match next() % 6 {
                    0 => '"',
                    1 => ',',
                    2 => '\\',
                    3 => '\n',
                    4 => 'x',
                    _ => '中',
                })
                .collect();
            let _ = parse_csv_line(&line, &options);
        }
    }

    #[test]
    fn copy_decoding_rejects_incomplete_sequences() {
        assert!(decode_copy_line(&[0x41, 0x00, 0x42], "UTF16LE").is_err());
        assert!(decode_copy_line(&[0x00, 0xD8], "UTF16LE").is_err());
        assert_eq!(
            decode_copy_line(&[0x41, 0x00], "UTF16LE").expect("utf16"),
            "A"
        );
        assert!(decode_copy_line(&[0xFF], "ASCII").is_err());
        assert!(decode_copy_line(&[0xFF], "GBK").is_err());
        assert!(decode_copy_line(&[0xFF], "UTF8").is_err());
        assert_eq!(decode_copy_line(b"ok", "UTF8").expect("utf8"), "ok");
    }

    #[test]
    fn copy_literal_quotes_hostile_text() {
        assert_eq!(copy_literal("a'b", DataType::Text), "'a''b'");
        assert_eq!(
            copy_literal("1; DROP TABLE t", DataType::Int),
            "'1; DROP TABLE t'"
        );
        assert_eq!(copy_literal("42", DataType::Int), "42");
        assert_eq!(copy_literal("1e999", DataType::Int), "'1e999'");
        assert_eq!(copy_literal("1.5", DataType::Int), "'1.5'");
        assert_eq!(
            copy_literal("18446744073709551615", DataType::Nat),
            "18446744073709551615"
        );
        assert_eq!(copy_literal("TRUE", DataType::Bool), "TRUE");
        assert_eq!(copy_literal("a\\b", DataType::Text), "'a\\b'");
    }

    #[test]
    fn unterminated_csv_quotes_are_rejected() {
        let options = CopyOptions::defaults(CopyFormat::Csv);
        let (fields, unterminated) = parse_csv_line("\"abc", &options);
        assert!(unterminated, "a quote that never closes must be reported");
        assert_eq!(fields.len(), 1);
        let (fields, unterminated) = parse_csv_line("\"abc\",def", &options);
        assert!(!unterminated);
        assert_eq!(fields.len(), 2);
    }

    struct Sink(Vec<u8>);

    impl Read for Sink {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for Sink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn substitution_amplification_is_capped() {
        // 20k copies of $1 times a 1 KB literal used to build a 20 MB string
        // from a 40 KB statement; the cap rejects it mid-substitution.
        let sql = "$1".repeat(20_000);
        let params = vec![Some("x".repeat(1000))];
        let error =
            substitute_params(&sql, &[25], &params).expect_err("amplified SQL must be rejected");
        assert!(error.to_string().contains("substituted SQL"), "{error}");
    }

    #[test]
    fn prepared_statement_count_is_capped() {
        let mut stream = Sink(Vec::new());
        let mut prepared = BTreeMap::new();
        for index in 0..MAX_PREPARED_STATEMENTS {
            let mut payload = Vec::new();
            payload.extend_from_slice(format!("s{index}").as_bytes());
            payload.push(0);
            payload.extend_from_slice(b"SELECT 1");
            payload.push(0);
            payload.extend_from_slice(&0i16.to_be_bytes());
            parse_prepared(&mut stream, &mut prepared, &payload).expect("parse");
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(b"overflow");
        payload.push(0);
        payload.extend_from_slice(b"SELECT 1");
        payload.push(0);
        payload.extend_from_slice(&0i16.to_be_bytes());
        let error = parse_prepared(&mut stream, &mut prepared, &payload)
            .expect_err("the statement cap must reject this Parse");
        assert!(
            error.to_string().contains("too many prepared statements"),
            "{error}"
        );
    }

    #[test]
    fn engine_lock_wait_times_out() {
        let dir = std::env::temp_dir().join(format!(
            "tritdb-pgwire-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let engine = Arc::new(Mutex::new(
            SqlEngine::open(&dir.join("lock.db")).expect("open"),
        ));
        let held = Arc::clone(&engine);
        let guard = held.lock().expect("lock");

        let start = Instant::now();
        let error = match lock_engine(&engine, Some(Duration::from_millis(50))) {
            Ok(_) => panic!("the lock is held, so the wait must time out"),
            Err(error) => error,
        };
        assert!(
            start.elapsed() >= Duration::from_millis(50),
            "returned before the timeout elapsed"
        );
        assert!(error.to_string().contains("timed out"), "{error}");

        drop(guard);
        drop(engine);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn constant_time_eq_rejects_length_and_value_mismatches() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn integer_parameter_values_must_be_numbers() {
        // A client can declare an int8 parameter and send SQL text.
        let result = substitute_params("SELECT $1", &[23], &[Some("1; DROP TABLE t".into())]);
        assert!(
            result.is_err(),
            "integer parameter accepted SQL text: {result:?}"
        );
        // A real integer still substitutes.
        assert_eq!(
            substitute_params("SELECT $1", &[23], &[Some("7".into())]).unwrap(),
            "SELECT 7"
        );
    }

    #[test]
    fn copy_table_names_cannot_carry_statements() {
        let plan = parse_copy("COPY t; DROP TABLE x FROM STDIN");
        assert!(
            plan.is_none() || !plan.unwrap().table.contains(';'),
            "COPY table name leaked a statement"
        );
    }

    #[test]
    fn substitution_skips_comments_and_strings() {
        let params = vec![Some("7".to_string())];
        let types = vec![23];
        assert_eq!(
            substitute_params("SELECT $1 -- $1\n, '$1' /* $1 */", &types, &params)
                .expect("substitute"),
            "SELECT 7 -- $1\n, '$1' /* $1 */"
        );
        assert_eq!(
            substitute_params("SELECT /* a /* b */ $1 */ $1", &types, &params)
                .expect("nested block comment"),
            "SELECT /* a /* b */ $1 */ 7"
        );
        assert_eq!(
            substitute_params("SELECT '$1''$1', $1", &types, &params).expect("escaped quote"),
            "SELECT '$1''$1', 7"
        );
    }

    #[test]
    fn parameter_substitution_respects_escaped_quotes() {
        let sql = substitute_params("SELECT 'it''s', $1", &[23], &[Some("7".into())])
            .expect("substitute");
        assert!(sql.ends_with(", 7"), "escaped quotes confused $1: {sql}");
    }
}
