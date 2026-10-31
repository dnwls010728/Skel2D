use serde_json::{Value,json};
use skel2d_core::{Attachment,Document,Mesh,Pose,apply,from_str,parse_ops,to_string,validate};
use skel2d_core::mesh::weight_map;

fn document() -> Document {
    let v=|x,y,weights|json!({"x":x,"y":y,"u":x/10.0,"v":y/10.0,"weights":weights});
    let weights=json!([{"bone":"root","weight":0.2},{"bone":"a","weight":0.6},{"bone":"b","weight":0.2}]);
    let mesh=json!({"type":"mesh","name":"cloth","vertices":[v(0.0,0.0,weights),v(10.0,0.0,json!([{"bone":"b","weight":1.0}])),v(0.0,10.0,json!([]))],"triangles":[[0,1,2]]});
    from_str(&json!({"bones":[{"name":"root"},{"name":"a","parent":"root","x":10,"length":10},{"name":"b","parent":"root","y":10,"length":10},{"name":"unused","parent":"a"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[mesh.clone()]}],"skins":[{"name":"armor","attachments":{"body":[mesh]}}]}).to_string()).unwrap()
}
fn mesh<'a>(doc:&'a Document,skin:Option<&str>)->&'a Mesh {
    let att=match skin {Some(k)=>doc.skin(k).unwrap().attachment("body","cloth"),None=>doc.slots[0].attachment("cloth")}.unwrap();
    let Attachment::Mesh(m)=att else {panic!()};m
}
fn run(doc:&Document,mut op:Value)->Document {
    op["slot"]=json!("body");op["attachment"]=json!("cloth");
    apply(doc,&parse_ops(&op.to_string()).unwrap()).unwrap()
}
fn w(doc:&Document,i:usize,bone:&str)->f32 {weight_map(&mesh(doc,None).vertices[i],"root").get(bone).copied().unwrap_or(0.0)}
fn near(a:f32,b:f32) {assert!((a-b).abs()<0.00011,"{a} != {b}");}

#[test]
fn smooth_prune_swap_and_numeric_have_expected_values() {
    let d=document();
    let s=run(&d,json!({"op":"smooth_weights","vertices":[0],"factor":1}));
    near(w(&s,0,"root"),0.5);near(w(&s,0,"b"),0.5);near(w(&s,0,"a"),0.0);
    assert_eq!(mesh(&d,None).vertices[1],mesh(&s,None).vertices[1]);
    let p=run(&d,json!({"op":"prune_weights","max_bones":2,"locked_bones":["root"]}));
    near(w(&p,0,"root"),0.2);near(w(&p,0,"a"),0.8);near(w(&p,0,"b"),0.0);
    let swap=run(&d,json!({"op":"swap_weights","a":"a","b":"b","vertices":[0]}));
    near(w(&swap,0,"a"),0.2);near(w(&swap,0,"b"),0.6);
    for (mode,value,expected) in [("set",0.4,0.4),("add",-0.2,0.4),("scale",0.5,0.3)] {
        let out=run(&d,json!({"op":"set_bone_weight","vertices":[0],"bone":"a","value":value,"mode":mode,"locked_bones":["root"]}));
        near(w(&out,0,"root"),0.2);near(w(&out,0,"a"),expected);near(w(&out,0,"b"),0.8-expected);
    }
}

#[test]
fn locks_hold_for_all_weight_operations_and_empty_selection_is_noop() {
    let d=document();
    for mut op in [json!({"op":"smooth_weights","iterations":3}),json!({"op":"prune_weights","max_bones":1,"threshold":0.9}),json!({"op":"auto_weights","bones":["b"]}),json!({"op":"swap_weights","a":"a","b":"b"}),json!({"op":"set_bone_weight","vertices":[0],"bone":"b","value":1,"mode":"set"}),json!({"op":"transfer_weights","from_slot":"body","from_attachment":"cloth","from_skin":"armor"})] {
        op["locked_bones"]=json!(["root"]);
        let out=run(&d,op.clone());
        near(w(&out,0,"root"),0.2);
        for v in &mesh(&out,None).vertices {near(weight_map(v,"root").values().sum(),1.0);}
        op["vertices"]=json!([]);
        assert_eq!(run(&d,op),d);
    }
    assert_eq!(run(&d,json!({"op":"swap_weights","a":"a","b":"b","locked_bones":["a"]})),d);
}

#[test]
fn binding_palette_persists_zero_influences_and_removal_redistributes() {
    let d=document();
    let added=run(&d,json!({"op":"bind_bones","add":["unused"],"redistribute":"auto"}));
    assert!(mesh(&added,None).bones.contains(&"unused".into()));
    assert_eq!(mesh(&added,None).vertices,mesh(&d,None).vertices);
    assert_eq!(from_str(&to_string(&added)).unwrap(),added);
    for mode in ["auto","nearest","parent"] {
        let out=run(&added,json!({"op":"bind_bones","remove":["a"],"redistribute":mode}));
        near(w(&out,0,"root"),if mode=="auto" {0.5} else {0.8});
        assert!(!mesh(&out,None).bones.contains(&"a".into()));
        for v in &mesh(&out,None).vertices {let m=weight_map(v,"root");assert!(!m.contains_key("a"));near(m.values().sum(),1.0);}
        assert_eq!(mesh(&out,Some("armor")),mesh(&d,Some("armor")));
        assert!(validate(&out).is_empty());
    }
    let out=run(&added,json!({"op":"bind_bones","remove":["root"],"redistribute":"nearest"}));
    for v in &mesh(&out,None).vertices {assert!(!weight_map(v,"root").contains_key("root"));}
}

#[test]
fn legacy_normalization_skin_isolation_and_pose() {
    let d=document();
    let serialized=to_string(&d);assert!(serde_json::to_value(mesh(&d,None)).unwrap().get("bones").is_none());
    assert_eq!(from_str(&serialized).unwrap(),d);
    let out=run(&d,json!({"op":"set_bone_weight","skin":"armor","vertices":[0],"bone":"b","mode":"set","value":1}));
    assert_eq!(mesh(&out,None),mesh(&d,None));near(weight_map(&mesh(&out,Some("armor")).vertices[0],"root")["b"],1.0);
    let mut raw=d.clone();let Attachment::Mesh(m)=&mut raw.slots[0].attachments[0] else {panic!()};
    for w in &mut m.vertices[0].weights {w.weight*=7.0;}
    let norm=run(&raw,json!({"op":"normalize_weights"}));near(w(&norm,0,"a"),0.6);
    let pose=Pose::compute(&d,None);
    assert_eq!(pose.deform_mesh(&d,mesh(&d,None),0),pose.deform_mesh(&norm,mesh(&norm,None),0));
}

#[test]
fn rename_delete_and_invalid_inputs_are_atomic() {
    let d=run(&document(),json!({"op":"bind_bones","add":["unused"],"redistribute":"auto"}));
    let renamed=apply(&d,&parse_ops(r#"{"op":"set_bone","name":"unused","rename":"extra"}"#).unwrap()).unwrap();
    assert!(mesh(&renamed,None).bones.contains(&"extra".into()));
    let deleted=apply(&renamed,&parse_ops(r#"{"op":"remove_bone","name":"extra"}"#).unwrap()).unwrap();
    assert!(!mesh(&deleted,None).bones.contains(&"extra".into()));
    for mut op in [json!({"op":"smooth_weights","factor":2}),json!({"op":"smooth_weights","iterations":0}),json!({"op":"prune_weights","max_bones":0}),json!({"op":"prune_weights","threshold":-1}),json!({"op":"set_bone_weight","vertices":[99],"bone":"a","value":0.5,"mode":"set"}),json!({"op":"set_bone_weight","vertices":[0,0],"bone":"a","value":0.5,"mode":"add"}),json!({"op":"swap_weights","a":"missing","b":"b"}),json!({"op":"bind_bones","remove":["root","a","b","unused"],"redistribute":"auto"}),json!({"op":"bind_bones","add":["a"],"remove":["a"],"redistribute":"parent"}),json!({"op":"normalize_weights","skin":"missing"})] {
        op["slot"]=json!("body");op["attachment"]=json!("cloth");
        let before=to_string(&d);
        let ops=parse_ops(&json!([{"op":"set_bone","name":"a","x":500},op]).to_string()).unwrap();
        assert!(apply(&d,&ops).is_err());assert_eq!(to_string(&d),before);
    }
    let mut bad=d.clone();let Attachment::Mesh(m)=&mut bad.slots[0].attachments[0] else {panic!()};m.bones=vec!["missing".into(),"missing".into()];assert!(!validate(&bad).is_empty());
}
