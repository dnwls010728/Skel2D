//! Physics constraints: bones lag behind their animated pose on damped springs.
//!
//! Each fixed step solves the spring exactly rather than approximating it, so
//! results stay stable at any stiffness. Values tuned for other simulators may
//! look similar but are not numerically identical.

use schemars::JsonSchema;
use serde::Serialize;

use crate::math::Affine;
use crate::model::{Document, PhysicsConstraint, Vec2};
use crate::pose::{Pose, Track, sample};

/// How physics takes part in pose evaluation (the `Physics` mode).
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
    /// bodies lag behind as if their bones had moved.
    pub fn translate(&mut self, x: f32, y: f32) {
        for b in &mut self.bodies {
            for moved in [&mut b.moved, &mut b.moved_tip] {
                moved[0] += x;
                moved[1] += y;
            }
        }
    }

    /// The skeleton rotated about a world point: bodies lag behind that turn.
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
                integrate(c, body, steps, Forces { inertia, strength, damping, mass, wind, gravity }, (world, tip));
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
    // The body state is the raw simulation; influences and mix only decide how
    // much of it reaches the bone. Untouched bones stay bit-identical.
    let shift = [body.x * c.x * mix, body.y * c.y * mix];
    let turn_y = body.rotate * c.rotate * mix;
    let turn_x = turn_y + body.rotate * c.shear_x * mix;
    let stretch = (1.0 + body.scale * c.scale_x * mix).max(MIN_STRETCH);
    if shift == [0.0, 0.0] && turn_x == 0.0 && turn_y == 0.0 && stretch == 1.0 {
        return None;
    }
    let [a, b] = turned(world.a * stretch, world.b * stretch, turn_x);
    let [c_axis, d] = turned(world.c, world.d, turn_y);
    Some(Affine { a, b, c: c_axis, d, tx: world.tx + shift[0], ty: world.ty + shift[1] })
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

/// A collapsed x axis would make the bone singular and unrecoverable into locals.
const MIN_STRETCH: f32 = 0.01;

/// Shortest bone used as a lever, so tiny or zero-length bones keep a sane
/// conversion from world units at the tip to radians.
const MIN_LEVER: f32 = 1.0;

fn turned(x: f32, y: f32, radians: f32) -> Vec2 {
    let (s, c) = radians.sin_cos();
    [x * c - y * s, x * s + y * c]
}

fn dot(a: Vec2, b: Vec2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

/// Shortens a vector to `max` without changing its direction.
fn capped(v: Vec2, max: f32) -> Vec2 {
    let len = v[0].hypot(v[1]);
    if len > max { [v[0] / len * max, v[1] / len * max] } else { v }
}

/// What one fixed step does to a mass on a spring: the closed-form solution of
/// the undamped oscillator over the step, then the damping as a separate factor.
/// Being exact, it cannot gain energy at any stiffness, damping 1 keeps all of
/// it, and it comes to rest precisely at force / strength.
struct SpringStep {
    h: f32,
    spring: f32,
    inv_mass: f32,
    omega: f32,
    sin: f32,
    cos: f32,
    keep: f32,
}

impl SpringStep {
    fn new(h: f32, f: &Forces) -> Self {
        let omega = (f.strength * f.mass).sqrt();
        let (sin, cos) = (omega * h).sin_cos();
        Self { h, spring: f.strength, inv_mass: f.mass, omega, sin, cos, keep: f.damping.powf(h * 60.0) }
    }

    /// Advances one channel under a constant `force`.
    fn advance(&self, value: &mut f32, speed: &mut f32, force: f32) {
        let (start, h) = (*value, self.h);
        if self.omega * h >= std::f32::consts::PI {
            // Faster than the step can show: the body simply sits at its rest point.
            *value = force / self.spring;
            *speed = 0.0;
        } else if self.omega * h < 1e-3 {
            // No usable spring: constant acceleration, which the general form
            // below cannot express (it divides by omega).
            let accel = self.inv_mass * (force - self.spring * *value);
            *value += h * (*speed + 0.5 * accel * h);
            *speed += accel * h;
        } else {
            let rest = force / self.spring;
            let away = *value - rest;
            *value = rest + away * self.cos + *speed * self.sin / self.omega;
            *speed = *speed * self.cos - away * self.omega * self.sin;
        }
        *speed *= self.keep;
        if !value.is_finite() || !speed.is_finite() {
            *value = start;
            *speed = 0.0;
        }
    }
}

/// Runs `steps` fixed steps. Three independent channels share the same spring:
/// a point mass at the bone's origin (x, y), a rod swinging about that origin
/// (rotate, also driving shear), and the rod's length (scale). Movement since
/// the last update is spread evenly over the steps, and a channel lags by the
/// `inertia` share of it, opposite to the motion.
fn integrate(c: &PhysicsConstraint, body: &mut BodyState, steps: u64, f: Forces, (world, tip): (&Affine, Vec2)) {
    let position = [world.tx, world.ty];
    let h = c.step;
    let share = f.inertia / steps as f32;
    let budget = c.limit * h * steps as f32;
    let root_motion = capped([position[0] - body.last[0] + body.moved[0], position[1] - body.last[1] + body.moved[1]], budget);
    let tip_motion = capped([tip[0] - body.last_tip[0] + body.moved_tip[0], tip[1] - body.last_tip[1] + body.moved_tip[1]], budget);
    // Where the origin itself lags, the tip does not have to swing to make up
    // for that part of the movement: only what is left turns the rod.
    let rod_motion = [tip_motion[0] - c.x * root_motion[0], tip_motion[1] - c.y * root_motion[1]];
    let force = [f.wind, -f.gravity];
    let lever = (tip[0] - position[0]).hypot(tip[1] - position[1]).max(MIN_LEVER);
    let heading = world.b.atan2(world.a);
    let spring = SpringStep::new(h, &f);
    let (move_x, move_y) = (c.x != 0.0, c.y != 0.0);
    let swing = c.rotate != 0.0 || c.shear_x != 0.0;
    let extend = c.scale_x != 0.0;
    if !move_x { (body.x, body.vx) = (0.0, 0.0); }
    if !move_y { (body.y, body.vy) = (0.0, 0.0); }
    if !swing { (body.rotate, body.vrotate) = (0.0, 0.0); }
    if !extend { (body.scale, body.vscale) = (0.0, 0.0); }

    // The limit is in world units: a rod's tip travels `lever` per radian.
    let (point_step, rod_speed, rod_step) = (c.limit * h, c.limit / lever, c.limit * h / lever);
    let bounded = |value: f32, before: f32| before + (value - before).clamp(-rod_step, rod_step);
    for _ in 0..steps {
        if move_x || move_y {
            if move_x { body.x -= share * root_motion[0]; }
            if move_y { body.y -= share * root_motion[1]; }
            let before = [body.x, body.y];
            if move_x { spring.advance(&mut body.x, &mut body.vx, force[0]); }
            if move_y { spring.advance(&mut body.y, &mut body.vy, force[1]); }
            let moved = capped([body.x - before[0], body.y - before[1]], point_step);
            (body.x, body.y) = (before[0] + moved[0], before[1] + moved[1]);
            // The second clamp only stops rounding from landing just past the limit.
            let v = capped([body.vx, body.vy], c.limit);
            (body.vx, body.vy) = (v[0].clamp(-c.limit, c.limit), v[1].clamp(-c.limit, c.limit));
        }
        if swing || extend {
            // The rod's axes follow its current swing, so a falling bone ends up
            // hanging rather than turning forever.
            let radial = turned(1.0, 0.0, heading + body.rotate);
            let tangent = [-radial[1], radial[0]];
            if swing {
                body.rotate -= share * dot(rod_motion, tangent) / lever;
                let before = body.rotate;
                spring.advance(&mut body.rotate, &mut body.vrotate, dot(force, tangent) / lever);
                body.rotate = bounded(body.rotate, before);
                // Whole turns look the same; keeping within one turn lets the
                // spring take the short way back.
                body.rotate = (body.rotate + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                body.vrotate = body.vrotate.clamp(-rod_speed, rod_speed);
            }
            if extend {
                body.scale -= share * dot(rod_motion, radial) / lever;
                let before = body.scale;
                spring.advance(&mut body.scale, &mut body.vscale, dot(force, radial) / lever);
                body.scale = bounded(body.scale, before);
                body.vscale = body.vscale.clamp(-rod_speed, rod_speed);
            }
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
