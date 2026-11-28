//! 扇出网表的 `direct.c` 字节门：共享节点落临时变量，声明合并成一条语句。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-fanout-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn run(dir: &Path, name: &str, source: &str, verify: Option<&str>) -> String {
    let source_path = dir.join(format!("{name}.v"));
    let out = dir.join(format!("{name}-out"));
    fs::write(&source_path, source).expect("写源文件");
    let mut command = Command::new(env!("CARGO_BIN_EXE_trit-build"));
    command.args([
        source_path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--backend",
        if verify.is_some() { "both" } else { "direct" },
    ]);
    if let Some(alg) = verify {
        command.args(["--verify-c", "--force", "--algebra", alg]);
    }
    let output = command.output().expect("运行 trit-build");
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(alg) = verify {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("\"result\":\"ok") && stdout.contains("rows=81"),
            "{alg}: {stdout}"
        );
    }
    fs::read_to_string(out.join("direct.c")).expect("读 direct.c")
}

const FANOUT: [(&str, &str, usize); 2] = [
    (
        "fan3",
        "module fan3(input a, input b, input c, input d, output y);\n  wire w, p, q;\n  and g1 (w, a, b);\n  or  g2 (p, w, c);\n  xor g3 (q, w, d);\n  and g4 (y, p, q);\nendmodule\n",
        206,
    ),
    (
        "fan5",
        "module fan5(input a, input b, input c, input d, output y);\n  wire w1, w2, p, q;\n  xor g1 (w1, a, b);\n  xor g2 (w2, c, d);\n  or  g3 (p, w1, w2);\n  and g4 (q, w1, w2);\n  xor g5 (y, p, q);\nendmodule\n",
        162,
    ),
];

#[test]
fn fanout_bytes_stay_within_budget() {
    let dir = scratch("size");
    for (name, source, budget) in FANOUT {
        let text = run(&dir, name, source, None);
        let bytes = text.len();
        assert!(
            bytes <= budget,
            "{name}: {bytes} B 超过预算 {budget} B\n{text}"
        );
        if name == "fan5" {
            assert_eq!(text.matches("T[3").count(), 3, "{text}");
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn verify_c_matches_fanout_every_row() {
    let dir = scratch("verify");
    for alg in ["kleene", "lukasiewicz", "bochvar"] {
        run(&dir, &format!("fan5-{alg}"), FANOUT[1].1, Some(alg));
    }
    let _ = fs::remove_dir_all(&dir);
}
