//! Analytic one- and two-bone IK. Solving adjusts local rotations, plus the x
//! scale when compressing or stretching, so the result is an ordinary pose that
//! can be inspected and keyed.

use crate::math::Affine;
use crate::model::{Document, IkConstraint, IkPose};
use crate::pose::LocalTransform;

/// How far a chain of length `reach` extends towards a target at `distance`.
/// With softness the extension eases over `reach - softness ..= reach + softness`
/// along a parabola that leaves the straight line and ends at `reach` with no
/// corner at either end. Without it the chain just stops at `reach`.
fn soft_extension(distance: f32, reach: f32, softness: f32) -> f32 {
    if softness <= 0.0 { return distance.min(reach); }
    let over = distance - (reach - softness);
    if over <= 0.0 {
        distance
    } else if over >= 2.0 * softness {
        reach
    } else {
        distance - over * over / (4.0 * softness)
    }
}

/// Applies `ik` with the animated `values` to `locals`. `worlds` must match
/// `locals`. Returns whether anything changed (the caller then recomputes world
/// transforms).
pub(crate) fn solve(
    doc: &Document,
    ik: &IkConstraint,
    values: &IkPose,
    parents: &[Option<usize>],
    locals: &mut [LocalTransform],
    worlds: &[Affine],
) -> bool {
    let Some(t) = doc.bone_index(&ik.target) else { return false };
    let target = worlds[t].apply(0.0, 0.0);
    let parent_world = |i: usize| parents[i].map(|p| worlds[p]).unwrap_or(Affine::IDENTITY);
    let mix = values.mix;

    match ik.bones.as_slice() {
        [bone] => {
            let Some(i) = doc.bone_index(bone) else { return false };
            let w = &worlds[i];
            let effector = w.apply(1.0, 0.0);
            let origin = w.apply(0.0, 0.0);
            let delta = aim_delta(origin, effector, target);
            locals[i].rotation += delta * mix * rotation_sign(&locals[i], &parent_world(i));
            let length = dist(origin, w.apply(doc.bones[i].length, 0.0));
            let distance = dist(origin, target);
            if length > 1e-4 && ((values.compress && distance < length) || (values.stretch && distance > length)) {
                locals[i].scale_x *= 1.0 + (distance / length - 1.0) * mix;
            }
            true
        }
        [parent, child] => {
            let (Some(p), Some(c)) = (doc.bone_index(parent), doc.bone_index(child)) else { return false };
            if parents[c] != Some(p) {
                return false;
            }
            let length = doc.bones[c].length;
            let origin = worlds[p].apply(0.0, 0.0);
            let elbow = worlds[c].apply(0.0, 0.0);
            let tip = worlds[c].apply(length, 0.0);
            let (mut l1, mut l2) = (dist(origin, elbow), dist(elbow, tip));
            let grand = parent_world(p);

            if l1 < 1e-4 || l2 < 1e-4 {
                // Degenerate chain: just swing the parent so the tip points at the target.
                let delta = aim_delta(origin, tip, target);
                locals[p].rotation += delta * mix * rotation_sign(&locals[p], &grand);
                return true;
            }

            let target_distance = dist(origin, target);
            let extension = soft_extension(target_distance, l1 + l2, values.softness);
            let mut stretch = 1.0;
            let mut goal = target;
            if values.stretch {
                // The soft triangle scaled up until its tip is on the target.
                if extension > 1e-4 && target_distance > extension {
                    stretch = target_distance / extension;
                    l1 *= stretch;
                    l2 *= stretch;
                }
            } else if values.softness > 0.0 && target_distance > extension {
                let k = extension / target_distance;
                goal = [origin[0] + (target[0] - origin[0]) * k, origin[1] + (target[1] - origin[1]) * k];
            }

            // Law of cosines for the angle between the parent bone and the target line.
            let d = dist(origin, goal).clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4).max(1e-4);
            let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
            let bend = if values.bend_positive { 1.0 } else { -1.0 };
            let base = angle(origin, goal);
            let wanted = base - bend * cos_a.acos().to_degrees();
            let d1 = wrap(wanted - angle(origin, elbow));

            // Child delta measured with the parent fully solved, then both are
            // scaled by `mix` so mix=0..1 blends smoothly from FK to IK.
            let mut solved_parent = locals[p];
            solved_parent.rotation += d1 * rotation_sign(&locals[p], &grand);
            solved_parent.scale_x *= stretch;
            let parent_w = solved_parent.world(Some(&grand));
            let sign = rotation_sign(&locals[c], &parent_w);
            let mut aimed = locals[c];
            // A stretched parent skews the child, so its local angle is not its
            // world angle: repeat the aim until the two agree.
            for _ in 0..if stretch == 1.0 { 1 } else { 4 } {
                let w = aimed.world(Some(&parent_w));
                let e = w.apply(0.0, 0.0);
                aimed.rotation += wrap(angle(e, goal) - angle(e, w.apply(length, 0.0))) * sign;
            }
            let child_rotation = aimed.rotation - locals[c].rotation;
            let child_stretch = if stretch == 1.0 { 1.0 } else {
                let w = aimed.world(Some(&parent_w));
                let current = dist(w.apply(0.0, 0.0), w.apply(length, 0.0));
                if current > 1e-4 { l2 / current } else { 1.0 }
            };

            locals[p].rotation += d1 * mix * rotation_sign(&locals[p], &grand);
            locals[c].rotation += child_rotation * mix;
            locals[p].scale_x *= 1.0 + (stretch - 1.0) * mix;
            locals[c].scale_x *= 1.0 + (child_stretch - 1.0) * mix;
            true
        }
        _ => false,
    }
}

/// Rotation (degrees) about `pivot` that turns `effector` towards `target`.
fn aim_delta(pivot: [f32; 2], effector: [f32; 2], target: [f32; 2]) -> f32 {
    if dist(pivot, target) < 1e-5 || dist(pivot, effector) < 1e-5 {
        return 0.0;
    }
    wrap(angle(pivot, target) - angle(pivot, effector))
}

/// A mirrored parent reverses the on-screen sense of local rotation.
fn rotation_sign(local: &LocalTransform, parent: &Affine) -> f32 {
    let inherits_reflection = matches!(local.inherit, crate::model::Inherit::Normal | crate::model::Inherit::NoScale);
    if inherits_reflection && parent.determinant() < 0.0 { -1.0 } else { 1.0 }
}

fn angle(from: [f32; 2], to: [f32; 2]) -> f32 {
    (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees()
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn wrap(deg: f32) -> f32 {
    (deg + 540.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use crate::{Document, Pose, apply, parse_ops};

    fn leg(bend_positive: bool, target: [f32; 2]) -> Document {
        let ops = format!(
            r#"[{{"op":"add_bone","name":"hip","rotation":-90,"length":50}},
               {{"op":"add_bone","name":"knee","parent":"hip","x":50,"length":50}},
               {{"op":"add_bone","name":"goal","x":{},"y":{}}},
               {{"op":"add_ik","name":"leg","bones":["hip","knee"],"target":"goal","bend_positive":{bend_positive}}}]"#,
            target[0], target[1]
        );
        apply(&Document::new("ik"), &parse_ops(&ops).unwrap()).unwrap()
    }

    fn tip(doc: &Document, pose: &Pose, bone: &str) -> [f32; 2] {
        let i = doc.bone_index(bone).unwrap();
        pose.bones[i].world.apply(doc.bones[i].length, 0.0)
    }

    #[test]
    fn two_bone_reaches_target_and_bends_both_ways() {
        for bend in [true, false] {
            let doc = leg(bend, [30.0, -60.0]);
            let pose = Pose::compute(&doc, None);
            let [x, y] = tip(&doc, &pose, "knee");
            assert!((x - 30.0).abs() < 0.05 && (y + 60.0).abs() < 0.05, "bend {bend}: tip {x},{y}");
            let elbow = pose.bones[doc.bone_index("knee").unwrap()].world.apply(0.0, 0.0);
            // Elbow sits on opposite sides of the hip->goal line for the two bend directions.
            let side = (30.0 * elbow[1] - (-60.0) * elbow[0]).signum();
            assert_eq!(side > 0.0, !bend, "bend {bend}: elbow {elbow:?}");
        }
    }

    #[test]
    fn unreachable_target_straightens_chain() {
        let doc = leg(true, [0.0, -500.0]);
        let pose = Pose::compute(&doc, None);
        let [x, y] = tip(&doc, &pose, "knee");
        assert!(x.abs() < 0.5 && (y + 100.0).abs() < 0.5, "tip {x},{y}");
    }

    #[test]
    fn one_bone_aims_and_mix_blends() {
        let ops = r#"[{"op":"add_bone","name":"arm","length":10},
                      {"op":"add_bone","name":"goal","y":20},
                      {"op":"add_ik","name":"aim","bones":["arm"],"target":"goal","mix":0.5}]"#;
        let doc = apply(&Document::new("ik"), &parse_ops(ops).unwrap()).unwrap();
        let pose = Pose::compute(&doc, None);
        assert!((pose.bones[0].world.rotation_deg() - 45.0).abs() < 1e-3);
    }

    fn chain(extra: &str, goal_x: f32) -> Document {
        let ops = format!(
            r#"[{{"op":"add_bone","name":"a","length":50}},
               {{"op":"add_bone","name":"b","parent":"a","x":50,"length":50}},
               {{"op":"add_bone","name":"goal","x":{goal_x},"y":0.0}},
               {{"op":"add_ik","name":"c","bones":["a","b"],"target":"goal"{extra}}}]"#
        );
        apply(&Document::new("ik"), &parse_ops(&ops).unwrap()).unwrap()
    }

    #[test]
    fn softness_eases_the_last_part_of_the_reach() {
        let reach = |goal: f32, softness: f32| {
            let doc = chain(&format!(r#","softness":{softness}"#), goal);
            let pose = Pose::compute(&doc, None);
            tip(&doc, &pose, "b")[0]
        };
        // Outside the soft zone it behaves as without softness.
        assert!((reach(70.0, 10.0) - 70.0).abs() < 0.01);
        assert!((reach(500.0, 10.0) - 100.0).abs() < 0.5);
        // At the reach itself: 100 - 10^2 / 40 = 97.5.
        assert!((reach(100.0, 10.0) - 97.5).abs() < 0.01);
        // Halfway through the zone: 95 - 5^2 / 40 = 94.375.
        assert!((reach(95.0, 10.0) - 94.375).abs() < 0.01);
    }

    #[test]
    fn stretch_reaches_far_targets_and_blends_with_mix() {
        let doc = chain(r#","stretch":true"#, 150.0);
        let pose = Pose::compute(&doc, None);
        assert!((tip(&doc, &pose, "b")[0] - 150.0).abs() < 0.05, "{:?}", tip(&doc, &pose, "b"));
        assert!((pose.bones[0].local.scale_x - 1.5).abs() < 0.01);
        // A reachable target leaves the scale alone.
        let doc = chain(r#","stretch":true"#, 80.0);
        assert_eq!(Pose::compute(&doc, None).bones[0].local.scale_x, 1.0);
        // Half the mix stretches half as far.
        let doc = chain(r#","stretch":true,"mix":0.5"#, 150.0);
        assert!((Pose::compute(&doc, None).bones[0].local.scale_x - 1.25).abs() < 0.01);
    }

    #[test]
    fn one_bone_compresses_and_stretches_along_x() {
        let scale = |flags: &str, goal: f32| {
            let ops = format!(r#"[{{"op":"add_bone","name":"a","length":50}},{{"op":"add_bone","name":"goal","x":{goal}}},{{"op":"add_ik","name":"c","bones":["a"],"target":"goal"{flags}}}]"#);
            let doc = apply(&Document::new("ik"), &parse_ops(&ops).unwrap()).unwrap();
            Pose::compute(&doc, None).bones[0].local.scale_x
        };
        assert_eq!(scale("", 25.0), 1.0);
        assert_eq!(scale("", 100.0), 1.0);
        assert!((scale(r#","compress":true"#, 25.0) - 0.5).abs() < 1e-4);
        assert_eq!(scale(r#","compress":true"#, 100.0), 1.0);
        assert!((scale(r#","stretch":true"#, 100.0) - 2.0).abs() < 1e-4);
        assert_eq!(scale(r#","stretch":true"#, 25.0), 1.0);
    }
}
