//! WSP1 分片：切分/合并往返、顺序与形状校验、CLI 端到端。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::artifact_shard::{WeightManifest, join, split};
use trit_wspace::{Layer, Net, from_wsp, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-shard-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sample_layer() -> Layer {
    let trits: Vec<i8> = (0..15).map(|index| (index % 3) as i8 - 1).collect();
    Layer::with_scales(
        5,
        3,
        &trits,
        &[1, -1, 0, 1, -1],
        vec![0.5, 0.25, 0.75, 1.0, 0.5],
    )
    .expect("layer")
}

fn assert_same_layer(got: &Layer, want: &Layer) {
    assert_eq!(got.rows(), want.rows());
    assert_eq!(got.cols(), want.cols());
    assert_eq!(got.trits(), want.trits());
    assert_eq!(got.bias(), want.bias());
    for (got, want) in got.scales().iter().zip(want.scales()) {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }
}

#[test]
fn split_join_round_trip() {
    let net = Net::new(vec![sample_layer()]).expect("net");
    let (manifest, texts) = split(&net, 2).expect("split");
    assert_eq!(manifest.rows, 5);
    assert_eq!(manifest.cols, 3);
    assert_eq!(manifest.shards.len(), 3);
    assert_eq!(
        manifest
            .shards
            .iter()
            .map(|shard| (shard.start, shard.rows))
            .collect::<Vec<_>>(),
        vec![(0, 2), (2, 2), (4, 1)]
    );
    let joined = join(&manifest, &texts).expect("join");
    assert_eq!(joined.layers().len(), 1);
    assert_same_layer(&joined.layers()[0], &sample_layer());
}

#[test]
fn shard_rows_at_least_total_is_one_shard() {
    let net = Net::new(vec![sample_layer()]).expect("net");
    let (manifest, texts) = split(&net, 99).expect("split");
    assert_eq!(manifest.shards.len(), 1);
    assert_eq!(manifest.shards[0].rows, 5);
    let joined = join(&manifest, &texts).expect("join");
    assert_same_layer(&joined.layers()[0], &sample_layer());
}

#[test]
fn split_rejects_multi_layer_and_zero_rows() {
    let net = Net::new(vec![
        Layer::new(2, 3, &[1, 0, -1, 0, 1, -1], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("net");
    assert!(split(&net, 2).is_err());
    let single = Net::new(vec![sample_layer()]).expect("net");
    assert!(split(&single, 0).is_err());
}

#[test]
fn join_rejects_bad_order_and_counts() {
    let net = Net::new(vec![sample_layer()]).expect("net");
    let (manifest, texts) = split(&net, 2).expect("split");

    let mut swapped = manifest.clone();
    swapped.shards.swap(0, 1);
    assert!(join(&swapped, &texts).is_err());

    let mut wrong_rows = manifest.clone();
    wrong_rows.rows = 6;
    assert!(join(&wrong_rows, &texts).is_err());

    let mut wrong_cols = manifest.clone();
    wrong_cols.cols = 4;
    assert!(join(&wrong_cols, &texts).is_err());

    assert!(join(&manifest, &texts[..2]).is_err());
}

#[test]
fn manifest_json_round_trip() {
    let net = Net::new(vec![sample_layer()]).expect("net");
    let (manifest, _) = split(&net, 2).expect("split");
    let json = manifest.to_json();
    let back = WeightManifest::from_json(&json).expect("from json");
    assert_eq!(back, manifest);
    assert!(WeightManifest::from_json(&serde_json::json!({"rows": 1})).is_err());
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
fn cli_split_join_round_trip() {
    let dir = temp_dir("cli");
    let net = Net::new(vec![sample_layer()]).expect("net");
    let artifact = dir.join("layer.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let shards = dir.join("parts");
    let split_out = stdout_json(&run(&[
        "split-artifact",
        artifact.to_str().unwrap(),
        "--rows",
        "2",
        "--out",
        shards.to_str().unwrap(),
    ]));
    assert_eq!(split_out["shards"], 3);
    for name in [
        "part-000.wsp",
        "part-001.wsp",
        "part-002.wsp",
        "manifest.json",
    ] {
        assert!(shards.join(name).is_file(), "{name}");
    }
    let joined = dir.join("joined.wsp");
    let join_out = stdout_json(&run(&[
        "join-artifact",
        shards.to_str().unwrap(),
        "--out",
        joined.to_str().unwrap(),
    ]));
    assert_eq!(join_out["rows"], 5);
    let back = from_wsp(&fs::read_to_string(&joined).expect("joined")).expect("parse");
    assert_same_layer(&back.layers()[0], &sample_layer());
}

#[test]
fn cli_join_rejects_missing_part() {
    let dir = temp_dir("cli-missing");
    let net = Net::new(vec![sample_layer()]).expect("net");
    let artifact = dir.join("layer.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let shards = dir.join("parts");
    let _ = stdout_json(&run(&[
        "split-artifact",
        artifact.to_str().unwrap(),
        "--rows",
        "2",
        "--out",
        shards.to_str().unwrap(),
    ]));
    fs::remove_file(shards.join("part-001.wsp")).expect("remove");
    let joined = dir.join("joined.wsp");
    let output = run(&[
        "join-artifact",
        shards.to_str().unwrap(),
        "--out",
        joined.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("part-001.wsp"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_split_rejects_multi_layer() {
    let dir = temp_dir("cli-multi");
    let net = Net::new(vec![
        Layer::new(2, 3, &[1, 0, -1, 0, 1, -1], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("net");
    let artifact = dir.join("net.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let out = dir.join("parts");
    let output = run(&[
        "split-artifact",
        artifact.to_str().unwrap(),
        "--rows",
        "1",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("单层"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
