#![cfg(unix)]
//! 随机表达式 / 网表 × 三代数 × 编译 C 逐行对拍。
//!
//! 固定种子，失败可复现；只调用 CLI，不依赖内部 IR 细节。
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ALGEBRAS: [&str; 3] = ["kleene", "lukasiewicz", "bochvar"];
const VARS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn pick(&mut self, upper: usize) -> usize {
        (self.next() % upper as u64) as usize
    }
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-random-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn expr(rng: &mut Rng, vars: &[&str], depth: usize) -> String {
    if depth == 0 || rng.pick(4) == 0 {
        return match rng.pick(5) {
            0 => "0".to_string(),
            1 => "1".to_string(),
            _ => vars[rng.pick(vars.len())].to_string(),
        };
    }
    match rng.pick(6) {
        0 => format!("(!{})", expr(rng, vars, depth - 1)),
        _ => {
            let op = ["&", "|", "^", "=>", "<=>"][rng.pick(5)];
            format!(
                "({} {} {})",
                expr(rng, vars, depth - 1),
                op,
                expr(rng, vars, depth - 1)
            )
        }
    }
}

fn netlist(rng: &mut Rng) -> String {
    let inputs = 3 + rng.pick(3);
    let gates = 3 + rng.pick(6);
    let mut sources: Vec<String> = VARS[..inputs].iter().map(|v| v.to_string()).collect();
    let mut lines = vec![format!(
        "module random({}, output y);",
        VARS[..inputs]
            .iter()
            .map(|v| format!("input {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    )];
    for i in 0..gates {
        let op = ["and", "or", "xor", "nand", "nor", "xnor"][rng.pick(6)];
        let left = &sources[rng.pick(sources.len())];
        let right = &sources[rng.pick(sources.len())];
        let wire = format!("w{i}");
        lines.push(format!("  {op} g{i} ({wire}, {left}, {right});"));
        sources.push(wire);
    }
    lines.push(format!("  assign y = {};", sources.last().unwrap()));
    lines.push("endmodule".to_string());
    lines.join("\n")
}

fn run(dir: &Path, name: &str, text: &str, algebra: &str) -> serde_json::Value {
    let ext = ["tri", "v"][usize::from(text.starts_with("module"))];
    let source = dir.join(format!("{name}.{ext}"));
    let out = dir.join(format!("{name}-{algebra}"));
    fs::write(&source, text).expect("写源文件");
    let output = Command::new(bin())
        .args([
            source.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--backend",
            "both",
            "--verify-c",
            "--algebra",
            algebra,
        ])
        .output()
        .expect("运行 trit-build");
    assert!(
        output.status.success(),
        "{name} {algebra} source={text}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout 是 JSON");
    assert_eq!(summary["verified"]["c"]["function"], "f", "{summary}");
    let rows = summary["verified"]["c"]["rows"].as_u64().expect("rows");
    let vars = summary["options"]["variables"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(rows, 3u64.pow(vars as u32), "{name} {algebra}: {summary}");
    assert!(
        summary["verified"]["c"]["result"]
            .as_str()
            .is_some_and(|value| value.starts_with("ok rows=")),
        "{name} {algebra}: {summary}"
    );
    summary
}

#[test]
fn random_expression_and_netlist_match_every_row() {
    let dir = scratch("diff");
    let mut rng = Rng::new(0x5eed_1234_abcd_ef01);
    for case in 0..16 {
        let nvars = 2 + rng.pick(5);
        let vars = &VARS[..nvars];
        let source = expr(&mut rng, vars, 3);
        for alg in ALGEBRAS {
            run(&dir, &format!("expr{case}"), &source, alg);
        }
    }
    for case in 0..8 {
        let source = netlist(&mut rng);
        for alg in ALGEBRAS {
            run(&dir, &format!("net{case}"), &source, alg);
        }
    }
    let _ = fs::remove_dir_all(&dir);
}
