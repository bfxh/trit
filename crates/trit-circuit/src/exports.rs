//! 公开 API 的再导出。
//!
//! `lib.rs` 只保留模块声明，把 `pub use` 集中到这里，避免它膨胀。

pub use crate::atpg::{
    AtpgReport, CoverageStat, TestPattern, generate_tests, generate_tests_sat,
    generate_tests_sat_shared_with_jobs, generate_tests_sat_with_jobs,
};
pub use crate::bridge::{
    BridgeError, BridgeFault, BridgeMode, BridgePair, BridgeReport, generate_bridge_tests,
    generate_bridge_tests_sat, generate_bridge_tests_sat_with_jobs,
};
pub use crate::circuit::{
    Bus, Circuit, CircuitError, CircuitReport, DEFAULT_EXACT_BUDGET, ExactPropagation, Fault, Gate,
    GateKind, Propagation, Signal, SimulationRow, UntestableReason, XOutcome, XPessimismReport,
    analyze, analyze_faults, analyze_x_pessimism, propagate, propagate_exact,
    propagate_exact_with_jobs,
};
pub use crate::map::{
    CellLibrary, CellTiming, MapError, MapStrategy, to_cells, to_cells_with_load, to_nand_only,
};
pub use crate::netlist::{NetlistError, from_json, parse_netlist, to_json, to_netlist};
pub use crate::sdf::{
    SdfError, SdfInstance, parse_sdf, parse_sdf_instances, timing_model_from_sdf,
    timing_model_from_sdf_for,
};
pub use crate::timing::{
    OutputTiming, TimingConstraints, TimingError, TimingModel, TimingModelError, TimingPath,
    TimingReport, analyze_timing, analyze_timing_with_constraints, critical_paths,
    critical_paths_with_constraints,
};
pub use crate::verilog::{VerilogError, VerilogErrorKind, parse_verilog, to_verilog};
pub use {
    crate::aiger::{AigerError, parse_aiger, to_aiger},
    crate::blif::{BlifError, parse_blif, to_blif},
};
