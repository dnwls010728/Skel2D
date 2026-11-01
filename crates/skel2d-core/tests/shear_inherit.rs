use skel2d_core::{Affine, Document, Inherit, Pose, apply, from_str, parse_ops, to_string};
use skel2d_core::pose::LocalTransform;

fn build(ops: &str) -> Document { apply(&Document::new("transforms"), &parse_ops(ops).unwrap()).unwrap() }
fn close(a: f32, b: f32) { assert!((a - b).abs() < 0.0002, "{a} != {b}"); }
fn matrix(a: Affine, b: Affine) {
    for (a, b) in [a.a, a.b, a.c, a.d, a.tx, a.ty].into_iter().zip([b.a, b.b, b.c, b.d, b.tx, b.ty]) { close(a, b); }
}

#[test]
fn legacy_files_omit_new_defaults_and_preserve_trs_matrices() {
    let doc = build(r#"[{"op":"add_bone","name":"root","rotation":32,"scale_x":2,"scale_y":-3}]"#);
    let json = to_string(&doc);
    assert!(!json.contains("shear"));
    assert!(!json.contains("inherit"));
    assert_eq!(from_str(&json).unwrap(), doc);
    assert_eq!(Pose::compute(&doc, None).bones[0].world, Affine::from_trs(0.0, 0.0, 32.0, 2.0, -3.0));
}

    // A bone named "root" is the locked root, which cannot be renamed: the tests use "base".
#[test]
fn axis_shear_and_animation_offsets_interpolate_and_survive_rename() {
    let doc = build(r#"[
        {"op":"add_bone","name":"base","shear_x":7,"shear_y":-4},
        {"op":"key_bone","animation":"anim","bone":"base","time":0,"shear":[0,0]},
        {"op":"key_bone","animation":"anim","bone":"base","time":1,"shear":[40,20]},
        {"op":"set_bone","name":"base","rename":"renamed","inherit":"no_scale"}
    ]"#);
    assert_eq!(doc.animations["anim"].duration(), 1.0);
    let pose = Pose::compute(&doc, Some((&doc.animations["anim"], 0.5)));
    close(pose.bones[0].local.shear_x, 27.0);
    close(pose.bones[0].local.shear_y, 6.0);
    matrix(pose.bones[0].world, Affine::from_trss(0.0, 0.0, 0.0, 1.0, 1.0, 27.0, 6.0));
    assert_eq!(from_str(&to_string(&doc)).unwrap(), doc);
    let doc = apply(&doc, &parse_ops(r#"{"op":"remove_key","animation":"anim","bone":"renamed","timeline":"shear","time":1}"#).unwrap()).unwrap();
    assert_eq!(doc.animations["anim"].bones["renamed"].shear.len(), 1);
    let doc = apply(&doc, &parse_ops(r#"{"op":"clear_timeline","animation":"anim","bone":"renamed","timeline":"shear"}"#).unwrap()).unwrap();
    assert!(!doc.animations["anim"].bones.contains_key("renamed"));
}

#[test]
fn inheritance_matches_axis_cases_with_and_without_reflection() {
    // Expected behaviour, as data: parent rotation 90°, scale ±2,3;
    // local child is identity at (5,1).
    for mirror in [false, true] {
        for (mode, positive, negative) in [
            ("normal", [0.0,2.0,-3.0,0.0], [0.0,-2.0,-3.0,0.0]),
            ("only_translation", [1.0,0.0,0.0,1.0], [1.0,0.0,0.0,1.0]),
            ("no_rotation_or_reflection", [3.0,0.0,0.0,2.0], [3.0,0.0,0.0,2.0]),
            ("no_scale", [0.0,1.0,-1.0,0.0], [0.0,-1.0,-1.0,0.0]),
            ("no_scale_or_reflection", [0.0,1.0,-1.0,0.0], [0.0,-1.0,1.0,0.0]),
        ] {
            let sx = if mirror { -2 } else { 2 };
            let doc = build(&format!(r#"[{{"op":"add_bone","name":"p","rotation":90,"scale_x":{sx},"scale_y":3}},{{"op":"add_bone","name":"c","parent":"p","x":5,"y":1,"inherit":"{mode}"}}]"#));
            let w = Pose::compute(&doc, None).bones[1].world;
            let expected = if mirror { negative } else { positive };
            matrix(w, Affine { a:expected[0],b:expected[1],c:expected[2],d:expected[3],tx:-3.0,ty:if mirror {-10.0} else {10.0} });
        }
    }
}

#[test]
fn complex_inheritance_bases_match_reference_with_parent_shear_and_reflection() {
    // Expected behaviour as data, computed separately in f64.
    let parent = Affine::from_trss(0.0, 0.0, 37.0, -2.0, 0.7, 13.0, -18.0);
    for (inherit, expected) in [
        (Inherit::Normal, [0.82865883,0.6127693,-0.07647946,0.92800867]),
        (Inherit::OnlyTranslation, [-0.57340643,-0.40150351,-0.13405628,1.09180077]),
        (Inherit::NoRotationOrReflection, [-0.95251368,-0.96604338,0.59466104,1.45964853]),
        (Inherit::NoScale, [0.60166688,0.35776664,-0.94027164,0.5708671]),
        (Inherit::NoScaleOrReflection, [0.42383408,0.55710383,0.46029569,-0.9990635]),
    ] {
        let local = LocalTransform { x:0.0,y:0.0,rotation:24.0,scale_x:-0.7,scale_y:1.1,shear_x:11.0,shear_y:-17.0,inherit };
        let w = local.world(Some(&parent));
        for (value, expected) in [w.a,w.b,w.c,w.d].into_iter().zip(expected) { close(value, expected); }
    }
}

#[test]
fn reparent_decomposition_preserves_sheared_world_for_all_modes() {
    let parent = Affine::from_trss(10.0, -5.0, 37.0, -2.0, 0.7, 13.0, -18.0);
    let desired = Affine::from_trss(7.0, 22.0, -23.0, 1.3, -0.9, 11.0, 24.0);
    for mode in [Inherit::Normal, Inherit::OnlyTranslation, Inherit::NoRotationOrReflection, Inherit::NoScale, Inherit::NoScaleOrReflection] {
        let local = LocalTransform::from_world(Some(&parent), &desired, mode).unwrap();
        matrix(local.world(Some(&parent)), desired);
    }
}

#[test]
fn singular_parent_branches_stay_finite_and_cannot_reparent_losslessly() {
    for mode in ["normal","only_translation","no_rotation_or_reflection","no_scale","no_scale_or_reflection"] {
        let doc = build(&format!(r#"[{{"op":"add_bone","name":"p","rotation":90,"scale_x":0,"scale_y":2}},{{"op":"add_bone","name":"c","parent":"p","inherit":"{mode}","shear_y":20}}]"#));
        let w = Pose::compute(&doc, None).bones[1].world;
        assert!([w.a,w.b,w.c,w.d,w.tx,w.ty].iter().all(|v| v.is_finite()));
    }
    assert!(LocalTransform::from_world(Some(&Affine::scale(0.0,1.0)), &Affine::IDENTITY, Inherit::Normal).is_none());
}

#[test]
fn shear_keys_change_skinning_and_ik_descendants_respect_inheritance() {
    let doc = build(r#"[
        {"op":"add_bone","name":"base"},
        {"op":"add_bone","name":"child","parent":"base","x":10,"inherit":"only_translation"},
        {"op":"add_bone","name":"target","y":10},
        {"op":"add_ik","name":"aim","bones":["base"],"target":"target"},
        {"op":"key_bone","animation":"anim","bone":"base","time":0,"shear":[0,30]}
    ]"#);
    let pose = Pose::compute(&doc, Some((&doc.animations["anim"], 0.0)));
    close(pose.bones[1].world.rotation_deg(), 0.0);
    close(pose.bones[1].world.tx, 0.0);
    close(pose.bones[1].world.ty, 10.0);
    let doc = build(r#"[{"op":"add_bone","name":"base"},{"op":"key_bone","animation":"anim","bone":"base","time":0,"shear":[0,30]}]"#);
    let pose = Pose::compute(&doc, Some((&doc.animations["anim"], 0.0)));
    let vertex = pose.bones[0].skin.apply(0.0, 10.0);
    close(vertex[0], -5.0); close(vertex[1], 8.660254);
}

#[test]
fn invalid_shear_and_unknown_inheritance_fail_atomically() {
    let doc = build(r#"[{"op":"add_bone","name":"root"}]"#);
    assert!(parse_ops(r#"{"op":"set_bone","name":"root","inherit":"unknown"}"#).is_err());
    for value in [f32::NAN,f32::INFINITY] {
        let op = skel2d_core::Op::SetBone(skel2d_core::ops::BonePatch {name:"root".into(),shear_x:Some(value),..Default::default()});
        assert!(apply(&doc,&[op]).is_err());
        let mut invalid = doc.clone(); invalid.bones[0].shear_y=value;
        assert!(!skel2d_core::validate(&invalid).is_empty());
    }
    assert_eq!(doc.bones[0].shear_x,0.0);
}
