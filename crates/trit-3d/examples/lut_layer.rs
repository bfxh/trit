//! 三值 LUT 层：加载 trit-build 的 TGPU，对拍 truth.csv，并在体素网格上套用规则。
//!
//! 运行：`cargo run -q -p trit-3d --example lut_layer -- <truth.gpu.bin> <truth.csv>`

use trit_3d::VoxelGrid;
use trit_3d::lut::{TernaryLut, apply_to_voxels};

fn main() {
    let bin = std::env::args()
        .nth(1)
        .expect("用法: lut_layer <truth.gpu.bin> <truth.csv>");
    let csv = std::env::args()
        .nth(2)
        .expect("用法: lut_layer <truth.gpu.bin> <truth.csv>");
    let lut = TernaryLut::load(std::path::Path::new(&bin)).expect("加载 TGPU");

    let text = std::fs::read_to_string(&csv).expect("读 CSV");
    let mut rows = 0usize;
    let mut mismatches = 0usize;
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        let code: usize = cols[0].trim().parse().expect("code");
        let verdict: i8 = cols[4].trim().parse().expect("verdict");
        if lut.eval_code(code) != verdict {
            mismatches += 1;
        }
        rows += 1;
    }

    let grid = VoxelGrid::cube(8, 2);
    let out = apply_to_voxels(&grid, &lut);
    let true_voxels = out.cells.iter().filter(|&&v| v == 1).count();
    let false_voxels = out.cells.iter().filter(|&&v| v == -1).count();
    let unknown_voxels = out.cells.iter().filter(|&&v| v == 0).count();

    println!(
        "{{\"nvars\":{},\"len\":{},\"csv_rows\":{},\"mismatches\":{},\"voxels_true\":{},\"voxels_false\":{},\"voxels_unknown\":{}}}",
        lut.nvars(),
        lut.len(),
        rows,
        mismatches,
        true_voxels,
        false_voxels,
        unknown_voxels
    );
}
