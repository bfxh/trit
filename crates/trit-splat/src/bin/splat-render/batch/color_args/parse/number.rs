//! 数字参数解析。

use crate::cli;

pub(crate) fn number(argv: &[String], flag: &str) -> Result<Option<f32>, String> {
    cli::value(argv, flag)
        .map(|v| {
            v.parse::<f32>()
                .map_err(|_| format!("{flag} 非法数字: {v}"))
        })
        .transpose()
}
