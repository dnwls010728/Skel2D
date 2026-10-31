use serde_json::json;
use skel2d_core::{Document, Pose, ROOT, apply, parse_ops, validate};

fn ops(d: &Document, v: serde_json::Value) -> skel2d_core::Result<Document> {
    apply(d, &parse_ops(&v.to_string()).unwrap())
}

#[test]
fn new_bones_go_under_the_root_and_it_cannot_be_lost() {
    let d = ops(&Document::with_root("t"), json!([
        {"op":"add_bone","name":"a"},
        {"op":"add_bone_world","name":"b","start":[1,2]},
        {"op":"add_bone","name":"c","parent":"a"}
    ])).unwrap();
    assert_eq!(d.bone("a").unwrap().parent.as_deref(), Some(ROOT));
    assert_eq!(d.bone("b").unwrap().parent.as_deref(), Some(ROOT));
    assert_eq!(d.bone("c").unwrap().parent.as_deref(), Some("a"));
    for bad in [
        json!({"op":"remove_bone","name":"root","recursive":true}),
        json!({"op":"set_bone","name":"root","rename":"hip"}),
        json!({"op":"set_bone","name":"root","parent":"a"}),
        json!({"op":"set_bone","name":"a","parent":null}),
        json!({"op":"add_bone","name":"b2","parent":"missing"}),
    ] {
        assert!(ops(&d, bad).is_err());
    }
    assert!(ops(&d, json!({"op":"set_bone","name":"root","x":5})).is_ok());
}

#[test]
fn older_documents_gain_a_root_without_moving_anything() {
    let dir = std::env::temp_dir().join(format!("skel2d-root-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = [
        // No root at all.
        json!({"bones":[{"name":"hip","rotation":30,"x":4},{"name":"leg","parent":"hip","x":2}]}),
        // A root beside a free bone.
        json!({"bones":[{"name":"root","x":10,"rotation":90,"scale_x":2},{"name":"goal","x":3,"y":1,"rotation":15}]}),
    ];
    for (i, case) in cases.iter().enumerate() {
        let path = dir.join(format!("{i}.skel.json"));
        std::fs::write(&path, case.to_string()).unwrap();
        let before = skel2d_core::from_str(&case.to_string()).unwrap();
        let after = skel2d_core::load(&path).unwrap();
        assert!(validate(&after).is_empty());
        assert!(after.bones.iter().all(|b| (b.name == ROOT) == b.parent.is_none()), "{:?}", after.bones);
        let (a, b) = (Pose::compute(&before, None), Pose::compute(&after, None));
        for bone in &before.bones {
            let (p, q) = (a.bones[before.bone_index(&bone.name).unwrap()].world, b.bones[after.bone_index(&bone.name).unwrap()].world);
            for (x, y) in [p.a, p.b, p.c, p.d, p.tx, p.ty].into_iter().zip([q.a, q.b, q.c, q.d, q.tx, q.ty]) {
                assert!((x - y).abs() < 1e-3, "{}: {x} {y}", bone.name);
            }
        }
    }
    std::fs::remove_dir_all(dir).ok();
}
