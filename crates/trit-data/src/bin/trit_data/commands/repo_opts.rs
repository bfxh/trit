//! repo 子命令参数：仓库根、输出目录、闭包起点与上限。

use std::path::PathBuf;

use super::die;

pub struct Opts {
    pub root: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub closure: Option<String>,
    pub max_files: usize,
}

pub fn parse(argv: &[String]) -> Opts {
    let mut o = Opts {
        root: None,
        out: None,
        closure: None,
        max_files: 64,
    };
    let mut i = 0;
    while i < argv.len() {
        let val = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--repo" => o.root = Some(PathBuf::from(val)),
            "--out" => o.out = Some(PathBuf::from(val)),
            "--closure" => o.closure = Some(val),
            "--max-files" => o.max_files = val.parse().unwrap_or(64),
            other => die(&format!("未知参数 {other}")),
        }
        i += 2;
    }
    o
}
