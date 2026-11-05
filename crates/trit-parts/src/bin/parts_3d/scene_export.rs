//! `export-scene`：一次跑完「网格 LOD + 纹理 + 高斯」，写一个场景总清单。
//!
//! ```text
//! parts-3d export-scene --model <occ1> --scene <dir> --frames 0,...,7 --out <dir> \
//!   [--cells 0.01,0.02,0.04] [--texture] [--atlas 1024] \
//!   [--splat-frames 0,2,4,6] [--splat-eval 1,3,5,7] [--splat-cell 0.01] \
//!   [--splat-fit 30] [--splat-fit-all]
//! ```
//!
//! 子产物分别由 `export-asset` / `splat-export` 生成（各自带清单），本命令只做编排与
//! 总清单；这样每个子产物仍能单独复核，总包也只记它们的清单哈希，不复制读数。

use std::path::{Path, PathBuf};

use super::cli::{arg, fail, usage};

fn run(bin: &Path, args: &[String], what: &str) -> String {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| fail(&format!("跑不了 {}: {e}", bin.display())));
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        fail(&format!("{what} 退出码 {:?}", out.status.code()));
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn sibling(name: &str) -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

/// 去掉 Windows `\\?\` 前缀：清单里存普通路径，别的工具才认得。
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest.to_string());
    }
    p
}

fn count_entries(txt: &str, key: &str) -> usize {
    let pat = format!("\"{key}\": [");
    let Some(i) = txt.find(&pat) else {
        return 0;
    };
    let rest = &txt[i + pat.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end].matches("{\"cell\": ").count()
}

/// 数 `"key": [ {"id": ...` 形式的条目（对象/对象网格清单用这个）。
/// 取 `"key": [ ... ]` 的数组体（支持嵌套，跳过字符串）。
fn array_body<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": [");
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
        } else if ch == '[' {
            depth += 1;
        } else if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
}

fn count_ids(txt: &str, key: &str) -> usize {
    array_body(txt, key)
        .map(|b| b.matches("{\"id\": ").count())
        .unwrap_or(0)
}

fn num_field(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn sha(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

struct Sub {
    json: String,
}

impl Sub {
    fn read(p: &Path) -> Self {
        Sub {
            json: std::fs::read_to_string(p)
                .unwrap_or_else(|e| fail(&format!("{}: {e}", p.display()))),
        }
    }
    fn num(&self, k: &str) -> f64 {
        num_field(&self.json, k).unwrap_or(0.0)
    }
    /// 在某个对象体之后找字段（同名键会出现在多处，如 layout 与 composed 的 count）。
    fn num_after(&self, anchor: &str, k: &str) -> f64 {
        self.json
            .find(anchor)
            .and_then(|i| num_field(&self.json[i..], k))
            .unwrap_or(0.0)
    }
}

pub(crate) fn scene_export_cmd(argv: &[String]) {
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames = arg(argv, "--frames").unwrap_or_else(|| "0,1,2,3,4,5,6,7".into());
    let cells = arg(argv, "--cells").unwrap_or_else(|| "0.01,0.02,0.04".into());
    let texture = argv.iter().any(|a| a == "--texture");
    let atlas: usize = arg(argv, "--atlas")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let sfit = arg(argv, "--splat-frames").unwrap_or_else(|| "0,2,4,6".into());
    let seval = arg(argv, "--splat-eval").unwrap_or_else(|| "1,3,5,7".into());
    let scell: f32 = arg(argv, "--splat-cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let sfit_iters: usize = arg(argv, "--splat-fit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let sfit_all = argv.iter().any(|a| a == "--splat-fit-all");
    let scale_k: f32 = arg(argv, "--scale-k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6);
    let alpha: f32 = arg(argv, "--alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.8);
    let full = argv.iter().any(|a| a == "--full");
    let obj_atlas: usize = arg(argv, "--obj-atlas")
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let cov_min: f64 = arg(argv, "--cov-min")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.4);
    let preview_views: usize = arg(argv, "--preview-views")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let preview_res: usize = arg(argv, "--preview-res")
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let parts3d = arg(argv, "--parts-3d")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-3d"));

    let mesh_dir = out.join("mesh");
    let splat_dir = out.join("splat");
    std::fs::create_dir_all(&mesh_dir)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", mesh_dir.display())));
    std::fs::create_dir_all(&splat_dir)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", splat_dir.display())));
    // 清单里记绝对路径：复核从任意 cwd 跑都能找到子清单
    let out = plain(
        out.canonicalize()
            .unwrap_or_else(|e| fail(&format!("{}: {e}", out.display()))),
    );
    let mesh_dir = out.join("mesh");
    let splat_dir = out.join("splat");

    let mut margs: Vec<String> = vec![
        "export-asset".into(),
        "--model".into(),
        model.display().to_string(),
        "--scene".into(),
        scene.display().to_string(),
        "--frames".into(),
        frames.clone(),
        "--cells".into(),
        cells.clone(),
        "--out".into(),
        mesh_dir.display().to_string(),
    ];
    if texture {
        margs.push("--texture".into());
        margs.push("--atlas".into());
        margs.push(atlas.to_string());
    }
    run(&parts3d, &margs, "export-asset");

    let splat_ply = splat_dir.join("scene.ply");
    let mut sargs: Vec<String> = vec![
        "splat-export".into(),
        "--scene".into(),
        scene.display().to_string(),
        "--frames".into(),
        sfit.clone(),
        "--eval".into(),
        seval.clone(),
        "--cell".into(),
        format!("{scell}"),
        "--scale-k".into(),
        format!("{scale_k}"),
        "--alpha".into(),
        format!("{alpha}"),
        "--fit".into(),
        sfit_iters.to_string(),
        "--out".into(),
        splat_ply.display().to_string(),
    ];
    if sfit_all {
        sargs.push("--fit-all".into());
    }
    run(&parts3d, &sargs, "splat-export");

    let mm = mesh_dir.join("asset.manifest.json");
    let sm = PathBuf::from(format!("{}.manifest.json", splat_ply.display()));
    let mesh_txt = Sub::read(&mm);
    let splat_txt = Sub::read(&sm);
    let msha = sha(&mm);
    let ssha = sha(&sm);
    let levels = count_entries(&mesh_txt.json, "levels");
    let count = splat_txt.num("count") as usize;

    // `--full`：对象 → 合成 → 对象网格 → 预览，各写各的清单
    let mut full_parts = String::new();
    if full {
        let obj_dir = out.join("objects");
        let comp_dir = out.join("compose");
        let om_dir = out.join("objects-mesh");
        let prev_dir = out.join("preview");

        run(
            &parts3d,
            &[
                "export-objects".into(),
                "--scene".into(),
                scene.display().to_string(),
                "--frames".into(),
                frames.clone(),
                "--cell".into(),
                format!("{scell}"),
                "--out".into(),
                obj_dir.display().to_string(),
            ],
            "export-objects",
        );
        run(
            &parts3d,
            &[
                "compose-scene".into(),
                "--objects".into(),
                obj_dir.display().to_string(),
                "--out".into(),
                comp_dir.display().to_string(),
                "--cov-min".into(),
                format!("{cov_min}"),
            ],
            "compose-scene",
        );
        let compose_manifest = comp_dir.join("compose.manifest.json");
        run(
            &parts3d,
            &[
                "export-object-meshes".into(),
                "--scene".into(),
                scene.display().to_string(),
                "--objects".into(),
                obj_dir.display().to_string(),
                "--out".into(),
                om_dir.display().to_string(),
                "--cell".into(),
                format!("{scell}"),
                "--atlas".into(),
                obj_atlas.to_string(),
                "--layout".into(),
                compose_manifest.display().to_string(),
            ],
            "export-object-meshes",
        );
        run(
            &parts3d,
            &[
                "preview-scene".into(),
                "--splat".into(),
                comp_dir.join("composed.ply").display().to_string(),
                "--out".into(),
                prev_dir.display().to_string(),
                "--views".into(),
                preview_views.to_string(),
                "--res".into(),
                preview_res.to_string(),
            ],
            "preview-scene",
        );

        let obj_m = obj_dir.join("objects.manifest.json");
        let om_m = om_dir.join("objects-mesh.manifest.json");
        let prev_m = prev_dir.join("preview.manifest.json");
        let obj_txt = Sub::read(&obj_m);
        let comp_txt = Sub::read(&compose_manifest);
        let om_txt = Sub::read(&om_m);
        let _ = Sub::read(&prev_m);
        let n_obj = count_ids(&obj_txt.json, "objects");
        let composed = comp_txt.num_after("\"composed\": {", "count");
        let cov = comp_txt.num_after("\"gate\": {", "coverage");
        let n_omesh = count_ids(&om_txt.json, "objects");
        println!(
            "总包：对象 {n_obj} 个、合成 {composed} 高斯（覆盖 {cov:.3}）、对象网格 {n_omesh} 份"
        );
        full_parts = format!(
            ", \"objects\": {{\"manifest\": {{\"path\": \"objects/objects.manifest.json\", \"sha256\": \"{}\"}}, \"count\": {n_obj}}}, \"compose\": {{\"manifest\": {{\"path\": \"compose/compose.manifest.json\", \"sha256\": \"{}\"}}, \"count\": {composed}, \"coverage\": {cov}}}, \"objects_mesh\": {{\"manifest\": {{\"path\": \"objects-mesh/objects-mesh.manifest.json\", \"sha256\": \"{}\"}}, \"count\": {n_omesh}}}, \"preview\": {{\"manifest\": {{\"path\": \"preview/preview.manifest.json\", \"sha256\": \"{}\"}}, \"views\": {preview_views}, \"sheet\": \"preview/preview.png\"}}",
            sha(&obj_m),
            sha(&compose_manifest),
            sha(&om_m),
            sha(&prev_m),
        );
    }

    let commit = trit_parts::partsrun::git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !trit_parts::partsrun::git_out(&["status", "--porcelain"])
        .trim()
        .is_empty();
    let argv_json: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| trit_parts::partsrun::json_str(&a))
        .collect();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-scene\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"mesh_frames\": [{}], \"splat_fit_frames\": [{}], \"splat_eval_frames\": [{}], \"cells\": [{}], \"texture\": {texture}, \"atlas\": {atlas}, \"splat_cell\": {scell}, \"splat_fit\": {sfit_iters}, \"splat_fit_all\": {sfit_all}, \"full\": {full}, \"obj_atlas\": {obj_atlas}, \"cov_min\": {cov_min}}},\n  \"scene\": {},\n  \"parts\": {{\"mesh\": {{\"manifest\": {{\"path\": {}, \"sha256\": \"{msha}\"}}, \"levels\": {levels}, \"texture\": {texture}}}, \"splat\": {{\"manifest\": {{\"path\": {}, \"sha256\": \"{ssha}\"}}, \"count\": {count}}}{full_parts}}}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames,
        sfit,
        seval,
        cells,
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        trit_parts::partsrun::json_str("mesh/asset.manifest.json"),
        trit_parts::partsrun::json_str("splat/scene.ply.manifest.json"),
    );
    let mp = out.join("scene.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!(
        "场景包 {}：网格 {levels} 档、高斯 {count} 个{}",
        out.display(),
        if full { "（full）" } else { "" }
    );
    println!("总清单 {}", mp.display());
}
