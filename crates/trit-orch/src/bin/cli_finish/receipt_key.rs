use std::{env, fs};

use trit_orch::Error;

pub(super) const KEY_FILE_ENV: &str = "TRIT_ORCH_RECEIPT_HMAC_KEY_FILE";

fn key_error(path: &str, message: impl std::fmt::Display) -> Error {
    Error::ReceiptExecution {
        task: "receipt-hmac-key".to_owned(),
        message: format!("cannot read HMAC key at {path}: {message}"),
    }
}

/// 读取 `--receipt-hmac-key-file <path>`；flag 出现但没带值按不可读拒绝。
pub(super) fn flag_path(flag: &str) -> Result<Option<String>, Error> {
    let mut args = env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args
                .next()
                .map(Some)
                .map(Ok)
                .unwrap_or_else(|| Err(key_error(flag, "missing key file path")));
        }
    }
    Ok(None)
}

/// 读取宿主 HMAC 密钥：显式参数优先，其次环境变量；都没给表示不启用签名。
/// 密钥文件按原始字节读取，只忽略末尾一个 CRLF/LF；少于 16 字节拒绝。
pub(super) fn load(flag_path: Option<String>) -> Result<Option<Vec<u8>>, Error> {
    let path = match flag_path {
        Some(path) => path,
        None => match env::var(KEY_FILE_ENV) {
            Ok(path) if !path.trim().is_empty() => path,
            _ => return Ok(None),
        },
    };
    let bytes = fs::read(&path).map_err(|error| key_error(&path, error))?;
    let key = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(&bytes);
    if key.len() < 16 {
        return Err(key_error(&path, "key must be at least 16 bytes"));
    }
    Ok(Some(key.to_vec()))
}
