//! Auto-rigging. The split of work is deliberate: an agent looks at the
//! image (with a labeled coordinate grid) and only states where joints are,
//! in pixels; this crate does everything numeric and deterministic —
//! placing the image in world space, building bones and IK, tracing a mesh
//! from the alpha channel and computing weights. The result is an ordinary
//! document that can be refined with ops.

pub mod grid;
pub mod asset_ops;
pub mod layers;
pub mod mask;
pub mod mesh;
pub mod psd;
pub mod skeleton;
pub mod slice;
pub mod template;
pub mod weights;

use schemars::JsonSchema;
use serde::Serialize;
use skel2d_core::Document;
use tiny_skia::Pixmap;

pub use mask::{Mask, PixelRect};
pub use mesh::Placement;
pub use template::RigSpec;
pub use asset_ops::{AnyOp, AssetOp, ApplyReport, MeshSummary, TraceParams, Interior, apply_any, expand, parse_ops};

/// Alpha above this counts as part of the character.
pub const ALPHA_THRESHOLD: u8 = 16;

#[derive(Debug, Clone)]
pub struct RigOptions {
    /// Image path to store in the document (relative to the document file).
    pub image_path: String,
    /// Mesh density: grid cell size, or interior vertex spacing of outline
    /// meshes, in pixels; `None` picks about 16 on the long side.
    pub cell: Option<usize>,
    /// Regular grid meshes instead of adaptive outline meshes.
    pub grid_mesh: bool,
    /// Region instead of a weighted mesh (rigid, one bone).
    pub no_mesh: bool,
    pub max_influences: usize,
    pub falloff: f32,
    pub name: String,
}

impl Default for RigOptions {
    fn default() -> Self {
        Self { image_path: String::new(), cell: None, grid_mesh: false, no_mesh: false, max_influences: 3, falloff: 3.0, name: "character".into() }
    }
}

/// What image analysis found; useful to agents before writing a spec.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    /// Bounds of the visible (non-transparent) pixels.
    pub bbox: Option<PixelRect>,
    /// Share of fully hidden pixels; near 0 means the image has no real
    /// transparency and the mesh will cover the whole rectangle.
    pub transparent_ratio: f32,
    /// Separate visible islands, largest first (at most 16).
    pub components: Vec<mask::Component>,
}

pub fn image_info(img: &Pixmap) -> ImageInfo {
    let mask = Mask::from_pixmap(img, ALPHA_THRESHOLD);
    let total = (mask.width * mask.height).max(1);
    let mut components = mask.components();
    // Ignore specks from antialiasing noise.
    components.retain(|c| c.pixels * 2000 >= total);
    components.truncate(16);
    ImageInfo {
        width: img.width(),
        height: img.height(),
        bbox: mask.bbox(),
        transparent_ratio: 1.0 - mask.count() as f32 / total as f32,
        components,
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RigReport {
    pub bones: usize,
    pub ik: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub warnings: Vec<String>,
    /// World origin in image pixels (bottom center of the visible area).
    pub origin_px: [f32; 2],
}

/// The placement used by `rig`: 1 world unit per pixel, origin at the
/// bottom center of the visible pixels so feet stand on y = 0.
pub fn placement_for(img: &Pixmap) -> Placement {
    let mask = Mask::from_pixmap(img, ALPHA_THRESHOLD);
    let origin = match mask.bbox() {
        Some(b) => [(b.x0 + b.x1) as f32 / 2.0, b.y1 as f32],
        None => [img.width() as f32 / 2.0, img.height() as f32],
    };
    Placement { origin, scale: 1.0 }
}

/// Builds a rigged document from a single image and a spec.
pub fn rig(img: &Pixmap, spec: &RigSpec, opts: &RigOptions) -> Result<(Document, RigReport), String> {
    // A single image is one full-canvas layer; a rigid one sits on the root.
    let rigid_bone = match spec {
        RigSpec::Custom { bones } => bones.first().map(|b| b.name.clone()),
        _ => Some(template::ROOT.to_string()),
    };
    let layer = layers::LayerInput {
        name: "body".into(),
        attachment: "image".into(),
        image: img.clone(),
        path: opts.image_path.clone(),
        offset: [0.0, 0.0],
        scale: 1.0,
        bone: if opts.no_mesh { rigid_bone } else { None },
        bones: None,
        mesh: Some(!opts.no_mesh),
        hidden: false,
        variants: Vec::new(),
    };
    layers::rig_layers(&[layer], Some([img.width(), img.height()]), spec, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crude stick figure: torso, two arms and two legs as filled bars.
    fn figure() -> Pixmap {
        let mut pm = Pixmap::new(200, 300).unwrap();
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(200, 80, 60, 255);
        for (x, y, w, h) in [(80.0, 40.0, 40.0, 40.0), (85.0, 80.0, 30.0, 100.0), (30.0, 90.0, 55.0, 12.0), (115.0, 90.0, 55.0, 12.0), (85.0, 180.0, 12.0, 110.0), (103.0, 180.0, 12.0, 110.0)] {
            pm.fill_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap(), &p, tiny_skia::Transform::identity(), None);
        }
        pm
    }

    #[test]
    fn humanoid_rig_reproduces_joints_and_weights() {
        let spec: RigSpec = serde_json::from_str(
            r#"{"template":"humanoid","joints":{
                "pelvis":[100,180],"neck":[100,80],"head_top":[100,40],
                "shoulder_l":[85,96],"elbow_l":[58,96],"wrist_l":[30,96],
                "shoulder_r":[115,96],"elbow_r":[142,96],"wrist_r":[170,96],
                "hip_l":[91,185],"knee_l":[91,235],"ankle_l":[91,288],
                "hip_r":[109,185],"knee_r":[109,235],"ankle_r":[109,288]}}"#,
        )
        .unwrap();
        let (doc, report) = rig(&figure(), &spec, &RigOptions::default()).unwrap();
        assert_eq!(report.ik, 2);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        // Feet stand on y=0: the ankle (pixel y 288 of a figure ending at 290) is ~2 units up.
        let pose = skel2d_core::Pose::compute(&doc, None);
        let shin = doc.bone_index("shin_l").unwrap();
        let [_, y] = pose.bones[shin].world.apply(doc.bones[shin].length, 0.0);
        assert!((y - 2.0).abs() < 0.1, "ankle y {y}");
        // IK in the setup pose keeps the drawn pose.
        let knee = pose.bones[shin].world.apply(0.0, 0.0);
        assert!((knee[1] - 55.0).abs() < 0.1, "knee {knee:?}");
        // Hand vertices belong to the forearm, not the torso.
        let Some(skel2d_core::Attachment::Mesh(m)) = doc.slots[0].attachment("image") else { panic!() };
        let hand = m.vertices.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        assert_eq!(hand.weights[0].bone, "forearm_l", "{:?}", hand.weights);
    }

    #[test]
    fn chain_traces_automatically() {
        let mut pm = Pixmap::new(300, 60).unwrap();
        pm.fill_rect(tiny_skia::Rect::from_xywh(10.0, 20.0, 280.0, 20.0).unwrap(), &{
            let mut p = tiny_skia::Paint::default();
            p.set_color_rgba8(0, 0, 0, 255);
            p
        }, tiny_skia::Transform::identity(), None);
        let spec: RigSpec = serde_json::from_str(r#"{"template":"chain","segments":5,"root":"left"}"#).unwrap();
        let (doc, report) = rig(&pm, &spec, &RigOptions::default()).unwrap();
        assert_eq!(report.bones, 6);
        assert!(doc.bone("chain5").is_some());
    }
}
