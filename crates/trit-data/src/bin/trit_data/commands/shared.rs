#[path = "input.rs"]
pub mod input;
pub(crate) use super::usage::usage;
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

pub(crate) fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
