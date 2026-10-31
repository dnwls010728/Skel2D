use serde_json::{Value, json};
use skel2d_core::mesh;
use skel2d_core::{
    Attachment, Document, Mesh, Op, Pose, apply, from_str, parse_ops, to_string, validate,
};

fn vertex(x: f32, y: f32) -> Value {
    json!({"x":x,"y":y,"u":x/10.0,"v":1.0-y/10.0,"weights":[{"bone":"root","weight":1.0-x/10.0},{"bone":"tip","weight":x/10.0}]})
}

fn shape(points: &[[f32; 2]], triangles: &[[u32; 3]]) -> Value {
    json!({"type":"mesh","name":"cloth","vertices":points.iter().map(|p|vertex(p[0],p[1])).collect::<Vec<_>>(),"triangles":triangles})
}

fn document() -> Document {
    let quad = shape(
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        &[[0, 1, 2], [0, 2, 3]],
    );
    let mut armor = quad.clone();
    armor["vertices"]
        .as_array_mut()
        .unwrap()
        .push(vertex(5.0, 5.0));
    armor["triangles"] = json!([[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]]);
    from_str(&json!({"bones":[{"name":"root"},{"name":"tip"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[quad]}],"skins":[{"name":"armor","attachments":{"body":[armor]}}]}).to_string()).unwrap()
}

fn edit(doc: &Document, ops: Value) -> Document {
    apply(doc, &parse_ops(&ops.to_string()).unwrap()).unwrap()
}
fn get<'a>(doc: &'a Document, skin: Option<&str>) -> &'a Mesh {
    let attachment = match skin {
        None => doc.slots[0].attachment("cloth"),
        Some(k) => doc.skin(k).unwrap().attachment("body", "cloth"),
    }
    .unwrap();
    let Attachment::Mesh(m) = attachment else {
        panic!()
    };
    m
}
fn offsets(m: &Mesh) -> Vec<[f32; 2]> {
    m.vertices
        .iter()
        .map(|v| [2.0 * v.x + v.y, v.y - v.x])
        .collect()
}
fn keyed() -> Document {
    let doc = document();
    edit(
        &doc,
        json!([
            {"op":"key_deform","animation":"bend","slot":"body","attachment":"cloth","time":0,"offsets":offsets(get(&doc,None))},
            {"op":"key_deform","animation":"bend","slot":"body","attachment":"cloth","skin":"armor","time":0,"offsets":offsets(get(&doc,Some("armor")))}
        ]),
    )
}
fn area(m: &Mesh) -> f32 {
    m.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| &m.vertices[i as usize]);
            ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5
        })
        .sum()
}
fn assert_keys(doc: &Document, skin: &str) {
    let m = get(doc, (skin != "default").then_some(skin));
    let keys = &doc.animations["bend"].deform[skin]["body"]["cloth"];
    assert_eq!(keys[0].value.len(), m.vertices.len());
    for (a, b) in keys[0].value.iter().zip(offsets(m)) {
        for i in 0..2 {
            assert!((a[i] - b[i]).abs() < 0.001, "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn legacy_metadata_defaults_never_reorder_vertices_on_load() {
    let doc = document();
    let m = get(&doc, None);
    assert_eq!(m.hull, 0);
    assert!(m.edges.is_empty());
    assert_eq!(
        m.vertices.iter().map(|v| [v.x, v.y]).collect::<Vec<_>>(),
        [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    );
    let text = to_string(&doc);
    assert!(!text.contains("\"hull\""));
    assert!(!text.contains("\"edges\""));
    assert_eq!(from_str(&text).unwrap(), doc);
    assert_eq!(
        Pose::compute(&from_str(&text).unwrap(), None),
        Pose::compute(&doc, None)
    );
}

#[test]
fn constrained_delaunay_keeps_edges_and_is_deterministic() {
    let doc = edit(
        &keyed(),
        json!({"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[1,3]]}),
    );
    let m = get(&doc, None);
    assert_eq!(m.hull, 4);
    assert_eq!(m.edges, [[1, 3]]);
    assert!(mesh::edges(m).contains(&[1, 3]));
    assert!((area(m) - 100.0).abs() < 0.001);
    assert_eq!(mesh::triangulate(m).unwrap().mesh, *m);
    assert_eq!(
        mesh::triangulate(m).unwrap().mesh,
        mesh::triangulate(m).unwrap().mesh
    );
    assert_keys(&doc, "default");
    assert_keys(&doc, "armor");
}

#[test]
fn concave_hull_clips_triangles_and_rejects_a_constraint_across_the_notch() {
    let mut doc = document();
    let attachment = shape(
        &[
            [0.0, 0.0],
            [6.0, 0.0],
            [6.0, 2.0],
            [2.0, 2.0],
            [2.0, 6.0],
            [0.0, 6.0],
        ],
        &[[0, 1, 3], [1, 2, 3], [0, 3, 5], [3, 4, 5]],
    );
    doc = edit(
        &doc,
        json!({"op":"set_attachment","slot":"body","attachment":attachment}),
    );
    let result = mesh::generate_interior(get(&doc, None), "root", Some(5), None).unwrap();
    assert_eq!(result.mesh.hull, 6);
    assert!((area(&result.mesh) - 20.0).abs() < 0.001);
    for t in &result.mesh.triangles {
        let points = t.map(|i| &result.mesh.vertices[i as usize]);
        let center = [
            points.iter().map(|v| v.x).sum::<f32>() / 3.0,
            points.iter().map(|v| v.y).sum::<f32>() / 3.0,
        ];
        assert!(center[0] <= 2.0 || center[1] <= 2.0);
    }
    let op =
        parse_ops(r#"{"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[2,4]]}"#)
            .unwrap();
    assert!(apply(&doc, &op).is_err());
}

#[test]
fn holes_and_disconnected_legacy_islands_are_not_filled() {
    let hole = shape(
        &[
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [3.0, 3.0],
            [7.0, 3.0],
            [7.0, 7.0],
            [3.0, 7.0],
        ],
        &[
            [0, 1, 4],
            [1, 5, 4],
            [1, 2, 5],
            [2, 6, 5],
            [2, 3, 6],
            [3, 7, 6],
            [3, 0, 7],
            [0, 4, 7],
        ],
    );
    let doc = edit(
        &document(),
        json!({"op":"set_attachment","slot":"body","attachment":hole}),
    );
    let generated = mesh::generate_interior(get(&doc, None), "root", Some(5), None)
        .unwrap()
        .mesh;
    assert_eq!(generated.hull, 4);
    assert!((area(&generated) - 84.0).abs() < 0.001);
    assert_eq!(mesh::boundary_loops(&generated).unwrap().len(), 2);
    let islands = shape(
        &[
            [0.0, 0.0],
            [2.0, 0.0],
            [0.0, 2.0],
            [6.0, 0.0],
            [8.0, 0.0],
            [6.0, 2.0],
        ],
        &[[0, 1, 2], [3, 4, 5]],
    );
    let doc = edit(
        &document(),
        json!({"op":"set_attachment","slot":"body","attachment":islands}),
    );
    let generated = mesh::generate_interior(get(&doc, None), "root", Some(3), None)
        .unwrap()
        .mesh;
    assert_eq!(generated.hull, 0); // No single outer loop represents disjoint islands.
    assert!((area(&generated) - 4.0).abs() < 0.001);
    assert_eq!(mesh::boundary_loops(&generated).unwrap().len(), 2);
}

#[test]
fn insertion_interpolates_weights_uvs_and_deform_with_hull_reordering() {
    let doc = keyed();
    let armor = doc.animations["bend"].deform["armor"].clone();
    let next = edit(
        &doc,
        json!({"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[3,2],[7,6],[5,0]],"on_edge_tolerance":0.01}),
    );
    let m = get(&next, None);
    assert_eq!(m.vertices.len(), 7);
    assert_eq!(m.hull, 5);
    assert_eq!(next.animations["bend"].deform["armor"], armor);
    assert_keys(&next, "default");
    for v in &m.vertices {
        assert!((v.u - v.x / 10.0).abs() < 0.001);
        assert!((v.v - (1.0 - v.y / 10.0)).abs() < 0.001);
        let w = v
            .weights
            .iter()
            .find(|w| w.bone == "tip")
            .map_or(0.0, |w| w.weight);
        assert!((w - v.x / 10.0).abs() < 0.001);
    }
    let pose = Pose::compute(&next, Some((&next.animations["bend"], 0.0)));
    for (i, p) in pose.deform_mesh(&next, m, 0).iter().enumerate() {
        assert!((p[0] - (3.0 * m.vertices[i].x + m.vertices[i].y)).abs() < 0.001);
    }
    let index = m
        .vertices
        .iter()
        .position(|v| v.x == 3.0 && v.y == 2.0)
        .unwrap();
    let removed = edit(
        &next,
        json!({"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[index]}),
    );
    assert_keys(&removed, "default");
    assert_eq!(get(&removed, None).vertices.len(), 6);
    assert!(
        get(&removed, None)
            .vertices
            .iter()
            .any(|v| v.x == 7.0 && v.y == 6.0)
    );
}

#[test]
fn removing_a_boundary_corner_promotes_surviving_fan_vertices_to_the_hull() {
    let doc = edit(
        &keyed(),
        json!({"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[2,2]]}),
    );
    let corner = get(&doc, None)
        .vertices
        .iter()
        .position(|v| v.x == 0.0 && v.y == 0.0)
        .unwrap();
    let next = edit(
        &doc,
        json!({"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[corner]}),
    );
    let mesh = get(&next, None);
    assert_eq!(mesh.vertices.len(), 4);
    assert_eq!(mesh.hull, 4);
    assert!((area(mesh) - 80.0).abs() < 0.001);
    assert!(mesh.vertices.iter().any(|v| v.x == 2.0 && v.y == 2.0));
    assert_keys(&next, "default");
}

#[test]
fn splitting_a_constraint_remaps_its_endpoints_and_deletion_drops_incident_edges() {
    let doc = edit(
        &keyed(),
        json!([
            {"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[0,2]]},
            {"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[5,5]],"on_edge_tolerance":0.1}
        ]),
    );
    let m = get(&doc, None);
    assert_eq!(m.edges.len(), 2);
    for e in &m.edges {
        assert!(mesh::edges(m).contains(e));
    }
    assert_keys(&doc, "default");
    let center = m
        .vertices
        .iter()
        .position(|v| v.x == 5.0 && v.y == 5.0)
        .unwrap();
    let removed = edit(
        &doc,
        json!({"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[center]}),
    );
    assert!(get(&removed, None).edges.is_empty());
    assert_keys(&removed, "default");
}

#[test]
fn generation_spacing_count_and_skin_targeting_preserve_keys() {
    let doc = keyed();
    for (extra, expected) in [(json!({"count":3}), 8), (json!({"spacing":2.5}), 13)] {
        let mut op = json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","skin":"armor"});
        op.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let next = edit(&doc, op);
        assert_eq!(get(&next, Some("armor")).vertices.len(), expected);
        assert_eq!(get(&next, None), get(&doc, None));
        assert_keys(&next, "armor");
    }
}

#[test]
fn uv_resets_preserve_weights_and_deformation_keys() {
    let doc = edit(
        &keyed(),
        json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","count":2}),
    );
    let original = get(&doc, None);
    for mode in ["vertices_to_uvs", "uvs_to_vertices"] {
        let next = edit(
            &doc,
            json!({"op":"reset_mesh","slot":"body","attachment":"cloth","mode":mode}),
        );
        assert_eq!(next.animations, doc.animations);
        for (a, b) in get(&next, None).vertices.iter().zip(&original.vertices) {
            assert_eq!(a.weights, b.weights);
        }
        assert!((area(get(&next, None)) - 100.0).abs() < 0.01);
    }
    let mut degenerate = doc.clone();
    let Attachment::Mesh(m) = &mut degenerate.slots[0].attachments[0] else {
        panic!()
    };
    for v in &mut m.vertices {
        v.u = 0.0;
        v.v = 0.0;
    }
    let ops = parse_ops(
        r#"{"op":"reset_mesh","slot":"body","attachment":"cloth","mode":"vertices_to_uvs"}"#,
    )
    .unwrap();
    assert!(apply(&degenerate, &ops).is_err());
}

#[test]
fn uv_resets_correct_non_affine_edits_using_the_documented_affine_fit() {
    let doc = edit(
        &document(),
        json!({"op":"set_attachment","slot":"body","attachment":shape(
        &[[0.0,0.0],[10.0,0.0],[10.0,10.0],[0.0,10.0],[5.0,5.0]],
        &[[0,1,4],[1,2,4],[2,3,4],[3,0,4]])}),
    );
    let mut mesh = get(&doc, None).clone();
    mesh.vertices[4].x = 7.0; // An interior vertex moved while its UV stayed fixed.
    let restored = mesh::reset_to_uvs(&mesh).unwrap().mesh;
    let center = restored
        .vertices
        .iter()
        .find(|v| (v.u - 0.5).abs() < 0.001 && (v.v - 0.5).abs() < 0.001)
        .unwrap();
    // UV-to-X least squares: x=10*u+0.4 across these five samples.
    assert!((center.x - 5.4).abs() < 0.001);
    assert!((center.y - 5.0).abs() < 0.001);
    let fitted = mesh::reset_uvs(&mesh).unwrap().mesh;
    // X-to-U least squares: u=(10/103.2)*x-0.023255814.
    assert!((fitted.vertices[4].u - 0.655).abs() < 0.0001);
    assert_eq!(fitted.vertices[4].x, 7.0);
    for (a, b) in fitted.vertices.iter().zip(&mesh.vertices) {
        assert_eq!(a.weights, b.weights);
    }
}

#[test]
fn bad_topology_ops_fail_atomically_and_bad_metadata_is_rejected() {
    let doc = keyed();
    let before = doc.clone();
    for op in [
        json!({"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[20,20]]}),
        json!({"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[0,0]]}),
        json!({"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[1,1]],"on_edge_tolerance":-1}),
        json!({"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[90]}),
        json!({"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[0,1]}),
        json!({"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[0,2],[1,3]]}),
        json!({"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[0,0]]}),
        json!({"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[0,99]]}),
        json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","count":1,"spacing":1}),
        json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","spacing":0}),
        json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","spacing":0.00001}),
    ] {
        let ops = parse_ops(&json!([{"op":"set_name","name":"roll back"},op]).to_string()).unwrap();
        assert!(apply(&doc, &ops).is_err());
        assert_eq!(doc, before);
    }
    let normalized = edit(
        &doc,
        json!({"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[]}),
    );
    for bad in ["small", "large", "range", "duplicate", "absent"] {
        let mut next = normalized.clone();
        let Attachment::Mesh(m) = &mut next.slots[0].attachments[0] else {
            panic!()
        };
        match bad {
            "small" => m.hull = 2,
            "large" => m.hull = 99,
            "range" => m.edges = vec![[0, 99]],
            "duplicate" => m.edges = vec![[0, 1], [1, 0]],
            _ => m.edges = vec![[1, 3], [0, 2]],
        }
        assert!(!validate(&next).is_empty(), "{bad}");
    }
    let nonfinite = Op::AddMeshVertices {
        slot: "body".into(),
        attachment: "cloth".into(),
        skin: None,
        points: vec![[f32::NAN, 1.0]],
        on_edge_tolerance: 0.0,
    };
    assert!(apply(&doc, &[nonfinite]).is_err());
}

#[test]
fn names_and_deletion_still_clean_paths_after_topology_edits() {
    let next = edit(
        &keyed(),
        json!([
            {"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","skin":"armor","count":2},
            {"op":"set_skin","name":"armor","rename":"metal"},
            {"op":"set_slot","name":"body","rename":"torso"},
            {"op":"rename_placeholder","slot":"torso","from":"cloth","to":"fabric"}
        ]),
    );
    assert_eq!(
        next.animations["bend"].deform["metal"]["torso"]["fabric"][0]
            .value
            .len(),
        7
    );
    let removed = edit(
        &next,
        json!({"op":"remove_skin_attachment","skin":"metal","slot":"torso","name":"fabric"}),
    );
    assert!(!removed.animations["bend"].deform.contains_key("metal"));
    assert!(validate(&removed).is_empty());
}

fn notched() -> Document {
    let points = [[0.0, 0.0], [0.0, 10.0], [4.0, 10.0], [5.0, 5.0], [6.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
    let triangles = [[0, 1, 2], [0, 2, 3], [0, 3, 6], [3, 4, 5], [3, 5, 6]];
    let mut cloth = shape(&points, &triangles);
    cloth["hull"] = json!(7);
    from_str(&json!({"bones":[{"name":"root"},{"name":"tip"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[cloth]}]}).to_string()).unwrap()
}

fn moved(index: usize, to: [f32; 2]) -> Value {
    json!([{"op":"set_vertices","slot":"body","attachment":"cloth","vertices":[{"index":index,"x":to[0],"y":to[1]}]}])
}

#[test]
fn moving_a_hull_vertex_refits_the_triangles_and_keeps_vertex_indices() {
    let before = notched();
    // The old triangle 0, 1, 2 would now lie outside the hull.
    let after = edit(&before, moved(1, [3.0, 3.0]));
    assert!(validate(&after).is_empty());
    let (old, new) = (get(&before, None), get(&after, None));
    assert_ne!(old.triangles, new.triangles);
    assert_eq!(new.hull, 7);
    for (i, (a, b)) in old.vertices.iter().zip(&new.vertices).enumerate() {
        assert_eq!((a.u, a.v, &a.weights), (b.u, b.v, &b.weights), "vertex {i}");
    }
    // The triangles tile the hull exactly: no overlap and no gap.
    let at = |i: u32| { let v = &new.vertices[i as usize]; (v.x, v.y) };
    let hull: f32 = (0..7).map(|i| { let (a, b) = (at(i), at((i + 1) % 7)); a.0 * b.1 - a.1 * b.0 }).sum::<f32>().abs() / 2.0;
    let tiled: f32 = new.triangles.iter().map(|t| { let (a, b, c) = (at(t[0]), at(t[1]), at(t[2])); ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0 }).sum();
    assert!((hull - tiled).abs() < 1e-3, "{hull} {tiled}");
}

#[test]
fn a_tangling_vertex_move_is_allowed_and_keeps_the_last_triangles() {
    let doc = notched();
    let tangled = edit(&doc, moved(2, [9.0, 9.0]));
    assert!(validate(&tangled).is_empty());
    let mesh = get(&tangled, None);
    assert_eq!(mesh.triangles, get(&doc, None).triangles);
    let problem = skel2d_core::mesh::geometry_problem(mesh).unwrap();
    assert!(problem.contains("hull crosses itself"), "{problem}");
    // It survives a save, and only ops that rebuild the triangles refuse.
    assert_eq!(from_str(&skel2d_core::to_string(&tangled)).unwrap(), tangled);
    let add = json!({"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","count":1});
    assert!(apply(&tangled, &parse_ops(&add.to_string()).unwrap()).unwrap_err().to_string().contains("hull crosses itself"));
    // Untangling goes back to the original triangles.
    let fixed = edit(&tangled, moved(2, [4.0, 10.0]));
    assert!(skel2d_core::mesh::geometry_problem(get(&fixed, None)).is_none());
}

#[test]
fn an_interior_vertex_may_leave_the_hull() {
    let doc = from_str(&json!({"bones":[{"name":"root"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[{
        "type":"mesh","name":"cloth","hull":4,"triangles":[[0,1,4],[1,2,4],[2,3,4],[3,0,4]],
        "vertices":[{"x":0,"y":0,"u":0,"v":0},{"x":0,"y":10,"u":0,"v":1},{"x":10,"y":10,"u":1,"v":1},{"x":10,"y":0,"u":1,"v":0},{"x":5,"y":5,"u":0.5,"v":0.5}]}]}]}).to_string()).unwrap();
    let out = edit(&doc, moved(4, [50.0, 50.0]));
    assert!(validate(&out).is_empty());
    assert!(skel2d_core::mesh::geometry_problem(get(&out, None)).is_some());
}

#[test]
fn edits_that_do_not_move_vertices_keep_the_triangles() {
    let doc = notched();
    let same = edit(&doc, json!([{"op":"set_vertices","slot":"body","attachment":"cloth","vertices":[{"index":3,"x":5.0,"y":5.0,"u":0.1}]}]));
    assert_eq!(get(&doc, None).triangles, get(&same, None).triangles);
}

#[test]
fn a_batch_may_pass_through_an_impossible_layout() {
    let doc = notched();
    let ops = json!([
        {"op":"set_vertices","slot":"body","attachment":"cloth","vertices":[{"index":2,"x":9.0,"y":9.0}]},
        {"op":"set_vertices","slot":"body","attachment":"cloth","vertices":[{"index":2,"x":3.0,"y":9.0}]}
    ]);
    let after = edit(&doc, ops);
    assert!(validate(&after).is_empty());
}

#[test]
fn moving_along_a_long_straight_side_stays_valid() {
    // A rope: many nearly collinear outline points far from the origin.
    let side = 25;
    let at = |i: usize| 1600.0 - i as f32 * 49.2037;
    let mut points: Vec<[f32; 2]> = (0..side).rev().map(|i| [-12.3457, at(i)]).collect();
    points.extend((0..side).map(|i| [12.6543, at(i)]));
    let mut triangles = Vec::new();
    for i in 0..side as u32 - 1 {
        let (a, b, c, d) = (i, i + 1, 2 * side as u32 - 2 - i, 2 * side as u32 - 1 - i);
        triangles.extend([[a, b, c], [a, c, d]]);
    }
    let vertices: Vec<_> = points.iter().map(|p| json!({"x":p[0],"y":p[1],"u":0,"v":0})).collect();
    let cloth = json!({"type":"mesh","name":"cloth","hull":2 * side,"vertices":vertices,"triangles":triangles});
    let doc = from_str(&json!({"bones":[{"name":"root"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[cloth]}]}).to_string()).unwrap();
    for index in 0..2 * side {
        let [x, y] = points[index];
        for to in [[x, y + 12.0], [x, y - 12.0]] {
            let ops = parse_ops(&moved(index, to).to_string()).unwrap();
            assert!(apply(&doc, &ops).is_ok(), "{index} {to:?}");
        }
    }
}
