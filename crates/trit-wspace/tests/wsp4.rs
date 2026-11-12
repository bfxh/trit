//! WSP4：Huffman 编解码、WSP2 外层往返/压缩率、坏输入与 CLI。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::{
    Layer, Net, from_wsp4, huffman_decode, huffman_encode, to_wsp, to_wsp2, to_wsp4,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wsp4-{tag}-{}", std::process::id()));
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
    Net::new(vec![
        Layer::new(64, 64, &[1; 4096], &[0; 64]).expect("layer"),
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
fn huffman_round_trips() {
    let mut noise = Vec::new();
    let mut x = 0x9e37_79b9u32;
    for _ in 0..2000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        noise.push(x as u8);
    }
    let cases = vec![
        Vec::new(),
        vec![42; 500],
        (0..=255u8).collect(),
        (0..500u16).map(|i| (i % 3) as u8).collect(),
        noise,
        to_wsp2(&sample_net()).expect("wsp2"),
        to_wsp2(&uniform_net()).expect("wsp2 uniform"),
    ];
    for case in cases {
        let packed = huffman_encode(&case).expect("encodable");
        assert_eq!(huffman_decode(&packed, case.len()).expect("decode"), case);
    }
}

#[test]
fn wsp4_round_trips_and_shrinks_uniform_networks() {
    let net = uniform_net();
    let wsp2 = to_wsp2(&net).expect("wsp2");
    let wsp4 = to_wsp4(&net).expect("wsp4");
    assert_eq!(
        from_wsp4(&wsp4).expect("back").state_hash(),
        net.state_hash()
    );
    assert!(wsp4.len() < wsp2.len(), "单符号 Huffman 应显著压小");
    let sample = sample_net();
    assert_eq!(
        from_wsp4(&to_wsp4(&sample).expect("wsp4"))
            .expect("back")
            .state_hash(),
        sample.state_hash()
    );
}

#[test]
fn wsp4_rejects_corruption() {
    let bytes = to_wsp4(&uniform_net()).expect("wsp4");
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(from_wsp4(&bad).is_err(), "magic");
    let mut bad = bytes.clone();
    bad[4] = 9;
    reseal(&mut bad);
    assert!(from_wsp4(&bad).is_err(), "version");
    let mut bad = bytes.clone();
    bad[5] = 9;
    reseal(&mut bad);
    assert!(from_wsp4(&bad).is_err(), "method");
    assert!(from_wsp4(&bytes[..10]).is_err(), "truncated");
    let mut bad = bytes.clone();
    let end = bad.len() - 1;
    bad[end] ^= 1;
    assert!(from_wsp4(&bad).is_err(), "checksum");
    let mut bad = bytes.clone();
    bad[6..14].copy_from_slice(&999u64.to_le_bytes());
    reseal(&mut bad);
    assert!(from_wsp4(&bad).is_err(), "raw_len");
}

#[test]
fn huffman_decode_rejects_bad_streams() {
    assert!(huffman_decode(&vec![0u8; 255], 3).is_err(), "码长表截断");
    assert!(huffman_decode(&vec![0u8; 256], 3).is_err(), "空码表");
    let mut lens = vec![0u8; 256];
    lens[0] = 1;
    let mut data = lens.clone();
    data.push(0);
    assert!(huffman_decode(&data, 9).is_err(), "位流不足");
    let mut bad = vec![0u8; 256];
    bad[0] = 33;
    advance(&mut bad);
    assert!(huffman_decode(&bad, 1).is_err(), "码长越界");
}

fn advance(bytes: &mut [u8]) {
    // 让“码长 33”单独出现时命中 max 32 的检查（表本身非法）。
    bytes[1] = 0;
}

#[test]
fn cli_pack_huffman_round_trip() {
    let dir = temp_dir("cli");
    let net = uniform_net();
    let artifact = dir.join("net.wsp");
    fs::write(&artifact, to_wsp(&net)).expect("wsp");
    let binary = dir.join("net.wsp4");
    let packed = stdout_json(&run(&[
        "pack-artifact",
        artifact.to_str().unwrap(),
        "--out",
        binary.to_str().unwrap(),
        "--huffman",
    ]));
    assert_eq!(packed["format"], json!("wsp4"));
    assert!(packed["bytes"].as_u64().expect("bytes") > 0);
    let back = dir.join("back.wsp");
    let _unpacked = stdout_json(&run(&[
        "unpack-artifact",
        binary.to_str().unwrap(),
        "--out",
        back.to_str().unwrap(),
    ]));
    assert_eq!(fs::read_to_string(&back).expect("back"), to_wsp(&net));
}
