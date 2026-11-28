//! splat-render：读 3DGS .ply/.spz + nerfstudio transforms.json，渲染帧出 PNG。
//!
//! 单帧与批量共用同一资产读取路径；批量选项见 `batch.rs`。

#[path = "splat-render/batch.rs"]
mod batch;
#[path = "splat-render/cli.rs"]
mod cli;
#[path = "splat-render/mip.rs"]
mod mip;

fn main() -> std::process::ExitCode {
    batch::run(std::env::args().collect())
}
