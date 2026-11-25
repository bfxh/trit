//! 位平面 `TriBit` 的 spec/exec；逐位证明在 `bits/proofs.rs`。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::k3::Trit;
#[cfg(verus_only)]
use crate::k3::{knot, tand, tor};

verus! {

mod proofs;

pub open spec fn bit_at(x: u64, bit: u32) -> bool {
    (x >> (bit % 64)) & 1 == 1
}

#[derive(Clone, Copy)]
pub struct BitPlane(pub u64);

impl BitPlane {
    pub open spec fn bit(self, bit: u32) -> bool {
        bit_at(self.0, bit)
    }
}

#[derive(Clone, Copy)]
pub struct TriBit {
    pub ones: BitPlane,
    pub unknowns: BitPlane,
}

impl TriBit {
    pub open spec fn get(self, bit: u32) -> Trit {
        if self.unknowns.bit(bit) {
            Trit::U
        } else if self.ones.bit(bit) {
            Trit::T
        } else {
            Trit::F
        }
    }

    pub open spec fn crisp_ones(self) -> u64 {
        self.ones.0 & !self.unknowns.0
    }

    pub open spec fn crisp_zeros(self) -> u64 {
        !self.ones.0 & !self.unknowns.0
    }

    pub open spec fn unknown_bits(self) -> u64 {
        self.unknowns.0
    }

    pub open spec fn not_spec(self) -> Self {
        TriBit {
            ones: BitPlane(!self.crisp_ones() & !self.unknown_bits()),
            unknowns: self.unknowns,
        }
    }

    pub open spec fn and_lattice_spec(self, other: Self) -> Self {
        let res0 = self.crisp_zeros() | other.crisp_zeros();
        let res1 = self.crisp_ones() & other.crisp_ones();
        let resx = !res0 & !res1;
        TriBit { ones: BitPlane(res1), unknowns: BitPlane(resx) }
    }

    pub open spec fn or_lattice_spec(self, other: Self) -> Self {
        let res1 = self.crisp_ones() | other.crisp_ones();
        let res0 = self.crisp_zeros() & other.crisp_zeros();
        let resx = !res1 & !res0;
        TriBit { ones: BitPlane(res1), unknowns: BitPlane(resx) }
    }


    // BEGIN BITS OPS
    pub fn crisp_ones_exec(self) -> (r: u64)
        ensures r == self.crisp_ones(),
    {
        self.ones.0 & !self.unknowns.0
    }

    pub fn crisp_zeros_exec(self) -> (r: u64)
        ensures r == self.crisp_zeros(),
    {
        !self.ones.0 & !self.unknowns.0
    }

    pub fn unknown_bits_exec(self) -> (r: u64)
        ensures r == self.unknown_bits(),
    {
        self.unknowns.0
    }

    pub fn not_exec(self) -> (r: TriBit)
        ensures r == self.not_spec(),
    {
        TriBit {
            ones: BitPlane(!self.crisp_ones_exec() & !self.unknown_bits_exec()),
            unknowns: self.unknowns,
        }
    }

    pub fn and_lattice_exec(self, other: TriBit) -> (r: TriBit)
        ensures r == self.and_lattice_spec(other),
    {
        let a0 = self.crisp_zeros_exec();
        let b0 = other.crisp_zeros_exec();
        let a1 = self.crisp_ones_exec();
        let b1 = other.crisp_ones_exec();
        let res0 = a0 | b0;
        let res1 = a1 & b1;
        let resx = !res0 & !res1;
        TriBit {
            ones: BitPlane(res1),
            unknowns: BitPlane(resx),
        }
    }

    pub fn or_lattice_exec(self, other: TriBit) -> (r: TriBit)
        ensures r == self.or_lattice_spec(other),
    {
        let a0 = self.crisp_zeros_exec();
        let b0 = other.crisp_zeros_exec();
        let a1 = self.crisp_ones_exec();
        let b1 = other.crisp_ones_exec();
        let res1 = a1 | b1;
        let res0 = a0 & b0;
        let resx = !res1 & !res0;
        TriBit {
            ones: BitPlane(res1),
            unknowns: BitPlane(resx),
        }
    }
    // END BITS OPS
}

}
