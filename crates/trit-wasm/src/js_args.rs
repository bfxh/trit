//! JS 边界上的入参解析与出参序列化——`lib.rs` 只留"求值 + 编解码"这三件事。
//!
//! 搬过来只为守住索引文件的行数预算；可见性用 `pub(crate)`，对外 API（`#[wasm_bindgen]`）
//! 仍然只在 `lib.rs` 出现，公开面没有变化。

use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

use trit_core::algebra::Algebra;
use trit_eval::eval::Assignment;

pub(crate) fn js_err(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

pub(crate) fn parse_algebra(src: &str) -> Result<Algebra, JsValue> {
    Algebra::parse(src).ok_or_else(|| {
        js_err(format!(
            "未知语义 {src:?}，可选 kleene / lukasiewicz / bochvar"
        ))
    })
}

pub(crate) fn parse_assignment(src: &str) -> Result<Assignment, JsValue> {
    let mut assignment = Assignment::new();
    for part in src.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            return Err(js_err(format!("赋值应为 name=value，收到 {part:?}")));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(js_err(format!("赋值 {part:?} 缺少变量名")));
        }
        let value = value.trim();
        let mut chars = value.chars();
        let ch = chars
            .next()
            .ok_or_else(|| js_err(format!("赋值 {part:?} 缺少取值")))?;
        if chars.next().is_some() {
            return Err(js_err(format!("赋值 {part:?} 的取值只能是单字符 0/1/X")));
        }
        let trit = trit_core::value::Ternary::parse_char(ch)
            .ok_or_else(|| js_err(format!("未知取值 {ch:?}，只能是 0 / 1 / X")))?;
        assignment.set(name.to_string(), trit);
    }
    Ok(assignment)
}

pub(crate) fn parse_b4_assignment(
    src: &str,
) -> Result<std::collections::BTreeMap<String, trit_belnap::B4>, JsValue> {
    let mut assignment = std::collections::BTreeMap::new();
    for part in src.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            return Err(js_err(format!("赋值应为 name=value，收到 {part:?}")));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(js_err(format!("赋值 {part:?} 缺少变量名")));
        }
        let value = value.trim();
        let mut chars = value.chars();
        let ch = chars
            .next()
            .ok_or_else(|| js_err(format!("赋值 {part:?} 缺少取值")))?;
        if chars.next().is_some() {
            return Err(js_err(format!("赋值 {part:?} 的取值只能是单字符 T/F/B/N")));
        }
        let v = trit_belnap::B4::parse_char(ch)
            .ok_or_else(|| js_err(format!("未知四值 {ch:?}，只能是 T/F/B/N（也接受 0/1/?/*）")))?;
        assignment.insert(name.to_string(), v);
    }
    Ok(assignment)
}

pub(crate) fn assignment_json(assignment: &Assignment, vars: &[String]) -> Map<String, Value> {
    vars.iter()
        .map(|name| {
            let value = assignment
                .get(name)
                .map(|v| v.as_char().to_string())
                .unwrap_or_else(|| "X".to_string());
            (name.clone(), Value::String(value))
        })
        .collect()
}
