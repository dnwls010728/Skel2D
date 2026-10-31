//! Transform constraints, per-channel curves and per-bone binds.

use serde_json::{Value, json};
use skel2d_core::pose::sample;
use skel2d_core::{Curve, Document, Key, Pose, apply, parse_ops};

fn edit(doc: &Document, ops: Value) -> skel2d_core::Result<Document> {
    apply(doc, &parse_ops(&ops.to_string())?)
}

fn bones() -> Document {
    edit(&Document::with_root("t"), json!([
        {"op":"add_bone","name":"control","x":10,"y":0},
        {"op":"add_bone","name":"follower","x":0,"y":20},
    ])).unwrap()
}

fn world(doc: &Document, pose: &Pose, name: &str) -> [f32; 2] {
    let w = &pose.bones[doc.bone_index(name).unwrap()].world;
    [w.tx, w.ty]
}

fn close(a: [f32; 2], b: [f32; 2]) {
    assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "{a:?} != {b:?}");
}

fn position_only(mix_x: f32, offset: [f32; 2]) -> Value {
    json!({"op":"add_transform","name":"copy","bones":["follower"],"source":"control","mix_rotate":0,"mix_x":mix_x,"mix_y":1,
        "mix_scale_x":0,"mix_scale_y":0,"mix_shear_y":0,"offset_x":offset[0],"offset_y":offset[1]})
}

#[test]
fn transform_offset_keeps_setup_and_follows_movement() {
    // The offset puts the source's target spot on the bone, as in the sample's rigs.
    let doc = edit(&bones(), position_only(-0.5, [-10.0, 20.0])).unwrap();
    close(world(&doc, &Pose::compute(&doc, None), "follower"), [0.0, 20.0]);
    let doc = edit(&doc, json!({"op":"key_bone","animation":"a","bone":"control","time":0,"translate":[4,6]})).unwrap();
    let pose = Pose::compute(&doc, Some((&doc.animations["a"], 0.0)));
    // A negative mix goes the other way, per axis.
    close(world(&doc, &pose, "follower"), [-2.0, 26.0]);
}

#[test]
fn transform_copies_rotation_the_short_way_and_scale() {
    let doc = edit(&bones(), json!([
        {"op":"set_bone","name":"control","rotation":170,"scale_x":2},
        {"op":"set_bone","name":"follower","rotation":-170},
        {"op":"add_transform","name":"copy","bones":["follower"],"source":"control","mix_rotate":0.5,"mix_x":0,"mix_y":0,"mix_scale_x":1,"mix_scale_y":0,"mix_shear_y":0},
    ])).unwrap();
    let w = Pose::compute(&doc, None).bones[doc.bone_index("follower").unwrap()].world;
    assert!((w.rotation_deg().abs() - 180.0).abs() < 1e-2, "{}", w.rotation_deg());
    assert!((w.scale_x() - 2.0).abs() < 1e-3);
}

#[test]
fn transform_local_and_relative() {
    let base = edit(&bones(), json!({"op":"set_bone","name":"control","rotation":30})).unwrap();
    let only_rotate = json!({"mix_x":0,"mix_y":0,"mix_scale_x":0,"mix_scale_y":0,"mix_shear_y":0});
    let add = |extra: Value| {
        let mut op = json!({"op":"add_transform","name":"copy","bones":["follower"],"source":"control"});
        for (k, v) in only_rotate.as_object().unwrap().iter().chain(extra.as_object().unwrap()) { op[k] = v.clone(); }
        op
    };
    let rotation = |doc: &Document| Pose::compute(doc, None).bones[doc.bone_index("follower").unwrap()].local.rotation;
    let setup = edit(&base, json!({"op":"set_bone","name":"follower","rotation":10})).unwrap();
    assert!((rotation(&edit(&setup, add(json!({"local":true,"relative":true}))).unwrap()) - 40.0).abs() < 1e-3);
    assert!((rotation(&edit(&setup, add(json!({"local":true}))).unwrap()) - 30.0).abs() < 1e-3);
}

#[test]
fn transform_follows_renames_and_removal_and_rejects_bad_input() {
    let doc = edit(&bones(), position_only(1.0, [0.0, 0.0])).unwrap();
    let renamed = edit(&doc, json!({"op":"set_bone","name":"control","rename":"boss"})).unwrap();
    assert_eq!(renamed.transforms[0].source, "boss");
    assert!(edit(&doc, json!({"op":"remove_bone","name":"control"})).unwrap().transforms.is_empty());
    assert!(edit(&doc, json!({"op":"remove_bone","name":"follower"})).unwrap().transforms.is_empty());
    assert!(edit(&doc, json!({"op":"remove_transform","name":"copy"})).unwrap().transforms.is_empty());
    let saved = skel2d_core::to_string(&doc);
    assert_eq!(skel2d_core::from_str(&saved).unwrap(), doc);
    for bad in [
        json!({"op":"add_transform","name":"copy","bones":["follower"],"source":"control"}),
        json!({"op":"add_transform","name":"own","bones":["control"],"source":"control"}),
        json!({"op":"add_transform","name":"none","bones":[],"source":"control"}),
        json!({"op":"add_transform","name":"gone","bones":["follower"],"source":"missing"}),
        json!({"op":"set_transform","name":"missing","mix_x":1}),
    ] {
        assert!(edit(&doc, bad.clone()).is_err(), "{bad}");
    }
    // Under its own bone the source would be moved by the result.
    let nested = edit(&doc, json!({"op":"add_bone","name":"child","parent":"follower"})).unwrap();
    assert!(edit(&nested, json!({"op":"set_transform","name":"copy","source":"child"})).is_err());
}

#[test]
fn transform_takes_part_in_constraint_order() {
    let doc = edit(&bones(), json!([position_only(1.0, [0.0, 0.0]), {"op":"add_physics","name":"p","bone":"control"}])).unwrap();
    let ordered = edit(&doc, json!({"op":"set_constraint_order","names":["p","copy"]})).unwrap();
    assert_eq!((ordered.physics[0].order, ordered.transforms[0].order), (0, 1));
    assert!(edit(&doc, json!({"op":"set_constraint_order","names":["p"]})).is_err());
}

#[test]
fn channels_follow_their_own_curves() {
    let keys = vec![
        Key { time: 0.0, value: [0.0, 0.0], curve: Curve::Linear, curves: vec![Curve::Stepped, Curve::Linear] },
        Key::new(1.0, [10.0, 10.0], Curve::Linear),
    ];
    assert_eq!(sample(&keys, 0.5), Some([0.0, 5.0]));
}

#[test]
fn key_ops_set_and_clear_channel_curves() {
    let doc = edit(&bones(), json!({"op":"key_bone","animation":"a","bone":"control","time":0,"translate":[0,0],"curves":["stepped","ease_in"]})).unwrap();
    assert_eq!(doc.animations["a"].bones["control"].translate[0].curves, [Curve::Stepped, Curve::EaseIn]);
    let saved = skel2d_core::to_string(&doc);
    assert_eq!(skel2d_core::from_str(&saved).unwrap(), doc);
    // A value edit keeps them; one curve for the whole key replaces them.
    let moved = edit(&doc, json!({"op":"key_bone","animation":"a","bone":"control","time":0,"translate":[1,1]})).unwrap();
    assert_eq!(moved.animations["a"].bones["control"].translate[0].curves.len(), 2);
    let one = edit(&doc, json!({"op":"key_bone","animation":"a","bone":"control","time":0,"translate":[1,1],"curve":"linear"})).unwrap();
    assert!(one.animations["a"].bones["control"].translate[0].curves.is_empty());
    assert!(edit(&doc, json!({"op":"key_bone","animation":"a","bone":"control","time":1,"translate":[0,0],"curves":["linear"]})).is_err());
    assert!(edit(&doc, json!({"op":"key_bone","animation":"a","bone":"control","time":1,"rotate":1,"curves":["linear","linear"]})).is_err());
    let mut bad = doc.clone();
    bad.animations.get_mut("a").unwrap().bones.get_mut("control").unwrap().translate[0].curves.pop();
    assert!(!skel2d_core::validate(&bad).is_empty());
}

/// Two bones each holding the vertex 5 units to its side.
fn bound() -> Document {
    edit(&bones(), json!([
        {"op":"add_slot","name":"s","bone":"root","attachment":"m","attachments":[{"type":"mesh","name":"m","hull":3,
            "vertices":[
                {"x":0,"y":0,"u":0,"v":0,"weights":[{"bone":"control","weight":0.5,"offset":[-5,0]},{"bone":"follower","weight":0.5,"offset":[5,0]}]},
                {"x":0,"y":10,"u":0,"v":1},
                {"x":10,"y":0,"u":1,"v":0}],
            "triangles":[[0,1,2]]}]},
        {"op":"key_bone","animation":"a","bone":"control","time":0,"rotate":90},
    ])).unwrap()
}

#[test]
fn bind_offsets_are_invisible_in_setup_and_carried_when_posed() {
    let doc = bound();
    let setup = Pose::compute(&doc, None);
    let skel2d_core::Attachment::Mesh(mesh) = doc.slots[0].attachment("m").unwrap() else { panic!() };
    close(setup.deform_mesh(&doc, mesh, 0)[0], [0.0, 0.0]);
    let saved = skel2d_core::to_string(&doc);
    assert_eq!(skel2d_core::from_str(&saved).unwrap(), doc);

    // control turns 90 degrees about (10, 0): the vertex at (0, 0) and the offset (-5, 0) turn with it.
    let pose = Pose::compute(&doc, Some((&doc.animations["a"], 0.0)));
    let [x, y] = pose.deform_mesh(&doc, mesh, 0)[0];
    let (control, follower) = ([10.0, -10.0], [0.0, 0.0]);
    close([x, y], [0.5 * (control[0] + 0.0) + 0.5 * (follower[0] + 5.0), 0.5 * (control[1] - 5.0) + 0.5 * follower[1]]);
}

#[test]
fn weight_edits_keep_a_bound_vertex_at_its_setup_position() {
    let doc = edit(&bound(), json!({"op":"set_bone_weight","slot":"s","attachment":"m","vertices":[0],"bone":"control","value":0.9,"mode":"set"})).unwrap();
    let skel2d_core::Attachment::Mesh(mesh) = doc.slots[0].attachment("m").unwrap() else { panic!() };
    close(Pose::compute(&doc, None).deform_mesh(&doc, mesh, 0)[0], [0.0, 0.0]);
}

#[test]
fn transform_mix_keys_blend_rename_and_go_with_the_constraint() {
    let doc = edit(&bones(), json!([position_only(1.0, [0.0, 0.0]),
        {"op":"key_bone","animation":"a","bone":"control","time":0,"translate":[10,0]},
        {"op":"key_transform","animation":"a","constraint":"copy","time":0,"mix":{"rotate":0,"x":0,"y":0,"scale_x":0,"scale_y":0,"shear_y":0}},
        {"op":"key_transform","animation":"a","constraint":"copy","time":1,"mix":{"rotate":0,"x":1,"y":0,"scale_x":0,"scale_y":0,"shear_y":0}},
    ])).unwrap();
    let at = |d: &Document, t| world(d, &Pose::compute(d, Some((&d.animations["a"], t))), "follower");
    close(at(&doc, 0.0), [0.0, 20.0]);
    close(at(&doc, 0.5), [10.0, 20.0]);
    close(at(&doc, 1.0), [20.0, 20.0]);
    assert_eq!(doc.animations["a"].duration(), 1.0);
    assert_eq!(skel2d_core::from_str(&skel2d_core::to_string(&doc)).unwrap(), doc);
    let renamed = edit(&doc, json!({"op":"set_transform","name":"copy","rename":"c2"})).unwrap();
    assert!(renamed.animations["a"].transforms.contains_key("c2"));
    let removed = edit(&doc, json!({"op":"remove_transform_key","animation":"a","constraint":"copy","time":0})).unwrap();
    assert_eq!(removed.animations["a"].transforms["copy"].len(), 1);
    assert!(edit(&doc, json!({"op":"remove_transform","name":"copy"})).unwrap().animations["a"].transforms.is_empty());
    assert!(edit(&doc, json!({"op":"remove_bone","name":"control"})).unwrap().animations["a"].transforms.is_empty());
    assert!(edit(&doc, json!({"op":"key_transform","animation":"a","constraint":"none","time":0,"mix":{}})).is_err());
}

#[test]
fn bone_color_is_saved_and_cleared() {
    let doc = edit(&bones(), json!({"op":"set_bone","name":"control","color":"#112233"})).unwrap();
    assert!(doc.bone("control").unwrap().color.is_some());
    assert_eq!(skel2d_core::from_str(&skel2d_core::to_string(&doc)).unwrap(), doc);
    let cleared = edit(&doc, json!({"op":"set_bone","name":"control","color":null})).unwrap();
    assert!(cleared.bone("control").unwrap().color.is_none());
}
