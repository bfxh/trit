//! `trit build` 集成测试：产物、增量缓存、产物校验。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn trit_bin() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-build{}", std::env::consts::EXE_SUFFIX))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trit-build-{}-{}-{}",
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
        .expect("运行 trit");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn build_emits_artifacts_and_is_incremental() {
    let dir = scratch("basic");
    let source = dir.join("demo.tri");
    fs::write(&source, "# demo\n(a & b) | !c\n").expect("写源文件");
    let out = dir.join("out");
    let src = source.to_str().unwrap();
    let outdir = out.to_str().unwrap();

    let (stdout, stderr, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0, "构建失败: {stderr}");
    assert!(stdout.contains("\"cached\":false"), "{stdout}");
    for name in [
        "expr.txt",
        "truth.csv",
        "truth.tritbin",
        "lookup.c",
        "build.json",
    ] {
        assert!(out.join(name).exists(), "缺产物 {name}");
    }
    let csv = fs::read_to_string(out.join("truth.csv")).unwrap();
    // 3 变量 → 27 行 + 表头。
    assert_eq!(csv.lines().count(), 28, "{csv}");
    // TRB2：27 + 3（CRC-3）trit → 5 trit/字节 = 6 字节，头 11 字节。
    let blob = fs::read(out.join("truth.tritbin")).unwrap();
    assert_eq!(&blob[..4], b"TRB2");
    assert_eq!(blob.len(), 11 + 6, "blob 大小 {}", blob.len());

    // 第二次：命中缓存，不重写产物。
    let (stdout2, _, code2) = run(&[src, "--out", outdir]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("\"cached\":true"), "{stdout2}");

    // 改源文件 ⇒ 换 key ⇒ 重新构建。
    fs::write(&source, "# demo\na & b\n").expect("改写源文件");
    let (stdout3, _, code3) = run(&[src, "--out", outdir]);
    assert_eq!(code3, 0);
    assert!(stdout3.contains("\"cached\":false"), "{stdout3}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_detects_tampered_artifact() {
    let dir = scratch("tamper");
    let source = dir.join("demo.tri");
    fs::write(&source, "a ^ b\n").expect("写源文件");
    let out = dir.join("out");
    let src = source.to_str().unwrap();
    let outdir = out.to_str().unwrap();
    let (_, stderr, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0, "{stderr}");

    let (stdout, _, code) = run(&[src, "--out", outdir, "--check"]);
    assert_eq!(code, 0, "{stdout}");

    fs::write(out.join("truth.tritbin"), b"tampered").expect("篡改");
    let (stdout, stderr, code) = run(&[src, "--out", outdir, "--check"]);
    assert_ne!(code, 0, "篡改后 --check 必须失败");
    assert!(stdout.contains("\"ok\":false"), "{stdout}");
    assert!(stderr.contains("产物校验失败"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_multi_line_source_and_list_limits() {
    let dir = scratch("bad");
    let source = dir.join("bad.tri");
    fs::write(&source, "a & b\nc | d\n").expect("写源文件");
    let outdir = dir.join("out");
    let (_, stderr, code) = run(&[source.to_str().unwrap(), "--out", outdir.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只能有一条表达式"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cache_miss_when_artifact_was_tampered() {
    let dir = scratch("cache-tamper");
    let source = dir.join("demo.tri");
    fs::write(
        &source,
        "a & b & c
",
    )
    .expect("写源文件");
    let out = dir.join("out");
    let src = source.to_str().unwrap();
    let outdir = out.to_str().unwrap();
    let (_, stderr, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0, "{stderr}");
    fs::write(
        out.join("lookup.c"),
        "/* tampered */
",
    )
    .expect("篡改");
    let (stdout, _, code) = run(&[src, "--out", outdir]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("\"cached\":false"),
        "篡改后必须重建: {stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
