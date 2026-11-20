//! GGUF 读取分配预算：把 `with_capacity` 前的上限校验拆出 `lib.rs`，
//! 既守住单文件行数预算，也让预算口径集中在同一处。

use crate::error::Error;

/// 单文件张量数上限：与 `trit-texture` 的 GGUF 导入器同一预算口径。
const MAX_TENSORS: usize = 100_000;

/// 在 `Vec::with_capacity(tensor_count)` 之前拒绝恶意计数。
pub(super) fn check(tensor_count: usize) -> Result<(), Error> {
    if tensor_count > MAX_TENSORS {
        return Err(Error::TooManyTensors(tensor_count));
    }
    Ok(())
}
