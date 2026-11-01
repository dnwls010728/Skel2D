//! Bottom panel: animation selection, playback, keying and a dope sheet with
//! key selection, retiming, copy/paste and a bezier curve editor.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Color32, DragValue, Event, PointerButton, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use skel2d_core::ops::{KeyBone, KeySlot, Timeline};
use skel2d_core::pose::sample;
use skel2d_core::{Animation, Color, Curve, Document, IkKey, Op};

use crate::app::EditorApp;
use crate::i18n::tr;
use crate::theme::{self, ACCENT, ANIMATE, BONE, IK, PHYSICS, SIZE_SMALL, SLOT, TEXT, TEXT_DIM};
use skel2d_core::ops::PhysicsProperty;

const ROW_H: f32 = 26.0;
const LABEL_W: f32 = 160.0;
const RULER_H: f32 = 38.0;
const EPS: f32 = skel2d_core::ops::TIME_EPSILON;

const HINT: &str = "Drag keys to retime (Alt: no snap, Ctrl+Shift: copy) · double-click a key to delete it · click a row to select its keys · arrow opens a row · Ctrl+A select all · K key edited · Ctrl+Shift+L key shown · Ctrl+wheel zoom · , . step frames · Del delete · right-click for key actions";

/// What a dope sheet row belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Bone(String),
    Slot(String),
    Ik(String),
    Transform(String),
    /// A physics constraint ("" = the timeline for all of them) and one of its rows:
    /// an index into PHYSICS_ROWS, or PHYSICS_RESET.
    Physics(String, u8),
    /// The draw order timeline of the animation.
    DrawOrder,
    /// The keys of one event.
    Event(String),
    /// The overview row: every key of the animation.
    All,
    /// One property row of a bone. `Bone` itself is the collapsed row.
    BoneProp(String, BoneProp),
    /// One property row of a slot. `Slot` itself is the collapsed row.
    SlotProp(String, SlotProp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BoneProp {
    Rotate,
    Translate,
    Scale,
    Shear,
}

const BONE_PROPS: [BoneProp; 4] = [BoneProp::Rotate, BoneProp::Translate, BoneProp::Scale, BoneProp::Shear];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SlotProp {
    Attachment,
    Color,
    /// The deformation of a mesh attachment: owning skin ("default" for none) and attachment name.
    Deform(String, String),
}

fn bone_prop_name(p: BoneProp) -> &'static str {
    match p {
        BoneProp::Rotate => "Rotate",
        BoneProp::Translate => "Translate",
        BoneProp::Scale => "Scale",
        BoneProp::Shear => "Shear",
    }
}

/// Whether a key of `row` is covered by the selected `sel` (same row, or an
/// aggregate row above it).
fn covers(sel: &Target, row: &Target) -> bool {
    match (sel, row) {
        (a, b) if a == b => true,
        (Target::All, _) => true,
        (Target::Bone(a), Target::BoneProp(b, _)) | (Target::Slot(a), Target::SlotProp(b, _)) => a == b,
        _ => false,
    }
}

/// Whether the key shown on `row` belongs to a selection that includes `sel` (either direction).
fn related(sel: &Target, row: &Target) -> bool {
    covers(sel, row) || covers(row, sel)
}

/// Physics rows in the order of PhysicsTimelines::property_keys.
const PHYSICS_ROWS: [PhysicsProperty; 7] = [
    PhysicsProperty::Inertia, PhysicsProperty::Strength, PhysicsProperty::Damping, PhysicsProperty::Mass,
    PhysicsProperty::Wind, PhysicsProperty::Gravity, PhysicsProperty::Mix,
];
const PHYSICS_RESET: u8 = 7;

/// Row name shown in the dope sheet and its tooltip.
fn target_label(t: &Target) -> String {
    match t {
        Target::Bone(n) | Target::Slot(n) | Target::Ik(n) | Target::Transform(n) | Target::Event(n) => n.clone(),
        Target::DrawOrder => tr("Draw order").to_string(),
        Target::All => tr("Overview").to_string(),
        Target::BoneProp(b, p) => format!("{b} · {}", tr(bone_prop_name(*p))),
        Target::SlotProp(s, SlotProp::Attachment) => format!("{s} · {}", tr("Attachment")),
        Target::SlotProp(s, SlotProp::Color) => format!("{s} · {}", tr("Color")),
        Target::SlotProp(s, SlotProp::Deform(skin, att)) => prop_deform_label(s, skin, att),
        Target::Physics(n, row) => {
            let who = if n.is_empty() { tr("All physics").to_string() } else { n.clone() };
            let what = PHYSICS_ROWS.get(*row as usize).map_or("reset", |p| physics_row_name(*p));
            format!("{who} · {}", tr(what))
        }
    }
}

fn prop_deform_label(slot: &str, skin: &str, att: &str) -> String {
    let who = if skin == "default" { att.to_string() } else { format!("{att} ({skin})") };
    format!("{slot} · {} {who}", tr("Deform"))
}

fn physics_row_name(p: PhysicsProperty) -> &'static str {
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

fn physics_keys(t: &skel2d_core::PhysicsTimelines, row: u8) -> &[skel2d_core::Key<f32>] {
    t.property_keys().get(row as usize).map_or(&[], |(_, k)| *k)
}

/// A key position on one row; all timelines of the row at that time move together.
/// Time is quantized (1e-4 s) so it can live in an ordered set.
type KeyId = (Target, i64);

fn tq(t: f32) -> i64 {
    (t * 10_000.0).round() as i64
}

fn from_q(q: i64) -> f32 {
    q as f32 / 10_000.0
}

/// One timeline value at a time, with everything needed to re-create it.
#[derive(Clone)]
enum KeyVal {
    Rotate(f32, Curve),
    // The last field holds per-channel curves, empty when `Curve` covers them.
    Translate([f32; 2], Curve, Vec<Curve>),
    Scale([f32; 2], Curve, Vec<Curve>),
    Shear([f32; 2], Curve, Vec<Curve>),
    Attach(Option<String>, bool),
    Color(Color, Curve, Vec<Curve>),
    /// All channels of an IK key, its curve and per-channel curves (`[mix, softness]`, empty when `Curve` covers them).
    Ik(IkKey, Curve, Vec<Curve>),
    /// Slots back to front, or `None` for the setup order.
    DrawOrder(Option<Vec<String>>),
    /// An event key's overrides; `None` follows the event's defaults.
    Event { int: Option<i32>, float: Option<f32>, string: Option<String> },
    Transform(skel2d_core::TransformMix, Curve),
    /// A physics property value (mass keys hold mass_inverse).
    Physics(f32, Curve),
    PhysicsReset,
    /// Mesh attachment name and its per-vertex offsets, with owning skin.
    Deform(String, Vec<[f32; 2]>, Curve, Option<String>),
}

#[derive(Clone)]
struct ClipKey {
    target: Target,
    dt: f32,
    val: KeyVal,
}

struct KeyDrag {
    /// Time of the key that was grabbed.
    anchor: f32,
    /// Offset between the pointer time and the anchor at grab.
    grab: f32,
    base: Document,
    delta: f32,
    pushed: bool,
    /// Ctrl+Shift drag: the originals stay and copies move.
    duplicate: bool,
}

struct BoxSel {
    origin: Pos2,
    add: bool,
}

/// Timeline view and key selection state.
#[derive(Default)]
pub struct TimelineState {
    /// Pixels per second (0 = not fitted yet).
    zoom: f32,
    /// Time at the left edge of the track.
    offset: f32,
    fit_anim: Option<String>,
    want_fit: bool,
    sel: BTreeSet<KeyId>,
    key_drag: Option<KeyDrag>,
    boxsel: Option<BoxSel>,
    clipboard: Vec<ClipKey>,
    /// The event picked in the Events group or by clicking its dope sheet row.
    pub event: Option<String>,
    /// Rows opened into their property rows. A view state: it is not part of the document.
    expanded: BTreeSet<Target>,
    /// Show the rows of every bone, not only those of the selection.
    pub show_all: bool,
    /// The pointer is over the timeline panel (Ctrl+A belongs to it then).
    hovered: bool,
    /// Where each row was drawn last frame (synthetic input tests aim at these).
    #[cfg(test)]
    rows: Vec<(Target, Rect)>,
}

impl TimelineState {
    /// Drops the key selection (e.g. when another animation is opened).
    pub fn clear_selection(&mut self) {
        self.sel.clear();
    }
}

#[derive(Clone, Copy)]
struct View {
    left: f32,
    zoom: f32,
    offset: f32,
}

impl View {
    fn x(&self, t: f32) -> f32 {
        self.left + (t - self.offset) * self.zoom
    }
    fn t(&self, x: f32) -> f32 {
        (x - self.left) / self.zoom + self.offset
    }
}

fn snap(t: f32, fps: f32) -> f32 {
    (t * fps).round() / fps
}

fn time_label(t: f32, fps: f32) -> String {
    format!("{t:.2} s · f{}", (t * fps).round() as i64)
}

fn side_hovered(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("side_hover_tree")).unwrap_or(false) || d.get_temp::<bool>(egui::Id::unique("side_hover_props")).unwrap_or(false) || d.get_temp::<bool>(egui::Id::unique("side_hover_tool")).unwrap_or(false))
}

pub fn ui(app: &mut EditorApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.timeline.hovered = ui.rect_contains_pointer(ui.max_rect());
    shortcuts(app, &ctx);
    if ui.rect_contains_pointer(ui.max_rect()) {
        app.hint = tr(HINT).into();
    }
    ui.spacing_mut().interact_size.y = 24.0;
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.spacing_mut().button_padding.y = 2.0;
    toolbar(app, ui);
    ui.separator();
    let Some(name) = app.playback.anim.clone().filter(|a| app.doc.animations.contains_key(a)) else {
        crate::theme::hint(ui, tr("No animation yet. Create one with New animation to start recording keys."));
        return;
    };
    if let Some((target, t)) = current_key(app, &name) {
        egui::Panel::right("tl_curve").exact_size(150.0).resizable(false).show_separator_line(true).show(ui, |ui| egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| curve_editor(app, ui, &name, &target, t)));
    }
    dope_sheet(app, ui, &name);
}

// ---------------------------------------------------------------- toolbar

/// Auto key beside Play: red while edits write keys, as a record button reads.
fn auto_key_button(app: &mut EditorApp, ui: &mut egui::Ui) {
    let on = app.settings.auto_key;
    let (fill, fg) = if on { (theme::ANIMATE, theme::ON_ACCENT) } else { (Color32::from_gray(58), theme::TEXT_DIM) };
    let text = egui::RichText::new(tr("Auto key")).strong().color(fg);
    let button = egui::Button::new((crate::icons::image(crate::icons::Icon::Key, 15.0).tint(fg), text)).fill(fill);
    let tip = crate::tf!("{} [R]", tr("On: every change writes a key. Off: changes stay unkeyed (orange key buttons) until you key them with K or a key button; moving the playhead discards them, with a notice."));
    if ui.add(button).on_hover_text(tip).clicked() {
        app.settings.auto_key = !on;
    }
}

fn toolbar(app: &mut EditorApp, ui: &mut egui::Ui) {
    let fps = app.settings.fps.max(1.0);
    ui.horizontal_wrapped(|ui| {
        let mut current = app.playback.anim.clone();
        let mut new_anim = false;
        let combo = egui::ComboBox::from_id_salt("anim")
            .selected_text(current.as_deref().unwrap_or(tr("(no animation)")))
            .width(130.0)
            .show_ui(ui, |ui| {
                for name in app.doc.animations.keys() {
                    ui.selectable_value(&mut current, Some(name.clone()), name);
                }
                ui.separator();
                if ui.selectable_label(false, format!("+ {}", tr("New animation"))).clicked() {
                    new_anim = true;
                    ui.close();
                }
            })
            .response
            .on_hover_text(tr("The animation you are editing. Right-click for new / rename / delete."));
        if current != app.playback.anim {
            app.playback.anim = current;
            app.playback.playing = false;
            app.playback.time = 0.0;
            app.timeline.sel.clear();
        }
        let anim_name = app.playback.anim.clone();
        // Its own popup id: a shared one would close the list on every left click.
        egui::Popup::context_menu(&combo).id(combo.id.with("context_menu")).show(|ui| {
            if ui.button(tr("New animation")).clicked() {
                new_anim = true;
                ui.close();
            }
            if let Some(name) = &anim_name {
                if ui.add(crate::icons::labeled(crate::icons::Icon::Rename, tr("Rename"))).clicked() {
                    crate::dialogs::rename(app, crate::dialogs::Item::Animation(name.clone()));
                    ui.close();
                }
                if ui.add(crate::icons::labeled(crate::icons::Icon::Duplicate, tr("Duplicate"))).clicked() {
                    crate::dialogs::duplicate_animation(app, name);
                    ui.close();
                }
                if ui.add(theme::danger_button(tr("Delete"))).clicked() {
                    crate::dialogs::delete(app, crate::dialogs::Item::Animation(name.clone()));
                    ui.close();
                }
            }
        });
        if new_anim { crate::dialogs::new_animation(app); }
        let anim_name = app.playback.anim.clone();

        if anim_name.is_some() || !app.playback.layers.is_empty() {
            ui.separator();
            let label = if app.playback.playing { tr("Pause") } else { tr("Play") };
            if ui
                .add(crate::icons::labeled(if app.playback.playing { crate::icons::Icon::Pause } else { crate::icons::Icon::Play }, egui::RichText::new(label).strong()).min_size(vec2(84.0, 0.0)))
                .on_hover_text(tr("Play / pause [Space] · step frame [, .] · go to start / end [Home / End]"))
                .clicked()
            {
                app.playback.playing = !app.playback.playing;
            }
        }
        if anim_name.is_some() {
            auto_key_button(app, ui);
        }
        let Some(name) = anim_name else {
            if !app.playback.layers.is_empty() {
                ui.label(tr("Time"));
                ui.add(DragValue::new(&mut app.playback.time).speed(0.01).range(0.0..=3600.0).suffix(" s"))
                    .on_hover_text(tr("Playhead time in seconds (drag, or type)"));
            }
            return;
        };
        ui.add(crate::icons::image(crate::icons::Icon::Loop, 16.0).tint(theme::TEXT_DIM));
        ui.checkbox(&mut app.playback.looping, tr("Loop")).on_hover_text(tr("Loop playback; off stops at the end"));
        let speed_label = |s: f32| format!("{s}×");
        egui::ComboBox::from_id_salt("speed")
            .selected_text(speed_label(app.playback.speed))
            .width(60.0)
            .show_ui(ui, |ui| {
                for s in [0.25_f32, 0.5, 1.0, 2.0] {
                    ui.selectable_value(&mut app.playback.speed, s, speed_label(s));
                }
            })
            .response
            .on_hover_text(tr("Playback speed"));

        ui.separator();
        let duration = app.animation().map_or(0.0, Animation::duration);
        ui.label(tr("Time")).on_hover_text(tr("Playhead position"));
        ui.add(DragValue::new(&mut app.playback.time).speed(0.01).range(0.0..=duration.max(0.0)).max_decimals(3).suffix(" s"))
            .on_hover_text(tr("Playhead time in seconds (drag, or type)"));
        let mut frame = (app.playback.time * fps).round() as i64;
        let last_frame = (duration * fps).round().max(0.0) as i64;
        if ui.add(DragValue::new(&mut frame).prefix("f").range(0..=last_frame))
            .on_hover_text(crate::tf!("Frame number at {} fps ({}) · type a frame and press Enter to move the playhead", fps, time_label(app.playback.time, fps)))
            .changed()
        {
            app.playback.time = (frame as f32 / fps).clamp(0.0, duration.max(0.0));
            app.playback.playing = false;
        }
        ui.checkbox(&mut app.timeline.show_all, tr("All rows"))
            .on_hover_text(tr("Show the rows of every bone. Off: only the selected bone's rows (and what touches it)"));
        ui.label(tr("Duration")).on_hover_text(tr("Animation length"));
        let mut d = duration;
        if ui.add(DragValue::new(&mut d).speed(0.01).range(0.0..=600.0).max_decimals(3).suffix(" s")).on_hover_text(tr("Animation length in seconds")).changed() {
            app.edit_coalesced(format!("anim:{name}:duration"), vec![Op::SetAnimation { name: name.clone(), rename: None, duration: Some(Some(d)) }]);
        }
    });

    ui.horizontal(|ui| {
        layers_menu(app, ui);
    });
}

#[derive(Clone)]
struct LayerDrag(String);

/// Highest track is at the top; evaluation starts at the bottom. Controls
/// operate by animation name, including while a drag changes the list order.
fn layers_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    let main = app.playback.anim.clone();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Play together with")).strong());
            if !app.playback.layers.is_empty() { theme::hint(ui, tr("Bottom first · drag to reorder")); }
            ui.menu_button(tr("Add animation"), |ui| {
                theme::hint(ui, tr("Add animations to play several together while you work (a preview only)"));
                let names: Vec<_> = app.doc.animations.keys()
                    .filter(|n| Some(*n) != main.as_ref() && !app.playback.layers.iter().any(|l| &l.anim == *n)).cloned().collect();
                if names.is_empty() { theme::hint(ui, tr("No other animations")); }
                for name in names {
                    if ui.button(&name).clicked() { app.playback.add_layer(&name); ui.close(); }
                }
            });
        });
        let mut action: Option<(String, Option<(String, bool)>)> = None;
        egui::ScrollArea::vertical().id_salt("preview_layers").max_height(theme::TRACK_LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
            let len = app.playback.layers.len();
            for (index, layer) in app.playback.layers.iter_mut().enumerate().rev() {
                let name = layer.anim.clone();
                let current = main.as_ref() == Some(&name);
                let row = ui.horizontal(|ui| {
                    ui.checkbox(&mut layer.enabled, "").on_hover_text(tr("Enable preview layer"));
                    let color = if layer.enabled && layer.alpha > 0.0 && !current { TEXT } else { TEXT_DIM };
                    let name_width = (ui.available_width() - theme::TRACK_CONTROLS_WIDTH).max(theme::TRACK_NAME_MIN_WIDTH);
                    let text = format!("{}  {}", index + 1, name);
                    let response = ui.add_sized([name_width, theme::ROW_H], egui::Label::new(egui::RichText::new(text).color(color)).truncate().sense(Sense::drag()));
                    response.dnd_set_drag_payload(LayerDrag(name.clone()));
                    response.on_hover_text(tr(if current { "This animation is being edited; its preview layer is paused" } else { "Drag up to apply later; drag down to apply earlier" }));
                    let mut percent = layer.alpha * 100.0;
                    ui.spacing_mut().slider_width = theme::TRACK_ALPHA_WIDTH;
                    if ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").fixed_decimals(0)).on_hover_text(tr("Track opacity")).changed() { layer.alpha = percent / 100.0; }
                    if ui.add_enabled(index + 1 < len, crate::icons::small(crate::icons::Icon::ArrowUp)).on_hover_text(tr("Move layer up (apply later)")).clicked() {
                        action = Some((name.clone(), Some((String::new(), true))));
                    }
                    if ui.add_enabled(index > 0, crate::icons::small(crate::icons::Icon::ArrowDown)).on_hover_text(tr("Move layer down (apply earlier)")).clicked() {
                        action = Some((name.clone(), Some((String::new(), false))));
                    }
                    if ui.add(crate::icons::small(crate::icons::Icon::Close)).on_hover_text(tr("Remove preview layer")).clicked() { action = Some((name.clone(), None)); }
                });
                if let Some(drag) = egui::DragAndDrop::payload::<LayerDrag>(ui.ctx())
                    && drag.0 != name
                    && let Some(pointer) = ui.ctx().pointer_latest_pos().filter(|p| row.response.rect.intersect(ui.clip_rect()).contains(*p))
                {
                    let above = pointer.y < row.response.rect.center().y;
                    let y = if above { row.response.rect.top() } else { row.response.rect.bottom() };
                    ui.painter().hline(row.response.rect.x_range(), y, Stroke::new(theme::TRACK_DROP_STROKE, ACCENT));
                    if ui.input(|i| i.pointer.any_released()) { action = Some((drag.0.clone(), Some((name, above)))); }
                }
            }
        });
        if let Some((name, target)) = action {
            match target {
                None => app.playback.layers.retain(|l| l.anim != name),
                Some((target, up)) if target.is_empty() => app.playback.move_layer(&name, up),
                Some((target, above)) => { app.playback.reorder_layer(&name, &target, above); }
            }
        }
        if let (Some(name), false) = (main, app.playback.layers.is_empty()) { theme::hint(ui, &crate::tf!("Track 0 · editing {}", name)); }
    });
}

/// The animation that keying acts on, or a status hint when there is none.
fn keying_anim(app: &mut EditorApp) -> Option<String> {
    let anim = app.playback.anim.clone().filter(|a| app.doc.animations.contains_key(a));
    if anim.is_none() {
        app.status = tr("Pick or create an animation first.").into();
    }
    anim
}

/// K, Key Edited: keys exactly what was changed and nothing else. With
/// nothing changed it keys the active tool's property of the selected bone (or
/// the selected slot's attachment) and says what to do otherwise.
pub fn key_edited(app: &mut EditorApp) -> bool {
    let Some(anim) = keying_anim(app) else { return false };
    if app.has_pending() {
        let (ops, n) = (app.pending_ops(), app.pending_count());
        let ok = app.edit_keyed(ops);
        if ok {
            app.status = if n == 1 { tr("Keyed 1 edited property").to_string() } else { crate::tf!("Keyed {} edited properties", n) };
        }
        return ok;
    }
    let leaf = if let Some(slot) = app.selection.slot.clone() {
        Some(Target::SlotProp(slot, SlotProp::Attachment))
    } else if let Some(bone) = app.selection.bone.clone() {
        match app.tool {
            crate::app::Tool::Translate => Some(Target::BoneProp(bone, BoneProp::Translate)),
            crate::app::Tool::Rotate => Some(Target::BoneProp(bone, BoneProp::Rotate)),
            crate::app::Tool::Scale => Some(Target::BoneProp(bone, BoneProp::Scale)),
            crate::app::Tool::Shear => Some(Target::BoneProp(bone, BoneProp::Shear)),
            _ => None,
        }
    } else {
        None
    };
    if let Some(leaf) = leaf {
        let ops = capture_ops(app, &anim, &[leaf]);
        return app.edit_keyed(ops);
    }
    if let Some(ik) = app.selection.ik.clone() {
        let ops = capture_ops(app, &anim, &[Target::Ik(ik)]);
        return app.edit_keyed(ops);
    }
    app.status = tr("Nothing changed to key. Pick the Move, Rotate or Scale tool to key that property, or press Ctrl+Shift+L to key everything shown.").into();
    false
}

/// Keys one transform property of the selected bone with its value shown now
/// (the key button beside the tool's values, and the palette).
pub fn key_bone_property(app: &mut EditorApp, prop: KeyProp) -> bool {
    let Some(anim) = keying_anim(app) else { return false };
    let Some(bone) = app.selection.bone.clone() else {
        app.status = tr("Select a bone first.").into();
        return false;
    };
    let p = match prop {
        KeyProp::Rotate => BoneProp::Rotate,
        KeyProp::Translate => BoneProp::Translate,
        KeyProp::Scale => BoneProp::Scale,
        KeyProp::Shear => BoneProp::Shear,
    };
    let ops = capture_ops(app, &anim, &[Target::BoneProp(bone, p)]);
    app.edit_keyed(ops)
}

/// A bone property for the key buttons and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProp {
    Rotate,
    Translate,
    Scale,
    Shear,
}

/// Keys the selected slot's tint with the value shown now.
pub fn key_slot_color(app: &mut EditorApp) -> bool {
    let Some(slot) = app.selection.slot.clone() else {
        app.status = tr("Select a slot first.").into();
        return false;
    };
    key_row(app, &KeyRow::SlotColor(slot))
}

/// What a key button in the tree stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRow {
    /// All the transform properties of a bone.
    Bone(String),
    /// One transform property of a bone (the key button beside the tool's values).
    BoneProp(String, KeyProp),
    SlotAttachment(String),
    SlotColor(String),
    /// The deformation of a mesh attachment: slot and attachment.
    Deform(String, String),
    Ik(String),
    Transform(String),
    Physics(String),
}

fn key_row_leaves(app: &EditorApp, row: &KeyRow) -> Vec<Target> {
    match row {
        KeyRow::Bone(b) => BONE_PROPS.iter().map(|p| Target::BoneProp(b.clone(), *p)).collect(),
        KeyRow::BoneProp(b, p) => vec![Target::BoneProp(b.clone(), match p {
            KeyProp::Rotate => BoneProp::Rotate,
            KeyProp::Translate => BoneProp::Translate,
            KeyProp::Scale => BoneProp::Scale,
            KeyProp::Shear => BoneProp::Shear,
        })],
        KeyRow::SlotAttachment(s) => vec![Target::SlotProp(s.clone(), SlotProp::Attachment)],
        KeyRow::SlotColor(s) => vec![Target::SlotProp(s.clone(), SlotProp::Color)],
        KeyRow::Deform(s, att) => vec![Target::SlotProp(s.clone(), SlotProp::Deform(app.skin_of(s, att).unwrap_or_else(|| "default".into()), att.clone()))],
        KeyRow::Ik(n) => vec![Target::Ik(n.clone())],
        KeyRow::Transform(n) => vec![Target::Transform(n.clone())],
        KeyRow::Physics(n) => (0..PHYSICS_RESET).map(|row| Target::Physics(n.clone(), row)).collect(),
    }
}

/// Whether the property `leaf` holds an unkeyed change.
fn leaf_pending(app: &EditorApp, leaf: &Target) -> bool {
    match leaf {
        Target::BoneProp(b, p) => app.pending.get(b).is_some_and(|k| match p {
            BoneProp::Rotate => k.rotate.is_some(),
            BoneProp::Translate => k.translate.is_some(),
            BoneProp::Scale => k.scale.is_some(),
            BoneProp::Shear => k.shear.is_some(),
        }),
        Target::SlotProp(s, SlotProp::Attachment) => app.pending_other.iter().any(|o| matches!(o, Op::KeySlot(k) if k.slot == *s && k.color.is_none())),
        Target::SlotProp(s, SlotProp::Color) => app.pending_other.iter().any(|o| matches!(o, Op::KeySlot(k) if k.slot == *s && k.color.is_some())),
        Target::SlotProp(s, SlotProp::Deform(skin, att)) => app.pending_other.iter().any(|o| matches!(o, Op::KeyDeform { slot, attachment, skin: k, .. } if slot == s && attachment == att && k.as_deref().unwrap_or("default") == skin)),
        Target::Ik(n) => app.pending_other.iter().any(|o| matches!(o, Op::KeyIk { ik, .. } if ik == n)),
        Target::Transform(n) => app.pending_other.iter().any(|o| matches!(o, Op::KeyTransform { constraint, .. } if constraint == n)),
        Target::Physics(n, row) => PHYSICS_ROWS.get(*row as usize).is_some_and(|p| app.pending_other.iter().any(|o| matches!(o, Op::KeyPhysics { constraint, property, .. } if constraint == n && property == p))),
        _ => false,
    }
}

/// Key button state of a tree row at the playhead: changed beats keyed beats "between".
pub fn key_row_state(app: &EditorApp, row: &KeyRow) -> crate::panels::KeyState {
    use crate::panels::KeyState;
    let Some(a) = app.animation() else { return KeyState::None };
    let q = tq(app.playback.time);
    let leaves = key_row_leaves(app, row);
    if leaves.iter().any(|l| leaf_pending(app, l)) {
        KeyState::Changed
    } else if leaves.iter().any(|l| all_ids(a, l).contains(&q)) {
        KeyState::Keyed
    } else if leaves.iter().any(|l| !all_ids(a, l).is_empty()) {
        KeyState::Interpolated
    } else {
        KeyState::None
    }
}

/// A tree key button was clicked: key the row's properties with the values shown now.
/// A bone with unkeyed changes keys just those; otherwise the active tool's property, else all.
pub fn key_row(app: &mut EditorApp, row: &KeyRow) -> bool {
    let Some(anim) = keying_anim(app) else { return false };
    let mut leaves = key_row_leaves(app, row);
    if let KeyRow::Bone(b) = row {
        let changed: Vec<Target> = leaves.iter().filter(|l| leaf_pending(app, l)).cloned().collect();
        let tool = match app.tool {
            crate::app::Tool::Translate => Some(BoneProp::Translate),
            crate::app::Tool::Rotate => Some(BoneProp::Rotate),
            crate::app::Tool::Scale => Some(BoneProp::Scale),
            crate::app::Tool::Shear => Some(BoneProp::Shear),
            _ => None,
        };
        if !changed.is_empty() {
            leaves = changed;
        } else if let Some(p) = tool {
            leaves = vec![Target::BoneProp(b.clone(), p)];
        }
    }
    let ops = capture_ops(app, &anim, &leaves);
    if ops.is_empty() {
        return false;
    }
    app.edit_keyed(ops)
}

/// Ctrl+A from the command palette.
pub fn select_all_keys(app: &mut EditorApp) {
    if let Some(anim) = app.playback.anim.clone() {
        select_all(app, &anim);
    }
}

/// Opens every bone and slot row into its property rows, or closes them all.
pub fn toggle_expand_all(app: &mut EditorApp) {
    let Some(a) = app.animation().cloned() else { return };
    let rows: Vec<Target> = build_items(app, &a).into_iter().filter_map(|it| match it {
        Item::Row(t @ (Target::Bone(_) | Target::Slot(_)), _) => Some(t),
        _ => None,
    }).collect();
    if rows.iter().all(|t| app.timeline.expanded.contains(t)) {
        app.timeline.expanded.clear();
    } else {
        app.timeline.expanded.extend(rows);
    }
}

/// Ctrl+Shift+L, Key Shown: keys every property row the dope sheet shows at
/// the playhead with its current value (collapsed rows count all their properties).
pub fn key_shown(app: &mut EditorApp) -> bool {
    let Some(anim) = keying_anim(app) else { return false };
    let leaves = shown_leaves(app, &anim);
    let ops = capture_ops(app, &anim, &leaves);
    if ops.is_empty() {
        app.status = tr("Nothing is shown in the dope sheet to key.").into();
        return false;
    }
    let n = ops.len();
    let ok = app.edit_keyed(ops);
    if ok {
        app.status = crate::tf!("Keyed {} shown rows", n);
    }
    ok
}

/// The leaf targets of the rows the dope sheet shows now.
fn shown_leaves(app: &EditorApp, anim: &str) -> Vec<Target> {
    let Some(a) = app.doc.animations.get(anim) else { return Vec::new() };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in build_items(app, a) {
        let Item::Row(t, _) = item else { continue };
        for leaf in leaf_targets(a, &t) {
            let keyable = !matches!(leaf, Target::All | Target::DrawOrder | Target::Event(_)) && !matches!(&leaf, Target::Physics(_, row) if *row == PHYSICS_RESET);
            if keyable && seen.insert(leaf.clone()) {
                out.push(leaf);
            }
        }
    }
    out
}

/// The setup value of a physics property, or `None` when the constraint uses the shared value.
fn physics_setup(c: &skel2d_core::PhysicsConstraint, p: PhysicsProperty) -> Option<f32> {
    let (v, global) = match p {
        PhysicsProperty::Inertia => (c.inertia, c.inertia_global),
        PhysicsProperty::Strength => (c.strength, c.strength_global),
        PhysicsProperty::Damping => (c.damping, c.damping_global),
        PhysicsProperty::Mass => (c.mass_inverse, c.mass_global),
        PhysicsProperty::Wind => (c.wind, c.wind_global),
        PhysicsProperty::Gravity => (c.gravity, c.gravity_global),
        PhysicsProperty::Mix => (c.mix, c.mix_global),
    };
    (!global).then_some(v)
}

/// Ops that key `leaves` at the playhead with the values shown now (unkeyed changes included).
fn capture_ops(app: &EditorApp, anim: &str, leaves: &[Target]) -> Vec<Op> {
    let doc = app.shown_doc();
    let t = app.playback.time;
    let a = doc.animations.get(anim);
    let mut bones: BTreeMap<String, KeyBone> = BTreeMap::new();
    let mut ops = Vec::new();
    for leaf in leaves {
        match leaf {
            Target::BoneProp(b, p) => {
                let tl = a.and_then(|a| a.bones.get(b));
                let k = bones.entry(b.clone()).or_insert_with(|| KeyBone { animation: anim.to_string(), bone: b.clone(), time: t, rotate: None, translate: None, scale: None, shear: None, curve: None, curves: None });
                match p {
                    BoneProp::Rotate => k.rotate = Some(tl.and_then(|tl| sample(&tl.rotate, t)).unwrap_or(0.0)),
                    BoneProp::Translate => k.translate = Some(tl.and_then(|tl| sample(&tl.translate, t)).unwrap_or([0.0, 0.0])),
                    BoneProp::Scale => k.scale = Some(tl.and_then(|tl| sample(&tl.scale, t)).unwrap_or([1.0, 1.0])),
                    BoneProp::Shear => k.shear = Some(tl.and_then(|tl| sample(&tl.shear, t)).unwrap_or([0.0; 2])),
                }
            }
            Target::SlotProp(s, SlotProp::Attachment) => {
                let tl = a.and_then(|a| a.slots.get(s));
                let reset = tl.and_then(|tl| skel2d_core::pose::sample_attachment(&tl.attachment, t)).is_some_and(|k| k.reset);
                let shown = doc.slot_index(s).and_then(|i| app.pose().slots.get(i).map(|s| s.attachment.clone())).flatten();
                ops.push(Op::KeySlot(KeySlot { animation: anim.to_string(), slot: s.clone(), time: t, attachment: (!reset).then_some(shown), reset, color: None, curve: None, curves: None }));
            }
            Target::SlotProp(s, SlotProp::Color) => {
                let tint = a.and_then(|a| a.slots.get(s)).and_then(|tl| sample(&tl.color, t)).unwrap_or(Color::WHITE);
                ops.push(Op::KeySlot(KeySlot { animation: anim.to_string(), slot: s.clone(), time: t, attachment: None, reset: false, color: Some(tint), curve: None, curves: None }));
            }
            Target::SlotProp(s, SlotProp::Deform(skin, att)) => {
                let sampled = a.and_then(|a| a.deform.get(skin)).and_then(|m| m.get(s)).and_then(|m| m.get(att)).and_then(|keys| sample(keys, t));
                // Unkeyed: no offsets yet, so the key holds the mesh as it is.
                let offsets = sampled.or_else(|| {
                    let si = doc.slot_index(s)?;
                    let source = if skin == "default" { doc.slots[si].attachment(att) } else { doc.skin(skin).and_then(|k| k.attachment(s, att)) }?;
                    match doc.materialize_attachment(si, source)? {
                        skel2d_core::Attachment::Mesh(m) => Some(vec![[0.0, 0.0]; m.vertices.len()]),
                        _ => None,
                    }
                });
                if let Some(offsets) = offsets {
                    ops.push(Op::KeyDeform { animation: anim.to_string(), slot: s.clone(), attachment: att.clone(), skin: (skin != "default").then(|| skin.clone()), time: t, offsets, curve: None });
                }
            }
            Target::Ik(n) => {
                if let Some(ik) = doc.ik.iter().find(|i| &i.name == n) {
                    let now = crate::ikbend::ik_values(&doc, Some(anim), t, ik);
                    let (one, two) = (ik.bones.len() == 1, ik.bones.len() == 2);
                    ops.push(Op::KeyIk { animation: anim.to_string(), ik: n.clone(), time: t, mix: Some(now.mix), softness: two.then_some(now.softness), bend_positive: two.then_some(now.bend_positive), compress: one.then_some(now.compress), stretch: Some(now.stretch), curve: None, curves: None });
                }
            }
            Target::Transform(n) => {
                if let Some(c) = doc.transforms.iter().find(|c| &c.name == n) {
                    let mix = a.and_then(|a| a.transforms.get(n)).and_then(|keys| sample(keys, t)).unwrap_or_else(|| c.mix());
                    ops.push(Op::KeyTransform { animation: anim.to_string(), constraint: n.clone(), time: t, mix, curve: None });
                }
            }
            Target::Physics(n, row) => {
                if let Some(property) = PHYSICS_ROWS.get(*row as usize) {
                    let keyed = a.and_then(|a| a.physics.get(n)).and_then(|tl| sample(physics_keys(tl, *row), t));
                    // Without a key the setup value, unless the shared value is in use.
                    let value = keyed.or_else(|| doc.physics.iter().find(|c| &c.name == n).and_then(|c| physics_setup(c, *property)));
                    if let Some(value) = value {
                        ops.push(Op::KeyPhysics { animation: anim.to_string(), constraint: n.clone(), property: *property, time: t, value, curve: None });
                    }
                }
            }
            _ => {}
        }
    }
    let mut all: Vec<Op> = bones.into_values().map(Op::KeyBone).collect();
    all.extend(ops);
    all
}

/// Row targets (bone/ik) with keys at the playhead for the selected row.
fn keys_at_playhead(app: &EditorApp, anim: &str) -> Vec<KeyId> {
    let Some(a) = app.doc.animations.get(anim) else { return Vec::new() };
    let q = tq(app.playback.time);
    let mut out = Vec::new();
    for target in selected_targets(app) {
        if !vals_at(a, &target, q).is_empty() {
            out.push((target, q));
        }
    }
    out
}

fn selected_targets(app: &EditorApp) -> Vec<Target> {
    let mut v = Vec::new();
    if let Some(b) = &app.selection.bone {
        v.push(Target::Bone(b.clone()));
    }
    if let Some(s) = &app.selection.slot {
        v.push(Target::Slot(s.clone()));
    }
    if let Some(i) = &app.selection.ik {
        v.push(Target::Ik(i.clone()));
    }
    if let Some(t) = &app.selection.transform {
        v.push(Target::Transform(t.clone()));
    }
    if let Some(p) = &app.selection.physics {
        v.extend((0..=PHYSICS_RESET).map(|row| Target::Physics(p.clone(), row)));
    }
    if let Some(e) = &app.timeline.event {
        v.push(Target::Event(e.clone()));
    }
    v
}

// ---------------------------------------------------------------- key data

fn vals_at(a: &Animation, target: &Target, q: i64) -> Vec<KeyVal> {
    let mut out = Vec::new();
    match target {
        Target::All => {}
        Target::Bone(b) => {
            for p in BONE_PROPS {
                out.extend(vals_at(a, &Target::BoneProp(b.clone(), p), q));
            }
        }
        Target::BoneProp(b, p) => {
            if let Some(tl) = a.bones.get(b) {
                match p {
                    BoneProp::Rotate => out.extend(tl.rotate.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Rotate(k.value, k.curve))),
                    BoneProp::Translate => out.extend(tl.translate.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Translate(k.value, k.curve, k.curves.clone()))),
                    BoneProp::Scale => out.extend(tl.scale.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Scale(k.value, k.curve, k.curves.clone()))),
                    BoneProp::Shear => out.extend(tl.shear.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Shear(k.value, k.curve, k.curves.clone()))),
                }
            }
        }
        Target::Slot(s) => {
            out.extend(vals_at(a, &Target::SlotProp(s.clone(), SlotProp::Attachment), q));
            out.extend(vals_at(a, &Target::SlotProp(s.clone(), SlotProp::Color), q));
            for (skin, slots) in &a.deform {
                for att in slots.get(s).into_iter().flat_map(|m| m.keys()) {
                    out.extend(vals_at(a, &Target::SlotProp(s.clone(), SlotProp::Deform(skin.clone(), att.clone())), q));
                }
            }
        }
        Target::SlotProp(s, SlotProp::Attachment) => {
            if let Some(tl) = a.slots.get(s) {
                out.extend(tl.attachment.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Attach(k.name.clone(), k.reset)));
            }
        }
        Target::SlotProp(s, SlotProp::Color) => {
            if let Some(tl) = a.slots.get(s) {
                out.extend(tl.color.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Color(k.value, k.curve, k.curves.clone())));
            }
        }
        Target::SlotProp(s, SlotProp::Deform(skin, att)) => {
            if let Some(keys) = a.deform.get(skin).and_then(|slots| slots.get(s)).and_then(|atts| atts.get(att)) {
                out.extend(keys.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Deform(att.clone(), k.value.clone(), k.curve, (skin != "default").then(|| skin.clone()))));
            }
        }
        Target::Ik(n) => {
            if let Some(keys) = a.ik.get(n) {
                out.extend(keys.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Ik(k.value, k.curve, k.curves.clone())));
            }
        }
        Target::DrawOrder => {
            out.extend(a.draw_order.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::DrawOrder(k.order.clone())));
        }
        Target::Event(n) => {
            out.extend(a.events.iter().filter(|k| &k.name == n && tq(k.time) == q).map(|k| KeyVal::Event { int: k.int, float: k.float, string: k.string.clone() }));
        }
        Target::Transform(n) => {
            if let Some(keys) = a.transforms.get(n) {
                out.extend(keys.iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Transform(k.value, k.curve)));
            }
        }
        Target::Physics(n, row) => {
            if let Some(tl) = a.physics.get(n) {
                if *row == PHYSICS_RESET {
                    out.extend(tl.reset.iter().filter(|t| tq(**t) == q).map(|_| KeyVal::PhysicsReset));
                } else {
                    out.extend(physics_keys(tl, *row).iter().filter(|k| tq(k.time) == q).map(|k| KeyVal::Physics(k.value, k.curve)));
                }
            }
        }
    }
    out
}

fn key_op(anim: &str, target: &Target, time: f32, v: &KeyVal) -> Op {
    let channels = |curves: &Vec<Curve>| (!curves.is_empty()).then(|| curves.clone());
    let bone = |b: &String, rotate, translate, scale, curve, curves| {
        Op::KeyBone(KeyBone { animation: anim.to_string(), bone: b.clone(), time, rotate, translate, scale, shear: None, curve: Some(curve), curves })
    };
    match (target, v) {
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Rotate(x, c)) => bone(b, Some(*x), None, None, *c, None),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Translate(x, c, cs)) => bone(b, None, Some(*x), None, *c, channels(cs)),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Scale(x, c, cs)) => bone(b, None, None, Some(*x), *c, channels(cs)),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Shear(x, c, cs)) => Op::KeyBone(KeyBone { animation: anim.to_string(), bone: b.clone(), time, shear: Some(*x), rotate: None, translate: None, scale: None, curve: Some(*c), curves: channels(cs) }),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Attach(name, reset)) => Op::KeySlot(KeySlot {
            animation: anim.to_string(),
            slot: s.clone(),
            time,
            attachment: if *reset { None } else { Some(name.clone()) },
            reset: *reset,
            color: None,
            curve: None,
            curves: None,
        }),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Color(c, curve, curves)) => Op::KeySlot(KeySlot {
            animation: anim.to_string(),
            slot: s.clone(),
            time,
            attachment: None,
            reset: false,
            color: Some(*c),
            curve: Some(*curve),
            curves: channels(curves),
        }),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Deform(att, offsets, c, skin)) => Op::KeyDeform {
            skin: skin.clone(),
            animation: anim.to_string(),
            slot: s.clone(),
            attachment: att.clone(),
            time,
            offsets: offsets.clone(),
            curve: Some(*c),
        },
        (Target::Ik(n), KeyVal::Ik(v, c, cs)) => Op::KeyIk {
            animation: anim.to_string(), ik: n.clone(), time,
            mix: v.mix, softness: v.softness, bend_positive: v.bend_positive, compress: v.compress, stretch: v.stretch,
            curve: Some(*c), curves: channels(cs),
        },
        (Target::DrawOrder, KeyVal::DrawOrder(order)) => Op::KeyDrawOrder { animation: anim.to_string(), time, order: order.clone() },
        (Target::Event(n), KeyVal::Event { int, float, string }) => Op::KeyEvent { animation: anim.to_string(), name: n.clone(), time, int: *int, float: *float, string: string.clone() },
        (Target::Transform(n), KeyVal::Transform(mix, c)) => Op::KeyTransform { animation: anim.to_string(), constraint: n.clone(), time, mix: *mix, curve: Some(*c) },
        (Target::Physics(n, row), KeyVal::Physics(value, c)) => match PHYSICS_ROWS.get(*row as usize) {
            Some(property) => Op::KeyPhysics { animation: anim.to_string(), constraint: n.clone(), property: *property, time, value: *value, curve: Some(*c) },
            None => Op::RemoveAnimation { name: String::new() },
        },
        (Target::Physics(n, _), KeyVal::PhysicsReset) => Op::KeyPhysicsReset { animation: anim.to_string(), constraint: (!n.is_empty()).then(|| n.clone()), time },
        // Mismatched pairs cannot come from vals_at; keep a harmless op.
        _ => Op::RemoveAnimation { name: String::new() },
    }
}

fn remove_op(anim: &str, target: &Target, time: f32, v: &KeyVal) -> Op {
    let rm = |bone: Option<String>, slot: Option<String>, timeline| Op::RemoveKey { skin: None, animation: anim.to_string(), bone, slot, timeline, time, attachment: None };
    match (target, v) {
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Rotate(..)) => rm(Some(b.clone()), None, Timeline::Rotate),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Translate(..)) => rm(Some(b.clone()), None, Timeline::Translate),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Scale(..)) => rm(Some(b.clone()), None, Timeline::Scale),
        (Target::Bone(b) | Target::BoneProp(b, _), KeyVal::Shear(..)) => rm(Some(b.clone()), None, Timeline::Shear),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Attach(..)) => rm(None, Some(s.clone()), Timeline::Attachment),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Color(..)) => rm(None, Some(s.clone()), Timeline::Color),
        (Target::Slot(s) | Target::SlotProp(s, _), KeyVal::Deform(att, _, _, skin)) => Op::RemoveKey {
            skin: skin.clone(),
            animation: anim.to_string(),
            bone: None,
            slot: Some(s.clone()),
            timeline: Timeline::Deform,
            time,
            attachment: Some(att.clone()),
        },
        (Target::Ik(n), _) => Op::RemoveIkKey { animation: anim.to_string(), ik: n.clone(), time },
        (Target::DrawOrder, _) => Op::RemoveDrawOrderKey { animation: anim.to_string(), time },
        (Target::Event(n), _) => Op::RemoveEventKey { animation: anim.to_string(), name: n.clone(), time },
        (Target::Transform(n), _) => Op::RemoveTransformKey { animation: anim.to_string(), constraint: n.clone(), time },
        (Target::Physics(n, row), _) => Op::RemovePhysicsKey { animation: anim.to_string(), constraint: Some(n.clone()), property: PHYSICS_ROWS.get(*row as usize).copied(), time },
        _ => Op::RemoveAnimation { name: String::new() },
    }
}

/// Every row target of an animation that has keys, collapsed rows only.
fn row_targets(a: &Animation) -> Vec<Target> {
    let mut v: Vec<Target> = a.bones.keys().map(|b| Target::Bone(b.clone())).collect();
    v.extend(a.slots.keys().chain(a.deform.values().flat_map(|slots| slots.keys())).map(|s| Target::Slot(s.clone())));
    v.extend(a.ik.keys().map(|n| Target::Ik(n.clone())));
    v.extend(a.transforms.keys().map(|n| Target::Transform(n.clone())));
    v.extend(a.physics.keys().flat_map(|n| (0..=PHYSICS_RESET).map(move |row| Target::Physics(n.clone(), row))));
    if !a.draw_order.is_empty() {
        v.push(Target::DrawOrder);
    }
    v.extend(a.events.iter().map(|k| Target::Event(k.name.clone())));
    v.sort();
    v.dedup();
    v
}

/// The property rows under a collapsed bone or slot row; any other target is its own leaf.
fn leaf_targets(a: &Animation, t: &Target) -> Vec<Target> {
    match t {
        Target::Bone(b) => BONE_PROPS.iter().map(|p| Target::BoneProp(b.clone(), *p)).collect(),
        Target::Slot(s) => {
            let mut v = vec![Target::SlotProp(s.clone(), SlotProp::Attachment), Target::SlotProp(s.clone(), SlotProp::Color)];
            for (skin, slots) in &a.deform {
                v.extend(slots.get(s).into_iter().flat_map(|m| m.keys()).map(|att| Target::SlotProp(s.clone(), SlotProp::Deform(skin.clone(), att.clone()))));
            }
            v
        }
        Target::All => row_targets(a).iter().flat_map(|r| leaf_targets(a, r)).collect(),
        other => vec![other.clone()],
    }
}

/// The selection as leaf keys that exist: collapsed rows and the overview act
/// on every property beneath them, each key once.
fn leaf_keys<'a>(a: &Animation, ids: impl IntoIterator<Item = &'a KeyId>) -> BTreeSet<KeyId> {
    let mut out = BTreeSet::new();
    for (t, q) in ids {
        for leaf in leaf_targets(a, t) {
            if !vals_at(a, &leaf, *q).is_empty() {
                out.insert((leaf, *q));
            }
        }
    }
    out
}

/// Moves every selected key by `delta` seconds: removals first, then re-creation,
/// so overlapping moves do not clobber each other.
#[cfg(test)]
fn retime_ops(base: &Document, anim: &str, sel: &BTreeSet<KeyId>, delta: f32) -> Vec<Op> {
    retime_ops_with(base, anim, sel, delta, false)
}

/// Like `retime_ops`; with `duplicate` the originals stay and copies appear at the new time.
fn retime_ops_with(base: &Document, anim: &str, sel: &BTreeSet<KeyId>, delta: f32, duplicate: bool) -> Vec<Op> {
    let Some(a) = base.animations.get(anim) else { return Vec::new() };
    let (mut remove, mut add) = (Vec::new(), Vec::new());
    for (target, q) in &leaf_keys(a, sel) {
        let t = from_q(*q);
        for v in vals_at(a, target, *q) {
            if !duplicate {
                remove.push(remove_op(anim, target, t, &v));
            }
            add.push(key_op(anim, target, (t + delta).max(0.0), &v));
        }
    }
    remove.extend(add);
    remove
}

fn delete_ops(a: &Animation, anim: &str, ids: &BTreeSet<KeyId>) -> Vec<Op> {
    let mut ops = Vec::new();
    for (target, q) in &leaf_keys(a, ids) {
        for v in vals_at(a, target, *q) {
            ops.push(remove_op(anim, target, from_q(*q), &v));
        }
    }
    ops
}

fn delete_keys(app: &mut EditorApp, anim: &str) {
    let ids: BTreeSet<KeyId> = if app.timeline.sel.is_empty() { keys_at_playhead(app, anim).into_iter().collect() } else { app.timeline.sel.clone() };
    let Some(a) = app.doc.animations.get(anim) else { return };
    let ops = delete_ops(a, anim, &ids);
    if app.edit(ops) {
        app.timeline.sel.clear();
    }
}

fn all_ids(a: &Animation, target: &Target) -> Vec<i64> {
    let mut v: Vec<i64> = match target {
        Target::All => row_targets(a).iter().flat_map(|t| all_ids(a, t)).collect(),
        Target::Bone(_) | Target::Slot(_) => leaf_targets(a, target).iter().flat_map(|t| all_ids(a, t)).collect(),
        Target::BoneProp(b, p) => a.bones.get(b).map(|tl| match p {
            BoneProp::Rotate => tl.rotate.iter().map(|k| tq(k.time)).collect(),
            BoneProp::Translate => tl.translate.iter().map(|k| tq(k.time)).collect(),
            BoneProp::Scale => tl.scale.iter().map(|k| tq(k.time)).collect(),
            BoneProp::Shear => tl.shear.iter().map(|k| tq(k.time)).collect(),
        }).unwrap_or_default(),
        Target::SlotProp(s, SlotProp::Attachment) => a.slots.get(s).map(|tl| tl.attachment.iter().map(|k| tq(k.time)).collect()).unwrap_or_default(),
        Target::SlotProp(s, SlotProp::Color) => a.slots.get(s).map(|tl| tl.color.iter().map(|k| tq(k.time)).collect()).unwrap_or_default(),
        Target::SlotProp(s, SlotProp::Deform(skin, att)) => a.deform.get(skin).and_then(|m| m.get(s)).and_then(|m| m.get(att)).map(|k| k.iter().map(|k| tq(k.time)).collect()).unwrap_or_default(),
        Target::Ik(n) => a.ik.get(n).map(|k| k.iter().map(|k| tq(k.time)).collect()).unwrap_or_default(),
        Target::Transform(n) => a.transforms.get(n).map(|k| k.iter().map(|k| tq(k.time)).collect()).unwrap_or_default(),
        Target::DrawOrder => a.draw_order.iter().map(|k| tq(k.time)).collect(),
        Target::Event(n) => a.events.iter().filter(|k| &k.name == n).map(|k| tq(k.time)).collect(),
        Target::Physics(n, row) => a.physics.get(n).map(|tl| if *row == PHYSICS_RESET { tl.reset.iter().map(|t| tq(*t)).collect() } else { physics_keys(tl, *row).iter().map(|k| tq(k.time)).collect() }).unwrap_or_default(),
    };
    v.sort_unstable();
    v.dedup();
    v
}

// ---------------------------------------------------------------- shortcuts

fn shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || !matches!(app.dialog, crate::app::Dialog::None) {
        return;
    }
    let side = side_hovered(ctx);
    let fps = app.settings.fps.max(1.0);
    struct Keys {
        step: i32,
        shift: bool,
        home: bool,
        end: bool,
        delete: bool,
        key: bool,
        key_shown: bool,
        select_all: bool,
        copy: bool,
        paste: bool,
    }
    let k = ctx.input(|i| {
        let m = i.modifiers;
        let plain = !m.command && !m.alt;
        let comma = i.key_pressed(egui::Key::Comma) && plain;
        let period = i.key_pressed(egui::Key::Period) && plain;
        Keys {
            step: i32::from(period) - i32::from(comma),
            shift: m.shift,
            home: i.key_pressed(egui::Key::Home) && plain,
            end: i.key_pressed(egui::Key::End) && plain,
            delete: (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && !m.command,
            key: i.key_pressed(egui::Key::K) && !m.any(),
            key_shown: i.key_pressed(egui::Key::L) && m.command && m.shift && !m.alt,
            select_all: i.key_pressed(egui::Key::A) && m.command && !m.shift && !m.alt,
            copy: i.events.iter().any(|e| matches!(e, Event::Copy)) || (m.command && i.key_pressed(egui::Key::C)),
            paste: i.events.iter().any(|e| matches!(e, Event::Paste(_))) || (m.command && i.key_pressed(egui::Key::V)),
        }
    });
    let Some(name) = app.playback.anim.clone().filter(|a| app.doc.animations.contains_key(a)) else { return };
    let duration = app.animation().map_or(0.0, Animation::duration);
    let t = app.playback.time;
    let mut new_time = None;
    if k.step != 0 {
        if k.shift {
            let times = key_times(app, &name);
            new_time = if k.step > 0 { times.iter().copied().find(|&x| x > t + EPS) } else { times.iter().copied().rev().find(|&x| x < t - EPS) };
        } else {
            new_time = Some(((t * fps).round() + k.step as f32) / fps);
        }
    }
    if k.home {
        new_time = Some(0.0);
    }
    if k.end {
        new_time = Some(duration);
    }
    if let Some(nt) = new_time {
        app.playback.time = nt.clamp(0.0, duration.max(0.0));
        app.playback.playing = false;
    }
    if k.key {
        key_edited(app);
    }
    if k.key_shown {
        key_shown(app);
    }
    if side {
        return;
    }
    if k.select_all && app.timeline.hovered {
        select_all(app, &name);
    }
    if k.delete && (!app.timeline.sel.is_empty() || !keys_at_playhead(app, &name).is_empty()) {
        delete_keys(app, &name);
    }
    if k.copy {
        copy_keys(app, &name);
    }
    if k.paste {
        paste_keys(app, &name);
    }
}

/// Key times of the selected row, or of every row when none is selected.
fn key_times(app: &EditorApp, anim: &str) -> Vec<f32> {
    let Some(a) = app.doc.animations.get(anim) else { return Vec::new() };
    let mut targets = selected_targets(app);
    let row = app.selection.slot.is_some() || app.selection.ik.is_some() || app.selection.transform.is_some() || app.selection.bone.is_some() || app.selection.physics.is_some();
    if !row || targets.iter().all(|t| all_ids(a, t).is_empty()) {
        targets = row_targets(a);
    }
    let mut qs: Vec<i64> = targets.iter().flat_map(|t| all_ids(a, t)).collect();
    qs.sort_unstable();
    qs.dedup();
    qs.into_iter().map(from_q).collect()
}

/// Ctrl+A: every key of the selected row, then (pressed again) every key of the shown rows.
fn select_all(app: &mut EditorApp, anim: &str) {
    let Some(a) = app.doc.animations.get(anim) else { return };
    let row: BTreeSet<KeyId> = selected_targets(app).into_iter().flat_map(|t| all_ids(a, &t).into_iter().map(move |q| (t.clone(), q))).collect();
    let shown: BTreeSet<KeyId> = build_items(app, a).into_iter().filter_map(|it| match it {
        Item::Row(t, ids) if t != Target::All => Some(ids.into_iter().map(move |q| (t.clone(), q))),
        _ => None,
    }).flatten().collect();
    app.timeline.sel = if !row.is_empty() && app.timeline.sel != row { row } else { shown };
}

fn copy_keys(app: &mut EditorApp, anim: &str) {
    let Some(a) = app.doc.animations.get(anim) else { return };
    let picked: Vec<KeyId> = if app.timeline.sel.is_empty() {
        match &app.selection.bone {
            Some(b) => all_ids(a, &Target::Bone(b.clone())).into_iter().map(|q| (Target::Bone(b.clone()), q)).collect(),
            None => Vec::new(),
        }
    } else {
        app.timeline.sel.iter().cloned().collect()
    };
    let ids: Vec<KeyId> = leaf_keys(a, &picked).into_iter().collect();
    let Some(base) = ids.iter().map(|(_, q)| *q).min() else { return };
    let mut clip = Vec::new();
    for (target, q) in &ids {
        for val in vals_at(a, target, *q) {
            clip.push(ClipKey { target: target.clone(), dt: from_q(*q - base), val });
        }
    }
    let n = ids.len();
    app.timeline.clipboard = clip;
    app.status = if n == 1 { crate::tf!("Copied {} key", n) } else { crate::tf!("Copied {} keys", n) };
}

fn paste_keys(app: &mut EditorApp, anim: &str) {
    if app.timeline.clipboard.is_empty() {
        return;
    }
    let t0 = app.playback.time;
    // A clipboard of bone keys from a single bone pastes onto the selected bone.
    let owners: BTreeSet<&str> = app.timeline.clipboard.iter().filter_map(|c| match &c.target {
        Target::Bone(b) | Target::BoneProp(b, _) => Some(b.as_str()),
        _ => None,
    }).collect();
    let all_bones = owners.len() == 1 && app.timeline.clipboard.iter().all(|c| matches!(c.target, Target::Bone(_) | Target::BoneProp(..)));
    let retarget = app.selection.bone.clone().filter(|_| all_bones);
    let mut ops = Vec::new();
    let mut ids = BTreeSet::new();
    for c in &app.timeline.clipboard {
        let target = match (&retarget, &c.target) {
            (Some(sel), Target::Bone(_)) => Target::Bone(sel.clone()),
            (Some(sel), Target::BoneProp(_, p)) => Target::BoneProp(sel.clone(), *p),
            _ => c.target.clone(),
        };
        ops.push(key_op(anim, &target, t0 + c.dt, &c.val));
        ids.insert((target, tq(t0 + c.dt)));
    }
    if app.edit(ops) {
        app.timeline.sel = ids;
    }
}

// ---------------------------------------------------------------- curves

/// The single bone key the curve controls apply to: the only selected key,
/// or the selected bone's key at the playhead.
/// The key the curve tools act on: the single selected one, else one of the
/// selected item's keys at the playhead.
fn current_key(app: &EditorApp, anim: &str) -> Option<(Target, f32)> {
    let a = app.doc.animations.get(anim)?;
    let has_curve = |t: &Target, q: i64| vals_at(a, t, q).iter().any(|v| val_curve(v).is_some());
    let sel = &app.timeline.sel;
    if sel.len() > 1 { return None; }
    if let Some((target, q)) = sel.iter().next() {
        return has_curve(target, *q).then(|| (target.clone(), from_q(*q)));
    }
    let q = tq(app.playback.time);
    let mut targets = selected_targets(app);
    // The slot before its bone, which is selected along with it.
    targets.sort_by_key(|t| matches!(t, Target::Bone(_)));
    targets.into_iter().find(|t| has_curve(t, q)).map(|t| (t, from_q(q)))
}

fn val_curve(v: &KeyVal) -> Option<Curve> {
    match v {
        KeyVal::Rotate(_, c) | KeyVal::Translate(_, c, _) | KeyVal::Scale(_, c, _) | KeyVal::Shear(_, c, _) | KeyVal::Color(_, c, _) | KeyVal::Ik(_, c, _) | KeyVal::Transform(_, c) | KeyVal::Physics(_, c) | KeyVal::Deform(_, _, c, _) => Some(*c),
        KeyVal::Attach(..) | KeyVal::PhysicsReset | KeyVal::DrawOrder(_) | KeyVal::Event { .. } => None,
    }
}

fn with_curve(v: KeyVal, curve: Curve, channels: &Option<Vec<Curve>>) -> KeyVal {
    let own = channels.clone().unwrap_or_default();
    match v {
        KeyVal::Rotate(x, _) => KeyVal::Rotate(x, curve),
        KeyVal::Translate(x, ..) => KeyVal::Translate(x, curve, own),
        KeyVal::Scale(x, ..) => KeyVal::Scale(x, curve, own),
        KeyVal::Shear(x, ..) => KeyVal::Shear(x, curve, own),
        KeyVal::Color(x, ..) => KeyVal::Color(x, curve, Vec::new()),
        KeyVal::Ik(x, ..) => KeyVal::Ik(x, curve, Vec::new()),
        KeyVal::Transform(x, _) => KeyVal::Transform(x, curve),
        KeyVal::Physics(x, _) => KeyVal::Physics(x, curve),
        KeyVal::Deform(a, o, _, k) => KeyVal::Deform(a, o, curve, k),
        other => other,
    }
}

fn key_curve(app: &EditorApp, anim: &str, target: &Target, t: f32) -> Curve {
    app.doc.animations.get(anim)
        .and_then(|a| vals_at(a, target, tq(t)).iter().find_map(val_curve))
        .unwrap_or_default()
}

/// The `[x, y]` curves of a key: `None` without a two-channel key at `t`.
fn key_channels(app: &EditorApp, anim: &str, target: &Target, t: f32) -> Option<[Curve; 2]> {
    let a = app.doc.animations.get(anim)?;
    vals_at(a, target, tq(t)).into_iter().find_map(|k| match k {
        KeyVal::Translate(_, c, cs) | KeyVal::Scale(_, c, cs) | KeyVal::Shear(_, c, cs) => Some([cs.first().copied().unwrap_or(c), cs.get(1).copied().unwrap_or(c)]),
        _ => None,
    })
}

fn set_curve(app: &mut EditorApp, anim: &str, target: &Target, t: f32, curve: Curve, coalesce: bool) {
    set_key_curves(app, anim, target, t, curve, None, coalesce);
}

fn set_key_curves(app: &mut EditorApp, anim: &str, target: &Target, t: f32, curve: Curve, curves: Option<Vec<Curve>>, coalesce: bool) {
    let q = tq(t);
    let Some(a) = app.doc.animations.get(anim) else { return };
    let ops: Vec<Op> = vals_at(a, target, q).into_iter().filter(|v| val_curve(v).is_some())
        .map(|v| key_op(anim, target, t, &with_curve(v, curve, &curves))).collect();
    if ops.is_empty() { return; }
    if coalesce {
        app.edit_coalesced(format!("curve:{anim}:{}:{q}", target_label(target)), ops);
    } else {
        app.edit(ops);
    }
}

/// "Curve" submenu with the easing presets for the current bone key.
fn curve_menu(app: &mut EditorApp, ui: &mut egui::Ui, anim: &str, key: Option<(Target, f32)>) {
    ui.add_enabled_ui(key.is_some(), |ui| {
        ui.menu_button(tr("Curve"), |ui| {
            let Some((target, t)) = &key else { return };
            let current = key_curve(app, anim, target, *t);
            for c in [Curve::Linear, Curve::Stepped, Curve::EaseIn, Curve::EaseOut, Curve::EaseInOut] {
                if ui.selectable_label(current == c, curve_name(c)).clicked() {
                    set_curve(app, anim, target, *t, c, false);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(tr("Easing from the key at the playhead to the next key"));
    });
}

/// Menu entry with right-aligned shortcut text.
fn menu_item(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let clicked = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(shortcut)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Right-click menu of the dope sheet: key actions, clipboard, curve and view.
fn key_menu(app: &mut EditorApp, ui: &mut egui::Ui, anim: &str) {
    if menu_item(ui, true, tr("Key edited"), "K") {
        key_edited(app);
    }
    if menu_item(ui, true, tr("Key shown rows"), "Ctrl+Shift+L") {
        key_shown(app);
    }
    ui.separator();
    if menu_item(ui, true, tr("Select all keys"), "Ctrl+A") {
        select_all(app, anim);
    }
    let can_delete = !app.timeline.sel.is_empty() || !keys_at_playhead(app, anim).is_empty();
    if menu_item(ui, can_delete, tr("Delete keys"), "Del") {
        delete_keys(app, anim);
    }
    ui.separator();
    if menu_item(ui, true, tr("Copy keys"), "Ctrl+C") {
        copy_keys(app, anim);
    }
    if menu_item(ui, !app.timeline.clipboard.is_empty(), tr("Paste keys"), "Ctrl+V") {
        paste_keys(app, anim);
    }
    ui.separator();
    let key = current_key(app, anim);
    curve_menu(app, ui, anim, key);
    ui.separator();
    let clearable = app.selection.slot.clone().map(|s| (None, Some(s))).or_else(|| app.selection.bone.clone().map(|b| (Some(b), None)));
    if menu_item(ui, clearable.is_some(), tr("Clear all keys of the selection"), "")
        && let Some((bone, slot)) = clearable
    {
        app.edit(vec![Op::ClearTimeline { skin: None, animation: anim.to_string(), bone, slot, timeline: None }]);
    }
    ui.separator();
    if menu_item(ui, true, tr("Fit to animation"), "") {
        app.timeline.want_fit = true;
    }
}

fn curve_name(c: Curve) -> &'static str {
    match c {
        Curve::Linear => tr("Linear"),
        Curve::Stepped => tr("Stepped"),
        Curve::EaseIn => tr("Ease in"),
        Curve::EaseOut => tr("Ease out"),
        Curve::EaseInOut => tr("Ease in-out"),
        Curve::Bezier(_) => tr("Bezier"),
    }
}

fn control_points(c: Curve) -> [f32; 4] {
    match c {
        Curve::Linear | Curve::Stepped => [0.0, 0.0, 1.0, 1.0],
        Curve::EaseIn => [0.42, 0.0, 1.0, 1.0],
        Curve::EaseOut => [0.0, 0.0, 0.58, 1.0],
        Curve::EaseInOut => [0.42, 0.0, 0.58, 1.0],
        Curve::Bezier(p) => p,
    }
}

/// 120x80 easing preview with two draggable bezier handles.
fn curve_editor(app: &mut EditorApp, ui: &mut egui::Ui, anim: &str, target: &Target, t: f32) {
    crate::theme::hint(ui, &crate::tf!("Curve · {} @ {} s", target_label(target), format!("{t:.2}")));
    let channels = key_channels(app, anim, target, t);
    let channel_id = ui.make_persistent_id("curve_channel");
    let mut channel: usize = ui.data(|d| d.get_temp(channel_id)).unwrap_or(0);
    if channels.is_none() { channel = 0; }
    if channels.is_some() {
        ui.horizontal(|ui| {
            for (i, label) in [tr("X and Y"), "X", "Y"].into_iter().enumerate() {
                ui.selectable_value(&mut channel, i, label);
            }
        });
        ui.data_mut(|d| d.insert_temp(channel_id, channel));
    }
    let curve = match (channel, channels) {
        (1 | 2, Some(c)) => c[channel - 1],
        _ => key_curve(app, anim, target, t),
    };
    if channel == 0 && channels.is_some_and(|c| c[0] != c[1]) {
        crate::theme::hint(ui, tr("X and Y have different curves. Editing here makes them the same."));
    }
    let mut pts = control_points(curve);
    let (rect, _) = ui.allocate_exact_size(vec2(120.0, 80.0), Sense::hover());
    let painter = ui.painter_at(rect.expand(8.0));
    painter.rect_filled(rect, 3.0, Color32::from_gray(32));
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Inside);
    // y in [-0.25, 1.25] fits the rect so overshoot handles stay reachable.
    let (y0, y1) = (-0.25_f32, 1.25_f32);
    let map = |x: f32, y: f32| pos2(rect.left() + x * rect.width(), rect.bottom() - (y - y0) / (y1 - y0) * rect.height());
    let unmap = |p: Pos2| ((p.x - rect.left()) / rect.width(), y0 + (rect.bottom() - p.y) / rect.height() * (y1 - y0));
    painter.line_segment([map(0.0, 0.0), map(1.0, 1.0)], Stroke::new(1.0, Color32::from_gray(50)));
    let line: Vec<Pos2> = (0..=40)
        .map(|i| {
            let x = i as f32 / 40.0;
            let y = if matches!(curve, Curve::Stepped) { if x < 1.0 { 0.0 } else { 1.0 } } else { Curve::Bezier(pts).apply(x) };
            map(x, y)
        })
        .collect();
    painter.add(Shape::line(line, Stroke::new(1.5, ACCENT)));
    let mut changed = false;
    for h in 0..2 {
        let anchor = if h == 0 { map(0.0, 0.0) } else { map(1.0, 1.0) };
        let hp = map(pts[h * 2], pts[h * 2 + 1]);
        let r = ui.interact(Rect::from_center_size(hp, vec2(14.0, 14.0)), ui.make_persistent_id(("curve_handle", h)), Sense::drag());
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            let (x, y) = unmap(p);
            pts[h * 2] = x.clamp(0.0, 1.0);
            pts[h * 2 + 1] = y.clamp(-0.5, 1.5);
            changed = true;
        }
        let hp = map(pts[h * 2], pts[h * 2 + 1]);
        painter.line_segment([anchor, hp], Stroke::new(1.0, Color32::from_gray(110)));
        painter.circle_filled(hp, 4.5, if r.hovered() || r.dragged() { Color32::WHITE } else { BONE });
        r.on_hover_text(tr("Drag to shape the easing"));
    }
    if changed {
        match channels.filter(|_| channel > 0) {
            Some(mut c) => {
                c[channel - 1] = Curve::Bezier(pts);
                set_key_curves(app, anim, target, t, key_curve(app, anim, target, t), Some(c.to_vec()), true);
            }
            None => set_curve(app, anim, target, t, Curve::Bezier(pts), true),
        }
    }
    crate::theme::hint(ui, &format!("{:.2} {:.2} · {:.2} {:.2}", pts[0], pts[1], pts[2], pts[3]));
}

// ---------------------------------------------------------------- dope sheet

enum Item {
    Header(&'static str, Color32), // English title; translated when drawn
    Row(Target, Vec<i64>),
}

fn build_items(app: &EditorApp, anim: &Animation) -> Vec<Item> {
    // With a bone selected only its rows (and those of what touches it) show,
    // the "All" toggle in the toolbar lifts that.
    let sel = &app.selection;
    let sel_bone = sel.bone.as_deref();
    let narrow = !app.timeline.show_all && sel_bone.is_some();
    let mut bones = Vec::new();
    for bone in &app.doc.bones {
        let selected = sel_bone == Some(bone.name.as_str());
        if (anim.bones.contains_key(&bone.name) || selected) && (!narrow || selected) {
            let t = Target::Bone(bone.name.clone());
            let ids = all_ids(anim, &t);
            let open = app.timeline.expanded.contains(&t);
            bones.push(Item::Row(t, ids));
            if open {
                for p in BONE_PROPS {
                    let t = Target::BoneProp(bone.name.clone(), p);
                    let ids = all_ids(anim, &t);
                    bones.push(Item::Row(t, ids));
                }
            }
        }
    }
    let mut slots = Vec::new();
    for slot in &app.doc.slots {
        let keyed = anim.slots.contains_key(&slot.name) || anim.deform.values().any(|slots| slots.contains_key(&slot.name));
        let mine = sel.slot.as_deref() == Some(slot.name.as_str()) || sel_bone == Some(slot.bone.as_str());
        if keyed && (!narrow || mine) {
            let t = Target::Slot(slot.name.clone());
            let ids = all_ids(anim, &t);
            let open = app.timeline.expanded.contains(&t);
            slots.push(Item::Row(t.clone(), ids));
            if open {
                for leaf in leaf_targets(anim, &t) {
                    let ids = all_ids(anim, &leaf);
                    slots.push(Item::Row(leaf, ids));
                }
            }
        }
    }
    let touches = |names: &[String]| sel_bone.is_some_and(|b| names.iter().any(|n| n == b));
    let mut iks = Vec::new();
    for ik in &app.doc.ik {
        let mine = sel.ik.as_deref() == Some(ik.name.as_str()) || touches(&ik.bones) || sel_bone == Some(ik.target.as_str());
        if anim.ik.get(&ik.name).is_some_and(|k| !k.is_empty()) && (!narrow || mine) {
            let t = Target::Ik(ik.name.clone());
            let ids = all_ids(anim, &t);
            iks.push(Item::Row(t, ids));
        }
    }
    for c in &app.doc.transforms {
        let mine = sel.transform.as_deref() == Some(c.name.as_str()) || touches(&c.bones) || sel_bone == Some(c.source.as_str());
        if (anim.transforms.get(&c.name).is_some_and(|k| !k.is_empty()) || sel.transform.as_deref() == Some(c.name.as_str())) && (!narrow || mine) {
            let t = Target::Transform(c.name.clone());
            let ids = all_ids(anim, &t);
            iks.push(Item::Row(t, ids));
        }
    }
    // Physics: a row per keyed property, and a reset row for every keyed or selected constraint.
    let mut physics = Vec::new();
    let names = std::iter::once(String::new()).chain(app.doc.physics.iter().map(|c| c.name.clone()));
    for name in names {
        let keyed = anim.physics.get(&name).is_some_and(|t| !t.is_empty());
        if !keyed && app.selection.physics.as_deref() != Some(name.as_str()) { continue; }
        let mine = name.is_empty() || sel.physics.as_deref() == Some(name.as_str()) || app.doc.physics.iter().any(|c| c.name == name && sel_bone == Some(c.bone.as_str()));
        if narrow && !mine { continue; }
        for row in 0..=PHYSICS_RESET {
            let t = Target::Physics(name.clone(), row);
            let ids = all_ids(anim, &t);
            if row == PHYSICS_RESET || !ids.is_empty() { physics.push(Item::Row(t, ids)); }
        }
    }
    let mut draw_order = Vec::new();
    if !anim.draw_order.is_empty() {
        draw_order.push(Item::Row(Target::DrawOrder, all_ids(anim, &Target::DrawOrder)));
    }
    let mut events = Vec::new();
    for name in app.doc.events.keys() {
        if anim.events.iter().any(|k| &k.name == name) || app.timeline.event.as_deref() == Some(name.as_str()) {
            let t = Target::Event(name.clone());
            let ids = all_ids(anim, &t);
            events.push(Item::Row(t, ids));
        }
    }
    let mut items = Vec::new();
    // Draw order and events close the list.
    for (title, color, rows) in [("BONES", BONE, bones), ("SLOTS", SLOT, slots), ("IK", IK, iks), ("PHYSICS", PHYSICS, physics), ("DRAW ORDER", theme::DRAW_ORDER, draw_order), ("EVENTS", theme::EVENT, events)] {
        if !rows.is_empty() {
            items.push(Item::Header(title, color));
            items.extend(rows);
        }
    }
    if !items.is_empty() {
        items.insert(0, Item::Row(Target::All, all_ids(anim, &Target::All)));
    }
    items
}

/// A bone row's label and key color: the bone's own color, lifted when too dark to read.
fn bone_target_color(app: &EditorApp, bone: &str) -> Color32 {
    app.doc.bone(bone).map_or(BONE, theme::bone_text_color)
}

fn select_target(app: &mut EditorApp, target: &Target) {
    match target {
        Target::All => {}
        Target::BoneProp(b, _) => select_target(app, &Target::Bone(b.clone())),
        Target::SlotProp(s, _) => select_target(app, &Target::Slot(s.clone())),
        Target::Bone(b) => {
            app.selection.bone = Some(b.clone());
            app.selection.slot = None;
            app.selection.ik = None;
            app.selection.physics = None;
        }
        Target::Ik(n) => { app.selection.ik = Some(n.clone()); app.selection.physics = None; app.selection.transform = None; }
        Target::DrawOrder => {}
        Target::Event(n) => app.timeline.event = Some(n.clone()),
        Target::Transform(n) => app.selection = crate::app::Selection { transform: Some(n.clone()), ..Default::default() },
        Target::Physics(n, _) => {
            // The "all physics" row has no inspector of its own.
            if !n.is_empty() { app.selection = crate::app::Selection { physics: Some(n.clone()), ..Default::default() }; }
        }
        Target::Slot(s) => {
            app.selection.slot = Some(s.clone());
            app.selection.bone = app.doc.slot(s).map(|s| s.bone.clone());
            app.selection.ik = None;
            app.selection.physics = None;
        }
    }
}

fn target_selected(app: &EditorApp, t: &Target) -> bool {
    match t {
        Target::All => false,
        Target::BoneProp(b, _) => target_selected(app, &Target::Bone(b.clone())),
        Target::SlotProp(s, _) => target_selected(app, &Target::Slot(s.clone())),
        Target::Bone(b) => app.selection.bone.as_deref() == Some(b) && app.selection.slot.is_none() && app.selection.ik.is_none() && app.selection.physics.is_none(),
        Target::Slot(s) => app.selection.slot.as_deref() == Some(s),
        Target::Ik(n) => app.selection.ik.as_deref() == Some(n),
        Target::Transform(n) => app.selection.transform.as_deref() == Some(n),
        Target::DrawOrder => false,
        Target::Event(n) => app.timeline.event.as_deref() == Some(n),
        Target::Physics(n, _) => !n.is_empty() && app.selection.physics.as_deref() == Some(n),
    }
}

/// Ruler ticks as (x, frame, is_major).
fn ticks(view: View, width: f32, fps: f32) -> Vec<(f32, i64, bool)> {
    let ppf = view.zoom / fps;
    let major = [1_i64, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600].into_iter().find(|&c| c as f32 * ppf >= 64.0).unwrap_or(7200);
    let minor = if ppf >= 4.0 { 1 } else { (major / 5).max(1) };
    let f0 = ((view.offset * fps).floor() as i64).max(0);
    let f1 = ((view.t(view.left + width) * fps).ceil() as i64).max(f0);
    let start = f0 - f0 % minor;
    (start..=f1).step_by(minor as usize).filter(|f| *f >= 0).map(|f| (view.x(f as f32 / fps), f, f % major == 0)).collect()
}

fn dope_sheet(app: &mut EditorApp, ui: &mut egui::Ui, name: &str) {
    let fps = app.settings.fps.max(1.0);
    let width = ui.available_width().max(LABEL_W + 100.0);
    let (ruler_rect, ruler) = ui.allocate_exact_size(vec2(width, RULER_H), Sense::click_and_drag());
    let sheet_area = ui.available_rect_before_wrap();
    let track_left = ruler_rect.left() + LABEL_W;
    let track_w = (width - LABEL_W).max(50.0);
    let duration = app.animation().map_or(0.0, Animation::duration);
    let span = duration.max(2.0 / fps);

    // Fit when the animation changes or on request.
    {
        let st = &mut app.timeline;
        if st.zoom <= 0.0 || st.want_fit || st.fit_anim.as_deref() != Some(name) {
            st.zoom = ((track_w - 28.0) / span).max(5.0);
            st.offset = -14.0 / st.zoom;
            st.fit_anim = Some(name.to_string());
            st.want_fit = false;
        }
    }
    let mut view = View { left: track_left, zoom: app.timeline.zoom, offset: app.timeline.offset };

    // Zoom (Ctrl+wheel) and pan (wheel, middle-drag).
    let items = build_items(app, &app.animation().cloned().unwrap_or_default());
    let rows_h = items.len().max(1) as f32 * ROW_H;
    let fits = rows_h <= sheet_area.height();
    let over_ruler = ruler.hovered();
    let over_sheet = ui.rect_contains_pointer(sheet_area);
    if over_ruler || over_sheet {
        let (zd, scroll, ptr) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if zd != 1.0
            && let Some(p) = ptr
        {
            let at = view.t(p.x.max(track_left));
            view.zoom = (view.zoom * zd).clamp(4.0, 6000.0);
            view.offset = at - (p.x.max(track_left) - track_left) / view.zoom;
        }
        let wheel = if over_ruler || fits { scroll.x + scroll.y } else { scroll.x };
        if wheel != 0.0 {
            view.offset -= wheel / view.zoom;
        }
    }
    if ruler.dragged_by(PointerButton::Middle) {
        view.offset -= ruler.drag_delta().x / view.zoom;
    }

    // Ruler scrubbing.
    let alt = ui.input(|i| i.modifiers.alt);
    if ruler.is_pointer_button_down_on()
        && ui.input(|i| i.pointer.primary_down())
        && let Some(p) = ui.input(|i| i.pointer.latest_pos())
        && p.x >= track_left
    {
        let t = view.t(p.x);
        app.playback.time = (if alt { t } else { snap(t, fps) }).clamp(0.0, duration);
        app.playback.playing = false;
    }

    // Retime drag in progress.
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let (Some(kd), Some(p)) = (&mut app.timeline.key_drag, pointer) {
        let min_t = app.timeline.sel.iter().map(|(_, q)| from_q(*q)).fold(f32::MAX, f32::min);
        let raw = view.t(p.x) - kd.grab;
        let new = if alt { raw } else { snap(raw, fps) };
        let delta = (new - kd.anchor).max(-min_t);
        if delta.abs() > 1e-5 || kd.pushed {
            let first = !kd.pushed;
            kd.pushed = true;
            kd.delta = delta;
            let base = kd.base.clone();
            let ops = retime_ops_with(&base, name, &app.timeline.sel, delta, kd.duplicate);
            if first {
                app.push_undo();
            }
            app.preview(&base, &ops);
        }
    }
    let drag_delta = app.timeline.key_drag.as_ref().map_or(0.0, |k| k.delta);

    app.timeline.zoom = view.zoom;
    app.timeline.offset = view.offset.clamp(-60.0 / view.zoom, span + 1.0);
    view.offset = app.timeline.offset;

    // Fresh rows after a possible preview edit.
    let anim = app.animation().cloned().unwrap_or_default();
    let items = build_items(app, &anim);
    if app.timeline.key_drag.is_none() {
        // Selected keys that no longer exist drop out, whether or not their row is open.
        let shown: BTreeSet<Target> = items.iter().filter_map(|it| if let Item::Row(t, _) = it { Some(t) } else { None }).flat_map(|t| std::iter::once(t.clone()).chain(leaf_targets(&anim, t))).collect();
        let sel: BTreeSet<KeyId> = app.timeline.sel.iter().filter(|(t, q)| shown.contains(t) && all_ids(&anim, t).contains(q)).cloned().collect();
        app.timeline.sel = sel;
    }

    let ticks = ticks(view, track_w, fps);
    let painter = ui.painter().clone();
    let time = app.playback.time;
    draw_ruler(&painter, ruler_rect, view, &ticks, fps, time, duration);

    let area_h = ((sheet_area.height() / ROW_H).floor() * ROW_H).max(ROW_H * 2.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(area_h).show(ui, |ui| {
        let h = rows_h.max(area_h);
        let (resp, painter) = ui.allocate_painter(vec2(width, h), Sense::click_and_drag());
        let rect = resp.rect;
        let track = Rect::from_min_max(pos2(track_left, rect.top()), pos2(rect.right(), rect.bottom()));
        painter.rect_filled(rect, 0.0, Color32::from_gray(38));
        let hover = resp.hover_pos();
        let hover_row = hover.map(|p| ((p.y - rect.top()) / ROW_H).floor() as i64);
        let tp = painter.with_clip_rect(track.intersect(ui.clip_rect()));

        // Frame grid.
        for (x, _, major) in &ticks {
            let c = if *major { Color32::from_white_alpha(14) } else { Color32::from_white_alpha(5) };
            tp.line_segment([pos2(*x, rect.top()), pos2(*x, rect.bottom())], Stroke::new(1.0, c));
        }
        // Beyond the end of the animation.
        let end_x = view.x(duration);
        if end_x < track.right() {
            tp.rect_filled(Rect::from_min_max(pos2(end_x.max(track.left()), rect.top()), track.right_bottom()), 0.0, Color32::from_black_alpha(70));
        }
        tp.line_segment([pos2(end_x, rect.top()), pos2(end_x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(90)));

        let label_clip = painter.with_clip_rect(Rect::from_min_max(rect.left_top(), pos2(track_left - 4.0, rect.bottom())));
        #[cfg(test)]
        app.timeline.rows.clear();
        for (i, item) in items.iter().enumerate() {
            let y = rect.top() + i as f32 * ROW_H;
            let row_rect = Rect::from_min_size(pos2(rect.left(), y), vec2(width, ROW_H));
            #[cfg(test)]
            if let Item::Row(t, _) = item {
                app.timeline.rows.push((t.clone(), row_rect));
            }
            let mid = y + ROW_H / 2.0;
            match item {
                Item::Header(title, color) => {
                    painter.rect_filled(row_rect, 0.0, Color32::from_gray(48));
                    painter.text(pos2(rect.left() + 8.0, mid), egui::Align2::LEFT_CENTER, tr(title), egui::FontId::proportional(SIZE_SMALL), *color);
                }
                Item::Row(target, ids) => {
                    let selected = target_selected(app, target);
                    if i % 2 == 0 {
                        painter.rect_filled(row_rect, 0.0, ui.visuals().faint_bg_color);
                    }
                    if hover_row == Some(i as i64) {
                        painter.rect_filled(row_rect, 0.0, Color32::from_white_alpha(10));
                    }
                    if selected {
                        painter.rect_filled(row_rect, 0.0, ACCENT.gamma_multiply(0.12));
                        painter.rect_filled(Rect::from_min_size(row_rect.left_top(), vec2(3.0, ROW_H)), 0.0, ACCENT);
                    }
                    let (text, color) = match target {
                        Target::All => (tr("Overview").to_string(), TEXT),
                        Target::Bone(b) => (b.clone(), bone_target_color(app, b)),
                        Target::BoneProp(b, p) => (tr(bone_prop_name(*p)).to_string(), bone_target_color(app, b)),
                        Target::Slot(s) => (s.clone(), SLOT),
                        Target::SlotProp(_, SlotProp::Attachment) => (tr("Attachment").to_string(), SLOT),
                        Target::SlotProp(_, SlotProp::Color) => (tr("Color").to_string(), SLOT),
                        Target::SlotProp(s, SlotProp::Deform(skin, att)) => (prop_deform_label(s, skin, att).split_once(" · ").map_or_else(String::new, |x| x.1.to_string()), SLOT),
                        Target::Ik(n) | Target::Transform(n) => (n.clone(), IK),
                        Target::Physics(..) => (target_label(target), PHYSICS),
                        Target::DrawOrder => (target_label(target), theme::DRAW_ORDER),
                        Target::Event(n) => (n.clone(), theme::EVENT),
                    };
                    let indent = if matches!(target, Target::BoneProp(..) | Target::SlotProp(..)) { 30.0 } else { 12.0 };
                    if matches!(target, Target::Bone(_) | Target::Slot(_)) {
                        let open = app.timeline.expanded.contains(target);
                        theme::chevron(&label_clip, pos2(rect.left() + 12.0, mid), open, if hover_row == Some(i as i64) { Color32::WHITE } else { Color32::from_gray(160) });
                    }
                    let indent = if matches!(target, Target::Bone(_) | Target::Slot(_)) { 24.0 } else { indent };
                    label_clip.text(pos2(rect.left() + indent, mid), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(SIZE_SMALL), if selected { TEXT } else { color });
                    let key_color = match target {
                        Target::All => TEXT,
                        Target::Bone(b) | Target::BoneProp(b, _) => bone_target_color(app, b),
                        Target::Slot(_) | Target::SlotProp(..) => SLOT,
                        Target::Ik(_) | Target::Transform(_) => IK,
                        Target::Physics(..) => PHYSICS,
                        Target::DrawOrder => theme::DRAW_ORDER,
                        Target::Event(_) => theme::EVENT,
                    };
                    // Lines between keys show how the value travels: straight for
                    // linear, wavy for a curve, dotted for stepped, none when it does not change.
                    for pair in ids.windows(2) {
                        let (x0, x1) = (view.x(from_q(pair[0])), view.x(from_q(pair[1])));
                        if x1 < track.left() - 8.0 || x0 > track.right() + 8.0 {
                            continue;
                        }
                        draw_interpolation(&tp, &anim, target, [pair[0], pair[1]], [x0, x1], mid, key_color);
                    }
                    for &q in ids {
                        let x = view.x(from_q(q));
                        if x < track.left() - 8.0 || x > track.right() + 8.0 {
                            continue;
                        }
                        let shown_q = tq(from_q(q) - drag_delta);
                        let is_sel = app.timeline.sel.iter().any(|(st, sq)| *sq == shown_q && related(st, target));
                        diamond(&tp, pos2(x, mid), if is_sel { ACCENT } else { key_color }, is_sel);
                    }
                }
            }
        }
        painter.line_segment([pos2(track_left, rect.top()), pos2(track_left, rect.bottom())], Stroke::new(1.0, Color32::from_gray(60)));

        // Box selection rectangle.
        if let (Some(b), Some(p)) = (&app.timeline.boxsel, resp.interact_pointer_pos()) {
            let r = Rect::from_two_pos(b.origin, p);
            painter.rect_filled(r, 0.0, ACCENT.gamma_multiply(0.15));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
        }
        let px = view.x(time);
        if px >= track_left - 1.0 && px <= rect.right() {
            painter.line_segment([pos2(px, rect.top()), pos2(px, rect.bottom())], Stroke::new(2.0, ANIMATE));
        }
        if items.is_empty() {
            painter.text(
                pos2(track_left + 16.0, rect.top() + 14.0),
                egui::Align2::LEFT_CENTER,
                tr("No keys yet. Select a bone and press K, or drag it in the viewport."),
                egui::FontId::proportional(SIZE_SMALL),
                TEXT_DIM,
            );
        }

        // ---- interaction
        let row_at = |p: Pos2| -> Option<(usize, &Item)> {
            let i = ((p.y - rect.top()) / ROW_H).floor();
            if i < 0.0 {
                return None;
            }
            items.get(i as usize).map(|it| (i as usize, it))
        };
        let hit_key = |p: Pos2| -> Option<KeyId> {
            let (_, Item::Row(t, ids)) = row_at(p)? else { return None };
            ids.iter().filter(|&&q| (view.x(from_q(q)) - p.x).abs() < 7.0).min_by(|a, b| (view.x(from_q(**a)) - p.x).abs().total_cmp(&(view.x(from_q(**b)) - p.x).abs())).map(|&q| (t.clone(), q))
        };
        let shift = ui.input(|i| i.modifiers.shift);

        if resp.dragged_by(PointerButton::Middle) {
            app.timeline.offset -= resp.drag_delta().x / view.zoom;
        }
        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let Some(o) = origin.filter(|o| o.x >= track_left) {
                if let Some(id) = hit_key(o) {
                    // Ctrl+Shift drags copies of the keys and leaves the originals.
                    let duplicate = ui.input(|i| i.modifiers.command && i.modifiers.shift);
                    if !app.timeline.sel.contains(&id) {
                        if !shift || duplicate {
                            app.timeline.sel.clear();
                        }
                        app.timeline.sel.insert(id.clone());
                    }
                    select_target(app, &id.0);
                    let anchor = from_q(id.1);
                    app.playback.time = anchor;
                    app.playback.playing = false;
                    app.timeline.key_drag = Some(KeyDrag { anchor, grab: view.t(o.x) - anchor, base: app.doc.clone(), delta: 0.0, pushed: false, duplicate });
                } else {
                    app.timeline.boxsel = Some(BoxSel { origin: o, add: shift });
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(kd) = app.timeline.key_drag.take()
                && kd.delta.abs() > 1e-5
            {
                app.timeline.sel = app.timeline.sel.iter().map(|(t, q)| (t.clone(), tq(from_q(*q) + kd.delta))).collect();
            }
            if let (Some(b), Some(p)) = (app.timeline.boxsel.take(), resp.interact_pointer_pos().or(pointer)) {
                let r = Rect::from_two_pos(b.origin, p);
                if !b.add {
                    app.timeline.sel.clear();
                }
                for (i, item) in items.iter().enumerate() {
                    let y = rect.top() + i as f32 * ROW_H;
                    if let Item::Row(t, ids) = item
                        && y + ROW_H > r.top()
                        && y < r.bottom()
                    {
                        for &q in ids {
                            let x = view.x(from_q(q));
                            if x >= r.left() - 4.0 && x <= r.right() + 4.0 {
                                app.timeline.sel.insert((t.clone(), q));
                            }
                        }
                    }
                }
            }
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            if p.x < track_left {
                if let Some((_, Item::Row(t, ids))) = row_at(p) {
                    if matches!(t, Target::Bone(_) | Target::Slot(_)) && p.x < rect.left() + 20.0 {
                        // The arrow opens the row into its properties.
                        if !app.timeline.expanded.remove(t) {
                            app.timeline.expanded.insert(t.clone());
                        }
                    } else {
                        // A row click selects the row and every key beneath it.
                        select_target(app, t);
                        let keys = ids.iter().map(|q| (t.clone(), *q));
                        if shift {
                            app.timeline.sel.extend(keys);
                        } else {
                            app.timeline.sel = keys.collect();
                        }
                    }
                }
            } else if let Some(id) = hit_key(p) {
                if shift {
                    if !app.timeline.sel.remove(&id) {
                        app.timeline.sel.insert(id.clone());
                    }
                } else {
                    app.timeline.sel = BTreeSet::from([id.clone()]);
                }
                select_target(app, &id.0);
                app.playback.time = from_q(id.1);
                app.playback.playing = false;
            } else {
                if !shift {
                    app.timeline.sel.clear();
                }
                if let Some((_, Item::Row(t, _))) = row_at(p) {
                    select_target(app, t);
                }
                let t = view.t(p.x);
                app.playback.time = (if alt { t } else { snap(t, fps) }).clamp(0.0, duration);
                app.playback.playing = false;
            }
        }
        // Double-click on a key deletes it (a collapsed row or the overview deletes what is beneath).
        if resp.double_clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos().filter(|p| p.x >= track_left)
            && let Some(id) = hit_key(p)
        {
            app.timeline.sel = BTreeSet::from([id]);
            delete_keys(app, name);
        }
        // Right-click on a key or row selects it first, then opens the key menu.
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos().filter(|p| p.x >= track_left)
        {
            if let Some(id) = hit_key(p) {
                if !app.timeline.sel.contains(&id) {
                    app.timeline.sel = BTreeSet::from([id.clone()]);
                }
                select_target(app, &id.0);
            } else if let Some((_, Item::Row(t, _))) = row_at(p) {
                select_target(app, t);
            }
        }
        resp.context_menu(|ui| key_menu(app, ui, name));
        if let Some(p) = hover.filter(|p| p.x >= track_left)
            && let Some(id) = hit_key(p)
        {
            resp.on_hover_text(format!("{} · {}", time_label(from_q(id.1), fps), target_label(&id.0)));
        }
    });
}

fn draw_ruler(painter: &egui::Painter, rect: Rect, view: View, ticks: &[(f32, i64, bool)], fps: f32, time: f32, duration: f32) {
    let track = Rect::from_min_max(pos2(rect.left() + LABEL_W, rect.top()), rect.right_bottom());
    painter.rect_filled(rect, 0.0, Color32::from_gray(48));
    painter.text(pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, format!("{} fps", fps), egui::FontId::proportional(SIZE_SMALL), TEXT_DIM);
    let tp = painter.with_clip_rect(track);
    let show_frames = view.zoom / fps >= 12.0;
    for (x, f, major) in ticks {
        let h = if *major { 12.0 } else { 5.0 };
        tp.line_segment([pos2(*x, rect.bottom() - h), pos2(*x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(if *major { 140 } else { 80 })));
        if *major {
            let t = *f as f32 / fps;
            let s = if (t - t.round()).abs() < 1e-3 { format!("{}s", t.round() as i64) } else { format!("{t:.2}s") };
            tp.text(pos2(x + 3.0, rect.top() + 3.0), egui::Align2::LEFT_TOP, s, egui::FontId::proportional(theme::SIZE_BODY), TEXT);
            if show_frames {
                tp.text(pos2(x + 3.0, rect.top() + 20.0), egui::Align2::LEFT_TOP, format!("f{f}"), egui::FontId::proportional(SIZE_SMALL), TEXT_DIM);
            }
        }
    }
    let end_x = view.x(duration);
    tp.line_segment([pos2(end_x, rect.top()), pos2(end_x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(90)));
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, Color32::from_gray(60)));
    let px = view.x(time);
    if px >= track.left() - 6.0 && px <= track.right() + 6.0 {
        tp.add(Shape::convex_polygon(vec![pos2(px - 6.0, rect.bottom() - 11.0), pos2(px + 6.0, rect.bottom() - 11.0), pos2(px, rect.bottom())], ANIMATE, Stroke::NONE));
        tp.line_segment([pos2(px, rect.top()), pos2(px, rect.bottom() - 11.0)], Stroke::new(2.0, ANIMATE));
        let g = tp.layout_no_wrap(time_label(time, fps), egui::FontId::proportional(SIZE_SMALL), Color32::WHITE);
        let pill = Rect::from_min_size(pos2(px + 7.0, rect.top() + 2.0), g.size()).expand2(vec2(4.0, 1.0));
        tp.rect_filled(pill, 3.0, ANIMATE.gamma_multiply(0.9));
        tp.galley(pill.min + vec2(4.0, 1.0), g, Color32::WHITE);
    }
}

/// The numbers a key holds, to tell whether two keys change anything.
fn key_numbers(v: &KeyVal) -> Option<Vec<f32>> {
    let flag = |b: Option<bool>| b.map_or(-1.0e9, |b| f32::from(u8::from(b)));
    let opt = |x: Option<f32>| x.unwrap_or(-1.0e9);
    Some(match v {
        KeyVal::Rotate(x, _) => vec![*x],
        KeyVal::Translate(x, ..) | KeyVal::Scale(x, ..) | KeyVal::Shear(x, ..) => x.to_vec(),
        KeyVal::Color(c, ..) => vec![c.r, c.g, c.b, c.a],
        KeyVal::Ik(k, ..) => vec![opt(k.mix), opt(k.softness), flag(k.bend_positive), flag(k.compress), flag(k.stretch)],
        KeyVal::Transform(m, _) => vec![m.rotate, m.x, m.y, m.scale_x, m.scale_y, m.shear_y],
        KeyVal::Physics(x, _) => vec![*x],
        KeyVal::Deform(_, o, ..) => o.iter().flatten().copied().collect(),
        KeyVal::Attach(..) | KeyVal::PhysicsReset | KeyVal::DrawOrder(_) | KeyVal::Event { .. } => return None,
    })
}

/// The line between two neighbouring keys of a value row: straight when linear,
/// wavy for a curve, dotted when stepped, nothing when the value stays the same.
fn draw_interpolation(p: &egui::Painter, a: &Animation, target: &Target, q: [i64; 2], x: [f32; 2], y: f32, color: Color32) {
    let valued = matches!(target, Target::BoneProp(..) | Target::SlotProp(_, SlotProp::Color | SlotProp::Deform(..)) | Target::Ik(_) | Target::Transform(_))
        || matches!(target, Target::Physics(_, row) if *row != PHYSICS_RESET);
    if !valued {
        return;
    }
    let (v0, v1) = (vals_at(a, target, q[0]), vals_at(a, target, q[1]));
    let (Some(k0), Some(k1)) = (v0.first(), v1.first()) else { return };
    if key_numbers(k0) == key_numbers(k1) {
        return;
    }
    let (x0, x1) = (x[0] + 7.0, x[1] - 7.0);
    if x1 <= x0 {
        return;
    }
    let color = color.gamma_multiply(0.75);
    match val_curve(k0).unwrap_or(Curve::Linear) {
        Curve::Linear => {
            p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.5, color));
        }
        Curve::Stepped => {
            let mut at = x0;
            while at < x1 {
                p.circle_filled(pos2(at, y), 1.1, color);
                at += 4.0;
            }
        }
        _ => {
            let points: Vec<Pos2> = (0..=16).map(|i| {
                let u = i as f32 / 16.0;
                pos2(x0 + (x1 - x0) * u, y - 3.5 * (u * std::f32::consts::TAU).sin())
            }).collect();
            p.add(Shape::line(points, Stroke::new(1.5, color)));
        }
    }
}

fn diamond(painter: &egui::Painter, c: Pos2, color: Color32, selected: bool) {
    let r = if selected { 7.5 } else { 6.0 };
    painter.add(Shape::convex_polygon(
        vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)],
        color,
        Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { Color32::WHITE } else { Color32::from_black_alpha(200) }),
    ));
}

#[cfg(test)]
mod keying_tests {
    use super::*;
    use crate::app::Tool;

    /// Two bones with a mesh slot, an IK chain and an animation "a" with a few keys.
    fn rig() -> Document {
        let doc = skel2d_core::from_str(r#"{"bones":[{"name":"root"}],"slots":[{"name":"s","bone":"root","attachment":"m","attachments":[{"type":"mesh","name":"m","vertices":[{"x":0,"y":0,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":0},{"x":0,"y":1,"u":0,"v":1}],"triangles":[[0,1,2]]}]}]}"#).unwrap();
        skel2d_core::apply(&doc, &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"up","parent":"root","length":10},
            {"op":"add_bone","name":"low","parent":"up","length":10},
            {"op":"add_bone","name":"goal","parent":"root"},
            {"op":"add_ik","name":"leg","bones":["up","low"],"target":"goal"},
            {"op":"key_bone","animation":"a","bone":"up","time":0,"rotate":0,"translate":[0,0]},
            {"op":"key_bone","animation":"a","bone":"up","time":1,"rotate":30,"translate":[10,0]}
        ]"#).unwrap()).unwrap()
    }

    fn app_at(time: f32, auto_key: bool) -> EditorApp {
        let mut app = EditorApp::test_app(rig(), std::path::PathBuf::from("t.skel.json"));
        app.settings.auto_key = auto_key;
        app.playback.anim = Some("a".into());
        app.playback.time = time;
        app.selection.bone = Some("up".into());
        app
    }

    fn rotate_op(bone: &str, time: f32, value: f32) -> Op {
        Op::KeyBone(KeyBone { animation: "a".into(), bone: bone.into(), time, rotate: Some(value), translate: None, scale: None, shear: None, curve: None, curves: None })
    }

    fn times<T>(keys: &[skel2d_core::Key<T>]) -> Vec<f32> {
        keys.iter().map(|k| k.time).collect()
    }

    #[test]
    fn key_edited_writes_only_the_changed_fields_in_one_undo_step() {
        let mut app = app_at(0.5, false);
        let before = app.doc.clone();
        assert!(app.edit(vec![rotate_op("up", 0.5, 40.0)]));
        assert_eq!(app.doc, before, "an unkeyed change does not touch the document");
        assert_eq!(app.pending_count(), 1);
        assert!(key_edited(&mut app));
        let tl = &app.doc.animations["a"].bones["up"];
        assert_eq!(times(&tl.rotate), [0.0, 0.5, 1.0]);
        assert_eq!(tl.rotate[1].value, 40.0);
        assert_eq!(times(&tl.translate), [0.0, 1.0], "translate must not get a key");
        assert!(tl.scale.is_empty() && tl.shear.is_empty());
        assert!(!app.has_pending());
        app.undo();
        assert_eq!(app.doc, before, "one undo undoes the whole K");
    }

    #[test]
    fn key_edited_keys_every_pending_change_of_every_target_atomically() {
        let mut app = app_at(0.5, false);
        let before = app.doc.clone();
        assert!(app.edit(vec![rotate_op("up", 0.5, 12.0)]));
        assert!(app.edit(vec![Op::KeyDeform { animation: "a".into(), slot: "s".into(), attachment: "m".into(), skin: None, time: 0.5, offsets: vec![[2.0, 0.0]; 3], curve: None }]));
        assert!(app.edit(vec![Op::KeyIk { animation: "a".into(), ik: "leg".into(), time: 0.5, mix: Some(0.25), softness: None, bend_positive: None, compress: None, stretch: None, curve: None, curves: None }]));
        assert_eq!((app.pending.len(), app.pending_other.len(), app.pending_count()), (1, 2, 3));
        assert_eq!(app.doc, before);
        // The viewport and inspectors see them as if keyed.
        let shown = app.shown_doc();
        assert_eq!(shown.animations["a"].ik["leg"][0].value.mix, Some(0.25));
        assert_eq!(shown.animations["a"].deform["default"]["s"]["m"][0].value[0], [2.0, 0.0]);
        drop(shown);
        assert!(key_edited(&mut app));
        let a = &app.doc.animations["a"];
        assert_eq!(a.ik["leg"][0].value.mix, Some(0.25));
        assert_eq!(a.ik["leg"][0].value.softness, None, "only the mix was changed");
        assert_eq!(a.deform["default"]["s"]["m"][0].time, 0.5);
        assert_eq!(a.bones["up"].rotate[1].value, 12.0);
        assert_eq!(app.pending_count(), 0);
        app.undo();
        assert_eq!(app.doc, before, "one undo step covers bone, deform and IK");
    }

    #[test]
    fn a_mix_with_a_direct_write_is_written_whole_not_half_held() {
        let mut app = app_at(0.5, false);
        let ops = vec![rotate_op("up", 0.5, 5.0), Op::KeyDrawOrder { animation: "a".into(), time: 0.5, order: None }];
        assert!(app.edit(ops));
        assert!(!app.has_pending());
        assert_eq!(app.doc.animations["a"].bones["up"].rotate[1].value, 5.0);
        assert_eq!(app.doc.animations["a"].draw_order.len(), 1);
    }

    #[test]
    fn moving_the_playhead_discards_pending_changes_and_counts_them() {
        let mut app = app_at(0.5, false);
        app.edit(vec![rotate_op("up", 0.5, 12.0)]);
        app.edit(vec![Op::KeyIk { animation: "a".into(), ik: "leg".into(), time: 0.5, mix: Some(0.5), softness: None, bend_positive: None, compress: None, stretch: None, curve: None, curves: None }]);
        app.drop_stale_pending();
        assert!(app.has_pending(), "nothing moved yet");
        app.playback.time = 0.75;
        app.drop_stale_pending();
        assert!(!app.has_pending());
        assert_eq!(app.status, crate::tf!("Discarded {} unkeyed changes (key them with K or the key buttons before moving the playhead)", 2));
        assert!(!app.doc.animations["a"].ik.contains_key("leg"));
    }

    #[test]
    fn with_auto_key_on_edits_are_written_at_once() {
        let mut app = app_at(0.5, true);
        app.edit(vec![rotate_op("up", 0.5, 12.0)]);
        assert!(!app.has_pending());
        assert_eq!(app.doc.animations["a"].bones["up"].rotate[1].value, 12.0);
    }

    #[test]
    fn k_without_changes_keys_the_tool_property_only() {
        for (tool, rotate, translate, scale) in [(Tool::Rotate, 3, 2, 0), (Tool::Translate, 2, 3, 0), (Tool::Scale, 2, 2, 1)] {
            let mut app = app_at(0.5, true);
            app.tool = tool;
            assert!(key_edited(&mut app));
            let tl = &app.doc.animations["a"].bones["up"];
            assert_eq!((tl.rotate.len(), tl.translate.len(), tl.scale.len(), tl.shear.len()), (rotate, translate, scale, 0), "{tool:?}");
        }
    }

    #[test]
    fn k_with_the_pose_tool_only_shows_a_hint() {
        let mut app = app_at(0.5, true);
        app.tool = Tool::Select;
        let before = app.doc.clone();
        assert!(!key_edited(&mut app));
        assert_eq!(app.doc, before);
        assert!(app.status.contains("Ctrl+Shift+L"), "{}", app.status);
        // A selected slot keys its attachment, not the tint or the bone.
        app.selection.slot = Some("s".into());
        assert!(key_edited(&mut app));
        let tl = &app.doc.animations["a"].slots["s"];
        assert_eq!((tl.attachment.len(), tl.color.len()), (1, 0));
        assert_eq!(app.doc.animations["a"].bones["up"].rotate.len(), 2);
    }

    #[test]
    fn key_shown_keys_every_property_of_the_shown_rows_with_current_values() {
        let mut app = app_at(0.5, true);
        app.selection.slot = None;
        assert!(key_shown(&mut app));
        let tl = &app.doc.animations["a"].bones["up"];
        assert_eq!(times(&tl.rotate), [0.0, 0.5, 1.0]);
        assert!((tl.rotate[1].value - 15.0).abs() < 1e-3, "the interpolated value is captured, got {}", tl.rotate[1].value);
        assert_eq!(tl.translate[1].value, [5.0, 0.0]);
        assert_eq!(times(&tl.scale), [0.5]);
        assert_eq!(tl.scale[0].value, [1.0, 1.0]);
        assert_eq!(times(&tl.shear), [0.5]);
        // Rows that are not shown stay untouched.
        assert!(!app.doc.animations["a"].bones.contains_key("low"));
    }

    #[test]
    fn retiming_one_property_leaves_the_others_and_a_collapsed_row_moves_all() {
        let doc = app_at(0.0, true).doc;
        let rotate = BTreeSet::from([(Target::BoneProp("up".into(), BoneProp::Rotate), tq(0.0))]);
        let moved = skel2d_core::apply(&doc, &retime_ops(&doc, "a", &rotate, 0.5)).unwrap();
        let tl = &moved.animations["a"].bones["up"];
        assert_eq!(times(&tl.rotate), [0.5, 1.0]);
        assert_eq!(times(&tl.translate), [0.0, 1.0], "translate stays");
        let row = BTreeSet::from([(Target::Bone("up".into()), tq(0.0))]);
        let moved = skel2d_core::apply(&doc, &retime_ops(&doc, "a", &row, 0.5)).unwrap();
        let tl = &moved.animations["a"].bones["up"];
        assert_eq!((times(&tl.rotate), times(&tl.translate)), (vec![0.5, 1.0], vec![0.5, 1.0]));
        // Copying a property keeps just that property.
        let a = &doc.animations["a"];
        let clip = vals_at(a, &Target::BoneProp("up".into(), BoneProp::Translate), tq(1.0));
        assert!(matches!(clip.as_slice(), [KeyVal::Translate(..)]));
    }

    #[test]
    fn overview_delete_removes_every_key_at_that_time_and_duplicate_keeps_originals() {
        let doc = skel2d_core::apply(&app_at(0.0, true).doc, &[
            Op::KeyIk { animation: "a".into(), ik: "leg".into(), time: 1.0, mix: Some(0.5), softness: None, bend_positive: None, compress: None, stretch: None, curve: None, curves: None },
            Op::KeyDrawOrder { animation: "a".into(), time: 1.0, order: None },
        ]).unwrap();
        let a = &doc.animations["a"];
        let ids = BTreeSet::from([(Target::All, tq(1.0))]);
        let gone = skel2d_core::apply(&doc, &delete_ops(a, "a", &ids)).unwrap();
        let g = &gone.animations["a"];
        assert_eq!(times(&g.bones["up"].rotate), [0.0]);
        assert_eq!(times(&g.bones["up"].translate), [0.0]);
        assert!(g.ik.is_empty() && g.draw_order.is_empty());
        let copied = skel2d_core::apply(&doc, &retime_ops_with(&doc, "a", &ids, 0.5, true)).unwrap();
        let c = &copied.animations["a"];
        assert_eq!(times(&c.bones["up"].rotate), [0.0, 1.0, 1.5]);
        assert_eq!(times(&c.ik["leg"]), [1.0, 1.5]);
    }

    // ---- synthetic input

    struct Sheet {
        ctx: egui::Context,
        app: EditorApp,
    }

    impl Sheet {
        fn new(app: EditorApp) -> Self {
            let mut s = Sheet { ctx: egui::Context::default(), app };
            s.frame(vec![]);
            s
        }
        fn frame(&mut self, events: Vec<egui::Event>) {
            let modifiers = events.iter().find_map(|e| if let egui::Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let events: Vec<egui::Event> = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))), ..Default::default() };
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(input, |ui| {
                shortcuts(app, &ui.ctx().clone());
                dope_sheet(app, ui, "a");
            });
            output.textures_delta.clear();
        }
        fn row(&self, target: &Target) -> Rect {
            self.app.timeline.rows.iter().find(|(t, _)| t == target).map(|r| r.1).unwrap_or_else(|| panic!("no row {target:?}: {:?}", self.app.timeline.rows.iter().map(|r| &r.0).collect::<Vec<_>>()))
        }
        fn key_x(&self, row: Rect, t: f32) -> f32 {
            row.left() + LABEL_W + (t - self.app.timeline.offset) * self.app.timeline.zoom
        }
        fn click(&mut self, at: Pos2) {
            let button = |pressed| egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
        }
        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        }
    }

    #[test]
    fn clicking_the_arrow_opens_the_property_rows_and_a_row_click_selects_its_keys() {
        let mut s = Sheet::new(app_at(0.0, true));
        let bone = s.row(&Target::Bone("up".into()));
        assert!(s.app.timeline.rows.iter().all(|(t, _)| !matches!(t, Target::BoneProp(..))), "rows start closed");
        s.click(pos2(bone.left() + 12.0, bone.center().y));
        s.frame(vec![]);
        assert!(s.app.timeline.expanded.contains(&Target::Bone("up".into())));
        for p in BONE_PROPS {
            s.row(&Target::BoneProp("up".into(), p));
        }
        // Clicking the property's name selects the keys beneath it.
        let rotate = s.row(&Target::BoneProp("up".into(), BoneProp::Rotate));
        s.click(pos2(rotate.left() + 60.0, rotate.center().y));
        assert_eq!(s.app.timeline.sel, BTreeSet::from([(Target::BoneProp("up".into(), BoneProp::Rotate), tq(0.0)), (Target::BoneProp("up".into(), BoneProp::Rotate), tq(1.0))]));
        // The arrow closes it again.
        let bone = s.row(&Target::Bone("up".into()));
        s.click(pos2(bone.left() + 12.0, bone.center().y));
        s.frame(vec![]);
        assert!(s.app.timeline.expanded.is_empty());
    }

    #[test]
    fn double_clicking_a_key_deletes_it() {
        let mut s = Sheet::new(app_at(0.0, true));
        let row = s.row(&Target::Bone("up".into()));
        let at = pos2(s.key_x(row, 1.0), row.center().y);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        s.frame(vec![egui::Event::PointerMoved(at)]);
        for _ in 0..2 {
            s.frame(vec![button(true)]);
            s.frame(vec![button(false)]);
        }
        s.frame(vec![]);
        let tl = &s.app.doc.animations["a"].bones["up"];
        assert_eq!((times(&tl.rotate), times(&tl.translate)), (vec![0.0], vec![0.0]), "the collapsed row deletes every property key at that time");
    }

    #[test]
    fn k_and_ctrl_shift_l_work_from_the_keyboard() {
        let mut app = app_at(0.5, false);
        app.edit(vec![rotate_op("up", 0.5, 20.0)]);
        let mut s = Sheet::new(app);
        s.key(egui::Key::K, egui::Modifiers::NONE);
        let tl = s.app.doc.animations["a"].bones["up"].clone();
        assert_eq!((times(&tl.rotate), times(&tl.translate)), (vec![0.0, 0.5, 1.0], vec![0.0, 1.0]));
        let ctrl_shift = egui::Modifiers { ctrl: true, command: true, shift: true, ..Default::default() };
        s.key(egui::Key::L, ctrl_shift);
        let tl = &s.app.doc.animations["a"].bones["up"];
        assert_eq!((tl.translate.len(), tl.scale.len(), tl.shear.len()), (3, 1, 1));
    }

    #[test]
    fn ctrl_a_selects_the_row_then_everything_shown() {
        let mut s = Sheet::new(app_at(0.0, true));
        // Pointer over the timeline, as the shortcut requires.
        let row = s.row(&Target::Bone("up".into()));
        s.frame(vec![egui::Event::PointerMoved(row.center())]);
        // `ui()` sets this from the pointer; the test drives the dope sheet directly.
        s.app.timeline.hovered = true;
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        s.key(egui::Key::A, ctrl);
        assert_eq!(s.app.timeline.sel.len(), 2, "the selected row's keys first");
        s.key(egui::Key::A, ctrl);
        // Only one bone row is shown here, so "everything shown" is the same two keys.
        assert!(s.app.timeline.sel.len() >= 2, "then every shown row");
    }
}

#[cfg(test)]
mod shear_tests {
    use super::*;
    #[test]
    fn physics_keys_retime_copy_and_delete_preserve_values_and_curves() {
        let doc = skel2d_core::apply(&Document::new("timeline"), &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"b"},
            {"op":"add_physics","name":"c","bone":"b"},
            {"op":"key_physics","animation":"a","constraint":"c","property":"mass","time":0,"value":0.25,"curve":"stepped"},
            {"op":"key_physics","animation":"a","constraint":"","property":"wind","time":0,"value":4,"curve":"ease_out"},
            {"op":"key_physics_reset","animation":"a","constraint":"c","time":0},
            {"op":"key_physics_reset","animation":"a","time":0}
        ]"#).unwrap()).unwrap();
        let a = &doc.animations["a"];
        let mass = Target::Physics("c".into(), 3);
        let wind = Target::Physics(String::new(), 4);
        let reset = Target::Physics("c".into(), PHYSICS_RESET);
        let all_reset = Target::Physics(String::new(), PHYSICS_RESET);
        for t in [&mass, &wind, &reset, &all_reset] { assert_eq!(all_ids(a, t), [tq(0.0)]); }
        assert_eq!(target_label(&wind), format!("{} · {}", tr("All physics"), tr("wind")));
        let sel: BTreeSet<KeyId> = [&mass, &wind, &reset, &all_reset].into_iter().map(|t| (t.clone(), tq(0.0))).collect();
        let moved = skel2d_core::apply(&doc, &retime_ops(&doc, "a", &sel, 1.0)).unwrap();
        let (c, g) = (&moved.animations["a"].physics["c"], &moved.animations["a"].physics[""]);
        assert_eq!((c.mass[0].time, c.mass[0].value, c.mass[0].curve), (1.0, 0.25, Curve::Stepped));
        assert_eq!((g.wind[0].time, g.wind[0].value, g.wind[0].curve), (1.0, 4.0, Curve::EaseOut));
        assert_eq!((&c.reset, &g.reset), (&vec![1.0], &vec![1.0]));
        // Copy to another animation and delete from the source.
        let copy: Vec<Op> = sel.iter().flat_map(|(t, q)| vals_at(a, t, *q).into_iter().map(move |v| key_op("copy", t, 0.5, &v))).collect();
        let copied = skel2d_core::apply(&doc, &copy).unwrap();
        assert_eq!(copied.animations["copy"].physics["c"].mass[0].curve, Curve::Stepped);
        assert_eq!(copied.animations["copy"].physics[""].reset, [0.5]);
        let removal: Vec<Op> = sel.iter().flat_map(|(t, q)| vals_at(a, t, *q).into_iter().map(move |v| remove_op("a", t, 0.0, &v))).collect();
        let removed = skel2d_core::apply(&doc, &removal).unwrap();
        assert!(removed.animations["a"].physics.is_empty());
    }

    /// Retimes `target`'s key at 0 by one second, copies it to another animation
    /// and deletes it, returning the three documents.
    fn move_copy_delete(doc: &Document, target: &Target) -> (Document, Document, Document) {
        let a = &doc.animations["a"];
        let sel = BTreeSet::from([(target.clone(), tq(0.0))]);
        assert!(all_ids(a, target).contains(&tq(0.0)));
        let moved = skel2d_core::apply(doc, &retime_ops(doc, "a", &sel, 1.0)).unwrap();
        let copy: Vec<Op> = vals_at(a, target, tq(0.0)).iter().map(|v| key_op("copy", target, 0.5, v)).collect();
        let copied = skel2d_core::apply(doc, &copy).unwrap();
        let removal: Vec<Op> = vals_at(a, target, tq(0.0)).iter().map(|v| remove_op("a", target, 0.0, v)).collect();
        let removed = skel2d_core::apply(doc, &removal).unwrap();
        (moved, copied, removed)
    }

    #[test]
    fn draw_order_keys_retime_copy_and_delete() {
        let doc = skel2d_core::apply(&Document::with_root("t"), &skel2d_core::parse_ops(r#"[
            {"op":"add_slot","name":"s1","bone":"root"},
            {"op":"add_slot","name":"s2","bone":"root"},
            {"op":"add_slot","name":"s3","bone":"root"}
        ]"#).unwrap()).unwrap();
        let order = vec!["s3".to_string(), "s1".to_string(), "s2".to_string()];
        let doc = skel2d_core::apply(&doc, &[
            Op::KeyDrawOrder { animation: "a".into(), time: 0.0, order: Some(order.clone()) },
            Op::KeyDrawOrder { animation: "a".into(), time: 2.0, order: None },
        ]).unwrap();
        let (moved, copied, removed) = move_copy_delete(&doc, &Target::DrawOrder);
        assert_eq!(moved.animations["a"].draw_order.iter().map(|k| (k.time, k.order.clone())).collect::<Vec<_>>(), [(1.0, Some(order.clone())), (2.0, None)]);
        assert_eq!(copied.animations["copy"].draw_order[0].order, Some(order));
        assert_eq!(copied.animations["copy"].draw_order[0].time, 0.5);
        assert_eq!(removed.animations["a"].draw_order.len(), 1);
        assert_eq!(removed.animations["a"].draw_order[0].time, 2.0);
        // A null key (setup order) survives too.
        let values = vals_at(&doc.animations["a"], &Target::DrawOrder, tq(2.0));
        assert!(matches!(values.as_slice(), [KeyVal::DrawOrder(None)]));
        assert_eq!(target_label(&Target::DrawOrder), tr("Draw order"));
    }

    #[test]
    fn event_keys_retime_copy_and_delete_keep_their_values() {
        let doc = skel2d_core::apply(&Document::with_root("t"), &[
            Op::AddEvent { name: "step".into(), int: 1, float: 0.5, string: "dust".into() },
            Op::AddEvent { name: "hit".into(), int: 0, float: 0.0, string: String::new() },
            Op::KeyEvent { animation: "a".into(), name: "step".into(), time: 0.0, int: Some(7), float: None, string: Some("mud".into()) },
            Op::KeyEvent { animation: "a".into(), name: "hit".into(), time: 0.0, int: None, float: Some(2.5), string: None },
        ]).unwrap();
        let step = Target::Event("step".into());
        let (moved, copied, removed) = move_copy_delete(&doc, &step);
        let key = |d: &Document, anim: &str, name: &str| d.animations[anim].events.iter().find(|k| k.name == name).cloned().unwrap();
        let k = key(&moved, "a", "step");
        assert_eq!((k.time, k.int, k.float, k.string.as_deref()), (1.0, Some(7), None, Some("mud")));
        let k = key(&copied, "copy", "step");
        assert_eq!((k.time, k.int, k.float, k.string.as_deref()), (0.5, Some(7), None, Some("mud")));
        // Only this event's key goes; the other event at the same time stays.
        assert!(removed.animations["a"].events.iter().all(|k| k.name == "hit"));
        assert_eq!(key(&moved, "a", "hit").time, 0.0);
        let hit = vals_at(&doc.animations["a"], &Target::Event("hit".into()), tq(0.0));
        assert!(matches!(hit.as_slice(), [KeyVal::Event { int: None, float: Some(f), string: None }] if *f == 2.5));
    }

    #[test]
    fn ik_keys_retime_copy_and_delete_keep_every_field_and_channel_curves() {
        let doc = skel2d_core::apply(&Document::with_root("t"), &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"up","parent":"root","length":10},
            {"op":"add_bone","name":"low","parent":"up","length":10},
            {"op":"add_bone","name":"goal","parent":"root"},
            {"op":"add_ik","name":"leg","bones":["up","low"],"target":"goal"}
        ]"#).unwrap()).unwrap();
        let doc = skel2d_core::apply(&doc, &[Op::KeyIk {
            animation: "a".into(), ik: "leg".into(), time: 0.0,
            mix: Some(0.5), softness: Some(4.0), bend_positive: Some(false), compress: Some(true), stretch: Some(true),
            curve: Some(Curve::Stepped), curves: Some(vec![Curve::EaseIn, Curve::EaseOut]),
        }]).unwrap();
        let want = doc.animations["a"].ik["leg"][0].clone();
        assert_eq!(want.curves, [Curve::EaseIn, Curve::EaseOut]);
        let (moved, copied, removed) = move_copy_delete(&doc, &Target::Ik("leg".into()));
        let k = &moved.animations["a"].ik["leg"][0];
        assert_eq!((k.time, k.value, k.curve, &k.curves), (1.0, want.value, Curve::Stepped, &want.curves));
        let k = &copied.animations["copy"].ik["leg"][0];
        assert_eq!((k.time, k.value, k.curve, &k.curves), (0.5, want.value, Curve::Stepped, &want.curves));
        assert!(!removed.animations["a"].ik.contains_key("leg"));
        // Changing the easing replaces per-channel curves like color does.
        let v = vals_at(&doc.animations["a"], &Target::Ik("leg".into()), tq(0.0)).remove(0);
        let KeyVal::Ik(value, curve, curves) = with_curve(v, Curve::EaseIn, &None) else { panic!("kind changed") };
        assert_eq!((value, curve, curves.len()), (want.value, Curve::EaseIn, 0));
    }

    #[test]
    fn deform_retime_copy_and_delete_preserve_both_owners() {
        let doc = skel2d_core::from_str(r#"{"bones":[{"name":"root"}],"slots":[{"name":"s","bone":"root","attachment":"m","attachments":[{"type":"mesh","name":"m","vertices":[{"x":0,"y":0,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":0},{"x":0,"y":1,"u":0,"v":1}],"triangles":[[0,1,2]]}]}],"skins":[{"name":"armor","attachments":{"s":[{"type":"mesh","name":"m","vertices":[{"x":0,"y":0,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":0},{"x":0,"y":1,"u":0,"v":1}],"triangles":[[0,1,2]]}]}}]}"#).unwrap();
        let doc = skel2d_core::apply(&doc, &[
            Op::KeyDeform { animation:"a".into(), slot:"s".into(), attachment:"m".into(), skin:None, time:0.0, offsets:vec![[1.0,0.0];3], curve:Some(Curve::Stepped) },
            Op::KeyDeform { animation:"a".into(), slot:"s".into(), attachment:"m".into(), skin:Some("armor".into()), time:0.0, offsets:vec![[2.0,0.0];3], curve:Some(Curve::EaseOut) },
        ]).unwrap();
        let target = Target::Slot("s".into());
        let values = vals_at(&doc.animations["a"], &target, tq(0.0));
        assert_eq!(values.len(), 2);
        let copied = skel2d_core::apply(&doc, &values.iter().map(|v| key_op("copy", &target, 0.5, v)).collect::<Vec<_>>()).unwrap();
        for skin in ["default", "armor"] {
            assert_eq!(copied.animations["copy"].deform[skin]["s"]["m"][0].value, doc.animations["a"].deform[skin]["s"]["m"][0].value);
        }
        let moved = skel2d_core::apply(&doc, &retime_ops(&doc,"a",&BTreeSet::from([(target.clone(),tq(0.0))]),1.0)).unwrap();
        for skin in ["default", "armor"] {
            let key = &moved.animations["a"].deform[skin]["s"]["m"][0];
            assert_eq!(key.time, 1.0);
            assert_eq!(key.curve, doc.animations["a"].deform[skin]["s"]["m"][0].curve);
        }
        let values = vals_at(&moved.animations["a"], &target, tq(1.0));
        let removed = skel2d_core::apply(&moved, &values.iter().map(|v| remove_op("a", &target, 1.0, v)).collect::<Vec<_>>()).unwrap();
        assert!(removed.animations["a"].deform.is_empty());
    }
    #[test]
    fn clicking_the_animation_box_opens_its_list() {
        let mut app = EditorApp::test_app(Document::new("t"), std::path::PathBuf::from("t.skel.json"));
        let ctx = egui::Context::default();
        let mut combo = Rect::NOTHING;
        let mut frame = |ctx: &egui::Context, events: Vec<egui::Event>, combo: &mut Rect| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                toolbar(&mut app, ui);
                *combo = ui.min_rect();
            });
            output.textures_delta.clear();
        };
        frame(&ctx, vec![], &mut combo);
        let at = pos2(combo.left() + 40.0, combo.top() + 10.0);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, vec![egui::Event::PointerMoved(at)], &mut combo);
        frame(&ctx, vec![button(true)], &mut combo);
        frame(&ctx, vec![button(false)], &mut combo);
        frame(&ctx, vec![], &mut combo);
        assert!(egui::Popup::is_any_open(&ctx), "the list should stay open after a click");
    }

    #[test]
    fn curves_can_be_set_on_every_kind_of_key() {
        let doc = skel2d_core::apply(&Document::with_root("t"), &skel2d_core::parse_ops(r##"[
            {"op":"add_bone","name":"a"},
            {"op":"add_slot","name":"s","bone":"a"},
            {"op":"add_transform","name":"c","bones":["a"],"source":"root"},
            {"op":"add_physics","name":"p","bone":"a"},
            {"op":"key_slot","animation":"m","slot":"s","time":0,"color":"#ff0000"},
            {"op":"key_transform","animation":"m","constraint":"c","time":0,"mix":{}},
            {"op":"key_physics","animation":"m","constraint":"p","property":"wind","time":0,"value":1}
        ]"##).unwrap()).unwrap();
        let mut app = EditorApp::test_app(doc, std::path::PathBuf::from("t.skel.json"));
        set_curve(&mut app, "m", &Target::Slot("s".into()), 0.0, Curve::Stepped, false);
        set_curve(&mut app, "m", &Target::Transform("c".into()), 0.0, Curve::EaseIn, false);
        let wind = PHYSICS_ROWS.iter().position(|p| *p == PhysicsProperty::Wind).unwrap() as u8;
        set_curve(&mut app, "m", &Target::Physics("p".into(), wind), 0.0, Curve::EaseOut, false);
        let m = &app.doc.animations["m"];
        assert_eq!(m.slots["s"].color[0].curve, Curve::Stepped);
        assert_eq!(m.transforms["c"][0].curve, Curve::EaseIn);
        assert_eq!(m.physics["p"].wind[0].curve, Curve::EaseOut);
    }

    #[test]
    fn retiming_and_copying_keep_channel_curves() {
        let doc=skel2d_core::apply(&Document::new("timeline"), &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"bone"},
            {"op":"key_bone","animation":"anim","bone":"bone","time":0,"translate":[1,2],"curves":["stepped","ease_in"]}
        ]"#).unwrap()).unwrap();
        let target=Target::Bone("bone".into());
        let moved=skel2d_core::apply(&doc,&retime_ops(&doc,"anim",&BTreeSet::from([(target.clone(),tq(0.0))]),1.0)).unwrap();
        assert_eq!(moved.animations["anim"].bones["bone"].translate[0].curves,[Curve::Stepped,Curve::EaseIn]);
        let values=vals_at(&doc.animations["anim"],&target,tq(0.0));
        let copied=skel2d_core::apply(&doc,&[key_op("copy",&target,0.5,&values[0])]).unwrap();
        assert_eq!(copied.animations["copy"].bones["bone"].translate[0].curves,[Curve::Stepped,Curve::EaseIn]);
    }

    #[test]
    fn retiming_and_copying_shear_preserve_values_and_curves() {
        let doc=skel2d_core::apply(&Document::new("timeline"), &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"bone"},
            {"op":"key_bone","animation":"anim","bone":"bone","time":0,"shear":[10,20],"curve":"stepped"}
        ]"#).unwrap()).unwrap();
        let target=Target::Bone("bone".into());
        let values=vals_at(&doc.animations["anim"],&target,tq(0.0));
        assert_eq!(values.len(),1);
        let copied=key_op("copy",&target,0.5,&values[0]);
        let copied=skel2d_core::apply(&doc,&[copied]).unwrap();
        let key=&copied.animations["copy"].bones["bone"].shear[0];
        assert_eq!(key.value,[10.0,20.0]);
        assert_eq!(key.curve,Curve::Stepped);
        let moved=skel2d_core::apply(&doc,&retime_ops(&doc,"anim",&BTreeSet::from([(target.clone(),tq(0.0))]),1.0)).unwrap();
        assert_eq!(all_ids(&moved.animations["anim"],&target),[tq(1.0)]);
        assert_eq!(moved.animations["anim"].bones["bone"].shear[0].curve,Curve::Stepped);
    }
}
