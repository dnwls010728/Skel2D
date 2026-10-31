use schemars::JsonSchema;
use serde::Serialize;
use tiny_skia::Pixmap;

/// Binary coverage of an image: a pixel is solid when its alpha exceeds the
/// threshold.
#[derive(Debug, Clone)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    solid: Vec<bool>,
}

/// Pixel-space rectangle, inclusive min, exclusive max.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl PixelRect {
    pub fn width(&self) -> usize {
        self.x1 - self.x0
    }

    pub fn height(&self) -> usize {
        self.y1 - self.y0
    }
}

impl Mask {
    pub fn from_pixmap(img: &Pixmap, threshold: u8) -> Mask {
        let solid = img.pixels().iter().map(|p| p.alpha() > threshold).collect();
        Mask { width: img.width() as usize, height: img.height() as usize, solid }
    }

    pub fn from_fn(width: usize, height: usize, f: impl Fn(usize, usize) -> bool) -> Mask {
        let solid = (0..width * height).map(|i| f(i % width, i / width)).collect();
        Mask { width, height, solid }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.solid[y * self.width + x]
    }

    pub fn count(&self) -> usize {
        self.solid.iter().filter(|&&s| s).count()
    }

    /// Whether any solid pixel lies in the rectangle (clamped to the image).
    pub fn any_in(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> bool {
        (y0..y1.min(self.height)).any(|y| (x0..x1.min(self.width)).any(|x| self.solid[y * self.width + x]))
    }

    pub fn bbox(&self) -> Option<PixelRect> {
        let mut r: Option<PixelRect> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.solid[y * self.width + x] {
                    let b = r.get_or_insert(PixelRect { x0: x, y0: y, x1: x + 1, y1: y + 1 });
                    b.x0 = b.x0.min(x);
                    b.y0 = b.y0.min(y);
                    b.x1 = b.x1.max(x + 1);
                    b.y1 = b.y1.max(y + 1);
                }
            }
        }
        r
    }

    /// 8-connected solid regions, largest first.
    pub fn components(&self) -> Vec<Component> {
        let mut label = vec![u32::MAX; self.solid.len()];
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for start in 0..self.solid.len() {
            if !self.solid[start] || label[start] != u32::MAX {
                continue;
            }
            let id = out.len() as u32;
            let (sx, sy) = (start % self.width, start / self.width);
            let mut c = Component { pixels: 0, bbox: PixelRect { x0: sx, y0: sy, x1: sx + 1, y1: sy + 1 } };
            label[start] = id;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % self.width, i / self.width);
                c.pixels += 1;
                c.bbox.x0 = c.bbox.x0.min(x);
                c.bbox.y0 = c.bbox.y0.min(y);
                c.bbox.x1 = c.bbox.x1.max(x + 1);
                c.bbox.y1 = c.bbox.y1.max(y + 1);
                for (nx, ny) in neighbors8(x, y, self.width, self.height) {
                    let j = ny * self.width + nx;
                    if self.solid[j] && label[j] == u32::MAX {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
            out.push(c);
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.pixels));
        out
    }

    /// Nearest-neighbour downscale so long sides are at most `max_side`.
    /// Returns the mask and the scale factor from new to original pixels.
    pub fn downscaled(&self, max_side: usize) -> (Mask, f32) {
        let side = self.width.max(self.height);
        if side <= max_side {
            return (self.clone(), 1.0);
        }
        let f = side as f32 / max_side as f32;
        let (w, h) = (((self.width as f32 / f).ceil() as usize).max(1), ((self.height as f32 / f).ceil() as usize).max(1));
        // A downscaled pixel is solid if any source pixel it covers is solid,
        // so thin limbs survive.
        let m = Mask::from_fn(w, h, |x, y| {
            let (x0, y0) = ((x as f32 * f) as usize, (y as f32 * f) as usize);
            let (x1, y1) = (((x + 1) as f32 * f).ceil() as usize, ((y + 1) as f32 * f).ceil() as usize);
            self.any_in(x0, y0, x1, y1)
        });
        (m, f)
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Component {
    pub pixels: usize,
    pub bbox: PixelRect,
}

pub(crate) fn neighbors8(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    (-1i64..=1)
        .flat_map(|dy| (-1i64..=1).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| dx != 0 || dy != 0)
        .filter_map(move |(dx, dy)| {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            (nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h).then_some((nx as usize, ny as usize))
        })
}
