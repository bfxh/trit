#[path = "input.rs"]
pub mod input;
use std::path::Path;

pub(crate) fn write_lines(path: &Path, lines: &[String]) {
    let body: String = lines
        .iter()
        .map(|l| {
            format!(
                "{l}
"
            )
        })
        .collect();
    std::fs::write(path, body).unwrap_or_else(|e| die(&format!("写 {}: {e}", path.display())));
}

pub(crate) fn created_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn usage() -> ! {
    die("用法: trit-data gate --jsonl <轨迹.jsonl> [--out <目录>] [--allow-unverified] [--lang-quality off|defer|strict]
      trit-data rewrite --jsonl <defer.jsonl> --cmd <重写器命令> [--timeout 30] [--out <目录>] [gate 参数…]
      trit-data compare --a <a.jsonl> --b <b.jsonl>；decontaminate --jsonl <in> --bench <bench.jsonl> [--ngram 13] [--threshold 0.2] [--out <目录>]；mix --jsonl <in> [--total 1000] [--weights code=0.7,writing=0.3] [--out <目录>]")
}

pub(crate) fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
