//! Document data model. Everything is addressed by name so that files stay
//! readable and edits (by humans or AI agents) never depend on indices.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::curve::Curve;

pub const FORMAT: &str = "skel2d/1";

/// The bone every skeleton hangs from. New bones without a parent go under it,
/// and it can be neither removed, renamed nor parented.
pub const ROOT: &str = "root";

pub type Vec2 = [f32; 2];

/// A complete Skel2D project: skeleton, draw order and animations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Format identifier, always "skel2d/1".
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub name: String,
    /// Bones, ordered so that every parent precedes its children.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bones: Vec<Bone>,
    /// Slots in draw order (first is drawn first, i.e. at the back).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<Slot>,
    /// IK constraints, solved by ascending order (array order breaks ties).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<IkConstraint>,
    /// Physics constraints, on the same evaluation order as `ik`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub physics: Vec<PhysicsConstraint>,
    /// Transform constraints, on the same evaluation order as `ik`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<TransformConstraint>,
    /// Values used by constraints with the matching `*_global` flag.
    #[serde(default, skip_serializing_if = "PhysicsGlobals::is_default")]
    pub physics_globals: PhysicsGlobals,
    /// Named events animations can fire, with their default values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, EventDef>,
    /// Named attachment sets (outfits, variants). A slot's
    /// own `attachments` are the default skin; an active skin replaces the
    /// attachment of the same (placeholder) name in that slot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skins: Vec<Skin>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub animations: BTreeMap<String, Animation>,
}

/// A skin: per slot, attachments keyed by placeholder name, plus
/// the skin-required bones and constraints that only work while it is active.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Skin {
    pub name: String,
    /// Slot -> attachments. Each attachment's name is the placeholder it fills.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attachments: BTreeMap<String, Vec<Attachment>>,
    /// Skin-required bones this skin turns on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bones: Vec<String>,
    /// Constraints that only apply while this skin is active.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
}

impl Skin {
    pub fn attachment(&self, slot: &str, name: &str) -> Option<&Attachment> {
        self.attachments.get(slot)?.iter().find(|a| a.name() == name)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format: default_format(),
            name: String::new(),
            bones: Vec::new(),
            slots: Vec::new(),
            ik: Vec::new(),
            physics: Vec::new(),
            transforms: Vec::new(),
            physics_globals: PhysicsGlobals::default(),
            events: BTreeMap::new(),
            skins: Vec::new(),
            animations: BTreeMap::new(),
        }
    }
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    /// A new document: just the root bone.
    pub fn with_root(name: impl Into<String>) -> Self {
        Self { bones: vec![Bone::new(ROOT)], ..Self::new(name) }
    }

    pub fn bone(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.name == name)
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.name == name)
    }

    pub fn slot_index(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.name == name)
    }

    pub fn physics_constraint(&self, name: &str) -> Option<&PhysicsConstraint> {
        self.physics.iter().find(|c| c.name == name)
    }

    /// Whether any kind of constraint has this name. Names are unique across kinds.
    pub fn has_constraint(&self, name: &str) -> bool {
        self.ik.iter().any(|c| c.name == name) || self.physics_constraint(name).is_some() || self.transforms.iter().any(|c| c.name == name)
    }

    /// Whether a constraint may apply with `skins` active: one owned by skins
    /// needs one of them.
    pub fn constraint_skin_active(&self, name: &str, skins: &[String]) -> bool {
        !self.skins.iter().any(|k| k.constraints.iter().any(|c| c == name))
            || skins.iter().filter_map(|k| self.skin(k)).any(|k| k.constraints.iter().any(|c| c == name))
    }

    pub fn skin(&self, name: &str) -> Option<&Skin> {
        self.skins.iter().find(|s| s.name == name)
    }

    /// The attachment `name` of slot `slot` with `skins` active: the last
    /// active skin that fills that placeholder wins, else the slot's own.
    pub fn resolve_attachment(&self, slot: usize, name: &str, skins: &[String]) -> Option<&Attachment> {
        self.resolve_attachment_from(slot, name, skins).map(|(_, attachment)| attachment)
    }

    /// Resolves an attachment and its owning skin; `None` denotes the default skin.
    pub fn resolve_attachment_from(&self, slot: usize, name: &str, skins: &[String]) -> Option<(Option<&str>, &Attachment)> {
        let s = self.slots.get(slot)?;
        skins.iter().rev().filter_map(|k| self.skin(k))
            .find_map(|k| k.attachment(&s.name, name).map(|a| (Some(k.name.as_str()), a)))
            .or_else(|| s.attachment(name).map(|a| (None, a)))
    }

    /// Materializes a linked mesh using its parent's current geometry and weights.
    pub fn materialize_attachment(&self, slot: usize, attachment: &Attachment) -> Option<Attachment> {
        let Attachment::LinkedMesh(link) = attachment else { return Some(attachment.clone()); };
        let slot_name = &self.slots.get(slot)?.name;
        let parent = match link.parent_skin.as_deref() {
            Some(skin) => self.skin(skin)?.attachment(slot_name, &link.parent)?,
            None => self.slots.get(slot)?.attachment(&link.parent)?,
        };
        let Attachment::Mesh(mesh) = parent else { return None; };
        let mut mesh = mesh.clone();
        mesh.name.clone_from(&link.name);
        if let Some(image) = &link.image { mesh.image = Some(image.clone()); }
        Some(Attachment::Mesh(mesh))
    }

    /// Whether bone `i` is active with `skins`: skin-required bones (and
    /// everything under them) need an active skin that lists them.
    pub fn bone_active(&self, i: usize, skins: &[String]) -> bool {
        let mut cur = self.bones.get(i);
        while let Some(b) = cur {
            if b.skin_required && !skins.iter().filter_map(|k| self.skin(k)).any(|k| k.bones.contains(&b.name)) {
                return false;
            }
            cur = b.parent.as_deref().and_then(|p| self.bone(p));
        }
        true
    }
}

fn default_format() -> String {
    FORMAT.to_string()
}

/// Setup pose of a bone, relative to its parent. Angles are in degrees,
/// counter-clockwise; +y is up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bone {
    pub name: String,
    /// Parent bone name; omitted for root bones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shear_x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shear_y: f32,
    #[serde(default, skip_serializing_if = "Inherit::is_normal")]
    pub inherit: Inherit,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_y: f32,
    /// Visual length along the local +x axis. Does not affect children.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub length: f32,
    /// Skin required: the bone (its slots and children) is only
    /// active while a skin listing it in `bones` is active.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skin_required: bool,
    /// Color of the bone in the editor. Does not affect the pose or rendering of images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

impl Bone {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parent: None,
            x: 0.0,
            y: 0.0,
            rotation: 0.0,
            shear_x: 0.0,
            shear_y: 0.0,
            inherit: Inherit::Normal,
            scale_x: 1.0,
            scale_y: 1.0,
            length: 0.0,
            skin_required: false,
            color: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Inherit {
    #[default]
    Normal,
    OnlyTranslation,
    NoRotationOrReflection,
    NoScale,
    NoScaleOrReflection,
}

impl Inherit {
    pub fn is_normal(&self) -> bool { *self == Self::Normal }
}

/// A drawable socket attached to a bone. Holds a set of attachments of which
/// at most one is visible at a time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub name: String,
    pub bone: String,
    #[serde(default, skip_serializing_if = "Color::is_white")]
    pub color: Color,
    /// How the slot's pixels combine with what is behind them.
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend: BlendMode,
    /// Another slot whose rendered alpha masks this one, like a Photoshop
    /// clipping layer. The two keep moving independently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    /// Attachment visible in the setup pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl Slot {
    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.name() == name)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    /// Adds light: glows and sparks.
    Additive,
    /// Darkens by the slot's color: shadows and lines.
    Multiply,
    /// Lightens without burning out: soft highlights.
    Screen,
}

impl BlendMode {
    pub fn is_normal(&self) -> bool { *self == Self::Normal }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Attachment {
    /// A rectangular image placed relative to the slot's bone.
    Region(Region),
    /// A triangulated image deformed by one or more weighted bones.
    Mesh(Mesh),
    /// Shares mesh geometry and weights with a parent in the same slot.
    LinkedMesh(LinkedMesh),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkedMesh {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub parent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_skin: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub timelines: bool,
}

fn default_true() -> bool { true }

impl Attachment {
    pub fn name(&self) -> &str {
        match self {
            Attachment::Region(r) => &r.name,
            Attachment::Mesh(m) => &m.name,
            Attachment::LinkedMesh(m) => &m.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub name: String,
    /// Image path relative to the document file. Without it a placeholder
    /// rectangle is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_y: f32,
    pub width: f32,
    pub height: f32,
}

/// Triangle mesh. Vertex positions are in setup-pose skeleton space (where
/// the vertex appears when every bone is in its setup pose), so a mesh can be
/// laid over a setup-pose render directly. Bones deform it by linear blend
/// skinning relative to their setup pose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mesh {
    /// Explicit binding palette, including bones with zero influence. Empty
    /// preserves legacy inference from the vertex weights and slot bone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bones: Vec<String>,
    /// The first hull vertices form the clockwise outer boundary. Zero infers
    /// boundaries from triangles, preserving legacy vertex indices on load.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub hull: u32,
    /// User constrained edges; endpoint indices refer to `vertices`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<[u32; 2]>,
    pub name: String,
    /// Image path relative to the document file. Without it the triangles are
    /// drawn as a placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub vertices: Vec<MeshVertex>,
    /// Vertex index triples.
    pub triangles: Vec<[u32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeshVertex {
    pub x: f32,
    pub y: f32,
    /// Texture coordinates in 0..=1, (0, 0) is the image's top-left corner.
    pub u: f32,
    pub v: f32,
    /// Bone influences, normalized when evaluated. Empty means the slot's bone
    /// with full weight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weights: Vec<Weight>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Weight {
    pub bone: String,
    pub weight: f32,
    /// Where this bone holds the vertex, relative to the vertex's position.
    /// The weighted offsets of a vertex average to zero, so its setup position
    /// stays put while bones that bound it elsewhere still pull it their way.
    #[serde(default, skip_serializing_if = "is_zero_vec2")]
    pub offset: Vec2,
}

/// Rotates one bone to point at `target`, or a parent and its direct child so
/// the child's tip reaches `target`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IkConstraint {
    pub name: String,
    /// Evaluation priority. Any i32 is valid; ties preserve array order.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    /// `[bone]` or `[parent, child]`.
    pub bones: Vec<String>,
    /// Bone whose world position is the goal. Must not be inside the chain.
    pub target: String,
    /// 0 = no effect, 1 = fully solved.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix: f32,
    /// Two-bone chains: bend counter-clockwise (true) or clockwise (false).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub bend_positive: bool,
    /// Two-bone chains: world distance before full extension at which the
    /// chain starts slowing down, so the limb never snaps straight.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub softness: f32,
    /// One-bone chains: shorten the bone along x when the target is closer than its length.
    #[serde(default, skip_serializing_if = "is_false")]
    pub compress: bool,
    /// Lengthen the bones along x when the target is out of reach.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stretch: bool,
}

impl IkConstraint {
    pub fn pose(&self) -> IkPose {
        IkPose { mix: self.mix, softness: self.softness, bend_positive: self.bend_positive, compress: self.compress, stretch: self.stretch }
    }
}

/// The values of an IK constraint that animations can change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IkPose {
    pub mix: f32,
    pub softness: f32,
    pub bend_positive: bool,
    pub compress: bool,
    pub stretch: bool,
}

/// Value of an IK key. An omitted field stays at the constraint's setup value.
/// A key holding only `mix` is written as a plain number. `mix` and `softness`
/// interpolate (curve channels 0 and 1); the switches hold until the next key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "IkKeyForm", into = "IkKeyForm")]
pub struct IkKey {
    pub mix: Option<f32>,
    pub softness: Option<f32>,
    pub bend_positive: Option<bool>,
    pub compress: Option<bool>,
    pub stretch: Option<bool>,
}

impl IkKey {
    pub fn resolve(&self, setup: &IkPose) -> IkPose {
        IkPose {
            mix: self.mix.unwrap_or(setup.mix),
            softness: self.softness.unwrap_or(setup.softness),
            bend_positive: self.bend_positive.unwrap_or(setup.bend_positive),
            compress: self.compress.unwrap_or(setup.compress),
            stretch: self.stretch.unwrap_or(setup.stretch),
        }
    }
}

impl From<f32> for IkKey {
    fn from(mix: f32) -> Self {
        Self { mix: Some(mix), ..Self::default() }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum IkKeyForm {
    Mix(f32),
    Values(IkKeyValues),
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IkKeyValues {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mix: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    softness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bend_positive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stretch: Option<bool>,
}

impl From<IkKeyForm> for IkKey {
    fn from(form: IkKeyForm) -> Self {
        match form {
            IkKeyForm::Mix(mix) => mix.into(),
            IkKeyForm::Values(v) => Self { mix: v.mix, softness: v.softness, bend_positive: v.bend_positive, compress: v.compress, stretch: v.stretch },
        }
    }
}

impl From<IkKey> for IkKeyForm {
    fn from(k: IkKey) -> Self {
        match k {
            IkKey { mix: Some(mix), softness: None, bend_positive: None, compress: None, stretch: None } => Self::Mix(mix),
            k => Self::Values(IkKeyValues { mix: k.mix, softness: k.softness, bend_positive: k.bend_positive, compress: k.compress, stretch: k.stretch }),
        }
    }
}

impl JsonSchema for IkKey {
    fn schema_name() -> Cow<'static, str> {
        "IkKey".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        IkKeyForm::json_schema(generator)
    }
}

/// A named event and the values its keys use unless they override them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub int: i32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub float: f32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub string: String,
}

/// Fires event `name` when playback passes `time`. Omitted values come from
/// the event's definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventKey {
    pub time: f32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub int: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string: Option<String>,
}

/// Changes the draw order from `time` until the next key. `order` lists slots
/// back to front; a slot added after the key was made keeps its place above its
/// setup neighbour. Without `order` the setup draw order returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawOrderKey {
    pub time: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

/// Makes bones copy part of another bone's transform. Each mix is how much of
/// that part is taken: 0 leaves it alone and negative values go the other way.
///
/// Normally a bone moves toward the source's value plus the offset, in world
/// space, or with `local` among the parent-relative values. With `relative` it
/// adds instead: in world space the offset turned by the source, plus the
/// source's rotation, scale and shear; with `local` the source's local values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformConstraint {
    pub name: String,
    /// Evaluation priority, shared with IK. Any i32 is valid.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    pub bones: Vec<String>,
    pub source: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_rotate: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_y: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_scale_x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_scale_y: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix_shear_y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_rotation: f32,
    /// Position offset, along the source's axes in world mode.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_scale_x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_scale_y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_shear_y: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub local: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub relative: bool,
}

/// The animatable mixes of a transform constraint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformMix {
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub rotate: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub y: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_x: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_y: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub shear_y: f32,
}

impl TransformMix {
    pub fn all_finite(&self) -> bool {
        [self.rotate, self.x, self.y, self.scale_x, self.scale_y, self.shear_y].iter().all(|v| v.is_finite())
    }
}

impl TransformConstraint {
    pub fn mix(&self) -> TransformMix {
        TransformMix { rotate: self.mix_rotate, x: self.mix_x, y: self.mix_y, scale_x: self.mix_scale_x, scale_y: self.mix_scale_y, shear_y: self.mix_shear_y }
    }

    pub fn with_mix(&self, m: TransformMix) -> Self {
        Self { mix_rotate: m.rotate, mix_x: m.x, mix_y: m.y, mix_scale_x: m.scale_x, mix_scale_y: m.scale_y, mix_shear_y: m.shear_y, ..self.clone() }
    }

    pub fn new(name: impl Into<String>, bones: Vec<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(), order: 0, bones, source: source.into(),
            mix_rotate: 1.0, mix_x: 1.0, mix_y: 1.0, mix_scale_x: 1.0, mix_scale_y: 1.0, mix_shear_y: 1.0,
            offset_rotation: 0.0, offset_x: 0.0, offset_y: 0.0, offset_scale_x: 0.0, offset_scale_y: 0.0, offset_shear_y: 0.0,
            local: false, relative: false,
        }
    }
}

/// Makes a bone lag behind its animated pose on a damped spring. Omitted values
/// give a body that lags partly and settles: full inertia without damping rings forever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhysicsConstraint {
    pub name: String,
    /// Evaluation priority, shared with IK. Any i32 is valid.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    pub bone: String,
    /// Influence 0..=1 of the simulation on world x. 0 does not simulate it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotate: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub scale_x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shear_x: f32,
    /// Speed limit in world units per second; also limits movement per frame.
    #[serde(default = "physics_limit", skip_serializing_if = "is_physics_limit")]
    pub limit: f32,
    /// Fixed simulation step in seconds.
    #[serde(default = "physics_step", skip_serializing_if = "is_physics_step")]
    pub step: f32,
    /// How much of the bone's movement bodies resist. 1 lags fully.
    #[serde(default = "physics_inertia", skip_serializing_if = "is_physics_inertia")]
    pub inertia: f32,
    /// Spring stiffness pulling back to the animated pose.
    #[serde(default = "physics_strength", skip_serializing_if = "is_physics_strength")]
    pub strength: f32,
    /// Velocity kept per 1/60 second, 0..=1. 1 never loses speed.
    #[serde(default = "physics_damping", skip_serializing_if = "is_physics_damping")]
    pub damping: f32,
    /// One over the body's mass: how strongly forces accelerate it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mass_inverse: f32,
    /// World +x acceleration.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wind: f32,
    /// World -y acceleration: positive sags downward.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gravity: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inertia_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strength_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub damping_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mass_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wind_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub gravity_global: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mix_global: bool,
}

impl PhysicsConstraint {
    pub fn new(name: impl Into<String>, bone: impl Into<String>) -> Self {
        Self {
            name: name.into(), order: 0, bone: bone.into(), x: 0.0, y: 0.0, rotate: 0.0, scale_x: 0.0, shear_x: 0.0,
            limit: physics_limit(), step: physics_step(), inertia: physics_inertia(), strength: physics_strength(), damping: physics_damping(), mass_inverse: 1.0,
            wind: 0.0, gravity: 0.0, mix: 1.0, inertia_global: false, strength_global: false, damping_global: false,
            mass_global: false, wind_global: false, gravity_global: false, mix_global: false,
        }
    }
}

/// Values shared by constraints that opt in with their `*_global` flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhysicsGlobals {
    #[serde(default = "physics_inertia", skip_serializing_if = "is_physics_inertia")]
    pub inertia: f32,
    #[serde(default = "physics_strength", skip_serializing_if = "is_physics_strength")]
    pub strength: f32,
    #[serde(default = "physics_damping", skip_serializing_if = "is_physics_damping")]
    pub damping: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mass_inverse: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wind: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gravity: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix: f32,
}

impl Default for PhysicsGlobals {
    fn default() -> Self {
        Self { inertia: physics_inertia(), strength: physics_strength(), damping: physics_damping(), mass_inverse: 1.0, wind: 0.0, gravity: 0.0, mix: 1.0 }
    }
}

impl PhysicsGlobals {
    pub fn is_default(&self) -> bool { *self == Self::default() }
}

/// Keys per physics property. `mass` keys hold `mass_inverse` values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhysicsTimelines {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inertia: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strength: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub damping: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mass: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wind: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gravity: Vec<Key<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mix: Vec<Key<f32>>,
    /// Times at which the simulation restarts from the current pose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset: Vec<f32>,
}

impl PhysicsTimelines {
    pub fn property_keys(&self) -> [(&'static str, &[Key<f32>]); 7] {
        [("inertia", &self.inertia), ("strength", &self.strength), ("damping", &self.damping), ("mass", &self.mass), ("wind", &self.wind), ("gravity", &self.gravity), ("mix", &self.mix)]
    }

    pub fn is_empty(&self) -> bool {
        self.reset.is_empty() && self.property_keys().iter().all(|(_, k)| k.is_empty())
    }
}

fn physics_limit() -> f32 { 5000.0 }
fn physics_step() -> f32 { 1.0 / 60.0 }
fn physics_strength() -> f32 { 100.0 }
fn physics_inertia() -> f32 { 0.5 }
fn physics_damping() -> f32 { 0.85 }
fn is_physics_inertia(v: &f32) -> bool { *v == physics_inertia() }
fn is_physics_damping(v: &f32) -> bool { *v == physics_damping() }
fn is_physics_limit(v: &f32) -> bool { *v == physics_limit() }
fn is_physics_step(v: &f32) -> bool { *v == physics_step() }
fn is_physics_strength(v: &f32) -> bool { *v == physics_strength() }

/// Runtime preview track, lowest first in a list. Not stored in the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerTrack {
    pub anim: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub alpha: f32,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl LayerTrack {
    pub fn new(anim: impl Into<String>) -> Self {
        Self { anim: anim.into(), alpha: 1.0, enabled: true }
    }
}

/// Keyframed animation. Bone values are offsets from the setup pose:
/// rotate and translate are added, scale is multiplied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    /// Mesh deformation per skin, slot and mesh attachment name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deform: DeformTimelines,
    /// Explicit length in seconds; defaults to the time of the last key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bones: BTreeMap<String, BoneTimelines>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<String, SlotTimelines>,
    /// Keys per IK constraint, replacing the constraint's setup values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ik: BTreeMap<String, Vec<Key<IkKey>>>,
    /// Mix keys per transform constraint, replacing its setup mixes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub transforms: BTreeMap<String, Vec<Key<TransformMix>>>,
    /// Keys per physics constraint. Name "" is the global timeline: its resets
    /// restart every constraint, its property keys drive those using that global.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub physics: BTreeMap<String, PhysicsTimelines>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draw_order: Vec<DrawOrderKey>,
    /// Sorted by time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventKey>,
}

impl Animation {
    pub fn duration(&self) -> f32 {
        if let Some(d) = self.duration {
            return d;
        }
        let bones = self.bones.values().flat_map(|t| {
            last_time(&t.rotate)
                .into_iter()
                .chain(last_time(&t.translate))
                .chain(last_time(&t.scale))
                .chain(last_time(&t.shear))
        });
        let slots = self.slots.values().flat_map(|t| {
            t.attachment
                .last()
                .map(|k| k.time)
                .into_iter()
                .chain(last_time(&t.color))
        });
        let ik = self.ik.values().filter_map(|k| last_time(k)).chain(self.transforms.values().filter_map(|k| last_time(k)));
        let deform = self.deform.values().flat_map(|slots| slots.values()).flat_map(|atts| atts.values()).filter_map(|k| last_time(k));
        let physics = self.physics.values().flat_map(|t| {
            t.reset.last().copied().into_iter().chain(t.property_keys().into_iter().filter_map(|(_, k)| last_time(k)))
        });
        let moments = self.draw_order.last().map(|k| k.time).into_iter().chain(self.events.last().map(|k| k.time));
        bones.chain(slots).chain(ik).chain(deform).chain(physics).chain(moments).fold(0.0, f32::max)
    }
}

fn last_time<T>(keys: &[Key<T>]) -> Option<f32> {
    keys.last().map(|k| k.time)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoneTimelines {
    /// Angular offsets added to setup shear X/Y.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shear: Vec<Key<Vec2>>,
    /// Degrees added to the setup rotation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotate: Vec<Key<f32>>,
    /// Offset added to the setup position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translate: Vec<Key<Vec2>>,
    /// Factor multiplied with the setup scale.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scale: Vec<Key<Vec2>>,
}

impl BoneTimelines {
    pub fn is_empty(&self) -> bool {
        self.rotate.is_empty() && self.translate.is_empty() && self.scale.is_empty() && self.shear.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotTimelines {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment: Vec<AttachmentKey>,
    /// Color multiplied with the slot's setup color.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub color: Vec<Key<Color>>,
}

impl SlotTimelines {
    pub fn is_empty(&self) -> bool {
        self.attachment.is_empty() && self.color.is_empty()
    }
}

/// Mesh deformation per mesh attachment name: one offset per vertex,
/// added to its setup-pose position before bone skinning. Outer keys are
/// skin names (`default` for slot attachments), followed by slot names.
pub type DeformTimelines = BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<Key<Vec<Vec2>>>>>>;

/// A keyframe. `curve` shapes the interpolation towards the next key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Key<T> {
    pub time: f32,
    pub value: T,
    #[serde(default, skip_serializing_if = "Curve::is_linear")]
    pub curve: Curve,
    /// One curve per channel, replacing `curve`: `[x, y]` or `[r, g, b, a]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<Curve>,
}

impl<T> Key<T> {
    pub fn new(time: f32, value: T, curve: Curve) -> Self {
        Self { time, value, curve, curves: Vec::new() }
    }

    pub fn curve_of(&self, channel: usize) -> Curve {
        self.curves.get(channel).copied().unwrap_or(self.curve)
    }
}

/// Switches the visible attachment; `name: null` hides the slot. With
/// `reset: true` the slot goes back to what it showed before this animation
/// applied (the setup pose, or a lower track such as a face pose), so a
/// blink or mouth animation layered on top works with any face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentKey {
    pub time: f32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
}

/// Linear RGBA in 0..=1, serialized as a hex string "#rrggbb" or "#rrggbbaa".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub fn is_white(&self) -> bool {
        *self == Self::WHITE
    }

    pub fn multiply(self, o: Color) -> Color {
        Color { r: self.r * o.r, g: self.g * o.g, b: self.b * o.b, a: self.a * o.a }
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }

    pub fn parse_hex(s: &str) -> Option<Color> {
        let h = s.strip_prefix('#').unwrap_or(s);
        if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
        Some(Color {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if h.len() == 8 { byte(6)? } else { 1.0 },
        })
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.to_rgba8();
        if a == 255 {
            write!(f, "#{r:02x}{g:02x}{b:02x}")
        } else {
            write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?}, expected #rrggbb or #rrggbbaa")))
    }
}

impl JsonSchema for Color {
    fn schema_name() -> Cow<'static, str> {
        "Color".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^#?([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
            "description": "Hex color #rrggbb or #rrggbbaa"
        })
    }
}

fn one() -> f32 {
    1.0
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool { *v == 0 }

fn is_zero_vec2(v: &Vec2) -> bool { *v == [0.0, 0.0] }

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}
