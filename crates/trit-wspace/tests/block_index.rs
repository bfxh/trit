//! WSP2 块索引：扫描、整层/行块随机读取、索引 JSON 往返、坏索引与 CLI。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::wsp2::BlockIndex;
use trit_wspace::{Layer, Net, from_wsp, from_wsp2, to_wsp, to_wsp2};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-block-idx-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sample() -> Net {
    let layer0 = Layer::with_scales(
        5,
        7,
        &[
            1, 0, -1, 1, 0, 1, -1, 0, 1, 0, -1, -1, 1, 0, 1, 0, -1, 0, 1, 1, -1, 0, 0, 1, -1, 0, 1,
            -1, 1, 0, 0, -1, 1, 0, -1,
        ],
        &[1, -1, 0, 1, -1],
        vec![0.5, 1.0, 1.5, 2.0, 2.5],
    )
    .expect("layer0")
    .with_bias_f32(vec![0.1, -0.2, 0.3, -0.4, 0.5])
    .expect("bias_f32");
    let layer1 = Layer::new(
        3,
        5,
        &[1, 0, -1, 0, 1, -1, 1, 0, 1, -1, 0, 1, -1, 0, 1],
        &[0, 1, -1],
    )
    .expect("layer1");
    let layer2 = Layer::with_scales(
        70,
        3,
        &(0..210)
            .map(|index| match index % 3 {
                0 => -1,
                1 => 0,
                _ => 1,
            })
            .collect::<Vec<_>>(),
        &(0..70)
            .map(|index| match index % 3 {
                0 => 1,
                _ => 0,
            })
            .collect::<Vec<_>>(),
        (0..70).map(|index| 0.25 + index as f32 * 0.01).collect(),
    )
    .expect("layer2");
    Net::new(vec![layer0, layer1, layer2]).expect("net")
}

fn assert_part(part: &Layer, full: &Layer, offset: usize) {
    for row in 0..part.rows() {
        assert_eq!(part.scale(row), full.scale(offset + row), "scale row {row}");
        assert_eq!(
            part.bias_trit(row),
            full.bias_trit(offset + row),
            "bias row {row}"
        );
        if full.bias_f32().len() == full.rows() {
            assert_eq!(
                part.bias_f32()[row],
                full.bias_f32()[offset + row],
                "bias_f32 row {row}"
            );
        }
        for col in 0..part.cols() {
            assert_eq!(
                part.trit(row, col),
                full.trit(offset + row, col),
                "trit {row},{col}"
            );
        }
    }
}

#[test]
fn scan_and_read_every_layer_matches_full_decode() {
    let net = sample();
    let bytes = to_wsp2(&net).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    assert_eq!(index.layers.len(), net.layers().len());
    assert_eq!(index.source_bytes, bytes.len());
    assert_eq!(
        index
            .layers
            .iter()
            .map(|layer| layer.row_blocks.len())
            .sum::<usize>(),
        1 + 1 + 2
    );
    let decoded = from_wsp2(&bytes).expect("decode");
    for (number, expected) in decoded.layers().iter().enumerate() {
        let layer = index.read_layer(&bytes, number).expect("read layer");
        assert_eq!(&layer, expected, "layer {number}");
    }
    assert!(index.read_layer(&bytes, 3).is_err());
}

#[test]
fn read_rows_matches_full_layer() {
    let net = sample();
    let bytes = to_wsp2(&net).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let full = &net.layers()[2];
    for (start, rows) in [(0, 1), (1, 4), (63, 2), (64, 6), (65, 5), (69, 1), (0, 70)] {
        let part = index
            .read_rows(&bytes, 2, start, rows)
            .unwrap_or_else(|error| panic!("{start}..{}: {error}", start + rows));
        assert_eq!(part.rows(), rows);
        assert_eq!(part.cols(), 3);
        assert_part(&part, full, start);
    }
    assert!(index.read_rows(&bytes, 2, 68, 3).is_err());
    assert!(index.read_rows(&bytes, 2, 0, 0).is_err());
}

#[test]
fn read_rows_io_matches_memory() {
    let net = sample();
    let bytes = to_wsp2(&net).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let dir = temp_dir("io");
    let path = dir.join("model.wsp2");
    fs::write(&path, &bytes).expect("write");
    let mut file = fs::File::open(&path).expect("open");
    for (start, rows) in [(0, 1), (63, 2), (64, 6), (65, 5), (0, 70)] {
        let io = index.read_rows_io(&mut file, 2, start, rows).expect("io");
        let memory = index.read_rows(&bytes, 2, start, rows).expect("memory");
        assert_eq!(io, memory, "{start}..{}", start + rows);
    }
    let io = index.read_layer_io(&mut file, 0).expect("layer io");
    assert_eq!(io, net.layers()[0]);
}

#[test]
fn index_json_round_trips() {
    let bytes = to_wsp2(&sample()).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let value = index.to_json();
    let back = BlockIndex::from_json(&value).expect("from json");
    assert_eq!(back, index);
    let text = serde_json::to_string(&value).expect("json");
    let parsed: Value = serde_json::from_str(&text).expect("parse");
    assert_eq!(BlockIndex::from_json(&parsed).expect("round trip"), index);
}

#[test]
fn rejects_tampered_index_and_source() {
    let bytes = to_wsp2(&sample()).expect("wsp2");
    let index = BlockIndex::scan(&bytes).expect("scan");
    let mut value = index.to_json();
    value["source_checksum"] = json!(0);
    let bad = BlockIndex::from_json(&value).expect("shape is still valid");
    assert!(bad.read_layer(&bytes, 0).is_err());

    let mut value = index.to_json();
    value["layers"][0]["row_blocks"][0]["trits_offset"] = json!(usize::MAX / 2);
    assert!(BlockIndex::from_json(&value).is_err());

    let mut bad = index.clone();
    bad.layers[0].row_blocks[0].trits_offset = usize::MAX / 2;
    assert!(bad.read_rows(&bytes, 0, 0, 2).is_err());

    let mut value = index.to_json();
    value["layers"][0]["row_blocks"][0]["row_end"] = json!(2);
    assert!(BlockIndex::from_json(&value).is_err());

    let mut bytes = bytes;
    bytes[0] = b'X';
    assert!(BlockIndex::scan(&bytes).is_err());
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

#[test]
fn cli_index_and_read_block() {
    let dir = temp_dir("cli");
    let net = sample();
    let wsp = dir.join("model.wsp");
    let wsp2 = dir.join("model.wsp2");
    let idx = dir.join("model.idx");
    let part_path = dir.join("part.wsp");
    fs::write(&wsp, to_wsp(&net)).expect("wsp1");

    let report = stdout_json(&run(&[
        "pack-artifact",
        wsp.to_str().expect("wsp"),
        "--out",
        wsp2.to_str().expect("wsp2"),
    ]));
    assert!(report["bytes"].as_u64().expect("bytes") > 0);

    let report = stdout_json(&run(&[
        "index-artifact",
        wsp2.to_str().expect("wsp2"),
        "--out",
        idx.to_str().expect("idx"),
    ]));
    assert_eq!(report["layers"], 3);
    assert_eq!(report["blocks"], 4);
    assert_eq!(report["block_rows"], 64);

    let report = stdout_json(&run(&[
        "read-block",
        wsp2.to_str().expect("wsp2"),
        "--index",
        idx.to_str().expect("idx"),
        "--layer",
        "2",
        "--row-start",
        "65",
        "--rows",
        "5",
        "--verify",
        "--out",
        part_path.to_str().expect("part"),
    ]));
    assert_eq!(report["rows"], 5);
    assert_eq!(report["cols"], 3);
    let part = from_wsp(&fs::read_to_string(&part_path).expect("part")).expect("parse");
    assert_eq!(part.layers().len(), 1);
    assert_part(&part.layers()[0], &net.layers()[2], 65);
}

#[test]
fn cli_read_block_preset_error() {
    let dir = temp_dir("cli-bad");
    let wsp2 = dir.join("model.wsp2");
    let idx = dir.join("model.idx");
    fs::write(&wsp2, to_wsp2(&sample()).expect("wsp2")).expect("write");
    stdout_json(&run(&[
        "index-artifact",
        wsp2.to_str().expect("wsp2"),
        "--out",
        idx.to_str().expect("idx"),
    ]));
    let out = run(&[
        "read-block",
        wsp2.to_str().expect("wsp2"),
        "--index",
        idx.to_str().expect("idx"),
        "--layer",
        "9",
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("越界"));
}

#[test]
fn cli_rejects_wsp4_for_index() {
    let dir = temp_dir("wsp4");
    let wsp = dir.join("model.wsp");
    let wsp4 = dir.join("model.wsp4");
    let idx = dir.join("model.idx");
    fs::write(&wsp, to_wsp(&sample())).expect("wsp1");
    stdout_json(&run(&[
        "pack-artifact",
        wsp.to_str().expect("wsp"),
        "--huffman",
        "--out",
        wsp4.to_str().expect("wsp4"),
    ]));
    let out = run(&[
        "index-artifact",
        wsp4.to_str().expect("wsp4"),
        "--out",
        idx.to_str().expect("idx"),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("WSP2"));
}
