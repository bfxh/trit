//! `objects.json` 的语义标签（GT 原语）：每个对象的 `pass_index / kind / center / scale`。
//!
//! 对象资产用 `pass_index`（= 分割 id）与 GT 对齐，清单里就能记 `kind` 与质心误差。

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub pass_index: u32,
    pub kind: String,
    pub center: [f64; 3],
    pub scale: [f64; 3],
}

fn field(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn number(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn nums(txt: &str, key: &str) -> Vec<f64> {
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

/// 解析 `objects.json`（数组，每项 `{"name","pass_index","center","kind","scale","rot"}`）。
pub fn parse_objects_json(txt: &str) -> Vec<Label> {
    let mut out = Vec::new();
    for seg in txt.split("{\"name\":").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let kind = field(seg, "kind").unwrap_or_default();
        let c = nums(seg, "center");
        let s = nums(seg, "scale");
        if kind.is_empty() || c.len() != 3 || s.len() != 3 {
            continue;
        }
        out.push(Label {
            pass_index: number(seg, "pass_index").unwrap_or(0.0) as u32,
            kind,
            center: [c[0], c[1], c[2]],
            scale: [s[0], s[1], s[2]],
        });
    }
    out
}

pub fn load(path: &Path) -> Result<Vec<Label>, String> {
    let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v = parse_objects_json(&txt);
    if v.is_empty() {
        return Err(format!("{}: 没有可用标签", path.display()));
    }
    Ok(v)
}

/// 两点欧氏距离（米）。
pub fn centroid_error(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[{\"name\": \"p001\", \"pass_index\": 1, \"center\": [0.4, 0.57, 0.95], \"kind\": \"part_cube\", \"scale\": [0.97, 0.73, 0.82], \"rot\": [53.2, 38.6, 177.2]}, {\"name\": \"p002\", \"pass_index\": 2, \"center\": [-0.16, 0.48, 0.79], \"kind\": \"torus\", \"scale\": [0.58, 0.58, 0.58], \"rot\": [0, 0, 0]}]";

    #[test]
    fn parses_labels_by_pass_index() {
        let l = parse_objects_json(SAMPLE);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].pass_index, 1);
        assert_eq!(l[0].kind, "part_cube");
        assert_eq!(l[0].center, [0.4, 0.57, 0.95]);
        assert_eq!(l[0].scale, [0.97, 0.73, 0.82]);
        assert_eq!(l[1].kind, "torus");
        assert_eq!(l[1].pass_index, 2);
        // 缺字段的条目不产生标签
        assert!(parse_objects_json("[{\"name\": \"x\"}]").is_empty());
    }

    #[test]
    fn centroid_error_is_euclidean() {
        assert!(centroid_error([0.0; 3], [0.0; 3]).abs() < 1e-12);
        assert!((centroid_error([0.0; 3], [1.0, 2.0, 2.0]) - 3.0).abs() < 1e-9);
    }
}
