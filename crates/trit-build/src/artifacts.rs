//! 产物层：`expr.txt` / `truth.csv` / `truth.tritbin` / `lookup.c` / `direct.c` / `design.v`。
//!
//! 从 `main.rs` 拆出来：主流程只负责“算”，这里只负责“写”。拆的理由是
//! `main.rs` 已经顶到 783 行的模块预算，加一个后端就得先腾地方 —— 按仓库规矩
//! 「要腾地方就拆，别抬台账」。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use trit_circuit::Circuit;
use trit_core::Algebra;
use trit_core::value::Ternary;

use crate::direct;

mod lookup;
pub(crate) use lookup::lookup_c;

/// 产物后端：`table` 只出 `lookup.c`，`direct` 只出 `direct.c`，`both` 两份都出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    Table,
    Direct,
    Both,
}

/// `direct.c` 的两条 lowering 入口：`.tri` 表达式，或 Verilog 组合网表。
pub(crate) enum DirectSource<'a> {
    Expr(&'a trit_core::Expr),
    Netlist {
        circuit: &'a Circuit,
        output: &'a str,
    },
}

impl Backend {
    pub(crate) fn parse(name: &str) -> Result<Self> {
        match name {
            "table" => Ok(Self::Table),
            "direct" => Ok(Self::Direct),
            "both" => Ok(Self::Both),
            other => bail!("未知的产物后端 {other:?}，可选：table / direct / both"),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Direct => "direct",
            Self::Both => "both",
        }
    }

    pub(crate) const fn emits_table(self) -> bool {
        matches!(self, Self::Table | Self::Both)
    }

    pub(crate) const fn emits_direct(self) -> bool {
        matches!(self, Self::Direct | Self::Both)
    }
}

fn trit_i8(value: Ternary) -> i8 {
    match value {
        Ternary::True => 1,
        Ternary::Unknown => 0,
        Ternary::False => -1,
    }
}

pub(crate) fn truth_csv(vars: &[String], values: &[Ternary]) -> String {
    let mut text = String::new();
    text.push_str("code,");
    for var in vars {
        text.push_str(var);
        text.push(',');
    }
    text.push_str("verdict\n");
    for (code, value) in values.iter().enumerate() {
        text.push_str(&code.to_string());
        text.push(',');
        let mut rest = code;
        for _ in vars {
            let digit = rest % 3;
            rest /= 3;
            text.push_str(match digit {
                0 => "-1",
                1 => "0",
                _ => "1",
            });
            text.push(',');
        }
        text.push_str(match value {
            Ternary::False => "-1",
            Ternary::Unknown => "0",
            _ => "1",
        });
        text.push('\n');
    }
    text
}

/// `TRB2`：基 3 打包（5 trit/字节）+ GF(3) CRC-3 + 变量名表，见 `trb`。
fn tritbin(values: &[Ternary], alg: Algebra, names: &[String]) -> Result<Vec<u8>> {
    crate::trb::encode_named(values, alg, names)
}

pub(crate) fn write_artifact(path: &Path, bytes: &[u8]) -> Result<String> {
    fs::write(path, bytes).with_context(|| format!("写 {} 失败", path.display()))?;
    Ok(crate::hex(crate::hash64(bytes)))
}

/// 写产物（两条链路共用）。
///
/// `direct_source` 二者之一：`.tri` 给表达式（重写后的形态），
/// Verilog 给网表 + 选中输出；`direct` 后端需要它。
pub(crate) struct EmitRequest<'a> {
    pub out_dir: &'a Path,
    pub header: &'a str,
    pub vars: &'a [String],
    pub values: &'a [Ternary],
    pub algebra: Algebra,
    pub rail: Option<&'a str>,
    pub direct_source: Option<DirectSource<'a>>,
    pub backend: Backend,
}

pub(crate) fn emit_artifacts(request: EmitRequest<'_>) -> Result<serde_json::Map<String, Value>> {
    let EmitRequest {
        out_dir,
        header,
        vars,
        values,
        algebra,
        rail,
        direct_source,
        backend,
    } = request;
    let csv = truth_csv(vars, values);
    let blob = tritbin(values, algebra, vars)?;
    let mut artifacts = vec![
        ("expr.txt", header.as_bytes().to_vec()),
        ("truth.csv", csv.as_bytes().to_vec()),
        ("truth.tritbin", blob),
    ];
    if backend.emits_table() {
        artifacts.push(("lookup.c", lookup_c(vars, values, algebra).into_bytes()));
    }
    if backend.emits_direct() {
        let source = direct_source
            .context("--backend direct/both 需要可降级的表达式或网表（当前构建没有）")?;
        let text = match source {
            DirectSource::Expr(expr) => direct::emit(expr, vars, algebra)?,
            DirectSource::Netlist { circuit, output } => {
                direct::emit_netlist(circuit, output, vars, algebra)?
            }
        };
        artifacts.push(("direct.c", text.into_bytes()));
    }
    if let Some(module) = rail {
        artifacts.push(("design.v", module.as_bytes().to_vec()));
    }
    let mut outputs = serde_json::Map::new();
    for (name, bytes) in artifacts {
        let hash = write_artifact(&out_dir.join(name), &bytes)?;
        outputs.insert(
            name.to_string(),
            json!({"bytes": bytes.len(), "hash": hash}),
        );
    }
    Ok(outputs)
}
