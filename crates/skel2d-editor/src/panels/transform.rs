//! Inspector of a transform constraint.

use eframe::egui::{self, DragValue};
use skel2d_core::ops::TransformPatch;
use skel2d_core::{Op, TransformConstraint, TransformMix};

use crate::app::EditorApp;
use crate::i18n::tr;
use crate::theme;

type Field = fn(&mut TransformPatch, f32);
type Row = (&'static str, &'static str, fn(&TransformConstraint) -> f32, Field);

const MIXES: [Row; 6] = [
    ("Rotation", "follow_rotate", |c| c.mix_rotate, |p, v| p.mix_rotate = Some(v)),
    ("Move X", "follow_x", |c| c.mix_x, |p, v| p.mix_x = Some(v)),
    ("Move Y", "follow_y", |c| c.mix_y, |p, v| p.mix_y = Some(v)),
    ("Scale X", "follow_scale_x", |c| c.mix_scale_x, |p, v| p.mix_scale_x = Some(v)),
    ("Scale Y", "follow_scale_y", |c| c.mix_scale_y, |p, v| p.mix_scale_y = Some(v)),
    ("Shear", "follow_shear", |c| c.mix_shear_y, |p, v| p.mix_shear_y = Some(v)),
];

const OFFSETS: [Row; 6] = [
    ("Rotation", "offset_rotate", |c| c.offset_rotation, |p, v| p.offset_rotation = Some(v)),
    ("Move X", "offset_x", |c| c.offset_x, |p, v| p.offset_x = Some(v)),
    ("Move Y", "offset_y", |c| c.offset_y, |p, v| p.offset_y = Some(v)),
    ("Scale X", "offset_scale_x", |c| c.offset_scale_x, |p, v| p.offset_scale_x = Some(v)),
    ("Scale Y", "offset_scale_y", |c| c.offset_scale_y, |p, v| p.offset_scale_y = Some(v)),
    ("Shear", "offset_shear", |c| c.offset_shear_y, |p, v| p.offset_shear_y = Some(v)),
];

pub fn transform_properties(app: &mut EditorApp, ui: &mut egui::Ui, c: &TransformConstraint) {
    ui.label(egui::RichText::new(&c.name).strong().size(theme::SIZE_HEADING));
    theme::hint(ui, tr("These bones copy part of the source bone's movement."));
    let patch = || TransformPatch { name: c.name.clone(), ..Default::default() };
    let mut edit: Option<(String, TransformPatch)> = None;
    let anim = app.playback.anim.clone();
    let time = app.playback.time;
    // While animating, show and key the mixes in effect at the current time.
    let shown_doc = app.shown_doc();
    let current = anim.as_ref().and_then(|a| shown_doc.animations.get(a)).and_then(|a| a.transforms.get(&c.name))
        .and_then(|keys| skel2d_core::pose::sample(keys, time)).unwrap_or_else(|| c.mix());
    let held = app.pending_other.iter().any(|o| matches!(o, Op::KeyTransform { constraint, .. } if *constraint == c.name));
    drop(shown_doc);
    let shown = c.with_mix(current);
    // A value edit (held as an unkeyed change with Auto Key off) and a click on the key button.
    let mut keyed: Option<TransformMix> = None;
    let mut key_button_mix: Option<TransformMix> = None;

    theme::prop(ui, tr("Source"), tr("The bone being copied"), |ui| {
        let mut source = c.source.clone();
        egui::ComboBox::from_id_salt("transform_source").selected_text(&source).show_ui(ui, |ui| {
            for b in &app.doc.bones { ui.selectable_value(&mut source, b.name.clone(), &b.name); }
        });
        if source != c.source { edit = Some(("source".into(), TransformPatch { source: Some(source), ..patch() })); }
    });
    theme::prop(ui, tr("Bones"), tr("The bones that copy it"), |ui| {
        ui.vertical(|ui| {
            for name in &c.bones {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if c.bones.len() > 1 && ui.add(crate::icons::small(crate::icons::Icon::Close)).on_hover_text(tr("Remove")).clicked() {
                        edit = Some(("bones".into(), TransformPatch { bones: Some(c.bones.iter().filter(|b| *b != name).cloned().collect()), ..patch() }));
                    }
                });
            }
            let mut added = None;
            egui::ComboBox::from_id_salt("transform_add_bone").selected_text(tr("Add bone")).show_ui(ui, |ui| {
                for b in app.doc.bones.iter().filter(|b| !c.bones.contains(&b.name)) {
                    if ui.selectable_label(false, &b.name).clicked() { added = Some(b.name.clone()); }
                }
            });
            if let Some(name) = added {
                edit = Some(("bones".into(), TransformPatch { bones: Some(c.bones.iter().cloned().chain([name]).collect()), ..patch() }));
            }
        });
    });

    theme::group(ui, "transform_mix", tr("How much to copy"), "", true, |ui| {
        theme::hint(ui, tr("1 copies fully, 0 ignores it, negative values go the other way."));
        for (label, id, get, set) in MIXES {
            let mut value = get(&shown);
            theme::prop(ui, tr(label), "", |ui| {
                if ui.add(DragValue::new(&mut value).speed(0.01).range(-10.0..=10.0).max_decimals(3)).changed() {
                    let mut p = patch();
                    set(&mut p, value);
                    if anim.is_some() {
                        keyed = Some(TransformMix {
                            rotate: p.mix_rotate.unwrap_or(current.rotate), x: p.mix_x.unwrap_or(current.x), y: p.mix_y.unwrap_or(current.y),
                            scale_x: p.mix_scale_x.unwrap_or(current.scale_x), scale_y: p.mix_scale_y.unwrap_or(current.scale_y), shear_y: p.mix_shear_y.unwrap_or(current.shear_y),
                        });
                    } else {
                        edit = Some((id.into(), p));
                    }
                }
            });
        }
        if anim.is_some() {
            let keys = anim.as_ref().and_then(|a| app.doc.animations.get(a)).and_then(|a| a.transforms.get(&c.name));
            let state = if held {
                super::KeyState::Changed
            } else if keys.is_some_and(|k| k.iter().any(|k| (k.time - time).abs() < skel2d_core::ops::TIME_EPSILON)) {
                super::KeyState::Keyed
            } else if keys.is_some_and(|k| !k.is_empty()) {
                super::KeyState::Interpolated
            } else {
                super::KeyState::None
            };
            if super::key_button(ui, state) {
                key_button_mix = Some(current);
            }
        } else {
            theme::hint(ui, tr("Mix here is the setup value. Pick an animation to key it."));
        }
    });
    theme::group(ui, "transform_offset", tr("Offset from the source"), "", false, |ui| {
        for (label, id, get, set) in OFFSETS {
            let mut value = get(c);
            theme::prop(ui, tr(label), "", |ui| {
                if ui.add(DragValue::new(&mut value).speed(0.1).max_decimals(3)).changed() {
                    let mut p = patch();
                    set(&mut p, value);
                    edit = Some((id.into(), p));
                }
            });
        }
    });
    let (mut local, mut relative) = (c.local, c.relative);
    if ui.checkbox(&mut local, tr("Use values relative to the parent")).on_hover_text(tr("Off compares positions on screen. On compares the bones' own values, so their parents do not matter.")).changed() {
        edit = Some(("local".into(), TransformPatch { local: Some(local), ..patch() }));
    }
    if ui.checkbox(&mut relative, tr("Add to the bone's own movement")).on_hover_text(tr("Off moves the bone toward the source. On adds the source's values to what the bone already does.")).changed() {
        edit = Some(("relative".into(), TransformPatch { relative: Some(relative), ..patch() }));
    }
    if let (Some(a), Some(mix)) = (&anim, key_button_mix) {
        app.edit_keyed(vec![Op::KeyTransform { animation: a.clone(), constraint: c.name.clone(), time, mix, curve: None }]);
    }
    if let (Some(a), Some(mix)) = (&anim, keyed) {
        let op = Op::KeyTransform { animation: a.clone(), constraint: c.name.clone(), time, mix, curve: None };
        app.edit_coalesced(format!("transform:{}:key:{a}:{time}", c.name), vec![op]);
    }
    if let Some((field, p)) = edit {
        app.edit_coalesced(format!("transform:{}:{field}", c.name), vec![Op::SetTransform(p)]);
    }
}
