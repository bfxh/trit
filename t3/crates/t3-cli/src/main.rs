//! Executable entry point and argument handling.
//!
//! Deliberately hand-rolled: no argument-parsing dependency, because the tool's
//! entire value proposition is that it has no supply chain to audit. The grammar is
//! small enough to describe in `--help`.

mod bench;
mod json;
mod report;
mod scan;

use std::process::ExitCode;

use t3_core::{rules, Engine, Grade};

const USAGE: &str = "\
t3 — 三值逻辑二进制分诊引擎

用法:
  t3 scan <路径...> [选项]        扫描文件或目录
  t3 explain <文件>               单文件证据与反事实解释
  t3 sig list                     列出内置签名规则
  t3 sig check <规则文件>         校验规则文件语法
  t3 bench [--dir <目录>]         吞吐与标定实测（对比标量基线）
  t3 selftest                     运行时自检（格律、SIMD 等价、规则可解析）

scan 选项:
  --jsonl                 每行一个 JSON 对象（稳定 schema）
  --threads <N>           工作线程数，默认 min(8, CPU 核数)
  --max-size <MiB>        跳过大于此值的文件，默认 512
  --min-grade <G>         只输出 clean|suspicious|malicious 及以上，默认 clean
  --rules <文件>          使用外部规则集，替换内置规则
  --no-rules              不跑签名引擎
  --quiet                 不输出逐文件结果，只输出汇总
  --no-color              禁用颜色

退出码（scan / explain）:
  0  确定干净        1  可疑
  2  确定恶意        3  未定（上限高于干净）
  4  证据冲突        10 用法或 I/O 错误
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print!("{USAGE}");
        return ExitCode::from(10);
    }
    let command = args[0].as_str();
    let rest = &args[1..];

    let result = match command {
        "scan" => scan::run(rest),
        "explain" => explain(rest),
        "sig" => sig(rest),
        "bench" => bench::run(rest),
        "selftest" => selftest(),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(0)
        }
        "-V" | "--version" => {
            println!("t3 {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => {
            eprintln!("未知子命令 `{other}`\n");
            print!("{USAGE}");
            Ok(10)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(message) => {
            eprintln!("错误: {message}");
            ExitCode::from(10)
        }
    }
}

/// Loads the hypothesis engine. User rule files currently replace only the
/// *signature* set, not the hypotheses; the distinction is reported rather than
/// silently ignored.
fn engine() -> Engine {
    rules::default_engine()
}

fn sig(args: &[String]) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        Some("list") => {
            print!("{}", t3_sig::builtin::BUILTIN_RULES);
            Ok(0)
        }
        Some("check") => {
            let path = args.get(1).ok_or("sig check 需要一个规则文件路径")?;
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("无法读取 {path}: {e}"))?;
            match t3_sig::RuleSet::parse(&text) {
                Ok(set) => {
                    println!("{} 条规则，语法通过", set.len());
                    for rule in set.rules() {
                        println!(
                            "  {:<28} {:<7} {} match / {} forbid",
                            rule.name,
                            rule.severity.name(),
                            rule.matches.len(),
                            rule.forbids.len()
                        );
                    }
                    Ok(0)
                }
                Err(err) => Err(format!("{path}: {err}")),
            }
        }
        _ => {
            eprintln!("用法: t3 sig list | t3 sig check <文件>");
            Ok(10)
        }
    }
}

fn explain(args: &[String]) -> Result<i32, String> {
    let mut path: Option<&str> = None;
    let mut rules_path: Option<&str> = None;
    let mut use_rules = true;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rules" => {
                i += 1;
                rules_path = Some(args.get(i).ok_or("--rules 需要一个路径")?);
            }
            "--no-rules" => use_rules = false,
            other if other.starts_with('-') => return Err(format!("未知选项 `{other}`")),
            other => path = Some(other),
        }
        i += 1;
    }
    let path = path.ok_or("explain 需要一个文件路径")?;

    let bytes = std::fs::read(path).map_err(|e| format!("无法读取 {path}: {e}"))?;
    let analysis = t3_format::analyze(&bytes);

    let mut facts = analysis.facts.clone();
    let mut scan_result = None;
    if use_rules {
        let ruleset = match rules_path {
            Some(p) => {
                let text = std::fs::read_to_string(p).map_err(|e| format!("无法读取 {p}: {e}"))?;
                t3_sig::RuleSet::parse(&text).map_err(|e| format!("{p}: {e}"))?
            }
            None => t3_sig::builtin_rules().clone(),
        };
        let scan = t3_sig::evaluate(&ruleset, &analysis.regions);
        facts.set(t3_core::FactId::SigHigh, scan.high, f32::NAN, "签名引擎");
        facts.set(
            t3_core::FactId::SigMedium,
            scan.medium,
            f32::NAN,
            "签名引擎",
        );
        scan_result = Some(scan);
    }

    let verdict = engine().explain(&facts);
    let sections = section_rows(&bytes, analysis.kind);
    report::explain(
        path,
        bytes.len() as u64,
        analysis.kind,
        analysis.note,
        &facts,
        &verdict,
        scan_result.as_ref(),
        &sections,
    );
    Ok(verdict.exit_code())
}

/// Builds the section table shown by `explain`.
fn section_rows(bytes: &[u8], kind: t3_format::Kind) -> Vec<report::SectionRow> {
    let mut rows = Vec::new();
    match kind {
        t3_format::Kind::Pe => {
            let Ok(info) = t3_format::pe::parse(bytes) else {
                return rows;
            };
            for index in 0..info.sections().len() {
                let section = info.sections()[index];
                let data = info.section_data(index).unwrap_or(&[]);
                let flags = format!(
                    "{}{}{}",
                    if section.characteristics & 0x4000_0000 != 0 {
                        'R'
                    } else {
                        '-'
                    },
                    if section.is_writable() { 'W' } else { '-' },
                    if section.is_executable() { 'X' } else { '-' }
                );
                rows.push(report::SectionRow {
                    name: section.name_lossy().into_owned(),
                    va: u64::from(section.virtual_address),
                    raw: u64::from(section.raw_size),
                    vsize: u64::from(section.virtual_size),
                    entropy: (!data.is_empty()).then(|| t3_kernel::entropy(data)),
                    flags,
                });
            }
        }
        t3_format::Kind::Elf => {
            let Ok(info) = t3_format::elf::parse(bytes) else {
                return rows;
            };
            for (index, section) in info.sections().iter().enumerate() {
                // SHT_NOBITS occupies no file bytes.
                if section.sh_type == 8 {
                    continue;
                }
                let start = section.offset as usize;
                let len = (section.size as usize).min(bytes.len().saturating_sub(start));
                let data = bytes.get(start..start + len).unwrap_or(&[]);
                let flags = format!(
                    "{}{}{}",
                    if section.flags & 0x2 != 0 { 'R' } else { '-' },
                    if section.flags & 0x1 != 0 { 'W' } else { '-' },
                    if section.flags & 0x4 != 0 { 'X' } else { '-' }
                );
                rows.push(report::SectionRow {
                    name: info
                        .section_name(index)
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default(),
                    va: section.offset,
                    raw: section.size,
                    vsize: section.size,
                    entropy: (!data.is_empty()).then(|| t3_kernel::entropy(data)),
                    flags,
                });
            }
        }
        t3_format::Kind::Unknown => {}
    }
    rows
}

/// Runtime self-check: the invariants that make the design claims true.
fn selftest() -> Result<i32, String> {
    let mut failures = 0usize;

    // 1. Lattice laws over the full value set.
    for a in t3_core::Tri::ALL {
        for b in t3_core::Tri::ALL {
            if a.and(b) != b.and(a) || a.or(b) != b.or(a) {
                eprintln!("FAIL: K3 commutativity {a:?} {b:?}");
                failures += 1;
            }
            for c in t3_core::Tri::ALL {
                if a.and(b.or(c)) != a.and(b).or(a.and(c)) {
                    eprintln!("FAIL: K3 distributivity");
                    failures += 1;
                }
            }
        }
    }
    for a in t3_core::B4::ALL {
        for b in t3_core::B4::ALL {
            if a.merge(b) != b.merge(a) || a.merge(a) != a {
                eprintln!("FAIL: B4 fusion is not a join");
                failures += 1;
            }
        }
    }

    // 2. Interval semantics against brute force.
    {
        use t3_core::{Expr, FactId, Facts};
        let expr = Expr::fact(FactId::ImportsLoader)
            .and(Expr::fact(FactId::ImportsNetwork))
            .or(Expr::fact(FactId::ImportsCrypto));
        let refs = expr.referenced();
        let mut min = t3_core::Tri::True;
        let mut max = t3_core::Tri::False;
        for mask in 0..(1u32 << refs.len()) {
            let mut f = Facts::new();
            for (bit, id) in refs.iter().enumerate() {
                let v = if mask & (1 << bit) != 0 {
                    t3_core::Tri::True
                } else {
                    t3_core::Tri::False
                };
                f.set(*id, v, f32::NAN, "");
            }
            let got = expr.eval(&f);
            min = min.and(got);
            max = max.or(got);
        }
        let interval = expr.eval_interval(&Facts::new());
        if interval.lo != min || interval.hi != max {
            eprintln!("FAIL: interval is not the exhaustive enclosure");
            failures += 1;
        }
    }

    // 3. SIMD byte search agrees with the scalar reference.
    {
        use t3_kernel::bytes;
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut data = vec![0u8; 4096];
        for b in data.iter_mut() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *b = s as u8;
        }
        data[4001] = 0x5A;
        for needle in [0x00u8, 0x5A, 0xFF, 0x41] {
            if bytes::find_byte(&data, needle) != bytes::find_byte_scalar(&data, needle) {
                eprintln!("FAIL: SIMD find_byte disagrees for {needle:#04x}");
                failures += 1;
            }
        }
    }

    // 4. Entropy of a uniform buffer is maximal.
    {
        let uniform: Vec<u8> = (0..=255u8).cycle().take(256 * 64).collect();
        let e = t3_kernel::entropy(&uniform);
        if (e - 8.0).abs() > 1e-9 {
            eprintln!("FAIL: entropy of a uniform buffer is {e}, expected 8");
            failures += 1;
        }
    }

    // 5. Built-in rules parse.
    if t3_sig::builtin_rules().is_empty() {
        eprintln!("FAIL: built-in rule set is empty");
        failures += 1;
    }

    // 6. Rule engine honours truncation.
    {
        use t3_format::{Region, RegionKind};
        let data = [0u8; 8];
        let regions = vec![Region::new(
            RegionKind::Section(0),
            b".text",
            0,
            &data,
            1 << 20,
        )];
        let scan = t3_sig::evaluate(t3_sig::builtin_rules(), &regions);
        if !scan.high.is_unknown() {
            eprintln!("FAIL: a truncated region must leave signatures undecided");
            failures += 1;
        }
    }

    if failures == 0 {
        println!("自检通过：K3 格律、B4 融合单调性、区间语义穷举、SIMD 等价、熵边界、规则解析");
        Ok(0)
    } else {
        Err(format!("{failures} 项自检失败"))
    }
}

/// Shared grade filter parsing.
fn parse_grade(text: &str) -> Result<Grade, String> {
    match text {
        "clean" => Ok(Grade::Clean),
        "suspicious" => Ok(Grade::Suspicious),
        "malicious" => Ok(Grade::Malicious),
        other => Err(format!(
            "未知等级 `{other}`（应为 clean / suspicious / malicious）"
        )),
    }
}
