//! Inspector of a physics constraint. In the setup pose values edit the rig;
//! in animate mode the keyable ones key at the playhead instead and the rest
//! is disabled, so nothing changes the setup by accident.

use super::*;
use skel2d_core::ops::{PhysicsGlobalsPatch, PhysicsPatch, PhysicsProperty};
use skel2d_core::{Key, PhysicsConstraint, PhysicsGlobals, PhysicsTimelines};

const PROPS: [PhysicsProperty; 7] = [
    PhysicsProperty::Inertia, PhysicsProperty::Strength, PhysicsProperty::Damping, PhysicsProperty::Mass,
    PhysicsProperty::Wind, PhysicsProperty::Gravity, PhysicsProperty::Mix,
];

/// Value bundles for common cases. The editor writes them as one `set_physics`.
struct Preset {
    label: &'static str,
    /// x, y, rotate, scale_x, shear_x
    influence: [f32; 5],
    inertia: f32,
    strength: f32,
    damping: f32,
    mass: f32,
    wind: f32,
    gravity: f32,
}

const PRESETS: [Preset; 5] = [
    Preset { label: "Hair", influence: [0.0, 0.0, 1.0, 0.0, 0.0], inertia: 0.6, strength: 40.0, damping: 0.9, mass: 1.0, wind: 0.0, gravity: 20.0 },
    Preset { label: "Cloth", influence: [0.0, 0.0, 1.0, 0.0, 0.3], inertia: 0.5, strength: 25.0, damping: 0.85, mass: 1.5, wind: 0.0, gravity: 30.0 },
    Preset { label: "Tail", influence: [0.0, 0.0, 1.0, 0.0, 0.0], inertia: 0.9, strength: 60.0, damping: 0.92, mass: 1.2, wind: 0.0, gravity: 10.0 },
    Preset { label: "Chest", influence: [0.3, 1.0, 0.0, 0.0, 0.0], inertia: 0.4, strength: 180.0, damping: 0.82, mass: 0.8, wind: 0.0, gravity: 0.0 },
    Preset { label: "Accessory", influence: [0.0, 0.0, 1.0, 0.0, 0.0], inertia: 0.8, strength: 120.0, damping: 0.88, mass: 0.5, wind: 0.0, gravity: 10.0 },
];

/// The patch a preset writes. Every property is set to its own value (not the
/// shared one), so what the preset says is what shows.
pub(super) fn preset_patch(name: &str, preset: usize) -> PhysicsPatch {
    let p = &PRESETS[preset];
    PhysicsPatch {
        name: name.to_string(),
        x: Some(p.influence[0]), y: Some(p.influence[1]), rotate: Some(p.influence[2]), scale_x: Some(p.influence[3]), shear_x: Some(p.influence[4]),
        inertia: Some(p.inertia), strength: Some(p.strength), damping: Some(p.damping), mass_inverse: Some(1.0 / p.mass),
        wind: Some(p.wind), gravity: Some(p.gravity),
        inertia_global: Some(false), strength_global: Some(false), damping_global: Some(false), mass_global: Some(false),
        wind_global: Some(false), gravity_global: Some(false),
        ..Default::default()
    }
}

fn setup_value(c: &PhysicsConstraint, p: PhysicsProperty) -> f32 {
    match p {
        PhysicsProperty::Inertia => c.inertia,
        PhysicsProperty::Strength => c.strength,
        PhysicsProperty::Damping => c.damping,
        PhysicsProperty::Mass => c.mass_inverse,
        PhysicsProperty::Wind => c.wind,
        PhysicsProperty::Gravity => c.gravity,
        PhysicsProperty::Mix => c.mix,
    }
}

fn uses_global(c: &PhysicsConstraint, p: PhysicsProperty) -> bool {
    match p {
        PhysicsProperty::Inertia => c.inertia_global,
        PhysicsProperty::Strength => c.strength_global,
        PhysicsProperty::Damping => c.damping_global,
        PhysicsProperty::Mass => c.mass_global,
        PhysicsProperty::Wind => c.wind_global,
        PhysicsProperty::Gravity => c.gravity_global,
        PhysicsProperty::Mix => c.mix_global,
    }
}

fn global_value(g: &PhysicsGlobals, p: PhysicsProperty) -> f32 {
    match p {
        PhysicsProperty::Inertia => g.inertia,
        PhysicsProperty::Strength => g.strength,
        PhysicsProperty::Damping => g.damping,
        PhysicsProperty::Mass => g.mass_inverse,
        PhysicsProperty::Wind => g.wind,
        PhysicsProperty::Gravity => g.gravity,
        PhysicsProperty::Mix => g.mix,
    }
}

fn keys_of(t: &PhysicsTimelines, p: PhysicsProperty) -> &[Key<f32>] {
    match p {
        PhysicsProperty::Inertia => &t.inertia,
        PhysicsProperty::Strength => &t.strength,
        PhysicsProperty::Damping => &t.damping,
        PhysicsProperty::Mass => &t.mass,
        PhysicsProperty::Wind => &t.wind,
        PhysicsProperty::Gravity => &t.gravity,
        PhysicsProperty::Mix => &t.mix,
    }
}

fn patch_value(patch: &mut PhysicsPatch, p: PhysicsProperty, v: f32) {
    let v = Some(v);
    match p {
        PhysicsProperty::Inertia => patch.inertia = v,
        PhysicsProperty::Strength => patch.strength = v,
        PhysicsProperty::Damping => patch.damping = v,
        PhysicsProperty::Mass => patch.mass_inverse = v,
        PhysicsProperty::Wind => patch.wind = v,
        PhysicsProperty::Gravity => patch.gravity = v,
        PhysicsProperty::Mix => patch.mix = v,
    }
}

fn patch_global(patch: &mut PhysicsPatch, p: PhysicsProperty, on: bool) {
    let v = Some(on);
    match p {
        PhysicsProperty::Inertia => patch.inertia_global = v,
        PhysicsProperty::Strength => patch.strength_global = v,
        PhysicsProperty::Damping => patch.damping_global = v,
        PhysicsProperty::Mass => patch.mass_global = v,
        PhysicsProperty::Wind => patch.wind_global = v,
        PhysicsProperty::Gravity => patch.gravity_global = v,
        PhysicsProperty::Mix => patch.mix_global = v,
    }
}

fn globals_patch(p: PhysicsProperty, v: f32) -> PhysicsGlobalsPatch {
    let mut patch = PhysicsGlobalsPatch::default();
    let v = Some(v);
    match p {
        PhysicsProperty::Inertia => patch.inertia = v,
        PhysicsProperty::Strength => patch.strength = v,
        PhysicsProperty::Damping => patch.damping = v,
        PhysicsProperty::Mass => patch.mass_inverse = v,
        PhysicsProperty::Wind => patch.wind = v,
        PhysicsProperty::Gravity => patch.gravity = v,
        PhysicsProperty::Mix => patch.mix = v,
    }
    patch
}

/// Between the document's `mass_inverse` and the mass shown. The reciprocal
/// undoes itself, so one function converts both ways.
fn convert(p: PhysicsProperty, v: f32) -> f32 {
    if p == PhysicsProperty::Mass { 1.0 / v.max(1e-4) } else { v }
}

/// Label, tooltip, drag speed, range.
fn meta(p: PhysicsProperty) -> (&'static str, &'static str, f64, std::ops::RangeInclusive<f32>) {
    match p {
        PhysicsProperty::Inertia => ("Inertia", "How much the body resists the bone's own movement. 1 lags fully, 0 follows the bone", 0.01, 0.0..=1.0),
        PhysicsProperty::Strength => ("Strength", "Spring stiffness pulling back to the animated pose", 0.5, 0.0..=10000.0),
        PhysicsProperty::Damping => ("Damping", "Velocity kept per 1/60 s. 1 never loses speed; keep it below 1 so the motion settles", 0.005, 0.0..=1.0),
        PhysicsProperty::Mass => ("Mass", "Heavier bodies react less to forces (the document stores 1 / mass)", 0.02, 0.01..=100.0),
        PhysicsProperty::Wind => ("Wind", "Constant acceleration along world +x", 0.5, -10000.0..=10000.0),
        PhysicsProperty::Gravity => ("Gravity", "Constant acceleration along world -y; positive sags downward", 0.5, -10000.0..=10000.0),
        PhysicsProperty::Mix => ("Mix", "How much of the simulation shows. 0 keeps the animated pose", 0.01, 0.0..=1.0),
    }
}

fn key_state(keys: &[Key<f32>], t: f32) -> KeyState {
    if keys.iter().any(|k| (k.time - t).abs() < TIME_EPS) { KeyState::Keyed } else if keys.is_empty() { KeyState::None } else { KeyState::Interpolated }
}

fn property_name(p: PhysicsProperty) -> &'static str {
    match p {
        PhysicsProperty::Inertia => "inertia",
        PhysicsProperty::Strength => "strength",
        PhysicsProperty::Damping => "damping",
        PhysicsProperty::Mass => "mass",
        PhysicsProperty::Wind => "wind",
        PhysicsProperty::Gravity => "gravity",
        PhysicsProperty::Mix => "mix",
    }
}

/// What the document shows for `p` at `t`: the setup (or shared) value, then
/// the shared timeline for constraints using the shared value, then the
/// constraint's own keys, like the simulation reads them.
fn effective(ctx: &Ctx, p: PhysicsProperty) -> f32 {
    let global = uses_global(ctx.c, p);
    let mut v = if global { global_value(ctx.g, p) } else { setup_value(ctx.c, p) };
    if ctx.animating {
        if global && let Some(s) = sample(keys_of(&ctx.shared, p), ctx.t) { v = s; }
        if let Some(s) = sample(keys_of(&ctx.own, p), ctx.t) { v = s; }
    }
    v
}

struct Ctx<'a> {
    c: &'a PhysicsConstraint,
    g: &'a PhysicsGlobals,
    own: PhysicsTimelines,
    shared: PhysicsTimelines,
    animating: bool,
    t: f32,
    /// Properties changed but not keyed yet: (shared timeline, property).
    held: Vec<(bool, PhysicsProperty)>,
}

/// The key button state of a property: changed beats keyed.
fn property_state(ctx: &Ctx, shared: bool, p: PhysicsProperty) -> KeyState {
    if ctx.held.contains(&(shared, p)) {
        return KeyState::Changed;
    }
    // The shown timelines already hold an unkeyed change, so look at real keys only.
    key_state(keys_of(if shared { &ctx.shared } else { &ctx.own }, p), ctx.t)
}

/// What the widgets asked for this frame.
#[derive(Default)]
struct Edit {
    patch: PhysicsPatch,
    /// Which setup field `patch` changes; keeps drags of one field one undo step.
    field: Option<String>,
    preset: Option<usize>,
    /// Key button clicks: key the value now.
    keys: Vec<(PhysicsProperty, f32)>,
    shared_keys: Vec<(PhysicsProperty, f32)>,
    /// Value edits while animating: a key with Auto Key on, an unkeyed change with it off.
    changes: Vec<(PhysicsProperty, f32)>,
    shared_changes: Vec<(PhysicsProperty, f32)>,
    globals: Vec<(PhysicsProperty, f32)>,
    key_reset: bool,
    shake: bool,
}

/// One value: key button (animate), label, number, shared-value toggle.
fn property_row(ui: &mut egui::Ui, ctx: &Ctx, edit: &mut Edit, p: PhysicsProperty) {
    let (label, tip, speed, range) = meta(p);
    let global = uses_global(ctx.c, p);
    let shown = convert(p, effective(ctx, p));
    ui.horizontal(|ui| {
        if ctx.animating {
            let state = property_state(ctx, false, p);
            if ui.add_enabled_ui(!global, |ui| key_button(ui, state)).inner {
                edit.keys.push((p, convert(p, shown)));
            }
        }
        theme::prop(ui, tr(label), tr(tip), |ui| {
            let mut v = shown;
            let w = ui.available_width() - 30.0;
            let tip = if global { "Uses the shared value. Edit it under Shared values, or switch the toggle off." } else { tip };
            let changed = ui.add_enabled_ui(!global, |ui| num_field(ui, &mut v, speed, "", w, tip).changed()).inner;
            if changed && v.is_finite() {
                let v = v.clamp(*range.start(), *range.end());
                if ctx.animating { edit.changes.push((p, convert(p, v))); } else {
                    patch_value(&mut edit.patch, p, convert(p, v));
                    edit.field = Some(property_name(p).to_string());
                }
            }
            let toggle = ui.add_enabled_ui(!ctx.animating, |ui| ui.selectable_label(global, "G")).inner
                .on_hover_text(tr(if ctx.animating { "Switch to SETUP to change which value this uses" } else { "Use the shared value (Shared values below) instead of this constraint's own" }));
            if toggle.clicked() {
                patch_global(&mut edit.patch, p, !global);
                edit.field = Some(format!("{}_global", property_name(p)));
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn influence_row(ui: &mut egui::Ui, edit: &mut Edit, enabled: bool, label: &str, tip: &str, field: &'static str, value: f32, set: fn(&mut PhysicsPatch, f32)) {
    theme::prop(ui, tr(label), tr(tip), |ui| {
        let mut v = value;
        ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(40.0);
        if ui.add_enabled(enabled, egui::Slider::new(&mut v, 0.0..=1.0).max_decimals(2)).on_hover_text(tr(tip)).changed() {
            set(&mut edit.patch, v);
            edit.field = Some(field.to_string());
        }
    });
}

pub(super) fn physics_properties(app: &mut EditorApp, ui: &mut egui::Ui, c: &PhysicsConstraint) {
    let name = c.name.clone();
    let anim = app.playback.anim.clone();
    let animating = anim.is_some();
    let t = app.playback.time;
    let g = app.doc.physics_globals.clone();
    // Values include unkeyed changes (Auto Key off); `held` marks which ones.
    let shown = app.shown_doc();
    let (own, shared) = anim.as_ref().and_then(|a| shown.animations.get(a)).map_or_else(Default::default, |a| (a.physics.get(&name).cloned().unwrap_or_default(), a.physics.get("").cloned().unwrap_or_default()));
    drop(shown);
    let held: Vec<(bool, PhysicsProperty)> = app.pending_other.iter().filter_map(|o| match o {
        Op::KeyPhysics { constraint, property, .. } if *constraint == name => Some((false, *property)),
        Op::KeyPhysics { constraint, property, .. } if constraint.is_empty() => Some((true, *property)),
        _ => None,
    }).collect();
    let ctx = Ctx { c, g: &g, own, shared, animating, t, held };
    let mut edit = Edit { patch: PhysicsPatch { name: name.clone(), ..Default::default() }, ..Default::default() };
    let mut renamed = None;

    let context = crate::tf!("on bone {}", c.bone);
    if let Some(new) = inspector_header(ui, crate::icons::Icon::Physics, theme::PHYSICS, tr("Physics constraint"), &format!("physics_name_{name}"), &name, &context, false) {
        renamed = Some(new);
    }
    if animating {
        theme::hint(ui, &crate::tf!("Animate mode: values below key at the playhead in \"{}\". Switch to SETUP to change the rig itself.", anim.clone().unwrap_or_default()));
    }
    // Bone, influence, preset: the rig itself, so setup pose only.
    ui.add_enabled_ui(!animating, |ui| {
        theme::prop(ui, tr("Bone"), tr("The bone that is simulated"), |ui| {
            let mut bone = c.bone.clone();
            egui::ComboBox::from_id_salt("physics_bone").selected_text(&bone).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                for b in &app.doc.bones { ui.selectable_value(&mut bone, b.name.clone(), &b.name); }
            });
            if bone != c.bone {
                edit.patch.bone = Some(bone);
                edit.field = Some("bone".into());
            }
        });
        theme::prop(ui, tr("Order"), tr("Lower orders run first; IK and physics share the same order"), |ui| {
            let mut order = c.order;
            if ui.add(DragValue::new(&mut order)).changed() {
                edit.patch.order = Some(order);
                edit.field = Some("order".into());
            }
        });
        theme::section(ui, tr("Influence"));
        let tip = "How much the simulation moves this property of the bone. 0 leaves it alone";
        influence_row(ui, &mut edit, true, "Position X", tip, "x", c.x, |p, v| p.x = Some(v));
        influence_row(ui, &mut edit, true, "Position Y", tip, "y", c.y, |p, v| p.y = Some(v));
        influence_row(ui, &mut edit, true, "Rotate", tip, "rotate", c.rotate, |p, v| p.rotate = Some(v));
        influence_row(ui, &mut edit, true, "Scale", tip, "scale_x", c.scale_x, |p, v| p.scale_x = Some(v));
        influence_row(ui, &mut edit, true, "Shear", tip, "shear_x", c.shear_x, |p, v| p.shear_x = Some(v));
        theme::prop(ui, tr("Preset"), tr("Fills the values below for a typical case, in one undo step"), |ui| {
            egui::ComboBox::from_id_salt("physics_preset").selected_text(tr("Choose...")).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                for (i, preset) in PRESETS.iter().enumerate() {
                    if ui.selectable_label(false, tr(preset.label)).clicked() { edit.preset = Some(i); }
                }
            });
        });
    });

    let summary = crate::tf!("inertia {} · strength {} · damping {}", short(effective(&ctx, PhysicsProperty::Inertia)), short(effective(&ctx, PhysicsProperty::Strength)), short(effective(&ctx, PhysicsProperty::Damping)));
    theme::group(ui, "physics_motion", tr("Motion"), &summary, true, |ui| {
        for p in [PhysicsProperty::Inertia, PhysicsProperty::Strength, PhysicsProperty::Damping, PhysicsProperty::Mass] { property_row(ui, &ctx, &mut edit, p); }
    });
    let summary = crate::tf!("wind {} · gravity {}", short(effective(&ctx, PhysicsProperty::Wind)), short(effective(&ctx, PhysicsProperty::Gravity)));
    theme::group(ui, "physics_environment", tr("Environment"), &summary, true, |ui| {
        for p in [PhysicsProperty::Wind, PhysicsProperty::Gravity] { property_row(ui, &ctx, &mut edit, p); }
    });
    let summary = crate::tf!("{} Hz · limit {} · mix {}", short(1.0 / c.step.max(1e-4)), short(c.limit), short(effective(&ctx, PhysicsProperty::Mix)));
    theme::group(ui, "physics_advanced", tr("Advanced"), &summary, false, |ui| {
        ui.add_enabled_ui(!animating, |ui| {
            theme::prop(ui, tr("Step"), tr("Simulation steps per second. The document stores the step as 1 / Hz"), |ui| {
                let mut hz = 1.0 / c.step.max(1e-4);
                let w = ui.available_width();
                if num_field(ui, &mut hz, 0.5, " Hz", w, "Simulation steps per second").changed() && hz.is_finite() {
                    edit.patch.step = Some(1.0 / hz.clamp(1.0, 480.0));
                    edit.field = Some("step".into());
                }
            });
            theme::prop(ui, tr("Limit"), tr("Speed limit in world units per second"), |ui| {
                let mut limit = c.limit;
                let w = ui.available_width();
                if num_field(ui, &mut limit, 10.0, "", w, "Speed limit in world units per second").changed() && limit.is_finite() {
                    edit.patch.limit = Some(limit.max(0.0));
                    edit.field = Some("limit".into());
                }
            });
        });
        property_row(ui, &ctx, &mut edit, PhysicsProperty::Mix);
    });

    // Shared values: setup edits them in the document, animate keys the shared timeline.
    let used = app.doc.physics.iter().map(|c| PROPS.iter().filter(|p| uses_global(c, **p)).count()).sum::<usize>();
    theme::group(ui, "physics_shared", tr("Shared values"), &crate::tf!("{} in use", used), false, |ui| {
        theme::hint(ui, tr(if animating { "Keys here move every value that uses the shared toggle (G)." } else { "Constraints with the G toggle on read these. Change them once for all." }));
        for p in PROPS {
            let (label, tip, speed, range) = meta(p);
            let mut value = g_effective(&ctx, p);
            ui.horizontal(|ui| {
                if animating && key_button(ui, property_state(&ctx, true, p)) { edit.shared_keys.push((p, value)); }
                theme::prop(ui, tr(label), tr(tip), |ui| {
                    let mut v = convert(p, value);
                    let w = ui.available_width();
                    if num_field(ui, &mut v, speed, "", w, tip).changed() && v.is_finite() {
                        value = convert(p, v.clamp(*range.start(), *range.end()));
                        if animating { edit.shared_changes.push((p, value)); } else { edit.globals.push((p, value)); }
                    }
                });
            });
        }
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(tr("Shake")).on_hover_text(tr("Kicks this constraint so you can judge the settings. A preview only; the document is not changed.")).clicked() {
            edit.shake = true;
        }
        if animating && ui.button(tr("Key reset")).on_hover_text(tr("Restart this constraint's simulation from the pose at the playhead")).clicked() {
            edit.key_reset = true;
        }
    });
    if anim.as_ref().is_some_and(|a| app.doc.animations.get(a).and_then(|a| a.physics.get(&name)).is_some_and(|t| t.reset.iter().any(|r| (r - ctx.t).abs() < TIME_EPS))) {
        theme::hint(ui, tr("A reset is keyed at this time."));
    }

    apply(app, &name, anim, t, renamed, edit);
}

/// The shared value at the playhead, from the shared timeline in animate mode.
fn g_effective(ctx: &Ctx, p: PhysicsProperty) -> f32 {
    let mut v = global_value(ctx.g, p);
    if ctx.animating && let Some(s) = sample(keys_of(&ctx.shared, p), ctx.t) { v = s; }
    v
}

fn short(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Turns what the widgets asked for into ops.
fn apply(app: &mut EditorApp, name: &str, anim: Option<String>, t: f32, renamed: Option<String>, edit: Edit) {
    if let Some(new) = renamed {
        let patch = PhysicsPatch { name: name.to_string(), rename: Some(new.clone()), ..Default::default() };
        if app.edit_coalesced(format!("physics:{name}:rename"), vec![Op::SetPhysics(patch)]) { app.selection.physics = Some(new); }
        return;
    }
    if let Some(i) = edit.preset {
        if app.edit(vec![Op::SetPhysics(preset_patch(name, i))]) {
            app.status = crate::tf!("Applied the {} preset · Ctrl+Z to undo", tr(PRESETS[i].label));
        }
        return;
    }
    if let Some(field) = &edit.field {
        app.edit_coalesced(format!("physics:{name}:{field}"), vec![Op::SetPhysics(edit.patch.clone())]);
    }
    if let Some(a) = &anim {
        let key = |constraint: &str, (property, value): (PhysicsProperty, f32)| Op::KeyPhysics { animation: a.clone(), constraint: constraint.to_string(), property, time: t, value, curve: None };
        for k in &edit.keys {
            app.edit_keyed(vec![key(name, *k)]);
        }
        for k in &edit.shared_keys {
            app.edit_keyed(vec![key("", *k)]);
        }
        for k in &edit.changes {
            app.edit_coalesced(format!("physics:{name}:key:{}:{a}:{t}", property_name(k.0)), vec![key(name, *k)]);
        }
        for k in &edit.shared_changes {
            app.edit_coalesced(format!("physics:shared:key:{}:{a}:{t}", property_name(k.0)), vec![key("", *k)]);
        }
        if edit.key_reset {
            app.edit(vec![Op::KeyPhysicsReset { animation: a.clone(), constraint: Some(name.to_string()), time: t }]);
        }
    }
    for (p, v) in &edit.globals {
        app.edit_coalesced(format!("physics:shared:{}", property_name(*p)), vec![Op::SetPhysicsGlobals(globals_patch(*p, *v))]);
    }
    if edit.shake { app.physics_shake(name); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_keep_damping_below_one_and_write_one_patch() {
        for i in 0..PRESETS.len() {
            let p = preset_patch("c", i);
            assert!(p.damping.is_some_and(|d| d > 0.0 && d < 1.0), "{}", PRESETS[i].label);
            assert!(p.mass_inverse.is_some_and(|m| m > 0.0));
        }
    }

    fn app() -> EditorApp {
        let doc = skel2d_core::from_str(r#"{"bones":[{"name":"root"},{"name":"hair","parent":"root","length":40}]}"#).unwrap();
        let doc = skel2d_core::apply(&doc, &[
            Op::AddPhysics(PhysicsConstraint { rotate: 1.0, damping: 0.9, ..PhysicsConstraint::new("sway", "hair") }),
            Op::AddAnimation { name: "idle".into(), duration: Some(1.0) },
        ]).unwrap();
        EditorApp::test_app(doc, std::path::PathBuf::from("physics.skel.json"))
    }

    fn timelines(app: &EditorApp) -> Option<&PhysicsTimelines> {
        app.doc.animations.get("idle").and_then(|a| a.physics.get("sway"))
    }

    #[test]
    fn preset_is_one_set_physics_and_one_undo_step() {
        let mut app = app();
        let original = app.doc.clone();
        apply(&mut app, "sway", None, 0.0, None, Edit { preset: Some(3), ..Default::default() });
        let c = app.doc.physics_constraint("sway").unwrap();
        assert_eq!((c.x, c.y, c.rotate), (0.3, 1.0, 0.0));
        assert_eq!(c.mass_inverse, 1.0 / 0.8);
        assert!(c.damping < 1.0);
        // The preset travels as a plain set_physics op.
        let json = serde_json::to_string(&Op::SetPhysics(preset_patch("sway", 3))).unwrap();
        let cli = skel2d_core::apply(&original, &skel2d_core::parse_ops(&json).unwrap()).unwrap();
        assert_eq!(app.doc, cli);
        app.undo();
        assert_eq!(app.doc, original);
    }

    #[test]
    fn animate_mode_keys_values_and_leaves_the_setup_alone() {
        let mut app = app();
        let setup = app.doc.physics.clone();
        let edit = Edit { keys: vec![(PhysicsProperty::Damping, 0.5), (PhysicsProperty::Mass, 0.25)], key_reset: true, ..Default::default() };
        apply(&mut app, "sway", Some("idle".into()), 0.5, None, edit);
        assert_eq!(app.doc.physics, setup);
        let t = timelines(&app).unwrap();
        assert_eq!((t.damping[0].time, t.damping[0].value), (0.5, 0.5));
        assert_eq!(t.mass[0].value, 0.25); // mass keys hold mass_inverse
        assert_eq!(t.reset, [0.5]);
        // Global values key the shared timeline "" instead.
        let edit = Edit { shared_keys: vec![(PhysicsProperty::Wind, 3.0)], ..Default::default() };
        apply(&mut app, "sway", Some("idle".into()), 0.5, None, edit);
        assert_eq!(app.doc.animations["idle"].physics[""].wind[0].value, 3.0);
        assert_eq!(app.doc.physics_globals, PhysicsGlobals::default());
    }

    #[test]
    fn setup_edits_write_one_patch_and_shared_values_use_the_globals_op() {
        let mut app = app();
        let mut patch = PhysicsPatch { name: "sway".into(), ..Default::default() };
        patch_value(&mut patch, PhysicsProperty::Mass, convert(PhysicsProperty::Mass, 2.0));
        patch_global(&mut patch, PhysicsProperty::Wind, true);
        apply(&mut app, "sway", None, 0.0, None, Edit { patch, field: Some("mass".into()), globals: vec![(PhysicsProperty::Wind, 7.0)], ..Default::default() });
        let c = app.doc.physics_constraint("sway").unwrap();
        assert_eq!((c.mass_inverse, c.wind_global), (0.5, true));
        assert_eq!(app.doc.physics_globals.wind, 7.0);
    }

    #[test]
    fn inspector_in_animate_mode_changes_nothing_by_itself() {
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        for anim in [None, Some("idle".to_string())] {
            let mut app = app();
            app.playback.anim = anim;
            let before = app.doc.clone();
            let c = app.doc.physics_constraint("sway").unwrap().clone();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| physics_properties(&mut app, ui, &c));
            });
            output.textures_delta.clear();
            assert_eq!(app.doc, before);
        }
    }

    #[test]
    fn effective_values_follow_the_simulation_rules() {
        let mut c = PhysicsConstraint::new("c", "b");
        c.wind = 1.0;
        c.wind_global = true;
        let g = PhysicsGlobals { wind: 5.0, ..Default::default() };
        let shared = PhysicsTimelines { wind: vec![Key::new(0.0, 9.0, Default::default())], ..Default::default() };
        let own = PhysicsTimelines { gravity: vec![Key::new(0.0, 2.0, Default::default())], ..Default::default() };
        let ctx = Ctx { c: &c, g: &g, own: own.clone(), shared: shared.clone(), animating: true, t: 0.0, held: Vec::new() };
        assert_eq!(effective(&ctx, PhysicsProperty::Wind), 9.0); // shared timeline reaches constraints using the shared value
        assert_eq!(effective(&ctx, PhysicsProperty::Gravity), 2.0);
        let ctx = Ctx { c: &c, g: &g, own, shared, animating: false, t: 0.0, held: Vec::new() };
        assert_eq!(effective(&ctx, PhysicsProperty::Wind), 5.0); // setup shows the shared value
    }

    #[test]
    fn mass_converts_both_ways() {
        assert_eq!(convert(PhysicsProperty::Mass, convert(PhysicsProperty::Mass, 4.0)), 4.0);
        assert_eq!(convert(PhysicsProperty::Wind, 3.0), 3.0);
    }
}
