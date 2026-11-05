//! 组合语义的线格式：行为、脚本、分片检查点。

use serde_json::{Value, json};

use crate::codec::{from_wsp, to_wsp};
use crate::error::Result;
use crate::oracle::Behavior;
use crate::program::{Kind, Program, Script, ScriptReport};
use crate::shard::Checkpoint;

use super::{parse_rows, patch_from_value, patch_to_value, report_to_value, trits_text, wire};

/// 行为（逐样本输出）→ JSON。
#[must_use]
pub fn behavior_to_value(behavior: &Behavior) -> Value {
    json!({
        "outputs": behavior
            .outputs()
            .iter()
            .map(|row| trits_text(row))
            .collect::<Vec<_>>(),
    })
}

/// JSON → 行为；`outputs` 每行是 trit 字符串或数组。
pub fn behavior_from_value(value: &Value) -> Result<Behavior> {
    let outputs = value
        .get("outputs")
        .ok_or_else(|| wire("behavior 需要 outputs"))?;
    Ok(Behavior::from_outputs(parse_rows(outputs)?))
}

/// 分片重放检查点 → JSON（工件是 WSP1 文本）。
#[must_use]
pub fn checkpoint_to_value(checkpoint: &Checkpoint) -> Value {
    json!({
        "version": 1,
        "shard": checkpoint.shard,
        "entries": checkpoint.entries,
        "head": format!("{:016x}", checkpoint.head),
        "applied": checkpoint.applied,
        "rolled_back": checkpoint.rolled_back,
        "skipped": checkpoint.skipped,
        "behavior_checked": checkpoint.behavior_checked,
        "artifact": to_wsp(&checkpoint.net),
    })
}

/// JSON → 检查点。
pub fn checkpoint_from_value(value: &Value) -> Result<Checkpoint> {
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| wire("checkpoint 缺少 version"))?;
    if version != 1 {
        return Err(wire(format!("checkpoint 版本 {version}（当前 1）")));
    }
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| wire(format!("checkpoint 缺少 `{name}`")))
    };
    let head_text = value
        .get("head")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("checkpoint 缺少 `head`"))?;
    let head = u64::from_str_radix(head_text, 16)
        .map_err(|_| wire(format!("checkpoint `head` 不是十六进制：{head_text}")))?;
    let artifact = value
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("checkpoint 缺少 `artifact`"))?;
    Ok(Checkpoint {
        shard: usize::try_from(field("shard")?).map_err(|_| wire("checkpoint shard 超出 usize"))?,
        entries: usize::try_from(field("entries")?)
            .map_err(|_| wire("checkpoint entries 超出 usize"))?,
        head,
        net: from_wsp(artifact)?,
        applied: usize::try_from(field("applied").unwrap_or(0))
            .map_err(|_| wire("checkpoint applied 超出 usize"))?,
        rolled_back: usize::try_from(field("rolled_back").unwrap_or(0))
            .map_err(|_| wire("checkpoint rolled_back 超出 usize"))?,
        skipped: usize::try_from(field("skipped").unwrap_or(0))
            .map_err(|_| wire("checkpoint skipped 超出 usize"))?,
        behavior_checked: usize::try_from(field("behavior_checked").unwrap_or(0))
            .map_err(|_| wire("checkpoint behavior_checked 超出 usize"))?,
    })
}

/// 脚本 → JSON（type = step / sequence / when）。
#[must_use]
pub fn script_to_value(script: &Script) -> Value {
    match &script.kind {
        Kind::Step(program) => {
            let mut value = json!({
                "type": "step",
                "name": program.name(),
                "patch": patch_to_value(program.patch()),
            });
            if let Some(before) = program.before() {
                value["before"] = behavior_to_value(before);
            }
            if let Some(after) = program.after() {
                value["after"] = behavior_to_value(after);
            }
            value
        }
        Kind::Sequence {
            name,
            atomic,
            steps,
        } => json!({
            "type": "sequence",
            "name": name,
            "atomic": atomic,
            "steps": steps.iter().map(script_to_value).collect::<Vec<_>>(),
        }),
        Kind::When {
            name,
            expected,
            then,
            otherwise,
        } => json!({
            "type": "when",
            "name": name,
            "when": behavior_to_value(expected),
            "then": script_to_value(then),
            "otherwise": otherwise.as_ref().map(|script| script_to_value(script)),
        }),
    }
}

/// JSON → 脚本。
pub fn script_from_value(value: &Value) -> Result<Script> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| wire("脚本缺少 type（step/sequence/when）"))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("script");
    let kind = match kind {
        "step" => {
            let patch =
                patch_from_value(value.get("patch").ok_or_else(|| wire("step 缺少 patch"))?)?;
            let mut program = Program::new(name, patch);
            if let Some(before) = value.get("before") {
                program = program.require_before(behavior_from_value(before)?);
            }
            if let Some(after) = value.get("after") {
                program = program.require_after(behavior_from_value(after)?);
            }
            Kind::Step(program)
        }
        "sequence" => {
            let atomic = value.get("atomic").and_then(Value::as_bool).unwrap_or(true);
            let raw = value
                .get("steps")
                .and_then(Value::as_array)
                .ok_or_else(|| wire("sequence 缺少 steps 数组"))?;
            let steps = raw
                .iter()
                .map(script_from_value)
                .collect::<Result<Vec<_>>>()?;
            Kind::Sequence {
                name: name.to_string(),
                atomic,
                steps,
            }
        }
        "when" => {
            let expected = behavior_from_value(
                value
                    .get("when")
                    .ok_or_else(|| wire("when 缺少 when（行为）"))?,
            )?;
            let then = Box::new(script_from_value(
                value.get("then").ok_or_else(|| wire("when 缺少 then"))?,
            )?);
            let otherwise = value
                .get("otherwise")
                .filter(|value| !value.is_null())
                .map(script_from_value)
                .transpose()?
                .map(Box::new);
            Kind::When {
                name: name.to_string(),
                expected,
                then,
                otherwise,
            }
        }
        other => return Err(wire(format!("未知脚本 type `{other}`"))),
    };
    Ok(Script { kind })
}

/// 脚本执行报告 → JSON（树形）。
#[must_use]
pub fn script_report_to_value(report: &ScriptReport) -> Value {
    match report {
        ScriptReport::Step(step) => {
            let mut value = report_to_value(step);
            value["type"] = json!("step");
            value
        }
        ScriptReport::Sequence {
            name,
            atomic,
            steps,
            rolled_back,
            final_signature,
        } => json!({
            "type": "sequence",
            "name": name,
            "atomic": atomic,
            "steps": steps.iter().map(script_report_to_value).collect::<Vec<_>>(),
            "rolled_back": rolled_back,
            "final_signature": format!("{final_signature:016x}"),
        }),
        ScriptReport::When {
            name,
            matched,
            chosen,
            final_signature,
        } => json!({
            "type": "when",
            "name": name,
            "matched": matched,
            "chosen": chosen.as_ref().map(|report| script_report_to_value(report)),
            "final_signature": format!("{final_signature:016x}"),
        }),
    }
}
