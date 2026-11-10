//! `--emit-gpu` 的产物布局（无 GPU 也能跑）：位平面必须逐行还原 truth.csv。
//! 真机 WGSL 对拍在 `src/gpu.rs` 的 `gpu` feature 单测里。

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
        "trit-build-gpu-{}-{}-{}",
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

fn verdicts(csv: &str) -> Vec<i8> {
    csv.lines()
        .skip(1)
        .map(|line| line.rsplit(',').next().unwrap().trim().parse().unwrap())
        .collect()
}

#[test]
fn gpu_export_is_self_describing_and_matches_truth_csv() {
    for algebra in ["kleene", "lukasiewicz", "bochvar"] {
        let dir = scratch("layout");
        let source = dir.join("e.tri");
        fs::write(&source, "(a & b) | (c => !a)\n").unwrap();
        let out = dir.join("out");
        let output = Command::new(bin())
            .args([
                source.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--emit-gpu",
                "--algebra",
                algebra,
            ])
            .output()
            .expect("运行 trit-build");
        assert!(
            output.status.success(),
            "{algebra}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
        assert_eq!(manifest["options"]["emitGpu"], true);
        assert_eq!(manifest["verified"]["gpu"]["workgroupSize"], 64);

        let wgsl = fs::read_to_string(out.join("truth.wgsl")).unwrap();
        assert!(wgsl.contains("@compute @workgroup_size(64)"), "{wgsl}");
        assert!(wgsl.contains("var<storage, read> table_pos"), "{wgsl}");

        let bin = fs::read(out.join("truth.gpu.bin")).unwrap();
        assert_eq!(&bin[..4], b"TGPU");
        assert_eq!(bin[4], 1, "TGPU 版本");
        let nvars = usize::from(bin[6]);
        let len = u32::from_le_bytes([bin[8], bin[9], bin[10], bin[11]]) as usize;
        let words = u32::from_le_bytes([bin[12], bin[13], bin[14], bin[15]]) as usize;
        assert_eq!(nvars, 3);
        assert_eq!(len, 27);
        assert_eq!(words, 1);
        assert_eq!(bin.len(), 16 + words * 8);

        let pos = u32::from_le_bytes([bin[16], bin[17], bin[18], bin[19]]);
        let neg = u32::from_le_bytes([bin[20], bin[21], bin[22], bin[23]]);
        let csv = fs::read_to_string(out.join("truth.csv")).unwrap();
        let expected = verdicts(&csv);
        assert_eq!(expected.len(), len);
        for (code, want) in expected.iter().enumerate() {
            let got = if pos & (1 << code) != 0 {
                1
            } else if neg & (1 << code) != 0 {
                -1
            } else {
                0
            };
            assert_eq!(got, *want, "{algebra} code={code}");
        }

        let meta: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out.join("truth_gpu.json")).unwrap()).unwrap();
        assert_eq!(meta["format"], "trit-gpu/1");
        assert_eq!(meta["algebra"], algebra);
        assert_eq!(meta["length"], 27);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn gpu_export_also_works_for_verilog_input() {
    let dir = scratch("verilog");
    let source = dir.join("d.v");
    fs::write(
        &source,
        "module d(input wire a, input wire b, output wire y);\n  assign y = a & b;\nendmodule\n",
    )
    .unwrap();
    let out = dir.join("out");
    let result = Command::new(bin())
        .args([
            source.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--emit-gpu",
        ])
        .output()
        .expect("运行 trit-build");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(out.join("truth.gpu.bin").exists());
    assert!(out.join("truth.wgsl").exists());
    let _ = fs::remove_dir_all(&dir);
}
