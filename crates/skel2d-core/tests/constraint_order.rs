use skel2d_core::{Document, Pose, apply, from_str, parse_ops, to_string, validate};

fn document() -> Document {
    apply(&Document::new("order"), &parse_ops(r#"[
        {"op":"add_bone","name":"arm","length":10},
        {"op":"add_bone","name":"tip","parent":"arm","x":10},
        {"op":"add_bone","name":"up","y":20},
        {"op":"add_bone","name":"right","x":20},
        {"op":"add_ik","name":"aim_up","bones":["arm"],"target":"up"},
        {"op":"add_ik","name":"aim_right","bones":["arm"],"target":"right"}
    ]"#).unwrap()).unwrap()
}

fn edited(doc: &Document, ops: &str) -> Document {
    apply(doc, &parse_ops(ops).unwrap()).unwrap()
}

#[test]
fn legacy_files_keep_array_order_and_omit_zero_priority() {
    let doc = document();
    let json = to_string(&doc);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value["ik"].as_array().unwrap().iter().all(|c| c.get("order").is_none()));
    assert_eq!(from_str(&json).unwrap(), doc);
    let pose = Pose::compute(&doc, None);
    assert!(pose.bones[doc.bone_index("arm").unwrap()].world.rotation_deg().abs() < 0.001);
}

#[test]
fn priority_changes_pose_and_updates_descendants_before_next_constraint() {
    let doc = edited(&document(), r#"{"op":"set_constraint_order","names":["aim_right","aim_up"]}"#);
    assert_eq!(doc.ik[0].order, 1);
    assert_eq!(doc.ik[1].order, 0);
    let pose = Pose::compute(&doc, None);
    let tip = pose.bones[doc.bone_index("tip").unwrap()].world.apply(0.0, 0.0);
    assert!(tip[0].abs() < 0.001 && (tip[1] - 10.0).abs() < 0.001);
    let chained = edited(&doc, r#"[
        {"op":"add_bone","name":"follower","x":-10},
        {"op":"add_ik","name":"follow","bones":["follower"],"target":"tip","order":2}
    ]"#);
    let pose = Pose::compute(&chained, None);
    assert!((pose.bones[chained.bone_index("follower").unwrap()].world.rotation_deg() - 45.0).abs() < 0.001);
    assert_eq!(from_str(&to_string(&chained)).unwrap(), chained);
}

#[test]
fn invalid_permutations_fail_atomically_even_after_an_earlier_edit() {
    let doc = document();
    let snapshot = doc.clone();
    for names in [r#"[]"#, r#"["aim_up"]"#, r#"["aim_up","aim_up"]"#, r#"["aim_up","missing"]"#,
        r#"["aim_up","aim_right","missing"]"#] {
        let ops = format!(r#"[{{"op":"set_ik","name":"aim_up","order":10}},{{"op":"set_constraint_order","names":{names}}}]"#);
        assert!(apply(&doc, &parse_ops(&ops).unwrap()).is_err());
        assert_eq!(doc, snapshot);
    }
    let empty = Document::new("empty");
    assert_eq!(edited(&empty, r#"{"op":"set_constraint_order","names":[]}"#), empty);
}

#[test]
fn priorities_support_full_i32_range_and_stable_ties() {
    let doc = edited(&document(), r#"[
        {"op":"set_ik","name":"aim_up","order":2147483647},
        {"op":"set_ik","name":"aim_right","order":-2147483648}
    ]"#);
    assert!(validate(&doc).is_empty());
    assert_eq!(from_str(&to_string(&doc)).unwrap(), doc);
    assert!((Pose::compute(&doc, None).bones[0].world.rotation_deg() - 90.0).abs() < 0.001);
    let tied = edited(&doc, r#"{"op":"set_ik","name":"aim_up","order":-2147483648}"#);
    assert!(Pose::compute(&tied, None).bones[0].world.rotation_deg().abs() < 0.001);
    for order in ["0.5", "2147483648", "null", "\"first\""] {
        assert!(parse_ops(&format!(r#"{{"op":"add_ik","name":"bad","bones":["arm"],"target":"up","order":{order}}}"#)).is_err());
    }
}

#[test]
fn reorder_uses_current_names_after_rename_and_removal() {
    let doc = edited(&document(), r#"[
        {"op":"set_ik","name":"aim_up","rename":"renamed"},
        {"op":"set_constraint_order","names":["aim_right","renamed"]},
        {"op":"remove_ik","name":"aim_right"},
        {"op":"set_constraint_order","names":["renamed"]}
    ]"#);
    assert_eq!(doc.ik.len(), 1);
    assert_eq!(doc.ik[0].name, "renamed");
    assert_eq!(doc.ik[0].order, 0);
}
