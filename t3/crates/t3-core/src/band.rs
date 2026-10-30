//! Calibrated bands: how a real number becomes a trit without a cliff.
//!
//! A hard threshold is the weakest link in every heuristic triage tool. If the
//! rule is "section entropy above 7.2 means packed", an adversary who lands at
//! 7.19 is declared clean, and the analyst has no way to see that they were one
//! hundredth of a bit from the other side. A second problem is measurement
//! noise: entropy of a 4 KiB section legitimately wobbles by a few hundredths.
//!
//! A band replaces the point with an interval:
//!
//! ```text
//!        value <= lo            lo < value < hi            value >= hi
//!            False                 Unknown                   True
//! ```
//!
//! `Unknown` now means something specific and defensible — *this measurement is
//! inside the region where the clean corpus and the packed corpus overlap* —
//! rather than "the tool could not be bothered".
//!
//! Bands here are meant to be filled in from a measured corpus (see
//! [`Band::calibrate`]); a hard threshold is the degenerate case `lo == hi`.

use crate::Tri;

/// An interval that maps a continuous measurement onto [`Tri`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// At or below this value the fact is `False`.
    pub lo: f32,
    /// At or above this value the fact is `True`.
    pub hi: f32,
}

impl Band {
    /// Builds a band, ordering the edges so `lo <= hi` always holds.
    #[must_use]
    pub fn new(lo: f32, hi: f32) -> Self {
        if lo <= hi {
            Band { lo, hi }
        } else {
            Band { lo: hi, hi: lo }
        }
    }

    /// A degenerate band: equivalent to `value >= threshold`.
    #[must_use]
    pub fn hard(threshold: f32) -> Self {
        Band {
            lo: threshold,
            hi: threshold,
        }
    }

    /// Width of the undecided region in measurement units.
    #[must_use]
    pub fn width(&self) -> f32 {
        self.hi - self.lo
    }

    /// True when the band has no undecided interior.
    #[must_use]
    pub fn is_hard(&self) -> bool {
        self.hi <= self.lo
    }

    /// Maps a measurement onto a trit.
    ///
    /// Non-finite input is `Unknown`: a NaN entropy means the measurement failed,
    /// and a failed measurement is not evidence of cleanliness.
    #[inline]
    #[must_use]
    pub fn classify(&self, value: f32) -> Tri {
        if !value.is_finite() {
            return Tri::Unknown;
        }
        if value >= self.hi {
            Tri::True
        } else if value <= self.lo {
            Tri::False
        } else {
            Tri::Unknown
        }
    }

    /// Derives a band from observations of *clean* files.
    ///
    /// `clean` should be the measured statistic over a corpus known to be benign;
    /// `quantile` is the point above which a clean file is considered surprising
    /// (0.995 is a reasonable default). The upper edge is placed `slack` units
    /// above that — deliberately *above* the clean tail rather than at it, so
    /// that a clean file sitting at the tail is reported as `Unknown` instead of
    /// being called packed.
    ///
    /// Nothing here knows what a packed file looks like. That is a limitation:
    /// the lower edge is data-driven, the width is a stated design margin. Feed a
    /// known-bad corpus through [`Band::from_two_corpora`] when one is available.
    #[must_use]
    pub fn calibrate(clean: &mut [f32], quantile: f32, slack: f32) -> Self {
        let q = quantile_of(clean, quantile);
        Band::new(q, q + slack)
    }

    /// Derives a band from two labelled corpora: the midpoint of the gap between
    /// the clean tail and the packed head.
    ///
    /// Returns `None` when the corpora overlap and no honest separation exists —
    /// in that case the statistic simply cannot distinguish the classes and a
    /// band would be a fiction.
    #[must_use]
    pub fn from_two_corpora(
        clean: &mut [f32],
        packed: &mut [f32],
        clean_quantile: f32,
        packed_quantile: f32,
    ) -> Option<Self> {
        let clean_tail = quantile_of(clean, clean_quantile);
        let packed_head = quantile_of(packed, packed_quantile);
        if packed_head <= clean_tail {
            return None;
        }
        let mid = 0.5 * (clean_tail + packed_head);
        // A narrow undecided region: both edges are measured, so the band only
        // has to absorb noise around the separation point.
        let slack = (packed_head - clean_tail) * 0.1;
        Some(Band::new(mid - slack, mid + slack))
    }
}

/// Linear-interpolated quantile of `values`, which is reordered in place.
///
/// Hand-rolled to keep the crate dependency-free; `f32` totals are accumulated in
/// `f64` so that large corpora do not lose precision in the index arithmetic.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "len 受 isize 限制且 q 已钳在 [0,1]，pos 落在 [0, len-1] 内，三类转换都不会越界"
)]
pub fn quantile_of(values: &mut [f32], q: f32) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    values.sort_unstable_by(f32::total_cmp);
    let q = q.clamp(0.0, 1.0);
    let pos = f64::from(q) * (values.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        return values[lo];
    }
    let frac = (pos - lo as f64) as f32;
    values[lo] * (1.0 - frac) + values[hi] * frac
}

#[cfg(test)]
mod tests {
    // 测试数据构造使用 i as f32 直接铺点，精度不影响断言语义。
    #![allow(clippy::cast_precision_loss)]
    use super::{quantile_of, Band};
    use crate::Tri;

    #[test]
    fn band_regions_are_disjoint() {
        let band = Band::new(1.0, 2.0);
        assert_eq!(band.classify(0.5), Tri::False);
        assert_eq!(band.classify(1.0), Tri::False);
        assert_eq!(band.classify(1.5), Tri::Unknown);
        assert_eq!(band.classify(2.0), Tri::True);
        assert_eq!(band.classify(9.0), Tri::True);
    }

    #[test]
    fn classification_is_monotone_in_the_measurement() {
        // Sweeping a value upwards must never go True -> False. This is what
        // makes the band safe to put inside a monotone formula: a larger
        // measurement can only ever push the verdict up.
        let band = Band::new(-1.0, 1.0);
        let mut steps = Vec::new();
        let mut x = -5.0f32;
        while x <= 5.0 {
            steps.push(band.classify(x).rank());
            x += 0.01;
        }
        for w in steps.windows(2) {
            assert!(w[0] <= w[1], "band classification went backwards");
        }
    }

    #[test]
    fn non_finite_measurements_are_unknown() {
        // A NaN or infinite entropy means the measurement failed. Treating failure
        // as "definitely packed" would turn a bug into a finding, and treating it
        // as "clean" would silently drop evidence; `Unknown` is the only honest
        // reading.
        let band = Band::hard(1.0);
        assert_eq!(band.classify(f32::NAN), Tri::Unknown);
        assert_eq!(band.classify(f32::INFINITY), Tri::Unknown);
        assert_eq!(band.classify(f32::NEG_INFINITY), Tri::Unknown);
    }

    #[test]
    fn hard_band_matches_threshold() {
        let band = Band::hard(7.2);
        assert!(band.is_hard());
        assert_eq!(band.classify(7.199), Tri::False);
        assert_eq!(band.classify(7.2), Tri::True);
    }

    #[test]
    fn edges_are_reordered() {
        let band = Band::new(5.0, 1.0);
        assert_eq!(band.lo, 1.0);
        assert_eq!(band.hi, 5.0);
    }

    #[test]
    fn quantile_is_interpolated() {
        let mut v = [0.0f32, 1.0, 2.0, 3.0];
        assert_eq!(quantile_of(&mut v, 0.0), 0.0);
        assert_eq!(quantile_of(&mut v, 1.0), 3.0);
        assert!((quantile_of(&mut v, 0.5) - 1.5).abs() < 1e-6);
        assert!(quantile_of(&mut [], 0.5).is_nan());
    }

    #[test]
    fn calibration_places_unknown_above_the_clean_tail() {
        // A clean corpus topping out near 7.0 yields a band whose *lower* edge is
        // that tail, so a clean outlier is Unknown rather than True.
        let mut clean: Vec<f32> = (0..1000).map(|i| 6.0 + i as f32 * 0.001).collect();
        let band = Band::calibrate(&mut clean, 0.995, 0.35);
        assert!(band.lo >= 6.99, "lo={}", band.lo);
        assert!((band.width() - 0.35).abs() < 1e-6);
        assert_eq!(
            band.classify(6.995),
            Tri::Unknown,
            "clean tail must be unknown"
        );
        assert_eq!(band.classify(7.9), Tri::True);
        assert_eq!(band.classify(6.0), Tri::False);
    }

    #[test]
    fn overlapping_corpora_yield_no_band() {
        let mut clean = [7.0f32, 7.1, 7.2];
        let mut packed = [6.8f32, 6.9, 7.0];
        assert!(Band::from_two_corpora(&mut clean, &mut packed, 0.99, 0.01).is_none());
    }

    #[test]
    fn separated_corpora_yield_a_band_between_them() {
        let mut clean: Vec<f32> = (0..100).map(|i| 6.0 + i as f32 * 0.001).collect();
        let mut packed: Vec<f32> = (0..100).map(|i| 7.8 + i as f32 * 0.001).collect();
        let band = Band::from_two_corpora(&mut clean, &mut packed, 0.99, 0.01).unwrap();
        assert_eq!(band.classify(5.0), Tri::False);
        assert_eq!(band.classify(8.0), Tri::True);
    }
}
