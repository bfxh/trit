use std::path::PathBuf;
use trit_data::gate::Config;
#[path = "gate_label_opts.rs"]
mod gate_label_opts;
#[path = "gate_lang_opts.rs"]
mod gate_lang_opts;
#[path = "gate_opts_parse.rs"]
mod parse;

pub use parse::parse;

pub struct Opts {
    pub jsonl: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub cfg: Config,
}
