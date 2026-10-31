use serde_json::{Value, json};
use skel2d_core::{Attachment, Document, Op, Pose};
use skel2d_rig::{AnyOp, AssetOp, TraceParams, apply_any, parse_ops};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

fn directory() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "skel2d-assets-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn fixture() -> (Document, PathBuf) {
    let dir = directory();
    let mut pm = tiny_skia::Pixmap::new(96, 64).unwrap();
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(180, 100, 80, 255);
    pm.fill_rect(
        tiny_skia::Rect::from_xywh(8.0, 8.0, 80.0, 48.0).unwrap(),
        &paint,
        tiny_skia::Transform::identity(),
        None,
    );
    pm.save_png(dir.join("body.png")).unwrap();
    let doc=skel2d_core::apply(&Document::new("assets"),&skel2d_core::parse_ops(r#"[
        {"op":"add_bone","name":"root","length":30},
        {"op":"add_bone","name":"other","x":20,"length":20},
        {"op":"add_slot","name":"body","bone":"root","attachment":"image","attachments":[{"type":"region","name":"image","image":"body.png","width":96,"height":64}]}
    ]"#).unwrap()).unwrap();
    (doc, dir)
}
fn run(doc: &Document, dir: &Path, ops: Value) -> skel2d_rig::ApplyReport {
    apply_any(doc, dir, &parse_ops(&ops.to_string()).unwrap()).unwrap()
}
fn mesh(doc: &Document) -> &skel2d_core::Mesh {
    let Attachment::Mesh(m) = &doc.slots[0].attachments[0] else {
        panic!()
    };
    m
}

#[test]
fn weight_ops_mix_with_assets_keep_locks_and_report_pruning() {
    let (doc,dir)=fixture();
    let report=run(&doc,&dir,json!([
        {"op":"trace_mesh","slot":"body","attachment":"image","interior":"none"},
        {"op":"set_bone_weight","slot":"body","attachment":"image","vertices":[0,1,2,3],"bone":"other","mode":"set","value":0.7},
        {"op":"bind_bones","slot":"body","attachment":"image","add":["other"],"redistribute":"auto"},
        {"op":"image_weights","slot":"body","attachment":"image","bones":["other"],"max_influences":1,"locked_bones":["root"]}
    ]));
    for v in &mesh(&report.document).vertices {
        let w=skel2d_core::mesh::weight_map(v,"root");
        assert!((w["root"]-0.3).abs()<0.0001);
        assert!((w["other"]-0.7).abs()<0.0001);
    }
    let pruned=run(&report.document,&dir,json!({"op":"prune_weights","slot":"body","attachment":"image","max_bones":1}));
    assert_eq!(pruned.pruned_influences,4);assert_eq!(pruned.meshes.len(),1);
    let empty=run(&report.document,&dir,json!({"op":"prune_weights","slot":"body","attachment":"image","max_bones":1,"vertices":[]}));
    assert_eq!(empty.pruned_influences,0);assert_eq!(empty.document,report.document);
    let replay=skel2d_core::apply(&doc,&report.ops).unwrap();assert_eq!(replay,report.document);
    std::fs::remove_file(dir.join("body.png")).unwrap();
    assert_eq!(skel2d_core::apply(&doc,&report.ops).unwrap(),report.document);
}

#[test]
fn flattened_defaults_strict_dispatch_and_schema_namespaces() {
    let ops=parse_ops(r#"[{"op":"set_name","name":"x"},{"op":"trace_mesh","slot":"body","attachment":"image"},{"op":"image_weights","slot":"body","attachment":"image"}]"#).unwrap();
    let AnyOp::Asset(AssetOp::TraceMesh { params, .. }) = &ops[1] else {
        panic!()
    };
    assert_eq!(*params, TraceParams::default());
    for op in &ops {
        assert_eq!(
            parse_ops(&serde_json::to_string(op).unwrap()).unwrap(),
            vec![op.clone()]
        );
    }
    assert!(
        parse_ops(r#"{"op":"trace_mesh","slot":"body","attachment":"image","typo":2}"#).is_err()
    );
    assert!(parse_ops(r#"{"op":"not_an_op"}"#).is_err());
    fn tags(v: &Value, out: &mut std::collections::BTreeSet<String>) {
        if let Some(op) = v.get("properties").and_then(|p| p.get("op")) {
            if let Some(s) = op.get("const").and_then(Value::as_str) {
                out.insert(s.into());
            }
            for s in op
                .get("enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                out.insert(s.into());
            }
        }
        match v {
            Value::Object(m) => {
                for value in m.values() {
                    tags(value, out)
                }
            }
            Value::Array(a) => {
                for value in a {
                    tags(value, out)
                }
            }
            _ => {}
        }
    }
    let (mut core, mut asset) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    tags(
        &serde_json::to_value(schemars::schema_for!(Op)).unwrap(),
        &mut core,
    );
    tags(
        &serde_json::to_value(schemars::schema_for!(AssetOp)).unwrap(),
        &mut asset,
    );
    assert!(!core.is_empty());
    assert_eq!(asset.len(), 2);
    assert!(core.is_disjoint(&asset));
    let schema = serde_json::to_string(&schemars::schema_for!(Vec<AnyOp>)).unwrap();
    for tag in core.union(&asset) {
        assert!(schema.contains(tag));
    }
}

#[test]
fn mixed_batches_are_sequential_atomic_and_materialized_ops_replay_without_images() {
    let (doc, dir) = fixture();
    let result = run(
        &doc,
        &dir,
        json!([
            {"op":"set_name","name":"mixed"},
            {"op":"trace_mesh","slot":"body","attachment":"image","interior":"grid","spacing":12},
            {"op":"image_weights","slot":"body","attachment":"image","bones":["root"],"max_influences":1},
            {"op":"add_mesh_vertices","slot":"body","attachment":"image","points":[[1,2]]}
        ]),
    );
    assert_eq!(result.meshes.len(), 2);
    assert!(result.ops.len() >= 4);
    assert_eq!(result.document.name, "mixed");
    assert!(mesh(&result.document).hull >= 3);
    assert!(skel2d_core::validate(&result.document).is_empty());
    std::fs::rename(dir.join("body.png"), dir.join("changed.png")).unwrap();
    assert_eq!(
        skel2d_core::apply(&doc, &result.ops).unwrap(),
        result.document
    );
    let before = doc.clone();
    assert!(apply_any(&doc,&dir,&parse_ops(r#"[{"op":"set_name","name":"rollback"},{"op":"trace_mesh","slot":"body","attachment":"image"}]"#).unwrap()).is_err());
    assert_eq!(doc, before);
    // Pure core batches keep core atomic semantics, including unfinished
    // intermediate slot references completed by a later core op.
    let transient = json!([{"op":"add_slot","name":"later","bone":"root","attachment":"r"},{"op":"set_attachment","slot":"later","attachment":{"type":"region","name":"r","width":2,"height":2}}]);
    let core = skel2d_core::apply(
        &doc,
        &skel2d_core::parse_ops(&transient.to_string()).unwrap(),
    )
    .unwrap();
    assert_eq!(run(&doc, &dir, transient).document, core);
}

#[test]
fn every_interior_mode_is_deterministic_and_hull_respects_alpha_padding() {
    let (doc, dir) = fixture();
    for interior in ["none", "grid", "auto"] {
        let ops = json!({"op":"trace_mesh","slot":"body","attachment":"image","interior":interior,"spacing":12,"padding":2,"detail":0.5});
        let first = run(&doc, &dir, ops.clone());
        let second = run(&doc, &dir, ops);
        assert_eq!(
            skel2d_core::to_string(&first.document),
            skel2d_core::to_string(&second.document)
        );
        let m = mesh(&first.document);
        assert!(m.hull >= 3);
        if interior == "none" {
            assert_eq!(m.hull as usize, m.vertices.len());
        }
        if interior == "grid" {
            assert!(m.vertices.len() > m.hull as usize);
        }
        for v in &m.vertices[..m.hull as usize] {
            let (px, py) = (v.u * 96.0, v.v * 64.0);
            assert!((5.99..=90.01).contains(&px), "{px}");
            assert!((5.99..=58.01).contains(&py), "{py}");
        }
    }
}

#[test]
fn retrace_preserves_weights_constraints_and_each_deform_curve() {
    let (doc, dir) = fixture();
    let mut doc = skel2d_core::apply(
        &doc,
        &skel2d_core::parse_ops(
            r#"{"op":"mesh_from_region","slot":"body","region":"image","cols":3,"rows":3}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let offsets: Vec<_> = mesh(&doc)
        .vertices
        .iter()
        .map(|v| [v.x * 0.1, v.y * 0.2])
        .collect();
    doc = skel2d_core::apply(
        &doc,
        &[Op::KeyDeform {
            animation: "bend".into(),
            slot: "body".into(),
            attachment: "image".into(),
            skin: None,
            time: 0.0,
            offsets,
            curve: Some(skel2d_core::Curve::Stepped),
        }],
    )
    .unwrap();
    let (a, b) = {
        let m = mesh(&doc);
        let mut inner = m
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.x.abs() < 20.0 && v.y.abs() < 20.0);
        (inner.next().unwrap().0, inner.next().unwrap().0)
    };
    doc=run(&doc,&dir,json!([
        {"op":"auto_weights","slot":"body","attachment":"image","bones":["other"],"max_influences":1},
        {"op":"set_mesh_edges","slot":"body","attachment":"image","edges":[[a,b]]}
    ])).document;
    let result = run(
        &doc,
        &dir,
        json!({"op":"trace_mesh","slot":"body","attachment":"image","interior":"grid","spacing":10}),
    );
    let m = mesh(&result.document);
    assert!(!m.edges.is_empty());
    assert!(m.vertices.iter().all(|v| {
        v.weights
            .iter()
            .any(|w| w.bone == "other" && w.weight == 1.0)
    }));
    let keys = &result.document.animations["bend"].deform["default"]["body"]["image"];
    assert_eq!(keys[0].curve, skel2d_core::Curve::Stepped);
    assert_eq!(keys[0].value.len(), m.vertices.len());
    for (v, k) in m.vertices.iter().zip(&keys[0].value) {
        assert!((k[0] - v.x * 0.1).abs() < 0.001);
        assert!((k[1] - v.y * 0.2).abs() < 0.001);
    }
}

#[test]
fn selected_image_weights_and_bad_parameters_leave_other_vertices_unchanged() {
    let (doc, dir) = fixture();
    let doc=run(&doc,&dir,json!({"op":"trace_mesh","slot":"body","attachment":"image","interior":"grid","spacing":12})).document;
    let result = run(
        &doc,
        &dir,
        json!({"op":"image_weights","slot":"body","attachment":"image","bones":["other"],"max_influences":1,"vertices":[0]}),
    );
    assert_eq!(mesh(&result.document).vertices[0].weights[0].bone, "other");
    for i in 1..mesh(&doc).vertices.len() {
        assert_eq!(mesh(&doc).vertices[i], mesh(&result.document).vertices[i]);
    }
    for op in [
        json!({"op":"trace_mesh","slot":"body","attachment":"image","padding":-1}),
        json!({"op":"trace_mesh","slot":"body","attachment":"image","detail":2}),
        json!({"op":"trace_mesh","slot":"body","attachment":"image","alpha_threshold":1}),
        json!({"op":"trace_mesh","slot":"body","attachment":"image","spacing":0}),
        json!({"op":"image_weights","slot":"body","attachment":"image","max_influences":0}),
        json!({"op":"image_weights","slot":"body","attachment":"image","falloff":0}),
        json!({"op":"image_weights","slot":"body","attachment":"image","bones":["missing"]}),
        json!({"op":"image_weights","slot":"body","attachment":"image","vertices":[999999]}),
    ] {
        assert!(apply_any(&doc, &dir, &parse_ops(&op.to_string()).unwrap()).is_err());
    }
}

#[test]
fn existing_leg_geodesic_regression_stays_on_its_own_bones() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/miya");
    let doc = skel2d_core::load(base.join("miya.skel.json")).unwrap();
    for (slot, own) in [
        ("LL", ["thigh_r", "shin_r", "foot_r"]),
        ("RL", ["thigh_l", "shin_l", "foot_l"]),
    ] {
        let Attachment::Mesh(m) = &doc.slot(slot).unwrap().attachments[0] else {
            panic!()
        };
        let weighted =
            skel2d_rig::asset_ops::image_weight_mesh(&doc, &base, m, None, 4, 3.0, None).unwrap();
        for v in &weighted.vertices {
            for w in &v.weights {
                assert!(
                    own.contains(&w.bone.as_str()) || w.bone == "hip",
                    "{slot}: {}",
                    w.bone
                );
            }
        }
    }
    let _ = Pose::compute(&doc, None);
}

#[test]
fn arm_fixture_threshold_padding_and_names_are_independent_of_default_skin() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/arm");
    let mut doc = skel2d_core::load(base.join("arm.skel.json")).unwrap();
    let Attachment::Mesh(source) = &mut doc.slots[0].attachments[0] else {
        panic!()
    };
    source.image = Some("trace.png".into());
    let source = source.clone();
    let doc = skel2d_core::apply(
        &doc,
        &[
            Op::AddSkin {
                name: "armor".into(),
            },
            Op::SetSkinAttachment {
                skin: "armor".into(),
                slot: "skin".into(),
                attachment: Attachment::Mesh(source.clone()),
            },
            Op::KeyDeform {
                animation: "deform".into(),
                slot: "skin".into(),
                attachment: "arm".into(),
                skin: Some("armor".into()),
                time: 0.0,
                offsets: vec![[2.0, 3.0]; source.vertices.len()],
                curve: None,
            },
        ],
    )
    .unwrap();
    let op = json!({"op":"trace_mesh","slot":"skin","attachment":"arm","skin":"armor","name":"traced","interior":"none","padding":0,"concavity":0});
    let a = run(&doc, &base, op.clone());
    let b = run(&doc, &base, op.clone());
    assert_eq!(
        skel2d_core::to_string(&a.document),
        skel2d_core::to_string(&b.document)
    );
    assert_eq!(a.document.slots[0].attachments, doc.slots[0].attachments);
    assert!(a.document.animations["deform"].deform["armor"]["skin"].contains_key("traced"));
    assert!(!a.document.animations["deform"].deform["armor"]["skin"].contains_key("arm"));
    let Attachment::Mesh(m) = a
        .document
        .skin("armor")
        .unwrap()
        .attachment("skin", "traced")
        .unwrap()
    else {
        panic!()
    };
    let min_u = |m: &skel2d_core::Mesh| {
        m.vertices
            .iter()
            .map(|v| v.u * 128.0)
            .fold(f32::INFINITY, f32::min)
    };
    assert!((min_u(m) - 4.0).abs() < 0.01);
    let mut high = op.clone();
    high["alpha_threshold"] = json!(0.5);
    let high = run(&doc, &base, high);
    let Attachment::Mesh(h) = high
        .document
        .skin("armor")
        .unwrap()
        .attachment("skin", "traced")
        .unwrap()
    else {
        panic!()
    };
    assert!((min_u(h) - 6.0).abs() < 0.01);
    let mut padded = op;
    padded["padding"] = json!(2);
    let padded = run(&doc, &base, padded);
    let Attachment::Mesh(p) = padded
        .document
        .skin("armor")
        .unwrap()
        .attachment("skin", "traced")
        .unwrap()
    else {
        panic!()
    };
    assert!((min_u(p) - 2.0).abs() < 0.01);
    let renamed=skel2d_core::apply(&a.document,&skel2d_core::parse_ops(r#"[{"op":"rename_attachment","slot":"skin","skin":"armor","from":"traced","to":"renamed"},{"op":"remove_skin_attachment","skin":"armor","slot":"skin","name":"renamed"}]"#).unwrap()).unwrap();
    assert!(
        renamed.animations["deform"]
            .deform
            .values()
            .all(|s| s.values().all(|a| !a.contains_key("renamed")))
    );
}

#[test]
fn holes_remain_empty_and_nonfinite_trace_parameters_are_rejected() {
    let (doc, dir) = fixture();
    let mut pm = tiny_skia::Pixmap::load_png(dir.join("body.png")).unwrap();
    for y in 24..40 {
        for x in 36..60 {
            pm.data_mut()[(y * 96 + x) * 4..(y * 96 + x) * 4 + 4].fill(0);
        }
    }
    pm.save_png(dir.join("body.png")).unwrap();
    let result = run(
        &doc,
        &dir,
        json!({"op":"trace_mesh","slot":"body","attachment":"image","interior":"grid","spacing":8,"padding":0,"concavity":0}),
    );
    let m = mesh(&result.document);
    for t in &m.triangles {
        let center = t.iter().fold([0.0, 0.0], |mut p, &i| {
            p[0] += m.vertices[i as usize].u * 96.0 / 3.0;
            p[1] += m.vertices[i as usize].v * 64.0 / 3.0;
            p
        });
        assert!(!(36.01..59.99).contains(&center[0]) || !(24.01..39.99).contains(&center[1]));
    }
    for params in [
        TraceParams {
            padding: f32::NAN,
            ..Default::default()
        },
        TraceParams {
            spacing: Some(f32::INFINITY),
            ..Default::default()
        },
    ] {
        let op = AssetOp::TraceMesh {
            slot: "body".into(),
            attachment: "image".into(),
            skin: None,
            name: None,
            params,
        };
        assert!(apply_any(&doc, &dir, &[op.into()]).is_err());
    }
}
