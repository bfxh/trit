//! 原始参数扫描：取值与布尔开关。

pub fn value(argv: &[String], key: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == key)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}

/// 是否存在布尔开关（如 `--psnr`，不带值）。
pub fn flag(argv: &[String], key: &str) -> bool {
    argv.iter().any(|a| a == key)
}
