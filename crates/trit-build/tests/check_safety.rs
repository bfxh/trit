//! issue #772 回归：缓存清单的产物名不得逃出 `out_dir`。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn trit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trit-build"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-check-{}-{}-{}",
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
    let out = Command::new(trit_bin())
        .args(args)
        .output()
        .expect("运行 trit-build");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn cache_manifest(out: &Path) -> PathBuf {
    let dir = out.join(".trit-cache");
    fs::read_dir(&dir)
        .expect("读 .trit-cache")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap_or_else(|| panic!("{} 里没有清单", dir.display()))
}

fn fnv1a(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn add_output(manifest_path: &Path, name: &str, bytes: &[u8]) {
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).expect("读清单"))
            .expect("解析清单");
    let outputs = manifest
        .get_mut("outputs")
        .and_then(Value::as_object_mut)
        .expect("清单 outputs");
    outputs.insert(
        name.to_string(),
        json!({"bytes": bytes.len(), "hash": fnv1a(bytes)}),
    );
    fs::write(manifest_path, manifest.to_string()).expect("写清单");
}

#[test]
fn check_rejects_manifest_name_escape() {
    let dir = scratch("escape-check");
    let source = dir.join("demo.tri");
    fs::write(&source, "a ^ b\n").expect("写源文件");
    let out = dir.join("out");
    let (_, stderr, code) = run(&[source.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");

    let secret = dir.join("secret.txt");
    let secret_body = b"outside out_dir 772";
    fs::write(&secret, secret_body).expect("写 out_dir 外文件");
    let manifest_path = cache_manifest(&out);
    add_output(&manifest_path, "../secret.txt", secret_body);

    let (stdout, stderr, code) = run(&[
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--check",
    ]);
    assert_ne!(code, 0, "--check 必须拒绝路径穿越: {stdout}");
    assert!(stdout.contains("\"unsafe\":true"), "{stdout}");
    assert!(
        !stdout.contains(&fnv1a(secret_body)),
        "不得输出 out_dir 外文件的哈希: {stdout}"
    );
    assert!(stderr.contains("产物校验失败"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cache_ignores_manifest_name_escape() {
    let dir = scratch("escape-cache");
    let source = dir.join("demo.tri");
    fs::write(&source, "a & b\n").expect("写源文件");
    let out = dir.join("out");
    let src = source.to_str().unwrap();
    let outdir = out.to_str().unwrap();

    let (_, stderr, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0, "{stderr}");
    let (stdout, _, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0);
    assert!(stdout.contains("\"cached\":true"), "{stdout}");

    let secret = dir.join("secret.txt");
    let secret_body = b"outside out_dir 772";
    fs::write(&secret, secret_body).expect("写 out_dir 外文件");
    let manifest_path = cache_manifest(&out);
    add_output(&manifest_path, "../secret.txt", secret_body);

    let (stdout, _, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("\"cached\":false"),
        "含路径穿越产物名的清单不得命中缓存: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
