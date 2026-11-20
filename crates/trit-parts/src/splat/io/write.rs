//! 写 PLY（从 splat/io.rs 下沉）：读外部文件那边要加溢出检查、得腾预算，
//! 而这里只是按 17 个 float 拼行，拆开之后各留各的上限。

use std::io::Write;
use std::path::Path;

use super::super::Splat;
use super::{PROPS, SH_C0};

fn logit(a: f32) -> f32 {
    let a = a.clamp(1e-4, 1.0 - 1e-4);
    (a / (1.0 - a)).ln()
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
