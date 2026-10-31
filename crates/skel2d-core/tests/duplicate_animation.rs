use skel2d_core::{Document, apply, from_str, parse_ops, to_string};

fn document() -> Document {
    apply(&Document::new("copy"), &parse_ops(r##"[
        {"op":"add_bone","name":"arm","length":10},
        {"op":"add_bone","name":"goal","y":10},
        {"op":"add_slot","name":"slot","bone":"arm"},
        {"op":"add_ik","name":"aim","bones":["arm"],"target":"goal"},
        {"op":"add_animation","name":"source","duration":2},
        {"op":"key_bone","animation":"source","bone":"arm","time":0,"rotate":20,"translate":[1,2],"scale":[2,3],"curve":"stepped"},
        {"op":"key_slot","animation":"source","slot":"slot","time":0,"color":"#ff0000"},
        {"op":"key_ik","animation":"source","ik":"aim","time":0,"mix":0.5}
    ]"##).unwrap()).unwrap()
}

#[test]
fn copy_preserves_every_channel_curve_and_duration_and_is_independent() {
    let original = document();
    let copied = apply(&original, &parse_ops(r#"{"op":"duplicate_animation","name":"source","new_name":"copy"}"#).unwrap()).unwrap();
    assert_eq!(copied.animations["source"], copied.animations["copy"]);
    assert_eq!(from_str(&to_string(&copied)).unwrap(), copied);
    let changed = apply(&copied, &parse_ops(r#"{"op":"key_bone","animation":"copy","bone":"arm","time":0,"rotate":90}"#).unwrap()).unwrap();
    assert_eq!(changed.animations["source"], original.animations["source"]);
    assert_ne!(changed.animations["source"], changed.animations["copy"]);
}

#[test]
fn missing_source_duplicate_destination_and_empty_names_fail_atomically() {
    let original = document();
    for (source, new) in [("missing", "copy"), ("source", "source"), ("source", "")] {
        let ops = format!(r#"[{{"op":"set_name","name":"edited"}},{{"op":"duplicate_animation","name":"{source}","new_name":"{new}"}}]"#);
        assert!(apply(&original, &parse_ops(&ops).unwrap()).is_err());
        assert_eq!(original.name, "copy");
        assert_eq!(original.animations.len(), 1);
    }
}
