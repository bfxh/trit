//! WSP2G 分组容器：往返逐位一致、体积诚实性、坏输入、CLI 端到端。
//!
//! 体积那几条是重点：`wsp1g` 文本实测 **0.75×**（比 f32 还大），二进制必须把组号按
//! `ceil(log2 组数)` 位打包才对得起「压缩」这个词。漏数组号会把 2.6× 报成 15×，
//! 所以这里直接按**文件字节数**算，不按理论位宽算。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_codec::to_wspg;
use trit_wspace::{from_wsp2_grouped, is_grouped_wsp2, to_wsp2_grouped};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wsp2g-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-wspace")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().expect("run cli")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json")
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn reseal(bytes: &mut [u8]) {
    let end = bytes.len() - 8;
    let sum = fnv(&bytes[..end]);
    bytes[end..].copy_from_slice(&sum.to_le_bytes());
}

fn values(count: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 40) as f32 / f64::from(1u32 << 24) as f32) - 0.5
        })
        .collect()
}

fn sample_layer(rows: usize, cols: usize, group: usize) -> GroupedLayer {
    let weights = values(rows * cols, 7);
    let bias = values(rows, 11);
    GroupedLayer::quantize(rows, cols, &weights, &bias, group).expect("quantize")
}

#[test]
fn roundtrip_is_bit_exact() {
    let layer = sample_layer(16, 12, 4);
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    assert!(is_grouped_wsp2(&bytes));
    let back = from_wsp2_grouped(&bytes).expect("decode");
    assert_eq!(back.len(), 1);
    let got = &back[0];
    assert_eq!((got.rows(), got.cols()), (layer.rows(), layer.cols()));
    assert_eq!(got.group_size(), layer.group_size());
    assert_eq!(got.group_count(), layer.group_count());
    for row in 0..layer.rows() {
        for col in 0..layer.cols() {
            assert_eq!(got.trit(row, col), layer.trit(row, col), "({row},{col})");
            assert_eq!(
                got.group_of(row, col),
                layer.group_of(row, col),
                "({row},{col})"
            );
            assert_eq!(
                got.effective(row, col).expect("effective").to_bits(),
                layer.effective(row, col).expect("effective").to_bits(),
                "({row},{col})"
            );
        }
    }
    for (index, want) in layer.scales().iter().enumerate() {
        assert_eq!(got.scale(index).expect("scale").to_bits(), want.to_bits());
    }
    // 往返后再序列化必须逐字节相同
    let again = to_wsp2_grouped(std::slice::from_ref(got)).expect("re-encode");
    assert_eq!(again, bytes);
}

#[test]
fn f32_bias_roundtrips() {
    let layer = sample_layer(9, 5, 3);
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    let got = from_wsp2_grouped(&bytes).expect("decode");
    let outputs_a = layer.forward(&values(layer.cols(), 23)).expect("forward");
    let outputs_b = got[0].forward(&values(layer.cols(), 23)).expect("forward");
    for (a, b) in outputs_a.iter().zip(&outputs_b) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
}

#[test]
fn text_format_is_bigger_than_binary() {
    // wsp1g 每个 trit 一个 ASCII 字节、每个组号一段十进制文本；二进制应按位打包。
    let layer = sample_layer(64, 64, 16);
    let text = to_wspg(&layer);
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    let elements = layer.rows() * layer.cols();
    let bits = 8.0 * bytes.len() as f64 / elements as f64;
    // 2（trit）+ log2(组数=256) = 8 + 32/16 = 2 → 12 bit/权重；
    // 再加 bias f32（32/64 = 0.5 bit）、bias 2-bit 与头部，落在 12.6 附近。
    assert!(
        (12.0..13.0).contains(&bits),
        "每权重 {bits:.3} bit，应在 12.0–13.0（12 理论 + bias/头部开销）"
    );
    assert!(
        bytes.len() * 2 < text.len(),
        "二进制 {} 字节，文本 {} 字节：二进制应至少小一半",
        bytes.len(),
        text.len()
    );
    // 对比 f32：4 字节/权重
    let f32_bytes = elements * 4;
    assert!(
        bytes.len() * 2 < f32_bytes,
        "实测压缩 {:.2}×，应 > 2×",
        f32_bytes as f64 / bytes.len() as f64
    );
}

#[test]
fn group_ids_are_packed_at_declared_width() {
    // 组数 1024 → 10 位；若按 32 位直存，文件会大 3 倍以上。
    let layer = sample_layer(128, 128, 16);
    let elements = layer.rows() * layer.cols();
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    let group_count = layer.group_count();
    let gbits = u8::try_from(usize::BITS - (group_count - 1).leading_zeros()).expect("bits");
    let group_bytes = elements * usize::from(gbits) / 8;
    let trit_bytes = elements / 4;
    // 头部 7 + 每层 18 + scales + bias_f32 + 尾部 8
    let expected = 7
        + 18
        + group_count * 4
        + layer.rows() * 4
        + trit_bytes
        + layer.rows() / 4
        + group_bytes
        + 8;
    assert_eq!(bytes.len(), expected, "文件布局与理论长度不符");
    assert!(gbits <= 12, "组数 {group_count} 只需 {gbits} 位");
}

#[test]
fn tampered_checksum_is_rejected() {
    let layer = sample_layer(8, 8, 2);
    let mut bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    let error = from_wsp2_grouped(&bytes).expect_err("篡改必须被校验和抓住");
    assert!(format!("{error}").contains("校验和"), "{error}");
}

#[test]
fn truncated_is_rejected() {
    let layer = sample_layer(8, 8, 2);
    let bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    for cut in [0, 1, 4, 7, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            from_wsp2_grouped(&bytes[..cut]).is_err(),
            "截断到 {cut} 字节仍被接受"
        );
    }
}

#[test]
fn out_of_range_group_id_is_rejected() {
    // 5×4、组大小 4 → 5 组，组号 3 位 ⇒ 值 5..7 是空档（组数不是 2 的幂才有越界值可用）。
    let layer = sample_layer(5, 4, 4);
    assert_eq!(layer.group_count(), 5);
    let mut bytes = to_wsp2_grouped(std::slice::from_ref(&layer)).expect("encode");
    let elements = layer.rows() * layer.cols();
    let group_bytes = (elements * 3).div_ceil(8);
    let start = bytes.len() - 8 - group_bytes;
    // 第一个元素占第一个字节的低 3 位：改成 7（越界），重算校验和，
    // 让越界检查而不是校验和先抓到。
    bytes[start] = (bytes[start] & !0b111) | 0b111;
    reseal(&mut bytes);
    let error = from_wsp2_grouped(&bytes).expect_err("越界组号必须报错");
    let text = format!("{error}");
    assert!(text.contains("越界"), "{text}");
}

#[test]
fn cli_pack_unpack_roundtrip() {
    let dir = temp_dir("cli");
    let text_path = dir.join("layer.wsp1g");
    let bin_path = dir.join("layer.wsp2g");
    let back_path = dir.join("back.wsp1g");
    let layer = sample_layer(12, 7, 3);
    let text = to_wspg(&layer);
    fs::write(&text_path, &text).expect("write wsp1g");

    let report = stdout_json(&run(&[
        "pack-artifact",
        text_path.to_str().expect("path"),
        "--out",
        bin_path.to_str().expect("path"),
    ]));
    assert_eq!(report["format"], "wsp2g");
    let packed = fs::read(&bin_path).expect("read bin");
    assert!(is_grouped_wsp2(&packed));

    let report = stdout_json(&run(&[
        "unpack-artifact",
        bin_path.to_str().expect("path"),
        "--out",
        back_path.to_str().expect("path"),
    ]));
    assert_eq!(report["format"], "wsp2g");
    let back = fs::read_to_string(&back_path).expect("read text");
    assert_eq!(back, text, "pack→unpack 必须逐字节回到原文本");
}

#[test]
fn unpack_rejects_multi_layer_grouped_container() {
    let dir = temp_dir("multi");
    let a = sample_layer(4, 3, 2);
    let b = sample_layer(2, 2, 1);
    let bytes = to_wsp2_grouped(&[a, b]).expect("encode");
    let bin_path = dir.join("two.wsp2g");
    fs::write(&bin_path, &bytes).expect("write");
    let out_path = dir.join("out.wsp1g");
    let output = run(&[
        "unpack-artifact",
        bin_path.to_str().expect("path"),
        "--out",
        out_path.to_str().expect("path"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("2 层"), "{stderr}");
}
