use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, TextureHandle};
use skel2d_core::pose::Track;
use skel2d_core::ops::KeyBone;
use skel2d_core::{Animation, Document, LayerTrack, Op, Pose};

use crate::icons::Icon;
use crate::viewport::{Camera, Drag, VertexDrag};

const MAX_UNDO: usize = 200;
/// Edits with the same coalesce key closer than this share one undo step
/// (e.g. dragging a number field).
const COALESCE_WINDOW: Duration = Duration::from_millis(600);
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);

#[cfg(test)]
mod history_shortcut_tests {
    use super::*;

    #[test]
    fn shifted_redo_does_not_get_consumed_as_undo() {
        let mut app=EditorApp::test_app(Document::new("before"),std::env::temp_dir().join("skel2d-history-test.skel.json"));
        assert!(app.edit(vec![Op::SetName {name:"after".into()}]));
        for (key,modifiers,expected) in [
            (Key::Z,Modifiers::COMMAND,"before"),
            (Key::Z,Modifiers::COMMAND.plus(Modifiers::SHIFT),"after"),
            (Key::Z,Modifiers::COMMAND,"before"),
            (Key::Y,Modifiers::COMMAND,"after"),
        ] {
            let ctx=egui::Context::default();
            let input=egui::RawInput {events:vec![egui::Event::Key {key,physical_key:None,pressed:true,repeat:false,modifiers}],..Default::default()};
            let mut output=ctx.run_pass(input,|ctx|app.shortcuts(ctx));output.textures_delta.clear();
            assert_eq!(app.doc.name,expected);
        }
    }
}
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Spine's Pose tool: select, drag a bone to move it (near its tip: rotate it).
    Select,
    Rotate,
    Translate,
    Scale,
    /// Skews the selected bone: the X handle shears its X axis, the Y handle its Y axis.
    Shear,
    /// Spine's Create tool: drag to draw a new bone.
    Bone,
    /// Add, remove and move vertices of the selected mesh.
    Mesh,
    /// Paint the selected bone's weights on the selected mesh.
    Weights,
}

impl Tool {
    /// Toolbar order and Spine's default hotkeys (Mesh has none in Spine).
    pub const ALL: [(Tool, &'static str, Key); 8] = [
        (Tool::Select, "Pose (B)", Key::B),
        (Tool::Bone, "Create (N)", Key::N),
        (Tool::Weights, "Weights (G)", Key::G),
        (Tool::Mesh, "Mesh (M)", Key::M),
        (Tool::Rotate, "Rotate (C)", Key::C),
        (Tool::Translate, "Translate (V)", Key::V),
        (Tool::Scale, "Scale (X)", Key::X),
        (Tool::Shear, "Shear (H)", Key::H),
    ];

    /// Toolbar label in English ("Create" for Bone), for `tr`.
    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|t| t.0 == self).map_or("", |t| t.1.split(' ').next().unwrap_or_default())
    }

    /// Lowercase name as shown on the toolbar ("create" for Bone).
    pub fn name(self) -> String {
        Self::ALL.iter().find(|t| t.0 == self).map_or("", |t| t.1.split(' ').next().unwrap_or_default()).to_lowercase()
    }

    /// Name shown on the tool buttons, in plain words (English, for `tr`).
    pub fn ui_label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Bone => "New bone",
            Tool::Translate => "Move",
            Tool::Rotate => "Rotate",
            Tool::Scale => "Scale",
            Tool::Shear => "Shear",
            Tool::Mesh => "Mesh",
            Tool::Weights => "Paint",
        }
    }

    /// One line saying what the tool does (English, for `tr`).
    pub fn tip(self) -> &'static str {
        match self {
            Tool::Select => "Click a bone or image to select it. Drag a bone to move it, near its tip to rotate it.",
            Tool::Bone => "Drag in the viewport to draw a bone under the selected bone (or root).",
            Tool::Translate => "Drag to move the selected bone (Ctrl: snap, Shift: fine, X/Y: lock axis).",
            Tool::Rotate => "Drag to rotate the selected bone (Ctrl: snap, Shift: fine).",
            Tool::Scale => "Drag to scale the selected bone (Ctrl: snap, Shift: fine).",
            Tool::Shear => "Drag to skew the selected bone. The X handle shears X, the Y handle or free drag shears Y (Ctrl: snap, Shift: fine).",
            Tool::Mesh => "Add, move and remove the mesh vertices of the picked image.",
            Tool::Weights => "Paint how strongly the picked bone moves the vertices of the image.",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::Select,
            Tool::Rotate => Icon::Rotate,
            Tool::Translate => Icon::Move,
            Tool::Scale => Icon::Scale,
            Tool::Shear => Icon::Shear,
            Tool::Bone => Icon::Bone,
            Tool::Mesh => Icon::Mesh,
            Tool::Weights => Icon::Weights,
        }
    }

    /// Tools that manipulate the selected bone with a gizmo.
    pub fn transforms(self) -> bool {
        matches!(self, Tool::Rotate | Tool::Translate | Tool::Scale | Tool::Shear)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Attachment row selected for rename/delete; owner is resolved through preview skins.
    pub attachment: Option<String>,
    pub bone: Option<String>,
    pub slot: Option<String>,
    pub ik: Option<String>,
    pub physics: Option<String>,
    pub transform: Option<String>,
    /// Skin shown in the inspector (picked in the Skins tab).
    pub skin: Option<String>,
}

pub struct Playback {
    /// `None` edits the setup pose.
    pub anim: Option<String>,
    /// Animation Tab returns to from the setup pose.
    pub last_anim: Option<String>,
    pub time: f32,
    pub playing: bool,
    pub looping: bool,
    /// Other animations that play on top of the main one (preview only).
    pub layers: Vec<LayerTrack>,
    /// Playback rate multiplier (1.0 = real time).
    pub speed: f32,
}

impl Playback {
    pub fn tracks<'a>(&self, doc: &'a Document, clock: f32) -> Vec<Track<'a>> {
        skel2d_core::pose::preview_tracks(doc, self.anim.as_deref(), clock, &self.layers)
    }

    pub fn layers_active(&self) -> bool {
        self.layers.iter().any(|l| l.enabled && l.alpha > 0.0 && Some(&l.anim) != self.anim.as_ref())
    }

    pub fn add_layer(&mut self, name: &str) {
        if !self.layers.iter().any(|layer| layer.anim == name) {
            self.layers.push(LayerTrack::new(name));
        }
    }

    /// Above in the UI means later in evaluation. Address tracks by name so
    /// dragging and keyboard actions survive reordering without stale indexes.
    pub fn reorder_layer(&mut self, name: &str, target: &str, above: bool) -> bool {
        if name == target { return false; }
        let (Some(from), Some(_)) = (self.layers.iter().position(|l| l.anim == name), self.layers.iter().position(|l| l.anim == target)) else { return false };
        let old = self.layers.clone();
        let moved = self.layers.remove(from);
        let to = self.layers.iter().position(|l| l.anim == target).expect("target was not removed");
        self.layers.insert(if above { to + 1 } else { to }, moved);
        self.layers != old
    }

    pub fn move_layer(&mut self, name: &str, up: bool) {
        let Some(i) = self.layers.iter().position(|l| l.anim == name) else { return };
        let next = if up { i.checked_add(1).filter(|n| *n < self.layers.len()) } else { i.checked_sub(1) };
        if let Some(next) = next { self.layers.swap(i, next); }
    }
}

#[cfg(test)]
mod playback_tests {
    use super::*;

    fn playback() -> Playback {
        Playback { anim: None, last_anim: None, time: 0.0, playing: false, looping: true, layers: Vec::new(), speed: 1.0 }
    }

    #[test]
    fn add_preserves_argument_order_and_reorder_preserves_track_settings() {
        let mut p = playback();
        for name in ["z", "a", "m", "z"] { p.add_layer(name); }
        assert_eq!(p.layers.iter().map(|l| l.anim.as_str()).collect::<Vec<_>>(), ["z", "a", "m"]);
        p.layers[1].alpha = 0.25;
        p.layers[1].enabled = false;
        assert!(p.reorder_layer("a", "m", true));
        assert_eq!(p.layers.iter().map(|l| l.anim.as_str()).collect::<Vec<_>>(), ["z", "m", "a"]);
        assert_eq!(p.layers[2].alpha, 0.25);
        assert!(!p.layers[2].enabled);
        let before = p.layers.clone();
        assert!(!p.reorder_layer("missing", "z", false));
        assert!(!p.reorder_layer("a", "a", true));
        assert!(!p.reorder_layer("z", "missing", true));
        assert_eq!(p.layers, before);
        p.move_layer("a", false);
        assert_eq!(p.layers[1].anim, "a");
        p.move_layer("a", true);
        assert_eq!(p.layers, before);
    }

    #[test]
    fn muted_zero_opacity_and_main_only_layers_do_not_advance_clock() {
        let mut p = playback();
        p.add_layer("main");
        p.anim = Some("main".into());
        assert!(!p.layers_active());
        p.add_layer("extra");
        assert!(p.layers_active());
        p.layers[1].alpha = 0.0;
        assert!(!p.layers_active());
        p.layers[1].alpha = 1.0;
        p.layers[1].enabled = false;
        assert!(!p.layers_active());
    }
}

/// Editor preferences, remembered between sessions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Overall UI zoom (1.0 = egui default sizes).
    pub ui_scale: f32,
    pub show_bone_names: bool,
    pub show_grid: bool,
    /// Ghosts of neighbouring frames in animate mode.
    pub onion_skin: bool,
    pub onion_frames: u32,
    /// Snapping on by default; holding Ctrl while dragging inverts it.
    pub snap: bool,
    /// Rotation snap step in degrees.
    pub snap_angle: f32,
    /// Move snap step in world units.
    pub snap_grid: f32,
    /// Timeline frame rate for snapping and stepping.
    pub fps: f32,
    /// Weight brush radius in screen pixels.
    pub brush_radius: f32,
    /// Weight added per second at the brush center (0..1).
    pub brush_strength: f32,
    /// Add raises up to, Replace moves toward this weight (0..1).
    pub brush_target: f32,
    /// Share of the brush radius over which its effect fades out (0..1).
    pub brush_feather: f32,
    pub brush_mode: crate::meshedit::BrushMode,
    /// Weights overlay: every bone in its own color instead of the paint bone's heat map.
    pub weights_all_bones: bool,
    /// Mesh editing display: triangle edges, dimmed image, other slots hidden.
    pub mesh_triangles: bool,
    pub mesh_dim: bool,
    pub mesh_isolate: bool,
    /// UI language; English is used anyway when no Korean font is found.
    pub language: crate::i18n::Lang,
    /// Last used GIF export settings.
    pub gif: skel2d_render::GifOptions,
    /// Moving bones in the setup pose carries meshes along, like regions.
    pub carry_meshes: bool,
    /// Animate mode: edits write keys at once. Off, they stay unkeyed (red)
    /// until keyed with a key button or K.
    pub auto_key: bool,
    /// The tree shows slots and their images under the bones (steps 1 and 5).
    pub tree_slots: bool,
    /// Height of the inspector under the tree; `None` until the divider is dragged.
    pub props_h: Option<f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            gif: skel2d_render::GifOptions::default(),
            show_bone_names: false,
            show_grid: true,
            onion_skin: false,
            onion_frames: 2,
            snap: false,
            snap_angle: 15.0,
            snap_grid: 10.0,
            fps: 30.0,
            brush_radius: 40.0,
            brush_strength: 0.5,
            brush_target: 1.0,
            brush_feather: 0.5,
            brush_mode: crate::meshedit::BrushMode::Add,
            weights_all_bones: false,
            mesh_triangles: true,
            mesh_dim: false,
            mesh_isolate: false,
            language: crate::i18n::Lang::default(),
            carry_meshes: true,
            auto_key: true,
            tree_slots: true,
            props_h: None,
        }
    }
}

/// Closed loops along an image's visible edge, in image coordinates.
pub type ImageOutline = std::sync::Arc<Vec<Vec<[f32; 2]>>>;

/// What replaces the current document once unsaved changes are settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Replace {
    New,
    /// A new document that then imports `EditorApp::import_after_new`.
    NewWithImport,
    Open,
    OpenExample,
}

pub(crate) enum Dialog {
    None,
    ExternalChange,
    ConfirmClose,
    /// Unsaved changes would be replaced by New or Open.
    ConfirmReplace(Replace),
    /// Asks how to split the PSDs among files being imported.
    PsdSplit(crate::import::PendingImport),
    /// Asks whether picked images start a new project or join the open one.
    ImportTarget(Vec<PathBuf>),
    /// Asks whether dropped PNGs go into the picked slot or become new slots.
    DropChoice(crate::import::PendingDrop),
    Editor(Box<crate::dialogs::EditorDialog>),
}

struct HistoryEntry {
    document: Document,
    /// Materialized core ops, never image-dependent requests.
    ops: Vec<Op>,
}

pub struct EditorApp {
    /// The GIF export running in the background, and what Ctrl+Shift+E repeats.
    pub gif_export: Option<crate::export::RunningExport>,
    pub last_gif: Option<crate::export::GifRequest>,
    /// Live and cached physics simulation; session state, not part of the document.
    pub physics: crate::physicsview::PhysicsView,
    physics_was_playing: bool,
    /// Events the playhead just passed while playing, with the time they fired.
    /// Session state shown for a moment in the viewport; never stored in the document.
    pub event_toasts: Vec<(String, Instant)>,
    pub doc: Document,
    saved: Document,
    pub path: Option<PathBuf>,
    file_mtime: Option<SystemTime>,
    last_watch: Instant,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    last_coalesce: Option<(String, Instant)>,
    pub selection: Selection,
    pub playback: Playback,
    pub tool: Tool,
    /// Work step (1-5): decides the layout and which tools are offered.
    pub step: crate::steps::Step,
    /// Setup step that Tab returns to from the animation step.
    pub last_setup_step: crate::steps::Step,
    /// Tool before the current one; right-click in the viewport switches back.
    pub prev_tool: Tool,
    pub camera: Camera,
    pub drag: Option<Drag>,
    pub vertex_drag: Option<VertexDrag>,
    pub show_bones: bool,
    pub settings: Settings,
    /// View-only visibility and locking (not saved in the document).
    /// Skins previewed in the viewport, lowest first (view state; Spine's
    /// Skins view pins several the same way). Edits to an attachment a skin
    /// supplies go into that skin.
    pub skins: Vec<String>,
    pub hidden_bones: BTreeSet<String>,
    pub hidden_slots: BTreeSet<String>,
    pub locked_bones: BTreeSet<String>,
    pub locked_slots: BTreeSet<String>,
    /// Contextual help for the status bar, set by whatever the pointer is over.
    pub hint: String,
    shown_hint: String,
    pub show_help: bool,
    pub view: crate::viewport::ViewState,
    pub timeline: crate::timeline::TimelineState,
    applied_scale: f32,
    textures: HashMap<String, Option<TextureHandle>>,
    outlines: HashMap<String, Option<ImageOutline>>,
    pub status: String,
    /// `status` holds why the last preview was refused.
    preview_refused: bool,
    /// Unkeyed animate-mode changes per bone (Auto Key off), all at the
    /// current animation and time; dropped when the playhead moves.
    pub pending: BTreeMap<String, KeyBone>,
    /// Unkeyed changes to everything that is not a bone's transform (deform,
    /// slot attachment and color, IK, transform and physics values), at the
    /// same animation and time as `pending`. One entry per property.
    pub pending_other: Vec<Op>,
    /// Transient messages as a stack in the viewport corner.
    pub toasts: crate::toasts::Toasts,
    /// The last `status` turned into a toast (a changed status makes a new one).
    status_seen: String,
    /// Meshes ("slot/attachment") whose tangle was already warned about.
    tangled: BTreeSet<String>,
    /// Command palette (Ctrl+K).
    pub palette: crate::palette::PaletteState,
    pub(crate) dialog: Dialog,
    allow_close: bool,
    last_title: String,
    screenshot: Option<Screenshot>,
    pub blend_pass: bool,
    blend_images: HashMap<egui::TextureId, std::sync::Arc<crate::blendpass::Pixels>>,
    /// Images waiting for the new document of [`Replace::NewWithImport`].
    import_after_new: Vec<PathBuf>,
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>, fresh_settings: bool, lang: Option<crate::i18n::Lang>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        crate::icons::load(&cc.egui_ctx);
        let remembered: Option<Settings> = cc.storage.and_then(|s| eframe::get_value(s, "settings"));
        let mut settings = if fresh_settings { Settings::default() } else { remembered.unwrap_or_default() };
        if let Some(lang) = lang {
            settings.language = lang;
        }
        // The first messages (opening the file) already use the chosen language.
        crate::i18n::set(if settings.language == crate::i18n::Lang::Korean && crate::theme::has_korean_font() { crate::i18n::Lang::Korean } else { crate::i18n::Lang::English });
        if fresh_settings {
            // Scripted screenshots: no tooltip stuck under the pointer.
            cc.egui_ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 600.0);
        }
        let mut app = Self::empty(settings);
        if let Some(p) = path {
            if p.exists() {
                app.open(&p);
            } else {
                // Start a new document that will be written to `p` on save.
                app.doc.name = p.file_stem().map(|s| s.to_string_lossy().replace(".skel", "")).unwrap_or_default();
                app.saved = app.doc.clone();
                app.path = Some(p);
            }
        }
        app
    }

    fn empty(settings: Settings) -> Self {
        Self {
            doc: Document::with_root("untitled"),
            saved: Document::with_root("untitled"),
            path: None,
            file_mtime: None,
            last_watch: Instant::now(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_coalesce: None,
            selection: Selection::default(),
            physics: Default::default(),
            gif_export: None,
            last_gif: None,
            physics_was_playing: false,
            event_toasts: Vec::new(),
            playback: Playback { anim: None, last_anim: None, time: 0.0, playing: false, looping: true, layers: Vec::new(), speed: 1.0 },
            tool: Tool::Select,
            step: crate::steps::Step::Bones,
            last_setup_step: crate::steps::Step::Bones,
            prev_tool: Tool::Rotate,
            camera: Camera::default(),
            drag: None,
            vertex_drag: None,
            show_bones: true,
            settings,
            skins: Vec::new(),
            hidden_bones: BTreeSet::new(),
            hidden_slots: BTreeSet::new(),
            locked_bones: BTreeSet::new(),
            locked_slots: BTreeSet::new(),
            hint: String::new(),
            shown_hint: String::new(),
            show_help: false,
            view: Default::default(),
            timeline: Default::default(),
            applied_scale: 0.0,
            textures: HashMap::new(),
            outlines: HashMap::new(),
            status: String::new(),
            preview_refused: false,
            pending: BTreeMap::new(),
            pending_other: Vec::new(),
            toasts: Default::default(),
            status_seen: String::new(),
            tangled: BTreeSet::new(),
            palette: Default::default(),
            dialog: Dialog::None,
            allow_close: false,
            last_title: String::new(),
            screenshot: None,
            blend_pass: false,
            blend_images: HashMap::new(),
            import_after_new: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_app(doc: Document, path: PathBuf) -> Self {
        let mut app = Self::empty(Settings::default());
        app.saved = doc.clone();
        app.doc = doc;
        app.path = Some(path);
        app
    }

    pub fn dirty(&self) -> bool {
        self.doc != self.saved
    }

    pub fn animation(&self) -> Option<&Animation> {
        self.playback.anim.as_ref().and_then(|a| self.doc.animations.get(a))
    }

    /// Pose at the playback clock: the main animation, then the preview
    /// layers (each looping over its own duration), like Spine tracks.
    pub fn pose(&self) -> Pose {
        if self.tool==Tool::Weights && self.playback.anim.is_none() {
            return crate::viewport::weight_test_pose(&self.doc,&self.skins,self.view.pose_rotation.as_ref());
        }
        // Bones and meshes are built in the setup pose without IK or layers
        // (as `add_bone_world` and mesh vertices define it), so the Bone and
        // Mesh tools show exactly that space.
        if self.playback.anim.is_none() && matches!(self.tool, Tool::Bone | Tool::Mesh) {
            let mut setup = self.doc.clone();
            setup.ik.clear();
            setup.transforms.clear();
            return Pose::compute_skinned(&setup, &[], &self.skins);
        }
        // Unkeyed changes show as if keyed.
        let shown = self.shown_doc();
        let doc: &Document = &shown;
        let tracks = self.playback.tracks(doc, self.playback.time);
        if doc.physics.is_empty() || !self.physics.enabled {
            return Pose::compute_skinned(doc, &tracks, &self.skins);
        }
        if self.physics.live_active(self.playback.playing, self.playback.anim.is_none()) {
            return self.physics.live_pose(doc, &tracks, &self.skins);
        }
        // Stopped with nothing animated there is nothing to simulate.
        if tracks.is_empty() {
            return Pose::compute_skinned(doc, &tracks, &self.skins);
        }
        self.physics.scrub_pose(doc, self.playback.anim.as_deref(), &self.playback.layers, &self.skins, self.playback.time)
    }

    /// Restarts the live simulation from the current pose (Shift+R).
    pub fn physics_reset(&mut self) {
        let tracks = self.playback.tracks(&self.doc, self.playback.time);
        self.physics.reset(&self.doc, &tracks, &self.skins);
    }

    /// Kicks one constraint to judge its settings; a preview, the document is untouched.
    pub fn physics_shake(&mut self, name: &str) {
        let tracks = self.playback.tracks(&self.doc, self.playback.time);
        self.physics.shake(&self.doc, &tracks, &self.skins, name);
    }

    /// Advances the live simulation after the playback clock has moved.
    fn advance_physics(&mut self, ctx: &egui::Context, was_playing: bool) {
        if self.doc.physics.is_empty() || !self.physics.enabled { return; }
        let playing = self.playback.playing;
        if playing && !was_playing {
            self.physics.start_live_at(&self.doc, self.playback.anim.as_deref(), &self.playback.layers, &self.skins, self.playback.time);
        }
        if !self.physics.live_active(playing, self.playback.anim.is_none()) { return; }
        let speed = if playing { self.playback.speed.clamp(0.05, 8.0) } else { 1.0 };
        let dt = ctx.input(|i| i.stable_dt).min(0.1) * speed;
        let tracks = self.playback.tracks(&self.doc, self.playback.time);
        self.physics.tick(&self.doc, &tracks, &self.skins, dt);
        ctx.request_repaint();
    }

    /// Applies ops as one undo step. Returns false and shows the error on failure.
    /// With Auto Key off, bone keys in animate mode become unkeyed changes instead.
    pub fn edit(&mut self, ops: Vec<Op>) -> bool {
        self.edit_inner(ops, None)
    }

    /// Like `edit`, but consecutive edits with the same key merge into one undo step.
    pub fn edit_coalesced(&mut self, key: impl Into<String>, ops: Vec<Op>) -> bool {
        self.edit_inner(ops, Some(key.into()))
    }

    /// Like `edit`, but always writes keys (key buttons and K, whatever Auto Key says).
    pub fn edit_keyed(&mut self, ops: Vec<Op>) -> bool {
        let keyed = ops.clone();
        let ok = self.apply_edit(ops, None);
        if ok {
            self.release_pending(&keyed);
        }
        ok
    }

    /// The unkeyed change for `bone` at the playhead, if any.
    pub fn pending_key(&self, bone: &str) -> Option<&KeyBone> {
        self.pending.get(bone)
    }

    /// Every unkeyed change as the key ops that would write it.
    pub fn pending_ops(&self) -> Vec<Op> {
        self.pending.values().cloned().map(Op::KeyBone).chain(self.pending_other.iter().cloned()).collect()
    }

    /// How many properties hold an unkeyed change.
    pub fn pending_count(&self) -> usize {
        let bone_fields: usize = self.pending.values().map(|k| [k.rotate.is_some(), k.translate.is_some(), k.scale.is_some(), k.shear.is_some()].iter().filter(|x| **x).count()).sum();
        bone_fields + self.pending_other.len()
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.pending_other.is_empty()
    }

    /// The document with unkeyed changes applied, as the viewport and the
    /// inspectors show it. The stored document is never touched.
    pub fn shown_doc(&self) -> std::borrow::Cow<'_, Document> {
        if !self.has_pending() {
            return std::borrow::Cow::Borrowed(&self.doc);
        }
        match skel2d_core::apply(&self.doc, &self.pending_ops()) {
            Ok(doc) => std::borrow::Cow::Owned(doc),
            Err(_) => std::borrow::Cow::Borrowed(&self.doc),
        }
    }

    /// Whether `op` is a value edit that waits for a key when Auto Key is off.
    fn holdable(op: &Op, anim: &str, t: f32) -> bool {
        let near = |x: f32| (x - t).abs() < skel2d_core::ops::TIME_EPSILON;
        match op {
            Op::KeyBone(k) => k.animation == anim && near(k.time),
            Op::KeySlot(k) => k.animation == anim && near(k.time),
            Op::KeyIk { animation, time, .. } | Op::KeyTransform { animation, time, .. } | Op::KeyPhysics { animation, time, .. } | Op::KeyDeform { animation, time, .. } => animation == anim && near(*time),
            _ => false,
        }
    }

    /// Same property (so a newer change replaces the older one).
    fn same_property(a: &Op, b: &Op) -> bool {
        match (a, b) {
            (Op::KeySlot(x), Op::KeySlot(y)) => x.slot == y.slot && (x.color.is_some() == y.color.is_some()),
            (Op::KeyIk { ik: x, .. }, Op::KeyIk { ik: y, .. }) => x == y,
            (Op::KeyTransform { constraint: x, .. }, Op::KeyTransform { constraint: y, .. }) => x == y,
            (Op::KeyPhysics { constraint: x, property: p, .. }, Op::KeyPhysics { constraint: y, property: q, .. }) => x == y && p == q,
            (Op::KeyDeform { slot: s, attachment: a, skin: k, .. }, Op::KeyDeform { slot: t, attachment: b, skin: l, .. }) => s == t && a == b && k == l,
            _ => false,
        }
    }

    /// With Auto Key off in animate mode, value edits at the playhead are held
    /// as unkeyed changes (merged per property). Returns true when `ops` were
    /// held. A mix with anything else (draw order, events, other times) is
    /// written as a whole, so no edit is half held.
    fn stash(&mut self, ops: &[Op]) -> bool {
        let Some(anim) = self.playback.anim.clone() else { return false };
        let t = self.playback.time;
        if self.settings.auto_key || ops.is_empty() || !ops.iter().all(|o| Self::holdable(o, &anim, t)) {
            return false;
        }
        for op in ops {
            match op {
                Op::KeyBone(k) => {
                    let p = self.pending.entry(k.bone.clone()).or_insert_with(|| KeyBone { rotate: None, translate: None, scale: None, shear: None, curve: None, curves: None, ..k.clone() });
                    p.rotate = k.rotate.or(p.rotate);
                    p.translate = k.translate.or(p.translate);
                    p.scale = k.scale.or(p.scale);
                    p.shear = k.shear.or(p.shear);
                }
                Op::KeySlot(k) => {
                    // The attachment and the tint are two properties.
                    let mut parts = Vec::new();
                    if k.attachment.is_some() || k.reset {
                        parts.push(Op::KeySlot(skel2d_core::ops::KeySlot { color: None, curve: None, curves: None, ..k.clone() }));
                    }
                    if k.color.is_some() {
                        parts.push(Op::KeySlot(skel2d_core::ops::KeySlot { attachment: None, reset: false, ..k.clone() }));
                    }
                    for part in parts {
                        self.pending_other.retain(|o| !(Self::same_property(o, &part) && Self::slot_part(o) == Self::slot_part(&part)));
                        self.pending_other.push(part);
                    }
                }
                Op::KeyIk { ik, mix, softness, bend_positive, compress, stretch, .. } => {
                    let held = self.pending_other.iter_mut().find_map(|o| match o { Op::KeyIk { ik: i, mix: m, softness: s, bend_positive: b, compress: c, stretch: x, .. } if i == ik => Some((m, s, b, c, x)), _ => None });
                    match held {
                        Some((m, s, b, c, x)) => {
                            *m = mix.or(*m);
                            *s = softness.or(*s);
                            *b = bend_positive.or(*b);
                            *c = compress.or(*c);
                            *x = stretch.or(*x);
                        }
                        None => {
                            let (animation, time) = (anim.clone(), t);
                            self.pending_other.push(Op::KeyIk { animation, ik: ik.clone(), time, mix: *mix, softness: *softness, bend_positive: *bend_positive, compress: *compress, stretch: *stretch, curve: None, curves: None });
                        }
                    }
                }
                other => {
                    self.pending_other.retain(|o| !Self::same_property(o, other));
                    self.pending_other.push(other.clone());
                }
            }
        }
        true
    }

    /// 0 for a slot key that sets the attachment, 1 for the tint.
    fn slot_part(op: &Op) -> u8 {
        match op {
            Op::KeySlot(k) => u8::from(k.color.is_some()),
            _ => 0,
        }
    }

    /// Forgets the unkeyed changes that `ops` have just keyed.
    fn release_pending(&mut self, ops: &[Op]) {
        for op in ops {
            match op {
                Op::KeyBone(k) => {
                    if let Some(p) = self.pending.get_mut(&k.bone) {
                        if k.rotate.is_some() { p.rotate = None; }
                        if k.translate.is_some() { p.translate = None; }
                        if k.scale.is_some() { p.scale = None; }
                        if k.shear.is_some() { p.shear = None; }
                        if p.rotate.is_none() && p.translate.is_none() && p.scale.is_none() && p.shear.is_none() {
                            self.pending.remove(&k.bone);
                        }
                    }
                }
                Op::KeySlot(k) => {
                    if k.attachment.is_some() || k.reset {
                        self.pending_other.retain(|o| !matches!(o, Op::KeySlot(p) if p.slot == k.slot && p.color.is_none()));
                    }
                    if k.color.is_some() {
                        self.pending_other.retain(|o| !matches!(o, Op::KeySlot(p) if p.slot == k.slot && p.color.is_some()));
                    }
                }
                Op::KeyIk { ik, mix, softness, bend_positive, compress, stretch, .. } => {
                    self.pending_other.retain_mut(|o| match o {
                        Op::KeyIk { ik: i, mix: m, softness: s, bend_positive: b, compress: c, stretch: x, .. } if i == ik => {
                            if mix.is_some() { *m = None; }
                            if softness.is_some() { *s = None; }
                            if bend_positive.is_some() { *b = None; }
                            if compress.is_some() { *c = None; }
                            if stretch.is_some() { *x = None; }
                            m.is_some() || s.is_some() || b.is_some() || c.is_some() || x.is_some()
                        }
                        _ => true,
                    });
                }
                other => self.pending_other.retain(|o| !Self::same_property(o, other)),
            }
        }
    }

    /// Drops unkeyed changes once the animation or playhead moves on, and
    /// says so (Spine discards them silently).
    /// Every new `status` text also becomes a toast (one place instead of each
    /// of the places that set it). The same text arriving again while it shows
    /// counts up instead of stacking.
    pub(crate) fn observe_status(&mut self) {
        if self.status == self.status_seen {
            return;
        }
        self.status_seen.clone_from(&self.status);
        if !self.status.is_empty() {
            let kind = crate::toasts::classify(&self.status);
            self.toasts.push(kind, self.status.clone());
        }
    }

    /// Warns once when an edit left a mesh tangled, and again only after it was fixed.
    fn check_tangles(&mut self, ops: &[Op]) {
        for op in ops {
            let Op::SetVertices { slot, attachment, skin, .. } = op else { continue };
            let mesh = match skin {
                Some(k) => self.doc.skin(k).and_then(|s| s.attachment(slot, attachment)),
                None => self.doc.slot(slot).and_then(|s| s.attachment(attachment)),
            };
            let problem = match mesh {
                Some(skel2d_core::Attachment::Mesh(m)) => skel2d_core::mesh::geometry_problem(m),
                _ => None,
            };
            let key = format!("{slot}/{attachment}");
            match problem {
                Some(p) if self.tangled.insert(key.clone()) => {
                    let text = crate::errors::describe(&p);
                    self.toasts.push(crate::toasts::Kind::Warning, crate::tf!("{} is tangled: {}", attachment, text));
                }
                Some(_) => {}
                None => {
                    self.tangled.remove(&key);
                }
            }
        }
    }

    pub(crate) fn drop_stale_pending(&mut self) {
        let (anim, t) = (self.playback.anim.as_deref(), self.playback.time);
        fn key_time(op: &Op) -> Option<(&str, f32)> {
            match op {
                Op::KeyBone(k) => Some((k.animation.as_str(), k.time)),
                Op::KeySlot(k) => Some((k.animation.as_str(), k.time)),
                Op::KeyIk { animation, time, .. } | Op::KeyTransform { animation, time, .. } | Op::KeyPhysics { animation, time, .. } | Op::KeyDeform { animation, time, .. } => Some((animation.as_str(), *time)),
                _ => None,
            }
        }
        let stale = self.pending_ops().iter().filter_map(key_time).any(|(a, time)| Some(a) != anim || (time - t).abs() >= skel2d_core::ops::TIME_EPSILON);
        if stale {
            let n = self.pending_count();
            self.pending.clear();
            self.pending_other.clear();
            self.status = crate::tf!("Discarded {} unkeyed changes (key them with K or the key buttons before moving the playhead)", n);
        }
    }

    fn edit_inner(&mut self, ops: Vec<Op>, key: Option<String>) -> bool {
        if self.stash(&ops) {
            return true;
        }
        self.apply_edit(ops, key)
    }

    fn apply_edit(&mut self, ops: Vec<Op>, key: Option<String>) -> bool {
        if ops.is_empty() {
            return true;
        }
        let ops = self.retarget_skins(ops);
        self.apply_any_edit(ops.into_iter().map(skel2d_rig::AnyOp::Core).collect(),key)
    }

    pub fn edit_any(&mut self, ops: Vec<skel2d_rig::AnyOp>) -> bool {
        if self.playback.anim.is_some() && ops.iter().any(|op|matches!(op,skel2d_rig::AnyOp::Asset(_))) {
            self.status=crate::i18n::tr("Asset edits require setup pose").into(); return false;
        }
        self.apply_any_edit(ops,None)
    }

    fn apply_any_edit(&mut self, ops: Vec<skel2d_rig::AnyOp>, key: Option<String>) -> bool {
        match skel2d_rig::apply_any(&self.doc, &self.base_dir(), &ops) {
            Ok(report) => {
                let (next,ops)=(report.document,report.ops);
                let next = match self.carry(&self.doc, next, &ops) {
                    Ok(next)=>next,
                    Err(error)=>{self.status=error;return false;}
                };
                if ops.iter().any(|op|matches!(op,Op::AddMeshVertices {..}|Op::RemoveMeshVertices {..}|Op::SetMeshEdges {..}|Op::GenerateMeshVertices {..}|Op::ResetMesh {..}|Op::SetAttachment {attachment:skel2d_core::Attachment::Mesh(_),..}|Op::SetSkinAttachment {attachment:skel2d_core::Attachment::Mesh(_),..}))
                    || (self.settings.carry_meshes && self.playback.anim.is_none() && ops.iter().any(|op|matches!(op,Op::SetBone(_)))) {
                    self.view.clear_mesh_selection();
                }
                let merge = match (&key, &self.last_coalesce) {
                    (Some(k), Some((last, at))) => k == last && at.elapsed() < COALESCE_WINDOW,
                    _ => false,
                };
                let applied = ops.clone();
                if !merge {
                    self.push_undo();
                }
                self.retarget_weight_state(&ops,false);
                if let Some(entry)=self.undo.last_mut() { entry.ops.extend(ops); }
                self.last_coalesce = key.map(|k| (k, Instant::now()));
                self.doc = next;
                self.check_tangles(&applied);
                self.status.clear();
                self.fix_selection();
                true
            }
            Err(e) => {
                self.status = crate::errors::describe(&e.to_string());
                false
            }
        }
    }

    /// The previewed skin that supplies attachment `name` of `slot`, if any
    /// (the last one wins, as in the pose).
    pub fn skin_of(&self, slot: &str, name: &str) -> Option<String> {
        self.skins.iter().rev().find(|k| self.doc.skin(k).is_some_and(|s| s.attachment(slot, name).is_some())).cloned()
    }

    /// Edits of an attachment that a previewed skin supplies go into that
    /// skin (Spine edits whatever the active skin shows), so panels and tools
    /// can keep writing slot-level ops.
    pub(crate) fn retarget_skins(&self, ops: Vec<Op>) -> Vec<Op> {
        if self.skins.is_empty() {
            return ops;
        }
        ops.into_iter()
            .map(|op| match op {
                Op::SetAttachment { slot, attachment } => match self.skin_of(&slot, attachment.name()) {
                    Some(skin) => Op::SetSkinAttachment { skin, slot, attachment },
                    None => Op::SetAttachment { slot, attachment },
                },
                Op::SetVertices { slot, attachment, vertices, skin: None } => {
                    let skin = self.skin_of(&slot, &attachment);
                    Op::SetVertices { slot, attachment, vertices, skin }
                }
                Op::AutoWeights { locked_bones, slot, attachment, bones, max_influences, falloff, vertices, skin: None } => {
                    let skin = self.skin_of(&slot, &attachment);
                    Op::AutoWeights { locked_bones, slot, attachment, bones, max_influences, falloff, vertices, skin }
                }
                Op::MirrorWeights { locked_bones, slot, attachment, axis, source, skin: None } => {
                    let skin = self.skin_of(&slot, &attachment);
                    Op::MirrorWeights { locked_bones, slot, attachment, axis, source, skin }
                }
                Op::TransferWeights { locked_bones, slot, attachment, from_slot, from_attachment, vertices, skin: None, from_skin: None } => {
                    let (skin, from_skin) = (self.skin_of(&slot, &attachment), self.skin_of(&from_slot, &from_attachment));
                    Op::TransferWeights { locked_bones, slot, attachment, from_slot, from_attachment, vertices, skin, from_skin }
                }
                other => other,
            })
            .collect()
    }

    /// Meshes follow setup-pose bone edits (see `import::carry_meshes`).
    fn carry(&self, before: &Document, after: Document, ops: &[Op]) -> std::result::Result<Document, String> {
        if self.settings.carry_meshes && ops.iter().any(|o| matches!(o, Op::SetBone(_))) {
            crate::import::carry_meshes(before, after)
        } else {
            Ok(after)
        }
    }

    /// Replaces the document without an undo step (used while dragging; the
    /// undo step is pushed when the drag starts).
    pub fn preview(&mut self, base: &Document, ops: &[Op]) {
        if self.stash(ops) {
            self.doc = base.clone();
            return;
        }
        let ops = &self.retarget_skins(ops.to_vec());
        match skel2d_rig::apply_any(base, &self.base_dir(), &ops.iter().cloned().map(skel2d_rig::AnyOp::Core).collect::<Vec<_>>()) {
            Ok(report) => match self.carry(base, report.document, &report.ops) {
                Ok(next)=>{
                    self.doc=next;
                    self.check_tangles(ops);
                    // A refusal earlier in this drag no longer applies.
                    if std::mem::take(&mut self.preview_refused) { self.status.clear(); }
                }
                Err(error)=>self.status=error,
            },
            Err(e) => {
                self.status = crate::errors::describe(&e.to_string());
                self.preview_refused = true;
            }
        }
    }

    pub fn push_undo(&mut self) {
        self.undo.push(HistoryEntry {document:self.doc.clone(),ops:Vec::new()});
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_coalesce = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.has_pending()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.view.clear_mesh_selection();
        // Unkeyed changes are the newest edit: undo drops them first.
        if self.has_pending() {
            self.pending.clear();
            self.pending_other.clear();
            return;
        }
        if let Some(prev) = self.undo.pop() {
            self.retarget_weight_state(&prev.ops,true);
            self.redo.push(HistoryEntry {document:std::mem::replace(&mut self.doc, prev.document),ops:prev.ops});
            self.last_coalesce = None;
            self.fix_selection();
        }
    }

    pub fn redo(&mut self) {
        self.view.clear_mesh_selection();
        if let Some(next) = self.redo.pop() {
            self.retarget_weight_state(&next.ops,false);
            self.undo.push(HistoryEntry {document:std::mem::replace(&mut self.doc, next.document),ops:next.ops});
            self.last_coalesce = None;
            self.fix_selection();
        }
    }

    /// Session weight tools follow bone renames, including history replay.
    fn retarget_weight_state(&mut self, ops:&[Op], reverse:bool) {
        let mut ordered:Vec<_>=ops.iter().collect();
        if reverse {ordered.reverse();}
        for op in ordered {
            if let Op::SetBone(p)=op && let Some(name)=&p.rename {
                let (old,new)=if reverse {(name,&p.name)} else {(&p.name,name)};
                if self.view.weight_locked.remove(old) {self.view.weight_locked.insert(new.clone());}
                if let Some(weights)=&mut self.view.weight_clipboard && let Some(value)=weights.remove(old) {weights.insert(new.clone(),value);}
                if let Some((bone,_))=&mut self.view.pose_rotation && bone==old {*bone=new.clone();}
            }
        }
    }

    /// Drops selections that no longer exist after an edit, undo or reload.
    fn fix_selection(&mut self) {
        self.view.weight_locked.retain(|b|self.doc.bone(b).is_some());
        if let Some(weights)=&mut self.view.weight_clipboard {weights.retain(|b,_|self.doc.bone(b).is_some());}
        if self.selection.attachment.as_ref().is_some_and(|a| self.selection.slot.as_ref().is_none_or(|s| !skel2d_core::ops::slot_has_attachment(&self.doc, s, a))) {
            self.selection.attachment = None;
        }
        if self.selection.bone.as_ref().is_some_and(|b| self.doc.bone(b).is_none()) {
            self.selection.bone = None;
        }
        if self.selection.slot.as_ref().is_some_and(|s| self.doc.slot(s).is_none()) {
            self.selection.slot = None;
        }
        if self.selection.ik.as_ref().is_some_and(|n| !self.doc.ik.iter().any(|c| &c.name == n)) {
            self.selection.ik = None;
        }
        if self.selection.transform.as_ref().is_some_and(|n| !self.doc.transforms.iter().any(|c| &c.name == n)) {
            self.selection.transform = None;
        }
        if self.selection.physics.as_ref().is_some_and(|n| self.doc.physics_constraint(n).is_none()) {
            self.selection.physics = None;
        }
        if self.selection.skin.as_ref().is_some_and(|k| self.doc.skin(k).is_none()) {
            self.selection.skin = None;
        }
        let doc = &self.doc;
        self.skins.retain(|k| doc.skin(k).is_some());
        self.playback.layers.retain(|layer| self.doc.animations.contains_key(&layer.anim));
        if self.playback.anim.as_ref().is_some_and(|a| !self.doc.animations.contains_key(a)) {
            self.playback.anim = None;
            self.playback.playing = false;
        }
    }

    pub fn new_document(&mut self) {
        self.push_undo();
        self.doc = Document::with_root("untitled");
        self.saved = self.doc.clone();
        self.path = None;
        self.file_mtime = None;
        self.textures.clear();
        self.outlines.clear();
        self.selection = Selection::default();
        self.playback.anim = None;
        self.playback.layers.clear();
        self.reset_steps();
        self.camera = Camera::default();
    }

    pub fn open(&mut self, path: &Path) {
        match skel2d_core::load(path) {
            Ok(doc) => {
                self.doc = doc;
                self.saved = self.doc.clone();
                self.path = Some(path.to_path_buf());
                self.file_mtime = mtime(path);
                self.undo.clear();
                self.redo.clear();
                self.textures.clear();
                self.outlines.clear();
                self.selection = Selection::default();
                self.playback.anim = None;
                self.playback.layers.clear();
                self.playback.time = 0.0;
                self.reset_steps();
                self.camera.needs_fit = true;
                self.status = crate::tf!("Opened {}", path.display());
            }
            Err(e) => self.status = crate::tf!("Cannot open {}: {}", path.display(), e),
        }
    }

    /// File dialog that starts in the `examples` folder next to the program or the project, when found.
    pub fn open_example_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("Skel2D", &["json"]);
        let start = std::env::current_exe().ok().and_then(|exe| exe.ancestors().skip(1).take(5).map(|d| d.join("examples")).find(|d| d.is_dir()));
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(p) = dialog.pick_file() {
            self.open(&p);
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Skel2D", &["json"]).pick_file() {
            self.open(&p);
        }
    }

    /// Asks first when the document has unsaved changes; otherwise runs `what` now.
    pub fn request_replace(&mut self, what: Replace) {
        if self.dirty() {
            self.dialog = Dialog::ConfirmReplace(what);
        } else {
            self.run_replace(what);
        }
    }

    fn run_replace(&mut self, what: Replace) {
        match what {
            Replace::New => self.new_document(),
            Replace::NewWithImport => {
                self.new_document();
                let files = std::mem::take(&mut self.import_after_new);
                crate::import::import_images(self, &files, [0.0, 0.0], None);
            }
            Replace::Open => self.open_dialog(),
            Replace::OpenExample => self.open_example_dialog(),
        }
    }

    /// The Save choice of the confirmation: continues only when the save really cleared the changes.
    pub(crate) fn save_then_replace(&mut self, what: Replace) -> bool {
        self.save();
        if self.dirty() {
            return false;
        }
        self.dialog = Dialog::None;
        self.run_replace(what);
        true
    }

    pub fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            self.save_as();
            return;
        };
        match skel2d_core::save(&path, &self.doc) {
            Ok(()) => {
                self.saved = self.doc.clone();
                self.file_mtime = mtime(&path);
                self.status = crate::tf!("Saved {}", path.display());
            }
            Err(e) => self.status = crate::tf!("Save failed: {}", e),
        }
    }

    pub fn save_as(&mut self) {
        // The default file name stays English: it is a file name, not UI text.
        let name = format!("{}.skel.json", if self.doc.name.is_empty() { "untitled" } else { &self.doc.name });
        if let Some(p) = rfd::FileDialog::new().add_filter("Skel2D", &["json"]).set_file_name(name).save_file() {
            self.path = Some(p);
            self.textures.clear();
            self.outlines.clear();
            self.save();
        }
    }

    /// Polls the file so edits made by the CLI, HTTP or MCP servers show up live.
    fn watch_file(&mut self) {
        if self.last_watch.elapsed() < WATCH_INTERVAL || !matches!(self.dialog, Dialog::None) {
            return;
        }
        self.last_watch = Instant::now();
        let Some(path) = self.path.clone() else { return };
        let now = mtime(&path);
        if now.is_none() || now == self.file_mtime {
            return;
        }
        if self.dirty() {
            self.dialog = Dialog::ExternalChange;
        } else {
            self.reload_external(&path);
        }
    }

    fn reload_external(&mut self, path: &Path) {
        self.file_mtime = mtime(path);
        match skel2d_core::load(path) {
            Ok(doc) => {
                // Keep the previous state undoable so an agent's edit can be reverted here.
                self.push_undo();
                self.doc = doc;
                self.saved = self.doc.clone();
                self.textures.clear();
                self.outlines.clear();
                self.fix_selection();
                self.status = crate::i18n::tr("Reloaded external change").to_string();
            }
            Err(e) => self.status = crate::tf!("External change is invalid, kept current document: {}", e),
        }
    }

    pub fn base_dir(&self) -> PathBuf {
        self.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default()
    }

    /// Texture for an image path relative to the document. Failures are
    /// cached so a missing file is not retried every frame.
    /// Pixels of a loaded image for the blend pass, kept per texture.
    pub fn blend_pixels(&mut self, rel: &str, id: egui::TextureId) -> Option<std::sync::Arc<crate::blendpass::Pixels>> {
        if let Some(p) = self.blend_images.get(&id) {
            return Some(p.clone());
        }
        let pm = tiny_skia::Pixmap::load_png(self.base_dir().join(rel)).ok()?;
        let pixels = std::sync::Arc::new(crate::blendpass::Pixels { width: pm.width(), height: pm.height(), data: pm.data().to_vec() });
        self.blend_images.insert(id, pixels.clone());
        Some(pixels)
    }

    /// The edge of an image's visible pixels as closed loops in image
    /// coordinates (0..=1), traced once per image.
    pub fn image_outline(&mut self, rel: &str) -> Option<ImageOutline> {
        if let Some(o) = self.outlines.get(rel) {
            return o.clone();
        }
        let outline = tiny_skia::Pixmap::load_png(self.base_dir().join(rel)).ok().map(|pm| std::sync::Arc::new(skel2d_rig::mesh::alpha_outline(&pm)));
        self.outlines.insert(rel.to_string(), outline.clone());
        outline
    }

    pub fn texture(&mut self, ctx: &egui::Context, rel: &str) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(rel) {
            return t.clone();
        }
        let tex = tiny_skia::Pixmap::load_png(self.base_dir().join(rel)).ok().map(|pm| {
            let image = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            ctx.load_texture(rel, image, egui::TextureOptions::LINEAR)
        });
        if tex.is_none() {
            self.status = crate::tf!("Missing image {}", rel);
        }
        self.textures.insert(rel.to_string(), tex.clone());
        tex
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // A modal owns keyboard input even before its first field gets focus.
        if !matches!(self.dialog, Dialog::None) {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Comma)) {
            crate::dialogs::preferences(self);
            return;
        }
        // egui permits extra modifiers: consume Ctrl+Shift+E before Ctrl+E.
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::E)) {
            crate::export::repeat_last(self);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::E)) {
            crate::dialogs::export_gif(self);
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::ALT, Key::G)) {
            crate::panels::toggle_clip(self);
            return;
        }
        let (undo, redo, save, open) = ctx.input_mut(|i| {
            // egui permits extra modifiers: consume Ctrl+Shift+Z before Ctrl+Z.
            let redo=i.consume_shortcut(&REDO) || i.consume_shortcut(&REDO_ALT);
            (
                !redo && i.consume_shortcut(&UNDO),
                redo,
                i.consume_shortcut(&SAVE),
                i.consume_shortcut(&OPEN),
            )
        });
        if save {
            self.save();
        }
        if open {
            self.request_replace(Replace::Open);
        }
        // Ctrl+K: command palette, also while typing elsewhere.
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            crate::palette::toggle(self);
        }
        if typing || self.palette.is_open() {
            return;
        }
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        // No tool switch mid-drag: X / Y lock the move axis then.
        let dragging = self.drag.is_some() || self.vertex_drag.is_some();
        ctx.input(|i| {
            if i.modifiers.any() {
                return;
            }
            for (tool, _, key) in Tool::ALL {
                if i.key_pressed(key) && !dragging {
                    self.set_tool(tool);
                }
            }
            for (n, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5].into_iter().enumerate() {
                if i.key_pressed(key) && !dragging
                    && let Some(step) = crate::steps::Step::from_number(n + 1)
                {
                    self.set_step(step);
                }
            }
            if i.key_pressed(Key::Space) && (self.playback.anim.is_some() || self.playback.layers_active()) {
                self.playback.playing = !self.playback.playing;
            }
            if i.key_pressed(Key::F) {
                self.camera.needs_fit = true;
            }
            if i.key_pressed(Key::F1) {
                self.show_help = !self.show_help;
            }
            if i.key_pressed(Key::O) {
                self.settings.onion_skin = !self.settings.onion_skin;
            }
        });
        // Shift+R comes first: egui lets plain R match it too.
        if !self.doc.physics.is_empty() && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::R)) {
            self.physics_reset();
            self.status = crate::i18n::tr("Physics restarted from the current pose").into();
        }
        if self.step == crate::steps::Step::Animate && !ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::R)) {
            self.settings.auto_key = !self.settings.auto_key;
            self.status = crate::i18n::tr(if self.settings.auto_key { "Auto key on: every change writes a key" } else { "Auto key off: changes stay unkeyed until you key them" }).into();
        }
        // Ctrl + = / - / 0: UI size (remembered in the settings, unlike egui's own zoom keys).
        let (bigger, smaller, reset) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus),
                i.consume_key(Modifiers::COMMAND, Key::Minus),
                i.consume_key(Modifiers::COMMAND, Key::Num0),
            )
        });
        let s = &mut self.settings.ui_scale;
        if bigger {
            *s = (*s + 0.1).min(2.0);
        }
        if smaller {
            *s = (*s - 0.1).max(0.8);
        }
        if reset {
            *s = 1.0;
        }
        if bigger || smaller || reset {
            self.status = crate::tf!("UI size {}%", format!("{:.0}", *s * 100.0));
        }
    }

    /// Whether snapping applies to the current drag: the toolbar toggle,
    /// inverted while Ctrl is held.
    pub fn snapping(&self, ctx: &egui::Context) -> bool {
        self.settings.snap != ctx.input(|i| i.modifiers.command)
    }

    /// Hidden bones and slots are not drawn or picked; locked ones are drawn
    /// but cannot be picked or dragged in the viewport.
    pub fn bone_pickable(&self, name: &str) -> bool {
        !self.hidden_bones.contains(name) && !self.locked_bones.contains(name)
    }

    pub fn slot_pickable(&self, name: &str) -> bool {
        !self.hidden_slots.contains(name) && !self.locked_slots.contains(name)
    }

    /// Over the top of the viewport: one line saying what can be done now,
    /// and below it, while animating, the animation, time and Auto key; the
    /// physics preview controls when the rig has physics.
    pub fn viewport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        use crate::i18n::tr;
        use crate::theme;
        let guide = crate::steps::guide(self);
        let painter = ui.painter_at(rect);
        let band = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), theme::GUIDE_BAND_H));
        painter.rect_filled(band, 0.0, theme::OVERLAY_BG);
        let accent = if self.step == crate::steps::Step::Animate { theme::ANIMATE } else { theme::SETUP };
        painter.rect_filled(egui::Rect::from_min_size(band.min, egui::vec2(4.0, band.height())), 0.0, accent);
        let mut job = egui::text::LayoutJob::simple_singleline(guide, egui::FontId::proportional(theme::SIZE_BODY), theme::TEXT);
        job.wrap = egui::text::TextWrapping::truncate_at_width((band.width() - 24.0).max(1.0));
        let galley = painter.layout_job(job);
        painter.galley(egui::pos2(band.left() + 14.0, band.center().y - galley.size().y / 2.0), galley, theme::TEXT);

        let popup_open = !matches!(self.dialog, Dialog::None);
        if crate::toasts::show(ui.ctx(), &mut self.toasts, rect, popup_open) {
            ui.ctx().request_repaint();
        }
        let animate = self.playback.anim.is_some();
        let physics = !self.doc.physics.is_empty();
        if !animate && !physics {
            return;
        }
        // Middle, not Foreground: modals and menus live in Foreground and must cover it.
        let area = egui::Area::new(egui::Id::unique("viewport_overlay")).fixed_pos(rect.left_top() + egui::vec2(10.0, theme::GUIDE_BAND_H + 8.0)).order(egui::Order::Middle);
        area.show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                if let Some(a) = self.playback.anim.clone() {
                    let text = format!("{a}  ·  {:.2}s  f{}", self.playback.time, (self.playback.time * self.settings.fps.max(1.0)).round() as i64);
                    egui::Frame::new().fill(theme::OVERLAY_BG).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                        ui.label(egui::RichText::new(text).strong().color(theme::ANIMATE));
                    });
                    // Auto key belongs to the animation step, so it sits by the time.
                    let pending = self.pending_count();
                    let on = self.settings.auto_key;
                    let text = if pending > 0 { crate::tf!("Auto key ({} unkeyed)", pending) } else { tr("Auto key").to_string() };
                    let color = if on { theme::KEY_SET_TEXT } else { theme::TEXT_DIM };
                    let state = tr(if on { "on" } else { "off" });
                    let button = egui::Button::new((crate::icons::image(Icon::Key, 15.0).tint(color), egui::RichText::new(format!("{text}: {state}")).strong().color(color))).fill(theme::OVERLAY_BG).selected(on);
                    if ui.add(button).on_hover_text(tr("On: every change writes a key. Off: changes stay unkeyed (orange key buttons) until you key them with K or a key button; moving the playhead discards them, with a notice.")).clicked() {
                        self.settings.auto_key = !on;
                    }
                }
                if physics {
                    ui.add_space(10.0);
                    self.physics_controls(ui);
                }
            });
        });
    }


    /// Pause, reset, off and the setup preview, next to the mode switch. They
    /// only steer the preview; the document never changes.
    fn physics_controls(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::tr;
        use crate::theme::{PHYSICS, SIZE_SMALL, TEXT_DIM};
        ui.add(crate::icons::image(crate::icons::Icon::Physics, 18.0).tint(PHYSICS));
        let setup = self.playback.anim.is_none();
        let toggle = |ui: &mut egui::Ui, icon: Icon, on: bool, label: &str, tip: &str| {
            let fg = if on { egui::Color32::WHITE } else { TEXT_DIM };
            let text = egui::RichText::new(tr(label)).size(SIZE_SMALL).color(fg);
            let button = egui::Button::new((crate::icons::image(icon, 14.0).tint(fg), text)).fill(if on { PHYSICS.gamma_multiply(0.45) } else { egui::Color32::from_black_alpha(140) }).min_size(egui::vec2(0.0, 30.0)).selected(on);
            ui.add(button).on_hover_text(tr(tip)).clicked()
        };
        if toggle(ui, Icon::Pause, self.physics.paused, "Pause", "Freeze the live physics simulation where it is. Unpause to continue.") {
            self.physics.paused = !self.physics.paused;
        }
        if toggle(ui, Icon::Reset, false, "Reset", "Restart the physics from the current pose (Shift+R)") {
            self.physics_reset();
        }
        if toggle(ui, Icon::Close, !self.physics.enabled, "Off", "Turn the physics preview off to see the plain animated pose. The document is not changed.") {
            self.physics.enabled = !self.physics.enabled;
        }
        if setup && toggle(ui, Icon::Play, self.physics.setup_preview, "Physics preview", "Simulate in the setup pose so bone drags and root moves make the physics react. Off by default; the document is not changed.") {
            self.physics.setup_preview = !self.physics.setup_preview;
        }
    }

    fn language_picker(&mut self, ui: &mut egui::Ui) {
        let current = self.settings.language;
        egui::ComboBox::from_id_salt("language").selected_text(current.native_name()).width(100.0).show_ui(ui, |ui| {
            for lang in crate::i18n::Lang::ALL {
                ui.selectable_value(&mut self.settings.language, lang, lang.native_name());
            }
        })
        .response
        .on_hover_text(if crate::theme::has_korean_font() { "Language / 언어" } else { "Language (no Korean font found, so English is shown)" });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::tr;
        let sel: Vec<String> = [
            self.selection.bone.as_ref().map(|b| crate::tf!("bone {}", b)),
            self.selection.slot.as_ref().map(|s| crate::tf!("slot {}", s)),
            self.selection.ik.as_ref().map(|k| format!("IK {k}")),
            self.selection.physics.as_ref().map(|k| crate::tf!("physics {}", k)),
            self.selection.transform.as_ref().map(|k| crate::tf!("copy {}", k)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let zoom = self.camera.zoom;
        // The right side (selection, zoom) keeps its room; the left truncates.
        egui::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let keys = crate::steps::keys_text(self);
                crate::theme::hint(ui, if self.shown_hint.is_empty() { &keys } else { &self.shown_hint });
            },
            |ui| {
                ui.label(egui::RichText::new(crate::tf!("zoom {}%", format!("{:.0}", zoom * 100.0))).size(crate::theme::SIZE_SMALL).color(crate::theme::TEXT_DIM));
                ui.separator();
                let text = if sel.is_empty() { tr("nothing selected").to_string() } else { sel.join(" · ") };
                ui.label(egui::RichText::new(text).size(crate::theme::SIZE_SMALL));
            },
        );
    }

    /// UI size 100 / 125 / 150% (remembered in the settings).
    fn ui_size_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("UI size"));
            for pct in [100, 125, 150] {
                let on = (self.settings.ui_scale * 100.0).round() as i32 == pct;
                if ui.selectable_label(on, format!("{pct}%")).clicked() {
                    self.settings.ui_scale = pct as f32 / 100.0;
                }
            }
        });
    }

    /// F1 help, written per step: what you do there and which keys help.
    fn help_window(&mut self, ctx: &egui::Context) {
        use crate::i18n::tr;
        let mut open = self.show_help;
        // The title is translated, so the window id is pinned.
        egui::Window::new(tr("Help")).id(egui::Id::unique("help_window")).open(&mut open).frame(crate::theme::popup_frame()).collapsible(false).resizable(false).default_width(520.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 120.0).max(240.0)).show(ui, |ui| {
                type Group<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
                let groups: [Group; 7] = [
                    ("Step 1: Bones", "Build the skeleton. Every bone hangs under another; the top bone is root and cannot be removed or renamed.", &[("N, drag", "Draw a bone under the selected bone (or root)"), ("N, Alt+drag", "Draw the selected bone again: its children and images stay"), ("B", "Select a bone; drag it to move it, near its tip to rotate it"), ("Alt+drag", "Move only this bone: its children and images stay where they are"), ("V / C / X", "Move / Rotate / Scale the selected bone"), ("Drag in the list", "Drop a bone on another bone to change its parent")]),
                    ("Step 2: Images", "Put pictures on the bones. A slot is a place on a bone that shows one picture at a time; it can hold several pictures.", &[("Drop PNG / PSD", "Add images as new slots"), ("Add image", "Add more pictures to the selected slot (eyes open / closed, mouth shapes)"), ("Drag in the list", "Change the front-to-back order")]),
                    ("Step 3: Mesh", "A mesh lets a picture bend. Make a mesh first, then reshape it.", &[("Move", "Drag a vertex"), ("Add", "Click to add a vertex"), ("Delete", "Click a vertex to remove it"), ("Fold line", "Drag from vertex to vertex")]),
                    ("Step 4: Weights", "Weights decide how much each bone moves each vertex.", &[("Auto weights", "Let the editor set weights from the image"), ("Drag, Ctrl+drag", "Paint / remove weight of the picked bone"), ("[ / ]", "Brush smaller / larger"), ("Alt+drag", "Bend a bone temporarily to check the result")]),
                    ("Step 5: Animation", "Pick or create an animation, move the time marker, then change something to record a key.", &[("Space", "Play / pause"), (", / .", "Previous / next frame"), ("K", "Key what you changed (nothing changed: the active tool's property, or the slot's image)"), ("Ctrl+Shift+L", "Key every row the dope sheet shows"), ("Key buttons", "Beside the tool's value, on tree rows (bone, slot, mesh, constraint) and in the inspector: red diamond keyed, orange circle changed, hollow diamond not keyed"), ("Dope sheet", "Arrow opens a row into properties; click a row to select its keys; double-click a key to delete; Ctrl+A select all; Ctrl+Shift+drag copies keys"), ("Ctrl+click, Ctrl+drag", "Pick more vertices / box-pick vertices with Move, Rotate or Scale, then deform them"), ("Image card", "Press a picture of the selected slot to switch to it at this time (blinking, mouth shapes)")]),
                    ("View", "", &[("Wheel", "Zoom at the cursor"), ("Middle / right drag", "Pan"), ("F", "Frame selection (Shift+F: everything)"), ("O", "Onion skin")]),
                    ("Anywhere", "", &[("1 - 5", "Switch step"), ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"), ("Ctrl+Alt+G", "Clip the selected slot to the one behind it, or remove its clip"), ("R", "Auto key on or off (step 5)"), ("Ctrl+K", "Search and run any command, bone, slot or animation"), ("Ctrl+S / Ctrl+O", "Save / open"), ("Ctrl + = / - / 0", "UI size larger / smaller / reset"), ("F2 / Delete", "Rename / delete the selected item"), ("Alt + click", "Cycle through overlapping bones"), ("Shift + drag", "Fine control (1/4 speed)"), ("Ctrl + drag", "Toggle snapping for this drag"), ("F1", "This window")]),
                ];
                for (title, intro, rows) in groups {
                    crate::theme::section(ui, tr(title));
                    if !intro.is_empty() {
                        ui.add(egui::Label::new(egui::RichText::new(tr(intro)).size(crate::theme::SIZE_SMALL).color(crate::theme::TEXT_DIM)).wrap());
                    }
                    egui::Grid::new(title).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                        for (k, d) in rows {
                            // Key names stay as written (monospace, accent); only the description is translated.
                            ui.label(egui::RichText::new(tr(k)).monospace().color(crate::theme::ACCENT));
                            ui.add(egui::Label::new(tr(d)).wrap());
                            ui.end_row();
                        }
                    });
                }
                ui.add_space(6.0);
                self.ui_size_buttons(ui);
            });
        });
        self.show_help = open;
    }

    fn advance_playback(&mut self, ctx: &egui::Context) {
        if !self.playback.playing {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.1) * self.playback.speed.clamp(0.05, 8.0);
        let Some(duration) = self.animation().map(Animation::duration) else {
            // No main animation: the clock still drives the preview layers.
            if !self.playback.layers_active() {
                self.playback.playing = false;
            } else {
                self.playback.time = (self.playback.time + dt) % 3600.0;
                ctx.request_repaint();
            }
            return;
        };
        let previous = self.playback.time;
        let mut t = previous + dt;
        if t > duration {
            if self.playback.looping && duration > 0.0 {
                t %= duration;
            } else {
                t = duration;
                self.playback.playing = false;
            }
        }
        self.playback.time = t;
        self.note_fired_events(previous, t);
        ctx.request_repaint();
    }

    /// Remembers the events between two playhead positions for the viewport
    /// to show briefly. A wrap (`to` before `from`) is handled by the core.
    pub fn note_fired_events(&mut self, from: f32, to: f32) {
        let Some(anim) = self.animation() else { return };
        let fired = skel2d_core::pose::fired_events(&self.doc, anim, Some(from), to);
        let now = Instant::now();
        self.event_toasts.retain(|(_, at)| at.elapsed().as_secs_f32() < crate::theme::EVENT_TOAST_SECS);
        for e in fired {
            let mut text = e.name.clone();
            let values: Vec<String> = [(e.int != 0).then(|| e.int.to_string()), (e.float != 0.0).then(|| format!("{:.2}", e.float).trim_end_matches('0').trim_end_matches('.').to_string()), (!e.string.is_empty()).then(|| format!("\"{}\"", e.string))].into_iter().flatten().collect();
            if !values.is_empty() {
                text = format!("{text}  {}", values.join(" · "));
            }
            self.event_toasts.push((text, now));
        }
    }

    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        format!("{}{} - Skel2D", name.unwrap_or_else(|| crate::i18n::tr("untitled").to_string()), if self.dirty() { " *" } else { "" })
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::tr;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr("File"), |ui| {
                if ui.add(crate::icons::labeled(Icon::NewFile, tr("New"))).clicked() {
                    self.request_replace(Replace::New);
                }
                if ui.add(crate::icons::labeled(Icon::OpenFolder, tr("Open... (Ctrl+O)"))).clicked() {
                    self.request_replace(Replace::Open);
                }
                if ui.add(crate::icons::labeled(Icon::Save, tr("Save (Ctrl+S)"))).clicked() {
                    self.save();
                }
                if ui.add(crate::icons::labeled(Icon::Save, tr("Save As..."))).clicked() {
                    self.save_as();
                }
                if ui.add(crate::icons::labeled(Icon::Export, tr("Export... (Ctrl+E)"))).clicked() {
                    crate::dialogs::export_gif(self);
                    ui.close();
                }
                ui.separator();
                if ui.add(crate::icons::labeled(Icon::Import, tr("Import images..."))).on_hover_text(tr("Add PNGs, or a layered PSD (you choose what becomes a slot), as slots on the selected bone (or drop files on the viewport)")).clicked() {
                    let files = crate::import::pick_images();
                    if !files.is_empty() {
                        crate::import::import_picked(self, files);
                    }
                }
            });
            ui.menu_button(tr("Edit"), |ui| {
                if ui.add(crate::icons::labeled(Icon::Settings, tr("Preferences... (Ctrl+,)"))).clicked() {
                    crate::dialogs::preferences(self);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), crate::icons::labeled(Icon::Undo, tr("Undo (Ctrl+Z)"))).clicked() {
                    self.undo();
                }
                if ui.add_enabled(!self.redo.is_empty(), crate::icons::labeled(Icon::Redo, tr("Redo (Ctrl+Shift+Z)"))).clicked() {
                    self.redo();
                }
            });
            ui.menu_button(tr("View"), |ui| {
                ui.checkbox(&mut self.show_bones, tr("Show bones"));
                if ui.add(crate::icons::labeled(Icon::FrameAll, tr("Frame all (F)"))).clicked() {
                    self.camera.needs_fit = true;
                }
                ui.checkbox(&mut self.settings.show_grid, tr("Grid"));
                ui.checkbox(&mut self.settings.carry_meshes, tr("Meshes follow bones in the setup pose"))
                    .on_hover_text(tr("Off: move bones under a mesh without moving the mesh (e.g. to fix joint positions)"));
                self.ui_size_buttons(ui);
                ui.horizontal(|ui| {
                    ui.label(tr("Snap angle"));
                    ui.add(egui::DragValue::new(&mut self.settings.snap_angle).range(1.0..=90.0).suffix("°"));
                    ui.label(tr("grid"));
                    ui.add(egui::DragValue::new(&mut self.settings.snap_grid).range(0.5..=500.0));
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Onion frames"));
                    ui.add(egui::DragValue::new(&mut self.settings.onion_frames).range(1..=6));
                    ui.label("FPS");
                    ui.add(egui::DragValue::new(&mut self.settings.fps).range(1.0..=240.0));
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Language"));
                    for lang in crate::i18n::Lang::ALL {
                        ui.selectable_value(&mut self.settings.language, lang, lang.native_name());
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let help = egui::Button::new((crate::icons::image(Icon::Help, 16.0), tr("Help"))).image_tint_follows_text_color(true);
                if ui.add(help).on_hover_text(tr("Shortcuts and tips [F1]")).clicked() {
                    self.show_help = !self.show_help;
                }
                self.language_picker(ui);
                // Every action is searchable here, so nothing needs a visible button to be reachable.
                let search = crate::icons::labeled(Icon::Search, egui::RichText::new(format!("{}   Ctrl+K", tr("Search commands"))).color(crate::theme::TEXT_DIM)).min_size(egui::vec2(220.0, 0.0));
                if ui.add(search).on_hover_text(tr("Find and run any action, bone, slot or animation by name")).clicked() {
                    crate::palette::toggle(self);
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        use crate::i18n::tr;
        if matches!(self.dialog, Dialog::Editor(_)) {
            if let Dialog::Editor(mut dialog) = std::mem::replace(&mut self.dialog, Dialog::None)
                && crate::dialogs::show(self, ctx, &mut dialog)
            {
                self.dialog = Dialog::Editor(dialog);
            }
            return;
        }
        if !matches!(self.dialog, Dialog::None) {
            let (enter, escape) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
            if matches!(self.dialog, Dialog::ExternalChange) && (enter || escape) {
                self.file_mtime = self.path.as_deref().and_then(mtime);
                self.dialog = Dialog::None;
                return;
            }
            if escape {
                self.dialog = Dialog::None;
                return;
            }
            if enter {
                match std::mem::replace(&mut self.dialog, Dialog::None) {
                    Dialog::ConfirmClose => {
                        self.save();
                        if self.dirty() { self.dialog = Dialog::ConfirmClose; } else { self.quit(ctx); }
                    }
                    Dialog::ConfirmReplace(what) => {
                        if !self.save_then_replace(what) {
                            self.dialog = Dialog::ConfirmReplace(what);
                        }
                    }
                    Dialog::PsdSplit(p) => crate::import::import_images_split(self, &p.files, p.at, p.bone, p.split, p.hidden),
                    Dialog::DropChoice(p) => { crate::panels::add_images_to_slot(self, &p.slot, &p.files); }
                    other => self.dialog = other,
                }
                return;
            }
        }
        match self.dialog {
            Dialog::Editor(_) => unreachable!(),
            Dialog::None => {}
            Dialog::PsdSplit(_) => {
                use skel2d_rig::psd::{Hidden, Split};
                let mut choice = None;
                let Dialog::PsdSplit(pending) = &mut self.dialog else { unreachable!() };
                crate::theme::modal("psd_split").show(ctx, |ui| {
                    use crate::theme::{Emphasis, choice_card, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    crate::theme::dialog_title(ui, tr("Import PSD"), tr("What should become one slot?"));
                    if choice_card(ui, pending.split == Split::Innermost, tr("Each innermost group"), tr("Parts follow your deepest folders; layers inside one are merged and keep their blending")) {
                        pending.split = Split::Innermost;
                    }
                    ui.add_space(6.0);
                    if choice_card(ui, pending.split == Split::Groups, tr("Each top-level group"), tr("A group's layers are merged into one image, e.g. line art over flat color")) {
                        pending.split = Split::Groups;
                    }
                    ui.add_space(6.0);
                    if choice_card(ui, pending.split == Split::Layers, tr("Each layer"), tr("Groups only organize the parts (layer-per-part PSD)")) {
                        pending.split = Split::Layers;
                    }
                    ui.add_space(6.0);
                    if choice_card(ui, pending.split == Split::LayersKeepClipping, tr("Each layer, keep clipping"), tr("Clipped layers stay separate slots clipped to their base, so they can move on their own")) {
                        pending.split = Split::LayersKeepClipping;
                    }
                    ui.add_space(crate::theme::DIALOG_GAP);
                    ui.label(tr("Hidden layers"));
                    ui.add_space(6.0);
                    if choice_card(ui, pending.hidden == Hidden::Skip, tr("Leave out"), tr("Only what shows in the PSD is imported")) {
                        pending.hidden = Hidden::Skip;
                    }
                    ui.add_space(6.0);
                    if choice_card(ui, pending.hidden == Hidden::Transparent, tr("Import as transparent"), tr("Hidden parts get a slot with alpha 0, ready to show later")) {
                        pending.hidden = Hidden::Transparent;
                    }
                    ui.add_space(6.0);
                    if choice_card(ui, pending.hidden == Hidden::Include, tr("Import as visible"), tr("Hidden layers are treated like visible ones")) {
                        pending.hidden = Hidden::Include;
                    }
                    ui.add_space(crate::theme::DIALOG_GAP);
                    let (enter, escape) = ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(tr("Import"), Emphasis::Primary)).on_hover_text("Enter").clicked() || enter {
                            choice = Some(Some((pending.split, pending.hidden)));
                        }
                        if ui.add(dialog_button(tr("Cancel"), Emphasis::Plain)).on_hover_text("Esc").clicked() || escape {
                            choice = Some(None);
                        }
                    });
                });
                if let Some(split) = choice
                    && let Dialog::PsdSplit(p) = std::mem::replace(&mut self.dialog, Dialog::None)
                    && let Some((split, hidden)) = split
                {
                    crate::import::import_images_split(self, &p.files, p.at, p.bone, split, hidden);
                }
            }
            Dialog::ImportTarget(_) => {
                // true = new project, false = this project; None = cancel.
                let mut choice = None;
                crate::theme::modal("import_target").show(ctx, |ui| {
                    use crate::theme::{Emphasis, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    crate::theme::dialog_title(ui, tr("Where should the images go?"), tr("Start a new project with these images, or add them to the open one?"));
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(tr("New project"), Emphasis::Primary)).clicked() {
                            choice = Some(Some(true));
                        }
                        if ui.add(dialog_button(tr("Add to this project"), Emphasis::Plain)).clicked() {
                            choice = Some(Some(false));
                        }
                        if ui.add(dialog_button(tr("Cancel"), Emphasis::Plain)).on_hover_text("Esc").clicked() {
                            choice = Some(None);
                        }
                    });
                });
                if let Some(choice) = choice
                    && let Dialog::ImportTarget(files) = std::mem::replace(&mut self.dialog, Dialog::None)
                    && let Some(new_project) = choice
                {
                    if new_project {
                        self.import_after_new = files;
                        self.request_replace(Replace::NewWithImport);
                    } else {
                        let (at, bone) = (self.camera.center, self.selection.bone.clone());
                        crate::import::import_images(self, &files, at, bone);
                    }
                }
            }
            Dialog::DropChoice(_) => {
                // true = into the picked slot, false = new slots; None = cancel.
                let mut choice = None;
                let slot_name = if let Dialog::DropChoice(p) = &self.dialog { p.slot.clone() } else { String::new() };
                crate::theme::modal("drop_choice").show(ctx, |ui| {
                    use crate::theme::{Emphasis, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    let about = crate::tf!("Slot \"{}\" is picked. Add the images to it as more looks, or make a new slot for each file?", slot_name);
                    crate::theme::dialog_title(ui, tr("Where should the images go?"), &about);
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(&crate::tf!("Add to slot {}", slot_name), Emphasis::Primary)).clicked() {
                            choice = Some(Some(true));
                        }
                        if ui.add(dialog_button(tr("New slots"), Emphasis::Plain)).clicked() {
                            choice = Some(Some(false));
                        }
                        if ui.add(dialog_button(tr("Cancel"), Emphasis::Plain)).clicked() {
                            choice = Some(None);
                        }
                    });
                });
                if let Some(choice) = choice
                    && let Dialog::DropChoice(p) = std::mem::replace(&mut self.dialog, Dialog::None)
                    && let Some(into_slot) = choice
                {
                    if into_slot {
                        crate::panels::add_images_to_slot(self, &p.slot, &p.files);
                    } else {
                        crate::import::import_images(self, &p.files, p.at, p.bone);
                    }
                }
            }
            Dialog::ExternalChange => {
                crate::theme::modal("external_change").show(ctx, |ui| {
                    use crate::theme::{Emphasis, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    crate::theme::dialog_title(ui, tr("File changed on disk"), tr("Another tool modified the file while you have unsaved changes."));
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(tr("Reload (discard mine)"), Emphasis::Danger)).clicked() {
                            if let Some(p) = self.path.clone() {
                                self.reload_external(&p);
                            }
                            self.dialog = Dialog::None;
                        }
                        if ui.add(dialog_button(tr("Keep mine"), Emphasis::Primary)).clicked() {
                            // Saving will overwrite the external change.
                            self.file_mtime = self.path.as_deref().and_then(mtime);
                            self.dialog = Dialog::None;
                        }
                    });
                });
            }
            Dialog::ConfirmReplace(what) => {
                let file = self.path.as_deref().and_then(|p| p.file_name()).map_or_else(|| tr("Untitled").to_string(), |n| n.to_string_lossy().into_owned());
                let body = if matches!(what, Replace::New | Replace::NewWithImport) {
                    crate::tf!("{} has changes that are not saved. Starting a new document without saving loses them.", file)
                } else {
                    crate::tf!("{} has changes that are not saved. Opening another file without saving loses them.", file)
                };
                crate::theme::modal("confirm_replace").show(ctx, |ui| {
                    use crate::theme::{Emphasis, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    crate::theme::dialog_title(ui, tr("Save your changes first?"), &body);
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(tr("Save"), Emphasis::Primary)).on_hover_text("Enter").clicked() {
                            self.save_then_replace(what);
                        }
                        if ui.add(dialog_button(tr("Don't save"), Emphasis::Danger)).clicked() {
                            self.dialog = Dialog::None;
                            self.run_replace(what);
                        }
                        if ui.add(dialog_button(tr("Cancel"), Emphasis::Plain)).on_hover_text("Esc").clicked() {
                            self.dialog = Dialog::None;
                        }
                    });
                });
            }
            Dialog::ConfirmClose => {
                let file = self.path.as_deref().and_then(|p| p.file_name()).map_or_else(|| tr("Untitled").to_string(), |n| n.to_string_lossy().into_owned());
                crate::theme::modal("confirm_close").show(ctx, |ui| {
                    use crate::theme::{Emphasis, dialog_button};
                    ui.set_width(crate::theme::DIALOG_WIDTH);
                    crate::theme::dialog_title(ui, tr("Save your changes before quitting?"), &crate::tf!("{} has changes that are not saved. Quitting without saving loses them.", file));
                    crate::theme::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(tr("Save and quit"), Emphasis::Primary)).on_hover_text("Enter").clicked() {
                            self.save();
                            if !self.dirty() {
                                self.quit(ctx);
                            }
                        }
                        if ui.add(dialog_button(tr("Quit without saving"), Emphasis::Danger)).clicked() {
                            self.quit(ctx);
                        }
                        if ui.add(dialog_button(tr("Cancel"), Emphasis::Plain)).on_hover_text("Esc").clicked() {
                            self.dialog = Dialog::None;
                        }
                    });
                });
            }
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_close = true;
        self.dialog = Dialog::None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Dialog::ConfirmClose;
        }
    }
}

/// Startup options, mainly so scripts and AI agents can open the editor in a
/// given state and capture what it shows.
#[derive(Default)]
pub struct StartOptions {
    pub path: Option<PathBuf>,
    pub anim: Option<String>,
    pub time: f32,
    pub select: Option<String>,
    pub select_slot: Option<String>,
    pub select_ik: Option<String>,
    pub select_physics: Option<String>,
    /// Scripted screenshots: a popup to open (close, mesh, smooth, prune, mirror).
    pub dialog: Option<String>,
    /// Extra animations to layer on top (preview).
    pub tracks: Vec<String>,
    pub layers: Option<String>,
    /// Skins to preview (repeatable).
    pub skins: Vec<String>,
    /// Active tool (default Rotate).
    pub tool: Option<Tool>,
    /// Work step 1-5 to open in.
    pub step: Option<usize>,
    /// UI language to use for this run only.
    pub lang: Option<crate::i18n::Lang>,
    /// Save a PNG of the window after the first frames, then quit.
    pub screenshot: Option<PathBuf>,
    /// Seconds to wait before capturing (e.g. to observe a live reload).
    pub wait: f32,
    /// Scripted screenshots: open every dope sheet row into its properties.
    pub expand: bool,
    /// Scripted screenshots: show one toast of each kind, one of them repeated.
    pub toast_demo: bool,
    /// Scripted screenshots: Auto Key on or off for this run.
    pub auto_key: Option<bool>,
    /// Scripted screenshots: rotate the selected bone by this many degrees at the playhead (unkeyed with Auto Key off).
    pub edit_rotate: Option<f32>,
}

struct Screenshot {
    path: PathBuf,
    frames: u32,
    started: Instant,
    wait: Duration,
    requested: bool,
    done: Arc<AtomicBool>,
}

impl EditorApp {
    pub fn with_options(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> Self {
        let mut app = Self::new(cc, opts.path, opts.screenshot.is_some(), opts.lang);
        app.blend_pass = cc.wgpu_render_state.as_ref().is_some_and(crate::blendpass::install);
        if let Some(lang) = opts.lang {
            app.settings.language = lang;
        }
        if let Some(t) = opts.tool {
            app.tool = t;
        }
        if let Some(a) = opts.anim.filter(|a| app.doc.animations.contains_key(a)) {
            app.playback.anim = Some(a);
            app.playback.time = opts.time.max(0.0);
        }
        app.skins = opts.skins.into_iter().filter(|k| app.doc.skin(k).is_some()).collect();
        if let Some(layers) = opts.layers {
            match skel2d_core::parse_layer_tracks(&app.doc, &layers) {
                Ok(layers) => app.playback.layers = layers,
                Err(error) => app.status = error.to_string(),
            }
        } else {
            for name in opts.tracks.into_iter().filter(|n| app.doc.animations.contains_key(n)) {
                app.playback.add_layer(&name);
            }
        }
        if app.playback.anim.is_none() && !app.playback.layers.is_empty() {
            app.playback.time = opts.time.max(0.0);
        }
        if let Some(b) = opts.select.filter(|b| app.doc.bone(b).is_some()) {
            app.selection.bone = Some(b);
        }
        if let Some(s) = opts.select_slot.filter(|s| app.doc.slot(s).is_some()) {
            app.selection.bone = app.doc.slot(&s).map(|s| s.bone.clone());
            app.selection.slot = Some(s);
        }
        if let Some(ik) = opts.select_ik.filter(|name| app.doc.ik.iter().any(|c| c.name == *name)) {
            app.selection.ik = Some(ik);
        }
        if let Some(name) = opts.select_physics.filter(|name| app.doc.physics_constraint(name).is_some()) {
            app.selection.physics = Some(name);
        }
        if let Some(step) = opts.step.and_then(crate::steps::Step::from_number) {
            app.set_step(step);
        } else {
            app.sync_step();
        }
        if let Some(on) = opts.auto_key {
            app.settings.auto_key = on;
        }
        if let (Some(deg), Some(anim), Some(bone)) = (opts.edit_rotate, app.playback.anim.clone(), app.selection.bone.clone()) {
            let time = app.playback.time;
            app.edit(vec![Op::KeyBone(KeyBone { animation: anim, bone, time, rotate: Some(deg), translate: None, scale: None, shear: None, curve: None, curves: None })]);
        }
        if opts.expand {
            crate::timeline::toggle_expand_all(&mut app);
        }
        if opts.toast_demo {
            use crate::toasts::Kind;
            app.toasts.push(Kind::Info, "Opened spineboy.skel.json");
            app.toasts.push(Kind::Success, "Keyed 3 edited properties");
            for _ in 0..4 {
                app.toasts.push(Kind::Warning, "Not applied (mouth): the mesh outline would cross itself. Move the point a shorter way.");
            }
            app.toasts.push(Kind::Error, "Could not save: the file is read-only");
        }
        match opts.dialog.as_deref() {
            Some("gif") => crate::dialogs::export_gif(&mut app),
            Some("gif-progress") => crate::export::preview_progress(&mut app),
            Some("psd") => app.dialog = Dialog::PsdSplit(crate::import::PendingImport { files: vec![], at: [0.0, 0.0], bone: None, split: Default::default(), hidden: Default::default() }),
            Some("import-target") => app.dialog = Dialog::ImportTarget(vec![]),
            Some("close") => app.dialog = Dialog::ConfirmClose,
            Some("replace") => app.dialog = Dialog::ConfirmReplace(Replace::Open),
            Some("mesh") => crate::dialogs::selected_mesh_options(&mut app),
            Some("smooth") => crate::dialogs::weight_options(&mut app, crate::dialogs::WeightAction::Smooth),
            Some("prune") => crate::dialogs::weight_options(&mut app, crate::dialogs::WeightAction::Prune),
            Some("mirror") => crate::dialogs::weight_options(&mut app, crate::dialogs::WeightAction::Mirror),
            Some("new-event") => crate::dialogs::new_event(&mut app),
            _ => {}
        }
        app.screenshot = opts.screenshot.map(|path| Screenshot {
            path,
            frames: 0,
            started: Instant::now(),
            wait: Duration::from_secs_f32(opts.wait.clamp(0.0, 600.0)),
            requested: false,
            done: Arc::new(AtomicBool::new(false)),
        });
        app
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else { return };
        if shot.done.load(Ordering::Acquire) {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        shot.frames += 1;
        ctx.request_repaint();
        // Let layout and fonts settle before capturing.
        if shot.frames < 5 || shot.started.elapsed() < shot.wait || shot.requested {
            return;
        }
        shot.requested = true;
        let (path, done, repaint) = (shot.path.clone(), shot.done.clone(), ctx.clone());
        ctx.request_screenshot(move |image| {
            if let Err(e) = save_color_image(&image, &path) {
                eprintln!("screenshot failed: {e}");
            }
            done.store(true, Ordering::Release);
            repaint.request_repaint();
        });
    }
}

fn save_color_image(image: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = image.size;
    let data: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let size = tiny_skia::IntSize::from_wh(w as u32, h as u32).ok_or("empty screenshot")?;
    let pixmap = tiny_skia::Pixmap::from_vec(data, size).ok_or("bad screenshot buffer")?;
    pixmap.save_png(path).map_err(|e| e.to_string())
}

impl eframe::App for EditorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Scripted screenshot runs must not overwrite the person's remembered settings.
        if self.screenshot.is_none() {
            eframe::set_value(storage, "settings", &self.settings);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_close(&ctx);
        self.watch_file();
        self.shortcuts(&ctx);
        crate::export::show_progress(self, &ctx);
        let was_playing = self.physics_was_playing;
        self.advance_playback(&ctx);
        self.advance_physics(&ctx, was_playing);
        self.physics_was_playing = self.playback.playing;
        self.drop_stale_pending();
        self.observe_status();
        let korean = self.settings.language == crate::i18n::Lang::Korean && crate::theme::has_korean_font();
        crate::i18n::set(if korean { crate::i18n::Lang::Korean } else { crate::i18n::Lang::English });
        let title = self.title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        self.screenshot_step(&ctx);

        if self.settings.ui_scale != self.applied_scale {
            ctx.set_zoom_factor(self.settings.ui_scale.clamp(0.8, 2.0));
            self.applied_scale = self.settings.ui_scale;
        }
        // Panels set a fresh hint every frame while hovered; the status bar is
        // laid out first, so it shows the previous frame's.
        self.shown_hint = std::mem::take(&mut self.hint);

        // Layout: tool rail on the left, tree with the selection's properties
        // under it on the right, the timeline under the viewport in animate
        // mode only, and the viewport filling the rest.
        self.sync_step();
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("steps").show(ui, |ui| crate::panels::step_bar(self, ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tools").exact_size(crate::theme::TOOLS_W).resizable(false).show(ui, |ui| crate::panels::tools(self, ui));
        // Bounded widths: a widget asking for more room than the panel has must
        // never be able to grow it frame after frame and squeeze the viewport.
        egui::Panel::right("side").resizable(true).default_size(crate::theme::SIDE_W).size_range(300.0..=560.0).show(ui, |ui| crate::panels::side(self, ui));
        if self.step == crate::steps::Step::Animate {
            egui::Panel::bottom("timeline_panel").resizable(true).default_size(320.0).size_range(150.0..=600.0).show(ui, |ui| crate::timeline::ui(self, ui));
        }
        egui::CentralPanel::default().show(ui, |ui| crate::viewport::ui(self, ui));

        self.dialogs(&ctx);
        self.help_window(&ctx);
        crate::palette::ui(self, &ctx);
        // Keep polling the file even when idle.
        ctx.request_repaint_after(WATCH_INTERVAL);
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}


#[cfg(test)]
mod step_tests {
    use super::*;
    use crate::panels::stepui_tests as ui_tests;
    use crate::steps::Step;

    fn press(app: &mut EditorApp, key: Key) {
        let ctx = egui::Context::default();
        let input = egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], ..Default::default() };
        let _ = ctx.run_pass(input, |ctx| app.shortcuts(ctx));
    }

    fn app() -> EditorApp {
        let doc = skel2d_core::apply(&Document::with_root("t"), &[
            Op::AddAnimation { name: "walk".into(), duration: Some(1.0) },
            Op::AddAnimation { name: "idle".into(), duration: Some(1.0) },
        ]).unwrap();
        EditorApp::test_app(doc, std::env::temp_dir().join("skel2d-step-test.skel.json"))
    }

    #[test]
    fn r_toggles_auto_key_only_in_the_animate_step() {
        let mut app = app();
        let on = app.settings.auto_key;
        app.step = Step::Bones;
        press(&mut app, Key::R);
        assert_eq!(app.settings.auto_key, on, "other steps have nothing to key");
        app.step = Step::Animate;
        press(&mut app, Key::R);
        assert_eq!(app.settings.auto_key, !on);
        press(&mut app, Key::R);
        assert_eq!(app.settings.auto_key, on);
    }

    #[test]
    fn status_message_shows_only_in_toast_and_keeps_status_bar_details() {
        let mut app = app();
        app.status = "Opened example.skel.json".into();
        app.selection.bone = Some("root".into());
        app.camera.zoom = 1.0;
        crate::i18n::set(crate::i18n::Lang::English);
        app.observe_status();
        assert_eq!(app.toasts.items.len(), 1);
        assert_eq!(app.toasts.items[0].text, app.status);
        assert_eq!(app.status_seen, app.status);

        let output = ui_tests::frames(&mut app, 2, |app, ui| app.status_bar(ui));
        let texts = ui_tests::texts(&output);
        assert!(!texts.iter().any(|text| text.contains("Opened example.skel.json")), "status bar repeated the toast");
        assert!(texts.iter().any(|text| text.contains("bone root")));
        assert!(texts.iter().any(|text| text.contains("zoom 100%")));
    }

    #[test]
    fn steps_one_to_four_are_the_setup_pose_and_five_is_the_animation() {
        let mut app = app();
        assert_eq!((app.step, app.playback.anim.as_deref()), (Step::Bones, None));
        for (key, step, tool) in [(Key::Num2, Step::Images, Tool::Select), (Key::Num3, Step::Mesh, Tool::Mesh), (Key::Num4, Step::Weights, Tool::Weights), (Key::Num1, Step::Bones, Tool::Select)] {
            press(&mut app, key);
            assert_eq!(app.step, step);
            assert_eq!(app.playback.anim, None, "{step:?} edits the setup pose");
            assert_eq!(app.tool, tool, "{step:?} starts with a tool that belongs to it");
        }
        press(&mut app, Key::Num5);
        assert_eq!(app.step, Step::Animate);
        assert_eq!(app.playback.anim.as_deref(), Some("idle"), "the first animation when none was used before");
    }

    #[test]
    fn leaving_and_returning_restores_the_last_animation() {
        let mut app = app();
        app.set_step(Step::Animate);
        app.playback.anim = Some("walk".into());
        app.playback.playing = true;
        app.set_step(Step::Mesh);
        assert_eq!((app.playback.anim.as_deref(), app.playback.last_anim.as_deref(), app.playback.playing), (None, Some("walk"), false));
        app.set_step(Step::Animate);
        assert_eq!(app.playback.anim.as_deref(), Some("walk"));
        // Tab-style toggle goes back to the setup step it came from.
        app.set_step(Step::Weights);
        app.toggle_mode();
        assert_eq!(app.step, Step::Animate);
        app.toggle_mode();
        assert_eq!(app.step, Step::Weights);
        // Something else switching animation on (palette, dialog) moves the step along.
        app.playback.anim = Some("idle".into());
        app.sync_step();
        assert_eq!(app.step, Step::Animate);
        // A deleted animation falls back to another one instead of editing the setup pose.
        app.doc.animations.remove("idle");
        app.fix_selection();
        app.sync_step();
        assert_eq!((app.step, app.playback.anim.as_deref()), (Step::Animate, Some("walk")));
    }

    #[test]
    fn tools_that_do_not_belong_to_a_step_send_you_to_theirs_or_are_refused() {
        let mut app = app();
        app.set_tool(Tool::Weights);
        assert_eq!((app.step, app.tool), (Step::Weights, Tool::Weights));
        app.set_tool(Tool::Bone);
        assert_eq!((app.step, app.tool), (Step::Bones, Tool::Bone));
        app.set_step(Step::Animate);
        assert!(app.step.allows(app.tool), "the bone tool is dropped in the animation step");
        app.status.clear();
        app.set_tool(Tool::Bone);
        assert_eq!(app.step, Step::Animate, "no setup tools while animating");
        assert!(!app.status.is_empty(), "and it says why");
        assert!(app.playback.anim.is_some());
    }

    #[test]
    fn importing_into_a_document_with_slots_asks_where_the_images_go() {
        let dir = std::env::temp_dir().join(format!("skel2d_import_target_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("arm.png");
        tiny_skia::Pixmap::new(4, 4).unwrap().save_png(&png).unwrap();

        let mut app = app();
        crate::import::import_picked(&mut app, vec![png.clone()]);
        assert!(matches!(app.dialog, Dialog::None), "an empty document just takes them");
        assert_eq!(app.doc.slots.len(), 1);

        crate::import::import_picked(&mut app, vec![png.clone()]);
        assert!(matches!(app.dialog, Dialog::ImportTarget(_)));
        assert_eq!(app.doc.slots.len(), 1, "nothing is added before the answer");

        app.dialog = Dialog::None;
        app.import_after_new = vec![png];
        app.run_replace(Replace::NewWithImport);
        assert_eq!(app.doc.slots.len(), 1, "the new project holds only the import");
        assert!(app.path.is_none());
        assert!(app.import_after_new.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_and_open_ask_first_only_when_there_are_unsaved_changes() {
        let mut app = app();
        assert!(!app.dirty());
        app.request_replace(Replace::New);
        assert!(matches!(app.dialog, Dialog::None), "a clean document is replaced without asking");
        app.doc.name = "changed".into();
        assert!(app.dirty());
        app.request_replace(Replace::Open);
        assert!(matches!(app.dialog, Dialog::ConfirmReplace(Replace::Open)));
        assert_eq!(app.doc.name, "changed", "nothing is replaced before the answer");
        app.request_replace(Replace::New);
        assert!(matches!(app.dialog, Dialog::ConfirmReplace(Replace::New)));
    }

    #[test]
    fn saving_before_replacing_continues_only_on_success() {
        let dir = std::env::temp_dir().join("skel2d-replace-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = EditorApp::test_app(Document::with_root("r"), dir.join("r.skel.json"));
        app.doc.name = "changed".into();
        app.dialog = Dialog::ConfirmReplace(Replace::New);
        assert!(app.save_then_replace(Replace::New));
        assert_eq!(app.doc.name, "untitled", "saved, then the new document replaced it");
        assert!(matches!(app.dialog, Dialog::None));
        // A save that fails keeps the document and the question.
        let mut app = EditorApp::test_app(Document::with_root("r"), dir.join("no-such-dir").join("r.skel.json"));
        app.doc.name = "changed".into();
        app.dialog = Dialog::ConfirmReplace(Replace::New);
        assert!(!app.save_then_replace(Replace::New));
        assert_eq!(app.doc.name, "changed");
        assert!(matches!(app.dialog, Dialog::ConfirmReplace(Replace::New)));
    }

    #[test]
    fn opening_or_creating_a_document_starts_at_step_one() {
        let mut app = app();
        app.set_step(Step::Animate);
        app.new_document();
        assert_eq!((app.step, app.playback.anim.as_deref(), app.playback.last_anim.as_deref()), (Step::Bones, None, None));
        assert_eq!(app.doc.bones.len(), 1, "a new document has the root");
        assert_eq!(app.doc.bones[0].name, skel2d_core::ROOT);
    }

    #[test]
    fn every_visible_string_has_a_korean_translation() {
        let (mut app, dir) = ui_tests::fixture("ko");
        let _ = dir;
        crate::i18n::SEEN.with(|s| s.borrow_mut().clear());
        for step in Step::ALL {
            app.set_step(step);
            for slot in [None, Some("eye"), Some("mouth"), Some("cloth")] {
                for bone in [None, Some("root"), Some("arm")] {
                    app.selection = crate::app::Selection { slot: slot.map(String::from), bone: bone.map(String::from), ..Default::default() };
                    let ctx = egui::Context::default();
                    crate::icons::load(&ctx);
                    let _ = ui_tests::run_frames(&ctx, &mut app, 2, &mut |app, ui| {
                        app.menu_bar(ui);
                        crate::panels::step_bar(app, ui);
                        crate::panels::tools(app, ui);
                        crate::panels::side(app, ui);
                        app.status_bar(ui);
                        crate::viewport::ui(app, ui);
                        app.help_window(ui.ctx());
                    }, Vec::new());
                }
            }
        }
        // The question after dropping PNGs on a picked slot.
        app.dialog = Dialog::DropChoice(crate::import::PendingDrop { files: Vec::new(), at: [0.0, 0.0], bone: None, slot: "eye".into() });
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        let _ = ui_tests::run_frames(&ctx, &mut app, 2, &mut |app, ui| app.dialogs(ui.ctx()), Vec::new());
        app.dialog = Dialog::None;
        // The start screen of an empty document.
        let mut empty = EditorApp::test_app(Document::with_root("e"), std::env::temp_dir().join("skel2d-empty-ko.skel.json"));
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        let _ = ui_tests::run_frames(&ctx, &mut empty, 2, &mut |app, ui| crate::viewport::ui(app, ui), Vec::new());
        let mut missing: Vec<String> = crate::i18n::SEEN.with(|s| s.borrow().clone());
        missing.sort();
        missing.dedup();
        // Text that is already Korean was translated by the caller (it is passed through `tr` twice).
        missing.retain(|k| k.chars().any(|c| c.is_ascii_alphabetic()) && !k.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)) && !crate::i18n::has_korean(k));
        assert!(missing.is_empty(), "no Korean text for:\n{}", missing.join("\n"));
    }
}
