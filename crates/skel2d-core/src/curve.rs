use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Interpolation from a key to the next one.
///
/// Serialized as `"linear"`, `"stepped"`, `"ease_in"`, `"ease_out"`,
/// `"ease_in_out"` or `{"bezier": [x1, y1, x2, y2]}` (CSS-style control points).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    /// Hold the value until the next key.
    Stepped,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bezier([f32; 4]),
}

impl Curve {
    pub fn is_linear(&self) -> bool {
        *self == Curve::Linear
    }

    /// Maps linear progress `t` in 0..=1 to eased progress.
    pub fn apply(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match *self {
            Curve::Linear => t,
            Curve::Stepped => 0.0,
            Curve::EaseIn => bezier(0.42, 0.0, 1.0, 1.0, t),
            Curve::EaseOut => bezier(0.0, 0.0, 0.58, 1.0, t),
            Curve::EaseInOut => bezier(0.42, 0.0, 0.58, 1.0, t),
            Curve::Bezier([x1, y1, x2, y2]) => bezier(x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2, t),
        }
    }
}

/// Cubic bezier from (0,0) to (1,1): solve for the parameter whose x equals
/// `x`, then return its y.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let coord = |p1: f32, p2: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
    };
    let slope = |p1: f32, p2: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * p1 + 6.0 * u * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
    };

    // Newton first, bisection as a fallback for flat slopes.
    let mut s = x;
    for _ in 0..8 {
        let err = coord(x1, x2, s) - x;
        if err.abs() < 1e-6 {
            return coord(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = x;
    for _ in 0..32 {
        let v = coord(x1, x2, s);
        if (v - x).abs() < 1e-6 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = 0.5 * (lo + hi);
    }
    coord(y1, y2, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_symmetry() {
        for c in [Curve::Linear, Curve::EaseIn, Curve::EaseOut, Curve::EaseInOut] {
            assert!(c.apply(0.0).abs() < 1e-4, "{c:?}");
            assert!((c.apply(1.0) - 1.0).abs() < 1e-4, "{c:?}");
        }
        assert!((Curve::EaseInOut.apply(0.5) - 0.5).abs() < 1e-3);
        assert!(Curve::EaseIn.apply(0.25) < 0.25);
        assert!(Curve::EaseOut.apply(0.25) > 0.25);
        assert_eq!(Curve::Stepped.apply(0.9), 0.0);
    }

    #[test]
    fn serde_forms() {
        let c: Curve = serde_json::from_str("\"ease_in_out\"").unwrap();
        assert_eq!(c, Curve::EaseInOut);
        let c: Curve = serde_json::from_str("{\"bezier\":[0.1,0.2,0.3,0.4]}").unwrap();
        assert_eq!(c, Curve::Bezier([0.1, 0.2, 0.3, 0.4]));
    }
}
