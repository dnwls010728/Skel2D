use std::collections::HashSet;
use std::fmt;

use schemars::JsonSchema;
use serde::Serialize;

use crate::model::{Attachment, Document, FORMAT, Key, LayerTrack, Mesh};

/// One validation problem. `path` points into the document, e.g.
/// `bones[arm].parent` or `animations[walk].bones[leg].rotate[2]`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Runtime preview configuration, separate from document validation. Names are
/// unique even when disabled, so callers can address tracks by animation name.
pub fn validate_layer_tracks(doc: &Document, layers: &[LayerTrack]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut names = HashSet::new();
    for (i, layer) in layers.iter().enumerate() {
        let mut issue = |field: &str, message: &str| issues.push(Issue { path: format!("layers[{i}].{field}"), message: message.into() });
        if !doc.animations.contains_key(&layer.anim) { issue("anim", "animation does not exist"); }
        if !names.insert(&layer.anim) { issue("anim", "duplicate preview animation"); }
        if !layer.alpha.is_finite() || !(0.0..=1.0).contains(&layer.alpha) { issue("alpha", "alpha must be finite and between 0 and 1"); }
    }
    issues
}

/// Checks every invariant the pose evaluator and renderer rely on.
pub fn validate(doc: &Document) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut issue = |path: String, message: String| issues.push(Issue { path, message });

    if doc.format != FORMAT {
        issue("format".into(), format!("unsupported format {:?}, expected {FORMAT:?}", doc.format));
    }

    let mut seen = HashSet::new();
    for bone in &doc.bones {
        let p = format!("bones[{}]", bone.name);
        if bone.name.is_empty() {
            issue(p.clone(), "bone name is empty".into());
        }
        if let Some(parent) = &bone.parent
            && !seen.contains(parent.as_str()) {
                let msg = if doc.bone(parent).is_some() {
                    format!("parent {parent:?} must come before this bone (cycle or bad order)")
                } else {
                    format!("parent {parent:?} does not exist")
                };
                issue(format!("{p}.parent"), msg);
            }
        if !seen.insert(bone.name.as_str()) {
            issue(p.clone(), "duplicate bone name".into());
        }
        if bone.name == crate::model::ROOT && bone.parent.is_some() {
            issue(format!("{p}.parent"), "the root bone cannot have a parent".into());
        }
        for (field, v) in [
            ("x", bone.x),
            ("y", bone.y),
            ("rotation", bone.rotation),
            ("scale_x", bone.scale_x),
            ("scale_y", bone.scale_y),
            ("shear_x", bone.shear_x),
            ("shear_y", bone.shear_y),
            ("length", bone.length),
        ] {
            if !v.is_finite() {
                issue(format!("{p}.{field}"), "value is not finite".into());
            }
        }
    }

    let mut slot_names = HashSet::new();
    for slot in &doc.slots {
        let p = format!("slots[{}]", slot.name);
        if slot.name.is_empty() {
            issue(p.clone(), "slot name is empty".into());
        }
        if !slot_names.insert(slot.name.as_str()) {
            issue(p.clone(), "duplicate slot name".into());
        }
        if doc.bone(&slot.bone).is_none() {
            issue(format!("{p}.bone"), format!("bone {:?} does not exist", slot.bone));
        }
        let mut names = HashSet::new();
        for att in &slot.attachments {
            if att.name().trim().is_empty() { issue(format!("{p}.attachments"), "attachment name is empty".into()); }
            if !names.insert(att.name()) {
                issue(format!("{p}.attachments[{}]", att.name()), "duplicate attachment name".into());
            }
            match att {
                Attachment::Region(r) => {
                    if !(r.width.is_finite() && r.height.is_finite() && r.width > 0.0 && r.height > 0.0) {
                        issue(
                            format!("{p}.attachments[{}]", r.name),
                            "width and height must be positive".into(),
                        );
                    }
                }
                Attachment::Mesh(m) => check_mesh(&mut issue, doc, &format!("{p}.attachments[{}]", m.name), m),
                Attachment::LinkedMesh(l) => check_linked(&mut issue, doc, &slot.name, &format!("{p}.attachments[{}]", l.name), l),
            }
        }
        if let Some(a) = &slot.attachment
            && !names.contains(a.as_str()) && !crate::ops::slot_has_attachment(doc, &slot.name, a) {
                issue(format!("{p}.attachment"), format!("attachment {a:?} does not exist in this slot"));
            }
    }

    for (anim_name, anim) in &doc.animations {
        let ap = format!("animations[{anim_name}]");
        if anim_name.is_empty() {
            issue(ap.clone(), "animation name is empty".into());
        }
        if let Some(d) = anim.duration
            && !(d.is_finite() && d >= 0.0) {
                issue(format!("{ap}.duration"), "duration must be >= 0".into());
            }
        for (bone, tl) in &anim.bones {
            let bp = format!("{ap}.bones[{bone}]");
            if doc.bone(bone).is_none() {
                issue(bp.clone(), "bone does not exist".into());
            }
            check_keys(&mut issue, &format!("{bp}.rotate"), &tl.rotate, |v| v.is_finite());
            check_channels(&mut issue, &format!("{bp}.rotate"), &tl.rotate, 1);
            for (name, keys) in [("translate", &tl.translate), ("scale", &tl.scale), ("shear", &tl.shear)] {
                check_channels(&mut issue, &format!("{bp}.{name}"), keys, 2);
            }
            check_keys(&mut issue, &format!("{bp}.translate"), &tl.translate, |v| v.iter().all(|c| c.is_finite()));
            check_keys(&mut issue, &format!("{bp}.scale"), &tl.scale, |v| v.iter().all(|c| c.is_finite()));
            check_keys(&mut issue, &format!("{bp}.shear"), &tl.shear, |v| v.iter().all(|c| c.is_finite()));
        }
        for (slot_name, tl) in &anim.slots {
            let sp = format!("{ap}.slots[{slot_name}]");
            let Some(_slot) = doc.slot(slot_name) else {
                issue(sp, "slot does not exist".into());
                continue;
            };
            check_keys(&mut issue, &format!("{sp}.color"), &tl.color, |_| true);
            check_channels(&mut issue, &format!("{sp}.color"), &tl.color, 4);
            let mut prev = f32::NEG_INFINITY;
            for (i, k) in tl.attachment.iter().enumerate() {
                let kp = format!("{sp}.attachment[{i}]");
                if !(k.time.is_finite() && k.time >= 0.0) {
                    issue(kp.clone(), "time must be >= 0".into());
                } else if k.time <= prev {
                    issue(kp.clone(), "keys must have strictly increasing time".into());
                }
                prev = k.time;
                if let Some(n) = &k.name
                    && !crate::ops::slot_has_attachment(doc, slot_name, n) {
                        issue(kp, format!("attachment {n:?} does not exist in slot"));
                    }
            }
        }
        for (skin, slots) in &anim.deform {
            if skin != "default" && doc.skin(skin).is_none() {
                issue(format!("{ap}.deform[{skin}]"), "skin does not exist".into());
            }
            for (slot_name, atts) in slots {
                if doc.slot(slot_name).is_none() {
                    issue(format!("{ap}.deform[{skin}][{slot_name}]"), "slot does not exist".into());
                }
                for (att, keys) in atts {
                    let dp = format!("{ap}.deform[{skin}][{slot_name}][{att}]");
                    let mesh = if skin == "default" { doc.slot(slot_name).and_then(|s| s.attachment(att)) }
                        else { doc.skin(skin).and_then(|s| s.attachment(slot_name, att)) };
                    let Some(mesh) = mesh.and_then(|a| doc.materialize_attachment(doc.slot_index(slot_name).unwrap_or(0), a)) else {
                        issue(dp, "not a mesh attachment of this slot".into());
                        continue;
                    };
                    let crate::model::Attachment::Mesh(mesh) = mesh else { issue(dp, "not a mesh attachment of this slot".into()); continue };
                    check_keys(&mut issue, &dp, keys, |v| v.iter().flatten().all(|c| c.is_finite()));
                    for (i, k) in keys.iter().enumerate().filter(|(_, k)| k.value.len() != mesh.vertices.len()) {
                        issue(format!("{dp}[{i}]"), format!("{} offsets, the mesh has {} vertices", k.value.len(), mesh.vertices.len()));
                    }
                }
            }
        }
        for (ik, keys) in &anim.ik {
            let kp = format!("{ap}.ik[{ik}]");
            if !doc.ik.iter().any(|c| &c.name == ik) {
                issue(kp.clone(), "IK constraint does not exist".into());
            }
            check_keys(&mut issue, &kp, keys, |v| v.mix.is_none_or(f32::is_finite) && v.softness.is_none_or(f32::is_finite));
            check_channels(&mut issue, &kp, keys, 2);
            for (i, _) in keys.iter().enumerate().filter(|(_, k)| k.value.softness.is_some_and(|s| s < 0.0)) {
                issue(format!("{kp}[{i}].softness"), "softness must be >= 0".into());
            }
        }
        let mut prev = f32::NEG_INFINITY;
        for (i, k) in anim.draw_order.iter().enumerate() {
            let kp = format!("{ap}.draw_order[{i}]");
            if !(k.time.is_finite() && k.time >= 0.0) {
                issue(kp.clone(), "time must be >= 0".into());
            } else if k.time <= prev {
                issue(kp.clone(), "keys must have strictly increasing time".into());
            }
            prev = k.time;
            let mut listed = HashSet::new();
            for slot in k.order.iter().flatten() {
                if doc.slot(slot).is_none() { issue(kp.clone(), format!("slot {slot:?} does not exist")); }
                if !listed.insert(slot) { issue(kp.clone(), format!("slot {slot:?} is listed twice")); }
            }
        }
        let mut prev = f32::NEG_INFINITY;
        let mut fired = HashSet::new();
        for (i, k) in anim.events.iter().enumerate() {
            let kp = format!("{ap}.events[{i}]");
            if !(k.time.is_finite() && k.time >= 0.0) {
                issue(kp.clone(), "time must be >= 0".into());
            } else if k.time < prev {
                issue(kp.clone(), "keys must be sorted by time".into());
            }
            prev = k.time;
            if !doc.events.contains_key(&k.name) { issue(kp.clone(), format!("event {:?} does not exist", k.name)); }
            if !fired.insert((k.name.as_str(), k.time.to_bits())) { issue(kp.clone(), "the same event is keyed twice at this time".into()); }
            if k.float.is_some_and(|v| !v.is_finite()) { issue(kp, "float is not finite".into()); }
        }
        for (name, keys) in &anim.transforms {
            let kp = format!("{ap}.transforms[{name}]");
            if !doc.transforms.iter().any(|c| &c.name == name) {
                issue(kp.clone(), "transform constraint does not exist".into());
            }
            check_keys(&mut issue, &kp, keys, |v| v.all_finite());
        }
        for (name, tl) in &anim.physics {
            let tp = format!("{ap}.physics[{name}]");
            if !name.is_empty() && doc.physics_constraint(name).is_none() {
                issue(tp.clone(), "physics constraint does not exist".into());
            }
            for (property, keys) in tl.property_keys() {
                check_keys(&mut issue, &format!("{tp}.{property}"), keys, |v| v.is_finite());
            }
            let mut prev = f32::NEG_INFINITY;
            for (i, &time) in tl.reset.iter().enumerate() {
                if !(time.is_finite() && time >= 0.0) {
                    issue(format!("{tp}.reset[{i}]"), "time must be >= 0".into());
                } else if time <= prev {
                    issue(format!("{tp}.reset[{i}]"), "keys must have strictly increasing time".into());
                }
                prev = time;
            }
        }
    }

    for (name, event) in &doc.events {
        if name.is_empty() { issue("events".into(), "event name is empty".into()); }
        if !event.float.is_finite() { issue(format!("events[{name}].float"), "value is not finite".into()); }
    }

    let mut skin_names = HashSet::new();
    for skin in &doc.skins {
        let p = format!("skins[{}]", skin.name);
        if skin.name.is_empty() {
            issue(p.clone(), "skin name is empty".into());
        }
        if skin.name == "default" {
            issue(p.clone(), "default is reserved for slot attachments".into());
        }
        if !skin_names.insert(skin.name.as_str()) {
            issue(p.clone(), "duplicate skin name".into());
        }
        for b in skin.bones.iter().filter(|b| doc.bone(b).is_none()) {
            issue(format!("{p}.bones[{b}]"), "bone does not exist".into());
        }
        let mut constraints = HashSet::new();
        for c in &skin.constraints {
            if !constraints.insert(c) { issue(format!("{p}.constraints[{c}]"), "duplicate constraint".into()); }
            if !doc.has_constraint(c) {
                issue(format!("{p}.constraints[{c}]"), "constraint does not exist".into());
            }
        }
        for (slot, atts) in &skin.attachments {
            let sp = format!("{p}.attachments[{slot}]");
            if doc.slot(slot).is_none() {
                issue(sp.clone(), "slot does not exist".into());
            }
            let mut seen = HashSet::new();
            for att in atts {
                if att.name().trim().is_empty() { issue(sp.clone(), "attachment name is empty".into()); }
                if !seen.insert(att.name()) {
                    issue(format!("{sp}[{}]", att.name()), "duplicate attachment name".into());
                }
                if let Attachment::Mesh(m) = att {
                    check_mesh(&mut issue, doc, &format!("{sp}[{}]", m.name), m);
                } else if let Attachment::LinkedMesh(l) = att {
                    check_linked(&mut issue, doc, slot, &format!("{sp}[{}]", l.name), l);
                }
            }
        }
    }

    // Order accepts every i32, including negative values and ties. Serde
    // enforces the integer range; evaluation preserves array order on ties.
    let mut constraint_names = HashSet::new();
    for c in &doc.physics {
        let p = format!("physics[{}]", c.name);
        if c.name.is_empty() {
            issue(p.clone(), "physics constraint name is empty".into());
        }
        if !constraint_names.insert(c.name.as_str()) {
            issue(p.clone(), "duplicate constraint name".into());
        }
        if doc.bone(&c.bone).is_none() {
            issue(format!("{p}.bone"), format!("bone {:?} does not exist", c.bone));
        }
        for (field, v) in [("x", c.x), ("y", c.y), ("rotate", c.rotate), ("scale_x", c.scale_x), ("shear_x", c.shear_x), ("mix", c.mix), ("damping", c.damping)] {
            if !(v.is_finite() && (0.0..=1.0).contains(&v)) {
                issue(format!("{p}.{field}"), "must be between 0 and 1".into());
            }
        }
        for (field, v) in [("inertia", c.inertia), ("strength", c.strength)] {
            if !(v.is_finite() && v >= 0.0) {
                issue(format!("{p}.{field}"), "must be >= 0".into());
            }
        }
        for (field, v) in [("limit", c.limit), ("mass_inverse", c.mass_inverse)] {
            if !(v.is_finite() && v > 0.0) {
                issue(format!("{p}.{field}"), "must be > 0".into());
            }
        }
        if !(c.step.is_finite() && c.step >= 1.0 / 1000.0 && c.step <= 1.0) {
            issue(format!("{p}.step"), "step must be between 0.001 and 1 second".into());
        }
        for (field, v) in [("wind", c.wind), ("gravity", c.gravity)] {
            if !v.is_finite() {
                issue(format!("{p}.{field}"), "value is not finite".into());
            }
        }
    }
    let g = &doc.physics_globals;
    for (field, v) in [("mix", g.mix), ("damping", g.damping)] {
        if !(v.is_finite() && (0.0..=1.0).contains(&v)) {
            issue(format!("physics_globals.{field}"), "must be between 0 and 1".into());
        }
    }
    for (field, v, min_ok) in [("inertia", g.inertia, g.inertia >= 0.0), ("strength", g.strength, g.strength >= 0.0), ("mass_inverse", g.mass_inverse, g.mass_inverse > 0.0), ("wind", g.wind, true), ("gravity", g.gravity, true)] {
        if !(v.is_finite() && min_ok) {
            issue(format!("physics_globals.{field}"), "value is out of range".into());
        }
    }

    for c in &doc.ik {
        let p = format!("ik[{}]", c.name);
        if c.name.is_empty() {
            issue(p.clone(), "IK name is empty".into());
        }
        if !constraint_names.insert(c.name.as_str()) {
            issue(p.clone(), "duplicate constraint name".into());
        }
        if !(c.mix.is_finite() && (0.0..=1.0).contains(&c.mix)) {
            issue(format!("{p}.mix"), "mix must be between 0 and 1".into());
        }
        if !(c.softness.is_finite() && c.softness >= 0.0) {
            issue(format!("{p}.softness"), "softness must be >= 0".into());
        }
        if !(1..=2).contains(&c.bones.len()) {
            issue(format!("{p}.bones"), "give one bone or a parent and its direct child".into());
        }
        for b in &c.bones {
            if doc.bone(b).is_none() {
                issue(format!("{p}.bones"), format!("bone {b:?} does not exist"));
            }
        }
        if let [parent, child] = c.bones.as_slice()
            && doc.bone(child).is_some_and(|b| b.parent.as_deref() != Some(parent.as_str()))
        {
            issue(format!("{p}.bones"), format!("{child:?} must be a direct child of {parent:?}"));
        }
        if doc.bone(&c.target).is_none() {
            issue(format!("{p}.target"), format!("bone {:?} does not exist", c.target));
        } else if let Some(root) = c.bones.first()
            && is_in_subtree(doc, &c.target, root)
        {
            issue(format!("{p}.target"), "target must not be part of the chain or its children".into());
        }
    }

    for c in &doc.transforms {
        let p = format!("transforms[{}]", c.name);
        if c.name.is_empty() {
            issue(p.clone(), "constraint name is empty".into());
        }
        if !constraint_names.insert(c.name.as_str()) {
            issue(p.clone(), "duplicate constraint name".into());
        }
        if c.bones.is_empty() {
            issue(format!("{p}.bones"), "give at least one bone".into());
        }
        let mut bound = HashSet::new();
        for b in &c.bones {
            if doc.bone(b).is_none() {
                issue(format!("{p}.bones"), format!("bone {b:?} does not exist"));
            }
            if !bound.insert(b) {
                issue(format!("{p}.bones"), format!("bone {b:?} is listed twice"));
            }
            if is_in_subtree(doc, &c.source, b) {
                issue(format!("{p}.source"), format!("source must not be {b:?} or one of its children"));
            }
        }
        if doc.bone(&c.source).is_none() {
            issue(format!("{p}.source"), format!("bone {:?} does not exist", c.source));
        }
        for (field, v) in [
            ("mix_rotate", c.mix_rotate), ("mix_x", c.mix_x), ("mix_y", c.mix_y), ("mix_scale_x", c.mix_scale_x), ("mix_scale_y", c.mix_scale_y), ("mix_shear_y", c.mix_shear_y),
            ("offset_rotation", c.offset_rotation), ("offset_x", c.offset_x), ("offset_y", c.offset_y), ("offset_scale_x", c.offset_scale_x), ("offset_scale_y", c.offset_scale_y), ("offset_shear_y", c.offset_shear_y),
        ] {
            if !v.is_finite() {
                issue(format!("{p}.{field}"), "value is not finite".into());
            }
        }
    }

    issues
}

fn check_channels<T>(issue: &mut impl FnMut(String, String), path: &str, keys: &[Key<T>], channels: usize) {
    for (i, _) in keys.iter().enumerate().filter(|(_, k)| !k.curves.is_empty() && k.curves.len() != channels) {
        issue(format!("{path}[{i}].curves"), format!("give {channels} curves, one per channel"));
    }
}

fn is_in_subtree(doc: &Document, bone: &str, root: &str) -> bool {
    let mut cur = Some(bone);
    for _ in 0..=doc.bones.len() {
        match cur {
            Some(n) if n == root => return true,
            Some(n) => cur = doc.bone(n).and_then(|b| b.parent.as_deref()),
            None => return false,
        }
    }
    false
}

fn check_linked(issue: &mut impl FnMut(String, String), doc: &Document, slot: &str, path: &str, l: &crate::model::LinkedMesh) {
    let parent = match l.parent_skin.as_deref() {
        Some(k) => doc.skin(k).and_then(|s| s.attachment(slot, &l.parent)),
        None => doc.slot(slot).and_then(|s| s.attachment(&l.parent)),
    };
    if !matches!(parent, Some(Attachment::Mesh(_))) {
        issue(format!("{path}.parent"), "parent must be an independent mesh in the same slot".into());
    }
}

fn check_mesh(issue: &mut impl FnMut(String, String), doc: &Document, path: &str, m: &Mesh) {
    let mut bound = std::collections::BTreeSet::new();
    for bone in &m.bones {
        if doc.bone(bone).is_none() || !bound.insert(bone) {
            issue(format!("{path}.bones"), format!("binding bone {bone:?} must exist and be unique"));
        }
    }
    if let Err(message) = crate::mesh::validate_topology(m) { issue(format!("{path}.topology"), message); }
    if m.vertices.len() < 3 || m.triangles.is_empty() {
        issue(path.to_string(), "mesh needs at least 3 vertices and 1 triangle".into());
    }
    for (i, v) in m.vertices.iter().enumerate() {
        let vp = format!("{path}.vertices[{i}]");
        if ![v.x, v.y, v.u, v.v].iter().all(|c| c.is_finite()) {
            issue(vp.clone(), "value is not finite".into());
        }
        for w in &v.weights {
            if doc.bone(&w.bone).is_none() {
                issue(vp.clone(), format!("weight bone {:?} does not exist", w.bone));
            }
            if !(w.weight.is_finite() && w.weight >= 0.0) {
                issue(vp.clone(), "weights must be >= 0".into());
            }
        }
    }
    let n = m.vertices.len() as u32;
    for (i, t) in m.triangles.iter().enumerate() {
        if t.iter().any(|&j| j >= n) {
            issue(format!("{path}.triangles[{i}]"), format!("index out of range (mesh has {n} vertices)"));
        }
    }
}

fn check_keys<T>(
    issue: &mut impl FnMut(String, String),
    path: &str,
    keys: &[Key<T>],
    finite: impl Fn(&T) -> bool,
) {
    let mut prev = f32::NEG_INFINITY;
    for (i, k) in keys.iter().enumerate() {
        let kp = format!("{path}[{i}]");
        if !(k.time.is_finite() && k.time >= 0.0) {
            issue(kp.clone(), "time must be >= 0".into());
        } else if k.time <= prev {
            issue(kp.clone(), "keys must have strictly increasing time".into());
        }
        prev = k.time;
        if !finite(&k.value) {
            issue(kp, "value is not finite".into());
        }
    }
}

/// Reorders bones so parents precede children while keeping the existing
/// relative order otherwise. Bones caught in a cycle or with a missing parent
/// are appended unchanged so that `validate` can report them.
pub fn sort_bones(doc: &mut Document) {
    let mut placed: HashSet<String> = HashSet::new();
    let mut remaining = std::mem::take(&mut doc.bones);
    let mut sorted = Vec::with_capacity(remaining.len());
    loop {
        let before = remaining.len();
        let mut rest = Vec::new();
        for bone in remaining {
            let ready = bone.parent.as_ref().is_none_or(|p| placed.contains(p));
            if ready {
                placed.insert(bone.name.clone());
                sorted.push(bone);
            } else {
                rest.push(bone);
            }
        }
        remaining = rest;
        if remaining.is_empty() || remaining.len() == before {
            break;
        }
    }
    sorted.extend(remaining);
    doc.bones = sorted;
}
