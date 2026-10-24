//! M3：e-graph 重写接进 CLI 之后，产物必须仍然逐字节可对拍；
//! 规则集按语义过滤（K3 有蕴含消去，Ł3 没有）。

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
        "trit-build-m3-{}-{}-{}",
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

fn build(dir: &std::path::Path, name: &str, source: &str, extra: &[&str]) -> (String, String, i32) {
    let path = dir.join(format!("{name}.tri"));
    fs::write(&path, format!("{source}\n")).expect("写源文件");
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

fn manifest(source: &str) -> serde_json::Value {
    serde_json::from_str(source).expect("build.json 应是合法 JSON")
}

#[test]
fn rewrite_fires_and_preserves_truth_table() {
    let dir = scratch("factor");
    let (_, stderr, code) = build(&dir, "expanded", "(a & b) | (a & c)", &["--emit-verilog"]);
    assert_eq!(code, 0, "{stderr}");
    let (_, stderr, code) = build(&dir, "factored", "a & (b | c)", &[]);
    assert_eq!(code, 0, "{stderr}");

    let expanded = fs::read_to_string(dir.join("expanded-out/expr.txt")).unwrap();
    assert!(expanded.contains("rewritten: (a ∧ (b ∨ c))"), "{expanded}");
    let summary = manifest(&fs::read_to_string(dir.join("expanded-out/build.json")).unwrap());
    let rules = summary["verified"]["rules"]
        .as_array()
        .expect("rules 必须是数组");
    assert!(
        rules.iter().any(|rule| rule == "factor-and-or"),
        "重写没触发：{rules:?}"
    );

    // 两条式子的真值表必须逐字节相同（重写保语义的最硬判据）。
    let left = fs::read(dir.join("expanded-out/truth.csv")).unwrap();
    let right = fs::read(dir.join("factored-out/truth.csv")).unwrap();
    assert_eq!(left, right, "重写前后 truth.csv 必须逐字节相同");
    let left_blob = fs::read(dir.join("expanded-out/truth.tritbin")).unwrap();
    let right_blob = fs::read(dir.join("factored-out/truth.tritbin")).unwrap();
    assert_eq!(left_blob, right_blob, "TRB2 也必须逐字节相同");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rule_set_is_filtered_by_algebra() {
    let dir = scratch("algebra");
    // K3：材料蕴含，`a => b` 可消去。
    let (stdout, stderr, code) = build(&dir, "k3", "a => b", &[]);
    assert_eq!(code, 0, "{stderr}");
    let rules = manifest(&stdout)["verified"]["rules"].clone();
    assert!(
        rules
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule == "implies-elim"),
        "K3 应该用蕴含消去：{rules}"
    );

    // Ł3：`X => X = 1`，材料蕴含不成立 ⇒ 这条规则必须自证失败、不进图。
    let (stdout, stderr, code) = build(&dir, "l3", "a => b", &["--algebra", "lukasiewicz"]);
    assert_eq!(code, 0, "{stderr}");
    let rules = manifest(&stdout)["verified"]["rules"].clone();
    assert!(
        rules.as_array().unwrap().is_empty(),
        "Ł3 不该用蕴含消去：{rules}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bit_parallel_build_keeps_row_order() {
    let dir = scratch("order");
    let (stdout, stderr, code) = build(&dir, "two", "a & b", &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(manifest(&stdout)["options"]["assignments"], 9);
    let csv = fs::read_to_string(dir.join("two-out/truth.csv")).unwrap();
    // 行序必须是 code 0..8，列序 a,b，最后一列是 -1/0/1。
    let rows: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(rows.len(), 9);
    assert_eq!(rows[0], "0,-1,-1,-1");
    assert_eq!(rows[4], "4,0,0,0");
    assert_eq!(rows[8], "8,1,1,1");
    let _ = fs::remove_dir_all(&dir);
}
