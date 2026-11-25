//! `TriBit` 的位级 lemma 与逐位等价证明（`bits.rs` 的子模块）。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bits::bit_at;
use crate::bits::TriBit;
#[cfg(verus_only)]
use crate::k3::Trit;

#[cfg(verus_only)]
use crate::k3::{knot, tand, tor};

verus! {

impl TriBit {
    // ---- 位级 lemma（参数是 u64，`by (bit_vector)` 才能看到） --------------

    proof fn not_off(oa: u64, ua: u64, bit: u32)
        requires
            bit_at(oa, bit),
            !bit_at(ua, bit),
        ensures
            !bit_at(!(oa & !ua) & !ua, bit),
    {
        assert(
            (bit_at(oa, bit) && !bit_at(ua, bit)) ==> !bit_at(!(oa & !ua) & !ua, bit)
        ) by (bit_vector);
    }

    proof fn not_on(oa: u64, ua: u64, bit: u32)
        requires
            !bit_at(oa, bit),
            !bit_at(ua, bit),
        ensures
            bit_at(!(oa & !ua) & !ua, bit),
    {
        assert(
            (!bit_at(oa, bit) && !bit_at(ua, bit)) ==> bit_at(!(oa & !ua) & !ua, bit)
        ) by (bit_vector);
    }

    pub proof fn not_get(self, bit: u32)
        ensures self.not_spec().get(bit) == knot(self.get(bit)),
    {
        if self.unknowns.bit(bit) {
            assert(self.get(bit) == Trit::U);
            assert(self.not_spec().unknowns.bit(bit));
            assert(self.not_spec().get(bit) == Trit::U);
        } else if self.ones.bit(bit) {
            Self::not_off(self.ones.0, self.unknowns.0, bit);
            assert(self.get(bit) == Trit::T);
            assert(!self.not_spec().unknowns.bit(bit));
            assert(!self.not_spec().ones.bit(bit));
            assert(self.not_spec().get(bit) == Trit::F);
        } else {
            Self::not_on(self.ones.0, self.unknowns.0, bit);
            assert(self.get(bit) == Trit::F);
            assert(!self.not_spec().unknowns.bit(bit));
            assert(self.not_spec().ones.bit(bit));
            assert(self.not_spec().get(bit) == Trit::T);
        }
    }
    proof fn and_res1_one(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            bit_at(oa, bit),
            !bit_at(ua, bit),
            bit_at(ob, bit),
            !bit_at(ub, bit),
        ensures
            bit_at((oa & !ua) & (ob & !ub), bit),
    {
        assert(
            (bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit))
                ==> bit_at((oa & !ua) & (ob & !ub), bit)
        ) by (bit_vector);
    }

    proof fn and_res1_zero(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            !(bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit)),
        ensures
            !bit_at((oa & !ua) & (ob & !ub), bit),
    {
        assert(
            (!(bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit)))
                ==> !bit_at((oa & !ua) & (ob & !ub), bit)
        ) by (bit_vector);
    }

    proof fn and_resx_zero(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            (!bit_at(oa, bit) && !bit_at(ua, bit))
                || (!bit_at(ob, bit) && !bit_at(ub, bit))
                || (bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit)),
        ensures
            !bit_at(!((!oa & !ua) | (!ob & !ub)) & !((oa & !ua) & (ob & !ub)), bit),
    {
        assert(
            (((!bit_at(oa, bit) && !bit_at(ua, bit))
                || (!bit_at(ob, bit) && !bit_at(ub, bit))
                || (bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> !bit_at(!((!oa & !ua) | (!ob & !ub)) & !((oa & !ua) & (ob & !ub)), bit)
        ) by (bit_vector);
    }

    proof fn and_resx_one(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            !((!bit_at(oa, bit) && !bit_at(ua, bit))
                || (!bit_at(ob, bit) && !bit_at(ub, bit))
                || (bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit))),
        ensures
            bit_at(!((!oa & !ua) | (!ob & !ub)) & !((oa & !ua) & (ob & !ub)), bit),
    {
        assert(
            (!((!bit_at(oa, bit) && !bit_at(ua, bit))
                || (!bit_at(ob, bit) && !bit_at(ub, bit))
                || (bit_at(oa, bit) && !bit_at(ua, bit) && bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> bit_at(!((!oa & !ua) | (!ob & !ub)) & !((oa & !ua) & (ob & !ub)), bit)
        ) by (bit_vector);
    }

    pub proof fn and_lattice_get(self, other: TriBit, bit: u32)
        ensures self.and_lattice_spec(other).get(bit) == tand(self.get(bit), other.get(bit)),
    {
        let a = self.get(bit);
        let b = other.get(bit);
        if a == Trit::F || b == Trit::F {
            Self::and_res1_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::and_resx_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.and_lattice_spec(other).get(bit) == Trit::F);
            assert(tand(a, b) == Trit::F);
        } else if a == Trit::T && b == Trit::T {
            Self::and_res1_one(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::and_resx_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.and_lattice_spec(other).get(bit) == Trit::T);
            assert(tand(a, b) == Trit::T);
        } else {
            Self::and_res1_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::and_resx_one(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.and_lattice_spec(other).get(bit) == Trit::U);
            assert(tand(a, b) == Trit::U);
        }
    }

    proof fn or_res1_one(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            (bit_at(oa, bit) && !bit_at(ua, bit)) || (bit_at(ob, bit) && !bit_at(ub, bit)),
        ensures
            bit_at((oa & !ua) | (ob & !ub), bit),
    {
        assert(
            (((bit_at(oa, bit) && !bit_at(ua, bit)) || (bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> bit_at((oa & !ua) | (ob & !ub), bit)
        ) by (bit_vector);
    }

    proof fn or_res1_zero(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            !((bit_at(oa, bit) && !bit_at(ua, bit)) || (bit_at(ob, bit) && !bit_at(ub, bit))),
        ensures
            !bit_at((oa & !ua) | (ob & !ub), bit),
    {
        assert(
            (!((bit_at(oa, bit) && !bit_at(ua, bit)) || (bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> !bit_at((oa & !ua) | (ob & !ub), bit)
        ) by (bit_vector);
    }

    proof fn or_resx_zero(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            (bit_at(oa, bit) && !bit_at(ua, bit))
                || (bit_at(ob, bit) && !bit_at(ub, bit))
                || (!bit_at(oa, bit) && !bit_at(ua, bit) && !bit_at(ob, bit) && !bit_at(ub, bit)),
        ensures
            !bit_at(!((oa & !ua) | (ob & !ub)) & !((!oa & !ua) & (!ob & !ub)), bit),
    {
        assert(
            (((bit_at(oa, bit) && !bit_at(ua, bit))
                || (bit_at(ob, bit) && !bit_at(ub, bit))
                || (!bit_at(oa, bit) && !bit_at(ua, bit) && !bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> !bit_at(!((oa & !ua) | (ob & !ub)) & !((!oa & !ua) & (!ob & !ub)), bit)
        ) by (bit_vector);
    }

    proof fn or_resx_one(oa: u64, ua: u64, ob: u64, ub: u64, bit: u32)
        requires
            !((bit_at(oa, bit) && !bit_at(ua, bit))
                || (bit_at(ob, bit) && !bit_at(ub, bit))
                || (!bit_at(oa, bit) && !bit_at(ua, bit) && !bit_at(ob, bit) && !bit_at(ub, bit))),
        ensures
            bit_at(!((oa & !ua) | (ob & !ub)) & !((!oa & !ua) & (!ob & !ub)), bit),
    {
        assert(
            (!((bit_at(oa, bit) && !bit_at(ua, bit))
                || (bit_at(ob, bit) && !bit_at(ub, bit))
                || (!bit_at(oa, bit) && !bit_at(ua, bit) && !bit_at(ob, bit) && !bit_at(ub, bit))))
                ==> bit_at(!((oa & !ua) | (ob & !ub)) & !((!oa & !ua) & (!ob & !ub)), bit)
        ) by (bit_vector);
    }

    pub proof fn or_lattice_get(self, other: TriBit, bit: u32)
        ensures self.or_lattice_spec(other).get(bit) == tor(self.get(bit), other.get(bit)),
    {
        let a = self.get(bit);
        let b = other.get(bit);
        if a == Trit::T || b == Trit::T {
            Self::or_res1_one(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::or_resx_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.or_lattice_spec(other).get(bit) == Trit::T);
            assert(tor(a, b) == Trit::T);
        } else if a == Trit::F && b == Trit::F {
            Self::or_res1_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::or_resx_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.or_lattice_spec(other).get(bit) == Trit::F);
            assert(tor(a, b) == Trit::F);
        } else {
            Self::or_res1_zero(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            Self::or_resx_one(self.ones.0, self.unknowns.0, other.ones.0, other.unknowns.0, bit);
            assert(self.or_lattice_spec(other).get(bit) == Trit::U);
            assert(tor(a, b) == Trit::U);
        }
    }
}

}
