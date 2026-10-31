//! Which way a two-bone IK chain folds, without thinking in +/-: new chains
//! keep the drawn bend, the knee (or elbow) has a handle that flips the bend
//! when clicked or dragged across the chain line, and `I` flips the IK of
//! the selection.

use eframe::egui::{self, Color32, PointerButton, Pos2, Rect, Stroke};
use skel2d_core::ops::IkPatch;
use skel2d_core::{Document, IkConstraint, IkPose, Op, Pose};

use crate::app::EditorApp;
use crate::i18n::tr;
use crate::theme;

/// Screen distance from the knee to its handle, and the handle's radius.
const OFFSET: f32 = 22.0;
const RADIUS: f32 = 8.0;

/// A two-bone chain in world space: where it starts, the middle joint and
/// the target it reaches for.
pub struct Chain {
    pub root: [f32; 2],
    pub knee: [f32; 2],
    pub target: [f32; 2],
}

pub fn chain(doc: &Document, pose: &Pose, ik: &IkConstraint) -> Option<Chain> {
    let [parent, child] = ik.bones.as_slice() else { return None };
    let at = |name: &str| doc.bone_index(name).map(|i| pose.bones[i].world.apply(0.0, 0.0));
    Some(Chain { root: at(parent)?, knee: at(child)?, target: at(&ik.target)? })
}

/// The bend that puts the middle joint on `point`'s side of the chain line.
fn bend_towards(c: &Chain, point: [f32; 2]) -> bool {
    let g = [c.target[0] - c.root[0], c.target[1] - c.root[1]];
    let m = [point[0] - c.root[0], point[1] - c.root[1]];
    g[0] * m[1] - g[1] * m[0] < 0.0
}

/// Bend for a new IK on `bones` (parent, child) in the current pose: the
/// drawn bend, or outward from the body when the limb is straight.
pub fn initial_bend(doc: &Document, pose: &Pose, bones: &[String]) -> bool {
    let [parent, child] = bones else { return true };
    let (Some(p), Some(c)) = (doc.bone_index(parent), doc.bone_index(child)) else { return true };
    let root = pose.bones[p].world.apply(0.0, 0.0);
    let knee = pose.bones[c].world.apply(0.0, 0.0);
    let tip = pose.bones[c].world.apply(doc.bones[c].length, 0.0);
    // The body's middle: where the limb's parent (hip, chest) starts.
    let center = doc.bones[p].parent.as_deref().and_then(|g| doc.bone_index(g)).map_or(root, |g| pose.bones[g].world.apply(0.0, 0.0));
    skel2d_rig::template::bend_for(root, knee, tip, center)
}

/// The IK values in effect: the setup ones, or with an animation selected
/// those it gives at `time` (what the pose shows).
pub fn ik_values(doc: &Document, anim: Option<&str>, time: f32, ik: &IkConstraint) -> IkPose {
    let setup = ik.pose();
    anim.and_then(|a| doc.animations.get(a))
        .and_then(|a| a.ik.get(&ik.name))
        .and_then(|keys| skel2d_core::pose::sample_ik(keys, time, &setup))
        .unwrap_or(setup)
}

/// Sets the bend: a key at the playhead in animate mode, else the setup.
fn bend_op(anim: Option<&str>, time: f32, ik: &IkConstraint, bend: bool) -> Op {
    match anim {
        Some(a) => Op::KeyIk {
            animation: a.to_string(), ik: ik.name.clone(), time, mix: None, softness: None,
            bend_positive: Some(bend), compress: None, stretch: None, curve: None, curves: None,
        },
        None => Op::SetIk(IkPatch { name: ik.name.clone(), bend_positive: Some(bend), ..Default::default() }),
    }
}

fn flip(app: &EditorApp, ik: &IkConstraint) -> Op {
    let anim = app.playback.anim.as_deref();
    let now = ik_values(&app.shown_doc(), anim, app.playback.time, ik).bend_positive;
    bend_op(anim, app.playback.time, ik, !now)
}

/// IK constraints whose knee handle is shown: those of the selection.
fn active(app: &EditorApp) -> impl Iterator<Item = &IkConstraint> {
    let sel = &app.selection;
    app.doc.ik.iter().filter(move |k| {
        k.bones.len() == 2
            && (sel.ik.as_deref() == Some(k.name.as_str())
                || sel.bone.as_ref().is_some_and(|b| k.bones.contains(b) || k.target == *b))
    })
}

/// Screen position of a chain's knee handle: just off the knee, on the side
/// it bends towards.
fn handle_pos(app: &EditorApp, rect: Rect, c: &Chain) -> Pos2 {
    let (root, target, knee) = (app.camera.to_screen(rect, c.root), app.camera.to_screen(rect, c.target), app.camera.to_screen(rect, c.knee));
    let line = target - root;
    let mut normal = egui::vec2(-line.y, line.x).normalized();
    if normal.dot(knee - root) < 0.0 {
        normal = -normal;
    }
    if !normal.is_finite() {
        normal = egui::vec2(OFFSET, 0.0).normalized();
    }
    knee + normal * OFFSET
}

fn handle_at(app: &EditorApp, rect: Rect, pose: &Pose, p: Pos2) -> Option<String> {
    active(app)
        .filter_map(|k| chain(&app.doc, pose, k).map(|c| (k, handle_pos(app, rect, &c))))
        .find(|(_, h)| h.distance(p) <= RADIUS + 3.0)
        .map(|(k, _)| k.name.clone())
}

pub fn draw(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose, hover: Option<Pos2>) {
    for k in active(app) {
        let Some(c) = chain(&app.doc, pose, k) else { continue };
        let (knee, h) = (app.camera.to_screen(rect, c.knee), handle_pos(app, rect, &c));
        let hot = hover.is_some_and(|p| h.distance(p) <= RADIUS + 3.0) || app.view.knee_drag.as_ref().is_some_and(|d| d.ik == k.name);
        let color = if hot { Color32::WHITE } else { theme::IK };
        painter.line_segment([knee, h], Stroke::new(1.5, theme::IK.gamma_multiply(0.7)));
        painter.circle(h, RADIUS, Color32::from_black_alpha(200), Stroke::new(2.0, color));
        // Two small arrows: "drag me across".
        let a = (h - knee).normalized().rot90() * 4.0;
        for s in [-1.0, 1.0] {
            let tip = h + a * s;
            painter.line_segment([h + a * s * 0.2, tip], Stroke::new(1.5, color));
            painter.circle_filled(tip, 1.6, color);
        }
    }
}

/// Clicks and drags on knee handles. Returns true when it took the pointer.
pub fn interact(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose) -> bool {
    if let Some(d) = app.view.knee_drag.as_ref() {
        if response.drag_stopped() {
            app.view.knee_drag = None;
            return true;
        }
        if let Some(p) = response.interact_pointer_pos()
            && let Some(ik) = d.base.ik.iter().find(|k| k.name == d.ik)
            && let Some(c) = chain(&d.base, pose, ik)
        {
            let want = bend_towards(&c, app.camera.to_world(rect, p));
            let base = d.base.clone();
            let op = bend_op(app.playback.anim.as_deref(), app.playback.time, ik, want);
            app.preview(&base, &[op]);
            app.hint = tr("Release on the side the joint should bend to").to_string();
        }
        return true;
    }
    let press = ui.input(|i| i.pointer.press_origin());
    let hover = response.hover_pos();
    if let Some(name) = hover.and_then(|p| handle_at(app, rect, pose, p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        app.hint = tr("Joint bend: drag across the limb or click to flip [I]").to_string();
        if response.clicked_by(PointerButton::Primary)
            && let Some(ik) = app.doc.ik.iter().find(|k| k.name == name)
        {
            let op = flip(app, ik);
            app.edit(vec![op]);
            return true;
        }
    }
    if response.drag_started_by(PointerButton::Primary)
        && let Some(name) = press.and_then(|p| handle_at(app, rect, pose, p))
    {
        app.push_undo();
        app.view.knee_drag = Some(KneeDrag { ik: name, base: app.doc.clone() });
        return true;
    }
    false
}

/// `I`: flips the IK of the selection (or the selected IK).
pub fn flip_key(app: &mut EditorApp, ctx: &egui::Context, typing: bool) {
    if typing || !ctx.input(|i| i.key_pressed(egui::Key::I) && !i.modifiers.any()) {
        return;
    }
    let ops: Vec<Op> = active(app).map(|k| flip(app, k)).collect();
    if ops.is_empty() {
        app.status = tr("Select an IK chain (or one of its bones) to flip its bend").to_string();
    } else {
        app.edit(ops);
    }
}

/// A knee handle being dragged: the IK and the document when it started.
pub struct KneeDrag {
    pub ik: String,
    pub base: Document,
}
