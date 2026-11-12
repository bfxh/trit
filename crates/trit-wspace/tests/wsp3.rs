//! WSP3：PackBits RLE 与 WSP2 外层的往返/压缩率/坏输入、CLI 端到端。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::{Layer, Net, from_wsp3, rle_decode, rle_encode, to_wsp, to_wsp2, to_wsp3};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wsp3-{tag}-{}", std::process::id()));
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

fn uniform_net() -> Net {
    Net::new(vec![Layer::new(8, 32, &[1; 256], &[0; 8]).expect("layer")]).expect("net")
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

#[test]
fn rle_round_trips_every_pattern() {
    let mut noise = Vec::new();
    let mut x = 0x1234_5678u32;
    for _ in 0..1000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        noise.push(x as u8);
    }
    let cases = vec![
        Vec::new(),
        vec![7],
        vec![0; 300],
        (0..=255u8).collect(),
        (0..300u16).map(|i| (i % 3) as u8).collect(),
        noise,
    ];
    for case in cases {
        let packed = rle_encode(&case);
        assert_eq!(rle_decode(&packed, case.len()).expect("decode"), case);
    }
}

#[test]
fn wsp3_round_trips_and_shrinks_uniform_networks() {
    let net = uniform_net();
    let wsp2 = to_wsp2(&net).expect("wsp2");
    let wsp3 = to_wsp3(&net).expect("wsp3");
    assert_eq!(
        from_wsp3(&wsp3).expect("wsp3 net").state_hash(),
        net.state_hash()
    );
    assert!(wsp3.len() < wsp2.len(), "RLE 应压小全同 trit 的工件");
    let sample = sample_net();
    assert_eq!(
        from_wsp3(&to_wsp3(&sample).expect("wsp3"))
            .expect("back")
            .state_hash(),
        sample.state_hash()
    );
}

#[test]
fn wsp3_rejects_corruption() {
    let bytes = to_wsp3(&sample_net()).expect("wsp3");
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(from_wsp3(&bad).is_err(), "magic");
    let mut bad = bytes.clone();
    bad[4] = 9;
    reseal(&mut bad);
    assert!(from_wsp3(&bad).is_err(), "version");
    assert!(from_wsp3(&bytes[..10]).is_err(), "truncated");
    let mut bad = bytes.clone();
    let end = bad.len() - 1;
    bad[end] ^= 1;
    assert!(from_wsp3(&bad).is_err(), "checksum");
    let mut bad = bytes.clone();
    bad[5..13].copy_from_slice(&999u64.to_le_bytes());
    reseal(&mut bad);
    assert!(from_wsp3(&bad).is_err(), "raw_len");
}

#[test]
fn rle_decode_rejects_bad_streams() {
    assert!(rle_decode(&[0x80], 5).is_err(), "重复字节缺失");
    assert!(rle_decode(&[0x03, 1, 2], 3).is_err(), "literal 截断");
    assert!(rle_decode(&[0x00, 1], 2).is_err(), "长度不符");
}

#[test]
fn cli_pack_rle_round_trip() {
    let dir = temp_dir("cli");
    let net = sample_net();
    let artifact = dir.join("net.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let binary = dir.join("net.wsp3");
    let packed = stdout_json(&run(&[
        "pack-artifact",
        artifact.to_str().unwrap(),
        "--out",
        binary.to_str().unwrap(),
        "--rle",
    ]));
    assert_eq!(packed["format"], json!("wsp3"));
    assert!(packed["bytes"].as_u64().expect("bytes") > 0);
    let back = dir.join("back.wsp");
    let unpacked = stdout_json(&run(&[
        "unpack-artifact",
        binary.to_str().unwrap(),
        "--out",
        back.to_str().unwrap(),
    ]));
    assert_eq!(unpacked["layers"], 2);
    assert_eq!(fs::read_to_string(&back).expect("back"), to_wsp(&net));
}
