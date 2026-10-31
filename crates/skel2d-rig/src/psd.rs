//! Photoshop import: a layered PSD becomes one image per part plus the
//! placement a `layers.json` manifest needs. With [`Split::Groups`] every
//! top-level group is one part (its layers composited, e.g. line art over
//! flat color) and a layer outside any group is a part of its own; with
//! [`Split::Layers`] every layer is a part and groups only organize them
//! (the Spine-style layout). Hidden layers are included, since artists
//! often hide the parts while looking at a flattened reference. A paper
//! layer (Clip Studio's `용지` / `Paper` / `用紙` background) is never a part.

use std::collections::HashMap;

use tiny_skia::{ColorU8, IntSize, Pixmap, PixmapPaint, Transform};

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
}

/// What becomes one part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Split {
    /// Each top-level group (composited) or ungrouped layer.
    #[default]
    Groups,
    /// Each layer; groups only organize them.
    Layers,
}

/// Layer names painting programs give the paper background.
const PAPER: [&str; 3] = ["용지", "Paper", "用紙"];

/// Canvas size and parts back to front. A layer is skipped when its name
/// or the name of any group it is in is in `exclude`, e.g. a flattened
/// reference.
pub fn import(bytes: &[u8], exclude: &[String], split: Split) -> Result<([u32; 2], Vec<PsdPart>), String> {
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

    // `layers()` lists the top of the stack first.
    let mut order: Vec<(String, Pixmap, usize)> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for layer in psd.layers().iter().rev() {
        if exclude.iter().any(|x| x == layer.name()) || ancestors(layer.parent_id()).any(|g| exclude.contains(&group_name(g))) {
            continue;
        }
        if layer.parent_id().is_none() && PAPER.contains(&layer.name()) {
            continue;
        }
        let group = match split {
            Split::Groups => ancestors(layer.parent_id()).last(),
            Split::Layers => None,
        };
        let name = group.map_or_else(|| layer.name().to_string(), group_name);
        let src = layer_pixmap(&layer.rgba(), w, h, layer.opacity())?;
        let slot = match group.and_then(|g| index.get(&g)) {
            Some(&i) => i,
            None => {
                order.push((name, Pixmap::new(w, h).expect("canvas checked by layer_pixmap"), 0));
                if let Some(g) = group {
                    index.insert(g, order.len() - 1);
                }
                order.len() - 1
            }
        };
        let (_, dst, count) = &mut order[slot];
        dst.draw_pixmap(0, 0, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        *count += 1;
    }

    let parts = order
        .into_iter()
        .filter_map(|(name, img, layers)| {
            let b = Mask::from_pixmap(&img, ALPHA_THRESHOLD).bbox()?;
            let rect = tiny_skia::IntRect::from_xywh(b.x0 as i32, b.y0 as i32, b.width() as u32, b.height() as u32)?;
            Some(PsdPart { name, image: img.clone_rect(rect)?, x: b.x0 as u32, y: b.y0 as u32, layers })
        })
        .collect();
    Ok(([w, h], parts))
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

/// Straight RGBA (full canvas) to a premultiplied pixmap, with layer opacity.
fn layer_pixmap(rgba: &[u8], w: u32, h: u32, opacity: u8) -> Result<Pixmap, String> {
    let size = IntSize::from_wh(w, h).ok_or("PSD canvas is empty")?;
    if rgba.len() != (w * h * 4) as usize {
        return Err("PSD layer size does not match the canvas".into());
    }
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = (p[3] as u16 * opacity as u16 / 255) as u8;
            let c = ColorU8::from_rgba(p[0], p[1], p[2], a).premultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Pixmap::from_vec(data, size).ok_or_else(|| "PSD layer size does not match the canvas".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_split_keeps_every_layer() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/miya/miya_a_idle02_field.psd")).unwrap();
        let (_, groups) = import(&bytes, &[], Split::Groups).unwrap();
        let (_, layers) = import(&bytes, &[], Split::Layers).unwrap();
        assert!(layers.len() > groups.len());
        assert!(layers.iter().all(|p| p.layers == 1));
    }
}
