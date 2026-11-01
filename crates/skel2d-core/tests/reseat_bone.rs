use skel2d_core::{Attachment, Document, Pose, apply, load, parse_ops, validate};

fn edit(doc: &Document, ops: &str) -> Result<Document, String> {
    apply(doc, &parse_ops(ops).unwrap()).map_err(|e| e.to_string())
}

fn close(a: [f32; 2], b: [f32; 2], what: &str) {
    assert!((a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01, "{what}: {a:?} vs {b:?}");
}

/// Rotated, non-uniformly scaled parents and a child with its own transform.
fn rig() -> Document {
    edit(
        &Document::new("t"),
        r#"[
        {"op":"add_bone","name":"root","x":5,"y":-3,"rotation":20,"scale_x":1.5},
        {"op":"add_bone","name":"mid","parent":"root","x":30,"y":10,"rotation":-35,"scale_x":0.8,"scale_y":1.3},
        {"op":"add_bone","name":"child","parent":"mid","x":12,"y":4,"rotation":50},
        {"op":"add_bone","name":"grand","parent":"child","x":9},
        {"op":"add_slot","name":"s","bone":"mid","attachment":"r","attachments":[{"type":"region","name":"r","x":3,"y":7,"rotation":15,"width":10,"height":6}]},
        {"op":"add_skin","name":"alt"},
        {"op":"set_skin_attachment","skin":"alt","slot":"s","attachment":{"type":"region","name":"r","x":-2,"y":1,"width":4,"height":4}}
    ]"#,
    )
    .unwrap()
}

fn world(doc: &Document, bone: &str) -> [f32; 2] {
    let w = Pose::compute(doc, None).bones[doc.bone_index(bone).unwrap()].world;
    w.apply(0.0, 0.0)
}

fn region_world(doc: &Document, a: &Attachment) -> [f32; 2] {
    let Attachment::Region(r) = a else { panic!() };
    let w = Pose::compute(doc, None).bones[doc.bone_index("mid").unwrap()].world;
    w.apply(r.x, r.y)
}

#[test]
fn only_the_bone_moves() {
    let doc = rig();
    let next = edit(&doc, r#"{"op":"reseat_bone","name":"mid","x":50,"y":-20}"#).unwrap();
    assert!(validate(&next).is_empty());
    let mid = next.bone("mid").unwrap();
    assert_eq!((mid.x, mid.y), (50.0, -20.0));
    assert_ne!(world(&doc, "mid"), world(&next, "mid"));
    close(world(&doc, "child"), world(&next, "child"), "child");
    close(world(&doc, "grand"), world(&next, "grand"), "grandchild");
    close(world(&doc, "root"), world(&next, "root"), "parent");
    close(region_world(&doc, &doc.slots[0].attachments[0]), region_world(&next, &next.slots[0].attachments[0]), "region");
    let skin = |d: &Document| region_world(d, &d.skins[0].attachments["s"][0]);
    close(skin(&doc), skin(&next), "skin region");
}

#[test]
fn a_root_can_be_reseated_without_moving_the_art() {
    let doc = rig();
    let next = edit(&doc, r#"{"op":"reseat_bone","name":"root","x":-40,"y":25}"#).unwrap();
    assert_eq!(world(&next, "root"), [-40.0, 25.0]);
    close(world(&doc, "mid"), world(&next, "mid"), "child of root");
    close(world(&doc, "grand"), world(&next, "grand"), "deep descendant");
}

#[test]
fn weighted_meshes_stay_put() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/arm/arm.skel.json");
    let mut doc = load(&path).unwrap();
    // The rig's IK would re-solve around the new joint, which is a different question.
    doc.ik.clear();
    doc.animations.clear();
    let next = edit(&doc, r#"{"op":"reseat_bone","name":"lower","x":70,"y":6}"#).unwrap();
    let Attachment::Mesh(before) = &doc.slots[0].attachments[0] else { panic!() };
    let Attachment::Mesh(after) = &next.slots[0].attachments[0] else { panic!() };
    let (p0, p1) = (Pose::compute(&doc, None).deform_mesh(&doc, before, 0), Pose::compute(&next, None).deform_mesh(&next, after, 0));
    for (a, b) in p0.iter().zip(&p1) {
        close(*a, *b, "mesh vertex");
    }
}

#[test]
fn redrawing_sets_the_bone_and_keeps_the_rest_in_place() {
    let doc = rig();
    let next = edit(&doc, r#"{"op":"redraw_bone","name":"mid","start":[20,30],"end":[20,90]}"#).unwrap();
    assert!(validate(&next).is_empty());
    close(world(&next, "mid"), [20.0, 30.0], "the new start");
    let w = Pose::compute(&next, None).bones[next.bone_index("mid").unwrap()].world;
    close(w.apply(next.bone("mid").unwrap().length, 0.0), [20.0, 90.0], "the tip reaches the new end");
    let full = |d: &Document, b: &str| {
        let m = Pose::compute(d, None).bones[d.bone_index(b).unwrap()].world;
        [m.apply(0.0, 0.0), m.apply(10.0, 0.0), m.apply(0.0, 10.0)]
    };
    for b in ["child", "grand", "root"] {
        let (a, n) = (full(&doc, b), full(&next, b));
        for k in 0..3 {
            close(a[k], n[k], b);
        }
    }
    let region = |d: &Document| {
        let Attachment::Region(r) = &d.slots[0].attachments[0] else { panic!() };
        let m = Pose::compute(d, None).bones[d.bone_index("mid").unwrap()].world.mul(&skel2d_core::Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));
        m.apply(0.0, 0.0)
    };
    close(region(&doc), region(&next), "region position");
    assert!(edit(&doc, r#"{"op":"redraw_bone","name":"mid","start":[5,5],"end":[5,5]}"#).is_err(), "no direction");
}

#[test]
fn redrawing_leaves_meshes_alone() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/arm/arm.skel.json");
    let mut doc = load(&path).unwrap();
    doc.ik.clear();
    doc.animations.clear();
    let next = edit(&doc, r#"{"op":"redraw_bone","name":"lower","start":[70,6],"end":[120,40]}"#).unwrap();
    let Attachment::Mesh(before) = &doc.slots[0].attachments[0] else { panic!() };
    let Attachment::Mesh(after) = &next.slots[0].attachments[0] else { panic!() };
    let (p0, p1) = (Pose::compute(&doc, None).deform_mesh(&doc, before, 0), Pose::compute(&next, None).deform_mesh(&next, after, 0));
    for (a, b) in p0.iter().zip(&p1) {
        close(*a, *b, "mesh vertex");
    }
}

#[test]
fn refusals() {
    let doc = rig();
    assert!(edit(&doc, r#"{"op":"reseat_bone","name":"nope","x":0,"y":0}"#).is_err());
    let flat = edit(&doc, r#"{"op":"set_bone","name":"mid","scale_x":0}"#).unwrap();
    let err = edit(&flat, r#"{"op":"reseat_bone","name":"mid","x":1,"y":1}"#).unwrap_err();
    assert!(err.contains("zero scale"), "{err}");
}
