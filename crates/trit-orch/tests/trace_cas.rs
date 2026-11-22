#![cfg(feature = "tritdb-cas")]
//! CAS manifest 快照/恢复轨迹 WAL（parent 链 + anchor HMAC）。

#[path = "trace_cas/core.rs"]
mod core;
