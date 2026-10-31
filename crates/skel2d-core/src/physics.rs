//! Physics constraints: bones lag behind their animated pose on damped springs.
//!
//! The integrator is Skel2D's own (semi-implicit Euler at a fixed step), so
//! values tuned elsewhere look similar without being numerically identical.

use schemars::JsonSchema;
use serde::Serialize;

use crate::math::Affine;
use crate::model::{Document, PhysicsConstraint, Vec2};
use crate::pose::{Pose, Track, sample};

/// How physics takes part in pose evaluation (Spine's `Physics` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsMode {
    /// Constraints do nothing: the plain animated pose.
    None,
    /// State restarts from the current pose.
    Reset,
    /// Time advances; bodies integrate and apply.
    Update,
    /// Last state is applied again without advancing time (scrubbing).
    Pose,
}

/// Simulation state of one constraint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct BodyState {
    pub name: String,
    /// Offsets currently applied: world x/y, radians of rotation and shear,
    /// and the scale factor change along the bone.
    pub x: f32,
    pub y: f32,
    pub rotate: f32,
    pub scale: f32,
    pub vx: f32,
    pub vy: f32,
    pub vrotate: f32,
    pub vscale: f32,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) ready: bool,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) steps: u64,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) last: Vec2,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) last_tip: Vec2,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) moved: Vec2,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) moved_tip: Vec2,
}

impl BodyState {
    fn restart(&mut self, position: Vec2, tip: Vec2, steps: u64) {
        *self = BodyState { name: std::mem::take(&mut self.name), ready: true, steps, last: position, last_tip: tip, ..BodyState::default() };
    }
}

/// Simulation state of a whole skeleton, in `doc.physics` order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicsState {
    pub bodies: Vec<BodyState>,
    /// Simulated seconds. A double so that step counts stay exact.
    pub time: f64,
    track_times: Vec<f32>,
}

impl PhysicsState {
    /// The skeleton moved in the world by this much since the last update:
    /// bodies lag behind as if their bones had moved (Spine `physicsTranslate`).
    pub fn translate(&mut self, x: f32, y: f32) {
        for b in &mut self.bodies {
            for moved in [&mut b.moved, &mut b.moved_tip] {
                moved[0] += x;
                moved[1] += y;
            }
        }
    }

    /// The skeleton rotated about a world point (Spine `physicsRotate`).
    pub fn rotate(&mut self, center: Vec2, degrees: f32) {
        let (s, c) = degrees.to_radians().sin_cos();
        let shift = |p: Vec2| {
            let [x, y] = [p[0] - center[0], p[1] - center[1]];
            [x * c - y * s - x, x * s + y * c - y]
        };
        for b in &mut self.bodies {
            let (m, t) = (shift(b.last), shift(b.last_tip));
            b.moved = [b.moved[0] + m[0], b.moved[1] + m[1]];
            b.moved_tip = [b.moved_tip[0] + t[0], b.moved_tip[1] + t[1]];
        }
    }

    /// Adds velocity to a body; used to preview a constraint without animating it.
    pub fn impulse(&mut self, name: &str, vx: f32, vy: f32) {
        if let Some(b) = self.bodies.iter_mut().find(|b| b.name == name) {
            b.vx += vx;
            b.vy += vy;
            b.vrotate += (vx - vy) * 0.05;
        }
    }

    /// Matches `bodies` to the constraints, keeping the state of those that remain.
    pub(crate) fn sync(&mut self, doc: &Document) {
        if self.bodies.len() == doc.physics.len() && self.bodies.iter().zip(&doc.physics).all(|(b, c)| b.name == c.name) {
            return;
        }
        let old = std::mem::take(&mut self.bodies);
        self.bodies = doc.physics.iter().map(|c| {
            old.iter().find(|b| b.name == c.name).cloned().unwrap_or_else(|| BodyState { name: c.name.clone(), ..BodyState::default() })
        }).collect();
    }

    pub(crate) fn remember_tracks(&mut self, tracks: &[Track]) {
        self.track_times = tracks.iter().map(|t| t.time).collect();
    }

    /// Whether a reset key of the constraint (or of all, name "") was passed
    /// since the last update, including across a loop.
    pub(crate) fn reset_passed(&self, tracks: &[Track], name: &str) -> bool {
        tracks.iter().enumerate().any(|(i, track)| {
            let Some(&prev) = self.track_times.get(i) else { return false };
            [name, ""].iter().filter_map(|n| track.animation.physics.get(*n)).flat_map(|t| &t.reset).any(|&key| {
                if track.time >= prev { key > prev && key <= track.time } else { key > prev || key <= track.time }
            })
        })
    }
}

/// Setup or animated value of one property: named timelines always apply, the
/// timelines of constraint "" only to constraints using the global value.
fn property(setup: f32, global: Option<f32>, tracks: &[Track], name: &str, keys: fn(&crate::model::PhysicsTimelines) -> &[crate::model::Key<f32>]) -> f32 {
    let mut value = global.unwrap_or(setup);
    for track in tracks {
        let alpha = track.alpha.clamp(0.0, 1.0);
        let shared = global.is_some().then(|| track.animation.physics.get("")).flatten();
        for tl in shared.into_iter().chain(track.animation.physics.get(name)) {
            if let Some(v) = sample(keys(tl), track.time) {
                value += (v - value) * alpha;
            }
        }
    }
    value
}

/// Advances `body` for one evaluation and returns the bone's new world
/// transform, or `None` when it stays as it is.
pub(crate) fn solve(doc: &Document, c: &PhysicsConstraint, body: &mut BodyState, mode: PhysicsMode, (time, passed_reset): (f64, bool), tracks: &[Track], (world, length): (&Affine, f32)) -> Option<Affine> {
    let g = &doc.physics_globals;
    let flag = |on: bool, v: f32| on.then_some(v);
    let name = c.name.as_str();
    let inertia = property(c.inertia, flag(c.inertia_global, g.inertia), tracks, name, |t| &t.inertia).max(0.0);
    let strength = property(c.strength, flag(c.strength_global, g.strength), tracks, name, |t| &t.strength).max(0.0);
    let damping = property(c.damping, flag(c.damping_global, g.damping), tracks, name, |t| &t.damping).clamp(0.0, 1.0);
    let mass = property(c.mass_inverse, flag(c.mass_global, g.mass_inverse), tracks, name, |t| &t.mass).max(1e-4);
    let wind = property(c.wind, flag(c.wind_global, g.wind), tracks, name, |t| &t.wind);
    let gravity = property(c.gravity, flag(c.gravity_global, g.gravity), tracks, name, |t| &t.gravity);
    let mix = property(c.mix, flag(c.mix_global, g.mix), tracks, name, |t| &t.mix).clamp(0.0, 1.0);

    let position = [world.tx, world.ty];
    let tip = world.apply(length, 0.0);
    let hz = 1.0 / f64::from(c.step);
    let due = (time * hz + 1e-9).floor() as u64;
    if !body.ready || mode == PhysicsMode::Reset || passed_reset {
        body.restart(position, tip, due);
        return None;
    }
    if mode == PhysicsMode::Update {
        let steps = due.saturating_sub(body.steps).min(MAX_STEPS);
        if due < body.steps {
            body.steps = due;
        }
        if steps > 0 {
            body.steps = due;
            if mix > 0.0 {
                integrate(c, body, steps, Forces { inertia, strength, damping, mass, wind, gravity }, position, tip, length);
            }
            body.last = position;
            body.last_tip = tip;
            body.moved = [0.0, 0.0];
            body.moved_tip = [0.0, 0.0];
        }
    }
    if mix <= 0.0 {
        return None;
    }
    let (mut w, dx, dy) = (*world, body.x * c.x * mix, body.y * c.y * mix);
    w.tx += dx;
    w.ty += dy;
    let spin = |w: &mut Affine, radians: f32, both: bool| {
        let (s, co) = radians.sin_cos();
        (w.a, w.b) = (w.a * co - w.b * s, w.a * s + w.b * co);
        if both {
            (w.c, w.d) = (w.c * co - w.d * s, w.c * s + w.d * co);
        }
    };
    spin(&mut w, body.rotate * c.rotate * mix, true);
    spin(&mut w, body.rotate * c.shear_x * mix, false);
    let scale = 1.0 + body.scale * c.scale_x * mix;
    w.a *= scale;
    w.b *= scale;
    (w != *world).then_some(w)
}

/// More steps than this in one update are dropped, so that a long stall (a
/// paused window, a huge delta) cannot freeze evaluation.
const MAX_STEPS: u64 = 2400;

struct Forces {
    inertia: f32,
    strength: f32,
    damping: f32,
    mass: f32,
    wind: f32,
    gravity: f32,
}

fn integrate(c: &PhysicsConstraint, body: &mut BodyState, steps: u64, f: Forces, position: Vec2, tip: Vec2, length: f32) {
    let h = c.step;
    // Movement faster than the limit is lost, so teleports do not fling bodies.
    let reach = c.limit * h * steps as f32;
    let travel = |to: Vec2, from: Vec2, moved: Vec2| [(to[0] - from[0] + moved[0]).clamp(-reach, reach), (to[1] - from[1] + moved[1]).clamp(-reach, reach)];
    let d = travel(position, body.last, body.moved);
    if c.x > 0.0 { body.x -= d[0] * f.inertia; }
    if c.y > 0.0 { body.y -= d[1] * f.inertia; }

    let angular = (c.rotate > 0.0 || c.shear_x > 0.0) && length != 0.0;
    let stretch = c.scale_x > 0.0 && length != 0.0;
    // The bone's axis per unit of length, for forces acting on its tip.
    let axis = if length != 0.0 { [(tip[0] - position[0]) / length.abs(), (tip[1] - position[1]) / length.abs()] } else { [1.0, 0.0] };
    let unit = axis[0].hypot(axis[1]).max(1e-6);
    let axis = [axis[0] / unit, axis[1] / unit];
    let reach_length = length.abs().max(1e-6);
    let across = |v: Vec2| (axis[0] * v[1] - axis[1] * v[0]) / reach_length;
    let along = |v: Vec2| (axis[0] * v[0] + axis[1] * v[1]) / reach_length;
    let t = travel(tip, body.last_tip, body.moved_tip);
    if angular { body.rotate -= across(t) * f.inertia; }
    if stretch { body.scale -= along(t) * f.inertia; }

    // Past k*m*h^2 = 4 semi-implicit Euler diverges: stiffer springs just settle.
    let k = f.strength.min(3.9 / (f.mass * h * h));
    let keep = f.damping.powf(60.0 * h);
    let force = [f.wind, -f.gravity];
    let limit = c.limit;
    let angular_limit = limit / reach_length;
    for _ in 0..steps {
        if c.x > 0.0 {
            body.vx = ((body.vx + f.mass * (force[0] - k * body.x) * h) * keep).clamp(-limit, limit);
            body.x += body.vx * h;
        }
        if c.y > 0.0 {
            body.vy = ((body.vy + f.mass * (force[1] - k * body.y) * h) * keep).clamp(-limit, limit);
            body.y += body.vy * h;
        }
        if angular {
            body.vrotate = ((body.vrotate + f.mass * (across(force) - k * body.rotate) * h) * keep).clamp(-angular_limit, angular_limit);
            body.rotate += body.vrotate * h;
        }
        if stretch {
            body.vscale = ((body.vscale + f.mass * (along(force) - k * body.scale) * h) * keep).clamp(-angular_limit, angular_limit);
            body.scale += body.vscale * h;
        }
    }
}

/// A piecewise linear world path of the whole skeleton, sorted by time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MovePath(pub Vec<(f32, Vec2)>);

impl MovePath {
    /// Parses `x,y@t;x,y@t` (the CLI's `--move`).
    pub fn parse(text: &str) -> Result<MovePath, String> {
        let mut path = Vec::new();
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let bad = || format!("bad move point {part:?}, expected x,y@seconds");
            let (point, time) = part.split_once('@').ok_or_else(bad)?;
            let (x, y) = point.split_once(',').ok_or_else(bad)?;
            let parse = |v: &str| v.trim().parse::<f32>().ok().filter(|v| v.is_finite()).ok_or_else(bad);
            path.push((parse(time)?, [parse(x)?, parse(y)?]));
        }
        if path.windows(2).any(|w| w[1].0 <= w[0].0) {
            return Err("move points must have increasing times".into());
        }
        Ok(MovePath(path))
    }

    pub fn at(&self, t: f32) -> Vec2 {
        let keys = &self.0;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return [0.0, 0.0] };
        if t <= first.0 { return first.1; }
        if t >= last.0 { return last.1; }
        let i = keys.partition_point(|k| k.0 <= t) - 1;
        let (a, b) = (keys[i], keys[i + 1]);
        let f = (t - a.0) / (b.0 - a.0);
        [a.1[0] + (b.1[0] - a.1[0]) * f, a.1[1] + (b.1[1] - a.1[1]) * f]
    }
}

/// A deterministic simulation from time 0, advanced frame by frame. Asking for
/// increasing times continues the same run and gives the same result as
/// simulating each time from scratch; earlier times restart it.
#[derive(Debug, Clone)]
pub struct PhysicsRun {
    pub state: PhysicsState,
    fps: f64,
    path: MovePath,
    /// The last frame simulated; `None` before the first.
    frame: Option<u64>,
}

impl PhysicsRun {
    pub fn new(fps: f32, path: MovePath) -> Self {
        Self { state: PhysicsState::default(), fps: f64::from(fps.max(1.0)), path, frame: None }
    }

    /// The frame index holding the simulated state for time `t`.
    pub fn frame_at(&self, t: f32) -> u64 {
        (f64::from(t.max(0.0)) * self.fps + 1e-6).floor() as u64
    }

    /// Continues from a saved state (see the editor's scrub cache).
    pub fn resume(&mut self, state: PhysicsState, frame: u64) {
        self.state = state;
        self.frame = Some(frame);
    }

    /// Simulates up to `t` and returns the pose there. `tracks_at` must give
    /// the tracks at a time and is called once per frame.
    pub fn advance<'a>(&mut self, doc: &'a Document, tracks_at: &impl Fn(f32) -> Vec<Track<'a>>, skins: &[String], t: f32) -> Pose {
        self.advance_frames(doc, tracks_at, skins, self.frame_at(t), &mut |_, _| {});
        Pose::compute_physics(doc, &tracks_at(t), skins, &mut self.state, PhysicsMode::Pose, 0.0)
    }

    /// Simulates up to frame `target`, reporting the state after every frame.
    pub fn advance_frames<'a>(&mut self, doc: &'a Document, tracks_at: &impl Fn(f32) -> Vec<Track<'a>>, skins: &[String], target: u64, each: &mut impl FnMut(u64, &PhysicsState)) {
        if self.frame.is_some_and(|f| f > target) {
            self.frame = None;
        }
        if self.frame.is_none() {
            self.state = PhysicsState::default();
            Pose::compute_physics(doc, &tracks_at(0.0), skins, &mut self.state, PhysicsMode::Reset, 0.0);
            self.frame = Some(0);
            each(0, &self.state);
        }
        let from = self.frame.unwrap_or(0) + 1;
        for frame in from..=target {
            let time = frame as f64 / self.fps;
            let (a, b) = (self.path.at(((frame - 1) as f64 / self.fps) as f32), self.path.at(time as f32));
            self.state.translate(b[0] - a[0], b[1] - a[1]);
            // Exact frame times: summing deltas would drift.
            self.state.time = time;
            Pose::compute_physics(doc, &tracks_at(time as f32), skins, &mut self.state, PhysicsMode::Update, 0.0);
            self.frame = Some(frame);
            each(frame, &self.state);
        }
    }
}
