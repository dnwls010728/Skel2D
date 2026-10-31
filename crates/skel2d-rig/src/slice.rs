//! Cutting a parts sheet (several drawings on one transparent image) into
//! separate images. Pieces closer than `merge` pixels count as one part, so
//! a pair of eyes stays together.

use schemars::JsonSchema;
use serde::Serialize;
use tiny_skia::{IntRect, Pixmap};

use crate::ALPHA_THRESHOLD;
use crate::mask::{Mask, PixelRect};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Piece {
    /// 1-based, in reading order (rows top to bottom, then left to right).
    pub index: usize,
    /// Where the piece was on the sheet (before padding).
    pub bbox: PixelRect,
    #[serde(skip)]
    pub image: Option<Pixmap>,
}

pub fn slice(sheet: &Pixmap, merge: usize, padding: u32, min_pixels: usize) -> Vec<Piece> {
    let mask = Mask::from_pixmap(sheet, ALPHA_THRESHOLD);
    let grown = dilate(&mask, merge / 2);
    let mut boxes: Vec<PixelRect> = grown
        .components()
        .into_iter()
        .filter_map(|c| {
            // Shrink back to the real pixels inside the grown island.
            let b = c.bbox;
            let mut tight: Option<PixelRect> = None;
            let mut count = 0;
            for y in b.y0..b.y1 {
                for x in b.x0..b.x1 {
                    if mask.get(x, y) {
                        count += 1;
                        let t = tight.get_or_insert(PixelRect { x0: x, y0: y, x1: x + 1, y1: y + 1 });
                        t.x0 = t.x0.min(x);
                        t.y0 = t.y0.min(y);
                        t.x1 = t.x1.max(x + 1);
                        t.y1 = t.y1.max(y + 1);
                    }
                }
            }
            (count >= min_pixels).then_some(tight).flatten()
        })
        .collect();

    // Reading order: a piece starts a new row when it lies below the
    // current row's vertical middle.
    boxes.sort_by_key(|b| b.y0);
    let mut rows: Vec<Vec<PixelRect>> = Vec::new();
    for b in boxes {
        match rows.last_mut() {
            Some(row) if (b.y0 + b.y1) / 2 < row.iter().map(|r| r.y1).max().unwrap_or(0) => row.push(b),
            _ => rows.push(vec![b]),
        }
    }
    rows.iter_mut().for_each(|r| r.sort_by_key(|b| b.x0));

    rows.into_iter()
        .flatten()
        .enumerate()
        .map(|(i, b)| {
            let (x0, y0) = (b.x0.saturating_sub(padding as usize), b.y0.saturating_sub(padding as usize));
            let x1 = (b.x1 + padding as usize).min(sheet.width() as usize);
            let y1 = (b.y1 + padding as usize).min(sheet.height() as usize);
            let image = IntRect::from_xywh(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32).and_then(|r| sheet.clone_rect(r));
            Piece { index: i + 1, bbox: b, image }
        })
        .collect()
}

/// Removes generator noise: pixels with alpha below `min_alpha` become fully
/// transparent, then 8-connected specks smaller than `min_pixels` are erased.
/// AI-generated "transparent" images often carry faint colored halos and
/// stray dots that would otherwise end up inside crops and meshes.
pub fn clean(img: &mut Pixmap, min_alpha: u8, min_pixels: usize) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data = img.data_mut();
    for px in data.as_chunks_mut::<4>().0 {
        if px[3] < min_alpha {
            px.fill(0);
        }
    }
    if min_pixels == 0 {
        return;
    }
    let solid = |data: &[u8], i: usize| data[i * 4 + 3] > 0;
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut piece = Vec::new();
    for start in 0..w * h {
        if seen[start] || !solid(data, start) {
            continue;
        }
        piece.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            piece.push(i);
            for (nx, ny) in crate::mask::neighbors8(i % w, i / w, w, h) {
                let j = ny * w + nx;
                if !seen[j] && solid(data, j) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if piece.len() < min_pixels {
            for &i in &piece {
                data[i * 4..i * 4 + 4].fill(0);
            }
        }
    }
}

/// Chroma key: pixels whose color is within `tolerance` (max channel
/// difference, 0..=255) of `key` become transparent; pixels within twice the
/// tolerance get partial alpha, and the key color is removed from edge pixels
/// (despill) so outlines don't keep a colored fringe. Image generators draw
/// flat key-color backgrounds far more reliably than real transparency.
pub fn chroma_key(img: &mut Pixmap, key: [u8; 3], tolerance: u8) {
    let tol = tolerance.max(1) as f32;
    let dominant = (0..3).max_by_key(|&i| key[i]).unwrap_or(1);
    for px in img.data_mut().as_chunks_mut::<4>().0 {
        let a = px[3] as f32 / 255.0;
        if a == 0.0 {
            continue;
        }
        // Back to straight color for the comparison.
        let c = [0, 1, 2].map(|i| (px[i] as f32 / a).min(255.0));
        let d = (0..3).map(|i| (c[i] - key[i] as f32).abs()).fold(0.0, f32::max);
        let keep = ((d - tol) / tol).clamp(0.0, 1.0);
        if keep == 0.0 {
            px.fill(0);
            continue;
        }
        let mut c = c;
        let others = (0..3).filter(|&i| i != dominant).map(|i| c[i]).fold(0.0, f32::max);
        if c[dominant] > others {
            c[dominant] = others;
        }
        let na = a * keep;
        for i in 0..3 {
            px[i] = (c[i] * na).round().min(255.0) as u8;
        }
        px[3] = (na * 255.0).round() as u8;
    }
}

/// Copies `[x0, y0, x1, y1]` (clamped to the image) into a new image.
pub fn crop(img: &Pixmap, [x0, y0, x1, y1]: [u32; 4]) -> Option<Pixmap> {
    let (x1, y1) = (x1.min(img.width()), y1.min(img.height()));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    img.clone_rect(IntRect::from_xywh(x0 as i32, y0 as i32, x1 - x0, y1 - y0)?)
}

/// Square dilation by `r` pixels (separable max filter).
fn dilate(mask: &Mask, r: usize) -> Mask {
    if r == 0 {
        return mask.clone();
    }
    let (w, h) = (mask.width, mask.height);
    let horizontal = Mask::from_fn(w, h, |x, y| mask.any_in(x.saturating_sub(r), y, x + r + 1, y + 1));
    Mask::from_fn(w, h, |x, y| horizontal.any_in(x, y.saturating_sub(r), x + 1, y + r + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_merge_and_order_is_by_rows() {
        let mut sheet = Pixmap::new(400, 200).unwrap();
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(0, 0, 0, 255);
        // Row 1: an "eye pair" (two dots 20px apart) and a single dot far right.
        // Row 2: one wide bar.
        for (x, y, w, h) in [(20.0, 20.0, 20.0, 20.0), (60.0, 22.0, 20.0, 20.0), (300.0, 25.0, 20.0, 20.0), (50.0, 120.0, 200.0, 30.0)] {
            sheet.fill_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap(), &p, tiny_skia::Transform::identity(), None);
        }
        let pieces = slice(&sheet, 30, 2, 10);
        assert_eq!(pieces.len(), 3);
        assert_eq!((pieces[0].bbox.x0, pieces[0].bbox.x1), (20, 80));
        assert_eq!(pieces[1].bbox.x0, 300);
        assert_eq!(pieces[2].bbox.y0, 120);
        let img = pieces[0].image.as_ref().unwrap();
        assert_eq!((img.width(), img.height()), (64, 26));
    }
}

#[cfg(test)]
mod clean_tests {
    use super::*;

    #[test]
    fn clean_drops_faint_halo_and_specks() {
        let mut img = Pixmap::new(50, 50).unwrap();
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(0, 0, 0, 255);
        img.fill_rect(tiny_skia::Rect::from_xywh(10.0, 10.0, 20.0, 20.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        img.fill_rect(tiny_skia::Rect::from_xywh(40.0, 40.0, 2.0, 2.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        p.set_color_rgba8(255, 0, 0, 10);
        img.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, 5.0, 50.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        clean(&mut img, 32, 20);
        assert_eq!(img.pixel(2, 25).unwrap().alpha(), 0, "halo");
        assert_eq!(img.pixel(40, 40).unwrap().alpha(), 0, "speck");
        assert_eq!(img.pixel(20, 20).unwrap().alpha(), 255, "real pixel");
        assert_eq!(crop(&img, [10, 10, 30, 30]).unwrap().width(), 20);
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn green_background_is_removed_and_despilled() {
        let mut img = Pixmap::new(20, 10).unwrap();
        img.fill(tiny_skia::Color::from_rgba8(0, 255, 0, 255));
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(200, 150, 120, 255);
        img.fill_rect(tiny_skia::Rect::from_xywh(5.0, 2.0, 10.0, 6.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        p.set_color_rgba8(120, 230, 100, 255); // greenish edge pixel
        img.fill_rect(tiny_skia::Rect::from_xywh(4.0, 2.0, 1.0, 6.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        chroma_key(&mut img, [0, 255, 0], 60);
        assert_eq!(img.pixel(0, 0).unwrap().alpha(), 0);
        let body = img.pixel(10, 5).unwrap();
        assert_eq!((body.alpha(), body.red()), (255, 200));
        let edge = img.pixel(4, 5).unwrap();
        assert!(edge.green() <= edge.red(), "despill {edge:?}");
    }
}
