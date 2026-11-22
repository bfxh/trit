//! `pipeline` 子命令：单仓库质量门，或按配置批量跑并与基线对比。

use std::path::PathBuf;

use trit_data::repo_gate::Config;

use super::{die, pipeline_batch, pipeline_run, usage};

pub fn run(argv: &[String]) {
    let mut root: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut promote = false;
    let mut cfg = Config::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--repo" => {
                root = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--config" => {
                config = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--promote" => {
                promote = true;
                i += 1;
            }
            "--require-license" => {
                cfg.require_license = true;
                i += 1;
            }
            "--strict-license" => {
                cfg.reject_missing_license = true;
                i += 1;
            }
            "--require-tests" => {
                cfg.require_tests = true;
                i += 1;
            }
            "--allow-generated" => {
                cfg.reject_generated = false;
                i += 1;
            }
            "--allow-duplicates" => {
                cfg.reject_duplicates = false;
                i += 1;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let dir = out.unwrap_or_else(|| PathBuf::from("pipeline-out"));
    if let Some(path) = config {
        pipeline_batch::run(&path, &dir, promote).unwrap_or_else(|e| die(&e));
        return;
    }
    let root = root.unwrap_or_else(|| usage());
    let s = pipeline_run::execute(&root, &dir, &cfg).unwrap_or_else(|e| die(&e));
    println!(
        "管线 {}：{} 文件 → accept {} / defer {} / reject {}（{}）",
        root.display(),
        s.files,
        s.accept,
        s.defer,
        s.reject,
        dir.display()
    );
}
