//! `parts-3d`：部件 id 图 + 深度 → 3D 点云（PLY），以及逐部件 3D IoU 评估。
//!
//! 四个子命令：
//! - `cloud`：GT seg + depth → 逐部件着色的 `.ply`；
//! - `infer`：模型预测 id + **外部深度**（任意来源，默认取 scene/depth）→ `.ply`（生产路径）；
//! - `eval`：模型预测 id + GT depth → 与 GT 反投影逐部件比对（3D IoU / 质心误差）；
//! - `mesh`：点云 → 实心占用 → Surface Nets 网格，并按 `--levels` 出 LOD 金字塔。

#[path = "parts_3d/cli.rs"]
mod cli;
#[path = "parts_3d/features.rs"]
mod features;
#[path = "parts_3d/light.rs"]
mod light;
#[path = "parts_3d/look.rs"]
mod look;
#[path = "parts_3d/material.rs"]
mod material;
#[path = "parts_3d/meshtex.rs"]
mod meshtex;
#[path = "parts_3d/occ.rs"]
mod occ;
#[path = "parts_3d/recon.rs"]
mod recon;
#[path = "parts_3d/splat.rs"]
mod splat;
#[path = "parts_3d/texgen.rs"]
mod texgen;
#[path = "parts_3d/texgen2.rs"]
mod texgen2;
#[path = "parts_3d/texstyle.rs"]
mod texstyle;

use cli::usage;
use light::{light_est_cmd, light_fit_cmd, light_relight_cmd, light_relight_normals_cmd};
use look::{look_cmd, style_cmd};
use material::{
    material_cnn_cmd, material_data_cmd, material_fit_cmd, material_predict_cmd, occl_stats_cmd,
};
use meshtex::mesh_tex_cmd;
use occ::occ_cmd;
use recon::{cloud, eval, fuse_cmd, infer, infer_ids_cmd, mesh_cmd};
use splat::{align_cmd, obs_split_cmd, splat_cmd};
use texgen::{tex_cmd, tex_conv_cmd, tex_prior_cmd};
use texgen2::tex_gen_cmd;
use texstyle::{tex_quilt_cmd, tex_stylize_cmd};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("cloud") => cloud(&argv),
        Some("mesh") => mesh_cmd(&argv),
        Some("fuse") => fuse_cmd(&argv),
        Some("splat") => splat_cmd(&argv),
        Some("occ") => occ_cmd(&argv),
        Some("align") => align_cmd(&argv),
        Some("obs-split") => obs_split_cmd(&argv),
        Some("mesh-tex") => mesh_tex_cmd(&argv),
        Some("tex") => tex_cmd(&argv),
        Some("tex-prior") => tex_prior_cmd(&argv),
        Some("tex-conv") => tex_conv_cmd(&argv),
        Some("tex-gen") => tex_gen_cmd(&argv),
        Some("tex-quilt") => tex_quilt_cmd(&argv),
        Some("light-est") => light_est_cmd(&argv),
        Some("light-relight") => light_relight_cmd(&argv),
        Some("light-fit") => light_fit_cmd(&argv),
        Some("light-relight-normals") => light_relight_normals_cmd(&argv),
        Some("material-data") => material_data_cmd(&argv),
        Some("material-fit") => material_fit_cmd(&argv),
        Some("material-predict") => material_predict_cmd(&argv),
        Some("material-cnn") => material_cnn_cmd(&argv),
        Some("occl-stats") => occl_stats_cmd(&argv),
        Some("tex-stylize") => tex_stylize_cmd(&argv),
        Some("look") => look_cmd(&argv),
        Some("style") => style_cmd(&argv),
        Some("infer") => {
            #[cfg(feature = "gpu")]
            infer(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        Some("infer-ids") => {
            #[cfg(feature = "gpu")]
            infer_ids_cmd(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        Some("eval") => {
            #[cfg(feature = "gpu")]
            eval(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        _ => usage(),
    }
}
