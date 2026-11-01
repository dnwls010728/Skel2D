use skel2d_core::{Document, apply, from_str, parse_ops, to_string, validate};

fn edit(doc: &Document, ops: &str) -> Result<Document, String> {
    apply(doc, &parse_ops(ops).unwrap()).map_err(|e| e.to_string())
}

fn rig() -> Document {
    edit(
        &Document::with_root("t"),
        r#"[
        {"op":"add_slot","name":"base","bone":"root"},
        {"op":"add_slot","name":"shade","bone":"root","clip":"base"},
        {"op":"add_slot","name":"free","bone":"root"}
    ]"#,
    )
    .unwrap()
}

#[test]
fn clip_survives_a_save_and_can_be_changed() {
    let doc = rig();
    assert!(validate(&doc).is_empty());
    let back = from_str(&to_string(&doc)).unwrap();
    assert_eq!(back.slot("shade").unwrap().clip.as_deref(), Some("base"));
    assert_eq!(back.slot("free").unwrap().clip, None);
    let set = edit(&doc, r#"{"op":"set_slot","name":"free","clip":"base"}"#).unwrap();
    assert_eq!(set.slot("free").unwrap().clip.as_deref(), Some("base"));
    let cleared = edit(&set, r#"{"op":"set_slot","name":"free","clip":null}"#).unwrap();
    assert_eq!(cleared.slot("free").unwrap().clip, None);
    let other = edit(&set, r##"{"op":"set_slot","name":"free","color":"#ff0000"}"##).unwrap();
    assert_eq!(other.slot("free").unwrap().clip.as_deref(), Some("base"), "other edits leave the clip alone");
}

#[test]
fn renaming_and_removing_the_base_follow_through() {
    let doc = rig();
    let renamed = edit(&doc, r#"{"op":"set_slot","name":"base","rename":"body"}"#).unwrap();
    assert_eq!(renamed.slot("shade").unwrap().clip.as_deref(), Some("body"));
    let removed = edit(&doc, r#"{"op":"remove_slot","name":"base"}"#).unwrap();
    assert_eq!(removed.slot("shade").unwrap().clip, None);
    assert!(validate(&removed).is_empty());
}

#[test]
fn bad_clips_are_refused() {
    let doc = rig();
    for (ops, why) in [
        (r#"{"op":"set_slot","name":"free","clip":"free"}"#, "itself"),
        (r#"{"op":"set_slot","name":"free","clip":"nope"}"#, "does not exist"),
        (r#"{"op":"set_slot","name":"free","clip":"shade"}"#, "clipped itself"),
        (r#"{"op":"set_slot","name":"base","clip":"free"}"#, "clip base"),
        (r#"{"op":"add_slot","name":"x","bone":"root","clip":"nope"}"#, "does not exist"),
    ] {
        let err = edit(&doc, ops).unwrap_err();
        assert!(err.contains(why), "{ops}: {err}");
    }
}
