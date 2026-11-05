//! `count` 子命令：精确模型计数（DPLL 主路，可选与穷举 / BDD 三路对拍）。
//!
//! 计数走 `trit-count` 的 CNF+DPLL 引擎；`--crosscheck` 才付穷举与 BDD 的钱
//! （穷举是 3^n，BDD 会炸——预算拦下时报 `refused` 而不是"两路不一致"）。

use std::collections::HashSet;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use trit_core::value::Ternary;

use crate::{parse_expr, parse_var_list, resolve_algebra};

#[derive(Debug, Subcommand)]
pub(crate) enum CountCommand {
    /// 精确计数使表达式取目标值的赋值（逗号分隔的 `--constraints` 必须先全为 `True`）。
    Models(CountArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CountArgs {
    /// 待计数的表达式，例如 `a & b | c`。
    expression: String,
    /// 计数域（逗号分隔）；省略则取表达式里出现的变量。
    #[arg(long, value_name = "VARS")]
    vars: Option<String>,
    /// 前置约束（逗号分隔的表达式），要求每一条都取 `True`。
    #[arg(long, value_name = "EXPRS")]
    constraints: Option<String>,
    /// 把列出的变量当作已知二值（逗号分隔），其取值域为 {0,1}。
    #[arg(long, value_name = "VARS")]
    binary: Option<String>,
    /// 目标值：`true`（默认）/ `false` / `unknown`。
    #[arg(long, default_value = "true", value_name = "VALUE")]
    target: String,
    /// 再跑穷举与 BDD 两条独立路径并对拍（穷举是 3^n，只在小规模上开）。
    #[arg(long)]
    crosscheck: bool,
    /// 对拍里 BDD 的节点预算（0 = 不设上限）。
    #[arg(long, value_name = "N", default_value_t = 200_000)]
    bdd_max_nodes: usize,
    /// JSON 输出。
    #[arg(long)]
    json: bool,
}

fn parse_target(word: &str) -> Result<Ternary> {
    match word.to_ascii_lowercase().as_str() {
        "true" | "t" | "1" => Ok(Ternary::True),
        "false" | "f" | "0" => Ok(Ternary::False),
        "unknown" | "u" | "x" | "?" => Ok(Ternary::Unknown),
        other => bail!("未知目标值 {other:?}，可选 true / false / unknown"),
    }
}

impl CountArgs {
    pub(crate) fn run(&self, alg_name: &str) -> Result<()> {
        let alg = resolve_algebra(alg_name)?;
        let expr = parse_expr(&self.expression)?;
        let all = expr.variables();
        let vars = match &self.vars {
            Some(list) => parse_var_list(list, &all, "--vars")?,
            None => all.clone(),
        };
        if vars.is_empty() {
            bail!("计数域为空：表达式没有变量，请用 --vars 显式给出域");
        }
        let constraints: Vec<_> = match &self.constraints {
            Some(list) => list
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(parse_expr)
                .collect::<Result<_>>()?,
            None => Vec::new(),
        };
        let mut binary = HashSet::new();
        if let Some(list) = &self.binary {
            binary.extend(parse_var_list(list, &all, "--binary")?);
        }
        let target = parse_target(&self.target)?;
        let result =
            trit_count::count_models_under(&expr, &constraints, &vars, &binary, alg, target);
        let mut report = serde_json::json!({
            "algebra": alg.as_str(),
            "method": "cnf-dpll",
            "vars": vars,
            "binary_vars": binary.iter().collect::<Vec<_>>(),
            "target": format!("{target:?}").to_ascii_lowercase(),
            "count": result.count.to_string(),
            "nodes": result.stats.nodes,
            "branches": result.stats.branches,
            "components": result.stats.components,
            "cache_hits": result.stats.cache_hits,
            "cache_misses": result.stats.cache_misses,
        });

        if self.crosscheck {
            let limits = if self.bdd_max_nodes == 0 {
                trit_count::BddLimits::UNLIMITED
            } else {
                trit_count::BddLimits {
                    max_nodes: self.bdd_max_nodes,
                }
            };
            let got = trit_count::crosscheck_under_with_limits(
                &expr,
                &constraints,
                &vars,
                &binary,
                alg,
                target,
                limits,
            );
            report["crosscheck"] = serde_json::json!({
                "matches": got.matches,
                "three_way": got.three_way,
                "dpll_count": got.dpll_count.to_string(),
                "exhaustive_count": got.exhaustive_count.to_string(),
                "bdd_count": got.bdd_count.map(|c| c.to_string()),
                "bdd_refused": got.bdd_error,
                "dpll_us": got.dpll_time.as_micros().to_string(),
                "exhaustive_us": got.exhaustive_time.as_micros().to_string(),
                "bdd_us": got.bdd_time.as_micros().to_string(),
            });
            if !got.matches {
                bail!(
                    "三路对拍不一致：DPLL {} / 穷举 {} / BDD {}",
                    got.dpll_count,
                    got.exhaustive_count,
                    got.bdd_count
                        .map_or_else(|| "refused".to_string(), |c| c.to_string())
                );
            }
        }

        if self.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "模型数 {} = {}  [{} / cnf-dpll，节点 {}、分支 {}、分解 {}]",
                self.expression,
                result.count,
                alg.as_str(),
                result.stats.nodes,
                result.stats.branches,
                result.stats.components,
            );
            if let Some(cc) = report.get("crosscheck") {
                // 拒绝原因本身就以 "BDD …" 开头，这里不再叠一层前缀。
                let bdd = match (cc["bdd_count"].as_str(), cc["bdd_refused"].as_str()) {
                    (Some(c), _) => c.to_string(),
                    (None, Some(reason)) => format!("拒（{reason}）"),
                    (None, None) => "-".to_string(),
                };
                println!(
                    "对拍：穷举 {} ｜ BDD {bdd} ｜ 三路一致 {}",
                    cc["exhaustive_count"].as_str().unwrap_or("-"),
                    if cc["three_way"].as_bool().unwrap_or(false) {
                        "是"
                    } else {
                        "否（预算拦下 BDD 时也是正常）"
                    }
                );
            }
        }
        Ok(())
    }
}
