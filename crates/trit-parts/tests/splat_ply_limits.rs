//! `splat::read_ply_3dgs` 的长度契约：`element vertex` 的顶点数与 property 条数
//! 都来自 PLY 头（外部文本），`n * stride` 必须 checked —— 否则大顶点数会让它
//! 溢出绕成小值、通过长度校验，紧接着 `Vec::with_capacity(n)` 直接 OOM。

use trit_parts::splat::read_ply_3dgs;

const PROPS: [&str; 17] = [
    "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
    "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
];

#[test]
fn absurd_vertex_count_is_rejected_before_capacity() {
    let d = std::env::temp_dir().join(format!("trit-ply-overflow-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("x.ply");
    // usize::MAX / 2 乘上 17 个 float 的 68 字节步长，32/64 位都会溢出。
    let huge = usize::MAX / 2;
    let mut txt = format!("ply\nformat binary_little_endian 1.0\nelement vertex {huge}\n");
    for k in PROPS {
        txt.push_str(&format!("property float {k}\n"));
    }
    txt.push_str("end_header\n");
    std::fs::write(&p, txt).unwrap();
    let err = match read_ply_3dgs(&p) {
        Ok(_) => panic!("顶点数 {huge} 必须在 with_capacity 之前被拒"),
        Err(e) => e,
    };
    assert!(err.contains("溢出"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}
