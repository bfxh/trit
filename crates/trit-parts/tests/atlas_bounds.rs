//! 图集烘焙的三处外部数字：`--atlas` 边长、贴图 `tw×th×3` 声明、`--texels` 密度。
//!
//! `--atlas` 直接决定 `img` 的 `atlas_px² × 3` 个 f32；调用方只判下限 64（`asset`）
//! 或非零（`meshtex` / `objects-mesh`），一个 `65536` 就是 51 GB。贴图长度的校验
//! 用 `tw * th * 3` 裸乘，绕回后可以等于任何实际长度。密度则会从 `f32` 饱和成
//! `usize::MAX`，让 `cx + w + pad` 绕回成小值、把「放不下」判断反过来通过。

use trit_parts::atlas::bake_atlas;
use trit_parts::mesh::Mesh;

fn cube() -> Mesh {
    let verts = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let tris = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    Mesh { verts, tris }
}

/// `unwrap_err` 要求 `Atlas` 实现 `Debug`；这里只取错误串。
fn err_of<T>(r: Result<T, String>) -> String {
    match r {
        Ok(_) => panic!("必须被拒"),
        Err(e) => e,
    }
}

/// `atlas_px = 2^30`：`atlas_px² × 3` 个 f32 是 1.4e19 字节，超过 `isize::MAX`，
/// 修前 `Vec::with_capacity` 直接 `capacity overflow`。
#[test]
fn absurd_atlas_side_is_rejected_before_the_image_buffer() {
    let m = cube();
    let tex = vec![0.5f32; 32 * 32 * 3];
    let e = err_of(bake_atlas(
        &m,
        &tex,
        32,
        32,
        1usize << 30,
        60.0,
        45.0,
        false,
        None,
    ));
    assert!(e.contains("超出 1..="), "{e}");
}

/// `tw = th = 2^33` 时 `tw * th * 3` 绕成 0 —— 和空贴图对上了，
/// 修前一路走到 `sample` 里 `tex[2^33]` 才越界。
#[test]
fn texture_shape_overflow_is_rejected_before_reading_pixels() {
    let m = cube();
    let tex: Vec<f32> = Vec::new();
    let big = 1usize << 33;
    let e = err_of(bake_atlas(&m, &tex, big, big, 256, 60.0, 45.0, false, None));
    assert!(e.contains("尺寸相乘溢出"), "{e}");
}

/// `--texels 1e38`：像素边长从 `f32` 饱和成 `usize::MAX`，`cx + w + pad` 绕回成 1，
/// 「放不下」判断因此通过，修前最后写 `img[usize::MAX]`。
#[test]
fn absurd_texel_density_is_rejected_instead_of_index_out_of_range() {
    let m = cube();
    let tex = vec![0.5f32; 8 * 8 * 3];
    let e = err_of(bake_atlas(
        &m,
        &tex,
        8,
        8,
        1024,
        f32::MAX,
        45.0,
        false,
        None,
    ));
    assert!(e.contains("图集打包失败"), "{e}");
}

/// `bake_atlas_with` 直通 `bake_shaded`，是另一条入口。上限若只写在 `bake_atlas`
/// 里，从这里绕过去仍然是 `atlas_px² × 3` 个 f32 的分配。
#[test]
fn bake_atlas_with_shares_the_same_atlas_cap() {
    let m = cube();
    let e = err_of(trit_parts::atlas::bake_atlas_with(
        &m,
        1usize << 30,
        60.0,
        45.0,
        [0.5; 3],
        |_fi, _p| None,
    ));
    assert!(e.contains("超出 1..="), "{e}");
}
