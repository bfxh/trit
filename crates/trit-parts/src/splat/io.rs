//! 标准 3DGS PLY 读写（binary_little_endian，17 个 float/高斯）。
//!
//! 只支持本仓产生的**各向同性**高斯：rotation 恒为单位四元数，scale 三轴相等。
//! 颜色按 3DGS 惯例存 SH 直流项 `f_dc = (c − 0.5) / C0`（`C0 = 0.5·√(1/π)`），
//! opacity 存 logit、scale 存自然对数——外部查看器（SuperSplat / gsplat）按同一约定还原。
//!
//! 读入是复核器的入口：它必须能从盘上的 PLY 把高斯原样还原并重渲，
//! 不接受旋转/各向异性（静默近似会把复核变成自我欺骗）。

use std::io::Write;
use std::path::Path;

use super::Splat;

/// SH 直流项系数。
pub const SH_C0: f32 = 0.282_094_79;

const PROPS: [&str; 17] = [
    "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
    "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
];

fn logit(a: f32) -> f32 {
    let a = a.clamp(1e-4, 1.0 - 1e-4);
    (a / (1.0 - a)).ln()
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// 写 PLY。顺序即输入顺序（调用方负责排序，保证可复现）。
pub fn write_ply_3dgs(path: &Path, splats: &[Splat]) -> Result<(), String> {
    let mut s = String::new();
    s.push_str("ply\nformat binary_little_endian 1.0\n");
    s.push_str(&format!("element vertex {}\n", splats.len()));
    for p in PROPS {
        s.push_str(&format!("property float {p}\n"));
    }
    s.push_str("end_header\n");
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(f);
    w.write_all(s.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for g in splats {
        let mut row = Vec::with_capacity(17 * 4);
        let mut put = |v: f32| row.extend_from_slice(&v.to_le_bytes());
        put(g.pos[0]);
        put(g.pos[1]);
        put(g.pos[2]);
        put(0.0);
        put(0.0);
        put(0.0);
        for c in g.color {
            put((c - 0.5) / SH_C0);
        }
        put(logit(g.alpha));
        let sc = g.scale.max(1e-6).ln();
        put(sc);
        put(sc);
        put(sc);
        put(1.0);
        put(0.0);
        put(0.0);
        put(0.0);
        w.write_all(&row)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    w.flush().map_err(|e| format!("{}: {e}", path.display()))
}

/// 读 PLY（只认 17 个 float 属性齐全的文件）。
pub fn read_ply_3dgs(path: &Path) -> Result<Vec<Splat>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mark = b"end_header\n";
    let off = bytes
        .windows(mark.len())
        .position(|w| w == mark)
        .map(|i| i + mark.len())
        .ok_or_else(|| format!("{}: 找不到 end_header", path.display()))?;
    let head = String::from_utf8_lossy(&bytes[..off]);
    let mut lines = head.lines();
    if lines.next() != Some("ply") {
        return Err(format!("{}: 不是 PLY", path.display()));
    }
    let mut n = None;
    let mut props: Vec<String> = Vec::new();
    let mut binary = false;
    for l in lines {
        if let Some(rest) = l.strip_prefix("format ") {
            binary = rest.starts_with("binary_little_endian");
        } else if let Some(rest) = l.strip_prefix("element vertex ") {
            n = rest.trim().parse::<usize>().ok();
        } else if let Some(rest) = l.strip_prefix("property ") {
            let ty = rest.split_whitespace().next().unwrap_or("");
            if !ty.starts_with("float") {
                return Err(format!("{}: 不支持的属性类型 {ty}", path.display()));
            }
            props.push(rest.split_whitespace().nth(1).unwrap_or("").to_string());
        }
    }
    if !binary {
        return Err(format!("{}: 只支持 binary_little_endian", path.display()));
    }
    let n = n.ok_or_else(|| format!("{}: 缺 element vertex", path.display()))?;
    let idx = |k: &str| {
        props
            .iter()
            .position(|p| p == k)
            .ok_or_else(|| format!("{}: 缺属性 {k}", path.display()))
    };
    let need: Vec<usize> = PROPS.iter().map(|k| idx(k)).collect::<Result<_, _>>()?;
    let stride = props.len() * 4;
    let body = &bytes[off..];
    if body.len() < n * stride {
        return Err(format!(
            "{}: 数据长度 {} < {n}×{stride}",
            path.display(),
            body.len()
        ));
    }
    let get = |row: usize, k: usize| -> f32 {
        let i = row * stride + need[k] * 4;
        f32::from_le_bytes([body[i], body[i + 1], body[i + 2], body[i + 3]])
    };
    let mut out = Vec::with_capacity(n);
    for r in 0..n {
        let pos = [get(r, 0), get(r, 1), get(r, 2)];
        let color = [
            0.5 + SH_C0 * get(r, 6),
            0.5 + SH_C0 * get(r, 7),
            0.5 + SH_C0 * get(r, 8),
        ];
        let alpha = sigmoid(get(r, 9));
        let sc = [get(r, 10), get(r, 11), get(r, 12)];
        let spread = sc[0].max(sc[1]).max(sc[2]) - sc[0].min(sc[1]).min(sc[2]);
        if spread > 1e-4 {
            return Err(format!(
                "{}: 第 {r} 个高斯是各向异性的，本仓不支持",
                path.display()
            ));
        }
        let rot = [get(r, 13), get(r, 14), get(r, 15), get(r, 16)];
        if (rot[0] - 1.0).abs() > 1e-4
            || rot[1].abs() > 1e-4
            || rot[2].abs() > 1e-4
            || rot[3].abs() > 1e-4
        {
            return Err(format!(
                "{}: 第 {r} 个高斯带旋转，本仓不支持",
                path.display()
            ));
        }
        out.push(Splat {
            pos,
            scale: sc[0].exp(),
            color,
            alpha,
        });
    }
    Ok(out)
}

/// 全序排序：位置 → 颜色 → 尺度 → alpha。`build` 走 HashMap，顺序随机；
/// 导出前必须先排序，否则同一输入两次运行会得到不同字节与不同重叠次序。
pub fn sort_splats(v: &mut [Splat]) {
    v.sort_by(|a, b| {
        let mut o = std::cmp::Ordering::Equal;
        for k in 0..3 {
            o = o
                .then(a.pos[k].total_cmp(&b.pos[k]))
                .then(a.color[k].total_cmp(&b.color[k]));
        }
        o.then(a.scale.total_cmp(&b.scale))
            .then(a.alpha.total_cmp(&b.alpha))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(std::path::PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("trit_splat_io_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Vec<Splat> {
        vec![
            Splat {
                pos: [0.1, 0.2, 0.3],
                scale: 0.005,
                color: [0.0, 0.5, 1.0],
                alpha: 0.8,
            },
            Splat {
                pos: [-1.5, 2.25, 0.75],
                scale: 0.02,
                color: [0.25, 0.75, 0.125],
                alpha: 0.6,
            },
        ]
    }

    #[test]
    fn sort_is_total_and_order_independent() {
        let src = sample();
        let extra = Splat {
            pos: [9.0, 0.0, 0.0],
            ..src[0].clone()
        };
        let mut a = src.clone();
        let mut b = src;
        b.reverse();
        a.push(extra.clone());
        b.insert(0, extra);
        sort_splats(&mut a);
        sort_splats(&mut b);
        let key = |v: &[Splat]| -> Vec<[u32; 8]> {
            v.iter()
                .map(|s| {
                    [
                        s.pos[0].to_bits(),
                        s.pos[1].to_bits(),
                        s.pos[2].to_bits(),
                        s.color[0].to_bits(),
                        s.color[1].to_bits(),
                        s.color[2].to_bits(),
                        s.scale.to_bits(),
                        s.alpha.to_bits(),
                    ]
                })
                .collect()
        };
        assert_eq!(key(&a), key(&b));
        assert_eq!(a.last().unwrap().pos, [9.0, 0.0, 0.0], "x 最大者排最后");
    }

    #[test]
    fn roundtrip_preserves_every_field() {
        let t = Tmp::new("rt");
        let p = t.0.join("a.ply");
        let src = sample();
        write_ply_3dgs(&p, &src).unwrap();
        let got = read_ply_3dgs(&p).unwrap();
        assert_eq!(got.len(), 2);
        for (a, b) in src.iter().zip(&got) {
            for k in 0..3 {
                assert!((a.pos[k] - b.pos[k]).abs() < 1e-7, "pos {k}");
                assert!((a.color[k] - b.color[k]).abs() < 1e-6, "color {k}");
            }
            assert!((a.scale - b.scale).abs() < 1e-7, "scale");
            assert!((a.alpha - b.alpha).abs() < 1e-6, "alpha");
        }
        let bytes = std::fs::read(&p).unwrap();
        let off = bytes
            .windows(11)
            .position(|w| {
                w == b"end_header
"
            })
            .unwrap();
        let head = String::from_utf8_lossy(&bytes[..off]);
        assert!(head.contains("element vertex 2"));
        assert!(head.contains("property float f_dc_0"));
    }

    #[test]
    fn writing_is_byte_deterministic() {
        let t = Tmp::new("det");
        let (p, q) = (t.0.join("a.ply"), t.0.join("b.ply"));
        let s = sample();
        write_ply_3dgs(&p, &s).unwrap();
        write_ply_3dgs(&q, &s).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(&q).unwrap());
    }

    #[test]
    fn rejects_missing_props_and_anisotropy() {
        let t = Tmp::new("bad");
        let p = t.0.join("bad.ply");
        std::fs::write(
            &p,
            "ply\nformat binary_little_endian 1.0\nelement vertex 0\nend_header\n",
        )
        .unwrap();
        assert!(read_ply_3dgs(&p).is_err(), "缺属性必须报错");
        // 各向异性的 scale（手改字节：stride 17，scale_1 在第 11 个属性）
        let a = t.0.join("aniso.ply");
        let s = sample();
        write_ply_3dgs(&a, &s).unwrap();
        let mut bytes = std::fs::read(&a).unwrap();
        let off = bytes
            .windows(11)
            .position(|w| w == b"end_header\n")
            .unwrap()
            + 11;
        let i = off + 11 * 4;
        bytes[i..i + 4].copy_from_slice(&0.5f32.to_le_bytes());
        std::fs::write(&a, &bytes).unwrap();
        let e = read_ply_3dgs(&a).unwrap_err();
        assert!(e.contains("各向异性"), "{e}");
    }

    #[test]
    fn extreme_alpha_and_scale_stay_finite() {
        let t = Tmp::new("ext");
        let p = t.0.join("e.ply");
        let s = vec![
            Splat {
                pos: [0.0; 3],
                scale: 0.0,
                color: [0.5; 3],
                alpha: 0.0,
            },
            Splat {
                pos: [1.0; 3],
                scale: 1e-9,
                color: [1.0; 3],
                alpha: 1.0,
            },
        ];
        write_ply_3dgs(&p, &s).unwrap();
        let got = read_ply_3dgs(&p).unwrap();
        assert!(
            got.iter()
                .all(|g| g.scale.is_finite() && g.alpha.is_finite())
        );
        assert!((got[0].alpha - 1e-4).abs() < 1e-6, "alpha 下限被夹紧");
        assert!(
            (got[1].alpha - (1.0 - 1e-4)).abs() < 1e-6,
            "alpha 上限被夹紧"
        );
    }
}
