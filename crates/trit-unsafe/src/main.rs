//! 用三值逻辑（K3）审 unsafe：每个点判 Safe / Unknown / Risky，按 crate 合成。
//!
//! **不是证明**：它只看"有没有常见防护证据"（`// SAFETY:` 说明、断言/边界检查）。
//! `Safe` = 该有的证据都在；`Risky` = 高危形态 + 既没说明也没检查；其余是 `Unknown`。
//!
//! 判定用 `trit-core` 的 `Ternary` 与 K3 的「或」合成（F∨U=U、U∨T=T）。

mod lexer;
mod scan;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use trit_core::value::Ternary;

#[derive(Parser)]
#[command(
    name = "trit-unsafe",
    about = "三值逻辑审 unsafe：Safe / Unknown / Risky"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// 出现 Risky 就以 1 退出
    #[arg(long, global = true)]
    strict: bool,
}

#[derive(Subcommand)]
enum Command {
    /// 扫描一个源码树
    Scan {
        /// 仓库根（默认当前目录）
        #[arg(long)]
        root: Option<PathBuf>,
        /// 同时扫描 Cargo.lock 里依赖的 registry 源码
        #[arg(long)]
        deps: bool,
        /// 最多扫多少个 .rs 文件（默认 20000）
        #[arg(long, default_value_t = 20_000)]
        max_files: usize,
        /// 输出多少个最危险的站点
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// 扫描依赖时跳过这些 crate 前缀（可重复，例如 windows / linux-raw-sys）
        #[arg(long = "exclude")]
        exclude: Vec<String>,
    },
}

fn cargo_home() -> PathBuf {
    if let Ok(home) = std::env::var("CARGO_HOME") {
        return PathBuf::from(home);
    }
    let user = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(user).join(".cargo")
}

fn registry_roots() -> Vec<PathBuf> {
    let registry = cargo_home().join("registry").join("src");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&registry) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                out.push(entry.path());
            }
        }
    }
    out
}

fn find_dep_dir(name: &str, version: &str) -> Option<PathBuf> {
    for root in registry_roots() {
        let candidate = root.join(format!("{name}-{version}"));
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn site_json(site: &scan::Site) -> Value {
    json!({
        "file": site.file,
        "line": site.line,
        "kind": site.kind,
        "verdict": scan::verdict_name(site.verdict),
        "apis": site.apis,
        "documented": site.documented,
        "guarded": site.guarded,
        "reasons": site.reasons(),
        "snippet": site.snippet,
    })
}

fn scan_command(
    root: Option<&PathBuf>,
    deps: bool,
    max_files: usize,
    top: usize,
    exclude: &[String],
) -> Result<(Value, bool)> {
    let root = root.cloned().unwrap_or_else(|| PathBuf::from("."));
    let mut rollup = scan::scan_tree(&root, max_files)?;

    let mut dep_crates: BTreeMap<String, (Ternary, usize, usize)> = BTreeMap::new();
    let mut deps_scanned = 0usize;
    if deps {
        let lock_path = root.join("Cargo.lock");
        let lock = std::fs::read_to_string(&lock_path)
            .with_context(|| format!("读 {} 失败（--deps 需要 lock 文件）", lock_path.display()))?;
        let packages = scan::lock_crates(&lock);
        let budget = max_files.max(1000);
        let mut remaining = budget;
        for (name, version) in packages {
            if remaining == 0 {
                break;
            }
            if exclude
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str()))
            {
                continue;
            }
            let Some(dir) = find_dep_dir(&name, &version) else {
                continue;
            };
            let subset = scan::scan_tree(&dir, remaining.min(400))?;
            remaining = remaining.saturating_sub(subset.files);
            deps_scanned += 1;
            let verdict = subset
                .sites
                .iter()
                .fold(Ternary::False, |acc, site| scan::join(acc, site.verdict));
            let risky = subset
                .sites
                .iter()
                .filter(|site| site.verdict == Ternary::True)
                .count();
            let entry = dep_crates
                .entry(format!("{}@{}", name, version))
                .or_insert((Ternary::False, 0, 0));
            entry.0 = scan::join(entry.0, verdict);
            entry.1 += subset.sites.len();
            entry.2 += risky;
            rollup.sites.extend(subset.sites);
        }
    }

    rollup
        .sites
        .sort_by_key(|site| std::cmp::Reverse(site.verdict));
    let (safe, unknown, risky, risky_tests, undoc_signatures) = scan::counts(&rollup);
    let mut crate_rows: Vec<(String, Ternary, usize, usize)> = rollup
        .per_crate
        .iter()
        .map(|(name, (verdict, sites, risky))| (name.clone(), *verdict, *sites, *risky))
        .chain(dep_crates.iter().map(|(name, (verdict, sites, risky))| {
            (format!("dep:{name}"), *verdict, *sites, *risky)
        }))
        .collect();
    crate_rows.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.2.cmp(&a.2)));
    let per_crate: Vec<Value> = crate_rows
        .iter()
        .map(|(name, verdict, sites, risky)| {
            json!({"crate": name, "verdict": scan::verdict_name(*verdict), "sites": sites, "risky": risky})
        })
        .collect();
    let top_sites: Vec<Value> = rollup
        .sites
        .iter()
        .filter(|site| site.verdict == Ternary::True && site.context == "src")
        .take(top)
        .map(site_json)
        .collect();
    let report = json!({
        "tool": "trit-unsafe",
        "root": root.display().to_string(),
        "files_scanned": rollup.files,
        "dep_crates_scanned": deps_scanned,
        "sites": rollup.sites.len(),
        "verdicts": {"safe": safe, "unknown": unknown, "risky": risky},
        "risky_in_tests": risky_tests,
        "undocumented_unsafe_signatures": undoc_signatures,
        "per_crate": per_crate,
        "top_risky": top_sites,
        "lattice": "K3：Safe=False / Unknown=Unknown / Risky=True；合成用 Kleene 的 or（F∨U=U、U∨T=T）",
        "caveat": "Safe 只表示常见防护证据齐全，不是 UB 证明；字符串/宏展开按保守词法处理",
    });
    Ok((report, risky > 0))
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Scan {
            root,
            deps,
            max_files,
            top,
            exclude,
        } => scan_command(root.as_ref(), *deps, *max_files, *top, exclude),
    };
    match result {
        Ok((report, finding)) => {
            println!("{report}");
            if cli.strict && finding {
                return std::process::ExitCode::FAILURE;
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
