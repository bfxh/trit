//! 帧头与尺寸硬顶：超长头行 / 超大 `Content-Length` 在分配前拒绝（不为无界输入撑内存）。

use std::io::{self, BufRead, Read};

/// 单条帧头行的字节上限：LSP 头都很小，超长头在分配前拒绝（不为无界头行撑内存）。
pub const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
/// 单帧帧体字节上限：`Content-Length` 声明值超过它在分配前拒绝。
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// 帧体尺寸门（纯函数，不分配）：声明长度在硬顶内才放行。
pub(super) fn frame_len_ok(len: usize) -> bool {
    len <= MAX_FRAME_BYTES
}

/// 读一条帧头行；EOF 返回 `Ok(None)`。最多读 `MAX_HEADER_LINE_BYTES + 1` 字节——
/// 足以判断超限，又不会为超长头行分配无界内存。
pub(super) fn read_header_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n =
        Read::take(&mut *reader, (MAX_HEADER_LINE_BYTES + 1) as u64).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_HEADER_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "帧头行超过上限（8 KiB）",
        ));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("帧头不是 UTF-8：{e}")))
}
