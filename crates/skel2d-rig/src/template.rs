//! Rig specs: what an agent (or a person) provides to describe a skeleton in
//! image pixel coordinates. Templates turn a handful of joint positions into
//! named bones, hierarchy and IK; the math is done here, not by the agent.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use skel2d_core::Op;
use skel2d_core::model::IkConstraint;

use crate::mesh::Placement;
use crate::skeleton::RootSide;

/// Pixel coordinates in the source image: x right, y down, origin top-left.
pub type Px = [f32; 2];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "template", rename_all = "snake_case", deny_unknown_fields)]
pub enum RigSpec {
    /// Biped from joint positions. Required: pelvis, neck, head_top. Limbs
    /// (suffix _l / _r, character's left/right or screen left/right, just be
    /// consistent): shoulder, elbow, wrist, optional hand (fingertips);
    /// hip, knee, ankle, optional toe. Optional chest (default: midway
    /// between pelvis and neck).
    Humanoid {
        joints: BTreeMap<String, Px>,
        /// Two-bone IK on each leg with a foot target bone.
        #[serde(default = "yes")]
        leg_ik: bool,
        #[serde(default)]
        arm_ik: bool,
    },
    /// A single chain (tail, tentacle, rope). Joints from root to tip, or
    /// omitted to trace the image's medial axis automatically.
    Chain {
        #[serde(default)]
        joints: Option<Vec<Px>>,
        /// Number of bones when tracing automatically.
        #[serde(default = "four")]
        segments: usize,
        /// Which end of an automatic trace is the root.
        #[serde(default)]
        root: Side,
        #[serde(default = "chain_prefix")]
        prefix: String,
    },
    /// Explicit bones, each from `start` to optional `end`, parents first.
    Custom { bones: Vec<BoneSpec> },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

impl From<Side> for RootSide {
    fn from(s: Side) -> Self {
        match s {
            Side::Top => RootSide::Top,
            Side::Bottom => RootSide::Bottom,
            Side::Left => RootSide::Left,
            Side::Right => RootSide::Right,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoneSpec {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub start: Px,
    #[serde(default)]
    pub end: Option<Px>,
}

fn yes() -> bool {
    true
}

fn four() -> usize {
    4
}

fn chain_prefix() -> String {
    "chain".into()
}

/// Bones and IK ops produced from a spec, plus the names of bones that
/// should not receive mesh weights (root and IK targets).
pub struct Built {
    pub ops: Vec<Op>,
    pub non_deforming: Vec<String>,
    pub warnings: Vec<String>,
}

pub const ROOT: &str = "root";

pub fn humanoid(joints: &BTreeMap<String, Px>, leg_ik: bool, arm_ik: bool, place: Placement) -> Result<Built, String> {
    let missing: Vec<&str> = ["pelvis", "neck", "head_top"].into_iter().filter(|j| !joints.contains_key(*j)).collect();
    if !missing.is_empty() {
        return Err(format!("humanoid needs joints: {}", missing.join(", ")));
    }
    let known = ["pelvis", "chest", "neck", "head_top"];
    let limb = ["shoulder", "elbow", "wrist", "hand", "hip", "knee", "ankle", "toe"];
    let mut warnings: Vec<String> = joints
        .keys()
        .filter(|k| {
            !known.contains(&k.as_str())
                && !limb.iter().any(|l| k.as_str() == format!("{l}_l") || k.as_str() == format!("{l}_r"))
        })
        .map(|k| format!("unknown joint {k:?} ignored"))
        .collect();

    let w = |name: &str| joints.get(name).map(|&p| place.to_world(p));
    let (pelvis, neck) = (w("pelvis").expect("checked"), w("neck").expect("checked"));
    let chest = w("chest").unwrap_or([(pelvis[0] + neck[0]) / 2.0, (pelvis[1] + neck[1]) / 2.0]);

    let mut ops = vec![world(ROOT, None, [pelvis[0], 0.0], None)];
    let mut non_deforming = vec![ROOT.to_string()];
    ops.push(world("hip", Some(ROOT), pelvis, Some(chest)));
    ops.push(world("chest", Some("hip"), chest, Some(neck)));
    ops.push(world("head", Some("chest"), neck, w("head_top")));

    for side in ["l", "r"] {
        let j = |n: &str| w(&format!("{n}_{side}"));
        let name = |n: &str| format!("{n}_{side}");
        if let (Some(shoulder), Some(elbow), Some(wrist)) = (j("shoulder"), j("elbow"), j("wrist")) {
            ops.push(world(&name("upper_arm"), Some("chest"), shoulder, Some(elbow)));
            ops.push(world(&name("forearm"), Some(&name("upper_arm")), elbow, Some(wrist)));
            if let Some(hand) = j("hand") {
                ops.push(world(&name("hand"), Some(&name("forearm")), wrist, Some(hand)));
            }
            if arm_ik {
                let target = name("arm_target");
                ops.push(world(&target, Some(ROOT), wrist, None));
                ops.push(ik(&name("arm_ik"), [name("upper_arm"), name("forearm")], &target, [shoulder, elbow, wrist], pelvis));
                non_deforming.push(target);
            }
        } else if ["shoulder", "elbow", "wrist"].iter().any(|n| j(n).is_some()) {
            warnings.push(format!("arm {side} skipped: needs shoulder_{side}, elbow_{side} and wrist_{side}"));
        }
        if let (Some(hip), Some(knee), Some(ankle)) = (j("hip"), j("knee"), j("ankle")) {
            ops.push(world(&name("thigh"), Some("hip"), hip, Some(knee)));
            ops.push(world(&name("shin"), Some(&name("thigh")), knee, Some(ankle)));
            if let Some(toe) = j("toe") {
                ops.push(world(&name("foot"), Some(&name("shin")), ankle, Some(toe)));
            }
            if leg_ik {
                let target = name("leg_target");
                ops.push(world(&target, Some(ROOT), ankle, None));
                ops.push(ik(&name("leg_ik"), [name("thigh"), name("shin")], &target, [hip, knee, ankle], pelvis));
                non_deforming.push(target);
            }
        } else if ["hip", "knee", "ankle"].iter().any(|n| j(n).is_some()) {
            warnings.push(format!("leg {side} skipped: needs hip_{side}, knee_{side} and ankle_{side}"));
        }
    }
    Ok(Built { ops, non_deforming, warnings })
}

pub fn chain(joints: &[Px], prefix: &str, place: Placement) -> Result<Built, String> {
    if joints.len() < 2 {
        return Err("chain needs at least 2 joints".into());
    }
    let pts: Vec<[f32; 2]> = joints.iter().map(|&p| place.to_world(p)).collect();
    let mut ops = vec![world(ROOT, None, pts[0], None)];
    let mut parent = ROOT.to_string();
    for (i, seg) in pts.windows(2).enumerate() {
        let name = format!("{prefix}{}", i + 1);
        ops.push(world(&name, Some(&parent), seg[0], Some(seg[1])));
        parent = name;
    }
    Ok(Built { ops, non_deforming: vec![ROOT.to_string()], warnings: Vec::new() })
}

pub fn custom(bones: &[BoneSpec], place: Placement) -> Result<Built, String> {
    if bones.is_empty() {
        return Err("custom rig needs at least one bone".into());
    }
    let ops = bones
        .iter()
        .map(|b| world(&b.name, b.parent.as_deref(), place.to_world(b.start), b.end.map(|e| place.to_world(e))))
        .collect();
    Ok(Built { ops, non_deforming: Vec::new(), warnings: Vec::new() })
}

fn world(name: &str, parent: Option<&str>, start: [f32; 2], end: Option<[f32; 2]>) -> Op {
    Op::AddBoneWorld { name: name.to_string(), parent: parent.map(str::to_string), start, end }
}

/// Two-bone IK whose bend direction reproduces the drawn pose.
fn ik(name: &str, bones: [String; 2], target: &str, [root, mid, end]: [[f32; 2]; 3], center: [f32; 2]) -> Op {
    Op::AddIk(IkConstraint {
        order: 0,
        name: name.to_string(),
        bones: bones.to_vec(),
        target: target.to_string(),
        mix: 1.0,
        bend_positive: bend_for(root, mid, end, center),
        softness: 0.0,
        compress: false,
        stretch: false,
    })
}

/// The bend direction (world space, y up) that keeps a two-bone chain bent
/// the way it is: positive puts the middle joint clockwise of the
/// root->end line. A chain too straight to tell bends away from `center`
/// (the body's middle), so limbs fold outward instead of across the body.
pub fn bend_for(root: [f32; 2], mid: [f32; 2], end: [f32; 2], center: [f32; 2]) -> bool {
    let g = [end[0] - root[0], end[1] - root[1]];
    let m = [mid[0] - root[0], mid[1] - root[1]];
    let cross = g[0] * m[1] - g[1] * m[0];
    let scale = g[0].hypot(g[1]) * m[0].hypot(m[1]);
    if cross.abs() > 0.03 * scale {
        return cross < 0.0;
    }
    // Clockwise normal of the chain line, compared with the way out of the body.
    let normal = [g[1], -g[0]];
    let out = [root[0] - center[0], root[1] - center[1]];
    normal[0] * out[0] + normal[1] * out[1] >= 0.0
}

#[cfg(test)]
mod tests {
    use super::bend_for;

    #[test]
    fn straight_limbs_fold_away_from_the_body() {
        let center = [0.0, 100.0];
        // Straight legs hanging down on either side: knees go outward.
        let left = bend_for([-10.0, 100.0], [-10.0, 50.0], [-10.0, 0.0], center);
        let right = bend_for([10.0, 100.0], [10.0, 50.0], [10.0, 0.0], center);
        assert_ne!(left, right);
        // Clockwise of a downward line is -x, i.e. outward for the left leg.
        assert!(left);
        // A drawn bend wins over the fallback.
        assert!(!bend_for([-10.0, 100.0], [0.0, 50.0], [-10.0, 0.0], center));
    }
}
