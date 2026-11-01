//! 三值逻辑分析器 —— 命令行入口。
//!
//! 子命令：
//!
//! - `eval`：在给定赋值下求值
//! - `table`：生成完整真值表
//! - `equiv`：判定两个表达式是否逻辑等价
//! - `quantify`：对三值变量做存在/全称量化
//! - `simplify`：化简表达式
//! - `circuit`：分析三值门级网表（`--assign` 对一条含 X 的输入向量做单遍传播）
//! - `belnap`：Belnap 四值（`T`/`F`/`B`/`N`）的 opt-in 求值、真值表与等价
//! - `repl`：交互式会话

#![forbid(unsafe_code)]

mod commands;
mod output;
mod timeout;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use trit_circuit::circuit::Circuit;
use trit_circuit::netlist::parse_netlist;
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser as ExprParser;
use trit_core::value::Ternary;
use trit_eval::eval::Assignment;

use crate::output::Format;

/// 三值逻辑分析器。
#[derive(Debug, Parser)]
#[command(
    name = "trit",
    version,
    about = "三值逻辑分析器：Kleene / Łukasiewicz / Bochvar 三套语义的求值、真值表、等价判定与化简",
    long_about = None
)]
struct Cli {
    /// 使用的三值语义：`kleene`（默认）、`lukasiewicz`、`bochvar`。
    #[arg(
        long,
        short = 'a',
        global = true,
        default_value = "kleene",
        value_name = "ALG"
    )]
    algebra: String,

    /// 硬超时（毫秒）：超时后终止整个子进程，退出码 124。
    #[arg(long, global = true, value_name = "MS")]
    timeout_ms: Option<u64>,

    /// 子命令。
    #[command(subcommand)]
    command: Commands,
}

/// CLI 子命令。
#[derive(Debug, Subcommand)]
enum Commands {
    /// 系统容量观测与准入判决（M5.1）：这台机器的显存够不够这个负载。
    ///
    /// 只观测与判决，不拦截；`--require` 时 WARN/REJECT/读数不可用 一律退出码 1。
    Capacity {
        /// 负载峰值需求（MiB）。省略则只打印快照，不做判决。
        #[arg(long, value_name = "MIB")]
        need_mb: Option<u64>,
        /// 竞争余量倍数（≥1.0；小于 1 会被夹到 1.0）。
        #[arg(long, default_value_t = 2.0)]
        headroom: f32,
        /// 严格模式：非 OK 即退出码 1（给 CI / 启动脚本用）。
        #[arg(long)]
        require: bool,
    },
    /// 先算账再启动子进程（M5.2 自壳）：判决不过时子进程**不启动**。
    ///
    /// 退出码：子进程的退出码原样透传；**3 = 容量拒绝（子进程未运行）**。
    Exec {
        /// 负载峰值需求（MiB）。省略则不做判决，直接执行（会提示）。
        #[arg(long, value_name = "MIB")]
        need_mb: Option<u64>,
        /// 竞争余量倍数（≥1.0；小于 1 会被夹到 1.0）。
        #[arg(long, default_value_t = 2.0)]
        headroom: f32,
        /// 越过判决强制启动（逃生口；读数照常打印）。
        #[arg(long)]
        force: bool,
        /// 严格模式：读数不可用也拒跑（fail-closed）。
        #[arg(long)]
        strict: bool,
        /// 要执行的命令与参数：`trit exec --need-mb 4096 -- prog arg...`。
        #[arg(last = true, required = true, value_name = "CMD")]
        argv: Vec<String>,
    },
    /// 在指定赋值下求值表达式。
    Eval {
        /// 待求值的表达式，例如 `a & b | !c`。
        expression: String,
        /// 变量赋值，形如 `a=1,b=X`。
        #[arg(long, short = 's', value_name = "ASSIG", default_value = "")]
        assign: String,
    },
    /// 生成表达式的完整真值表。
    Table {
        /// 待求值的表达式。
        expression: String,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    /// 判定两个表达式是否逻辑等价。
    Equiv {
        /// 第一个表达式。
        left: String,
        /// 第二个表达式。
        right: String,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
        /// 用 SAT 符号化判定（dual-rail），不做 3^n 穷举。
        #[arg(long)]
        symbolic: bool,
        /// 用 BDD（canonical）判定；与 `--symbolic` 互斥。
        #[arg(long)]
        bdd: bool,
        /// BDD 变量排序：appearance（默认）/ reverse / frequency / frequency-asc / best。
        #[arg(long, value_name = "ORDER", default_value = "appearance")]
        bdd_order: String,
        /// BDD 单图节点预算（0 表示不设上限）。
        #[arg(long, value_name = "N", default_value_t = 1_000_000)]
        bdd_max_nodes: usize,
        /// 穷举判定的变量数上限；超过则报错并提示 `--symbolic`。
        #[arg(long, default_value_t = 18)]
        max_vars: usize,
        /// 把列出的变量当作已知二值（逗号分隔）；其子图走经典 CNF。
        #[arg(long, value_name = "VARS")]
        binary: Option<String>,
        /// 把所有变量当作已知二值。
        #[arg(long)]
        binary_all: bool,
        /// 打印 SAT 编码规模（仅 `--symbolic`）。
        #[arg(long)]
        stats: bool,
        /// 输出最小反例：只保留区分两边所必需的变量（仅 `--symbolic`）。
        #[arg(long)]
        minimize: bool,
    },
    /// 对三值变量做存在/全称量化（BDD）。
    Quantify {
        /// 待量化的表达式，例如 `a & b`。
        expression: String,
        /// 要量化的变量（逗号分隔）；省略则量化全部变量。
        #[arg(long, value_name = "VARS")]
        vars: Option<String>,
        /// 量化方向：`exists`（默认）或 `forall`。
        #[arg(long, default_value = "exists", value_name = "KIND")]
        kind: String,
        /// 把列出的变量当作已知二值（逗号分隔），其量化域为 {0,1}。
        #[arg(long, value_name = "VARS")]
        binary: Option<String>,
        /// 把所有变量当作已知二值。
        #[arg(long)]
        binary_all: bool,
        /// BDD 变量排序：appearance（默认）/ reverse / frequency / frequency-asc / best。
        #[arg(long, value_name = "ORDER", default_value = "appearance")]
        bdd_order: String,
        /// BDD 单图节点预算（0 表示不设上限）。
        #[arg(long, value_name = "N", default_value_t = 1_000_000)]
        bdd_max_nodes: usize,
        /// JSON 输出。
        #[arg(long)]
        json: bool,
    },
    /// 化简表达式。
    Simplify {
        /// 待化简的表达式。
        expression: String,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
        /// 是否把 `a ⇒ b` 展开为 `¬a ∨ b`（仅 Kleene 语义合法）。
        #[arg(long)]
        expand_implies: bool,
    },
    /// 分析三值门级网表。
    /// 三值拓扑：从起点传播可达性，或判定「通过 / 未定 / 否定」并给证书。
    Topology {
        /// 拓扑 JSON：{"nodes": N, "origin": 0, "edges": [{"from":..,"to":..,"value":"X"}]}。
        path: std::path::PathBuf,
        /// 覆盖 JSON 里的起点。
        #[arg(long, value_name = "NODE")]
        origin: Option<usize>,
        /// 只判定这一个终点，并输出证书路径与需要补全的 Unknown 边。
        #[arg(long = "to", value_name = "NODE")]
        target: Option<usize>,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    Circuit {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 单向量传播：`a=1,b=X`（给全所有主输入）。
        #[arg(long, value_name = "ASSIGN")]
        assign: Option<String>,
        /// 配合 `--assign`：用 SAT 区分真 X 与悲观 X（假 X）。
        #[arg(long)]
        pessimism: bool,
        /// 配合 `--assign`：枚举所有 X 补全做相关性感知的精确传播。
        #[arg(long)]
        exact: bool,
        /// 精确传播的并行度：0 表示自动（每线程至少 1024 个完成）。
        #[arg(long, value_name = "N", default_value_t = 0)]
        jobs: usize,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    /// 生成 stuck-at 测试向量并报告覆盖率。
    Atpg {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
        /// 用 SAT 逐故障求解（支持 ≤64 主输入；不可测由 UNSAT 证明）。
        #[arg(long)]
        sat: bool,
        /// 并行工作线程数（0 = 自动，1 = 串行）。
        #[arg(long, value_name = "N", default_value_t = 0)]
        jobs: usize,
        /// 复用一份 CNF、用 selector 逐故障求解（适合大规模；见证向量可能与默认路径不同）。
        #[arg(long, requires = "sat")]
        shared_cnf: bool,
    },
    /// 桥接故障（wired-AND / wired-OR）可测性与测试向量。
    Bridge {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 桥接对 JSON 文件；省略则枚举主输入/门输出之间的全部配对。
        #[arg(long, value_name = "FILE")]
        pairs: Option<std::path::PathBuf>,
        /// 自动枚举的桥接上限（每个网络对展开 AND/OR 两种）。
        #[arg(long, value_name = "N", default_value_t = 128)]
        max_pairs: usize,
        /// 用 SAT 逐桥接求解（支持 ≤64 主输入）。
        #[arg(long)]
        sat: bool,
        /// 并行工作线程数（0 = 自动，1 = 串行）。
        #[arg(long, value_name = "N", default_value_t = 0)]
        jobs: usize,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    /// 组合时序估算（单位延迟 STA-lite）。
    Timing {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 时钟周期（周期数）；省略时取关键路径，即 slack 全为 0。
        #[arg(long, value_name = "CYCLES")]
        clock: Option<u32>,
        /// 门延迟（单位周期）；被 --library 覆盖。
        #[arg(long, default_value_t = 1)]
        delay: u32,
        /// 每个扇出（门输入引用）追加的延迟；0 = 不看负载。
        #[arg(long, value_name = "N")]
        load_delay: Option<u32>,
        /// 时序库 JSON（`default_delay` + `delays`），形如 `{"default_delay":1,"delays":{"AND":2}}`。
        #[arg(long, value_name = "FILE")]
        library: Option<std::path::PathBuf>,
        /// SDF 延迟文件（保守子集：每类 celltype 取最坏 IOPATH，向上取整；TIMESCALE 不换算）。
        #[arg(long, value_name = "FILE", conflicts_with = "library")]
        sdf: Option<std::path::PathBuf>,
        /// I/O 时序约束 JSON（`input_arrival` / `output_required`）。
        #[arg(long, value_name = "FILE")]
        constraints: Option<std::path::PathBuf>,
        /// 列出最慢的 N 条路径（0 = 不列）。
        #[arg(long, default_value_t = 0)]
        paths: usize,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    /// 技术映射：把组合网表降到 NAND（+ 常量）。
    Map {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 输出格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        to: String,
        /// 单元库 JSON（如 `{"AND":1,"NAND":2}`）；给出时按延迟选实现。
        #[arg(long, value_name = "JSON")]
        cells: Option<std::path::PathBuf>,
        /// 面积优先（单元 area，未声明按 1），平手再看延迟；需配合 `--cells`。
        #[arg(long)]
        area_first: bool,
        /// 每个扇出（门输入引用）追加的延迟；0 = 不看负载。
        #[arg(long, default_value_t = 0, value_name = "N")]
        load_delay: u32,
    },
    /// 设计检查：结构 + 时序 + X 传播 + ATPG 覆盖率（一条命令）。
    Check {
        /// 网表文件路径。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        from: String,
        /// 时钟周期；省略时取关键路径。
        #[arg(long, value_name = "CYCLES")]
        clock: Option<u32>,
        /// 门延迟（单位周期）；被 --library 覆盖。
        #[arg(long, default_value_t = 1)]
        delay: u32,
        /// 每个扇出（门输入引用）追加的延迟；0 = 不看负载。
        #[arg(long, value_name = "N")]
        load_delay: Option<u32>,
        /// 时序库 JSON。
        #[arg(long, value_name = "FILE")]
        library: Option<std::path::PathBuf>,
        /// SDF 延迟文件（保守子集：每类 celltype 取最坏 IOPATH，向上取整）。
        #[arg(long, value_name = "FILE", conflicts_with = "library")]
        sdf: Option<std::path::PathBuf>,
        /// I/O 时序约束 JSON（`input_arrival` / `output_required`）。
        #[arg(long, value_name = "FILE")]
        constraints: Option<std::path::PathBuf>,
        /// ATPG 覆盖率下限（0..=1）。
        #[arg(long, default_value_t = 0.0)]
        min_coverage: f64,
        /// 输出格式。
        #[arg(long, short = 'f', value_name = "FORMAT", default_value = "table")]
        format: Format,
    },
    /// 静态三值 DAG 调度：依赖可为 True / False / Unknown。
    Schedule {
        /// 调度输入 JSON（`workers` + `tasks`）。
        path: std::path::PathBuf,
        /// 覆盖 JSON 里的 worker 数。
        #[arg(long, value_name = "N")]
        workers: Option<usize>,
        /// 输出格式。
        #[arg(long, short = 'f', default_value = "table")]
        format: Format,
    },
    /// 网表格式转换：text <-> json。
    Netlist {
        /// 输入文件。
        path: std::path::PathBuf,
        /// 输入格式：`text`（默认）/ `json` / `verilog` / `blif` / `aiger`。
        #[arg(long, default_value = "text")]
        from: String,
        /// 输出格式：`json`（默认）/ `text` / `verilog`。
        #[arg(long, default_value = "json")]
        to: String,
    },
    /// 验证三值决策表策略文件。
    Verify {
        /// 策略 JSON 路径。
        path: std::path::PathBuf,
        /// 引擎：sat（默认）或 bdd。
        #[arg(long, default_value = "sat")]
        engine: String,
        /// BDD 变量排序（仅 --engine bdd）：appearance（默认）/ reverse / frequency / frequency-asc / best。
        #[arg(long, value_name = "ORDER", default_value = "appearance")]
        bdd_order: String,
        /// BDD 单图节点预算（仅 --engine bdd；0 表示不设上限）。
        #[arg(long, value_name = "N", default_value_t = 1_000_000)]
        bdd_max_nodes: usize,
        /// 并行检查断言的线程数（0 = 自动，1 = 串行）。
        #[arg(long, value_name = "N", default_value_t = 0)]
        jobs: usize,
        /// JSON 输出。
        #[arg(long)]
        json: bool,
    },
    /// Belnap 四值（opt-in）：T / F / B / N。
    Belnap {
        /// 子命令。
        #[command(subcommand)]
        cmd: commands::belnap::BelnapCmd,
    },
    /// 交互式会话：逐行输入表达式，直接看到真值表。
    Repl {
        /// 启动时使用的语义。
        #[arg(long, value_name = "ALG")]
        algebra: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(timeout_ms) = cli.timeout_ms {
        if timeout_ms == 0 {
            bail!("--timeout-ms 必须大于 0");
        }
        if std::env::var_os("TRIT_TIMEOUT_CHILD").is_none() {
            timeout::run_with_timeout(timeout_ms)?;
            return Ok(());
        }
    }
    match cli.command {
        Commands::Capacity {
            need_mb,
            headroom,
            require,
        } => commands::capacity::cmd_capacity(need_mb, headroom, require),
        Commands::Exec {
            need_mb,
            headroom,
            force,
            strict,
            argv,
        } => commands::exec::cmd_exec(need_mb, headroom, force, strict, &argv),
        Commands::Eval { expression, assign } => {
            commands::eval::cmd_eval(&expression, &assign, &cli.algebra)
        }
        Commands::Table { expression, format } => {
            commands::table::cmd_table(&expression, format, &cli.algebra)
        }
        Commands::Equiv {
            left,
            right,
            format,
            symbolic,
            bdd,
            bdd_order,
            bdd_max_nodes,
            max_vars,
            binary,
            binary_all,
            stats,
            minimize,
        } => commands::equiv::cmd_equiv(
            &left,
            &right,
            format,
            &cli.algebra,
            commands::equiv::EquivOptions {
                symbolic,
                bdd,
                bdd_order,
                bdd_max_nodes,
                max_vars,
                binary,
                binary_all,
                stats,
                minimize,
            },
        ),
        Commands::Quantify {
            expression,
            vars,
            kind,
            binary,
            binary_all,
            bdd_order,
            bdd_max_nodes,
            json,
        } => commands::quantify::cmd_quantify(
            &expression,
            &cli.algebra,
            commands::quantify::QuantifyOptions {
                vars,
                kind: &kind,
                binary,
                binary_all,
                bdd_order,
                bdd_max_nodes,
                json,
            },
        ),
        Commands::Simplify {
            expression,
            format,
            expand_implies,
        } => commands::simplify::cmd_simplify(&expression, format, expand_implies, &cli.algebra),
        Commands::Circuit {
            path,
            assign,
            pessimism,
            exact,
            jobs,
            from,
            format,
        } => commands::circuit::cmd_circuit(
            &path,
            format,
            &cli.algebra,
            &commands::circuit::PropagationOptions {
                assign: assign.as_deref(),
                pessimism,
                exact,
                jobs,
            },
            &from,
        ),
        Commands::Topology {
            path,
            origin,
            target,
            format,
        } => commands::topology::cmd_topology(&path, format, origin, target),
        Commands::Atpg {
            path,
            from,
            format,
            sat,
            jobs,
            shared_cnf,
        } => commands::atpg::cmd_atpg(&path, format, &cli.algebra, &from, sat, jobs, shared_cnf),
        Commands::Bridge {
            path,
            from,
            pairs,
            max_pairs,
            sat,
            jobs,
            format,
        } => commands::bridge::cmd_bridge(
            &path,
            format,
            &cli.algebra,
            &from,
            pairs.as_deref(),
            max_pairs,
            sat,
            jobs,
        ),
        Commands::Timing {
            path,
            from,
            clock,
            delay,
            load_delay,
            library,
            sdf,
            constraints,
            paths,
            format,
        } => commands::timing::cmd_timing(
            &path,
            &from,
            clock,
            delay,
            load_delay,
            library.as_deref(),
            sdf.as_deref(),
            constraints.as_deref(),
            paths,
            format,
        ),
        Commands::Map {
            path,
            from,
            to,
            cells,
            area_first,
            load_delay,
        } => commands::map::cmd_map(&path, &from, &to, cells.as_deref(), area_first, load_delay),
        Commands::Check {
            path,
            from,
            clock,
            delay,
            load_delay,
            library,
            sdf,
            constraints,
            min_coverage,
            format,
        } => commands::check::cmd_check(
            &path,
            &from,
            clock,
            delay,
            load_delay,
            library.as_deref(),
            sdf.as_deref(),
            constraints.as_deref(),
            min_coverage,
            format,
            &cli.algebra,
        ),
        Commands::Schedule {
            path,
            workers,
            format,
        } => commands::schedule::cmd_schedule(&path, workers, format),
        Commands::Netlist { path, from, to } => commands::netlist::cmd_netlist(&path, &from, &to),
        Commands::Verify {
            path,
            engine,
            bdd_order,
            bdd_max_nodes,
            jobs,
            json,
        } => commands::verify::cmd_verify(&path, &engine, &bdd_order, bdd_max_nodes, jobs, json),
        Commands::Belnap { cmd } => commands::belnap::cmd_belnap(cmd),
        Commands::Repl { algebra } => {
            let alg_name = algebra.unwrap_or(cli.algebra);
            commands::repl::cmd_repl(&alg_name)
        }
    }
}

/// 解析语义名称。
pub(crate) fn resolve_algebra(name: &str) -> Result<Algebra> {
    Algebra::parse(name)
        .with_context(|| format!("未知的三值语义 {name:?}，可选：kleene / lukasiewicz / bochvar"))
}

/// 解析表达式。
pub(crate) fn parse_expr(src: &str) -> Result<Expr> {
    ExprParser::parse(src).with_context(|| format!("表达式解析失败：{src}"))
}

/// 解析赋值串 `a=1,b=X`。
pub(crate) fn parse_assignment(src: &str) -> Result<Assignment> {
    let mut asg = Assignment::new();
    for part in src.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            bail!("赋值格式应为 name=value，收到 {part:?}");
        };
        let name = name.trim();
        let value = value.trim();
        let v = match value {
            "0" | "false" | "False" | "f" => Ternary::False,
            "1" | "true" | "True" | "t" => Ternary::True,
            "X" | "x" | "?" | "unknown" | "Unknown" => Ternary::Unknown,
            other => bail!("未知取值 {other:?}，只能是 0 / 1 / X"),
        };
        asg.set(name.to_string(), v);
    }
    Ok(asg)
}

/// `eval` 子命令。
/// `table` 子命令。
/// `equiv` 子命令。
pub(crate) fn parse_var_list(src: &str, all: &[String], flag: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for name in src.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if !all.iter().any(|v| v == name) {
            bail!("{flag} 变量 {name} 不在表达式里");
        }
        if !names.iter().any(|v| v == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

pub(crate) fn assignment_text(asg: &Assignment) -> String {
    if asg.is_empty() {
        return "（空）".to_string();
    }
    asg.variables()
        .iter()
        .map(|name| {
            format!(
                "{name}={}",
                asg.get(name).unwrap_or(Ternary::Unknown).as_char()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn assignment_json(asg: &Assignment) -> serde_json::Value {
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
    serde_json::Value::Object(pairs)
}

/// `circuit` 子命令：读取网表文件并分析。
///
/// 网表文件格式（每行一条，空行与 `#` 注释忽略）：
///
/// ```text
/// input a
/// input b
/// output out
/// gate AND a b out
/// ```
pub(crate) fn timing_model(
    library: Option<&std::path::Path>,
    sdf: Option<&std::path::Path>,
    delay: u32,
    load_delay: Option<u32>,
    circuit: &Circuit,
) -> Result<trit_circuit::timing::TimingModel> {
    let mut model = match (library, sdf) {
        (Some(_), Some(_)) => {
            anyhow::bail!("--library 与 --sdf 只能选一个");
        }
        (Some(library), None) => {
            let text = std::fs::read_to_string(library)
                .with_context(|| format!("无法读取时序库 {}", library.display()))?;
            trit_circuit::timing::TimingModel::from_json(&text)
                .with_context(|| format!("时序库解析失败：{}", library.display()))?
        }
        (None, Some(sdf)) => {
            let text = std::fs::read_to_string(sdf)
                .with_context(|| format!("无法读取 SDF {}", sdf.display()))?;
            let mut model = trit_circuit::sdf::timing_model_from_sdf_for(&text, circuit)
                .with_context(|| format!("SDF 解析失败：{}", sdf.display()))?;
            model.default_delay = delay;
            model
        }
        (None, None) => trit_circuit::timing::TimingModel {
            default_delay: delay,
            ..Default::default()
        },
    };
    if let Some(load) = load_delay {
        model.load_delay = load;
    }
    Ok(model)
}

pub(crate) fn load_timing_constraints(
    path: Option<&std::path::Path>,
) -> Result<trit_circuit::timing::TimingConstraints> {
    let Some(path) = path else {
        return Ok(Default::default());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取时序约束 {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("时序约束 JSON 无效：{}", path.display()))
}

pub(crate) fn load_circuit(path: &std::path::Path, from: &str) -> Result<Circuit> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取网表文件 {}", path.display()))?;
    match from {
        "text" => parse_netlist(&text).with_context(|| format!("网表解析失败：{}", path.display())),
        "json" => trit_circuit::netlist::from_json(&text)
            .with_context(|| format!("JSON 解析失败：{}", path.display())),
        "verilog" => trit_circuit::verilog::parse_verilog(&text)
            .with_context(|| format!("Verilog 解析失败：{}", path.display())),
        "blif" => trit_circuit::parse_blif(&text)
            .with_context(|| format!("BLIF 解析失败：{}", path.display())),
        "aiger" => trit_circuit::parse_aiger(&text)
            .with_context(|| format!("AIGER 解析失败：{}", path.display())),
        other => bail!("未知输入格式 {other:?}，可选 text / json / verilog / blif / aiger"),
    }
}
