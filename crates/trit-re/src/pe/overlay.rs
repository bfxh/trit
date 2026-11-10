//! 叠加段（overlay）计算。
//!
//! 独立成文件是为了守住 800 行预算：`pe.rs` 在本分支上已到 825 行。

use crate::entropy;

use super::{Overlay, Section};

/// 计算叠加段：最后一个节区之后、且不属于 Authenticode 证书的字节。
///
/// 证书按规范位于所有节区之后，所以「节区末尾到文件末尾」这一段里装的通常正是它。
/// 不排除它时每个已签名二进制都会得到一个高熵叠加段；而 `packed` 假设恰好把
/// `authenticode` 当作良性解释，于是每个签名程序都落进四值 `B`（冲突）而不是
/// `F`。实测 `C:\Windows\System32` 全量：2990 个带叠加段的文件全部是证书，
/// 排除后只剩 2 个真命中。
pub(super) fn parse_overlay(
    bytes: &[u8],
    sections: &[Section],
    headers_size: u32,
    security_dir: (u32, u32),
) -> Option<Overlay> {
    let mut end = if headers_size > 0 {
        headers_size as usize
    } else {
        0
    };
    for section in sections {
        if section.raw_ptr == 0 {
            continue;
        }
        let section_end = (section.raw_ptr as usize).saturating_add(section.raw_size as usize);
        end = end.max(section_end);
    }
    if bytes.len() <= end || end < 0x40 {
        return None;
    }

    let mut start = end;
    let mut tail = bytes.len();
    let (cert_offset, cert_size) = security_dir;
    if cert_size > 8 {
        let cert_start = cert_offset as usize;
        let cert_end = cert_start.saturating_add(cert_size as usize);
        // 只在证书确实落在节区之后、且完整存在于文件内时才排除它；否则保持原行为，
        // 避免把畸形头解读成「没有叠加段」。
        if cert_start >= end && cert_end <= bytes.len() {
            if cert_end == bytes.len() {
                // 证书位于文件末尾（正常布局）：截掉即可。
                tail = cert_start;
            } else {
                // 证书之后还有数据：那段尾巴本身就是可疑项，取它。
                start = cert_end;
            }
        }
    }
    if tail <= start {
        return None;
    }
    let overlay = &bytes[start..tail];
    Some(Overlay {
        offset: start,
        size: overlay.len(),
        entropy: entropy::shannon(overlay),
    })
}
