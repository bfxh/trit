//! 三值逻辑核心：值、代数、AST、解析器、位并行表示与二进制互操作。

#![forbid(unsafe_code)]

pub mod algebra;
pub mod ast;
pub mod bits;
pub mod bulk;
pub mod limits;
pub mod parallel;
pub mod parser;
pub mod topology;
pub mod tristate;
pub mod signfield;
pub mod value;

pub use algebra::Algebra;
pub use ast::{Expr, VarNameError};
pub use bits::{BitPlane, TriBit, TruthTable};
pub use bulk::{TriVec, TriVecError};
pub use limits::{LimitError, Limits, check_expr, check_vars, checked_pow3};
pub use parser::Parser;
pub use topology::{TernaryEdge, TernaryTopology, TopologyError};
pub use tristate::{
    BusError, LaneConflict, TriState, TriStateBus, TriStateConflict, TriStateDriver,
    resolve_drivers,
};
pub use value::Ternary;
pub use signfield::{SignField3D, SignMesh, TriSign, extract_surface};
