//! trit 的 WASM 绑定：给 `playground/` 用的最小 API。
//!
//! 所有函数只做三件事：解析、求值、把结果编成 JSON 字符串。错误用 `JsValue` 抛出，
//! 页面上直接显示消息。

#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};
use wasm_bindgen::prelude::*;

mod js_args;
use js_args::{assignment_json, js_err, parse_algebra, parse_assignment, parse_b4_assignment};

use trit_circuit::netlist::parse_netlist;
use trit_core::limits::{Limits, check_expr};
use trit_core::parser::Parser;
use trit_eval::eval::{decode_assignment, equivalent_within, eval_within, truth_table_within};

/// 单点求值，返回 `0` / `1` / `X`。
#[wasm_bindgen(js_name = evalExpr)]
pub fn eval_expr_js(expr: &str, assign: &str, algebra: &str) -> Result<String, JsValue> {
    let parsed = Parser::parse(expr).map_err(|e| js_err(format!("解析失败：{e}")))?;
    let assignment = parse_assignment(assign)?;
    let alg = parse_algebra(algebra)?;
    let value = eval_within(&parsed, &assignment, alg, Limits::default())
        .map_err(|e| js_err(format!("{e}")))?;
    Ok(value.as_char().to_string())
}

/// 三值真值表，返回 `{ variables: [...], rows: [{ assignment, value }] }`。
#[wasm_bindgen(js_name = truthTable)]
pub fn truth_table_js(expr: &str, algebra: &str) -> Result<String, JsValue> {
    let parsed = Parser::parse(expr).map_err(|e| js_err(format!("解析失败：{e}")))?;
    let alg = parse_algebra(algebra)?;
    let table = truth_table_within(
        &parsed,
        alg,
        Limits {
            max_vars: 10,
            ..Limits::default()
        },
    )
    .map_err(|e| js_err(format!("{e}")))?;
    let vars = table.variables.clone();
    let rows: Vec<Value> = table
        .values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let assignment = decode_assignment(index as u64, &vars);
            json!({
                "assignment": assignment_json(&assignment, &vars),
                "value": value.as_char().to_string(),
            })
        })
        .collect();
    Ok(json!({ "variables": vars, "rows": rows }).to_string())
}

/// 等价判定，返回 `{ equivalent, variable_count, counterexample }`。
#[wasm_bindgen(js_name = equivalent)]
pub fn equivalent_js(left: &str, right: &str, algebra: &str) -> Result<String, JsValue> {
    let l = Parser::parse(left).map_err(|e| js_err(format!("左式解析失败：{e}")))?;
    let r = Parser::parse(right).map_err(|e| js_err(format!("右式解析失败：{e}")))?;
    let alg = parse_algebra(algebra)?;
    let verdict =
        equivalent_within(&l, &r, alg, Limits::default()).map_err(|e| js_err(format!("{e}")))?;
    let counterexample = verdict
        .counterexample_assignment
        .as_ref()
        .map(|assignment| {
            let vars = assignment.variables();
            assignment_json(assignment, &vars)
        });
    Ok(json!({
        "equivalent": verdict.equivalent,
        "variable_count": verdict.variable_count,
        "counterexample": counterexample,
    })
    .to_string())
}

/// Belnap 四值单点求值，返回 `T` / `F` / `B` / `N`。
#[wasm_bindgen(js_name = belnapEval)]
pub fn belnap_eval_js(expr: &str, assign: &str) -> Result<String, JsValue> {
    let parsed = Parser::parse(expr).map_err(|e| js_err(format!("解析失败：{e}")))?;
    check_expr(&parsed, Limits::default()).map_err(|e| js_err(format!("{e}")))?;
    let assignment = parse_b4_assignment(assign)?;
    Ok(trit_belnap::eval(&parsed, &assignment)
        .as_char()
        .to_string())
}

/// Belnap 四值真值表（最多 6 个变量）。
#[wasm_bindgen(js_name = belnapTable)]
pub fn belnap_table_js(expr: &str) -> Result<String, JsValue> {
    let parsed = Parser::parse(expr).map_err(|e| js_err(format!("解析失败：{e}")))?;
    check_expr(&parsed, Limits::default()).map_err(|e| js_err(format!("{e}")))?;
    let vars = parsed.variables();
    if vars.len() > 6 {
        return Err(js_err("四值真值表最多 6 个变量"));
    }
    let table = trit_belnap::table(&parsed).map_err(|e| js_err(format!("{e}")))?;
    let rows: Vec<Value> = table
        .rows
        .iter()
        .map(|row| {
            json!({
                "assignment": table
                    .vars
                    .iter()
                    .zip(row.values.iter())
                    .map(|(name, value)| (name.clone(), Value::String(value.as_char().to_string())))
                    .collect::<Map<String, Value>>(),
                "value": row.value.as_char().to_string(),
            })
        })
        .collect();
    Ok(json!({ "variables": table.vars, "rows": rows }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_eval::eval::Assignment;

    #[test]
    fn eval_expr_works_on_host() {
        assert_eq!(eval_expr_js("a & b", "a=1", "kleene").unwrap(), "X");
        assert_eq!(eval_expr_js("a & b", "a=1,b=0", "kleene").unwrap(), "0");
    }

    #[test]
    fn truth_table_has_three_to_the_n_rows() {
        let raw = truth_table_js("a & b", "kleene").unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["variables"].as_array().unwrap().len(), 2);
        assert_eq!(value["rows"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn de_morgan_is_equivalent() {
        let raw = equivalent_js("!(a & b)", "!a | !b", "kleene").unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["equivalent"], json!(true));
    }

    #[test]
    fn belnap_conflict_is_false() {
        assert_eq!(belnap_eval_js("a & b", "a=N,b=B").unwrap(), "F");
        let raw = belnap_table_js("a | !a").unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["rows"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn assignment_json_marks_variables_without_a_value_as_x() {
        // 出口契约：没赋值的变量要显式印成 `X`（页面看得见"这格没值"），
        // 不能省略键，也不能被读成别的意思。
        let assignment = Assignment::new();
        let vars = ["a".to_string(), "b".to_string()];
        let map = assignment_json(&assignment, &vars);
        assert_eq!(map.get("a"), Some(&json!("X")), "缺值必须印 X：{map:?}");
        assert_eq!(map.get("b"), Some(&json!("X")));

        let mut with_a = Assignment::new();
        with_a.set("a", trit_core::value::Ternary::True);
        let map = assignment_json(&with_a, &vars);
        assert_eq!(map.get("a"), Some(&json!("1")), "有值按 as_char 印出");
        assert_eq!(map.get("b"), Some(&json!("X")), "另一格仍按缺值处理");
    }
}

/// 对网表做 ATPG，返回覆盖率、测试向量与不可测故障数（JSON 字符串）。
#[wasm_bindgen]
pub fn atpg_js(netlist: &str, algebra: &str) -> Result<String, JsValue> {
    let alg = parse_algebra(algebra)?;
    let circuit = parse_netlist(netlist).map_err(|e| js_err(format!("{e}")))?;
    if circuit.input_count() > 10 {
        return Err(js_err("ATPG 最多 10 个主输入"));
    }
    let report = trit_circuit::generate_tests(&circuit, alg).map_err(|e| js_err(format!("{e}")))?;
    let patterns: Vec<Value> = report
        .patterns
        .iter()
        .map(|pattern| {
            let inputs: serde_json::Map<String, Value> = pattern
                .inputs
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.as_char().to_string())))
                .collect();
            json!({
                "inputs": inputs,
                "covers": pattern.covers,
                "dont_care": pattern.dont_care,
            })
        })
        .collect();
    Ok(json!({
        "coverage": report.coverage,
        "testable": report.testable,
        "total": report.faults.len(),
        "optimal": report.optimal,
        "exhaustive": report.exhaustive,
        "x_coverage": report.x_coverage,
        "merged": report.merged,
        "dont_care_bits": report.dont_care_bits,
        "patterns": patterns,
        "untestable": report.untestable.len(),
    })
    .to_string())
}
