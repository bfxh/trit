use std::io::Read;

/// 单条执行命令的 stdout 保留上限；超出部分继续 drain（避免管道阻塞），但不入内存。
pub(super) const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// 读取子进程输出；超过上限继续读空，但不保留。
pub(super) fn read_capped(reader: &mut impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if kept.len() < MAX_OUTPUT_BYTES {
                    let room = MAX_OUTPUT_BYTES - kept.len();
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
    }
    kept
}
