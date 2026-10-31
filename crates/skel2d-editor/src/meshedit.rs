//! Mesh edits for the Mesh and Weights tools: inserting and removing
//! vertices, an image-fixed UV map for moving vertices, and weight painting.
//! Pure functions on core meshes (setup-pose positions); the viewport turns
//! the results into `set_attachment` / `set_vertices` ops.

#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use skel2d_core::{Mesh, MeshVertex, Weight};

#[cfg(test)]
type P = [f32; 2];

#[cfg(test)]
fn pos(v: &MeshVertex) -> P {
    [v.x, v.y]
}

#[cfg(test)]
fn orient(a: P, b: P, c: P) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}


pub use skel2d_core::mesh::UvMap;

pub use skel2d_core::mesh::{BrushMode, brush_weight, copy_weights};
#[cfg(test)]
use skel2d_core::mesh::{bones_used, smooth_weights, prune_weights, prune_count, swap_weights, set_weight, paste_weights, edges, neighbors};

// Regression adapters exercise the same core implementation as editor ops.
#[cfg(test)]
fn insert_vertex(mesh: &Mesh, p: P, tolerance: f32, bone: &str) -> Option<(Mesh, usize)> {
    let edit = skel2d_core::mesh::insert_vertex(mesh, p, tolerance, bone).ok()?;
    let index = edit.remap.iter().position(|parts| parts.len() > 1)?;
    Some((edit.mesh, index))
}
#[cfg(test)]
fn remove_vertex(mesh: &Mesh, index: usize) -> Option<Mesh> {
    skel2d_core::mesh::remove_vertices(mesh, &[index]).ok().map(|edit| edit.mesh)
}
#[cfg(test)]
fn remove_vertices(mesh: &Mesh, indices: &BTreeSet<usize>) -> Option<Mesh> {
    skel2d_core::mesh::remove_vertices(mesh, &indices.iter().copied().collect::<Vec<_>>()).ok().map(|edit| edit.mesh)
}
#[cfg(test)]
fn generate(mesh: &Mesh, bone: &str, count: usize) -> Mesh {
    skel2d_core::mesh::generate_interior(mesh, bone, Some(count), None).unwrap().mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Mesh {
        let v = |x: f32, y: f32| MeshVertex { x, y, u: x / 10.0, v: 1.0 - y / 10.0, weights: Vec::new() };
        Mesh { bones: Vec::new(), hull: 0, edges: Vec::new(), name: "m".into(), image: None, vertices: vec![v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)], triangles: vec![[0, 1, 2], [0, 2, 3]] }
    }

    fn area(m: &Mesh) -> f32 {
        m.triangles.iter().map(|t| orient(pos(&m.vertices[t[0] as usize]), pos(&m.vertices[t[1] as usize]), pos(&m.vertices[t[2] as usize])).abs() / 2.0).sum()
    }

    #[test]
    fn insert_inside_splits_triangle_and_keeps_uv() {
        let (m, i) = insert_vertex(&quad(), [7.0, 2.0], 0.1, "root").unwrap();
        assert_eq!(i, 4);
        assert_eq!(m.triangles.len(), 4);
        assert!((area(&m) - 100.0).abs() < 1e-3);
        assert!((m.vertices[4].u - 0.7).abs() < 1e-3 && (m.vertices[4].v - 0.8).abs() < 1e-3);
    }

    #[test]
    fn insert_on_edge_splits_both_sides() {
        let (m, _) = insert_vertex(&quad(), [5.0, 5.05], 0.5, "root").unwrap();
        assert_eq!(m.triangles.len(), 4);
        assert!((area(&m) - 100.0).abs() < 1e-3);
        assert!(insert_vertex(&quad(), [20.0, 5.0], 0.5, "root").is_none());
    }

    #[test]
    fn remove_refills_the_hole() {
        let (m, i) = insert_vertex(&quad(), [3.0, 6.0], 0.1, "root").unwrap();
        let back = remove_vertex(&m, i).unwrap();
        assert_eq!(back.vertices.len(), 4);
        assert_eq!(back.triangles.len(), 2);
        assert!((area(&back) - 100.0).abs() < 1e-3);
        // A boundary corner: the remaining triangle covers half the quad.
        let corner = remove_vertex(&quad(), 1).unwrap();
        assert_eq!(corner.triangles.len(), 1);
        assert!((area(&corner) - 50.0).abs() < 1e-3);
        assert!(remove_vertex(&corner, 0).is_none());
    }

    #[test]
    fn uv_map_reproduces_affine_uvs() {
        let map = UvMap::fit(&quad()).unwrap();
        assert_eq!(map.at([2.5, 7.5]), [0.25, 0.25]);
    }

    #[test]
    fn painting_keeps_weights_normalized() {
        let none = BTreeSet::new();
        let brush = |v: &MeshVertex, bone: &str, mode: BrushMode, amount: f32, fallback: Option<&str>| brush_weight(v, bone, mode, amount, 1.0, "root", &none, fallback);
        let v = MeshVertex { x: 0.0, y: 0.0, u: 0.0, v: 0.0, weights: Vec::new() };
        let w = brush(&v, "arm", BrushMode::Add, 0.25, None);
        assert_eq!(w.iter().map(|w| (w.bone.as_str(), w.weight)).collect::<Vec<_>>(), [("root", 0.75), ("arm", 0.25)]);
        let v2 = MeshVertex { weights: w, ..v.clone() };
        assert!(brush(&v2, "arm", BrushMode::Remove, 1.0, None).is_empty());
        // Only bone, removing weight: goes to the fallback.
        let w = brush(&v, "root", BrushMode::Remove, 0.4, Some("parent"));
        assert_eq!(w.iter().map(|w| (w.bone.as_str(), w.weight)).collect::<Vec<_>>(), [("root", 0.6), ("parent", 0.4)]);
        assert!(brush(&v, "root", BrushMode::Remove, 0.4, None).is_empty());
    }

    fn lock(bones: &[&str]) -> BTreeSet<String> {
        bones.iter().map(|b| b.to_string()).collect()
    }

    fn vw(ws: &[(&str, f32)]) -> MeshVertex {
        MeshVertex { x: 0.0, y: 0.0, u: 0.0, v: 0.0, weights: ws.iter().map(|(b, w)| Weight { bone: b.to_string(), weight: *w, offset: [0.0, 0.0] }).collect() }
    }

    fn wl(ws: &[Weight]) -> Vec<(&str, f32)> {
        ws.iter().map(|w| (w.bone.as_str(), w.weight)).collect()
    }

    #[test]
    fn edges_and_neighbors() {
        assert_eq!(edges(&quad()), vec![[0, 1], [0, 2], [0, 3], [1, 2], [2, 3]]);
        assert_eq!(neighbors(&quad()), vec![vec![1, 2, 3], vec![0, 2], vec![0, 1, 3], vec![0, 2]]);
    }

    #[test]
    fn bones_used_lists_sorted() {
        let mut m = quad();
        assert_eq!(bones_used(&m, "root"), ["root"]);
        m.vertices[0].weights = vw(&[("b", 0.5), ("a", 0.5)]).weights;
        assert_eq!(bones_used(&m, "root"), ["a", "b", "root"]);
    }

    #[test]
    fn brush_modes() {
        let none = BTreeSet::new();
        let v = vw(&[]);
        let w = brush_weight(&v, "arm", BrushMode::Add, 0.5, 0.3, "root", &none, None);
        assert_eq!(wl(&w), [("root", 0.7), ("arm", 0.3)]);
        let v2 = MeshVertex { weights: w, ..v.clone() };
        // Already at target: unchanged.
        assert_eq!(wl(&brush_weight(&v2, "arm", BrushMode::Add, 0.5, 0.3, "root", &none, None)), [("root", 0.7), ("arm", 0.3)]);
        assert_eq!(wl(&brush_weight(&v2, "arm", BrushMode::Replace, 0.1, 1.0, "root", &none, None)), [("root", 0.6), ("arm", 0.4)]);
        assert_eq!(wl(&brush_weight(&v2, "arm", BrushMode::Replace, 1.0, 0.1, "root", &none, None)), [("root", 0.9), ("arm", 0.1)]);
        assert_eq!(wl(&brush_weight(&v2, "arm", BrushMode::Remove, 0.1, 1.0, "root", &none, None)), [("root", 0.8), ("arm", 0.2)]);
        // Remove with nowhere to go, then with a fallback.
        assert!(brush_weight(&v, "root", BrushMode::Remove, 0.4, 1.0, "root", &none, None).is_empty());
        assert_eq!(wl(&brush_weight(&v, "root", BrushMode::Remove, 0.4, 1.0, "root", &none, Some("p"))), [("root", 0.6), ("p", 0.4)]);
    }

    #[test]
    fn locks_hold_weights() {
        let v = vw(&[("a", 0.5), ("b", 0.3), ("c", 0.2)]);
        assert_eq!(wl(&set_weight(&v, "c", 1.0, "root", &lock(&["a"]), None)), [("a", 0.5), ("c", 0.5)]);
        assert_eq!(wl(&set_weight(&v, "c", 0.3, "root", &lock(&["a"]), None)), [("a", 0.5), ("c", 0.3), ("b", 0.2)]);
        // The bone itself is locked.
        assert_eq!(wl(&set_weight(&v, "a", 0.0, "root", &lock(&["a"]), None)), [("a", 0.5), ("b", 0.3), ("c", 0.2)]);
        // A locked fallback is not used.
        assert!(set_weight(&vw(&[]), "root", 0.5, "root", &lock(&["p"]), Some("p")).is_empty());
    }

    #[test]
    fn smoothing_blends_neighbors_and_respects_locks() {
        let mut m = quad();
        m.vertices[0].weights = vw(&[("a", 1.0)]).weights;
        let s = smooth_weights(&m, "root", &[], &BTreeSet::new(), 1.0);
        // Vertex 1 neighbours 0 and 2: half a, half root.
        assert_eq!(wl(&s.vertices[1].weights), [("a", 0.5), ("root", 0.5)]);
        // Jacobi: vertex 0 averages the old root-only neighbours.
        assert!(s.vertices[0].weights.is_empty());
        let l = smooth_weights(&m, "root", &[], &lock(&["a"]), 1.0);
        assert_eq!(wl(&l.vertices[0].weights), [("a", 1.0)]);
        assert!(l.vertices[1].weights.is_empty());
        let o = smooth_weights(&m, "root", &[1], &BTreeSet::new(), 0.5);
        assert_eq!(wl(&o.vertices[1].weights), [("root", 0.75), ("a", 0.25)]);
        assert_eq!(o.vertices[2].weights, m.vertices[2].weights);
    }

    #[test]
    fn pruning() {
        let mut m = quad();
        m.vertices[0].weights = vw(&[("a", 0.5), ("b", 0.3), ("c", 0.15), ("d", 0.05)]).weights;
        assert_eq!(prune_count(&m, "root", &[], 2, 0.1, &BTreeSet::new()), 2);
        let p = prune_weights(&m, "root", &[], 2, 0.1, &BTreeSet::new());
        assert_eq!(wl(&p.vertices[0].weights), [("a", 0.625), ("b", 0.375)]);
        // A locked bone is kept and counts toward the limit.
        let p = prune_weights(&m, "root", &[], 2, 0.1, &lock(&["d"]));
        assert_eq!(wl(&p.vertices[0].weights), [("a", 0.95), ("d", 0.05)]);
        // A high threshold still keeps the heaviest bone.
        let p = prune_weights(&m, "root", &[0], 3, 0.9, &BTreeSet::new());
        assert_eq!(wl(&p.vertices[0].weights), [("a", 1.0)]);
    }

    #[test]
    fn swap_remove_many_generate_copy_paste() {
        let mut m = quad();
        m.vertices[0].weights = vw(&[("a", 0.75), ("b", 0.25)]).weights;
        let s = swap_weights(&m, "root", &[], "a", "b");
        assert_eq!(wl(&s.vertices[0].weights), [("b", 0.75), ("a", 0.25)]);
        assert!(s.vertices[1].weights.is_empty());
        let s = swap_weights(&m, "root", &[], "a", "root");
        assert_eq!(wl(&s.vertices[1].weights), [("a", 1.0)]);

        let g = generate(&quad(), "root", 3);
        assert_eq!(g.vertices.len(), 7);
        assert!((area(&g) - 100.0).abs() < 1e-3);
        for vertex in &quad().vertices { assert!(g.vertices.contains(vertex)); }

        let back = remove_vertices(&g, &BTreeSet::from([4, 5, 6])).unwrap();
        assert_eq!(back.vertices.len(), 4);
        assert!((area(&back) - 100.0).abs() < 1e-3);
        assert!(remove_vertices(&quad(), &BTreeSet::from([0, 1, 2])).is_none());

        let mut c = quad();
        c.vertices[0].weights = vw(&[("a", 1.0)]).weights;
        let copied = copy_weights(&c, &[0, 1], "root");
        assert_eq!(copied, BTreeMap::from([("a".to_string(), 0.5), ("root".to_string(), 0.5)]));
        let p = paste_weights(&c, &[2, 3], &copied, "root");
        assert_eq!(wl(&p.vertices[3].weights), [("a", 0.5), ("root", 0.5)]);
        assert_eq!(p.vertices[1].weights, c.vertices[1].weights);
    }
}
