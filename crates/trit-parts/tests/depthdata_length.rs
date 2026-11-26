//! 第 40 轮 40-1 回归：`depthdata::load_sample` 在循环前核对 depth/seg 与图像像素数等长。
//!
//! 修复前：短 npy 在整图分支（`dep[si]`/`seg[si]`）与中心裁剪分支直接索引越界 panic，
//! debug/release 均可复现。修复后：长度对不上的场景走既有 `None` 通道判空返回，
//! 不新增错误变体，也不允许越界索引。

use std::fs;
use std::path::Path;

use trit_parts::data::write_rgb;
use trit_parts::depthdata::load_sample;

/// 小端 v1.0 `.npy` 夹具（与审计 driver 生成器同一格式口径）。
fn npy_bytes(descr: &str, shape: &[usize], payload: &[u8]) -> Vec<u8> {
    let shape_s = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!("{shape:?}")
    };
    let dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape_s}, }}",);
    let pad = (64 - (10 + dict.len() + 1) % 64) % 64;
    let mut header = format!("{dict}{}", " ".repeat(pad));
    header.push('\n');
    let mut out = Vec::new();
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(payload);
    out
}

fn f32_npy(values: &[f32]) -> Vec<u8> {
    let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    npy_bytes("<f4", &[values.len()], &payload)
}

fn i32_npy(values: &[i32]) -> Vec<u8> {
    let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    npy_bytes("<i4", &[values.len()], &payload)
}

/// IEEE CRC-32（仅用于手搓 0×0 PNG 的 IHDR/IEND 校验）。
fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut crc = i as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
        *entry = crc;
    }
    let mut crc = 0xFFFF_FFFF;
    for &byte in bytes {
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn png_chunk(tag: &[u8], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + data.len());
    body.extend_from_slice(tag);
    body.extend_from_slice(data);
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// 写一张宽高均为 0 的合法 PNG（解码器给出 w=0、h=0）。
fn write_zero_png(path: &Path) {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&0u32.to_be_bytes()); // width
    ihdr.extend_from_slice(&0u32.to_be_bytes()); // height
    ihdr.push(8); // bit depth
    ihdr.push(2); // RGB
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&png_chunk(b"IEND", &[]));
    fs::write(path, out).unwrap();
}

fn scene_dir(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "trit-parts-depthdata-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("rgb")).unwrap();
    fs::create_dir_all(dir.join("depth")).unwrap();
    fs::create_dir_all(dir.join("seg")).unwrap();
    dir
}

fn write_scene(dir: &Path, w: usize, h: usize, dep: &[f32], seg: &[i32]) {
    let pix: Vec<f32> = (0..w * h * 3).map(|_| 0.5).collect();
    write_rgb(&dir.join("rgb/000000.png"), w, h, &pix).unwrap();
    fs::write(dir.join("depth/000000.npy"), f32_npy(dep)).unwrap();
    fs::write(dir.join("seg/000000.npy"), i32_npy(seg)).unwrap();
}

/// 4×4、depth/seg 各 16 个合法前景元素：对照组必须正常出样本。
#[test]
fn matched_lengths_load_sample() {
    let dir = scene_dir("ok");
    let dep = vec![2.0f32; 16];
    let seg = vec![1i32; 16];
    write_scene(&dir, 4, 4, &dep, &seg);

    let crop = load_sample(&dir, 0, 4, false, 0, 0);
    assert!(crop.is_some(), "等长对照组（中心裁剪）必须出样本");
    let full = load_sample(&dir, 0, 4, true, 0, 0);
    assert!(full.is_some(), "等长对照组（整图降采样）必须出样本");

    let _ = fs::remove_dir_all(&dir);
}

/// 短 / 空 depth、seg：两个分支都不得越界 panic，统一判空返回 `None`。
#[test]
fn short_or_empty_arrays_return_none_without_panic() {
    let cases: &[(&str, usize, usize)] = &[
        ("dep0-seg16", 0, 16),
        ("dep16-seg0", 16, 0),
        ("dep1-seg1", 1, 1),
        ("dep8-seg8", 8, 8),
        ("dep16-seg1", 16, 1),
        ("dep1-seg16", 1, 16),
        ("dep8-seg16", 8, 16),
    ];
    for (tag, dep_len, seg_len) in cases {
        let dir = scene_dir(tag);
        let dep = vec![2.0f32; *dep_len];
        let seg = vec![1i32; *seg_len];
        write_scene(&dir, 4, 4, &dep, &seg);

        for full in [false, true] {
            let result = load_sample(&dir, 0, 4, full, 0, 0);
            assert!(
                result.is_none(),
                "{tag} full={full} 长度对不上必须判空返回，得到 {result:?}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// 超长数组（元素多于像素）：只取前 npix 个，不越界、不 panic。
#[test]
fn oversized_arrays_are_not_indexed_past_pixels() {
    let dir = scene_dir("over");
    let dep = vec![2.0f32; 32];
    let seg = vec![1i32; 32];
    write_scene(&dir, 4, 4, &dep, &seg);

    for full in [false, true] {
        let result = load_sample(&dir, 0, 4, full, 0, 0);
        assert!(
            result.is_some(),
            "超长数组不应被误判为短数组：full={full}，得到 {result:?}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// 0×0 图像：像素数为 0，窗口守卫在读取 npy 之前就判空，不得 panic。
#[test]
fn zero_pixel_image_is_rejected_before_indexing() {
    let dir = scene_dir("zero");
    write_zero_png(&dir.join("rgb/000000.png"));
    // depth/seg 同样写 0 个元素；即便图像能解码出 0×0，窗口也放不下 size=4。
    fs::write(dir.join("depth/000000.npy"), f32_npy(&[])).unwrap();
    fs::write(dir.join("seg/000000.npy"), i32_npy(&[])).unwrap();

    let result = load_sample(&dir, 0, 4, false, 0, 0);
    assert!(result.is_none(), "0×0 图像必须判空返回，得到 {result:?}");
    let _ = fs::remove_dir_all(&dir);
}
