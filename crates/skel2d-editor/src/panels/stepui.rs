//! The five step screens: the step bar, the tools column, the list of the
//! current step and the panel for the picked item. Everything here only
//! builds ops (or opens the preview dialogs); the document is never edited
//! behind the ops' back, so AI agents can do the same through the same API.

use skel2d_core::BlendMode;
use std::path::PathBuf;

use super::*;
use crate::icons::Icon;
use crate::steps::{self, Step, Target};

// ------------------------------------------------------------ small widgets

/// A button that is greyed out with a reason printed right under it. The
/// reason is always visible, never only a tooltip.
pub(crate) fn action(ui: &mut egui::Ui, label: &str, enabled: bool, reason: &str) -> bool {
    let button = egui::Button::new(RichText::new(tr(label)).size(theme::SIZE_BODY)).min_size(egui::vec2(0.0, theme::MIN_CLICK_H));
    let clicked = ui.add_enabled(enabled, button).clicked();
    if !enabled && !reason.is_empty() {
        reason_line(ui, reason);
    }
    clicked && enabled
}

/// The explanation under a disabled action.
pub(crate) fn reason_line(ui: &mut egui::Ui, reason: &str) {
    ui.add(egui::Label::new(RichText::new(tr(reason)).size(theme::SIZE_SMALL).color(theme::TEXT_DIM)).wrap());
}

/// A wide primary button with an optional icon; `reason` is shown when disabled.
pub(crate) fn big_button(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, enabled: bool, reason: &str) -> bool {
    let size = egui::vec2(ui.available_width(), theme::STEP_BTN_H);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if !enabled {
        Color32::from_gray(52)
    } else if r.is_pointer_button_down_on() {
        theme::STEP_ACTIVE.gamma_multiply(0.8)
    } else if r.hovered() {
        theme::STEP_ACTIVE.gamma_multiply(1.15)
    } else {
        theme::STEP_ACTIVE
    };
    let fg = if enabled { theme::ON_ACCENT } else { theme::TEXT_DISABLED };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, fill);
    if r.has_focus() {
        p.rect_stroke(rect.expand(1.0), 6.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
    }
    let galley = p.layout_no_wrap(label.to_string(), egui::FontId::proportional(theme::SIZE_BODY), fg);
    let icon_w = if icon.is_some() { 24.0 } else { 0.0 };
    let x = rect.center().x - (galley.size().x + icon_w) / 2.0;
    if let Some(icon) = icon {
        crate::icons::image(icon, 18.0).tint(fg).paint_at(ui, egui::Rect::from_center_size(egui::pos2(x + 9.0, rect.center().y), egui::vec2(18.0, 18.0)));
    }
    ui.painter().galley(egui::pos2(x + icon_w, rect.center().y - galley.size().y / 2.0), galley, fg);
    if !enabled && !reason.is_empty() {
        reason_line(ui, reason);
    }
    r.clicked() && enabled
}

/// A line of explanation text.
pub(crate) fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::SIZE_SMALL).color(theme::TEXT_DIM)).wrap());
}

/// A line of body text that wraps.
fn body(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::SIZE_BODY).color(theme::TEXT)).wrap());
}

fn step_tip(step: Step) -> &'static str {
    match step {
        Step::Bones => "Draw the skeleton: the bones your pictures will hang on.",
        Step::Images => "Put pictures on the bones. A slot can hold several pictures.",
        Step::Mesh => "Turn a picture into a mesh so that it can bend.",
        Step::Weights => "Decide how strongly each bone moves each part of a mesh.",
        Step::Animate => "Make motion: move the time marker and change things to record keys.",
    }
}

// ------------------------------------------------------------------ step bar

/// The five steps as big labelled buttons, with Undo / Redo on the right.
pub fn step_bar(app: &mut EditorApp, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for step in Step::ALL {
            let on = app.step == step;
            let label = format!("{}  {}", step.number(), tr(step.label()));
            let icon = match step {
                Step::Bones => Icon::Bone,
                Step::Images => Icon::Image,
                Step::Mesh => Icon::Mesh,
                Step::Weights => Icon::Weights,
                Step::Animate => Icon::Animation,
            };
            if step_button(ui, icon, &label, on, step == Step::Animate).on_hover_text(tr(step_tip(step))).clicked() && !on {
                app.set_step(step);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let redo = crate::icons::labeled(Icon::Redo, format!("{}  Ctrl+Shift+Z", tr("Redo"))).min_size(egui::vec2(0.0, theme::MIN_CLICK_H));
            if ui.add_enabled(app.can_redo(), redo).clicked() {
                app.redo();
            }
            let undo = crate::icons::labeled(Icon::Undo, format!("{}  Ctrl+Z", tr("Undo"))).min_size(egui::vec2(0.0, theme::MIN_CLICK_H));
            if ui.add_enabled(app.can_undo(), undo).clicked() {
                app.undo();
            }
        });
    });
    ui.add_space(4.0);
}

fn step_button(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool, animate: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(theme::SIZE_HEADING), Color32::WHITE);
    let size = egui::vec2((galley.size().x + 56.0).max(124.0), theme::STEP_BTN_H);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let active = if animate { theme::ANIMATE } else { theme::STEP_ACTIVE };
    let (fill, fg) = if on {
        (active, theme::ON_ACCENT)
    } else if r.hovered() {
        (Color32::from_gray(78), Color32::WHITE)
    } else {
        (Color32::from_gray(58), theme::TEXT)
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, fill);
    if !on {
        p.rect_stroke(rect, 5.0, egui::Stroke::new(1.0, Color32::from_gray(84)), egui::StrokeKind::Inside);
    }
    if r.has_focus() {
        p.rect_stroke(rect.expand(1.0), 6.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
    }
    let galley = p.layout_no_wrap(label.to_string(), egui::FontId::proportional(theme::SIZE_HEADING), fg);
    // Icon and label centered together.
    let left = rect.center().x - (galley.size().x + 24.0) / 2.0;
    p.galley(egui::pos2(left + 24.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    crate::icons::image(icon, 18.0).tint(fg).paint_at(ui, egui::Rect::from_center_size(egui::pos2(left + 9.0, rect.center().y), egui::vec2(18.0, 18.0)));
    r
}

// --------------------------------------------------------------- tools column

/// Tools of the current step with their names and keys, and the view toggles.
pub fn tools(app: &mut EditorApp, ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.label(RichText::new(tr("Tools")).strong().size(theme::SIZE_SECTION));
    ui.add_space(2.0);
    let mut picked = None;
    for &tool in app.step.tools() {
        let key = Tool::ALL.iter().find(|t| t.0 == tool).map(|t| format!("{:?}", t.2)).unwrap_or_default();
        let on = app.tool == tool;
        if tool_button(ui, tool.icon(), tr(tool.ui_label()), &key, on).on_hover_text(tr(tool.tip())).clicked() {
            picked = Some(tool);
        }
    }
    if let Some(tool) = picked {
        app.set_tool(tool);
    }
    if matches!(app.step, Step::Mesh | Step::Weights) {
        ui.add_space(4.0);
        note(ui, tr("Options are in the panel on the right."));
    }
    tool_values(app, ui);
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new(tr("Show")).strong().size(theme::SIZE_SECTION));
    let snap_tip = crate::tf!("Snap rotation to {}° and moves to {} units. Hold Ctrl while dragging to invert.", app.settings.snap_angle, app.settings.snap_grid);
    let animate = app.step == Step::Animate;
    let s = &mut app.settings;
    let mut toggles: Vec<(&mut bool, Icon, &str, String)> = vec![
        (&mut app.show_bones, Icon::Bones, "Bones", tr("Show the bones over the images").to_string()),
        (&mut s.show_bone_names, Icon::Names, "Names", tr("Bone names in the viewport").to_string()),
    ];
    if matches!(app.step, Step::Bones | Step::Images | Step::Animate) {
        toggles.push((&mut s.snap, Icon::Snap, "Snap", snap_tip));
    }
    if animate {
        toggles.push((&mut s.onion_skin, Icon::Onion, "Onion skin", tr("Show neighbouring frames as ghosts [O]").to_string()));
    }
    for (flag, icon, label, tip) in toggles {
        if toggle_button(ui, icon, tr(label), *flag).on_hover_text(tip).clicked() {
            *flag = !*flag;
        }
    }
}

/// The active transform tool's value for the selected bone, with its key
/// button beside it (animate step). Typing a value edits it like the
/// inspector does; the button keys the value shown.
fn tool_values(app: &mut EditorApp, ui: &mut egui::Ui) {
    use crate::timeline::{KeyProp, KeyRow};
    let (Some(anim), Some(bone)) = (app.playback.anim.clone(), app.selection.bone.clone()) else { return };
    if app.step != Step::Animate {
        return;
    }
    let (prop, label, field) = match app.tool {
        Tool::Rotate => (KeyProp::Rotate, "Rotate", "rotate"),
        Tool::Translate => (KeyProp::Translate, "Move", "translate"),
        Tool::Scale => (KeyProp::Scale, "Scale", "scale"),
        Tool::Shear => (KeyProp::Shear, "Shear", "shear"),
        _ => return,
    };
    let t = app.playback.time;
    let (rot, trans, scale, shear) = {
        let doc = app.shown_doc();
        let tl = doc.animations.get(&anim).and_then(|a| a.bones.get(&bone));
        (
            tl.and_then(|tl| skel2d_core::pose::sample(&tl.rotate, t)).unwrap_or(0.0),
            tl.and_then(|tl| skel2d_core::pose::sample(&tl.translate, t)).unwrap_or([0.0, 0.0]),
            tl.and_then(|tl| skel2d_core::pose::sample(&tl.scale, t)).unwrap_or([1.0, 1.0]),
            tl.and_then(|tl| skel2d_core::pose::sample(&tl.shear, t)).unwrap_or([0.0, 0.0]),
        )
    };
    let row = KeyRow::BoneProp(bone.clone(), prop);
    let state = crate::timeline::key_row_state(app, &row);
    ui.add_space(8.0);
    ui.separator();
    let mut key = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr(label)).strong().size(theme::SIZE_SECTION));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(theme::MIN_CLICK_H, theme::MIN_CLICK_H), egui::Sense::click());
            let bg = if r.hovered() { Color32::from_gray(74) } else { Color32::from_gray(58) };
            ui.painter().rect_filled(rect, 4.0, bg);
            if r.has_focus() {
                ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            super::paint_key_state(ui.painter(), rect.center(), state, bg);
            key = r.on_hover_text(format!("{} · {}", crate::tf!("Key the {} value of the selected bone [K when it is the only change]", tr(label)), tr(super::key_state_tip(state)))).clicked();
        });
    });
    let mut op = KeyBone { animation: anim.clone(), bone: bone.clone(), time: t, rotate: None, translate: None, scale: None, shear: None, curve: None, curves: None };
    let mut changed = false;
    ui.spacing_mut().interact_size.x = 24.0;
    match prop {
        KeyProp::Rotate => {
            let mut v = rot;
            if ui.add(egui::DragValue::new(&mut v).speed(0.5).max_decimals(1).suffix("°")).on_hover_text(tr("Rotation offset in degrees")).changed() && v.is_finite() {
                op.rotate = Some(v);
                changed = true;
            }
        }
        KeyProp::Translate | KeyProp::Scale => {
            let scale_tool = prop == KeyProp::Scale;
            let mut v = if scale_tool { scale } else { trans };
            ui.horizontal(|ui| {
                for (i, axis) in ["x", "y"].into_iter().enumerate() {
                    let drag = if scale_tool { egui::DragValue::new(&mut v[i]).speed(0.01).max_decimals(2) } else { egui::DragValue::new(&mut v[i]).speed(0.5).max_decimals(1) };
                    changed |= ui.add(drag.prefix(format!("{axis} "))).on_hover_text(tr(if scale_tool { "Scale factor" } else { "Move offset" })).changed();
                }
            });
            if changed && v.iter().all(|x| x.is_finite()) {
                if scale_tool { op.scale = Some(v); } else { op.translate = Some(v); }
            } else {
                changed = false;
            }
        }
        KeyProp::Shear => {
            let mut v = shear;
            ui.horizontal(|ui| {
                for (i, axis) in ["x", "y"].into_iter().enumerate() {
                    changed |= ui.add(egui::DragValue::new(&mut v[i]).speed(0.5).max_decimals(1).suffix("°").prefix(format!("{axis} "))).on_hover_text(tr("Shear offset in degrees")).changed();
                }
            });
            if changed && v.iter().all(|x| x.is_finite()) {
                op.shear = Some(v);
            } else {
                changed = false;
            }
        }
    }
    if changed {
        app.edit_coalesced(format!("animkey:{anim}:{bone}:{field}:{}", (t * 10_000.0).round() as i64), vec![Op::KeyBone(op)]);
    }
    if key {
        crate::timeline::key_bone_property(app, prop);
    }
}

fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, key: &str, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
    let (fill, fg) = if on {
        (theme::STEP_ACTIVE, theme::ON_ACCENT)
    } else if r.hovered() {
        (Color32::from_gray(78), Color32::WHITE)
    } else {
        (Color32::from_gray(58), theme::TEXT)
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, fill);
    if r.has_focus() {
        p.rect_stroke(rect.expand(1.0), 6.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
    }
    crate::icons::image(icon, 22.0).tint(fg).paint_at(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), egui::vec2(22.0, 22.0)));
    let p = ui.painter();
    p.text(egui::pos2(rect.left() + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(theme::SIZE_BODY), fg);
    if !key.is_empty() {
        let key_fg = if on { theme::ON_ACCENT } else { theme::TEXT_DIM };
        p.text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, key, egui::FontId::monospace(theme::SIZE_SMALL), key_fg);
    }
    r
}

/// A view toggle: label plus a check box drawn as a square (filled with a
/// tick when on), so the state is not carried by color alone.
fn toggle_button(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::MIN_CLICK_H + 2.0), egui::Sense::click());
    let fg = if r.hovered() { Color32::WHITE } else { theme::TEXT };
    let p = ui.painter();
    if r.hovered() {
        p.rect_filled(rect, 4.0, Color32::from_gray(58));
    }
    if r.has_focus() {
        p.rect_stroke(rect, 4.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
    }
    crate::icons::image(icon, 18.0).tint(fg).paint_at(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(18.0, 18.0)));
    let p = ui.painter();
    p.text(egui::pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(theme::SIZE_BODY), fg);
    let boxr = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(16.0, 16.0));
    if on {
        p.rect_filled(boxr, 3.0, theme::STEP_ACTIVE);
        let (a, b, c) = (boxr.min + egui::vec2(3.5, 8.0), boxr.min + egui::vec2(6.5, 11.5), boxr.min + egui::vec2(12.5, 4.5));
        let stroke = egui::Stroke::new(2.0, theme::ON_ACCENT);
        p.line_segment([a, b], stroke);
        p.line_segment([b, c], stroke);
    } else {
        p.rect_stroke(boxr, 3.0, egui::Stroke::new(1.5, theme::TEXT_DIM), egui::StrokeKind::Inside);
    }
    r
}

// ------------------------------------------------------------------ side panel

/// The right column: this step's list on top, the picked item's panel below.
pub fn side(app: &mut EditorApp, ui: &mut egui::Ui) {
    let h = ui.ctx().content_rect().height();
    // A taller tree by default; the divider is draggable and its height is remembered.
    let default_h = app.settings.props_h.unwrap_or(h * 0.36).clamp(180.0, (h * 0.6).max(181.0));
    let props = egui::Panel::bottom("inspector").resizable(true).default_size(default_h).size_range(180.0..=(h * 0.6).max(181.0)).show(ui, |ui| {
        let hovered = ui.rect_contains_pointer(ui.max_rect());
        if hovered {
            app.hint = tr("Drag a number to change it (Shift: fine, double-click to type)").into();
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match app.step {
            Step::Bones => bones_panel(app, ui),
            Step::Images => images_panel(app, ui),
            Step::Mesh => mesh_panel(app, ui),
            Step::Weights => weights_step_panel(app, ui),
            Step::Animate => animate_panel(app, ui),
        });
    });
    let height = props.response.rect.height();
    if app.settings.props_h.is_none_or(|p| (p - height).abs() > 2.0) && ui.input(|i| i.pointer.any_released()) {
        app.settings.props_h = Some(height);
    }
    egui::CentralPanel::default().show(ui, |ui| {
        list_header(app, ui);
        tree(app, ui);
    });
}

/// The step's main button(s) above its list.
fn list_header(app: &mut EditorApp, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    match app.step {
        Step::Bones => {
            let sel = app.selection.bone.clone().filter(|b| app.doc.bone(b).is_some());
            let label = match &sel {
                Some(b) if b != skel2d_core::ROOT => crate::tf!("New bone under {}", b),
                _ => tr("New bone under root").to_string(),
            };
            if big_button(ui, Some(Icon::Bone), &label, true, "") {
                add_bone_under_selection(app);
            }
            note(ui, tr("Every bone hangs under root. You can also draw bones in the viewport."));
        }
        Step::Images => {
            if big_button(ui, Some(Icon::Image), tr("Load images"), true, "") {
                let files = crate::import::pick_images();
                if !files.is_empty() {
                    let bone = app.selection.bone.clone();
                    crate::import::import_images(app, &files, app.camera.center, bone);
                }
            }
            note(ui, tr("PNG or PSD. You can also drop files on the viewport. Each file becomes a slot, in front-to-back order below."));
        }
        Step::Mesh => note(ui, tr("Pick the image you want to bend. Images that are not meshes yet are marked \"Picture\".")),
        Step::Weights => note(ui, tr("Pick the mesh to set weights on. Only meshes can have weights.")),
        Step::Animate => {
            let none = app.doc.animations.is_empty();
            if none {
                if big_button(ui, Some(Icon::Animation), tr("New animation"), true, "") {
                    crate::dialogs::new_animation(app);
                }
                note(ui, tr("An animation is a list of keys over time. Create one to start."));
            } else if action(ui, "New animation", true, "") {
                crate::dialogs::new_animation(app);
            }
        }
    }
    ui.add_space(4.0);
}

fn nothing_picked(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(tr(title)).strong().size(theme::SIZE_HEADING));
    ui.add_space(2.0);
    body(ui, tr(hint));
}

// ---------------------------------------------------------------- step 1: bones

fn bones_panel(app: &mut EditorApp, ui: &mut egui::Ui) {
    let ik = app.selection.ik.clone().and_then(|n| app.doc.ik.iter().find(|c| c.name == n).cloned());
    let physics = app.selection.physics.clone().and_then(|n| app.doc.physics_constraint(&n).cloned());
    let bone = app.selection.bone.clone().and_then(|b| app.doc.bone(&b).cloned());
    if let Some(c) = app.selection.transform.clone().and_then(|n| app.doc.transforms.iter().find(|c| c.name == n).cloned()) {
        super::transform::transform_properties(app, ui, &c);
        return;
    }
    if let Some(ik) = &ik {
        ik_properties(app, ui, ik);
        return;
    }
    if let Some(c) = &physics {
        physics::physics_properties(app, ui, c);
        return;
    }
    // A slot picked in the tree shows its inspector here, without leaving the step.
    if let Some(slot) = app.selection.slot.clone().and_then(|s| app.doc.slot(&s).cloned()) {
        slot_panel(app, ui, &slot);
        return;
    }
    let Some(bone) = bone else {
        skeleton_overview(app, ui);
        return;
    };
    bone_properties(app, ui, &bone);
    let is_root = bone.name == skel2d_core::ROOT;
    ui.add_space(4.0);
    theme::group(ui, "bone_follow", tr("Follow target (IK)"), "", false, |ui| {
        note(ui, tr("Makes this bone (and its parent) reach for a target bone, like a hand following a goal. Add it, then move the target."));
        if action(ui, "Add follow target (IK)", true, "") {
            add_ik(app, &bone.name);
        }
    });
    theme::group(ui, "bone_jiggle", tr("Jiggle (physics)"), "", false, |ui| {
        note(ui, tr("Makes this bone lag behind and sway on a spring, like hair or a tail."));
        if action(ui, "Add jiggle (physics)", true, "") {
            crate::dialogs::new_physics(app, Some(bone.name.clone()));
        }
    });
    theme::group(ui, "bone_copy", tr("Copy another bone"), "", false, |ui| {
        note(ui, tr("Makes this bone follow part of another bone's movement, like a face turning with a control bone."));
        if action(ui, "Add copy constraint", true, "") {
            crate::dialogs::new_transform(app, Some(bone.name.clone()));
        }
    });
    ui.add_space(8.0);
    ui.separator();
    let reason = if is_root { ROOT_REASON } else { "" };
    ui.horizontal(|ui| {
        if action(ui, "Rename", !is_root, "") {
            crate::dialogs::rename(app, crate::dialogs::Item::Bone(bone.name.clone()));
        }
        let button = theme::danger_button(tr("Delete bone (with children)")).min_size(egui::vec2(0.0, theme::MIN_CLICK_H));
        if ui.add_enabled(!is_root, button).clicked() {
            crate::dialogs::delete(app, crate::dialogs::Item::Bone(bone.name.clone()));
        }
    });
    if is_root {
        reason_line(ui, reason);
    }
}

/// Nothing picked in step 1: the skeleton's name and what to do next.
fn skeleton_overview(app: &mut EditorApp, ui: &mut egui::Ui) {
    let name = app.doc.name.clone();
    let context = app.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| tr("Not saved yet").to_string());
    if let Some(n) = inspector_header(ui, Icon::Skeleton, ACCENT, tr("Skeleton"), "doc_name", &name, &context, false) {
        app.edit(vec![Op::SetName { name: n }]);
    }
    let bones = app.doc.bones.len();
    ui.add_space(6.0);
    body(ui, &crate::tf!("{} bones. Pick a bone in the list or in the viewport to change its values.", bones));
    ui.add_space(4.0);
    note(ui, tr("Tip: draw bones from the body outwards (spine, then arms and legs). Keep them where the joints bend."));
}

// --------------------------------------------------------------- step 2: images

/// Short kind name of an image, in plain words.
pub(crate) fn kind_name(att: &Attachment) -> &'static str {
    match att {
        Attachment::Region(_) => "Picture",
        Attachment::Mesh(_) => "Mesh",
        Attachment::LinkedMesh(_) => "Shared mesh",
    }
}

/// The image file an attachment draws, following a shared mesh to its parent.
pub(crate) fn attachment_image(slot: &Slot, att: &Attachment) -> Option<String> {
    match att {
        Attachment::Region(r) => r.image.clone(),
        Attachment::Mesh(m) => m.image.clone(),
        Attachment::LinkedMesh(l) => l.image.clone().or_else(|| slot.attachments.iter().find_map(|a| match a {
            Attachment::Mesh(m) if m.name == l.parent => m.image.clone(),
            _ => None,
        })),
    }
}

/// A thumbnail of an image, or a neutral box when it has none (yet).
pub(crate) fn thumb(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot, att: Option<&Attachment>, rect: egui::Rect) {
    let p = ui.painter();
    p.rect_filled(rect, 3.0, Color32::from_gray(34));
    p.rect_stroke(rect, 3.0, egui::Stroke::new(1.0, Color32::from_gray(80)), egui::StrokeKind::Inside);
    let tex = att.and_then(|a| attachment_image(slot, a)).and_then(|rel| app.texture(ui.ctx(), &rel));
    match tex {
        Some(tex) => {
            let [w, h] = tex.size();
            let scale = ((rect.width() - 4.0) / w.max(1) as f32).min((rect.height() - 4.0) / h.max(1) as f32);
            let size = egui::vec2(w as f32 * scale, h as f32 * scale);
            ui.painter().image(tex.id(), egui::Rect::from_center_size(rect.center(), size), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            let icon = if att.is_some() { Icon::Image } else { Icon::EyeOff };
            let size = (rect.width() * 0.5).min(24.0);
            crate::icons::image(icon, size).tint(Color32::from_gray(120)).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(size, size)));
        }
    }
}

/// How an image card reacts to a click.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardMode {
    /// Step 2: pick the card to edit it; "Show" makes it the visible one.
    Setup,
    /// Step 5: a click keys the slot to this image at the playhead.
    Animate,
}

/// Keys `slot` to show `image` (or nothing) from the playhead on. This is
/// what clicking an image card does in the animation step.
pub(crate) fn key_slot_image(app: &mut EditorApp, slot: &str, image: Option<&str>) -> bool {
    let Some(animation) = app.playback.anim.clone() else {
        app.status = tr("Create an animation first.").into();
        return false;
    };
    let time = app.playback.time;
    let op = Op::KeySlot(skel2d_core::ops::KeySlot { animation, slot: slot.to_string(), time, attachment: Some(image.map(str::to_string)), reset: false, color: None, curve: None, curves: None });
    let ok = app.edit_keyed(vec![op]);
    if ok {
        app.status = crate::tf!("Keyed \"{}\" at {} s · Ctrl+Z to undo", image.unwrap_or("(hidden)"), format!("{time:.2}"));
    }
    ok
}

/// The image cards of a slot: thumbnail, name, kind and the Showing marker.
pub(crate) fn image_cards(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot, mode: CardMode) {
    let si = app.doc.slot_index(&slot.name);
    let shown_now = match mode {
        CardMode::Animate => si.and_then(|i| app.pose().slots.get(i).and_then(|s| s.attachment.clone())),
        CardMode::Setup => slot.attachment.clone(),
    };
    let picked = app.selection.attachment.clone().or_else(|| slot.attachment.clone());
    let cards: Vec<Attachment> = slot.attachments.clone();
    for att in &cards {
        let name = att.name().to_string();
        let showing = shown_now.as_deref() == Some(name.as_str());
        let selected = mode == CardMode::Setup && picked.as_deref() == Some(name.as_str());
        let h = theme::THUMB + 12.0;
        let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
        let fill = if selected { theme::ACCENT.gamma_multiply(0.22) } else if r.hovered() { Color32::from_gray(60) } else { theme::HEADER_BG };
        let p = ui.painter();
        p.rect_filled(rect, 5.0, fill);
        p.rect_stroke(rect, 5.0, if selected || r.has_focus() { egui::Stroke::new(2.0, ACCENT) } else { egui::Stroke::new(1.0, Color32::from_gray(80)) }, egui::StrokeKind::Inside);
        let thumb_rect = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 6.0), egui::vec2(theme::THUMB, theme::THUMB));
        thumb(app, ui, slot, Some(att), thumb_rect);
        let tx = thumb_rect.right() + 10.0;
        let right = rect.right() - 8.0;
        let name_w = (right - tx - if mode == CardMode::Setup { 96.0 } else { 70.0 }).max(40.0);
        let p = ui.painter().clone();
        let galley = truncated(&p, &name, egui::FontId::proportional(theme::SIZE_BODY), Color32::WHITE, name_w);
        p.galley(egui::pos2(tx, rect.top() + 10.0), galley, Color32::WHITE);
        p.text(egui::pos2(tx, rect.top() + 32.0), egui::Align2::LEFT_TOP, tr(kind_name(att)), egui::FontId::proportional(theme::SIZE_SMALL), theme::TEXT_DIM);
        // Showing: a filled dot with a word, not a color.
        let mark = if showing { crate::tf!("● {}", tr("Showing")) } else { String::new() };
        if !mark.is_empty() {
            p.text(egui::pos2(right, rect.top() + 10.0), egui::Align2::RIGHT_TOP, mark, egui::FontId::proportional(theme::SIZE_SMALL), theme::SETUP);
        }
        match mode {
            CardMode::Animate => {
                p.text(egui::pos2(right, rect.bottom() - 10.0), egui::Align2::RIGHT_BOTTOM, tr("Click: show from now on"), egui::FontId::proportional(theme::SIZE_SMALL), theme::TEXT_DIM);
                if r.on_hover_text(tr("Switch the slot to this image at the time marker (records a key)")).clicked() {
                    key_slot_image(app, &slot.name, Some(&name));
                }
            }
            CardMode::Setup => {
                let menu_rect = egui::Rect::from_min_max(egui::pos2(right - 110.0, rect.bottom() - 36.0), egui::pos2(right, rect.bottom() - 6.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(menu_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
                let menu = child.menu_button(tr("More"), |ui| card_menu(app, ui, slot, att, showing));
                menu.response.on_hover_text(tr("Rename, replace the file, make a mesh, delete"));
                if r.on_hover_text(tr("Pick this image to edit its values below")).clicked() {
                    app.selection.attachment = Some(name.clone());
                    app.selection.slot = Some(slot.name.clone());
                    app.selection.bone = Some(slot.bone.clone());
                    app.selection.ik = None;
                    app.selection.physics = None;
                }
                if !showing {
                    let show_rect = egui::Rect::from_min_max(egui::pos2(right - 96.0, rect.top() + 6.0), egui::pos2(right, rect.top() + 34.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(show_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    if child.button(tr("Show")).on_hover_text(tr("Make this the image the slot shows")).clicked() {
                        show_attachment(app, &slot.name, &name);
                    }
                }
            }
        }
        ui.add_space(4.0);
    }
    if mode == CardMode::Animate {
        // Hiding is a key too: the slot shows nothing from this time on.
        let hidden = shown_now.is_none();
        let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::MIN_CLICK_H + 12.0), egui::Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, 5.0, if r.hovered() { Color32::from_gray(60) } else { theme::HEADER_BG });
        p.rect_stroke(rect, 5.0, egui::Stroke::new(1.0, Color32::from_gray(80)), egui::StrokeKind::Inside);
        crate::icons::image(Icon::EyeOff, 20.0).tint(theme::TEXT).paint_at(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), egui::vec2(20.0, 20.0)));
        let p = ui.painter();
        p.text(egui::pos2(rect.left() + 42.0, rect.center().y), egui::Align2::LEFT_CENTER, tr("Hide the slot"), egui::FontId::proportional(theme::SIZE_BODY), Color32::WHITE);
        if hidden {
            p.text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, crate::tf!("● {}", tr("Showing")), egui::FontId::proportional(theme::SIZE_SMALL), theme::SETUP);
        }
        if r.on_hover_text(tr("Show nothing from the time marker on (records a key)")).clicked() {
            key_slot_image(app, &slot.name, None);
        }
        ui.add_space(4.0);
    }
}

/// The menu of an image card.
fn card_menu(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot, att: &Attachment, showing: bool) {
    let name = att.name().to_string();
    let skin = app.skin_of(&slot.name, &name);
    if ui.button(tr("Rename")).clicked() {
        crate::dialogs::rename(app, crate::dialogs::Item::Attachment { slot: slot.name.clone(), name: name.clone(), skin: skin.clone() });
        ui.close();
    }
    if ui.button(tr("Replace image file...")).on_hover_text(tr("Pick another PNG for this image")).clicked() {
        ui.close();
        if let Some(file) = rfd::FileDialog::new().add_filter(tr("PNG image"), &["png"]).pick_file() {
            match crate::import::replace_image(app, &slot.name, att, &file) {
                Some(op) => {
                    app.edit(vec![op]);
                }
                None => app.status = crate::tf!("Cannot read {}", file.display()),
            }
        }
    }
    if ui.button(tr("Make mesh (step 3)")).on_hover_text(tr("Go to step 3 with this image picked")).clicked() {
        app.selection.attachment = Some(name.clone());
        app.selection.slot = Some(slot.name.clone());
        app.set_step(Step::Mesh);
        ui.close();
    }
    ui.separator();
    let _ = showing;
    if ui.add(theme::danger_button(tr("Delete image"))).clicked() {
        crate::dialogs::delete(app, crate::dialogs::Item::Attachment { slot: slot.name.clone(), name, skin });
        ui.close();
    }
}

/// Where a new image of a slot goes: on top of the one shown now, so that
/// swapping images (eyes open / closed) does not move anything.
fn slot_anchor(app: &EditorApp, slot: &Slot) -> (f32, f32, f32, f32, f32) {
    let shown = slot.attachment.as_deref().and_then(|a| slot.attachments.iter().find(|x| x.name() == a)).or_else(|| slot.attachments.first());
    match shown {
        Some(Attachment::Region(r)) => (r.x, r.y, r.rotation, r.scale_x, r.scale_y),
        Some(Attachment::Mesh(m)) if !m.vertices.is_empty() => {
            // Mesh vertices are in skeleton space: take their center in the bone's space.
            let n = m.vertices.len() as f32;
            let (cx, cy) = (m.vertices.iter().map(|v| v.x).sum::<f32>() / n, m.vertices.iter().map(|v| v.y).sum::<f32>() / n);
            let pose = crate::import::setup_pose(&app.doc);
            match app.doc.bone_index(&slot.bone) {
                Some(i) => crate::import::local_trs(&pose.bones[i].world, &skel2d_core::Affine::from_trs(cx, cy, 0.0, 1.0, 1.0)),
                None => (cx, cy, 0.0, 1.0, 1.0),
            }
        }
        _ => {
            let length = app.doc.bone(&slot.bone).map_or(40.0, |b| b.length.abs().max(20.0));
            (length / 2.0, 0.0, 0.0, 1.0, 1.0)
        }
    }
}

/// Ops that add `files` as new images (pictures) of `slot`. Names come from
/// the file names and stay unique; each picture is as big as its PNG and sits
/// where the slot's current image sits.
pub(crate) fn add_image_ops(app: &EditorApp, slot: &str, files: &[PathBuf]) -> Result<Vec<Op>, String> {
    let slot = app.doc.slot(slot).ok_or_else(|| crate::tf!("Slot \"{}\" does not exist", slot))?;
    let base = app.base_dir();
    let (x, y, rotation, scale_x, scale_y) = slot_anchor(app, slot);
    let mut taken: Vec<String> = slot.attachments.iter().map(|a| a.name().to_string()).collect();
    let mut ops = Vec::new();
    let mut first = None;
    for file in files {
        let [w, h] = crate::import::image_size(file).ok_or_else(|| crate::tf!("Cannot read {}", file.display()))?;
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "image".into());
        let name = unique_name_from(&stem, |n| taken.iter().any(|t| t == n));
        taken.push(name.clone());
        first.get_or_insert_with(|| name.clone());
        let region = Region { name, image: Some(crate::import::rel_path(&base, file)), x, y, rotation, scale_x, scale_y, width: w as f32, height: h as f32 };
        ops.push(Op::SetAttachment { slot: slot.name.clone(), attachment: Attachment::Region(region) });
    }
    // A slot that shows nothing starts showing the first new image.
    if slot.attachment.is_none()
        && let Some(name) = first
    {
        ops.push(Op::SetSlot(SlotPatch { name: slot.name.clone(), attachment: Some(Some(name)), ..Default::default() }));
    }
    Ok(ops)
}

/// `stem`, or `stem_2`, `stem_3`... when taken.
fn unique_name_from(stem: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(stem) {
        return stem.to_string();
    }
    (2..).map(|i| format!("{stem}_{i}")).find(|n| !taken(n)).expect("unbounded range")
}

/// Adds the files as images of the slot in one undo step.
pub(crate) fn add_images(app: &mut EditorApp, slot: &str, files: &[PathBuf]) -> bool {
    match add_image_ops(app, slot, files) {
        Ok(ops) if ops.is_empty() => false,
        Ok(ops) => {
            let last = ops.iter().rev().find_map(|o| match o {
                Op::SetAttachment { attachment, .. } => Some(attachment.name().to_string()),
                _ => None,
            });
            let n = files.len();
            if app.edit(ops) {
                app.selection.slot = Some(slot.to_string());
                app.selection.attachment = last;
                app.status = crate::tf!("Added {} images to slot {} · Ctrl+Z to undo", n, slot);
                return true;
            }
            false
        }
        Err(e) => {
            app.status = e;
            false
        }
    }
}

fn pick_png_files() -> Vec<PathBuf> {
    rfd::FileDialog::new().add_filter(tr("PNG image"), &["png"]).pick_files().unwrap_or_default()
}

fn images_panel(app: &mut EditorApp, ui: &mut egui::Ui) {
    let slot = app.selection.slot.clone().and_then(|s| app.doc.slot(&s).cloned());
    match slot {
        Some(slot) => slot_panel(app, ui, &slot),
        None => {
            if let Some(skin) = app.selection.skin.clone().and_then(|n| app.doc.skin(&n).cloned()) {
                skin_properties(app, ui, &skin);
            } else if app.doc.slots.is_empty() {
                nothing_picked(ui, "No images yet", "Press Load images above, or drop PNG files on the viewport. Each file becomes a slot: a place on a bone that shows one picture.");
            } else {
                nothing_picked(ui, "Pick a slot", "Pick a slot in the list to see its images. A slot can hold several images (eyes open and closed, mouth shapes) and shows one at a time.");
            }
            outfits_group(app, ui);
        }
    }
}

/// The slot panel of step 2: its images as cards, the picked image's values,
/// then the slot's own settings.
/// Clipping at the top of the slot panel: says the slot's state in words, with
/// the one action that fits it. The same actions are on the slot's right-click
/// menu, on Ctrl+Alt+G and in command search.
fn clip_section(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot) {
    let name = slot.name.clone();
    ui.add_space(4.0);
    theme::section(ui, tr("Clipping"));
    if let Some(base) = slot.clip.clone() {
        ui.horizontal(|ui| {
            ui.add(crate::icons::image(Icon::Clip, 16.0).tint(theme::SLOT));
            ui.label(crate::tf!("Shows only inside {}", base));
        });
        if ui.add(crate::icons::labeled(Icon::Close, tr("Remove clip"))).on_hover_text("Ctrl+Alt+G").clicked() {
            super::set_clip(app, &name, None);
        }
        return;
    }
    let clipped: Vec<String> = app.doc.slots.iter().filter(|s| s.clip.as_deref() == Some(name.as_str())).map(|s| s.name.clone()).collect();
    if !clipped.is_empty() {
        note(ui, &crate::tf!("Clipping {}. Those slots show only inside this one.", clipped.join(", ")));
        return;
    }
    let below = super::clip_below_target(&app.doc, &name);
    match below {
        Some(base) => {
            if ui.add(crate::icons::labeled(Icon::Clip, crate::tf!("Clip to {}", base))).on_hover_text(format!("{} · Ctrl+Alt+G", tr("Show this slot only inside the slot behind it, like a Photoshop clipping layer. Both keep moving on their own."))).clicked() {
                super::set_clip(app, &name, Some(base));
            }
        }
        None => note(ui, tr("There is no slot behind this one to clip to")),
    }
    let mut clip = None;
    egui::ComboBox::from_id_salt("slot_clip").selected_text(tr("Clip to another slot")).show_ui(ui, |ui| {
        for s in app.doc.slots.iter().rev().filter(|s| s.name != name && s.clip.is_none()) {
            if ui.selectable_label(false, &s.name).clicked() {
                clip = Some(s.name.clone());
            }
        }
    });
    if clip.is_some() {
        super::set_clip(app, &name, clip);
    }
    ui.add_space(4.0);
}

fn slot_panel(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot) {
    let name = slot.name.clone();
    let index = app.doc.slot_index(&name).unwrap_or_default();
    let context = crate::tf!("on bone {} · draw order {} of {} from the back", slot.bone, index + 1, app.doc.slots.len());
    if let Some(new) = inspector_header(ui, Icon::Slot, theme::SLOT, tr("Slot"), &format!("slot_name_{name}"), &name, &context, false)
        && app.edit(vec![Op::SetSlot(SlotPatch { name: name.clone(), rename: Some(new.clone()), ..Default::default() })])
    {
        app.selection.slot = Some(new);
        return;
    }
    clip_section(app, ui, slot);
    theme::section(ui, &crate::tf!("Images in this slot ({})", slot.attachments.len()));
    note(ui, tr("A slot shows one image at a time. Add more images for other looks, like eyes open and closed or mouth shapes."));
    ui.add_space(4.0);
    image_cards(app, ui, slot, CardMode::Setup);
    if big_button(ui, Some(Icon::Add), tr("Add images"), true, "") {
        let files = pick_png_files();
        if !files.is_empty() {
            add_images(app, &name, &files);
        }
    }
    note(ui, tr("Pick one or several PNG files. They are added to this slot, named after their files."));

    let mut op: Option<(String, Op)> = None;
    let mut multi: Option<Vec<Op>> = None;
    let shown = app.selection.attachment.as_deref().and_then(|a| slot.attachments.iter().find(|x| x.name() == a)).cloned().or_else(|| slot.attachment.as_deref().and_then(|a| slot.attachments.iter().find(|x| x.name() == a)).cloned());
    if let Some(Attachment::Region(region)) = shown.as_ref() {
        let mut r = region.clone();
        let mut field = None;
        let summary = format!("{:.0} × {:.0}", region.width, region.height);
        theme::group(ui, "region", &crate::tf!("Values of \"{}\"", region.name), &summary, true, |ui| {
            note(ui, tr("Position, turn and size of this image on its bone."));
            theme::prop(ui, tr("File"), "", |ui| {
                if let Some(path) = text_commit(ui, &format!("img_{name}_{}", r.name), r.image.as_deref().unwrap_or_default()) {
                    r.image = Some(path);
                    field = Some("image");
                }
            });
            theme::prop(ui, "", "", |ui| {
                if ui.button(tr("Browse...")).on_hover_text(tr("Pick a PNG; the picture takes its pixel size")).clicked()
                    && let Some(file) = rfd::FileDialog::new().add_filter(tr("PNG image"), &["png"]).pick_file()
                {
                    match crate::import::replace_image(app, &name, &Attachment::Region(region.clone()), &file) {
                        Some(o) => multi = Some(vec![o]),
                        None => app.status = crate::tf!("Cannot read {}", file.display()),
                    }
                }
                if ui.add_enabled(region.image.is_some(), egui::Button::new(tr("Fit size"))).on_hover_text(tr("Width and height from the image (1 unit per pixel)")).clicked() {
                    match crate::import::fit_region_to_image(app, &name, region) {
                        Some(o) => multi = Some(vec![o]),
                        None => app.status = crate::tf!("Missing image {}", region.image.as_deref().unwrap_or_default()),
                    }
                }
            });
            if let Some(v) = pair_row(ui, "Position", [r.x, r.y], 0.5, "", "Offset from the bone") {
                (r.x, r.y) = (v[0], v[1]);
                field = Some("xy");
            }
            if let Some(v) = num_row(ui, "Rotation", r.rotation, 0.5, "°", "Rotation relative to the bone") {
                r.rotation = v;
                field = Some("rotation");
            }
            if let Some(v) = pair_row(ui, "Scale", [r.scale_x, r.scale_y], 0.01, "×", "Scale factor") {
                (r.scale_x, r.scale_y) = (v[0], v[1]);
                field = Some("scale");
            }
            if let Some(v) = pair_row(ui, "Dimensions", [r.width, r.height], 0.5, "", "Image size in units (width, height)") {
                (r.width, r.height) = (v[0], v[1]);
                field = Some("size");
            }
        });
        if let Some(f) = field {
            op = Some((format!("region:{}:{f}", r.name), Op::SetAttachment { slot: name.clone(), attachment: Attachment::Region(r) }));
        }
    } else if let Some(att) = shown.as_ref() {
        theme::group(ui, "mesh_values", &crate::tf!("Values of \"{}\"", att.name()), tr(kind_name(att)), true, |ui| {
            note(ui, tr("The shape of a mesh is edited in step 3, its weights in step 4."));
            if let Attachment::Mesh(mesh) = att
                && let Some(path) = text_commit(ui, &format!("img_{name}_{}", mesh.name), mesh.image.as_deref().unwrap_or_default())
            {
                let mut m = mesh.clone();
                m.image = Some(path);
                op = Some((format!("mesh:{}:image", m.name), Op::SetAttachment { slot: name.clone(), attachment: Attachment::Mesh(m) }));
            }
            if ui.button(tr("Edit shape (step 3)")).clicked() {
                app.set_step(Step::Mesh);
            }
        });
    }

    theme::group(ui, "slot_settings", tr("Slot settings"), "", false, |ui| {
        theme::prop(ui, tr("Bone"), tr("Pictures keep their place when the slot moves to another bone"), |ui| {
            let mut bone = slot.bone.clone();
            egui::ComboBox::from_id_salt("slot_bone").selected_text(&bone).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                for b in &app.doc.bones {
                    ui.selectable_value(&mut bone, b.name.clone(), &b.name);
                }
            });
            if bone != slot.bone {
                multi = Some(crate::import::rebind_slot(&app.doc, slot, &bone));
            }
        });
        theme::prop(ui, tr("Color"), tr("Tint multiplied with the image; alpha fades it"), |ui| {
            let [r, g, b, a] = slot.color.to_rgba8();
            let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
            if ui.color_edit_button_srgba(&mut c).changed() {
                let [r, g, b, a] = c.to_srgba_unmultiplied();
                let color = Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 };
                op = Some(("color".into(), Op::SetSlot(SlotPatch { name: name.clone(), color: Some(color), ..Default::default() })));
            }
            theme::hint(ui, &format!("#{r:02X}{g:02X}{b:02X} · {}%", (a as f32 / 2.55).round()));
        });
        theme::prop(ui, tr("Blend"), tr("How this slot combines with what is behind it"), |ui| {
            let mut blend = slot.blend;
            let label = |b: BlendMode| match b {
                BlendMode::Normal => tr("Normal"),
                BlendMode::Additive => tr("Add (glow)"),
                BlendMode::Multiply => tr("Multiply (shadow)"),
                BlendMode::Screen => tr("Screen (soft light)"),
            };
            egui::ComboBox::from_id_salt("slot_blend").selected_text(label(blend)).show_ui(ui, |ui| {
                for b in [BlendMode::Normal, BlendMode::Additive, BlendMode::Multiply, BlendMode::Screen] {
                    ui.selectable_value(&mut blend, b, label(b));
                }
            });
            if blend != slot.blend {
                op = Some(("blend".into(), Op::SetSlot(SlotPatch { name: name.clone(), blend: Some(blend), ..Default::default() })));
            }
        });
        theme::prop(ui, tr("Front to back"), tr("Pictures higher in the list are drawn in front. Drag the slot in the list to change it."), |ui| {
            let count = app.doc.slots.len();
            if ui.add_enabled(index + 1 < count, egui::Button::new(tr("Forward"))).on_hover_text(tr("Move forward")).clicked() {
                op = Some(("order".into(), Op::MoveSlot { name: name.clone(), index: index + 1 }));
            }
            if ui.add_enabled(index > 0, egui::Button::new(tr("Back"))).on_hover_text(tr("Move back")).clicked() {
                op = Some(("order".into(), Op::MoveSlot { name: name.clone(), index: index.saturating_sub(1) }));
            }
            ui.label(RichText::new(crate::tf!("{} of {}", count - index, count)).color(TEXT_DIM));
        });
        ui.horizontal(|ui| {
            if ui.add(theme::danger_button(tr("Delete slot")).min_size(egui::vec2(0.0, theme::MIN_CLICK_H))).clicked() {
                crate::dialogs::delete(app, crate::dialogs::Item::Slot(name.clone()));
            }
        });
    });

    if let Some(ops) = multi {
        app.edit(ops);
    }
    if let Some((field, op)) = op {
        let renamed = match &op {
            Op::SetSlot(SlotPatch { rename: Some(n), .. }) => Some(n.clone()),
            _ => None,
        };
        if app.edit_coalesced(format!("slot:{name}:{field}"), vec![op])
            && let Some(n) = renamed
        {
            app.selection.slot = Some(n);
        }
    }
    slot_skins_section(app, ui, slot);
    outfits_group(app, ui);
}

/// "Outfits (skins)": what they are, the list with preview eyes, a new one.
fn outfits_group(app: &mut EditorApp, ui: &mut egui::Ui) {
    let summary = app.doc.skins.len().to_string();
    theme::group(ui, "outfits", tr("Outfits (skin)"), &summary, !app.doc.skins.is_empty(), |ui| {
        note(ui, tr("An outfit swaps images in some slots, like another hair style or clothes, without changing the rig."));
        let names: Vec<String> = app.doc.skins.iter().map(|k| k.name.clone()).collect();
        for skin in names {
            ui.horizontal(|ui| {
                let on = app.skins.contains(&skin);
                let icon = if on { Icon::Eye } else { Icon::EyeOff };
                let eye = egui::Button::image_and_text(crate::icons::image(icon, 16.0).tint(theme::TEXT), tr(if on { "Shown" } else { "Hidden" })).min_size(egui::vec2(0.0, theme::MIN_CLICK_H));
                if ui.add(eye).on_hover_text(tr("Preview this outfit in the viewport")).clicked() {
                    toggle_skin_preview(app, &skin);
                }
                let selected = app.selection.skin.as_deref() == Some(skin.as_str());
                if ui.selectable_label(selected, &skin).clicked() {
                    select_skin(app, &skin);
                }
                ui.menu_button(tr("More"), |ui| {
                    if ui.button(tr("Rename")).clicked() {
                        crate::dialogs::rename(app, crate::dialogs::Item::Skin(skin.clone()));
                        ui.close();
                    }
                    if ui.add(crate::icons::labeled(Icon::Duplicate, tr("Duplicate"))).clicked() {
                        if let Some(s) = app.doc.skin(&skin).cloned() {
                            duplicate_skin(app, &s);
                        }
                        ui.close();
                    }
                    if ui.add(theme::danger_button(tr("Delete"))).clicked() {
                        crate::dialogs::delete(app, crate::dialogs::Item::Skin(skin.clone()));
                        ui.close();
                    }
                });
            });
        }
        if action(ui, "New outfit", true, "") {
            new_skin(app);
        }
    });
}

// ------------------------------------------------------------------ step 3: mesh

/// The "Slot › Image" line that tells what the step works on.
fn target_bar(ui: &mut egui::Ui, t: &Target) {
    egui::Frame::new().fill(theme::HEADER_BG).corner_radius(5).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match steps::target_path(t) {
            Some(path) => {
                ui.label(RichText::new(tr("Working on")).size(theme::SIZE_SMALL).color(TEXT_DIM));
                ui.label(RichText::new(path).strong().size(theme::SIZE_HEADING));
            }
            None => {
                ui.label(RichText::new(tr("Nothing picked")).strong().size(theme::SIZE_HEADING));
            }
        }
    });
    ui.add_space(4.0);
}

/// Images that can become meshes, as a clickable list.
fn pick_list(app: &mut EditorApp, ui: &mut egui::Ui, only_meshes: bool) {
    let mut picked = None;
    let mut any = false;
    let slots: Vec<Slot> = app.doc.slots.iter().rev().cloned().collect();
    for slot in &slots {
        for att in &slot.attachments {
            if only_meshes && !matches!(att, Attachment::Mesh(_)) {
                continue;
            }
            if !only_meshes && matches!(att, Attachment::LinkedMesh(_)) {
                continue;
            }
            any = true;
            let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::THUMB_SMALL + 8.0), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 4.0, if r.hovered() { Color32::from_gray(64) } else { theme::HEADER_BG });
            if r.has_focus() {
                p.rect_stroke(rect, 4.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            thumb(app, ui, slot, Some(att), egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(theme::THUMB_SMALL, theme::THUMB_SMALL)));
            let p = ui.painter();
            p.text(egui::pos2(rect.left() + theme::THUMB_SMALL + 14.0, rect.center().y), egui::Align2::LEFT_CENTER, format!("{} › {}", slot.name, att.name()), egui::FontId::proportional(theme::SIZE_BODY), Color32::WHITE);
            p.text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, tr(kind_name(att)), egui::FontId::proportional(theme::SIZE_SMALL), TEXT_DIM);
            if r.clicked() {
                picked = Some((slot.name.clone(), slot.bone.clone(), att.name().to_string()));
            }
            ui.add_space(2.0);
        }
    }
    if !any {
        note(ui, tr(if only_meshes { "No meshes yet. Make one in step 3." } else { "No images yet. Load images in step 2." }));
    }
    if let Some((slot, bone, att)) = picked {
        app.selection = crate::app::Selection { slot: Some(slot), bone: Some(bone), attachment: Some(att), ..Default::default() };
    }
}

fn mesh_panel(app: &mut EditorApp, ui: &mut egui::Ui) {
    let target = steps::target(app);
    target_bar(ui, &target);
    body(ui, &steps::mesh_status(&target));
    ui.add_space(6.0);
    match &target {
        Target::None => {
            ui.label(RichText::new(tr("Images")).strong());
            pick_list(app, ui, false);
        }
        Target::Hidden(_) => {}
        Target::Picture { slot, image } => {
            let Some((s, att)) = steps::selected_attachment(app) else { return };
            let (slot, image) = (slot.clone(), image.clone());
            if big_button(ui, Some(Icon::Mesh), tr("Make mesh"), true, "") {
                crate::dialogs::attachment_mesh_options(app, &s, &att);
            }
            note(ui, tr("A preview shows the mesh before it is applied."));
            let _ = (slot, image);
        }
        Target::Shared { slot, parent, .. } => {
            if let Some((_, Attachment::LinkedMesh(link))) = steps::selected_attachment(app) {
                let slot = slot.clone();
                if big_button(ui, None, tr("Go to parent mesh"), true, "") {
                    go_to_linked_parent(app, &slot, &link);
                }
            }
            note(ui, &crate::tf!("A shared mesh copies the shape and weights of \"{}\" and can show another image.", parent));
        }
        Target::Mesh { .. } => mesh_tools(app, ui),
    }
}

/// Step 3 for a mesh: modes with their explanation, vertex actions, rebuild.
fn mesh_tools(app: &mut EditorApp, ui: &mut egui::Ui) {
    use crate::viewport::{MeshAction, MeshMode};
    let Some((slot, Attachment::Mesh(mesh))) = steps::selected_attachment(app) else { return };
    let name = slot.name.clone();
    let mut multi: Option<(Vec<Op>, String)> = None;
    ui.label(RichText::new(tr("What the mouse does in the viewport")).strong());
    for (mode, label, what) in [
        (MeshMode::Modify, "Move", "drag a vertex to reshape the mesh"),
        (MeshMode::Create, "Add", "click inside the image to add a vertex"),
        (MeshMode::Delete, "Delete", "click a vertex to remove it"),
        (MeshMode::Edges, "Fold line", "drag between two vertices to make a fold line"),
    ] {
        let on = app.view.mesh_mode == mode;
        let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, 5.0, if on { theme::STEP_ACTIVE } else if r.hovered() { Color32::from_gray(70) } else { Color32::from_gray(54) });
        if r.has_focus() {
            p.rect_stroke(rect, 5.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
        }
        let fg = if on { theme::ON_ACCENT } else { theme::TEXT };
        let sub = if on { theme::ON_ACCENT } else { theme::TEXT_DIM };
        // A filled dot marks the active mode, besides the color.
        if on {
            p.circle_filled(egui::pos2(rect.left() + 14.0, rect.center().y), 5.0, theme::ON_ACCENT);
        } else {
            p.circle_stroke(egui::pos2(rect.left() + 14.0, rect.center().y), 5.0, egui::Stroke::new(1.5, theme::TEXT_DIM));
        }
        p.text(egui::pos2(rect.left() + 30.0, rect.top() + 6.0), egui::Align2::LEFT_TOP, tr(label), egui::FontId::proportional(theme::SIZE_BODY), fg);
        let mut job = egui::text::LayoutJob::simple_singleline(tr(what).to_string(), egui::FontId::proportional(theme::SIZE_SMALL), sub);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 40.0).max(1.0));
        let galley = p.layout_job(job);
        p.galley(egui::pos2(rect.left() + 30.0, rect.top() + 25.0), galley, sub);
        if r.clicked() {
            app.view.mesh_mode = mode;
        }
        ui.add_space(2.0);
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut app.settings.mesh_triangles, tr("Triangles")).on_hover_text(tr("Show the inner triangle edges (the outline is always shown in cyan)"));
        ui.toggle_value(&mut app.settings.mesh_dim, tr("Dim image")).on_hover_text(tr("Dim the image so vertices and edges stand out"));
        ui.toggle_value(&mut app.settings.mesh_isolate, tr("Hide others")).on_hover_text(tr("Hide the other slots while editing this mesh"));
    });
    let picked = crate::viewport::picked(app, &app.pose()).map(|(_, p)| p).unwrap_or_default();
    theme::section(ui, tr("Vertices"));
    note(ui, &crate::tf!("Fold lines: {}", mesh.edges.len()));
    let delete_label = if picked.is_empty() { tr("Delete picked vertices").to_string() } else { crate::tf!("Delete {} picked vertices", picked.len()) };
    if action(ui, &delete_label, !picked.is_empty(), "To pick vertices: click one, or drag a box around several in the viewport.") {
        multi = crate::viewport::selected_mesh_op(app, MeshAction::RemovePicked).map(|op| (vec![op], tr("Vertices deleted").to_string()));
    }
    let remove_edge = crate::viewport::selected_mesh_op(app, MeshAction::RemoveEdge);
    if action(ui, "Remove picked fold line", remove_edge.is_some(), "Click an orange fold line in the viewport first (use the Fold line mode).") {
        multi = remove_edge.map(|op| (vec![op], tr("Fold line removed").to_string()));
    }
    let gid = ui.make_persistent_id("generate_count");
    let mut count: usize = ui.data_mut(|d| d.get_temp(gid)).unwrap_or(8);
    ui.horizontal(|ui| {
        if action(ui, "Add vertices automatically", true, "") {
            multi = crate::viewport::selected_mesh_op(app, MeshAction::Generate(count)).map(|op| (vec![op], crate::tf!("{} vertices added · Ctrl+Z to undo", count)));
        }
        ui.add(DragValue::new(&mut count).range(1..=200).suffix(tr(" vertices")));
    });
    ui.data_mut(|d| d.insert_temp(gid, count));
    note(ui, tr("Adds vertices where the triangles are largest. Existing vertices stay."));

    theme::section(ui, tr("The whole mesh"));
    if mesh.image.is_some() {
        if action(ui, "Rebuild mesh", true, "") {
            crate::dialogs::attachment_mesh_options(app, &slot, &Attachment::Mesh(mesh.clone()));
        }
        note(ui, tr("Replaces the mesh with a new automatic one over the same image. Painted weights carry over."));
    } else {
        reason_line(ui, "This mesh has no image file, so it cannot be rebuilt automatically.");
    }
    if action(ui, "Make shared mesh", true, "") {
        crate::dialogs::new_linked_mesh(app, &name, &mesh.name);
    }
    note(ui, tr("Creates another image that copies this mesh's shape and weights, for outfits and variants."));
    theme::group(ui, "mesh_advanced", tr("Advanced"), "", false, |ui| {
        for (label, mode, what) in [
            ("Reset vertices to UVs", skel2d_core::ops::MeshResetMode::VerticesToUvs, "Moves the vertices back to the image's corners."),
            ("Fit UVs to vertices", skel2d_core::ops::MeshResetMode::UvsToVertices, "Re-maps the image onto the current vertices."),
        ] {
            if action(ui, label, true, "") {
                multi = crate::viewport::selected_mesh_op(app, MeshAction::Reset(mode)).map(|op| (vec![op], tr("Done · Ctrl+Z to undo").to_string()));
            }
            note(ui, tr(what));
        }
    });
    if let Some((ops, message)) = multi
        && app.edit(ops)
    {
        app.status = message;
    }
}

// --------------------------------------------------------------- step 4: weights

fn weights_step_panel(app: &mut EditorApp, ui: &mut egui::Ui) {
    let target = steps::target(app);
    target_bar(ui, &target);
    body(ui, &steps::weights_status(app, &target));
    ui.add_space(6.0);
    match &target {
        Target::None => {
            ui.label(RichText::new(tr("Meshes")).strong());
            pick_list(app, ui, true);
        }
        Target::Hidden(_) => {}
        Target::Picture { .. } => {
            if big_button(ui, None, tr("Go to step 3 and make a mesh"), true, "") {
                app.set_step(Step::Mesh);
                if let Some((slot, att)) = steps::selected_attachment(app) {
                    crate::dialogs::attachment_mesh_options(app, &slot, &att);
                }
            }
        }
        Target::Shared { slot, .. } => {
            if let Some((_, Attachment::LinkedMesh(link))) = steps::selected_attachment(app) {
                let slot = slot.clone();
                if big_button(ui, None, tr("Go to parent mesh"), true, "") {
                    go_to_linked_parent(app, &slot, &link);
                }
            }
        }
        Target::Mesh { .. } => weights_tools(app, ui),
    }
}

fn weights_tools(app: &mut EditorApp, ui: &mut egui::Ui) {
    let Some((slot, Attachment::Mesh(mesh))) = steps::selected_attachment(app) else { return };
    let name = slot.name.clone();
    let picked = crate::viewport::picked(app, &app.pose()).map(|(_, p)| p).unwrap_or_default();
    let mut multi: Option<Vec<Op>> = None;
    let mut asset = None;

    // The main action first: let the editor set the weights.
    let id = ui.make_persistent_id("max_influences");
    let mut max: usize = ui.data_mut(|d| d.get_temp(id)).unwrap_or(4);
    let only: Option<Vec<usize>> = (!picked.is_empty()).then(|| picked.to_vec());
    let label = match &only {
        Some(v) => crate::tf!("Set weights automatically ({} picked)", v.len()),
        None => tr("Set weights automatically").to_string(),
    };
    let has_bones = app.doc.bones.len() > 1;
    if big_button(ui, Some(Icon::Weights), &label, has_bones, "Add some bones first (step 1): weights need more than one bone.") {
        if mesh.image.is_some() {
            asset = Some(crate::import::auto_weights(app, &name, &mesh.name, max, only.clone()));
        } else {
            multi = Some(vec![Op::AutoWeights { locked_bones: app.view.weight_locked.iter().cloned().collect(), slot: name.clone(), attachment: mesh.name.clone(), bones: None, max_influences: max, falloff: 2.0, vertices: only.clone(), skin: None }]);
        }
    }
    note(ui, tr("Weights are set from the bones that run through the image. Then check by bending a bone, and fix by painting."));
    ui.horizontal(|ui| {
        ui.label(tr("Bones per vertex"));
        ui.add(DragValue::new(&mut max).range(1..=8)).on_hover_text(tr("Most bones that may move one vertex"));
        if ui.button(tr("By distance instead")).on_hover_text(tr("Weights from the distance to each bone, ignoring the image")).clicked() {
            multi = Some(vec![Op::AutoWeights { slot: name.clone(), attachment: mesh.name.clone(), skin: app.skin_of(&name, &mesh.name), bones: None, vertices: only.clone(), max_influences: max, falloff: 3.0, locked_bones: app.view.weight_locked.iter().cloned().collect() }]);
        }
    });
    ui.data_mut(|d| d.insert_temp(id, max));

    theme::section(ui, tr("Bound bones and painting"));
    if let Some(o) = weights_panel(app, ui, &name, &mesh, &slot.bone, &picked) {
        multi = Some(vec![o]);
    }
    theme::section(ui, tr("More tools"));
    ui.horizontal_wrapped(|ui| {
        if ui.button(tr("Transfer weights…")).on_hover_text(tr("Copy the weights of another mesh onto this one")).clicked() {
            crate::dialogs::weight_options(app, crate::dialogs::WeightAction::Transfer);
        }
        if ui.button(tr("Mirror weights…")).on_hover_text(tr("Copy the weights of one side to the other side")).clicked() {
            crate::dialogs::weight_options(app, crate::dialogs::WeightAction::Mirror);
        }
    });
    note(ui, tr("Transfer copies weights from another mesh (clothing over a body). Mirror copies left to right."));
    if let Some(o) = asset {
        app.edit_any(vec![skel2d_rig::AnyOp::Asset(o)]);
    }
    if let Some(ops) = multi
        && app.edit(ops)
    {
        app.status = tr("Weights updated · Ctrl+Z to undo").into();
    }
}

// ------------------------------------------------------------- step 5: animation

fn animate_panel(app: &mut EditorApp, ui: &mut egui::Ui) {
    if app.playback.anim.is_none() {
        ui.add_space(8.0);
        ui.label(RichText::new(tr("No animation yet")).strong().size(theme::SIZE_HEADING));
        body(ui, tr("Create an animation to record keys. Everything you change here is stored as keys over time."));
        ui.add_space(6.0);
        if big_button(ui, Some(Icon::Animation), tr("New animation"), true, "") {
            crate::dialogs::new_animation(app);
        }
        return;
    }
    let slot = app.selection.slot.clone().and_then(|s| app.doc.slot(&s).cloned());
    let bone = app.selection.bone.clone().and_then(|b| app.doc.bone(&b).cloned());
    let ik = app.selection.ik.clone().and_then(|n| app.doc.ik.iter().find(|c| c.name == n).cloned());
    let physics = app.selection.physics.clone().and_then(|n| app.doc.physics_constraint(&n).cloned());
    let transform = app.selection.transform.clone().and_then(|n| app.doc.transforms.iter().find(|c| c.name == n).cloned());
    animate_inspector(app, ui, (transform, ik, physics, slot, bone));
    // Draw order and events belong to the animation, not to a pick; they stay open until folded.
    super::animextra::draw_order_group(app, ui, true);
    super::animextra::events_group(app, ui, true);
}

type Picked = (Option<skel2d_core::TransformConstraint>, Option<IkConstraint>, Option<skel2d_core::PhysicsConstraint>, Option<Slot>, Option<Bone>);

fn animate_inspector(app: &mut EditorApp, ui: &mut egui::Ui, (transform, ik, physics, slot, bone): Picked) {
    if let Some(c) = transform {
        super::transform::transform_properties(app, ui, &c);
    } else if let Some(ik) = &ik {
        ik_properties(app, ui, ik);
    } else if let Some(c) = &physics {
        physics::physics_properties(app, ui, c);
    } else if let Some(slot) = &slot {
        slot_animate_panel(app, ui, slot);
    } else if let Some(bone) = &bone {
        bone_properties(app, ui, bone);
    } else {
        nothing_picked(ui, "Pick something to animate", "Pick a bone in the list or the viewport, then move the time marker and change it. Pick a slot to switch its image at a time (blinking, mouth shapes).");
    }
}

/// A slot in the animation step: its image cards (a click records a key) and its tint.
fn slot_animate_panel(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot) {
    let Some(anim) = app.playback.anim.clone() else { return };
    let t = app.playback.time;
    let name = slot.name.clone();
    let context = crate::tf!("on bone {}", slot.bone);
    inspector_header_ex(ui, Icon::Slot, theme::SLOT, tr("Slot"), &format!("slot_name_{name}"), &name, &context, false, true);
    let tl = app.doc.animations.get(&anim).and_then(|a| a.slots.get(&name)).cloned().unwrap_or_default();
    let here = |time: f32| (time - t).abs() < TIME_EPS;
    // Changed but not keyed yet (Auto Key off): what the pose and the tint show include it.
    let held = |color: bool| app.pending_other.iter().any(|o| matches!(o, Op::KeySlot(k) if k.slot == name && k.color.is_some() == color));
    let state = |keys: bool, here_key: bool, changed: bool| match (changed, here_key, keys) {
        (true, ..) => KeyState::Changed,
        (_, true, _) => KeyState::Keyed,
        (_, _, true) => KeyState::Interpolated,
        _ => KeyState::None,
    };
    let att_state = state(!tl.attachment.is_empty(), tl.attachment.iter().any(|k| here(k.time)), held(false));
    let color_state = state(!tl.color.is_empty(), tl.color.iter().any(|k| here(k.time)), held(true));
    let shown_tl = app.shown_doc().animations.get(&anim).and_then(|a| a.slots.get(&name)).cloned().unwrap_or_default();
    theme::section(ui, &crate::tf!("Image at {} s", format!("{t:.2}")));
    note(ui, tr("Blinking: put an open-eye and a closed-eye image in the eye slot (step 2). Here, press the closed eye at the time it should close, and the open eye right after."));
    ui.horizontal(|ui| {
        if key_button(ui, att_state) {
            key_slot_current(app, &anim, &name);
        }
        note(ui, tr("Key the image shown now"));
    });
    ui.add_space(4.0);
    image_cards(app, ui, slot, CardMode::Animate);
    let tint = sample(&shown_tl.color, t).unwrap_or(Color::WHITE);
    theme::group(ui, "slot_tint", tr("Tint"), &format!("{t:.2}s"), false, |ui| {
        note(ui, tr("Multiplied with the setup color; alpha fades the slot."));
        ui.horizontal(|ui| {
            let key = |color: Color| Op::KeySlot(skel2d_core::ops::KeySlot { animation: anim.clone(), slot: name.clone(), time: t, attachment: None, reset: false, color: Some(color), curve: None, curves: None });
            if key_button(ui, color_state) {
                app.edit_keyed(vec![key(tint)]);
            }
            let [r, g, b, a] = tint.to_rgba8();
            let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
            if ui.color_edit_button_srgba(&mut c).changed() {
                let [r, g, b, a] = c.to_srgba_unmultiplied();
                let color = Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 };
                app.edit_coalesced(format!("slotcolor:{anim}:{name}:{}", (t * 10_000.0).round() as i64), vec![key(color)]);
            }
            theme::hint(ui, &format!("#{r:02X}{g:02X}{b:02X} · {}%", (a as f32 / 2.55).round()));
        });
    });
    ui.add_space(6.0);
    note(ui, tr("To add or rename images, go back to step 2."));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use skel2d_core::{Document, Mesh, MeshVertex};

    /// Text drawn in one frame, in paint order.
    pub(crate) fn texts(out: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, v: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => v.push(t.galley.job.text.clone()),
                egui::Shape::Vec(list) => list.iter().for_each(|s| walk(s, v)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut v));
        v
    }

    /// Runs `f` for a few frames in a 1280x800 window and returns the last frame.
    pub(crate) fn frames(app: &mut EditorApp, n: usize, mut f: impl FnMut(&mut EditorApp, &mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        run_frames(&ctx, app, n, &mut f, Vec::new())
    }

    pub(crate) fn run_frames(ctx: &egui::Context, app: &mut EditorApp, n: usize, f: &mut dyn FnMut(&mut EditorApp, &mut egui::Ui), events: Vec<egui::Event>) -> egui::FullOutput {
        let mut out = egui::FullOutput::default();
        for i in 0..n {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                events: if i + 1 == n { events.clone() } else { Vec::new() },
                ..Default::default()
            };
            out = ctx.run_ui(input, |ui| f(app, ui));
        }
        out
    }

    fn png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        tiny_skia::Pixmap::new(w, h).unwrap().save_png(&path).unwrap();
        path
    }

    fn region(name: &str, image: &str) -> Attachment {
        Attachment::Region(Region { name: name.into(), image: Some(image.into()), x: 5.0, y: 2.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0, width: 10.0, height: 10.0 })
    }

    fn mesh(name: &str) -> Attachment {
        let v = |x: f32, y: f32| MeshVertex { x, y, u: x, v: y, weights: Vec::new() };
        Attachment::Mesh(Mesh { bones: Vec::new(), hull: 3, edges: Vec::new(), name: name.into(), image: None, vertices: vec![v(0.0, 0.0), v(0.0, 1.0), v(1.0, 0.0)], triangles: vec![[0, 1, 2]] })
    }

    /// Root + `arm`, slot `eye` with picture `open`, slot `mouth` showing a mesh,
    /// slot `cloth` showing a shared mesh of it, and a one second animation.
    pub(crate) fn fixture(tag: &str) -> (EditorApp, PathBuf) {
        let dir = std::env::temp_dir().join(format!("skel2d_stepui_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        png(&dir, "open.png", 8, 6);
        let slot = |name: &str, shown: &str, atts: Vec<Attachment>| Slot { name: name.into(), bone: "arm".into(), color: Color::WHITE, blend: Default::default(), clip: None, attachment: Some(shown.to_string()), attachments: atts };
        let doc = skel2d_core::apply(&Document::with_root("t"), &[
            Op::AddBone({ let mut b = Bone::new("arm"); b.parent = Some("root".into()); b.length = 30.0; b }),
            Op::AddSlot { slot: slot("eye", "open", vec![region("open", "open.png")]), index: None },
            Op::AddSlot { slot: slot("mouth", "m", vec![mesh("m")]), index: None },
            Op::AddSlot { slot: slot("cloth", "l", vec![mesh("cm"), Attachment::LinkedMesh(skel2d_core::LinkedMesh { name: "l".into(), image: None, parent: "cm".into(), parent_skin: None, timelines: true })]), index: None },
            Op::AddAnimation { name: "blink".into(), duration: Some(1.0) },
        ]).unwrap();
        (EditorApp::test_app(doc, dir.join("t.skel.json")), dir)
    }

    #[test]
    fn adding_images_makes_unique_named_ops_that_round_trip_through_json() {
        let (mut app, dir) = fixture("add");
        let files = [png(&dir, "closed.png", 8, 6), png(&dir, "sub/closed.png", 4, 4), png(&dir, "open.png", 8, 6)];
        let before = app.doc.clone();
        let ops = add_image_ops(&app, "eye", &files).unwrap();
        assert_eq!(ops.len(), 3, "one SetAttachment per file, the slot already shows an image");
        let names: Vec<&str> = ops.iter().map(|o| match o { Op::SetAttachment { attachment, .. } => attachment.name(), _ => "?" }).collect();
        assert_eq!(names, ["closed", "closed_2", "open_2"], "names come from the files and stay unique, also against the slot's existing images");
        for (op, size) in ops.iter().zip([[8.0, 6.0], [4.0, 4.0], [8.0, 6.0]]) {
            let Op::SetAttachment { attachment: Attachment::Region(r), .. } = op else { panic!("pictures expected") };
            assert_eq!([r.width, r.height], size, "sized from the PNG");
            assert_eq!((r.x, r.y), (5.0, 2.0), "placed where the shown image is");
        }
        assert!(add_images(&mut app, "eye", &files));
        let slot = app.doc.slot("eye").unwrap();
        assert_eq!(slot.attachments.len(), 4, "several images in one slot");
        assert_eq!(slot.attachment.as_deref(), Some("open"), "the shown image does not change");
        assert_eq!(app.selection.attachment.as_deref(), Some("open_2"));
        // The same ops reach the same document through the JSON the CLI, HTTP and MCP use.
        let json = serde_json::to_string(&ops).unwrap();
        let parsed = skel2d_rig::parse_ops(&json).unwrap();
        let report = skel2d_rig::apply_any(&before, &dir, &parsed).unwrap();
        assert_eq!(report.document, app.doc, "editor and API agree");
        app.undo();
        assert_eq!(app.doc, before, "one undo step");
    }

    #[test]
    fn a_slot_that_shows_nothing_shows_the_first_new_image() {
        let (mut app, dir) = fixture("empty");
        app.edit(vec![Op::SetSlot(SlotPatch { name: "eye".into(), attachment: Some(None), ..Default::default() })]);
        assert!(add_images(&mut app, "eye", &[png(&dir, "a.png", 3, 3), png(&dir, "b.png", 3, 3)]));
        assert_eq!(app.doc.slot("eye").unwrap().attachment.as_deref(), Some("a"));
        assert!(add_image_ops(&app, "missing", &[]).is_err());
        assert!(add_image_ops(&app, "eye", &[dir.join("nope.png")]).is_err(), "unreadable files are reported, nothing is half added");
    }

    #[test]
    fn image_card_click_in_animation_keys_the_slot_at_the_playhead() {
        let (mut app, dir) = fixture("card");
        add_images(&mut app, "eye", &[png(&dir, "closed.png", 8, 6)]);
        app.set_step(Step::Animate);
        app.playback.anim = Some("blink".into());
        app.playback.time = 0.5;
        assert!(key_slot_image(&mut app, "eye", Some("closed")));
        let keys = &app.doc.animations["blink"].slots["eye"].attachment;
        assert_eq!(keys.len(), 1);
        assert!((keys[0].time - 0.5).abs() < 1e-4);
        assert_eq!(keys[0].name.as_deref(), Some("closed"));
        // The hide card keys "no image".
        app.playback.time = 0.8;
        assert!(key_slot_image(&mut app, "eye", None));
        assert_eq!(app.doc.animations["blink"].slots["eye"].attachment[1].name, None);
        // The rig itself is untouched.
        assert_eq!(app.doc.slot("eye").unwrap().attachment.as_deref(), Some("open"));
        // Without an animation there is nothing to key.
        app.playback.anim = None;
        app.step = Step::Images;
        assert!(!key_slot_image(&mut app, "eye", Some("open")));
    }

    #[test]
    fn clicking_a_card_with_the_pointer_keys_it_in_the_animation_step() {
        let (mut app, dir) = fixture("click");
        add_images(&mut app, "eye", &[png(&dir, "closed.png", 8, 6)]);
        app.set_step(Step::Animate);
        app.playback.anim = Some("blink".into());
        app.playback.time = 0.25;
        let slot = app.doc.slot("eye").unwrap().clone();
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        let mut draw = |app: &mut EditorApp, ui: &mut egui::Ui| {
            egui::Area::new(egui::Id::unique("cards")).fixed_pos(egui::pos2(100.0, 100.0)).show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                image_cards(app, ui, &slot, CardMode::Animate);
            });
        };
        run_frames(&ctx, &mut app, 3, &mut draw, Vec::new());
        // The second card ("closed") sits one card height (+ gap) below the first.
        let at = egui::pos2(160.0, 100.0 + (theme::THUMB + 12.0 + 4.0) + 10.0);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        run_frames(&ctx, &mut app, 1, &mut draw, vec![egui::Event::PointerMoved(at)]);
        run_frames(&ctx, &mut app, 1, &mut draw, vec![button(true)]);
        run_frames(&ctx, &mut app, 1, &mut draw, vec![button(false)]);
        let keys = &app.doc.animations["blink"].slots["eye"].attachment;
        assert_eq!(keys.len(), 1, "one click, one key");
        assert_eq!(keys[0].name.as_deref(), Some("closed"));
        assert!((keys[0].time - 0.25).abs() < 1e-4);
    }

    #[test]
    fn status_text_says_what_the_picked_image_is() {
        let (mut app, _dir) = fixture("status");
        use crate::steps::{mesh_status, target, weights_status};
        assert_eq!(target(&app), Target::None);
        assert_eq!(mesh_status(&target(&app)), tr("Pick an image in the list on the left."));
        app.selection.slot = Some("eye".into());
        assert_eq!(target(&app), Target::Picture { slot: "eye".into(), image: "open".into() });
        assert_eq!(mesh_status(&target(&app)), tr("Not a mesh yet. Turn it into a mesh to be able to bend the image."));
        assert_eq!(weights_status(&app, &target(&app)), tr("Not a mesh yet. Make it a mesh first (step 3)."));
        app.selection.slot = Some("mouth".into());
        assert!(matches!(target(&app), Target::Mesh { vertices: 3, triangles: 1, .. }));
        assert_eq!(mesh_status(&target(&app)), crate::tf!("Mesh with {} vertices and {} triangles.", 3, 1));
        assert_eq!(weights_status(&app, &target(&app)), crate::tf!("No weights: the whole image follows bone {}.", "arm"));
        app.selection.slot = Some("cloth".into());
        assert!(matches!(target(&app), Target::Shared { .. }));
        assert_eq!(mesh_status(&target(&app)), crate::tf!("Uses the shape of its parent mesh \"{}\". Edit the parent to change it.", "cm"));
        assert_eq!(steps::target_path(&target(&app)).as_deref(), Some("cloth › l"));
    }

    #[test]
    fn the_root_row_refuses_delete_and_rename_and_explains_why() {
        let (mut app, _dir) = fixture("root");
        let before = app.doc.clone();
        crate::dialogs::delete(&mut app, crate::dialogs::Item::Bone("root".into()));
        assert_eq!(app.status, tr(ROOT_REASON));
        assert!(matches!(app.dialog, crate::app::Dialog::None), "no confirmation dialog for a bone that cannot go");
        app.status.clear();
        crate::dialogs::rename(&mut app, crate::dialogs::Item::Bone("root".into()));
        assert_eq!(app.status, tr(ROOT_REASON));
        assert!(matches!(app.dialog, crate::app::Dialog::None), "no rename dialog");
        reparent_bone(&mut app, "root", Some("arm".into()));
        reparent_bone(&mut app, "arm", None);
        assert_eq!(app.doc, before, "nothing changed, nothing left without a parent");
        // Dragging cannot make a root or move the root.
        assert_eq!(drop_action(&app.doc, &TreeDrag::Bone("arm".into()), &DropTarget::Bone(None)), None);
        assert_eq!(drop_action(&app.doc, &TreeDrag::Bone("root".into()), &DropTarget::Bone(Some("arm".into()))), None);
        // The panel shows the reason and the locked state next to the disabled actions.
        app.selection.bone = Some("root".into());
        let out = frames(&mut app, 2, |app, ui| bones_panel(app, ui));
        let t = texts(&out);
        assert!(t.iter().any(|s| s.contains(tr(ROOT_REASON))), "reason is printed in the panel: {t:?}");
        assert!(t.iter().any(|s| s == tr("Delete bone (with children)")));
        app.selection.bone = Some("arm".into());
        let out = frames(&mut app, 2, |app, ui| bones_panel(app, ui));
        assert!(!texts(&out).iter().any(|s| s.contains(tr(ROOT_REASON))), "other bones carry no root notice");
    }

    #[test]
    fn disabled_actions_print_their_reason() {
        let (mut app, _dir) = fixture("reason");
        app.set_step(Step::Mesh);
        app.selection.slot = Some("mouth".into());
        let out = frames(&mut app, 2, |app, ui| mesh_panel(app, ui));
        let t = texts(&out);
        assert!(t.iter().any(|s| s == tr("Delete picked vertices")), "button is there: {t:?}");
        assert!(t.iter().any(|s| s == tr("To pick vertices: click one, or drag a box around several in the viewport.")), "and says why it is off: {t:?}");
        assert!(t.iter().any(|s| s == tr("Click an orange fold line in the viewport first (use the Fold line mode).")));
        // Weights step: enabled with two bones, so no reason is printed.
        app.set_step(Step::Weights);
        let out = frames(&mut app, 2, |app, ui| weights_step_panel(app, ui));
        let t = texts(&out);
        assert!(t.iter().any(|s| s == tr("Set weights automatically")), "{t:?}");
        assert!(!t.iter().any(|s| s == tr("Add some bones first (step 1): weights need more than one bone.")));
        // With the root only, the main weights button is off and says what to do.
        let mut solo = EditorApp::test_app(Document::with_root("solo"), std::env::temp_dir().join("skel2d_solo.skel.json"));
        solo.edit(vec![Op::AddSlot { slot: Slot { name: "m".into(), bone: "root".into(), color: Color::WHITE, blend: Default::default(), clip: None, attachment: Some("m".into()), attachments: vec![mesh("m")] }, index: None }]);
        solo.set_step(Step::Weights);
        solo.selection.slot = Some("m".into());
        let out = frames(&mut solo, 2, |app, ui| weights_step_panel(app, ui));
        assert!(texts(&out).iter().any(|s| s == tr("Add some bones first (step 1): weights need more than one bone.")), "{:?}", texts(&out));
        // The helper itself: disabled => reason shown, enabled => not.
        let out = frames(&mut app, 2, |_, ui| {
            assert!(!action(ui, "Delete", false, "Because it is locked."));
        });
        assert!(texts(&out).iter().any(|s| s == tr("Because it is locked.")));
        let out = frames(&mut app, 2, |_, ui| {
            action(ui, "Delete", true, "Because it is locked.");
        });
        assert!(!texts(&out).iter().any(|s| s == tr("Because it is locked.")));
    }

    #[test]
    fn dropping_pngs_on_a_picked_slot_asks_where_they_go() {
        let (mut app, dir) = fixture("drop");
        let files = vec![png(&dir, "d1.png", 4, 4), png(&dir, "d2.png", 4, 4)];
        // Nothing picked: new slots, no question.
        crate::import::dropped_images(&mut app, files.clone(), [0.0, 0.0], None);
        assert!(matches!(app.dialog, crate::app::Dialog::None));
        assert_eq!(app.doc.slots.len(), 5, "one new slot per file");
        // A slot picked: the dialog asks, and nothing is added yet.
        app.selection.slot = Some("eye".into());
        let slots = app.doc.slots.len();
        crate::import::dropped_images(&mut app, files.clone(), [0.0, 0.0], None);
        let crate::app::Dialog::DropChoice(p) = std::mem::replace(&mut app.dialog, crate::app::Dialog::None) else { panic!("a choice dialog is expected") };
        assert_eq!((p.slot.as_str(), p.files.len(), app.doc.slots.len()), ("eye", 2, slots));
        // "This slot" adds both as images of the slot.
        assert!(add_images(&mut app, &p.slot, &p.files));
        assert_eq!(app.doc.slots.len(), slots);
        assert_eq!(app.doc.slot("eye").unwrap().attachments.len(), 3);
    }

    #[test]
    fn every_step_screen_renders_for_every_kind_of_selection() {
        let (mut app, _dir) = fixture("render");
        for step in Step::ALL {
            app.set_step(step);
            for slot in [None, Some("eye"), Some("mouth"), Some("cloth")] {
                for bone in [None, Some("root"), Some("arm")] {
                    app.selection = crate::app::Selection { slot: slot.map(String::from), bone: bone.map(String::from), ..Default::default() };
                    let out = frames(&mut app, 2, |app, ui| {
                        step_bar(app, ui);
                        tools(app, ui);
                        side(app, ui);
                    });
                    assert!(!texts(&out).is_empty(), "{step:?} {slot:?} {bone:?}");
                }
            }
        }
    }
}
