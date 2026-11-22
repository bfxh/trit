include!("gate_mods.inc");

pub use parse::parse;
use std::path::PathBuf;
use trit_data::gate::Config;
pub struct Opts {
    pub jsonl: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub cfg: Config,
}
