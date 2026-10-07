// SPDX-License-Identifier: Apache-2.0

use std::fmt;

use om_types::{Finite, NonFiniteError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A finite 2-D point. Serialised as `[x, y]`.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Point2 {
    x: Finite,
    y: Finite,
}

impl Point2 {
    pub fn new(x: f64, y: f64) -> Result<Self, NonFiniteError> {
        Ok(Self {
            x: Finite::new(x)?,
            y: Finite::new(y)?,
        })
    }

    /// For compile-time-known finite values.
    #[must_use]
    pub const fn from_finite(x: Finite, y: Finite) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn x(self) -> f64 {
        self.x.get()
    }

    #[must_use]
    pub const fn y(self) -> f64 {
        self.y.get()
    }

    #[must_use]
    pub fn to_tuple(self) -> (f64, f64) {
        (self.x(), self.y())
    }

    /// Unit-square corners in quad order (TL, TR, BR, BL).
    #[must_use]
    pub fn unit_square() -> [Self; 4] {
        [
            Self::from_finite(Finite::ZERO, Finite::ZERO),
            Self::from_finite(Finite::ONE, Finite::ZERO),
            Self::from_finite(Finite::ONE, Finite::ONE),
            Self::from_finite(Finite::ZERO, Finite::ONE),
        ]
    }

    /// Unit right triangle corners: (0,0), (1,0), (0,1).
    #[must_use]
    pub fn unit_triangle() -> [Self; 3] {
        [
            Self::from_finite(Finite::ZERO, Finite::ZERO),
            Self::from_finite(Finite::ONE, Finite::ZERO),
            Self::from_finite(Finite::ZERO, Finite::ONE),
        ]
    }
}

impl fmt::Debug for Point2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x(), self.y())
    }
}

impl Serialize for Point2 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.x, self.y].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Point2 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let [x, y] = <[Finite; 2]>::deserialize(deserializer)?;
        Ok(Self { x, y })
    }
}
