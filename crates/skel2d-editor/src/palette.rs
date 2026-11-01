//! Command palette (Ctrl+K): type to filter every editor command and every
//! bone, slot and animation by name; Up/Down pick, Enter runs, Esc closes.

use eframe::egui::{self, Align2, Key, Modifiers};

use crate::app::{EditorApp, Tool};
use crate::i18n::{self, Lang, tr};
use crate::theme::{ACCENT, PANEL_BG, TEXT, TEXT_DIM};

const WIDTH: f32 = 520.0;
const ROW_H: f32 = 26.0;
const MAX_ROWS: f32 = 12.0;

#[derive(Default)]
pub struct PaletteState {
    open: bool,
    query: String,
    sel: usize,
}

impl PaletteState {
    pub fn is_open(&self) -> bool {
        self.open
    }
}

#[derive(Clone)]
enum Action {
    MeshMode(crate::viewport::MeshMode),
    MeshAction(crate::viewport::MeshAction),
    ResetShear,
    ToggleClip,
    BoneInheritance(skel2d_core::Inherit),
    AddLayer(String),
    RemoveLayer(String),
    ToggleLayer(String),
    MoveLayer(String, bool),
    LayerAlpha(String, f32),
    Preferences,
    ExportGif,
    NewSkin,
    CompareSkins,
    NewLinkedMesh,
    GoToLinkedParent,
    NewConstraint,
    NewPhysics,
    NewTransform,
    PhysicsReset,
    PhysicsPause,
    PhysicsOff,
    PhysicsSetupPreview,
    PhysicsShake(String),
    Physics(String),
    RenameItem,
    RenameAttachment,
    DeleteItem,
    MeshOptions,
    ImageWeights,
    WeightOptions(crate::dialogs::WeightAction),
    PoseTest,
    DuplicateAnimation,
    MoveConstraint(bool),
    KeyIk,
    KeyEdited,
    KeyShown,
    KeyBoneProp(crate::timeline::KeyProp),
    KeySlotColor,
    SelectAllKeys,
    ExpandRows,
    AllRows,
    DiscardUnkeyed,
    DrawOrderKey,
    DrawOrderSetup,
    DrawOrderRemove,
    DrawOrderMove(String, bool),
    NewEvent,
    EventSelect(String),
    EventKey(String),
    EventKeyRemove(String),
    EventRename(String),
    EventDelete(String),
    Tool(Tool),
    ToggleMode,
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
    Frame,
    Snap,
    Onion,
    Names,
    Grid,
    Bones,
    AutoKey,
    Help,
    UiScale(f32),
    Language(Lang),
    ImportImages,
    NewAnimation,
    Bone(String),
    Slot(String),
    Animation(String),
}

struct Command {
    /// Translated label, shown.
    label: String,
    /// English label, also matched.
    en: String,
    shortcut: String,
    action: Action,
}

/// Opens or closes the palette, clearing the query.
pub fn toggle(app: &mut EditorApp) {
    app.palette.open = !app.palette.open;
    app.palette.query.clear();
    app.palette.sel = 0;
}

fn cmd(en: &str, shortcut: &str, action: Action) -> Command {
    Command { label: tr(en).to_string(), en: en.to_string(), shortcut: shortcut.to_string(), action }
}

/// A command named after a document item, e.g. "Select bone arm".
fn item(label: String, en: String, action: Action) -> Command {
    Command { label, en, shortcut: String::new(), action }
}

fn commands(app: &EditorApp) -> Vec<Command> {
    let mut out = Vec::new();
    if app.selection.slot.is_some() {
        out.push(cmd("Clip or unclip the selected slot", "Ctrl+Alt+G", Action::ToggleClip));
    }
    if app.selection.bone.is_some() {
        out.push(cmd("Reset shear", "", Action::ResetShear));
        if app.playback.anim.is_none() {
            for (mode, label) in [
                (skel2d_core::Inherit::Normal, "Inheritance: normal"),
                (skel2d_core::Inherit::OnlyTranslation, "Inheritance: only translation"),
                (skel2d_core::Inherit::NoRotationOrReflection, "Inheritance: no rotation or reflection"),
                (skel2d_core::Inherit::NoScale, "Inheritance: no scale"),
                (skel2d_core::Inherit::NoScaleOrReflection, "Inheritance: no scale or reflection"),
            ] { out.push(cmd(label, "", Action::BoneInheritance(mode))); }
        }
    }
    if app.selection.ik.is_some() || app.selection.physics.is_some() || app.selection.transform.is_some() {
        out.push(cmd("Move constraint earlier", "", Action::MoveConstraint(true)));
        out.push(cmd("Move constraint later", "", Action::MoveConstraint(false)));
    }
    if app.playback.anim.is_some() {
        if app.selection.ik.as_ref().is_some_and(|n| app.doc.ik.iter().any(|c| &c.name == n)) {
            out.push(cmd("Key IK values at the playhead", "", Action::KeyIk));
        }
        out.push(cmd("Key edited (only what you changed)", "K", Action::KeyEdited));
        out.push(cmd("Key shown rows (the whole dope sheet)", "Ctrl+Shift+L", Action::KeyShown));
        if app.selection.bone.is_some() {
            use crate::timeline::KeyProp;
            out.push(cmd("Key move of the selected bone", "", Action::KeyBoneProp(KeyProp::Translate)));
            out.push(cmd("Key rotation of the selected bone", "", Action::KeyBoneProp(KeyProp::Rotate)));
            out.push(cmd("Key scale of the selected bone", "", Action::KeyBoneProp(KeyProp::Scale)));
            out.push(cmd("Key shear of the selected bone", "", Action::KeyBoneProp(KeyProp::Shear)));
        }
        if app.selection.slot.is_some() {
            out.push(cmd("Key tint of the selected slot", "", Action::KeySlotColor));
        }
        if app.has_pending() {
            out.push(cmd("Discard unkeyed changes", "", Action::DiscardUnkeyed));
        }
        out.push(cmd("Dope sheet: select all keys", "Ctrl+A", Action::SelectAllKeys));
        out.push(cmd("Dope sheet: open or close all rows", "", Action::ExpandRows));
        out.push(cmd("Dope sheet: show all bones' rows on/off", "", Action::AllRows));
        out.push(cmd("Draw order: key the current order here", "", Action::DrawOrderKey));
        out.push(cmd("Draw order: use the setup order here", "", Action::DrawOrderSetup));
        if crate::panels::draw_order_key_here(app) {
            out.push(cmd("Draw order: remove the key here", "", Action::DrawOrderRemove));
        }
        if let Some(slot) = &app.selection.slot {
            out.push(item(crate::tf!("Draw order: bring slot {} forward", slot), format!("Draw order: bring slot {} forward", slot), Action::DrawOrderMove(slot.clone(), true)));
            out.push(item(crate::tf!("Draw order: send slot {} back", slot), format!("Draw order: send slot {} back", slot), Action::DrawOrderMove(slot.clone(), false)));
        }
    }
    out.push(cmd("New event", "", Action::NewEvent));
    for name in app.doc.events.keys() {
        out.push(item(crate::tf!("Select event {}", name), format!("Select event {}", name), Action::EventSelect(name.clone())));
        out.push(item(crate::tf!("Rename event {}", name), format!("Rename event {}", name), Action::EventRename(name.clone())));
        out.push(item(crate::tf!("Delete event {}", name), format!("Delete event {}", name), Action::EventDelete(name.clone())));
        if app.playback.anim.is_some() {
            out.push(item(crate::tf!("Key event {} at the playhead", name), format!("Key event {} at the playhead", name), Action::EventKey(name.clone())));
            if crate::panels::event_key_here(app, name).is_some() {
                out.push(item(crate::tf!("Remove event {} key at the playhead", name), format!("Remove event {} key at the playhead", name), Action::EventKeyRemove(name.clone())));
            }
        }
    }
    for (tool, _, key) in Tool::ALL {
        out.push(cmd(tool.label(), key.name(), Action::Tool(tool)));
    }
    out.push(cmd("Toggle setup / animate mode", "Tab", Action::ToggleMode));
    out.push(cmd("New", "", Action::New));
    out.push(cmd("Open...", "Ctrl+O", Action::Open));
    out.push(cmd("Save", "Ctrl+S", Action::Save));
    out.push(cmd("Save As...", "", Action::SaveAs));
    out.push(cmd("Undo", "Ctrl+Z", Action::Undo));
    out.push(cmd("Redo", "Ctrl+Shift+Z", Action::Redo));
    out.push(cmd("Frame all", "Shift+F", Action::Frame));
    out.push(cmd("Snapping on/off", "", Action::Snap));
    out.push(cmd("Onion skin", "O", Action::Onion));
    out.push(cmd("Bone names", "", Action::Names));
    out.push(cmd("Grid", "", Action::Grid));
    out.push(cmd("Bones", "", Action::Bones));
    if app.playback.anim.is_some() {
        out.push(cmd("Auto Key", "", Action::AutoKey));
    }
    out.push(cmd("Shortcuts and tips", "F1", Action::Help));
    out.push(cmd("UI size: bigger", "Ctrl+=", Action::UiScale(0.1)));
    out.push(cmd("UI size: smaller", "Ctrl+-", Action::UiScale(-0.1)));
    for lang in Lang::ALL {
        out.push(item(crate::tf!("Language: {}", lang.native_name()), format!("Language: {}", lang.native_name()), Action::Language(lang)));
    }
    out.push(cmd("Import images...", "", Action::ImportImages));
    out.push(cmd("New animation", "", Action::NewAnimation));
    out.push(cmd("New skin", "", Action::NewSkin));
    if app.doc.skins.len() >= 2 { out.push(cmd("Compare skins", "", Action::CompareSkins)); }
    out.push(cmd("New constraint", "", Action::NewConstraint));
    out.push(cmd("New physics constraint", "", Action::NewPhysics));
    out.push(cmd("New copy constraint", "", Action::NewTransform));
    if !app.doc.physics.is_empty() {
        out.push(cmd("Physics: restart from the current pose", "Shift+R", Action::PhysicsReset));
        out.push(cmd("Physics: pause / resume", "", Action::PhysicsPause));
        out.push(cmd("Physics: turn preview off / on", "", Action::PhysicsOff));
        if app.playback.anim.is_none() { out.push(cmd("Physics: preview in setup pose on / off", "", Action::PhysicsSetupPreview)); }
        if let Some(name) = &app.selection.physics { out.push(item(crate::tf!("Shake physics {}", name), format!("Shake physics {}", name), Action::PhysicsShake(name.clone()))); }
        for c in &app.doc.physics {
            out.push(item(crate::tf!("Select physics {}", c.name), format!("Select physics {}", c.name), Action::Physics(c.name.clone())));
        }
    }
    out.push(cmd("Preferences", "Ctrl+,", Action::Preferences));
    out.push(cmd("Export GIF or sprite sheet", "Ctrl+E", Action::ExportGif));
    if crate::dialogs::selected_item(app).is_some() {
        out.push(cmd("Rename", "F2", Action::RenameItem));
        out.push(cmd("Delete", "Del", Action::DeleteItem));
    }
    if app.playback.anim.is_some() { out.push(cmd("Duplicate current animation", "", Action::DuplicateAnimation)); }
    if app.selection.slot.is_some() { out.push(cmd("Mesh generation options", "", Action::MeshOptions)); }
    if app.playback.anim.is_none()
        && let Some(slot) = app.selection.slot.as_ref().and_then(|s| app.doc.slot(s))
        && let Some(name) = app.selection.attachment.as_deref().or(slot.attachment.as_deref())
        && let Some(si) = app.doc.slot_index(&slot.name)
        && matches!(app.doc.resolve_attachment(si, name, &app.skins), Some(skel2d_core::Attachment::Mesh(_))) {
        out.push(cmd("Create linked mesh...", "", Action::NewLinkedMesh));
    }
    if let Some(slot) = app.selection.slot.as_ref().and_then(|s| app.doc.slot(s))
        && let Some(si) = app.doc.slot_index(&slot.name)
        && let Some(name) = app.selection.attachment.as_deref().or(slot.attachment.as_deref())
        && matches!(app.doc.resolve_attachment(si, name, &app.skins), Some(skel2d_core::Attachment::LinkedMesh(_))) {
        out.push(cmd("Go to parent mesh", "", Action::GoToLinkedParent));
    }
    if app.selection.slot.is_some() { out.push(cmd("Rename attachment…", "", Action::RenameAttachment)); }
    if app.playback.anim.is_none() && crate::viewport::picked(app,&app.pose()).is_some() {
        out.push(cmd("Auto weights from image", "", Action::ImageWeights));
        use crate::dialogs::WeightAction;
        for (action,label) in [(WeightAction::Smooth,"Smooth weights"),(WeightAction::Prune,"Prune weights…"),(WeightAction::Mirror,"Mirror weights…"),(WeightAction::Transfer,"Transfer weights…"),(WeightAction::Bind,"Remove selected bone"),(WeightAction::BindAdd,"Add binding bone…"),(WeightAction::Swap,"Swap bone weights…"),(WeightAction::Set,"Set bone weight…"),(WeightAction::Normalize,"Normalize weights"),(WeightAction::Distance,"Auto weights (distance)")] {
            out.push(cmd(label,"",Action::WeightOptions(action)));
        }
        out.push(cmd("Pose test","Alt+drag",Action::PoseTest));
        use crate::viewport::{MeshAction,MeshMode};
        use skel2d_core::ops::MeshResetMode;
        for (mode,label) in [(MeshMode::Modify,"Mesh mode: modify"),(MeshMode::Create,"Mesh mode: create"),(MeshMode::Delete,"Mesh mode: delete"),(MeshMode::Edges,"Mesh mode: edges")] {
            out.push(cmd(label,"",Action::MeshMode(mode)));
        }
        for (action,label) in [(MeshAction::Generate(8),"Generate mesh vertices"),(MeshAction::RemovePicked,"Remove picked mesh vertices"),(MeshAction::RemoveEdge,"Remove selected mesh edge"),(MeshAction::ClearEdges,"Clear constrained edges"),(MeshAction::Reset(MeshResetMode::VerticesToUvs),"Reset vertices to UVs"),(MeshAction::Reset(MeshResetMode::UvsToVertices),"Fit UVs to vertices")] {
            if crate::viewport::selected_mesh_op(app,action).is_some() { out.push(cmd(label,"",Action::MeshAction(action))); }
        }
    }
    for b in &app.doc.bones {
        out.push(item(crate::tf!("Select bone {}", b.name), format!("Select bone {}", b.name), Action::Bone(b.name.clone())));
    }
    for s in &app.doc.slots {
        out.push(item(crate::tf!("Select slot {}", s.name), format!("Select slot {}", s.name), Action::Slot(s.name.clone())));
    }
    for name in app.doc.animations.keys() {
        out.push(item(crate::tf!("Open animation {}", name), format!("Open animation {}", name), Action::Animation(name.clone())));
        if Some(name) != app.playback.anim.as_ref() && !app.playback.layers.iter().any(|l| &l.anim == name) {
            out.push(item(crate::tf!("Preview animation {}", name), format!("Preview animation {}", name), Action::AddLayer(name.clone())));
        }
    }
    for layer in &app.playback.layers {
        let name = &layer.anim;
        out.push(item(crate::tf!("Toggle preview layer {}", name), format!("Toggle preview layer {}", name), Action::ToggleLayer(name.clone())));
        out.push(item(crate::tf!("Remove preview layer {}", name), format!("Remove preview layer {}", name), Action::RemoveLayer(name.clone())));
        out.push(item(crate::tf!("Move preview layer {} up", name), format!("Move preview layer {} up", name), Action::MoveLayer(name.clone(), true)));
        out.push(item(crate::tf!("Move preview layer {} down", name), format!("Move preview layer {} down", name), Action::MoveLayer(name.clone(), false)));
        for percent in [25, 50, 75, 100] {
            out.push(item(crate::tf!("Preview layer {} opacity {}%", name, percent), format!("Preview layer {} opacity {}%", name, percent), Action::LayerAlpha(name.clone(), percent as f32 / 100.0)));
        }
    }
    out
}

/// Subsequence match score (higher is better), `None` when `query` does not match.
/// Word starts, runs and a plain substring rank higher.
fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let lower = text.to_lowercase();
    let t: Vec<char> = lower.chars().collect();
    let mut total = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for &qc in &q {
        let pos = (ti..t.len()).find(|&i| t[i] == qc)?;
        total += 1;
        if pos == 0 || !t[pos - 1].is_alphanumeric() {
            total += 6;
        }
        if prev.is_some_and(|p| p + 1 == pos) {
            total += 4;
        }
        if pos == 0 {
            total += 10;
        }
        prev = Some(pos);
        ti = pos + 1;
    }
    let joined: String = q.iter().collect();
    if lower.contains(&joined) {
        total += 20;
    }
    Some(total - t.len() as i32 / 8)
}

fn run(app: &mut EditorApp, action: Action) {
    match action {
        Action::ToggleClip => crate::panels::toggle_clip(app),
        Action::ResetShear => {
            if let Some(name) = app.selection.bone.clone() {
                let op = if let Some(animation) = app.playback.anim.clone() {
                    skel2d_core::Op::KeyBone(skel2d_core::ops::KeyBone { animation, bone:name, time:app.playback.time, shear:Some([0.0; 2]), rotate:None, translate:None, scale:None, curve:None, curves:None })
                } else {
                    skel2d_core::Op::SetBone(skel2d_core::ops::BonePatch {name,shear_x:Some(0.0),shear_y:Some(0.0),..Default::default()})
                };
                app.edit(vec![op]);
            }
        }
        Action::BoneInheritance(inherit) => {
            if app.playback.anim.is_none() && let Some(name) = app.selection.bone.clone() {
                app.edit(vec![skel2d_core::Op::SetBone(skel2d_core::ops::BonePatch {name,inherit:Some(inherit),..Default::default()})]);
            }
        }
        Action::AddLayer(name) => app.playback.add_layer(&name),
        Action::RemoveLayer(name) => app.playback.layers.retain(|l| l.anim != name),
        Action::ToggleLayer(name) => { if let Some(layer) = app.playback.layers.iter_mut().find(|l| l.anim == name) { layer.enabled = !layer.enabled; } }
        Action::MoveLayer(name, up) => app.playback.move_layer(&name, up),
        Action::LayerAlpha(name, alpha) => { if let Some(layer) = app.playback.layers.iter_mut().find(|l| l.anim == name) { layer.alpha = alpha; } }
        Action::MoveConstraint(earlier) => crate::panels::move_constraint(app, earlier),
        Action::KeyEdited => { crate::timeline::key_edited(app); }
        Action::KeyShown => { crate::timeline::key_shown(app); }
        Action::KeyBoneProp(p) => { crate::timeline::key_bone_property(app, p); }
        Action::KeySlotColor => { crate::timeline::key_slot_color(app); }
        Action::SelectAllKeys => crate::timeline::select_all_keys(app),
        Action::ExpandRows => crate::timeline::toggle_expand_all(app),
        Action::AllRows => app.timeline.show_all = !app.timeline.show_all,
        Action::DiscardUnkeyed => {
            app.pending.clear();
            app.pending_other.clear();
        }
        Action::KeyIk => {
            if let Some(ik) = app.selection.ik.as_ref().and_then(|n| app.doc.ik.iter().find(|c| &c.name == n)).cloned() {
                crate::panels::key_ik_now(app, &ik);
            }
        }
        Action::DrawOrderKey => {
            let order = crate::panels::current_draw_order(app);
            crate::panels::key_draw_order(app, Some(order));
        }
        Action::DrawOrderSetup => { crate::panels::key_draw_order(app, None); }
        Action::DrawOrderRemove => { crate::panels::remove_draw_order_key(app); }
        Action::DrawOrderMove(slot, forward) => { crate::panels::move_slot_in_draw_order(app, &slot, forward); }
        Action::NewEvent => crate::dialogs::new_event(app),
        Action::EventSelect(name) => app.timeline.event = Some(name),
        Action::EventKey(name) => {
            app.timeline.event = Some(name.clone());
            crate::panels::key_event_now(app, &name);
        }
        Action::EventKeyRemove(name) => { crate::panels::remove_event_key(app, &name); }
        Action::EventRename(name) => crate::dialogs::rename(app, crate::dialogs::Item::Event(name)),
        Action::EventDelete(name) => crate::dialogs::delete(app, crate::dialogs::Item::Event(name)),
        Action::Tool(t) => app.set_tool(t),
        Action::ToggleMode => app.toggle_mode(),
        Action::New => app.request_replace(crate::app::Replace::New),
        Action::Open => app.request_replace(crate::app::Replace::Open),
        Action::Save => app.save(),
        Action::SaveAs => app.save_as(),
        Action::Undo => app.undo(),
        Action::Redo => app.redo(),
        Action::Frame => app.camera.needs_fit = true,
        Action::Snap => app.settings.snap = !app.settings.snap,
        Action::Onion => app.settings.onion_skin = !app.settings.onion_skin,
        Action::Names => app.settings.show_bone_names = !app.settings.show_bone_names,
        Action::Grid => app.settings.show_grid = !app.settings.show_grid,
        Action::Bones => app.show_bones = !app.show_bones,
        Action::AutoKey => app.settings.auto_key = !app.settings.auto_key,
        Action::Help => app.show_help = !app.show_help,
        Action::UiScale(d) => app.settings.ui_scale = (app.settings.ui_scale + d).clamp(0.8, 2.0),
        Action::Language(l) => {
            app.settings.language = l;
            i18n::set(l);
        }
        Action::ImportImages => {
            let files = crate::import::pick_images();
            if !files.is_empty() {
                crate::import::import_picked(app, files);
            }
        }
        Action::NewAnimation => crate::dialogs::new_animation(app),
        Action::Preferences => crate::dialogs::preferences(app),
        Action::ExportGif => crate::dialogs::export_gif(app),
        Action::NewSkin => crate::dialogs::new_skin(app),
        Action::CompareSkins => crate::panels::compare_skins(app),
        Action::NewLinkedMesh => {
            if let Some(slot) = app.selection.slot.clone()
                && let Some(name) = app.selection.attachment.clone().or_else(|| app.doc.slot(&slot).and_then(|s| s.attachment.clone())) {
                crate::dialogs::new_linked_mesh(app, &slot, &name);
            }
        }
        Action::GoToLinkedParent => {
            if let Some(slot) = app.selection.slot.clone()
                && let Some(si) = app.doc.slot_index(&slot)
                && let Some(name) = app.selection.attachment.clone().or_else(|| app.doc.slot(&slot).and_then(|s| s.attachment.clone()))
                && let Some(skel2d_core::Attachment::LinkedMesh(link)) = app.doc.resolve_attachment(si, &name, &app.skins).cloned() {
                crate::panels::go_to_linked_parent(app, &slot, &link);
            }
        }
        Action::NewConstraint => crate::dialogs::new_ik(app, app.selection.bone.clone()),
        Action::NewPhysics => crate::dialogs::new_physics(app, app.selection.bone.clone()),
        Action::NewTransform => crate::dialogs::new_transform(app, app.selection.bone.clone()),
        Action::PhysicsReset => app.physics_reset(),
        Action::PhysicsPause => app.physics.paused = !app.physics.paused,
        Action::PhysicsOff => app.physics.enabled = !app.physics.enabled,
        Action::PhysicsSetupPreview => app.physics.setup_preview = !app.physics.setup_preview,
        Action::PhysicsShake(name) => app.physics_shake(&name),
        Action::Physics(name) => { app.selection = crate::app::Selection { physics: Some(name), ..Default::default() }; }
        Action::RenameItem => { if let Some(item) = crate::dialogs::selected_item(app) { crate::dialogs::rename(app, item); } }
        Action::MeshMode(mode) => { app.set_tool(Tool::Mesh); app.view.mesh_mode=mode; }
        Action::MeshAction(action) => crate::viewport::apply_mesh_action(app,action),
        Action::RenameAttachment => {
            if let Some(slot) = app.selection.slot.clone()
                && let Some(si) = app.doc.slot_index(&slot)
                && let Some(attachment) = app.pose().attachment(&app.doc, si) {
                    let name = attachment.name().to_string();
                    let skin = app.skin_of(&slot, &name);
                    crate::dialogs::rename(app, crate::dialogs::Item::Attachment { slot, name, skin });
                }
        }
        Action::DeleteItem => { if let Some(item) = crate::dialogs::selected_item(app) { crate::dialogs::delete(app, item); } }
        Action::DuplicateAnimation => { if let Some(name) = app.playback.anim.clone() { crate::dialogs::duplicate_animation(app, &name); } }
        Action::MeshOptions => crate::dialogs::selected_mesh_options(app),
        Action::ImageWeights => crate::dialogs::selected_image_weights(app),
        Action::WeightOptions(action) => {app.set_tool(Tool::Weights);crate::dialogs::weight_options(app,action);}
        Action::PoseTest => {app.set_tool(Tool::Weights);app.view.pose_test = !app.view.pose_test;if app.view.pose_angle==0.0 {app.view.pose_angle=30.0;}}
        Action::Bone(b) => {
            app.selection.bone = Some(b);
            app.selection.slot = None;
            app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
        }
        Action::Slot(s) => {
            app.selection.bone = app.doc.slot(&s).map(|s| s.bone.clone());
            app.selection.slot = Some(s);
            app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
        }
        Action::Animation(a) => open_animation(app, &a),
    }
}

fn open_animation(app: &mut EditorApp, name: &str) {
    app.playback.anim = Some(name.to_string());
    app.playback.playing = false;
    app.playback.time = 0.0;
    app.timeline.clear_selection();
}

/// Draws the palette when open and runs the chosen command after closing it.
pub fn ui(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.palette.open {
        return;
    }
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        app.palette.open = false;
        return;
    }

    let mut matches: Vec<(i32, Command)> = commands(app)
        .into_iter()
        .filter_map(|c| {
            let s = score(&app.palette.query, &c.label).into_iter().chain(score(&app.palette.query, &c.en)).max()?;
            Some((s, c))
        })
        .collect();
    matches.sort_by_key(|m| std::cmp::Reverse(m.0));
    let n = matches.len();
    if down && n > 0 {
        app.palette.sel = (app.palette.sel + 1) % n;
    }
    if up && n > 0 {
        app.palette.sel = (app.palette.sel + n - 1) % n;
    }
    app.palette.sel = app.palette.sel.min(n.saturating_sub(1));

    let mut chosen: Option<Action> = None;
    if enter {
        chosen = matches.get(app.palette.sel).map(|m| m.1.action.clone());
    }
    let nav = up || down;
    let area = egui::Area::new(egui::Id::unique("command_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, [0.0, 56.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(PANEL_BG).corner_radius(8.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(WIDTH);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut app.palette.query)
                        .hint_text(tr("Type a command, bone, slot or animation..."))
                        .desired_width(f32::INFINITY),
                );
                edit.request_focus();
                if edit.changed() {
                    app.palette.sel = 0;
                }
                ui.add_space(4.0);
                if matches.is_empty() {
                    ui.colored_label(TEXT_DIM, tr("No matching commands"));
                    return;
                }
                egui::ScrollArea::vertical().max_height(ROW_H * MAX_ROWS).auto_shrink([false, true]).show(ui, |ui| {
                    for (i, (_, c)) in matches.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
                        if i == app.palette.sel {
                            ui.painter().rect_filled(rect, 4.0, ACCENT.gamma_multiply(0.25));
                            if nav {
                                ui.scroll_to_rect(rect, None);
                            }
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 4.0, egui::Color32::from_white_alpha(10));
                        }
                        ui.painter().text(rect.left_center() + egui::vec2(8.0, 0.0), Align2::LEFT_CENTER, &c.label, egui::FontId::proportional(14.0), TEXT);
                        if !c.shortcut.is_empty() {
                            ui.painter().text(rect.right_center() - egui::vec2(8.0, 0.0), Align2::RIGHT_CENTER, &c.shortcut, egui::FontId::monospace(12.0), TEXT_DIM);
                        }
                        if resp.clicked() {
                            chosen = Some(c.action.clone());
                        }
                    }
                });
            });
        });
    let outside = ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p));
    if outside {
        app.palette.open = false;
    }
    if let Some(action) = chosen {
        app.palette.open = false;
        app.palette.query.clear();
        run(app, action);
    }
}
