//! Animated GIF export: one shared camera and one global palette over all
//! frames, so colors do not flicker and unchanged pixels compress away.

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use skel2d_core::pose::preview_tracks;
use skel2d_core::{Document, LayerTrack, MovePath, PhysicsMode, PhysicsRun, Pose};
use tiny_skia::Pixmap;

use crate::{Bounds, RenderOptions, Renderer};

/// GIF delays are whole hundredths of a second and viewers slow anything under
/// two of them down, so 50 is the highest frame rate that plays back correctly.
pub const MAX_GIF_FPS: f32 = 50.0;
const MAX_GIF_SIZE: u32 = 4096;
/// Pixels kept free around the content when the size comes from its bounds.
const MARGIN: u32 = 2;
const PHYSICS_FPS: f32 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GifOptions {
    pub fps: f32,
    /// Palette size including the transparent entry (2..=256).
    pub colors: u16,
    /// Floyd-Steinberg dithering: smoother gradients, larger files.
    pub dither: bool,
    /// 1..=10: frames sampled for the palette and how far it is refined.
    pub quality: u8,
    /// With `transparent`: pixels below this alpha become fully transparent.
    pub alpha_threshold: u8,
    /// Keeps the background see-through. Other pixels are flattened over `background`.
    pub transparent: bool,
    pub background: [u8; 3],
    /// Loops forever; otherwise the file plays once.
    pub forever: bool,
    /// Off leaves out the end of the animation, which repeats the first frame of a loop.
    pub include_last_frame: bool,
    /// Plays of the animation stored in the file.
    pub plays: u32,
    /// Seconds to hold the last frame of each play.
    pub pause_after: f32,
    /// Plays simulated before the first frame, so physics starts in motion.
    pub warm_up: u32,
    pub bones: bool,
    pub physics: bool,
    /// Size as a percentage of world units; ignored with `fit`.
    pub scale: f32,
    /// Fits the content into this many pixels.
    pub fit: Option<[u32; 2]>,
    /// With `fit`: small content grows until it touches the limit.
    pub enlarge: bool,
    /// With `fit`: the image is exactly `fit`, with space around the content.
    pub pad: bool,
    /// World rectangle [x, y, width, height] instead of the content's bounds.
    pub crop: Option<[f32; 4]>,
    /// First and last frame to keep, counted from 0.
    pub range: Option<[u32; 2]>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            fps: MAX_GIF_FPS,
            colors: 256,
            dither: false,
            quality: 5,
            alpha_threshold: 128,
            transparent: false,
            background: [255, 255, 255],
            forever: true,
            include_last_frame: false,
            plays: 1,
            pause_after: 0.0,
            warm_up: 0,
            bones: false,
            physics: true,
            scale: 100.0,
            fit: None,
            enlarge: false,
            pad: false,
            crop: None,
            range: None,
        }
    }
}

/// What to play: the setup pose without an animation or enabled layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GifJob {
    pub anim: Option<String>,
    pub layers: Vec<LayerTrack>,
    pub skins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GifStats {
    pub width: u32,
    pub height: u32,
    /// Stored frames; identical neighbours are merged into a longer delay.
    pub frames: u32,
    pub seconds: f32,
    pub colors: usize,
    pub missing_images: Vec<String>,
}

/// One planned output of a batch: the file name without extension and its job.
pub fn gif_plan(doc: &Document, job: &GifJob, all_animations: bool, all_skins: bool) -> Vec<(String, GifJob)> {
    let anims: Vec<Option<String>> = if all_animations { doc.animations.keys().cloned().map(Some).collect() } else { vec![job.anim.clone()] };
    let skins: Vec<Option<&str>> = if all_skins && !doc.skins.is_empty() { doc.skins.iter().map(|s| Some(s.name.as_str())).collect() } else { vec![None] };
    let mut plan = Vec::new();
    for anim in &anims {
        for skin in &skins {
            let mut stem = anim.clone().unwrap_or_else(|| "setup".to_string());
            if let Some(skin) = skin {
                stem = format!("{stem}-{skin}");
            }
            let skins = skin.map_or_else(|| job.skins.clone(), |s| vec![s.to_string()]);
            plan.push((safe_file_name(&stem), GifJob { anim: anim.clone(), layers: job.layers.clone(), skins }));
        }
    }
    plan
}

/// Animation names may hold `/` (face/happy); skin names may hold anything.
pub fn safe_file_name(name: &str) -> String {
    name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).collect()
}

/// The frame count of a job with these options, before identical frames merge.
pub fn gif_frame_count(doc: &Document, job: &GifJob, opts: &GifOptions) -> u32 {
    frame_plan(doc, job, opts).times.len() as u32
}

struct FramePlan {
    span: f32,
    /// Seconds from the start of the first play, and that frame's delay in hundredths.
    times: Vec<f32>,
    delays: Vec<u16>,
}

fn frame_plan(doc: &Document, job: &GifJob, opts: &GifOptions) -> FramePlan {
    let main = job.anim.as_ref().and_then(|n| doc.animations.get(n)).map(|a| a.duration());
    let span = main.unwrap_or_else(|| job.layers.iter().filter(|l| l.enabled && l.alpha > 0.0).filter_map(|l| doc.animations.get(&l.anim)).map(|a| a.duration()).fold(0.0, f32::max));
    let fps = opts.fps.clamp(0.1, MAX_GIF_FPS);
    let plays = opts.plays.max(1);
    let per_play = (span * fps).round() as u32;
    if per_play == 0 {
        return FramePlan { span, times: vec![0.0], delays: vec![100] };
    }
    let steps = per_play * plays;
    let total = steps + u32::from(opts.include_last_frame);
    let cs = |i: u32| (f64::from(i) * 100.0 / f64::from(fps)).round() as i64;
    let (mut times, mut delays) = (Vec::new(), Vec::new());
    for i in 0..total {
        if opts.range.is_some_and(|[first, last]| i < first || i > last) {
            continue;
        }
        times.push((i as f32 / fps).min(span * plays as f32));
        let mut delay = (cs(i + 1) - cs(i)).max(2);
        if (i + 1) % per_play == 0 && i + 1 >= per_play && (opts.include_last_frame || i + 1 < steps) || i + 1 == total {
            delay += (f64::from(opts.pause_after.max(0.0)) * 100.0).round() as i64;
        }
        delays.push(delay.min(i64::from(u16::MAX)) as u16);
    }
    FramePlan { span, times, delays }
}

fn poses(doc: &Document, job: &GifJob, opts: &GifOptions, plan: &FramePlan) -> Vec<Pose> {
    let plays = opts.plays.max(1);
    let warm = plan.span * opts.warm_up as f32;
    let main = job.anim.as_deref();
    let tracks = |clock: f32| {
        let mut tracks = preview_tracks(doc, main, clock, &job.layers);
        if main.is_some() && plan.span > 0.0
            && let Some(first) = tracks.first_mut() {
                // A clock on a play boundary is the start of the next play,
                // except past the last one, where it is the very end.
                let play = (clock / plan.span).floor().min((opts.warm_up + plays - 1) as f32);
                first.time = clock - play * plan.span;
            }
        tracks
    };
    let simulate = opts.physics && !doc.physics.is_empty() && !tracks(0.0).is_empty();
    let mut run = simulate.then(|| PhysicsRun::new(PHYSICS_FPS, MovePath::default()));
    plan.times.iter().map(|&t| {
        let clock = warm + t;
        match &mut run {
            Some(run) => {
                let frame = run.frame_at(clock);
                run.advance_frames(doc, &tracks, &job.skins, frame, &mut |_, _| {});
                Pose::compute_physics(doc, &tracks(clock), &job.skins, &mut run.state, PhysicsMode::Pose, 0.0)
            }
            None => Pose::compute_skinned(doc, &tracks(clock), &job.skins),
        }
    }).collect()
}

fn camera(renderer: &Renderer, doc: &Document, poses: &[Pose], opts: &GifOptions) -> Result<RenderOptions, String> {
    let fit = match opts.crop {
        Some([x, y, w, h]) => {
            if w <= 0.0 || h <= 0.0 || w.is_nan() || h.is_nan() { return Err("crop width and height must be positive".to_string()); }
            Bounds { min: [x, y], max: [x + w, y + h] }
        }
        None => poses.iter().map(|p| renderer.visible_bounds(doc, p, opts.bones)).reduce(Bounds::union).expect("at least one frame"),
    };
    let size = [(fit.max[0] - fit.min[0]).max(1.0), (fit.max[1] - fit.min[1]).max(1.0)];
    let margin = if opts.crop.is_some() { 0 } else { MARGIN };
    let zoom = match opts.fit {
        Some([w, h]) => {
            if w == 0 || h == 0 { return Err("fit width and height must be positive".to_string()); }
            let room = |limit: u32| limit.saturating_sub(margin * 2).max(1) as f32;
            let zoom = (room(w) / size[0]).min(room(h) / size[1]);
            if opts.enlarge { zoom } else { zoom.min(1.0) }
        }
        None => {
            if opts.scale <= 0.0 || opts.scale.is_nan() { return Err("scale must be positive".to_string()); }
            opts.scale / 100.0
        }
    };
    let measured = |side: f32| (side * zoom).ceil() as u32 + margin * 2;
    let (width, height) = match opts.fit {
        Some(limit) if opts.pad => (limit[0], limit[1]),
        _ => (measured(size[0]), measured(size[1])),
    };
    if width > MAX_GIF_SIZE || height > MAX_GIF_SIZE {
        return Err(format!("image would be {width}x{height} pixels, the limit is {MAX_GIF_SIZE}: lower the scale or fit it"));
    }
    let [r, g, b] = opts.background;
    Ok(RenderOptions {
        width,
        height,
        zoom: Some(zoom),
        center: Some([(fit.min[0] + fit.max[0]) / 2.0, (fit.min[1] + fit.max[1]) / 2.0]),
        background: if opts.transparent { [0, 0, 0, 0] } else { [r, g, b, 255] },
        draw_bones: opts.bones,
        draw_axes: false,
    })
}

/// Straight colors over the background; `None` marks a transparent pixel.
fn flatten(frame: &Pixmap, opts: &GifOptions) -> Vec<Option<[u8; 3]>> {
    let threshold = if opts.transparent { opts.alpha_threshold.max(1) } else { 0 };
    let bg = opts.background;
    frame.data().as_chunks::<4>().0.iter().map(|&[r, g, b, a]| {
        (a >= threshold).then(|| {
            // Pixels are premultiplied, so only the background needs scaling.
            let over = |c: u8, back: u8| (u16::from(c) + (u16::from(back) * u16::from(255 - a) + 127) / 255).min(255) as u8;
            [over(r, bg[0]), over(g, bg[1]), over(b, bg[2])]
        })
    }).collect()
}

const CELLS: usize = 1 << 18;

fn cell([r, g, b]: [u8; 3]) -> usize {
    (usize::from(r >> 2) << 12) | (usize::from(g >> 2) << 6) | usize::from(b >> 2)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    2.0 * d[0] * d[0] + 4.0 * d[1] * d[1] + 3.0 * d[2] * d[2]
}

#[derive(Default)]
struct Histogram {
    cells: Vec<[u64; 4]>,
}

impl Histogram {
    fn new() -> Self {
        Self { cells: vec![[0; 4]; CELLS] }
    }

    fn add(&mut self, pixels: &[Option<[u8; 3]>]) {
        for rgb in pixels.iter().flatten() {
            let c = &mut self.cells[cell(*rgb)];
            for (sum, v) in c.iter_mut().zip(rgb) {
                *sum += u64::from(*v);
            }
            c[3] += 1;
        }
    }

    /// Median cut over the cells in use, then `passes` rounds of k-means.
    fn palette(&self, colors: usize, passes: u32) -> Vec<[u8; 3]> {
        let mut entries: Vec<([f32; 3], f32)> = self.cells.iter().filter(|c| c[3] > 0).map(|c| {
            let n = c[3] as f32;
            ([c[0] as f32 / n, c[1] as f32 / n, c[2] as f32 / n], n)
        }).collect();
        #[allow(clippy::single_range_in_vec_init)] // One box that gets split further.
        let mut boxes = vec![0..entries.len()];
        while boxes.len() < colors {
            let scored = boxes.iter().enumerate().filter_map(|(i, b)| {
                let items = &entries[b.clone()];
                if items.len() < 2 { return None; }
                let spread = |axis: usize| {
                    let (lo, hi) = items.iter().fold((f32::MAX, f32::MIN), |(lo, hi), e| (lo.min(e.0[axis]), hi.max(e.0[axis])));
                    hi - lo
                };
                let (axis, range) = (0..3).map(|a| (a, spread(a))).max_by(|x, y| x.1.total_cmp(&y.1)).expect("three axes");
                let pixels: f32 = items.iter().map(|e| e.1).sum();
                Some((i, axis, range * range * pixels))
            }).max_by(|a, b| a.2.total_cmp(&b.2));
            let Some((i, axis, _)) = scored else { break };
            let range = boxes[i].clone();
            entries[range.clone()].sort_by(|a, b| a.0[axis].total_cmp(&b.0[axis]));
            let half = entries[range.clone()].iter().map(|e| e.1).sum::<f32>() / 2.0;
            let mut seen = 0.0;
            let mut cut = range.start + 1;
            for (k, e) in entries[range.clone()].iter().enumerate() {
                seen += e.1;
                if seen >= half {
                    cut = (range.start + k + 1).clamp(range.start + 1, range.end - 1);
                    break;
                }
            }
            boxes[i] = range.start..cut;
            boxes.push(cut..range.end);
        }
        let mean = |items: &[([f32; 3], f32)]| {
            let total: f32 = items.iter().map(|e| e.1).sum();
            let channel = |a: usize| items.iter().map(|e| e.0[a] * e.1).sum::<f32>() / total;
            [channel(0), channel(1), channel(2)]
        };
        let mut palette: Vec<[f32; 3]> = boxes.iter().filter(|b| !b.is_empty()).map(|b| mean(&entries[b.clone()])).collect();
        for _ in 0..passes {
            let mut sums = vec![([0.0f32; 3], 0.0f32); palette.len()];
            for (color, n) in &entries {
                let nearest = (0..palette.len()).min_by(|&a, &b| distance(*color, palette[a]).total_cmp(&distance(*color, palette[b]))).expect("palette not empty");
                let s = &mut sums[nearest];
                for (sum, v) in s.0.iter_mut().zip(color) { *sum += v * n; }
                s.1 += n;
            }
            for (p, s) in palette.iter_mut().zip(&sums) {
                if s.1 > 0.0 { *p = [s.0[0] / s.1, s.0[1] / s.1, s.0[2] / s.1]; }
            }
        }
        palette.into_iter().map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect()
    }
}

struct Mapper {
    palette: Vec<[f32; 3]>,
    cache: Vec<u16>,
}

impl Mapper {
    fn new(palette: &[[u8; 3]]) -> Self {
        Self { palette: palette.iter().map(|c| c.map(f32::from)).collect(), cache: vec![u16::MAX; CELLS] }
    }

    fn index(&mut self, rgb: [u8; 3]) -> u8 {
        let key = cell(rgb);
        if self.cache[key] == u16::MAX {
            let target = rgb.map(f32::from);
            let nearest = (0..self.palette.len()).min_by(|&a, &b| distance(target, self.palette[a]).total_cmp(&distance(target, self.palette[b]))).expect("palette not empty");
            self.cache[key] = nearest as u16;
        }
        self.cache[key] as u8
    }

    fn map(&mut self, pixels: &[Option<[u8; 3]>], width: usize, transparent: u8, dither: bool) -> Vec<u8> {
        if !dither {
            return pixels.iter().map(|p| p.map_or(transparent, |rgb| self.index(rgb))).collect();
        }
        // Floyd-Steinberg, with the error kept for this and the next row.
        let mut out = Vec::with_capacity(pixels.len());
        let (mut here, mut below) = (vec![[0.0f32; 3]; width + 2], vec![[0.0f32; 3]; width + 2]);
        for row in pixels.chunks(width) {
            for (x, pixel) in row.iter().enumerate() {
                let Some(rgb) = pixel else { out.push(transparent); continue };
                let wanted = [0, 1, 2].map(|a| (f32::from(rgb[a]) + here[x + 1][a]).clamp(0.0, 255.0));
                let index = self.index(wanted.map(|v| v.round() as u8));
                out.push(index);
                let got = self.palette[usize::from(index)];
                for a in 0..3 {
                    let error = wanted[a] - got[a];
                    here[x + 2][a] += error * 7.0 / 16.0;
                    below[x][a] += error * 3.0 / 16.0;
                    below[x + 1][a] += error * 5.0 / 16.0;
                    below[x + 2][a] += error * 1.0 / 16.0;
                }
            }
            std::mem::swap(&mut here, &mut below);
            below.fill([0.0; 3]);
        }
        out
    }
}

/// The smallest rectangle holding every difference, as (left, top, width, height).
fn changed_rect(a: &[u8], b: &[u8], width: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut left, mut right, mut top, mut bottom) = (usize::MAX, 0, usize::MAX, 0);
    for (y, (ra, rb)) in a.chunks(width).zip(b.chunks(width)).enumerate() {
        if ra == rb { continue; }
        let first = ra.iter().zip(rb).position(|(x, y)| x != y).expect("rows differ");
        let last = ra.iter().zip(rb).rposition(|(x, y)| x != y).expect("rows differ");
        left = left.min(first);
        right = right.max(last);
        top = top.min(y);
        bottom = y;
    }
    (top != usize::MAX).then(|| (left, top, right - left + 1, bottom - top + 1))
}

/// Writes the job as a GIF. `progress(done, total)` returning false cancels.
pub fn render_gif(doc: &Document, base_dir: &Path, job: &GifJob, opts: &GifOptions, out: &mut impl Write, progress: &mut dyn FnMut(u32, u32) -> bool) -> Result<GifStats, String> {
    if let Some(name) = &job.anim && !doc.animations.contains_key(name) {
        return Err(format!("animation {name:?} does not exist"));
    }
    if let Some(s) = job.skins.iter().find(|s| doc.skin(s).is_none()) {
        return Err(format!("skin {s:?} does not exist"));
    }
    if opts.fps <= 0.0 || opts.fps > MAX_GIF_FPS || opts.fps.is_nan() {
        return Err(format!("fps must be above 0 and at most {MAX_GIF_FPS}: GIF cannot time frames faster than that"));
    }
    let plan = frame_plan(doc, job, opts);
    if plan.times.is_empty() {
        return Err("the frame range holds no frames".to_string());
    }
    let poses = poses(doc, job, opts, &plan);
    let mut renderer = Renderer::new(base_dir);
    let view = camera(&renderer, doc, &poses, opts)?;
    let (width, height) = (view.width as usize, view.height as usize);

    let quality = u32::from(opts.quality.clamp(1, 10));
    let samples = (quality * 6).min(poses.len() as u32).max(1);
    let total = samples + poses.len() as u32;
    let mut histogram = Histogram::new();
    for i in 0..samples {
        let pose = &poses[i as usize * poses.len() / samples as usize];
        histogram.add(&flatten(&renderer.render(doc, pose, &view, None), opts));
        if !progress(i + 1, total) {
            return Err("cancelled".to_string());
        }
    }
    let bg = opts.background;
    let colors = usize::from(opts.colors.clamp(2, 256)) - 1;
    let mut palette = histogram.palette(colors, quality / 2);
    if palette.is_empty() {
        palette.push(bg);
    }
    drop(histogram);
    let transparent = palette.len() as u8;
    let mut mapper = Mapper::new(&palette);
    let mut global: Vec<u8> = palette.iter().flatten().copied().collect();
    global.extend(bg);

    let mut encoder = gif::Encoder::new(&mut *out, view.width as u16, view.height as u16, &global).map_err(|e| e.to_string())?;
    if opts.forever {
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    }
    // A see-through background cannot be patched: a pixel that turns transparent
    // has to clear what is below it, so such frames are stored whole.
    let dispose = if opts.transparent { gif::DisposalMethod::Background } else { gif::DisposalMethod::Keep };
    let (mut previous, mut pending): (Option<Vec<u8>>, Option<gif::Frame>) = (None, None);
    let mut written = 0;
    for (i, pose) in poses.iter().enumerate() {
        let image = renderer.render(doc, pose, &view, None);
        let indices = mapper.map(&flatten(&image, opts), width, transparent, opts.dither);
        let delay = plan.delays[i];
        let rect = previous.as_ref().map_or(Some((0, 0, width, height)), |p| changed_rect(p, &indices, width));
        match rect {
            None => {
                let frame = pending.as_mut().expect("a previous frame exists");
                frame.delay = frame.delay.saturating_add(delay);
            }
            Some((left, top, w, h)) => {
                if let Some(frame) = pending.take() {
                    encoder.write_frame(&frame).map_err(|e| e.to_string())?;
                    written += 1;
                }
                let (rect, buffer) = if let (false, Some(before)) = (opts.transparent, &previous) {
                    let mut patch = Vec::with_capacity(w * h);
                    for y in top..top + h {
                        for x in left..left + w {
                            let at = y * width + x;
                            patch.push(if before[at] == indices[at] { transparent } else { indices[at] });
                        }
                    }
                    ((left, top, w, h), patch)
                } else {
                    ((0, 0, width, height), indices.clone())
                };
                pending = Some(gif::Frame {
                    delay,
                    dispose,
                    transparent: Some(transparent),
                    left: rect.0 as u16,
                    top: rect.1 as u16,
                    width: rect.2 as u16,
                    height: rect.3 as u16,
                    buffer: Cow::Owned(buffer),
                    ..gif::Frame::default()
                });
            }
        }
        previous = Some(indices);
        if !progress(samples + i as u32 + 1, total) {
            return Err("cancelled".to_string());
        }
    }
    if let Some(frame) = pending.take() {
        encoder.write_frame(&frame).map_err(|e| e.to_string())?;
        written += 1;
    }
    drop(encoder);
    Ok(GifStats {
        width: view.width,
        height: view.height,
        frames: written,
        seconds: plan.delays.iter().map(|&d| f32::from(d)).sum::<f32>() / 100.0,
        colors: palette.len(),
        missing_images: renderer.missing_images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use skel2d_core::{apply, parse_ops};

    fn doc() -> Document {
        let ops = parse_ops(
            r##"[{"op":"add_slot","name":"s","bone":"root","color":"#ff0000","attachment":"r",
                 "attachments":[{"type":"region","name":"r","width":40,"height":20}]},
                {"op":"key_bone","animation":"spin","bone":"root","time":0,"rotate":0},
                {"op":"key_bone","animation":"spin","bone":"root","time":1,"rotate":360},
                {"op":"key_slot","animation":"still","slot":"s","time":1,"color":"#ff0000"}]"##,
        )
        .unwrap();
        apply(&Document::with_root("t"), &ops).unwrap()
    }

    fn job(anim: &str) -> GifJob {
        GifJob { anim: Some(anim.to_string()), ..GifJob::default() }
    }

    fn encode(doc: &Document, job: &GifJob, opts: &GifOptions) -> (GifStats, Vec<u8>) {
        let mut bytes = Vec::new();
        let stats = render_gif(doc, Path::new("."), job, opts, &mut bytes, &mut |_, _| true).unwrap();
        (stats, bytes)
    }

    fn decode(bytes: &[u8]) -> Vec<(u16, Vec<u8>, gif::DisposalMethod)> {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(bytes).unwrap();
        let mut frames = Vec::new();
        while let Some(f) = decoder.read_next_frame().unwrap() {
            frames.push((f.delay, f.buffer.to_vec(), f.dispose));
        }
        frames
    }

    #[test]
    fn timing_adds_up_to_the_animation_at_any_rate() {
        let doc = doc();
        for (fps, frames) in [(50.0, 50), (30.0, 30), (12.0, 12)] {
            let opts = GifOptions { fps, ..GifOptions::default() };
            let (stats, bytes) = encode(&doc, &job("spin"), &opts);
            let decoded = decode(&bytes);
            assert_eq!(decoded.len(), frames, "{fps} fps");
            assert_eq!(decoded.iter().map(|f| u32::from(f.0)).sum::<u32>(), 100, "{fps} fps");
            assert_eq!(stats.frames as usize, frames);
        }
    }

    #[test]
    fn last_frame_range_and_plays_change_the_frame_list() {
        let doc = doc();
        let count = |opts: GifOptions| gif_frame_count(&doc, &job("spin"), &opts);
        assert_eq!(count(GifOptions { fps: 10.0, ..GifOptions::default() }), 10);
        assert_eq!(count(GifOptions { fps: 10.0, include_last_frame: true, ..GifOptions::default() }), 11);
        assert_eq!(count(GifOptions { fps: 10.0, plays: 3, ..GifOptions::default() }), 30);
        assert_eq!(count(GifOptions { fps: 10.0, range: Some([2, 4]), ..GifOptions::default() }), 3);
    }

    #[test]
    fn pause_extends_the_last_frame_of_each_play() {
        let doc = doc();
        let opts = GifOptions { fps: 10.0, plays: 2, pause_after: 0.5, ..GifOptions::default() };
        let plan = frame_plan(&doc, &job("spin"), &opts);
        assert_eq!((plan.delays[9], plan.delays[19], plan.delays[0]), (60, 60, 10));
    }

    #[test]
    fn unchanged_frames_merge_into_one_longer_delay() {
        let doc = doc();
        let (stats, bytes) = encode(&doc, &job("still"), &GifOptions { fps: 10.0, ..GifOptions::default() });
        let decoded = decode(&bytes);
        assert_eq!((stats.frames, decoded.len()), (1, 1));
        assert_eq!(decoded[0].0, 100);
    }

    #[test]
    fn opaque_and_transparent_backgrounds() {
        let doc = doc();
        let corner_alpha = |transparent: bool| {
            let opts = GifOptions { fps: 5.0, transparent, ..GifOptions::default() };
            let (stats, bytes) = encode(&doc, &job("spin"), &opts);
            let decoded = decode(&bytes);
            if transparent { assert!(decoded.iter().all(|f| f.2 == gif::DisposalMethod::Background)); }
            assert_eq!(decoded[0].1.len() as u32, stats.width * stats.height * 4);
            decoded[0].1[3]
        };
        assert_eq!(corner_alpha(false), 255);
        assert_eq!(corner_alpha(true), 0);
    }

    #[test]
    fn size_follows_scale_fit_and_pad() {
        let doc = doc();
        let size = |opts: GifOptions| {
            let (stats, _) = encode(&doc, &job("still"), &GifOptions { fps: 5.0, ..opts });
            (stats.width, stats.height)
        };
        let (w, h) = size(GifOptions::default());
        assert_eq!((w, h), (40 + MARGIN * 2, 20 + MARGIN * 2));
        assert_eq!(size(GifOptions { scale: 200.0, ..GifOptions::default() }), (80 + MARGIN * 2, 40 + MARGIN * 2));
        assert_eq!(size(GifOptions { fit: Some([24, 24]), ..GifOptions::default() }), (24, 14));
        assert_eq!(size(GifOptions { fit: Some([100, 100]), ..GifOptions::default() }), (w, h));
        assert_eq!(size(GifOptions { fit: Some([84, 84]), enlarge: true, ..GifOptions::default() }), (84, 44));
        assert_eq!(size(GifOptions { fit: Some([100, 60]), pad: true, ..GifOptions::default() }), (100, 60));
        assert_eq!(size(GifOptions { crop: Some([-5.0, -5.0, 10.0, 8.0]), ..GifOptions::default() }), (10, 8));
    }

    #[test]
    fn colors_are_limited_and_exact_for_flat_art() {
        let dir = std::env::temp_dir().join(format!("skel2d-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut red = Pixmap::new(8, 8).unwrap();
        red.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        red.save_png(dir.join("red.png")).unwrap();
        let ops = parse_ops(r#"[{"op":"add_slot","name":"s","bone":"root","attachment":"r",
            "attachments":[{"type":"region","name":"r","image":"red.png","width":40,"height":20}]}]"#).unwrap();
        let doc = apply(&Document::with_root("t"), &ops).unwrap();
        let mut bytes = Vec::new();
        let opts = GifOptions { background: [0, 0, 255], ..GifOptions::default() };
        let stats = render_gif(&doc, &dir, &GifJob::default(), &opts, &mut bytes, &mut |_, _| true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stats.colors <= 3, "{}", stats.colors);
        let pixels = &decode(&bytes)[0].1;
        let center = (stats.height / 2 * stats.width + stats.width / 2) as usize * 4;
        assert_eq!(&pixels[center..center + 3], &[255, 0, 0]);
        assert_eq!(&pixels[..3], &[0, 0, 255]);
        let (few, _) = encode(&self::tests::doc(), &job("spin"), &GifOptions { fps: 5.0, colors: 4, ..GifOptions::default() });
        assert!(few.colors <= 3);
    }

    #[test]
    fn errors_and_cancel_are_reported() {
        let doc = doc();
        let fail = |job: &GifJob, opts: &GifOptions| render_gif(&doc, Path::new("."), job, opts, &mut Vec::new(), &mut |_, _| true).unwrap_err();
        assert!(fail(&job("nope"), &GifOptions::default()).contains("does not exist"));
        assert!(fail(&job("spin"), &GifOptions { fps: 60.0, ..GifOptions::default() }).contains("fps"));
        assert!(fail(&job("spin"), &GifOptions { range: Some([500, 600]), ..GifOptions::default() }).contains("range"));
        assert!(fail(&job("spin"), &GifOptions { scale: 100000.0, ..GifOptions::default() }).contains("limit"));
        let cancelled = render_gif(&doc, Path::new("."), &job("spin"), &GifOptions::default(), &mut Vec::new(), &mut |_, _| false);
        assert_eq!(cancelled.unwrap_err(), "cancelled");
    }

    #[test]
    fn plan_names_files_by_animation_and_skin() {
        let ops = parse_ops(r#"[{"op":"add_skin","name":"red"},{"op":"add_skin","name":"blue"},
            {"op":"key_bone","animation":"face/happy","bone":"root","time":0,"rotate":0}]"#).unwrap();
        let doc = apply(&doc(), &ops).unwrap();
        let plan = gif_plan(&doc, &job("spin"), false, true);
        assert_eq!(plan.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["spin-red", "spin-blue"]);
        assert_eq!(plan[1].1.skins, ["blue"]);
        let all = gif_plan(&doc, &job("spin"), true, false);
        assert!(all.iter().any(|p| p.0 == "face_happy"));
        assert_eq!(all.len(), doc.animations.len());
    }
}
