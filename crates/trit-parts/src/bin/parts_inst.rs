//! `parts-inst`：多物体实例分割的标签准备与评测。
//!
//! 两个子命令：
//! - `prep`：把渲染的部件 id 图 + `instances.json` 折成实例 id 图（`inst/` 子目录）；
//! - `eval`：模型 → 2D 实例，按 IoU 贪心匹配算实例 mIoU。
//!   - `--components`：只学前景，前景连通域当实例（经典基线）；
//!   - 缺省：直接按模型 id 图当实例（学出来的实例头）。

use std::path::{Path, PathBuf};

use trit_parts::data::{read_npy_i32, read_rgb, write_npy_i32};
use trit_parts::inst::{boundary_of, components, fg_iou, match_labels, order_by_x};

fn arg(argv: &[String], key: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == key)?;
    argv.get(i + 1).cloned()
}

fn usage() -> ! {
    eprintln!(
        "用法:\n  parts-inst prep --renders <dir> --scenes <dir> [--limit N]\n  \
         parts-inst eval --model <TPM1> --renders <dir> --scenes <dir> \
         [--components] [--k N] [--res 384] [--limit N] [--report N]"
    );
    std::process::exit(2)
}

fn scene_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap_or_else(|e| {
            eprintln!("读不了 {}: {e}", root.display());
            std::process::exit(2)
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("scene"))
        })
        .collect();
    v.sort();
    v
}

/// 从 `instances.json` 里取 `(first_id, last_id)` 区间（实例 id = 区间序号 + 1）。
fn instance_ranges(scene: &Path) -> Result<Vec<(u32, u32)>, String> {
    let p = scene.join("instances.json");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut cursor = 0usize;
    let mut first: Option<u32> = None;
    while let Some(rel) = text[cursor..].find('"') {
        let at = cursor + rel;
        let rest = &text[at + 1..];
        if let Some(after) = rest.strip_prefix("first_id\":") {
            first = num(after);
            cursor = at + 1 + 9;
        } else if let Some(after) = rest.strip_prefix("last_id\":") {
            let last = num(after).ok_or("last_id 缺数值")?;
            let f = first.take().ok_or("last_id 前没有 first_id")?;
            out.push((f, last));
            cursor = at + 1 + 8;
        } else {
            cursor = at + 1;
        }
    }
    if out.is_empty() {
        return Err(format!("{}: 没有实例区间", p.display()));
    }
    Ok(out)
}

fn num(s: &str) -> Option<u32> {
    let t = s.trim_start();
    let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    t[..end].parse().ok()
}

fn seg_files(rd: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(rd.join("seg"))
        .unwrap_or_else(|e| {
            eprintln!("读不了 {}: {e}", rd.join("seg").display());
            std::process::exit(2)
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "npy"))
        .collect();
    v.sort();
    v
}

fn prep(argv: &[String]) {
    let renders = PathBuf::from(arg(argv, "--renders").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(arg(argv, "--scenes").unwrap_or_else(|| usage()));
    let limit: usize = arg(argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (mut nframes, mut unmapped) = (0usize, 0usize);
    for sd in scene_dirs(&scenes) {
        let name = sd.file_name().unwrap().to_string_lossy().to_string();
        let ranges = instance_ranges(&sd).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
        let rd = renders.join(&name);
        let out = rd.join("inst");
        let out_bnd = rd.join("bnd");
        for d in [&out, &out_bnd] {
            std::fs::create_dir_all(d).unwrap_or_else(|e| {
                eprintln!("建不了 {}: {e}", d.display());
                std::process::exit(2)
            });
        }
        for (i, f) in seg_files(&rd).iter().enumerate() {
            if limit > 0 && i >= limit {
                break;
            }
            let (ids, shape) = read_npy_i32(f).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2)
            });
            if shape.len() != 2 {
                eprintln!("{}: seg 不是二维", f.display());
                std::process::exit(2);
            }
            let inst = order_by_x(&ids, &ranges, shape[1]);
            unmapped += ids
                .iter()
                .zip(&inst)
                .filter(|(a, b)| **a > 0 && **b == 0)
                .count();
            let inst_i: Vec<i32> = inst.iter().map(|&v| v as i32).collect();
            write_npy_i32(&out.join(f.file_name().unwrap()), &inst_i, shape[1], shape[0])
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(2)
                });
            let bnd = boundary_of(&inst, shape[1], shape[0], 2);
            let bnd_i: Vec<i32> = bnd.iter().map(|&v| v as i32).collect();
            write_npy_i32(&out_bnd.join(f.file_name().unwrap()), &bnd_i, shape[1], shape[0])
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(2)
                });
            nframes += 1;
        }
        println!(
            "{name}: 实例 {}，帧 {}",
            ranges.len(),
            seg_files(&rd)
                .len()
                .min(if limit > 0 { limit } else { usize::MAX })
        );
    }
    println!("PREP frames={nframes} 未映射前景像素={unmapped}");
    if unmapped > 0 {
        std::process::exit(1);
    }
}

#[cfg(feature = "gpu")]
fn eval(argv: &[String]) {
    use trit_parts::api::{decode_ids, wgpu_from_tpm};
    use trit_parts::model::Backend;
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let renders = PathBuf::from(arg(argv, "--renders").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(arg(argv, "--scenes").unwrap_or_else(|| usage()));
    let comps = argv.iter().any(|a| a == "--components");
    let bnd_mode = argv.iter().any(|a| a == "--boundary");
    let res: u32 = arg(argv, "--res")
        .and_then(|v| v.parse().ok())
        .unwrap_or(384);
    let k: usize = arg(argv, "--k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(if comps { 5 } else { 2 });
    let limit: usize = arg(argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let close_r: usize = arg(argv, "--close")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let dump = arg(argv, "--dump").map(PathBuf::from);
    let dump_n: usize = arg(argv, "--dump-n")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let report: usize = arg(argv, "--report")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let m = TpmModel::load(&model).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let mut be = wgpu_from_tpm(&m).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });

    let mut miou: Vec<f64> = Vec::new();
    let mut fgis: Vec<f64> = Vec::new();
    let mut npred: Vec<usize> = Vec::new();
    let mut ngt: Vec<usize> = Vec::new();
    let mut cnt_ok = 0usize;
    let mut cnt_exact = 0usize;
    let mut frames = 0usize;
    for sd in scene_dirs(&scenes) {
        let name = sd.file_name().unwrap().to_string_lossy().to_string();
        let ranges = instance_ranges(&sd).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
        let pl = PartList {
            k,
            parts: (1..=if bnd_mode { 2 } else { ranges.len() as u32 })
                .map(|id| {
                    let mut t = id - 1;
                    let mut d = vec![0u8; k];
                    for i in (0..k).rev() {
                        d[i] = (t % 3) as u8;
                        t /= 3;
                    }
                    (id, d)
                })
                .collect(),
        };
        let rd = renders.join(&name);
        for (i, f) in seg_files(&rd).iter().enumerate() {
            if limit > 0 && i >= limit {
                break;
            }
            let stem = f.file_stem().unwrap().to_str().unwrap().to_string();
            let rgbp = rd.join("rgb").join(format!("{stem}.png"));
            let (w, h, pix) = read_rgb(&rgbp).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2)
            });
            if w as u32 != res || h as u32 != res {
                eprintln!("{}: {w}×{h} != res {res}", rgbp.display());
                std::process::exit(2);
            }
            let x = pix;
            let (ids, _) = read_npy_i32(f).unwrap();
            // GT 也只算**该帧可见**的实例（不可见实例无法被预测，不该进分母）
            let gt = order_by_x(&ids, &ranges, res as usize);
            let n_gt = gt.iter().max().copied().unwrap_or(0) as usize;
            let logits = be.forward(&x, res).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2)
            });
            let hw = (res * res) as usize;
            let ch = logits.len() / hw;
            if k * 3 >= ch {
                eprintln!("模型输出 {ch} 通道，不够 k={k}（需 {}）", k * 3 + 1);
                std::process::exit(2);
            }
            let pred: Vec<u32> = if comps {
                let fg: Vec<u32> = (0..hw)
                    .map(|p| (logits[p * ch + k * 3] > 0.0) as u32)
                    .collect();
                let fg = trit_parts::inst::close(&fg, res as usize, res as usize, close_r);
                components(&fg, res as usize, res as usize).0
            } else if bnd_mode {
                let dec = decode_ids(&logits, res, &pl);
                let fg: Vec<u32> = dec.iter().map(|&v| (v > 0) as u32).collect();
                let bd: Vec<u32> = dec.iter().map(|&v| (v == 2) as u32).collect();
                trit_parts::inst::split_by_boundary(&fg, &bd, res as usize, res as usize)
            } else {
                decode_ids(&logits, res, &pl)
            };
            if let Some(dd) = &dump
                && i < dump_n
            {
                std::fs::create_dir_all(dd).ok();
                let out = dd.join(format!("{name}_{stem}.npy"));
                let vals: Vec<i32> = pred.iter().map(|&v| v as i32).collect();
                let _ = write_npy_i32(&out, &vals, res as usize, res as usize);
            }
            let mt = match_labels(&pred, &gt, n_gt);
            miou.push(mt.miou);
            fgis.push(fg_iou(&pred, &gt));
            npred.push(mt.n_pred);
            ngt.push(n_gt);
            cnt_ok += ((mt.n_pred as i64 - n_gt as i64).abs() <= 1) as usize;
            cnt_exact += (mt.n_pred == n_gt) as usize;
            frames += 1;
            if report > 0 && frames.is_multiple_of(report) {
                println!(
                    "[{frames}] {name}/{stem} mIoU {:.3} fg {:.3} pred {} / gt {}",
                    mt.miou,
                    fgis.last().unwrap(),
                    mt.n_pred,
                    n_gt
                );
            }
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() { 0.0 } else { v[v.len() / 2] }
    };
    println!(
        "frames={frames} 实例 mIoU mean {:.4} median {:.4}（p10 {:.4}）| 前景 IoU mean {:.4} | pred/gt {:.2} | 计数±1 {:.1}% | 计数全对 {:.1}%",
        mean(&miou),
        median(&mut miou.clone()),
        {
            let mut s = miou.clone();
            s.sort_by(f64::total_cmp);
            if s.is_empty() { 0.0 } else { s[s.len() / 10] }
        },
        mean(&fgis),
        npred.iter().sum::<usize>() as f64 / ngt.iter().sum::<usize>().max(1) as f64,
        100.0 * cnt_ok as f64 / frames.max(1) as f64,
        100.0 * cnt_exact as f64 / frames.max(1) as f64
    );
}

#[cfg(not(feature = "gpu"))]
fn eval(_argv: &[String]) {
    eprintln!("本构建未启用 gpu feature");
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(|s| s.as_str()) {
        Some("prep") => prep(&argv),
        Some("eval") => eval(&argv),
        _ => usage(),
    }
}
