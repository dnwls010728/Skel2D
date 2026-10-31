//! Pure mesh topology shared by editor, CLI, HTTP and MCP.
//! Vertex remaps carry interpolation factors back to the input mesh, so
//! topology edits preserve skin weights and every deformation key.

use crate::{Mesh, MeshVertex, Weight};
use spade::{ConstrainedDelaunayTriangulation, HasPosition, Point2, Triangulation};
use std::collections::{BTreeMap, BTreeSet};

type P = [f32; 2];

fn pos(v: &MeshVertex) -> P {
    [v.x, v.y]
}

fn orient(a: P, b: P, c: P) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Barycentric coordinates of `p` in triangle `abc`, `None` when degenerate.
fn barycentric(p: P, [a, b, c]: [P; 3]) -> Option<[f32; 3]> {
    let area = orient(a, b, c);
    if area.abs() < 1e-9 {
        return None;
    }
    let l0 = orient(p, b, c) / area;
    let l1 = orient(a, p, c) / area;
    Some([l0, l1, 1.0 - l0 - l1])
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round4(v: f32) -> f32 {
    (v * 10000.0).round() / 10000.0
}

/// Weights as an explicit, normalized bone -> weight map (empty = the slot's bone).
pub fn weight_map(v: &MeshVertex, slot_bone: &str) -> BTreeMap<String, f32> {
    let mut m = BTreeMap::new();
    for w in v
        .weights
        .iter()
        .filter(|w| w.weight > 0.0 && w.weight.is_finite())
    {
        *m.entry(w.bone.clone()).or_insert(0.0) += w.weight;
    }
    let total: f32 = m.values().sum();
    if total <= 0.0 {
        m.clear();
        m.insert(slot_bone.to_string(), 1.0);
    } else {
        m.values_mut().for_each(|w| *w /= total);
    }
    m
}

/// Back to the stored form: tiny weights dropped, renormalized, rounded,
/// heaviest first, and empty when the slot's bone has everything.
fn weight_list(mut m: BTreeMap<String, f32>, slot_bone: &str) -> Vec<Weight> {
    m.retain(|_, w| *w > 0.0 && w.is_finite());
    let total: f32 = m.values().sum();
    if total <= 0.0 || (m.len() == 1 && m.contains_key(slot_bone)) {
        return Vec::new();
    }
    let mut out: Vec<Weight> = m
        .into_iter()
        .map(|(bone, w)| Weight {
            bone,
            weight: round4(w / total), offset: [0.0, 0.0],
        })
        .filter(|w| w.weight > 0.0)
        .collect();
    out.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.bone.cmp(&b.bone))
    });
    let sum: f32 = out.iter().map(|w| w.weight).sum();
    if let Some(first) = out.first_mut() { first.weight = round4(first.weight + 1.0 - sum); }
    out
}

/// Blend of vertices by factors summing to 1 (position, UV and weights).
fn blend(mesh: &Mesh, parts: &[(usize, f32)], slot_bone: &str) -> MeshVertex {
    let (mut x, mut y, mut u, mut v) = (0.0, 0.0, 0.0, 0.0);
    let mut weights: BTreeMap<String, f32> = BTreeMap::new();
    let all_default = parts
        .iter()
        .all(|&(i, _)| mesh.vertices[i].weights.is_empty());
    for &(i, f) in parts {
        let vert = &mesh.vertices[i];
        x += vert.x * f;
        y += vert.y * f;
        u += vert.u * f;
        v += vert.v * f;
        for (b, w) in weight_map(vert, slot_bone) {
            *weights.entry(b).or_insert(0.0) += w * f;
        }
    }
    MeshVertex {
        x: round2(x),
        y: round2(y),
        u: round4(u.clamp(0.0, 1.0)),
        v: round4(v.clamp(0.0, 1.0)),
        weights: if all_default {
            Vec::new()
        } else {
            weight_list(weights, slot_bone)
        },
    }
}

/// Least-squares affine map from setup-pose position to UV, so a moved or
/// added vertex can keep showing the image pixel under it.
#[derive(Debug, Clone, Copy)]
pub struct UvMap {
    u: [f32; 3],
    v: [f32; 3],
}

impl UvMap {
    pub fn fit(mesh: &Mesh) -> Option<UvMap> {
        if mesh.vertices.len() < 3 {
            return None;
        }
        // Centered for numerical stability.
        let n = mesh.vertices.len() as f64;
        let (cx, cy) = mesh.vertices.iter().fold((0.0, 0.0), |(a, b), v| {
            (a + v.x as f64 / n, b + v.y as f64 / n)
        });
        let (mut sxx, mut sxy, mut syy) = (0.0f64, 0.0f64, 0.0f64);
        let (mut su, mut sv) = ([0.0f64; 3], [0.0f64; 3]);
        for vert in &mesh.vertices {
            let (x, y) = (vert.x as f64 - cx, vert.y as f64 - cy);
            sxx += x * x;
            sxy += x * y;
            syy += y * y;
            for (s, t) in [(&mut su, vert.u as f64), (&mut sv, vert.v as f64)] {
                s[0] += x * t;
                s[1] += y * t;
                s[2] += t;
            }
        }
        let det = sxx * syy - sxy * sxy;
        if det.abs() < 1e-9 * (sxx + syy).max(1.0).powi(2) {
            return None;
        }
        let solve = |s: [f64; 3]| {
            let a = (s[0] * syy - s[1] * sxy) / det;
            let b = (s[1] * sxx - s[0] * sxy) / det;
            let c = s[2] / n - a * cx - b * cy;
            [a as f32, b as f32, c as f32]
        };
        Some(UvMap {
            u: solve(su),
            v: solve(sv),
        })
    }

    pub fn at(&self, [x, y]: P) -> P {
        let f = |m: [f32; 3]| round4((m[0] * x + m[1] * y + m[2]).clamp(0.0, 1.0));
        [f(self.u), f(self.v)]
    }
}

/// Nearest triangle edge within `tol` of `p`: (a, b, t along a->b).
fn nearest_edge(mesh: &Mesh, p: P, tol: f32) -> Option<(u32, u32, f32)> {
    let mut best: Option<(f32, u32, u32, f32)> = None;
    for t in &mesh.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let (pa, pb) = (
                pos(&mesh.vertices[a as usize]),
                pos(&mesh.vertices[b as usize]),
            );
            let ab = [pb[0] - pa[0], pb[1] - pa[1]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1];
            if len2 < 1e-9 {
                continue;
            }
            let s = (((p[0] - pa[0]) * ab[0] + (p[1] - pa[1]) * ab[1]) / len2).clamp(0.0, 1.0);
            let d = (pa[0] + ab[0] * s - p[0]).hypot(pa[1] + ab[1] * s - p[1]);
            // Close to an end point means the user aimed at the vertex, not the edge.
            if d < tol && s > 0.08 && s < 0.92 && best.is_none_or(|bst| d < bst.0) {
                best = Some((d, a, b, s));
            }
        }
    }
    best.map(|(_, a, b, s)| (a, b, s))
}

/// For each output vertex, the input indices and interpolation factors.
pub type VertexRemap = Vec<Vec<(usize, f32)>>;

/// Interpolation factors at the nearest point of a mesh's setup triangles.
/// Rebuilding an attachment uses these for both deformation and constraints.
pub fn interpolation_parts(mesh: &Mesh, point: [f32; 2]) -> Result<Vec<(usize, f32)>, String> {
    let mut best = None;
    for triangle in &mesh.triangles {
        let vertices = triangle.map(|i| mesh.vertices.get(i as usize));
        let [Some(a),Some(b),Some(c)] = vertices else { return Err("triangle index out of range".into()) };
        if orient(pos(a),pos(b),pos(c)).abs() < 1e-10 { continue; }
        let (nearest, factors) = crate::ops::closest_on_triangle(point,pos(a),pos(b),pos(c));
        let distance = (point[0]-nearest[0]).hypot(point[1]-nearest[1]);
        if best.as_ref().is_none_or(|(d,_)| distance < *d) {
            best = Some((distance,triangle.iter().zip(factors).map(|(&i,f)|(i as usize,f)).collect()));
        }
    }
    best.map(|(_,parts)|parts).ok_or_else(||"mesh has no nondegenerate triangle to sample".into())
}

#[derive(Debug, Clone)]
pub struct MeshEdit {
    pub mesh: Mesh,
    pub remap: VertexRemap,
}

#[derive(Clone)]
struct IndexedPoint {
    point: Point2<f64>,
    index: u32,
}
impl HasPosition for IndexedPoint {
    type Scalar = f64;
    fn position(&self) -> Point2<f64> {
        self.point
    }
}

fn identity(mesh: &Mesh) -> VertexRemap {
    (0..mesh.vertices.len()).map(|i| vec![(i, 1.0)]).collect()
}

/// Unique undirected triangle edges as [min, max], sorted.
pub fn edges(mesh: &Mesh) -> Vec<[u32; 2]> {
    mesh.triangles
        .iter()
        .flat_map(|t| (0..3).map(move |k| edge(t[k], t[(k + 1) % 3])))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn edge(a: u32, b: u32) -> [u32; 2] {
    [a.min(b), a.max(b)]
}

fn ring_area(mesh: &Mesh, ring: &[u32]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let a = &mesh.vertices[ring[i] as usize];
            let b = &mesh.vertices[ring[(i + 1) % ring.len()] as usize];
            a.x as f64 * b.y as f64 - a.y as f64 * b.x as f64
        })
        .sum::<f64>()
        * 0.5
}

/// Actual boundary loops, including holes and disconnected legacy islands.
pub fn boundary_loops(mesh: &Mesh) -> Result<Vec<Vec<u32>>, String> {
    let mut counts = BTreeMap::new();
    for t in &mesh.triangles {
        if t.iter().any(|&i| i as usize >= mesh.vertices.len()) {
            return Err("triangle index out of range".into());
        }
        for k in 0..3 {
            *counts.entry(edge(t[k], t[(k + 1) % 3])).or_insert(0usize) += 1;
        }
    }
    if counts.values().any(|&n| n > 2) {
        return Err("mesh boundary is non-manifold".into());
    }
    let mut graph: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for ([a, b], n) in counts {
        if n == 1 {
            graph.entry(a).or_default().push(b);
            graph.entry(b).or_default().push(a);
        }
    }
    if graph.values().any(|n| n.len() != 2) {
        return Err("mesh boundary joins separate parts".into());
    }
    let mut unseen: BTreeSet<u32> = graph.keys().copied().collect();
    let mut rings = Vec::new();
    while let Some(&start) = unseen.first() {
        let mut ring = Vec::new();
        let (mut prev, mut at) = (u32::MAX, start);
        loop {
            if !unseen.remove(&at) {
                return Err("mesh boundary does not form simple loops".into());
            }
            ring.push(at);
            let next = graph[&at].iter().copied().find(|&i| i != prev).unwrap();
            if next == start {
                break;
            }
            prev = at;
            at = next;
        }
        if ring.len() < 3 {
            return Err("mesh boundary needs three vertices".into());
        }
        rings.push(ring);
    }
    if rings.is_empty() {
        return Err("mesh has no boundary".into());
    }
    rings.sort_by(|a, b| {
        ring_area(mesh, b)
            .abs()
            .total_cmp(&ring_area(mesh, a).abs())
            .then_with(|| a.cmp(b))
    });
    Ok(rings)
}

fn domain(mesh: &Mesh) -> Result<Vec<Vec<u32>>, String> {
    if mesh.hull == 0 {
        return boundary_loops(mesh);
    }
    if mesh.hull < 3 || mesh.hull as usize > mesh.vertices.len() {
        return Err("invalid hull vertex count".into());
    }
    let mut rings = vec![(0..mesh.hull).collect()];
    // Preserve holes from existing triangles. A user constraint loop alone
    // is not a hole; only the actual triangle boundary defines a cutout.
    if let Ok(old) = boundary_loops(mesh) {
        rings.extend(
            old.into_iter()
                .filter(|r| r.iter().all(|&i| i >= mesh.hull)),
        );
    }
    Ok(rings)
}

fn on_segment(p: P, a: P, b: P) -> bool {
    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
    orient(a, b, p).abs() <= 1e-6 * length.max(1.0)
        && (p[0] - a[0]) * (p[0] - b[0]) + (p[1] - a[1]) * (p[1] - b[1]) <= 1e-6
}

fn inside_ring(mesh: &Mesh, ring: &[u32], p: P) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (
            pos(&mesh.vertices[ring[i] as usize]),
            pos(&mesh.vertices[ring[(i + 1) % ring.len()] as usize]),
        );
        if on_segment(p, a, b) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

fn inside(mesh: &Mesh, rings: &[Vec<u32>], p: P) -> bool {
    if rings.iter().any(|r| {
        (0..r.len()).any(|i| {
            on_segment(
                p,
                pos(&mesh.vertices[r[i] as usize]),
                pos(&mesh.vertices[r[(i + 1) % r.len()] as usize]),
            )
        })
    }) {
        return true;
    }
    rings.iter().filter(|r| inside_ring(mesh, r, p)).count() % 2 == 1
}

/// Whether two non-adjacent segments of the ring cross.
fn ring_crosses_itself(mesh: &Mesh, ring: &[u32]) -> bool {
    let n = ring.len();
    let at = |i: usize| pos(&mesh.vertices[ring[i % n] as usize]);
    (0..n).any(|i| {
        (i + 2..n).filter(|&j| (j + 1) % n != i).any(|j| {
            let (a, b, c, d) = (at(i), at(i + 1), at(j), at(j + 1));
            orient(a, b, c) * orient(a, b, d) < 0.0 && orient(c, d, a) * orient(c, d, b) < 0.0
        })
    })
}

fn split_edges(mesh: &Mesh, constraints: &[[u32; 2]]) -> Result<Vec<[u32; 2]>, String> {
    let mut out = BTreeSet::new();
    for &[a, b] in constraints {
        if a == b || a as usize >= mesh.vertices.len() || b as usize >= mesh.vertices.len() {
            return Err("invalid constrained edge endpoints".into());
        }
        let (pa, pb) = (
            pos(&mesh.vertices[a as usize]),
            pos(&mesh.vertices[b as usize]),
        );
        let mut points: Vec<_> = mesh
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| on_segment(pos(v), pa, pb))
            .map(|(i, v)| (i as u32, (v.x - pa[0]).hypot(v.y - pa[1])))
            .collect();
        points.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        for pair in points.windows(2) {
            out.insert(edge(pair[0].0, pair[1].0));
        }
    }
    Ok(out.into_iter().collect())
}

/// Constrained Delaunay with polygon clipping, deterministic vertex/face order.
fn rebuild(
    mut mesh: Mesh,
    mut rings: Vec<Vec<u32>>,
    mut remap: VertexRemap,
) -> Result<MeshEdit, String> {
    if mesh.vertices.len() < 3 || mesh.vertices.len() > 100_000 {
        return Err("mesh must have 3..100000 vertices".into());
    }
    if rings.iter().any(|r| {
        r.len() < 3
            || r.iter().any(|&i| i as usize >= mesh.vertices.len())
            || ring_area(&mesh, r).abs() < 1e-10
    }) {
        return Err("degenerate mesh boundary".into());
    }
    if mesh
        .vertices
        .iter()
        .any(|v| ![v.x, v.y, v.u, v.v].iter().all(|c| c.is_finite()))
    {
        return Err("mesh coordinates must be finite".into());
    }
    // Legacy disjoint islands cannot be represented by one front hull.
    // Keep hull=0 there, with their exact boundaries retained in triangles.
    let single_outer = rings
        .iter()
        .skip(1)
        .all(|r| inside_ring(&mesh, &rings[0], pos(&mesh.vertices[r[0] as usize])));
    if single_outer {
        if ring_area(&mesh, &rings[0]) > 0.0 {
            rings[0].reverse();
        }
        // Stable start makes repeated triangulation idempotent.
        let start = rings[0]
            .iter()
            .enumerate()
            .min_by_key(|(_, i)| **i)
            .unwrap()
            .0;
        rings[0].rotate_left(start);
        let mut order = rings[0].clone();
        let front: BTreeSet<_> = order.iter().copied().collect();
        order.extend((0..mesh.vertices.len() as u32).filter(|i| !front.contains(i)));
        let mut map = vec![0u32; order.len()];
        for (new, &old) in order.iter().enumerate() {
            map[old as usize] = new as u32;
        }
        mesh.vertices = order
            .iter()
            .map(|&i| mesh.vertices[i as usize].clone())
            .collect();
        remap = order.iter().map(|&i| remap[i as usize].clone()).collect();
        for ring in &mut rings {
            for i in ring {
                *i = map[*i as usize];
            }
        }
        for e in &mut mesh.edges {
            if e.iter().any(|&i| i as usize >= map.len()) {
                return Err("constrained edge index out of range".into());
            }
            *e = e.map(|i| map[i as usize]);
        }
        mesh.hull = rings[0].len() as u32;
    } else {
        mesh.hull = 0;
    }
    if mesh.vertices.iter().any(|v| !inside(&mesh, &rings, pos(v))) {
        return Err("vertex lies outside the mesh boundary".into());
    }
    mesh.edges = split_edges(&mesh, &mesh.edges)?;
    let mut constraints = mesh.edges.clone();
    for r in &rings {
        constraints.extend((0..r.len()).map(|i| edge(r[i], r[(i + 1) % r.len()])));
    }
    constraints.sort_unstable();
    constraints.dedup();
    let points = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| IndexedPoint {
            point: Point2::new(v.x as f64, v.y as f64),
            index: i as u32,
        })
        .collect();
    let mut conflict = false;
    let cdt = ConstrainedDelaunayTriangulation::<IndexedPoint>::try_bulk_load_cdt(
        points,
        constraints
            .iter()
            .map(|e| [e[0] as usize, e[1] as usize])
            .collect(),
        |_| conflict = true,
    )
    .map_err(|e| format!("cannot triangulate mesh: {e}"))?;
    if conflict || cdt.num_vertices() != mesh.vertices.len() {
        return Err("crossing constraints or duplicate vertex positions".into());
    }
    mesh.triangles = cdt
        .inner_faces()
        .filter_map(|face| {
            let mut t = face.vertices().map(|v| v.data().index);
            let k = t.iter().enumerate().min_by_key(|(_, i)| **i).unwrap().0;
            t.rotate_left(k);
            // In stored order: slivers along a straight side sit within rounding
            // of the outline, and validation must reach the same verdict.
            let p = t.map(|i| pos(&mesh.vertices[i as usize]));
            let center = [
                (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            ];
            inside(&mesh, &rings, center).then_some(t)
        })
        .collect();
    mesh.triangles.sort_unstable();
    if mesh.triangles.is_empty() {
        return Err("mesh must keep at least one triangle".into());
    }
    let triangle_edges: BTreeSet<_> = edges(&mesh).into_iter().collect();
    if mesh.edges.iter().any(|e| !triangle_edges.contains(e)) {
        return Err("constrained edge leaves the mesh boundary".into());
    }
    Ok(MeshEdit { mesh, remap })
}

pub fn triangulate(mesh: &Mesh) -> Result<MeshEdit, String> {
    rebuild(mesh.clone(), domain(mesh)?, identity(mesh))
}

/// Refits the triangles after vertices moved, keeping every vertex index, so
/// weights and deform keys stay valid. A layout that cannot be triangulated
/// (or a flipped hull) is left as it is for validation to report: other ops of
/// the same batch may still fix it.
pub fn refit_triangles(mesh: &mut Mesh) {
    if mesh.hull == 0 || hull_crossing(mesh).is_some() || ring_area(mesh, &(0..mesh.hull).collect::<Vec<_>>()) >= 0.0 {
        return;
    }
    if let Ok(edit) = triangulate(mesh) {
        mesh.triangles = edit.mesh.triangles;
        mesh.edges = edit.mesh.edges;
    }
}

/// Validates stored hull/constraint metadata against actual triangles. Where
/// the vertices happen to be is not part of this: a mesh may be folded or
/// tangled while it is worked on, and still draws (see [`geometry_problem`]).
/// Legacy meshes with no metadata retain their original load semantics.
pub fn validate_topology(mesh: &Mesh) -> Result<(), String> {
    if mesh.hull == 0 && mesh.edges.is_empty() {
        return Ok(());
    }
    let triangle_edges: BTreeSet<_> = edges(mesh).into_iter().collect();
    let mut seen = BTreeSet::new();
    for &[a, b] in &mesh.edges {
        if a == b || a as usize >= mesh.vertices.len() || b as usize >= mesh.vertices.len() {
            return Err("invalid constrained edge endpoints".into());
        }
        if !seen.insert(edge(a, b)) {
            return Err("duplicate constrained edge".into());
        }
        if !triangle_edges.contains(&edge(a, b)) {
            return Err("constraint is not an edge of the mesh triangles".into());
        }
    }
    if mesh.hull != 0 {
        if mesh.hull < 3 || mesh.hull as usize > mesh.vertices.len() {
            return Err("hull must be 0 or 3..vertex count".into());
        }
        if mesh.triangles.iter().flatten().any(|&i| i as usize >= mesh.vertices.len()) {
            return Err("triangle index out of range".into());
        }
        for i in 0..mesh.hull {
            if !triangle_edges.contains(&edge(i, (i + 1) % mesh.hull)) {
                return Err("hull segment is not an edge of the mesh triangles".into());
            }
        }
    }
    Ok(())
}

/// Why the mesh cannot be triangulated as it now lies, if so. The mesh stays
/// valid and keeps its last triangles; ops that rebuild the triangles (adding
/// vertices, for instance) refuse with this reason until it is untangled.
pub fn geometry_problem(mesh: &Mesh) -> Option<String> {
    if mesh.hull != 0 {
        let ring: Vec<_> = (0..mesh.hull).collect();
        if ring_crosses_itself(mesh, &ring) {
            return Some("hull crosses itself".into());
        }
        if ring_area(mesh, &ring) >= 0.0 {
            return Some("hull must have nonzero area and clockwise winding".into());
        }
    }
    if mesh.hull == 0 && mesh.edges.is_empty() {
        return None;
    }
    triangulate(mesh).err()
}

/// A hull crossing itself leaves no inside to rebuild triangles in.
pub fn hull_crossing(mesh: &Mesh) -> Option<String> {
    (mesh.hull >= 3 && ring_crosses_itself(mesh, &(0..mesh.hull).collect::<Vec<_>>())).then(|| "hull crosses itself".to_string())
}

/// Whether the hull is wound the wrong way, as after a mirroring transform.
pub fn hull_reversed(mesh: &Mesh) -> bool {
    mesh.hull >= 3 && ring_area(mesh, &(0..mesh.hull).collect::<Vec<_>>()) >= 0.0
}

fn insert(edit: MeshEdit, p: P, tolerance: f32, slot_bone: &str) -> Result<MeshEdit, String> {
    let mesh = &edit.mesh;
    if !p.iter().all(|c| c.is_finite()) || !tolerance.is_finite() || tolerance < 0.0 {
        return Err(
            "point and edge tolerance must be finite; tolerance must be nonnegative".into(),
        );
    }
    if mesh
        .vertices
        .iter()
        .any(|v| (v.x - p[0]).hypot(v.y - p[1]) < 1e-5)
    {
        return Err("point coincides with an existing vertex".into());
    }
    let mut rings = domain(mesh)?;
    let parts = if let Some((a, b, s)) = nearest_edge(mesh, p, tolerance) {
        vec![(a as usize, 1.0 - s), (b as usize, s)]
    } else {
        mesh.triangles
            .iter()
            .find_map(|t| {
                let l = barycentric(p, t.map(|i| pos(&mesh.vertices[i as usize])))?;
                l.iter().all(|&v| v >= -1e-6).then(|| {
                    (0..3)
                        .filter(|&k| l[k] > 1e-7)
                        .map(|k| (t[k] as usize, l[k]))
                        .collect::<Vec<_>>()
                })
            })
            .ok_or("point lies outside the mesh")?
    };
    let mut vertex = blend(mesh, &parts, slot_bone);
    if parts.len() == 3 {
        vertex.x = p[0];
        vertex.y = p[1];
    }
    // Do not round the projected position off a narrow boundary edge.
    if parts.len() == 2 {
        vertex.x = parts.iter().map(|&(i, f)| mesh.vertices[i].x * f).sum();
        vertex.y = parts.iter().map(|&(i, f)| mesh.vertices[i].y * f).sum();
    }
    let n = mesh.vertices.len() as u32;
    for ring in &mut rings {
        if let Some(k) = (0..ring.len()).find(|&k| {
            parts.len() == 2
                && edge(ring[k], ring[(k + 1) % ring.len()])
                    == edge(parts[0].0 as u32, parts[1].0 as u32)
        }) {
            ring.insert(k + 1, n);
        }
    }
    let mut weights: BTreeMap<usize, f32> = BTreeMap::new();
    for &(i, f) in &parts {
        for &(old, w) in &edit.remap[i] {
            *weights.entry(old).or_default() += f * w;
        }
    }
    let mut next = edit;
    next.mesh.vertices.push(vertex);
    next.remap.push(weights.into_iter().collect());
    rebuild(next.mesh, rings, next.remap)
}

/// Adds a vertex at p, splitting a nearby edge or containing triangle.
/// UV, weights and deformation interpolation share the same factors.
pub fn insert_vertex(
    mesh: &Mesh,
    p: P,
    tolerance: f32,
    slot_bone: &str,
) -> Result<MeshEdit, String> {
    insert(triangulate(mesh)?, p, tolerance, slot_bone)
}

pub fn add_vertices(
    mesh: &Mesh,
    points: &[P],
    tolerance: f32,
    slot_bone: &str,
) -> Result<MeshEdit, String> {
    if points.is_empty() || points.len() > 2000 {
        return Err("give 1..2000 points".into());
    }
    let mut out = triangulate(mesh)?;
    for &p in points {
        out = insert(out, p, tolerance, slot_bone)?;
    }
    Ok(out)
}

/// Removes several vertices and retriangulates the remaining boundary.
/// Spine과 다름: weights and all deformation indices survive through remap.
pub fn remove_vertices(mesh: &Mesh, indices: &[usize]) -> Result<MeshEdit, String> {
    let remove: BTreeSet<_> = indices.iter().copied().collect();
    if remove.is_empty() || remove.iter().any(|&i| i >= mesh.vertices.len()) {
        return Err("invalid vertex indices".into());
    }
    let mut edit = MeshEdit {
        mesh: mesh.clone(),
        remap: identity(mesh),
    };
    for original in remove.into_iter().rev() {
        let index = edit
            .remap
            .iter()
            .position(|parts| parts.len() == 1 && parts[0].0 == original)
            .ok_or("vertex remap lost a surviving input vertex")?;
        edit = remove_one(edit, index)?;
    }
    Ok(edit)
}

/// Deleting a boundary vertex exposes its triangle fan's opposite path.
/// That path can contain former interior vertices, which become boundary
/// vertices rather than being discarded or stranded outside a shortened hull.
fn remove_one(edit: MeshEdit, index: usize) -> Result<MeshEdit, String> {
    let mesh = &edit.mesh;
    let mut rings = domain(mesh)?;
    for ring in &mut rings {
        let Some(at) = ring.iter().position(|&v| v as usize == index) else {
            continue;
        };
        let prev = ring[(at + ring.len() - 1) % ring.len()];
        let next = ring[(at + 1) % ring.len()];
        let mut fan: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for t in mesh
            .triangles
            .iter()
            .filter(|t| t.contains(&(index as u32)))
        {
            let opposite: Vec<_> = t.iter().copied().filter(|&v| v as usize != index).collect();
            if opposite.len() != 2 {
                return Err("degenerate triangle fan".into());
            }
            fan.entry(opposite[0]).or_default().push(opposite[1]);
            fan.entry(opposite[1]).or_default().push(opposite[0]);
        }
        if fan.get(&prev).is_none_or(|v| v.len() != 1)
            || fan.get(&next).is_none_or(|v| v.len() != 1)
            || fan
                .iter()
                .any(|(&v, n)| v != prev && v != next && n.len() != 2)
        {
            return Err("boundary vertex joins separate triangle fans".into());
        }
        let mut visited = BTreeSet::from([prev]);
        let mut path = Vec::new();
        let (mut before, mut current) = (u32::MAX, prev);
        while current != next {
            let after = fan
                .get(&current)
                .and_then(|n| n.iter().copied().find(|&v| v != before))
                .ok_or("boundary triangle fan is open")?;
            if !visited.insert(after) {
                return Err("boundary triangle fan contains a cycle".into());
            }
            if after != next {
                path.push(after);
            }
            before = current;
            current = after;
        }
        if visited.len() != fan.len() {
            return Err("boundary vertex joins separate parts".into());
        }
        ring.splice(at..at + 1, path);
        if ring.len() < 3 {
            return Err("removal would collapse a boundary below three vertices".into());
        }
    }
    let mut out = edit.mesh;
    out.vertices.remove(index);
    let map_index = |i: u32| if i as usize > index { i - 1 } else { i };
    for ring in &mut rings {
        for i in ring {
            *i = map_index(*i);
        }
    }
    out.edges.retain(|e| !e.contains(&(index as u32)));
    for e in &mut out.edges {
        *e = e.map(map_index);
    }
    let mut remap = edit.remap;
    remap.remove(index);
    rebuild(out, rings, remap)
}

/// Adds up to count interior vertices at the currently largest triangle's
/// centroid, or a deterministic spacing grid. Existing vertices are kept.
pub fn generate_interior(
    mesh: &Mesh,
    slot_bone: &str,
    count: Option<usize>,
    spacing: Option<f32>,
) -> Result<MeshEdit, String> {
    if count.is_some() && spacing.is_some() {
        return Err("give count or spacing, not both".into());
    }
    let mut out = triangulate(mesh)?;
    if let Some(spacing) = spacing {
        if !spacing.is_finite() || spacing <= 0.0 {
            return Err("spacing must be finite and positive".into());
        }
        let (minx, maxx, miny, maxy) = out.mesh.vertices.iter().fold(
            (
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ),
            |(a, b, c, d), v| (a.min(v.x), b.max(v.x), c.min(v.y), d.max(v.y)),
        );
        let (nx, ny) = (
            ((maxx - minx) / spacing).ceil() as usize,
            ((maxy - miny) / spacing).ceil() as usize,
        );
        if nx.saturating_mul(ny) > 2000 {
            return Err("spacing would generate more than 2000 candidates".into());
        }
        let rings = domain(&out.mesh)?;
        for iy in 1..ny {
            for ix in 1..nx {
                let p = [minx + ix as f32 * spacing, miny + iy as f32 * spacing];
                let boundary = rings.iter().any(|r| {
                    (0..r.len()).any(|k| {
                        on_segment(
                            p,
                            pos(&out.mesh.vertices[r[k] as usize]),
                            pos(&out.mesh.vertices[r[(k + 1) % r.len()] as usize]),
                        )
                    })
                });
                if !boundary
                    && inside(&out.mesh, &rings, p)
                    && !out
                        .mesh
                        .vertices
                        .iter()
                        .any(|v| (v.x - p[0]).hypot(v.y - p[1]) < 1e-5)
                {
                    out = insert(out, p, 0.0, slot_bone)?;
                }
            }
        }
    } else {
        let count = count.unwrap_or(8);
        if count == 0 || count > 2000 {
            return Err("count must be 1..2000".into());
        }
        for _ in 0..count {
            let m = &out.mesh;
            let t = m
                .triangles
                .iter()
                .max_by(|a, b| {
                    let area = |t: &&[u32; 3]| {
                        let p = t.map(|i| pos(&m.vertices[i as usize]));
                        orient(p[0], p[1], p[2]).abs()
                    };
                    area(a).total_cmp(&area(b))
                })
                .ok_or("mesh has no triangle")?;
            let p = t.map(|i| pos(&m.vertices[i as usize]));
            let center = [
                (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            ];
            out = insert(out, center, 0.0, slot_bone)?;
        }
    }
    Ok(out)
}

/// Fits UV to the current vertices without changing weights or deform keys.
pub fn reset_uvs(mesh: &Mesh) -> Result<MeshEdit, String> {
    let map = UvMap::fit(mesh).ok_or("cannot fit UVs to degenerate mesh")?;
    let mut out = mesh.clone();
    for v in &mut out.vertices {
        [v.u, v.v] = map.at([v.x, v.y]);
    }
    Ok(MeshEdit {
        mesh: out,
        remap: identity(mesh),
    })
}

/// Fits an affine UV-to-setup transform and restores vertices to that map.
/// No image I/O is needed; affine placement is retained.
pub fn reset_to_uvs(mesh: &Mesh) -> Result<MeshEdit, String> {
    let mut inverse = mesh.clone();
    for v in &mut inverse.vertices {
        (v.x, v.y, v.u, v.v) = (v.u, v.v, v.x, v.y);
    }
    let map = UvMap::fit(&inverse).ok_or("cannot fit vertices to degenerate UVs")?;
    let mut out = mesh.clone();
    for v in &mut out.vertices {
        v.x = map.u[0] * v.u + map.u[1] * v.v + map.u[2];
        v.y = map.v[0] * v.u + map.v[1] * v.v + map.v[2];
    }
    rebuild(out, domain(mesh)?, identity(mesh))
}

/// How a brush stroke changes the active bone's weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BrushMode {
    Add,
    Remove,
    Replace,
}

/// Sets `bone` to `value` (clamped to what the locked bones leave free) and
/// rescales the other unlocked bones so the total stays 1; locked bones keep
/// their exact weight. Freed weight with no unlocked bone to take it goes to
/// `fallback`, otherwise the vertex stays as it is.
fn set_bone(v: &MeshVertex, bone: &str, value: f32, slot_bone: &str, locked: &BTreeSet<String>, fallback: Option<&str>) -> Vec<Weight> {
    let mut m = weight_map(v, slot_bone);
    if locked.contains(bone) {
        return v.weights.clone();
    }
    let locked_sum: f32 = m.iter().filter(|(b, _)| locked.contains(*b)).map(|(_, w)| *w).sum();
    let free = (1.0 - locked_sum).max(0.0);
    let mut new = if value.is_finite() { value.clamp(0.0, free) } else { m.get(bone).copied().unwrap_or(0.0) };
    let others: f32 = m.iter().filter(|(b, _)| b.as_str() != bone && !locked.contains(*b)).map(|(_, w)| *w).sum();
    let remaining = free - new;
    if others > 1e-5 {
        let k = remaining / others;
        m.iter_mut().filter(|(b, _)| b.as_str() != bone && !locked.contains(*b)).for_each(|(_, w)| *w *= k);
    } else if remaining > 1e-5 {
        match fallback.filter(|f| *f != bone && !locked.contains(*f)) {
            Some(f) => *m.entry(f.to_string()).or_insert(0.0) += remaining,
            None => new = free,
        }
    }
    m.insert(bone.to_string(), new);
    preserve_locked(v, &m, slot_bone, locked)
}

/// One brush dab on a vertex. `Add` raises `bone` by `amount` up to `target`
/// (the brush strength), `Remove` lowers it by `amount`, `Replace` moves it
/// toward `target` by at most `amount`. Locked bones keep their weight.
#[allow(clippy::too_many_arguments)]
pub fn brush_weight(
    v: &MeshVertex,
    bone: &str,
    mode: BrushMode,
    amount: f32,
    target: f32,
    slot_bone: &str,
    locked: &BTreeSet<String>,
    fallback: Option<&str>,
) -> Vec<Weight> {
    let cur = weight_map(v, slot_bone).get(bone).copied().unwrap_or(0.0);
    let amount = amount.clamp(0.0, 1.0);
    let target = target.clamp(0.0, 1.0);
    let new = match mode {
        BrushMode::Add if cur >= target => cur,
        BrushMode::Add => (cur + amount).min(target),
        BrushMode::Remove => cur - amount,
        BrushMode::Replace => cur + (target - cur).clamp(-amount, amount),
    };
    set_bone(v, bone, new, slot_bone, locked, fallback)
}

/// Direct numeric weight for `bone` (the Spine "Direct" slider).
pub fn set_weight(v: &MeshVertex, bone: &str, value: f32, slot_bone: &str, locked: &BTreeSet<String>, fallback: Option<&str>) -> Vec<Weight> {
    set_bone(v, bone, value, slot_bone, locked, fallback)
}


/// Per-vertex adjacent vertices (sorted, unique).
pub fn neighbors(mesh: &Mesh) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); mesh.vertices.len()];
    for [a, b] in edges(mesh) {
        if (a as usize) < out.len() && (b as usize) < out.len() {
            out[a as usize].push(b);
            out[b as usize].push(a);
        }
    }
    out.iter_mut().for_each(|n| n.sort_unstable());
    out
}

/// Bones with nonzero weight on any vertex (the slot's bone for vertices with
/// default weights), sorted by name.
pub fn bones_used(mesh: &Mesh, slot_bone: &str) -> Vec<String> {
    let set: BTreeSet<String> = mesh.vertices.iter().flat_map(|v| weight_map(v, slot_bone).into_keys()).collect();
    set.into_iter().collect()
}

fn selected(mesh: &Mesh, only: &[usize]) -> Vec<usize> {
    if only.is_empty() { (0..mesh.vertices.len()).collect() } else { only.iter().copied().filter(|&i| i < mesh.vertices.len()).collect() }
}

/// One smoothing pass: each vertex blends its weights toward the average of
/// its edge neighbours' (as they were before the pass) by `factor`; locked
/// bones keep their weight. `only` empty = all vertices.
pub fn smooth_weights(mesh: &Mesh, slot_bone: &str, only: &[usize], locked: &BTreeSet<String>, factor: f32) -> Mesh {
    let factor = if factor.is_finite() { factor.clamp(0.0, 1.0) } else { 0.0 };
    let maps: Vec<BTreeMap<String, f32>> = mesh.vertices.iter().map(|v| weight_map(v, slot_bone)).collect();
    let nb = neighbors(mesh);
    let mut out = mesh.clone();
    for i in selected(mesh, only) {
        if nb[i].is_empty() {
            continue;
        }
        let cur = &maps[i];
        let mut avg: BTreeMap<String, f32> = BTreeMap::new();
        for &n in &nb[i] {
            for (b, w) in &maps[n as usize] {
                *avg.entry(b.clone()).or_insert(0.0) += w / nb[i].len() as f32;
            }
        }
        let locked_sum: f32 = cur.iter().filter(|(b, _)| locked.contains(*b)).map(|(_, w)| *w).sum();
        let mut blended: BTreeMap<String, f32> = BTreeMap::new();
        for b in cur.keys().chain(avg.keys()).filter(|b| !locked.contains(*b)) {
            let w = (1.0 - factor) * cur.get(b).copied().unwrap_or(0.0) + factor * avg.get(b).copied().unwrap_or(0.0);
            blended.insert(b.clone(), w);
        }
        let sum: f32 = blended.values().sum();
        let mut m: BTreeMap<String, f32> = cur.iter().filter(|(b, _)| locked.contains(*b)).map(|(b, w)| (b.clone(), *w)).collect();
        if sum > 1e-6 {
            let k = (1.0 - locked_sum).max(0.0) / sum;
            m.extend(blended.into_iter().map(|(b, w)| (b, w * k)));
        } else {
            m = cur.clone();
        }
        out.vertices[i].weights = preserve_locked(&mesh.vertices[i], &m, slot_bone, locked);
    }
    out
}

/// The influences of `m` that survive pruning: locked bones, then the heaviest
/// unlocked ones at or above `threshold`, at most `max_bones` in all (never
/// empty).
fn prune_map(m: &BTreeMap<String, f32>, max_bones: usize, threshold: f32, locked: &BTreeSet<String>) -> BTreeMap<String, f32> {
    let mut keep: BTreeMap<String, f32> = m.iter().filter(|(b, _)| locked.contains(*b)).map(|(b, w)| (b.clone(), *w)).collect();
    let mut rest: Vec<(&String, &f32)> = m.iter().filter(|(b, w)| !locked.contains(*b) && **w >= threshold).collect();
    rest.sort_by(|a, b| b.1.total_cmp(a.1).then_with(|| a.0.cmp(b.0)));
    keep.extend(rest.into_iter().take(max_bones.saturating_sub(keep.len())).map(|(b, w)| (b.clone(), *w)));
    // A partial locked budget still needs an unlocked recipient. Locks take
    // precedence over the count/threshold rather than discarding free weight.
    if keep.keys().all(|b|locked.contains(b))
        && let Some((b,w))=m.iter().filter(|(b,_)|!locked.contains(*b)).max_by(|a,b|a.1.total_cmp(b.1).then_with(||b.0.cmp(a.0)))
    {keep.insert(b.clone(),*w);}
    if keep.is_empty()
        && let Some((b, w)) = m.iter().max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
    {
        keep.insert(b.clone(), *w);
    }
    keep
}

/// Drops unlocked influences below `threshold`, keeps at most `max_bones`
/// (locked bones first, then the heaviest) and renormalizes.
pub fn prune_weights(mesh: &Mesh, slot_bone: &str, only: &[usize], max_bones: usize, threshold: f32, locked: &BTreeSet<String>) -> Mesh {
    let mut out = mesh.clone();
    for i in selected(mesh, only) {
        let m = weight_map(&mesh.vertices[i], slot_bone);
        let keep = prune_map(&m, max_bones, threshold, locked);
        out.vertices[i].weights = preserve_locked(&mesh.vertices[i], &keep, slot_bone, locked);
    }
    out
}

/// Number of (vertex, bone) influences `prune_weights` would remove.
pub fn prune_count(mesh: &Mesh, slot_bone: &str, only: &[usize], max_bones: usize, threshold: f32, locked: &BTreeSet<String>) -> usize {
    selected(mesh, only)
        .into_iter()
        .map(|i| {
            let m = weight_map(&mesh.vertices[i], slot_bone);
            m.len() - prune_map(&m, max_bones, threshold, locked).len()
        })
        .sum()
}

/// Swaps the weights of bones `a` and `b` on each vertex (`only` empty = all).
pub fn swap_weights(mesh: &Mesh, slot_bone: &str, only: &[usize], a: &str, b: &str) -> Mesh {
    let mut out = mesh.clone();
    if a == b {
        return out;
    }
    for i in selected(mesh, only) {
        let mut m = weight_map(&mesh.vertices[i], slot_bone);
        let (wa, wb) = (m.remove(a), m.remove(b));
        if wa.is_none() && wb.is_none() {
            continue;
        }
        if let Some(w) = wb {
            m.insert(a.to_string(), w);
        }
        if let Some(w) = wa {
            m.insert(b.to_string(), w);
        }
        out.vertices[i].weights = weight_list(m, slot_bone);
    }
    out
}


/// Average normalized weight map of the given vertices.
pub fn copy_weights(mesh: &Mesh, indices: &[usize], slot_bone: &str) -> BTreeMap<String, f32> {
    let idx: Vec<usize> = indices.iter().copied().filter(|&i| i < mesh.vertices.len()).collect();
    let mut avg: BTreeMap<String, f32> = BTreeMap::new();
    for &i in &idx {
        for (b, w) in weight_map(&mesh.vertices[i], slot_bone) {
            *avg.entry(b).or_insert(0.0) += w / idx.len() as f32;
        }
    }
    if avg.is_empty() {
        avg.insert(slot_bone.to_string(), 1.0);
    }
    avg
}

/// Sets each given vertex to the weight map `weights`.
pub fn paste_weights(mesh: &Mesh, indices: &[usize], weights: &BTreeMap<String, f32>, slot_bone: &str) -> Mesh {
    let mut out = mesh.clone();
    for &i in indices.iter().filter(|&&i| i < mesh.vertices.len()) {
        out.vertices[i].weights = weight_list(weights.clone(), slot_bone);
    }
    out
}

/// Spine과 다름: every weight operation keeps locked influences, including
/// automatic weights. Only the remaining budget is redistributed and rounded.
pub fn preserve_locked(v: &MeshVertex, candidate: &BTreeMap<String, f32>, slot_bone: &str, locked: &BTreeSet<String>) -> Vec<Weight> {
    if locked.is_empty() { return weight_list(candidate.clone(), slot_bone); }
    let old = weight_map(v, slot_bone);
    let budget = (1.0 - old.iter().filter(|(b, _)| locked.contains(*b)).map(|(_, w)| w).sum::<f32>()).max(0.0);
    let mut free: BTreeMap<String, f32> = candidate.iter().filter(|(b,w)| !locked.contains(*b) && **w > 0.0).map(|(b,w)|(b.clone(),*w)).collect();
    if free.is_empty() && budget > 0.0 {
        free = old.iter().filter(|(b,w)| !locked.contains(*b) && **w > 0.0).map(|(b,w)|(b.clone(),*w)).collect();
    }
    let total: f32 = free.values().sum();
    if budget > 0.0 && total <= 0.0 { return v.weights.clone(); }
    let mut out: Vec<Weight> = old.into_iter().filter(|(b,_)|locked.contains(b)).map(|(bone,weight)|Weight{bone,weight, offset: [0.0, 0.0]}).collect();
    let offset = out.len();
    if total > 0.0 {
        out.extend(free.into_iter().map(|(bone,w)|Weight{bone,weight:round4(w / total * budget), offset: [0.0, 0.0]}).filter(|w|w.weight>0.0));
    }
    let sum: f32 = out.iter().map(|w|w.weight).sum();
    if let Some(w) = out[offset..].iter_mut().max_by(|a,b|a.weight.total_cmp(&b.weight)) { w.weight = round4(w.weight + 1.0 - sum); }
    out.sort_by(|a,b|b.weight.total_cmp(&a.weight).then_with(||a.bone.cmp(&b.bone)));
    if out.len()==1 && out[0].bone==slot_bone { return Vec::new(); }
    out
}

/// Stored palette plus all effective influences, sorted and unique. Legacy
/// meshes infer their palette without changing serialization on load.
pub fn bound_bones(mesh: &Mesh, slot_bone: &str) -> Vec<String> {
    mesh.bones.iter().cloned().chain(bones_used(mesh,slot_bone)).collect::<BTreeSet<_>>().into_iter().collect()
}

pub fn normalize_weights(mesh: &Mesh, slot_bone: &str) -> Mesh {
    let mut out = mesh.clone();
    for v in &mut out.vertices { v.weights = weight_list(weight_map(v,slot_bone),slot_bone); }
    out
}


pub(crate) fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - abx * t).hypot(p[1] - a[1] - aby * t)
}

/// Redistributes removed palette entries without modifying geometry or keys.
/// The caller validates bone names and supplies the setup skeleton pose.
pub fn bind_weights(mesh: &Mesh, doc: &crate::Document, pose: &crate::Pose, slot_bone: &str, palette: BTreeSet<String>, remove: &[String], redistribute: crate::ops::Redistribute) -> Result<Mesh,String> {
    use crate::{Bone,ops::Redistribute};
    let mut out=mesh.clone();
    for v in &mut out.vertices {
        let mut weights=crate::mesh::weight_map(v,slot_bone);
        let lost: Vec<_> = remove.iter().filter_map(|b|weights.remove(b).map(|w|(b,w))).collect();
        if lost.is_empty() {continue;}
        let remaining: f32=weights.values().sum();
        for (removed,amount) in lost {
            if redistribute==Redistribute::Auto && remaining>0.0 {
                // Normalization below redistributes every removed influence
                // proportionally over the surviving nonzero influences.
                continue;
            } else {
                let parent=if redistribute==Redistribute::Parent {
                    let mut p=doc.bone(removed).and_then(|b|b.parent.as_deref()); let mut found=None;
                    for _ in 0..doc.bones.len() {
                        let Some(name)=p else {break};
                        if palette.contains(name) {found=Some(name.to_string());break;}
                        p=doc.bone(name).and_then(|b|b.parent.as_deref());
                    }
                    found
                } else {None};
                let nearest=|| doc.bones.iter().zip(&pose.bones).filter(|(b,_)|palette.contains(&b.name)).min_by(|(a,pa),(b,pb)| {
                    let distance=|bone:&Bone,p:&crate::pose::BonePose|segment_distance([v.x,v.y],p.world.apply(0.0,0.0),p.world.apply(bone.length,0.0));
                    distance(a,pa).total_cmp(&distance(b,pb)).then_with(||a.name.cmp(&b.name))
                }).map(|(b,_)|b.name.clone());
                let target=parent.or_else(nearest).ok_or("no remaining binding bone")?;
                *weights.entry(target).or_default()+=amount;
            }
        }
        v.weights=crate::mesh::preserve_locked(v,&weights,slot_bone,&BTreeSet::new());
    }
    out.bones=palette.into_iter().collect();
    Ok(out)
}
