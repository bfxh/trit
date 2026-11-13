//! CLI `task-merge` 的 TIES trim 参数（从 `cli_task.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::task_merge::{TrimOptions, TrimTop};

use super::super::Parsed;

/// 解析后的 trim 参数。
pub(super) struct TrimChoice {
    /// 直接交给合并函数的选项。
    pub(super) options: TrimOptions,
    top: Option<TrimTop>,
    margin: f64,
}

impl TrimChoice {
    /// 解析 `--trim` / `--trim-margin`；与 DARE 同时使用时报错。
    pub(super) fn parse(parsed: &Parsed, with_dare: bool) -> Result<Self, String> {
        let margin = parsed
            .values
            .get("trim-margin")
            .map(|raw| raw.parse::<f64>())
            .transpose()
            .map_err(|_| "--trim-margin 需要数字".to_string())?
            .unwrap_or(0.0);
        if !margin.is_finite() || margin < 0.0 {
            return Err("--trim-margin 需要有限非负数".into());
        }
        let top = match parsed.values.get("trim") {
            None => None,
            Some(raw) => {
                let value: f32 = raw
                    .parse()
                    .map_err(|_| format!("--trim 需要正整数票数或 (0,1) 比例，得到 `{raw}`"))?;
                if !value.is_finite() || value <= 0.0 {
                    return Err("--trim 需要正整数票数或 (0,1) 比例".into());
                }
                if value >= 1.0 {
                    if value.fract() != 0.0 {
                        return Err("--trim 的票数必须是整数（≥1）".into());
                    }
                    Some(TrimTop::Count(value as usize))
                } else {
                    Some(TrimTop::Fraction(value))
                }
            }
        };
        let options = TrimOptions { top, margin };
        if !options.is_identity() && with_dare {
            return Err(
                "--trim/--trim-margin 与 DARE 不能同时给（两条路径各自有自己的 dropped 语义）"
                    .into(),
            );
        }
        Ok(Self {
            options,
            top,
            margin,
        })
    }

    /// 报告形状：`{"top": {kind,value} | null, "margin": m}`。
    pub(super) fn to_json(&self) -> Value {
        let top = self.top.map(|top| match top {
            TrimTop::Count(count) => json!({"kind": "count", "value": count}),
            TrimTop::Fraction(ratio) => json!({"kind": "fraction", "value": format!("{ratio}")}),
        });
        json!({"top": top, "margin": self.margin})
    }
}
