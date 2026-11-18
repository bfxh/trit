//! 三值组合网表的 X 传播与 stuck-at 可测性分析。

#![forbid(unsafe_code)]

pub mod aiger;
pub mod atpg;
pub mod blif;
pub mod bridge;
pub mod circuit;
mod coverage;
pub mod map;
pub mod netlist;
mod pattern;
mod sat_cnf;
pub mod sdf;
pub mod sequential;
pub mod timing;
pub mod verilog;

pub use aiger::{AigerError, parse_aiger, to_aiger};
pub use atpg::{
    AtpgReport, CoverageStat, TestPattern, generate_tests, generate_tests_sat,
    generate_tests_sat_shared_with_jobs, generate_tests_sat_with_jobs,
};
pub use blif::{BlifError, parse_blif, to_blif};
pub use bridge::{
    BridgeError, BridgeFault, BridgeMode, BridgePair, BridgeReport, generate_bridge_tests,
    generate_bridge_tests_sat, generate_bridge_tests_sat_with_jobs,
};
pub use circuit::{
    Bus, Circuit, CircuitError, CircuitReport, DEFAULT_EXACT_BUDGET, ExactPropagation, Fault, Gate,
    GateKind, Propagation, Signal, SimulationRow, UntestableReason, XOutcome, XPessimismReport,
    analyze, analyze_faults, analyze_x_pessimism, propagate, propagate_exact,
    propagate_exact_with_jobs,
};
pub use map::{
    CellLibrary, CellTiming, MapError, MapStrategy, to_cells, to_cells_with_load, to_nand_only,
};
pub use netlist::{NetlistError, from_json, parse_netlist, to_json, to_netlist};
pub use sdf::{
    SdfError, SdfInstance, parse_sdf, parse_sdf_instances, timing_model_from_sdf,
    timing_model_from_sdf_for,
};
pub use sequential::{
    AndGate, Latch, Literal, SequentialCircuit, SequentialError, parse_sequential, to_sequential,
};
pub use timing::{
    OutputTiming, TimingConstraints, TimingError, TimingModel, TimingModelError, TimingPath,
    TimingReport, analyze_timing, analyze_timing_with_constraints, critical_paths,
    critical_paths_with_constraints,
};
pub use verilog::{VerilogError, VerilogErrorKind, parse_verilog, to_verilog};
