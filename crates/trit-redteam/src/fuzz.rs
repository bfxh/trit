//! TRB2 的差分模糊测试：**Rust 解析器 vs C 运行时**必须对每个畸形 blob 给出一致结论。
//!
//! 两份实现互为 oracle：只要有一边把「该拒绝的」接受、或对同一字节串解出不同的表，
//! 就说明格式契约没写清或有实现 bug。这比单侧断言强，因为单侧测试只能证明"它自己喜欢自己"。
//!
//! 输入用确定性的变异语料（固定种子 ⇒ 可复现）：翻位、改成非法字节 243..255、
//! 截断/加长、改头字段（版本/语义/nvars/长度）、改填充槽。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::blob;

/// C 侧 harness：读「长度前缀 + blob」的用例文件，每例输出一行。
const HARNESS: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include "trit_runtime.h"

static uint64_t fnv_trits(const int8_t *data, uint32_t len) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (uint32_t i = 0; i < len; i++) {
        hash ^= (uint8_t)(data[i] + 1);
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: %s cases.bin\n", argv[0]); return 2; }
    FILE *in = fopen(argv[1], "rb");
    if (in == NULL) { perror("open"); return 2; }
    for (;;) {
        uint32_t len = 0;
        if (fread(&len, 4, 1, in) != 1) { break; }
        unsigned char *buf = malloc(len == 0 ? 1u : len);
        if (buf == NULL) { fclose(in); return 2; }
        if (fread(buf, 1, len, in) != len) { free(buf); break; }
        trit_table table;
        int status = trit_trb2_decode(buf, (size_t)len, &table);
        if (status != TRIT_OK) {
            printf("err %d\n", status);
        } else {
            printf("ok %u %016llx\n", table.count, (unsigned long long)fnv_trits(table.trits, table.count));
            trit_table_free(&table);
        }
        free(buf);
    }
    fclose(in);
    return 0;
}
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok { count: usize, hash: u64 },
    Err,
}

impl Verdict {
    fn to_json(&self) -> Value {
        match self {
            Self::Ok { count, hash } => {
                json!({"ok": true, "count": count, "hash": format!("{hash:016x}")})
            }
            Self::Err => json!({"ok": false}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Disagreement {
    pub index: usize,
    pub bytes: Vec<u8>,
    pub rust: Verdict,
    pub c: Verdict,
}

#[derive(Debug, Clone, Default)]
pub struct Comparison {
    pub cases: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub disagreements: Vec<Disagreement>,
}

fn fnv_trits(values: &[i8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in values {
        hash ^= (*value as i16 + 1) as u8 as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn rust_verdict(bytes: &[u8]) -> Verdict {
    match blob::parse(bytes) {
        Ok(parsed) => Verdict::Ok {
            count: parsed.values.len(),
            hash: fnv_trits(&parsed.values),
        },
        Err(_) => Verdict::Err,
    }
}

pub fn compare(rust: &[Verdict], c: &[Verdict], cases: &[Vec<u8>]) -> Comparison {
    let mut comparison = Comparison {
        cases: cases.len(),
        ..Comparison::default()
    };
    for (index, (left, right)) in rust.iter().zip(c).enumerate() {
        match (left, right) {
            (Verdict::Ok { .. }, Verdict::Ok { .. }) => {
                comparison.accepted += 1;
                if left != right {
                    comparison.disagreements.push(Disagreement {
                        index,
                        bytes: cases[index].clone(),
                        rust: left.clone(),
                        c: right.clone(),
                    });
                }
            }
            (Verdict::Err, Verdict::Err) => comparison.rejected += 1,
            _ => comparison.disagreements.push(Disagreement {
                index,
                bytes: cases[index].clone(),
                rust: left.clone(),
                c: right.clone(),
            }),
        }
    }
    comparison
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// 确定性变异语料（同一种子必然给出同一批用例）。
pub fn mutants(seed: u64, base: &[u8], count: usize) -> Vec<Vec<u8>> {
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut bytes = base.to_vec();
        let operator = rng.below(7);
        match operator {
            // 翻 1..3 个位
            0 => {
                for _ in 0..=rng.below(3) {
                    let index = rng.below(bytes.len());
                    bytes[index] ^= 1u8 << rng.below(8);
                }
            }
            // 改成非法字节 243..=255
            1 => {
                let index = rng.below(bytes.len());
                bytes[index] = 243 + rng.below(13) as u8;
            }
            // 整字节随机
            2 => {
                let index = rng.below(bytes.len());
                bytes[index] = rng.below(256) as u8;
            }
            // 截断（保留头，至少 0 字节）
            3 => {
                let keep = 4 + rng.below(bytes.len() - 4 + 1);
                bytes.truncate(keep);
            }
            // 加长
            4 => {
                for _ in 0..=rng.below(4) {
                    bytes.push(rng.below(256) as u8);
                }
            }
            // 改头字段：版本/语义/nvars/长度
            5 => {
                let index = 4 + rng.below(3);
                bytes[index] = rng.below(256) as u8;
            }
            // 改长度字段（u32 LE）
            _ => {
                let length = rng.below(4096) as u32;
                bytes[7..11].copy_from_slice(&length.to_le_bytes());
            }
        }
        out.push(bytes);
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-redteam-{name}-{}-{}",
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

fn compiler() -> Option<String> {
    let candidates = [
        std::env::var("CC").unwrap_or_default(),
        "cc".to_string(),
        "gcc".to_string(),
        "clang".to_string(),
    ];
    candidates
        .into_iter()
        .filter(|name| !name.is_empty())
        .find(|name| {
            Command::new(name)
                .arg("--version")
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        })
}

fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../trit-build/runtime")
}

fn build_harness(cc: &str, dir: &Path) -> Result<PathBuf> {
    let runtime = runtime_dir();
    if !runtime.join("trit_runtime.c").exists() {
        bail!("找不到 C 运行时：{}", runtime.display());
    }
    let source = dir.join("harness.c");
    fs::write(&source, HARNESS).context("写 harness.c")?;
    let exe = dir.join(format!("harness{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(cc)
        .arg("-std=c99")
        .arg("-O1")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-o")
        .arg(&exe)
        .arg(&source)
        .arg(runtime.join("trit_runtime.c"))
        .arg("-I")
        .arg(&runtime)
        .output()
        .context("运行 C 编译器")?;
    if !output.status.success() {
        bail!(
            "编译 C harness 失败：\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(exe)
}

fn run_c(harness: &Path, cases: &[Vec<u8>]) -> Result<Vec<Verdict>> {
    let dir = harness.parent().expect("harness 有父目录");
    let cases_path = dir.join("cases.bin");
    let mut buffer = Vec::new();
    for case in cases {
        buffer.extend_from_slice(&(case.len() as u32).to_le_bytes());
        buffer.extend_from_slice(case);
    }
    fs::write(&cases_path, &buffer).context("写用例文件")?;
    let output = Command::new(harness)
        .arg(&cases_path)
        .output()
        .context("运行 C harness")?;
    if !output.status.success() {
        bail!(
            "C harness 退出 {:?}：{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut verdicts = Vec::with_capacity(cases.len());
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["ok", count, hash] => verdicts.push(Verdict::Ok {
                count: count.parse().context("解析 count")?,
                hash: u64::from_str_radix(hash, 16).context("解析 hash")?,
            }),
            ["err", _code] => verdicts.push(Verdict::Err),
            other => bail!("无法解析 C 输出 {other:?}"),
        }
    }
    if verdicts.len() != cases.len() {
        bail!("C 侧输出 {} 行，用例 {} 个", verdicts.len(), cases.len());
    }
    Ok(verdicts)
}

/// 跑差分 fuzz；返回（报告，是否有分歧）。
pub fn run(blob_bytes: &[u8], iterations: usize, seed: u64) -> Result<(Value, bool)> {
    let Some(cc) = compiler() else {
        return Ok((
            json!({
                "tool": "trit-redteam",
                "check": "trb2-differential",
                "skipped": true,
                "reason": "本机没有 C 编译器（cc/gcc/clang）；CI 的 ubuntu-latest 有",
            }),
            false,
        ));
    };
    let cases = mutants(seed, blob_bytes, iterations);
    let rust: Vec<Verdict> = cases.iter().map(|case| rust_verdict(case)).collect();
    let dir = scratch("fuzz");
    let harness = build_harness(&cc, &dir)?;
    let c = run_c(&harness, &cases)?;
    let comparison = compare(&rust, &c, &cases);
    let examples: Vec<Value> = comparison
        .disagreements
        .iter()
        .take(5)
        .map(|disagreement| {
            json!({
                "index": disagreement.index,
                "hex": disagreement.bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "rust": disagreement.rust.to_json(),
                "c": disagreement.c.to_json(),
            })
        })
        .collect();
    let report = json!({
        "tool": "trit-redteam",
        "check": "trb2-differential",
        "seed": seed,
        "iterations": iterations,
        "cases": comparison.cases,
        "accepted": comparison.accepted,
        "rejected": comparison.rejected,
        "disagreements": comparison.disagreements.len(),
        "examples": examples,
        "note": "Rust 解析器与 C 运行时必须对每个畸形 blob 给出同一结论（接受/拒绝 + 逐 trit 相同）",
    });
    let _ = fs::remove_dir_all(&dir);
    Ok((report, !comparison.disagreements.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "5452423202000209000000514004";

    fn fixture() -> Vec<u8> {
        (0..FIXTURE.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&FIXTURE[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn mutation_corpus_is_deterministic() {
        let base = fixture();
        let first = mutants(7, &base, 64);
        let again = mutants(7, &base, 64);
        assert_eq!(first, again, "同一种子必须给出同一批用例");
        let other = mutants(8, &base, 64);
        assert_ne!(first, other, "换种子必须换语料");
        assert!(first.iter().any(|case| case != &base), "语料里必须有变异");
    }

    #[test]
    fn comparator_reports_both_kinds_of_disagreement() {
        let cases = vec![vec![1u8], vec![2u8], vec![3u8], vec![4u8]];
        let rust = vec![
            Verdict::Ok { count: 3, hash: 9 },
            Verdict::Err,
            Verdict::Ok { count: 3, hash: 9 },
            Verdict::Ok { count: 3, hash: 9 },
        ];
        let c = vec![
            Verdict::Ok { count: 3, hash: 9 },  // 一致
            Verdict::Err,                       // 一致
            Verdict::Err,                       // Rust 接受、C 拒绝
            Verdict::Ok { count: 3, hash: 10 }, // 都接受但值不同
        ];
        let comparison = compare(&rust, &c, &cases);
        assert_eq!(comparison.cases, 4);
        assert_eq!(comparison.accepted, 2);
        assert_eq!(comparison.rejected, 1);
        assert_eq!(comparison.disagreements.len(), 2);
        assert_eq!(comparison.disagreements[0].index, 2);
        assert_eq!(comparison.disagreements[1].index, 3);
    }

    #[test]
    fn fuzz_finds_no_disagreement_on_a_real_blob() {
        if compiler().is_none() {
            eprintln!("跳过：本机没有 C 编译器");
            return;
        }
        let (report, finding) = run(&fixture(), 200, 1).unwrap();
        assert!(!finding, "出现了分歧：{report}");
        assert_eq!(report["disagreements"], 0);
        assert!(report["cases"].as_u64().unwrap() == 200);
        assert!(
            report["rejected"].as_u64().unwrap() > 0,
            "变异语料里必须真被拒绝的用例：{report}"
        );
    }
}
