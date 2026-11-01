//! Bringing images into the rig: new slots from PNG or PSD files (menu or
//! drag-and-drop), re-pointing and resizing an attachment's image, keeping a
//! region in place when its slot moves to another bone, and tracing a mesh
//! from an image's alpha channel.

use std::path::{Component, Path, PathBuf};

use skel2d_core::{Affine, Attachment, BlendMode, Bone, Color, Document, Mesh, Op, Pose, Region, Slot};
use skel2d_rig::psd::{Hidden, OriginSource, Split};

use crate::app::{Dialog, EditorApp};

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Setup pose without IK, the space region and mesh placement is defined in.
pub fn setup_pose(doc: &Document) -> Pose {
    // Built field by field: animations are most of a rig and the setup pose never reads them.
    let setup = Document {
        format: doc.format.clone(),
        name: String::new(),
        bones: doc.bones.clone(),
        slots: doc.slots.clone(),
        ik: Vec::new(),
        physics: doc.physics.clone(),
        transforms: Vec::new(),
        physics_globals: doc.physics_globals.clone(),
        events: Default::default(),
        skins: doc.skins.clone(),
        animations: Default::default(),
    };
    Pose::compute(&setup, None)
}

fn bone_world(doc: &Document, pose: &Pose, bone: &str) -> Affine {
    doc.bone_index(bone).map_or(Affine::IDENTITY, |i| pose.bones[i].world)
}

/// `after` with every mesh carried along by the bones' setup-pose change
/// since `before`. Mesh vertices are stored in setup world space, so
/// without this a mesh stays put while its bones (and regions) move.
pub fn carry_meshes(before: &Document, mut after: Document) -> Result<Document, String> {
    let (p0, p1) = (setup_pose(before), setup_pose(&after));
    let moves: Vec<Affine> = after
        .bones
        .iter()
        .zip(&p1.bones)
        .map(|(b, p)| {
            before.bone_index(&b.name).and_then(|i| p0.bones[i].world.inverse()).map_or(Affine::IDENTITY, |inv| p.world.mul(&inv))
        })
        .collect();
    if moves.iter().all(|m| *m == Affine::IDENTITY) {
        return Ok(after);
    }
    let index: std::collections::HashMap<&str, usize> = after.bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i)).collect();
    let mut slots = std::mem::take(&mut after.slots);
    // Meshes no moved bone touches keep their triangles untouched below.
    let mut moved_meshes = std::collections::HashSet::new();
    for slot in &mut slots {
        let own = index.get(slot.bone.as_str()).map_or(Affine::IDENTITY, |&i| moves[i]);
        for att in &mut slot.attachments {
            let Attachment::Mesh(mesh) = att else { continue };
            let affected = |v: &skel2d_core::MeshVertex| {
                if v.weights.iter().all(|w| w.weight <= 0.0) { own != Affine::IDENTITY } else { v.weights.iter().any(|w| w.weight > 0.0 && index.get(w.bone.as_str()).is_none_or(|&i| moves[i] != Affine::IDENTITY)) }
            };
            if !mesh.vertices.iter().any(affected) {
                continue;
            }
            moved_meshes.insert((slot.name.clone(), mesh.name.clone()));
            for v in &mut mesh.vertices {
                let (mut x, mut y, mut total) = (0.0, 0.0, 0.0);
                for w in v.weights.iter().filter(|w| w.weight > 0.0) {
                    let Some(&i) = index.get(w.bone.as_str()) else { continue };
                    let [px, py] = moves[i].apply(v.x, v.y);
                    (x, y, total) = (x + px * w.weight, y + py * w.weight, total + w.weight);
                }
                let [nx, ny] = if total > 0.0 { [x / total, y / total] } else { own.apply(v.x, v.y) };
                (v.x, v.y) = (round2(nx), round2(ny));
            }
        }
    }
    after.slots = slots;
    // Weighted vertices move by different amounts, so a mesh can end up tangled
    // for a while. That must not stop the bone: it keeps its last triangles.
    let mut reversed = Vec::new();
    for slot in &mut after.slots {
        for att in &mut slot.attachments {
            let Attachment::Mesh(mesh) = att else { continue };
            if !moved_meshes.contains(&(slot.name.clone(), mesh.name.clone())) {
                continue;
            }
            if skel2d_core::mesh::hull_reversed(mesh) {
                reversed.push(Op::SetMeshEdges { slot: slot.name.clone(), attachment: mesh.name.clone(), skin: None, edges: mesh.edges.clone() });
            } else {
                skel2d_core::mesh::refit_triangles(mesh);
            }
        }
    }
    // A reflected setup transform reverses the front hull's winding. Use the
    // same core operation to normalize it and remap all deform keys.
    for op in reversed {
        if let Ok(fixed) = skel2d_core::apply(&after, std::slice::from_ref(&op)) { after = fixed; }
    }
    Ok(after)
}

/// Pixel size of a PNG.
pub fn image_size(path: &Path) -> Option<[u32; 2]> {
    tiny_skia::Pixmap::load_png(path).ok().map(|p| [p.width(), p.height()])
}

/// `file` relative to `base` with forward slashes (absolute when there is no
/// base, i.e. the document was never saved, or they are on different drives).
pub fn rel_path(base: &Path, file: &Path) -> String {
    let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if base.as_os_str().is_empty() {
        return slash(&file);
    }
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let (b, f): (Vec<Component>, Vec<Component>) = (base.components().collect(), file.components().collect());
    if b.first() != f.first() {
        return slash(&file);
    }
    let common = b.iter().zip(&f).take_while(|(x, y)| x == y).count();
    let mut rel = PathBuf::new();
    for _ in common..b.len() {
        rel.push("..");
    }
    for c in &f[common..] {
        rel.push(c.as_os_str());
    }
    slash(&rel)
}

/// Region transform relative to `bone` that puts the image upright at world
/// `center` with 1 world unit per pixel.
fn upright_on(doc: &Document, pose: &Pose, bone: &str, center: [f32; 2]) -> (f32, f32, f32, f32, f32) {
    local_trs(&bone_world(doc, pose, bone), &Affine::translate(center[0], center[1]))
}

/// Decomposes `world` into translate/rotate/scale relative to `parent`.
pub fn local_trs(parent: &Affine, world: &Affine) -> (f32, f32, f32, f32, f32) {
    let m = parent.inverse().unwrap_or(Affine::IDENTITY).mul(world);
    (round2(m.tx), round2(m.ty), round3(m.rotation_deg()), round3(m.scale_x()), round3(m.scale_y()))
}

/// The bone new images attach to: the one given, else the first root,
/// else a new `root` bone (returned as an op to apply first).
fn target_bone(doc: &Document, bone: Option<String>) -> (String, Option<Op>) {
    if let Some(b) = bone.filter(|b| doc.bone(b).is_some()) {
        return (b, None);
    }
    match doc.bones.iter().find(|b| b.parent.is_none()) {
        Some(b) => (b.name.clone(), None),
        None => ("root".into(), Some(Op::AddBone(Bone::new("root")))),
    }
}

fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = if base.is_empty() { "image" } else { base };
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base}_{i}")).find(|n| !taken(n)).expect("unbounded range")
}

/// Adds one slot per PNG, on top of the draw order, attached to `bone` (or
/// a root bone) and centered on world `at`. Several images share the
/// center, like layers exported from one canvas. A PSD adds one slot per
/// part (see [`split_psd`]), back to front, laid out as on its canvas with
/// the canvas centered on `at`; a dialog first asks how to split it. One
/// undo step.
pub fn import_images(app: &mut EditorApp, files: &[PathBuf], at: [f32; 2], bone: Option<String>) {
    if files.iter().any(|f| is_psd(f)) {
        app.dialog = Dialog::PsdSplit(PendingImport { files: files.to_vec(), at, bone, split: Split::default(), hidden: Hidden::default() });
        return;
    }
    import_images_split(app, files, at, bone, Split::default(), Hidden::default());
}

/// Files picked with the Import images command. A document that already has
/// slots first asks whether they start a new project or join this one; an
/// empty document just takes them.
pub fn import_picked(app: &mut EditorApp, files: Vec<PathBuf>) {
    if app.doc.slots.is_empty() {
        let (at, bone) = (app.camera.center, app.selection.bone.clone());
        import_images(app, &files, at, bone);
    } else {
        app.dialog = Dialog::ImportTarget(files);
    }
}

/// PNG files dropped on the viewport while a slot is picked: add them to that
/// slot or make new slots (asked in a dialog).
pub struct PendingDrop {
    pub files: Vec<PathBuf>,
    pub at: [f32; 2],
    pub bone: Option<String>,
    pub slot: String,
}

/// Dropped files: with a slot picked in the setup pose and no PSD among them,
/// ask "this slot or new slots"; otherwise import them as new slots.
pub fn dropped_images(app: &mut EditorApp, files: Vec<PathBuf>, at: [f32; 2], bone: Option<String>) {
    let slot = app.selection.slot.clone().filter(|s| app.doc.slot(s).is_some());
    match slot {
        Some(slot) if app.playback.anim.is_none() && !files.iter().any(|f| is_psd(f)) => app.dialog = Dialog::DropChoice(PendingDrop { files, at, bone, slot }),
        _ => import_images(app, &files, at, bone),
    }
}

/// Files waiting for the PSD split choice.
pub struct PendingImport {
    pub files: Vec<PathBuf>,
    pub at: [f32; 2],
    pub bone: Option<String>,
    pub split: Split,
    pub hidden: Hidden,
}

/// [`import_images`] with PSDs split by `split` and hidden layers handled by
/// `hidden`. A part kept by [`Hidden::Transparent`] gets a slot with alpha 0.
pub fn import_images_split(app: &mut EditorApp, files: &[PathBuf], at: [f32; 2], bone: Option<String>, split: Split, hidden: Hidden) {
    let frame = app.doc.slots.is_empty();
    let (bone, add_root) = target_bone(&app.doc, bone);
    let mut ops: Vec<Op> = add_root.into_iter().collect();
    let doc = match skel2d_core::apply(&app.doc, &ops) {
        Ok(d) => d,
        Err(e) => {
            app.status = e.to_string();
            return;
        }
    };
    let pose = setup_pose(&doc);
    let base = app.base_dir();
    let mut images: Vec<PlacedImage> = Vec::new();
    let mut psd_dirs = Vec::new();
    let mut origin_note = None;
    let mut skipped = Vec::new();
    for file in files {
        if !is_psd(file) {
            images.push(PlacedImage { file: file.clone(), center: at, transparent: false, blend: BlendMode::Normal, clip: None });
            continue;
        }
        match split_psd(file, split, hidden).map(|(dir, parts, source)| {
            // A PSD that names its origin puts it on the bone; otherwise the canvas center goes where it was asked.
            let explicit = source != OriginSource::Canvas;
            (dir, parts, explicit.then_some(source), if explicit { bone_world(&doc, &pose, &bone).apply(0.0, 0.0) } else { at })
        }) {
            Ok((dir, parts, source, at)) => {
                origin_note = source.or(origin_note);
                let offset = images.len();
                images.extend(parts.into_iter().map(|p| PlacedImage { center: [round2(at[0] + p.center[0]), round2(at[1] + p.center[1])], clip: p.clip.map(|c| c + offset), ..p }));
                psd_dirs.push(dir.display().to_string());
            }
            Err(e) => skipped.push(format!("{} ({e})", file.display())),
        }
    }
    let mut names: Vec<String> = Vec::new();
    // Per image, the slot it became: a clip names its base's slot.
    let mut slot_of: Vec<Option<String>> = Vec::with_capacity(images.len());
    for PlacedImage { file, center, transparent, blend, clip } in &images {
        let Some([w, h]) = image_size(file) else {
            skipped.push(file.display().to_string());
            slot_of.push(None);
            continue;
        };
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = unique(&stem, |n| doc.slot(n).is_some() || names.iter().any(|x| x == n));
        let (x, y, rotation, scale_x, scale_y) = upright_on(&doc, &pose, &bone, *center);
        let region = Region { name: stem.clone(), image: Some(rel_path(&base, file)), x, y, rotation, scale_x, scale_y, width: w as f32, height: h as f32 };
        let color = if *transparent { Color { a: 0.0, ..Color::WHITE } } else { Color::WHITE };
        let slot = Slot { name: name.clone(), bone: bone.clone(), color, blend: *blend, clip: clip.and_then(|c| slot_of.get(c).cloned().flatten()), attachment: Some(stem), attachments: vec![Attachment::Region(region)] };
        ops.push(Op::AddSlot { slot, index: None });
        slot_of.push(Some(name.clone()));
        names.push(name);
    }
    if names.is_empty() {
        app.status = crate::tf!("Not a PNG or PSD image: {}", skipped.join(", "));
        return;
    }
    if app.edit(ops) {
        app.camera.needs_fit |= frame;
        app.selection.bone = Some(bone.clone());
        app.selection.slot = names.last().cloned();
        app.selection.ik = None;app.selection.physics = None;app.selection.transform = None;
        let mut msg = crate::tf!("Added {} on bone {}", names.join(", "), bone);
        match origin_note {
            Some(OriginSource::Layer) => msg += crate::i18n::tr(" · origin from the [origin] layer"),
            Some(OriginSource::Guides) => msg += crate::i18n::tr(" · origin from the guides"),
            _ => {}
        }
        if !psd_dirs.is_empty() {
            msg += &crate::tf!(" · PSD parts saved in {}", psd_dirs.join(", "));
        }
        if !skipped.is_empty() {
            msg += &crate::tf!(" (skipped: {})", skipped.join(", "));
        }
        if app.path.is_none() {
            msg += crate::i18n::tr(" · save the document to store image paths relative to it");
        }
        app.status = msg;
    }
}

/// An image waiting to become a slot; `clip` is the index of its base image.
struct PlacedImage {
    file: PathBuf,
    /// In world units.
    center: [f32; 2],
    transparent: bool,
    blend: BlendMode,
    clip: Option<usize>,
}

fn is_psd(file: &Path) -> bool {
    file.extension().is_some_and(|e| e.eq_ignore_ascii_case("psd"))
}

/// Splits a layered PSD into one PNG per part (see [`skel2d_rig::psd::import`])
/// in a new `<name>_parts` folder beside it. Returns the folder and each
/// part's file, back to front, and where the origin came from, with each center relative to the PSD's origin
/// center in world units (1 per pixel, y up).
fn split_psd(file: &Path, split: Split, hidden: Hidden) -> Result<(PathBuf, Vec<PlacedImage>, OriginSource), String> {
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let origin = skel2d_rig::psd::origin(&bytes)?;
    let (_, parts) = skel2d_rig::psd::import(&bytes, &[], split, hidden)?;
    if parts.is_empty() {
        return Err(crate::i18n::tr("no visible layers").to_string());
    }
    let parent = file.parent().unwrap_or(Path::new("."));
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = parent.join(unique(&format!("{stem}_parts"), |n| parent.join(n).exists()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for (part, name) in parts.iter().zip(skel2d_rig::psd::file_names(&parts)) {
        let path = dir.join(format!("{name}.png"));
        part.image.save_png(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let cx = part.x as f32 + part.image.width() as f32 / 2.0 - origin.x;
        let cy = part.y as f32 + part.image.height() as f32 / 2.0 - origin.y;
        out.push(PlacedImage { file: path, center: [cx, -cy], transparent: part.hidden, blend: part.blend, clip: part.clip_to });
    }
    Ok((dir, out, origin.source))
}

pub fn pick_images() -> Vec<PathBuf> {
    rfd::FileDialog::new().add_filter(crate::i18n::tr("PNG or PSD image"), &["png", "psd"]).pick_files().unwrap_or_default()
}

/// Points an attachment at another image. A region is resized to the
/// image's pixel size around its current center; a mesh keeps its UVs.
pub fn replace_image(app: &EditorApp, slot: &str, att: &Attachment, file: &Path) -> Option<Op> {
    let rel = rel_path(&app.base_dir(), file);
    let attachment = match att {
        Attachment::Region(r) => {
            let [w, h] = image_size(file)?;
            Attachment::Region(Region { image: Some(rel), width: w as f32, height: h as f32, ..r.clone() })
        }
        Attachment::Mesh(m) => Attachment::Mesh(Mesh { image: Some(rel), ..m.clone() }),
        Attachment::LinkedMesh(m) => Attachment::LinkedMesh(skel2d_core::LinkedMesh { image: Some(rel), ..m.clone() }),
    };
    Some(Op::SetAttachment { slot: slot.to_string(), attachment })
}

/// Sets a region's size to its image's pixel size (1 unit per pixel).
pub fn fit_region_to_image(app: &EditorApp, slot: &str, r: &Region) -> Option<Op> {
    let [w, h] = image_size(&app.base_dir().join(r.image.as_deref()?))?;
    Some(Op::SetAttachment { slot: slot.to_string(), attachment: Attachment::Region(Region { width: w as f32, height: h as f32, ..r.clone() }) })
}

/// Moves a slot to another bone. Its regions keep their place on screen
/// (their transforms are re-expressed relative to the new bone); meshes are
/// in setup-pose space already and only change which bone unweighted
/// vertices follow.
pub fn rebind_slot(doc: &Document, slot: &Slot, to: &str) -> Vec<Op> {
    let pose = setup_pose(doc);
    let (from, to_w) = (bone_world(doc, &pose, &slot.bone), bone_world(doc, &pose, to));
    let mut ops = vec![Op::SetSlot(skel2d_core::ops::SlotPatch { name: slot.name.clone(), bone: Some(to.to_string()), ..Default::default() })];
    for att in &slot.attachments {
        if let Attachment::Region(r) = att {
            let world = from.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));
            let (x, y, rotation, scale_x, scale_y) = local_trs(&to_w, &world);
            let region = Region { x, y, rotation, scale_x, scale_y, ..r.clone() };
            ops.push(Op::SetAttachment { slot: slot.name.clone(), attachment: Attachment::Region(region) });
        }
    }
    ops
}

/// Trace and retrace share the asset op used by CLI, HTTP and MCP.
/// Painted weights and deform keys are preserved by the common expansion.
pub fn trace_mesh(app: &EditorApp, slot: &str, attachment: &str, params: skel2d_rig::TraceParams) -> skel2d_rig::AssetOp {
    skel2d_rig::AssetOp::TraceMesh { slot: slot.into(), attachment: attachment.into(), skin: app.skin_of(slot, attachment), name: None, params }
}

/// Geodesic weights only change the selected vertices, when supplied.
pub fn auto_weights(app: &EditorApp, slot: &str, attachment: &str, max_influences: usize, vertices: Option<Vec<usize>>) -> skel2d_rig::AssetOp {
    skel2d_rig::AssetOp::ImageWeights { locked_bones: app.view.weight_locked.iter().cloned().collect(), slot: slot.into(), attachment: attachment.into(), skin: app.skin_of(slot, attachment), bones: None, max_influences, falloff: 3.0, vertices }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflecting_a_carried_mesh_preserves_valid_hull_constraints_and_deform_indices() {
        let doc=skel2d_core::apply(&Document::new("reflection"),&skel2d_core::parse_ops(r#"[
            {"op":"add_bone","name":"root"},
            {"op":"add_slot","name":"s","bone":"root","attachment":"m","attachments":[{"type":"region","name":"m","width":10,"height":10}]},
            {"op":"mesh_from_region","slot":"s","region":"m","cols":1,"rows":1},
            {"op":"set_mesh_edges","slot":"s","attachment":"m","edges":[[0,2]]}
        ]"#).unwrap()).unwrap();
        let Attachment::Mesh(m)=&doc.slots[0].attachments[0] else {panic!()};
        let offsets=m.vertices.iter().map(|v|[v.x,v.y]).collect();
        let doc=skel2d_core::apply(&doc,&[Op::KeyDeform {animation:"a".into(),slot:"s".into(),attachment:"m".into(),skin:None,time:0.0,offsets,curve:None}]).unwrap();
        let op=Op::SetBone(skel2d_core::ops::BonePatch {name:"root".into(),scale_x:Some(-1.0),..Default::default()});
        let reflected=carry_meshes(&doc,skel2d_core::apply(&doc,&[op]).unwrap()).unwrap();
        assert!(skel2d_core::validate(&reflected).is_empty());
        let Attachment::Mesh(m)=&reflected.slots[0].attachments[0] else {panic!()};
        assert_eq!(m.hull,4);assert_eq!(m.edges.len(),1);
        let keys=&reflected.animations["a"].deform["default"]["s"]["m"][0].value;
        for (v,offset) in m.vertices.iter().zip(keys) {
            assert!((v.x+offset[0]).abs()<0.001);
            assert!((v.y-offset[1]).abs()<0.001);
        }
    }

    #[test]
    fn meshes_follow_setup_bone_edits() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/miya");
        let doc: Document = serde_json::from_str(&std::fs::read_to_string(dir.join("miya.skel.json")).unwrap()).unwrap();
        let ops = [Op::SetBone(skel2d_core::ops::BonePatch { name: "root".into(), rotation: Some(30.0), ..Default::default() })];
        let rotated = carry_meshes(&doc, skel2d_core::apply(&doc, &ops).unwrap()).unwrap();
        // Every mesh vertex turned 30° about the root, like the regions did.
        let turn = Affine::from_trs(0.0, 0.0, 30.0, 1.0, 1.0);
        let root = setup_pose(&doc).bones[doc.bone_index("root").unwrap()].world;
        let about = root.mul(&turn).mul(&root.inverse().unwrap());
        for (a, b) in doc.slots.iter().zip(&rotated.slots) {
            if let (Some(Attachment::Mesh(m0)), Some(Attachment::Mesh(m1))) = (a.attachments.first(), b.attachments.first()) {
                for (v0, v1) in m0.vertices.iter().zip(&m1.vertices) {
                    let [x, y] = about.apply(v0.x, v0.y);
                    assert!((x - v1.x).abs() < 0.05 && (y - v1.y).abs() < 0.05, "{}: {x},{y} vs {},{}", a.name, v1.x, v1.y);
                }
            }
        }
    }

    #[test]
    fn dragging_a_bone_is_never_refused_by_the_meshes_it_carries() {
        // The rig where this stuck: weighted vertices leave the hull partway through.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/celestial-circus");
        let doc: Document = serde_json::from_str(&std::fs::read_to_string(dir.join("celestial-circus.skel.json")).unwrap()).unwrap();
        let bone = doc.bone("leg-back-down").unwrap();
        for step in 1..=40 {
            let offset = step as f32 * 25.0;
            let ops = [Op::SetBone(skel2d_core::ops::BonePatch { name: bone.name.clone(), x: Some(bone.x + offset), y: Some(bone.y - offset), ..Default::default() })];
            let moved = skel2d_core::apply(&doc, &ops).unwrap();
            let carried = carry_meshes(&doc, moved).unwrap();
            assert!(skel2d_core::validate(&carried).is_empty(), "step {step}");
        }
    }

    #[test]
    fn uv_world_fit_recovers_placement() {
        let truth = Affine::from_trs(12.0, -7.0, 30.0, 1.0, 1.0).mul(&Affine::translate(-50.0, 40.0).mul(&Affine::scale(100.0, -80.0)));
        let vertices = [[0.0, 0.0], [1.0, 0.0], [0.3, 0.9], [0.7, 0.4]]
            .map(|[u, v]| {
                let [x, y] = truth.apply(u, v);
                skel2d_core::MeshVertex { x, y, u, v, weights: Vec::new() }
            })
            .to_vec();
        let mesh = Mesh { bones: Vec::new(), hull: 0, edges: Vec::new(), name: "m".into(), image: None, vertices, triangles: Vec::new() };
        let fit = skel2d_rig::asset_ops::fit_uv_world(&mesh).unwrap();
        for p in [[0.5, 0.5], [0.0, 1.0]] {
            let (a, b) = (fit.apply(p[0], p[1]), truth.apply(p[0], p[1]));
            assert!((a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_psd_with_guides_puts_their_crossing_on_the_root() {
        let source = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/miya/miya_a_idle02_field.psd")).unwrap();
        // The same file with one vertical guide at x=100 and one horizontal at y=200.
        let mut data = vec![0, 0, 0, 1];
        data.extend([0; 8]);
        data.extend(2u32.to_be_bytes());
        data.extend((100i32 * 32).to_be_bytes());
        data.push(0);
        data.extend((200i32 * 32).to_be_bytes());
        data.push(1);
        let mut resource = b"8BIM".to_vec();
        resource.extend(1032u16.to_be_bytes());
        resource.extend([0, 0]);
        resource.extend((data.len() as u32).to_be_bytes());
        resource.extend(data);
        let at = 26 + 4 + u32::from_be_bytes(source[26..30].try_into().unwrap()) as usize;
        let len = u32::from_be_bytes(source[at..at + 4].try_into().unwrap()) as usize;
        let mut guided = source.clone();
        guided.splice(at + 4 + len..at + 4 + len, resource.iter().copied());
        guided[at..at + 4].copy_from_slice(&((len + resource.len()) as u32).to_be_bytes());

        let tmp = std::env::temp_dir().join(format!("skel2d_psd_origin_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let import = |name: &str, bytes: &[u8], at: [f32; 2]| {
            let file = tmp.join(name);
            std::fs::write(&file, bytes).unwrap();
            let mut app = EditorApp::test_app(Document::with_root("t"), tmp.join("t.skel.json"));
            import_images_split(&mut app, &[file], at, None, Split::Layers, Hidden::Include);
            app.doc
        };
        let region = |doc: &Document| match &doc.slots[0].attachments[0] {
            Attachment::Region(r) => [r.x, r.y],
            _ => panic!(),
        };
        let plain = import("plain.psd", &source, [0.0, 0.0]);
        let marked = import("marked.psd", &guided, [500.0, 500.0]);
        let (w, h) = (461.0, 720.0);
        let ([px, py], [mx, my]) = (region(&plain), region(&marked));
        assert!((mx - (px + w / 2.0 - 100.0)).abs() < 0.01, "{mx} vs {px}: x from the guide, not the drop point");
        assert!((my - (py - h / 2.0 + 200.0)).abs() < 0.01, "{my} vs {py}: y up");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn psd_parts_keep_canvas_layout() {
        let psd = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/miya/miya_a_idle02_field.psd");
        let tmp = std::env::temp_dir().join(format!("skel2d_psd_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("miya.psd");
        std::fs::copy(&psd, &file).unwrap();
        let (dir, parts, _) = split_psd(&file, Split::Groups, Hidden::Include).unwrap();
        let (second, _, _) = split_psd(&file, Split::Groups, Hidden::Include).unwrap();
        assert_ne!(dir, second, "a second import gets its own folder");
        let ([w, h], raw) = skel2d_rig::psd::import(&std::fs::read(&psd).unwrap(), &[], Split::Groups, Hidden::Include).unwrap();
        assert_eq!(parts.len(), raw.len());
        for (PlacedImage { file: path, center: [cx, cy], .. }, p) in parts.iter().zip(&raw) {
            assert_eq!(image_size(path), Some([p.image.width(), p.image.height()]));
            // Back on the canvas: center + half size gives the part's top-left.
            let x = cx + w as f32 / 2.0 - p.image.width() as f32 / 2.0;
            let y = -cy + h as f32 / 2.0 - p.image.height() as f32 / 2.0;
            assert!((x - p.x as f32).abs() < 1e-3 && (y - p.y as f32).abs() < 1e-3, "{}", path.display());
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn relative_paths() {
        let base = Path::new("/proj/chars");
        assert_eq!(rel_path(base, Path::new("/proj/chars/parts/arm.png")), "parts/arm.png");
        assert_eq!(rel_path(base, Path::new("/proj/art/arm.png")), "../art/arm.png");
        assert!(rel_path(Path::new(""), Path::new("/x/a.png")).ends_with("/x/a.png"));
    }

    #[test]
    fn rebind_keeps_region_in_place() {
        let mut doc = Document::new("t");
        let mut a = Bone::new("a");
        a.rotation = 30.0;
        a.x = 5.0;
        doc.bones.push(a);
        let mut b = Bone::new("b");
        b.x = -20.0;
        b.y = 7.0;
        b.rotation = -45.0;
        // Uniform: a non-uniform scale under a rotation would need skew, which regions lack.
        b.scale_x = 2.0;
        b.scale_y = 2.0;
        doc.bones.push(b);
        let region = Region { name: "r".into(), image: None, x: 3.0, y: 4.0, rotation: 10.0, scale_x: 1.0, scale_y: 1.0, width: 10.0, height: 6.0 };
        let slot = Slot { name: "s".into(), bone: "a".into(), color: Color::WHITE, blend: Default::default(), clip: None, attachment: Some("r".into()), attachments: vec![Attachment::Region(region.clone())] };
        doc.slots.push(slot.clone());
        let before = setup_pose(&doc).bones[0].world.mul(&Affine::from_trs(3.0, 4.0, 10.0, 1.0, 1.0)).apply(5.0, 3.0);
        let next = skel2d_core::apply(&doc, &rebind_slot(&doc, &slot, "b")).unwrap();
        let Attachment::Region(r) = &next.slots[0].attachments[0] else { panic!() };
        let after = setup_pose(&next).bones[1].world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y)).apply(5.0, 3.0);
        assert!((before[0] - after[0]).abs() < 0.05 && (before[1] - after[1]).abs() < 0.05, "{before:?} vs {after:?}");
    }
}
