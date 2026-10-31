//! Draw order keys, events and animated IK values.

use skel2d_core::pose::{Track, fired_events};
use skel2d_core::{Document, Pose, apply, from_str, parse_ops, to_string, validate};

fn doc(ops: &str) -> Document {
    apply(&Document::with_root("t"), &parse_ops(ops).unwrap()).unwrap()
}

const SLOTS: &str = r#"{"op":"add_slot","name":"a","bone":"root"},{"op":"add_slot","name":"b","bone":"root"},{"op":"add_slot","name":"c","bone":"root"}"#;

fn order_at(d: &Document, time: f32) -> Vec<usize> {
    Pose::compute(d, Some((&d.animations["m"], time))).draw_order
}

#[test]
fn draw_order_keys_hold_until_the_next_key_and_reset_to_setup() {
    let d = doc(&format!(r#"[{SLOTS},
        {{"op":"key_draw_order","animation":"m","time":1,"order":["c","a","b"]}},
        {{"op":"key_draw_order","animation":"m","time":2}}]"#));
    assert_eq!(Pose::compute(&d, None).draw_order, [0, 1, 2]);
    assert_eq!(order_at(&d, 0.5), [0, 1, 2]);
    assert_eq!(order_at(&d, 1.0), [2, 0, 1]);
    assert_eq!(order_at(&d, 1.9), [2, 0, 1]);
    assert_eq!(order_at(&d, 2.0), [0, 1, 2]);
    assert_eq!(d.animations["m"].duration(), 2.0);
    let report = Pose::compute(&d, Some((&d.animations["m"], 1.0))).report(&d);
    assert_eq!(report.draw_order, ["c", "a", "b"]);
    assert!(Pose::compute(&d, None).report(&d).draw_order.is_empty());
    assert_eq!(from_str(&to_string(&d)).unwrap(), d);
}

#[test]
fn draw_order_follows_slot_edits() {
    let key = format!(r#"[{SLOTS},{{"op":"key_draw_order","animation":"m","time":0,"order":["c","a","b"]}}]"#);
    let renamed = apply(&doc(&key), &parse_ops(r#"{"op":"set_slot","name":"a","rename":"z"}"#).unwrap()).unwrap();
    assert_eq!(renamed.animations["m"].draw_order[0].order.as_ref().unwrap(), &["c", "z", "b"]);
    let removed = apply(&doc(&key), &parse_ops(r#"{"op":"remove_slot","name":"a"}"#).unwrap()).unwrap();
    assert_eq!(removed.animations["m"].draw_order[0].order.as_ref().unwrap(), &["c", "b"]);
    assert_eq!(order_at(&removed, 0.0), [1, 0]);
    // A slot added after the key sits above its setup neighbour.
    let added = apply(&doc(&key), &parse_ops(r#"{"op":"add_slot","name":"d","bone":"root","index":1}"#).unwrap()).unwrap();
    let names: Vec<_> = order_at(&added, 0.0).iter().map(|&i| added.slots[i].name.clone()).collect();
    assert_eq!(names, ["c", "a", "d", "b"]);
}

#[test]
fn draw_order_rejects_bad_lists() {
    let base = doc(&format!("[{SLOTS}]"));
    for order in [r#"["a","b"]"#, r#"["a","b","c","nope"]"#, r#"["a","b","c","a"]"#] {
        let op = format!(r#"{{"op":"key_draw_order","animation":"m","time":0,"order":{order}}}"#);
        assert!(apply(&base, &parse_ops(&op).unwrap()).is_err(), "{order}");
    }
    let bad = r#"{"slots":[],"animations":{"m":{"draw_order":[{"time":1},{"time":1}]}}}"#;
    assert!(from_str(bad).is_err());
}

#[test]
fn draw_order_upper_track_wins_only_when_mostly_visible() {
    let d = doc(&format!(r#"[{SLOTS},{{"op":"key_draw_order","animation":"m","time":0,"order":["c","b","a"]}},{{"op":"add_animation","name":"plain"}}]"#));
    let tracks = |alpha| [Track { animation: &d.animations["plain"], time: 0.0, alpha: 1.0 }, Track { animation: &d.animations["m"], time: 0.0, alpha }];
    assert_eq!(Pose::compute_with(&d, &tracks(0.4)).draw_order, [0, 1, 2]);
    assert_eq!(Pose::compute_with(&d, &tracks(0.5)).draw_order, [2, 1, 0]);
}

#[test]
fn events_fire_with_defaults_and_overrides() {
    let d = doc(r#"[{"op":"add_event","name":"step","int":3,"float":0.5,"string":"grass"},
        {"op":"add_event","name":"bare"},
        {"op":"key_event","animation":"m","name":"step","time":0},
        {"op":"key_event","animation":"m","name":"bare","time":0.5},
        {"op":"key_event","animation":"m","name":"step","time":1,"int":7,"string":"rock"}]"#);
    let a = &d.animations["m"];
    assert_eq!(a.duration(), 1.0);
    let times = |from, to| fired_events(&d, a, from, to).into_iter().map(|e| e.time).collect::<Vec<_>>();
    assert_eq!(times(None, 0.0), [0.0]);
    assert_eq!(times(Some(0.0), 0.5), [0.5]);
    assert_eq!(times(Some(0.5), 0.5), [] as [f32; 0]);
    assert_eq!(times(Some(0.75), 0.25), [1.0, 0.0]); // looped
    let last = &fired_events(&d, a, Some(0.5), 1.0)[0];
    assert_eq!((last.int, last.float, last.string.as_str()), (7, 0.5, "rock"));
    let first = &fired_events(&d, a, None, 0.0)[0];
    assert_eq!((first.int, first.string.as_str()), (3, "grass"));
    assert_eq!(from_str(&to_string(&d)).unwrap(), d);
}

#[test]
fn events_rename_replace_and_remove() {
    let d = doc(r#"[{"op":"add_event","name":"step"},{"op":"add_event","name":"other"},
        {"op":"key_event","animation":"m","name":"step","time":1,"int":1},
        {"op":"key_event","animation":"m","name":"other","time":1},
        {"op":"key_event","animation":"m","name":"step","time":1,"int":2}]"#);
    assert_eq!(d.animations["m"].events.len(), 2);
    let renamed = apply(&d, &parse_ops(r#"{"op":"set_event","name":"step","rename":"foot","float":2}"#).unwrap()).unwrap();
    assert!(renamed.animations["m"].events.iter().any(|k| k.name == "foot" && k.int == Some(2)));
    assert_eq!(renamed.events["foot"].float, 2.0);
    assert!(apply(&d, &parse_ops(r#"{"op":"set_event","name":"step","rename":"other"}"#).unwrap()).is_err());
    let removed = apply(&d, &parse_ops(r#"{"op":"remove_event","name":"step"}"#).unwrap()).unwrap();
    assert_eq!(removed.animations["m"].events.len(), 1);
    let unkeyed = apply(&d, &parse_ops(r#"{"op":"remove_event_key","animation":"m","name":"step","time":1}"#).unwrap()).unwrap();
    assert_eq!(unkeyed.animations["m"].events[0].name, "other");
    assert!(apply(&d, &parse_ops(r#"{"op":"key_event","animation":"m","name":"nope","time":0}"#).unwrap()).is_err());
    assert!(from_str(r#"{"animations":{"m":{"events":[{"time":0,"name":"nope"}]}}}"#).is_err());
}

const LEG: &str = r#"{"op":"add_bone","name":"a","length":50},{"op":"add_bone","name":"b","parent":"a","x":50,"length":50},{"op":"add_bone","name":"goal","x":100},{"op":"add_ik","name":"c","bones":["a","b"],"target":"goal"}"#;

fn tip_x(d: &Document, time: f32) -> f32 {
    let i = d.bone_index("b").unwrap();
    Pose::compute(d, Some((&d.animations["m"], time))).bones[i].world.apply(50.0, 0.0)[0]
}

#[test]
fn old_ik_keys_are_plain_mix_numbers_and_stay_that_way() {
    let d = doc(&format!(r#"[{LEG},{{"op":"key_ik","animation":"m","ik":"c","time":0,"mix":0.25}}]"#));
    let text = to_string(&d);
    assert!(text.contains(r#""value":0.25"#) || text.contains(r#""value": 0.25"#), "{text}");
    assert_eq!(from_str(&text).unwrap(), d);
    assert!(apply(&d, &parse_ops(r#"{"op":"key_ik","animation":"m","ik":"c","time":1}"#).unwrap()).is_err());
}

#[test]
fn ik_keys_animate_softness_and_switches() {
    let d = doc(&format!(r#"[{LEG},
        {{"op":"key_ik","animation":"m","ik":"c","time":0,"softness":0}},
        {{"op":"key_ik","animation":"m","ik":"c","time":1,"softness":10,"stretch":true}},
        {{"op":"key_ik","animation":"m","ik":"c","time":2,"softness":10,"stretch":false}}]"#));
    assert!(tip_x(&d, 0.0) > 99.0);
    // Halfway the softness is 5: 100 - 5^2 / 20 = 98.75.
    assert!((tip_x(&d, 0.5) - 98.75).abs() < 0.01, "{}", tip_x(&d, 0.5));
    // A switch holds from its key; the stretch reaches the target through softness.
    assert!((tip_x(&d, 1.0) - 100.0).abs() < 0.05, "{}", tip_x(&d, 1.0));
    assert!((tip_x(&d, 1.99) - 100.0).abs() < 0.05);
    assert!((tip_x(&d, 2.0) - 97.5).abs() < 0.01);
    // Merging: a second key_ik at the same time keeps the other values.
    let merged = apply(&d, &parse_ops(r#"{"op":"key_ik","animation":"m","ik":"c","time":1,"mix":0.5}"#).unwrap()).unwrap();
    let v = merged.animations["m"].ik["c"][1].value;
    assert_eq!((v.mix, v.softness, v.stretch), (Some(0.5), Some(10.0), Some(true)));
    assert_eq!(from_str(&to_string(&merged)).unwrap(), merged);
}

#[test]
fn ik_keys_bend_direction_flips_the_joint() {
    let d = doc(r#"[{"op":"add_bone","name":"a","rotation":-90,"length":50},{"op":"add_bone","name":"b","parent":"a","x":50,"length":50},
        {"op":"add_bone","name":"goal","x":30,"y":-60},{"op":"add_ik","name":"c","bones":["a","b"],"target":"goal"},
        {"op":"key_ik","animation":"m","ik":"c","time":1,"bend_positive":false,"curve":"stepped"}]"#);
    let side = |pose: Pose| { let e = pose.bones[d.bone_index("b").unwrap()].world.apply(0.0, 0.0); 30.0 * e[1] + 60.0 * e[0] };
    assert!(side(Pose::compute(&d, None)) * side(Pose::compute(&d, Some((&d.animations["m"], 1.0)))) < 0.0);
}

#[test]
fn ik_values_are_checked() {
    let base = doc(&format!("[{LEG}]"));
    let bad = [r#"{"op":"set_ik","name":"c","softness":-1}"#, r#"{"op":"key_ik","animation":"m","ik":"c","time":0,"softness":-1}"#, r#"{"op":"key_ik","animation":"m","ik":"c","time":0,"mix":1,"curves":["linear"]}"#];
    for op in bad { assert!(apply(&base, &parse_ops(op).unwrap()).is_err(), "{op}"); }
    let good = apply(&base, &parse_ops(r#"{"op":"set_ik","name":"c","softness":4,"compress":true,"stretch":true}"#).unwrap()).unwrap();
    assert!(validate(&good).is_empty());
    let c = &good.ik[0];
    assert_eq!((c.softness, c.compress, c.stretch), (4.0, true, true));
    assert_eq!(from_str(&to_string(&good)).unwrap(), good);
}
