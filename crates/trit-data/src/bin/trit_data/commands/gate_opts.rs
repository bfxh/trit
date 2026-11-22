//! `gate` 的参数。

use std::path::PathBuf;

use trit_data::gate::Config;

#[path = "gate_opts_parse.rs"]
mod parse;

pub use parse::parse;

pub struct Opts {
    pub jsonl: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub cfg: Config,
}
