//! Headless CPU renderer. Produces PNG previews of a pose so that tools and
//! AI agents can check results without a GPU or a window.

mod gif;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use gif::{GifJob, GifOptions, GifStats, MAX_GIF_FPS, MAX_SHEET_FPS, SheetOptions, gif_frame_count, gif_plan, render_gif, render_sheet, safe_file_name, sheet_frame_count};

use skel2d_core::{Affine, Attachment, BlendMode, Document, Pose};
use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    /// Pixels per world unit; `None` fits the content into the frame.
    pub zoom: Option<f32>,
    /// World point at the frame center; `None` centers the content.
    pub center: Option<[f32; 2]>,
    pub background: [u8; 4],
    pub draw_bones: bool,
    pub draw_axes: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            zoom: None,
            center: None,
            background: [40, 42, 48, 255],
            draw_bones: true,
            draw_axes: true,
        }
    }
}

/// Axis-aligned world-space bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Bounds {
    fn empty() -> Self {
        Bounds { min: [f32::INFINITY; 2], max: [f32::NEG_INFINITY; 2] }
    }

    fn add(&mut self, [x, y]: [f32; 2]) {
        self.min = [self.min[0].min(x), self.min[1].min(y)];
        self.max = [self.max[0].max(x), self.max[1].max(y)];
    }

    pub fn union(self, o: Bounds) -> Bounds {
        Bounds {
            min: [self.min[0].min(o.min[0]), self.min[1].min(o.min[1])],
            max: [self.max[0].max(o.max[0]), self.max[1].max(o.max[1])],
        }
    }

    fn is_valid(&self) -> bool {
        self.min[0] <= self.max[0] && self.min[1] <= self.max[1]
    }
}

/// Loads images relative to the document's directory and caches them,
/// including failures so a missing file is only reported once.
pub struct Renderer {
    base_dir: PathBuf,
    images: HashMap<String, Option<Pixmap>>,
    pub missing_images: Vec<String>,
}

impl Renderer {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into(), images: HashMap::new(), missing_images: Vec::new() }
    }

    fn image(&mut self, rel: &str) -> Option<&Pixmap> {
        if !self.images.contains_key(rel) {
            let img = Pixmap::load_png(self.base_dir.join(rel)).ok();
            if img.is_none() {
                self.missing_images.push(rel.to_string());
            }
            self.images.insert(rel.to_string(), img);
        }
        self.images.get(rel).and_then(Option::as_ref)
    }

    /// World bounds of all visible attachments and bones.
    pub fn bounds(&self, doc: &Document, pose: &Pose) -> Bounds {
        self.visible_bounds(doc, pose, true)
    }

    /// World bounds of what a frame shows, with or without its bones.
    pub fn visible_bounds(&self, doc: &Document, pose: &Pose, bones: bool) -> Bounds {
        let mut b = Bounds::empty();
        if bones {
            for (bone, bp) in doc.bones.iter().zip(&pose.bones) {
                b.add(bp.world.apply(0.0, 0.0));
                b.add(bp.world.apply(bone.length, 0.0));
            }
        }
        for (si, slot) in doc.slots.iter().enumerate() {
            let Some(att) = pose.attachment(doc, si) else { continue };
            let Some(materialized) = doc.materialize_attachment(si, att) else { continue };
            let att = &materialized;
            let Some(bi) = doc.bone_index(&slot.bone) else { continue };
            match att {
                Attachment::Region(r) => {
                    let m = pose.bones[bi].world.mul(&Affine::from_trs(r.x, r.y, r.rotation, r.scale_x, r.scale_y));
                    let (hw, hh) = (r.width / 2.0, r.height / 2.0);
                    for (x, y) in [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
                        b.add(m.apply(x, y));
                    }
                }
                Attachment::Mesh(mesh) => pose.deform_mesh(doc, mesh, si).into_iter().for_each(|p| b.add(p)),
                Attachment::LinkedMesh(_) => unreachable!(),
            }
        }
        if !b.is_valid() {
            b = Bounds { min: [-50.0, -50.0], max: [50.0, 50.0] };
        }
        b
    }

    /// Draws one slot onto `pm` with `blend`, ignoring its clip.
    fn draw_slot(&mut self, pm: &mut Pixmap, doc: &Document, pose: &Pose, si: usize, view: &Affine, blend: BlendMode) {
        let (slot, sp) = (&doc.slots[si], &pose.slots[si]);
        let Some(att) = pose.attachment(doc, si) else { return };
        let Some(materialized) = doc.materialize_attachment(si, att) else { return };
        let att = &materialized;
        let Some(bi) = doc.bone_index(&slot.bone) else { return };
        let region = match att {
            Attachment::Region(r) => r,
            Attachment::Mesh(mesh) => {
                let points: Vec<[f32; 2]> =
                    pose.deform_mesh(doc, mesh, si).into_iter().map(|[x, y]| view.apply(x, y)).collect();
                let image = mesh.image.as_deref().and_then(|p| self.image(p)).map(|img| {
                    if sp.color.is_white() { img.clone() } else { tinted(img, sp.color.r, sp.color.g, sp.color.b, sp.color.a) }
                });
                draw_mesh(pm, mesh, &points, image.as_ref(), sp.color.to_rgba8(), blend_mode(blend));
                return;
            }
            Attachment::LinkedMesh(_) => unreachable!(),
        };
        let local = Affine::from_trs(region.x, region.y, region.rotation, region.scale_x, region.scale_y);
        let world = view.mul(&pose.bones[bi].world).mul(&local);
        let tint = sp.color.to_rgba8();

        match region.image.as_deref().and_then(|p| self.image(p)) {
            Some(img) => {
                let (iw, ih) = (img.width() as f32, img.height() as f32);
                // Image pixels are y-down with origin at the top-left; map them
                // onto a width x height rectangle centered on the attachment.
                let to_local = Affine {
                    a: region.width / iw,
                    b: 0.0,
                    c: 0.0,
                    d: -region.height / ih,
                    tx: -region.width / 2.0,
                    ty: region.height / 2.0,
                };
                let m = world.mul(&to_local);
                let paint = PixmapPaint { quality: FilterQuality::Bilinear, blend_mode: blend_mode(blend), ..PixmapPaint::default() };
                if tint == [255; 4] {
                    pm.draw_pixmap(0, 0, img.as_ref(), &paint, to_ts(&m), None);
                } else {
                    let tinted = tinted(img, sp.color.r, sp.color.g, sp.color.b, sp.color.a);
                    pm.draw_pixmap(0, 0, tinted.as_ref(), &paint, to_ts(&m), None);
                }
            }
            None => draw_placeholder(pm, &world, region.width, region.height, tint),
        }
    }

    /// Renders one pose. `fit` overrides the bounds used for auto framing,
    /// so a sequence of frames can share one camera.
    pub fn render(&mut self, doc: &Document, pose: &Pose, opts: &RenderOptions, fit: Option<Bounds>) -> Pixmap {
        let (w, h) = (opts.width.max(1), opts.height.max(1));
        let mut pm = Pixmap::new(w, h).expect("non-zero size");
        let [r, g, b, a] = opts.background;
        pm.fill(Color::from_rgba8(r, g, b, a));

        let view = view_transform(opts, fit.unwrap_or_else(|| self.bounds(doc, pose)));
        if opts.draw_axes {
            draw_axes(&mut pm, &view);
        }

        // A clipped slot is drawn on its own, then cut by its base's rendered alpha.
        let mut masks: std::collections::HashMap<usize, Pixmap> = std::collections::HashMap::new();
        for &si in &pose.draw_order {
            let slot = &doc.slots[si];
            let Some(base) = slot.clip.as_deref().and_then(|c| doc.slot_index(c)) else {
                self.draw_slot(&mut pm, doc, pose, si, &view, slot.blend);
                continue;
            };
            let mask = masks.entry(base).or_insert_with(|| {
                let mut mask = Pixmap::new(w, h).expect("non-zero size");
                self.draw_slot(&mut mask, doc, pose, base, &view, BlendMode::Normal);
                mask
            });
            let mut layer = Pixmap::new(w, h).expect("non-zero size");
            self.draw_slot(&mut layer, doc, pose, si, &view, BlendMode::Normal);
            let cut = PixmapPaint { blend_mode: tiny_skia::BlendMode::DestinationIn, ..PixmapPaint::default() };
            layer.draw_pixmap(0, 0, mask.as_ref(), &cut, Transform::identity(), None);
            let over = PixmapPaint { blend_mode: blend_mode(slot.blend), ..PixmapPaint::default() };
            pm.draw_pixmap(0, 0, layer.as_ref(), &over, Transform::identity(), None);
        }

        if opts.draw_bones {
            for (bone, bp) in doc.bones.iter().zip(&pose.bones) {
                let [r, g, b, a] = bone.color.map_or([255, 196, 64, 255], |c| c.to_rgba8());
                draw_bone(&mut pm, &view.mul(&bp.world), bone.length, [r, g, b, (u16::from(a) * 220 / 255) as u8]);
            }
        }
        pm
    }

    /// Renders `frames` evenly spaced frames of an animation (first and last
    /// key included) side by side, with one shared camera.
    pub fn render_strip(
        &mut self,
        doc: &Document,
        anim: &skel2d_core::Animation,
        frames: u32,
        opts: &RenderOptions,
    ) -> Pixmap {
        let frames = frames.max(1);
        let duration = anim.duration();
        let poses: Vec<Pose> = (0..frames)
            .map(|i| {
                let t = if frames == 1 { 0.0 } else { duration * i as f32 / (frames - 1) as f32 };
                Pose::compute(doc, Some((anim, t)))
            })
            .collect();
        self.render_poses(doc, &poses, opts)
    }

    /// Renders poses side by side with one shared camera.
    pub fn render_poses(&mut self, doc: &Document, poses: &[Pose], opts: &RenderOptions) -> Pixmap {
        let frames = poses.len().max(1) as u32;
        let fit = poses.iter().map(|p| self.bounds(doc, p)).reduce(Bounds::union);
        let mut strip = Pixmap::new(opts.width.max(1) * frames, opts.height.max(1)).expect("non-zero size");
        for (i, pose) in poses.iter().enumerate() {
            let frame = self.render(doc, pose, opts, fit);
            strip.draw_pixmap(
                (i as u32 * opts.width.max(1)) as i32,
                0,
                frame.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            if i > 0 {
                let x = (i as u32 * opts.width.max(1)) as f32;
                fill_rect(&mut strip, x, 0.0, 1.0, opts.height as f32, [0, 0, 0, 255]);
            }
        }
        strip
    }
}

/// World (y-up) to pixels (y-down).
fn view_transform(opts: &RenderOptions, fit: Bounds) -> Affine {
    let (w, h) = (opts.width.max(1) as f32, opts.height.max(1) as f32);
    let size = [(fit.max[0] - fit.min[0]).max(1.0), (fit.max[1] - fit.min[1]).max(1.0)];
    let zoom = opts.zoom.unwrap_or_else(|| (w / size[0]).min(h / size[1]) * 0.85);
    let [cx, cy] = opts
        .center
        .unwrap_or([(fit.min[0] + fit.max[0]) / 2.0, (fit.min[1] + fit.max[1]) / 2.0]);
    Affine { a: zoom, b: 0.0, c: 0.0, d: -zoom, tx: w / 2.0 - cx * zoom, ty: h / 2.0 + cy * zoom }
}

fn blend_mode(mode: BlendMode) -> tiny_skia::BlendMode {
    match mode {
        BlendMode::Normal => tiny_skia::BlendMode::SourceOver,
        BlendMode::Additive => tiny_skia::BlendMode::Plus,
        BlendMode::Multiply => tiny_skia::BlendMode::Multiply,
        BlendMode::Screen => tiny_skia::BlendMode::Screen,
    }
}

fn to_ts(m: &Affine) -> Transform {
    Transform::from_row(m.a, m.b, m.c, m.d, m.tx, m.ty)
}

/// Multiplies a premultiplied image by a color.
fn tinted(img: &Pixmap, r: f32, g: f32, b: f32, a: f32) -> Pixmap {
    let mut out = img.clone();
    let f = [r * a, g * a, b * a, a].map(|v| v.clamp(0.0, 1.0));
    for px in out.data_mut().as_chunks_mut::<4>().0 {
        for (c, k) in px.iter_mut().zip(f) {
            *c = (*c as f32 * k).round() as u8;
        }
    }
    out
}

fn paint([r, g, b, a]: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

/// Pixel-aligned, non-antialiased rectangle. tiny-skia's antialiased hairline
/// path can assert on sub-pixel thin rects, so thin lines go through here.
fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
    let (x0, y0) = (x.round().max(0.0), y.round().max(0.0));
    let x1 = (x + w).round().max(x0 + 1.0).min(pm.width() as f32);
    let y1 = (y + h).round().max(y0 + 1.0).min(pm.height() as f32);
    if !(x0.is_finite() && y0.is_finite()) || x0 >= x1 || y0 >= y1 {
        return;
    }
    if let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) {
        let mut p = paint(color);
        p.anti_alias = false;
        pm.fill_rect(rect, &p, Transform::identity(), None);
    }
}

fn polygon(points: &[[f32; 2]]) -> Option<tiny_skia::Path> {
    // Degenerate transforms (zero scale, huge zoom) must not reach the rasterizer.
    if !points.iter().all(|p| p[0].is_finite() && p[1].is_finite() && p[0].abs() < 1e6 && p[1].abs() < 1e6) {
        return None;
    }
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to(first[0], first[1]);
    for p in rest {
        pb.line_to(p[0], p[1]);
    }
    pb.close();
    pb.finish()
}

fn draw_axes(pm: &mut Pixmap, view: &Affine) {
    let [ox, oy] = view.apply(0.0, 0.0);
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    fill_rect(pm, 0.0, oy, w, 1.0, [80, 84, 96, 255]);
    fill_rect(pm, ox, 0.0, 1.0, h, [80, 84, 96, 255]);
}

fn draw_placeholder(pm: &mut Pixmap, m: &Affine, w: f32, h: f32, tint: [u8; 4]) {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let pts: Vec<[f32; 2]> = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().map(|&(x, y)| m.apply(x, y)).collect();
    let Some(path) = polygon(&pts) else { return };
    let [r, g, b, a] = tint;
    let fill_alpha = (a as u16 * 140 / 255) as u8;
    pm.fill_path(&path, &paint([r, g, b, fill_alpha]), FillRule::Winding, Transform::identity(), None);
    pm.stroke_path(&path, &paint([r, g, b, a]), &Stroke { width: 1.5, ..Stroke::default() }, Transform::identity(), None);
    // Diagonal marks the attachment's local +x direction (towards the right edge).
    let Some(diag) = polygon(&[m.apply(-hw, -hh), m.apply(hw, 0.0), m.apply(-hw, hh)]) else { return };
    pm.stroke_path(&diag, &paint([r, g, b, a]), &Stroke { width: 1.0, ..Stroke::default() }, Transform::identity(), None);
}

/// Draws a mesh's triangles, each filled with the image through the affine
/// map from its UV triangle to its screen triangle.
fn draw_mesh(pm: &mut Pixmap, mesh: &skel2d_core::Mesh, points: &[[f32; 2]], image: Option<&Pixmap>, tint: [u8; 4], blend_mode: tiny_skia::BlendMode) {
    let [r, g, b, a] = tint;
    let fill = paint([r, g, b, (a as u16 * 140 / 255) as u8]);
    let edge = paint([r, g, b, a]);
    for tri in &mesh.triangles {
        let idx = tri.map(|i| i as usize);
        if idx.iter().any(|&i| i >= points.len()) {
            continue;
        }
        let s = idx.map(|i| points[i]);
        let Some(img) = image else {
            if let Some(path) = polygon(&s) {
                pm.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
                pm.stroke_path(&path, &edge, &Stroke { width: 0.75, ..Stroke::default() }, Transform::identity(), None);
            }
            continue;
        };
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let p = idx.map(|i| [mesh.vertices[i].u * iw, mesh.vertices[i].v * ih]);
        let Some(m) = triangle_map(p, s) else { continue };
        // Without antialiasing adjacent triangles partition pixels exactly, so
        // shared edges never show seams. The silhouette is aliased, which is
        // fine for previews.
        let Some(path) = polygon(&s) else { continue };
        let shader = tiny_skia::Pattern::new(img.as_ref(), tiny_skia::SpreadMode::Pad, FilterQuality::Bilinear, 1.0, to_ts(&m));
        let paint = Paint { shader, anti_alias: false, blend_mode, ..Paint::default() };
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Affine map taking triangle `from` onto triangle `to`.
fn triangle_map(from: [[f32; 2]; 3], to: [[f32; 2]; 3]) -> Option<Affine> {
    let src = Affine {
        a: from[1][0] - from[0][0],
        b: from[1][1] - from[0][1],
        c: from[2][0] - from[0][0],
        d: from[2][1] - from[0][1],
        tx: from[0][0],
        ty: from[0][1],
    };
    let dst = Affine {
        a: to[1][0] - to[0][0],
        b: to[1][1] - to[0][1],
        c: to[2][0] - to[0][0],
        d: to[2][1] - to[0][1],
        tx: to[0][0],
        ty: to[0][1],
    };
    Some(dst.mul(&src.inverse()?))
}

/// Classic bone glyph: a thin diamond from origin to tip plus a joint dot.
fn draw_bone(pm: &mut Pixmap, m: &Affine, length: f32, color: [u8; 4]) {
    let o = m.apply(0.0, 0.0);
    if !(o[0].is_finite() && o[1].is_finite() && o[0].abs() < 1e6 && o[1].abs() < 1e6) {
        return;
    }
    let fill = paint(color);
    let outline = paint([20, 20, 20, (u16::from(color[3]) * 230 / 220).min(230) as u8]);
    if length != 0.0 {
        let t = m.apply(length, 0.0);
        let screen_len = (t[0] - o[0]).hypot(t[1] - o[1]);
        if screen_len > 1.0 {
            let (dx, dy) = ((t[0] - o[0]) / screen_len, (t[1] - o[1]) / screen_len);
            let half_w = (screen_len * 0.08).clamp(2.0, 7.0);
            let mid = [o[0] + dx * screen_len * 0.15, o[1] + dy * screen_len * 0.15];
            let side = [-dy * half_w, dx * half_w];
            if let Some(path) =
                polygon(&[o, [mid[0] + side[0], mid[1] + side[1]], t, [mid[0] - side[0], mid[1] - side[1]]])
            {
                pm.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
                pm.stroke_path(&path, &outline, &Stroke { width: 1.0, ..Stroke::default() }, Transform::identity(), None);
            }
        }
    }
    if let Some(dot) = PathBuilder::from_circle(o[0], o[1], 3.0) {
        pm.fill_path(&dot, &fill, FillRule::Winding, Transform::identity(), None);
        pm.stroke_path(&dot, &outline, &Stroke { width: 1.0, ..Stroke::default() }, Transform::identity(), None);
    }
}

/// Renders the setup pose or one animation frame and returns PNG bytes.
pub fn render_png(doc: &Document, base_dir: &Path, anim: Option<(&str, f32)>, opts: &RenderOptions) -> Result<Vec<u8>, String> {
    let animation = match anim {
        Some((name, t)) => Some((doc.animations.get(name).ok_or_else(|| format!("animation {name:?} does not exist"))?, t)),
        None => None,
    };
    let pose = Pose::compute(doc, animation);
    let mut r = Renderer::new(base_dir);
    r.render(doc, &pose, opts, None).encode_png().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skel2d_core::{Document, apply, parse_ops};

    fn doc() -> Document {
        let ops = parse_ops(
            r##"[{"op":"add_bone","name":"root","length":40},
                {"op":"add_slot","name":"s","bone":"root","color":"#ff0000","attachment":"r",
                 "attachments":[{"type":"region","name":"r","x":20,"width":40,"height":10}]},
                {"op":"key_bone","animation":"spin","bone":"root","time":1,"rotate":360}]"##,
        )
        .unwrap();
        apply(&Document::new("t"), &ops).unwrap()
    }

    #[test]
    fn the_clipping_example_stays_inside_its_bottle_while_it_moves() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/clipping");
        let doc = skel2d_core::load(&dir.join("clipping.skel.json")).unwrap();
        assert!(skel2d_core::validate(&doc).is_empty());
        let free = apply(&doc, &parse_ops(r#"[{"op":"set_slot","name":"liquid","clip":null},{"op":"set_slot","name":"shine","clip":null}]"#).unwrap()).unwrap();
        let opts = RenderOptions { width: 800, height: 800, background: [0, 0, 0, 0], zoom: Some(1.0), center: Some([0.0, 0.0]), draw_bones: false, draw_axes: false, ..RenderOptions::default() };
        // Left of the bottle's body, where the liquid band reaches when it tilts.
        let (x, y) = (400 - 260, 400 + 150);
        for time in [0.0, 0.5, 1.0, 1.5] {
            let pose = |d: &Document| Pose::compute(d, Some((&d.animations["slosh"], time)));
            let mut r = Renderer::new(&dir);
            assert!(r.render(&free, &pose(&free), &opts, None).pixel(x, y).unwrap().alpha() > 0, "at {time} the band reaches past the bottle");
            let shown = r.render(&doc, &pose(&doc), &opts, None);
            assert_eq!(shown.pixel(x, y).unwrap().alpha(), 0, "at {time} the clip stops it");
            assert!(shown.pixel(400, 400 + 200).unwrap().alpha() > 0, "the bottle shows");
            assert!(r.missing_images.is_empty(), "{:?}", r.missing_images);
        }
    }

    #[test]
    fn a_clipped_slot_shows_only_inside_its_base() {
        let ops = parse_ops(
            r##"[{"op":"add_bone","name":"root"},
                {"op":"add_slot","name":"base","bone":"root","attachment":"r","attachments":[{"type":"region","name":"r","width":20,"height":20}]},
                {"op":"add_slot","name":"shade","bone":"root","clip":"base","color":"#0000ff","attachment":"r","attachments":[{"type":"region","name":"r","width":60,"height":60}]}]"##,
        )
        .unwrap();
        let clipped = apply(&Document::new("t"), &ops).unwrap();
        let free = apply(&clipped, &parse_ops(r#"{"op":"set_slot","name":"shade","clip":null}"#).unwrap()).unwrap();
        // World units map 1:1, centered, so x=50 is inside the base and x=25 outside it.
        let opts = RenderOptions { width: 100, height: 100, background: [0, 0, 0, 0], zoom: Some(1.0), center: Some([0.0, 0.0]), draw_bones: false, draw_axes: false, ..RenderOptions::default() };
        let render = |d: &Document| {
            let mut r = Renderer::new(".");
            r.render(d, &Pose::compute(d, None), &opts, None)
        };
        let (a, b) = (render(&clipped), render(&free));
        assert!(b.pixel(25, 50).unwrap().alpha() > 0, "unclipped it reaches past the base");
        assert_eq!(a.pixel(25, 50).unwrap().alpha(), 0, "clipped it stops at the base");
        assert!(a.pixel(50, 50).unwrap().blue() > a.pixel(50, 50).unwrap().red(), "and shows inside it");
    }

    #[test]
    fn renders_frames_and_degenerate_poses() {
        let doc = doc();
        let mut r = Renderer::new(".");
        let opts = RenderOptions { width: 64, height: 48, ..RenderOptions::default() };
        let strip = r.render_strip(&doc, &doc.animations["spin"], 5, &opts);
        assert_eq!((strip.width(), strip.height()), (320, 48));
        // Zero scale and extreme zoom must not panic.
        let ops = parse_ops(r#"{"op":"set_bone","name":"root","scale_x":0,"scale_y":0}"#).unwrap();
        let flat = apply(&doc, &ops).unwrap();
        r.render(&flat, &Pose::compute(&flat, None), &opts, None);
        let zoomed = RenderOptions { zoom: Some(1e9), ..opts };
        r.render(&doc, &Pose::compute(&doc, None), &zoomed, None);
    }
}

#[cfg(test)]
mod mesh_tests {
    use super::*;
    use skel2d_core::{Document, apply, parse_ops};

    #[test]
    fn multiply_darkens_what_is_behind_it() {
        let dir = std::env::temp_dir().join(format!("skel2d-blend-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut gray = Pixmap::new(8, 8).unwrap();
        gray.fill(Color::from_rgba8(128, 128, 128, 255));
        gray.save_png(dir.join("gray.png")).unwrap();
        let mut white = Pixmap::new(8, 8).unwrap();
        white.fill(Color::from_rgba8(255, 200, 100, 255));
        white.save_png(dir.join("back.png")).unwrap();
        for (mode, expected) in [("normal", [128, 128, 128]), ("multiply", [128, 100, 50]), ("additive", [255, 255, 228]), ("screen", [255, 228, 178])] {
            let ops = parse_ops(&format!(
                r#"[{{"op":"add_slot","name":"back","bone":"root","attachment":"b","attachments":[{{"type":"region","name":"b","image":"back.png","width":100,"height":100}}]}},
                    {{"op":"add_slot","name":"top","bone":"root","blend":"{mode}","attachment":"t",
                      "attachments":[{{"type":"region","name":"t","image":"gray.png","width":100,"height":100}}]}},
                    {{"op":"mesh_from_region","slot":"top","region":"t","cols":1,"rows":1}}]"#
            )).unwrap();
            let doc = apply(&Document::with_root("t"), &ops).unwrap();
            let opts = RenderOptions { width: 20, height: 20, zoom: Some(1.0), center: Some([0.0, 0.0]), draw_bones: false, draw_axes: false, ..RenderOptions::default() };
            let pm = Renderer::new(&dir).render(&doc, &Pose::compute(&doc, None), &opts, None);
            let p = pm.pixel(10, 10).unwrap();
            for (got, want) in [p.red(), p.green(), p.blue()].into_iter().zip(expected) {
                assert!((i32::from(got) - want).abs() <= 2, "{mode}: {p:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn textured_mesh_covers_its_area_without_seams() {
        let dir = std::env::temp_dir().join(format!("skel2d-mesh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = Pixmap::new(8, 8).unwrap();
        img.fill(Color::from_rgba8(255, 0, 0, 255));
        img.save_png(dir.join("red.png")).unwrap();

        let ops = parse_ops(
            r#"[{"op":"add_bone","name":"root","length":10},
                {"op":"add_slot","name":"s","bone":"root","attachment":"r",
                 "attachments":[{"type":"region","name":"r","image":"red.png","width":100,"height":100}]},
                {"op":"mesh_from_region","slot":"s","region":"r","cols":3,"rows":3}]"#,
        )
        .unwrap();
        let doc = apply(&Document::new("t"), &ops).unwrap();
        let opts = RenderOptions { width: 100, height: 100, zoom: Some(1.0), center: Some([0.0, 0.0]), draw_bones: false, draw_axes: false, ..RenderOptions::default() };
        let mut r = Renderer::new(&dir);
        let pm = r.render(&doc, &Pose::compute(&doc, None), &opts, None);
        assert!(r.missing_images.is_empty());
        // Every pixel well inside the quad is opaque red, including those on
        // the internal triangle edges.
        for y in 5..95 {
            for x in 5..95 {
                let p = pm.pixel(x, y).unwrap();
                assert!(p.red() > 250 && p.green() < 5, "pixel {x},{y} = {p:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bone_color_tests {
    use super::*;
    use skel2d_core::{Document, apply, parse_ops};

    #[test]
    fn bone_color_alpha_fades_the_bone() {
        let shown = |color: &str| {
            let ops = parse_ops(&format!(r#"{{"op":"set_bone","name":"root","length":100,"color":"{color}"}}"#)).unwrap();
            let doc = apply(&Document::with_root("t"), &ops).unwrap();
            let opts = RenderOptions { width: 128, height: 64, draw_axes: false, background: [0, 0, 0, 255], ..RenderOptions::default() };
            let pm = Renderer::new(".").render(&doc, &Pose::compute(&doc, None), &opts, None);
            pm.pixels().iter().map(|p| u32::from(p.red())).max().unwrap()
        };
        assert!(shown("#ff0000") > 180);
        assert!(shown("#ff000040") < 90);
        assert_eq!(shown("#ff000000"), 0);
    }
}

#[cfg(test)]
mod draw_order_tests {
    use super::*;
    use skel2d_core::{Document, apply, parse_ops};

    #[test]
    fn keyed_draw_order_changes_which_slot_is_on_top() {
        let box_at = |name: &str, color: &str| format!(r#"{{"op":"add_slot","name":"{name}","bone":"root","color":"{color}","attachment":"i","attachments":[{{"type":"region","name":"i","width":40,"height":40}}]}}"#);
        let ops = format!(
            r#"[{},{},{{"op":"key_draw_order","animation":"m","time":1,"order":["blue","red"]}}]"#,
            box_at("red", "#ff0000"), box_at("blue", "#0000ff"),
        );
        let doc = apply(&Document::with_root("t"), &parse_ops(&ops).unwrap()).unwrap();
        let center = |time: f32| {
            let pose = Pose::compute(&doc, Some((&doc.animations["m"], time)));
            let opts = RenderOptions { width: 64, height: 64, draw_bones: false, ..RenderOptions::default() };
            let pm = Renderer::new(std::env::temp_dir()).render(&doc, &pose, &opts, None);
            let p = pm.pixel(32, 32).unwrap();
            (p.red(), p.blue())
        };
        let (r, b) = center(0.0);
        assert!(b > r, "setup order: blue on top, got r{r} b{b}");
        let (r, b) = center(1.0);
        assert!(r > b, "keyed order: red on top, got r{r} b{b}");
    }
}
