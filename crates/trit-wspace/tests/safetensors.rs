//! safetensors 外部权重读写：库级往返 + 坏输入 + CLI 端到端。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::safetensors::{list, read_tensor, write_tensor};
use trit_wspace::{Layer, from_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-safetensors-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// 写一个 safetensors 夹具；`entries` 是 `(name, dtype, [rows, cols], bytes)`。
fn fixture(path: &Path, entries: &[(&str, &str, [usize; 2], Vec<u8>)]) {
    let mut header = String::from("{");
    let mut data: Vec<u8> = Vec::new();
    for (index, (name, dtype, shape, bytes)) in entries.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        let start = data.len();
        data.extend_from_slice(bytes);
        header.push_str(&format!(
            r#""{name}":{{"dtype":"{dtype}","shape":[{},{}],"data_offsets":[{},{}]}}"#,
            shape[0],
            shape[1],
            start,
            data.len()
        ));
    }
    header.push('}');
    let mut header = header.into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&data);
    fs::write(path, out).expect("write fixture");
}

fn sample_f32() -> Vec<f32> {
    vec![0.5, -0.25, 0.0, 1.0, -1.0, 0.75]
}

#[test]
fn list_reports_shape_dtype_and_bytes() {
    let dir = temp_dir("list");
    let path = dir.join("one.safetensors");
    fixture(&path, &[("w", "F32", [2, 3], f32_bytes(&sample_f32()))]);
    let tensors = list(path.to_str().unwrap()).expect("list");
    assert_eq!(tensors.len(), 1);
    assert_eq!(tensors[0].name, "w");
    assert_eq!(tensors[0].dtype.name(), "F32");
    assert_eq!(tensors[0].shape, vec![2, 3]);
    assert_eq!(tensors[0].offsets, (0, 24));
}

#[test]
fn read_f32_is_exact_and_matches_from_f32_layer() {
    let dir = temp_dir("f32");
    let path = dir.join("one.safetensors");
    let values = sample_f32();
    fixture(&path, &[("w", "F32", [2, 3], f32_bytes(&values))]);
    let weights = read_tensor(path.to_str().unwrap(), "w").expect("read");
    assert_eq!((weights.rows, weights.cols), (2, 3));
    assert_eq!(weights.values, values);
    let layer = weights.to_layer().expect("layer");
    let want = Layer::from_f32(2, 3, &values, &[0.0, 0.0]).expect("want");
    assert_eq!(layer.trits(), want.trits());
    for (got, want) in layer.scales().iter().zip(want.scales()) {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }
}

#[test]
fn f16_values_decode() {
    let dir = temp_dir("f16");
    let path = dir.join("one.safetensors");
    // 1.0 / -2.0 / 0.33325（0x3555）
    let bits: [u16; 3] = [0x3c00, 0xc000, 0x3555];
    let bytes: Vec<u8> = bits.iter().flat_map(|bits| bits.to_le_bytes()).collect();
    fixture(&path, &[("w", "F16", [1, 3], bytes)]);
    let weights = read_tensor(path.to_str().unwrap(), "w").expect("read");
    assert!((weights.values[0] - 1.0).abs() < 1e-6);
    assert!((weights.values[1] + 2.0).abs() < 1e-6);
    assert!(
        (weights.values[2] - 0.33325195).abs() < 1e-6,
        "{}",
        weights.values[2]
    );
}

#[test]
fn f16_denormal_decodes() {
    let dir = temp_dir("f16-denormal");
    let path = dir.join("one.safetensors");
    // 最小次正规数 = 2^-24
    let bytes: Vec<u8> = 0x0001u16.to_le_bytes().to_vec();
    fixture(&path, &[("w", "F16", [1, 1], bytes)]);
    let weights = read_tensor(path.to_str().unwrap(), "w").expect("read");
    assert!(
        (weights.values[0] - 2f32.powi(-24)).abs() < 1e-12,
        "{}",
        weights.values[0]
    );
}

#[test]
fn bf16_values_decode() {
    let dir = temp_dir("bf16");
    let path = dir.join("one.safetensors");
    // 1.0 / -1.0 / 3.0
    let bits: [u16; 3] = [0x3f80, 0xbf80, 0x4040];
    let bytes: Vec<u8> = bits.iter().flat_map(|bits| bits.to_le_bytes()).collect();
    fixture(&path, &[("w", "BF16", [1, 3], bytes)]);
    let weights = read_tensor(path.to_str().unwrap(), "w").expect("read");
    assert_eq!(weights.values, vec![1.0, -1.0, 3.0]);
}

#[test]
fn write_read_round_trip_preserves_trits_and_scales() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("one.safetensors");
    let layer = Layer::from_f32(2, 2, &[0.5, -0.5, 0.5, 0.5], &[0.0, 0.0]).expect("layer");
    let (rows, cols) = (layer.rows(), layer.cols());
    let mut values = Vec::new();
    for row in 0..rows {
        let scale = layer.scale(row).expect("scale");
        for col in 0..cols {
            values.push(f32::from(layer.trit(row, col).expect("trit").to_i8()) * scale);
        }
    }
    write_tensor(path.to_str().unwrap(), "w", rows, cols, &values).expect("write");
    let back = read_tensor(path.to_str().unwrap(), "w").expect("read");
    let back = back.to_layer().expect("layer");
    assert_eq!(back.trits(), layer.trits());
    for (got, want) in back.scales().iter().zip(layer.scales()) {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }
}

fn raw_file(path: &Path, header: &str, data: &[u8]) {
    let mut header = header.as_bytes().to_vec();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    fs::write(path, out).expect("write raw");
}

#[test]
fn rejects_out_of_bounds_offsets() {
    let dir = temp_dir("oob");
    let path = dir.join("bad.safetensors");
    raw_file(
        &path,
        r#"{"w":{"dtype":"F32","shape":[1,2],"data_offsets":[0,8]}}"#,
        &[0u8; 4],
    );
    let error = list(path.to_str().unwrap()).expect_err("越界必须报错");
    assert!(format!("{error}").contains("越界"), "{error}");
}

#[test]
fn rejects_shape_byte_mismatch() {
    let dir = temp_dir("mismatch");
    let path = dir.join("bad.safetensors");
    raw_file(
        &path,
        r#"{"w":{"dtype":"F32","shape":[1,2],"data_offsets":[0,4]}}"#,
        &[0u8; 4],
    );
    let error = list(path.to_str().unwrap()).expect_err("字节数不符必须报错");
    assert!(format!("{error}").contains("需要 8"), "{error}");
}

#[test]
fn rejects_unknown_dtype_and_non_2d() {
    let dir = temp_dir("dtype");
    let path = dir.join("bad.safetensors");
    raw_file(
        &path,
        r#"{"w":{"dtype":"I64","shape":[1,1],"data_offsets":[0,8]}}"#,
        &[0u8; 8],
    );
    assert!(list(path.to_str().unwrap()).is_err());

    let path = dir.join("flat.safetensors");
    raw_file(
        &path,
        r#"{"w":{"dtype":"F32","shape":[4],"data_offsets":[0,16]}}"#,
        &[0u8; 16],
    );
    assert!(list(path.to_str().unwrap()).is_err());
}

#[test]
fn rejects_missing_tensor_and_non_finite() {
    let dir = temp_dir("missing");
    let path = dir.join("one.safetensors");
    fixture(&path, &[("w", "F32", [1, 1], f32_bytes(&[1.0]))]);
    assert!(read_tensor(path.to_str().unwrap(), "nope").is_err());

    let path = dir.join("nan.safetensors");
    fixture(&path, &[("w", "F32", [1, 1], f32_bytes(&[f32::NAN]))]);
    let error = read_tensor(path.to_str().unwrap(), "w").expect_err("NaN 必须报错");
    assert!(format!("{error}").contains("非有限"), "{error}");
}

#[test]
fn rejects_truncated_header() {
    let dir = temp_dir("truncated");
    let path = dir.join("bad.safetensors");
    fs::write(&path, 999u64.to_le_bytes()).expect("write");
    assert!(list(path.to_str().unwrap()).is_err());
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
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

#[test]
fn cli_list_import_export_round_trip() {
    let dir = temp_dir("cli");
    let path = dir.join("one.safetensors");
    fixture(&path, &[("w", "F32", [2, 3], f32_bytes(&sample_f32()))]);
    let path = path.to_str().unwrap();

    let listed = stdout_json(&run(&["list-safetensors", path]));
    assert_eq!(listed["tensors"][0]["name"], "w");
    assert_eq!(listed["tensors"][0]["shape"], serde_json::json!([2, 3]));

    let downloaded = dir.join("w.wsp");
    let imported = stdout_json(&run(&[
        "import-safetensors",
        path,
        "--tensor",
        "w",
        "--out",
        downloaded.to_str().unwrap(),
    ]));
    assert_eq!(imported["tensor"]["rows"], 2);
    let net = from_wsp(&fs::read_to_string(&downloaded).expect("wsp")).expect("parse wsp");

    let exported = dir.join("back.safetensors");
    let _ = stdout_json(&run(&[
        "export-safetensors",
        downloaded.to_str().unwrap(),
        "--out",
        exported.to_str().unwrap(),
        "--tensor",
        "w",
    ]));
    let back = read_tensor(exported.to_str().unwrap(), "w").expect("read back");
    assert_eq!(back.values.len(), 6);

    let again = stdout_json(&run(&[
        "import-safetensors",
        exported.to_str().unwrap(),
        "--tensor",
        "w",
    ]));
    let again = from_wsp(again["wsp1"].as_str().expect("wsp1")).expect("parse again");
    assert_eq!(again.layers()[0].trits(), net.layers()[0].trits());
}
