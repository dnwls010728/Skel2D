use serde_json::{Value, json};
use skel2d_core::pose::Track;
use skel2d_core::{Document, MovePath, PhysicsMode, PhysicsRun, PhysicsState, Pose, apply, from_str, parse_ops, to_string, validate};

fn doc(physics: Value) -> Document {
    from_str(&json!({
        "bones":[{"name":"root"},{"name":"hair","parent":"root","length":10,"x":5},{"name":"tip","parent":"hair","x":10}],
        "physics":[physics],
        "animations":{"sway":{"duration":2,"bones":{"root":{"translate":[
            {"time":0,"value":[0,0]},{"time":0.5,"value":[40,0]},{"time":1,"value":[40,0]}
        ]}}}}
    }).to_string()).unwrap()
}

fn edit(d: &Document, ops: Value) -> Document {
    apply(d, &parse_ops(&ops.to_string()).unwrap()).unwrap()
}

fn tracks<'a>(d: &'a Document) -> impl Fn(f32) -> Vec<Track<'a>> {
    move |time| vec![Track { animation: &d.animations["sway"], time, alpha: 1.0 }]
}

fn hair_physics() -> Value {
    json!({"name":"hair_p","bone":"hair","x":1,"y":1,"rotate":1,"strength":50,"damping":0.9})
}

#[test]
fn omitted_values_settle_after_movement() {
    let d = doc(json!({"name":"p","bone":"hair","x":1}));
    let (pose, state) = Pose::simulate(&d, tracks(&d), &[], 2.0, 60.0);
    assert!((pose.bones[1].world.tx - 45.0).abs() < 0.01, "{}", pose.bones[1].world.tx);
    assert!(state.bodies[0].vx.abs() < 0.1, "{}", state.bodies[0].vx);
}

#[test]
fn defaults_stay_out_of_files() {
    let d = doc(json!({"name":"p","bone":"hair"}));
    let c = &d.physics[0];
    assert_eq!((c.limit, c.inertia, c.strength, c.damping, c.mass_inverse, c.wind, c.gravity, c.mix), (5000.0, 0.5, 100.0, 0.85, 1.0, 0.0, 0.0, 1.0));
    assert_eq!((c.x, c.y, c.rotate, c.scale_x, c.shear_x), (0.0, 0.0, 0.0, 0.0, 0.0));
    assert!((c.step - 1.0 / 60.0).abs() < 1e-7);
    let text = to_string(&d);
    assert!(!text.contains("limit") && !text.contains("physics_globals"), "{text}");
    assert_eq!(from_str(&text).unwrap(), d);
}

#[test]
fn compute_skinned_ignores_physics() {
    let d = doc(hair_physics());
    let mut plain = d.clone();
    plain.physics.clear();
    let (with, without) = (Pose::compute_skinned(&d, &tracks(&d)(0.25), &[]), Pose::compute_skinned(&plain, &tracks(&plain)(0.25), &[]));
    assert_eq!(with.bones, without.bones);
    assert!(with.physics.is_empty());
}

#[test]
fn simulation_is_deterministic_and_frames_continue_the_same_run() {
    let d = doc(hair_physics());
    let (a, state) = Pose::simulate(&d, tracks(&d), &[], 0.75, 60.0);
    let (b, _) = Pose::simulate(&d, tracks(&d), &[], 0.75, 60.0);
    assert_eq!(a, b);
    assert!(state.bodies[0].x.abs() > 0.1, "{:?}", state.bodies[0]);
    let mut run = PhysicsRun::new(60.0, MovePath::default());
    for t in [0.1, 0.33, 0.5, 0.74] {
        let continued = run.advance(&d, &tracks(&d), &[], t);
        assert_eq!(continued, Pose::simulate(&d, tracks(&d), &[], t, 60.0).0, "{t}");
    }
    assert_eq!(run.advance(&d, &tracks(&d), &[], 0.75), a);
    // Going back restarts rather than continuing from a later state.
    assert_eq!(run.advance(&d, &tracks(&d), &[], 0.1), Pose::simulate(&d, tracks(&d), &[], 0.1, 60.0).0);
}

#[test]
fn bodies_lag_and_children_follow() {
    let d = doc(json!({"name":"p","bone":"hair","x":1,"strength":50,"damping":0.9}));
    let plain = Pose::compute_skinned(&d, &tracks(&d)(0.25), &[]);
    let (sim, _) = Pose::simulate(&d, tracks(&d), &[], 0.25, 60.0);
    let (hair, tip) = (sim.bones[1].world.tx - plain.bones[1].world.tx, sim.bones[2].world.tx - plain.bones[2].world.tx);
    assert!(hair < -0.5, "root moves +x, the body stays behind: {hair}");
    assert!((tip - hair).abs() < 1e-3, "{tip} {hair}");
    assert_eq!(sim.bones[0], plain.bones[0]);
}

#[test]
fn strength_zero_full_damping_stays_at_rest_and_settles_back() {
    // No movement, no forces: nothing happens however long it runs.
    let mut d = doc(json!({"name":"p","bone":"hair","x":1,"y":1,"rotate":1,"strength":0,"damping":1}));
    d.animations.get_mut("sway").unwrap().bones.clear();
    let (pose, state) = Pose::simulate(&d, tracks(&d), &[], 3.0, 60.0);
    assert_eq!(pose.bones, Pose::compute(&d, None).bones);
    assert_eq!((state.bodies[0].x, state.bodies[0].vx), (0.0, 0.0));
    // A damped spring returns to the animated pose once it stops moving.
    let d = doc(json!({"name":"p","bone":"hair","x":1,"strength":100,"damping":0.8}));
    let (pose, _) = Pose::simulate(&d, tracks(&d), &[], 2.0, 60.0);
    assert!((pose.bones[1].world.tx - 45.0).abs() < 0.01, "{}", pose.bones[1].world.tx);
}

#[test]
fn gravity_sags_down_and_wind_pushes_along_x() {
    let still = |c: Value| { let mut d = doc(c); d.animations.get_mut("sway").unwrap().bones.clear(); d };
    let d = still(json!({"name":"p","bone":"hair","y":1,"gravity":200,"strength":100,"damping":0.8}));
    let (pose, _) = Pose::simulate(&d, tracks(&d), &[], 3.0, 60.0);
    assert!((pose.bones[1].world.ty + 2.0).abs() < 0.01, "gravity/strength below: {}", pose.bones[1].world.ty);
    let d = still(json!({"name":"p","bone":"hair","x":1,"wind":200,"strength":100,"damping":0.8}));
    assert!(Pose::simulate(&d, tracks(&d), &[], 3.0, 60.0).0.bones[1].world.tx > 6.9);
    // Gravity on a horizontal bone turns its tip downward.
    let d = still(json!({"name":"p","bone":"hair","rotate":1,"gravity":200,"strength":100,"damping":0.8}));
    assert!(Pose::simulate(&d, tracks(&d), &[], 3.0, 60.0).0.bones[1].world.rotation_deg() < -1.0);
}

#[test]
fn limit_bounds_speed_and_extreme_settings_stay_finite() {
    let d = doc(json!({"name":"p","bone":"hair","x":1,"limit":10,"strength":100,"damping":1}));
    let mut run = PhysicsRun::new(60.0, MovePath::default());
    for f in 0..60 {
        run.advance(&d, &tracks(&d), &[], f as f32 / 60.0);
        assert!(run.state.bodies[0].vx.abs() <= 10.0, "{:?}", run.state.bodies[0]);
    }
    let d = doc(json!({"name":"p","bone":"hair","x":1,"y":1,"rotate":1,"scale_x":1,"strength":1e7,"damping":1,"mass_inverse":1000}));
    let (pose, _) = Pose::simulate(&d, tracks(&d), &[], 2.0, 60.0);
    assert!(pose.bones.iter().all(|b| b.world.tx.is_finite() && b.world.a.is_finite()));
}

#[test]
fn reset_key_returns_to_the_animated_pose() {
    let d = edit(&doc(hair_physics()), json!({"op":"key_physics_reset","animation":"sway","time":0.5}));
    assert_eq!(d.animations["sway"].physics[""].reset, [0.5]);
    let at = 30.0 / 60.0;
    let (pose, state) = Pose::simulate(&d, tracks(&d), &[], at, 60.0);
    assert_eq!(state.bodies[0].x, 0.0);
    let plain = Pose::compute_skinned(&d, &tracks(&d)(at), &[]);
    assert!((pose.bones[1].world.tx - plain.bones[1].world.tx).abs() < 1e-4);
    // One frame earlier the body was still lagging.
    assert!(Pose::simulate(&d, tracks(&d), &[], 29.0 / 60.0, 60.0).1.bodies[0].x.abs() > 0.1);
    // Passing the key while looping back counts too.
    let mut state = PhysicsState::default();
    let run = |state: &mut PhysicsState, t: f32| { Pose::compute_physics(&d, &tracks(&d)(t), &[], state, PhysicsMode::Update, 0.1); };
    run(&mut state, 1.8);
    state.impulse("hair_p", 100.0, 0.0);
    run(&mut state, 1.9);
    assert_ne!(state.bodies[0].x, 0.0);
    run(&mut state, 0.6);
    assert_eq!(state.bodies[0].x, 0.0);
}

#[test]
fn pose_mode_applies_without_advancing_and_none_leaves_state_alone() {
    let d = doc(hair_physics());
    let (_, mut state) = Pose::simulate(&d, tracks(&d), &[], 0.3, 60.0);
    let before = state.clone();
    let a = Pose::compute_physics(&d, &tracks(&d)(0.3), &[], &mut state, PhysicsMode::Pose, 5.0);
    assert_eq!(state.bodies, before.bodies);
    assert_ne!(a.bones[1].world.tx, Pose::compute_skinned(&d, &tracks(&d)(0.3), &[]).bones[1].world.tx);
    Pose::compute_physics(&d, &tracks(&d)(0.3), &[], &mut state, PhysicsMode::None, 5.0);
    assert_eq!(state, before);
    let reset = Pose::compute_physics(&d, &tracks(&d)(0.3), &[], &mut state, PhysicsMode::Reset, 0.0);
    assert_eq!(reset.bones[1].world.tx, Pose::compute_skinned(&d, &tracks(&d)(0.3), &[]).bones[1].world.tx);
}

#[test]
fn skeleton_movement_drives_bodies_without_animation() {
    let mut d = doc(json!({"name":"p","bone":"hair","x":1,"strength":50,"damping":0.9}));
    d.animations.get_mut("sway").unwrap().bones.clear();
    let mut run = PhysicsRun::new(60.0, MovePath::parse("0,0@0;30,0@0.5").unwrap());
    let moved = run.advance(&d, &tracks(&d), &[], 0.25);
    assert!(moved.bones[1].world.tx < 5.0 - 0.5, "{}", moved.bones[1].world.tx);
    assert!(MovePath::parse("oops").is_err() && MovePath::parse("0,0@1;1,1@0").is_err());
    let mut state = PhysicsState::default();
    Pose::compute_physics(&d, &tracks(&d)(0.0), &[], &mut state, PhysicsMode::Reset, 0.0);
    state.rotate([0.0, 0.0], 90.0);
    state.translate(1.0, 0.0);
    let after = Pose::compute_physics(&d, &tracks(&d)(0.0), &[], &mut state, PhysicsMode::Update, 1.0 / 60.0);
    assert_ne!(after.bones[1].world.tx, 5.0);
}

#[test]
fn skin_owned_physics_applies_only_with_its_skin_and_restarts() {
    let d = edit(&doc(json!({"name":"p","bone":"hair","x":1,"strength":50,"damping":0.9})), json!([
        {"op":"add_skin","name":"long"},{"op":"set_skin","name":"long","constraints":["p"]}
    ]));
    let plain = Pose::compute_skinned(&d, &tracks(&d)(0.25), &[]).bones[1].world.tx;
    assert_eq!(Pose::simulate(&d, tracks(&d), &[], 0.25, 60.0).0.bones[1].world.tx, plain);
    let skins = vec!["long".to_string()];
    assert_ne!(Pose::simulate(&d, tracks(&d), &skins, 0.25, 60.0).0.bones[1].world.tx, plain);
}

#[test]
fn globals_and_global_timelines_apply_only_to_flagged_properties() {
    let base = |flag: bool| {
        let d = doc(json!({"name":"p","bone":"hair","y":1,"strength":100,"damping":0.8,"gravity":0,"gravity_global":flag}));
        let mut d = edit(&d, json!({"op":"set_physics_globals","gravity":200}));
        d.animations.get_mut("sway").unwrap().bones.clear();
        d
    };
    let sag = |d: &Document| Pose::simulate(d, tracks(d), &[], 3.0, 60.0).0.bones[1].world.ty;
    assert!((sag(&base(true)) + 2.0).abs() < 0.01);
    assert!(sag(&base(false)).abs() < 0.01);
    let keyed = |d: &Document| edit(d, json!({"op":"key_physics","animation":"sway","constraint":"","property":"gravity","time":0,"value":400}));
    assert!((sag(&keyed(&base(true))) + 4.0).abs() < 0.01);
    assert!(sag(&keyed(&base(false))).abs() < 0.01);
    let named = edit(&base(false), json!({"op":"key_physics","animation":"sway","constraint":"p","property":"gravity","time":0,"value":100}));
    assert!((sag(&named) + 1.0).abs() < 0.01);
    let mixed = edit(&named, json!({"op":"key_physics","animation":"sway","constraint":"p","property":"mix","time":0,"value":0}));
    assert_eq!(sag(&mixed), 0.0);
}

#[test]
fn ops_keep_references_and_constraint_order_runs_physics_with_ik() {
    let d = edit(&doc(hair_physics()), json!([
        {"op":"add_skin","name":"k"},{"op":"set_skin","name":"k","constraints":["hair_p"]},
        {"op":"key_physics","animation":"sway","constraint":"hair_p","property":"wind","time":1,"value":5,"curve":"stepped"},
        {"op":"key_physics_reset","animation":"sway","constraint":"hair_p","time":1},
        {"op":"set_physics","name":"hair_p","rename":"h","wind":3,"order":2,"wind_global":true}
    ]));
    assert_eq!(d.physics[0].name, "h");
    assert_eq!((d.physics[0].wind, d.physics[0].order, d.physics[0].wind_global), (3.0, 2, true));
    assert_eq!(d.skin("k").unwrap().constraints, ["h"]);
    assert_eq!(d.animations["sway"].physics["h"].wind[0].value, 5.0);
    assert!(validate(&d).is_empty());
    let renamed = edit(&d, json!({"op":"set_bone","name":"hair","rename":"lock"}));
    assert_eq!(renamed.physics[0].bone, "lock");
    let removed = edit(&d, json!({"op":"remove_physics_key","animation":"sway","constraint":"h","property":"wind","time":1}));
    assert!(removed.animations["sway"].physics["h"].wind.is_empty());
    let removed = edit(&removed, json!({"op":"remove_physics_key","animation":"sway","constraint":"h","time":1}));
    assert!(!removed.animations["sway"].physics.contains_key("h"));
    let gone = edit(&d, json!({"op":"remove_physics","name":"h"}));
    assert!(gone.physics.is_empty() && gone.skin("k").unwrap().constraints.is_empty() && gone.animations["sway"].physics.is_empty());
    let cut = edit(&d, json!({"op":"remove_bone","name":"hair","recursive":true}));
    assert!(cut.physics.is_empty() && cut.skin("k").unwrap().constraints.is_empty() && cut.animations["sway"].physics.is_empty());
    let with_ik = edit(&d, json!([{"op":"add_bone","name":"goal"},{"op":"add_ik","name":"reach","bones":["tip"],"target":"goal"},{"op":"set_constraint_order","names":["h","reach"]}]));
    assert_eq!((with_ik.physics[0].order, with_ik.ik[0].order), (0, 1));
}

#[test]
fn invalid_input_is_rejected_atomically() {
    let d = doc(hair_physics());
    for op in [
        json!({"op":"add_physics","name":"hair_p","bone":"hair"}),
        json!({"op":"add_physics","name":"","bone":"hair"}),
        json!({"op":"add_physics","name":"n","bone":"missing"}),
        json!({"op":"add_ik","name":"hair_p","bones":["tip"],"target":"root"}),
        json!({"op":"set_physics","name":"missing","wind":1}),
        json!({"op":"set_physics","name":"hair_p","mix":2}),
        json!({"op":"set_physics","name":"hair_p","damping":-1}),
        json!({"op":"set_physics","name":"hair_p","step":0}),
        json!({"op":"set_physics","name":"hair_p","limit":0}),
        json!({"op":"set_physics","name":"hair_p","mass_inverse":0}),
        json!({"op":"set_physics","name":"hair_p","bone":"missing"}),
        json!({"op":"set_physics_globals","mix":3}),
        json!({"op":"key_physics","animation":"sway","constraint":"missing","property":"wind","time":0,"value":1}),
        json!({"op":"key_physics","animation":"sway","constraint":"hair_p","property":"wind","time":-1,"value":1}),
        json!({"op":"key_physics_reset","animation":"sway","constraint":"missing","time":0}),
        json!({"op":"remove_physics_key","animation":"sway","constraint":"hair_p","time":0}),
        json!({"op":"set_constraint_order","names":[]}),
        json!({"op":"remove_physics","name":"missing"}),
    ] {
        let ops = parse_ops(&json!([{"op":"set_name","name":"rollback"}, op]).to_string()).unwrap();
        assert!(apply(&d, &ops).is_err(), "{ops:?}");
    }
    assert_eq!(d.name, "");
    let mut bad = serde_json::to_value(&d).unwrap();
    bad["animations"]["sway"]["physics"] = json!({"missing":{"reset":[1,0]}});
    assert!(from_str(&bad.to_string()).is_err());
}
