//! 3DGS binary_little_endian PLY 解析。
//!
//! 按属性名建列映射（不假设固定排布），必需属性：
//! `x/y/z`、`f_dc_0..2`、`opacity`、`scale_0..2`、`rot_0..3`；
//! `f_rest_0..N` 可缺（无视角色，退化为 DC）。
//! 已知坑（见 depth-contract/c1/convert_to_spz.py 记账）：gaussforge 回转的
//! PLY 头**没有 `end_header`**——解析器在遇到非头语法行时也停。

use crate::SplatError;

mod model;

pub use model::{RawGaussian, activate};

const REQUIRED: [&str; 12] = [
    "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
    "rot_0", "rot_1",
];

/// 解析 binary_little_endian PLY 的头部，返回 (顶点数, 属性名列表, 数据段起始偏移)。
fn parse_header(bytes: &[u8]) -> Result<(usize, Vec<String>, usize), SplatError> {
    let text_end = bytes
        .windows(11)
        .position(|w| w == b"end_header\n")
        .map(|p| p + 11);
    let (head_len, head_text) = match text_end {
        Some(end) => (end, String::from_utf8_lossy(&bytes[..end]).into_owned()),
        None => {
            // gaussforge 回转 ply：无 end_header，属性列表后直接是二进制。
            // 逐行扫，遇到第一个非 "ply/format/comment/property/element" 语法行即停。
            let mut offset = 0usize;
            let mut head = String::new();
            loop {
                // 找不到 \n 时把剩余全部当末行（二进制段可能不含换行）
                let line_end = match bytes[offset..].iter().position(|&b| b == b'\n') {
                    Some(p) => offset + p + 1,
                    None => bytes.len(),
                };
                let line = String::from_utf8_lossy(&bytes[offset..line_end]).into_owned();
                let first = line.split_whitespace().next().unwrap_or("");
                let is_header_line =
                    matches!(first, "ply" | "format" | "comment" | "property" | "element");
                if !is_header_line {
                    break;
                }
                head.push_str(&line);
                offset = line_end;
                if head.contains("element face") {
                    break; // 面元素之后的行不再属于头部
                }
            }
            (offset, head)
        }
    };

    let mut vertex_count = 0usize;
    let mut props: Vec<String> = Vec::new();
    let mut in_vertex = false;
    for line in head_text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.first().copied() {
            Some("format") => {
                if tokens.get(1) != Some(&"binary_little_endian") {
                    return Err(SplatError::UnsupportedFormat(
                        tokens.get(1).copied().unwrap_or("?").into(),
                    ));
                }
            }
            Some("element") => {
                in_vertex = tokens.get(1) == Some(&"vertex");
                if in_vertex {
                    vertex_count = tokens
                        .get(2)
                        .and_then(|v| v.parse().ok())
                        .ok_or(SplatError::Transforms("element vertex 数非法".into()))?;
                }
            }
            Some("property") if in_vertex => {
                // property <type> <name>；float 每列 4 字节
                props.push(tokens.get(2).copied().unwrap_or("").into());
            }
            _ => {}
        }
    }
    for name in REQUIRED {
        if !props.iter().any(|p| p == name) {
            return Err(SplatError::MissingProperty(name.into()));
        }
    }
    Ok((vertex_count, props, head_len))
}

/// 解析全部顶点。rot_2/rot_3 允许缺失（伪 rot 兼容）。
pub fn parse_ply(bytes: &[u8]) -> Result<Vec<RawGaussian>, SplatError> {
    let (count, props, data_start) = parse_header(bytes)?;
    let stride = props.len() * 4;
    let need = data_start + count * stride;
    if bytes.len() < need {
        return Err(SplatError::TruncatedData {
            need,
            got: bytes.len(),
        });
    }
    let index_of = |name: &str| props.iter().position(|p| p == name);
    let col = |name: &str| index_of(name);
    let i = |name: &str| col(name).unwrap();

    let mut out = Vec::with_capacity(count);
    for v in 0..count {
        let base = data_start + v * stride;
        let f = |k: usize| -> f32 {
            let o = base + k * 4;
            f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        };
        // f_rest 列在 PLY 头里可能是字典序（实测 Brush v0.3.0 导出：
        // f_rest_0, f_rest_1, f_rest_10, ...），必须按数字后缀重排，
        // 否则 SH 系数全部错位。
        let mut rest_cols: Vec<usize> = props
            .iter()
            .enumerate()
            .filter(|(_k, p)| p.starts_with("f_rest_"))
            .map(|(k, _p)| k)
            .collect();
        rest_cols.sort_by_key(|k| {
            props[*k]
                .strip_prefix("f_rest_")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(usize::MAX)
        });
        let rot_w = col("rot_3");
        out.push(RawGaussian {
            position: [f(i("x")), f(i("y")), f(i("z"))],
            f_dc: [f(i("f_dc_0")), f(i("f_dc_1")), f(i("f_dc_2"))],
            f_rest: rest_cols.iter().map(|k| f(*k)).collect(),
            opacity_logit: f(i("opacity")),
            scale_log: [f(i("scale_0")), f(i("scale_1")), f(i("scale_2"))],
            rot_xyzw: [
                f(i("rot_0")),
                f(i("rot_1")),
                f(i("rot_2")),
                rot_w.map(f).unwrap_or(0.0),
            ],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
