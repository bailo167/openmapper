// SPDX-License-Identifier: Apache-2.0
//! Floating-point wrappers that can never hold NaN or infinity.
//!
//! Persisted documents forbid non-finite numbers, so these types reject them
//! at construction and deserialisation time.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Error for a number that is NaN, infinite or out of range.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum NonFiniteError {
    #[error("value {0} is not finite")]
    NotFinite(f64),
    #[error("value {0} is outside the range 0.0..=1.0")]
    OutOfUnitRange(f64),
}

/// A finite `f64`.
#[derive(Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Finite(f64);

impl Finite {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    /// Wraps `value`, rejecting NaN and infinities.
    pub fn new(value: f64) -> Result<Self, NonFiniteError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(NonFiniteError::NotFinite(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl fmt::Debug for Finite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl Serialize for Finite {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for Finite {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(f64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A finite `f64` in `0.0..=1.0` (opacity, normalised levels, …).
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    /// Wraps `value`, rejecting non-finite and out-of-range values.
    pub fn new(value: f64) -> Result<Self, NonFiniteError> {
        if !value.is_finite() {
            Err(NonFiniteError::NotFinite(value))
        } else if !(0.0..=1.0).contains(&value) {
            Err(NonFiniteError::OutOfUnitRange(value))
        } else {
            Ok(Self(value))
        }
    }

    /// Clamps any finite value into range; non-finite values become 0.
    #[must_use]
    pub fn saturating(value: f64) -> Self {
        if value.is_finite() {
            Self(value.clamp(0.0, 1.0))
        } else {
            Self::ZERO
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Default for UnitInterval {
    fn default() -> Self {
        Self::ONE
    }
}

impl fmt::Debug for UnitInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl Serialize for UnitInterval {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for UnitInterval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(f64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_rejects_nan_and_infinity() {
        assert!(Finite::new(f64::NAN).is_err());
        assert!(Finite::new(f64::INFINITY).is_err());
        assert_eq!(Finite::new(2.5).unwrap().get(), 2.5);
    }

    #[test]
    fn unit_interval_enforces_range() {
        assert!(UnitInterval::new(-0.01).is_err());
        assert!(UnitInterval::new(1.01).is_err());
        assert_eq!(UnitInterval::saturating(7.0).get(), 1.0);
        assert_eq!(UnitInterval::saturating(f64::NAN).get(), 0.0);
        assert!(serde_json::from_str::<UnitInterval>("1.5").is_err());
        assert_eq!(
            serde_json::from_str::<UnitInterval>("0.25").unwrap().get(),
            0.25
        );
    }
}
