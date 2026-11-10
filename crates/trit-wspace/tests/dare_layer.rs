//! 层 DARE：整层一起保留/丢弃（从 `tests/dare.rs` 拆出，控制单文件行数）。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Patch, dare_drop_layers, from_wsp, to_wsp};

fn flat_net(rows: usize, cols: usize, layers: usize, value: i8) -> Net {
    let mut built = Vec::new();
    let mut width = cols;
    for _ in 0..layers {
        built.push(
            Layer::new(rows, width, &vec![value; rows * width], &vec![value; rows]).expect("layer"),
        );
        width = rows;
    }
    Net::new(built).expect("net")
}

fn base_net() -> Net {
    flat_net(4, 4, 2, -1)
}

fn tuned_net() -> Net {
    flat_net(4, 4, 2, 1)
}

fn apply(net: &Net, patch: &Patch) -> Net {
    let mut work = net.clone();
    patch.apply(&mut work).expect("apply");
    work
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-wspace-dare-layer-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
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
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    server
        .dispatch(&json!({
            "jsonrpc": "2.0", "id": 51, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }))
        .expect("response")["result"]
        .clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(raw).expect("json payload")
}

#[test]
fn layer_drop_keeps_layers_whole_and_can_drop() {
    let base = base_net();
    let tuned = tuned_net();
    let mut all_kept_seen = false;
    let mut all_dropped_seen = false;
    for seed in 0..32 {
        let outcome = dare_drop_layers(&base, &tuned, 0.5, seed).expect("layers");
        let mut per_layer = [0usize; 2];
        for edit in outcome.patch.edits() {
            if let Edit::SetTrit { layer, .. } = edit {
                per_layer[*layer] += 1;
            }
        }
        for (layer, count) in per_layer.iter().enumerate() {
            assert!(
                *count == 0 || *count == 16,
                "层 {layer} 必须整层保留(16)或整层丢弃(0)，得到 {count}"
            );
        }
        if per_layer == [16, 16] {
            all_kept_seen = true;
        }
        if per_layer == [0, 0] {
            all_dropped_seen = true;
        }
    }
    assert!(
        all_kept_seen && all_dropped_seen,
        "32 个 seed 内应同时出现整层保留与整层丢弃"
    );
}

#[test]
fn layer_drop_rate_zero_is_full_delta() {
    let base = base_net();
    let tuned = tuned_net();
    let outcome = dare_drop_layers(&base, &tuned, 0.0, 7).expect("layers");
    assert_eq!(outcome.kept, 40);
    assert_eq!(outcome.dropped, 0);
    assert_eq!(outcome.rescaled_rows, 0);
}

#[test]
fn cli_dare_drop_by_layer_round_trip() {
    let dir = temp_dir("cli-layer");
    let base = base_net();
    let tuned = tuned_net();
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    let out_path = dir.join("out.wsp");
    fs::write(&base_path, to_wsp(&base)).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned)).expect("write tuned");
    let output = stdout_json(&run(&[
        "dare-drop",
        &path_string(&base_path),
        "--tuned",
        &path_string(&tuned_path),
        "--rate",
        "0.5",
        "--seed",
        "7",
        "--by-layer",
        "--out",
        &path_string(&out_path),
    ]));
    assert_eq!(output["by_layer"], json!(true));
    let expected = dare_drop_layers(&base, &tuned, 0.5, 7).expect("layers");
    let work = apply(&base, &expected.patch);
    let written = from_wsp(&fs::read_to_string(&out_path).expect("read out")).expect("parse");
    assert_eq!(written.state_hash(), work.state_hash());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_dare_drop_rejects_conflicting_units() {
    let dir = temp_dir("cli-conflict");
    let base_path = dir.join("base.wsp");
    let tuned_path = dir.join("tuned.wsp");
    fs::write(&base_path, to_wsp(&base_net())).expect("write base");
    fs::write(&tuned_path, to_wsp(&tuned_net())).expect("write tuned");
    let output = run(&[
        "dare-drop",
        &path_string(&base_path),
        "--tuned",
        &path_string(&tuned_path),
        "--rate",
        "0.5",
        "--by-row",
        "--by-layer",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("只能给一个"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mcp_dare_drop_by_layer_round_trip() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_dare_drop",
        json!({
            "artifact": to_wsp(&base_net()),
            "tuned": to_wsp(&tuned_net()),
            "rate": 0.5,
            "seed": 7,
            "by_layer": true,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["by_layer"], json!(true));
    let expected = dare_drop_layers(&base_net(), &tuned_net(), 0.5, 7).expect("layers");
    let work = apply(&base_net(), &expected.patch);
    let artifact = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        from_wsp(artifact).expect("parse").state_hash(),
        work.state_hash()
    );
}
