use serde_json::json;
use skel2d_core::{Attachment, Document, Pose, apply, from_str, parse_ops, to_string, validate};

fn fixture() -> Document {
    from_str(&json!({
        "bones":[{"name":"root"}],
        "slots":[{"name":"body","bone":"root","attachment":"parent","attachments":[
            {"type":"mesh","name":"parent","vertices":[
                {"x":0,"y":0,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":0},{"x":0,"y":1,"u":0,"v":1}
            ],"triangles":[[0,1,2]]}
        ]}],
        "skins":[{"name":"armor"}]
    }).to_string()).unwrap()
}

fn edit(doc: &Document, ops: serde_json::Value) -> Document {
    apply(doc, &parse_ops(&ops.to_string()).unwrap()).unwrap()
}

#[test]
fn linked_geometry_follows_parent_and_deform_toggle() {
    let doc = edit(&fixture(), json!([
        {"op":"add_linked_mesh","slot":"body","name":"child","parent":"parent","skin":"armor","image":"other.png"},
        {"op":"set_slot","name":"body","attachment":"child"},
        {"op":"key_deform","animation":"bend","slot":"body","attachment":"parent","time":0,"offsets":[[2,0],[2,0],[2,0]]}
    ]));
    let skins = vec!["armor".to_string()];
    let pose = Pose::compute_skinned(&doc, &[skel2d_core::pose::Track { animation:&doc.animations["bend"], time:0.0, alpha:1.0 }], &skins);
    let link = pose.attachment(&doc, 0).unwrap();
    let Attachment::Mesh(mesh) = doc.materialize_attachment(0, link).unwrap() else { panic!() };
    assert_eq!(mesh.image.as_deref(), Some("other.png"));
    assert_eq!(pose.deform_mesh(&doc, &mesh, 0)[0], [2.0, 0.0]);
    assert_eq!(pose.report(&doc).slots[0].source_skin.as_deref(), Some("armor"));
    let next = edit(&doc, json!({"op":"set_attachment","slot":"body","attachment":{
        "type":"mesh","name":"parent","vertices":[
            {"x":10,"y":0,"u":0,"v":0},{"x":11,"y":0,"u":1,"v":0},{"x":10,"y":1,"u":0,"v":1}
        ],"triangles":[[0,1,2]]}}));
    let Attachment::Mesh(m) = next.materialize_attachment(0, next.skin("armor").unwrap().attachment("body", "child").unwrap()).unwrap() else { panic!() };
    assert_eq!(m.vertices[0].x, 10.0);
    let disabled = edit(&doc, json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{
        "type":"linked_mesh","name":"child","parent":"parent","timelines":false}}));
    let pose = Pose::compute_skinned(&disabled, &[skel2d_core::pose::Track { animation:&disabled.animations["bend"], time:0.0, alpha:1.0 }], &skins);
    let Attachment::Mesh(m) = disabled.materialize_attachment(0, pose.attachment(&disabled, 0).unwrap()).unwrap() else { panic!() };
    assert_eq!(pose.deform_mesh(&disabled, &m, 0)[0], [0.0, 0.0]);
    let own_key = edit(&disabled, json!({"op":"key_deform","animation":"bend","slot":"body","skin":"armor","attachment":"child","time":1,
        "offsets":[[1,0],[1,0],[1,0]]}));
    let grown = edit(&own_key, json!({"op":"set_attachment","slot":"body","attachment":{
        "type":"mesh","name":"parent","vertices":[
            {"x":0,"y":0,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":0},{"x":0,"y":1,"u":0,"v":1},{"x":1,"y":1,"u":1,"v":1}
        ],"triangles":[[0,1,2],[1,3,2]]}}));
    assert_eq!(grown.animations["bend"].deform["armor"]["body"]["child"][0].value.len(), 4);
    let keyed_from_child = edit(&doc, json!({"op":"key_deform","animation":"bend","slot":"body","skin":"armor","attachment":"child","time":1,
        "offsets":[[4,0],[4,0],[4,0]]}));
    assert!(keyed_from_child.animations["bend"].deform["default"]["body"]["parent"].iter().any(|k| k.time == 1.0));
    assert!(!keyed_from_child.animations["bend"].deform.contains_key("armor"));
    let removed = edit(&keyed_from_child, json!({"op":"remove_key","animation":"bend","slot":"body","skin":"armor","timeline":"deform","attachment":"child","time":1}));
    assert!(removed.animations["bend"].deform["default"]["body"]["parent"].iter().all(|k| k.time != 1.0));
}

#[test]
fn parent_removal_is_explicit_and_atomic() {
    let doc = edit(&fixture(), json!([{"op":"add_linked_mesh","slot":"body","name":"child","parent":"parent","skin":"armor"},{"op":"set_slot","name":"body","attachment":"child"}]));
    let chained = parse_ops(r#"{"op":"add_linked_mesh","slot":"body","name":"grandchild","parent":"child","parent_skin":"armor"}"#).unwrap();
    assert!(apply(&doc, &chained).is_err());
    assert!(apply(&doc, &parse_ops(r#"[{"op":"set_name","name":"rollback"},{"op":"remove_attachment","slot":"body","name":"parent"}]"#).unwrap()).is_err());
    assert_eq!(doc.name, "");
    let frozen = edit(&doc, json!({"op":"remove_attachment","slot":"body","name":"parent","on_parent_removed":"freeze"}));
    assert!(matches!(frozen.skin("armor").unwrap().attachment("body","child"), Some(Attachment::Mesh(_))));
    assert!(validate(&frozen).is_empty());
    let deleted = edit(&doc, json!({"op":"remove_attachment","slot":"body","name":"parent","on_parent_removed":"delete"}));
    assert!(deleted.skin("armor").unwrap().attachment("body","child").is_none());
    assert!(deleted.slots[0].attachment.is_none());
}

#[test]
fn parent_rename_and_legacy_skin_constraints_migrate() {
    let doc = edit(&fixture(), json!({"op":"add_linked_mesh","slot":"body","name":"child","parent":"parent","skin":"armor"}));
    let renamed = edit(&doc, json!({"op":"rename_attachment","slot":"body","from":"parent","to":"new"}));
    let Some(Attachment::LinkedMesh(link)) = renamed.skin("armor").unwrap().attachment("body","child") else { panic!() };
    assert_eq!(link.parent, "new");
    assert!(validate(&renamed).is_empty());
    let mut legacy = serde_json::to_value(&renamed).unwrap();
    legacy["skins"][0]["ik"] = json!([]);
    legacy["skins"][0].as_object_mut().unwrap().remove("constraints");
    let migrated = from_str(&legacy.to_string()).unwrap();
    assert_eq!(migrated, renamed);
    assert!(!to_string(&migrated).contains("\"ik\""));
    legacy["skins"][0]["constraints"] = json!([]);
    assert!(from_str(&legacy.to_string()).is_err());
    let bad = parse_ops(&json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{
        "type":"linked_mesh","name":"child","parent":"missing"}}).to_string()).unwrap();
    assert!(apply(&fixture(), &bad).is_err());
}

#[test]
fn skin_constraints_gate_ik_and_follow_rename_removal() {
    let doc = from_str(&json!({
        "bones":[{"name":"arm","length":10},{"name":"goal","x":0,"y":10}],
        "ik":[{"name":"reach","bones":["arm"],"target":"goal"}],
        "skins":[{"name":"armor","constraints":["reach"]}]
    }).to_string()).unwrap();
    let off = Pose::compute_skinned(&doc, &[], &[]);
    let on = Pose::compute_skinned(&doc, &[], &["armor".into()]);
    assert!(off.bones[0].world.rotation_deg().abs() < 0.01);
    assert!((on.bones[0].world.rotation_deg() - 90.0).abs() < 0.01);
    let mut legacy = serde_json::to_value(&doc).unwrap();
    legacy["skins"][0].as_object_mut().unwrap().remove("constraints");
    legacy["skins"][0]["ik"] = json!(["reach"]);
    assert_eq!(from_str(&legacy.to_string()).unwrap(), doc);
    let renamed = edit(&doc, json!({"op":"set_ik","name":"reach","rename":"aim"}));
    assert_eq!(renamed.skin("armor").unwrap().constraints, ["aim"]);
    let removed = edit(&renamed, json!({"op":"remove_ik","name":"aim"}));
    assert!(removed.skin("armor").unwrap().constraints.is_empty());
    assert!(apply(&doc, &parse_ops(r#"{"op":"set_skin","name":"armor","constraints":["missing"]}"#).unwrap()).is_err());
}

#[test]
fn removing_parent_skin_requires_policy_and_freezes_external_link() {
    let doc = edit(&fixture(), json!([
        {"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{
            "type":"mesh","name":"armor_parent","vertices":[
                {"x":3,"y":0,"u":0,"v":0},{"x":4,"y":0,"u":1,"v":0},{"x":3,"y":1,"u":0,"v":1}
            ],"triangles":[[0,1,2]]}},
        {"op":"add_linked_mesh","slot":"body","name":"child","parent":"armor_parent","parent_skin":"armor"}
    ]));
    assert!(apply(&doc, &parse_ops(r#"{"op":"remove_skin","name":"armor"}"#).unwrap()).is_err());
    let renamed = edit(&doc, json!({"op":"set_skin","name":"armor","rename":"metal"}));
    let Some(Attachment::LinkedMesh(link)) = renamed.slot("body").unwrap().attachment("child") else { panic!() };
    assert_eq!(link.parent_skin.as_deref(), Some("metal"));
    let frozen = edit(&renamed, json!({"op":"remove_skin","name":"metal","on_parent_removed":"freeze"}));
    let Some(Attachment::Mesh(mesh)) = frozen.slot("body").unwrap().attachment("child") else { panic!() };
    assert_eq!(mesh.vertices[0].x, 3.0);
}

fn five_vertex_fixture() -> Document {
    from_str(&json!({
        "bones":[{"name":"root"}],
        "slots":[{"name":"body","bone":"root","attachment":"parent","attachments":[
            {"type":"mesh","name":"parent","vertices":[
                {"x":0,"y":0,"u":0,"v":1},{"x":0,"y":1,"u":0,"v":0},{"x":1,"y":1,"u":1,"v":0},{"x":1,"y":0,"u":1,"v":1},{"x":0.5,"y":0.5,"u":0.5,"v":0.5}
            ],"triangles":[[0,1,4],[1,2,4],[2,3,4],[3,0,4]]}
        ]}],
        "skins":[{"name":"armor"}]
    }).to_string()).unwrap()
}

#[test]
fn parent_topology_edits_remap_own_child_keys() {
    let doc = edit(&five_vertex_fixture(), json!([
        {"op":"add_linked_mesh","slot":"body","name":"child","parent":"parent","skin":"armor","timelines":false},
        {"op":"key_deform","animation":"a","slot":"body","skin":"armor","attachment":"child","time":0,"offsets":[[0,0],[1,0],[2,0],[3,0],[4,0]]}
    ]));
    let keys = |d: &Document| d.animations["a"].deform["armor"]["body"]["child"][0].value.clone();
    let removed = edit(&doc, json!({"op":"remove_mesh_vertices","slot":"body","attachment":"parent","indices":[4]}));
    let values = keys(&removed);
    assert_eq!(values.len(), 4);
    assert!(!values.contains(&[4.0, 0.0]), "{values:?}");
    assert_eq!(keys(&edit(&doc, json!({"op":"add_mesh_vertices","slot":"body","attachment":"parent","points":[[0.25,0.75]]}))).len(), 6);
    assert_eq!(keys(&edit(&doc, json!({"op":"generate_mesh_vertices","slot":"body","attachment":"parent","count":3}))).len(), 8);
}

#[test]
fn shared_timelines_ignore_and_drop_stale_own_keys() {
    let own = edit(&five_vertex_fixture(), json!([
        {"op":"add_linked_mesh","slot":"body","name":"child","parent":"parent","skin":"armor","timelines":false},
        {"op":"key_deform","animation":"a","slot":"body","skin":"armor","attachment":"child","time":0,"offsets":[[1,0],[1,0],[1,0],[1,0],[1,0]]}
    ]));
    let shared = edit(&own, json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{"type":"linked_mesh","name":"child","parent":"parent"}}));
    assert!(!shared.animations["a"].deform.contains_key("armor"));
    // A hand-written file with leftover keys is ignored at evaluation too.
    let mut stale = shared.clone();
    stale.animations.get_mut("a").unwrap().deform = own.animations["a"].deform.clone();
    stale.slots[0].attachment = Some("child".into());
    let skins = vec!["armor".to_string()];
    let pose = Pose::compute_skinned(&stale, &[skel2d_core::pose::Track { animation:&stale.animations["a"], time:0.0, alpha:1.0 }], &skins);
    assert!(pose.deform_offsets(0, "child").is_empty());
}

#[test]
fn skin_local_rename_keeps_other_owners_visible() {
    let doc = edit(&five_vertex_fixture(), json!({"op":"set_skin_attachment","skin":"armor","slot":"body","attachment":{"type":"region","name":"parent","width":1,"height":1}}));
    let armor = edit(&doc, json!({"op":"rename_attachment","slot":"body","from":"parent","to":"plate","skin":"armor"}));
    assert_eq!(Pose::compute_skinned(&armor, &[], &[]).attachment(&armor, 0).map(|a| a.name().to_string()).as_deref(), Some("parent"));
    let base = edit(&doc, json!({"op":"rename_attachment","slot":"body","from":"parent","to":"base"}));
    let shown = Pose::compute_skinned(&base, &[], &["armor".to_string()]);
    assert!(matches!(shown.attachment(&base, 0), Some(Attachment::Region(_))));
    // The last owner renaming moves the shared references with it.
    let both = edit(&armor, json!({"op":"rename_attachment","slot":"body","from":"parent","to":"base"}));
    assert_eq!(both.slots[0].attachment.as_deref(), Some("base"));
}
