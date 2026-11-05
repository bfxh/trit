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

/// GT 原语的世界半尺寸（与 `occ::Primitive` 同一套基准）：
/// `objects.json` 的 `scale` 是**基准体上的倍率**，不是米。
/// - part_cube/cube：半边长 0.0275 / 0.5
/// - sphere：r=0.7；cylinder：r=0.45、半高 0.6；torus：外径 0.5+0.18、管径 0.18
pub fn half_extents(l: &Label) -> [f64; 3] {
    let s = l.scale;
    match l.kind.as_str() {
        "part_cube" => [0.0275 * s[0], 0.0275 * s[1], 0.0275 * s[2]],
        "cube" => [0.5 * s[0], 0.5 * s[1], 0.5 * s[2]],
        "sphere" => [0.7 * s[0], 0.7 * s[1], 0.7 * s[2]],
        "cylinder" => [0.45 * s[0], 0.45 * s[1], 0.6 * s[2]],
        "torus" => [0.68 * s[0], 0.68 * s[1], 0.18 * s[2]],
        _ => [0.5 * s[0], 0.5 * s[1], 0.5 * s[2]],
    }
}

/// GT 原语包围盒的对角线（旋转不变）：`2·||half_extents||`。
pub fn gt_diag(l: &Label) -> f64 {
    let h = half_extents(l);
    2.0 * (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt()
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
    fn gt_diag_is_rotation_invariant() {
        let l = Label {
            pass_index: 1,
            kind: "sphere".into(),
            center: [0.0; 3],
            scale: [0.3, 0.4, 0.5],
        };
        // 球：half = 0.7·scale = (0.21,0.28,0.35)；diag = 2·||half|| = 2·sqrt(0.1897)
        assert!((gt_diag(&l) - 2.0 * (0.0441f64 + 0.0784 + 0.1225).sqrt()).abs() < 1e-12);
        let pc = Label {
            kind: "part_cube".into(),
            scale: [2.366, 2.366, 2.366],
            ..l
        };
        assert!((half_extents(&pc)[0] - 0.0275 * 2.366).abs() < 1e-12);
        let cyl = Label {
            kind: "cylinder".into(),
            scale: [1.0, 1.0, 1.0],
            ..l
        };
        assert_eq!(half_extents(&cyl), [0.45, 0.45, 0.6]);
    }

    #[test]
    fn centroid_error_is_euclidean() {
        assert!(centroid_error([0.0; 3], [0.0; 3]).abs() < 1e-12);
        assert!((centroid_error([0.0; 3], [1.0, 2.0, 2.0]) - 3.0).abs() < 1e-9);
    }
}
