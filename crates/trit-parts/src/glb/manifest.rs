//! 清单（manifest）读取：从固定形状的 JSON 里取值，并约束取出来的路径。
//!
//! WHY 手写而不用 serde：这里只取几项，且清单由本仓自己的打包步骤写出。但手写
//! 解析的边界必须自己守 —— 下面每个取值函数都只返回 `Option` / `Vec`，缺项就当
//! 没有，绝不在外部文本上 panic。
//!
//! WHY 路径要过 `resolve`：清单里的 `obj` / `png` / `path` 是外部文本，`../` 能让
//! 打包器去读包外的任意文件，再把它原样塞进输出的 `.glb`（第三方资源包场景下
//! 这就是一条信息外带通道）。

pub(super) use super::path::resolve;

pub(super) fn field(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub(super) fn number(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

pub(super) fn nums(txt: &str, key: &str) -> Vec<f64> {
    let pat = format!("\"{key}\": [");
    let Some(i) = txt.find(&pat) else {
        return Vec::new();
    };
    let rest = &txt[i + pat.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

pub(super) fn object_block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let mut depth = 1usize;
    let (mut in_str, mut esc) = (false, false);
    for (j, ch) in rest.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
}

pub(super) fn read(p: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

pub(super) struct MeshEntry {
    pub(super) id: u32,
    pub(super) obj: String,
    pub(super) png: String,
}

pub(super) fn mesh_entries(txt: &str) -> Vec<MeshEntry> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        out.push(MeshEntry {
            id: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            obj: field(seg, "obj").unwrap_or_default(),
            png: field(seg, "png").unwrap_or_default(),
        });
    }
    out
}
