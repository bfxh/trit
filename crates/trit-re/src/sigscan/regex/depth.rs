//! 正则嵌套深度预检（第 37 轮 S2：递归下降与 `emit` 都没有自己的深度计数）。

/// 允许的最大嵌套层数。
const MAX_DEPTH: usize = 64;

/// 按括号配对扫一遍，超限直接 `Err`——8000 层括号不再让 release 栈溢出（134）。
pub(super) fn check(body: &str) -> Result<(), String> {
    let mut depth = 0usize;
    for b in body.bytes() {
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            _ => continue,
        }
        if depth > MAX_DEPTH {
            return Err(format!("正则嵌套超过 {MAX_DEPTH} 层"));
        }
    }
    Ok(())
}
