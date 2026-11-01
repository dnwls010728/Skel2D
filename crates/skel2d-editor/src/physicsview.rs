//! Physics while editing. Playing keeps one live simulation running across
//! loops. Stopped, the pose at the playhead comes from a cache of every frame
//! simulated from time 0. Scrubbing in either direction shows the
//! same result as playing up to that time, instead of jumping or resetting.

use std::cell::RefCell;

use skel2d_core::pose::Track;
use skel2d_core::{Document, LayerTrack, MovePath, PhysicsMode, PhysicsRun, PhysicsState, Pose};

/// Frame rate of the scrub cache, the same as the CLI's default.
pub const CACHE_FPS: f32 = 60.0;

/// How long "shake" keeps a stopped simulation running, in seconds.
const SHAKE_SECONDS: f32 = 2.0;
const SHAKE_SPEED: f32 = 300.0;

struct Cache {
    doc: Document,
    anim: Option<String>,
    layers: Vec<LayerTrack>,
    skins: Vec<String>,
    run: PhysicsRun,
    frames: Vec<PhysicsState>,
}

pub struct PhysicsView {
    /// Off shows the plain animated pose.
    pub enabled: bool,
    /// Freezes the live simulation at its current state.
    pub paused: bool,
    /// Simulates in the setup pose, reacting to bone edits. The document never changes.
    pub setup_preview: bool,
    pub(crate) live: PhysicsState,
    shake_left: f32,
    cache: RefCell<Option<Cache>>,
}

impl Default for PhysicsView {
    fn default() -> Self {
        Self { enabled: true, paused: false, setup_preview: false, live: PhysicsState::default(), shake_left: 0.0, cache: RefCell::new(None) }
    }
}

impl PhysicsView {
    /// Whether the pose comes from the live simulation rather than the cache.
    pub fn live_active(&self, playing: bool, setup: bool) -> bool {
        playing || self.shake_left > 0.0 || (setup && self.setup_preview)
    }

    /// Advances the live simulation by one editor frame.
    pub fn tick(&mut self, doc: &Document, tracks: &[Track], skins: &[String], dt: f32) {
        self.shake_left = (self.shake_left - dt).max(0.0);
        if !self.paused {
            Pose::compute_physics(doc, tracks, skins, &mut self.live, PhysicsMode::Update, dt);
        }
    }

    pub fn reset(&mut self, doc: &Document, tracks: &[Track], skins: &[String]) {
        self.shake_left = 0.0;
        Pose::compute_physics(doc, tracks, skins, &mut self.live, PhysicsMode::Reset, 0.0);
    }

    /// Kicks a constraint so its settings can be judged without animating.
    pub fn shake(&mut self, doc: &Document, tracks: &[Track], skins: &[String], name: &str) {
        if self.shake_left <= 0.0 && !self.paused {
            Pose::compute_physics(doc, tracks, skins, &mut self.live, PhysicsMode::Reset, 0.0);
        }
        self.live.impulse(name, SHAKE_SPEED, SHAKE_SPEED);
        self.shake_left = SHAKE_SECONDS;
    }

    /// The pose from the live simulation, without advancing it.
    pub fn live_pose(&self, doc: &Document, tracks: &[Track], skins: &[String]) -> Pose {
        let mut state = self.live.clone();
        Pose::compute_physics(doc, tracks, skins, &mut state, PhysicsMode::Pose, 0.0)
    }

    /// Continues the live simulation from where a stopped playhead shows it.
    pub fn start_live_at(&mut self, doc: &Document, anim: Option<&str>, layers: &[LayerTrack], skins: &[String], time: f32) {
        self.live = self.state_at(doc, anim, layers, skins, time);
    }

    /// The simulated state for a playhead time, filling the cache up to it.
    fn state_at(&self, doc: &Document, anim: Option<&str>, layers: &[LayerTrack], skins: &[String], time: f32) -> PhysicsState {
        let mut slot = self.cache.borrow_mut();
        let fresh = slot.as_ref().is_some_and(|c| c.anim.as_deref() == anim && c.layers == layers && c.skins == skins && c.doc == *doc);
        if !fresh {
            *slot = Some(Cache {
                doc: doc.clone(), anim: anim.map(str::to_string), layers: layers.to_vec(), skins: skins.to_vec(),
                run: PhysicsRun::new(CACHE_FPS, MovePath::default()), frames: Vec::new(),
            });
        }
        let Some(cache) = slot.as_mut() else { return PhysicsState::default() };
        let target = cache.run.frame_at(time);
        if cache.frames.len() as u64 <= target {
            let Cache { run, frames, .. } = cache;
            let tracks_at = |t: f32| skel2d_core::pose::preview_tracks(doc, anim, t, layers);
            run.advance_frames(doc, &tracks_at, skins, target, &mut |_, state| frames.push(state.clone()));
        }
        cache.frames.get(target as usize).cloned().unwrap_or_default()
    }

    /// The pose at a stopped playhead: identical to simulating from time 0.
    pub fn scrub_pose(&self, doc: &Document, anim: Option<&str>, layers: &[LayerTrack], skins: &[String], time: f32) -> Pose {
        let mut state = self.state_at(doc, anim, layers, skins, time);
        let tracks = skel2d_core::pose::preview_tracks(doc, anim, time, layers);
        Pose::compute_physics(doc, &tracks, skins, &mut state, PhysicsMode::Pose, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use skel2d_core::{Document, from_str};

    use super::*;

    fn doc() -> Document {
        from_str(&json!({
            "bones":[{"name":"root"},{"name":"hair","parent":"root","x":5,"length":10}],
            "physics":[{"name":"p","bone":"hair","x":1,"rotate":1,"strength":50,"damping":0.9}],
            "animations":{"a":{"duration":2,"bones":{"root":{"translate":[{"time":0,"value":[0,0]},{"time":0.5,"value":[40,0]}]}}}}
        }).to_string()).unwrap()
    }

    #[test]
    fn scrubbing_either_way_matches_the_simulation_from_zero() {
        let d = doc();
        let view = PhysicsView::default();
        let tracks = |t: f32| skel2d_core::pose::preview_tracks(&d, Some("a"), t, &[]);
        for t in [0.5, 0.1, 1.2, 0.3, 0.3, 1.9, 0.0] {
            let expected = Pose::simulate(&d, tracks, &[], t, CACHE_FPS).0;
            assert_eq!(view.scrub_pose(&d, Some("a"), &[], &[], t).bones, expected.bones, "{t}");
        }
    }

    #[test]
    fn document_changes_invalidate_the_cache() {
        let mut d = doc();
        let view = PhysicsView::default();
        let before = view.scrub_pose(&d, Some("a"), &[], &[], 0.25);
        d.physics[0].strength = 5.0;
        let after = view.scrub_pose(&d, Some("a"), &[], &[], 0.25);
        assert_ne!(before.bones, after.bones);
        let tracks = |t: f32| skel2d_core::pose::preview_tracks(&d, Some("a"), t, &[]);
        assert_eq!(after.bones, Pose::simulate(&d, tracks, &[], 0.25, CACHE_FPS).0.bones);
    }

    #[test]
    fn live_keeps_running_across_loops_and_pause_freezes_it() {
        let d = doc();
        let mut view = PhysicsView::default();
        view.start_live_at(&d, Some("a"), &[], &[], 0.2);
        let at = |t: f32| skel2d_core::pose::preview_tracks(&d, Some("a"), t, &[]);
        view.tick(&d, &at(0.25), &[], 0.05);
        let lagging = view.live.bodies[0].x;
        assert_ne!(lagging, 0.0);
        // Wrapping the clock keeps the state: only a reset key would restart it.
        view.tick(&d, &at(0.01), &[], 1.0 / 60.0);
        assert_ne!(view.live.bodies[0].x, 0.0);
        view.paused = true;
        let frozen = view.live.clone();
        view.tick(&d, &at(0.5), &[], 1.0);
        assert_eq!(view.live, frozen);
    }

    #[test]
    fn shake_moves_a_still_skeleton_then_stops_being_live() {
        let mut d = doc();
        d.animations.clear();
        let mut view = PhysicsView::default();
        assert!(!view.live_active(false, false));
        view.shake(&d, &[], &[], "p");
        assert!(view.live_active(false, false));
        view.tick(&d, &[], &[], 0.1);
        assert_ne!(view.live.bodies[0].x, 0.0);
        let shaken = view.live_pose(&d, &[], &[]);
        assert_ne!(shaken.bones[1].world.tx, 5.0);
        view.tick(&d, &[], &[], SHAKE_SECONDS);
        assert!(!view.live_active(false, false));
    }
}
