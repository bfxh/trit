//! `gguf-audit`：模型级往返要分开「语义核心」与「已知的字节/标记损失」。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::{Layer, Net, to_wsp};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-gguf-audit-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args(args)
        .output()
        .expect("run cli")
}

#[test]
fn audit_separates_semantics_from_expected_losses() {
    let dir = temp_dir("mixed");
    let weight = Net::new(vec![
        Layer::with_scales(2, 3, &[1, 0, -1, 0, 1, -1], &[1, -1], vec![0.5, 1.5])
            .expect("layer")
            .with_bias_f32(vec![0.25, -0.5])
            .expect("bias_f32"),
    ])
    .expect("weight");
    let vector =
        Net::new(vec![Layer::new(1, 3, &[1, 0, -1], &[0]).expect("vector")]).expect("vector net");
    fs::write(dir.join("0000-w.wsp"), to_wsp(&weight)).expect("wsp weight");
    fs::write(dir.join("0001-v.wsp"), to_wsp(&vector)).expect("wsp vector");
    let manifest = json!({
        "source": "audit-fixture",
        "tensors": [
            { "name": "encoder.weight", "file": "0000-w.wsp", "rows": 2, "cols": 3, "bias": "encoder.bias" },
            { "name": "pool.vector", "file": "0001-v.wsp", "rows": 1, "cols": 3, "vector": true },
        ]
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("manifest"),
    )
    .expect("write manifest");

    let out = run(&["gguf-audit", dir.to_str().expect("dir")]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).expect("report json");
    assert_eq!(report["tensors"], 2);
    assert_eq!(report["roundtrip_tensors"], 2);
    assert_eq!(report["bytes_identical"], 1, "{report}");
    assert_eq!(report["semantics_equal"], 2, "{report}");
    assert_eq!(report["biasf_lost"], 1, "{report}");
    assert_eq!(report["manifest"]["vector_markers"], 1);
    assert_eq!(report["manifest"]["vector_preserved"], 0);
    assert_eq!(report["manifest"]["bias_markers"], 1);
    assert_eq!(report["manifest"]["bias_preserved"], 0);
    assert_eq!(report["ok"], true);
}
