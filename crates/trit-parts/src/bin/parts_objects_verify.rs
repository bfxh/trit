//! `parts-objects-verify`：对象级资产（A2）的**独立复核**。
//!
//! 重算语料指纹与每个 PLY 的 SHA，把 PLY 反解回高斯，再用分割 id **重新建一遍**，
//! 逐高斯比对（位置/尺度/颜色/alpha）以及包围盒/质心；不一致就写
//! `objects.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-objects-verify --objects runs/objects/scene0084
//! ```

use std::path::{Path, PathBuf};

use trit_parts::data::{read_npy_i32, read_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::{fingerprint_scene_frames_rgb, norm_root};
use trit_parts::splat::{Splat, build_filtered, read_ply_3dgs, sort_splats};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-objects-verify --objects <目录> [--manifest <json>] [--json-out <path>]"
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

/// `"key": [a, b, c]`（只取数字）。
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

fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    Some(&rest[..rest.find('}')?])
}

#[derive(Debug, Clone)]
struct Obj {
    id: u32,
    path: String,
    sha: String,
    bytes: u64,
    count: usize,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
    centroid: [f64; 3],
    kind: Option<String>,
    gt_center: Vec<f64>,
    centroid_error: Option<f64>,
    gt_diag: Option<f64>,
    diag_ratio: Option<f64>,
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
        let bx = nums(seg, "bbox_min");
        let bh = nums(seg, "bbox_max");
        let ce = nums(seg, "centroid");
        if bx.len() != 3 || bh.len() != 3 || ce.len() != 3 {
            continue;
        }
        out.push(Obj {
            id,
            path: field(seg, "path").unwrap_or_default(),
            sha: field(seg, "sha256").unwrap_or_default(),
            bytes: number(seg, "bytes").unwrap_or(0.0) as u64,
            count: number(seg, "count").unwrap_or(0.0) as usize,
            bbox_min: [bx[0], bx[1], bx[2]],
            bbox_max: [bh[0], bh[1], bh[2]],
            centroid: [ce[0], ce[1], ce[2]],
            kind: field(seg, "kind"),
            gt_center: nums(seg, "gt_center"),
            centroid_error: number(seg, "centroid_error"),
            gt_diag: number(seg, "gt_diag"),
            diag_ratio: number(seg, "diag_ratio"),
        })
    }
    out
}

/// 清单里的标签三件套（kind / gt_center / centroid_error）必须与 GT 和重算值一致。
fn label_ok(
    o: &Obj,
    l: Option<&trit_parts::objlabel::Label>,
    cen: [f64; 3],
    our_diag: f64,
) -> bool {
    let Some(l) = l else {
        return false;
    };
    let err = trit_parts::objlabel::centroid_error(cen, l.center);
    let gd = trit_parts::objlabel::gt_diag(l);
    let ratio = our_diag / gd.max(1e-9);
    o.kind.as_deref() == Some(l.kind.as_str())
        && o.gt_center.len() == 3
        && (0..3).all(|k| (o.gt_center[k] - l.center[k]).abs() <= 1e-9)
        && o.centroid_error
            .map(|e| (e - err).abs() <= 1e-6)
            .unwrap_or(false)
        && o.gt_diag.map(|g| (g - gd).abs() <= 1e-9).unwrap_or(false)
        && o.diag_ratio
            .map(|r| (r - ratio).abs() <= 1e-6)
            .unwrap_or(false)
}

fn load_view(scene: &Path, f: usize) -> View {
    let (ids, shape) =
        read_npy_i32(&scene.join("seg").join(format!("{f:06}.npy"))).unwrap_or_else(|e| fail(&e));
    let (d, _) =
        read_npy_f32(&scene.join("depth").join(format!("{f:06}.npy"))).unwrap_or_else(|e| fail(&e));
    View {
        ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
        depth: d,
        w: shape[1],
        h: shape[0],
        cam: Camera::load(&scene.join("meta").join(format!("{f:06}.json")))
            .unwrap_or_else(|e| fail(&e)),
    }
}

fn stats(splats: &[Splat]) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let mut cen = [0f64; 3];
    for s in splats {
        for k in 0..3 {
            lo[k] = lo[k].min(s.pos[k] as f64);
            hi[k] = hi[k].max(s.pos[k] as f64);
            cen[k] += s.pos[k] as f64;
        }
    }
    let n = splats.len().max(1) as f64;
    for c in &mut cen {
        *c /= n;
    }
    (lo, hi, cen)
}

/// 盘上 PLY 反解出的一批高斯 vs 重新建出来的一批：数量与逐字段都必须一致。
fn splats_match(got: &[Splat], want: &[Splat]) -> bool {
    if got.len() != want.len() || got.is_empty() {
        return false;
    }
    got.iter().zip(want).all(|(g, w)| {
        (0..3).all(|k| (g.pos[k] - w.pos[k]).abs() <= 1e-5)
            && (0..3).all(|k| (g.color[k] - w.color[k]).abs() <= 1e-5)
            && (g.scale - w.scale).abs() <= 1e-5
            && (g.alpha - w.alpha).abs() <= 1e-5
    })
}

/// 清单声明的对象一个不少，且（对象 ∪ 跳过）恰好等于分割里出现的 id 集合。
fn ids_complete(
    declared: usize,
    parsed: usize,
    expect: &std::collections::BTreeSet<u32>,
    seg: &std::collections::BTreeSet<u32>,
) -> bool {
    declared == parsed && expect == seg
}

fn stats_match(want: &Obj, lo: [f64; 3], hi: [f64; 3], cen: [f64; 3]) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-4;
    (0..3).all(|k| near(want.bbox_min[k], lo[k]))
        && (0..3).all(|k| near(want.bbox_max[k], hi[k]))
        && (0..3).all(|k| near(want.centroid[k], cen[k]))
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
    let dir = arg(&argv, "--objects").map(PathBuf::from);
    let mpath = match (arg(&argv, "--manifest"), &dir) {
        (Some(m), _) => PathBuf::from(m),
        (None, Some(d)) => d.join("objects.manifest.json"),
        (None, None) => usage(),
    };
    let base = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let data = block(&manifest, "data").unwrap_or_else(|| usage());
    let cfg = block(&manifest, "config").unwrap_or_else(|| usage());
    let scene = PathBuf::from(field(data, "scene").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = {
        let pat = "\"frames\": [";
        let i = cfg.find(pat).unwrap_or_else(|| usage()) + pat.len();
        let rest = &cfg[i..];
        let end = rest.find(']').unwrap_or(rest.len());
        rest[..end]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };
    if frames.is_empty() {
        eprintln!("清单缺 frames");
        std::process::exit(2);
    }
    let cell = number(cfg, "cell").unwrap_or(0.01) as f32;
    let scale_k = number(cfg, "scale_k").unwrap_or(0.6) as f32;
    let alpha = number(cfg, "alpha").unwrap_or(0.8) as f32;
    let labels = field(cfg, "scenes").map(|root| {
        let name = scene
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        trit_parts::objlabel::load(&PathBuf::from(root).join(name).join("objects.json"))
            .unwrap_or_else(|e| fail(&e))
    });
    let want_objects = objects(&manifest);

    let fp = fingerprint_scene_frames_rgb(&scene, &frames).unwrap_or_else(|e| {
        eprintln!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let ok_data = manifest.contains("\"kind\": \"extract-objects\"")
        && manifest.contains("\"algo\": \"sha256-content-v1\"")
        && field(data, "hash") == Some(format!("sha256:{}", fp.hash))
        && number(data, "files") == Some(fp.files as f64)
        && number(data, "bytes") == Some(fp.bytes as f64);

    let views: Vec<View> = frames.iter().map(|&f| load_view(&scene, f)).collect();
    let rgbs: Vec<Vec<f32>> = frames
        .iter()
        .map(|&f| {
            read_rgb(&scene.join("rgb").join(format!("{f:06}.png")))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();

    // 清单里声明的对象必须一个不少、并且覆盖分割里出现的全部 id（否则删一条也算 PASS）
    let declared = manifest.matches("{\"id\": ").count();
    let skipped: Vec<u32> = nums(&manifest, "skipped")
        .iter()
        .map(|v| *v as u32)
        .collect();
    let seen: std::collections::BTreeSet<u32> = want_objects.iter().map(|o| o.id).collect();
    let seg_ids: std::collections::BTreeSet<u32> = views
        .iter()
        .flat_map(|v| v.ids.iter().copied())
        .filter(|&i| i != 0)
        .collect();
    let mut expect_ids = seen.clone();
    expect_ids.extend(skipped.iter().copied());
    let ok_ids = ids_complete(declared, want_objects.len(), &expect_ids, &seg_ids);

    let mut bad = Vec::new();
    let mut labels_all = true;
    for o in &want_objects {
        let p = base.join(&o.path);
        let got = read_ply_3dgs(&p).unwrap_or_else(|e| {
            eprintln!("读 PLY 失败: {e}");
            std::process::exit(2);
        });
        let mut want = build_filtered(&views, &rgbs, cell, scale_k, |v| v == o.id);
        for s in want.iter_mut() {
            s.alpha = alpha.clamp(1e-4, 1.0 - 1e-4);
        }
        sort_splats(&mut want);
        let (lo, hi, cen) = stats(&got);
        let our_diag =
            ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
        let ok_label = match &labels {
            None => true,
            Some(ls) => {
                let l = ls.iter().find(|l| l.pass_index == o.id);
                label_ok(o, l, cen, our_diag)
            }
        };
        labels_all &= ok_label;
        let ok = o.sha == hash(&p)
            && o.bytes == std::fs::metadata(&p).map(|m| m.len()).unwrap_or(u64::MAX)
            && o.count == got.len()
            && splats_match(&got, &want)
            && stats_match(o, lo, hi, cen)
            && ok_label;
        println!(
            "对象 {}: {} 个高斯{}",
            o.id,
            got.len(),
            if ok { "" } else { "  ← 不一致" }
        );
        if !ok {
            bad.push(o.id);
        }
    }
    let ok_all = ok_data && ok_ids && labels_all && !want_objects.is_empty() && bad.is_empty();
    println!(
        "复核 {}：语料 {}｜对象 {}/{} 一致｜id 集合 {}｜标签 {}",
        mpath.display(),
        if ok_data { "一致" } else { "不一致" },
        want_objects.len() - bad.len(),
        want_objects.len(),
        if ok_ids { "一致" } else { "不一致" },
        if labels_all { "一致" } else { "不一致" }
    );
    println!(
        "{}",
        if ok_all {
            "复核 PASS：语料指纹 / 每对象 SHA / 高斯逐字段重建 / 包围盒质心全一致"
        } else {
            "复核 FAIL：与清单或重建结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-objects-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"objects\": {{\"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}, \"total\": {}, \"bad\": [{}], \"ok\": {ok_all}}},\n  \"data\": {{\"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"files\": {}, \"bytes\": {}, \"ok\": {ok_data}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&mpath)),
        hash(&mpath),
        want_objects.len(),
        bad.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        fp.hash,
        fp.files,
        fp.bytes,
        if ok_all { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        base.join(if ok_all {
            "objects.verify.json"
        } else {
            "objects.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(base.join(if ok_all {
            "objects.verify.reject.json"
        } else {
            "objects.verify.json"
        }));
    }
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录 {}: {e}", vp.display());
        std::process::exit(2);
    });
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if ok_all { "pass" } else { "reject" }
    );
    if !ok_all {
        std::process::exit(1);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("FAIL {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAN: &str = "{\"kind\": \"extract-objects\", \"config\": {\"frames\": [0, 1], \"cell\": 0.01, \"scale_k\": 0.6, \"alpha\": 0.8}, \"data\": {\"files\": 8, \"bytes\": 100, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:ab\"}, \"objects\": [{\"id\": 1, \"path\": \"obj-1.ply\", \"sha256\": \"a1\", \"bytes\": 68, \"count\": 2, \"bbox_min\": [0.0, 0.0, 0.0], \"bbox_max\": [1.0, 1.0, 1.0], \"centroid\": [0.5, 0.5, 0.5]}, {\"id\": 5, \"path\": \"obj-5.ply\", \"sha256\": \"a5\", \"bytes\": 68, \"count\": 3, \"bbox_min\": [1.0, 0.0, 0.0], \"bbox_max\": [2.0, 1.0, 1.0], \"centroid\": [1.5, 0.5, 0.5]}], \"skipped\": []}";

    fn sp(pos: [f32; 3]) -> Splat {
        Splat {
            pos,
            scale: 0.01,
            color: [0.5; 3],
            alpha: 0.8,
        }
    }

    #[test]
    fn parses_objects_and_stats() {
        let objs = objects(MAN);
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[0].id, 1);
        assert_eq!(objs[0].path, "obj-1.ply");
        assert_eq!(objs[0].count, 2);
        assert_eq!(objs[1].id, 5);
        assert_eq!(objs[1].bbox_max, [2.0, 1.0, 1.0]);
        assert_eq!(objs[1].centroid, [1.5, 0.5, 0.5]);
        assert_eq!(number(block(MAN, "config").unwrap(), "alpha"), Some(0.8));
    }

    #[test]
    fn splat_and_stat_comparisons_are_deciding() {
        let a = vec![sp([0.0, 0.0, 0.0]), sp([1.0, 1.0, 1.0])];
        assert!(splats_match(&a, &a));
        let mut b = a.clone();
        b[1].color[2] = 0.6;
        assert!(!splats_match(&a, &b), "颜色漂移必须判红");
        let mut b = a.clone();
        b[0].alpha = 0.7;
        assert!(!splats_match(&a, &b), "alpha 漂移必须判红");
        assert!(!splats_match(&a, &a[..1]), "数量不一致必须判红");
        assert!(!splats_match(&[], &[]), "空对象不是有效资产");
        let (lo, hi, cen) = stats(&a);
        let o = Obj {
            id: 1,
            path: String::new(),
            sha: String::new(),
            bytes: 0,
            count: 2,
            bbox_min: lo,
            bbox_max: hi,
            centroid: cen,
            kind: None,
            gt_center: Vec::new(),
            centroid_error: None,
            gt_diag: None,
            diag_ratio: None,
        };
        assert!(stats_match(&o, lo, hi, cen));
        assert!(!stats_match(&o, lo, [hi[0] + 0.01, hi[1], hi[2]], cen));
        let lab = trit_parts::objlabel::Label {
            pass_index: 1,
            kind: "sphere".into(),
            center: cen,
            scale: [1.0; 3],
        };
        let our_diag =
            ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
        let gd = trit_parts::objlabel::gt_diag(&lab);
        let mut with_lab = o.clone();
        with_lab.kind = Some("sphere".into());
        with_lab.gt_center = cen.to_vec();
        with_lab.centroid_error = Some(0.0);
        with_lab.gt_diag = Some(gd);
        with_lab.diag_ratio = Some(our_diag / gd);
        assert!(label_ok(&with_lab, Some(&lab), cen, our_diag));
        assert!(
            !label_ok(&o, Some(&lab), cen, our_diag),
            "缺标签字段必须判红"
        );
        let mut wrong = with_lab.clone();
        wrong.kind = Some("torus".into());
        assert!(
            !label_ok(&wrong, Some(&lab), cen, our_diag),
            "kind 不符必须判红"
        );
        let mut wrong = with_lab.clone();
        wrong.centroid_error = Some(0.5);
        assert!(
            !label_ok(&wrong, Some(&lab), cen, our_diag),
            "质心误差不符必须判红"
        );
        let mut wrong = with_lab.clone();
        wrong.diag_ratio = Some(9.0);
        assert!(
            !label_ok(&wrong, Some(&lab), cen, our_diag),
            "对角线比不符必须判红"
        );
    }

    #[test]
    fn id_set_must_be_complete() {
        let seg: std::collections::BTreeSet<u32> = [1, 2, 5].into_iter().collect();
        let all = seg.clone();
        assert!(ids_complete(3, 3, &all, &seg));
        let missing: std::collections::BTreeSet<u32> = [1, 2].into_iter().collect();
        assert!(!ids_complete(2, 2, &missing, &seg), "少一个对象必须判红");
        assert!(
            !ids_complete(4, 3, &all, &seg),
            "声明数与解析数不符必须判红"
        );
    }
}
