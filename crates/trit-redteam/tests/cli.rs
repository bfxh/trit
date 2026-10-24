//! CLI 行为：三个检查各自的发现与 `--strict` 退出码（可直接当 CI 门）。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// 由 `trit-build`（main `582b6c6`）对 `a & b` 生成的真实产物（14 字节）。
const BLOB_HEX: &str = "5452423202000209000000514004";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-redteam-cli-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn blob() -> Vec<u8> {
    (0..BLOB_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&BLOB_HEX[i..i + 2], 16).unwrap())
        .collect()
}

fn run(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_trit-redteam"))
        .args(args)
        .output()
        .expect("运行 trit-redteam");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn encode_reports_all_equivalent_encodings() {
    let dir = scratch("encode");
    let path = dir.join("truth.tritbin");
    fs::write(&path, blob()).expect("写 blob");
    let (stdout, stderr, code) = run(&["encode", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["padding"]["unused_trits"], 3);
    assert_eq!(report["padding"]["equivalent_encodings"], 27);
    assert_eq!(report["padding"]["other_encodings"], 26);
    assert_eq!(report["padding"]["canonical"], true);
    let samples = report["samples"].as_array().unwrap();
    assert!(!samples.is_empty());
    for sample in samples {
        assert_eq!(sample["same_table"], true, "变体必须解出同一张表");
        assert_ne!(sample["fnv64"], report["blob"]["fnv64"], "哈希必须不同");
    }

    // `--strict`：有发现就非 0。
    let (_, _, code) = run(&["encode", path.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "有非规范编码时 --strict 必须判红");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn crc_reports_minimum_undetected_weight() {
    let (stdout, stderr, code) = run(&["crc", "--trits", "9", "--max-weight", "3"]);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["detects_all_up_to_weight_3"], false);
    assert_eq!(report["min_undetected"]["weight"], 3);
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows[0]["weight"], 1);
    assert_eq!(rows[0]["tested"], 18);
    assert_eq!(rows[0]["undetected"], 0);
    assert_eq!(rows[1]["tested"], 288);

    let (_, _, code) = run(&["crc", "--trits", "9", "--max-weight", "3", "--strict"]);
    assert_eq!(code, 1, "CRC-3 漏检重量 3 时 --strict 必须判红");
}

#[test]
fn rail_reports_fail_open_on_illegal_inputs() {
    let design = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/three_rail.v");
    let (stdout, stderr, code) = run(&["rail", design.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["legal_inputs"], 27);
    assert_eq!(report["legal_output_invalid"], 0);
    assert!(report["illegal_output_true"].as_u64().unwrap() > 0);
    assert_eq!(report["all_illegal_output"], "invalid");
    assert!(!report["examples"].as_array().unwrap().is_empty());

    let (_, _, code) = run(&["rail", design.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "存在 fail-open 时 --strict 必须判红");
}

#[test]
fn fuzz_agrees_with_the_c_runtime() {
    let dir = scratch("fuzz");
    let path = dir.join("truth.tritbin");
    fs::write(&path, blob()).expect("写 blob");
    let (stdout, stderr, code) = run(&[
        "fuzz",
        path.to_str().unwrap(),
        "--iterations",
        "200",
        "--seed",
        "3",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    if report["skipped"] == true {
        eprintln!("跳过：本机没有 C 编译器");
        return;
    }
    assert_eq!(report["cases"], 200);
    assert_eq!(report["disagreements"], 0, "Rust 与 C 必须一致：{report}");
    assert!(report["rejected"].as_u64().unwrap() > 0, "{report}");
    // 没有分歧 ⇒ --strict 不该判红。
    let (_, _, code) = run(&[
        "fuzz",
        path.to_str().unwrap(),
        "--iterations",
        "200",
        "--seed",
        "3",
        "--strict",
    ]);
    assert_eq!(code, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn chain_flags_a_binary_job() {
    let dir = scratch("chain");
    let path = dir.join("jobs.json");
    fs::write(
        &path,
        r#"{"jobs":[{"name":"x","kind":"binary","cmd":["curl","http://example.invalid"]}]}"#,
    )
    .expect("写 jobs.json");
    let (stdout, stderr, code) = run(&["chain", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["binary_jobs"], 1);
    let flags = report["binaries"][0]["flags"].as_array().unwrap();
    assert!(flags.iter().any(|flag| flag == "network-tool"), "{flags:?}");
    let (_, _, code) = run(&["chain", path.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "有 binary 任务时 --strict 必须判红");
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn rejects_a_non_trb2_file() {
    let dir = scratch("bad");
    let path = dir.join("junk.bin");
    fs::write(&path, b"not a trb2 blob").unwrap();
    let (_, stderr, code) = run(&["encode", path.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("error"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}
