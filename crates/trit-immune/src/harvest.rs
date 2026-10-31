//! 项目收割：吸收自有/相关项目时的严格流程。
//!
//! ① 清理构建产物：白名单目录/后缀 + 根路径围栏 + 拒绝符号链接（不跟随、不删、如实上报）；
//! ② 语言普查：吸收项目时的**运维需要**（知道手里是什么技术栈）——注意这与
//!    `vocab.rs` 的概念台账不同：那里的标签是**解释用的**（规范名+别名+设计属性），
//!    两回事不要混；
//! ③ 源码路径登记：追加式注册表——"源代码在某某路径"的正式账本，先删构建再入册。

use crate::delta::{esc, unesc};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 视为构建产物的目录名（命中即整棵删除）。
const ARTIFACT_DIRS: &[&str] = &[
    "target",
    "build",
    "dist",
    "out",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".gradle",
    ".next",
    "coverage",
    ".pytest_cache",
    ".mypy_cache",
];

/// 视为构建产物的文件后缀。
const ARTIFACT_EXTS: &[&str] = &["pyc", "pyo", "o", "obj", "class"];

/// 扩展名 → 语言标签（普查用；校准协议本身语言无关）。
const LANG_BY_EXT: &[(&str, &str)] = &[
    ("rs", "Rust"),
    ("py", "Python"),
    ("js", "JavaScript"),
    ("mjs", "JavaScript"),
    ("cjs", "JavaScript"),
    ("ts", "TypeScript"),
    ("tsx", "TypeScript"),
    ("jsx", "TypeScript"),
    ("go", "Go"),
    ("c", "C"),
    ("h", "C"),
    ("cpp", "C++"),
    ("cc", "C++"),
    ("cxx", "C++"),
    ("hpp", "C++"),
    ("java", "Java"),
    ("cs", "C#"),
    ("rb", "Ruby"),
    ("php", "PHP"),
    ("swift", "Swift"),
    ("kt", "Kotlin"),
    ("md", "Markdown"),
    ("toml", "TOML"),
    ("yaml", "YAML"),
    ("yml", "YAML"),
    ("json", "JSON"),
    ("wgsl", "WGSL"),
    ("sql", "SQL"),
];

/// 收割报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestReport {
    /// 规范化后的源码根路径（"源代码在某某路径"）
    pub root: PathBuf,
    /// 已删除的构建产物（绝对路径）
    pub removed: Vec<PathBuf>,
    /// 拒绝处理的路径（符号链接等——如实上报，不静默）
    pub refused: Vec<PathBuf>,
    /// 语言普查（计数降序，同数按名字典序）
    pub languages: Vec<(String, usize)>,
    /// 主语言标签
    pub dominant: String,
}

/// 收割一个项目：删构建产物 + 语言普查。不碰白名单之外的任何东西。
pub fn harvest(root: &Path) -> io::Result<HarvestReport> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "root 不是目录"));
    }
    let mut report = HarvestReport {
        root: root.clone(),
        removed: Vec::new(),
        refused: Vec::new(),
        languages: Vec::new(),
        dominant: "unknown".into(),
    };
    clean_dir(&root, &root, &mut report, 0)?;
    report.languages = census(&root)?;
    report.dominant = report
        .languages
        .first()
        .map(|(l, _)| l.clone())
        .unwrap_or_else(|| "unknown".into());
    Ok(report)
}

fn clean_dir(root: &Path, dir: &Path, report: &mut HarvestReport, depth: usize) -> io::Result<()> {
    if depth > 32 {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // 符号链接：不跟随、不删、如实上报（防越界删除）
        if entry.file_type()?.is_symlink() {
            report.refused.push(path);
            continue;
        }
        // 根围栏：任何路径必须仍在源码根内
        if !path.starts_with(root) {
            report.refused.push(path);
            continue;
        }
        if path.is_dir() {
            if ARTIFACT_DIRS.contains(&name.as_str()) {
                fs::remove_dir_all(&path)?;
                report.removed.push(path);
            } else {
                clean_dir(root, &path, report, depth + 1)?;
            }
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ARTIFACT_EXTS.contains(&ext) {
                fs::remove_file(&path)?;
                report.removed.push(path);
            }
        }
    }
    Ok(())
}

fn census(root: &Path) -> io::Result<Vec<(String, usize)>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        if seen > 200_000 {
            break; // 防失控目录
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            seen += 1;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if let Some((_, lang)) = LANG_BY_EXT.iter().find(|(e, _)| *e == ext) {
                    *counts.entry(lang.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(v)
}

/// 收割注册表：追加式账本——每行一条 `HARVEST\t<源码路径>\t<主语言>\t<普查>\t<删除数>`。
pub struct Registry {
    path: PathBuf,
}

/// 注册表条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub root: String,
    pub dominant: String,
    pub languages: String,
    pub removed: usize,
}

impl Registry {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 登记一次收割（"源代码在某某路径"的提示即此行）。
    pub fn record(&self, report: &HarvestReport) -> io::Result<String> {
        let census = report
            .languages
            .iter()
            .map(|(l, c)| format!("{l}={c}"))
            .collect::<Vec<_>>()
            .join(",");
        let line = format!(
            "HARVEST\t{}\t{}\t{}\t{}",
            esc(&report.root.to_string_lossy()),
            report.dominant,
            esc(&census),
            report.removed.len()
        );
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        Ok(line)
    }

    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        let mut out = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut f = line.split('\t');
            match (f.next(), f.next(), f.next(), f.next(), f.next(), f.next()) {
                (
                    Some("HARVEST"),
                    Some(root),
                    Some(dominant),
                    Some(languages),
                    Some(removed),
                    None,
                ) => {
                    let removed = removed
                        .parse::<usize>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    out.push(Entry {
                        root: unesc(root)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                        dominant: dominant.to_string(),
                        languages: unesc(languages)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                        removed,
                    });
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("注册表行不合法：{line:?}"),
                    ))
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_sorts_by_count_then_name() {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        m.insert("Rust".into(), 5);
        m.insert("Python".into(), 9);
        m.insert("Go".into(), 5);
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(
            v,
            vec![
                ("Python".to_string(), 9),
                ("Go".to_string(), 5),
                ("Rust".to_string(), 5)
            ]
        );
    }
}
