//! Geodesic bone weights: distance from each bone is measured through the
//! visible pixels only. A torso pixel beside a hanging arm is close to the
//! arm in a straight line but far along the body (around the armpit), so it
//! stays on the torso and raising the arm no longer drags the jacket along.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use skel2d_core::{Mesh, Weight};

use crate::mask::Mask;
use crate::mesh::Placement;

/// A bone segment in source-image pixels.
pub struct BoneSegment {
    pub name: String,
    pub start: [f32; 2],
    pub end: [f32; 2],
}

pub fn geodesic_weights(mesh: &mut Mesh, mask: &Mask, bones: &[BoneSegment], place: Placement, max_influences: usize, falloff: f32) {
    if bones.is_empty() {
        return;
    }
    // Distance fields are per pixel and per bone, so bound the resolution.
    let (small, f) = mask.downscaled(192);
    let fields: Vec<Vec<f32>> = bones.iter().map(|b| distance_field(&small, [b.start[0] / f, b.start[1] / f], [b.end[0] / f, b.end[1] / f])).collect();
    let nearest = nearest_solid(&small);
    let (w, h) = (small.width, small.height);
    let (max_influences, falloff) = (max_influences.clamp(1, 8), falloff.clamp(0.1, 16.0));

    for v in &mut mesh.vertices {
        let [px, py] = place.to_pixel([v.x, v.y]);
        let (x, y) = (((px / f) as usize).min(w - 1), ((py / f) as usize).min(h - 1));
        // Vertices outside the silhouette borrow the nearest visible pixel.
        let i = nearest[y * w + x].unwrap_or(y * w + x);
        let mut scored: Vec<(f32, usize)> = fields
            .iter()
            .enumerate()
            .filter(|(_, d)| d[i].is_finite())
            .map(|(b, d)| (1.0 / (d[i] * f + 1.0).powf(falloff), b))
            .collect();
        if scored.is_empty() {
            // No bone touches this piece of the image (e.g. a separate layer):
            // fall back to straight-line distance.
            scored = bones
                .iter()
                .enumerate()
                .map(|(b, s)| (1.0 / (segment_distance([px, py], s.start, s.end) + 1.0).powf(falloff), b))
                .collect();
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(max_influences);
        let total: f32 = scored.iter().map(|s| s.0).sum();
        let mut weights: Vec<Weight> = scored
            .iter()
            .map(|&(s, b)| Weight { bone: bones[b].name.clone(), weight: round3(s / total), offset: [0.0, 0.0] })
            .filter(|w| w.weight >= 0.01)
            .collect();
        let sum: f32 = weights.iter().map(|w| w.weight).sum();
        if let Some(first) = weights.first_mut() {
            first.weight = round3(first.weight + 1.0 - sum);
        }
        v.weights = weights;
    }
}

#[derive(PartialEq)]
struct Node(f32, usize);

impl Eq for Node {}

impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0)
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Dijkstra over solid pixels (8-connected), seeded along the segment.
/// Unreachable pixels stay infinite.
fn distance_field(mask: &Mask, a: [f32; 2], b: [f32; 2]) -> Vec<f32> {
    let (w, h) = (mask.width, mask.height);
    let mut dist = vec![f32::INFINITY; w * h];
    let mut heap = BinaryHeap::new();
    let steps = ((b[0] - a[0]).hypot(b[1] - a[1]) * 2.0).ceil().max(1.0) as usize;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            continue;
        }
        let i = y as usize * w + x as usize;
        // Bones may pass over transparent gaps; seed them anyway so the
        // field still flows into the nearby visible pixels.
        if dist[i] > 0.0 {
            dist[i] = 0.0;
            heap.push(Node(0.0, i));
        }
    }
    while let Some(Node(d, i)) = heap.pop() {
        if d > dist[i] {
            continue;
        }
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        for (dx, dy, cost) in [(1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0), (1, 1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (-1, -1, 1.414)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let j = ny as usize * w + nx as usize;
            if !mask.get(nx as usize, ny as usize) {
                continue;
            }
            let nd = d + cost;
            if nd < dist[j] {
                dist[j] = nd;
                heap.push(Node(nd, j));
            }
        }
    }
    dist
}

/// For every pixel, the index of the nearest solid pixel (multi-source BFS).
fn nearest_solid(mask: &Mask) -> Vec<Option<usize>> {
    let (w, h) = (mask.width, mask.height);
    let mut near: Vec<Option<usize>> = vec![None; w * h];
    let mut queue = std::collections::VecDeque::new();
    for (i, n) in near.iter_mut().enumerate() {
        if mask.get(i % w, i / w) {
            *n = Some(i);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        for (nx, ny) in crate::mask::neighbors8(x, y, w, h) {
            let j = ny * w + nx;
            if near[j].is_none() {
                near[j] = near[i];
                queue.push_back(j);
            }
        }
    }
    near
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Length in pixels of the part of segment `a`-`b` that lies on solid
/// pixels: how much a bone actually runs through an image.
pub fn solid_length(mask: &Mask, a: [f32; 2], b: [f32; 2]) -> f32 {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let steps = len.ceil().max(1.0) as usize;
    let on = (0..=steps)
        .filter(|&s| {
            let t = s as f32 / steps as f32;
            let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            x >= 0.0 && y >= 0.0 && mask.get(x as usize, y as usize)
        })
        .count();
    len * on as f32 / (steps + 1) as f32
}

pub fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - abx * t).hypot(p[1] - a[1] - aby * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skel2d_core::MeshVertex;

    /// Two parallel bars joined only at the top: a "torso" and an "arm".
    #[test]
    fn distance_flows_around_gaps() {
        let mask = Mask::from_fn(100, 100, |x, y| (y < 10 && x < 60) || (x < 20) || (40..60).contains(&x));
        let place = Placement { origin: [0.0, 0.0], scale: 1.0 };
        let bones = [
            BoneSegment { name: "torso".into(), start: [10.0, 0.0], end: [10.0, 99.0] },
            BoneSegment { name: "arm".into(), start: [50.0, 0.0], end: [50.0, 99.0] },
        ];
        // A vertex on the torso's inner edge, 20px from the arm in a straight line.
        let mut mesh = Mesh {
            bones: Vec::new(),
            hull: 0, edges: Vec::new(),
            name: "m".into(),
            image: None,
            vertices: vec![MeshVertex { x: 19.0, y: -90.0, u: 0.0, v: 0.0, weights: vec![] }],
            triangles: vec![],
        };
        geodesic_weights(&mut mesh, &mask, &bones, place, 2, 3.0);
        let w = &mesh.vertices[0].weights;
        assert_eq!(w[0].bone, "torso", "{w:?}");
        assert!(w.iter().all(|x| x.bone != "arm" || x.weight < 0.05), "{w:?}");
    }
}

