//! Edit operations: the single mutation API shared by the CLI, the HTTP API
//! and (later) the editor. A batch is applied atomically: either every op
//! succeeds and the result validates, or the document is left untouched.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::curve::Curve;
use crate::error::{Error, Result};
use crate::math::Affine;
use crate::mesh::segment_distance;
use crate::model::{
    Attachment, AttachmentKey, Bone, Color, Document, IkConstraint, Key, LinkedMesh, Mesh, MeshVertex, PhysicsConstraint, PhysicsTimelines, ROOT, Slot, TransformConstraint, Vec2, Weight,
};
use crate::pose::{LocalTransform, Pose};
use crate::validate::{sort_bones, validate};

/// Two keys closer than this are considered the same key.
pub const TIME_EPSILON: f32 = 1e-4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    SetName {
        name: String,
    },
    /// Adds a bone. Fields are the same as in the document.
    AddBone(Bone),
    /// Adds a bone from setup-pose world coordinates: it starts at `start`
    /// and points to `end` (length 0 and world rotation 0 without `end`).
    /// The local transform relative to `parent` is computed for you.
    AddBoneWorld {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        start: Vec2,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<Vec2>,
    },
    /// Changes fields of an existing bone; omitted fields stay unchanged.
    SetBone(BonePatch),
    /// Removes a bone, its slots and its timelines. With `recursive` the
    /// whole subtree is removed, otherwise children are an error.
    RemoveBone {
        name: String,
        #[serde(default)]
        recursive: bool,
    },
    /// Adds a slot on top of the draw order (drawn last), or at `index` (0 = back).
    AddSlot {
        #[serde(flatten)]
        slot: Slot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    SetSlot(SlotPatch),
    RemoveSlot {
        name: String,
    },
    /// Moves a slot to `index` in the draw order (0 = back).
    MoveSlot {
        name: String,
        index: usize,
    },
    /// Adds an attachment to a slot or replaces the one with the same name.
    SetAttachment {
        slot: String,
        attachment: Attachment,
    },
    RemoveAttachment {
        slot: String,
        name: String,
        #[serde(default, skip_serializing_if = "is_parent_error")]
        on_parent_removed: ParentRemoval,
    },
    AddLinkedMesh {
        slot: String,
        name: String,
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        timelines: bool,
    },
    /// Renames an attachment in one skin (omitted skin means default).
    RenameAttachment {
        slot: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
    },
    /// Renames the same placeholder in the default skin and every named skin.
    RenamePlaceholder { slot: String, from: String, to: String },
    /// Creates an empty skin (a named attachment set, e.g. an outfit).
    AddSkin {
        name: String,
    },
    /// Renames a skin or replaces its skin-required bones / constraints.
    SetSkin {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bones: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ik: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        constraints: Option<Vec<String>>,
    },
    RemoveSkin {
        name: String,
        #[serde(default, skip_serializing_if = "is_parent_error")]
        on_parent_removed: ParentRemoval,
    },
    /// Puts an attachment into a skin for `slot`, or replaces the one with
    /// the same name. The name is the placeholder: wherever the slot (or a
    /// key) shows that name, the active skin's attachment is drawn instead.
    SetSkinAttachment {
        skin: String,
        slot: String,
        attachment: Attachment,
    },
    RemoveSkinAttachment {
        skin: String,
        slot: String,
        name: String,
        #[serde(default, skip_serializing_if = "is_parent_error")]
        on_parent_removed: ParentRemoval,
    },
    /// Creates an empty animation (keying into a missing animation also creates it).
    AddAnimation {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<f32>,
    },
    /// Copies an animation, including every key and curve, to a new name.
    DuplicateAnimation {
        name: String,
        new_name: String,
    },
    SetAnimation {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename: Option<String>,
        /// Explicit duration; `null` returns to "last key time".
        #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<f32>")]
        duration: Option<Option<f32>>,
    },
    RemoveAnimation {
        name: String,
    },
    /// Sets bone keys at `time` (replacing keys already there).
    KeyBone(KeyBone),
    /// Sets slot keys at `time` (replacing keys already there).
    KeySlot(KeySlot),
    /// Keys a mesh attachment's deformation at `time` (replacing a key
    /// already there): one `[dx, dy]` offset per vertex, in setup-pose
    /// skeleton space, added before bone skinning.
    KeyDeform {
        animation: String,
        slot: String,
        attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        time: f32,
        offsets: Vec<Vec2>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
    },
    /// Removes the key at `time` from one timeline of a bone or slot.
    RemoveKey {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        animation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bone: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<String>,
        timeline: Timeline,
        time: f32,
        /// With the deform timeline: only this mesh attachment's key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachment: Option<String>,
    },
    /// Removes all keys of a bone or slot, or only one timeline of it.
    ClearTimeline {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        animation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bone: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline: Option<Timeline>,
    },
    /// Adds an IK constraint; equal priorities preserve insertion order.
    AddIk(IkConstraint),
    /// Adds a transform constraint. Fields are the same as in the document.
    AddTransform(TransformConstraint),
    /// Changes fields of a transform constraint; omitted fields stay unchanged.
    SetTransform(TransformPatch),
    RemoveTransform { name: String },
    /// Keys a transform constraint's mixes at `time` (replacing a key already there).
    KeyTransform {
        animation: String,
        constraint: String,
        time: f32,
        mix: crate::model::TransformMix,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
    },
    RemoveTransformKey {
        animation: String,
        constraint: String,
        time: f32,
    },
    /// Adds a physics constraint. Fields are the same as in the document.
    AddPhysics(PhysicsConstraint),
    /// Changes fields of a physics constraint; omitted fields stay unchanged.
    SetPhysics(PhysicsPatch),
    /// Removes a physics constraint and its keys.
    RemovePhysics { name: String },
    /// Keys a physics property at `time` (replacing a key already there).
    /// Constraint "" is the global timeline of constraints using that global.
    KeyPhysics {
        animation: String,
        constraint: String,
        property: PhysicsProperty,
        time: f32,
        value: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
    },
    /// Restarts the simulation at `time`; without `constraint`, every constraint.
    KeyPhysicsReset {
        animation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        constraint: Option<String>,
        time: f32,
    },
    /// Removes a property key, or with no `property` a reset key.
    RemovePhysicsKey {
        animation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        constraint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        property: Option<PhysicsProperty>,
        time: f32,
    },
    /// Changes the shared values; omitted fields stay unchanged.
    SetPhysicsGlobals(PhysicsGlobalsPatch),
    SetIk(IkPatch),
    /// Lists every constraint exactly once, assigning priorities 0, 1, 2, ... .
    SetConstraintOrder {
        names: Vec<String>,
    },
    RemoveIk {
        name: String,
    },
    /// Keys IK values at `time`. Give at least one value; others keep what a key
    /// already there holds, or else follow the setup value.
    KeyIk {
        animation: String,
        ik: String,
        time: f32,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
        /// `[mix, softness]`, replacing `curve`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curves: Option<Vec<Curve>>,
    },
    RemoveIkKey {
        animation: String,
        ik: String,
        time: f32,
    },
    /// Changes the draw order at `time` (replacing a key already there).
    /// `order` lists every slot back to front; omitted or null returns to the setup order.
    KeyDrawOrder {
        animation: String,
        time: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<Vec<String>>,
    },
    RemoveDrawOrderKey {
        animation: String,
        time: f32,
    },
    /// Defines an event with default values.
    AddEvent {
        name: String,
        #[serde(default, skip_serializing_if = "is_zero_int")]
        int: i32,
        #[serde(default, skip_serializing_if = "is_zero_float")]
        float: f32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        string: String,
    },
    /// Renames an event or changes its defaults; omitted fields stay unchanged.
    SetEvent {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        int: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        float: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        string: Option<String>,
    },
    /// Removes an event and all its keys.
    RemoveEvent {
        name: String,
    },
    /// Fires an event at `time` (replacing that event's key already there).
    /// Omitted values use the event's defaults.
    KeyEvent {
        animation: String,
        name: String,
        time: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        int: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        float: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        string: Option<String>,
    },
    RemoveEventKey {
        animation: String,
        name: String,
        time: f32,
    },
    /// Replaces a region attachment with a grid mesh covering the same area
    /// (vertices in setup-pose skeleton space, weighted to the slot's bone).
    MeshFromRegion {
        slot: String,
        region: String,
        /// Name of the new mesh; defaults to the region's name (replacing it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default = "default_grid")]
        cols: u32,
        #[serde(default = "default_grid")]
        rows: u32,
    },
    /// Weights every vertex of a mesh to nearby bones by distance to each
    /// bone segment in the setup pose.
    AutoWeights {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
        slot: String,
        attachment: String,
        /// Candidate bones; defaults to all bones.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bones: Option<Vec<String>>,
        /// Maximum bones per vertex.
        #[serde(default = "default_influences")]
        max_influences: usize,
        /// Higher values make weights fall off faster with distance.
        #[serde(default = "default_falloff")]
        falloff: f32,
        /// Only these vertex indices; others keep their weights.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
        /// The mesh lives in this skin instead of the slot itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
    },
    /// Copies weights onto a mesh from another mesh by position in the setup
    /// pose (each vertex takes the interpolated weights of the closest point
    /// on the source), e.g. clothing from the body, or after rebuilding a mesh.
    TransferWeights {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
        slot: String,
        attachment: String,
        from_slot: String,
        from_attachment: String,
        /// Only these vertex indices; others keep their weights.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_skin: Option<String>,
    },
    /// Mirrors weights across the vertical line x = `axis` (setup pose): the
    /// vertices on the other side of `source` take the weights found at their
    /// mirrored position, with left/right bones swapped by name
    /// (`_l`/`_r`, `_L`/`_R`, `left`/`right`, `Left`/`Right`, `L_`/`R_`).
    MirrorWeights {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
        slot: String,
        attachment: String,
        #[serde(default)]
        axis: f32,
        /// Side whose weights are kept: `positive` (x > axis) or `negative`.
        #[serde(default)]
        source: MirrorSide,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
    },
    SmoothWeights {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
        #[serde(default = "default_smooth_factor")]
        factor: f32,
        #[serde(default = "default_iterations")]
        iterations: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
    },
    PruneWeights {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
        #[serde(default = "default_influences")]
        max_bones: usize,
        #[serde(default = "default_prune_threshold")]
        threshold: f32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
    },
    SwapWeights {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
        a: String, b: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
    },
    SetBoneWeight {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        vertices: Vec<usize>, bone: String, value: f32, mode: WeightMode,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
    },
    BindBones {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
        redistribute: Redistribute,
    },
    NormalizeWeights {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
    },
    /// Adds points in setup space, preserving UVs, weights and deformation keys.
    AddMeshVertices {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        points: Vec<Vec2>,
        #[serde(default)]
        on_edge_tolerance: f32,
    },
    RemoveMeshVertices {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        indices: Vec<usize>,
    },
    SetMeshEdges {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        edges: Vec<[u32; 2]>,
    },
    GenerateMeshVertices {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spacing: Option<f32>,
    },
    ResetMesh {
        slot: String, attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        mode: MeshResetMode,
    },
    /// Edits individual mesh vertices; omitted fields stay unchanged.
    SetVertices {
        slot: String,
        attachment: String,
        vertices: Vec<VertexPatch>,
        /// The mesh lives in this skin instead of the slot itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeshResetMode { VerticesToUvs, UvsToVertices }

fn default_grid() -> u32 {
    4
}

fn default_influences() -> usize {
    4
}

fn default_falloff() -> f32 {
    2.0
}

fn default_smooth_factor() -> f32 { 0.5 }
fn default_iterations() -> usize { 1 }
fn default_prune_threshold() -> f32 { 0.01 }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WeightMode { Set, Add, Scale }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Redistribute { Auto, Nearest, Parent }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParentRemoval { #[default] Error, Freeze, Delete }

fn default_true() -> bool { true }
fn is_zero_int(v: &i32) -> bool { *v == 0 }
fn is_zero_float(v: &f32) -> bool { *v == 0.0 }
fn is_true(v: &bool) -> bool { *v }
fn is_parent_error(v: &ParentRemoval) -> bool { *v == ParentRemoval::Error }

/// Which side of the mirror axis keeps its weights.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MirrorSide {
    /// x > axis (the character's left when facing the viewer).
    #[default]
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PhysicsProperty { Inertia, Strength, Damping, Mass, Wind, Gravity, Mix }

impl PhysicsProperty {
    pub fn keys(self, t: &mut PhysicsTimelines) -> &mut Vec<Key<f32>> {
        match self {
            Self::Inertia => &mut t.inertia,
            Self::Strength => &mut t.strength,
            Self::Damping => &mut t.damping,
            Self::Mass => &mut t.mass,
            Self::Wind => &mut t.wind,
            Self::Gravity => &mut t.gravity,
            Self::Mix => &mut t.mix,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformPatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bones: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_rotate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_scale_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_scale_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_shear_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_rotation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_scale_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_scale_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_shear_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhysicsPatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass_inverse: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity_global: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_global: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhysicsGlobalsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass_inverse: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IkPatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bones: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend_positive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VertexPatch {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub u: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<f32>,
    /// Replaces the vertex's weights; `[]` binds it to the slot's bone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Vec<Weight>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Timeline {
    Rotate,
    Translate,
    Scale,
    Shear,
    Attachment,
    Color,
    /// Mesh deform keys of every attachment of the slot.
    Deform,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BonePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<crate::model::Inherit>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    /// New parent bone; `null` makes it a root bone.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f32>,
    /// Only active while a skin listing the bone is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_required: Option<bool>,
    /// Editor color; `null` returns to the default.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Color>")]
    pub color: Option<Option<Color>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotPatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<crate::model::BlendMode>,
    /// Visible setup attachment; `null` hides the slot.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub attachment: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyBone {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear: Option<Vec2>,
    pub animation: String,
    pub bone: String,
    pub time: f32,
    /// Degrees added to the setup rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f32>,
    /// Offset added to the setup position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate: Option<Vec2>,
    /// Factor multiplied with the setup scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Vec2>,
    /// Curve towards the next key, applied to every key set by this op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
    /// `[x, y]` curves for the translate, scale and shear keys set by this op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curves: Option<Vec<Curve>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeySlot {
    pub animation: String,
    pub slot: String,
    pub time: f32,
    /// Attachment to show from `time`; `null` hides the slot.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub attachment: Option<Option<String>>,
    /// From `time`, show what lower tracks (or the setup pose) show.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
    /// `[r, g, b, a]` curves for the color key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curves: Option<Vec<Curve>>,
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D, T>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Parses either a single op object or an array of ops.
pub fn parse_ops(json: &str) -> Result<Vec<Op>> {
    // Parse the shape first and then each op, so errors name the failing op
    // (an untagged enum would swallow serde's message).
    let value: serde_json::Value = serde_json::from_str(json)?;
    let ops = match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(i, v)| serde_json::from_value(v).map_err(|e| Error::op(i, e.to_string())))
            .collect::<Result<Vec<Op>>>()?,
        v => vec![serde_json::from_value(v).map_err(|e| Error::op(0, e.to_string()))?],
    };
    Ok(ops)
}

/// Applies `ops` to a copy of `doc` and returns it if every op succeeded and
/// the result validates.
pub fn apply(doc: &Document, ops: &[Op]) -> Result<Document> {
    let mut next = doc.clone();
    for (i, op) in ops.iter().enumerate() {
        apply_one(&mut next, op).map_err(|m| Error::op(i, m))?;
    }
    sort_bones(&mut next);
    recenter_binds(&mut next);
    let issues = validate(&next);
    if !issues.is_empty() {
        return Err(Error::Invalid(issues));
    }
    Ok(next)
}

/// Expands external operations against the current staged document, then
/// applies the flattened core batch atomically. The callback may inspect
/// intermediate state; only a validated final document is returned.
pub fn apply_expanded<T>(
    doc: &Document,
    ops: &[T],
    mut expand: impl FnMut(&Document, &T) -> std::result::Result<Vec<Op>, String>,
) -> Result<(Document, Vec<Op>)> {
    let mut next = doc.clone();
    let mut flattened = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        sort_bones(&mut next);
        let expanded = expand(&next, op).map_err(|message| Error::op(i, message))?;
        for core in &expanded { apply_one(&mut next, core).map_err(|message| Error::op(i, message))?; }
        flattened.extend(expanded);
    }
    sort_bones(&mut next);
    recenter_binds(&mut next);
    let issues = validate(&next);
    if !issues.is_empty() { return Err(Error::Invalid(issues)); }
    Ok((next, flattened))
}

type OpResult = std::result::Result<(), String>;

/// Weight edits can unbalance per-bone binds, which would move a vertex away
/// from its stored setup position.
fn recenter_binds(doc: &mut Document) {
    for_each_mesh(doc, |m| {
        for v in &mut m.vertices {
            if v.weights.iter().all(|w| w.offset == [0.0, 0.0]) { continue; }
            let total: f32 = v.weights.iter().map(|w| w.weight.max(0.0)).sum();
            if total <= 0.0 { continue; }
            let mean = [0, 1].map(|c| v.weights.iter().map(|w| w.offset[c] * w.weight.max(0.0)).sum::<f32>() / total);
            if mean[0].abs().max(mean[1].abs()) < 1e-3 { continue; }
            for w in &mut v.weights {
                w.offset = [w.offset[0] - mean[0], w.offset[1] - mean[1]];
            }
        }
    });
}

fn apply_one(doc: &mut Document, op: &Op) -> OpResult {
    match op {
        Op::SetName { name } => doc.name = name.clone(),
        Op::AddBone(bone) => {
            if bone.name == ROOT && bone.parent.is_some() { return Err("the root bone cannot have a parent".into()); }
            // A new document already has the root, so scripts that start by
            // adding it set its values instead.
            if bone.name == ROOT && let Some(i) = doc.bone_index(ROOT) {
                doc.bones[i] = bone.clone();
                return Ok(());
            }
            if doc.bone(&bone.name).is_some() {
                return Err(format!("bone {:?} already exists", bone.name));
            }
            if let Some(p) = &bone.parent {
                bone_exists(doc, p)?;
            }
            let mut bone = bone.clone();
            if bone.parent.is_none() && bone.name != ROOT && doc.bone(ROOT).is_some() { bone.parent = Some(ROOT.into()); }
            doc.bones.push(bone);
        }
        Op::AddBoneWorld { name, parent, start, end } => {
            let under_root = parent.is_none() && name != ROOT && doc.bone(ROOT).is_some();
            add_bone_world(doc, name, if under_root { Some(ROOT) } else { parent.as_deref() }, *start, *end)?
        },
        Op::SetBone(p) => set_bone(doc, p)?,
        Op::RemoveBone { name, recursive } => remove_bone(doc, name, *recursive)?,
        Op::AddSlot { slot, index } => {
            if doc.slot(&slot.name).is_some() {
                return Err(format!("slot {:?} already exists", slot.name));
            }
            bone_exists(doc, &slot.bone)?;
            let i = index.unwrap_or(doc.slots.len()).min(doc.slots.len());
            doc.slots.insert(i, slot.clone());
        }
        Op::SetSlot(p) => set_slot(doc, p)?,
        Op::RemoveSlot { name } => {
            let i = slot_index(doc, name)?;
            doc.slots.remove(i);
            for anim in doc.animations.values_mut() {
                anim.slots.remove(name);
                for slots in anim.deform.values_mut() { slots.remove(name); }
                for order in anim.draw_order.iter_mut().filter_map(|k| k.order.as_mut()) { order.retain(|s| s != name); }
            }
            for skin in &mut doc.skins {
                skin.attachments.remove(name);
            }
        }
        Op::AddSkin { .. } | Op::SetSkin { .. } | Op::RemoveSkin { .. } | Op::SetSkinAttachment { .. } | Op::RemoveSkinAttachment { .. } => skin_op(doc, op)?,
        Op::AddLinkedMesh { slot, name, parent, parent_skin, skin, image, timelines } => {
            add_linked_mesh(doc, slot, skin.as_deref(), LinkedMesh {
                name: name.clone(), image: image.clone(), parent: parent.clone(),
                parent_skin: parent_skin.clone(), timelines: *timelines,
            })?;
        }
        Op::MoveSlot { name, index } => {
            let i = slot_index(doc, name)?;
            let slot = doc.slots.remove(i);
            let to = (*index).min(doc.slots.len());
            doc.slots.insert(to, slot);
        }
        Op::SetAttachment { slot, attachment } => {
            let i = slot_index(doc, slot)?;
            let atts = &mut doc.slots[i].attachments;
            match atts.iter().position(|a| a.name() == attachment.name()) {
                Some(j) => atts[j] = attachment.clone(),
                None => atts.push(attachment.clone()),
            }
            fit_deform_keys(doc, slot, attachment, None);
            fit_linked_deform_keys(doc, slot, attachment.name(), None);
        }
        Op::RenameAttachment { slot, from, to, skin } => rename_attachment(doc, slot, from, to, skin.as_deref(), false)?,
        Op::RenamePlaceholder { slot, from, to } => rename_attachment(doc, slot, from, to, None, true)?,
        Op::RemoveAttachment { slot, name, on_parent_removed } => {
            let i = slot_index(doc, slot)?;
            remove_linked_dependents(doc, slot, name, None, *on_parent_removed, None)?;
            let s = &mut doc.slots[i];
            let before = s.attachments.len();
            s.attachments.retain(|a| a.name() != name);
            if s.attachments.len() == before {
                return Err(format!("attachment {name:?} does not exist in slot {slot:?}"));
            }
            remove_deform(doc, "default", slot, name);
            clean_placeholder(doc, slot, name);
        }
        Op::AddAnimation { name, duration } => {
            if doc.animations.contains_key(name) {
                return Err(format!("animation {name:?} already exists"));
            }
            let anim = doc.animations.entry(name.clone()).or_default();
            anim.duration = *duration;
        }
        Op::DuplicateAnimation { name, new_name } => {
            if doc.animations.contains_key(new_name) {
                return Err(format!("animation {new_name:?} already exists"));
            }
            let animation = doc.animations.get(name)
                .ok_or_else(|| format!("animation {name:?} does not exist"))?.clone();
            doc.animations.insert(new_name.clone(), animation);
        }
        Op::SetAnimation { name, rename, duration } => {
            let mut anim = doc
                .animations
                .remove(name)
                .ok_or_else(|| format!("animation {name:?} does not exist"))?;
            if let Some(d) = duration {
                anim.duration = *d;
            }
            let key = rename.clone().unwrap_or_else(|| name.clone());
            if doc.animations.contains_key(&key) {
                return Err(format!("animation {key:?} already exists"));
            }
            doc.animations.insert(key, anim);
        }
        Op::RemoveAnimation { name } => {
            doc.animations
                .remove(name)
                .ok_or_else(|| format!("animation {name:?} does not exist"))?;
        }
        Op::KeyBone(k) => key_bone(doc, k)?,
        Op::KeySlot(k) => key_slot(doc, k)?,
        Op::KeyDeform { animation, slot, attachment, skin, time, offsets, curve } => {
            check_time(*time)?;
            let si = slot_index(doc, slot)?;
            let source = match skin { Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, attachment)), None => doc.slots[si].attachment(attachment) };
            let Some(mesh) = source.and_then(|a| doc.materialize_attachment(si, a)) else {
                return Err(format!("attachment {attachment:?} in slot {slot:?} is not a mesh"));
            };
            let Attachment::Mesh(mesh) = mesh else { return Err(format!("attachment {attachment:?} is not a mesh")); };
            if offsets.len() != mesh.vertices.len() {
                return Err(format!("{} offsets given, the mesh has {} vertices", offsets.len(), mesh.vertices.len()));
            }
            if offsets.iter().flatten().any(|c| !c.is_finite()) {
                return Err("offsets must be finite".into());
            }
            let (target_skin, target_name) = match source {
                Some(Attachment::LinkedMesh(link)) if link.timelines => (link.parent_skin.as_deref().unwrap_or("default").to_string(), link.parent.clone()),
                _ => (skin.as_deref().unwrap_or("default").to_string(), attachment.clone()),
            };
            let tl = doc.animations.entry(animation.clone()).or_default().deform.entry(target_skin).or_default().entry(slot.clone()).or_default();
            upsert(tl.entry(target_name).or_default(), *time, offsets.clone(), *curve);
        }
        Op::RemoveKey { animation, bone, slot, timeline, time, attachment, skin } => {
            if attachment.is_some() && *timeline != Timeline::Deform {
                return Err("attachment is only used with the deform timeline".into());
            }
            let inherited = if *timeline == Timeline::Deform {
                slot.as_deref().zip(attachment.as_deref()).and_then(|(s, a)| {
                    let source = match skin.as_deref() {
                        Some(k) => doc.skin(k).and_then(|owner| owner.attachment(s, a)),
                        None => doc.slot(s).and_then(|owner| owner.attachment(a)),
                    };
                    match source { Some(Attachment::LinkedMesh(link)) if link.timelines => Some((link.parent.clone(), link.parent_skin.clone())), _ => None }
                })
            } else { None };
            let target_attachment = inherited.as_ref().map(|(a, _)| a.as_str()).or(attachment.as_deref());
            let target_skin = inherited.as_ref().map(|(_, k)| k.as_deref()).unwrap_or(skin.as_deref());
            remove_keys(doc, animation, bone.as_deref(), slot.as_deref(), Some(*timeline), Some(*time), target_attachment, target_skin)?
        }
        Op::ClearTimeline { animation, bone, slot, timeline, skin } => {
            remove_keys(doc, animation, bone.as_deref(), slot.as_deref(), *timeline, None, None, skin.as_deref())?
        }
        Op::AddIk(c) => {
            if doc.has_constraint(&c.name) {
                return Err(format!("constraint {:?} already exists", c.name));
            }
            doc.ik.push(c.clone());
        }
        Op::AddPhysics(c) => {
            if doc.has_constraint(&c.name) {
                return Err(format!("constraint {:?} already exists", c.name));
            }
            doc.physics.push(c.clone());
        }
        Op::AddTransform(c) => {
            if doc.has_constraint(&c.name) {
                return Err(format!("constraint {:?} already exists", c.name));
            }
            doc.transforms.push(c.clone());
        }
        Op::SetTransform(p) => set_transform(doc, p)?,
        Op::RemoveTransform { name } => {
            let i = transform_index(doc, name)?;
            doc.transforms.remove(i);
            for anim in doc.animations.values_mut() {
                anim.transforms.remove(name);
            }
            for skin in &mut doc.skins {
                skin.constraints.retain(|c| c != name);
            }
        }
        Op::KeyTransform { animation, constraint, time, mix, curve } => {
            check_time(*time)?;
            transform_index(doc, constraint)?;
            if !mix.all_finite() { return Err("mix must be finite".into()); }
            let keys = doc.animations.entry(animation.clone()).or_default().transforms.entry(constraint.clone()).or_default();
            upsert(keys, *time, *mix, *curve);
        }
        Op::RemoveTransformKey { animation, constraint, time } => {
            let anim = doc.animations.get_mut(animation).ok_or_else(|| format!("animation {animation:?} does not exist"))?;
            let keys = anim.transforms.get_mut(constraint).ok_or_else(|| format!("constraint {constraint:?} has no keys in {animation:?}"))?;
            let before = keys.len();
            keys.retain(|k| (k.time - time).abs() >= TIME_EPSILON);
            if keys.len() == before { return Err(format!("no key at time {time}")); }
            if keys.is_empty() { anim.transforms.remove(constraint); }
        }
        Op::SetPhysics(p) => set_physics(doc, p)?,
        Op::RemovePhysics { name } => {
            let i = physics_index(doc, name)?;
            doc.physics.remove(i);
            for anim in doc.animations.values_mut() {
                anim.physics.remove(name);
            }
            for skin in &mut doc.skins {
                skin.constraints.retain(|c| c != name);
            }
        }
        Op::KeyPhysics { animation, constraint, property, time, value, curve } => {
            check_time(*time)?;
            if !value.is_finite() { return Err("value must be finite".into()); }
            if !constraint.is_empty() { physics_index(doc, constraint)?; }
            let timelines = doc.animations.entry(animation.clone()).or_default().physics.entry(constraint.clone()).or_default();
            upsert(property.keys(timelines), *time, *value, *curve);
        }
        Op::KeyPhysicsReset { animation, constraint, time } => {
            check_time(*time)?;
            let name = constraint.clone().unwrap_or_default();
            if !name.is_empty() { physics_index(doc, &name)?; }
            let reset = &mut doc.animations.entry(animation.clone()).or_default().physics.entry(name).or_default().reset;
            if !reset.iter().any(|t| (t - time).abs() < TIME_EPSILON) {
                reset.insert(reset.partition_point(|t| t < time), *time);
            }
        }
        Op::RemovePhysicsKey { animation, constraint, property, time } => {
            let name = constraint.clone().unwrap_or_default();
            let anim = doc.animations.get_mut(animation).ok_or_else(|| format!("animation {animation:?} does not exist"))?;
            let timelines = anim.physics.get_mut(&name).ok_or_else(|| format!("constraint {name:?} has no keys in {animation:?}"))?;
            let removed = match property {
                Some(property) => { let keys = property.keys(timelines); let n = keys.len(); keys.retain(|k| (k.time - time).abs() >= TIME_EPSILON); keys.len() != n }
                None => { let n = timelines.reset.len(); timelines.reset.retain(|t| (t - time).abs() >= TIME_EPSILON); timelines.reset.len() != n }
            };
            if !removed { return Err(format!("no key at time {time}")); }
            if timelines.is_empty() { anim.physics.remove(&name); }
        }
        Op::SetPhysicsGlobals(p) => {
            let g = &mut doc.physics_globals;
            for (dst, src) in [(&mut g.inertia, p.inertia), (&mut g.strength, p.strength), (&mut g.damping, p.damping), (&mut g.mass_inverse, p.mass_inverse), (&mut g.wind, p.wind), (&mut g.gravity, p.gravity), (&mut g.mix, p.mix)] {
                if let Some(v) = src { *dst = v; }
            }
        }
        Op::SetIk(p) => set_ik(doc, p)?,
        Op::SetConstraintOrder { names } => set_constraint_order(doc, names)?,
        Op::RemoveIk { name } => {
            let i = ik_index(doc, name)?;
            doc.ik.remove(i);
            for anim in doc.animations.values_mut() {
                anim.ik.remove(name);
            }
            for skin in &mut doc.skins {
                skin.constraints.retain(|c| c != name);
            }
        }
        Op::KeyIk { animation, ik, time, mix, softness, bend_positive, compress, stretch, curve, curves } => {
            check_time(*time)?;
            ik_index(doc, ik)?;
            if mix.is_none() && softness.is_none() && bend_positive.is_none() && compress.is_none() && stretch.is_none() {
                return Err("give at least one of mix, softness, bend_positive, compress or stretch".into());
            }
            let keys = doc.animations.entry(animation.clone()).or_default().ik.entry(ik.clone()).or_default();
            let old = keys.iter().find(|k| (k.time - time).abs() < TIME_EPSILON).map(|k| k.value).unwrap_or_default();
            let value = crate::model::IkKey {
                mix: mix.or(old.mix),
                softness: softness.or(old.softness),
                bend_positive: bend_positive.or(old.bend_positive),
                compress: compress.or(old.compress),
                stretch: stretch.or(old.stretch),
            };
            upsert(keys, *time, value, *curve);
            if let Some(curves) = curves
                && let Some(key) = keys.iter_mut().find(|k| (k.time - time).abs() < TIME_EPSILON) {
                key.curves.clone_from(curves);
            }
        }
        Op::RemoveIkKey { animation, ik, time } => {
            let anim = doc
                .animations
                .get_mut(animation)
                .ok_or_else(|| format!("animation {animation:?} does not exist"))?;
            let keys = anim.ik.get_mut(ik).ok_or_else(|| format!("IK {ik:?} has no keys in {animation:?}"))?;
            let before = keys.len();
            keys.retain(|k| (k.time - time).abs() >= TIME_EPSILON);
            if keys.len() == before {
                return Err(format!("no key at time {time}"));
            }
            if keys.is_empty() {
                anim.ik.remove(ik);
            }
        }
        Op::KeyDrawOrder { animation, time, order } => {
            check_time(*time)?;
            if let Some(order) = order {
                let listed: BTreeSet<&String> = order.iter().collect();
                if let Some(s) = doc.slots.iter().find(|s| !listed.contains(&s.name)) {
                    return Err(format!("order must list every slot, {:?} is missing", s.name));
                }
                // Unknown or repeated names are reported by validation.
            }
            let keys = &mut doc.animations.entry(animation.clone()).or_default().draw_order;
            let key = crate::model::DrawOrderKey { time: *time, order: order.clone() };
            match keys.iter().position(|k| (k.time - time).abs() < TIME_EPSILON) {
                Some(i) => keys[i] = key,
                None => keys.insert(keys.partition_point(|k| k.time < *time), key),
            }
        }
        Op::RemoveDrawOrderKey { animation, time } => {
            let anim = doc.animations.get_mut(animation).ok_or_else(|| format!("animation {animation:?} does not exist"))?;
            let before = anim.draw_order.len();
            anim.draw_order.retain(|k| (k.time - time).abs() >= TIME_EPSILON);
            if anim.draw_order.len() == before { return Err(format!("no key at time {time}")); }
        }
        Op::AddEvent { name, int, float, string } => {
            if doc.events.contains_key(name) { return Err(format!("event {name:?} already exists")); }
            doc.events.insert(name.clone(), crate::model::EventDef { int: *int, float: *float, string: string.clone() });
        }
        Op::SetEvent { name, rename, int, float, string } => {
            let event = doc.events.get_mut(name).ok_or_else(|| format!("event {name:?} does not exist"))?;
            if let Some(v) = int { event.int = *v; }
            if let Some(v) = float { event.float = *v; }
            if let Some(v) = string { event.string.clone_from(v); }
            if let Some(new) = rename.as_ref().filter(|new| *new != name) {
                if doc.events.contains_key(new) { return Err(format!("event {new:?} already exists")); }
                if let Some(event) = doc.events.remove(name) { doc.events.insert(new.clone(), event); }
                for key in doc.animations.values_mut().flat_map(|a| &mut a.events).filter(|k| &k.name == name) {
                    key.name.clone_from(new);
                }
            }
        }
        Op::RemoveEvent { name } => {
            doc.events.remove(name).ok_or_else(|| format!("event {name:?} does not exist"))?;
            for anim in doc.animations.values_mut() { anim.events.retain(|k| &k.name != name); }
        }
        Op::KeyEvent { animation, name, time, int, float, string } => {
            check_time(*time)?;
            if !doc.events.contains_key(name) { return Err(format!("event {name:?} does not exist")); }
            let keys = &mut doc.animations.entry(animation.clone()).or_default().events;
            let key = crate::model::EventKey { time: *time, name: name.clone(), int: *int, float: *float, string: string.clone() };
            match keys.iter().position(|k| &k.name == name && (k.time - time).abs() < TIME_EPSILON) {
                Some(i) => keys[i] = key,
                None => keys.insert(keys.partition_point(|k| k.time <= *time), key),
            }
        }
        Op::RemoveEventKey { animation, name, time } => {
            let anim = doc.animations.get_mut(animation).ok_or_else(|| format!("animation {animation:?} does not exist"))?;
            let before = anim.events.len();
            anim.events.retain(|k| &k.name != name || (k.time - time).abs() >= TIME_EPSILON);
            if anim.events.len() == before { return Err(format!("no key at time {time}")); }
        }
        Op::MeshFromRegion { slot, region, name, cols, rows } => {
            mesh_from_region(doc, slot, region, name.as_deref(), *cols, *rows)?
        }
        Op::AutoWeights { slot, attachment, bones, max_influences, falloff, vertices, skin, locked_bones } => {
            weight_edit(doc, (slot, attachment, skin.as_deref()), vertices.as_deref(), locked_bones, |doc, at| {
                auto_weights(doc, at, bones.as_deref(), *max_influences, *falloff, vertices.as_deref())
            })?;
        }
        Op::TransferWeights { slot, attachment, from_slot, from_attachment, vertices, skin, from_skin, locked_bones } => {
            weight_edit(doc, (slot, attachment, skin.as_deref()), vertices.as_deref(), locked_bones, |doc,at| transfer_weights(doc, at, (from_slot, from_attachment, from_skin.as_deref()), vertices.as_deref()))?;
        }
        Op::MirrorWeights { slot, attachment, axis, source, skin, locked_bones } => {
            if !axis.is_finite() { return Err("axis must be finite".into()); }
            weight_edit(doc, (slot, attachment, skin.as_deref()), None, locked_bones, |doc,at| mirror_weights(doc,at,*axis,*source))?;
        }
        Op::SmoothWeights { slot, attachment, skin, vertices, factor, iterations, locked_bones } => {
            if !factor.is_finite() || !(0.0..=1.0).contains(factor) || !(1..=100).contains(iterations) { return Err("factor must be in 0..=1; iterations in 1..=100".into()); }
            weight_edit(doc,(slot,attachment,skin.as_deref()),vertices.as_deref(),locked_bones,|doc,at| {
                let bone=doc.slot(slot).unwrap().bone.clone();
                let mesh=mesh_mut(doc,at)?;
                for _ in 0..*iterations { *mesh=crate::mesh::smooth_weights(mesh,&bone,vertices.as_deref().unwrap_or(&[]),&locked_bones.iter().cloned().collect(),*factor); }
                Ok(())
            })?;
        }
        Op::PruneWeights { slot, attachment, skin, vertices, max_bones, threshold, locked_bones } => {
            if !(1..=8).contains(max_bones) || !threshold.is_finite() || !(0.0..=1.0).contains(threshold) { return Err("max_bones must be in 1..=8; threshold in 0..=1".into()); }
            weight_edit(doc,(slot,attachment,skin.as_deref()),vertices.as_deref(),locked_bones,|doc,at| {
                let bone=doc.slot(slot).unwrap().bone.clone();
                let mesh=mesh_mut(doc,at)?;
                *mesh=crate::mesh::prune_weights(mesh,&bone,vertices.as_deref().unwrap_or(&[]),*max_bones,*threshold,&locked_bones.iter().cloned().collect());
                Ok(())
            })?;
        }
        Op::SwapWeights { slot, attachment, skin, vertices, a, b, locked_bones } => {
            bone_exists(doc,a)?; bone_exists(doc,b)?;
            weight_edit(doc,(slot,attachment,skin.as_deref()),vertices.as_deref(),locked_bones,|doc,at| {
                if locked_bones.contains(a) || locked_bones.contains(b) { return Ok(()); }
                let bone=doc.slot(slot).unwrap().bone.clone(); let mesh=mesh_mut(doc,at)?;
                *mesh=crate::mesh::swap_weights(mesh,&bone,vertices.as_deref().unwrap_or(&[]),a,b); Ok(())
            })?;
        }
        Op::SetBoneWeight { slot, attachment, skin, vertices, bone, value, mode, locked_bones } => {
            bone_exists(doc,bone)?;
            if !value.is_finite() || (*mode != WeightMode::Add && *value < 0.0) { return Err("weight value must be finite and nonnegative except add".into()); }
            weight_edit(doc,(slot,attachment,skin.as_deref()),Some(vertices),locked_bones,|doc,at| {
                let slot_bone=doc.slot(slot).unwrap().bone.clone();
                let fallback=if bone!=&slot_bone { Some(slot_bone.clone()) } else { doc.bone(bone).and_then(|b|b.parent.clone()) };
                let mesh=mesh_mut(doc,at)?;
                for &i in vertices {
                    let v=&mesh.vertices[i]; let cur=crate::mesh::weight_map(v,&slot_bone).get(bone).copied().unwrap_or(0.0);
                    let target=match mode { WeightMode::Set=>*value,WeightMode::Add=>cur+value,WeightMode::Scale=>cur*value };
                    if !target.is_finite() { return Err("weight result must be finite".into()); }
                    mesh.vertices[i].weights=crate::mesh::set_weight(v,bone,target,&slot_bone,&locked_bones.iter().cloned().collect(),fallback.as_deref());
                }
                Ok(())
            })?;
        }
        Op::BindBones { slot, attachment, skin, add, remove, redistribute } => bind_bones(doc,(slot,attachment,skin.as_deref()),add,remove,*redistribute)?,
        Op::NormalizeWeights { slot, attachment, skin } => {
            weight_edit(doc,(slot,attachment,skin.as_deref()),None,&[],|doc,at| {
                let bone=doc.slot(slot).unwrap().bone.clone(); let mesh=mesh_mut(doc,at)?;
                *mesh=crate::mesh::normalize_weights(mesh,&bone); Ok(())
            })?;
        }
        Op::SetVertices { slot, attachment, vertices, skin } => set_vertices(doc, (slot, attachment, skin.as_deref()), vertices)?,
        Op::AddMeshVertices { slot, attachment, skin, points, on_edge_tolerance } => {
            topology_edit(doc, (slot, attachment, skin.as_deref()), |m,b| crate::mesh::add_vertices(m, points, *on_edge_tolerance, b))?;
        }
        Op::RemoveMeshVertices { slot, attachment, skin, indices } => {
            topology_edit(doc, (slot, attachment, skin.as_deref()), |m,_| crate::mesh::remove_vertices(m, indices))?;
        }
        Op::SetMeshEdges { slot, attachment, skin, edges } => {
            topology_edit(doc, (slot, attachment, skin.as_deref()), |m,_| {
                let mut m = m.clone(); m.edges = edges.clone(); crate::mesh::triangulate(&m)
            })?;
        }
        Op::GenerateMeshVertices { slot, attachment, skin, count, spacing } => {
            topology_edit(doc, (slot, attachment, skin.as_deref()), |m,b| crate::mesh::generate_interior(m, b, *count, *spacing))?;
        }
        Op::ResetMesh { slot, attachment, skin, mode } => {
            topology_edit(doc, (slot, attachment, skin.as_deref()), |m,_| match mode {
                MeshResetMode::VerticesToUvs => crate::mesh::reset_to_uvs(m),
                MeshResetMode::UvsToVertices => crate::mesh::reset_uvs(m),
            })?;
        }
    }
    Ok(())
}

/// Keeps deform keys valid after an attachment is set: dropped when it is no
/// longer a mesh, otherwise cut or zero-padded to its vertex count (an
/// appended vertex starts undeformed).
fn fit_deform_keys(doc: &mut Document, slot: &str, attachment: &Attachment, skin: Option<&str>) {
    let count = match attachment {
        Attachment::Mesh(m) => Some(m.vertices.len()),
        // Shared timelines read the parent's path, so any own keys are stale.
        Attachment::LinkedMesh(l) if l.timelines => None,
        Attachment::LinkedMesh(l) => {
            let parent = match l.parent_skin.as_deref() {
                Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &l.parent)),
                None => doc.slot(slot).and_then(|s| s.attachment(&l.parent)),
            };
            match parent { Some(Attachment::Mesh(m)) => Some(m.vertices.len()), _ => None }
        }
        Attachment::Region(_) => None,
    };
    for anim in doc.animations.values_mut() {
        let Some(tl) = anim.deform.get_mut(skin.unwrap_or("default")).and_then(|s| s.get_mut(slot)) else { continue };
        match count {
            None => {
                tl.remove(attachment.name());
            }
            Some(n) => {
                for k in tl.get_mut(attachment.name()).into_iter().flatten() {
                    k.value.resize(n, [0.0, 0.0]);
                }
            }
        }
        if matches!(attachment, Attachment::LinkedMesh(_)) { prune_deform(anim); }
    }
}

/// A linked mesh and the skin that owns it (`None` is the default skin).
pub struct LinkedChild {
    pub owner: Option<String>,
    pub name: String,
    pub timelines: bool,
}

/// Linked meshes in `slot` whose parent is the attachment `parent` of `parent_skin`.
pub fn linked_children(doc: &Document, slot: &str, parent: &str, parent_skin: Option<&str>) -> Vec<LinkedChild> {
    let default = doc.slot(slot).map(|s| (None, &s.attachments));
    let skins = doc.skins.iter().filter_map(|k| k.attachments.get(slot).map(|a| (Some(k.name.clone()), a)));
    default.into_iter().chain(skins).flat_map(|(owner, atts)| atts.iter().filter_map(move |a| match a {
        Attachment::LinkedMesh(l) if l.parent == parent && l.parent_skin.as_deref() == parent_skin =>
            Some(LinkedChild { owner: owner.clone(), name: l.name.clone(), timelines: l.timelines }),
        _ => None,
    })).collect()
}

fn fit_linked_deform_keys(doc: &mut Document, slot: &str, parent: &str, parent_skin: Option<&str>) {
    for child in linked_children(doc, slot, parent, parent_skin) {
        let owner = child.owner.as_deref();
        let att = match owner { Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &child.name)), None => doc.slot(slot).and_then(|s| s.attachment(&child.name)) }.cloned();
        if let Some(att) = att { fit_deform_keys(doc, slot, &att, owner); }
    }
}

fn bone_exists(doc: &Document, name: &str) -> OpResult {
    doc.bone(name).map(|_| ()).ok_or_else(|| format!("bone {name:?} does not exist"))
}

fn slot_index(doc: &Document, name: &str) -> std::result::Result<usize, String> {
    doc.slot_index(name).ok_or_else(|| format!("slot {name:?} does not exist"))
}

fn set_bone(doc: &mut Document, p: &BonePatch) -> OpResult {
    let i = doc.bone_index(&p.name).ok_or_else(|| format!("bone {:?} does not exist", p.name))?;
    if p.name == ROOT {
        if p.rename.as_deref().is_some_and(|new| new != ROOT) { return Err("the root bone cannot be renamed".into()); }
        if matches!(&p.parent, Some(Some(_))) { return Err("the root bone cannot have a parent".into()); }
    } else if matches!(&p.parent, Some(None)) && doc.bone(ROOT).is_some() {
        return Err(format!("bone {:?} must stay under the root bone", p.name));
    }
    if let Some(Some(parent)) = &p.parent {
        bone_exists(doc, parent)?;
        if is_descendant_or_self(doc, parent, &p.name) {
            return Err(format!("parent {parent:?} would create a cycle"));
        }
    }
    let bone = &mut doc.bones[i];
    if let Some(parent) = &p.parent {
        bone.parent = parent.clone();
    }
    for (dst, src) in [
        (&mut bone.x, p.x),
        (&mut bone.y, p.y),
        (&mut bone.rotation, p.rotation),
        (&mut bone.scale_x, p.scale_x),
        (&mut bone.shear_x, p.shear_x),
        (&mut bone.shear_y, p.shear_y),
        (&mut bone.scale_y, p.scale_y),
        (&mut bone.length, p.length),
    ] {
        if let Some(v) = src {
            *dst = v;
        }
    }
    if let Some(inherit) = p.inherit { bone.inherit = inherit; }
    if let Some(c) = p.color {
        bone.color = c;
    }
    if let Some(r) = p.skin_required {
        bone.skin_required = r;
    }
    if let Some(new) = &p.rename {
        rename_bone(doc, &p.name, new)?;
    }
    Ok(())
}

fn skin_index(doc: &Document, name: &str) -> std::result::Result<usize, String> {
    doc.skins.iter().position(|s| s.name == name).ok_or_else(|| format!("skin {name:?} does not exist"))
}

/// Whether `slot` has an attachment (or a skin placeholder) called `name`.
pub fn slot_has_attachment(doc: &Document, slot: &str, name: &str) -> bool {
    doc.slot(slot).is_some_and(|s| s.attachment(name).is_some()) || doc.skins.iter().any(|k| k.attachment(slot, name).is_some())
}

fn skin_op(doc: &mut Document, op: &Op) -> OpResult {
    match op {
        Op::AddSkin { name } => {
            if name.is_empty() {
                return Err("skin name is empty".into());
            }
            if doc.skin(name).is_some() {
                return Err(format!("skin {name:?} already exists"));
            }
            doc.skins.push(skel2d_skin(name));
        }
        Op::SetSkin { name, rename, bones, ik, constraints } => {
            let i = skin_index(doc, name)?;
            if let Some(new) = rename
                && new != name
                && doc.skin(new).is_some()
            {
                return Err(format!("skin {new:?} already exists"));
            }
            for b in bones.iter().flatten() {
                bone_exists(doc, b)?;
            }
            for c in ik.iter().flatten().chain(constraints.iter().flatten()) {
                if !doc.has_constraint(c) { return Err(format!("constraint {c:?} does not exist")); }
            }
            if ik.is_some() && constraints.is_some() { return Err("give either ik or constraints".into()); }
            let skin = &mut doc.skins[i];
            if let Some(b) = bones {
                skin.bones = b.clone();
            }
            if let Some(c) = ik {
                skin.constraints = c.clone();
            }
            if let Some(c) = constraints {
                skin.constraints = c.clone();
            }
            if let Some(new) = rename {
                if new.is_empty() {
                    return Err("skin name is empty".into());
                }
                skin.name = new.clone();
                for owner in &mut doc.slots {
                    for a in &mut owner.attachments {
                        if let Attachment::LinkedMesh(l) = a && l.parent_skin.as_deref() == Some(name) { l.parent_skin = Some(new.clone()); }
                    }
                }
                for owner in &mut doc.skins {
                    for atts in owner.attachments.values_mut() {
                        for a in atts {
                            if let Attachment::LinkedMesh(l) = a && l.parent_skin.as_deref() == Some(name) { l.parent_skin = Some(new.clone()); }
                        }
                    }
                }
                for anim in doc.animations.values_mut() {
                    if let Some(tl) = anim.deform.remove(name) { anim.deform.insert(new.clone(), tl); }
                }
            }
        }
        Op::RemoveSkin { name, on_parent_removed } => {
            let i = skin_index(doc, name)?;
            let parents: Vec<_> = doc.skins[i].attachments.iter().flat_map(|(slot, atts)| atts.iter().filter(|a| matches!(a, Attachment::Mesh(_))).map(|a| (slot.clone(), a.name().to_string()))).collect();
            for (slot, parent) in parents {
                remove_linked_dependents(doc, &slot, &parent, Some(name), *on_parent_removed, Some(name))?;
            }
            doc.skins.remove(i);
            for anim in doc.animations.values_mut() { anim.deform.remove(name); }
            let names: Vec<_> = doc.slots.iter().map(|s| (s.name.clone(), s.attachment.clone())).collect();
            for (slot, attachment) in names {
                if let Some(a) = attachment { clean_placeholder(doc, &slot, &a); }
            }
            clean_missing_attachment_keys(doc);
        }
        Op::SetSkinAttachment { skin, slot, attachment } => {
            let i = skin_index(doc, skin)?;
            slot_index(doc, slot)?;
            let atts = doc.skins[i].attachments.entry(slot.clone()).or_default();
            match atts.iter().position(|a| a.name() == attachment.name()) {
                Some(j) => atts[j] = attachment.clone(),
                None => atts.push(attachment.clone()),
            }
            fit_deform_keys(doc, slot, attachment, Some(skin));
            fit_linked_deform_keys(doc, slot, attachment.name(), Some(skin));
        }
        Op::RemoveSkinAttachment { skin, slot, name, on_parent_removed } => {
            remove_linked_dependents(doc, slot, name, Some(skin), *on_parent_removed, None)?;
            let i = skin_index(doc, skin)?;
            let atts = doc.skins[i].attachments.get_mut(slot).ok_or_else(|| format!("skin {skin:?} has nothing for slot {slot:?}"))?;
            let before = atts.len();
            atts.retain(|a| a.name() != name);
            if atts.len() == before {
                return Err(format!("skin {skin:?} has no attachment {name:?} in slot {slot:?}"));
            }
            if atts.is_empty() {
                doc.skins[i].attachments.remove(slot);
            }
            remove_deform(doc, skin, slot, name);
            clean_placeholder(doc, slot, name);
        }
        _ => unreachable!("skin_op only takes skin ops"),
    }
    Ok(())
}

fn skel2d_skin(name: &str) -> crate::model::Skin {
    crate::model::Skin { name: name.to_string(), ..Default::default() }
}

fn add_linked_mesh(doc: &mut Document, slot: &str, skin: Option<&str>, link: LinkedMesh) -> OpResult {
    let si = slot_index(doc, slot)?;
    if link.name.trim().is_empty() { return Err("attachment name is empty".into()); }
    if let Some(k) = skin { skin_index(doc, k)?; }
    if let Some(k) = &link.parent_skin { skin_index(doc, k)?; }
    let parent_att = match link.parent_skin.as_deref() {
        Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &link.parent)),
        None => doc.slots[si].attachment(&link.parent),
    };
    if !matches!(parent_att, Some(Attachment::Mesh(_))) { return Err(format!("parent {:?} is not an independent mesh", link.parent)); }
    let target = match skin { Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &link.name)), None => doc.slots[si].attachment(&link.name) };
    if target.is_some() { return Err(format!("attachment {:?} already exists", link.name)); }
    let linked = Attachment::LinkedMesh(link);
    match skin {
        Some(k) => { let i = skin_index(doc, k)?; doc.skins[i].attachments.entry(slot.into()).or_default().push(linked); }
        None => doc.slots[si].attachments.push(linked),
    }
    Ok(())
}

fn remove_linked_dependents(doc: &mut Document, slot: &str, parent: &str, parent_skin: Option<&str>, policy: ParentRemoval, skip_owner: Option<&str>) -> OpResult {
    let si = slot_index(doc, slot)?;
    let mut refs = Vec::new();
    for (owner, atts) in std::iter::once((None, &doc.slots[si].attachments)).chain(doc.skins.iter().filter_map(|s| s.attachments.get(slot).map(|a| (Some(s.name.as_str()), a)))) {
        for a in atts {
            if let Attachment::LinkedMesh(l) = a
                && l.parent == parent && l.parent_skin.as_deref() == parent_skin && owner != skip_owner {
                refs.push((owner.map(str::to_string), l.name.clone()));
            }
        }
    }
    if !refs.is_empty() && policy == ParentRemoval::Error { return Err(format!("mesh {parent:?} has linked children; choose on_parent_removed")); }
    for (owner, name) in refs {
        let link = match owner.as_deref() { Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &name)), None => doc.slots[si].attachment(&name) };
        let link = match link { Some(Attachment::LinkedMesh(l)) => l.clone(), _ => continue };
        let replacement = if policy == ParentRemoval::Freeze {
            let att = match owner.as_deref() { Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &name)), None => doc.slots[si].attachment(&name) }.unwrap();
            doc.materialize_attachment(si, att)
        } else { None };
        let owner_index = owner.as_deref().map(|k| skin_index(doc, k)).transpose()?;
        let atts = match owner_index { Some(i) => doc.skins[i].attachments.get_mut(slot).unwrap(), None => &mut doc.slots[si].attachments };
        if let Some(i) = atts.iter().position(|a| a.name() == name) {
            if let Some(mesh) = replacement {
                atts[i] = mesh;
                if link.timelines {
                    for anim in doc.animations.values_mut() {
                        let inherited = anim.deform.get(link.parent_skin.as_deref().unwrap_or("default"))
                            .and_then(|slots| slots.get(slot)).and_then(|atts| atts.get(parent)).cloned();
                        if let Some(keys) = inherited {
                            anim.deform.entry(owner.as_deref().unwrap_or("default").into()).or_default()
                                .entry(slot.into()).or_default().insert(name.clone(), keys);
                        }
                    }
                }
            }
            else { atts.remove(i); remove_deform(doc, owner.as_deref().unwrap_or("default"), slot, &name); clean_placeholder(doc, slot, &name); }
        }
    }
    Ok(())
}

fn prune_deform(anim: &mut crate::model::Animation) {
    for slots in anim.deform.values_mut() {
        for atts in slots.values_mut() { atts.retain(|_, keys| !keys.is_empty()); }
        slots.retain(|_, atts| !atts.is_empty());
    }
    anim.deform.retain(|_, slots| !slots.is_empty());
}

fn remove_deform(doc: &mut Document, skin: &str, slot: &str, attachment: &str) {
    for anim in doc.animations.values_mut() {
        if let Some(atts) = anim.deform.get_mut(skin).and_then(|slots| slots.get_mut(slot)) { atts.remove(attachment); }
        prune_deform(anim);
    }
}

fn clean_placeholder(doc: &mut Document, slot: &str, name: &str) {
    if slot_has_attachment(doc, slot, name) { return; }
    if let Some(s) = doc.slots.iter_mut().find(|s| s.name == slot)
        && s.attachment.as_deref() == Some(name) { s.attachment = None; }
    for anim in doc.animations.values_mut() {
        if let Some(tl) = anim.slots.get_mut(slot) { tl.attachment.retain(|k| k.name.as_deref() != Some(name)); }
    }
}

fn clean_missing_attachment_keys(doc: &mut Document) {
    let missing: Vec<_> = doc.animations.values().flat_map(|a| a.slots.iter())
        .flat_map(|(s, tl)| tl.attachment.iter().filter_map(|k| k.name.as_ref().map(|n| (s.clone(), n.clone()))))
        .filter(|(s, n)| !slot_has_attachment(doc, s, n)).collect();
    for (s, n) in missing { clean_placeholder(doc, &s, &n); }
}

fn rename_attachment(doc: &mut Document, slot: &str, from: &str, to: &str, skin: Option<&str>, all: bool) -> OpResult {
    let si = slot_index(doc, slot)?;
    if to.trim().is_empty() { return Err("attachment name is empty".into()); }
    if let Some(k) = skin { skin_index(doc, k)?; }
    let mut owners = Vec::new();
    if (all || skin.is_none()) && doc.slots[si].attachment(from).is_some() { owners.push(None); }
    for k in &doc.skins {
        if (all || skin == Some(k.name.as_str())) && k.attachment(slot, from).is_some() { owners.push(Some(k.name.clone())); }
    }
    if owners.is_empty() { return Err(format!("attachment {from:?} does not exist in the selected skin")); }
    if from == to { return Ok(()); }
    if all && slot_has_attachment(doc, slot, to) { return Err(format!("placeholder {to:?} already exists")); }
    for owner in &owners {
        let target = match owner { None => doc.slots[si].attachment(to), Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, to)) };
        if target.is_some() { return Err(format!("attachment {to:?} already exists")); }
    }
    // Spine과 다름: local and all-skin placeholder renames are explicit ops.
    for owner in owners {
        let atts = match &owner {
            None => &mut doc.slots[si].attachments,
            Some(k) => doc.skins.iter_mut().find(|s| &s.name == k).unwrap().attachments.get_mut(slot).unwrap(),
        };
        match atts.iter_mut().find(|a| a.name() == from).unwrap() {
            Attachment::Region(r) => r.name = to.to_string(),
            Attachment::Mesh(m) => m.name = to.to_string(),
            Attachment::LinkedMesh(m) => m.name = to.to_string(),
        }
        for anim in doc.animations.values_mut() {
            if let Some(atts) = anim.deform.get_mut(owner.as_deref().unwrap_or("default")).and_then(|s| s.get_mut(slot))
                && let Some(keys) = atts.remove(from) { atts.insert(to.to_string(), keys); }
        }
        for a in doc.slots[si].attachments.iter_mut().chain(doc.skins.iter_mut().flat_map(|s| s.attachments.get_mut(slot).into_iter().flatten())) {
            if let Attachment::LinkedMesh(link) = a
                && link.parent == from && link.parent_skin == owner {
                link.parent = to.to_string();
            }
        }
    }
    // Setup and attachment keys name a placeholder shared by every skin; while
    // another owner still fills `from`, they keep showing that owner's attachment.
    if slot_has_attachment(doc, slot, from) { return Ok(()); }
    if doc.slots[si].attachment.as_deref() == Some(from) { doc.slots[si].attachment = Some(to.to_string()); }
    for anim in doc.animations.values_mut() {
        if let Some(tl) = anim.slots.get_mut(slot) {
            for key in &mut tl.attachment {
                if key.name.as_deref() == Some(from) { key.name = Some(to.to_string()); }
            }
        }
    }
    Ok(())
}

fn is_descendant_or_self(doc: &Document, candidate: &str, ancestor: &str) -> bool {
    let mut cur = Some(candidate);
    // Bounded walk in case the document already contains a cycle.
    for _ in 0..=doc.bones.len() {
        match cur {
            Some(name) if name == ancestor => return true,
            Some(name) => cur = doc.bone(name).and_then(|b| b.parent.as_deref()),
            None => return false,
        }
    }
    true
}

fn rename_bone(doc: &mut Document, old: &str, new: &str) -> OpResult {
    if old == new {
        return Ok(());
    }
    if doc.bone(new).is_some() {
        return Err(format!("bone {new:?} already exists"));
    }
    for bone in &mut doc.bones {
        if bone.name == old {
            bone.name = new.to_string();
        }
        if bone.parent.as_deref() == Some(old) {
            bone.parent = Some(new.to_string());
        }
    }
    for slot in &mut doc.slots {
        if slot.bone == old {
            slot.bone = new.to_string();
        }
    }
    for anim in doc.animations.values_mut() {
        if let Some(tl) = anim.bones.remove(old) {
            anim.bones.insert(new.to_string(), tl);
        }
    }
    for c in &mut doc.ik {
        for b in c.bones.iter_mut().chain(std::iter::once(&mut c.target)) {
            if b == old {
                *b = new.to_string();
            }
        }
    }
    for c in &mut doc.physics {
        if c.bone == old {
            c.bone = new.to_string();
        }
    }
    for c in &mut doc.transforms {
        for b in c.bones.iter_mut().chain(std::iter::once(&mut c.source)) {
            if b == old {
                *b = new.to_string();
            }
        }
    }
    for b in doc.skins.iter_mut().flat_map(|s| &mut s.bones) {
        if b == old {
            *b = new.to_string();
        }
    }
    for_each_mesh(doc, |m| {
        for b in &mut m.bones { if b == old { *b = new.to_string(); } }
        for w in m.vertices.iter_mut().flat_map(|v| &mut v.weights) {
            if w.bone == old {
                w.bone = new.to_string();
            }
        }
    });
    Ok(())
}

/// Every mesh, in slots and in skins.
fn for_each_mesh(doc: &mut Document, mut f: impl FnMut(&mut Mesh)) {
    let skins = doc.skins.iter_mut().flat_map(|k| k.attachments.values_mut().flatten());
    for att in doc.slots.iter_mut().flat_map(|s| &mut s.attachments).chain(skins) {
        if let Attachment::Mesh(m) = att {
            f(m);
        }
    }
}

fn ik_index(doc: &Document, name: &str) -> std::result::Result<usize, String> {
    doc.ik.iter().position(|c| c.name == name).ok_or_else(|| format!("IK constraint {name:?} does not exist"))
}

fn physics_index(doc: &Document, name: &str) -> std::result::Result<usize, String> {
    doc.physics.iter().position(|c| c.name == name).ok_or_else(|| format!("physics constraint {name:?} does not exist"))
}

fn transform_index(doc: &Document, name: &str) -> std::result::Result<usize, String> {
    doc.transforms.iter().position(|c| c.name == name).ok_or_else(|| format!("transform constraint {name:?} does not exist"))
}

fn set_transform(doc: &mut Document, p: &TransformPatch) -> OpResult {
    let i = transform_index(doc, &p.name)?;
    if let Some(new) = &p.rename
        && new != &p.name && doc.has_constraint(new) {
            return Err(format!("constraint {new:?} already exists"));
        }
    let c = &mut doc.transforms[i];
    if let Some(order) = p.order { c.order = order; }
    if let Some(b) = &p.bones { c.bones = b.clone(); }
    if let Some(s) = &p.source { c.source = s.clone(); }
    for (dst, src) in [
        (&mut c.mix_rotate, p.mix_rotate), (&mut c.mix_x, p.mix_x), (&mut c.mix_y, p.mix_y), (&mut c.mix_scale_x, p.mix_scale_x), (&mut c.mix_scale_y, p.mix_scale_y), (&mut c.mix_shear_y, p.mix_shear_y),
        (&mut c.offset_rotation, p.offset_rotation), (&mut c.offset_x, p.offset_x), (&mut c.offset_y, p.offset_y), (&mut c.offset_scale_x, p.offset_scale_x), (&mut c.offset_scale_y, p.offset_scale_y), (&mut c.offset_shear_y, p.offset_shear_y),
    ] {
        if let Some(v) = src { *dst = v; }
    }
    if let Some(v) = p.local { c.local = v; }
    if let Some(v) = p.relative { c.relative = v; }
    if let Some(new) = &p.rename {
        c.name = new.clone();
        for anim in doc.animations.values_mut() {
            if let Some(keys) = anim.transforms.remove(&p.name) { anim.transforms.insert(new.clone(), keys); }
        }
        for n in doc.skins.iter_mut().flat_map(|s| &mut s.constraints) {
            if *n == p.name { *n = new.clone(); }
        }
    }
    Ok(())
}

fn set_physics(doc: &mut Document, p: &PhysicsPatch) -> OpResult {
    let i = physics_index(doc, &p.name)?;
    if let Some(new) = &p.rename
        && new != &p.name && doc.has_constraint(new) {
            return Err(format!("constraint {new:?} already exists"));
        }
    if let Some(b) = &p.bone { bone_exists(doc, b)?; }
    let c = &mut doc.physics[i];
    if let Some(order) = p.order { c.order = order; }
    if let Some(b) = &p.bone { c.bone = b.clone(); }
    for (dst, src) in [
        (&mut c.x, p.x), (&mut c.y, p.y), (&mut c.rotate, p.rotate), (&mut c.scale_x, p.scale_x), (&mut c.shear_x, p.shear_x),
        (&mut c.limit, p.limit), (&mut c.step, p.step), (&mut c.inertia, p.inertia), (&mut c.strength, p.strength), (&mut c.damping, p.damping),
        (&mut c.mass_inverse, p.mass_inverse), (&mut c.wind, p.wind), (&mut c.gravity, p.gravity), (&mut c.mix, p.mix),
    ] {
        if let Some(v) = src { *dst = v; }
    }
    for (dst, src) in [
        (&mut c.inertia_global, p.inertia_global), (&mut c.strength_global, p.strength_global), (&mut c.damping_global, p.damping_global),
        (&mut c.mass_global, p.mass_global), (&mut c.wind_global, p.wind_global), (&mut c.gravity_global, p.gravity_global), (&mut c.mix_global, p.mix_global),
    ] {
        if let Some(v) = src { *dst = v; }
    }
    if let Some(new) = &p.rename {
        c.name = new.clone();
        for anim in doc.animations.values_mut() {
            if let Some(keys) = anim.physics.remove(&p.name) { anim.physics.insert(new.clone(), keys); }
        }
        for n in doc.skins.iter_mut().flat_map(|s| &mut s.constraints) {
            if *n == p.name { *n = new.clone(); }
        }
    }
    Ok(())
}

fn set_constraint_order(doc: &mut Document, names: &[String]) -> OpResult {
    if names.len() != doc.ik.len() + doc.physics.len() + doc.transforms.len() {
        return Err("list every constraint exactly once".into());
    }
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !doc.has_constraint(name) { return Err(format!("constraint {name:?} does not exist")); }
        if !seen.insert(name) {
            return Err(format!("duplicate constraint {name:?}"));
        }
    }
    for (order, name) in names.iter().enumerate() {
        let order = i32::try_from(order).map_err(|_| "too many constraints")?;
        match doc.ik.iter().position(|c| &c.name == name) {
            Some(i) => doc.ik[i].order = order,
            None => match doc.transforms.iter().position(|c| &c.name == name) {
                Some(i) => doc.transforms[i].order = order,
                None => { let i = physics_index(doc, name)?; doc.physics[i].order = order; }
            },
        }
    }
    Ok(())
}

fn set_ik(doc: &mut Document, p: &IkPatch) -> OpResult {
    let i = ik_index(doc, &p.name)?;
    if let Some(new) = &p.rename
        && new != &p.name && doc.has_constraint(new) {
            return Err(format!("constraint {new:?} already exists"));
        }
    let c = &mut doc.ik[i];
    if let Some(order) = p.order {
        c.order = order;
    }
    if let Some(b) = &p.bones {
        c.bones = b.clone();
    }
    if let Some(t) = &p.target {
        c.target = t.clone();
    }
    if let Some(m) = p.mix {
        c.mix = m;
    }
    if let Some(b) = p.bend_positive {
        c.bend_positive = b;
    }
    if let Some(s) = p.softness {
        c.softness = s;
    }
    if let Some(b) = p.compress {
        c.compress = b;
    }
    if let Some(b) = p.stretch {
        c.stretch = b;
    }
    if let Some(new) = &p.rename {
        c.name = new.clone();
        for anim in doc.animations.values_mut() {
            if let Some(keys) = anim.ik.remove(&p.name) {
                anim.ik.insert(new.clone(), keys);
            }
        }
        for n in doc.skins.iter_mut().flat_map(|s| &mut s.constraints) {
            if *n == p.name {
                *n = new.clone();
            }
        }
    }
    Ok(())
}

/// Setup-pose world transforms, valid even in the middle of a batch.
fn setup_pose(doc: &mut Document) -> Pose {
    sort_bones(doc);
    let mut setup = doc.clone();
    setup.ik.clear();
    setup.transforms.clear();
    Pose::compute(&setup, None)
}

fn add_bone_world(doc: &mut Document, name: &str, parent: Option<&str>, start: Vec2, end: Option<Vec2>) -> OpResult {
    if doc.bone(name).is_some() {
        return Err(format!("bone {name:?} already exists"));
    }
    if !start.iter().chain(end.iter().flatten()).all(|v| v.is_finite()) {
        return Err("start and end must be finite".into());
    }
    let parent_world = match parent {
        Some(p) => {
            bone_exists(doc, p)?;
            let pose = setup_pose(doc);
            pose.bones[doc.bone_index(p).expect("checked above")].world
        }
        None => Affine::IDENTITY,
    };
    let inv = parent_world.inverse().ok_or_else(|| format!("parent {parent:?} has zero scale"))?;
    let [x, y] = inv.apply(start[0], start[1]);
    // Direction in the parent's space gives the local rotation and a length in
    // the bone's own (unscaled) units; without `end` the world angle is 0.
    let [dx, dy] = match end {
        Some(e) => inv.apply_vector(e[0] - start[0], e[1] - start[1]),
        None => inv.apply_vector(1.0, 0.0),
    };
    let mut bone = Bone::new(name);
    bone.parent = parent.map(str::to_string);
    bone.x = round3(x);
    bone.y = round3(y);
    bone.rotation = round3(dy.atan2(dx).to_degrees());
    bone.length = if end.is_some() { round3(dx.hypot(dy)) } else { 0.0 };
    doc.bones.push(bone);
    Ok(())
}

fn mesh_from_region(doc: &mut Document, slot: &str, region: &str, name: Option<&str>, cols: u32, rows: u32) -> OpResult {
    let si = slot_index(doc, slot)?;
    let (cols, rows) = (cols.clamp(1, 64), rows.clamp(1, 64));
    let ai = doc.slots[si]
        .attachments
        .iter()
        .position(|a| a.name() == region)
        .ok_or_else(|| format!("attachment {region:?} does not exist in slot {slot:?}"))?;
    let Attachment::Region(r) = doc.slots[si].attachments[ai].clone() else {
        return Err(format!("attachment {region:?} is not a region"));
    };
    let name = name.unwrap_or(region).to_string();
    if name != region && doc.slots[si].attachment(&name).is_some() {
        return Err(format!("attachment {name:?} already exists in slot {slot:?}"));
    }
    let bone = doc.slots[si].bone.clone();
    let pose = setup_pose(doc);
    let bi = doc.bone_index(&bone).ok_or_else(|| format!("bone {bone:?} does not exist"))?;
    let m = pose.bones[bi].world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));

    let mut vertices = Vec::with_capacity(((cols + 1) * (rows + 1)) as usize);
    for row in 0..=rows {
        for col in 0..=cols {
            let (u, v) = (col as f32 / cols as f32, row as f32 / rows as f32);
            let [x, y] = m.apply(-r.width / 2.0 + u * r.width, r.height / 2.0 - v * r.height);
            vertices.push(MeshVertex { x: round3(x), y: round3(y), u, v, weights: Vec::new() });
        }
    }
    let mut triangles = Vec::with_capacity((cols * rows * 2) as usize);
    for row in 0..rows {
        for col in 0..cols {
            let a = row * (cols + 1) + col;
            let below = a + cols + 1;
            triangles.push([a, below, a + 1]);
            triangles.push([a + 1, below, below + 1]);
        }
    }
    let mesh = Attachment::Mesh(crate::mesh::triangulate(&Mesh { bones: Vec::new(), hull: 0, edges: Vec::new(), name: name.clone(), image: r.image.clone(), vertices, triangles })?.mesh);
    let s = &mut doc.slots[si];
    if name == region {
        s.attachments[ai] = mesh;
    } else {
        s.attachments.insert(ai + 1, mesh);
    }
    Ok(())
}

fn weight_edit(doc: &mut Document, at: MeshAt, only: Option<&[usize]>, locked: &[String], edit: impl FnOnce(&mut Document, MeshAt) -> OpResult) -> OpResult {
    for b in locked { bone_exists(doc,b)?; }
    let before=mesh_ref(doc,at)?.clone();
    check_vertices(&before,only)?;
    if let Some(indices)=only {
        let unique: BTreeSet<_> = indices.iter().collect();
        if unique.len()!=indices.len() { return Err("duplicate vertex index".into()); }
        if indices.is_empty() { return Ok(()); }
    }
    let bone=doc.slot(at.0).unwrap().bone.clone();
    edit(doc,at)?;
    let locked:BTreeSet<String>=locked.iter().cloned().collect();
    let after=mesh_mut(doc,at)?;
    for (i,v) in after.vertices.iter_mut().enumerate() {
        if only.is_none_or(|o|o.contains(&i)) {
            if v.weights==before.vertices[i].weights && !locked.is_empty() { continue; }
            let explicit=!v.weights.is_empty();
            v.weights=crate::mesh::preserve_locked(&before.vertices[i],&crate::mesh::weight_map(v,&bone),&bone,&locked);
            if explicit && v.weights.is_empty() { v.weights=vec![Weight {bone:bone.clone(),weight:1.0, offset: [0.0, 0.0]}]; }
        }
    }
    if !after.bones.is_empty() { after.bones=crate::mesh::bound_bones(after,&bone); }
    Ok(())
}

fn bind_bones(doc: &mut Document, at: MeshAt, add: &[String], remove: &[String], redistribute: Redistribute) -> OpResult {
    for b in add.iter().chain(remove) { bone_exists(doc,b)?; }
    if add.iter().any(|b|remove.contains(b)) { return Err("cannot add and remove the same bone".into()); }
    if add.iter().collect::<BTreeSet<_>>().len()!=add.len() || remove.iter().collect::<BTreeSet<_>>().len()!=remove.len() { return Err("duplicate binding bone".into()); }
    let slot_bone=doc.slot(at.0).ok_or("slot does not exist")?.bone.clone();
    let mesh=mesh_ref(doc,at)?.clone();
    let old=crate::mesh::bound_bones(&mesh,&slot_bone);
    if remove.iter().any(|b|!old.contains(b)) { return Err("removed bone is not bound".into()); }
    let palette: BTreeSet<String> = old.into_iter().chain(add.iter().cloned()).filter(|b|!remove.contains(b)).collect();
    if palette.is_empty() { return Err("a mesh needs at least one binding bone".into()); }
    let pose=setup_pose(doc);
    let out=crate::mesh::bind_weights(&mesh,doc,&pose,&slot_bone,palette,remove,redistribute)?;
    *mesh_mut(doc,at)?=out;
    Ok(())
}

fn auto_weights(doc: &mut Document, at: MeshAt, bones: Option<&[String]>, max_influences: usize, falloff: f32, only: Option<&[usize]>) -> OpResult {
    if !(1..=8).contains(&max_influences) || !falloff.is_finite() || !(0.1..=16.0).contains(&falloff) { return Err("max_influences must be in 1..=8; falloff in 0.1..=16".into()); }
    let palette=mesh_ref(doc,at)?.bones.clone();
    let bones=bones.or_else(||(!palette.is_empty()).then_some(palette.as_slice()));
    let attachment = at.1;
    if let Some(list) = bones {
        if list.iter().collect::<BTreeSet<_>>().len()!=list.len() {return Err("duplicate candidate bone".into());}
        if list.is_empty() {
            return Err("bones must not be empty".into());
        }
        for b in list {
            bone_exists(doc, b)?;
        }
    }
    mesh_ref(doc, at)?;
    let pose = setup_pose(doc);
    let candidates: Vec<(String, [f32; 2], [f32; 2])> = doc
        .bones
        .iter()
        .zip(&pose.bones)
        .filter(|(b, _)| bones.is_none_or(|list| list.contains(&b.name)))
        .map(|(b, p)| (b.name.clone(), p.world.apply(0.0, 0.0), p.world.apply(b.length, 0.0)))
        .collect();
    let (max_influences, falloff) = (max_influences.clamp(1, 8), falloff.clamp(0.1, 16.0));

    let mesh = mesh_mut(doc, at)?;
    if let Some(bad) = only.and_then(|o| o.iter().find(|&&i| i >= mesh.vertices.len())) {
        return Err(format!("vertex {bad} does not exist in mesh {attachment:?}"));
    }
    for (vi, v) in mesh.vertices.iter_mut().enumerate() {
        if only.is_some_and(|o| !o.contains(&vi)) {
            continue;
        }
        let mut scored: Vec<(f32, &str)> = candidates
            .iter()
            .map(|(name, a, b)| (1.0 / (segment_distance([v.x, v.y], *a, *b) + 1.0).powf(falloff), name.as_str()))
            .collect();
        scored.sort_by(|x, y| y.0.total_cmp(&x.0));
        scored.truncate(max_influences);
        let total: f32 = scored.iter().map(|s| s.0).sum();
        let mut weights: Vec<Weight> = scored
            .iter()
            .map(|&(s, bone)| Weight { bone: bone.to_string(), weight: round3(s / total), offset: [0.0, 0.0] })
            .filter(|w| w.weight >= 0.01)
            .collect();
        // Keep the stored weights summing to 1 after rounding.
        let sum: f32 = weights.iter().map(|w| w.weight).sum();
        if let Some(first) = weights.first_mut() {
            first.weight = round3(first.weight + 1.0 - sum);
        }
        v.weights = weights;
    }
    Ok(())
}

/// (slot, attachment, skin): a mesh in a slot, or in a skin for that slot.
type MeshAt<'a> = (&'a str, &'a str, Option<&'a str>);

fn topology_edit(doc: &mut Document, at: MeshAt, change: impl FnOnce(&Mesh, &str) -> std::result::Result<crate::mesh::MeshEdit, String>) -> OpResult {
    if let Some(problem) = mesh_ref(doc, at).ok().and_then(crate::mesh::hull_crossing) { return Err(problem); }
    let bone = doc.slot(at.0).ok_or_else(|| format!("slot {:?} does not exist", at.0))?.bone.clone();
    let edit = change(mesh_ref(doc, at)?, &bone)?;
    // Spine과 다름: added offsets are interpolated, removed indices are
    // dropped, and every surviving vertex keeps its own keys and weights.
    // Linked children with their own timelines share these vertex indices.
    let mut paths = vec![(at.2.map(str::to_string), at.1.to_string())];
    paths.extend(linked_children(doc, at.0, at.1, at.2).into_iter().filter(|c| !c.timelines).map(|c| (c.owner, c.name)));
    for anim in doc.animations.values_mut() {
        for (owner, name) in &paths {
            let Some(keys) = anim.deform.get_mut(owner.as_deref().unwrap_or("default")).and_then(|s| s.get_mut(at.0)).and_then(|a| a.get_mut(name)) else { continue };
            for key in keys {
                let old = &key.value;
                key.value = edit.remap.iter().map(|parts| {
                    let mut value = [0.0, 0.0];
                    for &(i, factor) in parts {
                        let offset = old.get(i).ok_or("deform key does not match the input mesh")?;
                        value[0] += offset[0] * factor; value[1] += offset[1] * factor;
                    }
                    Ok(value)
                }).collect::<std::result::Result<_, String>>()?;
            }
        }
    }
    *mesh_mut(doc, at)? = edit.mesh;
    Ok(())
}

fn not_a_mesh((slot, attachment, skin): MeshAt) -> String {
    match skin {
        Some(k) => format!("attachment {attachment:?} for slot {slot:?} in skin {k:?} is not a mesh"),
        None => format!("attachment {attachment:?} in slot {slot:?} is not a mesh"),
    }
}

fn mesh_ref<'a>(doc: &'a Document, at: MeshAt) -> std::result::Result<&'a Mesh, String> {
    let (slot, attachment, skin) = at;
    let s = doc.slot(slot).ok_or_else(|| format!("slot {slot:?} does not exist"))?;
    let att = match skin {
        Some(k) => doc.skin(k).ok_or_else(|| format!("skin {k:?} does not exist"))?.attachment(slot, attachment),
        None => s.attachment(attachment),
    };
    match att {
        Some(Attachment::Mesh(m)) => Ok(m),
        _ => Err(not_a_mesh(at)),
    }
}

fn mesh_mut<'a>(doc: &'a mut Document, at: MeshAt) -> std::result::Result<&'a mut Mesh, String> {
    let (slot, attachment, skin) = at;
    let si = slot_index(doc, slot)?;
    let atts = match skin {
        Some(k) => {
            let ki = skin_index(doc, k)?;
            doc.skins[ki].attachments.get_mut(slot).ok_or_else(|| not_a_mesh(at))?
        }
        None => &mut doc.slots[si].attachments,
    };
    match atts.iter_mut().find(|a| a.name() == attachment) {
        Some(Attachment::Mesh(m)) => Ok(m),
        _ => Err(not_a_mesh(at)),
    }
}

fn check_vertices(mesh: &Mesh, only: Option<&[usize]>) -> OpResult {
    match only.and_then(|o| o.iter().find(|&&i| i >= mesh.vertices.len())) {
        Some(bad) => Err(format!("vertex {bad} does not exist in mesh {:?}", mesh.name)),
        None => Ok(()),
    }
}

/// The weights at `p` interpolated over the closest point of `mesh`'s
/// triangles (barycentric), or of its nearest vertex for a mesh without
/// triangles. `None` for an empty mesh. Unweighted vertices count as fully
/// weighted to `slot_bone`.
fn weights_at(mesh: &Mesh, slot_bone: &str, p: Vec2) -> Option<Vec<Weight>> {
    let pos = |i: u32| mesh.vertices.get(i as usize).map(|v| [v.x, v.y]);
    let mut best: Option<(f32, [(u32, f32); 3])> = None;
    for t in &mesh.triangles {
        let (Some(a), Some(b), Some(c)) = (pos(t[0]), pos(t[1]), pos(t[2])) else { continue };
        let (q, bary) = closest_on_triangle(p, a, b, c);
        let d = (q[0] - p[0]).hypot(q[1] - p[1]);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, [(t[0], bary[0]), (t[1], bary[1]), (t[2], bary[2])]));
        }
    }
    let parts: Vec<(u32, f32)> = match best {
        Some((_, parts)) => parts.to_vec(),
        None => {
            let (i, _) = mesh.vertices.iter().enumerate().min_by(|a, b| {
                let d = |v: &MeshVertex| (v.x - p[0]).hypot(v.y - p[1]);
                d(a.1).total_cmp(&d(b.1))
            })?;
            vec![(i as u32, 1.0)]
        }
    };
    let mut sum: BTreeMap<String, f32> = BTreeMap::new();
    for (i, f) in parts {
        let v = &mesh.vertices[i as usize];
        let total: f32 = v.weights.iter().map(|w| w.weight).sum();
        if v.weights.is_empty() || total <= 0.0 {
            *sum.entry(slot_bone.to_string()).or_default() += f;
        } else {
            for w in &v.weights {
                *sum.entry(w.bone.clone()).or_default() += f * w.weight / total;
            }
        }
    }
    Some(normalized_weights(sum.into_iter().collect()))
}

/// Strongest first, at most 8, rounded to 3 decimals and summing to 1.
fn normalized_weights(mut list: Vec<(String, f32)>) -> Vec<Weight> {
    list.sort_by(|a, b| b.1.total_cmp(&a.1));
    list.truncate(8);
    let total: f32 = list.iter().map(|w| w.1).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<Weight> = list.into_iter().map(|(bone, w)| Weight { bone, weight: round3(w / total), offset: [0.0, 0.0] }).filter(|w| w.weight >= 0.01).collect();
    let rest: f32 = out.iter().map(|w| w.weight).sum();
    if let Some(first) = out.first_mut() {
        first.weight = round3(first.weight + 1.0 - rest);
    }
    out
}

/// Closest point of triangle abc to p, with its barycentric coordinates.
pub(crate) fn closest_on_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> (Vec2, [f32; 3]) {
    let sub = |x: Vec2, y: Vec2| [x[0] - y[0], x[1] - y[1]];
    let dot = |x: Vec2, y: Vec2| x[0] * y[0] + x[1] * y[1];
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return ([a[0] + ab[0] * v, a[1] + ab[1] * v], [1.0 - v, v, 0.0]);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return ([a[0] + ac[0] * w, a[1] + ac[1] * w], [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return ([b[0] + (c[0] - b[0]) * w, b[1] + (c[1] - b[1]) * w], [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    ([a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w], [1.0 - v - w, v, w])
}

/// Gives every vertex of `target` the weights found at its position on
/// `source` (see `TransferWeights`); `source_bone` stands in for unweighted
/// source vertices. Used to keep painted weights when a mesh is rebuilt.
pub fn transfer_mesh_weights(target: &mut Mesh, source: &Mesh, source_bone: &str) {
    for v in &mut target.vertices {
        if let Some(w) = weights_at(source, source_bone, [v.x, v.y]) {
            v.weights = w;
        }
    }
}

fn transfer_weights(doc: &mut Document, at: MeshAt, from: MeshAt, only: Option<&[usize]>) -> OpResult {
    let (from_slot, from_attachment, _) = from;
    let source = mesh_ref(doc, from)?.clone();
    let source_bone = doc.slot(from_slot).map(|s| s.bone.clone()).unwrap_or_default();
    if source.vertices.is_empty() {
        return Err(format!("mesh {from_attachment:?} in slot {from_slot:?} has no vertices"));
    }
    let mesh = mesh_mut(doc, at)?;
    check_vertices(mesh, only)?;
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        if only.is_none_or(|o| o.contains(&i))
            && let Some(w) = weights_at(&source, &source_bone, [v.x, v.y])
        {
            v.weights = w;
        }
    }
    Ok(())
}

/// The bone on the other side by naming convention, if it exists.
fn mirror_bone(doc: &Document, name: &str) -> Option<String> {
    const PAIRS: [(&str, &str); 6] = [("_l", "_r"), ("_L", "_R"), ("left", "right"), ("Left", "Right"), (".l", ".r"), (".L", ".R")];
    let mut candidates = Vec::new();
    for (a, b) in PAIRS {
        for (x, y) in [(a, b), (b, a)] {
            if let Some(stem) = name.strip_suffix(x) {
                candidates.push(format!("{stem}{y}"));
            }
            if let Some(i) = name.find(x) {
                candidates.push(format!("{}{y}{}", &name[..i], &name[i + x.len()..]));
            }
        }
    }
    for (x, y) in [("L_", "R_"), ("R_", "L_"), ("l_", "r_"), ("r_", "l_")] {
        if let Some(rest) = name.strip_prefix(x) {
            candidates.push(format!("{y}{rest}"));
        }
    }
    candidates.into_iter().find(|c| doc.bone(c).is_some())
}

fn mirror_weights(doc: &mut Document, at: MeshAt, axis: f32, source: MirrorSide) -> OpResult {
    let slot = at.0;
    let mesh = mesh_ref(doc, at)?.clone();
    let slot_bone = doc.slot(slot).map(|s| s.bone.clone()).unwrap_or_default();
    let swap: BTreeMap<String, String> = doc.bones.iter().filter_map(|b| mirror_bone(doc, &b.name).map(|m| (b.name.clone(), m))).collect();
    let keeps = |x: f32| match source {
        MirrorSide::Positive => x >= axis,
        MirrorSide::Negative => x <= axis,
    };
    // Sample only the source half so the target half never reads itself.
    let mut half = mesh.clone();
    half.triangles.retain(|t| t.iter().all(|&i| half.vertices.get(i as usize).is_some_and(|v| keeps(v.x))));
    let out = mesh_mut(doc, at)?;
    for v in out.vertices.iter_mut().filter(|v| !keeps(v.x)) {
        let Some(w) = weights_at(&half, &slot_bone, [2.0 * axis - v.x, v.y]) else { continue };
        let swapped = w.into_iter().map(|w| (swap.get(&w.bone).cloned().unwrap_or(w.bone), w.weight)).collect();
        v.weights = normalized_weights(swapped);
    }
    Ok(())
}

fn set_vertices(doc: &mut Document, at: MeshAt, patches: &[VertexPatch]) -> OpResult {
    for w in patches.iter().flat_map(|p| p.weights.iter().flatten()) {
        bone_exists(doc, &w.bone)?;
    }
    let slot_bone=doc.slot(at.0).ok_or("slot does not exist")?.bone.clone();
    let mesh = mesh_mut(doc, at)?;
    let n = mesh.vertices.len();
    let mut moved = false;
    for p in patches {
        let v = mesh.vertices.get_mut(p.index).ok_or_else(|| format!("vertex {} out of range (mesh has {n})", p.index))?;
        moved |= p.x.is_some_and(|x| x != v.x) || p.y.is_some_and(|y| y != v.y);
        for (dst, src) in [(&mut v.x, p.x), (&mut v.y, p.y), (&mut v.u, p.u), (&mut v.v, p.v)] {
            if let Some(val) = src {
                *dst = val;
            }
        }
        if let Some(w) = &p.weights {
            v.weights = w.clone();
        }
    }
    if !mesh.bones.is_empty() { mesh.bones=crate::mesh::bound_bones(mesh,&slot_bone); }
    if moved { crate::mesh::refit_triangles(mesh); }
    Ok(())
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Gives a document the root bone: its other parentless bones move under it,
/// keeping where they are in the setup pose. Returns whether anything changed.
pub fn ensure_root(doc: &mut Document) -> bool {
    let free = |doc: &Document| doc.bones.iter().filter(|b| b.parent.is_none() && b.name != ROOT).map(|b| b.name.clone()).collect::<Vec<_>>();
    let misplaced = doc.bone(ROOT).is_some_and(|b| b.parent.is_some());
    if doc.bone(ROOT).is_some() && !misplaced && free(doc).is_empty() { return false; }
    if misplaced {
        let new = (1..).map(|i| format!("{ROOT}_{i}")).find(|n| doc.bone(n).is_none()).expect("unbounded names");
        rename_bone(doc, ROOT, &new).expect("the new name is free");
    }
    let root_world = doc.bone_index(ROOT).map(|i| setup_pose(doc).bones[i].world);
    if doc.bone(ROOT).is_none() { doc.bones.insert(0, Bone::new(ROOT)); }
    for name in free(doc) {
        let i = doc.bone_index(&name).expect("listed above");
        let b = &mut doc.bones[i];
        b.parent = Some(ROOT.into());
        if let Some(root_world) = &root_world {
            let own = LocalTransform { x: b.x, y: b.y, rotation: b.rotation, scale_x: b.scale_x, scale_y: b.scale_y, shear_x: b.shear_x, shear_y: b.shear_y, inherit: b.inherit };
            if let Some(l) = LocalTransform::from_world(Some(root_world), &own.world(None), b.inherit) {
                (b.x, b.y, b.rotation, b.scale_x, b.scale_y, b.shear_x, b.shear_y) = (l.x, l.y, l.rotation, l.scale_x, l.scale_y, l.shear_x, l.shear_y);
            }
        }
    }
    sort_bones(doc);
    true
}

fn remove_bone(doc: &mut Document, name: &str, recursive: bool) -> OpResult {
    bone_exists(doc, name)?;
    if name == ROOT { return Err("the root bone cannot be removed".into()); }
    let doomed: Vec<String> = doc
        .bones
        .iter()
        .filter(|b| is_descendant_or_self(doc, &b.name, name))
        .map(|b| b.name.clone())
        .collect();
    if doomed.len() > 1 && !recursive {
        return Err(format!(
            "bone {name:?} has children ({}); pass \"recursive\": true to remove them",
            doomed[1..].join(", ")
        ));
    }
    let gone = |n: &str| doomed.iter().any(|d| d == n);
    let removed_slots: Vec<String> =
        doc.slots.iter().filter(|s| gone(&s.bone)).map(|s| s.name.clone()).collect();
    doc.bones.retain(|b| !gone(&b.name));
    doc.slots.retain(|s| !gone(&s.bone));
    let mut removed_ik: Vec<String> = doc
        .ik
        .iter()
        .filter(|c| gone(&c.target) || c.bones.iter().any(|b| gone(b)))
        .map(|c| c.name.clone())
        .collect();
    doc.ik.retain(|c| !removed_ik.contains(&c.name));
    removed_ik.extend(doc.physics.iter().filter(|c| gone(&c.bone)).map(|c| c.name.clone()));
    doc.physics.retain(|c| !gone(&c.bone));
    for c in &mut doc.transforms {
        c.bones.retain(|b| !gone(b));
    }
    removed_ik.extend(doc.transforms.iter().filter(|c| gone(&c.source) || c.bones.is_empty()).map(|c| c.name.clone()));
    doc.transforms.retain(|c| !removed_ik.contains(&c.name));
    for skin in &mut doc.skins {
        skin.bones.retain(|b| !gone(b));
        skin.constraints.retain(|c| !removed_ik.contains(c));
        skin.attachments.retain(|s, _| !removed_slots.contains(s));
    }
    for_each_mesh(doc, |m| {
        m.bones.retain(|b| !gone(b));
        for v in &mut m.vertices {
            v.weights.retain(|w| !gone(&w.bone));
        }
    });
    for anim in doc.animations.values_mut() {
        anim.bones.retain(|b, _| !gone(b));
        anim.slots.retain(|s, _| !removed_slots.contains(s));
        for order in anim.draw_order.iter_mut().filter_map(|k| k.order.as_mut()) { order.retain(|s| !removed_slots.contains(s)); }
        for slots in anim.deform.values_mut() { slots.retain(|s, _| !removed_slots.contains(s)); }
        anim.ik.retain(|c, _| !removed_ik.contains(c));
        anim.physics.retain(|c, _| !removed_ik.contains(c));
        anim.transforms.retain(|c, _| !removed_ik.contains(c));
    }
    Ok(())
}

fn set_slot(doc: &mut Document, p: &SlotPatch) -> OpResult {
    let i = slot_index(doc, &p.name)?;
    if let Some(b) = &p.bone {
        bone_exists(doc, b)?;
    }
    if let Some(new) = &p.rename
        && new != &p.name && doc.slot(new).is_some() {
            return Err(format!("slot {new:?} already exists"));
        }
    let slot = &mut doc.slots[i];
    if let Some(b) = &p.bone {
        slot.bone = b.clone();
    }
    if let Some(c) = p.color {
        slot.color = c;
    }
    if let Some(b) = p.blend {
        slot.blend = b;
    }
    if let Some(Some(name)) = &p.attachment
        && !slot_has_attachment(doc, &p.name, name)
    {
        return Err(format!("attachment {name:?} does not exist in slot {:?}", p.name));
    }
    let slot = &mut doc.slots[i];
    if let Some(a) = &p.attachment {
        slot.attachment = a.clone();
    }
    if let Some(new) = &p.rename {
        slot.name = new.clone();
        for anim in doc.animations.values_mut() {
            if let Some(tl) = anim.slots.remove(&p.name) {
                anim.slots.insert(new.clone(), tl);
            }
            for slots in anim.deform.values_mut() {
                if let Some(tl) = slots.remove(&p.name) { slots.insert(new.clone(), tl); }
            }
            for s in anim.draw_order.iter_mut().filter_map(|k| k.order.as_mut()).flatten() {
                if *s == p.name { s.clone_from(new); }
            }
        }
        for skin in &mut doc.skins {
            if let Some(a) = skin.attachments.remove(&p.name) {
                skin.attachments.insert(new.clone(), a);
            }
        }
    }
    Ok(())
}

fn check_time(time: f32) -> OpResult {
    if time.is_finite() && time >= 0.0 { Ok(()) } else { Err(format!("time must be >= 0, got {time}")) }
}

fn upsert<T>(keys: &mut Vec<Key<T>>, time: f32, value: T, curve: Option<Curve>) {
    match keys.iter().position(|k| (k.time - time).abs() < TIME_EPSILON) {
        Some(i) => {
            keys[i].value = value;
            if let Some(c) = curve {
                keys[i].curve = c;
                keys[i].curves.clear();
            }
        }
        None => {
            let i = keys.partition_point(|k| k.time < time);
            keys.insert(i, Key::new(time, value, curve.unwrap_or_default()));
        }
    }
}

fn set_channels<T>(keys: &mut [Key<T>], time: f32, curves: &Option<Vec<Curve>>) {
    if let Some(curves) = curves
        && let Some(key) = keys.iter_mut().find(|k| (k.time - time).abs() < TIME_EPSILON) {
            key.curves.clone_from(curves);
        }
}

fn key_bone(doc: &mut Document, k: &KeyBone) -> OpResult {
    check_time(k.time)?;
    bone_exists(doc, &k.bone)?;
    if k.rotate.is_none() && k.translate.is_none() && k.scale.is_none() && k.shear.is_none() {
        return Err("key_bone needs at least one of rotate, translate, scale, shear".into());
    }
    if let Some(curves) = &k.curves {
        if curves.len() != 2 { return Err("curves needs two entries, [x, y]".into()); }
        if k.translate.is_none() && k.scale.is_none() && k.shear.is_none() {
            return Err("curves needs a translate, scale or shear key".into());
        }
    }
    let tl = doc.animations.entry(k.animation.clone()).or_default().bones.entry(k.bone.clone()).or_default();
    if let Some(v) = k.rotate {
        upsert(&mut tl.rotate, k.time, v, k.curve);
    }
    if let Some(v) = k.translate {
        upsert(&mut tl.translate, k.time, v, k.curve);
        set_channels(&mut tl.translate, k.time, &k.curves);
    }
    if let Some(v) = k.scale {
        upsert(&mut tl.scale, k.time, v, k.curve);
        set_channels(&mut tl.scale, k.time, &k.curves);
    }
    if let Some(v) = k.shear {
        upsert(&mut tl.shear, k.time, v, k.curve);
        set_channels(&mut tl.shear, k.time, &k.curves);
    }
    Ok(())
}

fn key_slot(doc: &mut Document, k: &KeySlot) -> OpResult {
    check_time(k.time)?;
    doc.slot(&k.slot).ok_or_else(|| format!("slot {:?} does not exist", k.slot))?;
    if let Some(Some(name)) = &k.attachment
        && !slot_has_attachment(doc, &k.slot, name) {
            return Err(format!("attachment {name:?} does not exist in slot {:?}", k.slot));
        }
    if k.attachment.is_none() && k.color.is_none() && !k.reset {
        return Err("key_slot needs at least one of attachment, reset, color".into());
    }
    if k.reset && k.attachment.is_some() {
        return Err("give either attachment or reset, not both".into());
    }
    if let Some(curves) = &k.curves {
        if curves.len() != 4 { return Err("curves needs four entries, [r, g, b, a]".into()); }
        if k.color.is_none() { return Err("curves needs a color key".into()); }
    }
    let tl = doc.animations.entry(k.animation.clone()).or_default().slots.entry(k.slot.clone()).or_default();
    if k.attachment.is_some() || k.reset {
        let key = AttachmentKey { time: k.time, name: k.attachment.clone().flatten(), reset: k.reset };
        match tl.attachment.iter().position(|a| (a.time - k.time).abs() < TIME_EPSILON) {
            Some(i) => tl.attachment[i] = key,
            None => {
                let i = tl.attachment.partition_point(|a| a.time < k.time);
                tl.attachment.insert(i, key);
            }
        }
    }
    if let Some(c) = k.color {
        upsert(&mut tl.color, k.time, c, k.curve);
        set_channels(&mut tl.color, k.time, &k.curves);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn remove_keys(
    doc: &mut Document,
    animation: &str,
    bone: Option<&str>,
    slot: Option<&str>,
    timeline: Option<Timeline>,
    time: Option<f32>,
    attachment: Option<&str>,
    skin: Option<&str>,
) -> OpResult {
    if skin.is_some() && (bone.is_some() || timeline.is_some_and(|t| t != Timeline::Deform)) {
        return Err("skin is only used with slot deform timelines".into());
    }
    if let Some(k) = skin { skin_index(doc, k)?; }
    if let Some(t) = time { check_time(t)?; }
    if let Some(s) = slot { slot_index(doc, s)?; }
    let anim = doc
        .animations
        .get_mut(animation)
        .ok_or_else(|| format!("animation {animation:?} does not exist"))?;
    let hit = |t: f32| time.is_none_or(|x| (t - x).abs() < TIME_EPSILON);
    let wants = |t: Timeline| timeline.is_none_or(|x| x == t);
    let mut removed = 0;
    let mut drop_keys = |keys_len_before: usize, keys_len_after: usize| removed += keys_len_before - keys_len_after;

    match (bone, slot) {
        (Some(b), None) => {
            if matches!(timeline, Some(Timeline::Attachment | Timeline::Color | Timeline::Deform)) {
                return Err("bones only have rotate, translate and scale timelines".into());
            }
            let tl = anim.bones.get_mut(b).ok_or_else(|| format!("bone {b:?} has no keys in {animation:?}"))?;
            if wants(Timeline::Rotate) {
                let n = tl.rotate.len();
                tl.rotate.retain(|k| !hit(k.time));
                drop_keys(n, tl.rotate.len());
            }
            if wants(Timeline::Translate) {
                let n = tl.translate.len();
                tl.translate.retain(|k| !hit(k.time));
                drop_keys(n, tl.translate.len());
            }
            if wants(Timeline::Scale) {
                let n = tl.scale.len();
                tl.scale.retain(|k| !hit(k.time));
                drop_keys(n, tl.scale.len());
            }
            if wants(Timeline::Shear) {
                let n = tl.shear.len();
                tl.shear.retain(|k| !hit(k.time));
                drop_keys(n, tl.shear.len());
            }
            if tl.is_empty() {
                anim.bones.remove(b);
            }
        }
        (None, Some(s)) => {
            if matches!(timeline, Some(Timeline::Rotate | Timeline::Translate | Timeline::Scale | Timeline::Shear)) {
                return Err("slots only have attachment, color and deform timelines".into());
            }
            let tl = anim.slots.entry(s.to_string()).or_default();
            if wants(Timeline::Attachment) {
                let n = tl.attachment.len();
                tl.attachment.retain(|k| !hit(k.time));
                drop_keys(n, tl.attachment.len());
            }
            if wants(Timeline::Color) {
                let n = tl.color.len();
                tl.color.retain(|k| !hit(k.time));
                drop_keys(n, tl.color.len());
            }
            if wants(Timeline::Deform) {
                for (_, keys) in anim.deform.get_mut(skin.unwrap_or("default")).and_then(|slots| slots.get_mut(s)).into_iter().flatten().filter(|(name, _)| attachment.is_none_or(|a| a == name.as_str())) {
                    let n = keys.len();
                    keys.retain(|k| !hit(k.time));
                    drop_keys(n, keys.len());
                }
            }
            if tl.is_empty() {
                anim.slots.remove(s);
            }
        }
        _ => return Err("give exactly one of bone or slot".into()),
    }
    prune_deform(anim);
    if removed == 0 && time.is_some() {
        return Err(format!("no key at time {}", time.unwrap_or_default()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(ops: &str) -> Document {
        apply(&Document::new("t"), &parse_ops(ops).unwrap()).unwrap()
    }

    #[test]
    fn build_rename_and_key() {
        let doc = doc_with(
            r#"[
            {"op":"add_bone","name":"root"},
            {"op":"add_bone","name":"arm","parent":"root","x":10,"length":20},
            {"op":"add_slot","name":"arm","bone":"arm","attachment":"a",
             "attachments":[{"type":"region","name":"a","width":20,"height":5}]},
            {"op":"key_bone","animation":"wave","bone":"arm","time":0,"rotate":0},
            {"op":"key_bone","animation":"wave","bone":"arm","time":0.5,"rotate":45,"curve":"ease_in_out"},
            {"op":"set_bone","name":"arm","rename":"upper_arm"}
        ]"#,
        );
        assert_eq!(doc.slots[0].bone, "upper_arm");
        let tl = &doc.animations["wave"].bones["upper_arm"];
        assert_eq!(tl.rotate.len(), 2);
        assert_eq!(tl.rotate[1].curve, Curve::EaseInOut);
        assert_eq!(doc.animations["wave"].duration(), 0.5);
    }

    #[test]
    fn batch_is_atomic_and_reports_index() {
        let doc = doc_with(r#"{"op":"add_bone","name":"root"}"#);
        let ops = parse_ops(r#"[{"op":"add_bone","name":"a"},{"op":"add_bone","name":"b","parent":"nope"}]"#).unwrap();
        match apply(&doc, &ops) {
            Err(Error::Op { index, message }) => {
                assert_eq!(index, 1);
                assert!(message.contains("nope"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_fields_rejected() {
        let err = parse_ops(r#"{"op":"set_bone","name":"root","rotaton":5}"#).unwrap_err();
        assert!(err.to_string().contains("rotaton"), "{err}");
    }

    #[test]
    fn reparent_keeps_order_and_blocks_cycles() {
        let doc = doc_with(
            r#"[{"op":"add_bone","name":"a"},{"op":"add_bone","name":"b"},
                {"op":"set_bone","name":"a","parent":"b"}]"#,
        );
        assert_eq!(doc.bones[0].name, "b");
        let ops = parse_ops(r#"{"op":"set_bone","name":"b","parent":"a"}"#).unwrap();
        assert!(apply(&doc, &ops).is_err());
        let doc = apply(&doc, &parse_ops(r#"{"op":"set_bone","name":"a","parent":null}"#).unwrap()).unwrap();
        assert_eq!(doc.bones.iter().find(|b| b.name == "a").unwrap().parent, None);
    }

    #[test]
    fn remove_bone_requires_recursive() {
        let doc = doc_with(r#"[{"op":"add_bone","name":"a"},{"op":"add_bone","name":"b","parent":"a"}]"#);
        assert!(apply(&doc, &parse_ops(r#"{"op":"remove_bone","name":"a"}"#).unwrap()).is_err());
        let doc = apply(&doc, &parse_ops(r#"{"op":"remove_bone","name":"a","recursive":true}"#).unwrap()).unwrap();
        assert!(doc.bones.is_empty());
    }
}

#[cfg(test)]
mod mesh_tests {
    use super::*;
    use crate::pose::Pose;

    fn arm() -> Document {
        let ops = parse_ops(
            r#"[{"op":"add_bone","name":"upper","length":50},
                {"op":"add_bone","name":"lower","parent":"upper","x":50,"length":50},
                {"op":"add_slot","name":"skin","bone":"upper","attachment":"img",
                 "attachments":[{"type":"region","name":"img","x":50,"width":100,"height":20}]},
                {"op":"mesh_from_region","slot":"skin","region":"img","cols":4,"rows":1},
                {"op":"auto_weights","slot":"skin","attachment":"img"}]"#,
        )
        .unwrap();
        apply(&Document::new("m"), &ops).unwrap()
    }

    fn mesh(doc: &Document) -> &Mesh {
        match doc.slots[0].attachment("img") {
            Some(Attachment::Mesh(m)) => m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grid_mesh_matches_region_in_setup_pose() {
        let doc = arm();
        let m = mesh(&doc);
        assert_eq!((m.vertices.len(), m.triangles.len()), (10, 8));
        assert_eq!([m.vertices[0].x, m.vertices[0].y], [0.0, 10.0]);
        let far = m.vertices.iter().position(|v|v.x==100.0 && v.y == -10.0).unwrap();
        assert!(m.hull >= 3);
        // Setup pose leaves vertices where they were authored.
        let pose = Pose::compute(&doc, None);
        let deformed = pose.deform_mesh(&doc, m, 0);
        assert!((deformed[far][0] - 100.0).abs() < 1e-3 && (deformed[far][1] + 10.0).abs() < 1e-3);
    }

    #[test]
    fn weights_follow_nearest_bone_and_skin() {
        let doc = arm();
        let m = mesh(&doc);
        let far = m.vertices.iter().position(|v|v.x==100.0 && v.y == -10.0).unwrap();
        let end = &m.vertices[far];
        assert_eq!(end.weights[0].bone, "lower");
        assert!((end.weights.iter().map(|w| w.weight).sum::<f32>() - 1.0).abs() < 1e-3);

        // Meshes are bound to the setup pose, so bend the elbow in an animation.
        let ops = r#"{"op":"key_bone","animation":"bend","bone":"lower","time":0,"rotate":90}"#;
        let bent = apply(&doc, &parse_ops(ops).unwrap()).unwrap();
        let pose = Pose::compute(&bent, Some((&bent.animations["bend"], 0.0)));
        let d = pose.deform_mesh(&bent, mesh(&bent), 0);
        // The far end swings up around the elbow at (50, 0); the root end stays.
        assert!(d[far][1] > 30.0, "{:?}", d[far]);
        assert!((d[0][0]).abs() < 2.0, "{:?}", d[0]);
    }

    #[test]
    fn deform_keys_offset_vertices_before_skinning() {
        let doc = arm();
        let mut offsets = vec![[0.0, 0.0]; 10];
        let far = mesh(&doc).vertices.iter().position(|v|v.x==100.0 && v.y == -10.0).unwrap();
        offsets[far] = [0.0, 8.0];
        let op = Op::KeyDeform { skin: None, animation: "d".into(), slot: "skin".into(), attachment: "img".into(), time: 1.0, offsets, curve: None };
        let bend = r#"{"op":"key_bone","animation":"d","bone":"lower","time":0,"rotate":90}"#;
        let mut ops = parse_ops(bend).unwrap();
        ops.push(op);
        let doc = apply(&doc, &ops).unwrap();
        let anim = &doc.animations["d"];
        assert_eq!(anim.duration(), 1.0);

        // Halfway there, the offset is half applied, then carried by the bent elbow.
        let half = Pose::compute(&doc, Some((anim, 0.5)));
        let plain = Pose::compute(&doc, Some((&crate::model::Animation { bones: anim.bones.clone(), ..Default::default() }, 0.5)));
        let (d, p) = (half.deform_mesh(&doc, mesh(&doc), 0), plain.deform_mesh(&doc, mesh(&doc), 0));
        // The +y setup offset turns into roughly -x once the elbow is at 90°.
        assert!(d[far][0] < p[far][0] - 3.0, "{:?} vs {:?}", d[far], p[far]);
        assert_eq!(d[0], p[0]);

        // A wrong offset count is rejected; removing the mesh drops the keys.
        let bad = Op::KeyDeform { skin: None, animation: "d".into(), slot: "skin".into(), attachment: "img".into(), time: 0.0, offsets: vec![[0.0, 0.0]], curve: None };
        assert!(apply(&doc, &[bad]).is_err());
        let rm = parse_ops(r#"{"op":"remove_key","animation":"d","slot":"skin","timeline":"deform","time":1}"#).unwrap();
        assert!(apply(&doc, &rm).unwrap().animations["d"].slots.is_empty());
        let region = parse_ops(r#"{"op":"set_attachment","slot":"skin","attachment":{"type":"region","name":"img","width":10,"height":10}}"#).unwrap();
        assert!(apply(&doc, &region).unwrap().animations["d"].slots.is_empty());
    }

    #[test]
    fn rename_and_remove_bone_update_weights_and_ik() {
        let doc = arm();
        let ops = parse_ops(
            r#"[{"op":"add_bone","name":"goal","x":80,"y":40},
                {"op":"add_ik","name":"reach","bones":["upper","lower"],"target":"goal"},
                {"op":"key_ik","animation":"a","ik":"reach","time":0,"mix":0},
                {"op":"set_bone","name":"lower","rename":"fore"}]"#,
        )
        .unwrap();
        let doc = apply(&doc, &ops).unwrap();
        assert_eq!(doc.ik[0].bones, vec!["upper".to_string(), "fore".to_string()]);
        assert!(mesh(&doc).vertices.iter().flat_map(|v| &v.weights).any(|w| w.bone == "fore"));
        let doc = apply(&doc, &parse_ops(r#"{"op":"remove_bone","name":"fore"}"#).unwrap()).unwrap();
        assert!(doc.ik.is_empty() && doc.animations["a"].ik.is_empty());
        assert!(mesh(&doc).vertices.iter().flat_map(|v| &v.weights).all(|w| w.bone != "fore"));
    }

    #[test]
    fn invalid_ik_is_rejected() {
        let doc = arm();
        for bad in [
            r#"{"op":"add_ik","name":"x","bones":["lower","upper"],"target":"upper"}"#,
            r#"{"op":"add_ik","name":"x","bones":["upper"],"target":"lower"}"#,
            r#"{"op":"add_ik","name":"x","bones":["upper"],"target":"nope"}"#,
        ] {
            assert!(apply(&doc, &parse_ops(bad).unwrap()).is_err(), "{bad}");
        }
    }
}

#[cfg(test)]
mod world_bone_tests {
    use super::*;
    use crate::pose::Pose;

    #[test]
    fn world_bones_land_where_requested() {
        let ops = parse_ops(
            r#"[{"op":"add_bone","name":"root","x":5,"rotation":30,"scale_x":2,"scale_y":0.5},
                {"op":"add_bone_world","name":"arm","parent":"root","start":[10,20],"end":[40,60]},
                {"op":"add_bone_world","name":"hand","parent":"arm","start":[40,60]}]"#,
        )
        .unwrap();
        let doc = apply(&Document::new("w"), &ops).unwrap();
        let pose = Pose::compute(&doc, None);
        let arm = doc.bone_index("arm").unwrap();
        let [x, y] = pose.bones[arm].world.apply(0.0, 0.0);
        let [tx, ty] = pose.bones[arm].world.apply(doc.bones[arm].length, 0.0);
        assert!((x - 10.0).abs() < 0.01 && (y - 20.0).abs() < 0.01, "{x},{y}");
        assert!((tx - 40.0).abs() < 0.05 && (ty - 60.0).abs() < 0.05, "{tx},{ty}");
        let hand = doc.bone_index("hand").unwrap();
        assert!(pose.bones[hand].world.rotation_deg().abs() < 0.01);
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;
    use crate::pose::{Pose, Track};

    /// A face rigged the Spine way: a face pose is a one-frame animation on
    /// a lower track; blink and talk animations play on top of it.
    fn face() -> Document {
        let region = |n: &str| format!(r#"{{"type":"region","name":"{n}","width":10,"height":5}}"#);
        let eyes = ["open", "closed", "happy"].map(region).join(",");
        let mouth = ["m_closed", "m_a", "m_o", "m_i"].map(region).join(",");
        let ops = format!(
            r##"[{{"op":"add_bone","name":"head"}},
                {{"op":"add_slot","name":"eyes","bone":"head","attachment":"open","attachments":[{eyes}]}},
                {{"op":"add_slot","name":"mouth","bone":"head","attachment":"m_closed","attachments":[{mouth}]}},
                {{"op":"key_slot","animation":"face/happy","slot":"eyes","time":0,"attachment":"happy"}},
                {{"op":"key_slot","animation":"face/happy","slot":"mouth","time":0,"attachment":"m_a","color":"#ff8080"}},
                {{"op":"key_slot","animation":"blink","slot":"eyes","time":1.88,"attachment":"closed"}},
                {{"op":"key_slot","animation":"blink","slot":"eyes","time":2,"reset":true}},
                {{"op":"key_slot","animation":"talk","slot":"mouth","time":0,"attachment":"m_o"}},
                {{"op":"key_slot","animation":"talk","slot":"mouth","time":1,"reset":true}}]"##
        );
        apply(&Document::new("f"), &parse_ops(&ops).unwrap()).unwrap()
    }

    fn eyes_at(doc: &Document, t: f32, face: Option<&str>) -> Option<String> {
        let mut tracks: Vec<Track> = face.map(|f| Track { animation: &doc.animations[f], time: 0.0, alpha: 1.0 }).into_iter().collect();
        tracks.push(Track { animation: &doc.animations["blink"], time: t, alpha: 1.0 });
        Pose::compute_with(doc, &tracks).slots[0].attachment.clone()
    }

    #[test]
    fn blink_closes_then_returns_to_the_face_below() {
        let doc = face();
        assert_eq!(doc.animations["blink"].duration(), 2.0);
        assert_eq!(eyes_at(&doc, 0.5, None).as_deref(), Some("open"));
        assert_eq!(eyes_at(&doc, 1.95, None).as_deref(), Some("closed"));
        assert_eq!(eyes_at(&doc, 1.95, Some("face/happy")).as_deref(), Some("closed"));
        // After the blink the lower track's eyes come back, not the setup ones.
        assert_eq!(eyes_at(&doc, 2.0, Some("face/happy")).as_deref(), Some("happy"));
        assert_eq!(eyes_at(&doc, 0.5, Some("face/happy")).as_deref(), Some("happy"));
    }

    #[test]
    fn tracks_layer_with_face_colors() {
        let doc = face();
        let tracks = [
            Track { animation: &doc.animations["face/happy"], time: 0.0, alpha: 1.0 },
            Track { animation: &doc.animations["blink"], time: 1.95, alpha: 1.0 },
            Track { animation: &doc.animations["talk"], time: 0.0, alpha: 1.0 },
        ];
        let pose = Pose::compute_with(&doc, &tracks);
        assert_eq!(pose.slots[0].attachment.as_deref(), Some("closed"));
        assert_eq!(pose.slots[1].attachment.as_deref(), Some("m_o"));
        assert_eq!(pose.slots[1].color.to_string(), "#ff8080");
    }

    #[test]
    fn legacy_expressions_become_face_animations() {
        let json = r##"{"format":"skel2d/1","name":"f","bones":[{"name":"head"}],
            "slots":[{"name":"eyes","bone":"head","attachment":"open","attachments":[
                {"type":"region","name":"open","width":1,"height":1},{"type":"region","name":"happy","width":1,"height":1}]}],
            "expressions":{"happy":{"slots":{"eyes":"happy"},"colors":{"eyes":"#ff0000"}}}}"##;
        let doc = crate::from_str(json).unwrap();
        let anim = &doc.animations["happy"];
        assert_eq!(anim.slots["eyes"].attachment[0].name.as_deref(), Some("happy"));
        assert_eq!(anim.slots["eyes"].color[0].value.to_string(), "#ff0000");
        assert!(!crate::to_string(&doc).contains("expressions"));
    }
}

#[cfg(test)]
mod weight_tool_tests {
    use super::*;

    /// Two bones side by side (arm_l at x>0, arm_r at x<0) and two grid meshes:
    /// `body` spanning both and `shirt` on top of it.
    fn doc() -> Document {
        let ops = r##"[{"op":"add_bone","name":"root"},
            {"op":"add_bone","name":"arm_l","parent":"root","x":10,"length":40},
            {"op":"add_bone","name":"arm_r","parent":"root","x":-10,"rotation":180,"length":40},
            {"op":"add_slot","name":"body","bone":"root","attachment":"body","attachments":[{"type":"region","name":"body","x":0,"width":100,"height":20}]},
            {"op":"add_slot","name":"shirt","bone":"root","attachment":"shirt","attachments":[{"type":"region","name":"shirt","x":0,"width":80,"height":10}]},
            {"op":"mesh_from_region","slot":"body","region":"body","cols":4,"rows":1},
            {"op":"mesh_from_region","slot":"shirt","region":"shirt","cols":3,"rows":1},
            {"op":"auto_weights","slot":"body","attachment":"body","bones":["arm_l","arm_r"]}]"##;
        apply(&Document::new("w"), &parse_ops(ops).unwrap()).unwrap()
    }

    fn mesh<'a>(doc: &'a Document, slot: &str) -> &'a Mesh {
        mesh_ref(doc, (slot, slot, None)).unwrap()
    }

    fn top(v: &MeshVertex) -> &str {
        v.weights.first().map_or("", |w| w.bone.as_str())
    }

    #[test]
    fn transfer_takes_the_nearest_body_weights() {
        let d = apply(&doc(), &parse_ops(r#"{"op":"transfer_weights","slot":"shirt","attachment":"shirt","from_slot":"body","from_attachment":"body"}"#).unwrap()).unwrap();
        for v in &mesh(&d, "shirt").vertices {
            let sum: f32 = v.weights.iter().map(|w| w.weight).sum();
            assert!((sum - 1.0).abs() < 0.002, "{sum}");
            if v.x > 20.0 {
                assert_eq!(top(v), "arm_l");
            }
            if v.x < -20.0 {
                assert_eq!(top(v), "arm_r");
            }
        }
    }

    #[test]
    fn mirror_swaps_left_and_right() {
        // Everything on arm_l, then mirror the positive side onto the negative one.
        let ops = r#"[{"op":"auto_weights","slot":"body","attachment":"body","bones":["arm_l"]},
            {"op":"mirror_weights","slot":"body","attachment":"body"}]"#;
        let d = apply(&doc(), &parse_ops(ops).unwrap()).unwrap();
        for v in &mesh(&d, "body").vertices {
            assert_eq!(top(v), if v.x >= 0.0 { "arm_l" } else { "arm_r" }, "x {}", v.x);
        }
    }

    #[test]
    fn auto_weights_on_some_vertices_only() {
        let before = doc();
        let ops = r#"{"op":"auto_weights","slot":"body","attachment":"body","bones":["root"],"vertices":[0]}"#;
        let d = apply(&before, &parse_ops(ops).unwrap()).unwrap();
        let (a, b) = (mesh(&before, "body"), mesh(&d, "body"));
        assert_eq!(top(&b.vertices[0]), "root");
        assert_eq!(a.vertices[1..], b.vertices[1..]);
        assert!(apply(&before, &parse_ops(r#"{"op":"auto_weights","slot":"body","attachment":"body","vertices":[999]}"#).unwrap()).is_err());
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;
    use crate::pose::Pose;

    /// A body slot with a "shirt" placeholder filled by two skins, and a
    /// skin-required cape bone only the "hero" skin turns on.
    fn doc() -> Document {
        let region = |n: &str, img: &str| format!(r#"{{"type":"region","name":"{n}","image":"{img}","width":10,"height":10}}"#);
        let ops = format!(
            r#"[{{"op":"add_bone","name":"root"}},
                {{"op":"add_bone","name":"cape","parent":"root","skin_required":true}},
                {{"op":"add_slot","name":"body","bone":"root","attachment":"shirt","attachments":[{plain}]}},
                {{"op":"add_slot","name":"cape","bone":"cape","attachment":"cape","attachments":[{cape}]}},
                {{"op":"add_skin","name":"red"}},
                {{"op":"add_skin","name":"hero"}},
                {{"op":"set_skin_attachment","skin":"red","slot":"body","attachment":{red}}},
                {{"op":"set_skin_attachment","skin":"hero","slot":"body","attachment":{hero}}},
                {{"op":"set_skin","name":"hero","bones":["cape"]}}]"#,
            plain = region("shirt", "plain.png"),
            cape = region("cape", "cape.png"),
            red = region("shirt", "red.png"),
            hero = region("shirt", "hero.png"),
        );
        apply(&Document::new("s"), &parse_ops(&ops).unwrap()).unwrap()
    }

    fn image(doc: &Document, skins: &[&str], slot: usize) -> Option<String> {
        let skins: Vec<String> = skins.iter().map(|s| s.to_string()).collect();
        let pose = Pose::compute_skinned(doc, &[], &skins);
        match pose.attachment(doc, slot)? {
            Attachment::Region(r) => r.image.clone(),
            Attachment::Mesh(m) => m.image.clone(),
            Attachment::LinkedMesh(m) => m.image.clone(),
        }
    }

    #[test]
    fn skins_fill_placeholders_last_wins() {
        let d = doc();
        assert_eq!(image(&d, &[], 0).as_deref(), Some("plain.png"));
        assert_eq!(image(&d, &["red"], 0).as_deref(), Some("red.png"));
        assert_eq!(image(&d, &["red", "hero"], 0).as_deref(), Some("hero.png"));
        assert_eq!(image(&d, &["hero", "red"], 0).as_deref(), Some("red.png"));
    }

    #[test]
    fn skin_required_bones_need_their_skin() {
        let d = doc();
        assert_eq!(image(&d, &[], 1), None);
        assert_eq!(image(&d, &["red"], 1), None);
        assert_eq!(image(&d, &["hero"], 1).as_deref(), Some("cape.png"));
    }

    #[test]
    fn renames_and_removals_follow_into_skins() {
        let d = doc();
        let ops = r#"[{"op":"set_slot","name":"body","rename":"torso"},
            {"op":"set_bone","name":"cape","rename":"cloak"}]"#;
        let d = apply(&d, &parse_ops(ops).unwrap()).unwrap();
        assert!(d.skin("red").unwrap().attachments.contains_key("torso"));
        assert_eq!(d.skin("hero").unwrap().bones, ["cloak"]);
        let d = apply(&d, &parse_ops(r#"{"op":"remove_bone","name":"cloak"}"#).unwrap()).unwrap();
        assert!(d.skin("hero").unwrap().bones.is_empty());
        let text = crate::to_string(&d);
        assert_eq!(crate::from_str(&text).unwrap(), d);
    }

    #[test]
    fn placeholder_only_in_a_skin_can_be_shown_and_keyed() {
        let d = doc();
        let ops = r#"[{"op":"set_skin_attachment","skin":"red","slot":"body","attachment":{"type":"region","name":"vest","width":5,"height":5}},
            {"op":"key_slot","animation":"a","slot":"body","time":0,"attachment":"vest"}]"#;
        assert!(apply(&d, &parse_ops(ops).unwrap()).is_ok());
        assert!(apply(&d, &parse_ops(r#"{"op":"set_slot","name":"body","attachment":"nope"}"#).unwrap()).is_err());
    }
}
