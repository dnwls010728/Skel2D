//! Evaluating a document (optionally with an animation at some time) into
//! world-space bone transforms and per-slot draw state.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::math::Affine;
use crate::physics::{BodyState, PhysicsMode, PhysicsState};
use crate::model::{Animation, Attachment, AttachmentKey, Color, Document, IkKey, IkPose, Inherit, Key, LayerTrack, Mesh, MeshVertex, TransformMix, Vec2};

/// Shared editor/onion/render track schedule. The main animation is track 0;
/// preview tracks follow in list order and loop over their own durations.
/// A preview of the main animation is dormant until a different main is picked.
/// Expects validated layers (see `validate_layer_tracks`).
pub fn preview_tracks<'a>(doc: &'a Document, main: Option<&str>, clock: f32, layers: &[LayerTrack]) -> Vec<Track<'a>> {
    let mut tracks: Vec<_> = main.and_then(|n| doc.animations.get(n))
        .map(|animation| Track { animation, time: clock, alpha: 1.0 }).into_iter().collect();
    for layer in layers.iter().filter(|l| l.enabled && l.alpha > 0.0 && Some(l.anim.as_str()) != main) {
        if let Some(animation) = doc.animations.get(&layer.anim) {
            let duration = animation.duration();
            tracks.push(Track { animation, time: if duration > 0.0 { clock % duration } else { 0.0 }, alpha: layer.alpha });
        }
    }
    tracks
}

pub trait Lerp: Clone {
    fn lerp(a: Self, b: Self, t: f32) -> Self;

    /// Interpolates with a separate progress per channel.
    fn lerp_channels(a: Self, b: Self, progress: &dyn Fn(usize) -> f32) -> Self {
        Self::lerp(a, b, progress(0))
    }
}

impl Lerp for f32 {
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }
}

impl Lerp for Vec2 {
    fn lerp(a: Vec2, b: Vec2, t: f32) -> Vec2 {
        [f32::lerp(a[0], b[0], t), f32::lerp(a[1], b[1], t)]
    }

    fn lerp_channels(a: Vec2, b: Vec2, progress: &dyn Fn(usize) -> f32) -> Vec2 {
        [f32::lerp(a[0], b[0], progress(0)), f32::lerp(a[1], b[1], progress(1))]
    }
}

impl Lerp for Color {
    fn lerp(a: Color, b: Color, t: f32) -> Color {
        Color {
            r: f32::lerp(a.r, b.r, t),
            g: f32::lerp(a.g, b.g, t),
            b: f32::lerp(a.b, b.b, t),
            a: f32::lerp(a.a, b.a, t),
        }
    }

    fn lerp_channels(a: Color, b: Color, progress: &dyn Fn(usize) -> f32) -> Color {
        Color {
            r: f32::lerp(a.r, b.r, progress(0)),
            g: f32::lerp(a.g, b.g, progress(1)),
            b: f32::lerp(a.b, b.b, progress(2)),
            a: f32::lerp(a.a, b.a, progress(3)),
        }
    }
}

impl Lerp for TransformMix {
    fn lerp(a: TransformMix, b: TransformMix, t: f32) -> TransformMix {
        TransformMix {
            rotate: f32::lerp(a.rotate, b.rotate, t),
            x: f32::lerp(a.x, b.x, t),
            y: f32::lerp(a.y, b.y, t),
            scale_x: f32::lerp(a.scale_x, b.scale_x, t),
            scale_y: f32::lerp(a.scale_y, b.scale_y, t),
            shear_y: f32::lerp(a.shear_y, b.shear_y, t),
        }
    }
}

/// Per-vertex offsets; a missing offset counts as zero.
impl Lerp for Vec<Vec2> {
    fn lerp(a: Vec<Vec2>, b: Vec<Vec2>, t: f32) -> Vec<Vec2> {
        let at = |v: &[Vec2], i: usize| v.get(i).copied().unwrap_or([0.0, 0.0]);
        (0..a.len().max(b.len())).map(|i| Vec2::lerp(at(&a, i), at(&b, i), t)).collect()
    }
}

/// Samples a sorted key list. Before the first key and after the last key the
/// nearest key's value is held.
pub fn sample<T: Lerp>(keys: &[Key<T>], time: f32) -> Option<T> {
    let first = keys.first()?;
    let last = keys.last()?;
    if time <= first.time {
        return Some(first.value.clone());
    }
    if time >= last.time {
        return Some(last.value.clone());
    }
    let i = keys.partition_point(|k| k.time <= time) - 1;
    let (k0, k1) = (&keys[i], &keys[i + 1]);
    let span = k1.time - k0.time;
    let t = if span > 0.0 { (time - k0.time) / span } else { 1.0 };
    Some(T::lerp_channels(k0.value.clone(), k1.value.clone(), &|channel| k0.curve_of(channel).apply(t)))
}

/// The attachment key in effect at `time`; `None` before the first key (the
/// base attachment stays).
pub fn sample_attachment(keys: &[AttachmentKey], time: f32) -> Option<&AttachmentKey> {
    let i = keys.partition_point(|k| k.time <= time);
    (i > 0).then(|| &keys[i - 1])
}

/// IK values in effect at `time`: `mix` and `softness` interpolate, the switches
/// hold until the next key. Fields a key omits follow `setup`.
pub fn sample_ik(keys: &[Key<IkKey>], time: f32, setup: &IkPose) -> Option<IkPose> {
    let first = keys.first()?;
    let last = keys.last()?;
    if time <= first.time {
        return Some(first.value.resolve(setup));
    }
    if time >= last.time {
        return Some(last.value.resolve(setup));
    }
    let i = keys.partition_point(|k| k.time <= time) - 1;
    let (k0, k1) = (&keys[i], &keys[i + 1]);
    let (a, b) = (k0.value.resolve(setup), k1.value.resolve(setup));
    let span = k1.time - k0.time;
    let t = if span > 0.0 { (time - k0.time) / span } else { 1.0 };
    Some(IkPose {
        mix: f32::lerp(a.mix, b.mix, k0.curve_of(0).apply(t)),
        softness: f32::lerp(a.softness, b.softness, k0.curve_of(1).apply(t)),
        ..a
    })
}

/// Slot indices back to front for a key's `order`. Slots the key does not list
/// go right above the slot below them in the setup order.
pub fn resolve_draw_order(doc: &Document, order: &[String]) -> Vec<usize> {
    let mut out: Vec<usize> = order.iter().filter_map(|s| doc.slot_index(s)).collect();
    for i in 0..doc.slots.len() {
        if !out.contains(&i) {
            let at = i.checked_sub(1).and_then(|below| out.iter().position(|&s| s == below)).map_or(0, |p| p + 1);
            out.insert(at, i);
        }
    }
    out
}

/// An event firing, with the key's overrides resolved against its definition.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FiredEvent {
    pub time: f32,
    pub name: String,
    pub int: i32,
    pub float: f32,
    pub string: String,
}

/// Events passed when playback moves from `from` to `to`: `from` itself has
/// already fired, `to` fires now. `None` starts before the animation, so a key
/// at time 0 fires too. Moving backwards means the animation looped: the rest
/// of the animation fires first, then its start up to `to`.
pub fn fired_events(doc: &Document, animation: &Animation, from: Option<f32>, to: f32) -> Vec<FiredEvent> {
    let fire = |low: Option<f32>, high: f32| animation.events.iter().filter(move |k| low.is_none_or(|l| k.time > l) && k.time <= high);
    let keys: Vec<_> = match from {
        Some(f) if to < f => fire(Some(f), f32::INFINITY).chain(fire(None, to)).collect(),
        _ => fire(from, to).collect(),
    };
    keys.into_iter().map(|k| {
        let def = doc.events.get(&k.name);
        FiredEvent {
            time: k.time,
            name: k.name.clone(),
            int: k.int.or(def.map(|d| d.int)).unwrap_or(0),
            float: k.float.or(def.map(|d| d.float)).unwrap_or(0.0),
            string: k.string.clone().or(def.map(|d| d.string.clone())).unwrap_or_default(),
        }
    }).collect()
}

/// One animation playing at `time`, blended in by `alpha` (0..=1).
#[derive(Debug, Clone, Copy)]
pub struct Track<'a> {
    pub animation: &'a Animation,
    pub time: f32,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
pub struct LocalTransform {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub shear_x: f32,
    pub shear_y: f32,
    pub inherit: Inherit,
}

impl LocalTransform {
    /// Recover local axes for a desired world transform. Reparenting can
    /// introduce shear even when both the old and new parents were TRS.
    pub fn from_world(parent: Option<&Affine>, world: &Affine, inherit: Inherit) -> Option<Self> {
        let p = parent.copied().unwrap_or(Affine::IDENTITY);
        let inv = p.inverse()?;
        let [x, y] = inv.apply(world.tx, world.ty);
        let mut local = Self { x, y, rotation: 0.0, scale_x: 1.0, scale_y: 1.0, shear_x: 0.0, shear_y: 0.0, inherit };
        if matches!(inherit, Inherit::NoScale | Inherit::NoScaleOrReflection) {
            let axis = inv.apply_vector(world.a, world.b);
            local.rotation = axis[1].atan2(axis[0]).to_degrees();
        }
        let basis = local.world(parent);
        let relative = basis.inverse()?.mul(world);
        let rotation = relative.b.atan2(relative.a).to_degrees();
        local.rotation += rotation;
        local.scale_x = relative.a.hypot(relative.b);
        local.scale_y = relative.c.hypot(relative.d) * if relative.determinant() < 0.0 { -1.0 } else { 1.0 };
        if local.scale_y.abs() > 1e-12 {
            let angle_y = (relative.d / local.scale_y).atan2(relative.c / local.scale_y).to_degrees();
            local.shear_y = (angle_y - rotation - 90.0 + 540.0).rem_euclid(360.0) - 180.0;
        }
        Some(local)
    }

    pub fn to_affine(&self) -> Affine {
        Affine::from_trss(self.x, self.y, self.rotation, self.scale_x, self.scale_y, self.shear_x, self.shear_y)
    }

    /// World transform under the parent. The position always follows the
    /// parent; each inherit mode decides what the axes take from it:
    /// - `Normal`: everything.
    /// - `OnlyTranslation`: nothing.
    /// - `NoRotationOrReflection`: the parent's stretch, without its rotation,
    ///   shear or mirroring.
    /// - `NoScale`: only where the parent turns the bone's direction, plus mirroring.
    /// - `NoScaleOrReflection`: the same without mirroring.
    pub fn world(&self, parent: Option<&Affine>) -> Affine {
        let Some(parent) = parent else { return self.to_affine() };
        let [tx, ty] = parent.apply(self.x, self.y);
        let spin = |degrees: f32| Affine::from_trs(0.0, 0.0, degrees, 1.0, 1.0);
        let linear = match self.inherit {
            Inherit::Normal => parent.mul(&self.to_affine()),
            Inherit::OnlyTranslation => self.to_affine(),
            Inherit::NoRotationOrReflection => {
                // The parent is a rotation of an upper triangular matrix. Its
                // diagonal is the stretch along the parent's own axes, applied
                // here in those directions while the rotation itself is dropped.
                let along = parent.a.hypot(parent.b);
                let (axis, stretch) = if along > 1e-3 {
                    (parent.b.atan2(parent.a).to_degrees(), Affine::scale(along, parent.determinant().abs() / along))
                } else {
                    // A collapsed x axis leaves only the y axis to say where the parent points.
                    (parent.d.atan2(parent.c).to_degrees() - 90.0, Affine::scale(0.0, parent.c.hypot(parent.d)))
                };
                spin(axis).mul(&stretch).mul(&spin(-axis)).mul(&self.to_affine())
            }
            Inherit::NoScale | Inherit::NoScaleOrReflection => {
                let direction = spin(self.rotation).apply_vector(1.0, 0.0);
                let [x, y] = parent.apply_vector(direction[0], direction[1]);
                let mirrored = self.inherit == Inherit::NoScale && parent.determinant() < 0.0;
                let own = Affine::from_trss(0.0, 0.0, 0.0, self.scale_x, self.scale_y, self.shear_x, self.shear_y);
                spin(y.atan2(x).to_degrees()).mul(&Affine::scale(1.0, if mirrored { -1.0 } else { 1.0 })).mul(&own)
            }
        };
        Affine { tx, ty, ..linear }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BonePose {
    /// Local transform after animation and IK.
    pub local: LocalTransform,
    pub world: Affine,
    /// `world * setup_world⁻¹`: moves setup-pose skeleton-space points (mesh
    /// vertices) along with the bone.
    pub skin: Affine,
}

fn world_transforms(parents: &[Option<usize>], locals: &[LocalTransform]) -> Vec<Affine> {
    let mut worlds: Vec<Affine> = Vec::with_capacity(locals.len());
    for (local, parent) in locals.iter().zip(parents) {
        let p = parent.map(|i| worlds[i]);
        worlds.push(local.world(p.as_ref()));
    }
    worlds
}

/// Shared schedule; new constraint types join this enum when implemented.
#[derive(Debug, Clone, Copy)]
enum ConstraintRef {
    Ik(usize),
    Physics(usize),
    Transform(usize),
}

fn constraint_order(doc: &Document) -> Vec<ConstraintRef> {
    let mut constraints: Vec<_> = (0..doc.ik.len()).map(ConstraintRef::Ik).chain((0..doc.physics.len()).map(ConstraintRef::Physics)).chain((0..doc.transforms.len()).map(ConstraintRef::Transform)).collect();
    constraints.sort_by_key(|constraint| match *constraint {
        ConstraintRef::Ik(i) => doc.ik[i].order,
        ConstraintRef::Physics(i) => doc.physics[i].order,
        ConstraintRef::Transform(i) => doc.transforms[i].order,
    });
    constraints
}

/// Recompute changed bones and their descendants in parent-first order.
fn update_world_subtrees(parents: &[Option<usize>], locals: &[LocalTransform], worlds: &mut [Affine], roots: &[usize]) {
    let mut dirty = vec![false; locals.len()];
    for &i in roots {
        dirty[i] = true;
    }
    for i in 0..locals.len() {
        dirty[i] |= parents[i].is_some_and(|p| dirty[p]);
        if dirty[i] {
            let parent = parents[i].map(|p| worlds[p]);
            worlds[i] = locals[i].world(parent.as_ref());
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotPose {
    pub color: Color,
    pub attachment: Option<String>,
    /// Animated vertex offsets per mesh attachment name (setup-pose space).
    pub deform: BTreeMap<String, Vec<Vec2>>,
}

/// Evaluated pose. `bones[i]` and `slots[i]` match `doc.bones[i]` and
/// `doc.slots[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub bones: Vec<BonePose>,
    pub slots: Vec<SlotPose>,
    /// Active skins, lowest first (see `Document::resolve_attachment`).
    pub skins: Vec<String>,
    /// Per bone: false for skin-required bones without an active skin.
    pub active: Vec<bool>,
    /// Simulation state per physics constraint; empty without physics.
    pub physics: Vec<BodyState>,
    /// Indices into `slots`, back to front.
    pub draw_order: Vec<usize>,
}

impl Pose {
    /// Setup pose, or the pose of `anim` at `time` (clamped to its duration).
    /// Expects a validated document (parents precede children).
    pub fn compute(doc: &Document, anim: Option<(&Animation, f32)>) -> Pose {
        let tracks: Vec<Track> = anim.map(|(animation, time)| Track { animation, time, alpha: 1.0 }).into_iter().collect();
        Self::compute_with(doc, &tracks)
    }

    /// Layers several animations (e.g. face pose + walk + blink + talk) over
    /// the setup pose, as Spine's track entries do. Tracks apply in order: a
    /// track only affects the timelines it keys, replacing what earlier tracks
    /// set by `alpha` (attachments switch when alpha >= 0.5).
    pub fn compute_with(doc: &Document, tracks: &[Track]) -> Pose {
        Self::compute_skinned(doc, tracks, &[])
    }

    /// [`Pose::compute_with`] with `skins` active (later ones win).
    pub fn compute_skinned(doc: &Document, tracks: &[Track], skins: &[String]) -> Pose {
        Self::compute_physics(doc, tracks, skins, &mut PhysicsState::default(), PhysicsMode::None, 0.0)
    }

    /// Simulates a time from scratch, deterministically: the same input gives
    /// bit-identical results (see [`crate::physics::PhysicsRun`] to continue a run).
    pub fn simulate<'a>(doc: &'a Document, tracks_at: impl Fn(f32) -> Vec<Track<'a>>, skins: &[String], t: f32, fps: f32) -> (Pose, PhysicsState) {
        let mut run = crate::physics::PhysicsRun::new(fps, crate::physics::MovePath::default());
        let pose = run.advance(doc, &tracks_at, skins, t);
        (pose, run.state)
    }

    /// [`Pose::compute_skinned`] with physics: `mode` says how `state` takes part.
    /// Update advances it by `delta` seconds.
    pub fn compute_physics(doc: &Document, tracks: &[Track], skins: &[String], state: &mut PhysicsState, mode: PhysicsMode, delta: f32) -> Pose {
        if mode != PhysicsMode::None {
            state.sync(doc);
            if mode == PhysicsMode::Update { state.time += f64::from(delta.max(0.0)); }
        }
        let active: Vec<bool> = (0..doc.bones.len()).map(|i| doc.bone_active(i, skins)).collect();
        let parents: Vec<Option<usize>> =
            doc.bones.iter().map(|b| b.parent.as_deref().and_then(|p| doc.bone_index(p))).collect();
        let setup_locals: Vec<LocalTransform> = doc
            .bones
            .iter()
            .map(|b| LocalTransform { x: b.x, y: b.y, rotation: b.rotation, scale_x: b.scale_x, scale_y: b.scale_y, shear_x: b.shear_x, shear_y: b.shear_y, inherit: b.inherit })
            .collect();

        let mut locals = setup_locals.clone();
        for track in tracks {
            let (a, t, alpha) = (track.animation, track.time, track.alpha.clamp(0.0, 1.0));
            for ((bone, local), setup) in doc.bones.iter().zip(&mut locals).zip(&setup_locals) {
                let Some(tl) = a.bones.get(&bone.name) else { continue };
                if let Some(r) = sample(&tl.rotate, t) {
                    local.rotation = f32::lerp(local.rotation, setup.rotation + r, alpha);
                }
                if let Some([dx, dy]) = sample(&tl.translate, t) {
                    local.x = f32::lerp(local.x, setup.x + dx, alpha);
                    local.y = f32::lerp(local.y, setup.y + dy, alpha);
                }
                if let Some([sx, sy]) = sample(&tl.scale, t) {
                    local.scale_x = f32::lerp(local.scale_x, setup.scale_x * sx, alpha);
                    local.scale_y = f32::lerp(local.scale_y, setup.scale_y * sy, alpha);
                }
                if let Some([sx, sy]) = sample(&tl.shear, t) {
                    local.shear_x = f32::lerp(local.shear_x, setup.shear_x + sx, alpha);
                    local.shear_y = f32::lerp(local.shear_y, setup.shear_y + sy, alpha);
                }
            }
        }

        let mut worlds = world_transforms(&parents, &locals);
        for constraint in constraint_order(doc) {
            let i = match constraint {
                ConstraintRef::Ik(i) => i,
                ConstraintRef::Physics(i) => {
                    if mode == PhysicsMode::None { continue; }
                    let c = &doc.physics[i];
                    let Some(bi) = doc.bone_index(&c.bone) else { continue };
                    let passed_reset = mode == PhysicsMode::Update && state.reset_passed(tracks, &c.name);
                    let time = state.time;
                    let body = &mut state.bodies[i];
                    // Constraints owned by skins (or on inactive bones) need their skin.
                    if !active[bi] || !doc.constraint_skin_active(&c.name, skins) {
                        *body = BodyState { name: c.name.clone(), ..BodyState::default() };
                        continue;
                    }
                    let world = crate::physics::solve(doc, c, body, mode, (time, passed_reset), tracks, (&worlds[bi], doc.bones[bi].length));
                    let parent = parents[bi].map(|p| worlds[p]);
                    if let Some(local) = world.and_then(|w| LocalTransform::from_world(parent.as_ref(), &w, locals[bi].inherit)) {
                        locals[bi] = local;
                        update_world_subtrees(&parents, &locals, &mut worlds, &[bi]);
                    }
                    continue;
                }
                ConstraintRef::Transform(i) => {
                    let c = &doc.transforms[i];
                    let Some(source) = doc.bone_index(&c.source) else { continue };
                    let on = active[source] && doc.constraint_skin_active(&c.name, skins);
                    let mut mix = c.mix();
                    for track in tracks {
                        if let Some(m) = track.animation.transforms.get(&c.name).and_then(|keys| sample(keys, track.time)) {
                            mix = TransformMix::lerp(mix, m, track.alpha.clamp(0.0, 1.0));
                        }
                    }
                    let c = &c.with_mix(mix);
                    for bone in c.bones.iter().filter_map(|b| doc.bone_index(b)).filter(|_| on) {
                        if !active[bone] { continue; }
                        let parent = parents[bone].map(|p| worlds[p]);
                        let source_local = locals[source];
                        if crate::transform::solve(c, &mut locals[bone], &worlds[bone], parent.as_ref(), &source_local, &worlds[source]) {
                            update_world_subtrees(&parents, &locals, &mut worlds, &[bone]);
                        }
                    }
                    continue;
                }
            };
            let ik = &doc.ik[i];
            let bones_on = ik.bones.iter().chain([&ik.target]).all(|b| doc.bone_index(b).is_none_or(|i| active[i]));
            if !doc.constraint_skin_active(&ik.name, skins) || !bones_on {
                continue;
            }
            let setup = ik.pose();
            let mut values = setup;
            for track in tracks {
                if let Some(v) = track.animation.ik.get(&ik.name).and_then(|keys| sample_ik(keys, track.time, &setup)) {
                    let alpha = track.alpha.clamp(0.0, 1.0);
                    values.mix = f32::lerp(values.mix, v.mix, alpha);
                    values.softness = f32::lerp(values.softness, v.softness, alpha);
                    if alpha >= 0.5 {
                        values.bend_positive = v.bend_positive;
                        values.compress = v.compress;
                        values.stretch = v.stretch;
                    }
                }
            }
            values.mix = values.mix.clamp(0.0, 1.0);
            values.softness = values.softness.max(0.0);
            if values.mix > 0.0 && crate::ik::solve(doc, ik, &values, &parents, &mut locals, &worlds) {
                let roots: Vec<_> = ik.bones.iter().filter_map(|b| doc.bone_index(b)).collect();
                update_world_subtrees(&parents, &locals, &mut worlds, &roots);
            }
        }

        // Skinning matrices map setup-pose skeleton space to the current pose.
        let setup_worlds = world_transforms(&parents, &setup_locals);
        let bones = locals
            .into_iter()
            .zip(worlds)
            .zip(setup_worlds)
            .map(|((local, world), setup)| BonePose {
                local,
                world,
                skin: setup.inverse().map(|inv| world.mul(&inv)).unwrap_or(world),
            })
            .collect::<Vec<_>>();

        let slots = doc
            .slots
            .iter()
            .enumerate()
            .map(|(si, slot)| {
                let (mut color, mut attachment) = (slot.color, slot.attachment.clone());
                let mut deform: BTreeMap<String, Vec<Vec2>> = BTreeMap::new();
                for track in tracks {
                    let Some(tl) = track.animation.slots.get(&slot.name) else { continue };
                    let alpha = track.alpha.clamp(0.0, 1.0);
                    if let Some(c) = sample(&tl.color, track.time) {
                        color = Color::lerp(color, slot.color.multiply(c), alpha);
                    }
                    // `reset` keeps what lower tracks (or the setup pose) show.
                    if let Some(key) = sample_attachment(&tl.attachment, track.time)
                        && alpha >= 0.5
                        && !key.reset
                    {
                        attachment = key.name.clone();
                    }
                }
                for track in tracks {
                    let alpha = track.alpha.clamp(0.0, 1.0);
                    for (skin, slots) in &track.animation.deform {
                        let Some(atts) = slots.get(&slot.name) else { continue };
                        for (name, keys) in atts {
                            let source = doc.resolve_attachment_from(si, name, skins).map(|(k, _)| k.unwrap_or("default"));
                            if source != Some(skin.as_str()) { continue; }
                            if let Some(d) = sample(keys, track.time) {
                                let cur = deform.remove(name).unwrap_or_default();
                                deform.insert(name.clone(), Vec::lerp(cur, d, alpha));
                            }
                        }
                    }
                }
                if let Some(name) = &attachment
                    && let Some(Attachment::LinkedMesh(link)) = doc.resolve_attachment(si, name, skins)
                    && link.timelines {
                        deform.remove(name);
                        let parent_skin = link.parent_skin.as_deref().unwrap_or("default");
                        let mut inherited = Vec::new();
                        for track in tracks {
                            if let Some(keys) = track.animation.deform.get(parent_skin).and_then(|s| s.get(&slot.name)).and_then(|a| a.get(&link.parent))
                                && let Some(offsets) = sample(keys, track.time) {
                                inherited = Vec::lerp(inherited, offsets, track.alpha.clamp(0.0, 1.0));
                            }
                        }
                        if !inherited.is_empty() { deform.insert(name.clone(), inherited); }
                }
                SlotPose { color, attachment, deform }
            })
            .collect();

        let physics = if mode == PhysicsMode::None { Vec::new() } else {
            if mode != PhysicsMode::Pose { state.remember_tracks(tracks); }
            state.bodies.clone()
        };
        let mut draw_order: Vec<usize> = (0..doc.slots.len()).collect();
        for track in tracks.iter().filter(|t| t.alpha >= 0.5) {
            let keys = &track.animation.draw_order;
            let i = keys.partition_point(|k| k.time <= track.time);
            if i > 0 {
                draw_order = match &keys[i - 1].order {
                    Some(order) => resolve_draw_order(doc, order),
                    None => (0..doc.slots.len()).collect(),
                };
            }
        }
        Pose { bones, slots, skins: skins.to_vec(), active, physics, draw_order }
    }

    /// The attachment slot `slot` shows in this pose, resolved through the
    /// active skins; `None` when hidden or its bone is inactive.
    pub fn attachment<'a>(&self, doc: &'a Document, slot: usize) -> Option<&'a Attachment> {
        let s = doc.slots.get(slot)?;
        if doc.bone_index(&s.bone).is_some_and(|b| !self.active.get(b).copied().unwrap_or(true)) {
            return None;
        }
        doc.resolve_attachment(slot, self.slots.get(slot)?.attachment.as_deref()?, &self.skins)
    }

    /// Current world positions of the vertices of `mesh`, an attachment of
    /// `doc.slots[slot]`: animated deform offsets, then linear blend
    /// skinning (see [`Pose::vertex_skin`]).
    pub fn deform_mesh(&self, doc: &Document, mesh: &Mesh, slot: usize) -> Vec<[f32; 2]> {
        let slot_bone = doc.slots.get(slot).and_then(|s| doc.bone_index(&s.bone)).unwrap_or(0);
        let offsets = self.deform_offsets(slot, &mesh.name);
        mesh.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let [dx, dy] = offsets.get(i).copied().unwrap_or([0.0, 0.0]);
                let [x, y] = self.vertex_skin(doc, v, slot_bone).apply(v.x + dx, v.y + dy);
                let [sx, sy] = self.bind_shift(doc, v);
                [x + sx, y + sy]
            })
            .collect()
    }

    /// Deform offsets of a slot's mesh attachment in this pose (empty when
    /// not animated).
    pub fn deform_offsets(&self, slot: usize, attachment: &str) -> &[Vec2] {
        self.slots.get(slot).and_then(|s| s.deform.get(attachment)).map_or(&[], Vec::as_slice)
    }

    /// The transform taking a vertex from setup-pose space to this pose: the
    /// weighted blend of its bones' skinning matrices. Vertices without
    /// weights follow `slot_bone`; weights naming missing bones are ignored.
    pub fn vertex_skin(&self, doc: &Document, v: &MeshVertex, slot_bone: usize) -> Affine {
        let mut m = Affine { a: 0.0, b: 0.0, c: 0.0, d: 0.0, tx: 0.0, ty: 0.0 };
        let mut total = 0.0;
        for w in &v.weights {
            let Some(i) = doc.bone_index(&w.bone) else { continue };
            if w.weight.is_nan() || w.weight <= 0.0 {
                continue;
            }
            let s = &self.bones[i].skin;
            m = Affine {
                a: m.a + s.a * w.weight,
                b: m.b + s.b * w.weight,
                c: m.c + s.c * w.weight,
                d: m.d + s.d * w.weight,
                tx: m.tx + s.tx * w.weight,
                ty: m.ty + s.ty * w.weight,
            };
            total += w.weight;
        }
        if total > 0.0 {
            Affine { a: m.a / total, b: m.b / total, c: m.c / total, d: m.d / total, tx: m.tx / total, ty: m.ty / total }
        } else {
            self.bones.get(slot_bone).map_or(Affine::IDENTITY, |b| b.skin)
        }
    }

    /// What the per-bone binds of a vertex add to [`Pose::vertex_skin`]: the
    /// weighted offsets, each carried by its bone. Zero in the setup pose.
    pub fn bind_shift(&self, doc: &Document, v: &MeshVertex) -> Vec2 {
        if v.weights.iter().all(|w| w.offset == [0.0, 0.0]) { return [0.0, 0.0]; }
        let (mut x, mut y, mut total) = (0.0, 0.0, 0.0);
        for w in v.weights.iter().filter(|w| w.weight > 0.0) {
            let Some(i) = doc.bone_index(&w.bone) else { continue };
            let [ox, oy] = self.bones[i].skin.apply_vector(w.offset[0], w.offset[1]);
            x += ox * w.weight;
            y += oy * w.weight;
            total += w.weight;
        }
        if total > 0.0 { [x / total, y / total] } else { [0.0, 0.0] }
    }

    /// Compact, human/AI readable summary of the pose.
    pub fn report(&self, doc: &Document) -> PoseReport {
        let bones = doc
            .bones
            .iter()
            .zip(&self.bones)
            .map(|(bone, p)| {
                let w = &p.world;
                BoneReport {
                    name: bone.name.clone(),
                    x: round(w.tx),
                    y: round(w.ty),
                    rotation: round(w.rotation_deg()),
                    scale_x: round(w.scale_x()),
                    scale_y: round(w.scale_y()),
                    shear_x: round(p.local.shear_x),
                    shear_y: round(p.local.shear_y),
                    inherit: p.local.inherit,
                    world: *w,
                    tip: (bone.length != 0.0).then(|| {
                        let [x, y] = w.apply(bone.length, 0.0);
                        [round(x), round(y)]
                    }),
                }
            })
            .collect();
        let slots = doc
            .slots
            .iter()
            .zip(&self.slots)
            .enumerate()
            .map(|(i, (slot, p))| {
                let resolved = self.attachment(doc, i).and_then(|_| p.attachment.as_deref().and_then(|n| doc.resolve_attachment_from(i, n, &self.skins)));
                SlotReport {
                name: slot.name.clone(),
                attachment: p.attachment.clone(),
                source_skin: resolved.map(|(k, _)| k.unwrap_or("default").to_string()),
                resolved_attachment: resolved.map(|(_, a)| a.name().to_string()),
                color: p.color.to_string(),
            }})
            .collect();
        let moved = self.draw_order.iter().enumerate().any(|(i, &s)| i != s);
        let draw_order = if moved { self.draw_order.iter().map(|&s| doc.slots[s].name.clone()).collect() } else { Vec::new() };
        PoseReport { bones, slots, draw_order, physics: self.physics.iter().map(|b| BodyState {
            name: b.name.clone(), x: round(b.x), y: round(b.y), rotate: round(b.rotate.to_degrees()), scale: round(b.scale),
            vx: round(b.vx), vy: round(b.vy), vrotate: round(b.vrotate.to_degrees()), vscale: round(b.vscale), ..BodyState::default()
        }).collect() }
    }
}

fn round(v: f32) -> f32 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

/// World-space pose summary.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PoseReport {
    pub bones: Vec<BoneReport>,
    pub slots: Vec<SlotReport>,
    /// Slots back to front; present only when an animation changed the setup order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub draw_order: Vec<String>,
    /// Per physics constraint: current offsets and velocities (angles in degrees).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub physics: Vec<BodyState>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BoneReport {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Evaluated local shear angles (setup plus keys).
    pub shear_x: f32,
    pub shear_y: f32,
    pub inherit: Inherit,
    /// Full matrix preserves shear which rotation/scale summaries cannot.
    pub world: Affine,
    /// World position of the bone's end point (present when length != 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SlotReport {
    pub name: String,
    pub attachment: Option<String>,
    pub resolved_attachment: Option<String>,
    pub source_skin: Option<String>,
    pub color: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Curve;
    use crate::model::{Bone, BoneTimelines};

    fn key<T>(time: f32, value: T) -> Key<T> {
        Key::new(time, value, Curve::Linear)
    }

    #[test]
    fn sample_interpolates_and_holds() {
        let keys = vec![key(0.0, 0.0), key(1.0, 10.0)];
        assert_eq!(sample(&keys, -1.0), Some(0.0));
        assert_eq!(sample(&keys, 0.5), Some(5.0));
        assert_eq!(sample(&keys, 2.0), Some(10.0));
        let stepped = vec![Key::new(0.0, 0.0, Curve::Stepped), key(1.0, 10.0)];
        assert_eq!(sample(&stepped, 0.99), Some(0.0));
    }

    #[test]
    fn child_follows_animated_parent() {
        let mut doc = Document::new("t");
        let mut root = Bone::new("root");
        root.length = 10.0;
        let mut arm = Bone::new("arm");
        arm.parent = Some("root".into());
        arm.x = 10.0;
        doc.bones = vec![root, arm];
        let mut anim = Animation::default();
        anim.bones.insert(
            "root".into(),
            BoneTimelines { rotate: vec![key(0.0, 0.0), key(1.0, 90.0)], ..Default::default() },
        );
        let pose = Pose::compute(&doc, Some((&anim, 1.0)));
        let [x, y] = pose.bones[1].world.apply(0.0, 0.0);
        assert!(x.abs() < 1e-4 && (y - 10.0).abs() < 1e-4);
    }
}
