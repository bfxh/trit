use std::io::Read;

use trit_orch::Error;

/// 单份输入 JSON 的上限；本地宿主的一次快照/计划远小于这个量级。
pub(crate) const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn read_stdin() -> Result<String, Error> {
    let mut input = String::new();
    let mut stdin = std::io::stdin().take(MAX_INPUT_BYTES + 1);
    stdin
        .read_to_string(&mut input)
        .map_err(|error| Error::InputParse {
            message: error.to_string(),
        })?;
    if input.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::InputTooLarge {
            size: input.len() as u64,
            max: MAX_INPUT_BYTES,
        });
    }
    Ok(input)
}
