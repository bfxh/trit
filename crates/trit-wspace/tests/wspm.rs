//! WSPM 批量容器：往返、格式选择、清单语义、坏输入与 CLI 闭环。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::wspm::{self, ModelItem};
use trit_wspace::{Layer, Net, from_wsp, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspm-{tag}-{}", std::process::id()));
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

/// 两层异构形状的单层网络（含 f32 偏置幅度与非常规 scale）。
fn fixture_nets() -> Vec<Net> {
    let mut with_bias_f32 =
        Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5]).expect("l0");
    with_bias_f32 = with_bias_f32
        .with_bias_f32(vec![0.25, -0.75])
        .expect("biasf");
    vec![
        Net::new(vec![with_bias_f32]).expect("net0"),
        Net::new(vec![Layer::new(1, 2, &[1, -1], &[0]).expect("l1")]).expect("net1"),
    ]
}

fn items<'a>(nets: &'a [Net], names: &'a [String]) -> Vec<ModelItem<'a>> {
    vec![
        ModelItem {
            name: &names[0],
            vector: false,
            bias: Some(&names[1]),
            net: &nets[0],
        },
        ModelItem {
            name: &names[1],
            vector: true,
            bias: None,
            net: &nets[1],
        },
    ]
}

#[test]
fn round_trips_bytes_and_metadata() {
    let nets = fixture_nets();
    let names = ["a.weight".to_string(), "a.bias".to_string()];
    let packed = wspm::pack(&items(&nets, &names), "auto").expect("pack");
    let model = wspm::unpack(&packed.bytes).expect("unpack");
    assert_eq!(model.entries.len(), 2);
    for (net, decoded) in nets.iter().zip(&model.nets) {
        assert_eq!(to_wsp(net), to_wsp(decoded), "WSP1 文本逐字节一致");
    }
    // bias 引用与 vector 标记原样还原。
    assert_eq!(model.entries[0].bias, Some(1));
    assert!(model.entries[1].vector);
    assert_eq!(model.entries[1].bias, None);
    // f32 偏置幅度经 WSP2 blob 保留。
    assert_eq!(
        model.nets[0].layers()[0].bias_f32(),
        nets[0].layers()[0].bias_f32()
    );
    // 名字与形状。
    assert_eq!(model.entries[0].name, "a.weight");
    assert_eq!(model.entries[0].rows, 2);
    assert_eq!(model.entries[0].cols, 3);
}

#[test]
fn auto_picks_smallest_format_per_tensor() {
    let nets = fixture_nets();
    let names = ["a.weight".to_string(), "a.bias".to_string()];
    let packed = wspm::pack(&items(&nets, &names), "auto").expect("pack");
    // 每个 blob 都不大于同张量的 WSP2。
    let model = wspm::unpack(&packed.bytes).expect("unpack");
    for (entry, net) in model.entries.iter().zip(&nets) {
        let wsp2 = trit_wspace::wsp2::to_wsp2(net).expect("wsp2");
        assert!(entry.len <= wsp2.len() as u64);
    }
    for format in packed.formats {
        assert!(["wsp2", "wsp3", "wsp4"].contains(&format));
    }
    // 固定格式被尊重：强制 wsp2 后 blob 与单工件 pack-artifact 等大。
    let fixed = wspm::pack(&items(&nets, &names), "wsp2").expect("pack wsp2");
    let model = wspm::unpack(&fixed.bytes).expect("unpack");
    for (entry, net) in model.entries.iter().zip(&nets) {
        assert_eq!(
            entry.len,
            trit_wspace::wsp2::to_wsp2(net).expect("wsp2").len() as u64
        );
    }
}

#[test]
fn rejects_bad_containers() {
    let nets = fixture_nets();
    let names = ["a.weight".to_string(), "a.bias".to_string()];
    let packed = wspm::pack(&items(&nets, &names), "auto").expect("pack");

    // 坏 magic / 坏版本 / 截断。
    let mut bad = packed.bytes.clone();
    bad[0] = b'X';
    assert!(wspm::unpack(&bad).is_err());
    let mut bad = packed.bytes.clone();
    bad[4] = 9;
    assert!(wspm::unpack(&bad).is_err());
    assert!(wspm::unpack(&packed.bytes[..packed.bytes.len() - 3]).is_err());

    // 校验和（翻转 blob 区一个字节）。
    let mut bad = packed.bytes.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xff;
    assert!(wspm::unpack(&bad).is_err());

    // 未知格式 / 未知 bias 引用 / 重复名 / 空名。
    assert!(wspm::pack(&items(&nets, &names), "wsp9").is_err());
    let lonely = [ModelItem {
        name: "x".get(0..1).expect("x"),
        vector: false,
        bias: Some("a.bias"),
        net: &nets[0],
    }];
    assert!(wspm::pack(&lonely, "auto").is_err());
    let dup = [
        ModelItem {
            name: &names[0],
            vector: false,
            bias: None,
            net: &nets[0],
        },
        ModelItem {
            name: &names[0],
            vector: false,
            bias: None,
            net: &nets[1],
        },
    ];
    assert!(wspm::pack(&dup, "auto").is_err());
    let empty_name = [ModelItem {
        name: "",
        vector: false,
        bias: None,
        net: &nets[0],
    }];
    assert!(wspm::pack(&empty_name, "auto").is_err());
}

/// 与 import-safetensors-dir 相同约定的夹具目录。
fn fixture_dir(dir: &Path) {
    let nets = fixture_nets();
    let mut entries = Vec::new();
    let names = ["enc.weight", "enc.bias"];
    for (index, (name, net)) in names.iter().zip(&nets).enumerate() {
        let file = format!("{index:04}-{name}.wsp");
        fs::write(dir.join(&file), to_wsp(net)).expect("write wsp1");
        let mut entry = json!({
            "name": name,
            "file": file,
            "rows": net.layers()[0].rows(),
            "cols": net.layers()[0].cols(),
        });
        if index == 0 {
            entry["bias"] = json!("enc.bias");
        } else {
            entry["vector"] = json!(true);
        }
        entries.push(entry);
    }
    let manifest = json!({ "source": "fixture", "tensors": entries });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).expect("manifest"),
    )
    .expect("write manifest");
}

#[test]
fn cli_pack_unpack_round_trips_dir() {
    let dir = temp_dir("cli");
    fixture_dir(&dir);
    let container = dir.join("model.wspm");
    let report = stdout_json(&run(&[
        "pack-model",
        dir.to_str().expect("dir"),
        "--out",
        container.to_str().expect("path"),
    ]));
    assert_eq!(report["tensors"], 2);
    assert!(report["bytes"].as_u64().expect("bytes") > 0);
    assert_eq!(
        report["source_bytes"].as_u64().expect("source"),
        dir.join("0000-enc.weight.wsp")
            .metadata()
            .expect("w0")
            .len()
            + dir.join("0001-enc.bias.wsp").metadata().expect("w1").len()
    );

    let out = dir.join("restored");
    let unpacked = stdout_json(&run(&[
        "unpack-model",
        container.to_str().expect("path"),
        "--out",
        out.to_str().expect("dir"),
    ]));
    assert_eq!(unpacked["tensors"].as_array().expect("tensors").len(), 2);

    // 还原目录与原目录逐字节一致（含 manifest 的 vector/bias 语义）。
    for file in ["0000-enc.weight.wsp", "0001-enc.bias.wsp"] {
        let before = fs::read(dir.join(file)).expect("before");
        let after = fs::read(out.join(file)).expect("after");
        assert_eq!(before, after, "{file} 逐字节一致");
    }
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    let tensors = manifest["tensors"].as_array().expect("tensors");
    assert_eq!(tensors[0]["bias"], json!("enc.bias"));
    assert_eq!(tensors[1]["vector"], json!(true));
    assert_eq!(manifest["skipped"], json!([]));

    // 再打包：容器逐字节一致（确定性编码）。
    let again = dir.join("again.wspm");
    let second = stdout_json(&run(&[
        "pack-model",
        out.to_str().expect("dir"),
        "--out",
        again.to_str().expect("path"),
    ]));
    assert_eq!(
        fs::read(&container).expect("first"),
        fs::read(&again).expect("second"),
        "WSPM 容器逐字节确定"
    );
    assert_eq!(second["bytes"], report["bytes"]);
}

#[test]
fn cli_rejects_bad_inputs() {
    let dir = temp_dir("bad");
    fixture_dir(&dir);

    // 坏 --format。
    let out = run(&[
        "pack-model",
        dir.to_str().expect("dir"),
        "--out",
        dir.join("m.wspm").to_str().expect("path"),
        "--format",
        "wsp9",
    ]);
    assert!(!out.status.success());

    // 缺 manifest。
    let empty = dir.join("empty");
    fs::create_dir_all(&empty).expect("dir");
    let out = run(&[
        "pack-model",
        empty.to_str().expect("dir"),
        "--out",
        dir.join("m.wspm").to_str().expect("path"),
    ]);
    assert!(!out.status.success());

    // 坏容器。
    let container = dir.join("model.wspm");
    assert!(
        run(&[
            "pack-model",
            dir.to_str().expect("dir"),
            "--out",
            container.to_str().expect("path"),
        ])
        .status
        .success()
    );
    let mut bytes = fs::read(&container).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&container, &bytes).expect("write corrupt");
    let out = run(&[
        "unpack-model",
        container.to_str().expect("path"),
        "--out",
        dir.join("restored").to_str().expect("dir"),
    ]);
    assert!(!out.status.success());
    assert!(!dir.join("restored").exists(), "坏容器不产出目录");
}

#[test]
fn unpacked_dir_reimports_like_original() {
    let dir = temp_dir("reimport");
    fixture_dir(&dir);
    let container = dir.join("model.wspm");
    assert!(
        run(&[
            "pack-model",
            dir.to_str().expect("dir"),
            "--out",
            container.to_str().expect("path"),
        ])
        .status
        .success()
    );
    let out = dir.join("restored");
    assert!(
        run(&[
            "unpack-model",
            container.to_str().expect("path"),
            "--out",
            out.to_str().expect("dir"),
        ])
        .status
        .success()
    );
    // 还原的 .wsp 能被 from_wsp 读回，且与原网络相等。
    let nets = fixture_nets();
    for (index, net) in nets.iter().enumerate() {
        let file = format!("{index:04}-{}.wsp", ["enc.weight", "enc.bias"][index]);
        let text = fs::read_to_string(out.join(&file)).expect("wsp1");
        let decoded = from_wsp(&text).expect("from_wsp");
        assert_eq!(to_wsp(&decoded), to_wsp(net));
    }
}
