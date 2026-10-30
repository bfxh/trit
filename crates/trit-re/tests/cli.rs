//! CLI 端到端：JSONL 台账与人读摘要都必须来自真实解析。

mod support;

use std::process::Command;

#[test]
fn jsonl_and_human_modes_agree_on_the_sample() {
    let bytes = support::pe_sample();
    let path = std::env::temp_dir().join(format!("trit-re-cli-{}.bin", std::process::id()));
    std::fs::write(&path, &bytes).expect("write fixture");

    let jsonl = Command::new(env!("CARGO_BIN_EXE_trit-re"))
        .arg("triage")
        .arg(&path)
        .arg("--jsonl")
        .output()
        .expect("run trit-re");
    assert!(
        jsonl.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&jsonl.stderr)
    );
    let text = String::from_utf8_lossy(&jsonl.stdout);
    assert!(text.contains("\"kind\":\"file\""));
    assert!(text.contains(&format!(
        "\"sha256\":\"{}\"",
        trit_re::hash::sha256_hex(&bytes)
    )));
    assert!(text.contains("\"id\":\"packed\""));
    assert!(text.contains("\"four\":\"B\""));

    let human = Command::new(env!("CARGO_BIN_EXE_trit-re"))
        .arg("triage")
        .arg(&path)
        .output()
        .expect("run trit-re");
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("[B] packed"), "{text}");
    assert!(text.contains("pe32+"), "{text}");

    std::fs::remove_file(&path).ok();
}

#[test]
fn missing_file_exits_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_trit-re"))
        .arg("triage")
        .arg("definitely-not-a-file-here.bin")
        .output()
        .expect("run trit-re");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("读取失败"));
}
