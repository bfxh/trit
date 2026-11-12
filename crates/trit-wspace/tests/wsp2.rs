//! WSP2 2-bit 容器：往返、体积、坏输入、CLI 端到端。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::{Layer, Net, from_wsp2, to_wsp, to_wsp2};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wsp2-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sample_net() -> Net {
    Net::new(vec![
        Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5]).expect("l0"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("l1"),
    ])
    .expect("net")
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

fn assert_same_net(got: &Net, want: &Net) {
    assert_eq!(got.layers().len(), want.layers().len());
    for (got, want) in got.layers().iter().zip(want.layers()) {
        assert_eq!(got.trits(), want.trits());
        assert_eq!(got.bias(), want.bias());
        for (a, b) in got.scales().iter().zip(want.scales()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }
}

#[test]
fn round_trip_preserves_weights() {
    let net = sample_net();
    let bytes = to_wsp2(&net).expect("encode");
    let back = from_wsp2(&bytes).expect("decode");
    assert_same_net(&back, &net);
}

#[test]
fn wsp2_is_much_smaller_than_wsp1() {
    let net = sample_net();
    let wsp1 = to_wsp(&net).len();
    let wsp2 = to_wsp2(&net).expect("encode").len();
    assert!(wsp2 * 2 < wsp1, "wsp1={wsp1} wsp2={wsp2}");
}

#[test]
fn rejects_bad_magic_version_and_truncation() {
    let mut bytes = to_wsp2(&sample_net()).expect("encode");
    bytes[0] = b'X';
    assert!(from_wsp2(&bytes).is_err());

    let mut bytes = to_wsp2(&sample_net()).expect("encode");
    bytes[4] = 9;
    reseal(&mut bytes);
    assert!(from_wsp2(&bytes).is_err());

    let bytes = to_wsp2(&sample_net()).expect("encode");
    assert!(from_wsp2(&bytes[..bytes.len() - 3]).is_err());
}

#[test]
fn rejects_checksum_tamper() {
    let mut bytes = to_wsp2(&sample_net()).expect("encode");
    let last = bytes.len() - 9;
    bytes[last] ^= 1;
    let error = from_wsp2(&bytes).expect_err("校验和必须失败");
    assert!(format!("{error}").contains("校验和"), "{error}");
}

#[test]
fn rejects_illegal_2bit_code() {
    let net = sample_net();
    let mut bytes = to_wsp2(&net).expect("encode");
    // header 5 + nlayers 2 + rows/cols 8 + flags 1 + scales 2×4 = 24
    let start = 5 + 2 + 4 + 4 + 1 + 2 * 4;
    bytes[start] = 0xff;
    reseal(&mut bytes);
    let error = from_wsp2(&bytes).expect_err("2-bit 值 3 必须报错");
    assert!(format!("{error}").contains("非法三值"), "{error}");
}

#[test]
fn rejects_zero_layers() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"WSP2");
    bytes.push(1);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    let sum = fnv(&bytes);
    bytes.extend_from_slice(&sum.to_le_bytes());
    assert!(from_wsp2(&bytes).is_err());
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
fn cli_pack_unpack_round_trip() {
    let dir = temp_dir("cli");
    let net = sample_net();
    let artifact = dir.join("net.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let binary = dir.join("net.wsp2");
    let packed = stdout_json(&run(&[
        "pack-artifact",
        artifact.to_str().unwrap(),
        "--out",
        binary.to_str().unwrap(),
    ]));
    assert!(packed["bytes"].as_u64().expect("bytes") > 0);

    let back = dir.join("back.wsp");
    let _unpacked = stdout_json(&run(&[
        "unpack-artifact",
        binary.to_str().unwrap(),
        "--out",
        back.to_str().unwrap(),
    ]));
    assert_eq!(
        fs::read_to_string(&back).expect("back"),
        to_wsp(&net),
        "解包后必须回到同一份 WSP1"
    );
}

#[test]
fn cli_unpack_rejects_corrupt_file() {
    let dir = temp_dir("cli-bad");
    let binary = dir.join("bad.wsp2");
    fs::write(&binary, b"WSP2\x01\x00\x00").expect("write");
    let out = dir.join("back.wsp");
    let output = run(&[
        "unpack-artifact",
        binary.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
}
