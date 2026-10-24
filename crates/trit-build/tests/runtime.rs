//! 主机侧运行时（`runtime/trit_runtime.c`）的独立验证。
//!
//! 这里**用 C 重算** 5-trit/字节解包与 GF(3) CRC-3：Rust 编码器与 C 解码器只要有一边
//! 理解错了格式，逐 trit 比对就会红。Rust 自己编码再自己解码只能说"自洽"，证明不了格式。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HARNESS: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include "trit_runtime.h"

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <blob>\n", argv[0]);
        return 2;
    }
    FILE *file = fopen(argv[1], "rb");
    if (file == NULL) {
        fprintf(stderr, "cannot open %s\n", argv[1]);
        return 2;
    }
    if (fseek(file, 0, SEEK_END) != 0) { fclose(file); return 2; }
    long size = ftell(file);
    if (size < 0 || fseek(file, 0, SEEK_SET) != 0) { fclose(file); return 2; }
    unsigned char *blob = malloc((size_t)size);
    if (blob == NULL) { fclose(file); return 2; }
    if (fread(blob, 1, (size_t)size, file) != (size_t)size) {
        free(blob); fclose(file); return 2;
    }
    fclose(file);

    trit_table table;
    int status = trit_trb2_decode(blob, (size_t)size, &table);
    free(blob);
    if (status != TRIT_OK) {
        printf("error %d\n", status);
        return 1;
    }
    printf("table %u %u %u\n", table.count, table.nvars, table.algebra);
    printf("trits");
    for (uint32_t i = 0; i < table.count; i++) {
        printf(" %d", table.trits[i]);
    }
    printf("\n");
    trit_table_free(&table);
    return 0;
}
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-runtime-{}-{}-{}",
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

fn bin() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-build{}", std::env::consts::EXE_SUFFIX))
}

/// 找一个能用的 C 编译器；没有就返回 None（CI 的 ubuntu 一定有 cc）。
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

fn verdicts(csv: &str) -> Vec<i8> {
    csv.lines()
        .skip(1)
        .map(|line| line.rsplit(',').next().unwrap().trim().parse().unwrap())
        .collect()
}

fn compile(cc: &str, dir: &Path) -> PathBuf {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let harness_c = dir.join("harness.c");
    fs::write(&harness_c, HARNESS).expect("写 harness.c");
    let exe = dir.join(format!("harness{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(cc)
        .arg("-std=c99")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-o")
        .arg(&exe)
        .arg(&harness_c)
        .arg(runtime.join("trit_runtime.c"))
        .arg("-I")
        .arg(&runtime)
        .output()
        .expect("运行 C 编译器");
    assert!(
        out.status.success(),
        "编译失败：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    exe
}

#[test]
fn c_runtime_decodes_the_blob_runtime_wrote() {
    let Some(cc) = compiler() else {
        eprintln!("跳过：本机没有 C 编译器（CI 的 ubuntu-latest 有 cc）");
        return;
    };
    let dir = scratch("decode");
    let source = dir.join("e.tri");
    fs::write(&source, "(a & b) | !c\n").expect("写源文件");
    let out_dir = dir.join("out");
    let built = Command::new(bin())
        .arg(&source)
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("运行 trit-build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let harness = compile(&cc, &dir);
    let blob = out_dir.join("truth.tritbin");
    let run = Command::new(&harness)
        .arg(&blob)
        .output()
        .expect("运行 harness");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "C 运行时解码失败：{stdout}");

    let mut lines = stdout.lines();
    let header = lines.next().unwrap_or_default();
    assert_eq!(header, "table 27 3 0", "{stdout}");
    let trits: Vec<i8> = lines
        .next()
        .unwrap_or_default()
        .trim_start_matches("trits")
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    let csv = fs::read_to_string(out_dir.join("truth.csv")).unwrap();
    assert_eq!(trits, verdicts(&csv), "C 解码与 truth.csv 逐 trit 必须相同");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c_runtime_rejects_tampered_blob() {
    let Some(cc) = compiler() else {
        eprintln!("跳过：本机没有 C 编译器（CI 的 ubuntu-latest 有 cc）");
        return;
    };
    let dir = scratch("tamper");
    let source = dir.join("e.tri");
    fs::write(&source, "a ^ b\n").expect("写源文件");
    let out_dir = dir.join("out");
    let built = Command::new(bin())
        .arg(&source)
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("运行 trit-build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let blob_path = out_dir.join("truth.tritbin");
    let mut bytes = fs::read(&blob_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0b101;
    fs::write(&blob_path, &bytes).unwrap();

    let harness = compile(&cc, &dir);
    let run = Command::new(&harness)
        .arg(&blob_path)
        .output()
        .expect("运行 harness");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(!run.status.success(), "篡改后必须失败：{stdout}");
    assert!(stdout.starts_with("error "), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}
