//! HDL 真仿真：`--verify-hdl` 必须用 iverilog/vvp 跑通；本机没装就跳过。
//!
//! 这个测试证明的是"生成的 design.v 能被独立 HDL 工具执行且逐行对拍"，
//! 而不是再用 Rust 里的位级解释器自证。

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
        "trit-build-hdl-{}-{}-{}",
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

fn simulator_available() -> bool {
    let iverilog = std::env::var("TRIT_IVERILOG").unwrap_or_else(|_| "iverilog".to_string());
    let vvp = std::env::var("TRIT_VVP").unwrap_or_else(|_| "vvp".to_string());
    let one = Command::new(iverilog)
        .arg("-V")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    let two = Command::new(vvp)
        .arg("-V")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    one && two
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

#[test]
fn verify_hdl_runs_a_real_simulator_for_every_algebra() {
    if !simulator_available() {
        eprintln!("跳过：没有 iverilog/vvp（设置 TRIT_IVERILOG/TRIT_VVP 或安装 icarus-verilog）");
        return;
    }
    let dir = scratch("all");
    let cases = [
        ("(a & b) | !c", 3usize),
        ("a => b", 2),
        ("a <=> b", 2),
        ("a ^ b", 2),
        ("!((a | b) & (c => a))", 3),
        ("1", 0),
    ];
    for algebra in ["kleene", "lukasiewicz", "bochvar"] {
        for (index, (source, vars)) in cases.into_iter().enumerate() {
            let path = dir.join(format!("{algebra}-{index}.tri"));
            fs::write(&path, format!("{source}\n")).unwrap();
            let out = dir.join(format!("{algebra}-{index}-out"));
            let (stdout, stderr, code) = run(&[
                path.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--verify-hdl",
                "--algebra",
                algebra,
                "--threads",
                "2",
            ]);
            assert_eq!(code, 0, "{source} @ {algebra}: {stderr}\n{stdout}");
            let manifest: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            assert_eq!(manifest["verified"]["hdl"]["rows"], 3usize.pow(vars as u32));
            assert_eq!(
                manifest["verified"]["hdl"]["result"],
                format!("OK {} rows", 3usize.pow(vars as u32)),
                "{manifest}"
            );
            assert!(out.join("design_tb.v").exists());
            assert!(out.join("expected.mem").exists());
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_design_is_caught_by_the_testbench() {
    if !simulator_available() {
        eprintln!("跳过：没有 iverilog/vvp");
        return;
    }
    let dir = scratch("broken");
    let source = dir.join("e.tri");
    fs::write(&source, "a & b\n").unwrap();
    let out = dir.join("out");
    let (_, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--verify-hdl",
    ]);
    assert_eq!(code, 0, "{stderr}");
    // 换成一个永远输出 Unknown 的同端口模块：testbench 必须报 MISMATCH。
    fs::write(
        out.join("design.v"),
        "module trit_module(
  input wire a_p, a_n,
  input wire b_p, b_n,
  output wire y_p, y_n
);
  assign y_p = 1'b0;
  assign y_n = 1'b0;
endmodule
",
    )
    .unwrap();
    let iverilog = std::env::var("TRIT_IVERILOG").unwrap_or_else(|_| "iverilog".to_string());
    let vvp = std::env::var("TRIT_VVP").unwrap_or_else(|_| "vvp".to_string());
    let compile = Command::new(&iverilog)
        .args(["-g2012", "-o", "bad.vvp", "design_tb.v", "design.v"])
        .current_dir(&out)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let sim = Command::new(&vvp)
        .arg("bad.vvp")
        .current_dir(&out)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&sim.stdout);
    assert!(!sim.status.success(), "错设计必须让 testbench 红：{stdout}");
    assert!(stdout.contains("MISMATCH"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}
