//! `parts-compose-verify`：场景合成（A2）的**独立复核**。
//!
//! 重算对象清单 SHA、用同一套货架规则**重新摆放**、把对象高斯重新变换并拼起来，
//! 与盘上的 `composed.ply` 逐高斯比对；同时重算穿插体积与覆盖率。不一致就写
//! `compose.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-compose-verify --scene runs/compose/scene0084
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::norm_root;
use trit_parts::splat::{Item, Placement, Splat, apply, overlap_volume, pack_best, read_ply_3dgs};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-compose-verify --scene <目录> [--objects <对象目录>] [--manifest <json>] [--json-out <path>]"
    );
    std::process::exit(2);
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

/// 取 `"key": { ... }` 的对象体（支持嵌套，跳过字符串）。
fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
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

struct Obj {
    id: u32,
    path: String,
    count: usize,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
}

fn objects(txt: &str) -> Vec<Obj> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let (lo, hi) = (nums(seg, "bbox_min"), nums(seg, "bbox_max"));
        if lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Obj {
            id,
            path: field(seg, "path").unwrap_or_default(),
            count: number(seg, "count").unwrap_or(0.0) as usize,
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
}

struct LayoutRow {
    id: u32,
    yaw: u32,
    translate: [f64; 3],
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
    count: usize,
}

fn layout(txt: &str) -> Vec<LayoutRow> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let tr = nums(seg, "translate");
        let lo = nums(seg, "bbox_min");
        let hi = nums(seg, "bbox_max");
        if tr.len() != 3 || lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(LayoutRow {
            id,
            yaw: number(seg, "yaw").unwrap_or(0.0) as u32,
            translate: [tr[0], tr[1], tr[2]],
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
            count: number(seg, "count").unwrap_or(0.0) as usize,
        });
    }
    out
}

fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-6)
}

fn placements_match(want: &LayoutRow, got: &Placement) -> bool {
    want.id == got.id
        && want.yaw == got.yaw
        && near3(want.translate, got.translate)
        && near3(want.bbox_min, got.bbox_min)
        && near3(want.bbox_max, got.bbox_max)
}

fn splats_match(got: &[Splat], want: &[Splat]) -> bool {
    got.len() == want.len()
        && !got.is_empty()
        && got.iter().zip(want).all(|(g, w)| {
            (0..3).all(|k| (g.pos[k] - w.pos[k]).abs() <= 1e-5)
                && (0..3).all(|k| (g.color[k] - w.color[k]).abs() <= 1e-5)
                && (g.scale - w.scale).abs() <= 1e-5
                && (g.alpha - w.alpha).abs() <= 1e-5
        })
}

/// 门：零穿插且覆盖率不低于阈值。
fn gate_ok(ps: &[Placement], cov: f64, cov_min: f64) -> bool {
    let overlap: f64 = (0..ps.len())
        .map(|i| {
            ps.iter()
                .skip(i + 1)
                .map(|b| overlap_volume(&ps[i], b))
                .sum::<f64>()
        })
        .sum();
    overlap <= 1e-12 && cov >= cov_min
}

fn hash(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了 {}: {e}", p.display());
        std::process::exit(2);
    })
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let dir = arg(&argv, "--scene").map(PathBuf::from);
    let mpath = match (arg(&argv, "--manifest"), &dir) {
        (Some(m), _) => PathBuf::from(m),
        (None, Some(d)) => d.join("compose.manifest.json"),
        (None, None) => usage(),
    };
    let base = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let cfg = block(&manifest, "config").unwrap_or_else(|| usage());
    let inputs = block(&manifest, "inputs").unwrap_or_else(|| usage());
    let oman = block(inputs, "objects_manifest").unwrap_or_else(|| usage());
    let gate = block(&manifest, "gate").unwrap_or_else(|| usage());
    let gap = number(cfg, "gap").unwrap_or(0.05);
    let cov_min = number(cfg, "cov_min").unwrap_or(0.4);
    let want_cov = number(gate, "coverage").unwrap_or(f64::NAN);
    let want_rows = layout(&manifest);

    let ompath = arg(&argv, "--objects")
        .map(|d| PathBuf::from(d).join("objects.manifest.json"))
        .unwrap_or_else(|| resolve(&base, field(oman, "path").unwrap_or_else(|| usage())));
    let objdir = ompath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let ok_oman = field(oman, "sha256")
        .map(|s| s == hash(&ompath))
        .unwrap_or(false);
    let om = read(&ompath);
    let objs = objects(&om);

    let items: Vec<Item> = objs
        .iter()
        .map(|o| Item {
            id: o.id,
            bbox_min: o.bbox_min,
            bbox_max: o.bbox_max,
        })
        .collect();
    let (placements, _width, cov) = pack_best(&items, gap);
    let mut map: HashMap<u32, Vec<Splat>> = HashMap::new();
    for o in &objs {
        let v = read_ply_3dgs(&objdir.join(&o.path)).unwrap_or_else(|e| {
            eprintln!("读对象 PLY 失败: {e}");
            std::process::exit(2);
        });
        map.insert(o.id, v);
    }
    let ok_obj_counts = objs
        .iter()
        .all(|o| map.get(&o.id).map(|v| v.len()) == Some(o.count));
    let mut want = Vec::new();
    for pl in &placements {
        let it = items
            .iter()
            .find(|i| i.id == pl.id)
            .unwrap_or_else(|| usage());
        want.extend(apply(&map[&pl.id], it, pl));
    }

    let ok_layout = want_rows.len() == placements.len()
        && want_rows
            .iter()
            .zip(&placements)
            .all(|(r, p)| placements_match(r, p));
    let comp = read_ply_3dgs(&base.join("composed.ply")).unwrap_or_else(|e| {
        eprintln!("读 composed.ply 失败: {e}");
        std::process::exit(2);
    });
    let comp_block = block(&manifest, "composed").unwrap_or_else(|| usage());
    let ok_file = field(comp_block, "sha256")
        .map(|s| s == hash(&base.join("composed.ply")))
        .unwrap_or(false)
        && number(comp_block, "count") == Some(comp.len() as f64)
        && splats_match(&comp, &want);
    let ok_counts = want_rows.iter().map(|r| r.count).sum::<usize>() == comp.len();
    let ok_gate = gate_ok(&placements, cov, cov_min)
        && (want_cov - cov).abs() <= 1e-9
        && gate.contains("\"ok\": true");

    let pass = ok_oman && ok_obj_counts && ok_layout && ok_file && ok_counts && ok_gate;
    println!(
        "复核 {}：对象清单 {}｜摆放 {}/{}｜合成 {} 个高斯｜覆盖 {cov:.3}｜门 {}",
        mpath.display(),
        if ok_oman && ok_obj_counts {
            "一致"
        } else {
            "不一致"
        },
        placements.len(),
        want_rows.len(),
        comp.len(),
        if ok_gate { "PASS" } else { "FAIL" }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：对象清单 / 摆放 / 逐高斯合成 / 门（穿插 0、覆盖）全一致"
        } else {
            "复核 FAIL：与清单或重跑结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-compose-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"compose\": {{\"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}}},\n  \"inputs\": {{\"objects_manifest\": {{\"path\": {}, \"sha256\": \"{}\", \"ok\": {ok_oman}}}}},\n  \"layout\": {{\"rows\": {}, \"ok\": {ok_layout}}},\n  \"composed\": {{\"count\": {}, \"file_ok\": {ok_file}, \"counts_ok\": {ok_counts}}},\n  \"gate\": {{\"coverage\": {cov}, \"cov_min\": {cov_min}, \"ok\": {ok_gate}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&mpath)),
        hash(&mpath),
        json_str(&norm_root(&ompath)),
        hash(&ompath),
        want_rows.len(),
        comp.len(),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        base.join(if pass {
            "compose.verify.json"
        } else {
            "compose.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(base.join(if pass {
            "compose.verify.reject.json"
        } else {
            "compose.verify.json"
        }));
    }
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录 {}: {e}", vp.display());
        std::process::exit(2);
    });
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if pass { "pass" } else { "reject" }
    );
    if !pass {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAN: &str = "{\"kind\": \"extract-compose\", \"config\": {\"gap\": 0.05, \"cov_min\": 0.4, \"width\": 2.0}, \"inputs\": {\"objects_manifest\": {\"path\": \"obj/objects.manifest.json\", \"sha256\": \"om\"}}, \"layout\": [{\"id\": 5, \"yaw\": 90, \"translate\": [-0.5, 0.5, 0.1], \"bbox_min\": [0.0, 0.0, 0.0], \"bbox_max\": [1.0, 1.0, 1.0], \"count\": 10}, {\"id\": 1, \"yaw\": 0, \"translate\": [1.0, 0.0, 0.0], \"bbox_min\": [1.05, 0.0, 0.0], \"bbox_max\": [2.0, 1.0, 1.0], \"count\": 20}], \"composed\": {\"path\": \"composed.ply\", \"sha256\": \"c1\", \"bytes\": 100, \"count\": 30}, \"gate\": {\"overlap\": 0, \"coverage\": 0.8, \"cov_min\": 0.4, \"ok\": true}}";

    fn sp(pos: [f32; 3]) -> Splat {
        Splat {
            pos,
            scale: 0.01,
            color: [0.5; 3],
            alpha: 0.8,
        }
    }

    #[test]
    fn parses_layout_rows() {
        let rows = layout(MAN);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 5);
        assert_eq!(rows[0].yaw, 90);
        assert_eq!(rows[0].translate, [-0.5, 0.5, 0.1]);
        assert_eq!(rows[0].count, 10);
        assert_eq!(rows[1].bbox_max, [2.0, 1.0, 1.0]);
        assert_eq!(number(block(MAN, "config").unwrap(), "gap"), Some(0.05));
        assert_eq!(
            field(
                block(block(MAN, "inputs").unwrap(), "objects_manifest").unwrap(),
                "sha256"
            )
            .as_deref(),
            Some("om")
        );
    }

    #[test]
    fn relative_input_paths_resolve_against_manifest() {
        let base = Path::new("bundle/compose");
        assert_eq!(
            resolve(base, "../objects/objects.manifest.json".into()),
            PathBuf::from("bundle/compose/../objects/objects.manifest.json")
        );
        let abs = std::env::temp_dir().join("trit_objman.json");
        assert_eq!(resolve(base, abs.display().to_string()), abs);
    }

    #[test]
    fn placement_and_gate_checks_are_deciding() {
        let p = Placement {
            id: 1,
            yaw: 0,
            translate: [0.0, 0.0, 0.0],
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [1.0, 1.0, 1.0],
        };
        let row = LayoutRow {
            id: 1,
            yaw: 0,
            translate: [0.0; 3],
            bbox_min: [0.0; 3],
            bbox_max: [1.0, 1.0, 1.0],
            count: 3,
        };
        assert!(placements_match(&row, &p));
        let mut bad = p.clone();
        bad.yaw = 90;
        assert!(!placements_match(&row, &bad), "朝向变化必须判红");
        let q = Placement {
            id: 2,
            yaw: 0,
            translate: [0.5, 0.0, 0.0],
            bbox_min: [0.5, 0.0, 0.0],
            bbox_max: [1.5, 1.0, 1.0],
        };
        assert!(!gate_ok(&[p.clone(), q], 0.9, 0.4), "穿插必须判红");
        let far = Placement {
            id: 3,
            yaw: 0,
            translate: [2.0, 0.0, 0.0],
            bbox_min: [2.0, 0.0, 0.0],
            bbox_max: [3.0, 1.0, 1.0],
        };
        assert!(gate_ok(&[p.clone(), far], 0.66, 0.4));
        assert!(!gate_ok(&[p], 0.3, 0.4), "覆盖率不足必须判红");
        let a = vec![sp([0.0, 0.0, 0.0]), sp([1.0, 0.0, 0.0])];
        assert!(splats_match(&a, &a));
        let mut b = a.clone();
        b[0].scale = 0.02;
        assert!(!splats_match(&a, &b));
    }
}
