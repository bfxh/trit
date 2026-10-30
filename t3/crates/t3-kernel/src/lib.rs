//! `t3-kernel` — the hot loops.
//!
//! Two inner loops dominate a triage run over thousands of files: searching for
//! signature bytes, and binning bytes to compute entropy. Both live here, both
//! have a scalar reference implementation next to the optimised one, and both are
//! measured by `t3 bench` on the same machine rather than asserted to be fast.
//!
//! The kernel is deliberately ignorant of file formats. It takes `&[u8]` and
//! returns offsets or numbers, which keeps the unsafe code confined to a few
//! dozen lines of intrinsic calls with no lifetime or aliasing questions.

#![deny(missing_docs)]
#![warn(clippy::pedantic)]

pub mod entropy;
pub mod scan;

pub use entropy::{
    distinct_bytes, entropy, entropy_of, histogram, histogram4, histogram8, histogram_naive,
    Histogram,
};
pub use scan::bytes;
