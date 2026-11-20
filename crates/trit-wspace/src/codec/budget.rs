//! WSP1 文本维度预算（从 `codec.rs` 拆出，守单文件行数预算）。
//!
//! WSP1 每个 trit 至少占一个字符，因此 `arch` 声明的 `sum(rows*cols)` 不可能
//! 超过整份文本的字节数。没有这道检查，一行 `arch usize::MAX 1` 就会在读到
//! 任何 row 行之前让 `Vec::with_capacity` 容量溢出。

use crate::error::{Error, Result};

pub(super) fn check(sizes: &[usize], line: usize, text_len: usize) -> Result<()> {
    let mut declared = 0usize;
    for pair in sizes.windows(2) {
        let elems = pair[0].checked_mul(pair[1]).ok_or_else(|| Error::Parse {
            line,
            msg: "arch 声明的权重数溢出 usize".into(),
        })?;
        declared = declared.checked_add(elems).ok_or_else(|| Error::Parse {
            line,
            msg: "arch 声明的权重总数溢出 usize".into(),
        })?;
    }
    if declared > text_len {
        return Err(Error::Parse {
            line,
            msg: format!(
                "arch 声明的权重总数 {declared} 超过文本长度 {text_len}（每个 trit 至少占一个字符）"
            ),
        });
    }
    Ok(())
}

pub(super) fn parse_sizes(tokens: &[&str], line: usize) -> Result<Vec<usize>> {
    let mut sizes = Vec::with_capacity(tokens.len());
    for token in tokens {
        let size = token.parse::<usize>().map_err(|_| Error::Parse {
            line,
            msg: format!("arch 维度 `{token}` 不是 usize"),
        })?;
        if size == 0 {
            return Err(Error::Parse {
                line,
                msg: "arch 维度不能为 0".into(),
            });
        }
        sizes.push(size);
    }
    if sizes.len() < 2 {
        return Err(Error::Parse {
            line,
            msg: "arch 至少需要输入维与输出维".into(),
        });
    }
    Ok(sizes)
}
