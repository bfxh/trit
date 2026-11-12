//! GGUF ↔ WSP1 CLI 闭环：导出、导入、逐字节往返、坏输入与目录约定。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_core::Ternary;
use trit_export::{MetaValue, read};
use trit_wspace::{Layer, Net, from_wsp, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-gguf-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args(args)
        .output()
        .expect("run cli")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout json")
}

/// 写一个与 `import-safetensors-dir` 相同约定的夹具目录，返回文件名列表。
fn fixture(dir: &Path) -> Vec<String> {
    let layers = [
        (
            "encoder.layer.0.weight",
            Net::new(vec![
                Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5])
                    .expect("l0"),
            ])
            .expect("net0"),
        ),
        (
            "encoder.layer.1.weight",
            Net::new(vec![Layer::new(1, 2, &[1, -1], &[0]).expect("l1")]).expect("net1"),
        ),
    ];
    let mut entries = Vec::new();
    let mut files = Vec::new();
    for (index, (name, net)) in layers.iter().enumerate() {
        let file = format!("{index:04}-{name}.wsp");
        fs::write(dir.join(&file), to_wsp(net)).expect("write wsp1");
        entries.push(json!({
            "name": name,
            "file": file,
            "rows": net.layers()[0].rows(),
            "cols": net.layers()[0].cols(),
        }));
        files.push(file);
    }
    let manifest = json!({ "source": "fixture", "tensors": entries });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).expect("manifest"),
    )
    .expect("write manifest");
    files
}

#[test]
fn export_then_import_round_trips_wsp1() {
    let dir = temp_dir("roundtrip");
    let files = fixture(&dir);
    let gguf = dir.join("model.gguf");
    let report = stdout_json(&run(&[
        "export-gguf",
        dir.to_str().expect("dir"),
        "--out",
        gguf.to_str().expect("gguf"),
    ]));
    assert_eq!(report["tensors"].as_array().expect("tensors").len(), 2);
    assert_eq!(report["trit_bytes"], 17);

    let model = read(&fs::read(&gguf).expect("read file")).expect("read gguf");
    assert_eq!(model.tensors.len(), 2);
    assert_eq!(model.tensors[0].name, "encoder.layer.0.weight");
    assert_eq!(model.tensors[0].rows, 2);
    assert_eq!(model.tensors[0].cols, 3);
    assert_eq!(model.tensors[0].scales, vec![0.5, 1.5]);
    assert_eq!(model.tensors[0].bias, vec![Ternary::True, Ternary::False]);
    assert_eq!(model.metadata.get("trit.wsp1"), Some(&MetaValue::U32(1)));

    let out = temp_dir("roundtrip-out");
    let report = stdout_json(&run(&[
        "import-gguf",
        gguf.to_str().expect("gguf"),
        "--out",
        out.to_str().expect("out"),
    ]));
    assert_eq!(report["tensors"].as_array().expect("tensors").len(), 2);
    assert_eq!(report["metadata"], json!(["trit.wsp1"]));

    for file in &files {
        let original = fs::read_to_string(dir.join(file)).expect("original");
        let imported = fs::read_to_string(out.join(file)).expect("imported");
        assert_eq!(original, imported, "{file} 往返不一致");
        assert_eq!(from_wsp(&imported).expect("parse").layers().len(), 1);
    }
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).expect("manifest"))
            .expect("json");
    assert_eq!(manifest["tensors"][0]["name"], "encoder.layer.0.weight");
    assert_eq!(manifest["tensors"][1]["name"], "encoder.layer.1.weight");
}

#[test]
fn import_rejects_non_gguf_bytes() {
    let dir = temp_dir("bad");
    let bad = dir.join("bad.gguf");
    fs::write(&bad, b"XXXX").expect("write");
    let out = run(&[
        "import-gguf",
        bad.to_str().expect("bad"),
        "--out",
        dir.join("out").to_str().expect("out"),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("GGUF"), "stderr: {stderr}");
}

#[test]
fn export_rejects_multi_layer_artifact() {
    let dir = temp_dir("multi");
    let net = Net::new(vec![
        Layer::new(2, 3, &[1, 0, -1, 0, 1, -1], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("net");
    fs::write(dir.join("multi.wsp"), to_wsp(&net)).expect("write");
    let manifest = json!({ "tensors": [{ "name": "multi", "file": "multi.wsp" }] });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("manifest"),
    )
    .expect("write manifest");
    let out = run(&[
        "export-gguf",
        dir.to_str().expect("dir"),
        "--out",
        dir.join("x.gguf").to_str().expect("gguf"),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("单层"));
}

#[test]
fn export_requires_manifest() {
    let dir = temp_dir("no-manifest");
    let out = run(&[
        "export-gguf",
        dir.to_str().expect("dir"),
        "--out",
        dir.join("x.gguf").to_str().expect("gguf"),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("manifest.json"));
}

#[test]
fn export_rejects_empty_tensor_list() {
    let dir = temp_dir("empty");
    fs::write(dir.join("manifest.json"), r#"{"tensors":[]}"#).expect("manifest");
    let out = run(&[
        "export-gguf",
        dir.to_str().expect("dir"),
        "--out",
        dir.join("x.gguf").to_str().expect("gguf"),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("没有张量"));
}

#[test]
fn export_drops_f32_bias_amplitude() {
    let dir = temp_dir("biasf");
    let layer = Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5])
        .expect("layer")
        .with_bias_f32(vec![0.25, -0.5])
        .expect("bias_f32");
    let net = Net::new(vec![layer]).expect("net");
    fs::write(dir.join("a.wsp"), to_wsp(&net)).expect("write");
    let manifest = json!({ "tensors": [{ "name": "a", "file": "a.wsp" }] });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("manifest"),
    )
    .expect("write manifest");
    let gguf = dir.join("a.gguf");
    stdout_json(&run(&[
        "export-gguf",
        dir.to_str().expect("dir"),
        "--out",
        gguf.to_str().expect("gguf"),
    ]));
    let out = temp_dir("biasf-out");
    stdout_json(&run(&[
        "import-gguf",
        gguf.to_str().expect("gguf"),
        "--out",
        out.to_str().expect("out"),
    ]));
    let text = fs::read_to_string(out.join("0000-a.wsp")).expect("wsp1");
    assert!(!text.contains("biasf"), "GGUF 不应保留 f32 bias：{text}");
    let net = from_wsp(&text).expect("parse");
    assert!(net.layers()[0].bias_f32().is_empty());
    assert_eq!(net.layers()[0].bias()[0].to_i8(), 1);
}
