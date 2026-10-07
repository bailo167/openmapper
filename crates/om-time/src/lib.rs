// SPDX-License-Identifier: Apache-2.0
//! Exact audiovisual time.
//!
//! Timeline positions are never stored as floating-point seconds. A
//! [`RationalTime`] is `ticks / ticks_per_second`, kept in lowest terms, so
//! fractional frame rates (e.g. 30000/1001) and audio sample positions can be
//! represented without drift. All arithmetic is checked: overflow is an error,
//! never a wrap or a panic.
//!
//! [`DEFAULT_TICKS_PER_SECOND`] (254 016 000 000) is divisible by every common
//! video rate (including the NTSC 1001 family) and audio rate, so converting a
//! frame or sample index into project ticks is exact.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Project timebase: ticks per second.
pub const DEFAULT_TICKS_PER_SECOND: i128 = 254_016_000_000;

/// Errors from time arithmetic and parsing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("time denominator must be positive, got {0}")]
    NonPositiveDenominator(i128),
    #[error("rate must have a positive numerator and denominator, got {num}/{den}")]
    InvalidRate { num: i64, den: i64 },
    #[error("time arithmetic overflowed")]
    Overflow,
    #[error("invalid integer string {0:?}")]
    Parse(String),
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Floor division for i128 (rounds toward negative infinity).
fn div_floor(a: i128, b: i128) -> Option<i128> {
    let q = a.checked_div(b)?;
    let r = a.checked_rem(b)?;
    if r != 0 && ((r < 0) != (b < 0)) {
        q.checked_sub(1)
    } else {
        Some(q)
    }
}

/// An exact point or duration in time: `ticks / ticks_per_second` seconds.
///
/// Always normalised (lowest terms, positive denominator), so derived
/// equality is value equality.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RationalTime {
    ticks: i128,
    ticks_per_second: i128,
}

impl RationalTime {
    pub const ZERO: Self = Self {
        ticks: 0,
        ticks_per_second: 1,
    };

    /// Creates a normalised time. The denominator must be positive.
    pub fn new(ticks: i128, ticks_per_second: i128) -> Result<Self, TimeError> {
        if ticks_per_second <= 0 {
            return Err(TimeError::NonPositiveDenominator(ticks_per_second));
        }
        let g = gcd(ticks, ticks_per_second).max(1);
        Ok(Self {
            ticks: ticks / g,
            ticks_per_second: ticks_per_second / g,
        })
    }

    /// Whole seconds.
    #[must_use]
    pub fn from_seconds(seconds: i64) -> Self {
        Self {
            ticks: i128::from(seconds),
            ticks_per_second: 1,
        }
    }

    #[must_use]
    pub const fn ticks(self) -> i128 {
        self.ticks
    }

    #[must_use]
    pub const fn ticks_per_second(self) -> i128 {
        self.ticks_per_second
    }

    /// Time of frame `index` at `rate` (exact).
    pub fn from_frame(index: i64, rate: Rate) -> Result<Self, TimeError> {
        // index / (num/den) = index * den / num
        let ticks = i128::from(index)
            .checked_mul(i128::from(rate.den))
            .ok_or(TimeError::Overflow)?;
        Self::new(ticks, i128::from(rate.num))
    }

    /// Index of the frame at `rate` that is displayed at this time
    /// (floor: a frame covers `[start, start + duration)`).
    pub fn to_frame_floor(self, rate: Rate) -> Result<i64, TimeError> {
        // floor(ticks/tps * num/den)
        let num = self
            .ticks
            .checked_mul(i128::from(rate.num))
            .ok_or(TimeError::Overflow)?;
        let den = self
            .ticks_per_second
            .checked_mul(i128::from(rate.den))
            .ok_or(TimeError::Overflow)?;
        let frame = div_floor(num, den).ok_or(TimeError::Overflow)?;
        i64::try_from(frame).map_err(|_| TimeError::Overflow)
    }

    /// Expresses this time in a target timebase, rounding toward negative
    /// infinity. Exact when the target is divisible by this denominator.
    pub fn to_ticks_floor(self, ticks_per_second: i128) -> Result<i128, TimeError> {
        if ticks_per_second <= 0 {
            return Err(TimeError::NonPositiveDenominator(ticks_per_second));
        }
        let num = self
            .ticks
            .checked_mul(ticks_per_second)
            .ok_or(TimeError::Overflow)?;
        div_floor(num, self.ticks_per_second).ok_or(TimeError::Overflow)
    }

    /// Lossy conversion for display and UI only. Never feed this back into
    /// scheduling.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_seconds_f64(self) -> f64 {
        self.ticks as f64 / self.ticks_per_second as f64
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, TimeError> {
        let (a, b, den) = self.common(rhs)?;
        Self::new(a.checked_add(b).ok_or(TimeError::Overflow)?, den)
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, TimeError> {
        let (a, b, den) = self.common(rhs)?;
        Self::new(a.checked_sub(b).ok_or(TimeError::Overflow)?, den)
    }

    /// Multiplies by an integer factor.
    pub fn checked_mul_int(self, factor: i128) -> Result<Self, TimeError> {
        Self::new(
            self.ticks.checked_mul(factor).ok_or(TimeError::Overflow)?,
            self.ticks_per_second,
        )
    }

    /// Multiplies by an exact rational speed.
    pub fn checked_mul_speed(self, speed: Speed) -> Result<Self, TimeError> {
        let ticks = self
            .ticks
            .checked_mul(i128::from(speed.num))
            .ok_or(TimeError::Overflow)?;
        let tps = self
            .ticks_per_second
            .checked_mul(i128::from(speed.den))
            .ok_or(TimeError::Overflow)?;
        Self::new(ticks, tps)
    }

    /// Euclidean remainder: the result is in `[0, modulus)`. Used for looping.
    pub fn rem_euclid(self, modulus: Self) -> Result<Self, TimeError> {
        if modulus.ticks <= 0 {
            return Err(TimeError::NonPositiveDenominator(modulus.ticks));
        }
        let (a, m, den) = self.common(modulus)?;
        Self::new(a.rem_euclid(m), den)
    }

    /// Absolute value.
    pub fn checked_abs(self) -> Result<Self, TimeError> {
        Ok(Self {
            ticks: self.ticks.checked_abs().ok_or(TimeError::Overflow)?,
            ticks_per_second: self.ticks_per_second,
        })
    }

    /// From a count of nanoseconds (exact).
    #[must_use]
    pub fn from_nanos(nanos: u128) -> Self {
        let ticks = i128::try_from(nanos).unwrap_or(i128::MAX);
        Self::new(ticks, 1_000_000_000).unwrap_or(Self::ZERO)
    }

    /// Exact comparison. Fails only if cross-multiplication overflows.
    pub fn checked_cmp(self, rhs: Self) -> Result<Ordering, TimeError> {
        let (a, b, _) = self.common(rhs)?;
        Ok(a.cmp(&b))
    }

    /// Numerators over a shared (least common) denominator.
    fn common(self, rhs: Self) -> Result<(i128, i128, i128), TimeError> {
        let g = gcd(self.ticks_per_second, rhs.ticks_per_second);
        let lhs_scale = rhs.ticks_per_second / g;
        let rhs_scale = self.ticks_per_second / g;
        let den = self
            .ticks_per_second
            .checked_mul(lhs_scale)
            .ok_or(TimeError::Overflow)?;
        let a = self
            .ticks
            .checked_mul(lhs_scale)
            .ok_or(TimeError::Overflow)?;
        let b = rhs
            .ticks
            .checked_mul(rhs_scale)
            .ok_or(TimeError::Overflow)?;
        Ok((a, b, den))
    }
}

impl Default for RationalTime {
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Debug for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}s", self.ticks, self.ticks_per_second)
    }
}

impl fmt::Display for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}s", self.as_seconds_f64())
    }
}

/// Serialised as `{"ticks": "<int>", "ticks_per_second": "<int>"}`; i128
/// values are strings because JSON numbers lose precision above 2^53.
#[derive(Serialize, Deserialize)]
struct RationalTimeRepr {
    ticks: I128Str,
    ticks_per_second: I128Str,
}

impl Serialize for RationalTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RationalTimeRepr {
            ticks: I128Str(self.ticks),
            ticks_per_second: I128Str(self.ticks_per_second),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RationalTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = RationalTimeRepr::deserialize(deserializer)?;
        Self::new(repr.ticks.0, repr.ticks_per_second.0).map_err(serde::de::Error::custom)
    }
}

/// An `i128` serialised as a decimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct I128Str(pub i128);

impl Serialize for I128Str {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for I128Str {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<i128>()
            .map(Self)
            .map_err(|_| serde::de::Error::custom(TimeError::Parse(s)))
    }
}

/// Playback speed as an exact ratio (`-1/1` is reverse, `1/2` half speed).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "SpeedRepr", into = "SpeedRepr")]
pub struct Speed {
    num: i32,
    den: i32,
}

#[derive(Serialize, Deserialize)]
struct SpeedRepr {
    num: i32,
    den: i32,
}

impl TryFrom<SpeedRepr> for Speed {
    type Error = TimeError;
    fn try_from(r: SpeedRepr) -> Result<Self, Self::Error> {
        Self::new(r.num, r.den)
    }
}

impl From<Speed> for SpeedRepr {
    fn from(s: Speed) -> Self {
        Self {
            num: s.num,
            den: s.den,
        }
    }
}

impl Speed {
    pub const NORMAL: Self = Self { num: 1, den: 1 };
    pub const STOPPED: Self = Self { num: 0, den: 1 };
    /// Largest supported magnitude.
    pub const MAX_MAGNITUDE: i32 = 16;

    pub fn new(num: i32, den: i32) -> Result<Self, TimeError> {
        if den <= 0 || num.unsigned_abs() > Self::MAX_MAGNITUDE.unsigned_abs() * den.unsigned_abs()
        {
            return Err(TimeError::InvalidRate {
                num: i64::from(num),
                den: i64::from(den),
            });
        }
        let g = i32::try_from(gcd(i128::from(num), i128::from(den)))
            .unwrap_or(1)
            .max(1);
        Ok(Self {
            num: num / g,
            den: den / g,
        })
    }

    /// Nearest speed with denominator 100 (for UI percent controls).
    pub fn from_percent(percent: i32) -> Result<Self, TimeError> {
        Self::new(percent, 100)
    }

    #[must_use]
    pub const fn num(self) -> i32 {
        self.num
    }

    #[must_use]
    pub const fn den(self) -> i32 {
        self.den
    }

    #[must_use]
    pub const fn is_reverse(self) -> bool {
        self.num < 0
    }

    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }
}

impl Default for Speed {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl fmt::Debug for Speed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}x", self.num, self.den)
    }
}

/// A frame or sample rate, `num/den` per second (e.g. 30000/1001).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RateRepr", into = "RateRepr")]
pub struct Rate {
    num: i64,
    den: i64,
}

#[derive(Serialize, Deserialize)]
struct RateRepr {
    num: i64,
    den: i64,
}

impl TryFrom<RateRepr> for Rate {
    type Error = TimeError;
    fn try_from(r: RateRepr) -> Result<Self, Self::Error> {
        Self::new(r.num, r.den)
    }
}

impl From<Rate> for RateRepr {
    fn from(r: Rate) -> Self {
        Self {
            num: r.num,
            den: r.den,
        }
    }
}

impl Rate {
    pub const FPS_24: Self = Self { num: 24, den: 1 };
    pub const FPS_25: Self = Self { num: 25, den: 1 };
    pub const FPS_30: Self = Self { num: 30, den: 1 };
    pub const FPS_50: Self = Self { num: 50, den: 1 };
    pub const FPS_60: Self = Self { num: 60, den: 1 };
    pub const FPS_23_976: Self = Self {
        num: 24000,
        den: 1001,
    };
    pub const FPS_29_97: Self = Self {
        num: 30000,
        den: 1001,
    };
    pub const FPS_59_94: Self = Self {
        num: 60000,
        den: 1001,
    };

    pub fn new(num: i64, den: i64) -> Result<Self, TimeError> {
        if num <= 0 || den <= 0 {
            return Err(TimeError::InvalidRate { num, den });
        }
        let g = i64::try_from(gcd(i128::from(num), i128::from(den))).unwrap_or(1);
        Ok(Self {
            num: num / g,
            den: den / g,
        })
    }

    #[must_use]
    pub const fn num(self) -> i64 {
        self.num
    }

    #[must_use]
    pub const fn den(self) -> i64 {
        self.den
    }

    /// Duration of one frame/sample.
    pub fn period(self) -> Result<RationalTime, TimeError> {
        RationalTime::new(i128::from(self.den), i128::from(self.num))
    }
}

impl fmt::Debug for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn normalises_to_lowest_terms() {
        let t = RationalTime::new(50, 100).unwrap();
        assert_eq!((t.ticks(), t.ticks_per_second()), (1, 2));
        assert_eq!(
            t,
            RationalTime::new(127_008_000_000, DEFAULT_TICKS_PER_SECOND).unwrap()
        );
        assert!(RationalTime::new(1, 0).is_err());
        assert!(RationalTime::new(1, -5).is_err());
    }

    #[test]
    fn default_timebase_is_exact_for_common_rates() {
        let rates = [
            Rate::FPS_23_976,
            Rate::FPS_24,
            Rate::FPS_25,
            Rate::FPS_29_97,
            Rate::FPS_30,
            Rate::FPS_50,
            Rate::FPS_59_94,
            Rate::FPS_60,
            Rate::new(44_100, 1).unwrap(),
            Rate::new(48_000, 1).unwrap(),
            Rate::new(96_000, 1).unwrap(),
            Rate::new(192_000, 1).unwrap(),
        ];
        for rate in rates {
            let period = rate.period().unwrap();
            assert_eq!(
                (period.ticks() * DEFAULT_TICKS_PER_SECOND) % period.ticks_per_second(),
                0,
                "rate {rate:?} not exact in default timebase"
            );
        }
    }

    #[test]
    fn ntsc_frames_do_not_drift_over_24_hours() {
        let rate = Rate::FPS_29_97;
        let frames_per_day: i64 = 30000 * 86_400 / 1001; // floor
        let mut t = RationalTime::ZERO;
        let period = rate.period().unwrap();
        // Accumulate one hour of frames by repeated addition, then compare
        // against direct computation.
        for _ in 0..(30000 * 3600 / 1001) {
            t = t.checked_add(period).unwrap();
        }
        assert_eq!(
            t,
            RationalTime::from_frame(30000 * 3600 / 1001, rate).unwrap()
        );
        let end = RationalTime::from_frame(frames_per_day, rate).unwrap();
        assert_eq!(end.to_frame_floor(rate).unwrap(), frames_per_day);
    }

    #[test]
    fn frame_floor_handles_negative_times() {
        let t = RationalTime::new(-1, 60).unwrap();
        assert_eq!(t.to_frame_floor(Rate::FPS_30).unwrap(), -1);
    }

    #[test]
    fn serialises_as_strings() {
        let t = RationalTime::new(1001, 30000).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"ticks":"1001","ticks_per_second":"30000"}"#);
        assert_eq!(serde_json::from_str::<RationalTime>(&json).unwrap(), t);
        assert!(
            serde_json::from_str::<RationalTime>(r#"{"ticks":"1","ticks_per_second":"0"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Rate>(r#"{"num":0,"den":1}"#).is_err());
    }

    #[test]
    fn overflow_is_an_error_not_a_panic() {
        let big = RationalTime::new(i128::MAX, 1).unwrap();
        assert_eq!(big.checked_add(big), Err(TimeError::Overflow));
        let odd = RationalTime::new(1, i128::MAX).unwrap();
        let odd2 = RationalTime::new(1, i128::MAX - 1).unwrap();
        assert_eq!(odd.checked_add(odd2), Err(TimeError::Overflow));
    }

    #[test]
    fn looping_and_speed_are_exact() {
        let d = RationalTime::from_frame(100, Rate::FPS_29_97).unwrap();
        let t = d
            .checked_mul_int(7)
            .unwrap()
            .checked_add(RationalTime::new(1, 3).unwrap())
            .unwrap();
        assert_eq!(t.rem_euclid(d).unwrap(), RationalTime::new(1, 3).unwrap());
        let neg = RationalTime::new(-1, 3).unwrap();
        assert_eq!(
            neg.rem_euclid(d).unwrap(),
            d.checked_sub(RationalTime::new(1, 3).unwrap()).unwrap()
        );
        let half = Speed::new(1, 2).unwrap();
        assert_eq!(
            RationalTime::from_seconds(3)
                .checked_mul_speed(half)
                .unwrap(),
            RationalTime::new(3, 2).unwrap()
        );
        assert!(Speed::new(17, 1).is_err());
        assert!(Speed::new(1, 0).is_err());
        assert_eq!(Speed::from_percent(50).unwrap(), half);
        assert_eq!(
            RationalTime::from_nanos(1_500_000_000),
            RationalTime::new(3, 2).unwrap()
        );
    }

    proptest! {
        #[test]
        fn add_sub_round_trip(a in -1_000_000_000i64..1_000_000_000, b in -1_000_000_000i64..1_000_000_000,
                              da in 1i64..100_000, db in 1i64..100_000) {
            let x = RationalTime::new(a.into(), da.into()).unwrap();
            let y = RationalTime::new(b.into(), db.into()).unwrap();
            prop_assert_eq!(x.checked_add(y).unwrap().checked_sub(y).unwrap(), x);
        }

        #[test]
        fn frame_round_trip(index in -10_000_000i64..10_000_000, num in 1i64..200_000, den in 1i64..2_000) {
            let rate = Rate::new(num, den).unwrap();
            let t = RationalTime::from_frame(index, rate).unwrap();
            prop_assert_eq!(t.to_frame_floor(rate).unwrap(), index);
        }

        #[test]
        fn compare_matches_f64_when_far_apart(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
            let x = RationalTime::new(a.into(), 7).unwrap();
            let y = RationalTime::new(b.into(), 11).unwrap();
            prop_assert_eq!(x.checked_cmp(y).unwrap(), (a * 11).cmp(&(b * 7)));
        }
    }
}
