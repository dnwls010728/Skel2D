//! Coordinate grid overlay. Vision models place points far more accurately
//! when the image carries labeled pixel coordinates, so agents look at this
//! before writing joint positions.

use tiny_skia::{Color, Paint, Pixmap, PixmapPaint, Rect, Transform};

/// 3x5 bitmap digits, one row per u8 (low 3 bits, MSB = left).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Draws `img` over a checker background with grid lines every `step`
/// pixels, labeled with their pixel coordinate along the top and left edges
/// (and every fourth line inside the image).
pub fn overlay(img: &Pixmap, step: u32, points: &[[f32; 2]]) -> Pixmap {
    let step = step.max(10);
    let (w, h) = (img.width(), img.height());
    let mut out = Pixmap::new(w, h).expect("image is non-empty");
    for y in (0..h).step_by(16) {
        for x in (0..w).step_by(16) {
            let shade = if (x / 16 + y / 16) % 2 == 0 { 70 } else { 90 };
            fill(&mut out, x as f32, y as f32, 16.0, 16.0, [shade, shade, shade, 255]);
        }
    }
    out.draw_pixmap(0, 0, img.as_ref(), &PixmapPaint::default(), Transform::identity(), None);

    let scale = ((w.max(h) as f32 / 600.0).round() as u32).clamp(2, 6);
    for (i, x) in (0..w).step_by(step as usize).enumerate() {
        let major = i % 2 == 0;
        fill(&mut out, x as f32, 0.0, 1.0, h as f32, if major { [0, 200, 255, 150] } else { [0, 200, 255, 70] });
        if major {
            label(&mut out, x, x as f32 + 3.0, 3.0, scale);
            if i % 4 == 0 && i > 0 {
                for y in (0..h).step_by(step as usize * 4).skip(1) {
                    label(&mut out, x, x as f32 + 3.0, y as f32 + 3.0, scale);
                }
            }
        }
    }
    for (i, y) in (0..h).step_by(step as usize).enumerate() {
        let major = i % 2 == 0;
        fill(&mut out, 0.0, y as f32, w as f32, 1.0, if major { [0, 200, 255, 150] } else { [0, 200, 255, 70] });
        if major && i > 0 {
            label(&mut out, y, 3.0, y as f32 + 3.0, scale);
        }
    }
    for &[x, y] in points {
        fill(&mut out, x - 4.0, y - 1.0, 9.0, 3.0, [255, 40, 160, 255]);
        fill(&mut out, x - 1.0, y - 4.0, 3.0, 9.0, [255, 40, 160, 255]);
    }
    out
}

fn fill(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, [r, g, b, a]: [u8; 4]) {
    let (x0, y0) = (x.round().max(0.0), y.round().max(0.0));
    let x1 = (x + w).round().min(pm.width() as f32);
    let y1 = (y + h).round().min(pm.height() as f32);
    if let Some(rect) = Rect::from_ltrb(x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0)) {
        let mut p = Paint::default();
        p.set_color(Color::from_rgba8(r, g, b, a));
        p.anti_alias = false;
        pm.fill_rect(rect, &p, Transform::identity(), None);
    }
}

/// Writes a number with a dark backing box so it reads on any image.
fn label(pm: &mut Pixmap, value: u32, x: f32, y: f32, scale: u32) {
    let text = value.to_string();
    let s = scale as f32;
    let width = text.len() as f32 * 4.0 * s;
    fill(pm, x - s, y - s, width + s, 7.0 * s, [0, 0, 0, 190]);
    for (i, ch) in text.bytes().enumerate() {
        let glyph = DIGITS[(ch - b'0') as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    let px = x + (i as f32 * 4.0 + col as f32) * s;
                    fill(pm, px, y + row as f32 * s, s, s, [255, 255, 255, 255]);
                }
            }
        }
    }
}
