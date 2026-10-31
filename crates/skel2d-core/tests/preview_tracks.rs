use skel2d_core::{Document, LayerTrack, Pose, apply, parse_layer_tracks, parse_ops};
use skel2d_core::pose::preview_tracks;

fn document() -> Document {
    apply(&Document::new("tracks"), &parse_ops(r#"[
        {"op":"add_bone","name":"arm","length":10},
        {"op":"add_animation","name":"main","duration":2},
        {"op":"key_bone","animation":"main","bone":"arm","time":0,"rotate":-10},
        {"op":"add_animation","name":"z_low","duration":1},
        {"op":"key_bone","animation":"z_low","bone":"arm","time":0,"rotate":20},
        {"op":"add_animation","name":"a_high","duration":0},
        {"op":"key_bone","animation":"a_high","bone":"arm","time":0,"rotate":80}
    ]"#).unwrap()).unwrap()
}

fn rotation(doc: &Document, layers: &[LayerTrack]) -> f32 {
    Pose::compute_with(doc, &preview_tracks(doc, Some("main"), 2.5, layers)).bones[0].local.rotation
}

#[test]
fn evaluation_preserves_input_order_opacity_and_enabled_state() {
    let doc = document();
    let mut layers = parse_layer_tracks(&doc, r#"[{"anim":"z_low"},{"anim":"a_high","alpha":0.5}]"#).unwrap();
    assert_eq!(rotation(&doc, &layers), 50.0);
    layers.swap(0, 1);
    assert_eq!(rotation(&doc, &layers), 20.0);
    layers[1].enabled = false;
    assert_eq!(rotation(&doc, &layers), 35.0); // halfway from main -10 to 80
    layers[0].alpha = 0.0;
    assert_eq!(rotation(&doc, &layers), -10.0);
}

#[test]
fn independent_clocks_main_deduplication_and_static_poses() {
    let doc = document();
    let layers = vec![LayerTrack::new("z_low"), LayerTrack::new("main"), LayerTrack::new("a_high")];
    let tracks = preview_tracks(&doc, Some("main"), 2.5, &layers);
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].time, 2.5); // main follows the playhead, not a layer loop
    assert_eq!(tracks[1].time, 0.5);
    assert_eq!(tracks[2].time, 0.0);
    assert_eq!(preview_tracks(&doc, None, 2.5, &layers).len(), 3);
}

#[test]
fn defaults_round_trip_and_invalid_runtime_inputs_are_rejected() {
    let doc = document();
    let layers = parse_layer_tracks(&doc, r#"[{"anim":"z_low"}]"#).unwrap();
    assert_eq!(layers, [LayerTrack::new("z_low")]);
    assert_eq!(serde_json::to_string(&layers).unwrap(), r#"[{"anim":"z_low"}]"#);
    for json in [r#"[{"anim":"missing","enabled":false}]"#, r#"[{"anim":"z_low"},{"anim":"z_low","enabled":false}]"#,
        r#"[{"anim":"z_low","alpha":-0.1}]"#, r#"[{"anim":"z_low","alpha":1.1}]"#, r#"[{"anim":"z_low","extra":0}]"#,
        r#"[{"anim":"z_low","enabled":"no"}]"#, "null"] {
        assert!(parse_layer_tracks(&doc, json).is_err(), "{json}");
    }
    for alpha in [f32::NAN, f32::INFINITY] {
        let layers = [LayerTrack { anim: "z_low".into(), alpha, enabled: true }];
        assert!(!skel2d_core::validate::validate_layer_tracks(&doc, &layers).is_empty());
    }
}

#[test]
fn reset_on_upper_track_keeps_the_lower_attachment() {
    let doc = apply(&document(), &parse_ops(r#"[
        {"op":"add_slot","name":"face","bone":"arm","attachments":[{"type":"region","name":"smile","width":10,"height":10}]},
        {"op":"key_slot","animation":"z_low","slot":"face","time":0,"attachment":"smile"},
        {"op":"key_slot","animation":"a_high","slot":"face","time":0,"reset":true}
    ]"#).unwrap()).unwrap();
    let layers = [LayerTrack::new("z_low"), LayerTrack::new("a_high")];
    let pose = Pose::compute_with(&doc, &preview_tracks(&doc, None, 0.0, &layers));
    assert_eq!(pose.slots[0].attachment.as_deref(), Some("smile"));
}
