//! `trit-build`：三进制构建工具的第一阶段（TTIR-0）。
//!
//! 做的事情（全部可复算）：
//! 1. 读一个 `.tri` 源文件（第一行非注释内容 = 三值表达式）；
//! 2. 解析 → 化简（trit-eval）→ **穷举复核等价**（不等价就红，绝不产出"优化后"的错件）；
//! 3. 生成四类产物：
//!    - `expr.txt`：原始/化简后表达式与判定信息；
//!    - `truth.csv`：完整真值表（3^n 行）；
//!    - `truth.tritbin`：`TRB1` 头 + 双导轨位平面（2 bit/trit）+ FNV-64 尾校验；
//!    - `lookup.c`：主机侧查表函数（**执行仍交给 C/Rust 工具链**，这是刻意的边界）；
//! 4. 写 `build.json`（输入/选项/产物哈希/阶段），并按内容哈希做增量缓存。
//!
//! 为什么不是"重写 LLVM"：LLVM 的职责是**二进制**后端的寄存器分配/调度/链接；
//! 三进制真正独有的是语义（K3/Ł3/B3）、X 的传播、基 3 打包与三值校验。
//! 这一层自己做，机器码/链接/OS 仍交给 LLVM/Rust——见 `docs/TERNARY-TOOLCHAIN.md`。
//!
//! 用法：
//! ```text
//! trit-build <source.tri> [--out DIR] [--algebra k3] [--force] [--check]
//! ```
//!
//! 独立 crate（`publish = false`）而不是 `trit-cli` 的子命令：`trit-cli` 是可发布包，
//! 发布图门禁禁止它依赖实验层（`trit-codec`）。

use std::fs;
use std::path::{Path, PathBuf};

mod parallel;
mod schedule;
mod verilog;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use parallel::{parse_prefer, run_parallel};
use trit_core::Algebra;
use trit_core::limits::Limits;
use trit_core::value::Ternary;
use trit_eval::eval::{Assignment, equivalent_within, eval};
use trit_eval::simplify::{SimplifyOptions, simplify_with};

use trit_core::Parser as ExprParser;

fn parse_expr(src: &str) -> Result<trit_core::Expr> {
    ExprParser::parse(src).with_context(|| format!("表达式解析失败：{src}"))
}

fn resolve_algebra(name: &str) -> Result<Algebra> {
    Algebra::parse(name)
        .with_context(|| format!("未知的三值语义 {name:?}，可选：kleene / lukasiewicz / bochvar"))
}

/// 构建工具版本（参与缓存键；产物格式变了就 bump）。
const BUILD_VERSION: u32 = 1;
/// `truth.tritbin` 魔数。
const BLOB_MAGIC: &[u8; 4] = b"TRB1";
/// 允许穷举的变量上限（3^12 = 531,441）。
const MAX_BUILD_VARS: u32 = 12;

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

fn alg_byte(alg: Algebra) -> u8 {
    match alg {
        Algebra::Kleene => 0,
        Algebra::Lukasiewicz => 1,
        Algebra::Bochvar => 2,
    }
}

fn alg_name(alg: Algebra) -> &'static str {
    match alg {
        Algebra::Kleene => "kleene",
        Algebra::Lukasiewicz => "lukasiewicz",
        Algebra::Bochvar => "bochvar",
    }
}

/// 穷举求值：返回按 base-3 编码排好序的输出序列（code 0..3^n）。
fn enumerate(expr: &trit_core::Expr, vars: &[String], alg: Algebra) -> Result<Vec<Ternary>> {
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

/// `TRB1`：魔数 + 版本 + 语义 + 变量数 + 长度 + 双导轨位平面 + FNV-64 尾部校验。
///
/// M0 用双导轨（2 bit/trit，pos/unknowns 各一层 u64 位平面）是为了**不依赖实验层**：
/// 发布图门禁禁止实验 crate 之间互相依赖（`trit-codec` 是 `publish = false`）。
/// 基 3 打包（1.6 bit/trit）与 GF(3) CRC-3 留到 M2，等打包能力下沉到 core 或 codec 稳定。
fn tritbin(values: &[Ternary], alg: Algebra) -> Vec<u8> {
    let words = values.len().div_ceil(64);
    let mut ones = vec![0u64; words];
    let mut unknowns = vec![0u64; words];
    for (index, value) in values.iter().enumerate() {
        let (word, bit) = (index / 64, index % 64);
        match value {
            Ternary::True => ones[word] |= 1 << bit,
            Ternary::Unknown => unknowns[word] |= 1 << bit,
            Ternary::False => {}
        }
    }
    let mut out = Vec::with_capacity(19 + 16 * words);
    out.extend_from_slice(BLOB_MAGIC);
    out.push(BUILD_VERSION as u8);
    out.push(alg_byte(alg));
    out.push(values.len().min(255) as u8);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for word in ones.iter().chain(unknowns.iter()) {
        out.extend_from_slice(&word.to_le_bytes());
    }
    let checksum = hash64(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    out
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
) -> Result<serde_json::Map<String, Value>> {
    let csv = truth_csv(vars, values);
    let blob = tritbin(values, algebra);
    let c_source = lookup_c(vars, values, algebra);
    let mut outputs = serde_json::Map::new();
    for (name, bytes) in [
        ("expr.txt", header.as_bytes().to_vec()),
        ("truth.csv", csv.as_bytes().to_vec()),
        ("truth.tritbin", blob),
        ("lookup.c", c_source.as_bytes().to_vec()),
    ] {
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
    algebra: Algebra,
    force: bool,
    output: Option<&str>,
    expect: Option<&str>,
) -> Result<Value> {
    let source_text =
        fs::read_to_string(source).with_context(|| format!("读 {} 失败", source.display()))?;
    let verilog = is_verilog(source);
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建输出目录 {} 失败", out_dir.display()))?;
    let cache_dir = out_dir.join(".trit-cache");
    let key = hex(hash64(
        format!(
            "trit-build/{BUILD_VERSION}\n{}\n{}\n{}\n{}\n{}",
            alg_name(algebra),
            output.unwrap_or("-"),
            expect.unwrap_or("-"),
            verilog,
            source_text
        )
        .as_bytes(),
    ));
    let manifest_path = cache_dir.join(format!("{key}.json"));

    if !force && let Some(manifest) = cache_hit(&manifest_path, out_dir) {
        let mut summary = manifest;
        summary["cached"] = json!(true);
        return Ok(summary);
    }

    let (vars, values, header, verified) = if verilog {
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
        // 阶段 1：化简；阶段 2：穷举复核（不等价 ⇒ 整个构建失败，不写任何产物）。
        let simplified = simplify_with(&expr, algebra, SimplifyOptions::default());
        let equivalent = equivalent_within(&expr, &simplified, algebra, Limits::default())
            .map_err(|error| anyhow::anyhow!("等价复核超出预算：{error}"))?;
        if !equivalent.equivalent {
            bail!(
                "化简结果与原文不等价（内部错误）：{} -> {}",
                expr.to_infix(),
                simplified.to_infix()
            );
        }
        let values = enumerate(&simplified, &vars, algebra)?;
        let header = format!(
            "# trit-build v{BUILD_VERSION}\n# algebra: {}\n# variables: {}\n# verified: exact ({})\noriginal: {}\nsimplified: {}\n",
            alg_name(algebra),
            vars.join(","),
            equivalent.variable_count,
            expr.to_infix(),
            simplified.to_infix()
        );
        (
            vars,
            values,
            header,
            json!({"equivalent": true, "variables": equivalent.variable_count}),
        )
    };

    let outputs = emit_artifacts(out_dir, &header, &vars, &values, algebra)?;
    let manifest = json!({
        "tool": "trit-build",
        "version": BUILD_VERSION,
        "cached": false,
        "source": {
            "path": source.display().to_string(),
            "kind": if verilog { "verilog" } else { "expr" },
            "hash": hex(hash64(source_text.as_bytes())),
        },
        "options": {"algebra": alg_name(algebra), "variables": vars, "assignments": values.len()},
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
fn cmd_build(
    source: &str,
    out_dir: &str,
    algebra: &str,
    force: bool,
    check: bool,
    output: Option<&str>,
    expect: Option<&str>,
) -> Result<()> {
    let alg = resolve_algebra(algebra)?;
    let source_path = PathBuf::from(source);
    let out_dir = PathBuf::from(out_dir);
    if check {
        let source_text = fs::read_to_string(&source_path)
            .with_context(|| format!("读 {} 失败", source_path.display()))?;
        let key = hex(hash64(
            format!(
                "trit-build/{BUILD_VERSION}\n{}\n{}\n{}\n{}\n{}",
                alg_name(alg),
                output.unwrap_or("-"),
                expect.unwrap_or("-"),
                is_verilog(&source_path),
                source_text
            )
            .as_bytes(),
        ));
        return check_artifacts(
            &out_dir.join(".trit-cache").join(format!("{key}.json")),
            &out_dir,
            &key,
        );
    }
    let summary = build_source(&source_path, &out_dir, alg, force, output, expect)?;
    println!("{summary}");
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
    about = "三进制构建工具（M0）：化简 + 精确复核 + TRB1/CSV/C 产物"
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
    /// 并行槽位（线程数）
    #[arg(long, default_value_t = 4)]
    jobs: usize,
    /// 优先支持的轨：ternary / binary / balanced
    #[arg(long, default_value = "ternary")]
    prefer: String,
    /// 只打印分配方案，不执行
    #[arg(long)]
    dry_run: bool,
}

fn main() -> std::process::ExitCode {
    let cli = <Cli as clap::Parser>::parse();
    let result = if let Some(manifest) = &cli.manifest {
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
        cmd_build(
            source,
            &cli.out,
            &cli.algebra,
            cli.force,
            cli.check,
            cli.output.as_deref(),
            cli.expect.as_deref(),
        )
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
