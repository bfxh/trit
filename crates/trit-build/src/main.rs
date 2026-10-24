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

mod schedule;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use schedule::{Prefer, Track};
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

/// 构建单个 `.tri`：化简 + 穷举复核 + 产物 + 缓存，返回清单。
fn build_source(source: &Path, out_dir: &Path, algebra: Algebra, force: bool) -> Result<Value> {
    let source_text =
        fs::read_to_string(source).with_context(|| format!("读 {} 失败", source.display()))?;
    let expression_text = parse_source(&source_text, source)?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建输出目录 {} 失败", out_dir.display()))?;
    let cache_dir = out_dir.join(".trit-cache");
    let key = hex(hash64(
        format!(
            "trit-build/{BUILD_VERSION}\n{}\n{}",
            alg_name(algebra),
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

    // 阶段 3：产物。
    let expr_text = format!(
        "# trit-build v{BUILD_VERSION}\n# algebra: {}\n# variables: {}\n# verified: exact ({})\noriginal: {}\nsimplified: {}\n",
        alg_name(algebra),
        vars.join(","),
        equivalent.variable_count,
        expr.to_infix(),
        simplified.to_infix()
    );
    let csv = truth_csv(&vars, &values);
    let blob = tritbin(&values, algebra);
    let c_source = lookup_c(&vars, &values, algebra);

    let mut outputs = serde_json::Map::new();
    for (name, bytes) in [
        ("expr.txt", expr_text.as_bytes().to_vec()),
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

    let manifest = json!({
        "tool": "trit-build",
        "version": BUILD_VERSION,
        "cached": false,
        "source": {"path": source.display().to_string(), "hash": hex(hash64(source_text.as_bytes()))},
        "options": {"algebra": alg_name(algebra), "variables": vars, "assignments": values.len()},
        "verified": {"equivalent": true, "variables": equivalent.variable_count},
        "outputs": outputs,
    });
    fs::create_dir_all(&cache_dir).with_context(|| "创建缓存目录失败")?;
    fs::write(&manifest_path, manifest.to_string()).with_context(|| "写缓存清单失败")?;
    fs::write(out_dir.join("build.json"), manifest.to_string())
        .with_context(|| "写 build.json 失败")?;
    Ok(manifest)
}

/// 单文件构建入口。
fn cmd_build(source: &str, out_dir: &str, algebra: &str, force: bool, check: bool) -> Result<()> {
    let alg = resolve_algebra(algebra)?;
    let source_path = PathBuf::from(source);
    let out_dir = PathBuf::from(out_dir);
    if check {
        let source_text = fs::read_to_string(&source_path)
            .with_context(|| format!("读 {} 失败", source_path.display()))?;
        let key = hex(hash64(
            format!(
                "trit-build/{BUILD_VERSION}\n{}\n{}",
                alg_name(alg),
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
    let summary = build_source(&source_path, &out_dir, alg, force)?;
    println!("{summary}");
    Ok(())
}

// ---------- 双轨并行（--manifest） ----------

#[derive(Debug, Clone)]
struct JobSpec {
    name: String,
    track: Track,
    source: Option<PathBuf>,
    out: Option<PathBuf>,
    cwd: Option<PathBuf>,
    cmd: Option<Vec<String>>,
}

fn parse_prefer(name: &str) -> Result<Prefer> {
    match name {
        "ternary" => Ok(Prefer::Ternary),
        "binary" => Ok(Prefer::Binary),
        "balanced" => Ok(Prefer::Balanced),
        other => bail!("未知的 --prefer {other:?}，可选：ternary / binary / balanced"),
    }
}

fn load_jobs(path: &Path) -> Result<Vec<JobSpec>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("读清单 {} 失败", path.display()))?;
    let value: Value =
        serde_json::from_str(&text).with_context(|| format!("{} 不是合法 JSON", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let jobs = value
        .get("jobs")
        .and_then(Value::as_array)
        .context("清单缺少 jobs 数组")?;
    let mut out = Vec::with_capacity(jobs.len());
    for (index, job) in jobs.iter().enumerate() {
        let name = job
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| format!("jobs[{index}] 缺少 name"))?
            .to_string();
        let kind = job
            .get("kind")
            .and_then(Value::as_str)
            .with_context(|| format!("jobs[{index}] 缺少 kind"))?;
        let rel = |key: &str| -> Option<PathBuf> {
            job.get(key).and_then(Value::as_str).map(|p| base.join(p))
        };
        match kind {
            "ternary" => {
                let source = rel("source").context(format!("ternary 任务 {index} 缺 source"))?;
                out.push(JobSpec {
                    name,
                    track: Track::Ternary,
                    source: Some(source),
                    out: rel("out"),
                    cwd: None,
                    cmd: None,
                });
            }
            "binary" => {
                let cmd: Vec<String> = job
                    .get("cmd")
                    .and_then(Value::as_array)
                    .with_context(|| format!("binary 任务 {index} 缺 cmd 数组"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .with_context(|| format!("binary 任务 {index} 的 cmd 含非字符串"))
                    })
                    .collect::<Result<_>>()?;
                if cmd.is_empty() {
                    bail!("binary 任务 {index} 的 cmd 为空");
                }
                out.push(JobSpec {
                    name,
                    track: Track::Binary,
                    source: None,
                    out: None,
                    cwd: rel("cwd"),
                    cmd: Some(cmd),
                });
            }
            other => bail!("jobs[{index}] 未知 kind {other:?}（ternary / binary）"),
        }
    }
    Ok(out)
}

/// 干跑：只给分配方案（不执行）。balanced = 清单顺序，其余 = 主轨在前。
fn dry_run_plan(jobs: &[JobSpec], prefer: Prefer) -> Value {
    let order: Vec<usize> = if prefer == Prefer::Balanced {
        (0..jobs.len()).collect()
    } else {
        let (primary, secondary) = match prefer {
            Prefer::Binary => (Track::Binary, Track::Ternary),
            _ => (Track::Ternary, Track::Binary),
        };
        let mut out: Vec<usize> = (0..jobs.len())
            .filter(|&i| jobs[i].track == primary)
            .collect();
        out.extend((0..jobs.len()).filter(|&i| jobs[i].track == secondary));
        out
    };
    json!({
        "mode": "dry-run",
        "prefer": format!("{prefer:?}").to_lowercase(),
        "plan": order
            .iter()
            .map(|&i| json!({
                "name": jobs[i].name,
                "track": match jobs[i].track { Track::Ternary => "ternary", Track::Binary => "binary" },
                "owner": match jobs[i].track { Track::Ternary => "ternary-track", Track::Binary => "binary-track" },
            }))
            .collect::<Vec<_>>(),
    })
}

fn execute_job(job: &JobSpec, algebra: Algebra, out_root: &Path) -> Value {
    let started = Instant::now();
    let result = match job.track {
        Track::Ternary => {
            let source = job.source.as_deref().expect("ternary 任务必有 source");
            let out = job.out.clone().unwrap_or_else(|| out_root.join(&job.name));
            build_source(source, &out, algebra, false).map(|summary| {
                json!({
                    "cached": summary.get("cached").cloned().unwrap_or(json!(false)),
                    "out": out.display().to_string(),
                })
            })
        }
        Track::Binary => {
            let cmd = job.cmd.as_ref().expect("binary 任务必有 cmd");
            let mut command = std::process::Command::new(&cmd[0]);
            command.args(&cmd[1..]);
            if let Some(cwd) = &job.cwd {
                command.current_dir(cwd);
            }
            // 捕获子进程输出，避免污染本工具的 JSON stdout；失败时带上尾部。
            command
                .output()
                .map_err(|error| anyhow::anyhow!("启动 {} 失败：{error}", cmd[0]))
                .and_then(|output| {
                    let status = output.status;
                    if status.success() {
                        return Ok(json!({"exit": status.code()}));
                    }
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let tail: String = stderr.chars().rev().take(400).collect::<String>();
                    let tail: String = tail.chars().rev().collect();
                    bail!("{} 失败（exit {:?}）：{}", cmd[0], status.code(), tail)
                })
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(extra) => {
            let mut value = json!({
                "name": job.name,
                "track": match job.track { Track::Ternary => "ternary", Track::Binary => "binary" },
                "ok": true,
                "ms": ms,
            });
            if let (Some(map), Some(extra_map)) = (value.as_object_mut(), extra.as_object()) {
                for (key, v) in extra_map {
                    map.insert(key.clone(), v.clone());
                }
            }
            value
        }
        Err(error) => json!({
            "name": job.name,
            "track": match job.track { Track::Ternary => "ternary", Track::Binary => "binary" },
            "ok": false,
            "ms": ms,
            "error": format!("{error:#}"),
        }),
    }
}

struct Shared {
    ternary: VecDeque<usize>,
    binary: VecDeque<usize>,
    fifo: VecDeque<usize>,
    running_ternary: usize,
    running_binary: usize,
    results: Vec<Option<Value>>,
    remaining: usize,
    prefer: Prefer,
    slots: usize,
}

fn take_next(shared: &mut Shared) -> Option<usize> {
    if shared.prefer == Prefer::Balanced {
        let index = shared.fifo.pop_front()?;
        return Some(index);
    }
    let load = schedule::Load {
        ternary_pending: shared.ternary.len(),
        binary_pending: shared.binary.len(),
        ternary_running: shared.running_ternary,
        binary_running: shared.running_binary,
    };
    match schedule::pick(load, shared.prefer, shared.slots)? {
        Track::Ternary => {
            let index = shared.ternary.pop_front()?;
            shared.running_ternary += 1;
            Some(index)
        }
        Track::Binary => {
            let index = shared.binary.pop_front()?;
            shared.running_binary += 1;
            Some(index)
        }
    }
}

fn run_parallel(
    manifest: &Path,
    out_root: &Path,
    algebra: Algebra,
    slots: usize,
    prefer: Prefer,
    dry_run: bool,
) -> Result<()> {
    let jobs = load_jobs(manifest)?;
    if jobs.is_empty() {
        bail!("清单里没有任务");
    }
    if dry_run {
        println!("{}", dry_run_plan(&jobs, prefer));
        return Ok(());
    }
    let slots = slots.max(1).min(jobs.len());
    let shared = Mutex::new(Shared {
        ternary: jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.track == Track::Ternary)
            .map(|(i, _)| i)
            .collect(),
        binary: jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.track == Track::Binary)
            .map(|(i, _)| i)
            .collect(),
        fifo: (0..jobs.len()).collect(),
        running_ternary: 0,
        running_binary: 0,
        results: vec![None; jobs.len()],
        remaining: jobs.len(),
        prefer,
        slots,
    });
    let cv = Condvar::new();
    let wall_start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..slots {
            scope.spawn(|| {
                loop {
                    let index = {
                        let mut guard = shared.lock().expect("调度锁中毒");
                        loop {
                            if let Some(index) = take_next(&mut guard) {
                                break index;
                            }
                            if guard.remaining == 0 {
                                cv.notify_all();
                                return;
                            }
                            guard = cv.wait(guard).expect("调度等待中毒");
                        }
                    };
                    let track = jobs[index].track;
                    let value = execute_job(&jobs[index], algebra, out_root);
                    let mut guard = shared.lock().expect("调度锁中毒");
                    guard.results[index] = Some(value);
                    guard.remaining -= 1;
                    match track {
                        Track::Ternary => guard.running_ternary -= 1,
                        Track::Binary => guard.running_binary -= 1,
                    }
                    cv.notify_all();
                }
            });
        }
    });
    let wall_ms = wall_start.elapsed().as_millis() as u64;
    let mut guard = shared.lock().expect("调度锁中毒");
    let results: Vec<Value> = guard
        .results
        .drain(..)
        .map(|v| v.unwrap_or_else(|| json!({"ok": false, "error": "任务未执行"})))
        .collect();
    drop(guard);
    let failed = results.iter().filter(|v| v["ok"] != json!(true)).count();
    let busy_ms: u64 = results.iter().filter_map(|v| v["ms"].as_u64()).sum();
    let speedup = if wall_ms == 0 {
        json!(null)
    } else {
        json!((busy_ms as f64 / wall_ms as f64 * 100.0).round() / 100.0)
    };
    let summary = json!({
        "mode": "parallel",
        "prefer": format!("{prefer:?}").to_lowercase(),
        "slots": slots,
        "jobs": results,
        "failed": failed,
        "wall_ms": wall_ms,
        "busy_ms": busy_ms,
        "speedup_vs_serial": speedup,
        "note": "speedup = busy_ms / wall_ms；仅统计本工具看到的耗时，外部命令的编译缓存不计",
    });
    println!("{summary}");
    if failed > 0 {
        bail!("{failed} 个任务失败");
    }
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
        cmd_build(source, &cli.out, &cli.algebra, cli.force, cli.check)
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
