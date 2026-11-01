//! Image-dependent operations expand to ordinary core operations. The core
//! never opens assets; editor, CLI, HTTP and MCP share this entry point.

pub use crate::mesh::Interior;
use crate::mesh::{Density, OutlineParams, Placement, trace_outline_mesh};
use crate::weights::{BoneSegment, geodesic_weights, solid_length};
use crate::{ALPHA_THRESHOLD, Mask};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use skel2d_core::{Affine, Attachment, Document, Mesh, Op, Pose};
use std::{borrow::Cow, collections::BTreeSet, path::Path};
use tiny_skia::Pixmap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TraceParams {
    pub alpha_threshold: f32,
    pub padding: f32,
    pub detail: f32,
    /// Optional outline simplification tolerance in source-image pixels.
    pub concavity: Option<f32>,
    pub interior: Interior,
    pub spacing: Option<f32>,
    pub keep_weights: bool,
}
impl Default for TraceParams {
    fn default() -> Self {
        Self {
            alpha_threshold: 0.05,
            padding: 2.0,
            detail: 0.5,
            concavity: None,
            interior: Interior::Auto,
            spacing: None,
            keep_weights: true,
        }
    }
}
impl TraceParams {
    pub fn preset(density: Density) -> Self {
        Self {
            detail: match density {
                Density::Low => 0.0,
                Density::Medium => 0.5,
                Density::High => 1.0,
            },
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, min, max) in [
            ("alpha_threshold", self.alpha_threshold, 0.0, 1.0),
            ("padding", self.padding, 0.0, 64.0),
            ("detail", self.detail, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!("{name} must be finite and in {min}..={max}"));
            }
        }
        if self
            .concavity
            .is_some_and(|v| !v.is_finite() || !(0.0..=64.0).contains(&v))
        {
            return Err("concavity must be finite and in 0..=64 pixels".into());
        }
        if self.spacing.is_some_and(|v| !v.is_finite() || v < 2.0) {
            return Err("spacing must be finite and at least 2 pixels".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetOp {
    TraceMesh {
        slot: String,
        attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(flatten)]
        params: TraceParams,
    },
    ImageWeights {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locked_bones: Vec<String>,
        slot: String,
        attachment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bones: Option<Vec<String>>,
        #[serde(default = "default_influences")]
        max_influences: usize,
        #[serde(default = "default_falloff")]
        falloff: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vertices: Option<Vec<usize>>,
    },
}
fn default_influences() -> usize {
    4
}
fn default_falloff() -> f32 {
    3.0
}

/// One shared op-tag namespace. Dispatch is explicit so unknown fields and
/// unknown tags retain serde diagnostics rather than untagged-enum errors.
#[derive(Debug, Clone, PartialEq)]
pub enum AnyOp {
    Core(Op),
    Asset(AssetOp),
}
impl From<Op> for AnyOp {
    fn from(op: Op) -> Self {
        Self::Core(op)
    }
}
impl From<AssetOp> for AnyOp {
    fn from(op: AssetOp) -> Self {
        Self::Asset(op)
    }
}
impl Serialize for AnyOp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Core(op) => op.serialize(s),
            Self::Asset(op) => op.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for AnyOp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        if matches!(
            value.get("op").and_then(|v| v.as_str()),
            Some("trace_mesh" | "image_weights")
        ) {
            serde_json::from_value(value)
                .map(Self::Asset)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Core)
                .map_err(serde::de::Error::custom)
        }
    }
}
impl JsonSchema for AnyOp {
    fn schema_name() -> Cow<'static, str> {
        "AnyOp".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let core = generator.subschema_for::<Op>();
        let asset = generator.subschema_for::<AssetOp>();
        json_schema!({"oneOf":[core,asset]})
    }
}
pub fn parse_ops(json: &str) -> skel2d_core::Result<Vec<AnyOp>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        item => vec![item],
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            serde_json::from_value(item).map_err(|e| skel2d_core::Error::op(index, e.to_string()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MeshSummary {
    pub slot: String,
    pub attachment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin: Option<String>,
    pub vertices: usize,
    pub triangles: usize,
    pub hull: u32,
    pub weight_bones: Vec<String>,
}
impl MeshSummary {
    fn new(slot: &str, skin: Option<&str>, mesh: &Mesh, bone: &str) -> Self {
        Self {
            slot: slot.into(),
            attachment: mesh.name.clone(),
            skin: skin.map(str::to_string),
            vertices: mesh.vertices.len(),
            triangles: mesh.triangles.len(),
            hull: mesh.hull,
            weight_bones: mesh
                .vertices
                .iter()
                .flat_map(|v| {
                    if v.weights.is_empty() {
                        vec![bone.to_string()]
                    } else {
                        v.weights
                            .iter()
                            .filter(|w| w.weight > 0.0)
                            .map(|w| w.bone.clone())
                            .collect()
                    }
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub document: Document,
    /// Fully materialized edits; history never needs to reopen an image.
    pub ops: Vec<Op>,
    pub meshes: Vec<MeshSummary>,
    /// Total vertex/bone influences removed by prune operations in this batch.
    pub pruned_influences: usize,
}
pub fn apply_any(doc: &Document, base: &Path, ops: &[AnyOp]) -> skel2d_core::Result<ApplyReport> {
    let mut meshes = Vec::new();
    let mut pruned_influences=0;
    let (document, ops) = skel2d_core::ops::apply_expanded(doc, ops, |current, op| match op {
        AnyOp::Core(op) => {
            if let Op::PruneWeights {slot,attachment:name,skin,vertices,max_bones,threshold,locked_bones}=op
                && !vertices.as_ref().is_some_and(|v|v.is_empty()) && let Attachment::Mesh(mesh)=attachment(current,slot,name,skin.as_deref())? {
                    pruned_influences+=skel2d_core::mesh::prune_count(mesh,&current.slot(slot).unwrap().bone,vertices.as_deref().unwrap_or(&[]),*max_bones,*threshold,&locked_bones.iter().cloned().collect());
            }
            Ok(vec![op.clone()])
        },
        AnyOp::Asset(op) => {
            let (ops, summary) = expand_one(current, base, op)?;
            meshes.push(summary);
            Ok(ops)
        }
    })?;
    for op in &ops {
        let at=match op {
            Op::SmoothWeights {slot,attachment,skin,..}|Op::PruneWeights {slot,attachment,skin,..}|Op::SwapWeights {slot,attachment,skin,..}|Op::SetBoneWeight {slot,attachment,skin,..}|Op::BindBones {slot,attachment,skin,..}|Op::NormalizeWeights {slot,attachment,skin,..}|Op::AutoWeights {slot,attachment,skin,..}|Op::MirrorWeights {slot,attachment,skin,..}|Op::TransferWeights {slot,attachment,skin,..}=>Some((slot,attachment,skin)),
            _=>None,
        };
        if let Some((slot,name,skin))=at
            && let Ok(Attachment::Mesh(mesh))=attachment(&document,slot,name,skin.as_deref())
            && !meshes.iter().any(|m| &m.slot==slot && &m.attachment==name && &m.skin==skin)
        { meshes.push(MeshSummary::new(slot,skin.as_deref(),mesh,&document.slot(slot).unwrap().bone)); }
    }
    Ok(ApplyReport {
        document,
        ops,
        meshes,
        pruned_influences,
    })
}
pub fn expand(doc: &Document, base: &Path, ops: &[AssetOp]) -> skel2d_core::Result<Vec<Op>> {
    apply_any(
        doc,
        base,
        &ops.iter().cloned().map(AnyOp::Asset).collect::<Vec<_>>(),
    )
    .map(|report| report.ops)
}
fn attachment<'a>(
    doc: &'a Document,
    slot: &str,
    name: &str,
    skin: Option<&str>,
) -> Result<&'a Attachment, String> {
    let s = doc
        .slot(slot)
        .ok_or_else(|| format!("slot {slot:?} does not exist"))?;
    let value = match skin {
        None => s.attachment(name),
        Some(k) => doc
            .skin(k)
            .ok_or_else(|| format!("skin {k:?} does not exist"))?
            .attachment(slot, name),
    };
    value.ok_or_else(|| {
        format!("attachment {name:?} does not exist in selected skin of slot {slot:?}")
    })
}
fn image(attachment: &Attachment) -> Result<&str, String> {
    match attachment {
        Attachment::Region(r) => r.image.as_deref(),
        Attachment::Mesh(m) => m.image.as_deref(),
        Attachment::LinkedMesh(m) => m.image.as_deref(),
    }
    .ok_or_else(|| "attachment has no image".into())
}
fn load_image(base: &Path, path: &str) -> Result<Pixmap, String> {
    let pixmap =
        Pixmap::load_png(base.join(path)).map_err(|e| format!("cannot load {path}: {e}"))?;
    if pixmap.width() as u64 * pixmap.height() as u64 > 16_777_216 {
        return Err("image exceeds 16777216 pixels".into());
    }
    Ok(pixmap)
}
fn skeleton_pose(doc: &Document) -> Result<Pose, String> {
    // Intermediate mixed batches may have unfinished slot references. Only
    // bones are needed here; validate their order and values before posing.
    let skeleton = Document {
        bones: doc.bones.clone(),
        ..Document::default()
    };
    let issues = skel2d_core::validate(&skeleton);
    if !issues.is_empty() {
        return Err(issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "));
    }
    Ok(Pose::compute(&skeleton, None))
}
fn set_attachment(slot: &str, skin: Option<&str>, mesh: Mesh) -> Op {
    match skin {
        None => Op::SetAttachment {
            slot: slot.into(),
            attachment: Attachment::Mesh(mesh),
        },
        Some(k) => Op::SetSkinAttachment {
            skin: k.into(),
            slot: slot.into(),
            attachment: Attachment::Mesh(mesh),
        },
    }
}
fn expand_one(doc: &Document, base: &Path, op: &AssetOp) -> Result<(Vec<Op>, MeshSummary), String> {
    match op {
        AssetOp::TraceMesh {
            slot,
            attachment: name,
            skin,
            name: new_name,
            params,
        } => trace(
            doc,
            base,
            slot,
            name,
            skin.as_deref(),
            new_name.as_deref(),
            params,
        ),
        AssetOp::ImageWeights {
            locked_bones,
            slot,
            attachment: name,
            skin,
            bones,
            max_influences,
            falloff,
            vertices,
        } => {
            let Attachment::Mesh(old) = attachment(doc, slot, name, skin.as_deref())? else {
                return Err("image_weights requires a mesh".into());
            };
            if locked_bones.iter().any(|b|doc.bone(b).is_none()) { return Err("locked bone does not exist".into()); }
            let mut mesh = image_weight_mesh(
                doc,
                base,
                old,
                bones.as_deref().or_else(||(!old.bones.is_empty()).then_some(old.bones.as_slice())),
                *max_influences,
                *falloff,
                vertices.as_deref(),
            )?;
            let bone=&doc.slot(slot).unwrap().bone;
            for (i,v) in mesh.vertices.iter_mut().enumerate() {
                if vertices.as_ref().is_none_or(|ids|ids.contains(&i)) {
                    v.weights=skel2d_core::mesh::preserve_locked(&old.vertices[i],&skel2d_core::mesh::weight_map(v,bone),bone,&locked_bones.iter().cloned().collect());
                }
            }
            let patches = mesh
                .vertices
                .iter()
                .enumerate()
                .filter(|(i, _)| vertices.as_ref().is_none_or(|v| v.contains(i)))
                .map(|(index, v)| skel2d_core::ops::VertexPatch {
                    index,
                    weights: Some(v.weights.clone()),
                    ..Default::default()
                })
                .collect();
            let summary =
                MeshSummary::new(slot, skin.as_deref(), &mesh, &doc.slot(slot).unwrap().bone);
            Ok((
                vec![Op::SetVertices {
                    slot: slot.clone(),
                    attachment: name.clone(),
                    skin: skin.clone(),
                    vertices: patches,
                }],
                summary,
            ))
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn trace(
    doc: &Document,
    base: &Path,
    slot: &str,
    name: &str,
    skin: Option<&str>,
    new_name: Option<&str>,
    params: &TraceParams,
) -> Result<(Vec<Op>, MeshSummary), String> {
    params.validate()?;
    let old = attachment(doc, slot, name, skin)?;
    let path = image(old)?;
    let pm = load_image(base, path)?;
    let mask = Mask::from_pixmap(&pm, (params.alpha_threshold * 255.0).floor() as u8);
    if mask.count() == 0 {
        return Err(format!(
            "{path} is fully transparent at this alpha threshold"
        ));
    }
    let pose = skeleton_pose(doc)?;
    let slot_bone = &doc.slot(slot).unwrap().bone;
    let bone = doc
        .bone_index(slot_bone)
        .ok_or_else(|| format!("bone {slot_bone:?} does not exist"))?;
    let uv_world = match old {
        Attachment::Region(r) => pose.bones[bone]
            .world
            .mul(&Affine::from_trs(
                r.x, r.y, r.rotation, r.scale_x, r.scale_y,
            ))
            .mul(
                &Affine::translate(-r.width / 2.0, r.height / 2.0)
                    .mul(&Affine::scale(r.width, -r.height)),
            ),
        Attachment::Mesh(m) => fit_uv_world(m).ok_or("the mesh is too degenerate to rebuild")?,
        Attachment::LinkedMesh(_) => return Err("linked mesh geometry is inherited; edit its parent".into()),
    };
    let inverse = uv_world
        .inverse()
        .ok_or("attachment image placement is singular")?;
    let target = new_name.unwrap_or(name);
    if target.trim().is_empty() {
        return Err("attachment name is empty".into());
    }
    let spacing = params
        .spacing
        .unwrap_or((pm.width().max(pm.height()) as f32 / (4.5 + 6.5 * params.detail)).max(4.0));
    let tolerance = params
        .concavity
        .unwrap_or((spacing / 8.0).clamp(0.25, 4.0) * (1.5 - params.detail));
    let mut joints: Vec<[f32; 2]> = Vec::new();
    if params.interior == Interior::Auto {
        for (b, p) in doc.bones.iter().zip(&pose.bones) {
            for xy in [p.world.apply(0.0, 0.0), p.world.apply(b.length, 0.0)] {
                let uv = inverse.apply(xy[0], xy[1]);
                let point = [uv[0] * pm.width() as f32, uv[1] * pm.height() as f32];
                if joints
                    .iter()
                    .all(|q| (q[0] - point[0]).hypot(q[1] - point[1]) > 2.0)
                {
                    joints.push(point);
                }
            }
        }
    }
    let pixel_mesh = trace_outline_mesh(
        &mask,
        target,
        Some(path.into()),
        OutlineParams { spacing, tolerance },
        Placement {
            origin: [0.0, 0.0],
            scale: 1.0,
        },
        &joints,
        params.padding,
        params.interior,
    )?;
    let mut mesh = pixel_mesh;
    for v in &mut mesh.vertices {
        let xy = uv_world.apply(v.u, v.v);
        v.x = xy[0];
        v.y = xy[1];
    }
    mesh = skel2d_core::mesh::triangulate(&mesh)?.mesh;
    if let Attachment::Mesh(source) = old {
        mesh.bones=source.bones.clone();
        mesh = preserve_edges(mesh, source, slot_bone)?;
        if params.keep_weights {
            skel2d_core::ops::transfer_mesh_weights(&mut mesh, source, slot_bone);
        }
    }
    let summary = MeshSummary::new(slot, skin, &mesh, slot_bone);
    let remap = if let Attachment::Mesh(source) = old {
        Some(
            mesh.vertices
                .iter()
                .map(|v| skel2d_core::mesh::interpolation_parts(source, [v.x, v.y]))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        None
    };
    let mut ops = Vec::new();
    if target != name {
        ops.push(Op::RenameAttachment {
            slot: slot.into(),
            from: name.into(),
            to: target.into(),
            skin: skin.map(str::to_string),
        });
    }
    ops.push(set_attachment(slot, skin, mesh));
    // Retracing preserves each owning skin's deformation
    // by sampling the original setup triangles, including key curves.
    if let Some(remap) = remap {
        // Linked children with their own timelines share the parent's vertices.
        let mut paths = vec![(skin.map(str::to_string), name.to_string(), target.to_string())];
        paths.extend(
            skel2d_core::ops::linked_children(doc, slot, name, skin)
                .into_iter()
                .filter(|c| !c.timelines)
                .map(|c| (c.owner, c.name.clone(), c.name)),
        );
        for (animation, a) in &doc.animations {
            for (owner, from, to) in &paths {
                let Some(keys) = a
                    .deform
                    .get(owner.as_deref().unwrap_or("default"))
                    .and_then(|s| s.get(slot))
                    .and_then(|s| s.get(from))
                else {
                    continue;
                };
                for key in keys {
                    let offsets = remap
                        .iter()
                        .map(|parts| {
                            let mut out = [0.0, 0.0];
                            for &(i, f) in parts {
                                let v = key
                                    .value
                                    .get(i)
                                    .ok_or("deform key does not match the source mesh")?;
                                out[0] += v[0] * f;
                                out[1] += v[1] * f;
                            }
                            Ok(out)
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    ops.push(Op::KeyDeform {
                        animation: animation.clone(),
                        slot: slot.into(),
                        attachment: to.clone(),
                        skin: owner.clone(),
                        time: key.time,
                        offsets,
                        curve: Some(key.curve),
                    });
                }
            }
        }
    }
    Ok((ops, summary))
}
fn preserve_edges(mut mesh: Mesh, source: &Mesh, bone: &str) -> Result<Mesh, String> {
    let mut constraints = Vec::new();
    for edge in &source.edges {
        let mut points = Vec::new();
        for &i in edge {
            let vertex = source
                .vertices
                .get(i as usize)
                .ok_or("source edge index out of range")?;
            let parts = skel2d_core::mesh::interpolation_parts(&mesh, [vertex.x, vertex.y])?;
            let p = parts.iter().fold([0.0, 0.0], |mut p, &(i, f)| {
                p[0] += mesh.vertices[i].x * f;
                p[1] += mesh.vertices[i].y * f;
                p
            });
            if !mesh
                .vertices
                .iter()
                .any(|v| (v.x - p[0]).hypot(v.y - p[1]) < 0.0001)
            {
                mesh = skel2d_core::mesh::insert_vertex(&mesh, p, 0.0001, bone)?.mesh;
            }
            points.push(p);
        }
        constraints.push(points);
    }
    for points in constraints {
        let indices: Vec<_> = points
            .iter()
            .map(|p| {
                mesh.vertices
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        let d = |v: &skel2d_core::MeshVertex| (v.x - p[0]).hypot(v.y - p[1]);
                        d(a.1).total_cmp(&d(b.1))
                    })
                    .map(|(i, _)| i as u32)
                    .unwrap()
            })
            .collect();
        if indices[0] == indices[1] {
            return Err("retrace would collapse a constrained edge".into());
        }
        mesh.edges.push([indices[0], indices[1]]);
    }
    if !mesh.edges.is_empty() {
        mesh = skel2d_core::mesh::triangulate(&mesh)?.mesh;
    }
    Ok(mesh)
}

/// Weights measured through visible pixels. Only bones overlapping the image
/// take part by default; explicit candidate names override that selection.
pub fn image_weight_mesh(
    doc: &Document,
    base: &Path,
    mesh: &Mesh,
    bones: Option<&[String]>,
    max_influences: usize,
    falloff: f32,
    vertices: Option<&[usize]>,
) -> Result<Mesh, String> {
    if !(1..=8).contains(&max_influences)
        || !falloff.is_finite()
        || !(0.1..=16.0).contains(&falloff)
    {
        return Err("max_influences must be 1..=8 and falloff must be finite in 0.1..=16".into());
    }
    if vertices.is_some_and(|v| v.iter().any(|&i| i >= mesh.vertices.len())) {
        return Err("selected vertex index out of range".into());
    }
    if vertices.is_some_and(|v|v.iter().collect::<BTreeSet<_>>().len()!=v.len()) {
        return Err("duplicate selected vertex index".into());
    }
    if let Some(bones) = bones {
        let unique: BTreeSet<_> = bones.iter().collect();
        if bones.is_empty()
            || unique.len() != bones.len()
            || bones.iter().any(|b| doc.bone(b).is_none())
        {
            return Err("candidate bones must be nonempty, unique and existing".into());
        }
    }
    let pm = load_image(base, mesh.image.as_deref().ok_or("the mesh has no image")?)?;
    let mask = Mask::from_pixmap(&pm, ALPHA_THRESHOLD);
    if mask.count() == 0 {
        return Err("the image is fully transparent".into());
    }
    let inverse = fit_uv_world(mesh)
        .and_then(|m| m.inverse())
        .ok_or("the mesh is too degenerate to rebuild")?;
    let to_px = |xy: [f32; 2]| {
        let uv = inverse.apply(xy[0], xy[1]);
        [uv[0] * pm.width() as f32, uv[1] * pm.height() as f32]
    };
    let pose = skeleton_pose(doc)?;
    let segments: Vec<_> = doc
        .bones
        .iter()
        .zip(&pose.bones)
        .filter(|(b, _)| match bones {
            Some(names) => names.contains(&b.name),
            None => !doc.ik.iter().any(|k| k.target == b.name),
        })
        .map(|(b, p)| BoneSegment {
            name: b.name.clone(),
            start: to_px(p.world.apply(0.0, 0.0)),
            end: to_px(p.world.apply(b.length, 0.0)),
        })
        .collect();
    if segments.is_empty() {
        return Err("no eligible bones for image weights".into());
    }
    let overlapping: Vec<_> = segments
        .iter()
        .filter(|s| solid_length(&mask, s.start, s.end) >= 4.0)
        .map(|s| BoneSegment {
            name: s.name.clone(),
            start: s.start,
            end: s.end,
        })
        .collect();
    let segments = if bones.is_none() && !overlapping.is_empty() {
        overlapping
    } else {
        segments
    };
    let mut pixels = mesh.clone();
    for v in &mut pixels.vertices {
        v.x = v.u * pm.width() as f32;
        v.y = -v.v * pm.height() as f32;
    }
    geodesic_weights(
        &mut pixels,
        &mask,
        &segments,
        Placement {
            origin: [0.0, 0.0],
            scale: 1.0,
        },
        max_influences,
        falloff,
    );
    let mut out = mesh.clone();
    for (i, (v, p)) in out.vertices.iter_mut().zip(pixels.vertices).enumerate() {
        if vertices.is_none_or(|indices| indices.contains(&i)) {
            v.weights = p.weights;
        }
    }
    Ok(out)
}

/// Least-squares affine map from UV to setup-pose position.
pub fn fit_uv_world(mesh: &Mesh) -> Option<Affine> {
    let n = mesh.vertices.len() as f64;
    if n < 3.0 {
        return None;
    }
    let (cu, cv) = mesh.vertices.iter().fold((0.0, 0.0), |(a, b), v| {
        (a + v.u as f64 / n, b + v.v as f64 / n)
    });
    let (mut suu, mut suv, mut svv) = (0.0f64, 0.0f64, 0.0f64);
    let (mut sx, mut sy) = ([0.0f64; 3], [0.0f64; 3]);
    for vert in &mesh.vertices {
        let (u, v) = (vert.u as f64 - cu, vert.v as f64 - cv);
        suu += u * u;
        suv += u * v;
        svv += v * v;
        for (s, t) in [(&mut sx, vert.x as f64), (&mut sy, vert.y as f64)] {
            s[0] += u * t;
            s[1] += v * t;
            s[2] += t;
        }
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return None;
    }
    let solve = |s: [f64; 3]| {
        let p = (s[0] * svv - s[1] * suv) / det;
        let q = (s[1] * suu - s[0] * suv) / det;
        (p as f32, q as f32, (s[2] / n - p * cu - q * cv) as f32)
    };
    let ((a, c, tx), (b, d, ty)) = (solve(sx), solve(sy));
    Some(Affine { a, b, c, d, tx, ty })
}
