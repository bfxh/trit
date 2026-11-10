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

    let (stdout, _, code) = run(&[
        "interleave",
        "--block",
        "3",
        "--deinterleave",
        "--len",
        "7",
        &shuffled,
    ]);
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

#[test]
fn rle_cli_round_trips() {
    // 111XX00001 → runs [["1",3],["X",2],["0",4],["1",1]]，总长 10。
    let (stdout, _, code) = run(&["rle", "111XX00001"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["run_count"], 4);
    let runs = v["runs"].to_string();
    let trits = v["trits"].as_str().unwrap().to_string();

    let (stdout, _, code) = run(&["rle", "--decode", "--len", &trits.len().to_string(), &runs]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["trits"], "111XX00001");
}

#[test]
fn delta_cli_round_trips() {
    let raw = "10X01XX10";
    let (stdout, _, code) = run(&["delta", raw]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["trits"], raw);
    let deltas = v["deltas"].as_str().unwrap().to_string();

    let (stdout, _, code) = run(&["delta", "--decode", &deltas]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["deltas"], deltas);
    assert_eq!(v["trits"], raw);
}

#[test]
fn gray_cli_round_trips() {
    // encode：gray --index 5 --n 2。手算 n=2 时 index 5 的码字应为 "XX"（锚点）。
    let (stdout, _, code) = run(&["gray", "--index", "5", "--n", "2"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["index"], 5);
    assert_eq!(v["n"], 2);
    let codeword = v["codeword"].as_str().unwrap().to_string();
    assert_eq!(codeword, "XX");

    // decode：gray --decode <codeword>，index 必须往返回 5。
    let (stdout, _, code) = run(&["gray", "--decode", &codeword]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["word"], codeword);
    assert_eq!(v["index"], 5);
}

#[test]
fn count_cli_basic() {
    // 手算："0X1" 覆盖 3 个赋值，"100" 覆盖 1 个，两者在首位 0/1 冲突无交集。
    // 并集 = 3 + 1 = 4。
    let (stdout, _, code) = run(&["count", "[\"0X1\",\"100\"]"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["count"], 4);
    assert_eq!(v["terms"], serde_json::json!(["0X1", "100"]));

    // 重叠情形："X0X"（9 个）完全包含 "X00"（3 个）→ 并集仍为 9。
    let (stdout, _, code) = run(&["count", "[\"X0X\",\"X00\"]"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["count"], 9);
}

#[test]
fn sop_cli_basic() {
    // rows JSON：[[input_string, output_char], ...]。
    // True 行 = (a,b=False) 的全部三个具体赋值 → 应化简成单条 "X0"。
    let (stdout, _, code) = run(&["sop", "[[\"00\",\"1\"],[\"x0\",\"1\"],[\"10\",\"1\"]]"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("sop 输出必须是合法 JSON");
    assert_eq!(v["term_count"], 1);
    assert_eq!(v["terms"], serde_json::json!(["X0"]));
}
