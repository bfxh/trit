//! M1：Verilog 子集前端 + 双链路真值表对拍。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-build{}", std::env::consts::EXE_SUFFIX))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-m1-{}-{}-{}",
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

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const DESIGN: &str =
    "module m(input a, input b, input c, output y);\n  assign y = (a & b) | !c;\nendmodule\n";

#[test]
fn verilog_matches_expression_chain_row_by_row() {
    let dir = scratch("agree");
    let design = dir.join("d.v");
    fs::write(&design, DESIGN).expect("写 Verilog");
    let expr_file = dir.join("d.tri");
    fs::write(&expr_file, "(a & b) | !c\n").expect("写表达式");

    let vout = dir.join("vout");
    let tout = dir.join("tout");
    let (stdout, stderr, code) = run(&[
        design.to_str().unwrap(),
        "--out",
        vout.to_str().unwrap(),
        "--expect",
        "(a & b) | !c",
    ]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let manifest: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(manifest["source"]["kind"], "verilog");
    assert_eq!(manifest["verified"]["exact"], serde_json::json!(true));

    let (stdout2, stderr2, code2) =
        run(&[expr_file.to_str().unwrap(), "--out", tout.to_str().unwrap()]);
    assert_eq!(code2, 0, "{stderr2}");
    assert!(!stdout2.is_empty());

    let vcsv = fs::read_to_string(vout.join("truth.csv")).unwrap();
    let tcsv = fs::read_to_string(tout.join("truth.csv")).unwrap();
    assert_eq!(vcsv, tcsv, "两条链路的真值表必须逐行相同");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cross_check_rejects_wrong_expression() {
    let dir = scratch("mismatch");
    let design = dir.join("d.v");
    fs::write(&design, DESIGN).expect("写 Verilog");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[
        design.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--expect",
        "a | b | c",
    ]);
    assert_ne!(code, 0, "不等价必须失败");
    assert!(stderr.contains("两条链路不一致"), "{stderr}");
    // 失败时不得留下 build.json（不产出"优化后的错件"）。
    assert!(!out.join("build.json").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_output_requires_explicit_output() {
    let dir = scratch("multi");
    let design = dir.join("multi.v");
    fs::write(
        &design,
        "module m(input a, input b, output y, output z);\n  assign y = a & b;\n  assign z = a ^ b;\nendmodule\n",
    )
    .expect("写 Verilog");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[design.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("--output"), "{stderr}");

    let (stdout, stderr2, code2) = run(&[
        design.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--output",
        "z",
    ]);
    assert_eq!(code2, 0, "{stderr2}");
    let manifest: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(manifest["options"]["assignments"], 9);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manifest_runs_verilog_job_on_ternary_track() {
    let dir = scratch("manifest");
    fs::write(dir.join("d.v"), DESIGN).expect("写 Verilog");
    let manifest = dir.join("jobs.json");
    fs::write(
        &manifest,
        r#"{"jobs":[
            {"name":"dv","kind":"verilog","source":"d.v","expect":"(a & b) | !c"}
        ]}"#,
    )
    .expect("写清单");
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        "--manifest",
        manifest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--jobs",
        "2",
    ]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["failed"], 0);
    assert_eq!(value["jobs"][0]["track"], "ternary");
    assert!(out.join("dv").join("truth.tritbin").exists());
    let _ = fs::remove_dir_all(&dir);
}
