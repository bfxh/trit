//! 批量 CLI 的纯函数测试：帧选择、输出路径、均值口径。

use super::{frame_paths, mean_psnr, parse_frames};

#[test]
fn parses_all_range_and_csv() {
    assert_eq!(parse_frames("all", 4).unwrap(), vec![0, 1, 2, 3]);
    assert_eq!(parse_frames("1..3,5", 6).unwrap(), vec![1, 2, 5]);
    assert_eq!(parse_frames("0..,", 3).unwrap(), vec![0, 1, 2]);
}

#[test]
fn rejects_bad_frame_specs() {
    assert!(parse_frames("0..9", 3).is_err());
    assert!(parse_frames("1..1", 3).is_err());
    assert!(parse_frames("2,2", 3).is_err());
    assert!(parse_frames("x", 3).is_err());
}

#[test]
fn single_frame_keeps_output_path() {
    let paths = frame_paths("one.png", &[7]).unwrap();
    assert_eq!(paths[0].to_string_lossy(), "one.png");
}

#[test]
fn template_and_directory_output() {
    let tmp = std::env::temp_dir().join(format!("trit-splat-batch-{}", std::process::id()));
    let template = tmp.join("frame_{frame}.png");
    let paths = frame_paths(&template.to_string_lossy(), &[0, 12]).unwrap();
    assert!(paths[0].ends_with("frame_0000.png"));
    assert!(paths[1].ends_with("frame_0012.png"));

    let dir = tmp.join("frames");
    let paths = frame_paths(&dir.to_string_lossy(), &[0, 1]).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(dir.is_dir());
    assert!(paths[1].ends_with("frame_0001.png"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn multi_frame_png_target_is_rejected() {
    assert!(frame_paths("out.png", &[0, 1]).is_err());
}

#[test]
fn mean_psnr_handles_regular_infinite_and_empty() {
    assert!((mean_psnr(&[(0, Some(30.0)), (1, Some(40.0))]).unwrap() - 35.0).abs() < 1e-9);
    assert!(
        mean_psnr(&[(0, Some(30.0)), (1, Some(f64::INFINITY))])
            .unwrap()
            .is_infinite()
    );
    assert!(mean_psnr(&[]).is_none());
}
