//! `image-info` and `rig`: the image side of auto-rigging.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use skel2d_rig::layers::{LayerInput, LayerManifest, VariantInput};
use skel2d_rig::{RigOptions, RigSpec};
use tiny_skia::Pixmap;

pub(crate) fn load_png(path: &Path) -> Result<Pixmap> {
    Pixmap::load_png(path).map_err(|e| anyhow!("{}: cannot read PNG: {e}", path.display()))
}

fn read_spec(spec: Option<&Path>, inline: Option<String>) -> Result<RigSpec> {
    let text = match (spec, inline) {
        (_, Some(s)) => s,
        (Some(p), None) => std::fs::read_to_string(p).with_context(|| p.display().to_string())?,
        (None, None) => bail!("give --spec FILE or --spec-json '<json>' (see `skel2d schema rig`)"),
    };
    serde_json::from_str(&text).map_err(|e| anyhow!("invalid rig spec: {e}"))
}

/// Pixel points named in a spec, for marking on the grid image.
pub(crate) fn spec_points(spec: &RigSpec) -> Vec<[f32; 2]> {
    match spec {
        RigSpec::Humanoid { joints, .. } => joints.values().copied().collect(),
        RigSpec::Chain { joints, .. } => joints.clone().unwrap_or_default(),
        RigSpec::Custom { bones } => bones.iter().flat_map(|b| std::iter::once(b.start).chain(b.end)).collect(),
    }
}

pub fn image_info(image: &Path, grid: Option<&Path>, step: u32, spec: Option<&Path>) -> Result<Value> {
    let img = load_png(image)?;
    let info = skel2d_rig::image_info(&img);
    if let Some(out) = grid {
        let points = match spec {
            Some(p) => spec_points(&read_spec(Some(p), None)?),
            None => Vec::new(),
        };
        skel2d_rig::grid::overlay(&img, step, &points)
            .save_png(out)
            .map_err(|e| anyhow!("{}: {e}", out.display()))?;
    }
    let mut v = serde_json::to_value(info)?;
    if let Some(out) = grid {
        v["grid"] = json!(out);
    }
    Ok(v)
}

/// Path of `target` relative to `base_dir`, with forward slashes. Falls back
/// to the absolute path when they share no prefix (e.g. another drive).
pub(crate) fn relative_to(target: &Path, base_dir: &Path) -> String {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let (t, b) = (abs(target), abs(base_dir));
    let (tc, bc): (Vec<_>, Vec<_>) = (t.components().collect(), b.components().collect());
    let common = tc.iter().zip(&bc).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return t.to_string_lossy().replace('\\', "/");
    }
    let mut rel = PathBuf::new();
    for _ in common..bc.len() {
        rel.push("..");
    }
    for c in &tc[common..] {
        rel.push(c);
    }
    rel.to_string_lossy().replace('\\', "/")
}

/// `image` is a PNG, or a `layers.json` manifest for a layered character.
#[allow(clippy::too_many_arguments)]
pub fn rig(
    image: &Path,
    spec: Option<&Path>,
    spec_json: Option<String>,
    out: &Path,
    cell: Option<usize>,
    grid_mesh: bool,
    no_mesh: bool,
    max_influences: usize,
    force: bool,
) -> Result<Value> {
    if out.exists() && !force {
        bail!("{} already exists (use --force to overwrite)", out.display());
    }
    let spec = read_spec(spec, spec_json)?;
    let out_dir = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = out
        .file_stem()
        .map(|s| s.to_string_lossy().split('.').next().unwrap_or_default().to_string())
        .unwrap_or_default();
    let mut opts = RigOptions { cell, grid_mesh, no_mesh, max_influences, name, ..RigOptions::default() };
    let is_manifest = image.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let (doc, report) = if is_manifest {
        let (layers, canvas) = load_layers(image, out_dir)?;
        skel2d_rig::layers::rig_layers(&layers, canvas, &spec, &opts)
    } else {
        opts.image_path = relative_to(image, out_dir);
        skel2d_rig::rig(&load_png(image)?, &spec, &opts)
    }
    .map_err(|e| anyhow!(e))?;
    let mut doc = doc;
    skel2d_core::ops::ensure_root(&mut doc);
    skel2d_core::save(out, &doc)?;
    let mut v = serde_json::to_value(report)?;
    v["out"] = json!(out);
    Ok(v)
}

/// Loads a layer manifest and its images; stored paths are relative to `out_dir`.
pub(crate) fn load_layers(manifest: &Path, out_dir: &Path) -> Result<(Vec<LayerInput>, Option<[u32; 2]>)> {
    let text = std::fs::read_to_string(manifest).with_context(|| manifest.display().to_string())?;
    let m: LayerManifest = serde_json::from_str(&text).map_err(|e| anyhow!("{}: invalid layer manifest: {e}", manifest.display()))?;
    let dir = manifest.parent().unwrap_or(Path::new("."));
    let layers = m
        .layers
        .into_iter()
        .map(|l| {
            if !(l.scale.is_finite() && l.scale > 0.0) {
                bail!("layer {:?}: scale must be > 0", l.name);
            }
            // A center places the scaled image's middle on that canvas pixel.
            let top_left = |center: Option<[f32; 2]>, img: &Pixmap, s: f32| {
                center.map(|[cx, cy]| [cx - img.width() as f32 * s / 2.0, cy - img.height() as f32 * s / 2.0])
            };
            let path = dir.join(&l.image);
            let image = load_png(&path)?;
            let variants = l
                .variants
                .into_iter()
                .map(|v| {
                    let vp = dir.join(&v.image);
                    let img = load_png(&vp)?;
                    let s = v.scale.unwrap_or(l.scale);
                    Ok(VariantInput {
                        name: v.name,
                        offset: top_left(v.center, &img, s).or(v.x.zip(v.y).map(|(x, y)| [x, y])),
                        scale: s,
                        image: img,
                        path: relative_to(&vp, out_dir),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(LayerInput {
                attachment: l.attachment.unwrap_or_else(|| l.name.clone()),
                name: l.name,
                offset: top_left(l.center, &image, l.scale).unwrap_or([l.x, l.y]),
                scale: l.scale,
                image,
                path: relative_to(&path, out_dir),
                bone: l.bone,
                bones: l.bones,
                mesh: l.mesh,
                hidden: l.hidden,
                variants,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((layers, m.canvas))
}

#[allow(clippy::too_many_arguments)]
pub fn slice(sheet: &Path, out_dir: &Path, prefix: &str, merge: usize, padding: u32, min_pixels: usize, key: KeyArgs, defringe: u8) -> Result<Value> {
    let mut img = load_png(sheet)?;
    key.apply(&mut img);
    if defringe > 0 {
        skel2d_rig::slice::clean(&mut img, defringe, min_pixels);
    }
    std::fs::create_dir_all(out_dir).with_context(|| out_dir.display().to_string())?;
    let pieces = skel2d_rig::slice::slice(&img, merge, padding, min_pixels);
    let mut out = Vec::new();
    for p in &pieces {
        let Some(image) = &p.image else { continue };
        let file = out_dir.join(format!("{prefix}_{:02}.png", p.index));
        if file.exists() {
            bail!("{} already exists; pick another --prefix or folder", file.display());
        }
        image.save_png(&file).map_err(|e| anyhow!("{}: {e}", file.display()))?;
        out.push(json!({ "file": file, "bbox": p.bbox, "size": [image.width(), image.height()] }));
    }
    Ok(json!({ "pieces": out }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedRect {
    name: String,
    rect: [u32; 4],
}

pub fn crop(sheet: &Path, rects: &Path, out_dir: &Path, key: KeyArgs, defringe: u8, min_pixels: usize) -> Result<Value> {
    let mut img = load_png(sheet)?;
    key.apply(&mut img);
    if defringe > 0 {
        skel2d_rig::slice::clean(&mut img, defringe, min_pixels);
    }
    let text = std::fs::read_to_string(rects).with_context(|| rects.display().to_string())?;
    let list: Vec<NamedRect> = serde_json::from_str(&text).map_err(|e| anyhow!("{}: {e}", rects.display()))?;
    std::fs::create_dir_all(out_dir).with_context(|| out_dir.display().to_string())?;
    let mut out = Vec::new();
    for r in &list {
        let file = out_dir.join(format!("{}.png", r.name));
        if file.exists() {
            bail!("{} already exists", file.display());
        }
        let piece = skel2d_rig::slice::crop(&img, r.rect).ok_or_else(|| anyhow!("{}: empty rect {:?}", r.name, r.rect))?;
        piece.save_png(&file).map_err(|e| anyhow!("{}: {e}", file.display()))?;
        out.push(json!({ "file": file, "size": [piece.width(), piece.height()] }));
    }
    Ok(json!({ "pieces": out }))
}

/// Optional chroma key applied before slicing or cropping.
#[derive(Clone, Copy, Default)]
pub struct KeyArgs(Option<([u8; 3], u8)>);

impl KeyArgs {
    pub fn new(color: Option<&str>, tolerance: u8) -> Result<Self> {
        Ok(KeyArgs(color.map(|c| parse_rgb(c).map(|rgb| (rgb, tolerance))).transpose()?))
    }

    fn apply(self, img: &mut Pixmap) {
        if let Some((rgb, tol)) = self.0 {
            skel2d_rig::slice::chroma_key(img, rgb, tol);
        }
    }
}

fn parse_rgb(s: &str) -> Result<[u8; 3]> {
    let c = skel2d_core::Color::parse_hex(s).ok_or_else(|| anyhow!("bad color {s:?}, expected RRGGBB"))?;
    let [r, g, b, _] = c.to_rgba8();
    Ok([r, g, b])
}

pub fn key(image: &Path, out: &Path, color: &str, tolerance: u8) -> Result<Value> {
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    let mut img = load_png(image)?;
    skel2d_rig::slice::chroma_key(&mut img, parse_rgb(color)?, tolerance);
    img.save_png(out).map_err(|e| anyhow!("{}: {e}", out.display()))?;
    let info = skel2d_rig::image_info(&img);
    Ok(json!({ "out": out, "transparent_ratio": info.transparent_ratio, "bbox": info.bbox }))
}

/// Splits a PSD into one PNG per part plus a `layers.json` manifest (back to
/// front, canvas positions) and `composite.png` (all parts, for placing joints).
pub fn psd(file: &Path, out_dir: &Path, exclude: &[String], split: skel2d_rig::psd::Split) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| file.display().to_string())?;
    let ([w, h], parts) = skel2d_rig::psd::import(&bytes, exclude, split).map_err(|e| anyhow!("{}: {e}", file.display()))?;
    if parts.is_empty() {
        bail!("{}: no visible parts", file.display());
    }
    let manifest_path = out_dir.join("layers.json");
    let composite_path = out_dir.join("composite.png");
    let names = skel2d_rig::psd::file_names(&parts);
    for path in names.iter().map(|n| out_dir.join(format!("{n}.png"))).chain([manifest_path.clone(), composite_path.clone()]) {
        if path.exists() {
            bail!("{} already exists; pick another folder", path.display());
        }
    }
    std::fs::create_dir_all(out_dir).with_context(|| out_dir.display().to_string())?;

    let mut composite = Pixmap::new(w, h).ok_or_else(|| anyhow!("empty canvas"))?;
    let mut layers = Vec::new();
    let mut out = Vec::new();
    for (part, name) in parts.iter().zip(&names) {
        let file = out_dir.join(format!("{name}.png"));
        part.image.save_png(&file).map_err(|e| anyhow!("{}: {e}", file.display()))?;
        composite.draw_pixmap(part.x as i32, part.y as i32, part.image.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        layers.push(json!({ "name": name, "image": format!("{name}.png"), "x": part.x, "y": part.y }));
        out.push(json!({ "name": name, "psd_name": part.name, "file": file, "x": part.x, "y": part.y,
                         "size": [part.image.width(), part.image.height()], "psd_layers": part.layers }));
    }
    composite.save_png(&composite_path).map_err(|e| anyhow!("{}: {e}", composite_path.display()))?;
    let manifest = json!({ "canvas": [w, h], "layers": layers });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)? + "\n").with_context(|| manifest_path.display().to_string())?;
    Ok(json!({ "canvas": [w, h], "manifest": manifest_path, "composite": composite_path, "parts": out }))
}
