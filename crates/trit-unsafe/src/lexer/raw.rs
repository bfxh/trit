//! 原始字符串起止识别（拆出 scrub.rs 守模块行数）。

pub(super) fn raw_string_start(bytes: &[char], index: usize) -> Option<(usize, usize)> {
    if bytes.get(index) != Some(&'r') {
        return None;
    }
    let mut cursor = index + 1;
    let mut hashes = 0usize;
    while bytes.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
        if hashes > 32 {
            return None;
        }
    }
    if bytes.get(cursor) == Some(&'"') {
        Some((hashes, cursor + 1))
    } else {
        None
    }
}

pub(super) fn raw_string_ends(bytes: &[char], index: usize, hashes: usize) -> bool {
    if bytes.get(index) != Some(&'"') {
        return false;
    }
    (1..=hashes).all(|offset| bytes.get(index + offset) == Some(&'#'))
}

#[cfg(test)]
mod tests;
