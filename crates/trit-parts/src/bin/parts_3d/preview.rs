//! `preview-scene`：对高斯资产做环绕新视角渲染，输出逐视角 PNG + 拼图 + 清单。
//!
//! ```text
//! parts-3d preview-scene --splat runs/compose/scene0084/composed.ply --out <目录> \
//!   [--views 8] [--res 256] [--elev 20] [--fov 40] [--cols 4]
//! ```
//!
//! 相机只由包围盒 + 仰角/视场角决定（见 `splat::orbit_camera`），同一资产在任何机器上
//! 得到同一组图；复核器逐像素重算。

use std::path::PathBuf;

use trit_parts::data::write_rgb;
use trit_parts::splat::{bounds, contact_sheet, orbit_camera, read_ply_3dgs, render};

use super::cli::{arg, fail, usage};

pub(crate) fn preview_scene_cmd(argv: &[String]) {
    let ply = PathBuf::from(arg(argv, "--splat").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let views: usize = arg(argv, "--views")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let res: usize = arg(argv, "--res")
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let elev: f32 = arg(argv, "--elev")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let fov: f32 = arg(argv, "--fov")
        .and_then(|v| v.parse().ok())
        .unwrap_or(40.0);
    let cols: usize = arg(argv, "--cols")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let bg: f32 = arg(argv, "--bg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    if views == 0 || views > 64 || res < 16 || res > 2048 {
        fail("--views 1..64、--res 16..2048");
    }
    let splats = read_ply_3dgs(&ply).unwrap_or_else(|e| fail(&e));
    if splats.is_empty() {
        fail("空资产");
    }
    let (lo, hi) = bounds(&splats);
    let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let half = (0..3).map(|k| (hi[k] - lo[k]) / 2.0).fold(0.0f32, f32::max);
    let fov_rad = f64::from(fov).to_radians();
    let radius = (f64::from(half) / (fov_rad / 2.0).tan() * 1.25) as f32;

    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    let mut frames = Vec::with_capacity(views);
    let mut entries = Vec::with_capacity(views);
    for i in 0..views {
        let yaw = 360.0 * i as f32 / views as f32;
        let cam = orbit_camera(center, radius, elev, fov, res, yaw);
        let img = render(&splats, &cam, res, res, [bg; 3]);
        let name = format!("view-{i:02}.png");
        let p = out.join(&name);
        write_rgb(&p, res, res, &img).unwrap_or_else(|e| fail(&e));
        let sha = trit_core::fingerprint::file_sha256(&p).unwrap_or_default();
        println!("视角 {i:02}（yaw {yaw:.0}°）→ {name}");
        entries.push(format!(
            "{{\"i\": {i}, \"yaw\": {yaw}, \"path\": {}, \"sha256\": \"{sha}\"}}",
            trit_parts::partsrun::json_str(&name)
        ));
        frames.push(img);
    }
    let (sw, sh, sheet) = contact_sheet(&frames, res, cols);
    let sp = out.join("preview.png");
    write_rgb(&sp, sw, sh, &sheet).unwrap_or_else(|e| fail(&e));
    let ssha = trit_core::fingerprint::file_sha256(&sp).unwrap_or_default();

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
    let nums = |v: [f32; 3]| format!("{}, {}, {}", v[0], v[1], v[2]);
    let src_sha = trit_core::fingerprint::file_sha256(&ply).unwrap_or_default();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-preview\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"views\": {views}, \"res\": {res}, \"elev\": {elev}, \"fov\": {fov}, \"cols\": {cols}, \"bg\": {bg}, \"radius\": {radius}}},\n  \"source\": {{\"path\": {}, \"sha256\": \"{src_sha}\", \"count\": {}}},\n  \"bounds\": {{\"min\": [{}], \"max\": [{}]}},\n  \"frames\": [{}],\n  \"sheet\": {{\"path\": \"preview.png\", \"sha256\": \"{ssha}\", \"w\": {sw}, \"h\": {sh}}}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        trit_parts::partsrun::json_str(&trit_parts::partsrun::rel_path(
            &out,
            &ply.canonicalize().unwrap_or_else(|_| ply.clone()),
        )),
        splats.len(),
        nums(lo),
        nums(hi),
        entries.join(", ")
    );
    let mp = out.join("preview.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("拼图 {sw}×{sh} → {}；清单 {}", sp.display(), mp.display());
}
