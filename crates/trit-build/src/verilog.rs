//! M1：Verilog 子集前端（复用 `trit-circuit`）+ 双链路真值表对拍。
//!
//! 这里不自己写 Verilog 解析器：`trit-circuit::parse_verilog` 已经处理
//! 声明/assign/位选/预算，且与 BLIF/AIGER 等其它入口共用同一套门模型。
//! 本模块只负责：
//! 1. 选主输出（单输出默认，多输出必须 `--output`）；
//! 2. 对 3^n 组输入跑 `propagate`（K3/Ł3/B3 语义）；
//! 3. 与 `.tri` 表达式链路逐行对拍——不一致即构建失败。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use trit_circuit::{parse_verilog, propagate};
use trit_core::Algebra;
use trit_core::value::Ternary;
use trit_eval::eval::{Assignment, eval};

/// 一个 Verilog 设计在 3^n 组输入下的输出序列。
pub struct Design {
    pub name: String,
    pub inputs: Vec<String>,
    pub output: String,
    pub table: Vec<Ternary>,
}

fn digit_value(code: &mut u64) -> Ternary {
    let digit = *code % 3;
    *code /= 3;
    match digit {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

/// 解析 + 穷举仿真。
pub fn load(path: &Path, output: Option<&str>, alg: Algebra, max_vars: u32) -> Result<Design> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let circuit = parse_verilog(&text)
        .map_err(|error| anyhow::anyhow!("Verilog 解析失败（{}）：{error}", path.display()))?;
    let output = match output {
        Some(name) => name.to_string(),
        None => match circuit.outputs.as_slice() {
            [single] => single.name().to_string(),
            many => bail!(
                "设计有 {} 个输出，必须用 --output 指定（可选：{}）",
                many.len(),
                many.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
            ),
        },
    };
    if !circuit.outputs.iter().any(|s| s.name() == output) {
        bail!(
            "没有名为 {output:?} 的输出（可选：{}）",
            circuit
                .outputs
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let inputs: Vec<String> = circuit
        .inputs
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    if inputs.len() as u32 > max_vars {
        bail!(
            "输入数 {} 超过构建穷举上限 {max_vars}（3^n 会爆）",
            inputs.len()
        );
    }
    let count = 3u64
        .checked_pow(inputs.len() as u32)
        .context("输入太多，3^n 溢出")?;
    let mut table = Vec::with_capacity(count as usize);
    for code in 0..count {
        let mut rest = code;
        let mut assignment = BTreeMap::new();
        for name in &inputs {
            assignment.insert(name.clone(), digit_value(&mut rest));
        }
        let propagation = propagate(&circuit, &assignment, alg)
            .map_err(|error| anyhow::anyhow!("仿真 {output} 失败：{error}"))?;
        let value = propagation
            .outputs
            .get(&output)
            .copied()
            .with_context(|| format!("输出 {output} 在仿真结果里缺失"))?;
        table.push(value);
    }
    Ok(Design {
        name: circuit.name.clone(),
        inputs,
        output,
        table,
    })
}

/// 与 `.tri` 表达式链路逐行对拍。
pub fn cross_check(design: &Design, expression: &trit_core::Expr, alg: Algebra) -> Result<()> {
    let expr_vars = expression.variables();
    if expr_vars.len() != design.inputs.len() {
        bail!(
            "变量数不一致：表达式 {} 个（{}），设计 {} 个（{}）",
            expr_vars.len(),
            expr_vars.join(","),
            design.inputs.len(),
            design.inputs.join(",")
        );
    }
    for var in &expr_vars {
        if !design.inputs.contains(var) {
            bail!(
                "表达式变量 {var:?} 不在设计的输入里（设计输入：{}）",
                design.inputs.join(",")
            );
        }
    }
    for (code, expected) in design.table.iter().enumerate() {
        let mut rest = code as u64;
        let mut assignment = Assignment::new();
        for name in &design.inputs {
            assignment.set(name.clone(), digit_value(&mut rest));
        }
        let actual = eval(expression, &assignment, alg);
        if actual != *expected {
            bail!(
                "两条链路不一致：code={code} 设计({})={expected:?} 表达式={actual:?}",
                design.output
            );
        }
    }
    Ok(())
}
