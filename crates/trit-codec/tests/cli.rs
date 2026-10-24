//! `trit-codec` CLI 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-codec") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-codec{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-codec");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn pack_and_unpack_round_trip_through_the_cli() {
    let (packed, _, code) = run(&["pack", "01X"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&packed).expect("valid json");
    assert_eq!(value["bytes"], "0f");
    assert_eq!(value["packed_len"], 1);

    let (unpacked, _, code) = run(&["unpack", "0f", "--len", "3"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&unpacked).expect("valid json");
    assert_eq!(value["trits"], "01X");
}

#[test]
fn stats_reports_the_density_win() {
    let (stdout, _, code) = run(&["stats", "--trits", "1000"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["packed_bytes"], 200);
    assert_eq!(value["dual_rail_bytes"], 250);
    assert_eq!(value["packed_bits_per_trit"], 1.6);
    assert_eq!(value["balanced_max_digits_i64"], 41);
}

#[test]
fn hamming_encodes_and_corrects_one_symbol() {
    let (stdout, _, code) = run(&["hamming", "01"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["codeword"], "011X");

    let (stdout, _, code) = run(&["hamming", "--decode", "0111"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["message"], "01");
    assert_eq!(value["corrected"], serde_json::json!([3, 1]));
}

#[test]
fn balanced_ternary_round_trip() {
    let (stdout, _, code) = run(&["balanced", "-13"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["balanced"], "---");
    assert_eq!(value["digits"], 3);
}

#[test]
fn invalid_unpack_length_is_rejected() {
    let (_, stderr, code) = run(&["unpack", "0f", "--len", "9"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("长度不足"), "{stderr}");
}

#[test]
fn quantize_then_dequantize_round_trips_signs() {
    let (stdout, _, code) = run(&["quantize", "1.0,-1.0,0.2"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["trits"], "10X");
    let scale = v["scale"].as_f64().unwrap();

    let (stdout, _, code) = run(&["dequantize", "10X", "--scale", &scale.to_string()]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["values"][0].as_f64().unwrap(), scale);
    assert_eq!(v["values"][1].as_f64().unwrap(), -scale);
    assert_eq!(v["values"][2].as_f64().unwrap(), 0.0);
}

#[test]
fn sparse_encode_then_decode_round_trips() {
    let (stdout, _, code) = run(&["sparse", "10XX0X1"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entries = v["entries"].to_string();

    let (stdout, _, code) = run(&["sparse", "--decode", "--len", "7", &entries]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["trits"], "10XX0X1");
}

#[test]
fn interleave_then_deinterleave_round_trips_with_len() {
    let (stdout, _, code) = run(&["interleave", "--block", "3", "10X10X1"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let shuffled = v["trits"].as_str().unwrap().to_string();

    let (stdout, _, code) =
        run(&["interleave", "--block", "3", "--deinterleave", "--len", "7", &shuffled]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["trits"], "10X10X1");
}

#[test]
fn checksum_append_then_verify_passes() {
    let (stdout, _, code) = run(&["checksum", "10X"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let appended = v["appended"].as_str().unwrap().to_string();

    let (stdout, _, code) = run(&["checksum", "--verify", &appended]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["valid"], true);
}
