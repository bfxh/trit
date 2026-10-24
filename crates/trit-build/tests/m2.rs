//! M2：TRB2 打包/解码 + 双导轨 Verilog 导出。
//!
//! 导出的 `design.v` 用测试内的**位级解释器**求值（不依赖 iverilog/verilator），
//! 与 `truth.csv` 逐行比对；两端任一不同都算失败。

use std::collections::BTreeMap;
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
        "trit-build-m2-{}-{}-{}",
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

/// 读取 truth.csv 的判定列（-1/0/1）。
fn verdicts(csv: &str) -> Vec<i8> {
    csv.lines()
        .skip(1)
        .map(|line| line.rsplit(',').next().unwrap().trim().parse().unwrap())
        .collect()
}

// ---------- 位级解释器（只覆盖本工具生成的子集） ----------

fn eval_expr(text: &str, env: &BTreeMap<String, u8>) -> u8 {
    struct P<'a> {
        chars: Vec<char>,
        index: usize,
        env: &'a BTreeMap<String, u8>,
    }
    impl P<'_> {
        fn parse_or(&mut self) -> u8 {
            let mut value = self.parse_and();
            loop {
                self.skip_ws();
                if self.peek() == Some('|') {
                    self.index += 1;
                    let right = self.parse_and();
                    value |= right;
                } else {
                    return value;
                }
            }
        }
        fn parse_and(&mut self) -> u8 {
            let mut value = self.parse_unary();
            loop {
                self.skip_ws();
                if self.peek() == Some('&') {
                    self.index += 1;
                    let right = self.parse_unary();
                    value &= right;
                } else {
                    return value;
                }
            }
        }
        fn parse_unary(&mut self) -> u8 {
            self.skip_ws();
            if self.peek() == Some('~') {
                self.index += 1;
                return 1 - self.parse_unary();
            }
            if self.peek() == Some('(') {
                self.index += 1;
                let value = self.parse_or();
                self.skip_ws();
                assert_eq!(self.peek(), Some(')'), "缺少右括号 @ {}", self.index);
                self.index += 1;
                return value;
            }
            self.parse_atom()
        }
        fn rest(&self) -> String {
            self.chars[self.index..].iter().collect()
        }
        fn parse_atom(&mut self) -> u8 {
            self.skip_ws();
            let rest = self.rest();
            if rest.starts_with("1'b0") {
                self.index += 4;
                return 0;
            }
            if rest.starts_with("1'b1") {
                self.index += 4;
                return 1;
            }
            let start = self.index;
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || c == '_' {
                    self.index += 1;
                } else {
                    break;
                }
            }
            assert!(self.index > start, "无法解析 @ {start}");
            let name: String = self.chars[start..self.index].iter().collect();
            *self
                .env
                .get(&name)
                .unwrap_or_else(|| panic!("未定义信号 {name}"))
        }
        fn peek(&self) -> Option<char> {
            self.chars.get(self.index).copied()
        }
        fn skip_ws(&mut self) {
            while matches!(self.peek(), Some(' ') | Some('\t')) {
                self.index += 1;
            }
        }
    }
    let mut parser = P {
        chars: text.chars().collect(),
        index: 0,
        env,
    };
    parser.parse_or()
}

/// 解析生成的模块：返回 (输入名, 赋值列表)。
fn parse_design(text: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut inputs = Vec::new();
    let mut assigns = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("input wire") {
            for part in line
                .trim_end_matches(',')
                .replace("input wire", "")
                .split(',')
            {
                inputs.push(part.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("assign ") {
            let (name, rhs) = rest.split_once('=').expect("assign 缺 =");
            assigns.push((
                name.trim().to_string(),
                rhs.trim_end_matches(';').trim().to_string(),
            ));
        }
    }
    (inputs, assigns)
}

#[test]
fn trb2_is_smaller_and_decodes_back() {
    let dir = scratch("trb2");
    let source = dir.join("e.tri");
    fs::write(&source, "(a & b) | !c\n").expect("写源文件");
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[source.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let manifest: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // 27 + 3 trit = 30 / 5 per byte = 6 字节 + 11 字节头 = 17；TRB1 双导轨是 35。
    assert_eq!(manifest["outputs"]["truth.tritbin"]["bytes"], 17);

    let decoded = dir.join("decoded");
    let (stdout2, stderr2, code2) = run(&[
        "--decode",
        out.join("truth.tritbin").to_str().unwrap(),
        "--out",
        decoded.to_str().unwrap(),
    ]);
    assert_eq!(code2, 0, "{stderr2}");
    assert!(stdout2.contains("\"crc3\":true"), "{stdout2}");
    let decoded_manifest: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
    assert_eq!(decoded_manifest["options"]["algebra"], "kleene");
    let original = fs::read_to_string(out.join("truth.csv")).unwrap();
    let round = fs::read_to_string(decoded.join("truth.csv")).unwrap();
    assert_eq!(
        verdicts(&original),
        verdicts(&round),
        "解码后判定列必须一致"
    );
    let reencoded = fs::read(decoded.join("truth.tritbin")).unwrap();
    let original_blob = fs::read(out.join("truth.tritbin")).unwrap();
    assert_eq!(reencoded, original_blob, "解码后重编码必须逐字节相同");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emit_verilog_is_not_served_from_a_plain_cache() {
    let dir = scratch("cache");
    let source = dir.join("e.tri");
    fs::write(&source, "(a & b) | !c\n").expect("写源文件");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[source.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        !out.join("design.v").exists(),
        "没要 --emit-verilog 就不该有 design.v"
    );

    let (stdout, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--emit-verilog",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("\"cached\":false"),
        "换选项必须换缓存键：{stdout}"
    );
    assert!(
        out.join("design.v").exists(),
        "--emit-verilog 必须真的写出 design.v"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decode_rejects_tampered_blob() {
    let dir = scratch("tamper");
    let source = dir.join("e.tri");
    fs::write(&source, "a ^ b\n").expect("写源文件");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[source.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let blob_path = out.join("truth.tritbin");
    let mut bytes = fs::read(&blob_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0b101;
    fs::write(&blob_path, &bytes).unwrap();
    let (_, stderr, code) = run(&[
        "--decode",
        blob_path.to_str().unwrap(),
        "--out",
        dir.join("x").to_str().unwrap(),
    ]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("CRC-3") || stderr.contains("长度不符"),
        "{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 导出 `design.v` 后用测试内位级解释器求值，与 `truth.csv` 逐行比对。
fn two_rail_matches_truth_table(source_text: &str, vars: &[&str]) {
    let dir = scratch("rail");
    let source = dir.join("e.tri");
    fs::write(
        &source,
        format!(
            "{source_text}
"
        ),
    )
    .expect("写源文件");
    let out = dir.join("out");
    let (stdout, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--emit-verilog",
    ]);
    assert_eq!(
        code, 0,
        "stdout={stdout}
stderr={stderr}"
    );
    assert!(out.join("design.v").exists());

    let design = fs::read_to_string(out.join("design.v")).unwrap();
    let (inputs, assigns) = parse_design(&design);
    assert_eq!(inputs.len(), vars.len() * 2, "每个三值输入 → 两根位线");
    let csv = fs::read_to_string(out.join("truth.csv")).unwrap();
    let expected = verdicts(&csv);
    assert_eq!(expected.len(), 3usize.pow(vars.len() as u32));

    let mut wrong = 0usize;
    let mut illegal = 0usize;
    for (code, want) in expected.iter().enumerate() {
        let mut env = BTreeMap::new();
        let mut rest = code;
        for var in vars {
            let digit = rest % 3;
            rest /= 3;
            let (pos, neg) = match digit {
                0 => (0u8, 1u8),
                1 => (0u8, 0u8),
                _ => (1u8, 0u8),
            };
            env.insert(format!("{var}_p"), pos);
            env.insert(format!("{var}_n"), neg);
        }
        for (name, rhs) in &assigns {
            let value = eval_expr(rhs, &env);
            env.insert(name.clone(), value);
        }
        let (pos, neg) = (env["y_p"], env["y_n"]);
        if pos == 1 && neg == 1 {
            illegal += 1;
        }
        let got = match (pos, neg) {
            (1, 0) => 1,
            (0, 1) => -1,
            (0, 0) => 0,
            _ => i8::MIN,
        };
        if got != *want {
            wrong += 1;
        }
    }
    assert_eq!(wrong, 0, "{source_text}：位级仿真与真值表不一致 {wrong} 行");
    assert_eq!(
        illegal, 0,
        "{source_text}：出现 (1,1) 非法导轨组合 {illegal} 行"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emitted_two_rail_matches_truth_table() {
    two_rail_matches_truth_table("(a & b) | !c", &["a", "b", "c"]);
    two_rail_matches_truth_table("!(a | (b & !c))", &["a", "b", "c"]);
}

#[test]
fn emitted_two_rail_matches_truth_table_for_strong_add() {
    // `Expr::Xor` 是强加法 ⊕：K3 下 U 传播、双双为真才真（等价于 AND），不是布尔异或。
    two_rail_matches_truth_table("a ^ b", &["a", "b"]);
    two_rail_matches_truth_table("!(a ^ b) | (c ^ a)", &["a", "b", "c"]);
}

#[test]
fn emit_verilog_handles_bare_variable() {
    let dir = scratch("bare");
    let source = dir.join("e.tri");
    fs::write(&source, "a\n").expect("写源文件");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--emit-verilog",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let design = fs::read_to_string(out.join("design.v")).unwrap();
    assert!(
        !design.contains("wire t"),
        "没有内部节点就不该声明 wire：{design}"
    );
    assert!(design.contains("assign y_p = a_p;"), "{design}");
    assert!(design.contains("assign y_n = a_n;"), "{design}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emit_verilog_rejects_non_kleene() {
    let dir = scratch("alg");
    let source = dir.join("e.tri");
    fs::write(&source, "a & b\n").expect("写源文件");
    let (_, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
        "--emit-verilog",
        "--algebra",
        "lukasiewicz",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("kleene"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}
