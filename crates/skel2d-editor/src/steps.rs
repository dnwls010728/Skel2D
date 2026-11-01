//! Work steps. The screen is organised by what the person is doing, not by
//! which tool is picked: 1 Bones, 2 Images, 3 Mesh, 4 Weights are the setup
//! pose (`playback.anim == None`), 5 Animation edits the last animation.
//! The internal `Tool` and `playback.anim` stay the source of truth; the step
//! decides which tools make sense and keeps the two in line.

use skel2d_core::{Attachment, Slot};

use crate::app::{EditorApp, Tool};
use crate::i18n::tr;
use crate::tf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    #[default]
    Bones,
    Images,
    Mesh,
    Weights,
    Animate,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Bones, Step::Images, Step::Mesh, Step::Weights, Step::Animate];

    pub fn number(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).map_or(1, |i| i + 1)
    }

    pub fn from_number(n: usize) -> Option<Step> {
        n.checked_sub(1).and_then(|i| Self::ALL.get(i)).copied()
    }

    /// English label for `tr`.
    pub fn label(self) -> &'static str {
        match self {
            Step::Bones => "Bones",
            Step::Images => "Images",
            Step::Mesh => "Mesh",
            Step::Weights => "Weights",
            Step::Animate => "Animation",
        }
    }

    /// Tools that make sense in this step; the others are not offered.
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Step::Bones => &[Tool::Select, Tool::Bone, Tool::Translate, Tool::Rotate, Tool::Scale, Tool::Shear],
            Step::Images => &[Tool::Select, Tool::Translate, Tool::Rotate, Tool::Scale],
            Step::Mesh => &[Tool::Mesh],
            Step::Weights => &[Tool::Weights],
            Step::Animate => &[Tool::Select, Tool::Translate, Tool::Rotate, Tool::Scale, Tool::Shear],
        }
    }

    pub fn allows(self, tool: Tool) -> bool {
        self.tools().contains(&tool)
    }

    pub fn default_tool(self) -> Tool {
        self.tools()[0]
    }

    /// The step a tool belongs to when it is asked for in another step.
    pub fn home_of(tool: Tool) -> Step {
        match tool {
            Tool::Mesh => Step::Mesh,
            Tool::Weights => Step::Weights,
            _ => Step::Bones,
        }
    }

    pub fn is_setup(self) -> bool {
        self != Step::Animate
    }
}

impl EditorApp {
    /// Switches the work step: 1-4 edit the setup pose, 5 the last animation.
    pub fn set_step(&mut self, step: Step) {
        self.view.pose_rotation = None;
        if step == Step::Animate {
            if self.step.is_setup() {
                self.last_setup_step = self.step;
            }
            self.step = Step::Animate;
            self.restore_animation();
        } else {
            if let Some(a) = self.playback.anim.take() {
                self.playback.last_anim = Some(a);
                self.playback.playing = false;
            }
            self.step = step;
            self.last_setup_step = step;
        }
        self.fit_tool_to_step();
    }

    /// A freshly opened or new document starts at step 1.
    pub fn reset_steps(&mut self) {
        self.step = Step::Bones;
        self.last_setup_step = Step::Bones;
        self.playback.last_anim = None;
        self.tool = Tool::Select;
    }

    /// The animation step edits the last animation, or the first one.
    fn restore_animation(&mut self) {
        if self.playback.anim.is_none() {
            let back = self.playback.last_anim.clone().filter(|a| self.doc.animations.contains_key(a));
            self.playback.anim = back.or_else(|| self.doc.animations.keys().next().cloned());
        }
    }

    fn fit_tool_to_step(&mut self) {
        if !self.step.allows(self.tool) {
            self.prev_tool = self.tool;
            self.tool = self.step.default_tool();
        }
    }

    /// Keeps step and animation in line when something else (the palette, a
    /// dialog, a deleted animation) changed `playback.anim`.
    pub fn sync_step(&mut self) {
        if self.playback.anim.is_some() && self.step != Step::Animate {
            if self.step.is_setup() {
                self.last_setup_step = self.step;
            }
            self.step = Step::Animate;
        }
        if self.step == Step::Animate && self.playback.anim.is_none() {
            self.restore_animation();
        }
        self.fit_tool_to_step();
    }

    /// Tab: between the last setup step and the animation step.
    pub fn toggle_mode(&mut self) {
        if self.step == Step::Animate {
            self.set_step(self.last_setup_step);
        } else {
            self.set_step(Step::Animate);
            if self.playback.anim.is_none() {
                crate::dialogs::new_animation(self);
            }
        }
    }

    /// Changes tool. A tool that belongs to another step takes you there
    /// (Mesh -> step 3, Weights -> step 4); while animating the setup tools
    /// are refused with a reason.
    pub fn set_tool(&mut self, tool: Tool) {
        self.view.pose_rotation = None;
        if !self.step.allows(tool) {
            if self.step == Step::Animate {
                self.status = tr("Not available while animating. Press 1-4 to go back to building the rig.").into();
                return;
            }
            self.set_step(Step::home_of(tool));
            if self.step.allows(tool) {
                self.tool = tool;
            }
            return;
        }
        if tool != self.tool {
            self.prev_tool = self.tool;
            self.tool = tool;
        }
    }
}

/// The slot picked in the list and the image it shows (or the picked card).
pub fn selected_attachment(app: &EditorApp) -> Option<(Slot, Attachment)> {
    let slot = app.doc.slot(app.selection.slot.as_deref()?)?.clone();
    let si = app.doc.slot_index(&slot.name)?;
    let name = app.selection.attachment.clone().filter(|a| app.doc.resolve_attachment(si, a, &app.skins).is_some()).or_else(|| slot.attachment.clone())?;
    let att = app.doc.resolve_attachment(si, &name, &app.skins)?.clone();
    Some((slot, att))
}

/// What the Mesh and Weights steps work on.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    None,
    /// The slot shows nothing.
    Hidden(String),
    Picture { slot: String, image: String },
    Mesh { slot: String, image: String, vertices: usize, triangles: usize },
    Shared { slot: String, image: String, parent: String },
}

pub fn target(app: &EditorApp) -> Target {
    let Some(slot_name) = app.selection.slot.clone().filter(|s| app.doc.slot(s).is_some()) else { return Target::None };
    let Some((slot, att)) = selected_attachment(app) else { return Target::Hidden(slot_name) };
    let (slot, image) = (slot.name, att.name().to_string());
    match att {
        Attachment::Region(_) => Target::Picture { slot, image },
        Attachment::Mesh(m) => Target::Mesh { slot, image, vertices: m.vertices.len(), triangles: m.triangles.len() },
        Attachment::LinkedMesh(l) => Target::Shared { slot, image, parent: l.parent },
    }
}

/// "Slot > Image", or nothing when no image is picked.
pub fn target_path(t: &Target) -> Option<String> {
    match t {
        Target::None => None,
        Target::Hidden(s) => Some(format!("{s} › {}", tr("(hidden)"))),
        Target::Picture { slot, image } | Target::Mesh { slot, image, .. } | Target::Shared { slot, image, .. } => Some(format!("{slot} › {image}")),
    }
}

/// Step 3 status line: says what the picked image is and what to do next.
pub fn mesh_status(t: &Target) -> String {
    match t {
        Target::None => tr("Pick an image in the list on the left.").into(),
        Target::Hidden(_) => tr("This slot shows no image. Pick an image card in step 2 first.").into(),
        Target::Picture { .. } => tr("Not a mesh yet. Turn it into a mesh to be able to bend the image.").into(),
        Target::Shared { parent, .. } => tf!("Uses the shape of its parent mesh \"{}\". Edit the parent to change it.", parent),
        Target::Mesh { vertices, triangles, .. } => tf!("Mesh with {} vertices and {} triangles.", vertices, triangles),
    }
}

/// Step 4 status line: how the picked mesh is bound to bones.
pub fn weights_status(app: &EditorApp, t: &Target) -> String {
    match t {
        Target::None => tr("Pick an image in the list on the left.").into(),
        Target::Hidden(_) => tr("This slot shows no image. Pick an image card in step 2 first.").into(),
        Target::Picture { .. } => tr("Not a mesh yet. Make it a mesh first (step 3).").into(),
        Target::Shared { parent, .. } => tf!("Uses the weights of its parent mesh \"{}\".", parent),
        Target::Mesh { .. } => {
            let Some((slot, Attachment::Mesh(mesh))) = selected_attachment(app) else { return String::new() };
            let mut bones: Vec<&str> = mesh.vertices.iter().flat_map(|v| v.weights.iter().map(|w| w.bone.as_str())).collect();
            bones.sort_unstable();
            bones.dedup();
            if bones.is_empty() {
                tf!("No weights: the whole image follows bone {}.", slot.bone)
            } else {
                tf!("Bound to {} bones.", bones.len())
            }
        }
    }
}

/// One line at the top of the viewport saying what can be done here now.
pub fn guide(app: &EditorApp) -> String {
    match app.step {
        Step::Bones => match app.tool {
            Tool::Bone => tr("Drag in the viewport to draw a bone. It becomes a child of the selected bone (or of root).").into(),
            Tool::Rotate => tr("Drag to rotate the selected bone.").into(),
            Tool::Translate => tr("Drag to move the selected bone.").into(),
            Tool::Scale => tr("Drag to scale the selected bone.").into(),
            Tool::Shear => tr("Drag to skew the selected bone.").into(),
            _ => tr("Click a bone to select it. Drag it to move it, or its tip to rotate it.").into(),
        },
        Step::Images => tr("Pick a slot in the list. Drop PNG files on the viewport or press Load images to add more.").into(),
        Step::Mesh => {
            if matches!(target(app), Target::Mesh { .. }) {
                use crate::viewport::MeshMode;
                match app.view.mesh_mode {
                    MeshMode::Modify => tr("Move: drag a vertex to reshape the mesh.").into(),
                    MeshMode::Create => tr("Add: click inside the image to add a vertex.").into(),
                    MeshMode::Delete => tr("Delete: click a vertex to remove it.").into(),
                    MeshMode::Edges => tr("Fold line: drag from vertex to vertex to make a line the mesh folds along.").into(),
                }
            } else {
                mesh_status(&target(app))
            }
        }
        Step::Weights => {
            if matches!(target(app), Target::Mesh { .. }) {
                tr("Pick a bone in the list, then paint on the image. Ctrl+drag removes.").into()
            } else {
                weights_status(app, &target(app))
            }
        }
        Step::Animate => {
            if app.playback.anim.is_none() {
                tr("Create an animation first. Everything you change is recorded as keys in it.").into()
            } else {
                tr("Move the time marker, then change a bone or press an image card to record a key.").into()
            }
        }
    }
}

/// Status bar text for the current step: tool shortcuts, step keys, help.
pub fn keys_text(app: &EditorApp) -> String {
    let tools: Vec<String> = app
        .step
        .tools()
        .iter()
        .filter_map(|t| Tool::ALL.iter().find(|a| a.0 == *t))
        .map(|(tool, _, key)| format!("{} {key:?}", tr(tool.ui_label())))
        .collect();
    format!("{}  ·  {}  ·  {}", tools.join("  "), tr("1-5 switch step"), tr("F1 help"))
}
