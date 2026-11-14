//! MCP 扩展工具的端到端：tile / 块索引 / 读块 / 锁 / learned / gguf / 量化差分 / 证书 / trim。
//!
//! 路径类工具受 `confined` 限制（工作区根 = 进程 cwd），所以临时目录建在 cwd 下并在
//! 每条测试结束时删除。

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Patch, Trit, to_wsp, to_wsp2};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_artifact() -> String {
    to_wsp(&xor_net())
}

/// 一个确定不精确的网络：末层权重全正 + 正偏置 ⇒ 输出恒 `+1`。
fn broken_artifact() -> String {
    let net = Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[1, 1], &[1]).expect("l1"),
    ])
    .expect("broken net");
    to_wsp(&net)
}

fn xor_oracle() -> Value {
    json!({"domain": "boolean", "vars": 2, "table": "0110"})
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    });
    server.dispatch(&request).expect("response")["result"].clone()
}

fn payload(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "工具报错：{result}");
    let raw = result["content"][0]["text"].as_str().expect("text");
    serde_json::from_str(raw).unwrap_or_else(|error| panic!("payload: {error}; raw={raw}"))
}

fn error_text(result: &Value) -> String {
    assert_eq!(result["isError"], true, "本应报错：{result}");
    result["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_string()
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("mcp-extra-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("workdir");
    dir
}

/// 把绝对路径转成相对 cwd 的形式（`confined` 只放行工作区内的路径）。
fn rel(path: &PathBuf) -> String {
    let cwd = std::env::current_dir().expect("cwd");
    let stripped = path.strip_prefix(&cwd).expect("path 必须在 cwd 内");
    stripped.to_string_lossy().replace('\\', "/")
}

#[test]
fn tile_fill_and_set_roundtrip() {
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_tile",
        json!({
            "artifact": xor_artifact(),
            "layer": 0, "row_start": 0, "rows": 1, "col_start": 0, "cols": 2,
            "fill": "+",
        }),
    ));
    assert_eq!(body["applied"], true);
    assert!(body["artifact"].as_str().expect("a").starts_with("wsp1"));

    let body = payload(&call(
        &mut server,
        "wsp_tile",
        json!({
            "artifact": body["artifact"].clone(),
            "layer": 0, "row_start": 0, "rows": 1, "col_start": 0, "cols": 2,
            "values": "--",
        }),
    ));
    assert_eq!(body["applied"], true);
}

#[test]
fn tile_rejects_wrong_value_count() {
    let mut server = Mcp::new();
    let text = error_text(&call(
        &mut server,
        "wsp_tile",
        json!({
            "artifact": xor_artifact(),
            "layer": 0, "row_start": 0, "rows": 1, "col_start": 0, "cols": 2,
            "values": "-",
        }),
    ));
    assert!(text.contains("values 需要"), "{text}");
}

#[test]
fn index_and_read_block() {
    let dir = workdir("idx");
    let path = dir.join("model.wsp2");
    fs::write(&path, to_wsp2(&xor_net()).expect("wsp2")).expect("write");
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_index_artifact",
        json!({"source": rel(&path)}),
    ));
    assert_eq!(body["layers"], 2);
    assert_eq!(body["blocks"], 2);
    let index = body["index"].clone();

    let body = payload(&call(
        &mut server,
        "wsp_read_block",
        json!({"source": rel(&path), "index": index, "layer": 0, "row_start": 0, "rows": 1}),
    ));
    assert_eq!(body["rows"], 1);
    assert!(body["artifact"].as_str().expect("a").starts_with("wsp1"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn index_rejects_non_wsp2_source() {
    let dir = workdir("badidx");
    let path = dir.join("model.wsp");
    fs::write(&path, xor_artifact()).expect("write");
    let mut server = Mcp::new();
    let text = error_text(&call(
        &mut server,
        "wsp_index_artifact",
        json!({"source": rel(&path)}),
    ));
    assert!(text.contains("magic"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lock_lifecycle() {
    let dir = workdir("lock");
    let path = dir.join("artifact.wsp");
    fs::write(&path, xor_artifact()).expect("write");
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_lock",
        json!({"path": rel(&path), "action": "acquire"}),
    ));
    assert_eq!(body["acquired"], true);

    let body = payload(&call(
        &mut server,
        "wsp_lock",
        json!({"path": rel(&path), "action": "status"}),
    ));
    assert_eq!(body["held"], true);

    let body = payload(&call(
        &mut server,
        "wsp_lock",
        json!({"path": rel(&path), "action": "release"}),
    ));
    assert_eq!(body["released"], true);

    let body = payload(&call(
        &mut server,
        "wsp_lock",
        json!({"path": rel(&path), "action": "status"}),
    ));
    assert_eq!(body["held"], false);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn train_proposer_returns_model() {
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_train_proposer",
        json!({"artifact": broken_artifact(), "oracle": xor_oracle(), "epochs": 20}),
    ));
    assert!(body["model"].is_object(), "model 不是对象：{body}");
    assert_eq!(body["model_info"]["features"], 16);
    assert!(body["report"]["samples"].as_u64().expect("samples") > 0);
}

#[test]
fn propose_learned_uses_trained_model() {
    let mut server = Mcp::new();
    let trained = payload(&call(
        &mut server,
        "wsp_train_proposer",
        json!({"artifact": broken_artifact(), "oracle": xor_oracle(), "epochs": 20}),
    ));
    let body = payload(&call(
        &mut server,
        "wsp_propose_learned",
        json!({
            "artifact": broken_artifact(), "oracle": xor_oracle(),
            "model": trained["model"].clone(), "limit": 4,
        }),
    ));
    let candidates = body["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
}

#[test]
fn propose_gradient_returns_candidates() {
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_propose_gradient",
        json!({"artifact": xor_artifact(), "oracle": xor_oracle(), "limit": 4}),
    ));
    let candidates = body["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
    assert!(body["evaluated"].as_u64().expect("evaluated") > 0);
}

#[test]
fn export_import_gguf_roundtrip() {
    let dir = workdir("gguf");
    let src = dir.join("wsp");
    fs::create_dir_all(&src).expect("src");
    // GGUF 只接受单层 WSP1
    let single = Net::new(vec![
        Layer::new(2, 2, &[-1, 1, 1, -1], &[-1, 1]).expect("l"),
    ])
    .expect("net");
    fs::write(src.join("0000-layer.wsp"), to_wsp(&single)).expect("wsp");
    fs::write(
        src.join("manifest.json"),
        json!({
            "source": "test",
            "tensors": [{"name": "layer0", "file": "0000-layer.wsp", "rows": 2, "cols": 2}],
            "metadata": [],
        })
        .to_string(),
    )
    .expect("manifest");

    let mut server = Mcp::new();
    let gguf = dir.join("model.gguf");
    let body = payload(&call(
        &mut server,
        "wsp_export_gguf",
        json!({"source": rel(&src), "out": rel(&gguf)}),
    ));
    assert_eq!(body["tensors"], 1);

    let back = dir.join("back");
    let body = payload(&call(
        &mut server,
        "wsp_import_gguf",
        json!({"source": rel(&gguf), "out": rel(&back)}),
    ));
    assert_eq!(body["tensors"], 1);
    assert!(back.join("manifest.json").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn quantize_delta_builds_patch() {
    let dir = workdir("delta");
    let base = dir.join("base.safetensors");
    let tuned = dir.join("tuned.safetensors");
    trit_wspace::safetensors::write_tensor(
        &base.display().to_string(),
        "dense.weight",
        2,
        3,
        &[1.0; 6],
    )
    .expect("base");
    trit_wspace::safetensors::write_tensor(
        &tuned.display().to_string(),
        "dense.weight",
        2,
        3,
        &[-1.0, -1.0, 1.0, 1.0, 1.0, 1.0],
    )
    .expect("tuned");
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_quantize_delta",
        json!({"base": rel(&base), "tuned": rel(&tuned), "tensor": "dense.weight", "trim": 1.0}),
    ));
    assert_eq!(body["stats"]["changed_trits"], 2);
    assert!(!body["patch"]["edits"].as_array().expect("edits").is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn quantize_delta_rejects_bad_trim() {
    let dir = workdir("delta2");
    let path = dir.join("base.safetensors");
    trit_wspace::safetensors::write_tensor(
        &path.display().to_string(),
        "dense.weight",
        2,
        3,
        &[1.0; 6],
    )
    .expect("base");
    let mut server = Mcp::new();
    let text = error_text(&call(
        &mut server,
        "wsp_quantize_delta",
        json!({"base": rel(&path), "tuned": rel(&path), "tensor": "dense.weight", "trim": 1.5}),
    ));
    assert!(text.contains("trim 必须在"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn certify_reports_certificate() {
    let mut server = Mcp::new();
    let body = payload(&call(
        &mut server,
        "wsp_certify",
        json!({
            "artifact": xor_artifact(),
            "keep_samples": 16,
            "patch": {"version": 1, "edits": []},
        }),
    ));
    assert_eq!(body["certificate"]["schema"], "trit-wspace.certificate.v1");
    assert_eq!(body["certificate"]["certified"], true);
    assert_eq!(body["certificate"]["keep"]["samples"], 16);
}

#[test]
fn task_trim_merges_two_tasks() {
    let mut server = Mcp::new();
    let mut a = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Pos,
    }])
    .apply(&mut a)
    .expect("a");
    let mut b = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 1,
        to: Trit::Pos,
    }])
    .apply(&mut b)
    .expect("b");
    let body = payload(&call(
        &mut server,
        "wsp_task_trim",
        json!({
            "base": xor_artifact(),
            "tasks": [to_wsp(&a), to_wsp(&b)],
            "trim": 1,
        }),
    ));
    assert!(body["artifact"].as_str().expect("a").starts_with("wsp1"));
    assert_eq!(body["merge_report"]["tasks"], 2);
}
