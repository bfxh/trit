/// GLB 结构：magic/版本/总长/两个分块；返回 (JSON 文本, BIN 切片)。
pub(super) fn chunks(b: &[u8]) -> Result<(String, &[u8]), String> {
    if b.len() < 20 || &b[0..4] != b"glTF" {
        return Err("不是 GLB".into());
    }
    let ver = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let total = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    if ver != 2 || total != b.len() {
        return Err(format!("版本/长度不对：v{ver} / {total} vs {}", b.len()));
    }
    let jl = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
    // `jl` 来自文件头：32 位 usize 上 `20 + jl + 8` 会绕过检查（绕成小值），必须先做检查加法。
    if &b[16..20] != b"JSON" || jl.checked_add(28).is_none_or(|e| e > b.len()) {
        return Err("JSON 分块坏".into());
    }
    let json = String::from_utf8_lossy(&b[20..20 + jl]).to_string();
    let bo = 20 + jl;
    let bl = u32::from_le_bytes([b[bo], b[bo + 1], b[bo + 2], b[bo + 3]]) as usize;
    let end = jl.checked_add(bl).and_then(|n| n.checked_add(28));
    if &b[bo + 4..bo + 8] != b"BIN\0" || end.is_none_or(|e| e != b.len()) {
        return Err("BIN 分块坏".into());
    }
    Ok((json, &b[bo + 8..]))
}

pub(super) fn png_count(bin: &[u8]) -> usize {
    bin.windows(4)
        .filter(|w| *w == [0x89, b'P', b'N', b'G'])
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_and_structure_checks() {
        let part = trit_parts::glb::Part {
            name: "obj-1".into(),
            mesh: trit_parts::mesh::Mesh {
                verts: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                tris: vec![[0, 1, 2]],
            },
            uv: vec![[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
            png: vec![0x89, b'P', b'N', b'G'],
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let b = trit_parts::glb::build(&[part], "t");
        let (json, bin) = chunks(&b).unwrap();
        assert!(json.contains("\"meshes\": ["));
        assert_eq!(json.matches("\"mesh\": ").count(), 1);
        assert_eq!(png_count(bin), 1);
        assert!(chunks(&b[..10]).is_err());
        let mut bad = b.clone();
        bad[8] ^= 1; // 总长与文件长度不符
        assert!(chunks(&bad).is_err());
    }

    /// 文件头里的 `jl` 是可构造的：让它逼近 usize 上限，检查加法必须先 overflow-safe。
    #[test]
    fn chunks_rejects_overflowing_json_length() {
        let mut b = vec![0u8; 20];
        b[0..4].copy_from_slice(b"glTF");
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        b[8..12].copy_from_slice(&20u32.to_le_bytes());
        b[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        b[16..20].copy_from_slice(b"JSON");
        assert!(chunks(&b).is_err(), "jl 逼近 usize 上限时必须拒绝");
    }
}
