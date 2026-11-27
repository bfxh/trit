//! 3DGS binary_little_endian PLY 解析。
//!
//! 按属性名建列映射（不假设固定排布），必需属性：
//! `x/y/z`、`f_dc_0..2`、`opacity`、`scale_0..2`、`rot_0..3`；
//! `f_rest_0..N` 可缺（无视角色，退化为 DC）。
//! 已知坑（见 depth-contract/c1/convert_to_spz.py 记账）：gaussforge 回转的
//! PLY 头**没有 `end_header`**——解析器在遇到非头语法行时也停。

use crate::SplatError;

/// 一颗高斯：原始 PLY 数值（未解码 log/sigmoid）。
#[derive(Clone, Debug)]
pub struct RawGaussian {
    pub position: [f32; 3],
    pub f_dc: [f32; 3],
    /// f_rest 展平（SH 阶数由调用方按 N/3 判定），可能为空。
    pub f_rest: Vec<f32>,
    pub opacity_logit: f32,
    pub scale_log: [f32; 3],
    /// xyzw（3DGS 存储口径）。
    pub rot_xyzw: [f32; 4],
}

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
        return Err(SplatError::TruncatedData { need, got: bytes.len() });
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
        let rest_cols: Vec<usize> = props
            .iter()
            .enumerate()
            .filter(|(_k, p)| p.starts_with("f_rest_"))
            .map(|(k, _p)| k)
            .collect();
        let rot_w = col("rot_3");
        out.push(RawGaussian {
            position: [f(i("x")), f(i("y")), f(i("z"))],
            f_dc: [f(i("f_dc_0")), f(i("f_dc_1")), f(i("f_dc_2"))],
            f_rest: rest_cols.iter().map(|k| f(*k)).collect(),
            opacity_logit: f(i("opacity")),
            scale_log: [f(i("scale_0")), f(i("scale_1")), f(i("scale_2"))],
            rot_xyzw: [f(i("rot_0")), f(i("rot_1")), f(i("rot_2")), rot_w.map(f).unwrap_or(0.0)],
        });
    }
    Ok(out)
}

/// 3DGS 激活：scale = exp(log_scale)，opacity = sigmoid(logit)，rot 保持 xyzw 4 元。
pub fn activate(g: &RawGaussian) -> ([f32; 3], [f32; 4], f32) {
    let scale = [
        g.scale_log[0].exp(),
        g.scale_log[1].exp(),
        g.scale_log[2].exp(),
    ];
    let opacity = 1.0 / (1.0 + (-g.opacity_logit).exp());
    (scale, g.rot_xyzw, opacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_ply(rest_props: usize) -> Vec<u8> {
        let mut head = String::from(
            "ply\nformat binary_little_endian 1.0\nelement vertex 2\n\
             property float x\nproperty float y\nproperty float z\n\
             property float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\n",
        );
        for k in 0..rest_props {
            head.push_str(&format!("property float f_rest_{k}\n"));
        }
        head.push_str(
            "property float opacity\nproperty float scale_0\nproperty float scale_1\n\
             property float scale_2\nproperty float rot_0\nproperty float rot_1\n\
             property float rot_2\nproperty float rot_3\nend_header\n",
        );
        let mut out = head.into_bytes();
        for v in 0..2u32 {
            // 全部 14(+rest) 列都要写数据
            let vals: Vec<f32> = (0..(14 + rest_props))
                .map(|k| (v as f32) + 0.25 * (k as f32))
                .collect();
            for f in vals {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn parses_standard_layout_by_property_names() {
        let bytes = write_test_ply(0);
        let g = parse_ply(&bytes).unwrap();
        assert_eq!(g.len(), 2);
        // 数据 = v + 0.25·列序：v=0 第 7 列 (opacity) = 1.5；v=1 第 10 列 (scale_2) = 3.25
        assert!((g[1].position[0] - 1.0).abs() < 1e-6);
        assert!((g[0].opacity_logit - 1.5).abs() < 1e-6, "opacity={}", g[0].opacity_logit);
        assert!((g[1].scale_log[2] - 3.25).abs() < 1e-6);
        assert!(g[0].f_rest.is_empty());
    }

    #[test]
    fn parses_sh_rest_columns() {
        let bytes = write_test_ply(45);
        let g = parse_ply(&bytes).unwrap();
        assert_eq!(g[0].f_rest.len(), 45);
    }

    #[test]
    fn parses_headerless_gaussforge_variant() {
        // 字节级抹掉 end_header 行，模拟 gaussforge 回转 PLY（头后直接二进制）
        let mut bytes = write_test_ply(0);
        let needle = b"end_header\n";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("end_header 应存在");
        bytes.drain(pos..pos + needle.len());
        let g = parse_ply(&bytes).unwrap();
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn missing_required_property_fails() {
        // 字节级删掉 opacity 属性行（头是 ASCII，按行删安全）
        let mut bytes = write_test_ply(0);
        let needle = b"property float opacity\n";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("opacity 属性行应存在");
        bytes.drain(pos..pos + needle.len());
        let err = parse_ply(&bytes).unwrap_err();
        assert!(matches!(err, SplatError::MissingProperty(_)));
    }

    #[test]
    fn activation_decodes_log_and_sigmoid() {
        let g = RawGaussian {
            position: [0.0; 3],
            f_dc: [0.0; 3],
            f_rest: vec![],
            opacity_logit: 0.0,
            scale_log: [0.0, 0.0, 0.0],
            rot_xyzw: [0.0, 0.0, 0.0, 1.0],
        };
        let (scale, rot, opacity) = activate(&g);
        assert!((opacity - 0.5).abs() < 1e-6);
        assert!((scale[0] - 1.0).abs() < 1e-6);
        assert!((rot[3] - 1.0).abs() < 1e-6);
    }
}
