//! Verilog 源的 direct 后端：产物形状与 C 逐行对拍（三种代数）。
//!
//! 与 `direct_c.rs` 分开：那份钉 `.tri` 的默认/缓存/边界，这份只钉网表链路。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DESIGN: &str = "module m(input a, input b, input c, output y);\n  wire n, o, k, x, z;\n  nand g1 (n, a, b);\n  nor  g2 (o, a, c);\n  buf  g3 (k, c);\n  xnor g4 (x, a, b);\n  assign z = 1'b1 & 1'b0;\n  assign y = (n & k) | (o ^ x) | z;\nendmodule\n";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-direct-{}-{}-{}",
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

fn run(dir: &Path, name: &str, extra: &[&str]) -> (String, String, i32) {
    let path = dir.join("design.v");
    fs::write(&path, DESIGN).expect("写 Verilog");
    let out = dir.join(format!("{name}-out"));
    let mut args = vec![
        path.to_str().unwrap().to_string(),
        "--out".to_string(),
        out.to_str().unwrap().to_string(),
    ];
    args.extend(extra.iter().map(|item| item.to_string()));
    let output = Command::new(bin())
        .args(&args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn direct_backend_emits_c_for_verilog() {
    let dir = scratch("verilog");
    let (stdout, stderr, code) = run(&dir, "v", &["--backend", "direct"]);
    assert_eq!(code, 0, "{stderr}");
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout 是 JSON");
    assert_eq!(summary["source"]["kind"], "verilog", "{summary}");
    assert!(summary["outputs"].get("direct.c").is_some(), "{summary}");
    assert!(summary["outputs"].get("lookup.c").is_none(), "{summary}");
    assert!(
        !dir.join("v-out/lookup.c").exists(),
        "direct 后端不该写 lookup.c"
    );
    let source = fs::read_to_string(dir.join("v-out/direct.c")).expect("读 direct.c");
    assert!(
        source.contains("int8_t trit_direct(const int8_t*v,unsigned n){"),
        "{source}"
    );
    assert!(source.contains("-1,0,1,0,0,0,1,0,-1"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn verify_c_matches_verilog_every_row() {
    let dir = scratch("verify-verilog");
    for alg in ["kleene", "lukasiewicz", "bochvar"] {
        let (stdout, stderr, code) = run(
            &dir,
            &format!("v-{alg}"),
            &["--backend", "both", "--verify-c", "--algebra", alg],
        );
        assert_eq!(code, 0, "{alg}: {stderr}");
        let summary: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("stdout 是 JSON");
        let result = summary["verified"]["c"]["result"]
            .as_str()
            .expect("verified.c.result");
        assert!(result.starts_with("ok rows=27"), "{alg}: {summary}");
    }
    let _ = fs::remove_dir_all(&dir);
}
