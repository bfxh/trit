//! 线格式：补丁、验证集、报告的 JSON 表示（CLI 与 MCP 共用）。
//!
//! 补丁格式（稳定，version=1）：
//!
//! ```json
//! {"version": 1, "edits": [
//!   {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "+"},
//!   {"op": "set_bias", "layer": 0, "row": 1, "to": "-"},
//!   {"op": "set_scale", "layer": 0, "row": 1, "to": 2.0},
//!   {"op": "swap_rows", "layer": 0, "a": 0, "b": 1},
//!   {"op": "negate_row", "layer": 0, "row": 1}
//! ]}
//! ```
//!
//! 验证集两种写法：
//!
//! - 显式：`{"inputs": [["-","+"], ...], "outputs": [["+"], ...]}`；
//! - 紧凑：`{"domain":"boolean","vars":2,"table":"0110"}`（字典序，`0/1/x`），
//!   或 `{"domain":"ternary","vars":2,"table":"-0+..."}`（字典序，`-0+`）。

use serde_json::{Value, json};

use crate::analyze::{
    Analysis, EditImpact, PairAnalysis, PairImpact, TripleAnalysis, TripleImpact,
};
use crate::codec::checksum;
use crate::edit::{Cost, Edit, Patch};
use crate::error::{Error, Result};
use crate::explain::{SampleEditImpact, SampleExplanation};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::probe::{ProbeReport, RowEffect, RowImpact};
use crate::program::Report;
use crate::repair::Repair;
use crate::trit::Trit;

/// 补丁线格式版本。
pub const PATCH_VERSION: u64 = 1;

/// 补丁 → JSON。
#[must_use]
pub fn patch_to_value(patch: &Patch) -> Value {
    let edits: Vec<Value> = patch.edits().iter().map(edit_to_value).collect();
    json!({"version": PATCH_VERSION, "edits": edits})
}

/// 单条编辑 → JSON。
#[must_use]
pub fn edit_to_value(edit: &Edit) -> Value {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => json!({
            "op": "set_trit", "layer": layer, "row": row, "col": col,
            "to": to.to_char().to_string(),
        }),
        Edit::SetBias { layer, row, to } => json!({
            "op": "set_bias", "layer": layer, "row": row,
            "to": to.to_char().to_string(),
        }),
        Edit::SetScale { layer, row, to } => json!({
            "op": "set_scale", "layer": layer, "row": row, "to": to,
        }),
        Edit::SwapRows { layer, a, b } => json!({
            "op": "swap_rows", "layer": layer, "a": a, "b": b,
        }),
        Edit::NegateRow { layer, row } => json!({
            "op": "negate_row", "layer": layer, "row": row,
        }),
    }
}

/// JSON → 补丁。缺 `version` 时按 1 处理；未知 op / 缺字段 / 非法三值都报错。
pub fn patch_from_value(value: &Value) -> Result<Patch> {
    if !value.is_object() {
        return Err(wire("patch 必须是对象"));
    }
    if let Some(version) = value.get("version") {
        let version = version
            .as_u64()
            .ok_or_else(|| wire("patch.version 必须是整数"))?;
        if version != PATCH_VERSION {
            return Err(wire(format!(
                "不支持的 patch.version={version}（当前 {PATCH_VERSION}）"
            )));
        }
    }
    let edits = value
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| wire("patch.edits 必须是数组"))?;
    let mut out = Vec::with_capacity(edits.len());
    for edit in edits {
        out.push(edit_from_value(edit)?);
    }
    Ok(Patch::new(out))
}

/// 改动量 → JSON。
#[must_use]
pub fn cost_to_value(cost: &Cost) -> Value {
    json!({
        "edits": cost.edits,
        "trits": cost.trits,
        "biases": cost.biases,
        "scales": cost.scales,
        "row_ops": cost.row_ops,
        "touched": cost.touched,
    })
}

/// 网络摘要 → JSON（不含 WSP1 正文）。
#[must_use]
pub fn net_summary(net: &Net) -> Value {
    json!({
        "arch": net.arch(),
        "depth": net.depth(),
        "trits": net.trit_count(),
        "checksum": format!("{:016x}", checksum(net)),
        "state": format!("{:016x}", net.state_hash()),
    })
}

/// 程序执行报告 → JSON。
#[must_use]
pub fn report_to_value(report: &Report) -> Value {
    json!({
        "name": report.name,
        "applied": report.applied,
        "rolled_back": report.rolled_back,
        "before_ok": report.before_ok,
        "after_ok": report.after_ok,
        "before_signature": format!("{:016x}", report.before_signature),
        "changed_signature": format!("{:016x}", report.changed_signature),
        "final_signature": format!("{:016x}", report.final_signature),
        "cost": cost_to_value(&report.cost),
    })
}

/// 修补结果 → JSON。
#[must_use]
pub fn repair_to_value(repair: &Repair) -> Value {
    json!({
        "before_loss": repair.before_loss,
        "after_loss": repair.after_loss,
        "evaluated": repair.evaluated,
        "patch": patch_to_value(&repair.patch),
    })
}

/// 分析结果 → JSON。
#[must_use]
pub fn analysis_to_value(analysis: &Analysis) -> Value {
    json!({
        "depth": 1,
        "before_loss": analysis.before_loss,
        "evaluated": analysis.evaluated,
        "exact": analysis.before_loss == 0,
        "candidates": analysis
            .candidates
            .iter()
            .map(impact_to_value)
            .collect::<Vec<_>>(),
    })
}

/// 两编辑分析结果 → JSON。
#[must_use]
pub fn pair_analysis_to_value(analysis: &PairAnalysis) -> Value {
    json!({
        "depth": 2,
        "before_loss": analysis.before_loss,
        "evaluated": analysis.evaluated,
        "exact": analysis.before_loss == 0,
        "candidates": analysis
            .candidates
            .iter()
            .map(pair_impact_to_value)
            .collect::<Vec<_>>(),
    })
}

/// 两编辑的实测影响 → JSON。
#[must_use]
pub fn pair_impact_to_value(impact: &PairImpact) -> Value {
    json!({
        "edits": [edit_to_value(&impact.first), edit_to_value(&impact.second)],
        "before_loss": impact.before_loss,
        "after_loss": impact.after_loss,
        "gain": impact.gain,
        "fixed": impact.fixed,
        "broken": impact.broken,
        "changed": impact.changed,
    })
}

/// 三编辑分析结果 → JSON。
#[must_use]
pub fn triple_analysis_to_value(analysis: &TripleAnalysis) -> Value {
    json!({
        "depth": 3,
        "before_loss": analysis.before_loss,
        "evaluated": analysis.evaluated,
        "exact": analysis.before_loss == 0,
        "candidates": analysis
            .candidates
            .iter()
            .map(triple_impact_to_value)
            .collect::<Vec<_>>(),
    })
}

/// 三编辑的实测影响 → JSON。
#[must_use]
pub fn triple_impact_to_value(impact: &TripleImpact) -> Value {
    json!({
        "edits": [
            edit_to_value(&impact.first),
            edit_to_value(&impact.second),
            edit_to_value(&impact.third),
        ],
        "before_loss": impact.before_loss,
        "after_loss": impact.after_loss,
        "gain": impact.gain,
        "fixed": impact.fixed,
        "broken": impact.broken,
        "changed": impact.changed,
    })
}

/// 单样本解释 → JSON。
#[must_use]
pub fn sample_explanation_to_value(explanation: &SampleExplanation) -> Value {
    json!({
        "index": explanation.index,
        "input": trits_text(&explanation.input),
        "want": trits_text(&explanation.want),
        "got": trits_text(&explanation.got),
        "before_loss": explanation.before_loss,
        "evaluated": explanation.evaluated,
        "fully_evaluated": explanation.fully_evaluated,
        "candidates": explanation
            .candidates
            .iter()
            .map(sample_impact_to_value)
            .collect::<Vec<_>>(),
    })
}

/// 单样本编辑影响 → JSON。
#[must_use]
pub fn sample_impact_to_value(impact: &SampleEditImpact) -> Value {
    json!({
        "edit": edit_to_value(&impact.edit),
        "flips": impact.flips,
        "fixes": impact.fixes,
        "after_loss": impact.after_loss,
        "gain": impact.gain,
    })
}

/// 行级探针报告 → JSON。
#[must_use]
pub fn probe_report_to_value(report: &ProbeReport) -> Value {
    json!({
        "arch": report.arch,
        "before_loss": report.before_loss,
        "samples": report.samples,
        "rows": report.rows,
        "probed_rows": report.probed_rows,
        "sampled": report.sampled,
        "selection": report.selection.as_str(),
        "stride": report.stride,
        "top": report.top.iter().map(row_impact_to_value).collect::<Vec<_>>(),
    })
}

/// 一行消融结果 → JSON。
#[must_use]
pub fn row_impact_to_value(impact: &RowImpact) -> Value {
    json!({
        "layer": impact.layer,
        "row": impact.row,
        "trits": impact.trits,
        "zero": row_effect_to_value(&impact.zero),
        "negate": row_effect_to_value(&impact.negate),
    })
}

/// 一种行操作的影响 → JSON。
#[must_use]
pub fn row_effect_to_value(effect: &RowEffect) -> Value {
    json!({
        "after_loss": effect.after_loss,
        "gain": effect.gain,
        "changed": effect.changed,
        "fixed": effect.fixed,
        "broken": effect.broken,
    })
}

/// 单条编辑的实测影响 → JSON。
#[must_use]
pub fn impact_to_value(impact: &EditImpact) -> Value {
    json!({
        "edit": edit_to_value(&impact.edit),
        "before_loss": impact.before_loss,
        "after_loss": impact.after_loss,
        "gain": impact.gain,
        "fixed": impact.fixed,
        "broken": impact.broken,
        "changed": impact.changed,
    })
}

/// 验证集 → 规范 JSON（紧凑行字符串，可被 [`oracle_from_value`] 读回）。
#[must_use]
pub fn oracle_to_value(oracle: &Oracle) -> Value {
    json!({
        "inputs": oracle.inputs().iter().map(|row| trits_text(row)).collect::<Vec<_>>(),
        "outputs": oracle.outputs().iter().map(|row| trits_text(row)).collect::<Vec<_>>(),
    })
}

/// 验证集 → JSON。
pub fn oracle_from_value(value: &Value) -> Result<Oracle> {
    if value.get("inputs").is_some() || value.get("outputs").is_some() {
        let inputs = parse_rows(field(value, "inputs")?)?;
        let outputs = parse_rows(field(value, "outputs")?)?;
        return Oracle::new(inputs, outputs);
    }
    let domain = value
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("oracle 需要 inputs/outputs 或 domain/vars/table"))?;
    let vars = field_usize(value, "vars")?;
    let table = value
        .get("table")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("oracle.table 必须是字符串"))?;
    let chars: Vec<char> = table.chars().collect();
    match domain {
        "boolean" => {
            let base = Oracle::boolean_trit(vars, |_| Trit::Zero)?;
            let expected = 1usize
                .checked_shl(vars as u32)
                .ok_or_else(|| wire("boolean vars 太大"))?;
            if chars.len() != expected {
                return Err(wire(format!(
                    "boolean 表长度 {}，期望 {expected}（2^{vars}）",
                    chars.len()
                )));
            }
            let outputs = chars
                .iter()
                .map(|&c| Ok(vec![boolean_trit(c)?]))
                .collect::<Result<Vec<_>>>()?;
            Oracle::new(base.inputs().to_vec(), outputs)
        }
        "ternary" => {
            let base = Oracle::ternary(vars, |_| Trit::Zero)?;
            let expected = 3usize
                .checked_pow(vars as u32)
                .ok_or_else(|| wire("ternary vars 太大"))?;
            if chars.len() != expected {
                return Err(wire(format!(
                    "ternary 表长度 {}，期望 {expected}（3^{vars}）",
                    chars.len()
                )));
            }
            let outputs = chars
                .iter()
                .map(|&c| Ok(vec![parse_trit_char(c)?]))
                .collect::<Result<Vec<_>>>()?;
            Oracle::new(base.inputs().to_vec(), outputs)
        }
        other => Err(wire(format!("未知 domain `{other}`（boolean / ternary）"))),
    }
}

/// 失配见证 → JSON；`limit` 控制返回条数（`count` 始终是全量）。
pub fn mismatches_value(net: &Net, oracle: &Oracle, limit: usize) -> Result<Value> {
    let actual = oracle.outputs_of(net)?;
    let mut count = 0usize;
    let mut items = Vec::new();
    for (index, (got, want)) in actual.iter().zip(oracle.outputs()).enumerate() {
        if got != want {
            count += 1;
            if items.len() < limit {
                items.push(json!({
                    "index": index,
                    "input": trits_text(&oracle.inputs()[index]),
                    "want": trits_text(want),
                    "got": trits_text(got),
                }));
            }
        }
    }
    Ok(json!({"count": count, "items": items, "truncated": count > items.len()}))
}

/// 三值切片 → 紧凑字符串。
#[must_use]
pub fn trits_text(values: &[Trit]) -> String {
    values.iter().map(|&t| t.to_char()).collect()
}

/// 从 proposal 候选构造补丁：接受 `{"edit": {...}}`（单点）或 `{"edits": [...]}`（组合）。
pub fn patch_from_candidate(value: &Value) -> Result<Patch> {
    if let Some(edit) = value.get("edit") {
        return Ok(Patch::new(vec![edit_from_value(edit)?]));
    }
    let edits = value
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| wire("候选需要 edit（单点）或 edits（组合）"))?;
    if edits.is_empty() {
        return Err(wire("候选 edits 不能为空"));
    }
    let mut out = Vec::with_capacity(edits.len());
    for edit in edits {
        out.push(edit_from_value(edit)?);
    }
    Ok(Patch::new(out))
}

/// 单条编辑 JSON → [`Edit`]。
pub fn edit_from_value(value: &Value) -> Result<Edit> {
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("edit.op 必须是字符串"))?;
    match op {
        "set_trit" => Ok(Edit::SetTrit {
            layer: field_usize(value, "layer")?,
            row: field_usize(value, "row")?,
            col: field_usize(value, "col")?,
            to: field_trit(value, "to")?,
        }),
        "set_bias" => Ok(Edit::SetBias {
            layer: field_usize(value, "layer")?,
            row: field_usize(value, "row")?,
            to: field_trit(value, "to")?,
        }),
        "set_scale" => Ok(Edit::SetScale {
            layer: field_usize(value, "layer")?,
            row: field_usize(value, "row")?,
            to: field_f32(value, "to")?,
        }),
        "swap_rows" => Ok(Edit::SwapRows {
            layer: field_usize(value, "layer")?,
            a: field_usize(value, "a")?,
            b: field_usize(value, "b")?,
        }),
        "negate_row" => Ok(Edit::NegateRow {
            layer: field_usize(value, "layer")?,
            row: field_usize(value, "row")?,
        }),
        other => Err(wire(format!("未知 edit.op `{other}`"))),
    }
}

fn parse_rows(value: &Value) -> Result<Vec<Vec<Trit>>> {
    let items = value
        .as_array()
        .ok_or_else(|| wire("inputs/outputs 必须是数组"))?;
    items.iter().map(row_trits).collect()
}

fn row_trits(value: &Value) -> Result<Vec<Trit>> {
    match value {
        Value::String(text) => text.chars().map(parse_trit_char).collect(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return parse_trit(text);
                }
                if let Some(number) = item.as_i64() {
                    return Trit::from_i8(number as i8)
                        .ok_or_else(|| wire(format!("非法三值 {number}")));
                }
                Err(wire("三值必须是字符串或 -1/0/1"))
            })
            .collect(),
        _ => Err(wire("一行必须是字符串或数组")),
    }
}

fn parse_trit(text: &str) -> Result<Trit> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => parse_trit_char(c),
        _ => Err(wire(format!("三值必须是单个字符 -/0/+，得到 {text:?}"))),
    }
}

fn parse_trit_char(c: char) -> Result<Trit> {
    Trit::from_char(c).ok_or_else(|| wire(format!("非法三值字符 `{c}`")))
}

fn boolean_trit(c: char) -> Result<Trit> {
    match c {
        '0' => Ok(Trit::Neg),
        '1' => Ok(Trit::Pos),
        'x' | 'X' => Ok(Trit::Zero),
        _ => Err(wire(format!("boolean 表只接受 0/1/x，得到 `{c}`"))),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| wire(format!("缺少字段 `{name}`")))
}

fn field_usize(value: &Value, name: &str) -> Result<usize> {
    let raw = field(value, name)?;
    let number = raw
        .as_u64()
        .ok_or_else(|| wire(format!("`{name}` 必须是非负整数")))?;
    usize::try_from(number).map_err(|_| wire(format!("`{name}` 超出 usize")))
}

fn field_f32(value: &Value, name: &str) -> Result<f32> {
    let number = field(value, name)?
        .as_f64()
        .ok_or_else(|| wire(format!("`{name}` 必须是数字")))?;
    let number = number as f32;
    if !number.is_finite() {
        return Err(wire(format!("`{name}` 必须是有限数")));
    }
    Ok(number)
}

fn field_trit(value: &Value, name: &str) -> Result<Trit> {
    parse_trit(
        field(value, name)?
            .as_str()
            .ok_or_else(|| wire(format!("`{name}` 必须是字符串")))?,
    )
}

fn wire(message: impl Into<String>) -> Error {
    Error::Wire(message.into())
}
