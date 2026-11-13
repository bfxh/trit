//! 分组嵌套闸：正则解析器与 Thompson 展开都是递归下降，`((((…` 叠上几万
//! 层会爆栈（进程直接 abort——实测串区与条件里的 `matches` 都能触发）。
//! 进解析器之前先数一遍括号，过深的如实 `Err`（串记名弃权 / 规则
//! `Unsupported`）。
//!
//! 只数字节不解析：`\` 后的一个字节跳过，其余 `(` 一律算开、`)` 算闭。
//! 字符类里的括号（`[(]`）因此也被计入——只会多拒，不会漏放。

/// 分组嵌套上限（官方没有这道闸；真实规则用不到两位数的嵌套）。
pub(super) const MAX_GROUP_DEPTH: usize = 64;

pub(super) fn check(body: &str) -> Result<(), String> {
    let mut depth = 0usize;
    let mut bytes = body.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next();
            }
            b'(' => {
                depth += 1;
                if depth > MAX_GROUP_DEPTH {
                    return Err(format!("分组嵌套过深（上限 {MAX_GROUP_DEPTH} 层）"));
                }
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(levels: usize) -> String {
        format!("{}a{}", "(".repeat(levels), ")".repeat(levels))
    }

    #[test]
    fn accepts_ordinary_nesting_and_rejects_runaway_nesting() {
        assert!(check("").is_ok());
        assert!(check("a(b|c)*d").is_ok());
        assert!(check(&nested(MAX_GROUP_DEPTH)).is_ok());
        assert!(check(&nested(MAX_GROUP_DEPTH + 1)).is_err());
        // 并排的分组不叠深度
        assert!(check(&"(a)".repeat(10_000)).is_ok());
        // 转义的括号不是分组
        assert!(check(&r"\(".repeat(10_000)).is_ok());
        // 多余的 `)` 不会把计数压成负数再放水
        let sneaky = format!("{}{}", ")".repeat(1000), nested(MAX_GROUP_DEPTH + 1));
        assert!(check(&sneaky).is_err());
    }

    #[test]
    fn runaway_nesting_no_longer_reaches_the_recursive_parser() {
        // 守卫上线前这一行会让测试进程爆栈
        let err = super::super::compile(&nested(100_000), false).expect_err("应被拒绝");
        assert!(err.contains("嵌套过深"), "{err}");
        assert!(super::super::compile(&nested(MAX_GROUP_DEPTH), false).is_ok());
    }
}
