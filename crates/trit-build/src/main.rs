//! `trit-build`：三进制构建工具（TTIR-0 → M3）。
//!
//! 做的事情（全部可复算）：
//! 1. 读 `.tri` 表达式或 Verilog 子集（组合逻辑）；
//! 2. 解析 → 化简 → e-graph **饱和重写** → **穷举复核等价**
//!    （不等价就红，绝不产出"优化后"的错件）；
//! 3. 生成产物：
//!    - `expr.txt`：原始/化简后表达式与判定信息；
//!    - `truth.csv`：完整真值表（3^n 行）；
//!    - `truth.tritbin`：`TRB2` v3（5 trit/字节、GF(3) CRC-3、变量名表）；
//!    - `lookup.c`：主机侧查表函数（**执行仍交给 C/Rust 工具链**，这是刻意的边界）；
//!    - `design.v`：双导轨 Verilog（K3/Ł3/B3）；
//!    - `design_tb.v` + `expected.mem`：`--verify-hdl` 的 iverilog 自检；
//!    - `truth.wgsl` + `truth.gpu.bin`：`--emit-gpu` 的 WGSL 查表内核；
//! 4. 写 `build.json`（输入/选项/产物哈希/阶段），并按内容哈希做增量缓存。
//!
//! 为什么不是"重写 LLVM"：LLVM 的职责是**二进制**后端的寄存器分配/调度/链接；
//! 三进制真正独有的是语义（K3/Ł3/B3）、X 的传播、基 3 打包与三值校验。
//! 这一层自己做，机器码/链接/OS 仍交给 LLVM/Rust——见 `docs/TERNARY-TOOLCHAIN.md`。
//!
//! 用法：
//! ```text
//! trit-build <source.tri|design.v> [--out DIR] [--algebra k3] [--force] [--check]
//!            [--decode BLOB] [--emit-verilog] [--verify-hdl] [--emit-gpu]
//!            [--cost nodes|gates] [--threads N]
//! ```
//!
//! 独立 crate（`publish = false`）而不是 `trit-cli` 的子命令：`trit-cli` 是可发布包，
//! 发布图门禁禁止它依赖实验层（`trit-codec`）。

use std::fs;
use std::path::{Path, PathBuf};

mod bitplan;
mod egraph;
mod gpu;
mod hdl;
mod parallel;
mod rail;
mod schedule;
mod trb;
mod verilog;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use parallel::{parse_prefer, run_parallel};
use trit_core::Algebra;
use trit_core::limits::Limits;
use trit_core::value::Ternary;
use trit_eval::eval::equivalent_within;
use trit_eval::simplify::{SimplifyOptions, simplify_with};

use egraph::{CostModel, RewriteOptions};

use trit_core::Parser as ExprParser;

fn parse_expr(src: &str) -> Result<trit_core::Expr> {
    ExprParser::parse(src).with_context(|| format!("表达式解析失败：{src}"))
}

fn resolve_algebra(name: &str) -> Result<Algebra> {
    Algebra::parse(name)
        .with_context(|| format!("未知的三值语义 {name:?}，可选：kleene / lukasiewicz / bochvar"))
}

/// 构建工具版本（参与缓存键；产物格式变了就 bump）。
const BUILD_VERSION: u32 = 4;
/// 允许穷举的变量上限（3^12 = 531,441）。
const MAX_BUILD_VARS: u32 = 12;

/// 单次构建的全部选项（`build_source` 的入参）。
pub(crate) struct BuildOptions<'a> {
    pub algebra: Algebra,
    pub force: bool,
    pub output: Option<&'a str>,
    pub expect: Option<&'a str>,
    pub emit_verilog: bool,
    pub verify_hdl: bool,
    pub emit_gpu: bool,
    pub threads: usize,
    pub egraph: RewriteOptions,
}

/// 缓存键：源文本 + 一切会影响产物的选项（线程数不影响结果，不入键）。
fn cache_key(source_text: &str, verilog: bool, options: &BuildOptions<'_>) -> String {
    hex(hash64(
        format!(
            "trit-build/{BUILD_VERSION}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            alg_name(options.algebra),
            options.output.unwrap_or("-"),
            options.expect.unwrap_or("-"),
            options.emit_verilog,
            options.verify_hdl,
            options.emit_gpu,
            verilog,
            options.egraph.max_rounds,
            options.egraph.node_limit,
            options.egraph.cost.name(),
            source_text
        )
        .as_bytes(),
    ))
}

/// 解析 `.tri`：忽略空行与 `#` 注释，必须恰好一行表达式。
fn parse_source(text: &str, path: &Path) -> Result<String> {
    let mut expression: Option<String> = None;
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if expression.is_some() {
            bail!(
                "{}:{}: 一个 .tri 文件只能有一条表达式（多行表达式暂不支持）",
                path.display(),
                line_no + 1
            );
        }
        expression = Some(line.to_string());
    }
    expression.with_context(|| format!("{}: 文件里没有表达式", path.display()))
}

fn hash64(bytes: &[u8]) -> u64 {
    // FNV-1a：稳定、无依赖。只作缓存键/完整性对照，不是密码学散列。
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

fn trit_i8(value: Ternary) -> i8 {
    match value {
        Ternary::True => 1,
        Ternary::Unknown => 0,
        Ternary::False => -1,
    }
}

fn alg_name(alg: Algebra) -> &'static str {
    match alg {
        Algebra::Kleene => "kleene",
        Algebra::Lukasiewicz => "lukasiewicz",
        Algebra::Bochvar => "bochvar",
    }
}

/// 逐赋值穷举：返回按 base-3 编码排好序的输出序列（code 0..3^n）。
///
/// 生产路径走 [`bitplan::table`]（位并行）；这份标量实现保留为**对拍基准**，
/// 位并行的测试拿它当 oracle，两边逐 trit 必须相同。
#[cfg(test)]
fn enumerate(expr: &trit_core::Expr, vars: &[String], alg: Algebra) -> Result<Vec<Ternary>> {
    use trit_eval::eval::{Assignment, eval};
    let count = 3u64
        .checked_pow(vars.len() as u32)
        .context("变量太多，3^n 溢出")?;
    let mut out = Vec::with_capacity(count as usize);
    for code in 0..count {
        let mut rest = code;
        let mut assignment = Assignment::new();
        for var in vars {
            let digit = rest % 3;
            rest /= 3;
            assignment.set(
                var.clone(),
                match digit {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                },
            );
        }
        out.push(eval(expr, &assignment, alg));
    }
    Ok(out)
}

fn truth_csv(vars: &[String], values: &[Ternary]) -> String {
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
    trb::encode_named(values, alg, names)
}

fn lookup_c(vars: &[String], values: &[Ternary], alg: Algebra) -> String {
    let mut text = String::new();
    text.push_str("/* generated by trit build; 语义 = ");
    text.push_str(alg_name(alg));
    text.push_str(", 变量顺序 = ");
    text.push_str(&vars.join(","));
    text.push_str(" */\n");
    text.push_str("#include <stdint.h>\n\n");
    text.push_str("static const int8_t TRIT_TABLE[");
    text.push_str(&values.len().to_string());
    text.push_str("] = {\n");
    for chunk in values.chunks(12) {
        text.push_str("   ");
        for value in chunk {
            text.push_str(&format!(" {:>2},", trit_i8(*value)));
        }
        text.push('\n');
    }
    text.push_str("};\n\n");
    text.push_str(
        "int8_t trit_lookup(const int8_t *vars, unsigned nvars) {\n\
         \x20   unsigned code = 0;\n\
         \x20   for (unsigned i = 0; i < nvars; i++) code = code * 3u + (unsigned)(vars[i] + 1);\n\
         \x20   return TRIT_TABLE[code];\n\
         }\n",
    );
    text
}

fn write_artifact(path: &Path, bytes: &[u8]) -> Result<String> {
    fs::write(path, bytes).with_context(|| format!("写 {} 失败", path.display()))?;
    Ok(hex(hash64(bytes)))
}

/// 读缓存清单；产物哈希全部对得上就返回清单（可跳过构建）。
fn cache_hit(manifest_path: &Path, out_dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(manifest_path).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    let outputs = manifest.get("outputs")?.as_object()?;
    for (name, info) in outputs {
        let path = out_dir.join(name);
        let bytes = fs::read(&path).ok()?;
        if hash64(&bytes) != u64::from_str_radix(info.get("hash")?.as_str()?, 16).ok()? {
            return None;
        }
    }
    Some(manifest)
}

fn is_verilog(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("v") | Some("sv")
    )
}

/// 写四类产物（两条链路共用）。
fn emit_artifacts(
    out_dir: &Path,
    header: &str,
    vars: &[String],
    values: &[Ternary],
    algebra: Algebra,
    rail: Option<&str>,
) -> Result<serde_json::Map<String, Value>> {
    let csv = truth_csv(vars, values);
    let blob = tritbin(values, algebra, vars)?;
    let c_source = lookup_c(vars, values, algebra);
    let mut outputs = serde_json::Map::new();
    let mut artifacts = vec![
        ("expr.txt", header.as_bytes().to_vec()),
        ("truth.csv", csv.as_bytes().to_vec()),
        ("truth.tritbin", blob),
        ("lookup.c", c_source.as_bytes().to_vec()),
    ];
    if let Some(module) = rail {
        artifacts.push(("design.v", module.as_bytes().to_vec()));
    }
    for (name, bytes) in artifacts {
        let hash = write_artifact(&out_dir.join(name), &bytes)?;
        outputs.insert(
            name.to_string(),
            json!({"bytes": bytes.len(), "hash": hash}),
        );
    }
    Ok(outputs)
}

/// 构建单个源：`.tri`（表达式）或 `.v`（Verilog 子集，M1）。
pub(crate) fn build_source(
    source: &Path,
    out_dir: &Path,
    options: BuildOptions<'_>,
) -> Result<Value> {
    let algebra = options.algebra;
    let force = options.force;
    let output = options.output;
    let expect = options.expect;
    let emit_verilog = options.emit_verilog;
    let verify_hdl = options.verify_hdl;
    let emit_gpu = options.emit_gpu;
    let threads = options.threads;
    let source_text =
        fs::read_to_string(source).with_context(|| format!("读 {} 失败", source.display()))?;
    let verilog = is_verilog(source);
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建输出目录 {} 失败", out_dir.display()))?;
    let cache_dir = out_dir.join(".trit-cache");
    let key = cache_key(&source_text, verilog, &options);
    let manifest_path = cache_dir.join(format!("{key}.json"));

    if !force && let Some(manifest) = cache_hit(&manifest_path, out_dir) {
        let mut summary = manifest;
        summary["cached"] = json!(true);
        return Ok(summary);
    }

    let mut rewritten_expr: Option<trit_core::Expr> = None;
    let (vars, values, header, mut verified) = if verilog {
        let design = verilog::load(source, output, algebra, MAX_BUILD_VARS)?;
        if let Some(expr_text) = expect {
            let expr = parse_expr(expr_text)?;
            verilog::cross_check(&design, &expr, algebra)?;
        }
        let header = format!(
            "# trit-build v{BUILD_VERSION}\n# source: verilog {}\n# module: {}\n# algebra: {}\n# inputs: {}\n# output: {}\n# verified: {}",
            source.display(),
            design.name,
            alg_name(algebra),
            design.inputs.join(","),
            design.output,
            if expect.is_some() {
                "cross-checked against .tri expression"
            } else {
                "exhaustive propagate"
            }
        );
        (
            design.inputs,
            design.table,
            header,
            json!({"exact": true, "crossCheckedWith": expect}),
        )
    } else {
        let expression_text = parse_source(&source_text, source)?;
        let expr = parse_expr(&expression_text)?;
        trit_core::limits::check_expr(&expr, Limits::default())
            .map_err(|error| anyhow::anyhow!("表达式超出预算：{error}"))?;
        let vars = expr.variables();
        if vars.len() as u32 > MAX_BUILD_VARS {
            bail!(
                "变量数 {} 超过构建穷举上限 {MAX_BUILD_VARS}（3^n 会爆）",
                vars.len()
            );
        }
        // 阶段 1：化简；阶段 2：e-graph 饱和重写（规则已逐条自证）；阶段 3：整链穷举复核。
        let simplified = simplify_with(&expr, algebra, SimplifyOptions::default());
        let egraph::RewriteReport {
            expr: rewritten,
            rules,
            rounds,
            e_nodes,
            saturated,
            cost,
            nodes,
        } = egraph::rewrite_with(&simplified, algebra, options.egraph);
        // 自证只覆盖「单条规则」；整条链是否有实现 bug，仍由穷举复核兜底。
        let equivalent = equivalent_within(&expr, &rewritten, algebra, Limits::default())
            .map_err(|error| anyhow::anyhow!("等价复核超出预算：{error}"))?;
        if !equivalent.equivalent {
            bail!(
                "化简/重写结果与原文不等价（内部错误）：{} -> {}",
                expr.to_infix(),
                rewritten.to_infix()
            );
        }
        let values = bitplan::table(&rewritten, &vars, algebra, threads)?;
        let header = format!(
            "# trit-build v{BUILD_VERSION}\n# algebra: {}\n# variables: {}\n# verified: exact ({})\n# egraph: rounds={rounds} e-nodes={e_nodes} saturated={saturated} cost={} ({cost}, {nodes} nodes)\noriginal: {}\nsimplified: {}\nrewritten: {}\nrules: {}\n",
            alg_name(algebra),
            vars.join(","),
            equivalent.variable_count,
            options.egraph.cost.name(),
            expr.to_infix(),
            simplified.to_infix(),
            rewritten.to_infix(),
            if rules.is_empty() {
                "-".to_string()
            } else {
                rules.join(",")
            }
        );
        rewritten_expr = Some(rewritten);
        (
            vars,
            values,
            header,
            json!({
                "equivalent": true,
                "variables": equivalent.variable_count,
                "rules": rules,
                "egraph": {
                    "rounds": rounds,
                    "eNodes": e_nodes,
                    "saturated": saturated,
                    "costModel": options.egraph.cost.name(),
                    "cost": cost,
                    "nodes": nodes,
                },
            }),
        )
    };

    let rail_text = if emit_verilog || verify_hdl {
        if verilog {
            bail!(
                "--emit-verilog / --verify-hdl 目前只支持 .tri 表达式（Verilog 读入的设计请先转表达式）"
            );
        }
        // 导出用重写后的形态（更小，且已整链复核过等价）。
        let expr = match rewritten_expr {
            Some(expr) => expr,
            None => parse_expr(&parse_source(&source_text, source)?)?,
        };
        Some(rail::emit(&expr, &vars, "trit_module", algebra)?)
    } else {
        None
    };
    let mut outputs = emit_artifacts(
        out_dir,
        &header,
        &vars,
        &values,
        algebra,
        rail_text.as_deref(),
    )?;
    if verify_hdl {
        let testbench = hdl::testbench(&vars, &values);
        let expected = hdl::expected_mem(&values);
        let tb_hash = write_artifact(&out_dir.join("design_tb.v"), testbench.as_bytes())?;
        outputs.insert(
            "design_tb.v".to_string(),
            json!({"bytes": testbench.len(), "hash": tb_hash}),
        );
        let mem_hash = write_artifact(&out_dir.join("expected.mem"), expected.as_bytes())?;
        outputs.insert(
            "expected.mem".to_string(),
            json!({"bytes": expected.len(), "hash": mem_hash}),
        );
        let result = hdl::verify(out_dir)?;
        verified["hdl"] = json!({
            "simulator": "iverilog/vvp",
            "rows": values.len(),
            "result": result,
        });
    }
    if emit_gpu {
        for (name, bytes) in [
            ("truth.wgsl", gpu::KERNEL.as_bytes().to_vec()),
            (
                "truth.gpu.bin",
                gpu::table_bin(&values, vars.len(), algebra),
            ),
            (
                "truth_gpu.json",
                gpu::metadata(values.len(), vars.len(), algebra).into_bytes(),
            ),
        ] {
            let hash = write_artifact(&out_dir.join(name), &bytes)?;
            outputs.insert(
                name.to_string(),
                json!({"bytes": bytes.len(), "hash": hash}),
            );
        }
        verified["gpu"] = json!({
            "kernel": "truth.wgsl",
            "table": "truth.gpu.bin",
            "words": values.len().div_ceil(32),
            "workgroupSize": gpu::WORKGROUP,
        });
    }
    let manifest = json!({
        "tool": "trit-build",
        "version": BUILD_VERSION,
        "cached": false,
        "source": {
            "path": source.display().to_string(),
            "kind": if verilog { "verilog" } else { "expr" },
            "hash": hex(hash64(source_text.as_bytes())),
        },
        "options": {
            "algebra": alg_name(algebra),
            "variables": vars,
            "assignments": values.len(),
            "threads": threads,
            "costModel": options.egraph.cost.name(),
            "emitVerilog": emit_verilog,
            "verifyHdl": verify_hdl,
            "emitGpu": emit_gpu,
        },
        "verified": verified,
        "outputs": outputs,
    });
    fs::create_dir_all(&cache_dir).with_context(|| "创建缓存目录失败")?;
    fs::write(&manifest_path, manifest.to_string()).with_context(|| "写缓存清单失败")?;
    fs::write(out_dir.join("build.json"), manifest.to_string())
        .with_context(|| "写 build.json 失败")?;
    Ok(manifest)
}

/// 单文件构建入口。
/// `cmd_build` 的入参：clap 字段直接映射（裸参数会撞 clippy 的 7 参数上限）。
struct BuildRequest<'a> {
    source: &'a str,
    out_dir: &'a str,
    algebra: &'a str,
    force: bool,
    check: bool,
    output: Option<&'a str>,
    expect: Option<&'a str>,
    emit_verilog: bool,
    verify_hdl: bool,
    emit_gpu: bool,
    threads: usize,
    egraph_rounds: usize,
    egraph_node_limit: usize,
    cost: &'a str,
}

fn cmd_build(request: BuildRequest<'_>) -> Result<()> {
    let BuildRequest {
        source,
        out_dir,
        algebra,
        force,
        check,
        output,
        expect,
        emit_verilog,
        verify_hdl,
        emit_gpu,
        threads,
        egraph_rounds,
        egraph_node_limit,
        cost,
    } = request;
    let alg = resolve_algebra(algebra)?;
    let cost = CostModel::parse(cost)
        .with_context(|| format!("未知代价模型 {cost:?}，可选：nodes / gates"))?;
    let options = BuildOptions {
        algebra: alg,
        force,
        output,
        expect,
        emit_verilog,
        verify_hdl,
        emit_gpu,
        threads,
        egraph: RewriteOptions {
            max_rounds: egraph_rounds,
            node_limit: egraph_node_limit,
            cost,
        },
    };
    let source_path = PathBuf::from(source);
    let out_dir = PathBuf::from(out_dir);
    if check {
        let source_text = fs::read_to_string(&source_path)
            .with_context(|| format!("读 {} 失败", source_path.display()))?;
        let key = cache_key(&source_text, is_verilog(&source_path), &options);
        return check_artifacts(
            &out_dir.join(".trit-cache").join(format!("{key}.json")),
            &out_dir,
            &key,
        );
    }
    let summary = build_source(&source_path, &out_dir, options)?;
    println!("{summary}");
    Ok(())
}

/// 解码 TRB2：校验魔数/长度/名字/CRC，重出 CSV 与 C 查表（不重建 blob）。
fn cmd_decode(blob_path: &Path, out_dir: &Path) -> Result<()> {
    let blob = fs::read(blob_path).with_context(|| format!("读 {} 失败", blob_path.display()))?;
    let (values, vars, alg) = trb::decode(&blob)?;
    let has_names = blob.get(4) == Some(&trb::VERSION);
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建输出目录 {} 失败", out_dir.display()))?;
    let header = format!(
        "# trit-build decode (TRB2)
# file: {}
# variables: {}
# assignments: {}
# verified: crc3{}",
        blob_path.display(),
        vars.join(","),
        values.len(),
        if has_names { " + names_hash" } else { "" }
    );
    let outputs = emit_artifacts(out_dir, &header, &vars, &values, alg, None)?;
    let manifest = json!({
        "tool": "trit-build",
        "mode": "decode",
        "source": {"path": blob_path.display().to_string(), "kind": "trb2", "bytes": blob.len()},
        "options": {"variables": vars, "assignments": values.len(), "algebra": alg_name(alg)},
        "verified": {"crc3": true, "namesHash": has_names, "trits": values.len()},
        "outputs": outputs,
    });
    fs::write(out_dir.join("build.json"), manifest.to_string())
        .with_context(|| "写 build.json 失败")?;
    println!("{manifest}");
    Ok(())
}

fn check_artifacts(manifest_path: &Path, out_dir: &Path, key: &str) -> Result<()> {
    let text = fs::read_to_string(manifest_path)
        .with_context(|| format!("没有缓存清单 {}（先跑一次构建）", manifest_path.display()))?;
    let manifest: Value = serde_json::from_str(&text).context("缓存清单不是合法 JSON")?;
    let outputs = manifest
        .get("outputs")
        .and_then(Value::as_object)
        .context("清单缺少 outputs")?;
    let mut checked = Vec::new();
    let mut bad = Vec::new();
    for (name, info) in outputs {
        let path = out_dir.join(name);
        match fs::read(&path) {
            Ok(bytes) => {
                let expected = info.get("hash").and_then(Value::as_str).unwrap_or("");
                let actual = hex(hash64(&bytes));
                if actual == expected {
                    checked.push(name.clone());
                } else {
                    bad.push(json!({"file": name, "expected": expected, "actual": actual}));
                }
            }
            Err(_) => bad.push(json!({"file": name, "missing": true})),
        }
    }
    let ok = bad.is_empty();
    println!(
        "{}",
        json!({"ok": ok, "key": key, "checked": checked, "bad": bad})
    );
    if !ok {
        bail!("产物校验失败：{} 个文件不一致", bad.len());
    }
    Ok(())
}

#[derive(clap::Parser)]
#[command(
    name = "trit-build",
    version,
    about = "三进制构建工具（M0–M3）：化简 + 饱和重写 + 精确复核 → TRB2/CSV/C/Verilog/WGSL"
)]
struct Cli {
    /// `.tri` 源文件（与 --manifest 二选一）
    source: Option<String>,
    /// 并行清单（JSON：jobs[{name,kind,source|cmd,cwd?,out?}]）
    #[arg(long)]
    manifest: Option<String>,
    /// 输出根目录
    #[arg(long, default_value = "target/trit-build")]
    out: String,
    /// 三值语义：kleene / lukasiewicz / bochvar
    #[arg(long, default_value = "kleene")]
    algebra: String,
    /// 忽略缓存强制重建
    #[arg(long)]
    force: bool,
    /// 只校验已有产物/清单，不重建
    #[arg(long)]
    check: bool,
    /// Verilog 多输出时指定主输出
    #[arg(long)]
    output: Option<String>,
    /// 与表达式链路对拍（例：`--expect "(a & b) | !c"`）
    #[arg(long)]
    expect: Option<String>,
    /// 额外导出双导轨 Verilog（`design.v`，支持 kleene/lukasiewicz/bochvar）
    #[arg(long)]
    emit_verilog: bool,
    /// 生成 testbench 并调用 iverilog/vvp 真仿真（隐含 --emit-verilog）
    #[arg(long)]
    verify_hdl: bool,
    /// 导出 WGSL 查表内核与 2 bit/trit 位平面（`truth.wgsl` / `truth.gpu.bin`）
    #[arg(long)]
    emit_gpu: bool,
    /// 解码 TRB2 产物（`--decode truth.tritbin`），校验 CRC 并重出 CSV/lookup.c
    #[arg(long)]
    decode: Option<String>,
    /// 并行槽位（线程数）
    #[arg(long, default_value_t = 4)]
    jobs: usize,
    /// 单个构建的位并行线程数（0=自动；清单任务固定 1，避免与槽位重叠）
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// e-graph 提取代价模型：nodes（节点数）/ gates（门数）
    #[arg(long, default_value = "nodes")]
    cost: String,
    /// e-graph 最大轮数（到不动点会提前停）
    #[arg(long, default_value_t = 64)]
    egraph_rounds: usize,
    /// e-graph e-node 上限
    #[arg(long, default_value_t = 4096)]
    egraph_node_limit: usize,
    /// 优先支持的轨：ternary / binary / balanced
    #[arg(long, default_value = "ternary")]
    prefer: String,
    /// 只打印分配方案，不执行
    #[arg(long)]
    dry_run: bool,
}

fn main() -> std::process::ExitCode {
    let cli = <Cli as clap::Parser>::parse();
    let result = if let Some(blob) = &cli.decode {
        cmd_decode(Path::new(blob), Path::new(&cli.out))
    } else if let Some(manifest) = &cli.manifest {
        parse_prefer(&cli.prefer).and_then(|prefer| {
            run_parallel(
                Path::new(manifest),
                Path::new(&cli.out),
                resolve_algebra(&cli.algebra)?,
                cli.jobs,
                prefer,
                cli.dry_run,
            )
        })
    } else if let Some(source) = &cli.source {
        cmd_build(BuildRequest {
            source,
            out_dir: &cli.out,
            algebra: &cli.algebra,
            force: cli.force,
            check: cli.check,
            output: cli.output.as_deref(),
            expect: cli.expect.as_deref(),
            emit_verilog: cli.emit_verilog,
            verify_hdl: cli.verify_hdl,
            emit_gpu: cli.emit_gpu,
            threads: cli.threads,
            egraph_rounds: cli.egraph_rounds,
            egraph_node_limit: cli.egraph_node_limit,
            cost: &cli.cost,
        })
    } else {
        Err(anyhow::anyhow!(
            "需要 <source.tri> 或 --manifest <jobs.json>"
        ))
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
