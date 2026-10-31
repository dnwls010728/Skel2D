use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 2D affine transform. Maps (x, y) to (a*x + c*y + tx, b*x + d*y + ty).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    /// translate(x, y) * rotate(deg) * scale(sx, sy)
    pub fn from_trs(x: f32, y: f32, rotation_deg: f32, sx: f32, sy: f32) -> Affine {
        let (sin, cos) = rotation_deg.to_radians().sin_cos();
        Affine { a: cos * sx, b: sin * sx, c: -sin * sy, d: cos * sy, tx: x, ty: y }
    }

    /// Axis angles are rotation + shear_x and rotation + 90 + shear_y.
    #[allow(clippy::too_many_arguments)]
    pub fn from_trss(x: f32, y: f32, rotation: f32, sx: f32, sy: f32, shear_x: f32, shear_y: f32) -> Affine {
        // Preserve the exact old path for files without shear.
        if shear_x == 0.0 && shear_y == 0.0 { return Self::from_trs(x, y, rotation, sx, sy); }
        let (sin_x, cos_x) = (rotation + shear_x).to_radians().sin_cos();
        let (sin_y, cos_y) = (rotation + 90.0 + shear_y).to_radians().sin_cos();
        Affine { a: cos_x * sx, b: sin_x * sx, c: cos_y * sy, d: sin_y * sy, tx: x, ty: y }
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, d: sy, ..Self::IDENTITY }
    }

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine { tx: x, ty: y, ..Self::IDENTITY }
    }

    /// `self * rhs`: applies `rhs` first, then `self`.
    pub fn mul(&self, r: &Affine) -> Affine {
        Affine {
            a: self.a * r.a + self.c * r.b,
            b: self.b * r.a + self.d * r.b,
            c: self.a * r.c + self.c * r.d,
            d: self.b * r.c + self.d * r.d,
            tx: self.a * r.tx + self.c * r.ty + self.tx,
            ty: self.b * r.tx + self.d * r.ty + self.ty,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> [f32; 2] {
        [self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty]
    }

    pub fn rotation_deg(&self) -> f32 {
        self.b.atan2(self.a).to_degrees()
    }

    pub fn scale_x(&self) -> f32 {
        self.a.hypot(self.b)
    }

    /// Signed so that a mirrored transform reports a negative y scale.
    pub fn scale_y(&self) -> f32 {
        let sx = self.scale_x();
        if sx == 0.0 { 0.0 } else { (self.a * self.d - self.b * self.c) / sx }
    }

    pub fn determinant(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// `None` for singular transforms (e.g. a zero scale).
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) })
    }

    /// Applies only the linear part (no translation), for directions and deltas.
    pub fn apply_vector(&self, x: f32, y: f32) -> [f32; 2] {
        [self.a * x + self.c * y, self.b * x + self.d * y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn trs_and_compose() {
        let parent = Affine::from_trs(10.0, 0.0, 90.0, 1.0, 1.0);
        let child = Affine::from_trs(5.0, 0.0, 0.0, 1.0, 1.0);
        let world = parent.mul(&child);
        assert!(close(world.apply(0.0, 0.0), [10.0, 5.0]));
        assert!((world.rotation_deg() - 90.0).abs() < 1e-4);
        let s = Affine::from_trs(0.0, 0.0, 30.0, 2.0, -3.0);
        assert!((s.scale_x() - 2.0).abs() < 1e-4);
        assert!((s.scale_y() + 3.0).abs() < 1e-4);
    }
}

#[cfg(test)]
mod inverse_tests {
    use super::*;

    #[test]
    fn inverse_round_trips() {
        let m = Affine::from_trs(3.0, -7.0, 37.0, 2.0, -0.5);
        let p = m.inverse().unwrap().apply(m.apply(4.0, 5.0)[0], m.apply(4.0, 5.0)[1]);
        assert!((p[0] - 4.0).abs() < 1e-3 && (p[1] - 5.0).abs() < 1e-3);
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
    }
}
