use std::collections::{HashMap, HashSet};

use skel2d_core::{Mesh, MeshVertex};
use spade::{
    AngleLimit, ConstrainedDelaunayTriangulation, Point2, RefinementParameters, Triangulation,
};

use crate::mask::Mask;
use crate::weights::segment_distance;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Interior {
    None,
    Grid,
    #[default]
    Auto,
}

/// Maps image pixels (y-down) to setup-pose world space (y-up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Pixel that lands on the world origin.
    pub origin: [f32; 2],
    /// World units per pixel.
    pub scale: f32,
}

impl Placement {
    pub fn to_world(&self, [px, py]: [f32; 2]) -> [f32; 2] {
        [
            round2((px - self.origin[0]) * self.scale),
            round2((self.origin[1] - py) * self.scale),
        ]
    }

    pub fn to_pixel(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            x / self.scale + self.origin[0],
            self.origin[1] - y / self.scale,
        ]
    }
}

/// A regular grid mesh covering every `cell`-sized square that touches the
/// mask. Grids deform evenly and never produce degenerate triangles, but
/// spend many vertices on flat interiors and staircase the outline; see
/// [`outline_mesh`] for the adaptive alternative.
pub fn grid_mesh(
    mask: &Mask,
    name: &str,
    image: Option<String>,
    cell: usize,
    placement: Placement,
) -> Mesh {
    let cell = cell.max(2);
    let (w, h) = (mask.width, mask.height);
    let (cols, rows) = (w.div_ceil(cell), h.div_ceil(cell));
    let mut index: HashMap<(usize, usize), u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    let mut vertex = |i: usize, j: usize, vertices: &mut Vec<MeshVertex>| -> u32 {
        *index.entry((i, j)).or_insert_with(|| {
            let (px, py) = ((i * cell).min(w) as f32, (j * cell).min(h) as f32);
            vertices.push(make_vertex(px, py, w, h, placement));
            (vertices.len() - 1) as u32
        })
    };
    for j in 0..rows {
        for i in 0..cols {
            // One pixel of margin keeps antialiased edge pixels inside the mesh.
            let (x0, y0) = ((i * cell).saturating_sub(1), (j * cell).saturating_sub(1));
            if !mask.any_in(x0, y0, (i + 1) * cell + 1, (j + 1) * cell + 1) {
                continue;
            }
            let a = vertex(i, j, &mut vertices);
            let b = vertex(i + 1, j, &mut vertices);
            let c = vertex(i + 1, j + 1, &mut vertices);
            let d = vertex(i, j + 1, &mut vertices);
            // Alternate diagonals so the grid bends the same way in both directions.
            if (i + j) % 2 == 0 {
                triangles.extend([[a, d, b], [b, d, c]]);
            } else {
                triangles.extend([[a, d, c], [a, c, b]]);
            }
        }
    }
    Mesh {
        bones: Vec::new(),
        hull: 0,
        edges: Vec::new(),
        name: name.to_string(),
        image,
        vertices,
        triangles,
    }
}

/// Density controls for [`outline_mesh`], in image pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineParams {
    /// Target spacing of interior vertices (about the edge length of
    /// interior triangles).
    pub spacing: f32,
    /// How far the simplified outline may stray from the traced one.
    pub tolerance: f32,
}

/// How many vertices an automatic mesh gets, relative to the image size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    Low,
    #[default]
    Medium,
    High,
}

impl OutlineParams {
    /// Spacing scaled to the image: medium gives roughly what riggers place
    /// by hand (10-30 vertices for a limb part), whatever the resolution.
    pub fn auto(width: usize, height: usize, density: Density) -> Self {
        let per_side = match density {
            Density::Low => 4.5,
            Density::Medium => 7.0,
            Density::High => 11.0,
        };
        Self::from_spacing((width.max(height) as f32 / per_side).max(4.0))
    }

    /// Spacing `spacing`, with an outline tolerance that grows with it.
    pub fn from_spacing(spacing: f32) -> Self {
        let spacing = spacing.max(2.0);
        Self {
            spacing,
            tolerance: (spacing / 8.0).clamp(1.0, 4.0),
        }
    }
}

/// An adaptive mesh: the mask outline (holes included), simplified to
/// `tolerance`, then filled by a quality Delaunay refinement so interiors
/// get only as many vertices as `spacing` asks for. Every `joints` pixel
/// inside the mask becomes a vertex with a small ring around it, so limbs
/// have the resolution to bend there. Falls back to [`grid_mesh`] if the
/// outline cannot be triangulated.
pub fn outline_mesh(
    mask: &Mask,
    name: &str,
    image: Option<String>,
    params: OutlineParams,
    placement: Placement,
    joints: &[[f32; 2]],
) -> Mesh {
    outline_mesh_impl(
        mask,
        name,
        image.clone(),
        params,
        placement,
        joints,
        None,
        Interior::Auto,
    )
    .unwrap_or_else(|_| {
        grid_mesh(
            mask,
            name,
            image,
            params.spacing.round() as usize,
            placement,
        )
    })
}

/// Explicit trace controls leave legacy auto-rig defaults unchanged. Padding
/// uses whole source-image pixels; interior modes share the traced outline.
#[allow(clippy::too_many_arguments)]
pub fn trace_outline_mesh(
    mask: &Mask,
    name: &str,
    image: Option<String>,
    params: OutlineParams,
    placement: Placement,
    joints: &[[f32; 2]],
    padding: f32,
    interior: Interior,
) -> Result<Mesh, String> {
    outline_mesh_impl(
        mask,
        name,
        image,
        params,
        placement,
        joints,
        Some(padding),
        interior,
    )
}

#[allow(clippy::too_many_arguments)]
fn outline_mesh_impl(
    mask: &Mask,
    name: &str,
    image: Option<String>,
    params: OutlineParams,
    placement: Placement,
    joints: &[[f32; 2]],
    padding: Option<f32>,
    interior: Interior,
) -> Result<Mesh, String> {
    let (w, h) = (mask.width, mask.height);
    let spacing = params.spacing.max(2.0);
    let tolerance = params.tolerance.max(0.0);
    // Simplification may cut up to `tolerance` into the shape, so grow it
    // first; the extra pixel keeps antialiased edges inside, as in the grid.
    let grown = without_pinches(&dilate(
        mask,
        padding.map_or(tolerance.ceil() as usize + 1, |v| v.floor() as usize),
    ));
    let loops = trace_loops(&grown);
    if loops.is_empty() {
        return Err("image has no opaque outline".into());
    }
    let mut tol = tolerance;
    let (mut cdt, polys) = loop {
        let polys: Vec<Vec<[f32; 2]>> = loops
            .iter()
            .map(|l| simplify_closed(l, tol))
            .filter(|p| p.len() >= 3)
            .collect();
        if padding.is_some() && polys.iter().map(Vec::len).sum::<usize>() > 50_000 {
            return Err("outline exceeds 50000 vertices; lower detail".into());
        }
        if let Some(cdt) = build_cdt(&polys) {
            break (cdt, polys);
        }
        // Simplified outlines of parts closer than the tolerance can cross;
        // the unsimplified ones never do.
        if tol == 0.0 {
            return Err("outline cannot be triangulated".into());
        }
        tol = if tol < 0.5 { 0.0 } else { tol / 2.0 };
    };

    let ring = spacing * 0.35;
    for &[jx, jy] in joints.iter().filter(|_| interior == Interior::Auto) {
        if !mask.get(jx.max(0.0) as usize, jy.max(0.0) as usize) {
            continue;
        }
        let ring_points = (0..6).map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 6.0;
            [jx + ring * a.cos(), jy + ring * a.sin()]
        });
        for [x, y] in std::iter::once([jx, jy]).chain(ring_points) {
            if mask.get(x.max(0.0) as usize, y.max(0.0) as usize) {
                // A point that cannot be inserted is only a missed hint.
                let _ = cdt.insert(spade_point(x, y));
            }
        }
    }

    if interior == Interior::Grid {
        let candidates =
            (w as f32 / spacing).ceil() as usize * (h as f32 / spacing).ceil() as usize;
        if candidates > 50_000 {
            return Err("grid exceeds 50000 candidate vertices; increase spacing".into());
        }
        for y in 1..(h as f32 / spacing).ceil() as usize {
            for x in 1..(w as f32 / spacing).ceil() as usize {
                let p = [x as f32 * spacing, y as f32 * spacing];
                if point_in_loops(&polys, p) {
                    cdt.insert(spade_point(p[0], p[1]))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    let max_area = f64::from(spacing * spacing) * 1.5;
    let expected = (mask.count() as f64 / max_area) as usize * 4 + cdt.num_vertices() * 2 + 256;
    let expected = if interior != Interior::Auto {
        0
    } else if padding.is_some() {
        expected.min(50_000usize.saturating_sub(cdt.num_vertices()))
    } else {
        expected
    };
    let result = cdt.refine(
        RefinementParameters::new()
            .exclude_outer_faces(true)
            .with_angle_limit(AngleLimit::from_deg(25.0))
            .with_max_allowed_area(max_area)
            .with_min_required_area(f64::from(tolerance.max(1.0)).powi(2))
            .with_max_additional_vertices(expected),
    );
    let outside: HashSet<_> = result.excluded_faces.into_iter().collect();

    let mut index: HashMap<usize, u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for face in cdt.inner_faces() {
        if outside.contains(&face.fix()) {
            continue;
        }
        let positions = face.vertices().map(|v| v.position());
        let center = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            (-(positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
        if !point_in_loops(&polys, center) {
            continue;
        }
        // Spade sees y flipped (world-up), so its counter-clockwise faces
        // are counter-clockwise in world space like the grid's.
        let tri = face.vertices().map(|v| {
            *index.entry(v.fix().index()).or_insert_with(|| {
                let p = v.position();
                let (px, py) = (
                    (p.x as f32).clamp(0.0, w as f32),
                    (-p.y as f32).clamp(0.0, h as f32),
                );
                vertices.push(make_vertex(px, py, w, h, placement));
                (vertices.len() - 1) as u32
            })
        });
        triangles.push(tri);
    }
    Ok(Mesh {
        bones: Vec::new(),
        hull: 0,
        edges: Vec::new(),
        name: name.to_string(),
        image,
        vertices,
        triangles,
    })
}

fn point_in_loops(loops: &[Vec<[f32; 2]>], p: [f32; 2]) -> bool {
    let mut inside = false;
    for polygon in loops {
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
        }
    }
    inside
}

fn make_vertex(px: f32, py: f32, w: usize, h: usize, placement: Placement) -> MeshVertex {
    let [x, y] = placement.to_world([px, py]);
    MeshVertex {
        x,
        y,
        u: round4(px / w as f32),
        v: round4(py / h as f32),
        weights: Vec::new(),
    }
}

fn spade_point(px: f32, py: f32) -> Point2<f64> {
    Point2::new(f64::from(px), -f64::from(py))
}

/// The outlines as constraint loops; `None` when two of them cross.
fn build_cdt(polys: &[Vec<[f32; 2]>]) -> Option<ConstrainedDelaunayTriangulation<Point2<f64>>> {
    let mut points = Vec::new();
    let mut edges = Vec::new();
    for p in polys {
        let base = points.len();
        points.extend(p.iter().map(|&[x, y]| spade_point(x, y)));
        edges.extend((0..p.len()).map(|k| [base + k, base + (k + 1) % p.len()]));
    }
    let mut conflict = false;
    let cdt = ConstrainedDelaunayTriangulation::try_bulk_load_cdt(points.clone(), edges, |_| {
        conflict = true
    })
    .ok()?;
    // Merged duplicates would silently reroute an outline.
    (!conflict && cdt.num_vertices() == points.len()).then_some(cdt)
}

/// Grows the mask by `r` pixels (square neighbourhood).
fn dilate(mask: &Mask, r: usize) -> Mask {
    let (w, h) = (mask.width, mask.height);
    let rows = Mask::from_fn(w, h, |x, y| {
        mask.any_in(x.saturating_sub(r), y, x + r + 1, y + 1)
    });
    Mask::from_fn(w, h, |x, y| {
        rows.any_in(x, y.saturating_sub(r), x + 1, y + r + 1)
    })
}

/// Fills one pixel of every 2x2 block whose solid pixels touch only
/// diagonally, so each outline corner has a single way on and loops never
/// touch.
fn without_pinches(mask: &Mask) -> Mask {
    let (w, h) = (mask.width, mask.height);
    let mut solid: Vec<bool> = (0..w * h).map(|i| mask.get(i % w, i / w)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let (a, b, c, d) = (
                    y * w + x,
                    y * w + x + 1,
                    (y + 1) * w + x,
                    (y + 1) * w + x + 1,
                );
                if solid[a] == solid[d] && solid[b] == solid[c] && solid[a] != solid[b] {
                    let fill = if solid[a] { b } else { a };
                    solid[fill] = true;
                    changed = true;
                }
            }
        }
    }
    Mask::from_fn(w, h, |x, y| solid[y * w + x])
}

/// Closed outlines along pixel edges, as corner points (pixel coordinates)
/// with straight runs collapsed. Outer loops run clockwise on screen, holes
/// counter-clockwise. The mask must be free of pinches.
fn trace_loops(mask: &Mask) -> Vec<Vec<[f32; 2]>> {
    let (w, h) = (mask.width, mask.height);
    let corner = |x: usize, y: usize| y * (w + 1) + x;
    let mut next: HashMap<usize, usize> = HashMap::new();
    let empty = |x: i64, y: i64| x < 0 || y < 0 || !mask.get(x as usize, y as usize);
    for y in 0..h {
        for x in 0..w {
            if !mask.get(x, y) {
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            if empty(xi, yi - 1) {
                next.insert(corner(x, y), corner(x + 1, y));
            }
            if empty(xi + 1, yi) {
                next.insert(corner(x + 1, y), corner(x + 1, y + 1));
            }
            if empty(xi, yi + 1) {
                next.insert(corner(x + 1, y + 1), corner(x, y + 1));
            }
            if empty(xi - 1, yi) {
                next.insert(corner(x, y + 1), corner(x, y));
            }
        }
    }
    let at = |c: usize| [(c % (w + 1)) as f32, (c / (w + 1)) as f32];
    let mut starts: Vec<usize> = next.keys().copied().collect();
    starts.sort_unstable();
    let mut seen = HashSet::new();
    let mut loops = Vec::new();
    for start in starts {
        if seen.contains(&start) {
            continue;
        }
        let mut chain = Vec::new();
        let mut cur = start;
        while seen.insert(cur) {
            chain.push(cur);
            cur = next[&cur];
        }
        let n = chain.len();
        let pts: Vec<[f32; 2]> = (0..n)
            .filter(|&k| {
                let (a, b, c) = (
                    at(chain[(k + n - 1) % n]),
                    at(chain[k]),
                    at(chain[(k + 1) % n]),
                );
                (b[0] - a[0], b[1] - a[1]) != (c[0] - b[0], c[1] - b[1])
            })
            .map(|k| at(chain[k]))
            .collect();
        loops.push(pts);
    }
    loops
}

/// Douglas-Peucker on a closed loop, split at the point farthest from the
/// first one.
fn simplify_closed(pts: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    let n = pts.len();
    if tol <= 0.0 || n <= 4 {
        return pts.to_vec();
    }
    let far = (1..n)
        .max_by(|&a, &b| dist2(pts[0], pts[a]).total_cmp(&dist2(pts[0], pts[b])))
        .expect("loop has points");
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let ring: Vec<usize> = (0..=n).map(|k| k % n).collect();
    douglas_peucker(pts, &ring[..=far], tol, &mut keep);
    douglas_peucker(pts, &ring[far..], tol, &mut keep);
    (0..n).filter(|&k| keep[k]).map(|k| pts[k]).collect()
}

fn douglas_peucker(pts: &[[f32; 2]], run: &[usize], tol: f32, keep: &mut [bool]) {
    let mut stack = vec![(0, run.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let (a, b) = (pts[run[s]], pts[run[e]]);
        let (k, d) = (s + 1..e)
            .map(|k| (k, segment_distance(pts[run[k]], a, b)))
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .expect("run has interior points");
        if d > tol {
            keep[run[k]] = true;
            stack.push((s, k));
            stack.push((k, e));
        }
    }
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round4(v: f32) -> f32 {
    (v * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXEL: Placement = Placement {
        origin: [0.0, 0.0],
        scale: 1.0,
    };

    fn pixel_pos(v: &MeshVertex) -> [f32; 2] {
        PIXEL.to_pixel([v.x, v.y])
    }

    fn contains(m: &Mesh, p: [f32; 2]) -> bool {
        m.triangles.iter().any(|t| {
            let [a, b, c] = t.map(|i| pixel_pos(&m.vertices[i as usize]));
            let s = |u: [f32; 2], v: [f32; 2]| {
                (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0])
            };
            let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
            !((d1 < -1e-4 || d2 < -1e-4 || d3 < -1e-4) && (d1 > 1e-4 || d2 > 1e-4 || d3 > 1e-4))
        })
    }

    fn signed_area(m: &Mesh, t: [u32; 3]) -> f32 {
        let [a, b, c] = t.map(|i| [m.vertices[i as usize].x, m.vertices[i as usize].y]);
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }

    /// A disc with a square hole and a separate bar: curves, holes and islands.
    fn shapes() -> Mask {
        Mask::from_fn(240, 200, |x, y| {
            let (dx, dy) = (x as f32 - 90.0, y as f32 - 100.0);
            let disc = dx * dx + dy * dy <= 70.0 * 70.0
                && !(70..110).contains(&x) | !(80..120).contains(&y);
            disc || ((190..220).contains(&x) && (20..180).contains(&y))
        })
    }

    #[test]
    fn outline_mesh_covers_every_pixel_with_fewer_vertices() {
        let mask = shapes();
        let params = OutlineParams::from_spacing(16.0);
        let m = outline_mesh(&mask, "m", None, params, PIXEL, &[]);
        let grid = grid_mesh(&mask, "g", None, 16, PIXEL);
        assert!(
            m.vertices.len() * 2 < grid.vertices.len(),
            "outline {} vs grid {}",
            m.vertices.len(),
            grid.vertices.len()
        );
        for y in 0..mask.height {
            for x in 0..mask.width {
                if mask.get(x, y) {
                    assert!(
                        contains(&m, [x as f32 + 0.5, y as f32 + 0.5]),
                        "pixel {x},{y} not covered"
                    );
                }
            }
        }
        // The hole stays open and the gap between the islands is not bridged.
        assert!(!contains(&m, [90.0, 100.0]));
        assert!(!contains(&m, [175.0, 100.0]));
        // Same winding as the grid, no degenerate triangles, UVs in range.
        assert!(m.triangles.iter().all(|&t| signed_area(&m, t) > 0.0));
        assert!(
            m.vertices
                .iter()
                .all(|v| (0.0..=1.0).contains(&v.u) && (0.0..=1.0).contains(&v.v))
        );
    }

    #[test]
    fn joints_add_local_vertices() {
        let mask = Mask::from_fn(300, 40, |x, y| {
            (10..290).contains(&x) && (10..30).contains(&y)
        });
        let params = OutlineParams::from_spacing(24.0);
        let plain = outline_mesh(&mask, "m", None, params, PIXEL, &[]);
        let bent = outline_mesh(&mask, "m", None, params, PIXEL, &[[150.0, 20.0]]);
        let near = |m: &Mesh| {
            m.vertices
                .iter()
                .filter(|v| dist2(pixel_pos(v), [150.0, 20.0]) < 12.0 * 12.0)
                .count()
        };
        assert!(
            near(&bent) > near(&plain) + 2,
            "{} vs {}",
            near(&bent),
            near(&plain)
        );
        // Joints outside the mask are ignored.
        let outside = outline_mesh(&mask, "m", None, params, PIXEL, &[[150.0, 2.0]]);
        assert_eq!(outside.vertices.len(), plain.vertices.len());
    }

    #[test]
    fn diagonal_pixels_and_close_parts_still_mesh() {
        // A checkerboard corner and two bars one pixel apart stress pinches
        // and crossing simplified outlines.
        let mask = Mask::from_fn(64, 64, |x, y| {
            (x < 8 && y < 8 && (x + y) % 2 == 0)
                || ((20..60).contains(&x) && ((20..30).contains(&y) || (31..41).contains(&y)))
        });
        let m = outline_mesh(
            &mask,
            "m",
            None,
            OutlineParams {
                spacing: 8.0,
                tolerance: 4.0,
            },
            PIXEL,
            &[],
        );
        for y in 0..64 {
            for x in 0..64 {
                if mask.get(x, y) {
                    assert!(
                        contains(&m, [x as f32 + 0.5, y as f32 + 0.5]),
                        "pixel {x},{y} not covered"
                    );
                }
            }
        }
    }
}
