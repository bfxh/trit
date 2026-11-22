#[path = "commands/card.rs"]
mod card;
#[path = "commands/compare.rs"]
mod compare;
#[path = "commands/decontaminate.rs"]
mod decontaminate;
#[path = "commands/dispatch.rs"]
mod dispatch;
#[path = "commands/gate.rs"]
mod gate;
#[path = "commands/json.rs"]
mod json;
#[path = "commands/lang.rs"]
mod lang;
#[path = "commands/lang_opts.rs"]
mod lang_opts;
#[path = "commands/mix.rs"]
mod mix;
#[path = "commands/project.rs"]
mod project;
#[path = "commands/shared.rs"]
mod shared;
pub(crate) use shared::{created_unix, die, usage, write_lines};

pub use dispatch::run;
