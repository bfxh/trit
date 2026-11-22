#![forbid(unsafe_code)]
//! Minimal SQL front end over [`tritdb_txn::TxnStore`].

mod ast;
mod bind;
mod engine;
mod error;
mod lexer;
mod parser;
mod plan;
mod stats;
mod storage;

pub use ast::{
    AggregateKind, ColumnDef, Cte, Expr, OrderBy, Quantifier, Query, Select, SelectItem, Statement,
    WindowFunction, With,
};
pub use engine::{JoinStrategy, QueryResult, SqlEngine};
pub use error::{NameKind, SqlError};
pub use parser::parse_script;
pub use stats::{ColumnStats, TableStats};
pub use storage::{decode_table, encode_table, TABLE_KEY_PREFIX};
