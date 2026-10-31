//! Central canvas: draws the posed skeleton, picks bones and slots, and turns
//! pointer drags, gizmos and key nudges into `set_bone` (setup pose) or
//! `key_bone` (animation) ops.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, Align2, Color32, FontId, Key, Mesh, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use skel2d_core::ops::{BonePatch, KeyBone, VertexPatch};
use skel2d_core::pose::sample;
use skel2d_core::{Affine, Attachment, Document, Mesh as CoreMesh, Op, Pose, Region};

use crate::app::{EditorApp, Tool};
use crate::meshedit::{self, BrushMode, UvMap};
use crate::{import, theme};

const ARM: f32 = 64.0;
const RING: f32 = 56.0;
const AXIS_X: Color32 = Color32::from_rgb(235, 90, 90);
const AXIS_Y: Color32 = Color32::from_rgb(110, 214, 110);

pub struct Camera {
    pub center: [f32; 2],
    /// Pixels per world unit.
    pub zoom: f32,
    pub needs_fit: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self { center: [0.0, 0.0], zoom: 2.0, needs_fit: true }
    }
}

type Bounds = ([f32; 2], [f32; 2]);

impl Camera {
    pub(crate) fn to_screen(&self, rect: Rect, [x, y]: [f32; 2]) -> Pos2 {
        rect.center() + vec2((x - self.center[0]) * self.zoom, -(y - self.center[1]) * self.zoom)
    }

    pub(crate) fn to_world(&self, rect: Rect, p: Pos2) -> [f32; 2] {
        let d = p - rect.center();
        [self.center[0] + d.x / self.zoom, self.center[1] - d.y / self.zoom]
    }

    fn fit(&mut self, rect: Rect, doc: &Document, pose: &Pose) {
        match bounds(doc, pose, None) {
            Some(b) => self.frame(rect, b, 20.0),
            None => *self = Camera { needs_fit: false, ..Camera::default() },
        }
    }

    fn frame(&mut self, rect: Rect, (min, max): Bounds, min_size: f32) {
        let size = [(max[0] - min[0]).max(min_size), (max[1] - min[1]).max(min_size)];
        self.center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        self.zoom = ((rect.width() / size[0]).min(rect.height() / size[1]) * 0.8).clamp(0.01, 100.0);
        self.needs_fit = false;
    }
}

/// World bounds of the skeleton and its slot shapes, optionally limited to some bones.
fn bounds(doc: &Document, pose: &Pose, only: Option<&BTreeSet<String>>) -> Option<Bounds> {
    let (mut min, mut max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut add = |[x, y]: [f32; 2]| {
        min = [min[0].min(x), min[1].min(y)];
        max = [max[0].max(x), max[1].max(y)];
    };
    let ok = |name: &str| only.is_none_or(|s| s.contains(name));
    for (bone, bp) in doc.bones.iter().zip(&pose.bones) {
        if ok(&bone.name) {
            add(bp.world.apply(0.0, 0.0));
            add(bp.world.apply(bone.length, 0.0));
        }
    }
    for shape in slot_shapes(doc, pose) {
        if ok(&doc.slots[shape.slot].bone) {
            shape.pts.into_iter().for_each(&mut add);
        }
    }
    (min[0] <= max[0] && min[1] <= max[1]).then_some((min, max))
}

/// The bone and all its descendants.
fn subtree(doc: &Document, name: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::from([name.to_string()]);
    for b in &doc.bones {
        if b.parent.as_ref().is_some_and(|p| set.contains(p)) {
            set.insert(b.name.clone());
        }
    }
    set
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Free,
    X,
    Y,
}

/// A grabbable part of the gizmo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Ring,
    Move(Axis),
    Scale(Axis),
}

impl Handle {
    fn axis(self) -> Axis {
        match self {
            Handle::Ring => Axis::Free,
            Handle::Move(a) | Handle::Scale(a) => a,
        }
    }
}

/// An in-progress manipulation of one bone. Ops are recomputed from `base`
/// every frame, so the drag never accumulates rounding errors.
pub struct Drag {
    base: Document,
    bone: String,
    tool: Tool,
    start: [f32; 2],
    /// Pointer position with the Shift slowdown applied, and the raw one it follows.
    eff: [f32; 2],
    last: [f32; 2],
    axis: Axis,
    handle: Option<Handle>,
    /// World directions of the bone's local x and y axes (unit length).
    axes: [[f32; 2]; 2],
    /// World position of the bone origin when the drag started.
    anchor: [f32; 2],
    /// Inverse of the parent's world transform, mapping world deltas to local.
    parent_inv: Option<Affine>,
    mirrored: bool,
    rotate: f32,
    translate: [f32; 2],
    scale: [f32; 2],
    shear: [f32; 2],
    shearing: bool,
    /// Set when the drag moves the selected slot's image instead of a bone
    /// (`bone` then names the slot).
    att: Option<AttachmentDrag>,
}

/// The selected slot's visible attachment as the transform tools see it in
/// the setup pose: they move the image itself, not its bone.
struct AttachmentDrag {
    slot: String,
    attachment: Attachment,
    /// World transform of the slot's bone; regions are placed relative to it.
    bone_world: Affine,
    /// World frame of the gizmo: the region's own transform, or the centroid
    /// of the moved mesh vertices with the bone's rotation.
    frame: Affine,
    /// Picked mesh vertices (sorted) that move alone; empty moves it all.
    vertices: Vec<usize>,
    /// In an animation: the picked vertices' world positions at the start
    /// and how to key them.
    deform: Option<(Vec<[f32; 2]>, DeformDrag)>,
}

/// World-space geometry of a visible region or mesh attachment, in draw order.
struct SlotShape {
    slot: usize,
    mesh: bool,
    pts: Vec<[f32; 2]>,
    uvs: Vec<[f32; 2]>,
    tris: Vec<[usize; 3]>,
}

fn slot_shapes(doc: &Document, pose: &Pose) -> Vec<SlotShape> {
    let mut out = Vec::new();
    // Back to front as the pose orders the slots (an animation can key the order).
    let order: Vec<usize> = if pose.draw_order.len() == doc.slots.len() { pose.draw_order.clone() } else { (0..doc.slots.len()).collect() };
    for i in order {
        let slot = &doc.slots[i];
        let Some(att) = pose.attachment(doc, i) else { continue };
        let Some(materialized) = doc.materialize_attachment(i, att) else { continue };
        let att = &materialized;
        let Some(bi) = doc.bone_index(&slot.bone) else { continue };
        out.push(match att {
            Attachment::Region(r) => {
                let m = pose.bones[bi].world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));
                let (hw, hh) = (r.width / 2.0, r.height / 2.0);
                // Top-left, top-right, bottom-right, bottom-left (matching image UVs).
                SlotShape {
                    slot: i,
                    mesh: false,
                    pts: vec![m.apply(-hw, hh), m.apply(hw, hh), m.apply(hw, -hh), m.apply(-hw, -hh)],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    tris: vec![[0, 1, 2], [0, 2, 3]],
                }
            }
            Attachment::Mesh(m) => {
                let pts = pose.deform_mesh(doc, m, i);
                let tris = m.triangles.iter().map(|t| t.map(|v| v as usize)).filter(|t| t.iter().all(|&v| v < pts.len())).collect();
                SlotShape { slot: i, mesh: true, uvs: m.vertices.iter().map(|v| [v.u, v.v]).collect(), pts, tris }
            }
            Attachment::LinkedMesh(_) => unreachable!(),
        });
    }
    out
}

/// Edges used by exactly one triangle: the silhouette.
fn boundary_edges(tris: &[[usize; 3]]) -> Vec<[usize; 2]> {
    let mut count: HashMap<(usize, usize), u32> = HashMap::new();
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    count.into_iter().filter(|(_, n)| *n == 1).map(|((a, b), _)| [a, b]).collect()
}

/// Every triangle edge once, for drawing a wireframe.
fn unique_edges(tris: &[[usize; 3]]) -> BTreeSet<[usize; 2]> {
    tris.iter().flat_map(|t| (0..3).map(|k| { let (a, b) = (t[k], t[(k + 1) % 3]); [a.min(b), a.max(b)] })).collect()
}

/// A closed outline as separate segments. egui's closed paths use unlimited
/// miter joins, which shoot far past sharp corners and collapsed triangles.
fn outline_shapes(pts: &[Pos2], stroke: Stroke) -> Vec<Shape> {
    (0..pts.len()).map(|i| Shape::line_segment([pts[i], pts[(i + 1) % pts.len()]], stroke)).collect()
}

/// A filled convex polygon with an outline. The fill is a plain mesh because
/// `convex_polygon` feathers along the same unlimited miter normals.
fn polygon_shapes(pts: &[Pos2], fill: Color32, stroke: Stroke) -> Vec<Shape> {
    let mut mesh = Mesh::default();
    for &p in pts {
        mesh.colored_vertex(p, fill);
    }
    for i in 2..pts.len() as u32 {
        mesh.add_triangle(0, i - 1, i);
    }
    let mut shapes = vec![Shape::mesh(mesh)];
    shapes.extend(outline_shapes(pts, stroke));
    shapes
}

/// The mesh attachment currently shown by the selected slot. A linked mesh
/// counts only in an animation: it can be deformed, but its geometry and
/// weights belong to the parent.
fn selected_mesh<'a>(app: &'a EditorApp, pose: &Pose) -> Option<(usize, Cow<'a, CoreMesh>)> {
    let si = app.doc.slot_index(app.selection.slot.as_deref()?)?;
    match pose.attachment(&app.doc, si)? {
        Attachment::Mesh(m) => Some((si, Cow::Borrowed(m))),
        link @ Attachment::LinkedMesh(_) if app.playback.anim.is_some() => match app.doc.materialize_attachment(si, link)? {
            Attachment::Mesh(m) => Some((si, Cow::Owned(m))),
            _ => None,
        },
        _ => None,
    }
}

/// Screen positions of the selected mesh's vertices.
fn vertex_dots(app: &EditorApp, pose: &Pose, rect: Rect) -> Vec<Pos2> {
    let Some((si, m)) = selected_mesh(app, pose) else { return Vec::new() };
    pose.deform_mesh(&app.doc, &m, si).into_iter().map(|p| app.camera.to_screen(rect, p)).collect()
}

fn vertex_at(app: &EditorApp, pose: &Pose, rect: Rect, p: Pos2) -> Option<usize> {
    vertex_dots(app, pose, rect)
        .into_iter()
        .enumerate()
        .map(|(i, d)| (d.distance(p), i))
        .filter(|(d, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// The selected mesh's picked vertices, sorted (empty when none are).
fn picked_vertices(app: &EditorApp, si: usize, m: &CoreMesh) -> Vec<usize> {
    let sel = &app.view.vertex_sel;
    if sel.slot != app.doc.slots[si].name || sel.attachment != m.name {
        return Vec::new();
    }
    sel.set.iter().copied().filter(|&i| i < m.vertices.len()).collect()
}

fn pick_vertices(app: &mut EditorApp, si: usize, attachment: &str, set: BTreeSet<usize>) {
    app.view.vertex_sel = VertexSel { slot: app.doc.slots[si].name.clone(), attachment: attachment.to_string(), set };
}

/// Vertex picking on a click or drag start: Shift toggles `index` (a drag
/// only adds), otherwise a vertex outside the selection becomes the only one.
fn pick_vertex(app: &mut EditorApp, si: usize, m: &CoreMesh, index: usize, shift: bool, toggle: bool) {
    let mut set: BTreeSet<usize> = picked_vertices(app, si, m).into_iter().collect();
    if shift {
        if !set.insert(index) && toggle {
            set.remove(&index);
        }
    } else if !set.contains(&index) {
        set = BTreeSet::from([index]);
    }
    pick_vertices(app, si, &m.name, set);
}

/// An in-progress move of mesh vertices: in the setup pose it moves them,
/// in an animation it keys the mesh's deformation.
pub struct VertexDrag {
    base: Document,
    slot: String,
    attachment: String,
    /// Moved vertices and their world positions when the drag started.
    moving: Vec<(usize, [f32; 2])>,
    /// Pointer world position when the drag started.
    start: [f32; 2],
    /// When set, UV follows the vertices so the image stays in place.
    uv: Option<UvMap>,
    deform: Option<DeformDrag>,
}

/// Vertex edits in animate mode, keyed as deform offsets.
struct DeformDrag {
    skin: Option<String>,
    animation: String,
    time: f32,
    /// The animation's offsets at the start (zeros when unkeyed).
    offsets: Vec<[f32; 2]>,
    /// Per moved vertex: its index, setup-pose position, and the inverse of
    /// its skinning (world to setup-pose space).
    /// Index, setup position, bind shift and inverse skin of each moved vertex.
    verts: Vec<(usize, [f32; 2], [f32; 2], Affine)>,
}

impl DeformDrag {
    fn start(app: &EditorApp, pose: &Pose, si: usize, m: &CoreMesh, indices: &[usize]) -> Option<DeformDrag> {
        let (animation, time) = (app.playback.anim.clone()?, app.playback.time);
        let slot = &app.doc.slots[si];
        let n = m.vertices.len();
        let (owner, source) = app.doc.resolve_attachment_from(si, &m.name, &app.skins)?;
        let skin = owner.map(str::to_string);
        // Shared linked timelines live on the parent's path.
        let (key_skin, key_name) = match source {
            Attachment::LinkedMesh(link) if link.timelines => (link.parent_skin.as_deref(), link.parent.as_str()),
            _ => (owner, m.name.as_str()),
        };
        // Start from what is shown, an unkeyed deformation included.
        let shown = app.shown_doc();
        let offsets = shown
            .animations
            .get(&animation)
            .and_then(|a| a.deform.get(key_skin.unwrap_or("default")))
            .and_then(|slots| slots.get(&slot.name))
            .and_then(|atts| atts.get(key_name))
            .and_then(|keys| sample(keys, time))
            .filter(|o| o.len() == n)
            .unwrap_or_else(|| vec![[0.0, 0.0]; n]);
        let bone = app.doc.bone_index(&slot.bone)?;
        let verts = indices
            .iter()
            .map(|&i| {
                let v = m.vertices.get(i)?;
                Some((i, [v.x, v.y], pose.bind_shift(&app.doc, v), pose.vertex_skin(&app.doc, v, bone).inverse()?))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(DeformDrag { skin, animation, time, offsets, verts })
    }

    /// The key that puts each moved vertex at its world position in `targets`
    /// (in the order of `verts`).
    fn op(&self, slot: &str, attachment: &str, targets: &[[f32; 2]]) -> Op {
        let mut offsets = self.offsets.clone();
        for ((i, setup, shift, unskin), at) in self.verts.iter().zip(targets) {
            let [x, y] = unskin.apply(at[0] - shift[0], at[1] - shift[1]);
            offsets[*i] = [round2(x - setup[0]), round2(y - setup[1])];
        }
        Op::KeyDeform { skin: self.skin.clone(), animation: self.animation.clone(), slot: slot.to_string(), attachment: attachment.to_string(), time: self.time, offsets, curve: None }
    }
}

fn point_in_triangle(p: Pos2, [a, b, c]: [Pos2; 3]) -> bool {
    let cross = |a: Pos2, b: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    let (d1, d2, d3) = (cross(a, b), cross(b, c), cross(c, a));
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}

pub fn weight_color(w: f32) -> Color32 {
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        let c = |i: usize| (a[i] + (b[i] - a[i]) * t) as u8;
        Color32::from_rgb(c(0), c(1), c(2))
    };
    let w = w.clamp(0.0, 1.0);
    if w < 0.5 {
        lerp([30.0, 50.0, 190.0], [230.0, 50.0, 40.0], w * 2.0)
    } else {
        lerp([230.0, 50.0, 40.0], [255.0, 240.0, 70.0], (w - 0.5) * 2.0)
    }
}

/// Influence of `bone` on a vertex, with weights normalized.
pub fn bone_weight(v: &skel2d_core::MeshVertex, bone: &str, slot_bone: &str) -> f32 {
    let total: f32 = v.weights.iter().map(|w| w.weight.max(0.0)).sum();
    if v.weights.is_empty() || total <= 0.0 {
        return (bone == slot_bone) as u8 as f32;
    }
    v.weights.iter().filter(|w| w.bone == bone).map(|w| w.weight.max(0.0)).sum::<f32>() / total
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Bone(String),
    Slot(String, String),
}

impl Pick {
    fn name(&self) -> &str {
        match self {
            Pick::Bone(n) | Pick::Slot(n, _) => n,
        }
    }
}

/// Everything under the cursor, nearest bone first, then slots front to back.
fn pick_all(app: &EditorApp, pose: &Pose, rect: Rect, p: Pos2) -> Vec<Pick> {
    let cam = &app.camera;
    let mut out = Vec::new();
    if app.show_bones {
        let mut hits: Vec<(f32, usize)> = Vec::new();
        for (i, (bone, bp)) in app.doc.bones.iter().zip(&pose.bones).enumerate() {
            if !app.bone_pickable(&bone.name) {
                continue;
            }
            let o = cam.to_screen(rect, bp.world.apply(0.0, 0.0));
            let t = cam.to_screen(rect, bp.world.apply(bone.length, 0.0));
            let d = distance_to_segment(p, o, t);
            if d < 7.0 {
                hits.push((d, i));
            }
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(hits.into_iter().map(|(_, i)| Pick::Bone(app.doc.bones[i].name.clone())));
    }
    for s in slot_shapes(&app.doc, pose).iter().rev() {
        let slot = &app.doc.slots[s.slot];
        if !app.slot_pickable(&slot.name) {
            continue;
        }
        let pts: Vec<Pos2> = s.pts.iter().map(|&c| cam.to_screen(rect, c)).collect();
        if s.tris.iter().any(|t| point_in_triangle(p, t.map(|i| pts[i]))) {
            out.push(Pick::Slot(slot.name.clone(), slot.bone.clone()));
        }
    }
    out
}

fn pick(app: &EditorApp, pose: &Pose, rect: Rect, p: Pos2) -> Option<Pick> {
    pick_all(app, pose, rect, p).into_iter().next()
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// Screen-space frame of the selected bone's gizmo.
struct Gizmo {
    o: Pos2,
    /// Screen directions (unit) of the bone's local x and y axes.
    ex: Vec2,
    ey: Vec2,
}

fn gizmo(app: &EditorApp, pose: &Pose, rect: Rect) -> Option<Gizmo> {
    if !app.tool.transforms() {
        return None;
    }
    let w = match attachment_target(app, pose) {
        Some(a) => a.frame,
        None => {
            let name = app.selection.bone.as_deref()?;
            if !app.bone_pickable(name) {
                return None;
            }
            pose.bones[app.doc.bone_index(name)?].world
        }
    };
    let dir = |x: f32, y: f32| {
        let [a, b] = w.apply_vector(x, y);
        vec2(a, -b).normalized()
    };
    let fix = |v: Vec2, d: Vec2| if v.is_finite() { v } else { d };
    Some(Gizmo { o: app.camera.to_screen(rect, w.apply(0.0, 0.0)), ex: fix(dir(1.0, 0.0), vec2(1.0, 0.0)), ey: fix(dir(0.0, 1.0), vec2(0.0, -1.0)) })
}

fn gizmo_hit(g: &Gizmo, tool: Tool, p: Pos2) -> Option<Handle> {
    let center = |h: Handle| (p - g.o).abs().max_elem() < 7.0 && matches!(h, Handle::Move(_) | Handle::Scale(_));
    let arm = |dir: Vec2| distance_to_segment(p, g.o, g.o + dir * ARM) < 6.0;
    match tool {
        Tool::Select | Tool::Bone | Tool::Mesh | Tool::Weights => None,
        Tool::Rotate => ((p.distance(g.o) - RING).abs() < 6.0).then_some(Handle::Ring),
        Tool::Translate => {
            if center(Handle::Move(Axis::Free)) {
                Some(Handle::Move(Axis::Free))
            } else if arm(vec2(1.0, 0.0)) {
                Some(Handle::Move(Axis::X))
            } else if arm(vec2(0.0, -1.0)) {
                Some(Handle::Move(Axis::Y))
            } else {
                None
            }
        }
        Tool::Scale => {
            if center(Handle::Scale(Axis::Free)) {
                Some(Handle::Scale(Axis::Free))
            } else if arm(g.ex) {
                Some(Handle::Scale(Axis::X))
            } else if arm(g.ey) {
                Some(Handle::Scale(Axis::Y))
            } else {
                None
            }
        }
    }
}

fn handle_hint(h: Handle) -> &'static str {
    match h {
        Handle::Ring => crate::i18n::tr("Drag to rotate"),
        Handle::Move(Axis::Free) => crate::i18n::tr("Drag to move freely"),
        Handle::Move(Axis::X) => crate::i18n::tr("Drag to move along world X"),
        Handle::Move(Axis::Y) => crate::i18n::tr("Drag to move along world Y"),
        Handle::Scale(Axis::Free) => crate::i18n::tr("Drag to scale uniformly"),
        Handle::Scale(Axis::X) => crate::i18n::tr("Drag to scale along local X"),
        Handle::Scale(Axis::Y) => crate::i18n::tr("Drag to scale along local Y"),
    }
}

/// Viewport-only interaction state (gizmo hover, pick cycling, ...).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MeshMode { #[default] Modify, Create, Delete, Edges }

impl MeshMode {
    pub fn next(self) -> Self {
        match self { Self::Modify=>Self::Create, Self::Create=>Self::Delete, Self::Delete=>Self::Edges, Self::Edges=>Self::Modify }
    }
}

struct MeshEdgeDrag { slot: String, attachment: String, skin: Option<String>, from: usize }

#[derive(Clone, Copy)]
pub enum MeshAction { Generate(usize), RemovePicked, RemoveEdge, ClearEdges, Reset(skel2d_core::ops::MeshResetMode) }

#[derive(Default)]
pub struct ViewState {
    pub pose_test: bool,
    pub pose_angle: f32,
    pub pose_rotation: Option<(String, f32)>,
    pub mesh_mode: MeshMode,
    edge_drag: Option<MeshEdgeDrag>,
    /// Explicit constraint selected in the current mesh (slot, attachment, skin, endpoints).
    edge_selection: Option<(String, String, Option<String>, [u32;2])>,
    hover: Option<Pick>,
    hover_handle: Option<Handle>,
    /// Live value of the active drag and where to show it.
    live: Option<(Pos2, String)>,
    /// Everything under the cursor at the last Alt+click, and which one is selected.
    cycle: Vec<Pick>,
    cycle_idx: usize,
    /// World position of the last right-click, for the context menu.
    menu_at: [f32; 2],
    /// Bone being drawn with the Bone tool.
    bone_draw: Option<BoneDraw>,
    /// Active weight paint stroke.
    paint: Option<Paint>,
    /// Where the Mesh tool would add a vertex (world), for the preview dot.
    mesh_insert: Option<[f32; 2]>,
    /// IK knee handle being dragged.
    pub(crate) knee_drag: Option<crate::ikbend::KneeDrag>,
    /// Selected vertices of the selected mesh.
    vertex_sel: VertexSel,
    /// Vertex box selection in progress (screen corners).
    vertex_box: Option<[Pos2; 2]>,
    /// Bone or slot under the pointer in the tree, highlighted in the viewport.
    pub tree_hover: Option<Pick>,
    /// Bones whose weights the weight tools leave alone.
    pub weight_locked: BTreeSet<String>,
    /// Weights copied with Ctrl+C in the Weights tool.
    pub weight_clipboard: Option<std::collections::BTreeMap<String, f32>>,
}

impl ViewState {
    pub(crate) fn clear_mesh_selection(&mut self) {
        self.vertex_sel.set.clear(); self.edge_selection=None; self.edge_drag=None;
    }
}

/// The selected mesh's slot index and its picked vertices (possibly none).
pub fn picked(app: &EditorApp, pose: &Pose) -> Option<(usize, Vec<usize>)> {
    let (si, m) = selected_mesh(app, pose)?;
    Some((si, picked_vertices(app, si, &m)))
}

/// UI and palette actions emit ordinary core ops, including the owning skin.
pub fn selected_mesh_op(app: &EditorApp, action: MeshAction) -> Option<Op> {
    if app.playback.anim.is_some() { return None; }
    let pose = app.pose();
    let (si,m) = selected_mesh(app,&pose)?;
    let (slot,attachment) = (app.doc.slots[si].name.clone(),m.name.clone());
    let skin = app.skin_of(&slot,&attachment);
    Some(match action {
        MeshAction::Generate(count) => Op::GenerateMeshVertices {slot,attachment,skin,count:Some(count),spacing:None},
        MeshAction::RemovePicked => {
            let indices = picked_vertices(app,si,&m);
            if indices.is_empty() { return None; }
            Op::RemoveMeshVertices {slot,attachment,skin,indices}
        }
        MeshAction::RemoveEdge => {
            let (s,a,k,e)=app.view.edge_selection.as_ref()?;
            if s!=&slot || a!=&attachment || k!=&skin || !m.edges.contains(e) { return None; }
            let edges=m.edges.iter().copied().filter(|v|v!=e).collect();
            Op::SetMeshEdges {slot,attachment,skin,edges}
        }
        MeshAction::ClearEdges => Op::SetMeshEdges {slot,attachment,skin,edges:Vec::new()},
        MeshAction::Reset(mode) => Op::ResetMesh {slot,attachment,skin,mode},
    })
}

pub fn apply_mesh_action(app: &mut EditorApp, action: MeshAction) {
    if let Some(op)=selected_mesh_op(app,action) && app.edit(vec![op]) {
        app.view.vertex_sel.set.clear(); app.view.edge_selection=None;
        app.status=crate::i18n::tr("Mesh updated · Ctrl+Z to undo").into();
    }
}

/// A stable, distinct color per bone for the weights overlay and bone list.
pub fn bone_color(name: &str) -> Color32 {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    // Golden-ratio hue steps spread similar names apart.
    let hue = (h as f32 / u32::MAX as f32 * 7.0 * 0.618_034).fract();
    egui::ecolor::Hsva::new(hue, 0.75, 0.95, 1.0).into()
}

/// Ctrl+A picks every vertex of the selected mesh, Esc unpicks, and in the
/// Weights tool Ctrl+C / Ctrl+V copy and paste the picked vertices' weights.
fn vertex_keys(app: &mut EditorApp, ctx: &egui::Context, pose: &Pose) {
    if ctx.egui_wants_keyboard_input() || !matches!(app.dialog, crate::app::Dialog::None) || !matches!(app.tool, Tool::Select | Tool::Mesh | Tool::Weights) {
        return;
    }
    let Some((si, m)) = selected_mesh(app, pose) else { return };
    let m = m.into_owned();
    // egui turns Ctrl+C / Ctrl+V into clipboard events rather than key presses.
    let (all, esc, copy, paste) = ctx.input(|i| {
        let cmd = i.modifiers.command;
        let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
        let paste = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) || (cmd && i.key_pressed(Key::V));
        (cmd && i.key_pressed(Key::A), i.key_pressed(Key::Escape), copy, paste)
    });
    if all {
        pick_vertices(app, si, &m.name, (0..m.vertices.len()).collect());
    }
    if esc {
        app.view.vertex_sel.set.clear();
    }
    if app.tool != Tool::Weights {
        return;
    }
    let picked = picked_vertices(app, si, &m);
    let slot_bone = app.doc.slots[si].bone.clone();
    if copy && !picked.is_empty() {
        app.view.weight_clipboard = Some(meshedit::copy_weights(&m, &picked, &slot_bone));
        app.status = crate::tf!("Copied the weights of {} vertices", picked.len());
    }
    if paste
        && !picked.is_empty()
        && app.playback.anim.is_none()
        && let Some(w) = app.view.weight_clipboard.clone()
    {
            let op=paste_weight_op(app,&app.doc.slots[si].name,&m,&picked,&w);
            app.edit(vec![op]);
    }
}

pub fn paste_weight_op(app: &EditorApp, slot: &str, mesh: &CoreMesh, indices: &[usize], weights: &std::collections::BTreeMap<String,f32>) -> Op {
    let bone=&app.doc.slot(slot).unwrap().bone;
    let vertices=indices.iter().filter_map(|&index|mesh.vertices.get(index).map(|v|VertexPatch {
        index, weights:Some(skel2d_core::mesh::preserve_locked(v,weights,bone,&app.view.weight_locked)), ..Default::default()
    })).collect();
    Op::SetVertices {slot:slot.into(),attachment:mesh.name.clone(),skin:app.skin_of(slot,&mesh.name),vertices}
}

/// Temporary pose only: mesh bind positions and the document stay unchanged.
pub fn weight_test_pose(doc: &Document, skins: &[String], rotation: Option<&(String,f32)>) -> Pose {
    let mut setup=doc.clone(); setup.ik.clear();
    if let Some((bone,angle))=rotation {
        let mut animation=skel2d_core::Animation::default();
        let mut timelines=skel2d_core::BoneTimelines::default();
        timelines.rotate.push(skel2d_core::Key::new(0.0,*angle,Default::default()));
        animation.bones.insert(bone.clone(),timelines);
        let mut name="__weight_test".to_string();
        while setup.animations.contains_key(&name) {name.push('_');}
        setup.animations.insert(name.clone(),animation);
        Pose::compute_skinned(&setup,&[skel2d_core::pose::Track {animation:&setup.animations[&name],time:0.0,alpha:1.0}],skins)
    } else {Pose::compute_skinned(&setup,&[],skins)}
}

/// Vertex indices picked on one mesh attachment; ignored once another slot
/// or attachment is selected.
#[derive(Default)]
struct VertexSel {
    slot: String,
    attachment: String,
    set: BTreeSet<usize>,
}

/// A bone being drawn with the Bone tool, in world space.
struct BoneDraw {
    start: [f32; 2],
    end: [f32; 2],
    parent: Option<String>,
}

/// A weight paint stroke: the mesh and the bone being painted.
struct Paint {
    base: Document,
    slot: String,
    attachment: String,
    bone: String,
}

pub fn ui(app: &mut EditorApp, ui: &mut egui::Ui) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, theme::VIEWPORT_BG);

    let ctx = ui.ctx().clone();
    let typing = ctx.egui_wants_keyboard_input() || !matches!(app.dialog, crate::app::Dialog::None);
    frame_keys(app, &ctx, typing, rect);
    crate::ikbend::flip_key(app, &ctx, typing);
    if response.hovered() && !typing && app.drag.is_none() {
        nudge(app, &ctx);
    }

    let pose = app.pose();
    if app.camera.needs_fit {
        app.camera.fit(rect, &app.doc, &pose);
    }
    navigate(app, ui, &response);
    if response.hovered() {
        vertex_keys(app, &ctx, &pose);
    }

    app.view.live = None;
    app.view.hover = None;
    app.view.hover_handle = None;
    app.view.mesh_insert = None;
    if app.drag.is_none()
        && app.vertex_drag.is_none()
        && !response.dragged()
        && let Some(p) = response.hover_pos()
    {
        app.view.hover_handle = gizmo(app, &pose, rect).and_then(|g| gizmo_hit(&g, app.tool, p));
        if app.view.hover_handle.is_none() {
            app.view.hover = pick(app, &pose, rect, p);
        }
    }
    // Pointing at a row in the tree highlights it here too.
    if let Some(h) = app.view.tree_hover.take()
        && app.view.hover.is_none()
    {
        app.view.hover = Some(h);
    }

    if app.settings.show_grid {
        draw_grid(app, &painter, rect);
    }
    draw_axes(app, &painter, rect);
    if app.settings.onion_skin && app.playback.anim.is_some() {
        draw_onion(app, &painter, rect);
    }
    draw_slots(app, &ctx, &painter, rect, &pose);
    draw_weight_overlay(app, &painter, rect, &pose);
    if app.show_bones {
        if rig_marks_shown(app.step, app.doc.bones.len()) {
            draw_physics_ghosts(app, &painter, rect, &pose);
        }
        draw_bones(app, &painter, rect, &pose);
        draw_physics_marks(app, &painter, rect, &pose);
    }
    draw_vertices(app, &painter, rect, &pose);
    draw_gizmo(app, &painter, rect, &pose);
    if app.show_bones && !matches!(app.tool, Tool::Bone | Tool::Weights | Tool::Mesh) {
        crate::ikbend::draw(app, &painter, rect, &pose, response.hover_pos());
    }
    // Without an animation, step 5 has nothing to key: the viewport only shows.
    if !(app.step == crate::steps::Step::Animate && app.playback.anim.is_none()) {
        interact(app, ui, &response, rect, &pose);
    }
    // Interaction may have edited the document (new bone, mesh vertex).
    let pose = app.pose();
    draw_tool_preview(app, &painter, rect, &pose, response.hover_pos());
    context_menu(app, &response, rect, &pose);
    drop_images(app, &ctx, &painter, rect, &pose);

    if let Some((at, text)) = &app.view.live {
        label(&painter, *at, Align2::LEFT_TOP, text, theme::SIZE_SMALL, Color32::WHITE);
    }
    if let Some(hover) = response.hover_pos() {
        let [x, y] = app.camera.to_world(rect, hover);
        label(&painter, rect.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, &crate::tf!("x {}  y {}   zoom {}", format!("{x:.1}"), format!("{y:.1}"), format!("{:.2}", app.camera.zoom)), theme::SIZE_SMALL, theme::TEXT);
    }
    if app.playback.anim.is_some() {
        painter.rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, theme::ANIMATE), StrokeKind::Inside);
    }
    draw_event_toasts(app, ui, &painter, rect);
    // The viewport's one-line guide, and the start screen of an empty document.
    app.viewport_overlay(ui, rect);
    if app.doc.slots.is_empty() && app.doc.bones.len() <= 1 {
        start_overlay(app, ui, rect);
    }
}

/// Events the playhead just passed, stacked at the top for `EVENT_TOAST_SECS`.
/// Fades out at the end; a repaint is requested until the last one is gone.
fn draw_event_toasts(app: &mut EditorApp, ui: &egui::Ui, painter: &egui::Painter, rect: Rect) {
    app.event_toasts.retain(|(_, at)| at.elapsed().as_secs_f32() < theme::EVENT_TOAST_SECS);
    if app.event_toasts.is_empty() {
        return;
    }
    for (i, (text, at)) in app.event_toasts.iter().rev().take(4).enumerate() {
        let fade = (1.0 - at.elapsed().as_secs_f32() / theme::EVENT_TOAST_SECS).clamp(0.0, 1.0);
        let color = theme::EVENT.gamma_multiply(0.35 + 0.65 * fade);
        let galley = painter.layout_no_wrap(text.clone(), FontId::proportional(theme::SIZE_BODY), color);
        let box_rect = Align2::CENTER_TOP.anchor_size(rect.center_top() + vec2(0.0, 12.0 + i as f32 * 30.0), galley.size() + vec2(20.0, 8.0));
        painter.rect_filled(box_rect, 6.0, theme::OVERLAY_BG.gamma_multiply(0.5 + 0.5 * fade));
        painter.rect_stroke(box_rect, 6.0, Stroke::new(1.0, color), StrokeKind::Inside);
        painter.galley(box_rect.min + vec2(10.0, 4.0), galley, color);
    }
    ui.ctx().request_repaint();
}

/// The empty document says how to start: the five steps and two buttons.
fn start_overlay(app: &mut EditorApp, ui: &mut egui::Ui, rect: Rect) {
    use crate::i18n::tr;
    let area = egui::Area::new(egui::Id::unique("start_overlay")).order(egui::Order::Foreground).fixed_pos(rect.center() - vec2(230.0, 150.0));
    area.show(ui.ctx(), |ui| {
        egui::Frame::new().fill(theme::OVERLAY_BG).corner_radius(8).stroke(Stroke::new(1.0, Color32::from_gray(90))).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
            ui.set_width(440.0);
            ui.label(egui::RichText::new(tr("Start here")).strong().size(20.0).color(Color32::WHITE));
            ui.add_space(6.0);
            for (n, line) in [
                (1, "Load images (your drawing, as PNG or PSD)"),
                (2, "Draw bones where the joints bend"),
                (3, "Turn pictures into meshes so they can bend"),
                (4, "Set weights: which bone moves which part"),
                (5, "Animate: record keys over time"),
            ] {
                ui.label(egui::RichText::new(format!("{n}.  {}", tr(line))).size(theme::SIZE_BODY).color(theme::TEXT));
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("The numbered buttons at the top switch between these steps (keys 1 to 5).")).size(theme::SIZE_SMALL).color(theme::TEXT_DIM));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let load = egui::Button::new(egui::RichText::new(tr("Load images")).strong().color(theme::ON_ACCENT).size(theme::SIZE_BODY)).fill(theme::STEP_ACTIVE).min_size(vec2(160.0, theme::STEP_BTN_H));
                if ui.add(load).clicked() {
                    app.set_step(crate::steps::Step::Images);
                    let files = crate::import::pick_images();
                    if !files.is_empty() {
                        let center = app.camera.center;
                        import::import_images(app, &files, center, None);
                    }
                }
                let example = egui::Button::new(egui::RichText::new(tr("Open example")).size(theme::SIZE_BODY)).min_size(vec2(160.0, theme::STEP_BTN_H));
                if ui.add(example).on_hover_text(tr("Open one of the example rigs that come with Skel2D")).clicked() {
                    app.request_replace(crate::app::Replace::OpenExample);
                }
            });
        });
    });
}

/// A small text with a dark backing so it reads over any content.
fn label(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: &str, size: f32, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(size), color);
    let r = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(r.expand2(vec2(3.0, 1.0)), 2.0, Color32::from_black_alpha(170));
    painter.galley(r.min, galley, color);
}

/// F frames the selection, Shift+F everything.
fn frame_keys(app: &mut EditorApp, ctx: &egui::Context, typing: bool, rect: Rect) {
    if typing {
        return;
    }
    let (f, m) = ctx.input(|i| (i.key_pressed(Key::F), i.modifiers));
    if !f || m.command || m.alt {
        return;
    }
    if m.shift {
        app.camera.needs_fit = true;
    } else {
        frame_selection(app, rect);
    }
}

fn frame_selection(app: &mut EditorApp, rect: Rect) {
    let pose = app.pose();
    let b = app.selection.bone.as_deref().and_then(|n| bounds(&app.doc, &pose, Some(&subtree(&app.doc, n))));
    match b {
        Some(b) => app.camera.frame(rect, b, 40.0),
        None => app.camera.needs_fit = true,
    }
}

fn navigate(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response) {
    let cam = &mut app.camera;
    if let Some(hover) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (scroll * 0.0015).exp();
        if factor != 1.0 {
            let before = cam.to_world(response.rect, hover);
            cam.zoom = (cam.zoom * factor).clamp(0.01, 100.0);
            let after = cam.to_world(response.rect, hover);
            cam.center[0] += before[0] - after[0];
            cam.center[1] += before[1] - after[1];
        }
    }
    if response.dragged_by(PointerButton::Middle) || response.dragged_by(PointerButton::Secondary) {
        let d = response.drag_delta();
        cam.center[0] -= d.x / cam.zoom;
        cam.center[1] += d.y / cam.zoom;
    }
}

fn current_pick(app: &EditorApp) -> Option<Pick> {
    match (&app.selection.slot, &app.selection.bone) {
        (Some(s), Some(b)) => Some(Pick::Slot(s.clone(), b.clone())),
        (None, Some(b)) => Some(Pick::Bone(b.clone())),
        _ => None,
    }
}

fn tool_verb(t: Tool) -> &'static str {
    match t {
        Tool::Select | Tool::Mesh => crate::i18n::tr("select"),
        Tool::Bone => crate::i18n::tr("draw"),
        Tool::Weights => crate::i18n::tr("paint"),
        Tool::Rotate => crate::i18n::tr("rotate"),
        Tool::Translate => crate::i18n::tr("move"),
        Tool::Scale => crate::i18n::tr("scale"),
    }
}

fn interact(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose) {
    match app.tool {
        Tool::Bone => return bone_tool(app, ui, response, rect, pose),
        Tool::Weights => return weight_tool(app, ui, response, rect, pose),
        Tool::Mesh => {}
        _ => {
            if app.show_bones && crate::ikbend::interact(app, ui, response, rect, pose) {
                return;
            }
        }
    }
    let ctx = ui.ctx();
    let (alt, shift, ctrl) = ctx.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
    // The select and mesh tools drag vertices: in the setup pose to edit the
    // mesh, in an animation to key its deformation. Topology edits (the
    // Mesh tool's clicks) are setup-only.
    let edit_vertices = matches!(app.tool, Tool::Select | Tool::Mesh);
    let mesh_tool = app.tool == Tool::Mesh && app.playback.anim.is_none();
    if mesh_tool && app.view.mesh_mode == MeshMode::Edges {
        mesh_edge_tool(app,ui,response,rect,pose);
        return;
    }
    let giz = gizmo(app, pose, rect);
    let handle_at = |tool: Tool, p: Pos2| giz.as_ref().and_then(|g| gizmo_hit(g, tool, p));

    let mut handled = false;
    if mesh_tool && let Some(p) = response.hover_pos() {
        handled = mesh_tool_click(app, ui, response, rect, pose, p);
    }
    if mesh_tool && app.view.mesh_mode == MeshMode::Delete { return; }

    if !handled
        && (edit_vertices || app.tool.transforms())
        && response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && let Some(index) = vertex_at(app, pose, rect, p)
        && let Some((si, m)) = selected_mesh(app, pose)
    {
        let m = m.into_owned();
        // Shift or Ctrl adds to (or removes from) the picked vertices.
        pick_vertex(app, si, &m, index, shift || ctrl, true);
        handled = true;
    }

    if !handled
        && response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && handle_at(app.tool, p).is_none()
    {
        if !shift {
            app.view.vertex_sel.set.clear();
        }
        if alt {
            let list = pick_all(app, pose, rect, p);
            if !list.is_empty() {
                let cur = current_pick(app);
                let idx = list.iter().position(|x| Some(x) == cur.as_ref()).map_or(0, |i| (i + 1) % list.len());
                select(app, Some(list[idx].clone()));
                app.view.cycle = list;
                app.view.cycle_idx = idx;
            }
        } else {
            select(app, pick(app, pose, rect, p));
        }
    }

    if response.drag_started_by(PointerButton::Primary) {
        let origin = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
        if let Some(p) = origin {
            let handle = handle_at(app.tool, p);
            // Ctrl+drag with Move, Rotate or Scale in an animation box-selects vertices
            // to deform with the tool (Select and Mesh box-select without Ctrl).
            let box_vertices = app.tool.transforms() && ctrl && app.playback.anim.is_some() && selected_mesh(app, pose).is_some();
            if handle.is_none() {
                if box_vertices {
                    app.view.vertex_box = Some([p, p]);
                } else if edit_vertices && (!mesh_tool || matches!(app.view.mesh_mode, MeshMode::Modify | MeshMode::Create))
                    && let Some(index) = vertex_at(app, pose, rect, p)
                    && let Some((si, m)) = selected_mesh(app, pose)
                {
                    let m = m.into_owned();
                    pick_vertex(app, si, &m, index, shift, false);
                    // The Mesh tool keeps the image in place (UV follows the
                    // vertices); Alt, or the Select tool, deforms the image instead.
                    let uv = if mesh_tool && !alt { UvMap::fit(&m) } else { None };
                    let indices = picked_vertices(app, si, &m);
                    let deform = DeformDrag::start(app, pose, si, &m, &indices);
                    if app.playback.anim.is_none() || deform.is_some() {
                        let pts = pose.deform_mesh(&app.doc, &m, si);
                        let moving = indices.iter().map(|&i| (i, pts[i])).collect();
                        let start = app.camera.to_world(rect, p);
                        let (base, slot) = (app.doc.clone(), app.doc.slots[si].name.clone());
                        app.vertex_drag = Some(VertexDrag { base, slot, attachment: m.name.clone(), moving, start, uv, deform });
                        app.push_undo();
                    }
                } else if let (Tool::Select, Some(Pick::Bone(b))) = (app.tool, pick(app, pose, rect, p)) {
                    // Pose tool, as in Spine: dragging a bone moves it, near its tip rotates it.
                    select(app, Some(Pick::Bone(b.clone())));
                    let (origin, tip) = (bone_origin(app, pose, &b), bone_tip(app, pose, &b));
                    let near_tip = match (origin, tip) {
                        (Some(o), Some(t)) => {
                            let (o, t) = (app.camera.to_screen(rect, o), app.camera.to_screen(rect, t));
                            o.distance(t) > 12.0 && p.distance(t) < p.distance(o).min(o.distance(t) * 0.45)
                        }
                        _ => false,
                    };
                    let tool = if near_tip { Tool::Rotate } else { Tool::Translate };
                    app.drag = start_drag(app, pose, app.camera.to_world(rect, p), None, tool, false);
                    if app.drag.is_some() {
                        app.push_undo();
                    }
                } else if edit_vertices && selected_mesh(app, pose).is_some() {
                    // Dragging off the vertices of the selected mesh box-selects them.
                    app.view.vertex_box = Some([p, p]);
                } else if let Some(hit) = pick(app, pose, rect, p) {
                    select(app, Some(hit));
                }
            }
            if app.tool.transforms() && app.vertex_drag.is_none() && app.view.vertex_box.is_none() {
                app.drag = start_drag(app, pose, app.camera.to_world(rect, p), handle, app.tool, ui.input(|i| i.modifiers.alt));
                if app.drag.is_some() {
                    app.push_undo();
                }
            }
        }
    }

    if app.drag.is_some()
        && response.dragged_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && let Some(mut d) = app.drag.take()
    {
        let now = app.camera.to_world(rect, p);
        let k = if shift { 0.25 } else { 1.0 };
        d.eff = [d.eff[0] + (now[0] - d.last[0]) * k, d.eff[1] + (now[1] - d.last[1]) * k];
        d.last = now;
        let (x, y) = ctx.input(|i| (i.key_down(Key::X), i.key_down(Key::Y)));
        let axis = if d.tool == Tool::Translate && x != y { if x { Axis::X } else { Axis::Y } } else { d.axis };
        let snap = Snap { on: app.snapping(ctx), angle: app.settings.snap_angle, grid: app.settings.snap_grid };
        if let Some(v) = drag_values(&d, axis, &snap) {
            let op = match &d.att {
                Some(a) => attachment_op(a, &v),
                None => Some(vals_op(app, &d.bone, &v)),
            };
            if let Some(op) = op {
                app.preview(&d.base, &[op]);
            }
            let mut text = v.text();
            if snap.on {
                text += crate::i18n::tr("  (snap)");
            }
            app.view.live = Some((p + vec2(16.0, 18.0), text));
        }
        let mut hint = app.view.live.as_ref().map(|l| l.1.clone()).unwrap_or_default();
        hint += crate::i18n::tr("  ·  Shift fine · Ctrl snap toggle");
        if d.tool == Tool::Translate {
            hint += crate::i18n::tr(" · hold X / Y to lock axis");
        }
        app.hint = hint;
        app.drag = Some(d);
    }

    if let (Some(vd), true) = (&app.vertex_drag, response.dragged_by(PointerButton::Primary))
        && let Some(p) = response.interact_pointer_pos()
    {
        let now = app.camera.to_world(rect, p);
        let (dx, dy) = (now[0] - vd.start[0], now[1] - vd.start[1]);
        let targets: Vec<[f32; 2]> = vd.moving.iter().map(|(_, [x, y])| [round2(x + dx), round2(y + dy)]).collect();
        let op = match &vd.deform {
            Some(d) => d.op(&vd.slot, &vd.attachment, &targets),
            None => Op::SetVertices {
                slot: vd.slot.clone(),
                attachment: vd.attachment.clone(),
                vertices: vd
                    .moving
                    .iter()
                    .zip(&targets)
                    .map(|((index, _), &[x, y])| {
                        let uv = vd.uv.map(|m| m.at([x, y]));
                        VertexPatch { index: *index, x: Some(x), y: Some(y), u: uv.map(|t| t[0]), v: uv.map(|t| t[1]), ..Default::default() }
                    })
                    .collect(),
                skin: None,
            },
        };
        let (base, count, keying) = (vd.base.clone(), vd.moving.len(), vd.deform.is_some());
        app.preview(&base, &[op]);
        let (dx, dy) = (format!("{dx:.1}"), format!("{dy:.1}"));
        app.hint = if keying {
            crate::tf!("Deforming {} vertices by x {} y {} (keyed at the playhead)", count, dx, dy)
        } else {
            crate::tf!("Moving {} vertices by x {} y {}", count, dx, dy)
        };
    }

    if let Some(b) = &mut app.view.vertex_box
        && let Some(p) = response.interact_pointer_pos()
    {
        b[1] = p;
    }

    if response.drag_stopped() {
        app.drag = None;
        app.vertex_drag = None;
        if let Some([a, b]) = app.view.vertex_box.take()
            && let Some((si, m)) = selected_mesh(app, pose)
        {
            let r = Rect::from_two_pos(a, b);
            let inside = vertex_dots(app, pose, rect).into_iter().enumerate().filter(|(_, d)| r.contains(*d)).map(|(i, _)| i);
            let mut set: BTreeSet<usize> = if shift { picked_vertices(app, si, &m).into_iter().collect() } else { BTreeSet::new() };
            set.extend(inside);
            let name = m.name.clone();
            pick_vertices(app, si, &name, set);
        }
    }

    if response.hovered() && app.drag.is_none() && app.vertex_drag.is_none() {
        if app.tool == Tool::Mesh {
            app.hint = mesh_tool_hint(app, pose, rect, response.hover_pos());
        } else if let Some(h) = app.view.hover_handle {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
            app.hint = crate::tf!("{} · Shift fine · Ctrl snap toggle", handle_hint(h));
        } else if let Some(hit) = app.view.hover.clone() {
            let over = response.hover_pos().map_or(1, |p| pick_all(app, pose, rect, p).len());
            let what = match &hit {
                Pick::Bone(_) => crate::i18n::tr("bone"),
                Pick::Slot(..) => crate::i18n::tr("slot"),
            };
            let act = if app.tool == Tool::Select || app.selection.bone.as_deref() != Some(hit_bone(&hit)) {
                crate::i18n::tr("click to select").to_string()
            } else {
                crate::tf!("drag to {}", tool_verb(app.tool))
            };
            let cycle = if over > 1 { crate::tf!(" · Alt+click cycle ({} here)", over) } else { crate::i18n::tr(" · Alt+click cycle").to_string() };
            app.hint = crate::tf!("{} {} — {} · Shift fine · Ctrl snap{}", hit.name(), what, act, cycle);
            response.clone().on_hover_text(format!("{what}: {}", hit.name()));
        } else if app.tool == Tool::Select {
            app.hint = crate::i18n::tr("Click to select · mesh vertices: click / Shift+click / drag a box to pick, drag to move (setup) or key deformation (animation) · F frame selection · Shift+F frame all").into();
        } else if let Some(a) = attachment_target(app, pose)
            && !a.vertices.is_empty()
        {
            app.hint = crate::tf!("Drag to {} the picked vertices (in an animation: keys deformation) · click empty space to unpick · Shift fine · Ctrl snap · arrows nudge (Shift ×10)", tool_verb(app.tool));
        } else if attachment_target(app, pose).is_some() {
            app.hint = crate::tf!("Drag to {} the selected image (click its bone to move the bone) · Shift fine · Ctrl snap · arrows nudge (Shift ×10) · F frame", tool_verb(app.tool));
        } else if app.selection.bone.is_some() {
            app.hint = crate::tf!("Drag to {} the selected bone · Shift fine · Ctrl snap · arrows nudge (Shift ×10) · F frame", tool_verb(app.tool));
        } else {
            app.hint = crate::i18n::tr("Click a bone to select it").into();
        }
    }
}

fn bone_tip(app: &EditorApp, pose: &Pose, name: &str) -> Option<[f32; 2]> {
    let i = app.doc.bone_index(name)?;
    // `get`: the pose may predate a bone added this frame.
    Some(pose.bones.get(i)?.world.apply(app.doc.bones[i].length, 0.0))
}

fn bone_origin(app: &EditorApp, pose: &Pose, name: &str) -> Option<[f32; 2]> {
    Some(pose.bones.get(app.doc.bone_index(name)?)?.world.apply(0.0, 0.0))
}

/// Bone tool: drag draws a bone (child of the selected bone, starting at its
/// tip when the drag starts near it); click selects a bone as the next
/// parent, or clears the selection so the next bone is a root.
fn bone_tool(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose) {
    if app.playback.anim.is_some() {
        app.view.bone_draw = None;
        if response.hovered() {
            app.hint = crate::i18n::tr("The Bone tool edits the rig: pick \"(setup pose)\" in the timeline").into();
        }
        return;
    }
    let snap = app.snapping(ui.ctx());
    let grid = app.settings.snap_grid;
    let snapped = |w: [f32; 2]| if snap { w.map(|c| snap_to(c, grid)) } else { w }.map(round2);

    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        let hit = pick_all(app, pose, rect, p).into_iter().find(|h| matches!(h, Pick::Bone(_)));
        select(app, hit);
    }
    if response.drag_started_by(PointerButton::Primary)
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
    {
        let parent = app.selection.bone.clone().filter(|b| app.doc.bone(b).is_some());
        let tip = parent.as_deref().and_then(|b| bone_tip(app, pose, b));
        let start = match tip {
            Some(t) if app.camera.to_screen(rect, t).distance(p) < 12.0 => t.map(round2),
            _ => snapped(app.camera.to_world(rect, p)),
        };
        app.view.bone_draw = Some(BoneDraw { start, end: start, parent });
    }
    if response.dragged_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && let Some(d) = &mut app.view.bone_draw
    {
        d.end = snapped(app.camera.to_world(rect, p));
        let (dx, dy) = (d.end[0] - d.start[0], d.end[1] - d.start[1]);
        let text = crate::tf!("length {}  angle {}°", format!("{:.1}", dx.hypot(dy)), format!("{:.1}", dy.atan2(dx).to_degrees()));
        let parent = d.parent.clone().map_or(crate::i18n::tr("a new root").to_string(), |p| crate::tf!("a child of {}", p));
        app.view.live = Some((p + vec2(16.0, 18.0), text));
        app.hint = crate::tf!("Drawing {} · Ctrl toggles snapping · release to create", parent);
    }
    if response.drag_stopped()
        && let Some(d) = app.view.bone_draw.take()
        && app.camera.to_screen(rect, d.start).distance(app.camera.to_screen(rect, d.end)) >= 5.0
    {
        let name = (1..).map(|i| if i == 1 { "bone".to_string() } else { format!("bone_{i}") }).find(|n| app.doc.bone(n).is_none()).expect("unbounded range");
        if app.edit(vec![Op::AddBoneWorld { name: name.clone(), parent: d.parent, start: d.start, end: Some(d.end) }]) {
            select(app, Some(Pick::Bone(name)));
        }
    }
    if response.hovered() && app.view.bone_draw.is_none() {
        app.hint = match &app.selection.bone {
            Some(b) => crate::tf!("Drag to draw a child of {} (start at its tip to chain) · click a bone to pick the parent · click empty space for a root", b),
            None => crate::i18n::tr("Drag to draw a root bone · click a bone to draw children of it").into(),
        };
    }
}

/// Mesh tool clicks and the Delete key: click inside the selected mesh adds
/// a vertex, Ctrl+click or Delete removes the one under the cursor. Returns
/// true when the click was used.
fn mesh_tool_click(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose, p: Pos2) -> bool {
    let Some((si, m)) = selected_mesh(app, pose) else { return false };
    let (slot, slot_bone, mesh) = (app.doc.slots[si].name.clone(), app.doc.slots[si].bone.clone(), m.into_owned());
    let skin = app.skin_of(&slot,&mesh.name);
    let over = vertex_at(app, pose, rect, p);
    let (at, tol) = (app.camera.to_world(rect, p), theme::MESH_PICK_RADIUS / app.camera.zoom);
    let mode = app.view.mesh_mode;
    app.view.mesh_insert = if mode == MeshMode::Create && over.is_none() {
        skel2d_core::mesh::insert_vertex(&mesh,at,tol,&slot_bone).ok().and_then(|edit| {
            edit.remap.iter().position(|parts|parts.len()>1).map(|i|[edit.mesh.vertices[i].x,edit.mesh.vertices[i].y])
        })
    } else { None };
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (ctrl, delete) = ui.input(|i| (i.modifiers.command, !typing && (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))));
    let clicked = response.clicked_by(PointerButton::Primary);
    let picked = picked_vertices(app, si, &mesh);
    let remove = if delete && !picked.is_empty() { Some(picked) }
        else { over.filter(|_|(clicked && (ctrl || mode==MeshMode::Delete)) || delete || response.double_clicked()).map(|i|vec![i]) };
    if let Some(indices)=remove {
        let n = indices.len();
        if app.edit(vec![Op::RemoveMeshVertices {slot,attachment:mesh.name,skin,indices}]) {
            app.view.vertex_sel.set.clear();
            app.status=crate::tf!("{} vertices deleted · Ctrl+Z to undo", n);
        }
        return true;
    }
    if clicked && over.is_none() && mode==MeshMode::Create && app.view.mesh_insert.is_some() {
        if app.edit(vec![Op::AddMeshVertices {slot,attachment:mesh.name,skin,points:vec![at],on_edge_tolerance:tol}]) {
            app.view.vertex_sel.set.clear();
            app.status=crate::i18n::tr("Vertex added · Ctrl+Z to undo").into();
        }
        return true;
    }
    mode==MeshMode::Delete && clicked
}

fn mesh_edge_tool(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose) {
    let Some((si,mesh))=selected_mesh(app,pose) else { return };
    let mesh=mesh.into_owned();
    let slot=app.doc.slots[si].name.clone();
    let skin=app.skin_of(&slot,&mesh.name);
    let points:Vec<_>=pose.deform_mesh(&app.doc,&mesh,si).into_iter().map(|p|app.camera.to_screen(rect,p)).collect();
    if response.clicked_by(PointerButton::Primary) && let Some(p)=response.interact_pointer_pos() {
        let nearest=mesh.edges.iter().filter_map(|e| {
            let (a,b)=(points[e[0] as usize],points[e[1] as usize]);
            let ab=b-a;
            let t=if ab.length_sq()>0.0 {((p-a).dot(ab)/ab.length_sq()).clamp(0.0,1.0)} else {0.0};
            let d=p.distance(a+ab*t);
            (d<=theme::MESH_PICK_RADIUS).then_some((*e,d))
        }).min_by(|a,b|a.1.total_cmp(&b.1)).map(|v|v.0);
        app.view.edge_selection=nearest.map(|e|(slot.clone(),mesh.name.clone(),skin.clone(),e));
    }
    let typing=ui.ctx().egui_wants_keyboard_input();
    if response.hovered() && !typing && ui.input(|i|i.key_pressed(Key::Delete)||i.key_pressed(Key::Backspace)) {
        apply_mesh_action(app,MeshAction::RemoveEdge);
    }
    if response.drag_started_by(PointerButton::Primary) {
        let origin=ui.input(|i|i.pointer.press_origin()).or(response.interact_pointer_pos());
        if let Some(p)=origin && let Some(from)=vertex_at(app,pose,rect,p) {
            app.view.edge_drag=Some(MeshEdgeDrag {slot:slot.clone(),attachment:mesh.name.clone(),skin:skin.clone(),from});
        }
    }
    if let Some(drag)=&app.view.edge_drag
        && drag.slot==slot && drag.attachment==mesh.name && drag.skin==skin
        && let Some(p)=response.hover_pos()
        && let Some(&from)=points.get(drag.from) {
            ui.painter().line_segment([from,p],Stroke::new(theme::MESH_EDGE_STROKE,theme::KEY_CHANGED));
        }
    if response.drag_stopped_by(PointerButton::Primary)
        && let Some(drag)=app.view.edge_drag.take()
        && drag.slot==slot && drag.attachment==mesh.name && drag.skin==skin
        && let Some(p)=response.interact_pointer_pos()
        && let Some(to)=vertex_at(app,pose,rect,p)
        && to!=drag.from {
            let mut edges=mesh.edges.clone();
            let e=[drag.from.min(to) as u32,drag.from.max(to) as u32];
            if !edges.contains(&e) {
                edges.push(e);
                if app.edit(vec![Op::SetMeshEdges {slot,attachment:mesh.name,skin,edges}]) {
                    app.view.edge_selection=None;
                    app.status=crate::i18n::tr("Fold line added · Ctrl+Z to undo").into();
                }
            }
        }
    app.hint=crate::i18n::tr("Drag vertex to vertex to add a constrained edge · click an orange edge and Delete to remove · right-click cycles mesh modes").into();
}
fn mesh_tool_hint(app: &EditorApp, pose: &Pose, rect: Rect, hover: Option<Pos2>) -> String {
    if app.playback.anim.is_some() {
        return crate::i18n::tr("Drag a vertex to key the mesh deformation at the playhead · adding or removing vertices needs \"(setup pose)\"").into();
    }
    if selected_mesh(app, pose).is_none() {
        return crate::i18n::tr("Click a slot to select it · a region needs Trace mesh or Convert to mesh (properties) first").into();
    }
    if app.view.mesh_mode == MeshMode::Delete {
        return crate::i18n::tr("Modify: drag vertices · Create: click inside · Delete: click a vertex · Edges: drag vertex to vertex").into();
    }
    match hover.and_then(|p| vertex_at(app, pose, rect, p)) {
        Some(i) => crate::tf!("Vertex {} — drag to move the picked vertices (the image stays) · Shift+click pick more · Ctrl+A all · Alt+drag deforms the image · double-click, Ctrl+click or Delete removes (Delete: all picked)", i),
        None if app.view.mesh_insert.is_some() => crate::i18n::tr("Click to add a vertex here (on an edge: split it) · F frame selection").into(),
        None => crate::i18n::tr("Modify: drag vertices · Create: click inside · Delete: click a vertex · Edges: drag vertex to vertex").into(),
    }
}

/// Weights tool: drag paints the selected bone's weight on the selected
/// mesh (Ctrl removes), a click paints one dab; clicking a bone picks it as
/// the paint bone and keeps the mesh selected.
fn weight_tool(app: &mut EditorApp, ui: &egui::Ui, response: &egui::Response, rect: Rect, pose: &Pose) {
    let ctx = ui.ctx();
    if app.playback.anim.is_some() { app.view.pose_rotation=None; return; }
    let hold_pose=app.view.pose_test && !ctx.egui_wants_keyboard_input() && ctx.input(|i|i.key_down(Key::T));
    if hold_pose {
        app.view.pose_rotation=app.selection.bone.clone().map(|bone|(bone,app.view.pose_angle));
        ctx.request_repaint();
    }
    if app.view.pose_test && ctx.input(|i|i.modifiers.alt && i.pointer.primary_down()) {
        if let Some(bone)=app.selection.bone.clone() {
            app.view.pose_rotation=Some((bone,response.drag_delta().x * 0.5));
            ctx.request_repaint();
        }
        return;
    }
    if !hold_pose && app.view.pose_rotation.take().is_some() { ctx.request_repaint(); }
    if response.hovered() && !ctx.egui_wants_keyboard_input() {
        let (smaller, larger) = ctx.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
        let r = &mut app.settings.brush_radius;
        if smaller {
            *r = (*r / 1.25).max(4.0);
        }
        if larger {
            *r = (*r * 1.25).min(400.0);
        }
    }
    let (subtract, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
    let mut dab = false;
    // Shift+click picks vertices: brushes then only touch the picked ones.
    let mut vertex_pick = false;
    if shift
        && response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && let Some(index) = vertex_at(app, pose, rect, p)
        && let Some((si, m)) = selected_mesh(app, pose)
    {
        let m = m.into_owned();
        pick_vertex(app, si, &m, index, true, true);
        vertex_pick = true;
    }
    if !vertex_pick
        && response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        match pick(app, pose, rect, p) {
            Some(Pick::Bone(b)) if app.selection.slot.is_some() => {
                app.selection.bone = Some(b);
                app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
            }
            Some(Pick::Slot(s, _)) if app.selection.slot.as_deref() == Some(s.as_str()) => dab = true,
            hit => select(app, hit),
        }
    }
    let target = selected_mesh(app, pose).map(|(si, m)| (app.doc.slots[si].name.clone(), m.name.clone()));
    let (Some((slot, attachment)), Some(bone)) = (target, app.selection.bone.clone()) else {
        app.view.paint = None;
        if response.hovered() {
            app.hint = crate::i18n::tr("Select a mesh slot, then click a bone, to paint that bone's weights").into();
        }
        return;
    };
    if dab && let Some(p) = response.interact_pointer_pos() {
        let paint=Paint {base:app.doc.clone(),slot:slot.clone(),attachment:attachment.clone(),bone:bone.clone()};
        paint_stroke(app, pose, rect, p, 0.15, subtract, &paint);
        finish_paint(app,paint);
    }
    if response.drag_started_by(PointerButton::Primary) {
        app.view.paint = Some(Paint { base:app.doc.clone(), slot, attachment, bone: bone.clone() });
    }
    if response.dragged_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
        && let Some(paint) = app.view.paint.take()
    {
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        paint_stroke(app, pose, rect, p, dt, subtract, &paint);
        app.view.paint = Some(paint);
        // Keep painting while the pointer rests.
        ctx.request_repaint();
    }
    if response.drag_stopped() && let Some(paint)=app.view.paint.take() {finish_paint(app,paint);}
    if response.hovered() {
        let mode = if subtract { BrushMode::Remove } else { app.settings.brush_mode };
        let verb = match mode {
            BrushMode::Add => crate::i18n::tr("Adding"),
            BrushMode::Remove => crate::i18n::tr("Removing"),
            BrushMode::Replace => crate::i18n::tr("Replacing"),
        };
        let locked = if app.view.weight_locked.contains(&bone) { crate::i18n::tr(" (locked)") } else { "" };
        app.hint = crate::tf!("{} {}{} weight · Ctrl removes · [ ] brush size ({}px) · Shift+click picks vertices (brush then only touches them) · Ctrl+A all · Esc none · Ctrl+C / Ctrl+V copy weights · click a bone to paint it instead", verb, bone, locked, format!("{:.0}", app.settings.brush_radius));
    }
}

fn finish_paint(app:&mut EditorApp,paint:Paint) {
    let Some(si)=app.doc.slot_index(&paint.slot) else {return};
    let Some(Attachment::Mesh(mesh))=app.doc.resolve_attachment(si,&paint.attachment,&app.skins) else {return};
    let skin=app.skin_of(&paint.slot,&paint.attachment);
    let vertices=mesh.vertices.iter().enumerate().map(|(index,v)|VertexPatch {index,weights:Some(v.weights.clone()),..Default::default()}).collect();
    let changed=app.doc!=paint.base;
    app.doc=paint.base;
    if changed {app.edit(vec![Op::SetVertices {slot:paint.slot,attachment:paint.attachment,skin,vertices}]);}
}

/// One step of a paint stroke: every vertex under the brush (only the picked
/// ones when some are) changes the paint bone's weight by `dt` × rate in the
/// brush mode, fading out over the feathered edge. Locked bones keep theirs.
fn paint_stroke(app: &mut EditorApp, pose: &Pose, rect: Rect, p: Pos2, dt: f32, subtract: bool, paint: &Paint) {
    let Some(si) = app.doc.slot_index(&paint.slot) else { return };
    let slot = &app.doc.slots[si];
    let Some(Attachment::Mesh(m)) = app.doc.resolve_attachment(si, &paint.attachment, &app.skins) else { return };
    let slot_bone = slot.bone.clone();
    // Weight taken from a vertex with no other bone goes to the slot's bone,
    // or to the parent when painting the slot's bone itself.
    let fallback = if slot_bone != paint.bone { Some(slot_bone.clone()) } else { app.doc.bone(&paint.bone).and_then(|b| b.parent.clone()) };
    let s = &app.settings;
    let (radius, mode) = (s.brush_radius.max(1.0), if subtract { BrushMode::Remove } else { s.brush_mode });
    let (rate, target, feather) = (s.brush_strength * 2.0 * dt, s.brush_target, s.brush_feather.clamp(0.0, 1.0));
    let picked = picked_vertices(app, si, m);
    let dots = vertex_dots(app, pose, rect);
    let locked = &app.view.weight_locked;
    let vertices: Vec<VertexPatch> = m
        .vertices
        .iter()
        .zip(&dots)
        .enumerate()
        .filter(|(index, _)| picked.is_empty() || picked.binary_search(index).is_ok())
        .filter_map(|(index, (v, d))| {
            let dist = d.distance(p) / radius;
            if dist >= 1.0 {
                return None;
            }
            // Full effect inside, fading to zero over the outer `feather` part.
            let falloff = if feather <= 0.0 { 1.0 } else { ((1.0 - dist) / feather).min(1.0) };
            let weights = meshedit::brush_weight(v, &paint.bone, mode, rate * falloff, target, &slot_bone, locked, fallback.as_deref());
            (weights != v.weights).then_some(VertexPatch { index, weights: Some(weights), ..Default::default() })
        })
        .collect();
    if vertices.is_empty() {
        return;
    }
    let op = Op::SetVertices { slot: paint.slot.clone(), attachment: paint.attachment.clone(), vertices, skin: None };
    let base = app.doc.clone();
    app.preview(&base, &[op]);
}

/// PNG and PSD files dropped on the window become slots at the drop point, on the
/// bone under the cursor or the selected bone.
fn drop_images(app: &mut EditorApp, ctx: &egui::Context, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let (hovering, dropped, at) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone(), i.pointer.latest_pos()));
    let p = at.filter(|p| rect.contains(*p)).unwrap_or(rect.center());
    let bone_at = || pick_all(app, pose, rect, p).into_iter().find_map(|h| if let Pick::Bone(b) = h { Some(b) } else { None }).or(app.selection.bone.clone());
    if hovering {
        let target = bone_at().map_or(crate::i18n::tr("a root bone").to_string(), |b| crate::tf!("bone {}", b));
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(110));
        painter.rect_stroke(rect.shrink(6.0), 6.0, Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
        label(painter, rect.center(), Align2::CENTER_CENTER, &crate::tf!("Drop PNG or PSD images to add them as slots on {}", target), 16.0, Color32::WHITE);
    }
    let files: Vec<std::path::PathBuf> = dropped.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect();
    if !files.is_empty() {
        let bone = bone_at();
        let world = app.camera.to_world(rect, p).map(round2);
        import::dropped_images(app, files, world, bone);
    }
}

/// Every bone's influence on a vertex as a blend of the bones' colors.
fn blended_bone_color(v: &skel2d_core::MeshVertex, slot_bone: &str) -> Color32 {
    let total: f32 = v.weights.iter().map(|w| w.weight.max(0.0)).sum();
    if v.weights.is_empty() || total <= 0.0 {
        return bone_color(slot_bone);
    }
    let mut c = [0.0f32; 3];
    for w in &v.weights {
        let (k, bc) = (w.weight.max(0.0) / total, bone_color(&w.bone));
        for (ch, x) in c.iter_mut().zip([bc.r(), bc.g(), bc.b()]) {
            *ch += x as f32 * k;
        }
    }
    Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8)
}

/// Weight overlay for the Weights tool: the mesh tinted by the paint bone's
/// weight, or by all bones in their colors.
fn draw_weight_overlay(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    if app.tool != Tool::Weights {
        return;
    }
    let Some((si, m)) = selected_mesh(app, pose) else { return };
    let bone = app.selection.bone.as_deref();
    if bone.is_none() && !app.settings.weights_all_bones {
        return;
    }
    let slot_bone = &app.doc.slots[si].bone;
    let dots = vertex_dots(app, pose, rect);
    let mut mesh = Mesh::default();
    for (v, d) in m.vertices.iter().zip(&dots) {
        let color = match bone {
            Some(b) if !app.settings.weights_all_bones => weight_color(bone_weight(v, b, slot_bone)),
            _ => blended_bone_color(v, slot_bone),
        };
        mesh.vertices.push(egui::epaint::Vertex { pos: *d, uv: egui::epaint::WHITE_UV, color: color.gamma_multiply(0.55) });
    }
    for t in m.triangles.iter().filter(|t| t.iter().all(|&i| (i as usize) < dots.len())) {
        mesh.add_triangle(t[0], t[1], t[2]);
    }
    painter.add(Shape::mesh(mesh));
}

/// Bone being drawn, the Mesh tool's insert point and the weight brush.
fn draw_tool_preview(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose, hover: Option<Pos2>) {
    let to = |w: [f32; 2]| app.camera.to_screen(rect, w);
    match app.tool {
        Tool::Bone if app.playback.anim.is_none() => match &app.view.bone_draw {
            Some(d) => {
                let (a, b) = (to(d.start), to(d.end));
                painter.line_segment([a, b], Stroke::new(5.0, Color32::from_black_alpha(120)));
                painter.line_segment([a, b], Stroke::new(3.0, theme::ACCENT));
                painter.circle(a, 4.0, theme::ACCENT, Stroke::new(1.0, Color32::BLACK));
                painter.circle_stroke(b, 4.0, Stroke::new(1.5, theme::ACCENT));
            }
            // Where a drag snaps to the selected bone's tip.
            None => {
                if let Some(tip) = app.selection.bone.as_deref().and_then(|b| bone_tip(app, pose, b)) {
                    painter.circle_stroke(to(tip), 12.0, Stroke::new(2.0, theme::ACCENT.gamma_multiply(0.8)));
                }
            }
        },
        Tool::Mesh => {
            if let Some(w) = app.view.mesh_insert {
                let c = to(w);
                painter.circle(c, 5.0, Color32::from_white_alpha(40), Stroke::new(2.0, theme::ACCENT));
                painter.line_segment([c - vec2(3.0, 0.0), c + vec2(3.0, 0.0)], Stroke::new(2.0, theme::ACCENT));
                painter.line_segment([c - vec2(0.0, 3.0), c + vec2(0.0, 3.0)], Stroke::new(2.0, theme::ACCENT));
            }
        }
        Tool::Weights => {
            if let Some(p) = hover {
                let r = app.settings.brush_radius;
                painter.circle_stroke(p, r, Stroke::new(3.0, Color32::from_black_alpha(120)));
                painter.circle_stroke(p, r, Stroke::new(1.5, Color32::from_white_alpha(220)));
                // Inner ring: where the feathered fade starts.
                let inner = r * (1.0 - app.settings.brush_feather.clamp(0.0, 1.0));
                if inner > 1.0 {
                    painter.circle_stroke(p, inner, Stroke::new(1.0, Color32::from_white_alpha(90)));
                }
            }
        }
        _ => {}
    }
}

fn hit_bone(p: &Pick) -> &str {
    match p {
        Pick::Bone(b) | Pick::Slot(_, b) => b,
    }
}

fn select(app: &mut EditorApp, hit: Option<Pick>) {
    match hit {
        Some(Pick::Bone(b)) => {
            app.selection.bone = Some(b);
            app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
            app.selection.slot = None;
        }
        Some(Pick::Slot(s, b)) => {
            app.selection.slot = Some(s);
            app.selection.bone = Some(b);
            app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
        }
        None => app.selection = Default::default(),
    }
}

fn parent_world(app: &EditorApp, pose: &Pose, name: &str) -> Affine {
    app.doc
        .bone(name)
        .and_then(|b| b.parent.as_deref())
        .and_then(|p| app.doc.bone_index(p))
        .map(|pi| pose.bones[pi].world)
        .unwrap_or(Affine::IDENTITY)
}

/// Animation offsets currently in effect, or setup values in setup mode.
fn current_values(app: &EditorApp, name: &str) -> (f32, [f32; 2], [f32; 2]) {
    match app.animation() {
        Some(anim) => {
            let t = app.playback.time;
            let tl = anim.bones.get(name);
            // Unkeyed changes (Auto Key off) win over the timeline.
            let p = app.pending_key(name);
            (
                p.and_then(|k| k.rotate).or_else(|| tl.and_then(|tl| sample(&tl.rotate, t))).unwrap_or(0.0),
                p.and_then(|k| k.translate).or_else(|| tl.and_then(|tl| sample(&tl.translate, t))).unwrap_or([0.0, 0.0]),
                p.and_then(|k| k.scale).or_else(|| tl.and_then(|tl| sample(&tl.scale, t))).unwrap_or([1.0, 1.0]),
            )
        }
        None => app.doc.bone(name).map_or((0.0, [0.0; 2], [1.0; 2]), |b| (b.rotation, [b.x, b.y], [b.scale_x, b.scale_y])),
    }
}

/// In the setup pose a selected slot's visible attachment is what the
/// transform tools move (click its bone to move the bone instead).
/// In an animation, picked mesh vertices are what they move (keyed as
/// deformation); otherwise the bone.
fn attachment_target(app: &EditorApp, pose: &Pose) -> Option<AttachmentDrag> {
    let name = app.selection.slot.as_deref().filter(|s| app.slot_pickable(s))?;
    let si = app.doc.slot_index(name)?;
    let slot = &app.doc.slots[si];
    let animating = app.playback.anim.is_some();
    let shown = pose.attachment(&app.doc, si)?;
    // A linked mesh deforms in an animation; its geometry belongs to the parent.
    if matches!(shown, Attachment::LinkedMesh(_)) && !animating { return None; }
    let materialized = app.doc.materialize_attachment(si, shown)?;
    let attachment = &materialized;
    let bone_world = pose.bones.get(app.doc.bone_index(&slot.bone)?)?.world;
    let vertices = match attachment {
        Attachment::Mesh(m) => picked_vertices(app, si, m),
        Attachment::Region(_) => Vec::new(),
        Attachment::LinkedMesh(_) => unreachable!(),
    };
    if animating && vertices.is_empty() {
        return None;
    }
    let (frame, deform) = match attachment {
        Attachment::Region(r) => (bone_world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y)), None),
        Attachment::Mesh(m) => {
            let pts = pose.deform_mesh(&app.doc, m, si);
            let moved: Vec<[f32; 2]> = if vertices.is_empty() { pts } else { vertices.iter().map(|&i| pts[i]).collect() };
            if moved.is_empty() {
                return None;
            }
            let n = moved.len() as f32;
            let [cx, cy] = moved.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
            let deform = if animating { Some((moved, DeformDrag::start(app, pose, si, m, &vertices)?)) } else { None };
            (Affine::from_trs(cx, cy, bone_world.rotation_deg(), 1.0, 1.0), deform)
        }
        Attachment::LinkedMesh(_) => unreachable!(),
    };
    Some(AttachmentDrag { slot: name.to_string(), attachment: attachment.clone(), bone_world, frame, vertices, deform })
}

/// The attachment changed by `v`, read as world-space deltas from where the
/// drag started: degrees about the frame origin, a translation, or scale
/// factors along the frame's axes.
fn attachment_op(a: &AttachmentDrag, v: &Vals) -> Option<Op> {
    let [ox, oy] = a.frame.apply(0.0, 0.0);
    let change = if let Some(r) = v.rotate {
        Affine::translate(ox, oy).mul(&Affine::from_trs(0.0, 0.0, r, 1.0, 1.0)).mul(&Affine::translate(-ox, -oy))
    } else if let Some([x, y]) = v.translate {
        Affine::translate(x, y)
    } else if let Some([sx, sy]) = v.scale {
        a.frame.mul(&Affine::scale(sx, sy)).mul(&a.frame.inverse()?)
    } else {
        return None;
    };
    let attachment = match &a.attachment {
        Attachment::Region(r) => {
            let world = change.mul(&a.bone_world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y)));
            let (x, y, rotation, scale_x, scale_y) = import::local_trs(&a.bone_world, &world);
            Attachment::Region(Region { x, y, rotation, scale_x, scale_y, ..r.clone() })
        }
        Attachment::Mesh(m) => {
            if let Some((points, d)) = &a.deform {
                let targets: Vec<[f32; 2]> = points.iter().map(|p| change.apply(p[0], p[1])).collect();
                return Some(d.op(&a.slot, &m.name, &targets));
            }
            let mut m = m.clone();
            for (i, vtx) in m.vertices.iter_mut().enumerate() {
                if a.vertices.is_empty() || a.vertices.binary_search(&i).is_ok() {
                    [vtx.x, vtx.y] = change.apply(vtx.x, vtx.y).map(round2);
                }
            }
            Attachment::Mesh(m)
        }
        Attachment::LinkedMesh(_) => return None,
    };
    Some(Op::SetAttachment { slot: a.slot.clone(), attachment })
}

fn current_shear(app: &EditorApp, name: &str) -> [f32; 2] {
    app.doc.bone(name).map_or([0.0; 2], |bone| {
        if let Some(anim) = app.animation() {
            app.pending_key(name).and_then(|k| k.shear)
                .or_else(|| anim.bones.get(name).and_then(|tl| sample(&tl.shear, app.playback.time))).unwrap_or([0.0; 2])
        } else { [bone.shear_x, bone.shear_y] }
    })
}

fn start_drag(app: &EditorApp, pose: &Pose, start: [f32; 2], handle: Option<Handle>, tool: Tool, alt: bool) -> Option<Drag> {
    let att = attachment_target(app, pose);
    let (name, w, parent_inv, mirrored, (rotate, translate, scale)) = match &att {
        // Attachment drags work in world deltas from the start.
        Some(a) => (a.slot.clone(), a.frame, Some(Affine::IDENTITY), false, (0.0, [0.0; 2], [1.0; 2])),
        None => {
            let name = app.selection.bone.clone()?;
            if !app.bone_pickable(&name) {
                return None;
            }
            let i = app.doc.bone_index(&name)?;
            let parent_world = parent_world(app, pose, &name);
            let values = current_values(app, &name);
            let inherits_reflection = matches!(app.doc.bones[i].inherit, skel2d_core::Inherit::Normal | skel2d_core::Inherit::NoScale);
            (name, pose.bones[i].world, parent_world.inverse(), inherits_reflection && parent_world.determinant() < 0.0, values)
        }
    };
    let unit = |x: f32, y: f32| {
        let [a, b] = w.apply_vector(x, y);
        let l = a.hypot(b);
        if l > 1e-6 { [a / l, b / l] } else { [x, y] }
    };

    let shear = current_shear(app, &name);
    let shearing = alt && tool == Tool::Scale && att.is_none();
    Some(Drag {
        base: app.doc.clone(),
        bone: name,
        tool,
        start,
        eff: start,
        last: start,
        axis: handle.map_or(Axis::Free, Handle::axis),
        handle,
        axes: [unit(1.0, 0.0), unit(0.0, 1.0)],
        anchor: w.apply(0.0, 0.0),
        parent_inv,
        mirrored,
        rotate,
        translate,
        scale,
        shear,
        shearing,
        att,
    })
}

struct Snap {
    on: bool,
    angle: f32,
    grid: f32,
}

fn snap_to(v: f32, step: f32) -> f32 {
    if step > 1e-6 { (v / step).round() * step } else { v }
}

#[derive(Default)]
struct Vals {
    rotate: Option<f32>,
    translate: Option<[f32; 2]>,
    scale: Option<[f32; 2]>,
    shear: Option<[f32; 2]>,
}

impl Vals {
    fn text(&self) -> String {
        match self {
            Vals { rotate: Some(r), .. } => crate::tf!("rot {}°", format!("{r:.1}")),
            Vals { translate: Some([x, y]), .. } => format!("x {x:.1} y {y:.1}"),
            Vals { scale: Some([x, y]), .. } => crate::tf!("scale {} × {}", format!("{x:.2}"), format!("{y:.2}")),
            Vals { shear: Some([x, y]), .. } => crate::tf!("shear {}° / {}°", format!("{x:.1}"), format!("{y:.1}")),
            _ => String::new(),
        }
    }
}

fn drag_values(d: &Drag, axis: Axis, snap: &Snap) -> Option<Vals> {
    let rel = |p: [f32; 2]| [p[0] - d.anchor[0], p[1] - d.anchor[1]];
    let (s, n) = (rel(d.start), rel(d.eff));
    let mut v = Vals::default();
    match d.tool {
        Tool::Select | Tool::Bone | Tool::Mesh | Tool::Weights => return None,
        Tool::Rotate => {
            let mut delta = (n[1].atan2(n[0]) - s[1].atan2(s[0])).to_degrees();
            delta = (delta + 540.0).rem_euclid(360.0) - 180.0;
            if d.mirrored {
                delta = -delta;
            }
            let r = d.rotate + delta;
            v.rotate = Some(round2(if snap.on { snap_to(r, snap.angle) } else { r }));
        }
        Tool::Translate => {
            let inv = d.parent_inv?;
            let mut delta = [d.eff[0] - d.start[0], d.eff[1] - d.start[1]];
            match axis {
                Axis::X => delta[1] = 0.0,
                Axis::Y => delta[0] = 0.0,
                Axis::Free => {}
            }
            if snap.on && axis != Axis::Free {
                let i = (axis == Axis::Y) as usize;
                delta[i] = snap_to(delta[i], snap.grid);
            }
            let [dx, dy] = inv.apply_vector(delta[0], delta[1]);
            let mut t = [d.translate[0] + dx, d.translate[1] + dy];
            if snap.on && axis == Axis::Free {
                t = t.map(|c| snap_to(c, snap.grid));
            }
            v.translate = Some(t.map(round2));
        }
        Tool::Scale => {
            // Spine과 다름: Alt+Scale drags shear a bone. X skews the X
            // axis; Y/free skews the Y axis. Attachments keep normal scaling.
            if d.shearing {
                let delta = (n[1].atan2(n[0]) - s[1].atan2(s[0])).to_degrees();
                let delta = (delta + 540.0).rem_euclid(360.0) - 180.0;
                let index = if axis == Axis::X { 0 } else { 1 };
                let mut shear = d.shear;
                shear[index] += if d.mirrored { -delta } else { delta };
                if snap.on { shear[index] = snap_to(shear[index], snap.angle); }
                v.shear = Some(shear.map(round2));
                return Some(v);
            }
            let sn = |c: f32| round3(if snap.on { snap_to(c, 0.05) } else { c });
            let f = match axis {
                Axis::Free => {
                    let start_len = s[0].hypot(s[1]);
                    if start_len < 1e-3 {
                        return None;
                    }
                    n[0].hypot(n[1]) / start_len
                }
                Axis::X | Axis::Y => {
                    let u = d.axes[(axis == Axis::Y) as usize];
                    let sp = s[0] * u[0] + s[1] * u[1];
                    if sp.abs() < 1e-3 {
                        return None;
                    }
                    (n[0] * u[0] + n[1] * u[1]) / sp
                }
            };
            v.scale = Some(match axis {
                Axis::Free => [sn(d.scale[0] * f), sn(d.scale[1] * f)],
                Axis::X => [sn(d.scale[0] * f), d.scale[1]],
                Axis::Y => [d.scale[0], sn(d.scale[1] * f)],
            });
        }
    }
    Some(v)
}

fn vals_op(app: &EditorApp, bone: &str, v: &Vals) -> Op {
    match &app.playback.anim {
        Some(anim) => Op::KeyBone(KeyBone {
            animation: anim.clone(),
            bone: bone.to_string(),
            time: app.playback.time,
            rotate: v.rotate,
            translate: v.translate,
            scale: v.scale,
            shear: v.shear,
            curve: None,
            curves: None,
        }),
        None => Op::SetBone(BonePatch {
            name: bone.to_string(),
            rotation: v.rotate,
            x: v.translate.map(|t| t[0]),
            y: v.translate.map(|t| t[1]),
            scale_x: v.scale.map(|s| s[0]),
            scale_y: v.scale.map(|s| s[1]),
            shear_x: v.shear.map(|s| s[0]),
            shear_y: v.shear.map(|s| s[1]),
            ..Default::default()
        }),
    }
}

/// Arrow keys move/rotate/scale the selected bone; a burst is one undo step.
fn nudge(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.tool.transforms() {
        return;
    }
    let (presses, shift, cmd) = ctx.input(|i| {
        let n = |k: Key| i.events.iter().filter(|e| matches!(e, egui::Event::Key { key, pressed: true, .. } if *key == k)).count() as f32;
        ([n(Key::ArrowRight) - n(Key::ArrowLeft), n(Key::ArrowUp) - n(Key::ArrowDown)], i.modifiers.shift, i.modifiers.command || i.modifiers.alt)
    });
    if cmd || presses == [0.0, 0.0] {
        return;
    }
    let pose = app.pose();
    let big = if shift { 10.0 } else { 1.0 };
    if let Some(a) = attachment_target(app, &pose) {
        let steps = (presses[0] + presses[1]) * big;
        let v = match app.tool {
            Tool::Translate => Vals { translate: Some(presses.map(|p| p * big)), ..Default::default() },
            Tool::Rotate => Vals { rotate: Some(steps), ..Default::default() },
            Tool::Scale => Vals { scale: Some([1.0 + steps * 0.01; 2]), ..Default::default() },
            Tool::Select | Tool::Bone | Tool::Mesh | Tool::Weights => return,
        };
        if let Some(op) = attachment_op(&a, &v) {
            app.edit_coalesced(format!("nudge:{}:{:?}", a.slot, app.tool), vec![op]);
        }
        return;
    }
    let Some(name) = app.selection.bone.clone().filter(|b| app.bone_pickable(b)) else { return };
    let (rot, tr, sc) = current_values(app, &name);
    let v = match app.tool {
        Tool::Translate => {
            let Some(inv) = parent_world(app, &pose, &name).inverse() else { return };
            let [dx, dy] = inv.apply_vector(presses[0] * big, presses[1] * big);
            Vals { translate: Some([round2(tr[0] + dx), round2(tr[1] + dy)]), ..Default::default() }
        }
        Tool::Rotate => {
            let mut delta = (presses[0] + presses[1]) * big;
            if parent_world(app, &pose, &name).determinant() < 0.0 {
                delta = -delta;
            }
            Vals { rotate: Some(round2(rot + delta)), ..Default::default() }
        }
        Tool::Scale => {
            let d = (presses[0] + presses[1]) * 0.01 * big;
            Vals { scale: Some([round3(sc[0] + d), round3(sc[1] + d)]), ..Default::default() }
        }
        Tool::Select | Tool::Bone | Tool::Mesh | Tool::Weights => return,
    };
    let op = vals_op(app, &name, &v);
    app.edit_coalesced(format!("nudge:{name}:{:?}", app.tool), vec![op]);
}

enum Act {
    AddChild,
    AddRoot,
    Key,
    Reset,
    Frame,
    FrameAll,
    Hide,
    ShowAll,
    Tool(Tool),
    SlotOrder(usize),
}

/// Right-click picks what is under the cursor and opens a menu for it: the
/// bone or slot there, else the empty canvas. Items that cannot apply are
/// left out rather than greyed, so the menu stays short.
fn context_menu(app: &mut EditorApp, response: &egui::Response, rect: Rect, pose: &Pose) {
    if app.tool == Tool::Mesh && app.playback.anim.is_none() {
        if response.secondary_clicked() {
            app.view.mesh_mode=app.view.mesh_mode.next();
            app.view.edge_drag=None;
        }
        return;
    }
    if response.secondary_clicked()
        && let Some(p) = response.interact_pointer_pos()
    {
        app.view.menu_at = app.camera.to_world(rect, p);
        let hit = pick(app, pose, rect, p);
        if hit.is_some() {
            select(app, hit);
        }
    }
    let mut act = None;
    let sel = app.selection.bone.clone();
    let slot = app.selection.slot.clone();
    let animating = app.playback.anim.is_some();
    let (prev, cur) = (app.prev_tool, app.tool);
    let tr = crate::i18n::tr;
    let slot_index = slot.as_deref().and_then(|s| app.doc.slot_index(s));
    let slots = app.doc.slots.len();
    let anything_hidden = !(app.hidden_bones.is_empty() && app.hidden_slots.is_empty());
    response.context_menu(|ui| {
        let mut item = |ui: &mut egui::Ui, label: String, a: Act| {
            if ui.button(label).clicked() {
                act = Some(a);
            }
        };
        if let (Some(s), Some(i)) = (&slot, slot_index) {
            ui.label(egui::RichText::new(crate::tf!("Slot {}", s)).color(theme::SLOT).small());
            if i + 1 < slots {
                item(ui, tr("Bring to front").into(), Act::SlotOrder(slots - 1));
            }
            if i > 0 {
                item(ui, tr("Send to back").into(), Act::SlotOrder(0));
            }
            item(ui, format!("{}    M", tr("Edit mesh")), Act::Tool(Tool::Mesh));
            item(ui, format!("{}    G", tr("Paint weights")), Act::Tool(Tool::Weights));
            ui.separator();
        }
        if let Some(b) = &sel {
            ui.label(egui::RichText::new(crate::tf!("Bone {}", b)).color(app.doc.bone(b).map_or(theme::BONE, theme::bone_text_color)).small());
            if !animating {
                item(ui, tr("Add child bone here").into(), Act::AddChild);
            }
            if animating {
                item(ui, format!("{}    K", tr("Key edited")), Act::Key);
            }
            item(ui, tr("Reset bone pose").into(), Act::Reset);
            item(ui, format!("{}    F", tr("Frame selection")), Act::Frame);
            item(ui, tr("Hide bone").into(), Act::Hide);
            ui.separator();
        }
        if !animating {
            item(ui, tr("Add bone here (root)").into(), Act::AddRoot);
        }
        item(ui, format!("{}    Shift+F", tr("Frame all")), Act::FrameAll);
        if anything_hidden {
            item(ui, tr("Show hidden bones and slots").into(), Act::ShowAll);
        }
        ui.separator();
        if prev != cur {
            item(ui, crate::tf!("Back to {}", tr(prev.label())), Act::Tool(prev));
        }
        ui.menu_button(tr("Tool"), |ui| {
            for (t, label, _) in Tool::ALL {
                if ui.button(format!("{}    {}", tr(label.split(' ').next().unwrap_or(label)), label.split(['(', ')']).nth(1).unwrap_or(""))).clicked() {
                    act = Some(Act::Tool(t));
                }
            }
        });
        if act.is_some() {
            ui.close();
        }
    });
    let Some(act) = act else { return };
    match act {
        Act::AddChild | Act::AddRoot => {
            let at = app.view.menu_at.map(round2);
            let mut name = "bone".to_string();
            let mut n = 1;
            while app.doc.bone(&name).is_some() {
                n += 1;
                name = format!("bone_{n}");
            }
            let parent = if matches!(act, Act::AddChild) { sel } else { None };
            if app.edit(vec![Op::AddBoneWorld { name: name.clone(), parent, start: at, end: None }]) {
                select(app, Some(Pick::Bone(name)));
            }
        }
        Act::Key => {
            crate::timeline::key_edited(app);
        }
        Act::Reset => {
            let Some(name) = sel else { return };
            let v = if animating {
                Vals { rotate: Some(0.0), translate: Some([0.0; 2]), scale: Some([1.0; 2]), shear: Some([0.0; 2]) }
            } else {
                Vals { rotate: Some(0.0), scale: Some([1.0; 2]), ..Default::default() }
            };
            let op = vals_op(app, &name, &v);
            app.edit(vec![op]);
        }
        Act::Frame => frame_selection(app, rect),
        Act::FrameAll => app.camera.needs_fit = true,
        Act::Hide => {
            if let Some(b) = sel {
                app.hidden_bones.insert(b);
            }
        }
        Act::ShowAll => {
            app.hidden_bones.clear();
            app.hidden_slots.clear();
        }
        Act::Tool(t) => app.set_tool(t),
        Act::SlotOrder(index) => {
            if let Some(name) = slot {
                app.edit(vec![Op::MoveSlot { name, index }]);
            }
        }
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Smallest 1-2-5 step that is at least `x`.
fn nice(x: f32) -> f32 {
    let e = 10f32.powf(x.log10().floor());
    let m = x / e;
    e * if m <= 1.0 { 1.0 } else if m <= 2.0 { 2.0 } else if m <= 5.0 { 5.0 } else { 10.0 }
}

fn draw_grid(app: &EditorApp, painter: &egui::Painter, rect: Rect) {
    let cam = &app.camera;
    let minor = nice(20.0 / cam.zoom);
    let major = nice(minor * 4.0);
    let ratio = (major / minor).round().max(1.0) as i64;
    let decimals = if major >= 1.0 { 0 } else { (-major.log10().floor()) as usize };
    let ([x0, y1], [x1, y0]) = (cam.to_world(rect, rect.left_top()), cam.to_world(rect, rect.right_bottom()));
    let font = FontId::proportional(12.0);
    let dim = theme::TEXT_DIM;
    let fmt = |v: f32| format!("{:.*}", decimals, v + 0.0);
    let range = |a: f32, b: f32| ((a / minor).floor() as i64)..=((b / minor).ceil() as i64);
    for i in range(x0, x1).take(600) {
        let x = i as f32 * minor;
        let sx = cam.to_screen(rect, [x, 0.0]).x.round() + 0.5;
        let is_major = i % ratio == 0;
        painter.line_segment([pos2(sx, rect.top()), pos2(sx, rect.bottom())], Stroke::new(1.0, if is_major { theme::GRID_MAJOR } else { theme::GRID_MINOR }));
        if is_major {
            painter.text(pos2(sx + 3.0, rect.top() + 2.0), Align2::LEFT_TOP, fmt(x), font.clone(), dim);
        }
    }
    for i in range(y0, y1).take(600) {
        let y = i as f32 * minor;
        let sy = cam.to_screen(rect, [0.0, y]).y.round() + 0.5;
        let is_major = i % ratio == 0;
        painter.line_segment([pos2(rect.left(), sy), pos2(rect.right(), sy)], Stroke::new(1.0, if is_major { theme::GRID_MAJOR } else { theme::GRID_MINOR }));
        if is_major {
            painter.text(pos2(rect.left() + 3.0, sy - 1.0), Align2::LEFT_BOTTOM, fmt(y), font.clone(), dim);
        }
    }
}

fn draw_axes(app: &EditorApp, painter: &egui::Painter, rect: Rect) {
    let o = app.camera.to_screen(rect, [0.0, 0.0]);
    let stroke = Stroke::new(1.5, theme::AXIS);
    painter.line_segment([pos2(rect.left(), o.y), pos2(rect.right(), o.y)], stroke);
    painter.line_segment([pos2(o.x, rect.top()), pos2(o.x, rect.bottom())], stroke);
}

fn color32(c: skel2d_core::Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Pose at another clock value, with the same tracks as `EditorApp::pose`.
fn pose_at(app: &EditorApp, clock: f32) -> Pose {
    let tracks = app.playback.tracks(&app.doc, clock);
    Pose::compute_skinned(&app.doc, &tracks, &app.skins)
}

/// Ghost poses before (blue) and after (green) the playhead: thin bones and silhouettes.
fn draw_onion(app: &EditorApp, painter: &egui::Painter, rect: Rect) {
    let Some(anim) = app.animation() else { return };
    let (t, fps, dur) = (app.playback.time, app.settings.fps.max(1.0), anim.duration());
    for k in (1..=app.settings.onion_frames.clamp(0, 8)).rev() {
        let alpha = (150 / k).clamp(30, 150) as u8;
        for (sign, [r, g, b]) in [(-1.0, [90u8, 140, 255]), (1.0, [110, 220, 130])] {
            let time = t + sign * k as f32 / fps;
            if time < 0.0 || time > dur {
                continue;
            }
            let color = Color32::from_rgba_unmultiplied(r, g, b, alpha);
            let pose = pose_at(app, time);
            for shape in slot_shapes(&app.doc, &pose) {
                if app.hidden_slots.contains(&app.doc.slots[shape.slot].name) {
                    continue;
                }
                let pts: Vec<Pos2> = shape.pts.iter().map(|&c| app.camera.to_screen(rect, c)).collect();
                for [a, b] in boundary_edges(&shape.tris) {
                    painter.line_segment([pts[a], pts[b]], Stroke::new(1.0, color));
                }
            }
            for (bone, bp) in app.doc.bones.iter().zip(&pose.bones) {
                if app.hidden_bones.contains(&bone.name) {
                    continue;
                }
                let o = app.camera.to_screen(rect, bp.world.apply(0.0, 0.0));
                let e = app.camera.to_screen(rect, bp.world.apply(bone.length, 0.0));
                painter.line_segment([o, e], Stroke::new(1.5, color));
            }
        }
    }
}

fn draw_slots(app: &mut EditorApp, ctx: &egui::Context, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let sel = theme::ACCENT;
    let hovered = match &app.view.hover {
        Some(Pick::Slot(s, _)) => Some(s.clone()),
        _ => None,
    };
    // Mesh and Weights tools on a selected mesh: optional isolation and
    // dimming, hull edges in cyan and inner triangle edges in gray.
    let mesh_view = matches!(app.tool, Tool::Mesh | Tool::Weights) && selected_mesh(app, pose).is_some();
    let show_triangles = app.settings.mesh_triangles;
    for shape in slot_shapes(&app.doc, pose) {
        let slot = app.doc.slots[shape.slot].clone();
        if app.hidden_slots.contains(&slot.name) {
            continue;
        }
        let selected = app.selection.slot.as_deref() == Some(slot.name.as_str());
        if mesh_view && app.settings.mesh_isolate && !selected {
            continue;
        }
        let mut tint = color32(pose.slots[shape.slot].color);
        if app.locked_slots.contains(&slot.name) {
            tint = tint.gamma_multiply(0.5);
        }
        if mesh_view && app.settings.mesh_dim && selected {
            tint = tint.gamma_multiply(0.4);
        }
        let effective = pose.attachment(&app.doc, shape.slot).and_then(|a| app.doc.materialize_attachment(shape.slot, a));
        let image = match effective.as_ref() {
            Some(Attachment::Region(r)) => r.image.clone(),
            Some(Attachment::Mesh(m)) => m.image.clone(),
            _ => None,
        };
        let pts: Vec<Pos2> = shape.pts.iter().map(|&c| app.camera.to_screen(rect, c)).collect();
        let constraints = match effective.as_ref() { Some(Attachment::Mesh(m))=>m.edges.clone(), _=>Vec::new() };
        let selected_constraint=app.view.edge_selection.as_ref().filter(|(s,a,k,_)| {
            s==&slot.name && pose.attachment(&app.doc,shape.slot).is_some_and(|m|m.name()==a) && k==&app.skin_of(s,a)
        }).map(|(_,_,_,e)|*e);
        let edges = |painter: &egui::Painter, stroke: Stroke| {
            if mesh_view && selected {
                if show_triangles {
                    for [a, b] in unique_edges(&shape.tris) {
                        painter.line_segment([pts[a], pts[b]], Stroke::new(theme::MESH_TRIANGLE_STROKE, theme::TEXT_DIM.gamma_multiply(0.5)));
                    }
                }
                for [a, b] in boundary_edges(&shape.tris) {
                    painter.line_segment([pts[a], pts[b]], Stroke::new(theme::MESH_HULL_STROKE, theme::HULL));
                }
                    for e in &constraints {
                        let active=selected_constraint==Some(*e);
                        painter.line_segment([pts[e[0] as usize],pts[e[1] as usize]], Stroke::new(theme::MESH_EDGE_STROKE, if active {theme::TEXT} else {theme::KEY_CHANGED}));
                    }
                return;
            }
            for [a, b] in unique_edges(&shape.tris) {
                painter.line_segment([pts[a], pts[b]], stroke);
            }
        };
        let hover_outline = |painter: &egui::Painter| {
            if hovered.as_deref() == Some(slot.name.as_str()) && !selected {
                for [a, b] in boundary_edges(&shape.tris) {
                    painter.line_segment([pts[a], pts[b]], Stroke::new(2.0, Color32::from_white_alpha(200)));
                }
            }
        };
        let effective_image = image.clone();
        match image.and_then(|img| app.texture(ctx, &img)) {
            Some(tex) => {
                let mut mesh = Mesh::with_texture(tex.id());
                for (p, uv) in pts.iter().zip(&shape.uvs) {
                    mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: pos2(uv[0], uv[1]), color: tint });
                }
                for t in &shape.tris {
                    mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
                }
                let pixels = if slot.blend.is_normal() || !app.blend_pass { None } else { effective_image.as_deref().and_then(|img| app.blend_pixels(img, tex.id())) };
                match pixels {
                    Some(pixels) => { painter.add(crate::blendpass::shape(rect, slot.blend, tex.id(), pixels, &mesh)); }
                    None => { painter.add(Shape::mesh(mesh)); }
                }
                hover_outline(painter);
                if selected {
                    if shape.mesh {
                        edges(painter, Stroke::new(1.5, sel.gamma_multiply(0.8)));
                    } else {
                        painter.extend(outline_shapes(&pts, Stroke::new(2.5, sel)));
                    }
                }
            }
            None if shape.mesh => {
                let mut mesh = Mesh::default();
                for p in &pts {
                    mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: egui::epaint::WHITE_UV, color: tint.gamma_multiply(0.55) });
                }
                for t in &shape.tris {
                    mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
                }
                painter.add(Shape::mesh(mesh));
                edges(painter, if selected { Stroke::new(1.0, sel) } else { Stroke::new(0.5, tint) });
                hover_outline(painter);
            }
            None => {
                let fill = tint.gamma_multiply(0.55);
                let outline = if selected { sel } else { tint };
                painter.extend(polygon_shapes(&pts, fill, Stroke::new(if selected { 2.5 } else { 1.5 }, outline)));
                hover_outline(painter);
            }
        }
    }
}

/// Vertex dots of the selected mesh, colored by the selected bone's weight.
fn draw_vertices(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let Some((si, m)) = selected_mesh(app, pose) else { return };
    let slot_bone = &app.doc.slots[si].bone;
    let bone = app.selection.bone.as_deref();
    let picked = picked_vertices(app, si, &m);
    let all_bones = app.tool == Tool::Weights && app.settings.weights_all_bones;
    for (i, (v, d)) in m.vertices.iter().zip(vertex_dots(app, pose, rect)).enumerate() {
        let fill = match bone {
            _ if all_bones => blended_bone_color(v, slot_bone),
            Some(b) => weight_color(bone_weight(v, b, slot_bone)),
            None => Color32::WHITE,
        };
        if picked.binary_search(&i).is_ok() {
            painter.circle(d, 6.0, fill, Stroke::new(2.5, theme::ACCENT));
        } else {
            painter.circle(d, 4.5, fill, Stroke::new(1.5, Color32::BLACK));
        }
    }
    if let Some([a, b]) = app.view.vertex_box {
        let r = Rect::from_two_pos(a, b);
        painter.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.12));
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    }
}

/// Above this, a rig is drawn quietly: full bones would cover the artwork.
const BUSY_RIG_BONES: usize = 30;

/// Steps about the artwork draw every bone that is not being worked on faintly,
/// and so do the others on a busy rig.
fn bone_is_faint(step: crate::steps::Step, busy: bool, bound: bool) -> bool {
    use crate::steps::Step;
    match step {
        Step::Images | Step::Mesh => true,
        // Weights are painted per bone: the mesh's own bones stay readable.
        Step::Weights => !bound,
        Step::Bones | Step::Animate => busy,
    }
}

/// Whether physics marks are drawn for every constraint, not just the selected one.
fn rig_marks_shown(step: crate::steps::Step, bones: usize) -> bool {
    use crate::steps::Step;
    matches!(step, Step::Bones | Step::Animate) && bones <= BUSY_RIG_BONES
}

/// Bones of the selected mesh.
fn bound_bones(app: &EditorApp, pose: &Pose) -> std::collections::HashSet<String> {
    let Some((si, mesh)) = selected_mesh(app, pose) else { return Default::default() };
    mesh.bones.iter().cloned().chain(mesh.vertices.iter().flat_map(|v| v.weights.iter().map(|w| w.bone.clone()))).chain([app.doc.slots[si].bone.clone()]).collect()
}

/// A library glyph at a fixed screen size on a dark disc ringed in `color`, so it
/// reads on bright and dark artwork alike.
fn glyph_marker(painter: &egui::Painter, icon: crate::icons::Icon, at: Pos2, size: f32, color: Color32, ring: Stroke) {
    painter.circle(at, size * 0.62, Color32::from_black_alpha(205), ring);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(crate::icons::texture_id(icon), Rect::from_center_size(at, Vec2::splat(size)), uv, color);
}

/// A zero-length bone with no constraint of its own: a ring with a crosshair, not a dot.
fn target_marker(painter: &egui::Painter, o: Pos2, r: f32, color: Color32, line: Stroke) {
    let halo = Stroke::new(line.width + 1.5, line.color);
    for (a, b) in [(vec2(-r - 3.0, 0.0), vec2(r + 3.0, 0.0)), (vec2(0.0, -r - 3.0), vec2(0.0, r + 3.0))] {
        painter.line_segment([o + a, o + b], halo);
    }
    painter.circle(o, r, Color32::from_black_alpha(160), Stroke::new(line.width + 1.5, line.color));
    for (a, b) in [(vec2(-r - 3.0, 0.0), vec2(r + 3.0, 0.0)), (vec2(0.0, -r - 3.0), vec2(0.0, r + 3.0))] {
        painter.line_segment([o + a, o + b], Stroke::new(1.5, color));
    }
    painter.circle_stroke(o, r, Stroke::new(1.5, color));
}

fn draw_bones(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let ik_targets: std::collections::HashSet<&str> = app.doc.ik.iter().map(|c| c.target.as_str()).collect();
    let outline = Stroke::new(2.0, Color32::from_black_alpha(235));
    let is_selected = |name: &str| app.selection.bone.as_deref() == Some(name);
    let hovered = match &app.view.hover {
        Some(Pick::Bone(b)) => Some(b.as_str()),
        _ => None,
    };
    // Selected last, then hovered, so overlapping bones never hide them.
    let mut order: Vec<usize> = (0..app.doc.bones.len()).filter(|&i| !app.hidden_bones.contains(&app.doc.bones[i].name)).collect();
    order.sort_by_key(|&i| {
        let n = app.doc.bones[i].name.as_str();
        (is_selected(n), hovered == Some(n))
    });
    let bound = bound_bones(app, pose);
    let busy = app.doc.bones.len() > BUSY_RIG_BONES;
    for i in order {
        let (bone, bp) = (&app.doc.bones[i], &pose.bones[i]);
        let (selected, hover) = (is_selected(&bone.name), hovered == Some(bone.name.as_str()));
        if !selected && !hover && bone_is_faint(app.step, busy, bound.contains(bone.name.as_str())) {
            let o = app.camera.to_screen(rect, bp.world.apply(0.0, 0.0));
            let t = app.camera.to_screen(rect, bp.world.apply(bone.length, 0.0));
            // A dark edge around a lightly filled shape reads on bright and dark
            // artwork alike without covering it.
            let color = bone.color.map_or(theme::BONE, color32);
            let opacity = bone.color.map_or(1.0, |c| c.a);
            let (edge, fill) = (Stroke::new(1.5, color), color.gamma_multiply(0.35));
            let halo = Stroke::new(3.5, Color32::from_black_alpha(170).gamma_multiply(opacity));
            let len = o.distance(t);
            if len > 1.0 {
                let dir = (t - o) / len;
                let side = vec2(-dir.y, dir.x) * (len * 0.08).clamp(2.0, 5.0);
                let mid = o + dir * len * 0.15;
                let poly = [o, mid + side, t, mid - side];
                painter.extend(outline_shapes(&poly, halo));
                painter.extend(polygon_shapes(&poly, fill, edge));
            } else if ik_targets.contains(bone.name.as_str()) {
                glyph_marker(painter, crate::icons::Icon::Constraint, o, 10.0, color, Stroke::new(1.0, color));
            } else {
                target_marker(painter, o, 2.5, color, Stroke::new(1.0, Color32::from_black_alpha(170)));
            }
            if app.settings.show_bone_names {
                let at = if o.distance(t) > 1.0 { o + (t - o) * 0.5 + vec2(0.0, -8.0) } else { o + vec2(6.0, -6.0) };
                label(painter, at, Align2::CENTER_BOTTOM, &bone.name, theme::SIZE_SMALL, theme::TEXT);
            }
            continue;
        }
        let mut fill = if selected {
            theme::ACCENT
        } else if hover {
            Color32::from_rgb(255, 232, 150)
        } else {
            bone.color.map_or(theme::BONE, color32)
        };
        let mut line = outline;
        // The picked color's alpha fades the whole bone, not only its fill.
        if !selected && !hover && let Some(c) = bone.color {
            line.color = line.color.gamma_multiply(c.a);
        }
        if app.locked_bones.contains(&bone.name) {
            fill = fill.gamma_multiply(0.4);
            line.color = line.color.gamma_multiply(0.4);
        }
        let o = app.camera.to_screen(rect, bp.world.apply(0.0, 0.0));
        let t = app.camera.to_screen(rect, bp.world.apply(bone.length, 0.0));
        let len = o.distance(t);
        if len > 1.0 {
            let dir = (t - o) / len;
            let half_w = (len * 0.08).clamp(2.0, 7.0);
            let mid = o + dir * len * 0.15;
            let side = vec2(-dir.y, dir.x) * half_w;
            let poly = vec![o, mid + side, t, mid - side];
            if selected {
                painter.extend(outline_shapes(&poly, Stroke::new(7.0, theme::ACCENT.gamma_multiply(0.4))));
                line = Stroke::new(2.0, Color32::WHITE);
            }
            painter.extend(polygon_shapes(&poly, fill, line));
        } else if selected {
            painter.circle_stroke(o, 7.0, Stroke::new(4.0, theme::ACCENT.gamma_multiply(0.5)));
        }
        if ik_targets.contains(bone.name.as_str()) {
            // The goal of an IK constraint: the constraint glyph, in the bone's color.
            glyph_marker(painter, crate::icons::Icon::Constraint, o, 15.0, fill, line);
        } else if len <= 1.0 {
            target_marker(painter, o, 4.5, fill, line);
        } else {
            painter.circle(o, 3.5, fill, line);
        }
        if selected || hover || app.settings.show_bone_names {
            let at = if len > 1.0 { o + (t - o) * 0.5 + vec2(0.0, -8.0) } else { o + vec2(6.0, -6.0) };
            label(painter, at, Align2::CENTER_BOTTOM, &bone.name, theme::SIZE_SMALL, if selected { theme::ACCENT } else { theme::TEXT });
        }
    }
}

/// Screen positions of a bone's origin and tip.
fn bone_ends(app: &EditorApp, rect: Rect, pose: &Pose, i: usize) -> (Pos2, Pos2) {
    let (bone, bp) = (&app.doc.bones[i], &pose.bones[i]);
    (app.camera.to_screen(rect, bp.world.apply(0.0, 0.0)), app.camera.to_screen(rect, bp.world.apply(bone.length, 0.0)))
}

/// A faint copy of every simulated bone where the plain animated pose would
/// put it, while the simulation moves it away. Drawn under the bones.
fn draw_physics_ghosts(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    if app.doc.physics.is_empty() || !app.physics.enabled { return; }
    let tracks = app.playback.tracks(&app.doc, app.playback.time);
    let plain = Pose::compute_skinned(&app.doc, &tracks, &app.skins);
    for c in &app.doc.physics {
        let Some(i) = app.doc.bone_index(&c.bone) else { continue };
        if app.hidden_bones.contains(&c.bone) || i >= plain.bones.len() { continue; }
        let (o, t) = bone_ends(app, rect, pose, i);
        let (go, gt) = bone_ends(app, rect, &plain, i);
        if o.distance(go) < 0.5 && t.distance(gt) < 0.5 { continue; }
        let len = go.distance(gt);
        if len <= 1.0 { continue; }
        let dir = (gt - go) / len;
        let half_w = (len * 0.08).clamp(2.0, 7.0);
        let mid = go + dir * len * 0.15;
        let side = vec2(-dir.y, dir.x) * half_w;
        let poly = vec![go, mid + side, gt, mid - side];
        painter.extend(polygon_shapes(&poly, theme::PHYSICS_GHOST, Stroke::new(1.0, theme::PHYSICS.gamma_multiply(0.35))));
    }
}

/// A small wave on every bone that has a physics constraint, and, for the
/// selected constraint, arrows for the axes it moves.
fn draw_physics_marks(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let stroke = |w: f32| Stroke::new(w, theme::PHYSICS);
    let shadow = Stroke::new(3.5, Color32::from_black_alpha(150));
    let seg = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], shadow);
        painter.line_segment([a, b], stroke(1.6));
    };
    let all = rig_marks_shown(app.step, app.doc.bones.len());
    for c in &app.doc.physics {
        let Some(i) = app.doc.bone_index(&c.bone) else { continue };
        if app.hidden_bones.contains(&c.bone) { continue; }
        if !all && app.selection.physics.as_deref() != Some(c.name.as_str()) && app.selection.bone.as_deref() != Some(c.bone.as_str()) { continue; }
        let (o, t) = bone_ends(app, rect, pose, i);
        let len = o.distance(t);
        let at = if len > 1.0 { o + (t - o) * 0.5 + vec2(-dirn(o, t).y, dirn(o, t).x) * ((len * 0.08).clamp(2.0, 7.0) + 8.0) } else { o + vec2(8.0, -8.0) };
        glyph_marker(painter, crate::icons::Icon::Physics, at, 14.0, theme::PHYSICS, stroke(1.2));
        // The selected constraint also shows what it moves.
        if app.selection.physics.as_deref() != Some(c.name.as_str()) { continue; }
        let arrow = |dir: Vec2, amount: f32| {
            if amount <= 0.0 { return; }
            let end = o + dir * (16.0 + 30.0 * amount.min(1.0));
            let side = vec2(-dir.y, dir.x);
            seg(o + dir * 6.0, end);
            seg(end, end - dir * 7.0 + side * 4.0);
            seg(end, end - dir * 7.0 - side * 4.0);
        };
        arrow(vec2(1.0, 0.0), c.x);
        arrow(vec2(0.0, -1.0), c.y);
        if c.rotate > 0.0 && len > 1.0 {
            let d = dirn(o, t);
            let r = 10.0 + 10.0 * c.rotate.min(1.0);
            let a0 = d.y.atan2(d.x);
            let arc: Vec<Pos2> = (0..=6).map(|k| { let a = a0 - 0.7 + 1.4 * k as f32 / 6.0; t + vec2(a.cos(), a.sin()) * r }).collect();
            for w in arc.windows(2) { seg(w[0], w[1]); }
        }
        if c.scale_x > 0.0 && len > 1.0 {
            let d = dirn(o, t);
            let n = vec2(-d.y, d.x) * 4.0;
            for k in [0.0_f32, 1.0] {
                let p = o + (t - o) * (0.35 + 0.3 * k);
                seg(p - n, p + n);
            }
        }
        if c.shear_x > 0.0 && len > 1.0 {
            let d = dirn(o, t);
            let n = vec2(-d.y, d.x);
            seg(t + n * 8.0 - d * 4.0, t + n * 8.0 + d * 4.0);
            seg(t - n * 8.0 - d * 4.0, t - n * 8.0 + d * 4.0);
        }
    }
}

fn dirn(a: Pos2, b: Pos2) -> Vec2 {
    let v = b - a;
    if v.length() > 0.0 { v / v.length() } else { vec2(1.0, 0.0) }
}

fn draw_gizmo(app: &EditorApp, painter: &egui::Painter, rect: Rect, pose: &Pose) {
    let Some(g) = gizmo(app, pose, rect) else { return };
    let active = app.drag.as_ref().and_then(|d| d.handle);
    let hot = |h: Handle| active == Some(h) || (active.is_none() && app.view.hover_handle == Some(h));
    let width = |h: Handle| if hot(h) { 3.5 } else { 2.0 };
    let tint = |c: Color32, h: Handle| if hot(h) { c.lerp_to_gamma(Color32::WHITE, 0.45) } else { c };
    let shadow = |a: Pos2, b: Pos2, w: f32| painter.line_segment([a, b], Stroke::new(w + 2.0, Color32::from_black_alpha(120)));
    let arm = |dir: Vec2, c: Color32, h: Handle, square: bool| {
        let end = g.o + dir * ARM;
        let c = tint(c, h);
        shadow(g.o, end, width(h));
        painter.line_segment([g.o, end], Stroke::new(width(h), c));
        if square {
            painter.rect_filled(Rect::from_center_size(end, Vec2::splat(9.0)), 1.0, c);
        } else {
            let side = vec2(-dir.y, dir.x);
            painter.add(Shape::convex_polygon(vec![end + dir * 9.0, end + side * 5.0, end - side * 5.0], c, Stroke::NONE));
        }
    };
    let center = |h: Handle| {
        let r = Rect::from_center_size(g.o, Vec2::splat(11.0));
        painter.rect_filled(r, 2.0, tint(Color32::from_gray(235), h).gamma_multiply(0.6));
        painter.rect_stroke(r, 2.0, Stroke::new(2.0, tint(Color32::WHITE, h)), StrokeKind::Inside);
    };
    match app.tool {
        Tool::Select | Tool::Bone | Tool::Mesh | Tool::Weights => {}
        Tool::Rotate => {
            let h = Handle::Ring;
            painter.circle_stroke(g.o, RING, Stroke::new(width(h) + 2.0, Color32::from_black_alpha(100)));
            painter.circle_stroke(g.o, RING, Stroke::new(width(h), tint(theme::ACCENT, h)));
            // The knob sits on the bone's direction.
            let knob = g.o + g.ex * RING;
            painter.circle(knob, if hot(h) { 7.0 } else { 5.5 }, tint(theme::ACCENT, h), Stroke::new(1.5, Color32::BLACK));
            if let Some(d) = &app.drag {
                let to = |p: [f32; 2]| app.camera.to_screen(rect, p);
                let o = to(d.anchor);
                painter.line_segment([o, to(d.start)], Stroke::new(1.0, Color32::from_white_alpha(70)));
                painter.line_segment([o, to(d.eff)], Stroke::new(2.0, theme::ACCENT));
            }
        }
        Tool::Translate => {
            arm(vec2(1.0, 0.0), AXIS_X, Handle::Move(Axis::X), false);
            arm(vec2(0.0, -1.0), AXIS_Y, Handle::Move(Axis::Y), false);
            center(Handle::Move(Axis::Free));
        }
        Tool::Scale => {
            arm(g.ex, AXIS_X, Handle::Scale(Axis::X), true);
            arm(g.ey, AXIS_Y, Handle::Scale(Axis::Y), true);
            center(Handle::Scale(Axis::Free));
        }
    }
}

#[cfg(test)]
mod rig_display_tests {
    use super::*;
    use crate::steps::Step;

    #[test]
    fn bones_quiet_down_by_step_and_rig_size() {
        for step in [Step::Images, Step::Mesh] {
            assert!(bone_is_faint(step, false, true));
        }
        assert!(bone_is_faint(Step::Weights, false, false));
        assert!(!bone_is_faint(Step::Weights, true, true), "weights need the mesh's bones");
        for step in [Step::Bones, Step::Animate] {
            assert!(!bone_is_faint(step, false, false));
            assert!(bone_is_faint(step, true, false));
        }
        assert!(rig_marks_shown(Step::Bones, 5));
        assert!(!rig_marks_shown(Step::Bones, 60));
        assert!(!rig_marks_shown(Step::Mesh, 5));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sharp_outlines_stay_near_their_corners() {
        // A long bone tip and a nearly collapsed triangle, the miter worst cases.
        for poly in [
            vec![pos2(0.0, 0.0), pos2(47.0, 7.0), pos2(311.0, 0.0), pos2(47.0, -7.0)],
            vec![pos2(0.0, 0.0), pos2(300.0, 0.001), pos2(150.0, 0.0)],
        ] {
            let bounds = Rect::from_points(&poly).expand(10.0);
            let mut shapes = polygon_shapes(&poly, Color32::WHITE, Stroke::new(2.0, Color32::WHITE));
            shapes.extend(outline_shapes(&poly, Stroke::new(7.0, Color32::WHITE)));
            let mut tessellator = egui::epaint::Tessellator::new(1.0, Default::default(), [1, 1], Vec::new());
            let mut out = Mesh::default();
            for shape in shapes {
                tessellator.tessellate_shape(shape, &mut out);
            }
            assert!(!out.vertices.is_empty());
            assert!(out.vertices.iter().all(|v| bounds.contains(v.pos)), "{poly:?}");
        }
        assert_eq!(unique_edges(&[[0, 1, 2], [2, 1, 3]]).len(), 5);
    }

    use super::*;

    #[test]
    fn linked_meshes_deform_in_animations_but_keep_geometry_in_setup() {
        let doc=skel2d_core::from_str(&serde_json::json!({
            "bones":[{"name":"root"}],
            "slots":[{"name":"s","bone":"root","attachment":"child","attachments":[
                {"type":"mesh","name":"parent","vertices":[{"x":0,"y":0,"u":0,"v":1},{"x":0,"y":1,"u":0,"v":0},{"x":1,"y":0,"u":1,"v":1}],"triangles":[[0,1,2]]},
                {"type":"linked_mesh","name":"child","parent":"parent"}]}],
            "animations":{"a":{"duration":1}}
        }).to_string()).unwrap();
        let mut app=EditorApp::test_app(doc,std::env::temp_dir().join("skel2d-linked-deform.skel.json"));
        app.selection.slot=Some("s".into());
        assert!(selected_mesh(&app,&app.pose()).is_none());
        app.playback.anim=Some("a".into());
        let pose=app.pose();
        let (si,mesh)=selected_mesh(&app,&pose).unwrap();
        assert_eq!(mesh.name,"child");
        let drag=DeformDrag::start(&app,&pose,si,&mesh,&[1]).unwrap();
        let op=drag.op("s","child",&[[3.0,4.0]]);
        let next=skel2d_core::apply(&app.doc,&[op]).unwrap();
        // Shared timelines key the parent's path.
        assert_eq!(next.animations["a"].deform["default"]["s"]["parent"][0].value[1],[3.0,3.0]);
    }

    #[test]
    fn weight_pose_test_and_stroke_preserve_document_and_record_one_undo() {
        let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/arm/arm.skel.json");
        let doc=skel2d_core::load(&path).unwrap();
        let setup=weight_test_pose(&doc,&[],None);
        let posed=weight_test_pose(&doc,&[],Some(&("lower".into(),90.0)));
        let Attachment::Mesh(mesh)=&doc.slots[0].attachments[0] else {panic!()};
        let points=posed.deform_mesh(&doc,mesh,0);
        // The tip rotates from (122, 13) around the elbow (60, 0).
        assert!((points[8][0]-50.45).abs()<0.001);
        assert!((points[8][1]-59.746).abs()<0.001);
        assert_eq!(weight_test_pose(&doc,&[],None),setup);
        let mut app=EditorApp::test_app(doc.clone(),path);
        let paint=Paint {base:doc.clone(),slot:"skin".into(),attachment:"arm".into(),bone:"lower".into()};
        let op=Op::SetBoneWeight {slot:"skin".into(),attachment:"arm".into(),skin:None,vertices:vec![0],bone:"lower".into(),value:0.5,mode:skel2d_core::ops::WeightMode::Set,locked_bones:vec![]};
        app.preview(&doc,&[op]);let preview=app.doc.clone();assert_ne!(preview,doc);
        finish_paint(&mut app,paint);assert_eq!(app.doc,preview);
        app.undo();assert_eq!(app.doc,doc);app.redo();assert_eq!(app.doc,preview);
        let Attachment::Mesh(changed)=&preview.slots[0].attachments[0] else {panic!()};
        let patches=changed.vertices.iter().enumerate().map(|(index,v)|VertexPatch {index,weights:Some(v.weights.clone()),..Default::default()}).collect();
        let json=serde_json::to_string(&Op::SetVertices {slot:"skin".into(),attachment:"arm".into(),skin:None,vertices:patches}).unwrap();
        let cli=skel2d_rig::apply_any(&doc,&app.base_dir(),&skel2d_rig::parse_ops(&json).unwrap()).unwrap();
        assert_eq!(cli.document,preview);
        let locked=std::collections::BTreeSet::from(["upper".into()]);app.view.weight_locked=locked;
        let weights=std::collections::BTreeMap::from([("lower".into(),1.0)]);
        let op=paste_weight_op(&app,"skin",mesh,&[0],&weights);
        let pasted=skel2d_core::apply(&doc,&[op]).unwrap();let Attachment::Mesh(m)=&pasted.slots[0].attachments[0] else {panic!()};
        assert_eq!(skel2d_core::mesh::weight_map(&m.vertices[0],"upper")["upper"],0.954);
        app.tool=Tool::Weights;app.selection.bone=Some("lower".into());
        app.view.pose_test=true;app.view.pose_angle=90.0;
        let baseline=app.doc.clone();let ctx=egui::Context::default();
        for pressed in [true,false] {
            let input=egui::RawInput {events:vec![egui::Event::Key {key:Key::T,physical_key:None,pressed,repeat:false,modifiers:Default::default()}],..Default::default()};
            let mut output=ctx.run_pass(input,|ctx| {egui::Window::new("weight test").show(ctx,|ui| {
                let (response,_)=ui.allocate_painter(ui.available_size(),Sense::click_and_drag());
                let pose=app.pose();weight_tool(&mut app,ui,&response,response.rect,&pose);
            });});output.textures_delta.clear();
            assert_eq!(app.view.pose_rotation.is_some(),pressed);assert_eq!(app.doc,baseline);
        }
        app.view.weight_clipboard=Some(std::collections::BTreeMap::from([("upper".into(),0.5),("lower".into(),0.5)]));
        assert!(app.edit(vec![Op::SetBone(BonePatch {name:"upper".into(),rename:Some("renamed".into()),..Default::default()})]));
        assert!(app.view.weight_locked.contains("renamed"));assert!(app.view.weight_clipboard.as_ref().unwrap().contains_key("renamed"));
        app.undo();assert!(app.view.weight_locked.contains("upper"));assert!(app.view.weight_clipboard.as_ref().unwrap().contains_key("upper"));
        app.redo();assert!(app.view.weight_locked.contains("renamed"));
    }

    #[test]
    fn alt_scale_drag_changes_only_shear_and_mirrors_angular_direction() {
        let mut drag = Drag {
            base: Document::new("shear"), bone:"bone".into(), tool:Tool::Scale,
            start:[1.0,0.0],eff:[0.0,1.0],last:[0.0,1.0],axis:Axis::Y,handle:None,
            axes:[[1.0,0.0],[0.0,1.0]],anchor:[0.0,0.0],parent_inv:Some(Affine::IDENTITY),
            mirrored:false,rotate:0.0,translate:[0.0,0.0],scale:[1.0,1.0],shear:[7.0,10.0],shearing:true,att:None,
        };
        let snap = Snap { on:false,angle:15.0,grid:1.0 };
        let value = drag_values(&drag,Axis::Y,&snap).unwrap();
        assert_eq!(value.shear,Some([7.0,100.0]));
        assert!(value.scale.is_none() && value.rotate.is_none());
        drag.mirrored=true;
        assert_eq!(drag_values(&drag,Axis::Y,&snap).unwrap().shear,Some([7.0,-80.0]));
    }

    fn region_on_turned_bone() -> AttachmentDrag {
        let region = Region { name: "img".into(), image: None, x: 10.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0, width: 20.0, height: 10.0 };
        let bone_world = Affine::from_trs(5.0, 5.0, 90.0, 1.0, 1.0);
        let frame = bone_world.mul(&Affine::translate(10.0, 0.0));
        AttachmentDrag { slot: "s".into(), attachment: Attachment::Region(region), bone_world, frame, vertices: Vec::new(), deform: None }
    }

    fn region_of(op: Option<Op>) -> Region {
        match op {
            Some(Op::SetAttachment { attachment: Attachment::Region(r), .. }) => r,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn attachment_moves_in_world_space_relative_to_its_bone() {
        let a = region_on_turned_bone();
        // World +x on a bone turned 90° is the bone's local -y.
        let r = region_of(attachment_op(&a, &Vals { translate: Some([4.0, 0.0]), ..Default::default() }));
        assert_eq!((r.x, r.y, r.rotation), (10.0, -4.0, 0.0));
        // Rotation turns the region about its own center, which stays put.
        let r = region_of(attachment_op(&a, &Vals { rotate: Some(30.0), ..Default::default() }));
        assert_eq!((r.x, r.y, r.rotation), (10.0, 0.0, 30.0));
        // Scale is along the region's own axes.
        let r = region_of(attachment_op(&a, &Vals { scale: Some([2.0, 1.0]), ..Default::default() }));
        assert_eq!((r.x, r.y, r.scale_x, r.scale_y), (10.0, 0.0, 2.0, 1.0));
    }

    fn strip() -> CoreMesh {
        let v = |x: f32| skel2d_core::MeshVertex { x, y: 0.0, u: 0.0, v: 0.0, weights: Vec::new() };
        CoreMesh { bones: Vec::new(), hull: 0, edges: Vec::new(), name: "m".into(), image: None, vertices: vec![v(0.0), v(10.0), v(20.0)], triangles: Vec::new() }
    }

    #[test]
    fn picked_vertices_move_alone() {
        let frame = Affine::translate(15.0, 0.0);
        let a = AttachmentDrag {
            slot: "s".into(),
            attachment: Attachment::Mesh(strip()),
            bone_world: Affine::IDENTITY,
            frame,
            vertices: vec![1, 2],
            deform: None,
        };
        // A half turn about the picked vertices' centroid swaps them; vertex 0 stays.
        let Some(Op::SetAttachment { attachment: Attachment::Mesh(m), .. }) = attachment_op(&a, &Vals { rotate: Some(180.0), ..Default::default() }) else { panic!() };
        let xs: Vec<f32> = m.vertices.iter().map(|v| v.x).collect();
        assert_eq!(xs, [0.0, 20.0, 10.0]);
    }

    #[test]
    fn deform_keys_only_the_moved_vertices_through_their_skinning() {
        // A bone turned 90°: world +y is setup-space -x for its vertices.
        let unskin = Affine::from_trs(0.0, 0.0, 90.0, 1.0, 1.0).inverse().unwrap();
        let d = DeformDrag { skin: Some("armor".into()), animation: "a".into(), time: 0.5, offsets: vec![[1.0, 1.0]; 3], verts: vec![(2, [20.0, 0.0], [0.0, 0.0], unskin)] };
        // Vertex 2 sits at world (0, 20); move it up by 5.
        let Op::KeyDeform { offsets, time, skin, .. } = d.op("s", "m", &[[0.0, 25.0]]) else { panic!() };
        assert_eq!(skin.as_deref(), Some("armor"));
        assert_eq!(time, 0.5);
        assert_eq!(offsets, [[1.0, 1.0], [1.0, 1.0], [5.0, 0.0]]);
    }
}
