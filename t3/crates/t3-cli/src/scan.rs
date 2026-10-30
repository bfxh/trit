//! Parallel scanning.
//!
//! Work is distributed by an atomic index over a pre-sorted file list and results are
//! stored by position, so the output is byte-identical regardless of how many threads
//! ran or in what order they finished. That matters more than it sounds: a scanner
//! whose output can be diffed across runs and machines turns "did anything change?"
//! into a question with an answer.
//!
//! Only the read and the per-file analysis are parallel; the emitted lines are written
//! by one thread afterwards. Interleaving stdout from workers would be faster and
//! useless.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use t3_core::{rules, Engine, Facts, Grade, Verdict};
use t3_sig::{Hit, RuleSet};

use crate::json;
use crate::report;

/// Options for a scan run.
pub struct Options {
    /// Emit one JSON object per line.
    pub jsonl: bool,
    /// Worker threads.
    pub threads: usize,
    /// Files larger than this are skipped with a note.
    pub max_size: u64,
    /// Only print files at or above this grade.
    pub min_grade: Grade,
    /// Optional external signature rule set.
    pub rules_path: Option<String>,
    /// Skip the signature engine.
    pub no_rules: bool,
    /// Suppress per-file lines.
    pub quiet: bool,
    /// ANSI colour in human output.
    pub color: bool,
}

/// One file's result.
pub struct Record {
    /// Path as given.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Container name.
    pub kind: &'static str,
    /// Aggregated verdict.
    pub verdict: Verdict,
    /// Observations.
    pub facts: Facts,
    /// Signature hits.
    pub hits: Vec<Hit>,
    /// Analysis time for this file, in microseconds.
    pub elapsed_us: u64,
    /// Read or analysis error.
    pub error: Option<String>,
}

/// Parses `scan` arguments.
pub fn parse_args(args: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut opts = Options {
        jsonl: false,
        threads: std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(4)
            .min(8),
        max_size: 512 * 1024 * 1024,
        min_grade: Grade::Clean,
        rules_path: None,
        no_rules: false,
        quiet: false,
        color: std::io::stdout().is_terminal(),
    };
    let mut inputs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--jsonl" => opts.jsonl = true,
            "--quiet" => opts.quiet = true,
            "--no-rules" => opts.no_rules = true,
            "--no-color" => opts.color = false,
            "--threads" => {
                i += 1;
                let v = args.get(i).ok_or("--threads 需要一个数字")?;
                opts.threads = v
                    .parse::<usize>()
                    .map_err(|_| format!("--threads 需要数字，得到 `{v}`"))?
                    .max(1);
            }
            "--max-size" => {
                i += 1;
                let v = args.get(i).ok_or("--max-size 需要兆字节数")?;
                let mib = v
                    .parse::<u64>()
                    .map_err(|_| format!("--max-size 需要数字，得到 `{v}`"))?;
                opts.max_size = mib.saturating_mul(1024 * 1024);
            }
            "--min-grade" => {
                i += 1;
                let v = args.get(i).ok_or("--min-grade 需要一个等级")?;
                opts.min_grade = crate::parse_grade(v)?;
            }
            "--rules" => {
                i += 1;
                opts.rules_path = Some(args.get(i).ok_or("--rules 需要一个路径")?.to_string());
            }
            other if other.starts_with('-') => return Err(format!("未知选项 `{other}`")),
            other => inputs.push(other.to_string()),
        }
        i += 1;
    }
    if inputs.is_empty() {
        return Err("scan 至少需要一个路径".to_string());
    }
    Ok((opts, inputs))
}

/// Runs `t3 scan`.
pub fn run(args: &[String]) -> Result<i32, String> {
    let (opts, inputs) = parse_args(args)?;
    let engine: Engine = rules::default_engine();

    let ruleset: Option<RuleSet> = if opts.no_rules {
        None
    } else {
        match &opts.rules_path {
            Some(path) => {
                let text =
                    std::fs::read_to_string(path).map_err(|e| format!("无法读取 {path}: {e}"))?;
                Some(RuleSet::parse(&text).map_err(|e| format!("{path}: {e}"))?)
            }
            None => Some(t3_sig::builtin_rules().clone()),
        }
    };

    let started = Instant::now();
    let (mut files, mut problems) = collect(&inputs, opts.max_size);
    files.sort();
    files.dedup();

    let records = scan_all(
        &files,
        &engine,
        ruleset.as_ref(),
        opts.threads,
        &mut problems,
    );

    let elapsed = started.elapsed();
    let mut bytes = 0u64;
    let mut tally = [0usize; 5];
    let mut worst = 0i32;

    for record in &records {
        bytes += record.size;
        let code = match &record.error {
            Some(_) => 10,
            None => record.verdict.exit_code(),
        };
        match code {
            0 => tally[0] += 1,
            1 => tally[1] += 1,
            2 => tally[2] += 1,
            3 => tally[3] += 1,
            4 => tally[4] += 1,
            _ => {}
        }
        worst = aggregate(worst, code);
    }

    if opts.jsonl {
        for record in &records {
            println!("{}", json::record(record));
        }
        for problem in &problems {
            println!("{}", json::error_line(problem));
        }
    } else {
        if !opts.quiet {
            for record in &records {
                if shown(record, opts.min_grade) {
                    println!("{}", report::line(record, opts.color));
                }
            }
            for problem in &problems {
                eprintln!("跳过: {problem}");
            }
        }
        report::summary(&records, &problems, bytes, elapsed, &tally);
    }

    Ok(worst)
}

fn aggregate(current: i32, code: i32) -> i32 {
    // Attention precedence: a definite malicious verdict outranks a conflict, which
    // outranks suspicion, which outranks "undecided". I/O errors rank last because
    // they are not a statement about content.
    fn rank(code: i32) -> u8 {
        match code {
            2 => 5,
            4 => 4,
            1 => 3,
            3 => 2,
            0 => 1,
            _ => 0,
        }
    }
    if rank(code) > rank(current) {
        code
    } else {
        current
    }
}

fn shown(record: &Record, min_grade: Grade) -> bool {
    if record.error.is_some() {
        return true;
    }
    record.verdict.hi >= min_grade || !record.verdict.is_decided()
}

/// Expands inputs into a file list, reporting anything it had to skip.
pub fn collect(inputs: &[String], max_size: u64) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut problems = Vec::new();
    for input in inputs {
        let path = Path::new(input);
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => {
                if meta.len() > max_size {
                    problems.push(format!("{input}: 超过大小上限"));
                } else {
                    files.push(path.to_path_buf());
                }
            }
            Ok(meta) if meta.is_dir() => walk(path, max_size, &mut files, &mut problems, 0),
            Ok(_) => problems.push(format!("{input}: 不是普通文件或目录")),
            Err(err) => problems.push(format!("{input}: {err}")),
        }
    }
    (files, problems)
}

fn walk(
    dir: &Path,
    max_size: u64,
    files: &mut Vec<PathBuf>,
    problems: &mut Vec<String>,
    depth: usize,
) {
    if depth > 32 {
        problems.push(format!("{}: 目录层级过深，已停止", dir.display()));
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            problems.push(format!("{}: {err}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `file_type` does not follow symlinks, which prevents an infinite walk over a
        // link that points at an ancestor.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&path, max_size, files, problems, depth + 1);
        } else if kind.is_file() {
            match entry.metadata() {
                Ok(meta) if meta.len() > max_size => {
                    problems.push(format!("{}: 超过大小上限", path.display()));
                }
                Ok(_) => files.push(path),
                Err(err) => problems.push(format!("{}: {err}", path.display())),
            }
        }
    }
}

fn scan_all(
    files: &[PathBuf],
    engine: &Engine,
    ruleset: Option<&RuleSet>,
    threads: usize,
    problems: &mut Vec<String>,
) -> Vec<Record> {
    let cursor = AtomicUsize::new(0);
    let threads = threads.max(1).min(files.len().max(1));
    let slots: Vec<std::sync::Mutex<Option<Record>>> = (0..files.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let issues: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                if index >= files.len() {
                    break;
                }
                let record = analyze_one(&files[index], engine, ruleset);
                if let Some(err) = &record.error {
                    if let Ok(mut guard) = issues.lock() {
                        guard.push(format!("{}: {err}", record.path));
                    }
                }
                if let Ok(mut slot) = slots[index].lock() {
                    *slot = Some(record);
                }
            });
        }
    });

    problems.extend(issues.into_inner().unwrap_or_default());
    slots
        .into_iter()
        .filter_map(|slot| slot.into_inner().ok().flatten())
        .collect()
}

/// Analyses one file. Reads it whole: a partial read would silently change an
/// `Unknown` into a `False`, which is exactly the failure this tool exists to avoid.
pub fn analyze_one(path: &Path, engine: &Engine, ruleset: Option<&RuleSet>) -> Record {
    let started = Instant::now();
    let display = path.display().to_string();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            return Record {
                path: display,
                size: 0,
                kind: "error",
                verdict: engine.verdict(&Facts::new()),
                facts: Facts::new(),
                hits: Vec::new(),
                elapsed_us: started.elapsed().as_micros() as u64,
                error: Some(err.to_string()),
            }
        }
    };

    let analysis = t3_format::analyze(&bytes);
    let mut facts = analysis.facts.clone();
    let hits = match ruleset {
        Some(rules) => {
            let scan = t3_sig::evaluate(rules, &analysis.regions);
            facts.set(t3_core::FactId::SigHigh, scan.high, f32::NAN, "签名引擎");
            facts.set(
                t3_core::FactId::SigMedium,
                scan.medium,
                f32::NAN,
                "签名引擎",
            );
            scan.hits
        }
        None => Vec::new(),
    };

    let verdict = engine.verdict(&facts);
    Record {
        path: display,
        size: bytes.len() as u64,
        kind: analysis.kind.name(),
        verdict,
        facts,
        hits,
        elapsed_us: started.elapsed().as_micros() as u64,
        error: None,
    }
}
