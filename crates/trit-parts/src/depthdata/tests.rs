//! `depthdata` 的测试（从 `depthdata.rs` 下沉）：外部 `--size` / `--pyr` 的边界钉在这里。

use super::*;

/// 目录名与完整路径两种写法都要命中；相近名字不能误命中。
#[test]
fn holdout_accepts_bare_name_and_full_path() {
    let p = Path::new("E:/trit-data/s1/mat60/renders/scene0050");
    assert!(is_held(p, &["scene0050".to_string()]));
    assert!(is_held(
        p,
        &["E:/trit-data/s1/mat60/renders/scene0050/".to_string()]
    ));
    assert!(!is_held(p, &["scene0051".to_string()]));
    assert!(!is_held(p, &[]));
}

/// 金字塔池化：4×4 图、level 1（2×2 网格、块 2×2）→ 每块均值铺回；level 0 是 no-op。
#[test]
fn pyramid_pool_block_means() {
    let size = 4;
    let mut planes = vec![vec![0.0f32; size * size]; 3];
    for (i, v) in planes[0].iter_mut().enumerate() {
        *v = i as f32;
    }
    let before = planes.len();
    add_pyramid_pool(&mut planes, size, 0);
    assert_eq!(planes.len(), before, "levels=0 不应加通道");

    add_pyramid_pool(&mut planes, size, 1);
    assert_eq!(planes.len(), before + 3, "level 1 加 3 个通道");
    // 左上块 [0,1;4,5] 均值 2.5；右下块 [10,11;14,15] 均值 12.5
    assert!((planes[3][0] - 2.5).abs() < 1e-6);
    assert!((planes[3][5] - 2.5).abs() < 1e-6);
    assert!((planes[3][15] - 12.5).abs() < 1e-6);
    assert!(planes[4].iter().all(|v| *v == 0.0), "全零通道均值仍为 0");
    assert_eq!(planes[4].len(), size * size);

    let n = planes.len();
    add_pyramid_pool(&mut planes, size, 1);
    assert_eq!(planes.len(), n + 3);
}

/// 内容级指纹：同内容两次一致；**同尺寸改一个字节也必须变**。
#[test]
fn data_fingerprint_is_content_sensitive() {
    let base = std::env::temp_dir().join(format!("trit_dp_{}", std::process::id()));
    let scene = base.join("scene0000");
    for sub in ["rgb", "depth", "seg"] {
        std::fs::create_dir_all(scene.join(sub)).unwrap();
    }
    let rgb = scene.join("rgb").join("000000.png");
    let dep = scene.join("depth").join("000000.npy");
    let seg = scene.join("seg").join("000000.npy");
    std::fs::write(&rgb, b"rgb-bytes").unwrap(); // 9
    std::fs::write(&dep, b"depth-bytes").unwrap(); // 11
    std::fs::write(&seg, b"seg-bytes").unwrap(); // 9

    let (dirs, files, bytes, h1) = data_fingerprint(std::slice::from_ref(&scene), 2);
    let (_, _, _, h1b) = data_fingerprint(std::slice::from_ref(&scene), 2);
    assert_eq!((dirs, files, bytes), (1, 3, 29));
    assert_eq!(h1, h1b, "同内容两次结果必须一致");

    // 长度不变、内容改一个字节
    std::fs::write(&dep, b"depth-byteZ").unwrap();
    let (_, _, bytes2, h2) = data_fingerprint(std::slice::from_ref(&scene), 2);
    assert_eq!(bytes, bytes2, "字节数不变");
    assert_ne!(h1, h2, "内容变了指纹必须变");

    // 路径写法归一：`E:\x` 与 `E:/x` 必须同指纹（目录不存在时只哈希名字）
    let h3 = data_fingerprint(&[std::path::PathBuf::from(r"E:\none\scene")], 2).3;
    let h4 = data_fingerprint(&[std::path::PathBuf::from("E:/none/scene")], 2).3;
    assert_eq!(h3, h4);

    let _ = std::fs::remove_dir_all(&base);
}

/// `--size` 是外部数字：0 会让 `w / size` 除零，比图宽高还大会让 `w - size` 下溢绕成巨数。
/// `load_sample` 靠这一条判定在分配 `size²` 之前判空。
#[test]
fn window_must_fit_inside_the_image() {
    assert!(window_is_usable(8, 8, 8), "恰好等于边长要放行");
    assert!(window_is_usable(4, 8, 6), "两边都放得下");
    assert!(window_is_usable(1, 4096, 4096));
    assert!(!window_is_usable(0, 8, 8), "0 会让 w / size 除零");
    assert!(!window_is_usable(9, 8, 8), "比图宽还大会让 w - size 下溢");
    assert!(!window_is_usable(8, 4, 8), "高度不够同样下溢");
    assert!(!window_is_usable(64, 8, 8));
}

/// 降采样因子来自调用方的 `4^lvl`（`--pyr` 从 argv 来）：回绕成 0 时 `size / factor`
/// 除零，比网格还大时 `s = size / factor` 成为 0 并立刻越界索引。两种都该 no-op。
#[test]
fn lowpass_with_unusable_factor_is_noop() {
    let size = 4;
    let mut planes = vec![vec![0.5f32; size * size]; 3];
    let before = planes.clone();

    add_lowpass(&mut planes, size, 0);
    assert_eq!(planes, before, "factor = 0 是除零，必须 no-op");

    add_lowpass(&mut planes, size, size + 1);
    assert_eq!(planes, before, "factor > size 会索引越界，必须 no-op");

    add_lowpass(&mut planes, size, size);
    assert_eq!(
        planes.len(),
        before.len() + 3,
        "factor = size 合法，降成 1×1"
    );
    assert!(
        planes[3].iter().all(|v| (*v - 0.5).abs() < 1e-6),
        "1×1 均值仍是 0.5"
    );
}
