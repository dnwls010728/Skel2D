//! Transform constraints: a bone takes part of another bone's transform.

use crate::math::Affine;
use crate::model::TransformConstraint;
use crate::pose::LocalTransform;

#[derive(Clone, Copy)]
struct Parts {
    rotation: f32,
    x: f32,
    y: f32,
    scale_x: f32,
    scale_y: f32,
    shear_y: f32,
}

fn world_parts(m: &Affine) -> Parts {
    let rotation = m.rotation_deg();
    let scale_y = m.c.hypot(m.d) * if m.determinant() < 0.0 { -1.0 } else { 1.0 };
    let shear_y = if scale_y.abs() > 1e-6 { wrap((m.d / scale_y).atan2(m.c / scale_y).to_degrees() - rotation - 90.0) } else { 0.0 };
    Parts { rotation, x: m.tx, y: m.ty, scale_x: m.scale_x(), scale_y, shear_y }
}

fn local_parts(l: &LocalTransform) -> Parts {
    Parts { rotation: l.rotation, x: l.x, y: l.y, scale_x: l.scale_x, scale_y: l.scale_y, shear_y: l.shear_y }
}

/// The shortest way around, in -180..=180 degrees.
fn wrap(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn blend(c: &TransformConstraint, bone: Parts, goal: Parts) -> Parts {
    if c.relative {
        let grow = |value: f32, goal: f32, mix: f32| value * (1.0 + (goal - 1.0) * mix);
        return Parts {
            rotation: bone.rotation + goal.rotation * c.mix_rotate,
            x: bone.x + goal.x * c.mix_x,
            y: bone.y + goal.y * c.mix_y,
            scale_x: grow(bone.scale_x, goal.scale_x, c.mix_scale_x),
            scale_y: grow(bone.scale_y, goal.scale_y, c.mix_scale_y),
            shear_y: bone.shear_y + goal.shear_y * c.mix_shear_y,
        };
    }
    let toward = |value: f32, goal: f32, mix: f32| value + (goal - value) * mix;
    Parts {
        rotation: bone.rotation + wrap(goal.rotation - bone.rotation) * c.mix_rotate,
        x: toward(bone.x, goal.x, c.mix_x),
        y: toward(bone.y, goal.y, c.mix_y),
        scale_x: toward(bone.scale_x, goal.scale_x, c.mix_scale_x),
        scale_y: toward(bone.scale_y, goal.scale_y, c.mix_scale_y),
        shear_y: bone.shear_y + wrap(goal.shear_y - bone.shear_y) * c.mix_shear_y,
    }
}

/// Applies `c` to one bone's local transform. Returns whether anything changed.
pub fn solve(c: &TransformConstraint, local: &mut LocalTransform, world: &Affine, parent: Option<&Affine>, source_local: &LocalTransform, source_world: &Affine) -> bool {
    let moves_axes = [c.mix_rotate, c.mix_scale_x, c.mix_scale_y, c.mix_shear_y].iter().any(|m| *m != 0.0);
    if !moves_axes && c.mix_x == 0.0 && c.mix_y == 0.0 { return false; }
    let source = if c.local { local_parts(source_local) } else { world_parts(source_world) };
    let [x, y] = match (c.local, c.relative) {
        (true, _) => [source.x + c.offset_x, source.y + c.offset_y],
        (false, false) => source_world.apply(c.offset_x, c.offset_y),
        (false, true) => source_world.apply_vector(c.offset_x, c.offset_y),
    };
    let goal = Parts {
        rotation: source.rotation + c.offset_rotation,
        x,
        y,
        scale_x: source.scale_x + c.offset_scale_x,
        scale_y: source.scale_y + c.offset_scale_y,
        shear_y: source.shear_y + c.offset_shear_y,
    };
    if c.local {
        let p = blend(c, local_parts(local), goal);
        (local.rotation, local.x, local.y, local.scale_x, local.scale_y, local.shear_y) = (p.rotation, p.x, p.y, p.scale_x, p.scale_y, p.shear_y);
        return true;
    }
    let p = blend(c, world_parts(world), goal);
    if !moves_axes {
        // Position alone: the axes stay exactly as they are.
        let [lx, ly] = match parent {
            Some(parent) => match parent.inverse() {
                Some(inverse) => inverse.apply(p.x, p.y),
                None => return false,
            },
            None => [p.x, p.y],
        };
        (local.x, local.y) = (lx, ly);
        return true;
    }
    let next = Affine::from_trss(p.x, p.y, p.rotation, p.scale_x, p.scale_y, 0.0, p.shear_y);
    match LocalTransform::from_world(parent, &next, local.inherit) {
        Some(solved) => { *local = solved; true }
        None => false,
    }
}
