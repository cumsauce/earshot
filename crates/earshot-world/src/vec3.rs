use std::ops::{Add, Mul, Neg, Sub};

/// A point or direction in block coordinates (one block is one metre).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    pub fn dot(self, other: Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// This direction at length 1, or zero if it has no length.
    pub fn normalized(self) -> Vec3 {
        let length = self.length();
        if length == 0.0 {
            Vec3::ZERO
        } else {
            self * (1.0 / length)
        }
    }

    pub fn axis(self, axis: usize) -> f64 {
        match axis {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }

    /// Whether the point is somewhere the sim can work with: finite and well
    /// inside `i32` on every axis. `floor() as i32` saturates past the type's
    /// range, and the ray walks then add steps to the saturated block, which
    /// overflows. Minecraft's own world border is near ±30 million blocks.
    pub fn is_usable(self) -> bool {
        const LIMIT: f64 = 1.0e9;
        [self.x, self.y, self.z]
            .iter()
            .all(|v| v.is_finite() && v.abs() <= LIMIT)
    }
}

impl Add for Vec3 {
    type Output = Vec3;

    fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;

    fn sub(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Vec3;

    fn mul(self, scale: f64) -> Vec3 {
        Vec3::new(self.x * scale, self.y * scale, self.z * scale)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;

    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that fixed the huge-coordinate crash, spelt out: finite on
    /// every axis and `abs() <= 1e9` (the bound is inclusive — exactly ±1e9
    /// passes, one step further does not).
    #[test]
    fn is_usable_accepts_finite_well_bounded_points_and_nothing_else() {
        let cases = [
            // NaN on any single axis spoils the point.
            (Vec3::new(f64::NAN, 0.0, 0.0), false),
            (Vec3::new(0.0, f64::NAN, 0.0), false),
            (Vec3::new(0.0, 0.0, f64::NAN), false),
            // ±infinity on any single axis, in either direction.
            (Vec3::new(f64::INFINITY, 1.0, -1.0), false),
            (Vec3::new(f64::NEG_INFINITY, 1.0, -1.0), false),
            (Vec3::new(1.0, f64::INFINITY, -1.0), false),
            (Vec3::new(1.0, f64::NEG_INFINITY, -1.0), false),
            (Vec3::new(1.0, -1.0, f64::INFINITY), false),
            (Vec3::new(1.0, -1.0, f64::NEG_INFINITY), false),
            // Signed zero is finite and inside: usable.
            (Vec3::new(0.0, 0.0, 0.0), true),
            (Vec3::new(-0.0, -0.0, -0.0), true),
            // Finite but far outside the bound: unusable.
            (Vec3::new(1.0e300, 0.0, 0.0), false),
            (Vec3::new(-1.0e300, 0.0, 0.0), false),
            (Vec3::new(0.0, 1.0e300, 0.0), false),
            (Vec3::new(0.0, -1.0e300, 0.0), false),
            (Vec3::new(0.0, 0.0, 1.0e300), false),
            (Vec3::new(0.0, 0.0, -1.0e300), false),
            // The documented boundary, inclusive on every axis.
            (Vec3::new(1.0e9, 0.0, 0.0), true),
            (Vec3::new(-1.0e9, 0.0, 0.0), true),
            (Vec3::new(0.0, 1.0e9, 0.0), true),
            (Vec3::new(0.0, 0.0, -1.0e9), true),
            // One ulp-ish step past it, on any axis: out.
            (Vec3::new(1.0e9 + 1.0, 0.0, 0.0), false),
            (Vec3::new(-1.0e9 - 1.0, 0.0, 0.0), false),
            (Vec3::new(0.0, 1.0e9 + 1.0, 0.0), false),
            (Vec3::new(0.0, 0.0, -1.0e9 - 1.0), false),
            // Ordinary finite values, including near Minecraft's border.
            (Vec3::new(0.5, 0.5, 0.5), true),
            (Vec3::new(-7.2, 3.9, 11.1), true),
            (Vec3::new(30_000_000.0, -30_000_000.0, 0.0), true),
        ];
        for (point, expected) in cases {
            assert_eq!(point.is_usable(), expected, "{point:?}");
        }
    }

    #[test]
    fn new_round_trips_the_components_it_was_given() {
        let point = Vec3::new(1.5, -2.25, 3.0);
        assert_eq!((point.x, point.y, point.z), (1.5, -2.25, 3.0));
        assert_eq!(point, Vec3::new(point.x, point.y, point.z));
        assert_eq!(Vec3::ZERO, Vec3::new(0.0, 0.0, 0.0));
        assert!(Vec3::ZERO.is_usable());
    }
}
