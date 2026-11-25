//! `equiv` 子命令。

use std::collections::HashSet;

use anyhow::{Result, bail};
use trit_bdd::{
    BddLimits, BddRequest, VarOrder, equivalent_bdd_best_with_limits, equivalent_bdd_with,
    order_variables,
};
use trit_core::limits::Limits;
use trit_core::value::Ternary;
use trit_eval::eval::equivalent_within;
use trit_sat::symbolic::{
    EncodingSize, encoding_size, equivalent_symbolic_with, minimal_counterexample,
};

use crate::output::Format;
use crate::{parse_expr, resolve_algebra};

pub(crate) struct EquivOptions {
    pub(crate) symbolic: bool,
    pub(crate) bdd: bool,
    pub(crate) bdd_order: String,
    pub(crate) bdd_max_nodes: usize,
    pub(crate) max_vars: usize,
    pub(crate) binary: Option<String>,
    pub(crate) binary_all: bool,
    pub(crate) stats: bool,
    pub(crate) minimize: bool,
}

pub(crate) fn cmd_equiv(
    left: &str,
    right: &str,
    format: Format,
    alg_name: &str,
    opts: EquivOptions,
) -> Result<()> {
    let EquivOptions {
        symbolic,
        bdd,
        bdd_order,
        bdd_max_nodes,
        max_vars,
        binary: binary_list,
        binary_all,
        stats,
        minimize,
    } = opts;
    let alg = resolve_algebra(alg_name)?;
    let a = parse_expr(left)?;
    let b = parse_expr(right)?;
    let mut vars = a.variables();
    for v in b.variables() {
        if !vars.contains(&v) {
            vars.push(v);
        }
    }
    let mut binary: HashSet<String> = HashSet::new();
    if binary_all {
        binary.extend(vars.iter().cloned());
    }
    if let Some(list) = binary_list {
        for name in list.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if !vars.iter().any(|v| v == name) {
                bail!("--binary 变量 {name} 不在表达式里");
            }
            binary.insert(name.to_string());
        }
    }
    if bdd && symbolic {
        bail!("--bdd 与 --symbolic 只能选一个");
    }
    if !binary.is_empty() && !symbolic && !bdd {
        bail!("--binary / --binary-all 只用于 --symbolic 或 --bdd");
    }
    if minimize && !symbolic {
        bail!("--minimize 只用于 --symbolic");
    }
    if minimize {
        let witness = minimal_counterexample(&a, &b, alg, &binary)?;
        match format {
            Format::Json => {
                let mut obj = serde_json::json!({
                    "algebra": alg.as_str(),
                    "method": "sat-min",
                    "equivalent": witness.is_none(),
                });
                if let Some(asg) = &witness {
                    let mut pairs = serde_json::Map::new();
                    for name in asg.variables() {
                        pairs.insert(
                            name.clone(),
                            serde_json::Value::String(
                                asg.get(&name)
                                    .unwrap_or(Ternary::Unknown)
                                    .as_char()
                                    .to_string(),
                            ),
                        );
                    }
                    obj["counterexample_minimal"] = serde_json::Value::Object(pairs);
                }
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
            _ => {
                if let Some(asg) = &witness {
                    let detail = asg
                        .variables()
                        .iter()
                        .map(|name| {
                            format!(
                                "{name}={}",
                                asg.get(name).unwrap_or(Ternary::Unknown).as_char()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("不等价：{left} ≢ {right}  [{alg} / sat-min]");
                    println!("最小反例：{detail}");
                } else {
                    println!("等价：{left} ≡ {right}  [{alg} / sat-min]");
                }
            }
        }
        return if witness.is_none() {
            Ok(())
        } else {
            std::process::exit(2)
        };
    }
    let stats_value: Option<EncodingSize> = if symbolic && stats {
        Some(encoding_size(&a, &b, alg, &binary)?)
    } else {
        None
    };
    let limits = if bdd_max_nodes == 0 {
        BddLimits::UNLIMITED
    } else {
        BddLimits {
            max_nodes: bdd_max_nodes,
        }
    };
    let mut bdd_order_used = None;
    let (result, method, bdd_nodes) = if bdd {
        let verdict = match bdd_order.as_str() {
            "best" => {
                let (verdict, order) =
                    equivalent_bdd_best_with_limits(&a, &b, alg, &binary, limits)?;
                bdd_order_used = Some(order.as_str());
                verdict
            }
            name => {
                let order = VarOrder::parse(name)
                    .ok_or_else(|| anyhow::anyhow!("未知 BDD 排序：{name}"))?;
                bdd_order_used = Some(order.as_str());
                let vars = (order != VarOrder::Appearance).then(|| order_variables(&a, &b, order));
                let request = BddRequest {
                    order: vars.as_deref(),
                    limits,
                };
                equivalent_bdd_with(&a, &b, alg, &binary, &request)?
            }
        };
        (verdict.result, "bdd", Some(verdict.nodes))
    } else if symbolic {
        let method = if binary.is_empty() {
            "sat"
        } else {
            "sat-binary"
        };
        (
            equivalent_symbolic_with(&a, &b, alg, &binary)?,
            method,
            None,
        )
    } else {
        if vars.len() > max_vars {
            bail!(
                "变量数 {} 超过穷举上限 {}；用 --symbolic 或 --bdd 走符号化判定",
                vars.len(),
                max_vars
            );
        }
        let limits = Limits {
            max_vars,
            max_rows: usize::MAX,
            ..Limits::default()
        };
        (equivalent_within(&a, &b, alg, limits)?, "exhaustive", None)
    };
    match format {
        Format::Json => {
            let mut obj = serde_json::json!({
                "equivalent": result.equivalent,
                "algebra": alg.as_str(),
                "variable_count": result.variable_count,
                "method": method,
            });
            if let Some(size) = &stats_value {
                obj["encoding"] = serde_json::json!({
                    "variables": size.variables,
                    "clauses": size.clauses,
                });
            }
            if let Some(nodes) = bdd_nodes.filter(|_| stats) {
                obj["bdd_nodes"] = serde_json::json!(nodes);
            }
            if let Some(order) = bdd_order_used {
                obj["bdd_order"] = serde_json::json!(order);
            }
            if let Some(asg) = &result.counterexample_assignment {
                let mut pairs = serde_json::Map::new();
                for name in asg.variables() {
                    pairs.insert(
                        name.clone(),
                        serde_json::Value::String(
                            asg.get(&name)
                                .unwrap_or(Ternary::Unknown)
                                .as_char()
                                .to_string(),
                        ),
                    );
                }
                obj["counterexample"] = serde_json::Value::Object(pairs);
                if let Some(ce) = result.counterexample {
                    obj["counterexample_index"] = serde_json::json!(ce);
                }
            }
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            if let Some(size) = &stats_value {
                let (variables, clauses) = (size.variables, size.clauses);
                trit_log::error!("编码：variables={variables} clauses={clauses}");
            }
            if let (Some(nodes), true) = (bdd_nodes, stats) {
                match bdd_order_used {
                    Some(order) => trit_log::error!("BDD：nodes={nodes} order={order}"),
                    None => trit_log::error!("BDD：nodes={nodes}"),
                }
            }
            if result.equivalent {
                println!("等价：{left} ≡ {right}  [{alg}]");
            } else if let Some(asg) = &result.counterexample_assignment {
                let detail = asg
                    .variables()
                    .iter()
                    .map(|name| {
                        format!(
                            "{name}={}",
                            asg.get(name).unwrap_or(Ternary::Unknown).as_char()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("不等价：{left} ≢ {right}  [{alg}]");
                if let Some(index) = result.counterexample {
                    println!("反例：{detail}（赋值编号 {index}）");
                } else {
                    println!("反例：{detail}（变量过多，无编号）");
                }
            }
        }
    }
    if result.equivalent {
        Ok(())
    } else {
        std::process::exit(2)
    }
}
