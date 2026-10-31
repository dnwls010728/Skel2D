use serde_json::{Value, json};
use skel2d_core::{Attachment, Document, Op, Pose, apply, from_str, parse_ops, to_string, validate};
use skel2d_core::pose::Track;

fn mesh(count: usize) -> Value {
    json!({"type":"mesh","name":"cloth","vertices":(0..count).map(|i| json!({"x":i as f32,"y":0,"u":0,"v":0})).collect::<Vec<_>>(),"triangles":[[0,1,2]]})
}

fn fixture() -> Document {
    from_str(&json!({"bones":[{"name":"root"},{"name":"limb","parent":"root"}],"slots":[{"name":"body","bone":"limb","attachment":"cloth","attachments":[mesh(4)]}],
        "skins":[{"name":"armor","attachments":{"body":[mesh(9)]}},{"name":"empty"}]}).to_string()).unwrap()
}

fn edit(doc: &Document, ops: Value) -> Document { apply(doc, &parse_ops(&ops.to_string()).unwrap()).unwrap() }

fn key(skin: Option<&str>, count: usize, dx: f32, time: f32) -> Value {
    let mut value = json!({"op":"key_deform","animation":"bend","slot":"body","attachment":"cloth","time":time,"offsets":vec![[dx,0.0];count]});
    if let Some(skin) = skin { value["skin"] = json!(skin); }
    value
}

fn keyed() -> Document { edit(&fixture(), json!([key(None,4,2.0,0.0),key(None,4,6.0,2.0),key(Some("armor"),9,10.0,0.0),key(Some("armor"),9,18.0,2.0)])) }

#[test]
fn four_and_nine_vertices_have_independent_numeric_deformation() {
    let doc = keyed();
    let tracks = [Track { animation:&doc.animations["bend"],time:1.0,alpha:0.5 }];
    for (skins, count, dx, source) in [(vec![],4,2.0,None),(vec!["armor".into()],9,7.0,Some("armor")),(vec!["armor".into(),"empty".into()],9,7.0,Some("armor")),(vec!["empty".into()],4,2.0,None)] {
        let pose = Pose::compute_skinned(&doc,&tracks,&skins);
        let Attachment::Mesh(m) = pose.attachment(&doc,0).unwrap() else { panic!() };
        let points = pose.deform_mesh(&doc,m,0);
        assert_eq!(points.len(),count);
        assert_eq!(points[0],[dx,0.0]);
        assert_eq!(doc.resolve_attachment_from(0,"cloth",&skins).unwrap().0,source);
    }
    assert_eq!(doc.animations["bend"].duration(),2.0);
    assert_eq!(from_str(&to_string(&doc)).unwrap(),doc);
}

#[test]
fn legacy_keys_migrate_and_preserve_pose() {
    let doc = edit(&fixture(),key(None,4,3.0,1.0));
    let mut legacy = serde_json::to_value(&doc).unwrap();
    let deform = legacy["animations"]["bend"].as_object_mut().unwrap().remove("deform").unwrap();
    legacy["animations"]["bend"]["slots"] = json!({"body":{"deform":deform["default"]["body"]}});
    let migrated = from_str(&legacy.to_string()).unwrap();
    assert_eq!(migrated.animations["bend"].deform,doc.animations["bend"].deform);
    assert_eq!(Pose::compute(&migrated,Some((&migrated.animations["bend"],1.0))),Pose::compute(&doc,Some((&doc.animations["bend"],1.0))));
    assert!(migrated.animations["bend"].slots.values().all(|s| s.is_empty()));
    legacy["animations"]["bend"]["deform"] = deform;
    assert!(from_str(&legacy.to_string()).is_err());
}

#[test]
fn wrong_skin_count_missing_targets_and_reserved_name_are_rejected_atomically() {
    let doc = keyed();
    let before = to_string(&doc);
    for op in [key(Some("armor"),4,1.0,0.0),key(None,9,1.0,0.0),key(Some("missing"),9,1.0,0.0),
        json!({"op":"add_skin","name":"default"}),json!({"op":"set_skin","name":"armor","rename":"default"}),
        json!({"op":"rename_attachment","slot":"body","from":"cloth","to":" "})] {
        let ops = parse_ops(&json!([{"op":"set_name","name":"must roll back"},op]).to_string()).unwrap();
        assert!(apply(&doc,&ops).is_err());
        assert_eq!(to_string(&doc),before);
    }
    for change in ["skin","slot","attachment","count","time","value"] {
        let mut bad = doc.clone();
        let slots = bad.animations.get_mut("bend").unwrap().deform.get_mut("armor").unwrap();
        match change {
            "skin" => { let v = slots.clone(); bad.animations.get_mut("bend").unwrap().deform.insert("missing".into(),v); }
            "slot" => { let v = slots.remove("body").unwrap(); slots.insert("missing".into(),v); }
            "attachment" => { let atts = slots.get_mut("body").unwrap(); let v = atts.remove("cloth").unwrap(); atts.insert("missing".into(),v); }
            "count" => { slots.get_mut("body").unwrap().get_mut("cloth").unwrap()[0].value.pop(); }
            "time" => slots.get_mut("body").unwrap().get_mut("cloth").unwrap()[0].time = -1.0,
            _ => slots.get_mut("body").unwrap().get_mut("cloth").unwrap()[0].value[0][0] = f32::NAN,
        }
        assert!(!validate(&bad).is_empty(),"{change}");
    }
}

#[test]
fn rename_local_and_all_placeholders_update_references_and_keep_other_skin_keys() {
    let doc = edit(&keyed(),json!({"op":"key_slot","animation":"bend","slot":"body","time":0,"attachment":"cloth"}));
    for (skin, owner) in [(None,"default"),(Some("armor"),"armor")] {
        let mut op = json!({"op":"rename_attachment","slot":"body","from":"cloth","to":"new"});
        if let Some(s) = skin { op["skin"] = json!(s); }
        let next = edit(&doc,op);
        let a = &next.animations["bend"];
        assert!(a.deform[owner]["body"].contains_key("new"));
        let other = if owner == "default" {"armor"} else {"default"};
        assert!(a.deform[other]["body"].contains_key("cloth"));
        // The other owner still fills "cloth", so shared references stay on it.
        assert_eq!(next.slots[0].attachment.as_deref(),Some("cloth"));
        assert_eq!(a.slots["body"].attachment[0].name.as_deref(),Some("cloth"));
    }
    let next = edit(&doc,json!({"op":"rename_placeholder","slot":"body","from":"cloth","to":"new"}));
    assert!(next.slots[0].attachment("new").is_some());
    assert!(next.skin("armor").unwrap().attachment("body","new").is_some());
    for s in ["default","armor"] { assert!(next.animations["bend"].deform[s]["body"].contains_key("new")); }
    let mut extra = mesh(9); extra["name"] = json!("taken");
    let collision = edit(&doc,json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":extra}));
    assert!(apply(&collision,&parse_ops(r#"{"op":"rename_placeholder","slot":"body","from":"cloth","to":"taken"}"#).unwrap()).is_err());
}

#[test]
fn rename_and_remove_skin_slot_bone_and_attachments_clean_deformation() {
    let doc = keyed();
    let renamed = edit(&doc,json!([{"op":"set_skin","name":"armor","rename":"metal"},{"op":"set_slot","name":"body","rename":"torso"}]));
    assert!(renamed.animations["bend"].deform["metal"].contains_key("torso"));
    for (op, skin) in [(json!({"op":"remove_attachment","slot":"body","name":"cloth"}),"default"),
        (json!({"op":"remove_skin_attachment","skin":"armor","slot":"body","name":"cloth"}),"armor"),
        (json!({"op":"remove_skin","name":"armor"}),"armor")] {
        let next = edit(&doc,op);
        assert!(!next.animations["bend"].deform.contains_key(skin));
        assert_eq!(next.slots[0].attachment.as_deref(),Some("cloth"));
    }
    for op in [json!({"op":"remove_slot","name":"body"}),json!({"op":"remove_bone","name":"limb","recursive":true})] {
        let next = edit(&doc,op);
        assert!(next.animations["bend"].deform.values().all(|s| !s.contains_key("body")));
    }
    let next = edit(&doc,json!([{"op":"remove_attachment","slot":"body","name":"cloth"},{"op":"remove_skin","name":"armor"}]));
    assert_eq!(next.slots[0].attachment,None);
}

#[test]
fn replace_mesh_resizes_only_its_skin_and_region_clears_only_its_skin() {
    let doc = keyed();
    let next = edit(&doc,json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":mesh(5)}));
    assert_eq!(next.animations["bend"].deform["armor"]["body"]["cloth"][0].value.len(),5);
    assert_eq!(next.animations["bend"].deform["default"]["body"]["cloth"][0].value.len(),4);
    let next = edit(&doc,json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{"type":"region","name":"cloth","width":10,"height":10}}));
    assert!(!next.animations["bend"].deform["armor"]["body"].contains_key("cloth"));
    assert!(next.animations["bend"].deform["default"]["body"].contains_key("cloth"));
}

#[test]
fn remove_and_clear_deform_timeline_respect_skin_without_slot_timelines() {
    let doc = keyed();
    let next = edit(&doc,json!({"op":"remove_key","animation":"bend","slot":"body","skin":"armor","timeline":"deform","attachment":"cloth","time":0}));
    assert_eq!(next.animations["bend"].deform["armor"]["body"]["cloth"].len(),1);
    assert_eq!(next.animations["bend"].deform["default"]["body"]["cloth"].len(),2);
    let next = edit(&next,json!({"op":"clear_timeline","animation":"bend","slot":"body","skin":"armor","timeline":"deform"}));
    assert!(!next.animations["bend"].deform.contains_key("armor"));
    assert!(next.animations["bend"].deform.contains_key("default"));
    let next = edit(&next,json!({"op":"clear_timeline","animation":"bend","slot":"body"}));
    assert!(next.animations["bend"].deform.is_empty());
    let schema = serde_json::to_string(&schemars::schema_for!(Op)).unwrap();
    for name in ["rename_attachment","rename_placeholder","skin"] { assert!(schema.contains(name)); }
}
