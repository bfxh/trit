//! 格对偶恒等式：`(a ∨ b) ⊕ (a ∧ b) = a ⊕ b`，强加与经典 XOR 三代数都成立。
//! 这里锁住强加分支的化简与逐赋值对拍；Verilog `XOR2` 分支由 `direct_c_fanout` 的 fan5 锁住。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-lattice-xor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn run(dir: &Path, alg: &str) -> String {
    let source = dir.join("id.tri");
    fs::write(&source, "(a | b) ^ (a & b)\n").expect("写源文件");
    let out = dir.join(format!("out-{alg}"));
    let output = Command::new(env!("CARGO_BIN_EXE_trit-build"))
        .args([
            source.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--backend",
            "both",
            "--verify-c",
            "--force",
            "--algebra",
            alg,
        ])
        .output()
        .expect("运行 trit-build");
    assert!(
        output.status.success(),
        "{alg}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"result\":\"ok") && stdout.contains("rows=9"),
        "{alg}: {stdout}"
    );
    fs::read_to_string(out.join("direct.c")).expect("读 direct.c")
}

#[test]
fn strong_xor_meet_join_identity_drops_and_or_tables() {
    let dir = scratch();
    let k3 = run(&dir, "kleene");
    assert_eq!(k3.len(), 120, "{k3}");
    assert!(!k3.contains("T_AND") && !k3.contains("T_OR"), "{k3}");
    assert_eq!(k3.matches("T[3").count(), 1, "{k3}");
    for alg in ["lukasiewicz", "bochvar"] {
        let c = run(&dir, alg);
        assert!(!c.contains("T_AND") && !c.contains("T_OR"), "{c}");
        assert_eq!(c.matches("T[3").count(), 1, "{c}");
    }
    let _ = fs::remove_dir_all(&dir);
}
