//! safetensors 1D bias / vector 工件：符号、配对、`--vector` 往返。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::safetensors::{list, read_tensor};
use trit_wspace::{Layer, Net, from_wsp, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-bias-{tag}-{}", std::process::id()));
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

fn fixture(path: &std::path::Path, entries: &[(&str, &str, [usize; 2], Vec<u8>)]) {
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

fn raw_file(path: &std::path::Path, header: &str, data: &[u8]) {
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
fn to_layer_with_bias_uses_sign_and_checks_len() {
    let weights = read_tensor_from_values(2, 3, &[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    let layer = weights
        .to_layer_with_bias(Some(&[0.25, -0.75]))
        .expect("layer");
    let bias: Vec<i8> = layer.bias().iter().map(|trit| trit.to_i8()).collect();
    assert_eq!(bias, vec![1, -1]);
    let error = weights
        .to_layer_with_bias(Some(&[0.25]))
        .expect_err("长度不符必须报错");
    assert!(format!("{error}").contains("bias"), "{error}");
}

fn read_tensor_from_values(
    rows: usize,
    cols: usize,
    values: &[f32],
) -> trit_wspace::safetensors::Weights {
    static INLINE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let index = INLINE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("trit-bias-inline-{}-{index}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("w.safetensors");
    fixture(&path, &[("w", "F32", [rows, cols], f32_bytes(values))]);
    read_tensor(path.to_str().unwrap(), "w").expect("read")
}

#[test]
fn cli_import_with_bias_writes_signs_into_layer() {
    let dir = temp_dir("cli-bias");
    let path = dir.join("one.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.25, -0.75]));
    raw_file(
        &path,
        r#"{"w":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"b":{"dtype":"F32","shape":[2],"data_offsets":[24,32]}}"#,
        &data,
    );
    let imported = stdout_json(&run(&[
        "import-safetensors",
        path.to_str().unwrap(),
        "--tensor",
        "w",
        "--bias",
        "b",
    ]));
    assert_eq!(imported["tensor"]["bias"], "b");
    let net = from_wsp(imported["wsp1"].as_str().expect("wsp1")).expect("parse");
    let bias: Vec<i8> = net.layers()[0]
        .bias()
        .iter()
        .map(|trit| trit.to_i8())
        .collect();
    assert_eq!(bias, vec![1, -1]);
}

#[test]
fn cli_import_rejects_bias_with_multiple_tensors() {
    let dir = temp_dir("cli-bias-multi");
    let path = dir.join("two.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.5, -0.5]));
    raw_file(
        &path,
        r#"{"w0":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"w1":{"dtype":"F32","shape":[1,2],"data_offsets":[24,32]}}"#,
        &data,
    );
    let output = run(&[
        "import-safetensors",
        path.to_str().unwrap(),
        "--tensor",
        "w0,w1",
        "--bias",
        "w1",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--bias 只支持单个"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_dir_pairs_weight_with_same_stem_bias() {
    let dir = temp_dir("cli-dir-bias");
    let path = dir.join("paired.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.25, -0.75]));
    raw_file(
        &path,
        r#"{"x.weight":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"x.bias":{"dtype":"F32","shape":[2],"data_offsets":[24,32]}}"#,
        &data,
    );
    let out = dir.join("tensors");
    let report = stdout_json(&run(&[
        "import-safetensors-dir",
        path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]));
    assert_eq!(report["tensors"][1]["bias"], "x.bias");
    let wsp = fs::read_dir(&out)
        .expect("dir")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with("x.weight.wsp")
        })
        .expect("wsp file");
    let net = from_wsp(&fs::read_to_string(wsp.path()).expect("wsp")).expect("parse");
    let bias: Vec<i8> = net.layers()[0]
        .bias()
        .iter()
        .map(|trit| trit.to_i8())
        .collect();
    assert_eq!(bias, vec![1, -1]);
}

#[test]
fn cli_dir_ignores_bias_with_wrong_len() {
    let dir = temp_dir("cli-dir-bias-bad");
    let path = dir.join("bad.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.25, -0.75, 0.5]));
    raw_file(
        &path,
        r#"{"y.weight":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"y.bias":{"dtype":"F32","shape":[3],"data_offsets":[24,36]}}"#,
        &data,
    );
    let out = dir.join("tensors");
    let report = stdout_json(&run(&[
        "import-safetensors-dir",
        path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]));
    let weight = report["tensors"]
        .as_array()
        .expect("tensors")
        .iter()
        .find(|tensor| tensor["name"] == "y.weight")
        .expect("y.weight");
    assert!(weight["bias"].is_null());
    let wsp = fs::read_dir(&out)
        .expect("dir")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with("y.weight.wsp")
        })
        .expect("wsp file");
    let net = from_wsp(&fs::read_to_string(wsp.path()).expect("wsp")).expect("parse");
    let bias: Vec<i8> = net.layers()[0]
        .bias()
        .iter()
        .map(|trit| trit.to_i8())
        .collect();
    assert_eq!(bias, vec![0, 0]);
}

#[test]
fn cli_import_vector_and_export_vector_shape() {
    let dir = temp_dir("cli-vector");
    let path = dir.join("vector.safetensors");
    // fixture 只支持 2D，这里手写一个真 1D 张量。
    let data = f32_bytes(&[0.5, -0.5, 0.25]);
    raw_file(
        &path,
        &format!(
            r#"{{"v":{{"dtype":"F32","shape":[3],"data_offsets":[0,{}]}}}}"#,
            data.len()
        ),
        &data,
    );
    let imported = stdout_json(&run(&[
        "import-safetensors",
        path.to_str().unwrap(),
        "--tensor",
        "v",
        "--out",
        dir.join("v.wsp").to_str().unwrap(),
    ]));
    assert_eq!(imported["tensor"]["vector"], true);
    let wsp = dir.join("v.wsp");
    let exported = dir.join("v-back.safetensors");
    stdout_json(&run(&[
        "export-safetensors",
        wsp.to_str().unwrap(),
        "--out",
        exported.to_str().unwrap(),
        "--tensor",
        "v",
        "--vector",
    ]));
    let listed = list(exported.to_str().unwrap()).expect("list");
    assert_eq!(listed[0].shape, vec![3]);
    let back = read_tensor(exported.to_str().unwrap(), "v");
    assert!(back.is_err(), "1D 不能再作为 2D 读");
    let vector =
        trit_wspace::safetensors::read_vector(exported.to_str().unwrap(), "v").expect("vector");
    assert_eq!(vector.len(), 3);
}

#[test]
fn wsp1_round_trip_preserves_bias_f32() {
    let weights = read_tensor_from_values(2, 3, &[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    let layer = weights
        .to_layer_with_bias(Some(&[0.25, -0.75]))
        .expect("layer");
    assert_eq!(layer.bias_f32(), &[0.25, -0.75]);
    let net = Net::new(vec![layer]).expect("net");
    let text = to_wsp(&net);
    assert!(text.contains("biasf"), "{text}");
    let back = from_wsp(&text).expect("parse");
    assert_eq!(back.layers()[0].bias_f32(), &[0.25, -0.75]);
    assert_eq!(back.layers()[0].bias(), net.layers()[0].bias());
}

#[test]
fn zero_bias_has_no_biasf() {
    let layer = Layer::from_f32(1, 2, &[0.5, -0.5], &[0.0]).expect("layer");
    assert!(layer.bias_f32().is_empty());
    let text = to_wsp(&Net::new(vec![layer]).expect("net"));
    assert!(!text.contains("biasf"), "{text}");
}

#[test]
fn export_with_bias_writes_f32_values() {
    let dir = temp_dir("export-bias");
    let path = dir.join("one.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.25, -0.75]));
    raw_file(
        &path,
        r#"{"w":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"b":{"dtype":"F32","shape":[2],"data_offsets":[24,32]}}"#,
        &data,
    );
    let wsp = dir.join("w.wsp");
    let _ = stdout_json(&run(&[
        "import-safetensors",
        path.to_str().unwrap(),
        "--tensor",
        "w",
        "--bias",
        "b",
        "--out",
        wsp.to_str().unwrap(),
    ]));
    let exported = dir.join("back.safetensors");
    let report = stdout_json(&run(&[
        "export-safetensors",
        wsp.to_str().unwrap(),
        "--out",
        exported.to_str().unwrap(),
        "--tensor",
        "w",
        "--bias",
    ]));
    assert_eq!(report["bias"], "w.bias");
    let vector = trit_wspace::safetensors::read_vector(exported.to_str().unwrap(), "w.bias")
        .expect("bias vector");
    assert!((vector[0] - 0.25).abs() < 1e-6, "{vector:?}");
    assert!((vector[1] + 0.75).abs() < 1e-6, "{vector:?}");
}

#[test]
fn export_dir_writes_bias_back() {
    let dir = temp_dir("dir-bias");
    let path = dir.join("paired.safetensors");
    let mut data = f32_bytes(&[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    data.extend_from_slice(&f32_bytes(&[0.25, -0.75]));
    raw_file(
        &path,
        r#"{"x.weight":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"x.bias":{"dtype":"F32","shape":[2],"data_offsets":[24,32]}}"#,
        &data,
    );
    let tensors = dir.join("tensors");
    let _ = stdout_json(&run(&[
        "import-safetensors-dir",
        path.to_str().unwrap(),
        "--out",
        tensors.to_str().unwrap(),
    ]));
    let exported = dir.join("back.safetensors");
    let _ = stdout_json(&run(&[
        "export-safetensors-dir",
        tensors.to_str().unwrap(),
        "--out",
        exported.to_str().unwrap(),
    ]));
    let listed = list(exported.to_str().unwrap()).expect("list");
    let names: Vec<&str> = listed.iter().map(|tensor| tensor.name.as_str()).collect();
    assert!(names.contains(&"x.bias"), "{names:?}");
    let vector = trit_wspace::safetensors::read_vector(exported.to_str().unwrap(), "x.bias")
        .expect("bias vector");
    assert!((vector[0] - 0.25).abs() < 1e-6, "{vector:?}");
    assert!((vector[1] + 0.75).abs() < 1e-6, "{vector:?}");
}

#[test]
fn wsp2_rejects_biasf() {
    let dir = temp_dir("wsp2-bias");
    let weights = read_tensor_from_values(2, 3, &[0.5, -0.5, 0.5, 0.5, 0.5, -0.5]);
    let layer = weights
        .to_layer_with_bias(Some(&[0.25, -0.75]))
        .expect("layer");
    let wsp = dir.join("w.wsp");
    fs::write(&wsp, to_wsp(&Net::new(vec![layer]).expect("net"))).expect("wsp");
    let binary = dir.join("w.wsp2");
    let output = run(&[
        "pack-artifact",
        wsp.to_str().unwrap(),
        "--out",
        binary.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("f32 bias"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
