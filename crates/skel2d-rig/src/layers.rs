//! Layered characters: one PNG per part (as exported from a paint program),
//! placed on a shared canvas. Each layer becomes a slot. Large parts get a
//! weighted mesh whose weights only flow through that part's own pixels, so
//! an arm layer is driven by arm bones even where it overlaps the torso.
//! Small parts (eyes, mouth, emotes) become rigid regions on the nearest
//! bone. Variants are alternative images for the same slot (eyes closed,
//! mouth shapes) used by face poses and animations that switch images.

use schemars::JsonSchema;
use serde::Deserialize;
use skel2d_core::{Attachment, Document, Op, Pose, Region, Slot};
use tiny_skia::Pixmap;

use crate::mask::Mask;
use crate::mesh::{OutlineParams, Placement, grid_mesh, outline_mesh};
use crate::template::{self, Built};
use crate::weights::{BoneSegment, geodesic_weights, segment_distance};
use crate::{ALPHA_THRESHOLD, RigOptions, RigReport, RigSpec, skeleton};

/// `layers.json`: layers back to front, positions in canvas pixels.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerManifest {
    /// Canvas size; defaults to the extent of all layers.
    #[serde(default)]
    pub canvas: Option<[u32; 2]>,
    pub layers: Vec<LayerSpec>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerSpec {
    /// Slot name.
    pub name: String,
    /// PNG path relative to the manifest.
    pub image: String,
    /// Top-left corner on the canvas (0, 0 for full-canvas layers).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    /// Center on the canvas; overrides x/y (handy when placing parts by eye).
    #[serde(default)]
    pub center: Option<[f32; 2]>,
    /// Image pixels to canvas pixels, e.g. 0.8 to shrink a too-large part.
    #[serde(default = "one")]
    pub scale: f32,
    /// Name of the default attachment; defaults to the layer name.
    #[serde(default)]
    pub attachment: Option<String>,
    /// Attach rigidly to this bone.
    #[serde(default)]
    pub bone: Option<String>,
    /// Restrict mesh weights to these bones.
    #[serde(default)]
    pub bones: Option<Vec<String>>,
    /// Force a weighted mesh (true) or a rigid region (false). Default: mesh
    /// for parts covering at least 1.5% of the character.
    #[serde(default)]
    pub mesh: Option<bool>,
    /// Hidden in the setup pose (emotes, alternative props).
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub variants: Vec<VariantSpec>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VariantSpec {
    pub name: String,
    pub image: String,
    /// Top-left on the canvas; defaults to centering on the layer's image.
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    #[serde(default)]
    pub center: Option<[f32; 2]>,
    /// Defaults to the layer's scale.
    #[serde(default)]
    pub scale: Option<f32>,
}

fn one() -> f32 {
    1.0
}

/// A layer with its images loaded. `path` is what the document stores.
pub struct LayerInput {
    pub name: String,
    pub attachment: String,
    pub image: Pixmap,
    pub path: String,
    pub offset: [f32; 2],
    pub scale: f32,
    pub bone: Option<String>,
    pub bones: Option<Vec<String>>,
    pub mesh: Option<bool>,
    pub hidden: bool,
    pub variants: Vec<VariantInput>,
}

pub struct VariantInput {
    pub name: String,
    pub image: Pixmap,
    pub path: String,
    pub offset: Option<[f32; 2]>,
    pub scale: f32,
}

/// Parts smaller than this share of the whole character become rigid regions.
const MESH_MIN_SHARE: f32 = 0.015;

pub fn rig_layers(layers: &[LayerInput], canvas: Option<[u32; 2]>, spec: &RigSpec, opts: &RigOptions) -> Result<(Document, RigReport), String> {
    if layers.is_empty() {
        return Err("no layers".into());
    }
    let masks: Vec<Mask> = layers.iter().map(|l| Mask::from_pixmap(&l.image, ALPHA_THRESHOLD)).collect();
    let [cw, ch] = canvas.unwrap_or_else(|| {
        let ext = |f: &dyn Fn(&LayerInput) -> f32| layers.iter().map(f).fold(1.0f32, f32::max).ceil() as u32;
        [ext(&|l| l.offset[0] + l.image.width() as f32 * l.scale), ext(&|l| l.offset[1] + l.image.height() as f32 * l.scale)]
    });
    let composite = Mask::from_fn(cw as usize, ch as usize, |x, y| {
        layers.iter().zip(&masks).any(|(l, m)| {
            let (lx, ly) = ((x as f32 - l.offset[0]) / l.scale, (y as f32 - l.offset[1]) / l.scale);
            lx >= 0.0 && ly >= 0.0 && m.get(lx as usize, ly as usize)
        })
    });
    let total = composite.count();
    let bbox = composite.bbox().ok_or("layers have no visible pixels")?;
    let place = Placement { origin: [(bbox.x0 + bbox.x1) as f32 / 2.0, bbox.y1 as f32], scale: 1.0 };

    let built = build(spec, &composite, place)?;
    let mut ops: Vec<Op> = vec![Op::SetName { name: opts.name.clone() }];
    ops.extend(built.ops.iter().cloned());
    let doc = skel2d_core::apply(&Document::new(""), &ops).map_err(|e| e.to_string())?;
    let first_bone = doc.bones.first().map(|b| b.name.clone()).ok_or("rig produced no bones")?;
    let pose = Pose::compute(&doc, None);
    // Deforming bones as canvas-pixel segments.
    let segments: Vec<BoneSegment> = doc
        .bones
        .iter()
        .zip(&pose.bones)
        .filter(|(b, _)| !built.non_deforming.contains(&b.name))
        .map(|(b, p)| BoneSegment {
            name: b.name.clone(),
            start: place.to_pixel(p.world.apply(0.0, 0.0)),
            end: place.to_pixel(p.world.apply(b.length, 0.0)),
        })
        .collect();
    let cell = opts.cell.unwrap_or_else(|| (bbox.width().max(bbox.height()) / 16).max(8));

    let mut warnings = built.warnings.clone();
    let (mut vertices, mut triangles) = (0, 0);
    let mut slot_ops = Vec::new();
    for (layer, mask) in layers.iter().zip(&masks) {
        let Some(lb) = mask.bbox() else {
            warnings.push(format!("layer {:?} is fully transparent; skipped", layer.name));
            continue;
        };
        if let Some(b) = layer.bone.as_ref().filter(|b| doc.bone(b).is_none()) {
            return Err(format!("layer {:?}: bone {b:?} does not exist", layer.name));
        }
        let share = mask.count() as f32 / total.max(1) as f32;
        let use_mesh = layer.mesh.unwrap_or(layer.bone.is_none() && share >= MESH_MIN_SHARE) && !segments.is_empty();
        let s = layer.scale;
        let center = [layer.offset[0] + (lb.x0 + lb.x1) as f32 / 2.0 * s, layer.offset[1] + (lb.y0 + lb.y1) as f32 / 2.0 * s];
        let rigid_bone = layer.bone.clone().unwrap_or_else(|| nearest_bone(&segments, center).unwrap_or_else(|| first_bone.clone()));
        let candidates: Vec<&BoneSegment> = match &layer.bones {
            Some(list) => segments.iter().filter(|s| list.contains(&s.name)).collect(),
            None => segments.iter().collect(),
        };

        let base_center = [layer.offset[0] + layer.image.width() as f32 * s / 2.0, layer.offset[1] + layer.image.height() as f32 * s / 2.0];
        let images = std::iter::once((layer.attachment.as_str(), &layer.image, layer.path.as_str(), layer.offset, s)).chain(
            layer.variants.iter().map(|v| {
                let vs = v.scale;
                let off = v.offset.unwrap_or([base_center[0] - v.image.width() as f32 * vs / 2.0, base_center[1] - v.image.height() as f32 * vs / 2.0]);
                (v.name.as_str(), &v.image, v.path.as_str(), off, vs)
            }),
        );
        let mut attachments = Vec::new();
        for (name, img, path, offset, s) in images {
            let image = (!path.is_empty()).then(|| path.to_string());
            if use_mesh {
                let m_mask = Mask::from_pixmap(img, ALPHA_THRESHOLD);
                // Image pixel p lands on canvas pixel offset + p * s.
                let local = Placement { origin: [(place.origin[0] - offset[0]) / s, (place.origin[1] - offset[1]) / s], scale: place.scale * s };
                let segs: Vec<BoneSegment> = candidates
                    .iter()
                    .map(|b| BoneSegment {
                        name: b.name.clone(),
                        start: [(b.start[0] - offset[0]) / s, (b.start[1] - offset[1]) / s],
                        end: [(b.end[0] - offset[0]) / s, (b.end[1] - offset[1]) / s],
                    })
                    .collect();
                let mut mesh = if opts.grid_mesh {
                    grid_mesh(&m_mask, name, image, cell, local)
                } else {
                    // Bones bend at their ends, so pack vertices there.
                    let joints: Vec<[f32; 2]> = segs.iter().flat_map(|b| [b.start, b.end]).collect();
                    outline_mesh(&m_mask, name, image, OutlineParams::from_spacing(cell as f32 / s), local, &joints)
                };
                geodesic_weights(&mut mesh, &m_mask, &segs, local, opts.max_influences, opts.falloff);
                vertices += mesh.vertices.len();
                triangles += mesh.triangles.len();
                attachments.push(Attachment::Mesh(mesh));
            } else {
                let bi = doc.bone_index(&rigid_bone).expect("bone checked or chosen from doc");
                let w = &pose.bones[bi].world;
                let c = place.to_world([offset[0] + img.width() as f32 * s / 2.0, offset[1] + img.height() as f32 * s / 2.0]);
                let [x, y] = w.inverse().ok_or("bone has zero scale")?.apply(c[0], c[1]);
                attachments.push(Attachment::Region(Region {
                    name: name.to_string(),
                    image,
                    x: round2(x),
                    y: round2(y),
                    rotation: round2(-w.rotation_deg()),
                    scale_x: 1.0,
                    scale_y: 1.0,
                    width: round2(img.width() as f32 * s),
                    height: round2(img.height() as f32 * s),
                }));
                vertices += 4;
                triangles += 2;
            }
        }
        slot_ops.push(Op::AddSlot {
            slot: Slot {
                name: layer.name.clone(),
                bone: if use_mesh { first_bone.clone() } else { rigid_bone },
                color: Default::default(),
                blend: Default::default(),
                attachment: (!layer.hidden).then(|| layer.attachment.clone()),
                attachments,
            },
            index: None,
        });
    }
    let doc = skel2d_core::apply(&doc, &slot_ops).map_err(|e| e.to_string())?;
    let report = RigReport { bones: doc.bones.len(), ik: doc.ik.len(), vertices, triangles, warnings, origin_px: place.origin };
    Ok((doc, report))
}

fn build(spec: &RigSpec, mask: &Mask, place: Placement) -> Result<Built, String> {
    match spec {
        RigSpec::Humanoid { joints, leg_ik, arm_ik } => template::humanoid(joints, *leg_ik, *arm_ik, place),
        RigSpec::Chain { joints: Some(j), prefix, .. } => template::chain(j, prefix, place),
        RigSpec::Chain { joints: None, segments, root, prefix } => {
            let joints = skeleton::chain_joints(mask, *segments, (*root).into())
                .ok_or("could not trace a medial axis; pass joints explicitly")?;
            template::chain(&joints, prefix, place)
        }
        RigSpec::Custom { bones } => template::custom(bones, place),
    }
}

fn nearest_bone(segments: &[BoneSegment], p: [f32; 2]) -> Option<String> {
    segments
        .iter()
        .map(|s| (segment_distance(p, s.start, s.end), &s.name))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, n)| n.clone())
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}
