//! Side panels: hierarchy (bones and draw order) and the property inspector.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use eframe::egui::{self, Color32, DragValue, RichText};
use skel2d_core::ops::{BonePatch, IkPatch, KeyBone, SlotPatch};
use skel2d_core::pose::sample;
use skel2d_core::{Attachment, Bone, Color, IkConstraint, Op, Region, Skin, Slot};

use crate::app::{EditorApp, Tool};
use crate::i18n::tr;
use crate::theme::{self, ACCENT, ANIMATE, BONE, IK, PHYSICS, TEXT, TEXT_DIM};

mod animextra;
pub(crate) use animextra::{current_order as current_draw_order, draw_order_key_here, event_key_here, key_draw_order, key_event_now, move_slot as move_slot_in_draw_order, remove_draw_order_key, remove_event_key};
mod physics;
mod transform;
mod stepui;
pub(crate) use stepui::add_images as add_images_to_slot;
pub use stepui::{side, step_bar, tools};
#[cfg(test)]
pub(crate) use stepui::tests as stepui_tests;

/// Why root cannot be removed, renamed or moved (shown wherever root is offered).
pub(crate) const ROOT_REASON: &str = "Root is the base of the skeleton: every bone hangs under it, so it cannot be deleted, renamed or moved.";

const LABEL_W: f32 = theme::LABEL_W;
const TIME_EPS: f32 = skel2d_core::ops::TIME_EPSILON;

fn unique_name(prefix: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..).map(|i| format!("{prefix}{i}")).find(|n| !taken(n)).expect("unbounded range")
}

/// Right-aligned label in a fixed-width column, with an explanatory tooltip.
fn lbl(ui: &mut egui::Ui, text: &str) {
    lbl_tip(ui, text, "");
}

fn lbl_tip(ui: &mut egui::Ui, text: &str, tip: &str) {
    lbl_tip_w(ui, LABEL_W, text, tip);
}

fn lbl_tip_w(ui: &mut egui::Ui, width: f32, text: &str, tip: &str) {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, tr(text), egui::FontId::proportional(theme::SIZE_BODY), TEXT_DIM);
    if !tip.is_empty() {
        r.on_hover_text(tr(tip));
    }
}

fn tip_text(tip: &str) -> String {
    format!("{}\n{}", tr(tip), tr("Drag to change · Shift: fine · double-click to type"))
}

fn set_flag(set: &mut BTreeSet<String>, name: &str, on: bool) {
    if on {
        set.insert(name.to_string());
    } else {
        set.remove(name);
    }
}

fn add_bone(app: &mut EditorApp, parent: Option<String>) {
    let name = unique_name("bone", |n| app.doc.bone(n).is_some());
    let mut bone = Bone::new(&name);
    bone.parent = parent;
    bone.length = 40.0;
    if let Some(parent) = bone.parent.as_deref().and_then(|p| app.doc.bone(p)) {
        bone.x = parent.length;
    }
    if app.edit(vec![Op::AddBone(bone)]) {
        app.selection.bone = Some(name);
        app.selection.slot = None;
    }
}

fn add_slot(app: &mut EditorApp, bone: String) {
    let name = unique_name(&format!("{bone}_slot"), |n| app.doc.slot(n).is_some());
    let length = app.doc.bone(&bone).map_or(40.0, |b| b.length.abs().max(20.0));
    let slot = Slot {
        name: name.clone(),
        bone,
        color: Color::WHITE,
        blend: Default::default(),
        attachment: Some("region".into()),
        attachments: vec![Attachment::Region(Region {
            name: "region".into(),
            image: None,
            x: length / 2.0,
            y: 0.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            width: length,
            height: (length * 0.4).max(10.0),
        })],
    };
    if app.edit(vec![Op::AddSlot { slot, index: None }]) {
        app.selection.slot = Some(name);
    }
}

/// A constraint that touches a bone, shown as a small glyph on its tree row.
#[derive(Clone, Debug)]
struct Badge {
    kind: BadgeKind,
    name: String,
    /// The bone is the constraint's target or source, not one of the bones it moves.
    target: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BadgeKind {
    Ik,
    Transform,
    Physics,
}

/// Every bone's constraint badges, built once per frame from the constraint lists.
fn constraint_badges(doc: &skel2d_core::Document) -> std::collections::HashMap<String, Vec<Badge>> {
    let mut map: std::collections::HashMap<String, Vec<Badge>> = std::collections::HashMap::new();
    let mut add = |bone: &str, kind: BadgeKind, name: &str, target: bool| map.entry(bone.to_string()).or_default().push(Badge { kind, name: name.to_string(), target });
    for c in &doc.ik {
        c.bones.iter().for_each(|b| add(b, BadgeKind::Ik, &c.name, false));
        add(&c.target, BadgeKind::Ik, &c.name, true);
    }
    for c in &doc.transforms {
        c.bones.iter().for_each(|b| add(b, BadgeKind::Transform, &c.name, false));
        add(&c.source, BadgeKind::Transform, &c.name, true);
    }
    for c in &doc.physics {
        add(&c.bone, BadgeKind::Physics, &c.name, false);
    }
    map
}

/// "IK leg · this bone is moved by it" for the tooltip and the overflow menu.
fn badge_text(b: &Badge) -> String {
    let kind = match b.kind {
        BadgeKind::Ik => tr("IK constraint"),
        BadgeKind::Transform => tr("Copy constraint"),
        BadgeKind::Physics => tr("Physics constraint"),
    };
    let role = match (b.kind, b.target) {
        (_, false) => tr("this bone is moved by it"),
        (BadgeKind::Transform, true) => tr("this bone is its source"),
        _ => tr("this bone is its target"),
    };
    format!("{kind} {} · {role}", b.name)
}

fn select_constraint(app: &mut EditorApp, tc: &mut TreeCtx, badge: &Badge) {
    match badge.kind {
        BadgeKind::Ik => {
            app.selection = crate::app::Selection { bone: app.selection.bone.clone(), ik: Some(badge.name.clone()), ..Default::default() };
            tc.reveal = Some(format!("ik:{}", badge.name));
        }
        BadgeKind::Transform => {
            app.selection = crate::app::Selection { transform: Some(badge.name.clone()), ..Default::default() };
            tc.reveal = Some(format!("transform:{}", badge.name));
        }
        BadgeKind::Physics => {
            app.selection = crate::app::Selection { physics: Some(badge.name.clone()), ..Default::default() };
            tc.reveal = Some(format!("physics:{}", badge.name));
        }
    }
}

#[derive(Default)]
struct TreeCtx {
    /// Constraint badges per bone name (only in the steps that have constraint inspectors).
    badges: std::collections::HashMap<String, Vec<Badge>>,
    navigation: Vec<crate::dialogs::Item>,
    collapsed: BTreeSet<String>,
    /// Bones to show while filtering (matches and their ancestors).
    visible: Option<HashSet<String>>,
    /// Drop released this frame, applied after the rows are drawn.
    drop: Option<(TreeDrag, DropTarget)>,
    /// Lowercase search text.
    query: String,
    /// Slots and attachments under their bones in the hierarchy.
    show_slots: bool,
    /// Rows drawn so far, for zebra striping.
    row: usize,
    /// Row key to scroll into view (selection made outside the tree).
    reveal: Option<String>,
}

/// A bone or slot being dragged in the hierarchy.
#[derive(Debug, Clone, PartialEq)]
enum TreeDrag {
    Bone(String),
    Slot(String),
    Constraint(String),
}

#[derive(Debug, Clone, PartialEq)]
enum DropTarget {
    /// New parent bone; `None` makes a root bone.
    Bone(Option<String>),
    /// Draw order: next to `slot`, in front of it or behind it.
    Slot { slot: String, front: bool },
    Constraint { name: String, before: bool },
}

/// What dropping `drag` on `target` does, or `None` when it changes nothing.
fn drop_action(doc: &skel2d_core::Document, drag: &TreeDrag, target: &DropTarget) -> Option<String> {
    match (drag, target) {
        (TreeDrag::Constraint(name), DropTarget::Constraint { name: target, before }) => {
            let names = reordered_constraints(doc, name, target, *before)?;
            let current = ordered_constraint_names(doc);
            (names != current).then(|| crate::tf!("Move constraint {} next to {}", name, target))
        }
        (TreeDrag::Bone(b), DropTarget::Bone(p)) => {
            let bone = doc.bone(b)?;
            // The root stays where it is, and nothing is left without a parent.
            if bone.parent == *p || b == skel2d_core::ROOT || p.is_none() {
                return None;
            }
            match p {
                // Not onto itself or into its own subtree.
                Some(p) if std::iter::successors(Some(p.as_str()), |n| doc.bone(n).and_then(|x| x.parent.as_deref())).any(|n| n == b) => None,
                Some(p) => Some(crate::tf!("Make {} a child of {}", b, p)),
                None => None,
            }
        }
        (TreeDrag::Slot(s), DropTarget::Bone(Some(b))) => (doc.slot(s)?.bone != *b).then(|| crate::tf!("Move slot {} to bone {}", s, b)),
        (TreeDrag::Slot(s), DropTarget::Slot { slot, front }) => {
            let (i, t) = (doc.slot_index(s)?, doc.slot_index(slot)?);
            (draw_index(i, t, *front) != i).then(|| match front {
                true => crate::tf!("Draw {} in front of {}", s, slot),
                false => crate::tf!("Draw {} behind {}", s, slot),
            })
        }
        _ => None,
    }
}

/// Draw-order index for `MoveSlot` that puts slot `i` right in front of or behind slot `t`.
fn draw_index(i: usize, t: usize, front: bool) -> usize {
    let t = if i < t { t - 1 } else { t };
    if front { t + 1 } else { t }
}

/// IK and physics constraints in evaluation order (ties keep IK first, then document order).
fn ordered_constraint_names(doc: &skel2d_core::Document) -> Vec<String> {
    let mut constraints: Vec<_> = doc.ik.iter().map(|c| (c.order, c.name.clone())).chain(doc.physics.iter().map(|c| (c.order, c.name.clone()))).chain(doc.transforms.iter().map(|c| (c.order, c.name.clone()))).collect();
    constraints.sort_by_key(|c| c.0);
    constraints.into_iter().map(|c| c.1).collect()
}

fn reordered_constraints(doc: &skel2d_core::Document, name: &str, target: &str, before: bool) -> Option<Vec<String>> {
    let mut names = ordered_constraint_names(doc);
    if name == target {
        return None;
    }
    let i = names.iter().position(|n| n == name)?;
    let moved = names.remove(i);
    let t = names.iter().position(|n| n == target)?;
    names.insert(if before { t } else { t + 1 }, moved);
    Some(names)
}

pub fn move_constraint(app: &mut EditorApp, earlier: bool) {
    let Some(name) = app.selection.ik.clone().or_else(|| app.selection.physics.clone()).or_else(|| app.selection.transform.clone()) else { return };
    let name = &name;
    let mut names = ordered_constraint_names(&app.doc);
    let Some(i) = names.iter().position(|n| n == name) else { return };
    let next = if earlier { i.checked_sub(1) } else { i.checked_add(1).filter(|n| *n < names.len()) };
    if let Some(next) = next {
        names.swap(i, next);
        app.edit(vec![Op::SetConstraintOrder { names }]);
    }
}

/// The tree item being dragged when the pointer is over `rect`, and whether
/// it is released there this frame.
fn hovered_drag(ui: &egui::Ui, rect: egui::Rect) -> Option<(TreeDrag, bool, egui::Pos2)> {
    let drag = egui::DragAndDrop::payload::<TreeDrag>(ui.ctx())?;
    let p = ui.ctx().pointer_latest_pos().filter(|p| rect.intersect(ui.clip_rect()).contains(*p))?;
    Some(((*drag).clone(), ui.input(|i| i.pointer.any_released()), p))
}

/// Highlights `rect` as a drop target for `drag` (a line above or below for
/// draw order), and records the drop when released.
fn drop_zone(app: &EditorApp, ui: &egui::Ui, rect: egui::Rect, tc: &mut TreeCtx, target: impl Fn(egui::Pos2) -> DropTarget) {
    let Some((drag, released, p)) = hovered_drag(ui, rect) else { return };
    let target = target(p);
    let Some(action) = drop_action(&app.doc, &drag, &target) else { return };
    let stroke = egui::Stroke::new(2.0, ACCENT);
    match &target {
        DropTarget::Slot { front, .. } => {
            let y = if *front { rect.top() } else { rect.bottom() };
            ui.painter().hline(rect.x_range(), y, stroke);
        }
        DropTarget::Constraint { before, .. } => {
            let y = if *before { rect.top() } else { rect.bottom() };
            ui.painter().hline(rect.x_range(), y, stroke);
        }
        DropTarget::Bone(_) => {
            ui.painter().rect_stroke(rect.shrink(1.0), 3.0, stroke, egui::StrokeKind::Inside);
        }
    }
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("tree_drop_hint"));
    ui.ctx().layer_painter(layer).text(p + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, action, egui::FontId::proportional(theme::SIZE_SMALL), Color32::WHITE);
    if released {
        tc.drop = Some((drag, target));
    }
}

/// Reparents a bone, keeping where it is in the setup pose (`None` = root).
fn reparent_bone(app: &mut EditorApp, name: &str, parent: Option<String>) {
    if name == skel2d_core::ROOT || parent.is_none() {
        app.status = tr(ROOT_REASON).to_string();
        return;
    }
    let pose = skel2d_core::Pose::compute(&app.doc, None);
    let Some(i) = app.doc.bone_index(name) else { return };
    let parent_world = parent.as_deref().and_then(|p| app.doc.bone_index(p)).map_or(skel2d_core::Affine::IDENTITY, |pi| pose.bones[pi].world);
    let Some(local) = skel2d_core::pose::LocalTransform::from_world(parent.as_ref().map(|_| &parent_world), &pose.bones[i].world, app.doc.bones[i].inherit) else {
        app.status = tr("Cannot preserve the pose under a zero-scale parent").to_string();
        return;
    };
    let patch = BonePatch { name: name.to_string(), parent: Some(parent), x: Some(local.x), y: Some(local.y), rotation: Some(local.rotation), scale_x: Some(local.scale_x), scale_y: Some(local.scale_y), shear_x: Some(local.shear_x), shear_y: Some(local.shear_y), ..Default::default() };
    if app.edit(vec![Op::SetBone(patch)]) {
        app.selection.bone = Some(name.to_string());
        app.selection.slot = None;
    }
}

/// Moves a slot to another bone; regions keep their place in the setup pose
/// (mesh vertices are already in skeleton space).
fn move_slot_to_bone(app: &mut EditorApp, name: &str, bone: &str) {
    let pose = skel2d_core::Pose::compute(&app.doc, None);
    let Some(slot) = app.doc.slot(name) else { return };
    let (Some(old), Some(new)) = (app.doc.bone_index(&slot.bone), app.doc.bone_index(bone)) else { return };
    let (old_world, new_world) = (pose.bones[old].world, pose.bones[new].world);
    let mut ops = vec![Op::SetSlot(SlotPatch { name: name.to_string(), bone: Some(bone.to_string()), ..Default::default() })];
    for a in &slot.attachments {
        if let Attachment::Region(r) = a {
            let world = old_world.mul(&skel2d_core::Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));
            let (x, y, rotation, scale_x, scale_y) = crate::import::local_trs(&new_world, &world);
            ops.push(Op::SetAttachment { slot: name.to_string(), attachment: Attachment::Region(Region { x, y, rotation, scale_x, scale_y, ..r.clone() }) });
        }
    }
    if app.edit(ops) {
        app.selection.slot = Some(name.to_string());
        app.selection.bone = Some(bone.to_string());
    }
}

fn apply_drop(app: &mut EditorApp, drag: TreeDrag, target: DropTarget) {
    match (drag, target) {
        (TreeDrag::Constraint(name), DropTarget::Constraint { name: target, before }) => {
            if let Some(names) = reordered_constraints(&app.doc, &name, &target, before) {
                app.edit(vec![Op::SetConstraintOrder { names }]);
            }
        }
        (TreeDrag::Bone(b), DropTarget::Bone(p)) => reparent_bone(app, &b, p),
        (TreeDrag::Slot(s), DropTarget::Bone(Some(b))) => move_slot_to_bone(app, &s, &b),
        (TreeDrag::Slot(s), DropTarget::Slot { slot, front }) => {
            if let (Some(i), Some(t)) = (app.doc.slot_index(&s), app.doc.slot_index(&slot)) {
                app.edit(vec![Op::MoveSlot { name: s, index: draw_index(i, t, front) }]);
            }
        }
        _ => {}
    }
}

/// Scrolls the tree while an item is dragged near its top or bottom edge.
fn drag_autoscroll(ui: &mut egui::Ui) {
    if !egui::DragAndDrop::has_payload_of_type::<TreeDrag>(ui.ctx()) {
        return;
    }
    let clip = ui.clip_rect();
    let Some(p) = ui.ctx().pointer_latest_pos().filter(|p| clip.x_range().contains(p.x)) else { return };
    let edge = 28.0;
    let dy = if p.y < clip.top() + edge {
        clip.top() + edge - p.y
    } else if p.y > clip.bottom() - edge {
        clip.bottom() - edge - p.y
    } else {
        return;
    };
    ui.scroll_with_delta(egui::vec2(0.0, dy * 0.4));
    ui.ctx().request_repaint();
}


pub fn tree(app: &mut EditorApp, ui: &mut egui::Ui) {
    let hovered = ui.rect_contains_pointer(ui.max_rect());
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("side_hover_tree"), hovered));
    if hovered {
        app.hint = tr("Click to select · right-click a row for actions · drag a bone onto another to reparent, a slot onto a bone to move it, slots in Draw order to reorder · F2 rename · Del delete").into();
        tree_keys(app, ui);
    }

    let fid = egui::Id::unique("tree_filter");
    let mut filter: String = ui.data_mut(|d| d.get_temp(fid)).unwrap_or_default();
    search_box(ui, &mut filter);
    if matches!(app.step, crate::steps::Step::Bones | crate::steps::Step::Animate) {
        let on = app.settings.tree_slots;
        if ui.selectable_label(on, tr("Slots")).on_hover_text(tr("Show the slots and their images under the bones. Off: only a count on each bone.")).clicked() {
            app.settings.tree_slots = !on;
        }
    }
    ui.data_mut(|d| d.insert_temp(fid, filter.clone()));
    let q = filter.trim().to_lowercase();
    let matches = |n: &str| q.is_empty() || n.to_lowercase().contains(&q);

    let cid = egui::Id::unique("tree_collapsed");
    let collapsed: BTreeSet<String> = ui.data_mut(|d| d.get_temp(cid)).unwrap_or_default();
    // Bones to show while filtering: matches (by bone or slot name) and their ancestors.
    let visible = (!q.is_empty()).then(|| {
        let mut set = HashSet::new();
        let hits = app.doc.bones.iter().map(|b| b.name.clone()).filter(|b| matches(b)).chain(app.doc.slots.iter().filter(|s| matches(&s.name)).map(|s| s.bone.clone()));
        for b in hits.collect::<Vec<_>>() {
            let mut cur = Some(b);
            while let Some(n) = cur {
                if !set.insert(n.clone()) {
                    break;
                }
                cur = app.doc.bone(&n).and_then(|b| b.parent.clone());
            }
        }
        set
    });
    let badges = if matches!(app.step, crate::steps::Step::Bones | crate::steps::Step::Animate) { constraint_badges(&app.doc) } else { Default::default() };
    let mut tc = TreeCtx { badges, navigation: Vec::new(), collapsed, visible, drop: None, query: q.clone(), show_slots: false, row: 0, reveal: None };

    // A selection made elsewhere (viewport, palette) unfolds its ancestors
    // and scrolls its row into view, so the tree never hides what is selected.
    let sid = egui::Id::unique("tree_last_selection");
    let sel = (app.selection.bone.clone(), app.selection.slot.clone(), app.selection.ik.clone(), app.selection.physics.clone());
    type Sel = (Option<String>, Option<String>, Option<String>, Option<String>);
    let last: Option<Sel> = ui.data_mut(|d| d.get_temp(sid));
    if last.as_ref() != Some(&sel) {
        ui.data_mut(|d| d.insert_temp(sid, sel.clone()));
        if last.is_some() && !hovered {
            let mut cur = app.selection.bone.as_ref().and_then(|b| app.doc.bone(b)).and_then(|b| b.parent.clone());
            while let Some(p) = cur {
                tc.collapsed.remove(&p);
                cur = app.doc.bone(&p).and_then(|b| b.parent.clone());
            }
            if let Some(s) = &app.selection.slot {
                tc.collapsed.remove(&format!("slot:{s}"));
                if let Some(b) = &app.selection.bone {
                    tc.collapsed.remove(b);
                }
            }
            tc.reveal = Some(match (&sel.2, &sel.3, &sel.1, &sel.0) {
                (Some(k), ..) => format!("ik:{k}"),
                (_, Some(k), ..) => format!("physics:{k}"),
                (_, _, Some(s), _) => format!("slot:{s}"),
                (_, _, _, Some(b)) => format!("bone:{b}"),
                _ => String::new(),
            });
        }
    }


    tc.show_slots = matches!(app.step, crate::steps::Step::Bones | crate::steps::Step::Animate) && app.settings.tree_slots;

    let scroll = egui::ScrollArea::vertical().id_salt(("tree_scroll", app.step.number())).auto_shrink([false, false]).show(ui, |ui| {
        drag_autoscroll(ui);
        ui.spacing_mut().item_spacing.y = 0.0;
        use crate::steps::Step;
        match app.step {
            Step::Bones => {
                hierarchy_view(app, ui, &mut tc);
                if !app.doc.ik.is_empty() || !app.doc.physics.is_empty() || !app.doc.transforms.is_empty() {
                    theme::section(ui, tr("Constraint order"));
                    theme::hint(ui, tr("Constraints run top to bottom. Drag to change the order."));
                    constraints_view(app, ui, &mut tc);
                }
            }
            Step::Images | Step::Mesh | Step::Weights => draw_order_view(app, ui, &mut tc),
            Step::Animate => {
                theme::section(ui, tr("Animations"));
                animations_view(app, ui, &mut tc);
                theme::section(ui, tr("Bones and images"));
                hierarchy_view(app, ui, &mut tc);
            }
        }
        // The empty space under the rows takes the tab's actions too.
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 4.0 {
            let r = ui.allocate_rect(rest, egui::Sense::click());
            if r.clicked() {
                app.selection = Default::default();
            }
            r.context_menu(|ui| tab_actions(app, ui));
        }
    });
    let _ = scroll;
    if hovered { tree_navigation(app, ui, &tc.navigation); }
    ui.data_mut(|d| d.insert_temp(cid, tc.collapsed));
    if let Some((drag, target)) = tc.drop.take() {
        apply_drop(app, drag, target);
    }
}

/// The filter field with a search glyph and a clear button inside it.
fn search_box(ui: &mut egui::Ui, filter: &mut String) {
    egui::Frame::new().fill(Color32::from_gray(30)).stroke(egui::Stroke::new(1.0, Color32::from_gray(70))).corner_radius(5).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add(crate::icons::image(crate::icons::Icon::Search, 15.0).tint(TEXT_DIM));
            let w = (ui.available_width() - if filter.is_empty() { 0.0 } else { 22.0 }).max(40.0);
            let r = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Search by name")).frame(egui::Frame::NONE).desired_width(w));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) && r.has_focus() {
                filter.clear();
            }
            if !filter.is_empty() && ui.add(egui::Button::new("×").frame(false)).on_hover_text(tr("Clear filter")).clicked() {
                filter.clear();
            }
        });
    });
    ui.add_space(4.0);
}

/// The Add menu (and empty-space menu) of the list in the current step.
fn tab_actions(app: &mut EditorApp, ui: &mut egui::Ui) {
    use crate::steps::Step;
    match app.step {
        Step::Bones => {
            if ui.button(tr("New bone under the selected bone")).clicked() {
                add_bone_under_selection(app);
                ui.close();
            }
            if ui.add_enabled(app.selection.bone.is_some(), egui::Button::new(tr("Add slot on the selected bone"))).clicked()
                && let Some(bone) = app.selection.bone.clone()
            {
                add_slot(app, bone);
                ui.close();
            }
            let tip = tr("Constrain the selected bone (and its parent) to a new target bone");
            if ui.add_enabled(app.selection.bone.is_some(), egui::Button::new(tr("Add IK on the selected bone"))).on_hover_text(tip).clicked()
                && let Some(bone) = app.selection.bone.clone()
            {
                add_ik(app, &bone);
                ui.close();
            }
            let tip = tr("Make the selected bone lag behind its animated pose on a damped spring");
            if ui.add_enabled(app.selection.bone.is_some(), egui::Button::new(tr("Add physics on the selected bone"))).on_hover_text(tip).clicked() {
                crate::dialogs::new_physics(app, app.selection.bone.clone());
                ui.close();
            }
            ui.separator();
            if ui.button(tr("Show all bones")).clicked() {
                app.hidden_bones.clear();
                app.locked_bones.clear();
                ui.close();
            }
        }
        Step::Images | Step::Mesh | Step::Weights => {
            import_button(app, ui);
            ui.separator();
            if ui.button(tr("Show all slots")).clicked() {
                app.hidden_slots.clear();
                app.locked_slots.clear();
                ui.close();
            }
        }
        Step::Animate => {
            if ui.button(tr("New animation")).clicked() {
                new_animation(app);
                ui.close();
            }
        }
    }
}

/// A new bone under the selected bone, or under root when none is selected.
pub(crate) fn add_bone_under_selection(app: &mut EditorApp) {
    let parent = app.selection.bone.clone().filter(|b| app.doc.bone(b).is_some()).unwrap_or_else(|| skel2d_core::ROOT.to_string());
    add_bone(app, Some(parent));
}

fn import_button(app: &mut EditorApp, ui: &mut egui::Ui) {
    if ui.button(tr("Import images...")).on_hover_text(tr("Add PNG images or a layered PSD as slots on the selected bone (or drop files on the viewport)")).clicked() {
        ui.close();
        let files = crate::import::pick_images();
        if !files.is_empty() {
            crate::import::import_images(app, &files, app.camera.center, app.selection.bone.clone());
        }
    }
}

/// An empty-list message with a hint of what to do.
fn empty_state(ui: &mut egui::Ui, icon: crate::icons::Icon, title: &str, hint: &str) {
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.add(crate::icons::image(icon, 32.0).tint(Color32::from_gray(110)));
        ui.add_space(6.0);
        ui.label(RichText::new(title).color(TEXT).size(theme::SIZE_BODY));
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(hint).color(TEXT_DIM).size(theme::SIZE_SMALL)).wrap());
    });
}

fn hierarchy_view(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx) {
    if app.doc.bones.is_empty() {
        empty_state(ui, crate::icons::Icon::Skeleton, tr("No bones yet"), tr("Draw one with Create (N), or use Add above."));
        return;
    }
    if tc.visible.as_ref().is_some_and(|v| v.is_empty()) {
        empty_state(ui, crate::icons::Icon::Search, tr("No matches"), tr("Nothing in the tree matches the search."));
        return;
    }
    let roots: Vec<String> = app.doc.bones.iter().filter(|b| b.parent.is_none()).map(|b| b.name.clone()).collect();
    for root in roots {
        bone_row(app, ui, &root, 0, tc);
    }
}

fn draw_order_view(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx) {
    if app.doc.slots.is_empty() {
        empty_state(ui, crate::icons::Icon::Image, tr("No slots yet"), tr("Drop PNG or PSD files on the viewport, or use Add above."));
        return;
    }
    let n = app.doc.slots.len();
    let slots: Vec<(usize, String, String)> = app.doc.slots.iter().enumerate().rev().map(|(i, s)| (i, s.name.clone(), s.bone.clone())).collect();
    let mut any = false;
    for (i, name, bone) in slots {
        if !(matches_q(&tc.query, &name) || matches_q(&tc.query, &bone)) {
            continue;
        }
        any = true;
        slot_row(app, ui, &name, &bone, n - i, tc);
    }
    if !any {
        empty_state(ui, crate::icons::Icon::Search, tr("No matches"), tr("No slot or bone name matches the search."));
    }
}

fn matches_q(q: &str, name: &str) -> bool {
    q.is_empty() || name.to_lowercase().contains(q)
}

fn constraints_view(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx) {
    let rows: Vec<_> = ordered_constraint_names(&app.doc).into_iter().enumerate()
        .filter_map(|(i, name)| app.doc.ik.iter().find(|c| c.name == name)
            .filter(|c| matches_q(&tc.query, &c.name)).map(|c| (i, c.clone()))).collect();
    let physics_rows: Vec<_> = ordered_constraint_names(&app.doc).into_iter().enumerate()
        .filter_map(|(i, name)| app.doc.physics_constraint(&name).filter(|c| matches_q(&tc.query, &c.name)).map(|c| (i, c.clone()))).collect();
    let transform_rows: Vec<_> = ordered_constraint_names(&app.doc).into_iter().enumerate()
        .filter_map(|(i, name)| app.doc.transforms.iter().find(|c| c.name == name).filter(|c| matches_q(&tc.query, &c.name)).map(|c| (i, c.clone()))).collect();
    if rows.is_empty() && physics_rows.is_empty() && transform_rows.is_empty() {
        let (title, hint) = if app.doc.ik.is_empty() && app.doc.physics.is_empty() && app.doc.transforms.is_empty() { (tr("No constraints yet"), tr("Select a bone, then Add → IK or Add → Physics. IK pulls a bone chain to a target; physics lets a bone lag and sway on a spring.")) } else { (tr("No matches"), tr("No constraint matches the search.")) };
        empty_state(ui, crate::icons::Icon::Constraint, title, hint);
        return;
    }
    // One list in evaluation order, whatever the kind.
    let mut order: Vec<(usize, Option<IkConstraint>, Option<skel2d_core::PhysicsConstraint>)> = rows.into_iter().map(|(i, c)| (i, Some(c), None)).chain(physics_rows.into_iter().map(|(i, c)| (i, None, Some(c)))).collect();
    order.sort_by_key(|r| r.0);
    let mut transform_rows = transform_rows.into_iter().peekable();
    for (index, ik, physics) in order {
        while let Some((i, c)) = transform_rows.next_if(|(i, _)| *i < index) {
            transform_row(app, ui, tc, i, &c);
        }
        if let Some(c) = physics {
            physics_row(app, ui, tc, index, &c);
        }
        let Some(c) = ik else { continue };
        let name = c.name.clone();
        let selected = app.selection.ik.as_deref() == Some(name.as_str());
        let meta = format!("{} → {}", c.bones.join(", "), c.target);
        let display_name = format!("{}. {}", index + 1, name);
        let row = crate::timeline::KeyRow::Ik(name.clone());
        let out = tree_row(ui, tc, RowSpec { key: format!("ik:{name}"), depth: 0, kind: NodeKind::Constraint, name: &display_name, color: IK, meta, selected, draggable: true, keys: [key_btn(app, &row, selected, "Key the IK values shown now"), None], ..Default::default() });
        if out.keyed[0] {
            crate::timeline::key_row(app, &row);
        }
        drop_zone(app, ui, out.rect, tc, |p| DropTarget::Constraint { name: name.clone(), before: p.y < out.rect.center().y });
        out.main.dnd_set_drag_payload(TreeDrag::Constraint(name.clone()));
        let r = out.main.on_hover_text(tr("Drag to change constraint order"));
        if r.clicked() {
            app.selection.ik = Some(name.clone());
        }
        r.context_menu(|ui| {
            if ui.button(tr("Rename")).clicked() {
                crate::dialogs::rename(app, crate::dialogs::Item::Ik(name.clone()));
                ui.close();
            }

            if ui.button(crate::tf!("Select target bone {}", c.target)).clicked() {
                app.selection.bone = Some(c.target.clone());
                app.selection.slot = None;
                ui.close();
            }
            ui.separator();
            if ui.add(theme::danger_button(tr("Delete"))).clicked() {
                crate::dialogs::delete(app, crate::dialogs::Item::Ik(name.clone()));
                ui.close();
            }
        });
    }
    for (i, c) in transform_rows {
        transform_row(app, ui, tc, i, &c);
    }
}

fn transform_row(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx, index: usize, c: &skel2d_core::TransformConstraint) {
    let name = c.name.clone();
    let selected = app.selection.transform.as_deref() == Some(name.as_str());
    let display_name = format!("{}. {}", index + 1, name);
    let meta = format!("{} ← {}", c.bones.join(", "), c.source);
    let row = crate::timeline::KeyRow::Transform(name.clone());
    let out = tree_row(ui, tc, RowSpec { key: format!("transform:{name}"), depth: 0, kind: NodeKind::Constraint, name: &display_name, color: IK, meta, selected, draggable: true, keys: [key_btn(app, &row, selected, "Key the mix values shown now"), None], ..Default::default() });
    if out.keyed[0] {
        crate::timeline::key_row(app, &row);
    }
    drop_zone(app, ui, out.rect, tc, |p| DropTarget::Constraint { name: name.clone(), before: p.y < out.rect.center().y });
    out.main.dnd_set_drag_payload(TreeDrag::Constraint(name.clone()));
    let r = out.main.on_hover_text(tr("Copy constraint · drag to change constraint order"));
    if r.clicked() {
        app.selection = crate::app::Selection { transform: Some(name.clone()), ..Default::default() };
    }
    r.context_menu(|ui| {
        if ui.button(format!("{}    F2", tr("Rename"))).clicked() {
            crate::dialogs::rename(app, crate::dialogs::Item::Transform(name.clone()));
            ui.close();
        }
        ui.separator();
        if ui.add(theme::danger_button(&format!("{}    Del", tr("Delete")))).clicked() {
            crate::dialogs::delete(app, crate::dialogs::Item::Transform(name.clone()));
            ui.close();
        }
    });
}

fn physics_row(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx, index: usize, c: &skel2d_core::PhysicsConstraint) {
    let name = c.name.clone();
    let selected = app.selection.physics.as_deref() == Some(name.as_str());
    let display_name = format!("{}. {}", index + 1, name);
    let row = crate::timeline::KeyRow::Physics(name.clone());
    let out = tree_row(ui, tc, RowSpec { key: format!("physics:{name}"), depth: 0, kind: NodeKind::Physics, name: &display_name, color: PHYSICS, meta: c.bone.clone(), selected, draggable: true, keys: [key_btn(app, &row, selected, "Key the physics values shown now"), None], ..Default::default() });
    if out.keyed[0] {
        crate::timeline::key_row(app, &row);
    }
    drop_zone(app, ui, out.rect, tc, |p| DropTarget::Constraint { name: name.clone(), before: p.y < out.rect.center().y });
    out.main.dnd_set_drag_payload(TreeDrag::Constraint(name.clone()));
    let r = out.main.on_hover_text(tr("Physics constraint · drag to change constraint order"));
    if r.clicked() {
        app.selection = crate::app::Selection { physics: Some(name.clone()), ..Default::default() };
    }
    r.context_menu(|ui| {
        if ui.button(format!("{}    F2", tr("Rename"))).clicked() {
            crate::dialogs::rename(app, crate::dialogs::Item::Physics(name.clone()));
            ui.close();
        }
        if ui.button(crate::tf!("Select bone {}", c.bone)).clicked() {
            app.selection = crate::app::Selection { bone: Some(c.bone.clone()), ..Default::default() };
            ui.close();
        }
        if ui.button(tr("Shake")).on_hover_text(tr("Kicks this constraint so you can judge the settings. A preview only; the document is not changed.")).clicked() {
            app.physics_shake(&name);
            ui.close();
        }
        ui.separator();
        if ui.add(theme::danger_button(&format!("{}    Del", tr("Delete")))).clicked() {
            crate::dialogs::delete(app, crate::dialogs::Item::Physics(name.clone()));
            ui.close();
        }
    });
}

fn animations_view(app: &mut EditorApp, ui: &mut egui::Ui, tc: &mut TreeCtx) {
    let anims: Vec<(String, f32)> = app.doc.animations.iter().filter(|(n, _)| matches_q(&tc.query, n)).map(|(n, a)| (n.clone(), a.duration())).collect();
    if anims.is_empty() {
        let (title, hint) = if app.doc.animations.is_empty() { (tr("No animations yet"), tr("Press Tab, or use Add above, to start one.")) } else { (tr("No matches"), tr("No animation matches the search.")) };
        empty_state(ui, crate::icons::Icon::Animation, title, hint);
        return;
    }
    for (name, duration) in anims {
        let current = app.playback.anim.as_deref() == Some(name.as_str());
        let layered = app.playback.layers.iter().any(|layer| layer.anim == name);
        let meta = if layered { format!("{} · {:.2}s", tr("layer"), duration) } else { format!("{duration:.2}s") };
        let out = tree_row(ui, tc, RowSpec { key: format!("anim:{name}"), depth: 0, kind: NodeKind::Animation, name: &name, color: if current { ANIMATE } else { TEXT }, meta, selected: current, accent: ANIMATE, ..Default::default() });
        // Spine: picking an animation in the tree enters animate mode on it.
        let r = out.main.on_hover_text(tr("Edit this animation (animate mode); click again to return to the setup pose · right-click: rename, delete"));
        if r.clicked() {
            if current {
                app.toggle_mode();
            } else {
                open_animation(app, &name);
            }
        }
        r.context_menu(|ui| {
            if ui.button(tr("Rename")).clicked() {
                crate::dialogs::rename(app, crate::dialogs::Item::Animation(name.clone()));
                ui.close();
            }
            if ui.button(tr("Duplicate")).clicked() {
                crate::dialogs::duplicate_animation(app, &name);
                ui.close();
            }
            ui.separator();
            if ui.add(theme::danger_button(tr("Delete"))).clicked() {
                crate::dialogs::delete(app, crate::dialogs::Item::Animation(name.clone()));
                ui.close();
            }
        });
    }
}

/// What a tree row shows.
struct RowSpec<'a> {
    /// Unique per row ("bone:hip"); also what `TreeCtx::reveal` matches.
    key: String,
    depth: usize,
    /// `Some(open)` draws a fold chevron.
    expander: Option<bool>,
    kind: NodeKind,
    name: &'a str,
    color: Color32,
    /// Dim text on the right (owner bone, counts, duration).
    meta: String,
    selected: bool,
    /// Selection bar color.
    accent: Color32,
    /// `Some(visible)` draws the eye column.
    eye: Option<bool>,
    /// `Some(locked)` draws the lock column (shown while locked or hovered).
    lock: Option<bool>,
    /// `Some(shown)`: Spine's attachment dot before the icon.
    dot: Option<bool>,
    draggable: bool,
    /// A small number before the icon (draw order position).
    index: Option<usize>,
    /// Row height; `None` is the normal list row.
    height: Option<f32>,
    /// Room kept free in front of the icon (for a thumbnail the caller paints).
    lead: f32,
    /// Replaces the type color of the icon (a bone's own color).
    icon_color: Option<Color32>,
    /// Key buttons at the right edge (animate step only), outermost last.
    keys: [Option<KeyBtn>; 2],
    /// Constraint glyphs before the key buttons (bone rows).
    badges: &'a [Badge],
}

/// A small key button in a tree row.
#[derive(Clone, Copy)]
struct KeyBtn {
    state: KeyState,
    /// Shown even when the pointer is elsewhere (unkeyed change, or the row is selected).
    always: bool,
    tip: &'static str,
}

impl Default for RowSpec<'_> {
    fn default() -> Self {
        Self { key: String::new(), depth: 0, expander: None, kind: NodeKind::Bone, name: "", color: TEXT, meta: String::new(), selected: false, accent: ACCENT, eye: None, lock: None, dot: None, draggable: false, index: None, height: None, lead: 0.0, icon_color: None, keys: [None; 2], badges: &[] }
    }
}

struct RowOut {
    main: egui::Response,
    rect: egui::Rect,
    expander: bool,
    eye: bool,
    lock: bool,
    dot: bool,
    /// Which key buttons were clicked.
    keyed: [bool; 2],
    /// The badge (index into the row's badges) that was clicked or picked from the overflow menu.
    badge: Option<usize>,
}

/// A tree key button for `row`, only in the animate step.
fn key_btn(app: &EditorApp, row: &crate::timeline::KeyRow, selected: bool, tip: &'static str) -> Option<KeyBtn> {
    if app.playback.anim.is_none() || app.step != crate::steps::Step::Animate {
        return None;
    }
    let state = crate::timeline::key_row_state(app, row);
    Some(KeyBtn { state, always: selected || state == KeyState::Changed, tip })
}

/// The shape of a key state: filled diamond keyed, filled circle changed,
/// hollow diamond not keyed here (shape carries the meaning, color backs it up).
fn paint_key_state(p: &egui::Painter, c: egui::Pos2, state: KeyState, bg: Color32) {
    let (color, s) = match state {
        KeyState::Keyed => (theme::KEY_SET, 6.0),
        KeyState::Changed => (theme::KEY_CHANGED, 5.0),
        KeyState::Interpolated | KeyState::None => (theme::KEY_NONE, 6.0),
    };
    let diamond = |s: f32| vec![c + egui::vec2(0.0, -s), c + egui::vec2(s, 0.0), c + egui::vec2(0.0, s), c + egui::vec2(-s, 0.0)];
    match state {
        KeyState::Keyed => {
            p.add(egui::Shape::convex_polygon(diamond(s), color, egui::Stroke::NONE));
        }
        KeyState::Changed => {
            p.circle_filled(c, s, color);
        }
        KeyState::Interpolated => {
            // Hollow with a dot: the value is interpolated from other keys.
            p.add(egui::Shape::convex_polygon(diamond(s), color, egui::Stroke::NONE));
            p.add(egui::Shape::convex_polygon(diamond(s - 2.5), bg, egui::Stroke::NONE));
            p.circle_filled(c, 1.2, color);
        }
        KeyState::None => {
            p.add(egui::Shape::convex_polygon(diamond(s), color, egui::Stroke::NONE));
            p.add(egui::Shape::convex_polygon(diamond(s - 2.5), bg, egui::Stroke::NONE));
        }
    }
}

fn key_state_tip(state: KeyState) -> &'static str {
    match state {
        KeyState::Keyed => "Keyed at this time (red diamond)",
        KeyState::Changed => "Changed but not keyed (orange circle)",
        KeyState::Interpolated => "Keyed at other times (hollow diamond with a dot)",
        KeyState::None => "Not keyed (hollow diamond)",
    }
}

/// How far a row at `depth` starts. Each level is a full step near the top and
/// a smaller one further down, so a deep tree never pushes names out of a row
/// `width` wide while every level still sits further in than its parent.
fn row_indent(depth: usize, width: f32) -> f32 {
    let limit = (width * 0.4).max(theme::ROW_INDENT);
    limit * (1.0 - (-(depth as f32) * theme::ROW_INDENT / limit).exp())
}

fn name_color_of(s: &RowSpec) -> Color32 {
    if s.selected { Color32::WHITE } else { s.color }
}

/// One full-width outliner row: zebra striping, indent guides, fold chevron,
/// type icon, name, right-aligned dim meta, and fixed eye/lock columns on
/// the right edge so they line up down the list.
fn tree_row(ui: &mut egui::Ui, tc: &mut TreeCtx, s: RowSpec) -> RowOut {
    if let Some((kind, name)) = s.key.split_once(':') {
        use crate::dialogs::Item;
        let item = match kind {
            "bone" => Some(Item::Bone(name.to_string())),
            "slot" => Some(Item::Slot(name.to_string())),
            "ik" => Some(Item::Ik(name.to_string())),
            "physics" => Some(Item::Physics(name.to_string())),
            "transform" => Some(Item::Transform(name.to_string())),
            "skin" => Some(Item::Skin(name.to_string())),
            "anim" => Some(Item::Animation(name.to_string())),
            _ => None,
        };
        if let Some(item) = item { tc.navigation.push(item); }
    }
    let full = ui.max_rect();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), s.height.unwrap_or(theme::ROW_H)), egui::Sense::hover());
    let rect = egui::Rect::from_x_y_ranges(full.x_range(), rect.y_range());
    let sense = if s.draggable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
    let main = ui.interact(rect, ui.scope_id().with(("row", &s.key)), sense);
    let hovered = ui.rect_contains_pointer(rect);
    tc.row += 1;
    if tc.reveal.as_deref() == Some(s.key.as_str()) {
        main.scroll_to_me(Some(egui::Align::Center));
        tc.reveal = None;
    }

    let p = ui.painter().clone();
    if s.selected {
        p.rect_filled(rect, 0.0, s.accent.gamma_multiply(0.28));
        p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())), 0.0, s.accent);
    } else if hovered {
        p.rect_filled(rect, 0.0, theme::ROW_HOVER);
    } else if tc.row.is_multiple_of(2) {
        p.rect_filled(rect, 0.0, theme::ROW_ALT);
    }
    let cy = rect.center().y;
    let left = rect.left() + 6.0;
    for level in 0..s.depth {
        let x = left + row_indent(level, rect.width()) + 7.0;
        p.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::GUIDE));
    }
    let mut x = left + row_indent(s.depth, rect.width());

    // Right columns first, so the name knows where to stop.
    let col = 24.0;
    let mut right = rect.right() - 2.0;
    let mut eye = false;
    if let Some(visible) = s.eye {
        let r = egui::Rect::from_min_max(egui::pos2(right - col, rect.top()), egui::pos2(right, rect.bottom()));
        let resp = ui.interact(r, ui.scope_id().with(("eye", &s.key)), egui::Sense::click());
        let c = if resp.hovered() { Color32::WHITE } else if visible { Color32::from_gray(190) } else { Color32::from_gray(96) };
        let icon = if visible { crate::icons::Icon::Eye } else { crate::icons::Icon::EyeOff };
        crate::icons::image(icon, 16.0).tint(c).paint_at(ui, egui::Rect::from_center_size(r.center(), egui::vec2(16.0, 16.0)));
        eye = resp.on_hover_text(tr(if visible { "Hide in the viewport" } else { "Show in the viewport" })).clicked();
        right -= col;
    }
    let mut lock = false;
    if let Some(locked) = s.lock {
        let r = egui::Rect::from_min_max(egui::pos2(right - col, rect.top()), egui::pos2(right, rect.bottom()));
        let resp = ui.interact(r, ui.scope_id().with(("lock", &s.key)), egui::Sense::click());
        if locked || hovered {
            let c = if resp.hovered() { Color32::WHITE } else if locked { ACCENT } else { Color32::from_gray(96) };
            let icon = if locked { crate::icons::Icon::Lock } else { crate::icons::Icon::Unlock };
            crate::icons::image(icon, 15.0).tint(c).paint_at(ui, egui::Rect::from_center_size(r.center(), egui::vec2(15.0, 15.0)));
        }
        lock = resp.on_hover_text(tr(if locked { "Unlock (allow picking in the viewport)" } else { "Lock (no picking in the viewport)" })).clicked();
        right -= col;
    }
    let mut keyed = [false; 2];
    let row_bg = if s.selected { s.accent.gamma_multiply(0.28) } else { Color32::from_gray(44) };
    for (i, kb) in s.keys.iter().enumerate() {
        let Some(kb) = kb else { continue };
        let r = egui::Rect::from_min_max(egui::pos2(right - col, rect.top()), egui::pos2(right, rect.bottom()));
        right -= col;
        let resp = ui.interact(r, ui.scope_id().with(("key", i, &s.key)), egui::Sense::click());
        if kb.always || hovered || resp.has_focus() {
            if resp.hovered() {
                p.rect_filled(r.shrink2(egui::vec2(2.0, 4.0)), 3.0, Color32::from_gray(74));
            }
            paint_key_state(&p, r.center(), kb.state, if resp.hovered() { Color32::from_gray(74) } else { row_bg });
        }
        keyed[i] = resp.on_hover_text(format!("{} · {}", tr(kb.tip), tr(key_state_tip(kb.state)))).clicked();
    }

    let mut expander = false;
    if let Some(open) = s.expander {
        let r = egui::Rect::from_center_size(egui::pos2(x + 7.0, cy), egui::vec2(16.0, theme::ROW_H));
        let resp = ui.interact(r, ui.scope_id().with(("fold", &s.key)), egui::Sense::click());
        theme::chevron(&p, r.center(), open, if resp.hovered() { Color32::WHITE } else { Color32::from_gray(160) });
        expander = resp.on_hover_text(tr(if open { "Collapse children" } else { "Expand children" })).clicked();
    }
    x += 16.0;
    let mut dot = false;
    if let Some(shown) = s.dot {
        let r = egui::Rect::from_center_size(egui::pos2(x + 6.0, cy), egui::vec2(14.0, theme::ROW_H));
        let resp = ui.interact(r, ui.scope_id().with(("dot", &s.key)), egui::Sense::click());
        if shown {
            p.circle_filled(r.center(), 4.0, Color32::WHITE);
        } else {
            p.circle_stroke(r.center(), 4.0, egui::Stroke::new(1.2, if resp.hovered() { Color32::WHITE } else { TEXT_DIM }));
        }
        dot = resp.clicked();
        x += 14.0;
    }
    if let Some(i) = s.index {
        let t = p.text(egui::pos2(x + 22.0, cy), egui::Align2::RIGHT_CENTER, i.to_string(), egui::FontId::monospace(theme::SIZE_SMALL), TEXT_DIM);
        x = t.right() + 6.0;
    }
    if s.lead > 0.0 {
        x += s.lead;
    } else {
        node_icon(ui, egui::Rect::from_center_size(egui::pos2(x + 8.0, cy), egui::vec2(16.0, 16.0)), s.kind, s.icon_color.filter(|_| !s.selected));
        x += 21.0;
    }

    // Constraint badges: as many as fit while the name keeps some room (names
    // truncate first), the rest behind a "+N" menu.
    let mut badge = None;
    if !s.badges.is_empty() {
        const SLOT_W: f32 = 18.0;
        const NAME_MIN: f32 = 70.0;
        let fit = ((right - 6.0 - x - NAME_MIN) / SLOT_W).floor().max(0.0) as usize;
        let (shown, hidden) = if s.badges.len() <= fit { (s.badges.len(), 0) } else { (fit.saturating_sub(1), s.badges.len() - fit.saturating_sub(1)) };
        if hidden > 0 && fit > 0 {
            let r = egui::Rect::from_min_max(egui::pos2(right - SLOT_W - 4.0, rect.top()), egui::pos2(right, rect.bottom()));
            right -= SLOT_W + 4.0;
            let resp = ui.interact(r, ui.scope_id().with(("more", &s.key)), egui::Sense::click());
            p.text(r.center(), egui::Align2::CENTER_CENTER, format!("+{hidden}"), egui::FontId::proportional(theme::SIZE_SMALL), if resp.hovered() { Color32::WHITE } else { TEXT_DIM });
            let resp = resp.on_hover_text(tr("More constraints on this bone"));
            egui::Popup::menu(&resp).show(|ui| {
                for (i, b) in s.badges.iter().enumerate().skip(shown) {
                    if ui.button(badge_text(b)).clicked() {
                        badge = Some(i);
                        ui.close();
                    }
                }
            });
        }
        for (i, b) in s.badges.iter().enumerate().take(shown).rev() {
            let r = egui::Rect::from_min_max(egui::pos2(right - SLOT_W, rect.top()), egui::pos2(right, rect.bottom()));
            right -= SLOT_W;
            let resp = ui.interact(r, ui.scope_id().with(("badge", i, &s.key)), egui::Sense::click());
            let color = if b.kind == BadgeKind::Physics { PHYSICS } else { IK };
            let icon = if b.kind == BadgeKind::Physics { crate::icons::Icon::Physics } else { crate::icons::Icon::Constraint };
            if resp.hovered() {
                p.rect_filled(r.shrink2(egui::vec2(1.0, 4.0)), 3.0, Color32::from_gray(74));
            }
            crate::icons::image(icon, 12.0).tint(color).paint_at(ui, egui::Rect::from_center_size(r.center(), egui::vec2(12.0, 12.0)));
            if b.target {
                // A ring marks the target or source; the bare glyph is a bone the constraint moves.
                p.circle_stroke(r.center(), 8.0, egui::Stroke::new(1.3, color));
            }
            if resp.on_hover_text(badge_text(b)).clicked() {
                badge = Some(i);
            }
        }
    }

    // The name comes first; the meta only gets what is left.
    let room = (right - 6.0 - x).max(0.0);
    let name_full = p.layout_no_wrap(s.name.to_string(), egui::FontId::proportional(theme::SIZE_BODY), name_color_of(&s)).size().x;
    let meta_left = room - name_full - 8.0;
    let meta_w = if s.meta.is_empty() || meta_left < 28.0 { 0.0 } else { p.layout_no_wrap(s.meta.clone(), egui::FontId::proportional(theme::SIZE_SMALL), TEXT_DIM).size().x.min(meta_left) };
    let name_w = (room - meta_w - if meta_w > 0.0 { 8.0 } else { 0.0 }).max(0.0);
    let name_color = name_color_of(&s);
    let galley = truncated(&p, s.name, egui::FontId::proportional(theme::SIZE_BODY), name_color, name_w);
    let cut = galley.elided;
    p.galley(egui::pos2(x, cy - galley.size().y / 2.0), galley, name_color);
    let main = if cut { main.on_hover_text(s.name) } else { main };
    if meta_w > 0.0 {
        let g = truncated(&p, &s.meta, egui::FontId::proportional(theme::SIZE_SMALL), TEXT_DIM, meta_w);
        p.galley(egui::pos2(right - 6.0 - g.size().x, cy - g.size().y / 2.0), g, TEXT_DIM);
    }
    RowOut { main, rect, expander, eye, lock, dot, keyed, badge }
}

/// `text` on one line, cut with "…" to fit `width`.
fn truncated(p: &egui::Painter, text: &str, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    p.layout_job(job)
}

/// A slot in the Draw order view: position, name, owner bone; drag to reorder.
fn slot_row(app: &mut EditorApp, ui: &mut egui::Ui, name: &str, bone: &str, position: usize, tc: &mut TreeCtx) {
    let hidden = app.hidden_slots.contains(name);
    let locked = app.locked_slots.contains(name);
    let selected = app.selection.slot.as_deref() == Some(name);
    let color = if hidden { TEXT_DIM.gamma_multiply(0.75) } else { TEXT };
    let slot = app.doc.slot(name).cloned();
    let shown = slot.as_ref().and_then(|s| shown_attachment(app, s));
    let kind = shown.as_ref().map_or(tr("(hidden)"), |a| tr(stepui::kind_name(a)));
    let height = theme::THUMB_SMALL + 8.0;
    let out = tree_row(ui, tc, RowSpec { key: format!("slot:{name}"), kind: NodeKind::Slot, name, color, meta: if matches!(shown, Some(Attachment::Region(_))) { bone.to_string() } else { format!("{bone} · {kind}") }, selected, eye: Some(!hidden), lock: Some(locked), draggable: true, index: Some(position), height: Some(height), lead: theme::THUMB_SMALL + 8.0, ..Default::default() });
    if let Some(slot) = &slot {
        let rect = egui::Rect::from_min_size(egui::pos2(out.rect.left() + 50.0, out.rect.top() + 4.0), egui::vec2(theme::THUMB_SMALL, theme::THUMB_SMALL));
        stepui::thumb(app, ui, slot, shown.as_ref(), rect);
    }
    if out.eye {
        set_flag(&mut app.hidden_slots, name, !hidden);
    }
    if out.lock {
        set_flag(&mut app.locked_slots, name, !locked);
    }
    let r = out.main;
    r.dnd_set_drag_payload(TreeDrag::Slot(name.to_string()));
    let r = r.on_hover_text(crate::tf!("{} (on bone {})", name, bone));
    if r.hovered() {
        app.view.tree_hover = Some(crate::viewport::Pick::Slot(name.to_string(), bone.to_string()));
    }
    if r.clicked() {
        app.selection.slot = Some(name.to_string());
        app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
        app.selection.bone = Some(bone.to_string());
    }
    r.context_menu(|ui| slot_menu(app, ui, name));
    let rect = out.rect;
    // The list runs front to back, so the upper half means "in front of".
    drop_zone(app, ui, rect, tc, |p| DropTarget::Slot { slot: name.to_string(), front: p.y < rect.center().y });
}

fn bone_row(app: &mut EditorApp, ui: &mut egui::Ui, name: &str, depth: usize, tc: &mut TreeCtx) {
    if tc.visible.as_ref().is_some_and(|v| !v.contains(name)) {
        return;
    }
    let children: Vec<String> = app.doc.bones.iter().filter(|b| b.parent.as_deref() == Some(name)).map(|b| b.name.clone()).collect();
    let filtering = tc.visible.is_some();
    let folded = !filtering && tc.collapsed.contains(name);
    let hidden = app.hidden_bones.contains(name);
    let locked = app.locked_bones.contains(name);
    let selected = app.selection.bone.as_deref() == Some(name) && app.selection.slot.is_none();
    let animated = app.animation().is_some_and(|a| a.bones.contains_key(name));
    let parent = app.doc.bone(name).and_then(|b| b.parent.clone());
    let slots_here: Vec<String> = app.doc.slots.iter().rev().filter(|s| s.bone == name && (!filtering || matches_q(&tc.query, &s.name))).map(|s| s.name.clone()).collect();
    let show_slots = tc.show_slots || filtering;
    let has_children = !children.is_empty() || (show_slots && !slots_here.is_empty());
    let bone_ref = app.doc.bone(name);
    let own_text = bone_ref.map_or(BONE, theme::bone_text_color);
    let own_icon = bone_ref.map(theme::bone_text_color).filter(|_| !hidden);
    let color = if hidden {
        TEXT_DIM.gamma_multiply(0.75)
    } else if animated {
        own_text
    } else if filtering && !matches_q(&tc.query, name) {
        TEXT_DIM
    } else {
        TEXT
    };
    // Folded rows say what they hide.
    let meta = if folded {
        match (children.len(), slots_here.len()) {
            (0, s) => crate::tf!("{} slots", s),
            (b, 0) => crate::tf!("{} bones", b),
            (b, s) => crate::tf!("{} bones · {} slots", b, s),
        }
    } else if !tc.show_slots && !slots_here.is_empty() {
        crate::tf!("{} slots", slots_here.len())
    } else {
        String::new()
    };
    let badges = tc.badges.get(name).cloned().unwrap_or_default();
    let out = tree_row(ui, tc, RowSpec {
        key: format!("bone:{name}"),
        depth,
        expander: (has_children && !filtering).then_some(!folded),
        kind: NodeKind::Bone,
        name,
        color,
        icon_color: own_icon,
        meta,
        selected,
        eye: Some(!hidden),
        lock: Some(locked),
        draggable: true,
        keys: [key_btn(app, &crate::timeline::KeyRow::Bone(name.to_string()), selected, "Key this bone (changed properties, else the active tool's property) [K]"), None],
        badges: &badges,
        ..Default::default()
    });
    if let Some(badge) = out.badge.and_then(|i| badges.get(i)) {
        select_constraint(app, tc, badge);
    }
    if out.keyed[0] {
        crate::timeline::key_row(app, &crate::timeline::KeyRow::Bone(name.to_string()));
    }
    if out.expander {
        set_flag(&mut tc.collapsed, name, !folded);
    }
    if out.eye {
        set_flag(&mut app.hidden_bones, name, !hidden);
    }
    if out.lock {
        set_flag(&mut app.locked_bones, name, !locked);
    }
    let r = out.main;
    r.dnd_set_drag_payload(TreeDrag::Bone(name.to_string()));
    let r = r.on_hover_text(match parent.as_deref() {
        Some(p) => crate::tf!("{} (child of {})", name, p),
        None => name.to_string(),
    });
    if r.hovered() {
        app.view.tree_hover = Some(crate::viewport::Pick::Bone(name.to_string()));
    }
    if r.double_clicked() {
        start_rename(app, ui, name);
    } else if r.clicked() {
        app.selection.bone = Some(name.to_string());
        app.selection.slot = None;
        app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
    }
    r.context_menu(|ui| {
        if ui.button(tr("Add child bone")).clicked() {
            add_bone(app, Some(name.to_string()));
            ui.close();
        }
        if ui.button(tr("Add slot")).clicked() {
            add_slot(app, name.to_string());
            ui.close();
        }
        if ui.button(tr("Import images onto this bone...")).clicked() {
            ui.close();
            import_onto_bone(app, name);
        }
        if ui.button(tr("Add IK constraint")).on_hover_text(tr("Constrain this bone (and its parent) to a new target bone")).clicked() {
            add_ik(app, name);
            ui.close();
        }
        if ui.button(tr("Add physics constraint")).on_hover_text(tr("Make this bone lag behind its animated pose on a damped spring")).clicked() {
            crate::dialogs::new_physics(app, Some(name.to_string()));
            ui.close();
        }
        ui.separator();
        let is_root = name == skel2d_core::ROOT;
        if is_root {
            ui.add(egui::Label::new(RichText::new(tr(ROOT_REASON)).size(theme::SIZE_SMALL).color(TEXT_DIM)).wrap());
        }
        if ui.add_enabled(!is_root, egui::Button::new(format!("{}    F2", tr("Rename")))).clicked() {
            start_rename(app, ui, name);
            ui.close();
        }
        if ui.button(tr(if hidden { "Show" } else { "Hide" })).clicked() {
            set_flag(&mut app.hidden_bones, name, !hidden);
            ui.close();
        }
        if ui.button(tr(if locked { "Unlock" } else { "Lock" })).clicked() {
            set_flag(&mut app.locked_bones, name, !locked);
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(!is_root, theme::danger_button(&format!("{}    Del", tr("Delete (with children)")))).clicked() {
            crate::dialogs::delete(app, crate::dialogs::Item::Bone(name.to_string()));
            ui.close();
        }
    });
    drop_zone(app, ui, out.rect, tc, |_| DropTarget::Bone(Some(name.to_string())));
    if !folded {
        for child in children {
            bone_row(app, ui, &child, depth + 1, tc);
        }
        // Slots sit under their bone, attachments under their slot, as in Spine's tree.
        if show_slots {
            for slot in slots_here {
                slot_node(app, ui, &slot, depth + 1, tc);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum NodeKind {
    Bone,
    Slot,
    Region,
    Mesh,
    Animation,
    Constraint,
    Physics,
}

/// The type glyph in front of a tree node's name, colored by type.
fn node_icon(ui: &egui::Ui, rect: egui::Rect, kind: NodeKind, own: Option<Color32>) {
    use crate::icons::Icon as I;
    let (icon, color) = match kind {
        NodeKind::Bone => (I::Bone, BONE),
        NodeKind::Slot => (I::Slot, theme::SLOT),
        NodeKind::Region => (I::Image, Color32::from_rgb(130, 180, 235)),
        NodeKind::Mesh => (I::Mesh, Color32::from_rgb(150, 190, 235)),
        NodeKind::Animation => (I::Animation, ANIMATE),
        NodeKind::Constraint => (I::Constraint, IK),
        NodeKind::Physics => (I::Physics, PHYSICS),
    };
    crate::icons::image(icon, rect.width()).tint(own.unwrap_or(color)).paint_at(ui, rect);
}

/// A slot under its bone in the tree: draggable onto other bones, folding
/// its attachments; clicking an attachment's dot makes it the visible one.
fn slot_node(app: &mut EditorApp, ui: &mut egui::Ui, name: &str, depth: usize, tc: &mut TreeCtx) {
    let Some(slot) = app.doc.slot(name).cloned() else { return };
    let key = format!("slot:{name}");
    let folded = tc.collapsed.contains(&key);
    let hidden = app.hidden_slots.contains(name);
    let locked = app.locked_slots.contains(name);
    let selected = app.selection.slot.as_deref() == Some(name);
    let color = if hidden { TEXT_DIM.gamma_multiply(0.75) } else { theme::SLOT };
    // A slot with one attachment names it on the right instead of a child row.
    let single = slot.attachments.is_empty();
    let meta = match &slot.attachment {
        _ if !(folded || single) => String::new(),
        Some(a) if a == name => String::new(),
        Some(a) => a.clone(),
        None => tr("(hidden)").to_string(),
    };
    let out = tree_row(ui, tc, RowSpec {
        key: key.clone(),
        depth,
        expander: (!single).then_some(!folded),
        kind: NodeKind::Slot,
        name,
        color,
        meta,
        selected,
        eye: Some(!hidden),
        lock: Some(locked),
        draggable: true,
        keys: [
            key_btn(app, &crate::timeline::KeyRow::SlotAttachment(name.to_string()), selected, "Key the attachment this slot shows"),
            key_btn(app, &crate::timeline::KeyRow::SlotColor(name.to_string()), selected, "Key this slot's tint"),
        ],
        ..Default::default()
    });
    if out.keyed[0] {
        crate::timeline::key_row(app, &crate::timeline::KeyRow::SlotAttachment(name.to_string()));
    }
    if out.keyed[1] {
        crate::timeline::key_row(app, &crate::timeline::KeyRow::SlotColor(name.to_string()));
    }
    if out.expander {
        set_flag(&mut tc.collapsed, &key, !folded);
    }
    if out.eye {
        set_flag(&mut app.hidden_slots, name, !hidden);
    }
    if out.lock {
        set_flag(&mut app.locked_slots, name, !locked);
    }
    let r = out.main;
    r.dnd_set_drag_payload(TreeDrag::Slot(name.to_string()));
    let r = r.on_hover_text(tr("Slot · drag onto a bone to move it there · right-click for actions"));
    if r.hovered() {
        app.view.tree_hover = Some(crate::viewport::Pick::Slot(name.to_string(), slot.bone.clone()));
    }
    if r.clicked() {
        app.selection.attachment = None;
        app.selection.slot = Some(name.to_string());
        app.selection.bone = Some(slot.bone.clone());
        app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
    }
    r.context_menu(|ui| slot_menu(app, ui, name));
    if folded || single {
        return;
    }
    let pose = app.pose();
    let si = app.doc.slot_index(name);
    let shown = si.and_then(|i| pose.slots.get(i)).and_then(|s| s.attachment.clone());
    for att in &slot.attachments {
        let a = att.name().to_string();
        let visible = shown.as_deref() == Some(a.as_str());
        let mesh = matches!(att, Attachment::Mesh(_) | Attachment::LinkedMesh(_));
        let out = tree_row(ui, tc, RowSpec {
            key: format!("att:{name}:{a}"),
            depth: depth + 1,
            kind: if mesh { NodeKind::Mesh } else { NodeKind::Region },
            name: &a,
            color: if visible { TEXT } else { TEXT_DIM },
            meta: if matches!(att, Attachment::LinkedMesh(_)) { tr("Linked").to_string() } else if mesh { tr("mesh").to_string() } else { String::new() },
            selected: selected && visible,
            dot: Some(visible),
            // A mesh deforms: its own key button, for the vertices as they are now.
            keys: [if mesh { key_btn(app, &crate::timeline::KeyRow::Deform(name.to_string(), a.clone()), selected && visible, "Key the deformation of this mesh") } else { None }, None],
            ..Default::default()
        });
        if out.keyed[0] {
            crate::timeline::key_row(app, &crate::timeline::KeyRow::Deform(name.to_string(), a.clone()));
        }
        let tip = if app.playback.anim.is_some() { "Show this attachment from the playhead (keys the slot)" } else { "Make this the slot's visible attachment" };
        if out.dot && !visible {
            show_attachment(app, name, &a);
        }
        if out.main.on_hover_text(tr(tip)).clicked() {
            app.selection.attachment = Some(a.clone());
            app.selection.slot = Some(name.to_string());
            app.selection.bone = Some(slot.bone.clone());
            app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
            if !visible {
                show_attachment(app, name, &a);
            }
        }
    }
}

/// Selects `bone` and focuses its name field in the properties.
fn start_rename(app: &mut EditorApp, _ui: &egui::Ui, bone: &str) {
    crate::dialogs::rename(app, crate::dialogs::Item::Bone(bone.to_string()));
}

fn tree_navigation(app: &mut EditorApp, ui: &egui::Ui, rows: &[crate::dialogs::Item]) {
    if ui.ctx().egui_wants_keyboard_input() || !matches!(app.dialog, crate::app::Dialog::None) || rows.is_empty() { return; }
    use crate::dialogs::Item;
    let selected = app.selection.slot.clone().map(Item::Slot).or_else(|| app.selection.bone.clone().map(Item::Bone));
    let (up, down) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)));
    if !up && !down { return; }
    let index = selected.as_ref().and_then(|s| rows.iter().position(|r| r == s));
    let next = index.map_or(if up { rows.len() - 1 } else { 0 }, |i| if up { i.saturating_sub(1) } else { (i + 1).min(rows.len() - 1) });
    match &rows[next] {
        Item::Bone(n) => app.selection = crate::app::Selection { bone: Some(n.clone()), ..Default::default() },
        Item::Slot(n) => app.selection = crate::app::Selection { slot: Some(n.clone()), bone: app.doc.slot(n).map(|s| s.bone.clone()), ..Default::default() },
        Item::Ik(n) => app.selection = crate::app::Selection { ik: Some(n.clone()), ..Default::default() },
        Item::Physics(n) => app.selection = crate::app::Selection { physics: Some(n.clone()), ..Default::default() },
        Item::Transform(n) => app.selection = crate::app::Selection { transform: Some(n.clone()), ..Default::default() },
        Item::Skin(n) => select_skin(app, n),
        Item::Animation(n) => open_animation(app, n),
        Item::Attachment { .. } | Item::Event(_) => {}
    }
}

fn tree_keys(app: &mut EditorApp, ui: &egui::Ui) {
    if ui.ctx().egui_wants_keyboard_input() || !matches!(app.dialog, crate::app::Dialog::None) {
        return;
    }
    let item = crate::dialogs::selected_item(app);
    let (f2, del) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::F2), i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)));
    if let Some(item) = item {
        if f2 { crate::dialogs::rename(app, item); }
        else if del { crate::dialogs::delete(app, item); }
    }
}

/// Adds an animation and opens it in animate mode.
fn new_animation(app: &mut EditorApp) {
    crate::dialogs::new_animation(app);
}

fn open_animation(app: &mut EditorApp, name: &str) {
    app.playback.anim = Some(name.to_string());
    app.playback.playing = false;
    app.playback.time = 0.0;
    app.timeline.clear_selection();
}

/// Right-click actions of a slot, shared by its rows under the bone and in Draw Order.
fn slot_menu(app: &mut EditorApp, ui: &mut egui::Ui, name: &str) {
    let (Some(index), count) = (app.doc.slot_index(name), app.doc.slots.len()) else { return };
    let (hidden, locked) = (app.hidden_slots.contains(name), app.locked_slots.contains(name));
    let mut move_to = |ui: &mut egui::Ui, label: &str, enabled: bool, to: usize| {
        if ui.add_enabled(enabled, egui::Button::new(tr(label))).clicked() {
            app.edit(vec![Op::MoveSlot { name: name.to_string(), index: to }]);
            ui.close();
        }
    };
    move_to(ui, "Bring to front", index + 1 < count, count - 1);
    move_to(ui, "Move forward", index + 1 < count, index + 1);
    move_to(ui, "Move back", index > 0, index.saturating_sub(1));
    move_to(ui, "Send to back", index > 0, 0);
    ui.separator();
    if ui.button(tr("Rename")).clicked() {
        crate::dialogs::rename(app, crate::dialogs::Item::Slot(name.to_string()));
        ui.close();
    }
    if ui.button(tr(if hidden { "Show" } else { "Hide" })).clicked() {
        set_flag(&mut app.hidden_slots, name, !hidden);
        ui.close();
    }
    if ui.button(tr(if locked { "Unlock" } else { "Lock" })).clicked() {
        set_flag(&mut app.locked_slots, name, !locked);
        ui.close();
    }
    ui.separator();
    if ui.add(theme::danger_button(&format!("{}    Del", tr("Delete")))).clicked() {
        crate::dialogs::delete(app, crate::dialogs::Item::Slot(name.to_string()));
        ui.close();
    }
}

/// Adds a target bone at the bone's tip and an IK constraint on it (and its
/// parent, if any) in one undo step.
fn add_ik(app: &mut EditorApp, bone: &str) {
    crate::dialogs::new_ik(app, Some(bone.to_string()));
}

/// Keys the attachment the selected slot shows at the playhead. The tint has
/// its own key button.
pub fn key_slot_current(app: &mut EditorApp, anim: &str, slot: &str) {
    let t = app.playback.time;
    let tl = app.doc.animations.get(anim).and_then(|a| a.slots.get(slot));
    let reset = tl.and_then(|tl| skel2d_core::pose::sample_attachment(&tl.attachment, t)).is_some_and(|k| k.reset);
    let shown = app.doc.slot_index(slot).and_then(|i| app.pose().slots.get(i).map(|s| s.attachment.clone())).flatten();
    let op = Op::KeySlot(skel2d_core::ops::KeySlot {
        animation: anim.to_string(),
        slot: slot.to_string(),
        time: t,
        attachment: (!reset).then_some(shown),
        reset,
        color: None,
        curve: None, curves: None,
    });
    app.edit_keyed(vec![op]);
}

fn show_attachment(app: &mut EditorApp, slot: &str, attachment: &str) {
    let op = match app.playback.anim.clone() {
        Some(animation) => Op::KeySlot(skel2d_core::ops::KeySlot {
            animation,
            slot: slot.to_string(),
            time: app.playback.time,
            attachment: Some(Some(attachment.to_string())),
            reset: false,
            color: None,
            curve: None, curves: None,
        }),
        None => Op::SetSlot(SlotPatch { name: slot.to_string(), attachment: Some(Some(attachment.to_string())), ..Default::default() }),
    };
    app.edit(vec![op]);
}

/// A text field that applies on Enter or when focus leaves.
fn text_commit(ui: &mut egui::Ui, id: &str, current: &str) -> Option<String> {
    text_commit_focus(ui, id, current, false)
}

fn text_commit_focus(ui: &mut egui::Ui, id: &str, current: &str, focus: bool) -> Option<String> {
    let id = ui.make_persistent_id(id);
    let mut text: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| current.to_string());
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width().min(400.0)));
    if focus {
        response.request_focus();
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
        None
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
        (response.lost_focus() && text != current && !text.trim().is_empty()).then(|| text.trim().to_string())
    }
}

/// The top of the inspector: a tinted type tile, the kind, and the item's
/// name as an editable field (Enter applies). `context` is a dim second
/// line (parent path, owner bone). Returns the new name.
#[allow(clippy::too_many_arguments)]
fn inspector_header(ui: &mut egui::Ui, icon: crate::icons::Icon, color: Color32, kind: &str, id: &str, name: &str, context: &str, focus: bool) -> Option<String> {
    inspector_header_ex(ui, icon, color, kind, id, name, context, focus, false)
}

/// [`inspector_header`] with a locked name (the root bone): shown with a lock, not editable.
#[allow(clippy::too_many_arguments)]
fn inspector_header_ex(ui: &mut egui::Ui, icon: crate::icons::Icon, color: Color32, kind: &str, id: &str, name: &str, context: &str, focus: bool, locked: bool) -> Option<String> {
    let mut renamed = None;
    egui::Frame::new().fill(theme::HEADER_BG).corner_radius(6).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (tile, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
            ui.painter().rect_filled(tile, 6.0, color.gamma_multiply(0.22));
            ui.painter().rect_stroke(tile, 6.0, egui::Stroke::new(1.0, color.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            crate::icons::image(icon, 22.0).tint(color).paint_at(ui, egui::Rect::from_center_size(tile.center(), egui::vec2(22.0, 22.0)));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(kind).size(theme::SIZE_SMALL).color(color).strong());
                if locked {
                    ui.horizontal(|ui| {
                        if name == skel2d_core::ROOT {
                            ui.add(crate::icons::image(crate::icons::Icon::Lock, 16.0).tint(ACCENT));
                        }
                        ui.label(RichText::new(name).strong().size(theme::SIZE_HEADING));
                    });
                } else {
                    renamed = text_commit_focus(ui, id, name, focus);
                }
            });
        });
        if !context.is_empty() {
            ui.add_space(2.0);
            let job = {
                let mut j = egui::text::LayoutJob::simple_singleline(context.to_string(), egui::FontId::proportional(theme::SIZE_SMALL), TEXT_DIM);
                j.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width());
                j
            };
            ui.label(job).on_hover_text(context);
        }
    });
    ui.add_space(2.0);
    renamed
}

/// A number field in an inspector row, filling `width`.
fn num_field(ui: &mut egui::Ui, v: &mut f32, speed: f64, unit: &str, width: f32, tip: &str) -> egui::Response {
    let mut dv = DragValue::new(v).speed(speed).max_decimals(3);
    if unit == "×" {
        dv = dv.prefix("×");
    } else if !unit.is_empty() {
        dv = dv.suffix(unit);
    }
    ui.add_sized([width.max(40.0), 22.0], dv).on_hover_text(tip_text(tip))
}

/// A labelled single number row; Some(new value) when changed.
fn num_row(ui: &mut egui::Ui, label: &str, value: f32, speed: f64, unit: &str, tip: &str) -> Option<f32> {
    theme::prop(ui, tr(label), "", |ui| {
        let mut v = value;
        let w = ui.available_width();
        let changed = num_field(ui, &mut v, speed, unit, w, tip).changed();
        (changed && v.is_finite()).then_some(v)
    })
}

/// A labelled X/Y pair on one row with colored axis tags.
fn pair_row(ui: &mut egui::Ui, label: &str, value: [f32; 2], speed: f64, unit: &str, tip: &str) -> Option<[f32; 2]> {
    theme::prop(ui, tr(label), "", |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut v = value;
        let w = ((ui.available_width() - 2.0 * 14.0 - 8.0) / 2.0).floor();
        theme::axis_tag(ui, "X", theme::AXIS_X);
        let a = num_field(ui, &mut v[0], speed, unit, w, &format!("{} X", tr(tip))).changed();
        ui.add_space(6.0);
        theme::axis_tag(ui, "Y", theme::AXIS_Y);
        let b = num_field(ui, &mut v[1], speed, unit, w, &format!("{} Y", tr(tip))).changed();
        ((a || b) && v.iter().all(|x| x.is_finite())).then_some(v)
    })
}

/// Whether `bone` hangs somewhere below `ancestor`.
fn is_below(doc: &skel2d_core::Document, bone: &str, ancestor: &str) -> bool {
    std::iter::successors(doc.bone(bone).and_then(|b| b.parent.as_deref()), |n| doc.bone(n).and_then(|b| b.parent.as_deref())).any(|n| n == ancestor)
}

/// "root › hip › chest": where a bone sits in the hierarchy.
fn bone_path(doc: &skel2d_core::Document, bone: &str) -> String {
    let mut chain: Vec<&str> = std::iter::successors(doc.bone(bone).and_then(|b| b.parent.as_deref()), |n| doc.bone(n).and_then(|b| b.parent.as_deref())).collect();
    chain.reverse();
    chain.join(" › ")
}

/// Rotate / translate / scale offsets at the playhead; edits write keys.
fn animate_block(app: &mut EditorApp, ui: &mut egui::Ui, bone: &Bone) {
    let Some(anim) = app.playback.anim.clone() else { return };
    let t = app.playback.time;
    let fps = app.settings.fps.max(1.0);
    let tl = app.doc.animations.get(&anim).and_then(|a| a.bones.get(&bone.name)).cloned().unwrap_or_default();
    let here = |time: f32| (time - t).abs() < TIME_EPS;
    let pending = app.pending_key(&bone.name).cloned();
    let state = |keys: bool, here_key: bool, changed: bool| match (changed, here_key, keys) {
        (true, ..) => KeyState::Changed,
        (_, true, _) => KeyState::Keyed,
        (_, _, true) => KeyState::Interpolated,
        _ => KeyState::None,
    };
    let rot_state = state(!tl.rotate.is_empty(), tl.rotate.iter().any(|k| here(k.time)), pending.as_ref().is_some_and(|p| p.rotate.is_some()));
    let tr_state = state(!tl.translate.is_empty(), tl.translate.iter().any(|k| here(k.time)), pending.as_ref().is_some_and(|p| p.translate.is_some()));
    let sh_state = state(!tl.shear.is_empty(), tl.shear.iter().any(|k| here(k.time)), pending.as_ref().is_some_and(|p| p.shear.is_some()));
    let sc_state = state(!tl.scale.is_empty(), tl.scale.iter().any(|k| here(k.time)), pending.as_ref().is_some_and(|p| p.scale.is_some()));
    let rot = pending.as_ref().and_then(|p| p.rotate).or_else(|| sample(&tl.rotate, t)).unwrap_or(0.0);
    let trans = pending.as_ref().and_then(|p| p.translate).or_else(|| sample(&tl.translate, t)).unwrap_or([0.0, 0.0]);
    let sh = pending.as_ref().and_then(|p| p.shear).or_else(|| sample(&tl.shear, t)).unwrap_or([0.0; 2]);
    let sc = pending.as_ref().and_then(|p| p.scale).or_else(|| sample(&tl.scale, t)).unwrap_or([1.0, 1.0]);

    let _ = fps;
    theme::hint(ui, &crate::tf!("Offsets from the setup pose in \"{}\" at the playhead. Editing a value writes a key here.", anim));

    let mut key = KeyBone { animation: anim.clone(), bone: bone.name.clone(), time: t, rotate: None, translate: None, scale: None, shear: None, curve: None, curves: None };
    let mut field: Option<&str> = None;
    // Which property's key button was clicked: keys its current value.
    let mut keyed: Option<&str> = None;
    // Resetting a value is in the field's right-click menu, not a button per row.
    let reset_menu = |r: &egui::Response, label: &str| {
        let mut hit = false;
        r.context_menu(|ui| {
            if ui.button(tr(label)).clicked() {
                hit = true;
                ui.close();
            }
        });
        hit
    };
    egui::Grid::new("anim_vals").num_columns(3).spacing([6.0, 6.0]).show(ui, |ui| {
        if key_button(ui, rot_state) {
            keyed = Some("rotate");
        }
        lbl_tip_w(ui, 66.0, "Rotate", "Rotation offset added to the setup rotation");
        let mut r = rot;
        let resp = ui.add(DragValue::new(&mut r).speed(0.5).max_decimals(2).suffix("°")).on_hover_text(tip_text("Rotation offset in degrees · right-click: reset"));
        if resp.changed() && r.is_finite() {
            key.rotate = Some(r);
            field = Some("rotate");
        }
        if reset_menu(&resp, "Key rotation 0°") {
            key.rotate = Some(0.0);
            field = Some("rotate_reset");
        }
        ui.end_row();

        if key_button(ui, tr_state) {
            keyed = Some("translate");
        }
        lbl_tip_w(ui, 66.0, "Translate", "Translation offset added to the setup position");
        let mut v = trans;
        let mut changed = false;
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let r = ui.add(DragValue::new(&mut v[0]).speed(0.5).max_decimals(2).prefix("x ")).on_hover_text(tip_text("Move offset X · right-click: reset"));
            changed |= r.changed();
            reset |= reset_menu(&r, "Key move 0, 0");
            let r = ui.add(DragValue::new(&mut v[1]).speed(0.5).max_decimals(2).prefix("y ")).on_hover_text(tip_text("Move offset Y · right-click: reset"));
            changed |= r.changed();
            reset |= reset_menu(&r, "Key move 0, 0");
        });
        if changed && v.iter().all(|x| x.is_finite()) {
            key.translate = Some(v);
            field = Some("translate");
        }
        if reset {
            key.translate = Some([0.0, 0.0]);
            field = Some("translate_reset");
        }
        ui.end_row();

        if key_button(ui, sc_state) {
            keyed = Some("scale");
        }
        lbl_tip_w(ui, 66.0, "Scale", "Factor multiplied with the setup scale (1 = unchanged)");
        let mut v = sc;
        let mut changed = false;
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let r = ui.add(DragValue::new(&mut v[0]).speed(0.01).max_decimals(3).prefix("×x ")).on_hover_text(tip_text("Scale factor X · right-click: reset"));
            changed |= r.changed();
            reset |= reset_menu(&r, "Key scale 1, 1");
            let r = ui.add(DragValue::new(&mut v[1]).speed(0.01).max_decimals(3).prefix("×y ")).on_hover_text(tip_text("Scale factor Y · right-click: reset"));
            changed |= r.changed();
            reset |= reset_menu(&r, "Key scale 1, 1");
        });
        if changed && v.iter().all(|x| x.is_finite()) {
            key.scale = Some(v);
            field = Some("scale");
        }
        if reset {
            key.scale = Some([1.0, 1.0]);
            field = Some("scale_reset");
        }
        ui.end_row();

        if key_button(ui, sh_state) { keyed = Some("shear"); }
        lbl_tip_w(ui, 66.0, "Shear", "Angular offsets added to setup shear X/Y");
        let mut v = sh;
        let mut changed = false;
        ui.horizontal(|ui| {
            for (i, axis) in ["X", "Y"].iter().enumerate() {
                theme::axis_tag(ui, axis, if i == 0 { theme::AXIS_X } else { theme::AXIS_Y });
                changed |= ui.add(DragValue::new(&mut v[i]).speed(0.5).suffix("°")).changed();
            }
        });
        if changed && v.iter().all(|x| x.is_finite()) { key.shear = Some(v); field = Some("shear"); }
        ui.end_row();
    });
    if let Some(f) = field {
        let op = Op::KeyBone(key);
        if f.ends_with("_reset") {
            app.edit(vec![op]);
        } else {
            app.edit_coalesced(format!("animkey:{anim}:{}:{f}:{}", bone.name, (t * 10_000.0).round() as i64), vec![op]);
        }
    }
    if let Some(f) = keyed {
        // Keys the shown value (the unkeyed change if any); other unkeyed
        // fields of the bone stay unkeyed.
        let mut k = KeyBone { animation: anim.clone(), bone: bone.name.clone(), time: t, rotate: None, translate: None, scale: None, shear: None, curve: None, curves: None };
        match f {
            "rotate" => k.rotate = Some(rot),
            "translate" => k.translate = Some(trans),
            "shear" => k.shear = Some(sh),
            _ => k.scale = Some(sc),
        }
        if let Some(p) = app.pending.get_mut(&bone.name) {
            match f {
                "rotate" => p.rotate = None,
                "translate" => p.translate = None,
                "shear" => p.shear = None,
                _ => p.scale = None,
            }
            if p.rotate.is_none() && p.translate.is_none() && p.scale.is_none() && p.shear.is_none() {
                app.pending.remove(&bone.name);
            }
        }
        app.edit_keyed(vec![Op::KeyBone(k)]);
    }
}

/// Key state of one animated property, as Spine colors its key buttons.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum KeyState {
    /// No keys for this property in the animation.
    None,
    /// Keyed elsewhere; the value here is interpolated.
    Interpolated,
    /// Keyed at the playhead.
    Keyed,
    /// Changed but not keyed yet (Auto Key off).
    Changed,
}

/// A key button: shape + color + word. A filled diamond is keyed here, a
/// hollow diamond is not keyed here, a filled circle is changed but unkeyed.
/// Returns true when clicked.
fn key_button(ui: &mut egui::Ui, state: KeyState) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(84.0, theme::MIN_CLICK_H - 4.0), egui::Sense::click());
    let (color, word, tip) = match state {
        KeyState::Keyed => (theme::KEY_SET, "Keyed", "Keyed at this time · click to key the current value again"),
        KeyState::Changed => (theme::KEY_CHANGED, "Changed", "Changed but not keyed · click to key it (or press K)"),
        KeyState::Interpolated => (theme::KEY_NONE, "Between", "Keyed at other times; this value is interpolated · click to key it here"),
        KeyState::None => (theme::KEY_NONE, "Key", "Not keyed in this animation · click to key the current value"),
    };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if r.hovered() { Color32::from_gray(74) } else { Color32::from_gray(58) });
    if r.has_focus() {
        p.rect_stroke(rect, 4.0, egui::Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
    }
    let c = egui::pos2(rect.left() + 14.0, rect.center().y);
    let diamond = |s: f32| vec![c + egui::vec2(0.0, -s), c + egui::vec2(s, 0.0), c + egui::vec2(0.0, s), c + egui::vec2(-s, 0.0)];
    match state {
        KeyState::Keyed => {
            p.add(egui::Shape::convex_polygon(diamond(7.0), color, egui::Stroke::NONE));
        }
        KeyState::Changed => {
            p.circle_filled(c, 6.0, color);
        }
        KeyState::Interpolated | KeyState::None => {
            // Hollow: an outer diamond with the button color punched into it.
            p.add(egui::Shape::convex_polygon(diamond(7.0), color, egui::Stroke::NONE));
            p.add(egui::Shape::convex_polygon(diamond(4.0), if r.hovered() { Color32::from_gray(74) } else { Color32::from_gray(58) }, egui::Stroke::NONE));
        }
    }
    p.text(egui::pos2(rect.left() + 28.0, rect.center().y), egui::Align2::LEFT_CENTER, tr(word), egui::FontId::proportional(theme::SIZE_SMALL + 1.0), theme::TEXT);
    r.on_hover_text(tr(tip)).clicked()
}

fn bone_properties(app: &mut EditorApp, ui: &mut egui::Ui, bone: &Bone) {
    let animating = app.playback.anim.is_some();
    let name = bone.name.clone();
    let mut patch: Option<(&str, BonePatch)> = None;
    let rename_id = egui::Id::unique("rename_bone");
    let focus = ui.ctx().data(|d| d.get_temp::<String>(rename_id)).as_deref() == Some(name.as_str());
    if focus {
        ui.ctx().data_mut(|d| d.remove::<String>(rename_id));
    }
    let is_root = name == skel2d_core::ROOT;
    let path = bone_path(&app.doc, &name);
    let context = if is_root { tr("Root bone · locked").to_string() } else if path.is_empty() { tr("Top-level bone").to_string() } else { path };
    if let Some(new) = inspector_header_ex(ui, crate::icons::Icon::Bone, theme::bone_text_color(bone), tr("Bone"), &format!("bone_name_{name}"), &name, &context, focus, is_root) {
        patch = Some(("rename", BonePatch { name: name.clone(), rename: Some(new), ..Default::default() }));
    }
    if is_root {
        ui.add(egui::Label::new(RichText::new(tr(ROOT_REASON)).size(theme::SIZE_SMALL).color(TEXT_DIM)).wrap());
    } else {
        theme::prop(ui, tr("Parent"), tr("Reparenting keeps the bone where it is"), |ui| {
            let mut parent = bone.parent.clone();
            // A bone always has a parent, and cannot go under its own children.
            let blocked: Vec<String> = app.doc.bones.iter().filter(|b| b.name == name || is_below(&app.doc, &b.name, &name)).map(|b| b.name.clone()).collect();
            egui::ComboBox::from_id_salt("parent").selected_text(parent.as_deref().unwrap_or(skel2d_core::ROOT)).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                for b in app.doc.bones.iter().filter(|b| !blocked.contains(&b.name)) {
                    ui.selectable_value(&mut parent, Some(b.name.clone()), &b.name);
                }
            });
            if parent != bone.parent {
                // Keeps the bone where it is, like dragging it in the tree.
                reparent_bone(app, &name, parent);
            }
        });
    }
    let mut color_children = false;
    theme::prop(ui, tr("Bone color"), tr("Only how the bone looks in the viewport. Pick a color that stands out against your image."), |ui| {
        let [r, g, b, a] = bone.color.map_or(theme::BONE.to_array(), |c| c.to_rgba8());
        let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
        if ui.color_edit_button_srgba(&mut c).changed() {
            let [r, g, b, a] = c.to_srgba_unmultiplied();
            let color = skel2d_core::Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 };
            patch = Some(("color", BonePatch { name: name.clone(), color: Some(Some(color)), ..Default::default() }));
        }
        if bone.color.is_some() && ui.small_button(tr("Reset")).clicked() {
            patch = Some(("color", BonePatch { name: name.clone(), color: Some(None), ..Default::default() }));
        }
        color_children = ui.small_button(tr("Apply to children")).clicked();
    });
    if color_children {
        let ops: Vec<Op> = app.doc.bones.iter().filter(|b| b.name != name && bone_path(&app.doc, &b.name).split(" › ").any(|p| p == name))
            .map(|b| Op::SetBone(BonePatch { name: b.name.clone(), color: Some(bone.color), ..Default::default() })).collect();
        if !ops.is_empty() { app.edit(ops); }
    }
    if !app.doc.skins.is_empty() || bone.skin_required {
        theme::prop(ui, tr("Skin required"), tr("Only active (with its slots and children) while a skin that lists this bone is on"), |ui| {
            let mut req = bone.skin_required;
            if ui.checkbox(&mut req, "").changed() {
                patch = Some(("skin_required", BonePatch { name: name.clone(), skin_required: Some(req), ..Default::default() }));
            }
            let skins: Vec<&str> = app.doc.skins.iter().filter(|k| k.bones.contains(&name)).map(|k| k.name.as_str()).collect();
            if req {
                theme::hint(ui, &if skins.is_empty() { tr("in no skin yet").to_string() } else { skins.join(", ") });
            }
        });
    }

    if animating {
        let t = app.playback.time;
        let summary = format!("{t:.2}s · f{}", (t * app.settings.fps.max(1.0)).round() as i64);
        theme::group(ui, "bone_anim", tr("Animation"), &summary, true, |ui| animate_block(app, ui, bone));
    }

    let setup_title = tr(if animating { "Setup pose (rig, not keyed)" } else { "Transform" });
    let p = |f: &dyn Fn(&mut BonePatch)| {
        let mut bp = BonePatch { name: name.clone(), ..Default::default() };
        f(&mut bp);
        bp
    };
    theme::group(ui, ("bone_setup", animating), setup_title, "", !animating, |ui| {
        if animating {
            theme::hint(ui, tr("These edit the rig itself, not the animation."));
        }
        if let Some(v) = pair_row(ui, "Position", [bone.x, bone.y], 0.5, "", "Position relative to the parent bone") {
            patch = Some(("xy", p(&|b| (b.x, b.y) = (Some(v[0]), Some(v[1])))));
        }
        if let Some(v) = num_row(ui, "Rotation", bone.rotation, 0.5, "°", "Setup rotation in degrees") {
            patch = Some(("rotation", p(&|b| b.rotation = Some(v))));
        }
        if let Some(v) = pair_row(ui, "Scale", [bone.scale_x, bone.scale_y], 0.01, "×", "Setup scale factor") {
            patch = Some(("scale", p(&|b| (b.scale_x, b.scale_y) = (Some(v[0]), Some(v[1])))));
        }
        if let Some(v) = num_row(ui, "Length", bone.length, 0.5, "", "Bone length (drawn tip; children attach here)") {
            patch = Some(("length", p(&|b| b.length = Some(v))));
        }
        if let Some(v) = pair_row(ui, "Shear", [bone.shear_x, bone.shear_y], 0.5, "°", "Setup shear angles in degrees") {
            patch = Some(("shear", p(&|b| (b.shear_x, b.shear_y) = (Some(v[0]), Some(v[1])))));
        }
    });
    let inheritance = [
        (skel2d_core::Inherit::Normal, "Normal", "Inherit parent rotation, scale, shear and reflection"),
        (skel2d_core::Inherit::OnlyTranslation, "Only translation", "Inherit parent translation only"),
        (skel2d_core::Inherit::NoRotationOrReflection, "No rotation or reflection", "Inherit scale without parent rotation or reflection"),
        (skel2d_core::Inherit::NoScale, "No scale", "Inherit rotation and reflection without parent scale"),
        (skel2d_core::Inherit::NoScaleOrReflection, "No scale or reflection", "Inherit rotation without parent scale or reflection"),
    ];
    let current = inheritance.iter().find(|(mode, _, _)| *mode == bone.inherit).expect("all inheritance modes");
    theme::group(ui, "bone_inherit", tr("Inheritance"), tr(current.1), false, |ui| {
        if animating { theme::hint(ui, tr("Inheritance is a setup setting; switch to setup mode to edit")); }
        ui.add_enabled_ui(!animating, |ui| {
            let mut value = bone.inherit;
            egui::ComboBox::from_id_salt("bone_inherit_mode").selected_text(tr(current.1)).show_ui(ui, |ui| {
                for (mode, label, hint) in inheritance { ui.selectable_value(&mut value, mode, tr(label)).on_hover_text(tr(hint)); }
            });
            if value != bone.inherit { patch = Some(("inherit", p(&|b| b.inherit = Some(value)))); }
        });
        theme::hint(ui, tr(current.2));
    });
    if let Some((field, bp)) = patch {
        let renamed = bp.rename.clone();
        if app.edit_coalesced(format!("bone:{name}:{field}"), vec![Op::SetBone(bp)])
            && let Some(n) = renamed
        {
            app.selection.bone = Some(n);
        }
    }

    let pose = app.pose();
    if let Some(i) = app.doc.bone_index(&name) {
        let w = &pose.bones[i].world;
        let summary = format!("{:.1}, {:.1} · {:.1}°", w.tx, w.ty, w.rotation_deg());
        theme::group(ui, "bone_world", tr("World"), &summary, false, |ui| {
            theme::prop(ui, tr("Position"), "", |ui| ui.label(format!("{:.2}, {:.2}", w.tx, w.ty)));
            theme::prop(ui, tr("Rotation"), "", |ui| ui.label(format!("{:.2}°", w.rotation_deg())));
            theme::hint(ui, tr("Where the bone ends up after its parents, at the playhead (read only)."));
        });
    }
    let slots: Vec<String> = app.doc.slots.iter().rev().filter(|s| s.bone == name).map(|s| s.name.clone()).collect();
    if !slots.is_empty() {
        theme::group(ui, "bone_slots", tr("Slots"), &slots.len().to_string(), true, |ui| {
            for s in slots {
                ui.horizontal(|ui| {
                    ui.add(crate::icons::image(crate::icons::Icon::Slot, 14.0).tint(theme::SLOT));
                    if ui.link(RichText::new(&s).color(theme::SLOT)).on_hover_text(tr("Select this slot")).clicked() {
                        app.selection.slot = Some(s.clone());
                    }
                });
            }
        });
    }
}

/// Asks for images and adds them as slots on `bone`, centered on it in the setup pose.
fn import_onto_bone(app: &mut EditorApp, bone: &str) {
    let files = crate::import::pick_images();
    let setup = crate::import::setup_pose(&app.doc);
    if !files.is_empty()
        && let Some(i) = app.doc.bone_index(bone)
    {
        let at = setup.bones[i].world.apply(app.doc.bones[i].length / 2.0, 0.0);
        crate::import::import_images(app, &files, at, Some(bone.to_string()));
    }
}

pub fn go_to_linked_parent(app: &mut EditorApp, slot: &str, link: &skel2d_core::LinkedMesh) {
    if let Some(parent_skin) = &link.parent_skin {
        app.skins.retain(|k| k != parent_skin);
        app.skins.push(parent_skin.clone());
    } else {
        app.skins.retain(|k| !app.doc.skin(k).is_some_and(|s| s.attachment(slot, &link.parent).is_some()));
    }
    app.selection.slot = Some(slot.to_string());
    app.selection.attachment = Some(link.parent.clone());
}

/// The Weights tool's panel, after Spine's Weights view: brush mode and
/// settings, the mesh's bones in their overlay colors with lock toggles and
/// the picked vertices' weights (editable directly), and Smooth / Prune /
/// Swap. Returns the edit to apply.
fn weights_panel(app: &mut EditorApp, ui: &mut egui::Ui, slot: &str, mesh: &skel2d_core::Mesh, slot_bone: &str, picked: &[usize]) -> Option<Op> {
    use crate::meshedit::{self, BrushMode};
    use skel2d_core::ops::{WeightMode, Redistribute};
    let skin=app.skin_of(slot,&mesh.name);
    let vertices=(!picked.is_empty()).then(||picked.to_vec());
    let locks=app.view.weight_locked.iter().cloned().collect::<Vec<_>>();
    let mut out = None;
    // Bones of this mesh, plus the paint bone when it has no weight yet.
    // M3 stores zero-influence bones explicitly rather than inferring only painted ones.
    let bones = skel2d_core::mesh::bound_bones(mesh, slot_bone);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("Bound bones (click one to paint it)")).strong());
        let mut add: Option<String> = None;
        egui::ComboBox::from_id_salt("bind_bone").selected_text(tr("+ Bone")).show_ui(ui, |ui| {
            for b in app.doc.bones.iter().filter(|b| !bones.contains(&b.name)) {
                if ui.selectable_label(false, &b.name).clicked() {
                    add = Some(b.name.clone());
                }
            }
        }).response.on_hover_text(tr("Bind a bone, including zero influence, then paint it"));
        if let Some(b) = add {
            out=Some(Op::BindBones { slot:slot.into(),attachment:mesh.name.clone(),skin:skin.clone(),add:vec![b.clone()],remove:vec![],redistribute:Redistribute::Auto });
            app.selection.bone = Some(b);
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(app.selection.bone.as_ref().is_some_and(|b|bones.contains(b)),egui::Button::new(tr("Remove selected bone"))).clicked() {
            crate::dialogs::weight_options(app,crate::dialogs::WeightAction::Bind);
        }
        if ui.button(tr("Normalize weights")).clicked() { out=Some(Op::NormalizeWeights {slot:slot.into(),attachment:mesh.name.clone(),skin:skin.clone()}); }
    });
    if !app.selection.bone.as_ref().is_some_and(|b| bones.contains(b)) {
        note_label(ui, tr("Remove is off until you pick one of the bound bones below."));
    }
    if !picked.is_empty() {
        theme::hint(ui, &crate::tf!("{} vertices picked: drag a value to set it directly (* = mixed)", picked.len()));
    }
    let mut weight_tree=TreeCtx {badges:Default::default(),navigation:vec![],collapsed:BTreeSet::new(),visible:None,drop:None,query:String::new(),show_slots:false,row:0,reveal:None};
    for bone in &bones {
        let active=app.selection.bone.as_deref()==Some(bone.as_str());
        let row=tree_row(ui,&mut weight_tree,RowSpec {key:format!("weight:{bone}"),name:bone,color:crate::viewport::bone_color(bone),selected:active,..Default::default()});
        if row.main.on_hover_text(tr("Paint this bone")).clicked() {app.selection.bone=Some(bone.clone());}
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, crate::viewport::bone_color(bone));
            let locked = app.view.weight_locked.contains(bone);
            if lock_icon_toggle(ui, locked, if locked { "Unlock: let the weight tools change this bone" } else { "Lock: the weight tools keep this bone's weights" }) {
                set_flag(&mut app.view.weight_locked, bone, !locked);
            }
            let indices=vertices.clone().unwrap_or_else(||(0..mesh.vertices.len()).collect());
            let values: Vec<f32> = indices.iter().filter_map(|&i| mesh.vertices.get(i)).map(|v| crate::viewport::bone_weight(v, bone, slot_bone)).collect();
            let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
            let mixed = values.iter().any(|v| (v - mean).abs() > 0.005);
            ui.add(egui::ProgressBar::new(mean).desired_width(theme::WEIGHT_BAR_WIDTH).show_percentage());
            let mut pct = mean * 100.0;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dv = DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).max_decimals(1).suffix("%");
                let changed = ui.add_enabled(!locked, dv).changed();
                if mixed {
                    ui.label(RichText::new("*").color(TEXT_DIM));
                }
                if changed {
                    out = Some(Op::SetBoneWeight { slot:slot.into(),attachment:mesh.name.clone(),skin:skin.clone(),vertices:indices,bone:bone.clone(),value:pct/100.0,mode:WeightMode::Set,locked_bones:locks.clone() });
                }
            });
        });
    }
    weight_legend(ui);
    theme::section(ui, tr("Brush"));
    note_label(ui, tr("Pick a bone above, then drag over the image to give it more weight. Ctrl+drag takes weight away."));

    ui.horizontal(|ui| {
        for (mode, label, tip) in [
            (BrushMode::Add, "Add", "Raise the paint bone's weight up to Strength (others shrink to keep 100%)"),
            (BrushMode::Remove, "Remove", "Lower the paint bone's weight; it goes to the other bones (also Ctrl while painting)"),
            (BrushMode::Replace, "Replace", "Move the paint bone's weight toward exactly Strength"),
        ] {
            ui.selectable_value(&mut app.settings.brush_mode, mode, tr(label)).on_hover_text(tr(tip));
        }
    });
    egui::Grid::new("brush").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        lbl_tip(ui, "Size", "Brush radius in screen pixels ([ and ] in the viewport)");
        ui.add(egui::Slider::new(&mut app.settings.brush_radius, 4.0..=400.0).logarithmic(true).suffix(" px"));
        ui.end_row();
        lbl_tip(ui, "Strength", "Weight that Add builds up to and Replace moves toward");
        ui.add(egui::Slider::new(&mut app.settings.brush_target, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        ui.end_row();
        lbl_tip(ui, "Speed", "How fast weight changes while painting");
        ui.add(egui::Slider::new(&mut app.settings.brush_strength, 0.05..=2.0));
        ui.end_row();
        lbl_tip(ui, "Feather", "Outer share of the brush over which its effect fades to nothing");
        ui.add(egui::Slider::new(&mut app.settings.brush_feather, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        ui.end_row();
    });
    ui.checkbox(&mut app.settings.weights_all_bones, tr("Overlay: all bones in their colors")).on_hover_text(tr("Off: heat map of the paint bone only (blue 0, red/yellow 1)"));
    theme::section(ui, tr("Check the result"));
    note_label(ui, tr("Bend a bone to see how the image follows: Alt+drag a bone in the viewport, or tick the box. Release to put it back."));
    if ui.checkbox(&mut app.view.pose_test, tr("Bend the bone to test")).on_hover_text(tr("Alt+drag to rotate the paint bone temporarily; release to restore setup")).changed() && app.view.pose_test && app.view.pose_angle == 0.0 {
        app.view.pose_angle = 30.0;
    }
    if app.view.pose_test {
        ui.label(tr("Hold T to paint in the test pose; release T to restore setup"));
        ui.add(egui::Slider::new(&mut app.view.pose_angle, -180.0..=180.0).text(tr("Test angle")));
    }
    if !app.view.pose_test {
        app.view.pose_rotation = None;
    }
    if !picked.is_empty() {
        theme::hint(ui, &crate::tf!("{} vertices picked: drag a value to set it directly (* = mixed)", picked.len()));
    }

    theme::section(ui, tr("Clean up and copy"));
    let scope = if picked.is_empty() { tr("all vertices") } else { tr("picked vertices") };
    note_label(ui, &crate::tf!("These work on the {}. To pick vertices: click one, or drag a box in the viewport.", scope));
    ui.horizontal_wrapped(|ui| {
        if ui.button(tr("Smooth")).on_hover_text(crate::tf!("Average weights with neighbouring vertices ({}); click again to spread further", scope)).clicked() {
            out = Some(Op::SmoothWeights { slot:slot.into(),attachment:mesh.name.clone(),skin:skin.clone(),vertices:vertices.clone(),factor:0.5,iterations:1,locked_bones:locks.clone() });
        }
        if ui.add_enabled(!picked.is_empty(), egui::Button::new(tr("Copy"))).on_hover_text(tr("Copy the picked vertices' average weights (Ctrl+C in the viewport)")).clicked() {
            app.view.weight_clipboard = Some(meshedit::copy_weights(mesh, picked, slot_bone));
        }
        let can_paste = !picked.is_empty() && app.view.weight_clipboard.is_some();
        if ui.add_enabled(can_paste, egui::Button::new(tr("Paste"))).on_hover_text(tr("Give the picked vertices the copied weights (Ctrl+V in the viewport)")).clicked()
            && let Some(w) = &app.view.weight_clipboard
        {
            out = Some(crate::viewport::paste_weight_op(app,slot,mesh,picked,w));
        }
    });
    note_label(ui, tr("Smooth blends the weights with the neighbours to remove harsh edges. Copy and Paste move weights between vertices."));
    if picked.is_empty() {
        note_label(ui, tr("Copy and Paste are off until you pick vertices."));
    } else if app.view.weight_clipboard.is_none() {
        note_label(ui, tr("Paste is off until you copy weights."));
    }
    if ui.button(tr("Remove small weights…")).on_hover_text(tr("Preview threshold and maximum influences before applying")).clicked() {
        crate::dialogs::weight_options(app,crate::dialogs::WeightAction::Prune);
    }
    note_label(ui, tr("Drops tiny weights and limits how many bones move one vertex, so the mesh deforms cleanly."));
    let sid = ui.make_persistent_id("swap_with");
    let mut other: Option<String> = ui.data_mut(|d| d.get_temp(sid)).flatten();
    let active = app.selection.bone.clone();
    let ok = active.is_some() && other.is_some() && active != other;
    ui.horizontal(|ui| {
        if ui.add_enabled(ok, egui::Button::new(tr("Swap with"))).on_hover_text(crate::tf!("Exchange the paint bone's weights with the chosen bone's ({})", scope)).clicked()
            && let (Some(a), Some(b)) = (&active, &other)
        {
            out = Some(Op::SwapWeights {slot:slot.into(),attachment:mesh.name.clone(),skin:skin.clone(),vertices:vertices.clone(),a:a.clone(),b:b.clone(),locked_bones:locks.clone()});
        }
        egui::ComboBox::from_id_salt("swap_bone").selected_text(other.as_deref().unwrap_or(tr("(pick a bone)"))).show_ui(ui, |ui| {
            for b in &bones {
                ui.selectable_value(&mut other, Some(b.clone()), b);
            }
        });
    });
    if !ok {
        note_label(ui, tr("Swap is off until a paint bone is picked in the list and another bone is chosen here."));
    }
    ui.data_mut(|d| d.insert_temp(sid, Some(other)));
    out
}

fn ik_properties(app: &mut EditorApp, ui: &mut egui::Ui, ik: &IkConstraint) {
    let name = ik.name.clone();
    let anim = app.playback.anim.clone();
    let time = app.playback.time;
    // The values in effect at the playhead, as the pose uses them.
    let now = crate::ikbend::ik_values(&app.shown_doc(), anim.as_deref(), time, ik);
    let own_keys: Vec<skel2d_core::Key<skel2d_core::IkKey>> = app.animation().and_then(|a| a.ik.get(&name)).cloned().unwrap_or_default();
    // Changed but not keyed yet (Auto Key off).
    let held: Option<skel2d_core::IkKey> = app.pending_other.iter().find_map(|o| match o {
        Op::KeyIk { ik: i, mix, softness, bend_positive, compress, stretch, .. } if *i == name => Some(skel2d_core::IkKey { mix: *mix, softness: *softness, bend_positive: *bend_positive, compress: *compress, stretch: *stretch }),
        _ => None,
    });
    let mut edit = IkEdit::default();
    let mut patch: Option<(&str, IkPatch)> = None;
    let context = format!("{} → {}", ik.bones.join(" › "), ik.target);
    if let Some(new) = inspector_header(ui, crate::icons::Icon::Constraint, IK, tr("IK constraint"), &format!("ik_name_{name}"), &name, &context, false) {
        patch = Some(("rename", IkPatch { name: name.clone(), rename: Some(new), ..Default::default() }));
    }
    egui::Grid::new("ik").num_columns(2).show(ui, |ui| {

        lbl_tip(ui, "Constraint order", "Lower orders run first; equal orders keep document order");
        let mut order = ik.order;
        if ui.add(egui::DragValue::new(&mut order)).changed() {
            patch = Some(("order", IkPatch { name: name.clone(), order: Some(order), ..Default::default() }));
        }
        ui.end_row();

        lbl(ui, "Bones");
        ui.label(ik.bones.join(" > "));
        ui.end_row();

        lbl(ui, "Target");
        let mut target = ik.target.clone();
        egui::ComboBox::from_id_salt("ik_target").selected_text(&target).show_ui(ui, |ui| {
            for b in app.doc.bones.iter().filter(|b| !ik.bones.contains(&b.name)) {
                ui.selectable_value(&mut target, b.name.clone(), &b.name);
            }
        });
        if target != ik.target {
            patch = Some(("target", IkPatch { name: name.clone(), target: Some(target), ..Default::default() }));
        }
        ui.end_row();

    });

    let animating = anim.is_some();
    if animating {
        theme::hint(ui, &crate::tf!("Animate mode: values below key at the playhead in \"{}\". Switch to SETUP to change the rig itself.", anim.clone().unwrap_or_default()));
    } else {
        theme::hint(ui, tr("Values here are the setup. Pick an animation to key them."));
    }
    let two_bone = ik.bones.len() == 2;
    let one_bone = ik.bones.len() == 1;
    // Whether the animation has a key holding this channel, and one at the playhead.
    let state = |has: fn(&skel2d_core::IkKey) -> bool| -> KeyState {
        let at = own_keys.iter().any(|k| (k.time - time).abs() < TIME_EPS && has(&k.value));
        if held.as_ref().is_some_and(has) { KeyState::Changed }
        else if at { KeyState::Keyed } else if own_keys.iter().any(|k| has(&k.value)) { KeyState::Interpolated } else { KeyState::None }
    };

    ik_row(ui, animating, state(|k| k.mix.is_some()), "Mix", "How much of the IK shows. 0 keeps the animated pose, 1 reaches the target", None, &mut edit.key_mix, |ui| {
        let mut mix = now.mix;
        ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(40.0);
        if ui.add(egui::Slider::new(&mut mix, 0.0..=1.0).max_decimals(2)).on_hover_text(tip_text("How much of the IK shows. 0 keeps the animated pose, 1 reaches the target")).changed() {
            edit.mix = Some(mix);
        }
    });
    let soft_why = (!two_bone).then_some("Softness only applies to a two-bone chain");
    ik_row(ui, animating, state(|k| k.softness.is_some()), "Softness", "Distance from full reach where the chain starts slowing down, so the limb never snaps straight (world units)", soft_why, &mut edit.key_softness, |ui| {
        let mut v = now.softness;
        let r = ui.add_enabled(two_bone, DragValue::new(&mut v).speed(0.5).range(0.0..=100_000.0).max_decimals(2));
        r.on_hover_text(tip_text("Distance from full reach where the chain starts slowing down, so the limb never snaps straight (world units)"))
            .on_disabled_hover_text(tr("Softness only applies to a two-bone chain"));
        if v.is_finite() && v != now.softness {
            edit.softness = Some(v.max(0.0));
        }
    });
    if two_bone {
        ik_row(ui, animating, state(|k| k.bend_positive.is_some()), "Bend", "Which way the middle joint (knee, elbow) folds. Also: drag the round handle by the joint across the limb, or press I.", None, &mut edit.key_bend, |ui| {
            let pose = app.pose();
            let chain = crate::ikbend::chain(&app.doc, &pose, ik);
            for bend in [true, false] {
                // Screen direction the joint points for this bend.
                let arrow = chain.as_ref().map_or(if bend { "↻" } else { "↺" }, |c| {
                    let g = [c.target[0] - c.root[0], c.target[1] - c.root[1]];
                    let n = if bend { [g[1], -g[0]] } else { [-g[1], g[0]] };
                    if n[0].abs() >= n[1].abs() { if n[0] > 0.0 { "→" } else { "←" } } else if n[1] > 0.0 { "↑" } else { "↓" }
                });
                if ui.selectable_label(now.bend_positive == bend, crate::tf!("Joint {}", arrow)).on_hover_text(tr("Fold the middle joint this way")).clicked() && now.bend_positive != bend {
                    edit.bend = Some(bend);
                }
            }
        });
    }
    let compress_why = (!one_bone).then_some("Compress only applies to a one-bone chain");
    ik_row(ui, animating, state(|k| k.compress.is_some()), "Compress", "Shortens the bone when the target is closer than its length", compress_why, &mut edit.key_compress, |ui| {
        let mut v = now.compress;
        let r = ui.add_enabled(one_bone, egui::Checkbox::new(&mut v, tr("Shorten")));
        r.on_hover_text(tr("Shortens the bone when the target is closer than its length"))
            .on_disabled_hover_text(tr("Compress only applies to a one-bone chain"));
        if v != now.compress { edit.compress = Some(v); }
    });
    ik_row(ui, animating, state(|k| k.stretch.is_some()), "Stretch", "Lengthens the bones when the target is out of reach", None, &mut edit.key_stretch, |ui| {
        let mut v = now.stretch;
        ui.checkbox(&mut v, tr("Lengthen")).on_hover_text(tr("Lengthens the bones when the target is out of reach"));
        if v != now.stretch { edit.stretch = Some(v); }
    });

    if let Some((field, p)) = patch {
        let renamed = p.rename.clone();
        if app.edit_coalesced(format!("ik:{name}:{field}"), vec![Op::SetIk(p)])
            && let Some(n) = renamed {
                app.selection.ik = Some(n);
            }
        return;
    }
    if let Some(a) = anim {
        // Key button clicks key the shown value now; edits only change the
        // value (a key with Auto Key on, an unkeyed change with it off).
        let blank = || Op::KeyIk { animation: a.clone(), ik: name.clone(), time, mix: None, softness: None, bend_positive: None, compress: None, stretch: None, curve: None, curves: None };
        let (mut changed, mut keyed) = (blank(), blank());
        let (mut changed_fields, mut keyed_fields) = (String::new(), String::new());
        if let (Op::KeyIk { mix, softness, bend_positive, compress, stretch, .. }, Op::KeyIk { mix: kmix, softness: ksoft, bend_positive: kbend, compress: kcomp, stretch: kstretch, .. }) = (&mut changed, &mut keyed) {
            if let Some(v) = edit.mix { *mix = Some(v); changed_fields.push_str("mix,"); } else if edit.key_mix { *kmix = Some(now.mix); keyed_fields.push_str("mix,"); }
            if let Some(v) = edit.softness { *softness = Some(v); changed_fields.push_str("softness,"); } else if edit.key_softness { *ksoft = Some(now.softness); keyed_fields.push_str("softness,"); }
            if let Some(v) = edit.bend { *bend_positive = Some(v); changed_fields.push_str("bend,"); } else if edit.key_bend { *kbend = Some(now.bend_positive); keyed_fields.push_str("bend,"); }
            if let Some(v) = edit.compress { *compress = Some(v); changed_fields.push_str("compress,"); } else if edit.key_compress { *kcomp = Some(now.compress); keyed_fields.push_str("compress,"); }
            if let Some(v) = edit.stretch { *stretch = Some(v); changed_fields.push_str("stretch,"); } else if edit.key_stretch { *kstretch = Some(now.stretch); keyed_fields.push_str("stretch,"); }
        }
        if !keyed_fields.is_empty() {
            app.edit_keyed(vec![keyed]);
        }
        if !changed_fields.is_empty() {
            app.edit_coalesced(format!("ik:{name}:key:{a}:{time}:{changed_fields}"), vec![changed]);
        }
        ui.add_space(6.0);
        let tip = tr("Key every value shown above at the playhead (a key per value, without moving the others)");
        if ui.button(tr("Key IK")).on_hover_text(tip).clicked() {
            key_ik_now(app, ik);
        }
    } else {
        let mut p = IkPatch { name: name.clone(), mix: edit.mix, softness: edit.softness, bend_positive: edit.bend, compress: edit.compress, stretch: edit.stretch, ..Default::default() };
        let field = [("mix", edit.mix.is_some()), ("softness", edit.softness.is_some()), ("bend", edit.bend.is_some()), ("compress", edit.compress.is_some()), ("stretch", edit.stretch.is_some())]
            .iter().find(|(_, on)| *on).map(|(f, _)| *f);
        if let Some(f) = field {
            p.name.clone_from(&name);
            app.edit_coalesced(format!("ik:{name}:{f}"), vec![Op::SetIk(p)]);
        }
    }
}

/// Keys every IK value that applies to the chain (as shown at the playhead) in the open animation.
pub(crate) fn key_ik_now(app: &mut EditorApp, ik: &IkConstraint) -> bool {
    let Some(a) = app.playback.anim.clone() else { return false };
    let time = app.playback.time;
    let now = crate::ikbend::ik_values(&app.shown_doc(), Some(&a), time, ik);
    let (one, two) = (ik.bones.len() == 1, ik.bones.len() == 2);
    app.edit_keyed(vec![Op::KeyIk {
        animation: a, ik: ik.name.clone(), time, mix: Some(now.mix), softness: two.then_some(now.softness),
        bend_positive: two.then_some(now.bend_positive), compress: one.then_some(now.compress), stretch: Some(now.stretch),
        curve: None, curves: None,
    }])
}

/// What the IK inspector asked for this frame.
#[derive(Default)]
struct IkEdit {
    mix: Option<f32>,
    softness: Option<f32>,
    bend: Option<bool>,
    compress: Option<bool>,
    stretch: Option<bool>,
    key_mix: bool,
    key_softness: bool,
    key_bend: bool,
    key_compress: bool,
    key_stretch: bool,
}

/// One IK value row: key button (animate mode, the way physics rows have it), label, widget.
#[allow(clippy::too_many_arguments)]
fn ik_row(ui: &mut egui::Ui, animating: bool, state: KeyState, label: &str, tip: &str, off_reason: Option<&str>, key: &mut bool, widget: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        if animating {
            let enabled = off_reason.is_none();
            let clicked = ui.add_enabled_ui(enabled, |ui| key_button(ui, state)).inner;
            if clicked { *key = true; }
        }
        let tip = off_reason.unwrap_or(tip);
        theme::prop(ui, tr(label), tr(tip), widget);
    });
}

/// A borderless lock/unlock icon button; accent while locked.
fn lock_icon_toggle(ui: &mut egui::Ui, locked: bool, tip: &str) -> bool {
    // Icon and word, so the state is not carried by the icon color alone.
    let label = tr(if locked { "Locked" } else { "Lock" });
    let (rect, r) = ui.allocate_exact_size(egui::vec2(78.0, theme::MIN_CLICK_H - 4.0), egui::Sense::click());
    let color = if r.hovered() { Color32::WHITE } else if locked { ACCENT } else { TEXT };
    let icon = if locked { crate::icons::Icon::Lock } else { crate::icons::Icon::Unlock };
    if r.hovered() {
        ui.painter().rect_filled(rect, 4.0, Color32::from_gray(64));
    }
    crate::icons::image(icon, 16.0).tint(color).paint_at(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 12.0, rect.center().y), egui::vec2(16.0, 16.0)));
    ui.painter().text(egui::pos2(rect.left() + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(theme::SIZE_SMALL), color);
    r.on_hover_text(tr(tip)).clicked()
}

/// The setup attachment `slot` shows with the previewed skins applied.
fn shown_attachment(app: &EditorApp, slot: &Slot) -> Option<Attachment> {
    let si = app.doc.slot_index(&slot.name)?;
    app.doc.resolve_attachment(si, slot.attachment.as_deref()?, &app.skins).cloned()
}

/// Adds a skin, previews it and shows it in the inspector.
fn new_skin(app: &mut EditorApp) {
    crate::dialogs::new_skin(app);
}

pub fn compare_skins(app: &mut EditorApp) {
    let names: Vec<_> = if app.skins.len() >= 2 { app.skins.iter().rev().take(2).rev().cloned().collect() }
        else { app.doc.skins.iter().take(2).map(|s| s.name.clone()).collect() };
    if names.len() < 2 { return; }
    let Some(a) = app.doc.skin(&names[0]) else { return };
    let Some(b) = app.doc.skin(&names[1]) else { return };
    let keys = |skin: &Skin| -> BTreeSet<(String, String)> {
        skin.attachments.iter().flat_map(|(slot, atts)| atts.iter().map(|att| (slot.clone(), att.name().to_string()))).collect()
    };
    let (ak, bk) = (keys(a), keys(b));
    let only_a: Vec<_> = ak.difference(&bk).map(|(s, a)| format!("{s}/{a}")).collect();
    let only_b: Vec<_> = bk.difference(&ak).map(|(s, a)| format!("{s}/{a}")).collect();
    app.status = crate::tf!("Skin comparison: {} lacks [{}]; {} lacks [{}]", b.name, only_a.join(", "), a.name, only_b.join(", "));
}

fn select_skin(app: &mut EditorApp, name: &str) {
    app.selection = crate::app::Selection { skin: Some(name.to_string()), ..Default::default() };
}

/// Toggles a skin's preview; a newly shown skin goes on top.
fn toggle_skin_preview(app: &mut EditorApp, name: &str) {
    if app.skins.iter().any(|k| k == name) {
        app.skins.retain(|k| k != name);
    } else {
        app.skins.push(name.to_string());
    }
}

/// A copy of `skin` under a new name, as one undo step.
fn duplicate_skin(app: &mut EditorApp, skin: &Skin) {
    let name = unique_name(&format!("{}_copy", skin.name), |n| app.doc.skin(n).is_some());
    let mut ops = vec![Op::AddSkin { name: name.clone() }, Op::SetSkin { name: name.clone(), rename: None, bones: Some(skin.bones.clone()), ik: None, constraints: Some(skin.constraints.clone()) }];
    for (slot, atts) in &skin.attachments {
        ops.extend(atts.iter().map(|a| Op::SetSkinAttachment { skin: name.clone(), slot: slot.clone(), attachment: a.clone() }));
    }
    if app.edit(ops) {
        select_skin(app, &name);
    }
}

/// Inspector for a skin: name, preview, what it fills, and the bones and
/// IK constraints that only work while it is on.
fn skin_properties(app: &mut EditorApp, ui: &mut egui::Ui, skin: &Skin) {
    let name = skin.name.clone();
    let count: usize = skin.attachments.values().map(Vec::len).sum();
    let context = crate::tf!("{} attachments in {} slots", count, skin.attachments.len());
    if let Some(new) = inspector_header(ui, crate::icons::Icon::Skin, ACCENT, tr("Skin"), &format!("skin_name_{name}"), &name, &context, false)
        && app.edit(vec![Op::SetSkin { name: name.clone(), rename: Some(new.clone()), bones: None, ik: None, constraints: None }])
    {
        for k in app.skins.iter_mut().filter(|k| **k == name) {
            *k = new.clone();
        }
        app.selection.skin = Some(new);
        return;
    }
    let mut shown = app.skins.contains(&name);
    theme::prop(ui, tr("Preview"), tr("Show this skin in the viewport (several can be on; later ones win)"), |ui| {
        if ui.checkbox(&mut shown, tr("Show in viewport")).changed() {
            toggle_skin_preview(app, &name);
        }
        if ui.button(tr("Only this")).on_hover_text(tr("Preview this skin alone")).clicked() {
            app.skins = vec![name.clone()];
        }
    });
    if ui.add_enabled(app.doc.skins.len() >= 2, egui::Button::new(tr("Compare skins"))).on_hover_text(tr("Compare the last two previewed skins, or the first two skins")).clicked() {
        compare_skins(app);
    }

    theme::group(ui, "skin_attachments", tr("Attachments"), &count.to_string(), true, |ui| {
        let mut placeholders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for slot in &app.doc.slots {
            for attachment in &slot.attachments { placeholders.entry(slot.name.clone()).or_default().insert(attachment.name().into()); }
        }
        for variant in &app.doc.skins {
            for (slot, atts) in &variant.attachments {
                for attachment in atts { placeholders.entry(slot.clone()).or_default().insert(attachment.name().into()); }
            }
        }
        if placeholders.is_empty() { theme::hint(ui, tr("No skin placeholders.")); }
        let mut tree = TreeCtx::default();
        let mut remove = None;
        for (slot, names) in placeholders {
            let present = skin.attachments.get(&slot).map_or(0, Vec::len);
            let row = tree_row(ui, &mut tree, RowSpec { key: format!("skin_slot:{name}:{slot}"), kind: NodeKind::Slot, name: &slot, color: theme::SLOT, meta: format!("{present}/{}", names.len()), ..Default::default() });
            if row.main.clicked() { app.selection.slot = Some(slot.clone()); app.selection.bone = app.doc.slot(&slot).map(|s| s.bone.clone()); }
            for placeholder in names {
                let found = skin.attachment(&slot, &placeholder);
                let missing = found.is_none();
                let row = tree_row(ui, &mut tree, RowSpec {
                    key: format!("skin_placeholder:{name}:{slot}:{placeholder}"), depth:1,
                    kind: NodeKind::Region, name:&placeholder,
                    color: if missing { TEXT_DIM.gamma_multiply(0.65) } else { TEXT },
                    meta: if missing { tr("Missing in this skin").into() } else { String::new() },
                    ..Default::default()
                });
                if row.main.clicked() { app.selection.slot = Some(slot.clone()); app.selection.attachment = Some(placeholder.clone()); }
                if let Some(a) = found {
                    let is_mesh = matches!(a, Attachment::Mesh(_) | Attachment::LinkedMesh(_));
                    let row = tree_row(ui, &mut tree, RowSpec {
                        key: format!("skin_att:{name}:{slot}:{placeholder}"), depth:2,
                        kind: if is_mesh { NodeKind::Mesh } else { NodeKind::Region }, name:a.name(), color:TEXT,
                        meta: if matches!(a, Attachment::LinkedMesh(_)) { tr("Linked").into() } else { String::new() },
                        selected: app.selection.slot.as_deref() == Some(&slot) && app.selection.attachment.as_deref() == Some(a.name()),
                        ..Default::default()
                    });
                    if row.main.clicked() {
                        app.selection.slot = Some(slot.clone()); app.selection.attachment = Some(a.name().into());
                        app.skins.retain(|k| k != &name); app.skins.push(name.clone());
                    }
                    row.main.context_menu(|ui| {
                        if ui.button(tr("Rename")).clicked() {
                            crate::dialogs::rename(app, crate::dialogs::Item::Attachment { slot: slot.clone(), name: a.name().into(), skin: Some(name.clone()) });
                            ui.close();
                        }
                        if ui.button(tr("Remove from this skin")).clicked() { remove = Some((slot.clone(), a.name().to_string())); ui.close(); }
                    });
                }
            }
        }
        if let Some((slot, att)) = remove {
            crate::dialogs::delete(app, crate::dialogs::Item::Attachment { slot, name: att, skin: Some(name.clone()) });
        }
    });

    theme::group(ui, "skin_bones", tr("Skin bones"), &skin.bones.len().to_string(), !skin.bones.is_empty(), |ui| {
        theme::hint(ui, tr("Bones marked \"Skin required\" only work while a skin that lists them is on (a cape bone only for the hero outfit)."));
        let mut bones = skin.bones.clone();
        let mut changed = false;
        bones.retain(|b| {
            let mut keep = true;
            ui.horizontal(|ui| {
                ui.add(crate::icons::image(crate::icons::Icon::Bone, 14.0).tint(app.doc.bone(b).map_or(BONE, theme::bone_text_color)));
                ui.label(b);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("×").frame(false)).on_hover_text(tr("Remove from this skin")).clicked() {
                        keep = false;
                    }
                });
            });
            changed |= !keep;
            keep
        });
        let sel = app.selection.bone.clone().filter(|b| !bones.contains(b));
        if ui.add_enabled(sel.is_some(), egui::Button::new(tr("Add selected bone"))).clicked()
            && let Some(b) = sel
        {
            bones.push(b);
            changed = true;
        }
        if changed {
            app.edit(vec![Op::SetSkin { name: name.clone(), rename: None, bones: Some(bones), ik: None, constraints: None }]);
        }
    });

    theme::group(ui, "skin_ik", tr("Skin constraints"), &skin.constraints.len().to_string(), !skin.constraints.is_empty(), |ui| {
        let mut ik = skin.constraints.clone();
        let mut changed = false;
        let all = app.doc.ik.iter().map(|c| (c.name.clone(), crate::icons::Icon::Constraint, IK))
            .chain(app.doc.physics.iter().map(|c| (c.name.clone(), crate::icons::Icon::Physics, PHYSICS)))
            .chain(app.doc.transforms.iter().map(|c| (c.name.clone(), crate::icons::Icon::Constraint, IK)));
        for (cname, icon, color) in all {
            let mut on = ik.contains(&cname);
            let response = ui.horizontal(|ui| {
                ui.add(crate::icons::image(icon, 14.0).tint(color));
                ui.checkbox(&mut on, RichText::new(&cname).color(color)).on_hover_text(tr("Only apply this constraint while this skin is on"))
            }).inner;
            if response.changed() {
                if on {
                    ik.push(cname.clone());
                } else {
                    ik.retain(|n| n != &cname);
                }
                changed = true;
            }
        }
        if app.doc.ik.is_empty() && app.doc.physics.is_empty() && app.doc.transforms.is_empty() {
            theme::hint(ui, tr("No constraints in the skeleton."));
        }
        if changed {
            app.edit(vec![Op::SetSkin { name: name.clone(), rename: None, bones: None, ik: None, constraints: Some(ik) }]);
        }
    });
}

/// In the slot inspector: which skins fill this slot's placeholders, and
/// copying the shown attachment into a skin to start a variant from it.
fn slot_skins_section(app: &mut EditorApp, ui: &mut egui::Ui, slot: &Slot) {
    if app.doc.skins.is_empty() {
        return;
    }
    let name = slot.name.clone();
    let filled: Vec<(String, Vec<String>)> = app
        .doc
        .skins
        .iter()
        .filter_map(|k| k.attachments.get(&name).map(|a| (k.name.clone(), a.iter().map(|x| x.name().to_string()).collect())))
        .collect();
    theme::group(ui, "slot_skins", tr("Skins"), &filled.len().to_string(), true, |ui| {
        for (skin, atts) in &filled {
            ui.horizontal(|ui| {
                let on = app.skins.contains(skin);
                if ui.selectable_label(on, skin).on_hover_text(tr("Toggle this skin's preview")).clicked() {
                    toggle_skin_preview(app, skin);
                }
                ui.label(RichText::new(atts.join(", ")).color(TEXT_DIM));
            });
        }
        let shown = shown_attachment(app, slot);
        let Some(att) = shown else {
            theme::hint(ui, tr("The slot shows no attachment to add to a skin."));
            return;
        };
        ui.horizontal(|ui| {
            ui.menu_button(tr("Add to skin"), |ui| {
                for k in app.doc.skins.iter().map(|k| k.name.clone()).collect::<Vec<_>>() {
                    if ui.button(&k).clicked() {
                        app.edit(vec![Op::SetSkinAttachment { skin: k.clone(), slot: name.clone(), attachment: att.clone() }]);
                        if !app.skins.contains(&k) {
                            app.skins.push(k);
                        }
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(crate::tf!("Copy \"{}\" into a skin under the same placeholder name; then change its image there", att.name()));
        });
        if let Some(k) = app.skin_of(&name, att.name()) {
            theme::hint(ui, &crate::tf!("Edits to \"{}\" go into skin \"{}\" while it is previewed.", att.name(), k));
        }
    });
}

/// Wrapping dim helper text (a label, not a hint strip).
fn note_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::SIZE_SMALL).color(TEXT_DIM)).wrap());
}

/// What the weight colors mean: a bar from 0% (blue) to 100% (red), with words.
fn weight_legend(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let bar = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 2.0), egui::pos2(rect.right(), rect.min.y + 12.0));
    let steps = 24;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        // Same ramp as the viewport heat map: blue, cyan, green, yellow, red.
        let c = crate::viewport::weight_color(t);
        let x0 = bar.left() + bar.width() * i as f32 / steps as f32;
        let x1 = bar.left() + bar.width() * (i + 1) as f32 / steps as f32;
        ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x0, bar.top()), egui::pos2(x1 + 0.5, bar.bottom())), 0.0, c);
    }
    let p = ui.painter();
    let font = egui::FontId::proportional(theme::SIZE_SMALL);
    p.text(egui::pos2(rect.left(), rect.bottom()), egui::Align2::LEFT_BOTTOM, tr("0% (not moved by this bone)"), font.clone(), TEXT_DIM);
    p.text(egui::pos2(rect.right(), rect.bottom()), egui::Align2::RIGHT_BOTTOM, tr("100%"), font, TEXT_DIM);
    ui.add_space(2.0);
}

#[cfg(test)]
mod row_indent_tests {
    use super::*;

    #[test]
    fn deep_rows_stay_ordered_and_leave_room() {
        for width in [200.0, 300.0, 600.0] {
            let mut last = 0.0;
            for depth in 1..80 {
                let x = row_indent(depth, width);
                assert!(x > last, "{width} {depth}");
                assert!(x <= width * 0.4);
                last = x;
            }
        }
        assert!(row_indent(1, 300.0) > 12.0, "the top of the tree keeps a readable step");
    }
}
