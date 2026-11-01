//! Editor modals. Forms produce the same atomic ops as CLI/HTTP/MCP; view
//! state is updated only after a successful apply. Cancel never edits a document.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Key, Modifiers};
use skel2d_core::ops::{BonePatch, IkPatch, PhysicsPatch, SlotPatch};
use skel2d_core::{Animation, Attachment, Bone, Document, IkConstraint, Op, PhysicsConstraint, Slot, TransformConstraint};
use skel2d_rig::mesh::Density;

use crate::app::{Dialog, EditorApp, Selection, Settings};
use crate::i18n::tr;
use crate::theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Attachment { slot: String, name: String, skin: Option<String> },
    Bone(String),
    Slot(String),
    Ik(String),
    Physics(String),
    Transform(String),
    Skin(String),
    Animation(String),
    Event(String),
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Self::Attachment { name, .. } => name,
            Self::Bone(n) | Self::Slot(n) | Self::Ik(n) | Self::Physics(n) | Self::Transform(n) | Self::Skin(n) | Self::Animation(n) | Self::Event(n) => n,
        }
    }

    fn exists(&self, doc: &Document, name: &str) -> bool {
        match self {
            Self::Attachment { slot, skin, .. } => match skin {
                Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, name)).is_some(),
                None => doc.slot(slot).and_then(|s| s.attachment(name)).is_some(),
            },
            Self::Bone(_) => doc.bone(name).is_some(),
            Self::Slot(_) => doc.slot(name).is_some(),
            Self::Ik(_) | Self::Physics(_) | Self::Transform(_) => doc.has_constraint(name),
            Self::Skin(_) => doc.skin(name).is_some(),
            Self::Animation(_) => doc.animations.contains_key(name),
            Self::Event(_) => doc.events.contains_key(name),
        }
    }

    fn rename_op(&self, name: String) -> Op {
        let old = self.name().to_string();
        match self {
            Self::Attachment { slot, skin, .. } => Op::RenameAttachment { slot: slot.clone(), from: old, to: name, skin: skin.clone() },
            Self::Bone(_) => Op::SetBone(BonePatch { name: old, rename: Some(name), ..Default::default() }),
            Self::Slot(_) => Op::SetSlot(SlotPatch { name: old, rename: Some(name), ..Default::default() }),
            Self::Ik(_) => Op::SetIk(IkPatch { name: old, rename: Some(name), ..Default::default() }),
            Self::Physics(_) => Op::SetPhysics(PhysicsPatch { name: old, rename: Some(name), ..Default::default() }),
            Self::Transform(_) => Op::SetTransform(skel2d_core::ops::TransformPatch { name: old, rename: Some(name), ..Default::default() }),
            Self::Skin(_) => Op::SetSkin { name: old, rename: Some(name), bones: None, ik: None, constraints: None },
            Self::Animation(_) => Op::SetAnimation { name: old, rename: Some(name), duration: None },
            Self::Event(_) => Op::SetEvent { name: old, rename: Some(name), int: None, float: None, string: None },
        }
    }

    fn remove_op(&self) -> Op {
        let name = self.name().to_string();
        match self {
            Self::Attachment { slot, skin, .. } => match skin {
                Some(k) => Op::RemoveSkinAttachment { skin: k.clone(), slot: slot.clone(), name, on_parent_removed: Default::default() },
                None => Op::RemoveAttachment { slot: slot.clone(), name, on_parent_removed: Default::default() },
            },
            Self::Bone(_) => Op::RemoveBone { name, recursive: true },
            Self::Slot(_) => Op::RemoveSlot { name },
            Self::Ik(_) => Op::RemoveIk { name },
            Self::Physics(_) => Op::RemovePhysics { name },
            Self::Transform(_) => Op::RemoveTransform { name },
            Self::Skin(_) => Op::RemoveSkin { name, on_parent_removed: Default::default() },
            Self::Animation(_) => Op::RemoveAnimation { name },
            Self::Event(_) => Op::RemoveEvent { name },
        }
    }
}

pub struct NewEvent {
    name: String,
    int: i32,
    float: f32,
    string: String,
}

pub struct NewAnimation {
    name: String,
    duration: f32,
    fps: f32,
    source: Option<String>,
    copy: bool,
}

pub struct NewSkin {
    name: String,
    copy: bool,
}

pub struct NewIk {
    name: String,
    bone: String,
    include_parent: bool,
    target: Option<String>,
    new_target: String,
}

pub struct NewPhysics {
    name: String,
    bone: String,
    /// One constraint per bone of the single-child chain below `bone`.
    per_bone: bool,
}

pub struct NewTransform {
    name: String,
    bone: String,
    source: String,
}

pub struct Rename {
    item: Item,
    name: String,
    all_skins: bool,
}

impl Rename {
    fn op(&self) -> Op {
        match &self.item {
            Item::Attachment { slot, name, .. } if self.all_skins => Op::RenamePlaceholder { slot: slot.clone(), from: name.clone(), to: self.name.trim().to_string() },
            _ => self.item.rename_op(self.name.trim().to_string()),
        }
    }
}

pub struct Delete {
    item: Item,
    impact: Impact,
    on_parent_removed: skel2d_core::ops::ParentRemoval,
}

pub struct NewLinkedMesh {
    slot: String,
    parent: String,
    parent_skin: Option<String>,
    skin: Option<String>,
    name: String,
    image: String,
    timelines: bool,
}

impl NewLinkedMesh {
    fn op(&self) -> Op {
        Op::AddLinkedMesh {
            slot: self.slot.clone(), name: self.name.trim().into(), parent: self.parent.clone(),
            parent_skin: self.parent_skin.clone(), skin: self.skin.clone(),
            image: (!self.image.trim().is_empty()).then(|| self.image.trim().into()), timelines: self.timelines,
        }
    }
}

impl Delete {
    fn op(&self) -> Op {
        with_parent_policy(self.item.remove_op(), self.on_parent_removed)
    }
}

fn with_parent_policy(op: Op, policy: skel2d_core::ops::ParentRemoval) -> Op {
    match op {
        Op::RemoveAttachment { slot, name, .. } => Op::RemoveAttachment { slot, name, on_parent_removed: policy },
        Op::RemoveSkinAttachment { skin, slot, name, .. } => Op::RemoveSkinAttachment { skin, slot, name, on_parent_removed: policy },
        Op::RemoveSkin { name, .. } => Op::RemoveSkin { name, on_parent_removed: policy },
        other => other,
    }
}

pub struct MeshOptions {
    slot: Slot,
    attachment: Attachment,
    skin: Option<String>,
    base: Document,
    params: skel2d_rig::TraceParams,
    density: Density,
    /// Without an image only an explicit rectangle grid is available.
    rectangle: bool,
    cols: u32,
    rows: u32,
    auto_weights: bool,
    signature: String,
    expanded: Vec<Op>,
    error: Option<String>,
    summary: String,
}

pub struct Preferences {
    settings: Settings,
    tab: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WeightAction { Prune, Mirror, Transfer, Bind, BindAdd, Swap, Set, Smooth, Normalize, Distance }

impl WeightAction {
    fn title(self) -> &'static str {
        match self {
            Self::Prune => "Clean up small weights",
            Self::Mirror => "Mirror weights",
            Self::Transfer => "Copy weights from another mesh",
            Self::Bind => "Take a bone off this mesh",
            Self::BindAdd => "Add a bone to this mesh",
            Self::Swap => "Swap two bones' weights",
            Self::Set => "Set a bone's weight",
            Self::Smooth => "Smooth weights",
            Self::Normalize => "Normalize weights",
            Self::Distance => "Weights by distance",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Self::Prune => "removes influences too small to see, so bending stays clean.",
            Self::Mirror => "copies one half's weights onto the other, for symmetrical images.",
            Self::Transfer => "each point takes the weights of the nearest point on the other mesh.",
            Self::Bind => "its weight goes to the other bones.",
            Self::BindAdd => "the bone can then move this mesh.",
            Self::Swap => "each takes over what the other one moved.",
            Self::Set => "changes how strongly the bone moves the points.",
            Self::Smooth => "evens out differences between neighbouring points, for softer bends.",
            Self::Normalize => "makes each point's weights add up to 100%.",
            Self::Distance => "closer bones move a point more.",
        }
    }
}

pub struct WeightOptions {
    base: Document,
    slot: String,
    attachment: String,
    skin: Option<String>,
    vertices: Option<Vec<usize>>,
    locked_bones: Vec<String>,
    action: WeightAction,
    max_bones: usize,
    threshold: f32,
    axis: f32,
    source: skel2d_core::ops::MirrorSide,
    from: Option<(String,String,Option<String>)>,
    remove: String,
    other: String,
    value: f32,
    mode: skel2d_core::ops::WeightMode,
    redistribute: skel2d_core::ops::Redistribute,
    expanded: Vec<Op>,
    error: Option<String>,
    summary: String,
}

pub fn weight_options(app: &mut EditorApp, action: WeightAction) {
    if app.playback.anim.is_some() {app.status=tr("Weight edits require setup pose").into();return;}
    let Some(slot)=app.selection.slot.as_ref().and_then(|s|app.doc.slot(s)).cloned() else {return};
    let Some(name)=slot.attachment.clone() else {return};
    let Some(Attachment::Mesh(mesh))=app.doc.resolve_attachment(app.doc.slot_index(&slot.name).unwrap(),&name,&app.skins) else {return};
    let vertices=crate::viewport::picked(app,&app.pose()).map(|(_,p)|p).filter(|p|!p.is_empty());
    let form=WeightOptions {
        base:app.doc.clone(),slot:slot.name.clone(),attachment:mesh.name.clone(),skin:app.skin_of(&slot.name,&mesh.name),
        vertices,locked_bones:app.view.weight_locked.iter().cloned().collect(),action,max_bones:4,threshold:0.01,axis:0.0,
        source:skel2d_core::ops::MirrorSide::Positive,from:None,remove:app.selection.bone.clone().unwrap_or_default(),
        other:slot.bone.clone(),value:0.5,mode:skel2d_core::ops::WeightMode::Set,
        redistribute:skel2d_core::ops::Redistribute::Auto,expanded:vec![],error:None,summary:String::new(),
    };
    open(app,Form::Weights(Box::new(form)));
}

enum Form {
    Animation(NewAnimation),
    Event(NewEvent),
    Skin(NewSkin),
    Ik(NewIk),
    Physics(NewPhysics),
    Transform(NewTransform),
    Rename(Rename),
    Delete(Delete),
    Mesh(Box<MeshOptions>),
    Weights(Box<WeightOptions>),
    Linked(NewLinkedMesh),
    Preferences(Preferences),
    Gif(crate::export::GifRequest),
}

pub struct EditorDialog {
    form: Form,
    first: bool,
    error: String,
}

pub enum DialogResult {
    Keep,
    Close,
    Apply(Vec<Op>),
}

fn open(app: &mut EditorApp, form: Form) {
    app.playback.playing = false;
    app.dialog = Dialog::Editor(Box::new(EditorDialog { form, first: true, error: String::new() }));
}

fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (1..).map(|i| format!("{base}{i}")).find(|n| !taken(n)).expect("unbounded names")
}

pub fn new_animation(app: &mut EditorApp) {
    let source = app.playback.anim.clone();
    open(app, Form::Animation(NewAnimation {
        name: unique("animation", |n| app.doc.animations.contains_key(n)),
        duration: app.animation().map_or(1.0, Animation::duration),
        fps: app.settings.fps,
        source,
        copy: false,
    }));
}

pub fn duplicate_animation(app: &mut EditorApp, name: &str) {
    let Some(animation) = app.doc.animations.get(name) else { return };
    let form = NewAnimation {
        name: unique(&format!("{name}_copy"), |n| app.doc.animations.contains_key(n)),
        duration: animation.duration(), fps: app.settings.fps, source: Some(name.to_string()), copy: true,
    };
    open(app, Form::Animation(form));
}

pub fn new_event(app: &mut EditorApp) {
    open(app, Form::Event(NewEvent { name: unique("event", |n| app.doc.events.contains_key(n)), int: 0, float: 0.0, string: String::new() }));
}

pub fn new_skin(app: &mut EditorApp) {
    open(app, Form::Skin(NewSkin { name: unique("skin", |n| app.doc.skin(n).is_some()), copy: false }));
}

pub fn new_ik(app: &mut EditorApp, bone: Option<String>) {
    let bone = bone.or_else(|| app.doc.bones.first().map(|b| b.name.clone())).unwrap_or_default();
    let form = NewIk {
        name: unique("ik", |n| app.doc.has_constraint(n)),
        new_target: unique(&format!("{bone}_target"), |n| app.doc.bone(n).is_some()),
        bone, include_parent: true, target: None,
    };
    open(app, Form::Ik(form));
}

pub fn new_physics(app: &mut EditorApp, bone: Option<String>) {
    let bone = bone.or_else(|| app.doc.bones.first().map(|b| b.name.clone())).unwrap_or_default();
    let name = unique(&format!("{bone}_physics"), |n| app.doc.has_constraint(n));
    open(app, Form::Physics(NewPhysics { name, bone, per_bone: false }));
}

pub fn new_transform(app: &mut EditorApp, bone: Option<String>) {
    let bone = bone.or_else(|| app.doc.bones.first().map(|b| b.name.clone())).unwrap_or_default();
    let name = unique(&format!("{bone}_copy"), |n| app.doc.has_constraint(n));
    let source = app.doc.bones.iter().find(|b| b.name != bone && b.name != skel2d_core::ROOT).map_or_else(String::new, |b| b.name.clone());
    open(app, Form::Transform(NewTransform { name, bone, source }));
}

pub fn rename(app: &mut EditorApp, item: Item) {
    if matches!(&item, Item::Bone(n) if n == skel2d_core::ROOT) {
        app.status = tr(crate::panels::ROOT_REASON).into();
        return;
    }
    open(app, Form::Rename(Rename { name: item.name().to_string(), item, all_skins: false }));
}

pub fn delete(app: &mut EditorApp, item: Item) {
    if matches!(&item, Item::Bone(n) if n == skel2d_core::ROOT) {
        app.status = tr(crate::panels::ROOT_REASON).into();
        return;
    }
    match deletion_impact(&app.doc, &item) {
        Ok(impact) if impact.needs_confirmation(&item) || has_linked_children(&app.doc, &item) => open(app, Form::Delete(Delete { item, impact, on_parent_removed: skel2d_core::ops::ParentRemoval::Freeze })),
        Ok(_) => {
            if app.edit(vec![item.remove_op()]) {
                app.status = crate::tf!("Deleted {} · Ctrl+Z to undo", item.name());
            }
        }
        Err(e) => app.status = e,
    }
}

fn has_linked_children(doc: &Document, item: &Item) -> bool {
    match item {
        Item::Attachment { slot, name, skin } => doc.slot(slot).into_iter().flat_map(|s| &s.attachments)
            .chain(doc.skins.iter().flat_map(|s| s.attachments.get(slot).into_iter().flatten()))
            .any(|a| matches!(a, Attachment::LinkedMesh(l) if &l.parent == name && &l.parent_skin == skin)),
        Item::Skin(name) => doc.slots.iter().flat_map(|s| &s.attachments)
            .chain(doc.skins.iter().filter(|s| &s.name != name).flat_map(|s| s.attachments.values().flatten()))
            .any(|a| matches!(a, Attachment::LinkedMesh(l) if l.parent_skin.as_ref() == Some(name))),
        _ => false,
    }
}

pub fn new_linked_mesh(app: &mut EditorApp, slot: &str, parent: &str) {
    if app.playback.anim.is_some() { app.status = tr("Asset edits require setup pose").into(); return; }
    let Some(si) = app.doc.slot_index(slot) else { return };
    let Some((parent_skin, Attachment::Mesh(_))) = app.doc.resolve_attachment_from(si, parent, &app.skins) else { return };
    let parent_skin = parent_skin.map(str::to_string);
    let skin = app.skins.last().cloned().or_else(|| app.doc.skins.first().map(|s| s.name.clone()));
    let base = if skin.is_some() { parent.to_string() } else { format!("{parent}_linked") };
    let name = unique(&base, |n| match skin.as_deref() {
        Some(k) => app.doc.skin(k).and_then(|s| s.attachment(slot, n)).is_some(),
        None => app.doc.slot(slot).and_then(|s| s.attachment(n)).is_some(),
    });
    open(app, Form::Linked(NewLinkedMesh { slot: slot.into(), parent: parent.into(), parent_skin, skin, name, image: String::new(), timelines: true }));
}

pub fn attachment_mesh_options(app: &mut EditorApp, slot: &Slot, attachment: &Attachment) {
    if app.playback.anim.is_some() {
        app.status = tr("Asset edits require setup pose").into();
        return;
    }
    let rectangle = matches!(attachment, Attachment::Region(r) if r.image.is_none());
    open(app, Form::Mesh(Box::new(MeshOptions {
        slot: slot.clone(), attachment: attachment.clone(), skin: app.skin_of(&slot.name, attachment.name()),
        base: app.doc.clone(), params: skel2d_rig::TraceParams::default(), density: Density::Medium,
        rectangle, cols: 4, rows: 4, auto_weights: false,
        signature: String::new(), expanded: vec![], error: None, summary: String::new(),
    })));
}

pub fn preferences(app: &mut EditorApp) {
    open(app, Form::Preferences(Preferences { settings: app.settings.clone(), tab: 0 }));
}

pub fn export_gif(app: &mut EditorApp) {
    let request = app.last_gif.clone().filter(|r| r.still_valid(&app.doc)).unwrap_or_else(|| crate::export::GifRequest::new(app));
    open(app, Form::Gif(request));
}

pub fn selected_item(app: &EditorApp) -> Option<Item> {
    if let (Some(slot), Some(name)) = (&app.selection.slot, &app.selection.attachment)
        && let Some(si) = app.doc.slot_index(slot)
        && let Some((skin, _)) = app.doc.resolve_attachment_from(si, name, &app.skins) {
            return Some(Item::Attachment { slot: slot.clone(), name: name.clone(), skin: skin.map(str::to_string) });
        }
    app.selection.ik.clone().map(Item::Ik)
        .or_else(|| app.selection.physics.clone().map(Item::Physics))
        .or_else(|| app.selection.transform.clone().map(Item::Transform))
        .or_else(|| app.selection.skin.clone().map(Item::Skin))
        .or_else(|| app.selection.slot.clone().map(Item::Slot))
        .or_else(|| app.selection.bone.clone().map(Item::Bone))
        .or_else(|| app.playback.anim.clone().map(Item::Animation))
}

pub fn selected_image_weights(app: &mut EditorApp) {
    let Some(slot) = app.selection.slot.clone() else { return };
    let Some(si) = app.doc.slot_index(&slot) else { return };
    let Some(name) = app.doc.slots[si].attachment.clone() else { return };
    let op = crate::import::auto_weights(app, &slot, &name, 4, None);
    if app.edit_any(vec![op.into()]) {
        app.status = tr("Image weights updated · Ctrl+Z to undo").into();
    }
}

pub fn selected_mesh_options(app: &mut EditorApp) {
    let Some(slot) = app.selection.slot.as_deref().and_then(|n| app.doc.slot(n)).cloned() else { return };
    let Some(si) = app.doc.slot_index(&slot.name) else { return };
    if let Some(attachment) = slot.attachment.as_deref().and_then(|n| app.doc.resolve_attachment(si, n, &app.skins)).cloned() {
        attachment_mesh_options(app, &slot, &attachment);
    }
}

fn name_error(name: &str, duplicate: bool) -> Option<String> {
    if name.trim().is_empty() {
        Some(tr("Name must not be empty").to_string())
    } else if duplicate {
        Some(tr("That name already exists").to_string())
    } else {
        None
    }
}

fn name_input(ui: &mut egui::Ui, name: &mut String, first: bool) {
    ui.label(tr("Name"));
    let response = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width()));
    if first {
        response.request_focus();
    }
}

fn number(ui: &mut egui::Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    number_tip(ui, label, "", value, range);
}

fn number_tip(ui: &mut egui::Ui, label: &str, tip: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let speed = if *range.end() - *range.start() <= 1.0 { 0.01 } else { 0.1 };
    theme::field(ui, tr(label), tr(tip), |ui| {
        ui.add_sized([96.0, 28.0], egui::DragValue::new(value).range(range).speed(speed).max_decimals(3));
    });
}

fn count_field(ui: &mut egui::Ui, label: &str, tip: &str, value: &mut usize, range: std::ops::RangeInclusive<usize>) {
    theme::field(ui, tr(label), tr(tip), |ui| {
        ui.add_sized([96.0, 28.0], egui::DragValue::new(value).range(range));
    });
}

impl EditorDialog {
    fn title(&self) -> &'static str {
        match &self.form {
            Form::Animation(_) => "New animation",
            Form::Event(_) => "New event",
            Form::Skin(_) => "New skin",
            Form::Ik(_) | Form::Physics(_) | Form::Transform(_) => "New constraint",
            Form::Rename(_) => "Rename",
            Form::Delete(_) => "Confirm deletion",
            Form::Mesh(_) => "Make a mesh",
            Form::Weights(f) => f.action.title(),
            Form::Linked(_) => "Create linked mesh",
            Form::Preferences(_) => "Preferences",
            Form::Gif(_) => "Export animation",
        }
    }

    /// One or two lines under the title: what this popup does and to what.
    fn about(&self) -> String {
        match &self.form {
            Form::Mesh(f) => crate::tf!("{} / {}: the image bends along this mesh. The result shows in the viewport as you change values.", f.slot.name, f.attachment.name()),
            Form::Weights(f) => format!("{} / {}: {}", f.slot, f.attachment, tr(f.action.about())),
            Form::Event(_) => tr("A named moment, such as a footstep, that animations can fire. The values are defaults; each key can override them.").to_string(),
            Form::Ik(_) => tr("Makes a bone reach for a target bone.").to_string(),
            Form::Physics(_) => tr("Makes a bone lag behind and sway on a spring.").to_string(),
            Form::Transform(_) => tr("Makes a bone follow part of another bone's movement.").to_string(),
            Form::Gif(_) => tr("Writes the animation as a looping GIF or a sprite sheet, with the previewed skins and the animations playing together.").to_string(),
            _ => String::new(),
        }
    }

    fn validation(&self, app: &EditorApp) -> Option<String> {
        let doc = &app.doc;
        match &self.form {
            Form::Animation(f) => name_error(&f.name, doc.animations.contains_key(f.name.trim())),
            Form::Skin(f) => name_error(&f.name, doc.skin(f.name.trim()).is_some()),
            Form::Event(f) => name_error(&f.name, doc.events.contains_key(f.name.trim())),
            Form::Ik(f) => name_error(&f.name, doc.has_constraint(f.name.trim()))
                .or_else(|| if f.target.is_none() { name_error(&f.new_target, doc.bone(f.new_target.trim()).is_some()) } else { None })
                .or_else(|| f.ops(app).and_then(|ops| skel2d_core::apply(doc, &ops).map_err(|e| e.to_string())).err()),
            Form::Physics(f) => (if f.per_bone && f.chain(doc).len() > 1 { None } else { name_error(&f.name, doc.has_constraint(f.name.trim())) })
                .or_else(|| f.ops(app).and_then(|ops| skel2d_core::apply(doc, &ops).map_err(|e| e.to_string())).err()),
            Form::Transform(f) => name_error(&f.name, doc.has_constraint(f.name.trim()))
                .or_else(|| f.ops(app).and_then(|ops| skel2d_core::apply(doc, &ops).map_err(|e| e.to_string())).err()),
            Form::Rename(f) => name_error(&f.name, f.name.trim() != f.item.name() && f.item.exists(doc, f.name.trim()))
                .or_else(|| skel2d_core::apply(doc, &[f.op()]).err().map(|e| e.to_string())),
            Form::Mesh(f) => f.error.clone(),
            Form::Weights(f) => f.error.clone(),
            Form::Gif(f) => f.error(app),
            Form::Linked(f) => name_error(&f.name, false).or_else(|| skel2d_core::apply(doc, &[f.op()]).err().map(|e| e.to_string())),
            _ => None,
        }
    }

    fn content(&mut self, app: &EditorApp, ui: &mut egui::Ui) {
        match &mut self.form {
            Form::Animation(f) => {
                name_input(ui, &mut f.name, self.first);
                number(ui, "Duration (seconds)", &mut f.duration, 0.0..=3600.0);
                number(ui, "FPS", &mut f.fps, 1.0..=240.0);
                ui.add_enabled_ui(f.source.is_some(), |ui| { ui.checkbox(&mut f.copy, tr("Duplicate current animation")); });
                if let Some(source) = &f.source { theme::hint(ui, &crate::tf!("Source: {}", source)); }
            }
            Form::Event(f) => {
                name_input(ui, &mut f.name, self.first);
                theme::field(ui, tr("Whole number"), tr("Default whole-number value of the event"), |ui| { ui.add_sized([96.0, 28.0], egui::DragValue::new(&mut f.int)); });
                number_tip(ui, "Decimal number", "Default decimal value of the event", &mut f.float, -1_000_000.0..=1_000_000.0);
                theme::field(ui, tr("Text"), tr("Default text value of the event"), |ui| { ui.add(egui::TextEdit::singleline(&mut f.string).desired_width(ui.available_width())); });
                theme::hint(ui, tr("Fire it from an animation with Key at playhead in the Events group."));
            }
            Form::Skin(f) => {
                name_input(ui, &mut f.name, self.first);
                ui.add_enabled_ui(!app.skins.is_empty(), |ui| { ui.checkbox(&mut f.copy, tr("Copy previewed skins")); });
                theme::hint(ui, tr("Later previewed skins win for matching attachments."));
            }
            Form::Ik(f) => {
                ui.label(tr("IK constraint"));
                name_input(ui, &mut f.name, self.first);
                bone_combo(ui, "Bone", &app.doc, &mut f.bone);
                ui.checkbox(&mut f.include_parent, tr("Include parent bone"));
                ui.label(tr("Target"));
                egui::ComboBox::from_id_salt("new_ik_target").selected_text(f.target.as_deref().unwrap_or(tr("Create target bone"))).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.target, None, tr("Create target bone"));
                    for b in &app.doc.bones { ui.selectable_value(&mut f.target, Some(b.name.clone()), &b.name); }
                });
                if f.target.is_none() {
                    ui.label(tr("Target bone name"));
                    ui.text_edit_singleline(&mut f.new_target);
                }
            }
            Form::Physics(f) => {
                ui.label(tr("Physics constraint"));
                let chain = f.chain(&app.doc);
                if !(f.per_bone && chain.len() > 1) { name_input(ui, &mut f.name, self.first); }
                bone_combo(ui, "Bone", &app.doc, &mut f.bone);
                if chain.len() > 1 {
                    ui.checkbox(&mut f.per_bone, tr("Create one per bone")).on_hover_text(tr("Makes a constraint for every bone of the chain below the selected bone (one action for the whole chain)"));
                    if f.per_bone { theme::hint(ui, &crate::tf!("{} constraints, named after their bones", chain.len())); }
                }
                theme::hint(ui, tr("Starts with rotation only and a little damping so it settles. Tune it in the inspector."));
            }
            Form::Transform(f) => {
                ui.label(tr("Copy constraint"));
                name_input(ui, &mut f.name, self.first);
                bone_combo(ui, "Bone", &app.doc, &mut f.bone);
                bone_combo(ui, "Source", &app.doc, &mut f.source);
                theme::hint(ui, tr("Starts by following the source's movement and rotation from where the bone is now. Tune it in the inspector."));
            }
            Form::Rename(f) => {
                theme::hint(ui, &crate::tf!("Renaming {}", f.item.name()));
                name_input(ui, &mut f.name, self.first);
                if matches!(f.item, Item::Attachment { .. }) {
                    ui.checkbox(&mut f.all_skins, tr("Rename matching placeholders in all skins"));
                }
                ui.label(reference_summary(&app.doc, &f.item));
            }
            Form::Delete(f) => {
                ui.label(crate::tf!("Delete {}?", f.item.name()));
                ui.label(f.impact.summary());
                if has_linked_children(&app.doc, &f.item) {
                    ui.label(tr("Linked children"));
                    egui::ComboBox::from_id_salt("on_parent_removed").selected_text(match f.on_parent_removed {
                        skel2d_core::ops::ParentRemoval::Error => tr("Cancel deletion"),
                        skel2d_core::ops::ParentRemoval::Freeze => tr("Keep independent meshes"),
                        skel2d_core::ops::ParentRemoval::Delete => tr("Delete linked children"),
                    }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut f.on_parent_removed, skel2d_core::ops::ParentRemoval::Freeze, tr("Keep independent meshes"));
                        ui.selectable_value(&mut f.on_parent_removed, skel2d_core::ops::ParentRemoval::Delete, tr("Delete linked children"));
                    });
                }
                theme::hint(ui, tr("References are updated together. Ctrl+Z restores this change."));
            }
            Form::Linked(f) => {
                theme::hint(ui, &crate::tf!("Parent: {} / {}", f.slot, f.parent));
                name_input(ui, &mut f.name, self.first);
                ui.label(tr("Target skin"));
                egui::ComboBox::from_id_salt("linked_target_skin").selected_text(f.skin.as_deref().unwrap_or("default")).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.skin, None, "default");
                    for skin in &app.doc.skins { ui.selectable_value(&mut f.skin, Some(skin.name.clone()), &skin.name); }
                });
                ui.label(tr("Image override (optional)"));
                ui.text_edit_singleline(&mut f.image);
                ui.checkbox(&mut f.timelines, tr("Share parent deform keys"));
                theme::hint(ui, tr("Geometry and weights follow the parent mesh."));
            }
            Form::Mesh(f) => {
                if matches!(f.attachment, Attachment::Region(_)) {
                    if theme::choice_card(ui, !f.rectangle, tr("Follow the image outline"), tr("Fits the visible shape. Best for most parts.")) { f.rectangle = false; }
                    if theme::choice_card(ui, f.rectangle, tr("Rectangle grid"), tr("Even cells over the whole image. Good for cloth and ribbons.")) { f.rectangle = true; }
                    ui.add_space(6.0);
                }
                if f.rectangle {
                    theme::field(ui, tr("Columns"), "", |ui| { ui.add_sized([96.0, 28.0], egui::DragValue::new(&mut f.cols).range(1..=64)); });
                    theme::field(ui, tr("Rows"), "", |ui| { ui.add_sized([96.0, 28.0], egui::DragValue::new(&mut f.rows).range(1..=64)); });
                } else {
                    theme::field(ui, tr("Density"), tr("More points bend more smoothly but take longer to weight."), |ui| {
                        if theme::segmented(ui, &mut f.density, &[(Density::Low, tr("Low")), (Density::Medium, tr("Medium")), (Density::High, tr("High"))]) {
                            let preset = skel2d_rig::TraceParams::preset(f.density);
                            f.params.detail = preset.detail;
                            f.params.spacing = preset.spacing;
                        }
                    });
                    ui.checkbox(&mut f.params.keep_weights, tr("Keep weights")).on_hover_text(tr("Points near the old ones take over their weights."));
                }
                ui.checkbox(&mut f.auto_weights, tr("Generate weights automatically")).on_hover_text(tr("Nearby bones get to move the new points right away."));
                ui.add_space(8.0);
                if !f.summary.is_empty() { theme::summary_banner(ui, &f.summary); }
                if !f.rectangle {
                    theme::group(ui, "mesh_advanced", tr("Fine tuning"), "", false, |ui| {
                        number_tip(ui, "Alpha threshold", "How opaque a pixel must be to count as part of the image", &mut f.params.alpha_threshold, 0.0..=1.0);
                        number_tip(ui, "Padding (pixels)", "Room between the image and the outline", &mut f.params.padding, 0.0..=64.0);
                        number_tip(ui, "Detail", "How closely the outline follows the image", &mut f.params.detail, 0.0..=1.0);
                        theme::field(ui, tr("Interior vertices"), tr("Points inside the outline"), |ui| {
                            theme::segmented(ui, &mut f.params.interior, &[(skel2d_rig::Interior::None, tr("None")), (skel2d_rig::Interior::Grid, tr("Grid")), (skel2d_rig::Interior::Auto, tr("Auto"))]);
                        });
                        optional_number(ui, "Spacing (pixels)", &mut f.params.spacing, 2.0..=512.0, 16.0);
                        optional_number(ui, "Concavity tolerance (pixels)", &mut f.params.concavity, 0.0..=64.0, 1.0);
                    });
                }
                ui.add_space(4.0);
                theme::hint(ui, tr("Preview updates in the viewport. Cancel restores the original mesh."));
            }
            Form::Preferences(f) => f.ui(ui),
            Form::Gif(f) => f.ui(app, ui),
            Form::Weights(f) => f.ui(ui),
        }
    }

    fn ops(&self, app: &EditorApp) -> Result<Vec<Op>, String> {
        Ok(match &self.form {
            Form::Animation(f) => {
                let name = f.name.trim().to_string();
                if f.copy {
                    vec![Op::DuplicateAnimation { name: f.source.clone().ok_or_else(|| tr("No source animation").to_string())?, new_name: name.clone() },
                         Op::SetAnimation { name, rename: None, duration: Some(Some(f.duration)) }]
                } else { vec![Op::AddAnimation { name, duration: Some(f.duration) }] }
            }
            Form::Event(f) => vec![Op::AddEvent { name: f.name.trim().to_string(), int: f.int, float: f.float, string: f.string.clone() }],
            Form::Skin(f) => skin_ops(&app.doc, f.name.trim(), if f.copy { &app.skins } else { &[] }),
            Form::Ik(f) => f.ops(app)?,
            Form::Physics(f) => f.ops(app)?,
            Form::Transform(f) => f.ops(app)?,
            Form::Rename(f) => vec![f.op()],
            Form::Delete(f) => vec![f.op()],
            Form::Linked(f) => {
                let mut ops = vec![f.op()];
                if app.doc.slot(&f.slot).and_then(|s| s.attachment.as_deref()) != Some(f.name.trim()) {
                    ops.push(Op::SetSlot(SlotPatch { name: f.slot.clone(), attachment: Some(Some(f.name.trim().into())), ..Default::default() }));
                }
                ops
            },
            Form::Mesh(f) => f.expanded.clone(),
            Form::Weights(f) => f.expanded.clone(),
            Form::Preferences(_) | Form::Gif(_) => vec![],
        })
    }

    fn ui(&mut self, app: &mut EditorApp, ui: &mut egui::Ui) -> DialogResult {
        ui.set_width(theme::DIALOG_WIDTH.min(ui.ctx().content_rect().width() - theme::DIALOG_GAP));
        let about = self.about();
        theme::dialog_title(ui, tr(self.title()), &about);
        egui::ScrollArea::vertical().max_height(theme::DIALOG_HEIGHT.min(ui.ctx().content_rect().height() * 0.65)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.content(app, ui)
        });
        if let Form::Mesh(f) = &mut self.form && f.refresh(app) { ui.ctx().request_repaint(); }
        if let Form::Weights(f) = &mut self.form { f.refresh(app); }
        self.first = false;
        let invalid = self.validation(app);
        if invalid.is_some() || !self.error.is_empty() { ui.add_space(8.0); }
        if let Some(error) = &invalid { theme::error_banner(ui, error); }
        if !self.error.is_empty() { theme::error_banner(ui, &self.error); }
        ui.add_space(theme::DIALOG_GAP);
        let mut close = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut apply = false;
        let destructive = matches!(self.form, Form::Delete(_));
        let label = if destructive { "Delete" } else if matches!(self.form, Form::Gif(_)) { "Export" } else if matches!(self.form, Form::Preferences(_) | Form::Rename(_) | Form::Mesh(_) | Form::Weights(_)) { "Apply" } else { "Create" };
        theme::dialog_buttons(ui, |ui| {
            let emphasis = if destructive { theme::Emphasis::Danger } else { theme::Emphasis::Primary };
            apply = ui.add_enabled(invalid.is_none(), theme::dialog_button(tr(label), emphasis)).on_hover_text("Enter").clicked();
            close |= ui.add(theme::dialog_button(tr("Cancel"), theme::Emphasis::Plain)).on_hover_text("Esc").clicked();
        });
        apply |= invalid.is_none() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        if close { return DialogResult::Close; }
        if apply {
            match self.ops(app) {
                Ok(ops) => return DialogResult::Apply(ops),
                Err(e) => self.error = e,
            }
        }
        DialogResult::Keep
    }

    fn applied(&self, app: &mut EditorApp) {
        match &self.form {
            Form::Animation(f) => {
                app.settings.fps = f.fps;
                app.playback.anim = Some(f.name.trim().to_string());
                app.playback.time = 0.0;
                app.timeline.clear_selection();
            }
            Form::Event(f) => app.timeline.event = Some(f.name.trim().to_string()),
            Form::Skin(f) => {
                let name = f.name.trim().to_string();
                app.skins.push(name.clone());
                app.selection = Selection { skin: Some(name), ..Default::default() };
            }
            Form::Physics(f) => {
                // The batch is in the document by now, so find its first constraint by bone (the newest on it).
                let first = if f.per_bone && f.chain(&app.doc).len() > 1 {
                    f.chain(&app.doc).first().and_then(|b| app.doc.physics.iter().filter(|c| &c.bone == b).max_by_key(|c| c.order)).map(|c| c.name.clone())
                } else { Some(f.name.trim().to_string()) };
                app.selection = Selection { physics: first, ..Default::default() };
            }
            Form::Transform(f) => {
                app.selection = Selection { transform: Some(f.name.trim().to_string()), ..Default::default() };
            }
            Form::Ik(f) => {
                app.selection = Selection { ik: Some(f.name.trim().to_string()), ..Default::default() };
                // The modal's context is supplied by the caller when opening the tab.
            }
            Form::Rename(f) => update_view_name(app, &f.item, f.name.trim()),
            Form::Delete(f) => app.status = crate::tf!("Deleted {} · Ctrl+Z to undo", f.item.name()),
            Form::Preferences(f) => { app.settings = f.settings.clone(); }
            Form::Gif(f) => crate::export::start(app, f.clone()),
            Form::Mesh(_) => {}
            Form::Weights(_) => { app.status=tr("Weights updated · Ctrl+Z to undo").into(); }
            Form::Linked(f) => {
                app.selection.slot = Some(f.slot.clone());
                app.selection.attachment = Some(f.name.trim().into());
                if let Some(skin) = &f.skin { app.skins.retain(|k| k != skin); app.skins.push(skin.clone()); }
            }
        }
    }
}

/// Return true to retain the form for the next frame (including apply errors).
pub fn show(app: &mut EditorApp, ctx: &egui::Context, dialog: &mut EditorDialog) -> bool {
    let response = theme::modal("editor_dialog").show(ctx, |ui| dialog.ui(app, ui));
    match response.inner {
        DialogResult::Keep => true,
        DialogResult::Close => {
            if let Form::Mesh(f) = &dialog.form { app.doc = f.base.clone(); }
            if let Form::Weights(f) = &dialog.form { app.doc = f.base.clone(); }
            false
        },
        DialogResult::Apply(ops) => {
            // apply() cleans up names which disappeared. Renames need the old
            // view references to retarget after that successful document edit.
            let view = (app.selection.clone(), app.playback.anim.clone(), app.playback.last_anim.clone(), app.playback.layers.clone(), app.skins.clone());
            if let Form::Mesh(f) = &dialog.form { app.doc = f.base.clone(); }
            if let Form::Weights(f) = &dialog.form { app.doc = f.base.clone(); }
            if app.edit(ops) {
                if matches!(dialog.form, Form::Rename(_)) {
                    (app.selection, app.playback.anim, app.playback.last_anim, app.playback.layers, app.skins) = view;
                }
                dialog.applied(app);
                false
            } else {
                dialog.error = app.status.clone();
                true
            }
        }
    }
}

fn bone_combo(ui: &mut egui::Ui, label: &str, doc: &Document, value: &mut String) {
    theme::field(ui, tr(label), "", |ui| {
        egui::ComboBox::from_id_salt(label).width(240.0).selected_text(value.as_str()).show_ui(ui, |ui| {
            for bone in &doc.bones { ui.selectable_value(value, bone.name.clone(), &bone.name); }
        });
    });
}

impl NewIk {
    fn ops(&self, app: &EditorApp) -> Result<Vec<Op>, String> {
        let i = app.doc.bone_index(&self.bone).ok_or_else(|| tr("Select a bone").to_string())?;
        let b = &app.doc.bones[i];
        let bones = if self.include_parent {
            b.parent.iter().cloned().chain([self.bone.clone()]).collect()
        } else { vec![self.bone.clone()] };
        let mut ops = Vec::new();
        let target = match &self.target {
            Some(target) => target.clone(),
            None => {
                let pose = crate::import::setup_pose(&app.doc);
                let [x, y] = pose.bones[i].world.apply(b.length, 0.0);
                let mut target = Bone::new(self.new_target.trim());
                (target.x, target.y) = (x, y);
                let name = target.name.clone();
                ops.push(Op::AddBone(target));
                name
            }
        };
        let bend_positive = crate::ikbend::initial_bend(&app.doc, &crate::import::setup_pose(&app.doc), &bones);
        ops.push(Op::AddIk(IkConstraint {
            name: self.name.trim().to_string(), bones, target, bend_positive, mix: 1.0, softness: 0.0, compress: false, stretch: false,
            order: app.doc.ik.iter().map(|c| c.order).max().unwrap_or(-1).saturating_add(1),
        }));
        Ok(ops)
    }
}

impl NewTransform {
    /// Offsets keep the bone where it is now, so adding the constraint moves nothing.
    fn ops(&self, app: &EditorApp) -> Result<Vec<Op>, String> {
        let doc = &app.doc;
        let (Some(bone), Some(source)) = (doc.bone_index(&self.bone), doc.bone_index(&self.source)) else { return Err(tr("Select a bone").to_string()); };
        let pose = crate::import::setup_pose(doc);
        let (b, s) = (&pose.bones[bone].world, &pose.bones[source].world);
        let mut c = TransformConstraint::new(self.name.trim(), vec![self.bone.clone()], self.source.clone());
        (c.mix_scale_x, c.mix_scale_y, c.mix_shear_y) = (0.0, 0.0, 0.0);
        if let Some(inverse) = s.inverse() {
            [c.offset_x, c.offset_y] = inverse.apply(b.tx, b.ty);
        }
        c.offset_rotation = b.rotation_deg() - s.rotation_deg();
        c.order = doc.ik.iter().map(|c| c.order).chain(doc.physics.iter().map(|c| c.order)).chain(doc.transforms.iter().map(|c| c.order)).max().unwrap_or(-1).saturating_add(1);
        Ok(vec![Op::AddTransform(c)])
    }
}

impl NewPhysics {
    /// The bone and its descendants as long as each has exactly one child.
    fn chain(&self, doc: &Document) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = doc.bone(&self.bone).map(|b| b.name.clone());
        while let Some(name) = cur {
            let children: Vec<_> = doc.bones.iter().filter(|b| b.parent.as_deref() == Some(name.as_str())).collect();
            cur = (children.len() == 1).then(|| children[0].name.clone());
            chain.push(name);
        }
        chain
    }

    /// Names of the constraints this form creates.
    fn names(&self, doc: &Document) -> Vec<String> {
        let chain = self.chain(doc);
        if !(self.per_bone && chain.len() > 1) { return vec![self.name.trim().to_string()]; }
        let mut taken: Vec<String> = Vec::new();
        for bone in chain {
            let name = unique(&format!("{bone}_physics"), |n| doc.has_constraint(n) || taken.iter().any(|t| t == n));
            taken.push(name);
        }
        taken
    }

    fn ops(&self, app: &EditorApp) -> Result<Vec<Op>, String> {
        let doc = &app.doc;
        if doc.bone(&self.bone).is_none() { return Err(tr("Select a bone").to_string()); }
        let chain = self.chain(doc);
        let bones = if self.per_bone && chain.len() > 1 { chain } else { vec![self.bone.clone()] };
        let order = doc.ik.iter().map(|c| c.order).chain(doc.physics.iter().map(|c| c.order)).chain(doc.transforms.iter().map(|c| c.order)).max().unwrap_or(-1);
        Ok(self.names(doc).into_iter().zip(bones).enumerate().map(|(i, (name, bone))| {
            let mut c = PhysicsConstraint::new(name, bone);
            c.rotate = 1.0;
            c.damping = 0.9;
            c.order = order.saturating_add(1 + i as i32);
            Op::AddPhysics(c)
        }).collect())
    }
}

fn skin_ops(doc: &Document, name: &str, previews: &[String]) -> Vec<Op> {
    let mut attachments = BTreeMap::new();
    let (mut bones, mut ik) = (BTreeSet::new(), BTreeSet::new());
    for skin in previews.iter().filter_map(|n| doc.skin(n)) {
        bones.extend(skin.bones.iter().cloned());
        ik.extend(skin.constraints.iter().cloned());
        for (slot, values) in &skin.attachments {
            for att in values { attachments.insert((slot.clone(), att.name().to_string()), att.clone()); }
        }
    }
    let mut ops = vec![Op::AddSkin { name: name.to_string() }];
    if !previews.is_empty() {
        ops.push(Op::SetSkin { name: name.to_string(), rename: None, bones: Some(bones.into_iter().collect()), ik: None, constraints: Some(ik.into_iter().collect()) });
        ops.extend(attachments.into_iter().map(|((slot, _), attachment)| Op::SetSkinAttachment { skin: name.to_string(), slot, attachment }));
    }
    ops
}

fn optional_number(ui: &mut egui::Ui, label: &str, value: &mut Option<f32>, range: std::ops::RangeInclusive<f32>, default: f32) {
    let mut enabled = value.is_some();
    theme::field(ui, tr(label), "", |ui| {
        let text = if enabled { "" } else { tr("Automatic") };
        if ui.checkbox(&mut enabled, text).changed() { *value = enabled.then_some(default); }
        if let Some(value) = value { ui.add_sized([96.0, 28.0], egui::DragValue::new(value).range(range).speed(0.1)); }
    });
}

impl WeightOptions {
    fn op(&self) -> Result<Op,String> {
        let (slot,attachment,skin,vertices,locked_bones)=(self.slot.clone(),self.attachment.clone(),self.skin.clone(),self.vertices.clone(),self.locked_bones.clone());
        Ok(match self.action {
            WeightAction::Prune=>Op::PruneWeights {slot,attachment,skin,vertices,locked_bones,max_bones:self.max_bones,threshold:self.threshold},
            WeightAction::Mirror=>Op::MirrorWeights {slot,attachment,skin,locked_bones,axis:self.axis,source:self.source},
            WeightAction::Transfer=> {
                let (from_slot,from_attachment,from_skin)=self.from.clone().ok_or_else(||tr("Select a source mesh").to_string())?;
                Op::TransferWeights {slot,attachment,skin,vertices,locked_bones,from_slot,from_attachment,from_skin}
            }
            WeightAction::Bind=>Op::BindBones {slot,attachment,skin,add:vec![],remove:vec![self.remove.clone()],redistribute:self.redistribute},
            WeightAction::BindAdd=>Op::BindBones {slot,attachment,skin,add:vec![self.remove.clone()],remove:vec![],redistribute:self.redistribute},
            WeightAction::Swap=>Op::SwapWeights {slot,attachment,skin,vertices,locked_bones,a:self.remove.clone(),b:self.other.clone()},
            WeightAction::Set=>Op::SetBoneWeight {slot,attachment,skin,vertices:vertices.unwrap_or_else(|| {
                let Some(Attachment::Mesh(m))=self.base.resolve_attachment(self.base.slot_index(&self.slot).unwrap(),&self.attachment,&self.skin.iter().cloned().collect::<Vec<_>>()) else {return vec![]};
                (0..m.vertices.len()).collect()
            }),locked_bones,bone:self.remove.clone(),value:self.value,mode:self.mode},
            WeightAction::Smooth=>Op::SmoothWeights {slot,attachment,skin,vertices,locked_bones,factor:0.5,iterations:1},
            WeightAction::Normalize=>Op::NormalizeWeights {slot,attachment,skin},
            WeightAction::Distance=>Op::AutoWeights {slot,attachment,skin,vertices,locked_bones,bones:None,max_influences:self.max_bones,falloff:3.0},
        })
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        use skel2d_core::ops::{MirrorSide,Redistribute,WeightMode};
        match self.action {
            WeightAction::Prune=> {
                count_field(ui,"Bones per point (max)","Each point keeps this many of its strongest bones",&mut self.max_bones,1..=8);
                number_tip(ui,"Remove below","Influences smaller than this are removed (0.05 = 5%)",&mut self.threshold,0.0..=1.0);
            }
            WeightAction::Mirror=> {
                number_tip(ui,"Mirror line X","The vertical line both halves mirror across",&mut self.axis,-100000.0..=100000.0);
                ui.add_space(4.0);
                if theme::choice_card(ui,self.source==MirrorSide::Positive,tr("Right half → left"),tr("The right side is correct")) {self.source=MirrorSide::Positive;}
                if theme::choice_card(ui,self.source==MirrorSide::Negative,tr("Left half → right"),tr("The left side is correct")) {self.source=MirrorSide::Negative;}
            }
            WeightAction::Transfer=> {
                theme::field(ui,tr("Copy from"),"",|ui| {
                    egui::ComboBox::from_id_salt("weight_source").width(280.0).selected_text(self.from.as_ref().map(|(s,a,k)|format!("{} / {s} / {a}",k.as_deref().unwrap_or("default"))).unwrap_or_else(||tr("Select a source mesh").into())).show_ui(ui,|ui| {
                        for slot in &self.base.slots {
                            for att in &slot.attachments {
                                if matches!(att,Attachment::Mesh(_)) {ui.selectable_value(&mut self.from,Some((slot.name.clone(),att.name().into(),None)),format!("default / {} / {}",slot.name,att.name()));}
                            }
                        }
                        for skin in &self.base.skins {
                            for (slot,atts) in &skin.attachments {
                                for att in atts {
                                    if matches!(att,Attachment::Mesh(_)) {ui.selectable_value(&mut self.from,Some((slot.clone(),att.name().into(),Some(skin.name.clone()))),format!("{} / {} / {}",skin.name,slot,att.name()));}
                                }
                            }
                        }
                    });
                });
            }
            WeightAction::Bind|WeightAction::BindAdd=> {
                bone_combo(ui,"Bone",&self.base,&mut self.remove);
                theme::field(ui,tr("Redistribute"),tr("Who takes over the weight"),|ui| {
                    theme::segmented(ui,&mut self.redistribute,&[(Redistribute::Auto,tr("Auto")),(Redistribute::Nearest,tr("Nearest")),(Redistribute::Parent,tr("Parent"))]);
                });
            }
            WeightAction::Swap=> {
                bone_combo(ui,"Bone",&self.base,&mut self.remove);
                bone_combo(ui,"Swap with",&self.base,&mut self.other);
            }
            WeightAction::Set=> {
                bone_combo(ui,"Bone",&self.base,&mut self.remove);
                number_tip(ui,"Weight value","1 = 100%",&mut self.value,-1.0..=10.0);
                theme::field(ui,tr("How"),"",|ui| {
                    theme::segmented(ui,&mut self.mode,&[(WeightMode::Set,tr("Set")),(WeightMode::Add,tr("Add")),(WeightMode::Scale,tr("Scale"))]);
                });
            }
            WeightAction::Distance=> count_field(ui,"Bones per point (max)","Each point keeps this many of its strongest bones",&mut self.max_bones,1..=8),
            WeightAction::Smooth|WeightAction::Normalize=> {}
        }
        ui.add_space(8.0);
        if !self.summary.is_empty() {theme::summary_banner(ui,&self.summary);}
        ui.add_space(4.0);
        theme::hint(ui,tr("Preview updates in the viewport. Cancel restores the original mesh."));
    }

    fn refresh(&mut self, app: &mut EditorApp) {
        let result=self.op().and_then(|op|skel2d_rig::apply_any(&self.base,&app.base_dir(),&[op.into()]).map_err(|e|e.to_string()));
        match result {
            Ok(report)=> {
                let si=self.base.slot_index(&self.slot).unwrap();
                let get=|doc:&Document|doc.resolve_attachment(si,&self.attachment,&self.skin.iter().cloned().collect::<Vec<_>>()).cloned();
                if let (Some(Attachment::Mesh(before)),Some(Attachment::Mesh(after)))=(get(&self.base),get(&report.document)) {
                    let bone=&self.base.slots[si].bone;
                    let changed=before.vertices.iter().zip(&after.vertices).filter(|(a,b)|a.weights!=b.weights).count();
                    let removed=before.vertices.iter().zip(&after.vertices).map(|(a,b)|{
                        let next=skel2d_core::mesh::weight_map(b,bone);
                        skel2d_core::mesh::weight_map(a,bone).keys().filter(|k|!next.contains_key(*k)).count()
                    }).sum::<usize>();
                    self.summary=crate::tf!("{} vertices changed · {} influences removed",changed,removed);
                }
                self.expanded=report.ops; self.error=None; app.doc=report.document;
            }
            Err(error)=> {self.error=Some(error);self.expanded.clear();self.summary.clear();app.doc=self.base.clone();}
        }
    }
}

impl MeshOptions {
    fn requests(&self, app: &EditorApp) -> Result<Vec<skel2d_rig::AnyOp>, String> {
        use skel2d_rig::{AnyOp, AssetOp};
        let mut ops = if self.rectangle {
            let Attachment::Region(region) = &self.attachment else { return Err(tr("Mesh does not exist").into()) };
            // Use the previewed region. SetAttachment then targets its skin.
            let next = skel2d_core::apply(&self.base, &[
                Op::SetAttachment { slot: self.slot.name.clone(), attachment: self.attachment.clone() },
                Op::MeshFromRegion { slot: self.slot.name.clone(), region: region.name.clone(), name: None, cols: self.cols, rows: self.rows },
            ]).map_err(|e| e.to_string())?;
            let attachment = next.slot(&self.slot.name).and_then(|s| s.attachment(region.name.as_str())).cloned().ok_or_else(|| tr("Mesh does not exist").to_string())?;
            vec![AnyOp::Core(match &self.skin {
                Some(skin) => Op::SetSkinAttachment { skin: skin.clone(), slot: self.slot.name.clone(), attachment },
                None => Op::SetAttachment { slot: self.slot.name.clone(), attachment },
            })]
        } else {
            let mut op = crate::import::trace_mesh(app, &self.slot.name, self.attachment.name(), self.params.clone());
            if let AssetOp::TraceMesh { skin, .. } = &mut op { *skin = self.skin.clone(); }
            vec![AnyOp::Asset(op)]
        };
        if self.auto_weights {
            ops.push(AnyOp::Asset(AssetOp::ImageWeights { locked_bones: app.view.weight_locked.iter().cloned().collect(), slot: self.slot.name.clone(), attachment: self.attachment.name().into(), skin: self.skin.clone(), bones: None, max_influences: 4, falloff: 3.0, vertices: None }));
        }
        Ok(ops)
    }

    fn refresh(&mut self, app: &mut EditorApp) -> bool {
        let result = self.requests(app);
        let signature = match &result {
            Ok(ops) => serde_json::to_string(ops).expect("finite UI parameters"),
            Err(e) => e.clone(),
        };
        if signature == self.signature { return false; }
        self.signature = signature;
        // Parameter changes preview immediately; there is no Trace button.
        match result.and_then(|ops| skel2d_rig::apply_any(&self.base, &app.base_dir(), &ops).map_err(|e|e.to_string())) {
            Ok(report) => {
                self.summary = report.meshes.last().map(|m|crate::tf!("{} vertices · {} triangles",m.vertices,m.triangles)).unwrap_or_default();
                self.expanded = report.ops;
                self.error = None;
                app.preview(&self.base, &self.expanded);
            }
            Err(error) => {
                self.expanded.clear();
                self.summary.clear();
                self.error = Some(error);
                app.doc = self.base.clone();
            }
        }
        true
    }
}

fn update_view_name(app: &mut EditorApp, item: &Item, new: &str) {
    let old = item.name();
    let update = |v: &mut Option<String>| { if v.as_deref() == Some(old) { *v = Some(new.to_string()); } };
    match item {
        Item::Attachment { .. } => { update(&mut app.selection.attachment); app.timeline.clear_selection(); }
        Item::Bone(_) => { update(&mut app.selection.bone); replace_flag(&mut app.hidden_bones, old, new); replace_flag(&mut app.locked_bones, old, new); replace_flag(&mut app.view.weight_locked,old,new); }
        Item::Slot(_) => { update(&mut app.selection.slot); replace_flag(&mut app.hidden_slots, old, new); replace_flag(&mut app.locked_slots, old, new); }
        Item::Ik(_) => update(&mut app.selection.ik),
        Item::Physics(_) => update(&mut app.selection.physics),
        Item::Transform(_) => update(&mut app.selection.transform),
        Item::Event(_) => update(&mut app.timeline.event),
        Item::Skin(_) => { update(&mut app.selection.skin); for n in &mut app.skins { if n == old { *n = new.to_string(); } } }
        Item::Animation(_) => {
            update(&mut app.playback.anim); update(&mut app.playback.last_anim);
            for layer in &mut app.playback.layers { if layer.anim == old { layer.anim = new.to_string(); } }
        }
    }
}

fn replace_flag(flags: &mut BTreeSet<String>, old: &str, new: &str) {
    if flags.remove(old) { flags.insert(new.to_string()); }
}

fn key_count(animation: &Animation) -> usize {
    animation.bones.values().map(|t| t.rotate.len() + t.translate.len() + t.scale.len() + t.shear.len()).sum::<usize>()
        + animation.slots.values().map(|t| t.color.len() + t.attachment.len()).sum::<usize>()
        + animation.deform.values().flat_map(|s| s.values()).flat_map(|a| a.values()).map(Vec::len).sum::<usize>()
        + animation.ik.values().map(Vec::len).sum::<usize>()
        + animation.draw_order.len()
        + animation.events.len()
        + animation.physics.values().map(physics_key_count).sum::<usize>()
}

fn physics_key_count(t: &skel2d_core::PhysicsTimelines) -> usize {
    t.property_keys().iter().map(|(_, k)| k.len()).sum::<usize>() + t.reset.len()
}

#[derive(Default, Clone, Debug)]
struct Impact {
    bones: usize,
    slots: usize,
    constraints: usize,
    attachments: usize,
    keys: usize,
    weights: usize,
    skin_refs: usize,
}

impl Impact {
    fn totals(doc: &Document) -> Self {
        let attachments: Vec<_> = doc.slots.iter().flat_map(|s| &s.attachments)
            .chain(doc.skins.iter().flat_map(|s| s.attachments.values().flatten())).collect();
        Self {
            bones: doc.bones.len(), slots: doc.slots.len(), constraints: doc.ik.len() + doc.physics.len() + doc.transforms.len(), attachments: attachments.len(),
            keys: doc.animations.values().map(key_count).sum(),
            weights: attachments.iter().map(|a| if let Attachment::Mesh(m) = a { m.vertices.iter().map(|v| v.weights.len()).sum() } else { 0 }).sum(),
            skin_refs: doc.skins.iter().map(|s| s.bones.len() + s.constraints.len() + s.attachments.len()).sum(),
        }
    }

    fn needs_confirmation(&self, item: &Item) -> bool {
        self.bones > usize::from(matches!(item, Item::Bone(_))) || self.slots > usize::from(matches!(item, Item::Slot(_)))
            || self.constraints > usize::from(matches!(item, Item::Ik(_) | Item::Physics(_) | Item::Transform(_))) || self.attachments > 0 || self.keys > 0 || self.weights > 0 || self.skin_refs > 0
    }

    fn summary(&self) -> String {
        crate::tf!("{} bones · {} slots · {} constraints · {} attachments · {} keys · {} weights · {} skin references",
            self.bones, self.slots, self.constraints, self.attachments, self.keys, self.weights, self.skin_refs)
    }
}

fn deletion_impact(doc: &Document, item: &Item) -> Result<Impact, String> {
    let after = skel2d_core::apply(doc, &[with_parent_policy(item.remove_op(), skel2d_core::ops::ParentRemoval::Freeze)]).map_err(|e| e.to_string())?;
    let (a, b) = (Impact::totals(doc), Impact::totals(&after));
    Ok(Impact {
        bones: a.bones.saturating_sub(b.bones), slots: a.slots.saturating_sub(b.slots), constraints: a.constraints.saturating_sub(b.constraints),
        attachments: a.attachments.saturating_sub(b.attachments), keys: a.keys.saturating_sub(b.keys), weights: a.weights.saturating_sub(b.weights), skin_refs: a.skin_refs.saturating_sub(b.skin_refs),
    })
}

fn reference_summary(doc: &Document, item: &Item) -> String {
    let name = item.name();
    let mut impact = Impact::default();
    for a in doc.animations.values() {
        match item {
            Item::Bone(_) => if let Some(t) = a.bones.get(name) { impact.keys += t.rotate.len() + t.translate.len() + t.scale.len() + t.shear.len(); },
            Item::Slot(_) => {
                if let Some(t) = a.slots.get(name) { impact.keys += t.color.len() + t.attachment.len(); }
                impact.keys += a.deform.values().filter_map(|s| s.get(name)).flat_map(|a| a.values()).map(Vec::len).sum::<usize>();
            }
            Item::Ik(_) => impact.keys += a.ik.get(name).map_or(0, Vec::len),
            Item::Physics(_) => impact.keys += a.physics.get(name).map_or(0, physics_key_count),
            Item::Event(_) => impact.keys += a.events.iter().filter(|k| k.name == name).count(),
            _ => {}
        }
    }
    impact.skin_refs = doc.skins.iter().filter(|s| match item {
        Item::Bone(_) => s.bones.iter().any(|n| n == name), Item::Slot(_) => s.attachments.contains_key(name),
        Item::Ik(_) | Item::Physics(_) | Item::Transform(_) => s.constraints.iter().any(|n| n == name), _ => false,
    }).count();
    if matches!(item, Item::Bone(_)) {
        impact.bones = doc.bones.iter().filter(|b| b.parent.as_deref() == Some(name)).count();
        impact.slots = doc.slots.iter().filter(|s| s.bone == name).count();
        impact.constraints = doc.ik.iter().filter(|c| c.target == name || c.bones.iter().any(|b| b == name)).count() + doc.physics.iter().filter(|c| c.bone == name).count() + doc.transforms.iter().filter(|c| c.source == name || c.bones.iter().any(|b| b == name)).count();
        impact.weights = doc.slots.iter().flat_map(|s| &s.attachments).chain(doc.skins.iter().flat_map(|s| s.attachments.values().flatten()))
            .map(|a| if let Attachment::Mesh(m) = a { m.vertices.iter().flat_map(|v| &v.weights).filter(|w| w.bone == name).count() } else { 0 }).sum();
    }
    crate::tf!("References updated: {}", impact.summary())
}

impl Preferences {
    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (i, name) in ["General", "Viewport", "Snapping", "Animation", "Mesh", "Weights"].iter().enumerate() {
                ui.selectable_value(&mut self.tab, i, tr(name));
            }
        });
        ui.separator();
        let s = &mut self.settings;
        match self.tab {
            0 => {
                ui.label(tr("Language"));
                ui.horizontal(|ui| { for lang in crate::i18n::Lang::ALL { ui.selectable_value(&mut s.language, lang, lang.native_name()); } });
                number(ui, "UI size", &mut s.ui_scale, 0.8..=2.0);
            }
            1 => {
                ui.checkbox(&mut s.show_bones, tr("Show bones"));
                ui.checkbox(&mut s.show_grid, tr("Grid"));
                ui.checkbox(&mut s.show_bone_names, tr("Bone names"));
                ui.checkbox(&mut s.onion_skin, tr("Onion skin"));
                ui.horizontal(|ui| { ui.label(tr("Onion frames")); ui.add(egui::DragValue::new(&mut s.onion_frames).range(1..=6)); });
            }
            2 => {
                ui.checkbox(&mut s.snap, tr("Snap"));
                number(ui, "Snap angle", &mut s.snap_angle, 1.0..=90.0);
                number(ui, "Snap grid", &mut s.snap_grid, 0.5..=500.0);
            }
            3 => {
                number(ui, "FPS", &mut s.fps, 1.0..=240.0);
                ui.checkbox(&mut s.auto_key, tr("Auto Key"));
            }
            4 => {
                ui.checkbox(&mut s.carry_meshes, tr("Meshes follow bones in the setup pose"));
                ui.checkbox(&mut s.mesh_triangles, tr("Triangle edges"));
                ui.checkbox(&mut s.mesh_dim, tr("Dim image"));
                ui.checkbox(&mut s.mesh_isolate, tr("Isolate mesh"));
            }
            _ => {
                number(ui, "Brush radius", &mut s.brush_radius, 1.0..=500.0);
                number(ui, "Brush strength", &mut s.brush_strength, 0.0..=1.0);
                number(ui, "Target weight", &mut s.brush_target, 0.0..=1.0);
                number(ui, "Brush feather", &mut s.brush_feather, 0.0..=1.0);
                ui.horizontal(|ui| {
                    use crate::meshedit::BrushMode;
                    for (mode, label) in [(BrushMode::Add, "Add"), (BrushMode::Remove, "Remove"), (BrushMode::Replace, "Replace")] {
                        ui.selectable_value(&mut s.brush_mode, mode, tr(label));
                    }
                });
                ui.checkbox(&mut s.weights_all_bones, tr("Show all bone weights"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_mesh_form_uses_shared_op_and_history() {
        let mut app = trace_app();
        assert!(app.edit(vec![Op::AddSkin { name: "armor".into() }]));
        let original = app.doc.clone();
        new_linked_mesh(&mut app, "skin", "arm");
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Linked(form) = &mut dialog.form else { panic!() };
        assert_eq!(form.skin.as_deref(), Some("armor"));
        form.image = "other.png".into();
        let ops = dialog.ops(&app).unwrap();
        let json = serde_json::to_string(&ops).unwrap();
        let cli = skel2d_rig::apply_any(&original, &app.base_dir(), &skel2d_rig::parse_ops(&json).unwrap()).unwrap();
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert_eq!(app.doc, cli.document);
        assert!(matches!(app.doc.skin("armor").unwrap().attachment("skin", "arm"), Some(Attachment::LinkedMesh(_))));
        app.undo(); assert_eq!(app.doc, original);
        app.redo(); assert_eq!(app.doc, cli.document);
    }

    #[test]
    fn deleting_linked_parent_opens_policy_dialog() {
        let mut app = trace_app();
        assert!(app.edit(vec![Op::AddSkin { name: "armor".into() }, Op::AddLinkedMesh {
            slot: "skin".into(), name: "arm".into(), parent: "arm".into(), parent_skin: None,
            skin: Some("armor".into()), image: None, timelines: true,
        }]));
        delete(&mut app, Item::Attachment { slot:"skin".into(), name:"arm".into(), skin:None });
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Delete(form) = &dialog.form else { panic!() };
        assert_eq!(form.on_parent_removed, skel2d_core::ops::ParentRemoval::Freeze);
        let before = app.doc.clone();
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert!(matches!(app.doc.skin("armor").unwrap().attachment("skin","arm"),Some(Attachment::Mesh(_))));
        app.undo(); assert_eq!(app.doc,before);
    }

    fn trace_app() -> EditorApp {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/arm");
        let mut doc = skel2d_core::load(base.join("arm.skel.json")).unwrap();
        let Attachment::Mesh(mesh) = &mut doc.slots[0].attachments[0] else { panic!() };
        mesh.image = Some("trace.png".into());
        EditorApp::test_app(doc, base.join("arm.skel.json"))
    }

    #[test]
    fn weight_forms_preview_cancel_apply_history_and_cli_match() {
        for action in [WeightAction::Prune,WeightAction::Mirror,WeightAction::Transfer,WeightAction::Bind,WeightAction::BindAdd,WeightAction::Set,WeightAction::Swap,WeightAction::Smooth,WeightAction::Normalize,WeightAction::Distance] {
            let mut app=trace_app();
            app.selection.slot=Some("skin".into());app.selection.bone=Some("lower".into());
            let original=app.doc.clone();
            weight_options(&mut app,action);
            let Dialog::Editor(mut dialog)=std::mem::replace(&mut app.dialog,Dialog::None) else {panic!()};
            let Form::Weights(f)=&mut dialog.form else {panic!()};
            f.max_bones=1;
            if action==WeightAction::Transfer {f.from=Some(("skin".into(),"arm".into(),None));}
            if action==WeightAction::BindAdd {f.remove="goal".into();}
            f.refresh(&mut app); assert!(f.error.is_none(),"{:?}: {:?}",action,f.error);
            let json=serde_json::to_string(&f.op().unwrap()).unwrap();
            let cli=skel2d_rig::apply_any(&original,&app.base_dir(),&skel2d_rig::parse_ops(&json).unwrap()).unwrap();
            assert_eq!(skel2d_core::to_string(&app.doc),skel2d_core::to_string(&cli.document));
            let preview=app.doc.clone();
            assert!(!key_dialog(&mut app,&mut dialog,Key::Escape));assert_eq!(app.doc,original);
            assert!(!key_dialog(&mut app,&mut dialog,Key::Enter));assert_eq!(app.doc,preview);
            app.undo();assert_eq!(app.doc,original);app.redo();assert_eq!(app.doc,preview);
        }
    }

    #[test]
    fn weight_forms_reject_animate_and_restore_invalid_preview() {
        let mut app=trace_app();app.selection.slot=Some("skin".into());
        let original=app.doc.clone();app.playback.anim=Some("bend".into());
        weight_options(&mut app,WeightAction::Prune);assert!(matches!(app.dialog,Dialog::None));
        app.playback.anim=None;weight_options(&mut app,WeightAction::Prune);
        let Dialog::Editor(mut dialog)=std::mem::replace(&mut app.dialog,Dialog::None) else {panic!()};
        let Form::Weights(f)=&mut dialog.form else {panic!()};
        f.max_bones=1;f.refresh(&mut app);assert_ne!(app.doc,original);
        f.locked_bones.push("missing".into());f.refresh(&mut app);assert!(f.error.is_some());assert_eq!(app.doc,original);
        assert!(key_dialog(&mut app,&mut dialog,Key::Enter));assert_eq!(app.doc,original);
    }

    fn key_dialog(app: &mut EditorApp, dialog: &mut EditorDialog, key: Key) -> bool {
        let ctx = egui::Context::default();
        let input = egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], ..Default::default() };
        let mut keep = true;
        let mut output = ctx.run_pass(input, |ctx| { keep = show(app, ctx, dialog); });
        output.textures_delta.clear();
        keep
    }

    #[test]
    fn trace_dialog_preview_cancel_apply_and_history_match_cli_bytes() {
        let mut app = trace_app();
        let original = app.doc.clone();
        let slot = app.doc.slots[0].clone();
        attachment_mesh_options(&mut app, &slot, &slot.attachments[0]);
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Mesh(form) = &mut dialog.form else { panic!() };
        assert!(form.refresh(&mut app));
        assert_ne!(app.doc, original);
        let first = app.doc.clone();
        form.params.interior = skel2d_rig::Interior::Grid;
        form.params.spacing = Some(8.0);
        assert!(form.refresh(&mut app));
        assert_ne!(app.doc, first);
        let json = serde_json::to_string(&form.requests(&app).unwrap()).unwrap();
        let cli = skel2d_rig::apply_any(&original, &app.base_dir(), &skel2d_rig::parse_ops(&json).unwrap()).unwrap();
        assert_eq!(skel2d_core::to_string(&app.doc), skel2d_core::to_string(&cli.document));
        assert!(!key_dialog(&mut app, &mut dialog, Key::Escape));
        assert_eq!(app.doc, original);
        app.undo();
        assert_eq!(app.doc, original); // Preview and Cancel did not create history.

        attachment_mesh_options(&mut app, &slot, &slot.attachments[0]);
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Mesh(form) = &mut dialog.form else { panic!() };
        form.auto_weights = true;
        assert!(form.refresh(&mut app));
        let preview = app.doc.clone();
        // Apply uses the previewed expansion even when the image path changes.
        app.path = Some(std::env::temp_dir().join("skel2d-missing-assets/arm.skel.json"));
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert_eq!(app.doc, preview);
        app.undo(); assert_eq!(app.doc, original);
        app.redo(); assert_eq!(app.doc, preview);
    }

    #[test]
    fn trace_dialog_rejects_animate_mode_and_invalid_preview_restores_original() {
        let mut app = trace_app();
        let original = app.doc.clone();
        let slot = app.doc.slots[0].clone();
        app.playback.anim = Some("bend".into());
        attachment_mesh_options(&mut app, &slot, &slot.attachments[0]);
        assert!(matches!(app.dialog, Dialog::None));
        app.playback.anim = None;
        attachment_mesh_options(&mut app, &slot, &slot.attachments[0]);
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Mesh(form) = &mut dialog.form else { panic!() };
        form.refresh(&mut app);
        form.params.alpha_threshold = 1.0;
        form.refresh(&mut app);
        assert!(form.error.is_some()); assert!(form.expanded.is_empty()); assert_eq!(app.doc, original);
        assert!(key_dialog(&mut app, &mut dialog, Key::Enter));
        assert_eq!(app.doc, original);
    }

    #[test]
    fn attachment_rename_form_and_cli_ops_produce_identical_documents() {
        let doc = document();
        for all_skins in [false, true] {
            let form = Rename { item: Item::Attachment { slot: "slot".into(), name: "region".into(), skin: Some("base".into()) }, name: "renamed".into(), all_skins };
            let op = form.op();
            let cli = skel2d_core::parse_ops(&serde_json::to_string(&op).unwrap()).unwrap();
            let editor = skel2d_core::apply(&doc, &[op]).unwrap();
            assert_eq!(editor, skel2d_core::apply(&doc, &cli).unwrap());
            assert!(editor.skin("base").unwrap().attachment("slot", "renamed").is_some());
            assert_eq!(editor.skin("top").unwrap().attachment("slot", "renamed").is_some(), all_skins);
        }
    }

    fn document() -> Document {
        skel2d_core::apply(&Document::new("dialogs"), &skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"root"},
            {"op":"add_bone","name":"limb","parent":"root"},
            {"op":"add_bone","name":"child","parent":"limb"},
            {"op":"add_slot","name":"slot","bone":"child"},
            {"op":"key_bone","animation":"anim","bone":"child","time":0,"rotate":10},
            {"op":"add_skin","name":"base"},
            {"op":"add_skin","name":"top"},
            {"op":"set_skin","name":"base","bones":["child"]},
            {"op":"set_skin_attachment","skin":"base","slot":"slot","attachment":{"type":"region","name":"region","width":10,"height":10}},
            {"op":"set_skin_attachment","skin":"top","slot":"slot","attachment":{"type":"region","name":"region","width":20,"height":20}}
        ]"#).unwrap()).unwrap()
    }

    #[test]
    fn deletion_preview_includes_descendant_keys_and_skin_attachments() {
        let doc = document();
        // The root cannot be removed, so the preview is about a limb with a child below it.
        let impact = deletion_impact(&doc, &Item::Bone("limb".into())).unwrap();
        assert_eq!(impact.bones, 2);
        assert_eq!(impact.slots, 1);
        assert_eq!(impact.keys, 1);
        assert_eq!(impact.attachments, 2);
        assert_eq!(impact.skin_refs, 3);
        assert!(impact.needs_confirmation(&Item::Bone("limb".into())));
        assert_eq!(doc.bones.len(), 3); // Preview never mutates its input.
        let empty = skel2d_core::apply(&Document::new("empty"), &[Op::AddBone(Bone::new("solo"))]).unwrap();
        assert!(!deletion_impact(&empty, &Item::Bone("solo".into())).unwrap().needs_confirmation(&Item::Bone("solo".into())));
    }

    #[test]
    fn skin_copy_merges_preview_order_and_preserves_required_bones() {
        let doc = document();
        let copied = skel2d_core::apply(&doc, &skin_ops(&doc, "copy", &["base".into(), "top".into()])).unwrap();
        let skin = copied.skin("copy").unwrap();
        assert_eq!(skin.bones, ["child"]);
        let Some(Attachment::Region(region)) = skin.attachment("slot", "region") else { panic!("region missing") };
        assert_eq!(region.width, 20.0);
        assert_eq!(doc.skin("base").unwrap().attachment("slot", "region").unwrap().name(), "region");
    }

    fn physics_form(app: &mut EditorApp, bone: &str) -> Box<EditorDialog> {
        new_physics(app, Some(bone.into()));
        let Dialog::Editor(dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        dialog
    }

    #[test]
    fn new_transform_leaves_the_bone_where_it_is() {
        let mut app = trace_app();
        app.doc.ik.clear();
        app.doc.animations.clear();
        new_transform(&mut app, Some("lower".into()));
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Transform(form) = &mut dialog.form else { panic!() };
        form.source = "upper".into();
        let ops = dialog.ops(&app).unwrap();
        let next = skel2d_core::apply(&app.doc, &ops).unwrap();
        assert_eq!(next.transforms.len(), 1);
        let i = next.bone_index("lower").unwrap();
        let (a, b) = (skel2d_core::Pose::compute(&app.doc, None).bones[i].world, skel2d_core::Pose::compute(&next, None).bones[i].world);
        for (x, y) in [(a.tx, b.tx), (a.ty, b.ty), (a.a, b.a), (a.b, b.b)] {
            assert!((x - y).abs() < 1e-3, "{a:?} {b:?}");
        }
    }

    #[test]
    fn physics_form_ops_round_trip_json_and_match_cli() {
        let mut app = trace_app();
        let original = app.doc.clone();
        let mut dialog = physics_form(&mut app, "upper");
        let Form::Physics(form) = &dialog.form else { panic!() };
        assert_eq!(form.name, "upper_physics");
        let ops = dialog.ops(&app).unwrap();
        // One constraint after the IK constraint (order 0), starting with rotation only and damping below 1.
        let [Op::AddPhysics(c)] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!((c.order, c.x, c.y, c.rotate, c.damping), (1, 0.0, 0.0, 1.0, 0.9));
        let json = serde_json::to_string(&ops).unwrap();
        let cli = skel2d_rig::apply_any(&original, &app.base_dir(), &skel2d_rig::parse_ops(&json).unwrap()).unwrap();
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert_eq!(app.doc, cli.document);
        assert_eq!(app.selection.physics.as_deref(), Some("upper_physics"));
        app.undo();
        assert_eq!(app.doc, original);
    }

    #[test]
    fn physics_form_creates_one_per_bone_of_a_chain_in_one_batch() {
        let mut app = trace_app();
        let original = app.doc.clone();
        // A bone with two children is not a chain.
        let dialog = physics_form(&mut app, "root");
        let Form::Physics(form) = &dialog.form else { panic!() };
        assert_eq!(form.chain(&app.doc), ["root"]);
        let mut dialog = physics_form(&mut app, "upper");
        let Form::Physics(form) = &mut dialog.form else { panic!() };
        assert_eq!(form.chain(&app.doc), ["upper", "lower"]);
        form.per_bone = true;
        let ops = dialog.ops(&app).unwrap();
        assert_eq!(ops.len(), 2);
        let json = serde_json::to_string(&ops).unwrap();
        let cli = skel2d_rig::apply_any(&original, &app.base_dir(), &skel2d_rig::parse_ops(&json).unwrap()).unwrap();
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert_eq!(app.doc, cli.document);
        assert_eq!(app.doc.physics.iter().map(|c| (c.name.as_str(), c.bone.as_str(), c.order)).collect::<Vec<_>>(), [("upper_physics", "upper", 1), ("lower_physics", "lower", 2)]);
        assert_eq!(app.selection.physics.as_deref(), Some("upper_physics"));
        app.undo(); // One batch, one undo step.
        assert_eq!(app.doc, original);
    }

    #[test]
    fn physics_names_are_unique_across_constraint_kinds() {
        let mut app = trace_app();
        let mut dialog = physics_form(&mut app, "upper");
        let Form::Physics(form) = &mut dialog.form else { panic!() };
        form.name = "reach".into(); // taken by IK
        assert!(dialog.validation(&app).is_some());
        assert!(key_dialog(&mut app, &mut dialog, Key::Enter)); // Stays open.
        assert!(app.doc.physics.is_empty());
    }

    #[test]
    fn physics_rename_and_delete_update_references_and_selection() {
        let mut app = trace_app();
        assert!(app.edit(vec![
            Op::AddPhysics(PhysicsConstraint::new("sway", "upper")),
            Op::KeyPhysics { animation: "wave".into(), constraint: "sway".into(), property: skel2d_core::ops::PhysicsProperty::Damping, time: 0.5, value: 0.8, curve: None },
            Op::KeyPhysicsReset { animation: "wave".into(), constraint: Some("sway".into()), time: 0.0 },
            Op::AddSkin { name: "armor".into() },
            Op::SetSkin { name: "armor".into(), rename: None, bones: None, ik: None, constraints: Some(vec!["sway".into()]) },
        ]));
        app.selection.physics = Some("sway".into());
        let before = app.doc.clone();
        rename(&mut app, Item::Physics("sway".into()));
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Rename(form) = &mut dialog.form else { panic!() };
        form.name = "swing".into();
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert!(app.doc.physics_constraint("swing").is_some() && app.doc.physics_constraint("sway").is_none());
        assert!(app.doc.animations["wave"].physics.contains_key("swing"));
        assert_eq!(app.doc.skin("armor").unwrap().constraints, ["swing"]);
        assert_eq!(app.selection.physics.as_deref(), Some("swing"));
        app.undo();
        assert_eq!(app.doc, before);
        app.selection.physics = Some("sway".into());
        // Deleting a keyed, skin-listed constraint asks first and counts its keys.
        delete(&mut app, Item::Physics("sway".into()));
        let Dialog::Editor(mut dialog) = std::mem::replace(&mut app.dialog, Dialog::None) else { panic!() };
        let Form::Delete(form) = &dialog.form else { panic!() };
        assert_eq!((form.impact.keys, form.impact.skin_refs, form.impact.constraints), (2, 1, 1));
        assert!(!key_dialog(&mut app, &mut dialog, Key::Enter));
        assert!(app.doc.physics.is_empty());
        assert!(app.doc.animations["wave"].physics.is_empty());
        assert!(app.doc.skin("armor").unwrap().constraints.is_empty());
        assert_eq!(app.selection.physics, None);
        app.undo();
        assert_eq!(app.doc, before);
    }

    #[test]
    fn deleting_a_bone_counts_the_physics_constraints_on_it() {
        let doc = skel2d_core::apply(&document(), &[Op::AddPhysics(PhysicsConstraint::new("sway", "child"))]).unwrap();
        let impact = deletion_impact(&doc, &Item::Bone("child".into())).unwrap();
        assert_eq!(impact.constraints, 1);
        assert!(impact.needs_confirmation(&Item::Bone("child".into())));
    }

    #[test]
    fn empty_names_and_duplicate_names_disable_submit() {
        assert!(name_error("   ", false).is_some());
        assert!(name_error("taken", true).is_some());
        assert!(name_error(" new ", false).is_none());
    }
}
