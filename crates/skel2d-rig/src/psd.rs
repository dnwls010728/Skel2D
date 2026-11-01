//! Photoshop import: a layered PSD becomes one image per part plus the
//! placement a `layers.json` manifest needs. With [`Split::Groups`] every
//! top-level group is one part (its layers composited, e.g. line art over
//! flat color) and a layer outside any group is a part of its own; with
//! [`Split::Layers`] every layer is a part and groups only organize them;
//! with [`Split::Innermost`] every innermost group is
//! a part. [`Hidden`] picks what happens to hidden layers.
//! A clipped layer is painted onto its base as Photoshop shows it, never as a
//! part of its own, so it cannot grow the part past its base. A paper layer
//! (Clip Studio's `용지` / `Paper` / `用紙` background) is never a part.
//! Blend modes apply inside a part: a layer alone in its part has nothing
//! under it to blend with. Layer masks and group opacity are not applied.

use std::collections::{HashMap, HashSet};

use tiny_skia::{BlendMode, ColorU8, IntSize, Pixmap, PixmapPaint, PremultipliedColorU8, Transform};

use crate::ALPHA_THRESHOLD;
use crate::mask::Mask;

pub struct PsdPart {
    pub name: String,
    /// Cropped to the visible pixels.
    pub image: Pixmap,
    /// Top-left of `image` on the canvas.
    pub x: u32,
    pub y: u32,
    /// Number of PSD layers composited into this part.
    pub layers: usize,
    /// Hidden in the PSD and kept by [`Hidden::Transparent`].
    pub hidden: bool,
    /// With [`Split::LayersKeepClipping`]: index of the part this one clips to.
    pub clip_to: Option<usize>,
    /// With [`Split::LayersKeepClipping`]: the layer's blend, for modes a slot has.
    pub blend: skel2d_core::BlendMode,
}

/// What becomes one part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Split {
    /// Each top-level group (composited) or ungrouped layer.
    Groups,
    /// Each layer; groups only organize them.
    Layers,
    /// Each innermost group (composited) or ungrouped layer. A group's own
    /// layers split around its subgroups, so the stacking order stays right.
    /// The same as `Groups` for a PSD whose groups do not nest.
    #[default]
    Innermost,
    /// Each layer, and a clipped layer stays a part of its own with
    /// [`PsdPart::clip_to`] naming its base, so it can move separately.
    LayersKeepClipping,
}

/// What happens to layers hidden in the PSD. A layer also counts as hidden
/// inside a hidden group or when clipped to a hidden base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hidden {
    /// Hidden layers are not imported.
    #[default]
    Skip,
    /// A part with only hidden layers is imported and marked
    /// [`PsdPart::hidden`]. Hidden layers of a part that also shows are dropped.
    Transparent,
    /// Hidden layers are imported like visible ones, e.g. when every part is
    /// hidden behind a flattened reference.
    Include,
}

/// Layer names painting programs give the paper background.
const PAPER: [&str; 3] = ["용지", "Paper", "用紙"];

/// A layer with this in its name marks the origin and is never a part.
const ORIGIN_TAG: &str = "[origin]";

/// Where a PSD says its skeleton origin is, in canvas pixels (y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Origin {
    pub x: f32,
    pub y: f32,
    pub source: OriginSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginSource {
    /// The center of a layer tagged `[origin]`.
    Layer,
    /// The first vertical and horizontal guides; a missing one falls back to the canvas center.
    Guides,
    /// Nothing in the file says, so the canvas center.
    Canvas,
}

/// The origin in order of priority: an `[origin]` layer (hidden or not), then
/// guides, then the canvas center. Photoshop's ruler origin is not used: the
/// sample files hold no trace of it and where it is stored is not documented.
pub fn origin(bytes: &[u8]) -> Result<Origin, String> {
    let psd = psd::Psd::from_bytes(bytes).map_err(|e| format!("not a readable PSD: {e}"))?;
    if let Some(l) = psd.layers().iter().rev().find(|l| l.name().contains(ORIGIN_TAG)) {
        return Ok(Origin { x: l.layer_left() as f32 + l.width() as f32 / 2.0, y: l.layer_top() as f32 + l.height() as f32 / 2.0, source: OriginSource::Layer });
    }
    let center = [psd.width() as f32 / 2.0, psd.height() as f32 / 2.0];
    match first_guides(bytes) {
        Some([None, None]) | None => Ok(Origin { x: center[0], y: center[1], source: OriginSource::Canvas }),
        Some([x, y]) => Ok(Origin { x: x.unwrap_or(center[0]), y: y.unwrap_or(center[1]), source: OriginSource::Guides }),
    }
}

/// The first vertical guide's x and horizontal guide's y, from the grid and
/// guides image resource (locations are in 1/32 pixel). `None` when unreadable.
fn first_guides(b: &[u8]) -> Option<[Option<f32>; 2]> {
    let u32_at = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize);
    let mut p = 26;
    p += 4 + u32_at(p)?; // color mode data
    let end = p + 4 + u32_at(p)?;
    p += 4;
    while p + 8 <= end && b.get(p..p + 4)? == b"8BIM" {
        let id = u16::from_be_bytes(b.get(p + 4..p + 6)?.try_into().ok()?);
        let q = p + 6 + ((*b.get(p + 6)? as usize + 2) & !1); // padded name
        let size = u32_at(q)?;
        let data = q + 4;
        if id == 1032 {
            // Version, grid cycles (8 bytes), guide count, then 5 bytes per guide.
            let count = u32_at(data + 12)?;
            let mut found = [None, None];
            for g in 0..count {
                let at = data + 16 + g * 5;
                let location = i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as f32 / 32.0;
                let axis = (*b.get(at + 4)? != 0) as usize; // 0 vertical, 1 horizontal
                found[axis].get_or_insert(location);
            }
            return Some(found);
        }
        p = data + ((size + 1) & !1);
    }
    Some([None, None])
}

/// A base layer and the clipped layers painted on it so far. `target` is
/// the image and its part, or `None` when the base was left out, which leaves
/// out what is clipped to it too.
struct ClipBase {
    parent: Option<u32>,
    blend: BlendMode,
    target: Option<(Piece, usize)>,
}

/// Pixels of one layer, cropped to its bounds, placed on the canvas. Working
/// on these rather than full canvases keeps a many-layer PSD fast and small.
struct Piece {
    image: Pixmap,
    x: i32,
    y: i32,
}

/// The blend a PSD mode key maps to; modes tiny-skia lacks draw as normal.
fn blend_mode(key: &[u8; 4]) -> BlendMode {
    match key {
        b"mul " => BlendMode::Multiply,
        b"scrn" => BlendMode::Screen,
        b"over" => BlendMode::Overlay,
        b"dark" => BlendMode::Darken,
        b"lite" => BlendMode::Lighten,
        b"div " => BlendMode::ColorDodge,
        b"idiv" => BlendMode::ColorBurn,
        b"hLit" => BlendMode::HardLight,
        b"sLit" => BlendMode::SoftLight,
        b"diff" => BlendMode::Difference,
        b"smud" => BlendMode::Exclusion,
        b"hue " => BlendMode::Hue,
        b"sat " => BlendMode::Saturation,
        b"colr" => BlendMode::Color,
        b"lum " => BlendMode::Luminosity,
        b"lddg" => BlendMode::Plus,
        _ => BlendMode::SourceOver,
    }
}

/// The slot blend closest to a PSD mode key; others show as normal.
fn slot_blend_mode(key: &[u8; 4]) -> skel2d_core::BlendMode {
    match key {
        b"mul " => skel2d_core::BlendMode::Multiply,
        b"scrn" => skel2d_core::BlendMode::Screen,
        b"lddg" => skel2d_core::BlendMode::Additive,
        _ => skel2d_core::BlendMode::Normal,
    }
}

/// Paints `src` onto `base` with `blend`, keeping the alpha of `base`.
fn clip_onto(base: &mut Piece, src: &Piece, blend: BlendMode) {
    let paint = |blend_mode| PixmapPaint { blend_mode, ..Default::default() };
    // Only the overlap, so the draw never starts off the image.
    let (x, y) = ((src.x - base.x).max(0), (src.y - base.y).max(0));
    let (sx, sy) = ((base.x - src.x).max(0), (base.y - src.y).max(0));
    let width = (base.image.width() as i32 - x).min(src.image.width() as i32 - sx);
    let height = (base.image.height() as i32 - y).min(src.image.height() as i32 - sy);
    if width <= 0 || height <= 0 {
        return;
    }
    let overlap = tiny_skia::IntRect::from_xywh(sx, sy, width as u32, height as u32).and_then(|r| src.image.clone_rect(r)).expect("overlap is inside src");
    if blend == BlendMode::SourceOver {
        base.image.draw_pixmap(x, y, overlap.as_ref(), &paint(BlendMode::SourceAtop), Transform::identity(), None);
        return;
    }
    let mut blended = base.image.clone();
    blended.draw_pixmap(x, y, overlap.as_ref(), &paint(blend), Transform::identity(), None);
    for (out, b) in base.image.pixels_mut().iter_mut().zip(blended.pixels()) {
        let a = out.alpha();
        if a == 0 || b.alpha() == 0 {
            continue;
        }
        let f = a as f32 / b.alpha() as f32;
        let channel = |c: u8| ((c as f32 * f).round() as u8).min(a);
        *out = PremultipliedColorU8::from_rgba(channel(b.red()), channel(b.green()), channel(b.blue()), a).expect("channels are clamped to alpha");
    }
}

struct Building {
    name: String,
    /// A full canvas for a merged group; `None` for a part of one layer.
    canvas: Option<Pixmap>,
    /// The layer of a part of one layer.
    piece: Option<Piece>,
    layers: usize,
    hidden: bool,
    /// The building this one clips to, with [`Split::LayersKeepClipping`].
    clip_to: Option<usize>,
    blend: skel2d_core::BlendMode,
}

fn flush(base: Option<ClipBase>, order: &mut [Building]) {
    if let Some(ClipBase { blend, target: Some((piece, slot)), .. }) = base {
        match &mut order[slot].canvas {
            Some(canvas) => {
                let paint = PixmapPaint { blend_mode: blend, ..Default::default() };
                canvas.draw_pixmap(piece.x, piece.y, piece.image.as_ref(), &paint, Transform::identity(), None);
            }
            None => order[slot].piece = Some(piece),
        }
    }
}

/// Canvas size and parts back to front. A layer is skipped when its name
/// or the name of any group it is in is in `exclude`, e.g. a flattened
/// reference.
pub fn import(bytes: &[u8], exclude: &[String], split: Split, hidden: Hidden) -> Result<([u32; 2], Vec<PsdPart>), String> {
    let psd = psd::Psd::from_bytes(bytes).map_err(|e| format!("not a readable PSD: {e}"))?;
    let (w, h) = (psd.width(), psd.height());
    let groups = psd.groups();
    // Group ids from the layer's own group up to the top level.
    let ancestors = |mut id: Option<u32>| {
        std::iter::from_fn(move || {
            let g = id?;
            id = groups.get(&g).and_then(|g| g.parent_id());
            Some(g)
        })
    };
    let group_name = |g: u32| groups.get(&g).map_or_else(|| format!("group_{g}"), |g| g.name().to_string());
    let part_group = |parent: Option<u32>| match split {
        Split::Groups => ancestors(parent).last(),
        Split::Layers | Split::LayersKeepClipping => None,
        Split::Innermost => parent,
    };
    let keep_clips = split == Split::LayersKeepClipping;

    // `layers()` lists the top of the stack first.
    let layers: Vec<_> = psd.layers().iter().rev().collect();
    let flags = match (hidden, record_flags(bytes)) {
        (_, Some(flags)) if flags.layers.len() == layers.len() => flags,
        (Hidden::Include, _) => RecordFlags { layers: vec![true; layers.len()], blends: vec![*b"norm"; layers.len()], folders: Vec::new() },
        _ => return Err("could not read which PSD layers are hidden".into()),
    };
    let slot_blend = |i: usize| if keep_clips { slot_blend_mode(&flags.blends[i]) } else { skel2d_core::BlendMode::Normal };
    // Records are stored bottom first, the same order as `layers` here.
    let own_visible = |i: usize| flags.layers[i];
    let folder_visible = |g: u32| flags.folders.get(g as usize - 1).copied().unwrap_or(true);
    // The crate's flag is true for a base layer: the record stores 0 for one.
    let clipped_to = |i: usize, base: Option<Option<u32>>| !layers[i].is_clipping_mask() && base == Some(layers[i].parent_id());

    // Back to front, what Photoshop shows.
    let mut shown = Vec::with_capacity(layers.len());
    let mut base: Option<(Option<u32>, bool)> = None;
    for (i, layer) in layers.iter().enumerate() {
        let own = own_visible(i) && ancestors(layer.parent_id()).all(folder_visible);
        shown.push(if clipped_to(i, base.map(|b| b.0)) {
            own && base.is_some_and(|b| b.1)
        } else {
            base = Some((layer.parent_id(), own));
            own
        });
    }
    let lit: HashSet<u32> = layers.iter().zip(&shown).filter(|(_, s)| **s).filter_map(|(l, _)| part_group(l.parent_id())).collect();

    let mut order: Vec<Building> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut open: Option<ClipBase> = None;
    for (i, layer) in layers.iter().enumerate() {
        let parent = layer.parent_id();
        let excluded = layer.name().contains(ORIGIN_TAG) || exclude.iter().any(|x| x == layer.name()) || ancestors(parent).any(|g| exclude.contains(&group_name(g)));
        if clipped_to(i, open.as_ref().map(|b| b.parent)) {
            let keep = match hidden {
                Hidden::Skip => shown[i],
                Hidden::Transparent => shown[i] || own_visible(i),
                Hidden::Include => true,
            };
            if keep_clips {
                if let Some(base) = open.as_ref().and_then(|b| b.target.as_ref()).map(|t| t.1)
                    && keep
                    && !excluded
                    && let Some(piece) = layer_piece(layer, w, h)?
                {
                    let is_hidden = hidden == Hidden::Transparent && !shown[i];
                    order.push(Building { name: layer.name().to_string(), canvas: None, piece: Some(piece), layers: 1, hidden: is_hidden, clip_to: Some(base), blend: slot_blend(i) });
                }
                continue;
            }
            if let Some((piece, slot)) = open.as_mut().and_then(|b| b.target.as_mut())
                && keep
                && !excluded
            {
                if let Some(src) = layer_piece(layer, w, h)? {
                    clip_onto(piece, &src, blend_mode(&flags.blends[i]));
                }
                order[*slot].layers += 1;
            }
            continue;
        }
        flush(open.take(), &mut order);
        let group = part_group(parent);
        let keep = match hidden {
            Hidden::Skip => shown[i],
            Hidden::Transparent => shown[i] || !group.is_some_and(|g| lit.contains(&g)),
            Hidden::Include => true,
        };
        if excluded || !keep || (parent.is_none() && PAPER.contains(&layer.name())) {
            open = Some(ClipBase { parent, blend: BlendMode::SourceOver, target: None });
            continue;
        }
        let name = group.map_or_else(|| layer.name().to_string(), group_name);
        let piece = layer_piece(layer, w, h)?;
        // Innermost: rejoin a group only when nothing was stacked in between.
        let slot = match group.and_then(|g| index.get(&g)).filter(|&&s| split != Split::Innermost || s + 1 == order.len()) {
            Some(&s) => s,
            None => {
                let canvas = group.map(|_| Pixmap::new(w, h).ok_or("PSD canvas is empty")).transpose()?;
                order.push(Building { name, canvas, piece: None, layers: 0, hidden: hidden == Hidden::Transparent && !shown[i], clip_to: None, blend: slot_blend(i) });
                if let Some(g) = group {
                    index.insert(g, order.len() - 1);
                }
                order.len() - 1
            }
        };
        order[slot].layers += 1;
        open = Some(ClipBase { parent, blend: blend_mode(&flags.blends[i]), target: piece.map(|p| (p, slot)) });
    }
    flush(open, &mut order);

    // Where each building ended up among the parts: empty ones drop out.
    let mut placed: Vec<Option<usize>> = Vec::with_capacity(order.len());
    let mut parts = Vec::new();
    for p in order {
        // What clips to a part with no pixels would never show.
        let base = p.clip_to.map(|b| placed[b]);
        let part = (|| {
            let base = match base {
                Some(None) => return None,
                Some(found) => found,
                None => None,
            };
            let (image, ox, oy) = match (p.canvas, p.piece) {
                (Some(canvas), _) => (canvas, 0, 0),
                (None, Some(piece)) => (piece.image, piece.x, piece.y),
                (None, None) => return None,
            };
            let b = Mask::from_pixmap(&image, ALPHA_THRESHOLD).bbox()?;
            let rect = tiny_skia::IntRect::from_xywh(b.x0 as i32, b.y0 as i32, b.width() as u32, b.height() as u32)?;
            Some(PsdPart { name: p.name, image: image.clone_rect(rect)?, x: (b.x0 as i32 + ox) as u32, y: (b.y0 as i32 + oy) as u32, layers: p.layers, hidden: p.hidden, clip_to: base, blend: p.blend })
        })();
        placed.push(part.is_some().then_some(parts.len()));
        parts.extend(part);
    }
    Ok(([w, h], parts))
}

/// Visibility read from the layer records, since the `psd` crate reads bit 1
/// of the flags the wrong way round and takes a group's flags from its closing
/// divider. `layers` runs in file order (bottom of the stack first); `folders`
/// runs top first, so index `n` is the crate's group id `n + 1`. `blends`
/// holds each layer's blend mode key, parallel to `layers`, because the crate
/// does not export its blend mode type.
struct RecordFlags {
    layers: Vec<bool>,
    blends: Vec<[u8; 4]>,
    folders: Vec<bool>,
}

/// `None` when the records do not parse.
fn record_flags(b: &[u8]) -> Option<RecordFlags> {
    let u16_at = |i: usize| Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize);
    let u32_at = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize);
    let mut p = 26;
    p += 4 + u32_at(p)?; // color mode data
    p += 4 + u32_at(p)?; // image resources
    p += 8; // layer and mask length, layer info length
    let count = (u16_at(p)? as u16 as i16).unsigned_abs();
    p += 2;
    let (mut layers, mut blends, mut folders) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..count {
        p += 16; // bounds
        p += 2 + u16_at(p)? * 6; // channel table
        let blend: [u8; 4] = b.get(p + 4..p + 8)?.try_into().ok()?;
        p += 10; // signature, blend mode, opacity, clipping
        let visible = b.get(p)? & 2 == 0;
        p += 2; // flags, filler
        let end = p + 4 + u32_at(p)?;
        p += 4;
        p += 4 + u32_at(p)?; // layer mask
        p += 4 + u32_at(p)?; // blending ranges
        p += (1 + *b.get(p)? as usize + 3) & !3; // name
        let mut divider = None;
        while p + 12 <= end && matches!(b.get(p..p + 4)?, b"8BIM" | b"8B64") {
            let len = u32_at(p + 8)?;
            if b.get(p + 4..p + 8)? == b"lsct" {
                divider = Some(u32_at(p + 12)?);
            }
            p += 12 + ((len + 1) & !1);
        }
        p = end;
        match divider {
            Some(1 | 2) => folders.push(visible),
            Some(3) => {}
            _ => {
                layers.push(visible);
                blends.push(blend);
            }
        }
    }
    folders.reverse();
    Some(RecordFlags { layers, blends, folders })
}

/// One file-safe, unique name per part (non-alphanumeric characters become
/// `_`, repeats get `_2`, `_3`, ...), in the same order.
pub fn file_names(parts: &[PsdPart]) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    parts
        .iter()
        .map(|p| {
            let base: String = p.name.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
            let base = if base.is_empty() { "part".to_string() } else { base };
            let mut name = base.clone();
            let mut n = 2;
            while !used.insert(name.clone()) {
                name = format!("{base}_{n}");
                n += 1;
            }
            name
        })
        .collect()
}

/// A layer's pixels inside its bounds and the canvas, premultiplied, with
/// layer opacity. `None` when no part of the layer is on the canvas.
fn layer_piece(layer: &psd::PsdLayer, w: u32, h: u32) -> Result<Option<Piece>, String> {
    let rgba = layer.rgba();
    if rgba.len() != (w * h * 4) as usize {
        return Err("PSD layer size does not match the canvas".into());
    }
    let (x0, y0) = (layer.layer_left().max(0), layer.layer_top().max(0));
    let x1 = layer.layer_left().saturating_add(layer.width() as i32).min(w as i32);
    let y1 = layer.layer_top().saturating_add(layer.height() as i32).min(h as i32);
    let Some(size) = IntSize::from_wh((x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32) else { return Ok(None) };
    let opacity = layer.opacity() as u16;
    let mut data = Vec::with_capacity(size.width() as usize * size.height() as usize * 4);
    for y in y0..y1 {
        let start = (y as usize * w as usize + x0 as usize) * 4;
        for p in rgba[start..start + size.width() as usize * 4].as_chunks::<4>().0 {
            let a = (p[3] as u16 * opacity / 255) as u8;
            let c = ColorU8::from_rgba(p[0], p[1], p[2], a).premultiply();
            data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
    }
    let image = Pixmap::from_vec(data, size).ok_or("PSD layer size does not match the canvas")?;
    Ok(Some(Piece { image, x: x0, y: y0 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const SIZE: u32 = 8;

    /// A test layer; records are listed bottom of the stack first, as stored.
    #[derive(Clone, Copy, Default)]
    struct Rec {
        name: &'static str,
        /// `[left, top, right, bottom]`, exclusive; empty for a divider.
        rect: [i32; 4],
        color: [u8; 4],
        clipped: bool,
        hidden: bool,
        /// Blend mode key; `None` is normal.
        blend: Option<&'static [u8; 4]>,
        /// Section divider type (1 opens a group, 3 closes it).
        lsct: Option<u32>,
    }

    fn layer(name: &'static str, rect: [i32; 4], color: [u8; 4]) -> Rec {
        Rec { name, rect, color, ..Default::default() }
    }

    fn folder(name: &'static str, hidden: bool) -> Rec {
        Rec { name, hidden, lsct: Some(1), ..Default::default() }
    }

    fn close() -> Rec {
        Rec { name: "</Layer group>", lsct: Some(3), ..Default::default() }
    }

    fn build(recs: &[Rec]) -> Vec<u8> {
        build_with(recs, &[])
    }

    /// The grid and guides resource for `(location in pixels, horizontal)`.
    fn guides(list: &[(i32, bool)]) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 1];
        data.extend([0; 8]);
        data.extend((list.len() as u32).to_be_bytes());
        for (at, horizontal) in list {
            data.extend((at * 32).to_be_bytes());
            data.push(*horizontal as u8);
        }
        let mut out = b"8BIM".to_vec();
        out.extend(1032u16.to_be_bytes());
        out.extend([0, 0]);
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(data);
        if out.len() % 2 == 1 {
            out.push(0); // resource data is padded to even
        }
        out
    }

    fn build_with(recs: &[Rec], resources: &[u8]) -> Vec<u8> {
        let (mut records, mut pixels) = (Vec::new(), Vec::new());
        for r in recs {
            let [l, t, right, bottom] = r.rect;
            let area = ((right - l) * (bottom - t)) as usize;
            for v in [t, l, bottom, right] {
                records.extend(v.to_be_bytes());
            }
            let channels: &[(i16, usize)] = if r.lsct.is_some() { &[] } else { &[(-1, 3), (0, 0), (1, 1), (2, 2)] };
            records.extend((channels.len() as u16).to_be_bytes());
            for (id, c) in channels {
                records.extend(id.to_be_bytes());
                records.extend((2 + area as u32).to_be_bytes());
                pixels.extend([0, 0]);
                pixels.extend(std::iter::repeat_n(r.color[*c], area));
            }
            records.extend(b"8BIM");
            records.extend(r.blend.unwrap_or(b"norm"));
            records.extend([255, r.clipped as u8, if r.hidden { 2 } else { 0 }, 0]);
            let mut extra = vec![0; 8]; // no mask, no blending ranges
            extra.push(r.name.len() as u8);
            extra.extend(r.name.as_bytes());
            extra.resize((extra.len() + 3) & !3, 0);
            if let Some(kind) = r.lsct {
                extra.extend(b"8BIMlsct");
                extra.extend(4u32.to_be_bytes());
                extra.extend(kind.to_be_bytes());
            }
            records.extend((extra.len() as u32).to_be_bytes());
            records.extend(extra);
        }
        let mut info = (recs.len() as u16).to_be_bytes().to_vec();
        info.extend(records);
        info.extend(pixels);
        if info.len() % 2 == 1 {
            info.push(0);
        }
        let mut f = b"8BPS".to_vec();
        f.extend([0, 1, 0, 0, 0, 0, 0, 0, 0, 4]);
        f.extend(SIZE.to_be_bytes());
        f.extend(SIZE.to_be_bytes());
        f.extend([0, 8, 0, 3]);
        f.extend([0; 4]); // no color mode data
        f.extend((resources.len() as u32).to_be_bytes());
        f.extend(resources);
        f.extend((info.len() as u32 + 8).to_be_bytes());
        f.extend((info.len() as u32).to_be_bytes());
        f.extend(info);
        f.extend(0u32.to_be_bytes()); // no global mask
        f.extend([0, 0]); // raw merged image
        f.extend(std::iter::repeat_n(0, (SIZE * SIZE * 4) as usize));
        f
    }

    fn pixel(p: &PsdPart, x: u32, y: u32) -> [u8; 4] {
        let c = p.image.pixel(x - p.x, y - p.y).unwrap();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn clipped_layer_stays_inside_its_base() {
        let bytes = build(&[layer("base", [2, 2, 4, 4], RED), Rec { clipped: true, ..layer("shade", [0, 0, 8, 3], BLUE) }, layer("top", [6, 6, 7, 7], RED)]);
        let (_, parts) = import(&bytes, &[], Split::Layers, Hidden::Skip).unwrap();
        assert_eq!(parts.len(), 2, "the clipped layer is not a part");
        let base = &parts[0];
        assert_eq!((base.x, base.y, base.image.width(), base.image.height(), base.layers), (2, 2, 2, 2, 2));
        assert_eq!(pixel(base, 2, 2), BLUE);
        assert_eq!(pixel(base, 2, 3), RED);
        assert_eq!(parts[1].name, "top");
    }

    #[test]
    fn clipped_multiply_shades_its_base() {
        const GRAY: [u8; 4] = [128, 128, 128, 255];
        let bytes = build(&[layer("base", [2, 2, 4, 4], RED), Rec { clipped: true, blend: Some(b"mul "), ..layer("shade", [0, 0, 8, 3], GRAY) }]);
        let (_, parts) = import(&bytes, &[], Split::Layers, Hidden::Skip).unwrap();
        assert_eq!((parts.len(), parts[0].image.width(), parts[0].image.height()), (1, 2, 2));
        assert_eq!(pixel(&parts[0], 2, 2), [128, 0, 0, 255], "shaded, not replaced");
        assert_eq!(pixel(&parts[0], 2, 3), RED, "outside the shade");
    }

    #[test]
    fn clipping_stays_in_order_inside_a_merged_group() {
        // Bottom to top: under, base, clipped, over.
        let bytes = build(&[close(), layer("under", [0, 0, 8, 8], BLUE), layer("base", [2, 2, 4, 4], RED), Rec { clipped: true, ..layer("shade", [0, 0, 8, 8], BLUE) }, layer("over", [3, 3, 4, 4], RED), folder("body", false)]);
        let (_, parts) = import(&bytes, &[], Split::Groups, Hidden::Skip).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].layers, 4);
        assert_eq!(pixel(&parts[0], 2, 2), BLUE, "the clipped layer covers its base");
        assert_eq!(pixel(&parts[0], 3, 3), RED, "a later layer still goes on top");
    }

    #[test]
    fn innermost_groups_become_parts_in_stacking_order() {
        // Bottom to top: back, inner { x, y }, front, all in outer; then alone.
        let bytes = build(&[
            close(),
            layer("back", [0, 0, 2, 2], RED),
            close(),
            layer("x", [2, 2, 4, 4], RED),
            layer("y", [3, 3, 5, 5], BLUE),
            folder("inner", false),
            layer("front", [6, 6, 8, 8], RED),
            folder("outer", false),
            layer("alone", [0, 6, 1, 7], BLUE),
        ]);
        let (_, parts) = import(&bytes, &[], Split::Innermost, Hidden::Skip).unwrap();
        let got: Vec<(&str, usize)> = parts.iter().map(|p| (p.name.as_str(), p.layers)).collect();
        assert_eq!(got, [("outer", 1), ("inner", 2), ("outer", 1), ("alone", 1)]);
        assert_eq!(pixel(&parts[1], 3, 3), BLUE, "layers of a group merge");
        let (_, top) = import(&bytes, &[], Split::Groups, Hidden::Skip).unwrap();
        assert_eq!(top.len(), 2, "top-level groups still merge everything inside");
    }

    #[test]
    fn keeping_clipping_makes_a_part_that_names_its_base() {
        let bytes = build(&[
            layer("base", [2, 2, 4, 4], RED),
            Rec { clipped: true, blend: Some(b"mul "), ..layer("shade", [0, 0, 8, 3], BLUE) },
            Rec { clipped: true, ..layer("empty_spot", [6, 6, 8, 8], BLUE) },
            layer("top", [6, 6, 7, 7], RED),
        ]);
        let (_, parts) = import(&bytes, &[], Split::LayersKeepClipping, Hidden::Skip).unwrap();
        let got: Vec<(&str, Option<usize>)> = parts.iter().map(|p| (p.name.as_str(), p.clip_to)).collect();
        assert_eq!(got, [("base", None), ("shade", Some(0)), ("empty_spot", Some(0)), ("top", None)]);
        assert_eq!(parts[1].blend, skel2d_core::BlendMode::Multiply);
        assert_eq!((parts[1].image.width(), parts[1].image.height()), (8, 3), "its pixels stay whole, so it can slide under the base");
        assert_eq!(pixel(&parts[0], 2, 2), RED, "the base is not painted on");
    }

    #[test]
    fn the_origin_comes_from_a_tagged_layer_then_guides_then_the_canvas() {
        let art = layer("art", [0, 0, 2, 2], RED);
        let at = |o: Origin| (o.x, o.y, o.source);
        assert_eq!(at(origin(&build(&[art])).unwrap()), (4.0, 4.0, OriginSource::Canvas));
        let guided = build_with(&[art], &guides(&[(2, false), (6, false), (5, true), (1, true)]));
        assert_eq!(at(origin(&guided).unwrap()), (2.0, 5.0, OriginSource::Guides), "the first guide of each direction");
        let one = build_with(&[art], &guides(&[(3, true)]));
        assert_eq!(at(origin(&one).unwrap()), (4.0, 3.0, OriginSource::Guides), "a missing direction stays centered");
        let tagged = Rec { hidden: true, ..layer("root [origin]", [4, 6, 6, 8], BLUE) };
        let both = build_with(&[art, tagged], &guides(&[(2, false), (5, true)]));
        assert_eq!(at(origin(&both).unwrap()), (5.0, 7.0, OriginSource::Layer), "the layer wins, hidden or not");
        for hidden in [Hidden::Skip, Hidden::Include] {
            let (_, parts) = import(&both, &[], Split::Layers, hidden).unwrap();
            assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["art"], "the mark is not art");
        }
    }

    #[test]
    fn hidden_modes() {
        let bytes = build(&[layer("shown", [0, 0, 2, 2], RED), Rec { hidden: true, ..layer("off", [4, 4, 6, 6], BLUE) }]);
        let names = |mode| import(&bytes, &[], Split::Layers, mode).unwrap().1.iter().map(|p| (p.name.clone(), p.hidden)).collect::<Vec<_>>();
        assert_eq!(names(Hidden::Skip), [("shown".to_string(), false)]);
        assert_eq!(names(Hidden::Transparent), [("shown".to_string(), false), ("off".to_string(), true)]);
        assert_eq!(names(Hidden::Include), [("shown".to_string(), false), ("off".to_string(), false)]);
    }

    #[test]
    fn hidden_group_hides_its_layers_and_what_clips_to_a_hidden_base() {
        let bytes = build(&[
            close(),
            layer("in_off", [0, 0, 2, 2], RED),
            folder("off", true),
            close(),
            layer("in_on", [2, 2, 4, 4], RED),
            folder("on", false),
            Rec { hidden: true, ..layer("base", [4, 4, 6, 6], RED) },
            Rec { clipped: true, ..layer("clip", [4, 4, 6, 6], BLUE) },
        ]);
        let names = |mode, split| import(&bytes, &[], split, mode).unwrap().1.iter().map(|p| (p.name.clone(), p.hidden)).collect::<Vec<_>>();
        assert_eq!(names(Hidden::Skip, Split::Groups), [("on".to_string(), false)]);
        assert_eq!(names(Hidden::Transparent, Split::Groups), [("off".to_string(), true), ("on".to_string(), false), ("base".to_string(), true)]);
    }

    #[test]
    fn transparent_group_drops_hidden_layers_when_the_group_shows() {
        let bytes = build(&[close(), layer("seen", [0, 0, 2, 2], RED), Rec { hidden: true, ..layer("unseen", [6, 6, 8, 8], BLUE) }, folder("body", false)]);
        let (_, parts) = import(&bytes, &[], Split::Groups, Hidden::Transparent).unwrap();
        assert_eq!((parts.len(), parts[0].hidden, parts[0].layers), (1, false, 1));
        assert_eq!((parts[0].image.width(), parts[0].image.height()), (2, 2));
    }

    #[test]
    fn layers_split_keeps_every_layer() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/miya/miya_a_idle02_field.psd")).unwrap();
        let (_, groups) = import(&bytes, &[], Split::Groups, Hidden::Include).unwrap();
        let (_, layers) = import(&bytes, &[], Split::Layers, Hidden::Include).unwrap();
        assert!(layers.len() > groups.len());
    }

    #[test]
    fn real_file_flags_parse() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/miya/miya_a_idle02_field.psd")).unwrap();
        let psd = psd::Psd::from_bytes(&bytes).unwrap();
        let flags = record_flags(&bytes).unwrap();
        assert_eq!(flags.layers.len(), psd.layers().len());
        assert_eq!(flags.folders.len(), psd.groups().len());
    }
}
