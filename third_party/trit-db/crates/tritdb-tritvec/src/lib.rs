//! Packed ternary vectors (K3 semantics) and balanced-ternary integers.
//!
//! This is the M8 groundwork for "Q 三进制化": q's vectors and verbs, but with
//! trits as the data model. [`TritVec`] stores Kleene-three-valued vectors in
//! the `r3` packing from `tritdb-radix` (5 trits per byte), and [`TritInt`]
//! gives balanced-ternary (`-1/0/+1`) integers over `i64`.
//!
//! Nothing here decides truth on its own: the mapping between a [`Trit`] and a
//! semantic value is fixed by the two types below (`T0 = FALSE`, `T1 =
//! UNKNOWN`, `T2 = TRUE` for K3), and callers that need Belnap or Łukasiewicz
//! should build their own mapping instead of reusing [`K3`].

use std::fmt;

pub use tritdb_radix::{pack, unpack, Radix, Trit};

/// Kleene three-valued logic: `FALSE < UNKNOWN < TRUE`.
///
/// The representation order matches [`Trit`]: `T0 = False`, `T1 = Unknown`,
/// `T2 = True`, so `meet` is `min` and `join` is `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum K3 {
    /// Definitely false.
    False,
    /// Unknown / missing.
    Unknown,
    /// Definitely true.
    True,
}

impl K3 {
    /// All values, in representation order.
    pub const ALL: [K3; 3] = [K3::False, K3::Unknown, K3::True];

    /// Maps a representation trit to K3.
    pub const fn from_trit(trit: Trit) -> Self {
        match trit.get() {
            0 => K3::False,
            1 => K3::Unknown,
            _ => K3::True,
        }
    }

    /// Maps K3 back to its representation trit.
    pub const fn to_trit(self) -> Trit {
        match self {
            K3::False => Trit::T0,
            K3::Unknown => Trit::T1,
            K3::True => Trit::T2,
        }
    }

    /// Kleene negation: `False <-> True`, `Unknown` unchanged.
    pub const fn neg(self) -> Self {
        match self {
            K3::False => K3::True,
            K3::Unknown => K3::Unknown,
            K3::True => K3::False,
        }
    }

    /// Kleene conjunction; `False` absorbs.
    pub const fn meet(self, other: Self) -> Self {
        match (self, other) {
            (K3::False, _) | (_, K3::False) => K3::False,
            (K3::Unknown, _) | (_, K3::Unknown) => K3::Unknown,
            (K3::True, K3::True) => K3::True,
        }
    }

    /// Kleene disjunction; `True` absorbs.
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (K3::True, _) | (_, K3::True) => K3::True,
            (K3::Unknown, _) | (_, K3::Unknown) => K3::Unknown,
            (K3::False, K3::False) => K3::False,
        }
    }
}

impl fmt::Display for K3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            K3::False => "F",
            K3::Unknown => "U",
            K3::True => "T",
        })
    }
}

/// Errors from vector and integer operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TritVecError {
    /// Two vectors of different lengths were combined element-wise.
    LengthMismatch {
        /// Left length.
        expected: usize,
        /// Right length.
        actual: usize,
    },
    /// A balanced-ternary integer overflowed `i64`.
    Overflow,
    /// A weight was `NaN` or infinite.
    NonFiniteWeight,
    /// The quantization step was non-positive or non-finite.
    InvalidScale,
}

impl fmt::Display for TritVecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TritVecError::LengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "vector length mismatch: expected {expected}, got {actual}"
                )
            }
            TritVecError::Overflow => formatter.write_str("balanced-ternary value overflows i64"),
            TritVecError::NonFiniteWeight => formatter.write_str("weight must be a finite number"),
            TritVecError::InvalidScale => {
                formatter.write_str("quantization step must be finite and positive")
            }
        }
    }
}

impl std::error::Error for TritVecError {}

/// A packed K3 vector: `r3` gives 5 trits per byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TritVec {
    bytes: Vec<u8>,
    len: usize,
}

impl TritVec {
    /// Builds a vector from K3 values.
    pub fn new(values: &[K3]) -> Self {
        let trits: Vec<Trit> = values.iter().map(|value| value.to_trit()).collect();
        let bytes =
            tritdb_radix::pack(tritdb_radix::Radix::R3, &trits).expect("K3 values always pack");
        Self {
            bytes,
            len: values.len(),
        }
    }

    /// Number of elements.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the vector holds no elements.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Packed bytes (`5` trits per byte, `r3`).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Unpacks one element.
    pub fn get(&self, index: usize) -> Option<K3> {
        if index >= self.len {
            return None;
        }
        let trits = unpack(tritdb_radix::Radix::R3, &self.bytes, self.len).ok()?;
        trits.get(index).map(|trit| K3::from_trit(*trit))
    }

    /// Unpacks all elements.
    pub fn to_vec(&self) -> Vec<K3> {
        unpack(tritdb_radix::Radix::R3, &self.bytes, self.len)
            .expect("packed vector keeps canonical padding")
            .into_iter()
            .map(K3::from_trit)
            .collect()
    }

    /// Element-wise Kleene negation.
    pub fn neg(&self) -> Self {
        Self::new(&self.to_vec().into_iter().map(K3::neg).collect::<Vec<K3>>())
    }

    /// Element-wise Kleene conjunction.
    pub fn meet(&self, other: &Self) -> Result<Self, TritVecError> {
        if self.len != other.len {
            return Err(TritVecError::LengthMismatch {
                expected: self.len,
                actual: other.len,
            });
        }
        let right = other.to_vec();
        let values: Vec<K3> = self
            .to_vec()
            .into_iter()
            .zip(right)
            .map(|(left, right)| left.meet(right))
            .collect();
        Ok(Self::new(&values))
    }

    /// Element-wise Kleene disjunction.
    pub fn join(&self, other: &Self) -> Result<Self, TritVecError> {
        if self.len != other.len {
            return Err(TritVecError::LengthMismatch {
                expected: self.len,
                actual: other.len,
            });
        }
        let right = other.to_vec();
        let values: Vec<K3> = self
            .to_vec()
            .into_iter()
            .zip(right)
            .map(|(left, right)| left.join(right))
            .collect();
        Ok(Self::new(&values))
    }

    /// Reduces with `join` (`∨/` in q terms); the empty vector is `False`.
    pub fn any_true(&self) -> K3 {
        self.to_vec()
            .into_iter()
            .fold(K3::False, |accumulator, value| accumulator.join(value))
    }

    /// Reduces with `meet` (`∧/`); the empty vector is `True`.
    pub fn all_true(&self) -> K3 {
        self.to_vec()
            .into_iter()
            .fold(K3::True, |accumulator, value| accumulator.meet(value))
    }

    /// Number of `UNKNOWN` elements.
    pub fn unknown_count(&self) -> usize {
        self.to_vec()
            .into_iter()
            .filter(|value| *value == K3::Unknown)
            .count()
    }
}

impl fmt::Display for TritVec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for value in self.to_vec() {
            write!(formatter, "{value}")?;
        }
        Ok(())
    }
}

/// One digit of a balanced-ternary number: `-1`, `0`, or `+1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BalancedTrit {
    /// `-1`.
    Neg,
    /// `0`.
    Zero,
    /// `+1`.
    Pos,
}

impl BalancedTrit {
    /// Numeric value (`-1`, `0`, `+1`).
    pub const fn value(self) -> i64 {
        match self {
            BalancedTrit::Neg => -1,
            BalancedTrit::Zero => 0,
            BalancedTrit::Pos => 1,
        }
    }

    /// Maps an `r3` representation trit (`0/1/2`) to `-1/0/+1`.
    pub const fn from_trit(trit: Trit) -> Self {
        match trit.get() {
            0 => BalancedTrit::Neg,
            1 => BalancedTrit::Zero,
            _ => BalancedTrit::Pos,
        }
    }

    /// Maps `-1/0/+1` back to the `r3` representation trit.
    pub const fn to_trit(self) -> Trit {
        match self {
            BalancedTrit::Neg => Trit::T0,
            BalancedTrit::Zero => Trit::T1,
            BalancedTrit::Pos => Trit::T2,
        }
    }
}

/// A balanced-ternary integer backed by `i64`.
///
/// The value domain is `i64`; the digit expansion needs at most 41 trits
/// (`(3^41 - 1) / 2 > i64::MAX`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TritInt {
    value: i64,
}

impl TritInt {
    /// Wraps an `i64`.
    pub const fn from_i64(value: i64) -> Self {
        Self { value }
    }

    /// Returns the underlying `i64`.
    pub const fn to_i64(self) -> i64 {
        self.value
    }

    /// Least-significant-first balanced-ternary digits.
    pub fn trits(self) -> Vec<BalancedTrit> {
        //  keeps the carry safe at `i64::MIN` (where `value -= 1`
        // would overflow the narrower type).
        let mut value = i128::from(self.value);
        let mut out = Vec::new();
        while value != 0 {
            // Rust's `%` truncates towards zero, so the remainder is in
            // `-2..=2`; fold `±2` into `∓1` and carry.
            let mut digit = (value % 3) as i64;
            if digit == 2 {
                digit = -1;
                value += 1;
            } else if digit == -2 {
                digit = 1;
                value -= 1;
            }
            out.push(match digit {
                -1 => BalancedTrit::Neg,
                0 => BalancedTrit::Zero,
                _ => BalancedTrit::Pos,
            });
            value /= 3;
        }
        out
    }

    /// Rebuilds an integer from least-significant-first digits.
    pub fn from_trits(trits: &[BalancedTrit]) -> Result<Self, TritVecError> {
        let mut value: i128 = 0;
        let mut weight: i128 = 1;
        for (index, trit) in trits.iter().enumerate() {
            if index > 0 {
                weight = weight.checked_mul(3).ok_or(TritVecError::Overflow)?;
            }
            let contribution = i128::from(trit.value())
                .checked_mul(weight)
                .ok_or(TritVecError::Overflow)?;
            value = value
                .checked_add(contribution)
                .ok_or(TritVecError::Overflow)?;
        }
        i64::try_from(value)
            .map(Self::from_i64)
            .map_err(|_| TritVecError::Overflow)
    }

    /// Checked addition.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.value.checked_add(other.value).map(Self::from_i64)
    }

    /// Checked negation.
    pub fn checked_neg(self) -> Option<Self> {
        self.value.checked_neg().map(Self::from_i64)
    }

    /// Checked multiplication.
    pub fn checked_mul(self, other: Self) -> Option<Self> {
        self.value.checked_mul(other.value).map(Self::from_i64)
    }
}

impl fmt::Display for TritInt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for trit in self.trits().into_iter().rev() {
            formatter.write_str(match trit {
                BalancedTrit::Neg => "-",
                BalancedTrit::Zero => "0",
                BalancedTrit::Pos => "+",
            })?;
        }
        if self.value == 0 {
            formatter.write_str("0")?;
        }
        Ok(())
    }
}

/// A packed vector of balanced trits (`-1/0/+1`).
///
/// This is the representation ternary-quantized model weights use: `r3`
/// packing gives `1.6` bit per weight, matching BitNet's 1.58-bit ternary
/// weights, and the same type is the ballot vector for trit voting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TernaryVec {
    bytes: Vec<u8>,
    len: usize,
}

impl TernaryVec {
    /// Builds a vector from balanced digits.
    pub fn new(digits: &[BalancedTrit]) -> Self {
        let trits: Vec<Trit> = digits.iter().map(|digit| digit.to_trit()).collect();
        let bytes = tritdb_radix::pack(tritdb_radix::Radix::R3, &trits)
            .expect("balanced digits always pack");
        Self {
            bytes,
            len: digits.len(),
        }
    }

    /// Quantizes floating-point weights to `-1/0/+1` with step `alpha`.
    ///
    /// Each weight becomes `round(w / alpha)` clamped to `[-1, 1]`, so
    /// dequantizing with the same `alpha` keeps the error `<= alpha / 2` for
    /// `|w| <= 1.5 * alpha` (the BitNet / ternary-quantization contract);
    /// weights beyond that band saturate at `±1`. `alpha` is usually
    /// `mean(|w|)`.
    pub fn quantize(weights: &[f64], alpha: f64) -> Result<Self, TritVecError> {
        if !alpha.is_finite() || alpha <= 0.0 {
            return Err(TritVecError::InvalidScale);
        }
        let mut digits = Vec::with_capacity(weights.len());
        for weight in weights {
            if !weight.is_finite() {
                return Err(TritVecError::NonFiniteWeight);
            }
            let scaled = (weight / alpha).round();
            digits.push(if scaled >= 1.0 {
                BalancedTrit::Pos
            } else if scaled <= -1.0 {
                BalancedTrit::Neg
            } else {
                BalancedTrit::Zero
            });
        }
        Ok(Self::new(&digits))
    }

    /// Number of elements.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the vector holds no elements.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Unpacks all digits.
    pub fn digits(&self) -> Vec<BalancedTrit> {
        tritdb_radix::unpack(tritdb_radix::Radix::R3, &self.bytes, self.len)
            .expect("packed vector keeps canonical padding")
            .into_iter()
            .map(BalancedTrit::from_trit)
            .collect()
    }

    /// Dequantizes with the same step used by [`TernaryVec::quantize`].
    pub fn dequantize(&self, alpha: f64) -> Vec<f64> {
        self.digits()
            .into_iter()
            .map(|digit| digit.value() as f64 * alpha)
            .collect()
    }

    /// Unbiased tally of `+1/-1/0` ballots.
    pub fn tally(&self) -> TritInt {
        let total: i64 = self.digits().iter().map(|digit| digit.value()).sum();
        TritInt::from_i64(total)
    }
}

/// Maps a model confidence in `[0, 1]` to a Łukasiewicz annotation in
/// thousandths (`0..=1000`), clamping out-of-range values.
///
/// This is the adapter from a model's softmax/score output to the
/// `LOGIC LUKASIEWICZ` storage domain.
pub fn confidence_to_annotation(confidence: f64) -> Result<u16, TritVecError> {
    if !confidence.is_finite() {
        return Err(TritVecError::NonFiniteWeight);
    }
    let clamped = confidence.clamp(0.0, 1.0);
    Ok((clamped * 1000.0).round() as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec(values: &[K3]) -> TritVec {
        TritVec::new(values)
    }

    #[test]
    fn k3_tables_match_kleene() {
        for value in K3::ALL {
            assert_eq!(value.neg().neg(), value);
            assert_eq!(value.meet(K3::False), K3::False);
            assert_eq!(value.join(K3::True), K3::True);
            assert_eq!(value.meet(K3::True), value);
            assert_eq!(value.join(K3::False), value);
        }
        assert_eq!(K3::Unknown.meet(K3::Unknown), K3::Unknown);
        assert_eq!(K3::Unknown.join(K3::Unknown), K3::Unknown);
        assert_eq!(K3::Unknown.meet(K3::False), K3::False);
        assert_eq!(K3::Unknown.join(K3::True), K3::True);
    }

    #[test]
    fn k3_round_trips_through_trits() {
        for value in K3::ALL {
            assert_eq!(K3::from_trit(value.to_trit()), value);
        }
    }

    #[test]
    fn vectors_pack_and_unpack() {
        let mut rng = tritdb_testkit::Rng::new(0x5EED_1234);
        for len in [0usize, 1, 2, 4, 5, 6, 13, 40, 257] {
            let values: Vec<K3> = (0..len)
                .map(|_| match rng.trit() {
                    0 => K3::False,
                    1 => K3::Unknown,
                    _ => K3::True,
                })
                .collect();
            let packed = vec(&values);
            assert_eq!(packed.len(), len);
            assert_eq!(packed.to_vec(), values);
            for (index, value) in values.iter().enumerate() {
                assert_eq!(packed.get(index), Some(*value));
            }
            assert_eq!(packed.get(len), None);
            // r3 packs 5 trits per byte.
            assert_eq!(packed.bytes().len(), len.div_ceil(5));
        }
    }

    #[test]
    fn vector_verbs_are_element_wise() {
        let left = vec(&[K3::False, K3::Unknown, K3::True, K3::Unknown]);
        let right = vec(&[K3::True, K3::False, K3::False, K3::Unknown]);
        assert_eq!(
            left.meet(&right).unwrap().to_vec(),
            vec![K3::False, K3::False, K3::False, K3::Unknown]
        );
        assert_eq!(
            left.join(&right).unwrap().to_vec(),
            vec![K3::True, K3::Unknown, K3::True, K3::Unknown]
        );
        assert_eq!(
            left.neg().to_vec(),
            vec![K3::True, K3::Unknown, K3::False, K3::Unknown]
        );
    }

    #[test]
    fn vector_verbs_reject_length_mismatch() {
        let left = vec(&[K3::True]);
        let right = vec(&[K3::True, K3::False]);
        assert_eq!(
            left.meet(&right),
            Err(TritVecError::LengthMismatch {
                expected: 1,
                actual: 2
            })
        );
        assert_eq!(
            left.join(&right),
            Err(TritVecError::LengthMismatch {
                expected: 1,
                actual: 2
            })
        );
    }

    #[test]
    fn reductions_follow_kleene_units() {
        assert_eq!(vec(&[]).any_true(), K3::False);
        assert_eq!(vec(&[]).all_true(), K3::True);
        assert_eq!(vec(&[K3::False, K3::Unknown]).any_true(), K3::Unknown);
        assert_eq!(vec(&[K3::True, K3::Unknown]).all_true(), K3::Unknown);
        assert_eq!(vec(&[K3::True, K3::True]).any_true(), K3::True);
        assert_eq!(vec(&[K3::False, K3::True]).all_true(), K3::False);
        assert_eq!(vec(&[K3::True, K3::Unknown, K3::False]).unknown_count(), 1);
    }

    #[test]
    fn balanced_trits_round_trip() {
        for value in [
            0i64,
            1,
            -1,
            2,
            -2,
            3,
            -3,
            4,
            -4,
            5,
            -5,
            13,
            -13,
            1_000_000,
            -1_000_000,
            i64::MAX,
            i64::MIN,
        ] {
            let integer = TritInt::from_i64(value);
            let digits = integer.trits();
            assert_eq!(
                TritInt::from_trits(&digits).unwrap(),
                integer,
                "value={value} digits={digits:?}"
            );
            let trits: Vec<Trit> = digits.iter().map(|trit| trit.to_trit()).collect();
            let packed = tritdb_radix::pack(tritdb_radix::Radix::R3, &trits).unwrap();
            let unpacked =
                tritdb_radix::unpack(tritdb_radix::Radix::R3, &packed, trits.len()).unwrap();
            let restored: Vec<BalancedTrit> = unpacked
                .iter()
                .map(|trit| BalancedTrit::from_trit(*trit))
                .collect();
            assert_eq!(TritInt::from_trits(&restored).unwrap(), integer);
        }
    }

    #[test]
    fn balanced_arithmetic_matches_i64() {
        let mut rng = tritdb_testkit::Rng::new(0xC0FF_EE00);
        for _ in 0..500 {
            let left = (rng.next_u64() as i64) % 1_000_000_007;
            let right = (rng.next_u64() as i64) % 1_000_000_007;
            let a = TritInt::from_i64(left);
            let b = TritInt::from_i64(right);
            assert_eq!(
                a.checked_add(b).map(TritInt::to_i64),
                Some(left.wrapping_add(right))
            );
            assert_eq!(
                a.checked_mul(b).map(TritInt::to_i64),
                Some(left.wrapping_mul(right))
            );
            assert_eq!(a.checked_neg().map(TritInt::to_i64), Some(-left));
        }
        assert_eq!(
            TritInt::from_i64(i64::MAX).checked_add(TritInt::from_i64(1)),
            None
        );
        assert_eq!(TritInt::from_i64(i64::MIN).checked_neg(), None);
    }

    #[test]
    fn ternary_quantization_round_trips_within_half_step() {
        // Within |w| <= 1.5 * alpha every weight rounds without clamping, so
        // the half-step error bound holds.
        let weights = [0.1, -0.1, 0.7, -0.7, 0.5, -0.5];
        let alpha = 0.5;
        let quantized = TernaryVec::quantize(&weights, alpha).unwrap();
        assert_eq!(
            quantized.digits(),
            vec![
                BalancedTrit::Zero,
                BalancedTrit::Zero,
                BalancedTrit::Pos,
                BalancedTrit::Neg,
                BalancedTrit::Pos,
                BalancedTrit::Neg,
            ]
        );
        let restored = quantized.dequantize(alpha);
        for (weight, back) in weights.iter().zip(&restored) {
            assert!(
                (weight - back).abs() <= alpha / 2.0 + 1e-12,
                "weight={weight} back={back}"
            );
        }
        // Packing stays at 5 digits per byte.
        assert_eq!(quantized.digits().len(), weights.len());
        // Weights beyond 1.5 * alpha saturate at +/-1; that is the documented
        // trade-off of ternary quantization, not a half-step guarantee.
        let clamped = TernaryVec::quantize(&[9.0, -9.0], 1.0).unwrap();
        assert_eq!(clamped.digits(), vec![BalancedTrit::Pos, BalancedTrit::Neg]);
    }

    #[test]
    fn ternary_quantization_rejects_bad_inputs() {
        assert_eq!(
            TernaryVec::quantize(&[0.1], 0.0),
            Err(TritVecError::InvalidScale)
        );
        assert_eq!(
            TernaryVec::quantize(&[0.1], f64::NAN),
            Err(TritVecError::InvalidScale)
        );
        assert_eq!(
            TernaryVec::quantize(&[f64::INFINITY], 1.0),
            Err(TritVecError::NonFiniteWeight)
        );
    }

    #[test]
    fn ternary_tally_is_an_unbiased_sum() {
        let ballots = TernaryVec::new(&[
            BalancedTrit::Pos,
            BalancedTrit::Neg,
            BalancedTrit::Zero,
            BalancedTrit::Pos,
        ]);
        assert_eq!(ballots.tally().to_i64(), 1);
        let all_against = TernaryVec::new(&[BalancedTrit::Neg, BalancedTrit::Neg]);
        assert_eq!(all_against.tally().to_i64(), -2);
        assert_eq!(TernaryVec::new(&[]).tally().to_i64(), 0);
    }

    #[test]
    fn confidence_maps_to_thousandths() {
        assert_eq!(confidence_to_annotation(0.0).unwrap(), 0);
        assert_eq!(confidence_to_annotation(0.5).unwrap(), 500);
        assert_eq!(confidence_to_annotation(1.0).unwrap(), 1000);
        assert_eq!(confidence_to_annotation(1.5).unwrap(), 1000);
        assert_eq!(confidence_to_annotation(-0.2).unwrap(), 0);
        assert_eq!(
            confidence_to_annotation(f64::NAN),
            Err(TritVecError::NonFiniteWeight)
        );
    }

    #[test]
    fn balanced_digits_fit_i64() {
        assert!(TritInt::from_i64(i64::MAX).trits().len() <= 41);
        assert!(TritInt::from_i64(i64::MIN).trits().len() <= 41);
        // Phrasing in the docs: 41 trits cover the whole i64 range.
        let digits = TritInt::from_i64(i64::MIN).trits();
        assert_eq!(TritInt::from_trits(&digits).unwrap().to_i64(), i64::MIN);
    }
}
