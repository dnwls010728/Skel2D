//! Skel2D core: document model, validation, atomic edit operations and pose
//! evaluation. No I/O beyond loading and saving documents.

pub mod curve;
pub mod error;
pub mod format;
pub mod ik;
pub mod math;
pub mod mesh;
pub mod model;
pub mod ops;
pub mod physics;
mod transform;
pub mod pose;
pub mod validate;

use std::path::Path;

pub use curve::Curve;
pub use error::{Error, Result};
pub use math::Affine;
pub use model::*;
pub use ops::{Op, apply, parse_ops};
pub use physics::{MovePath, PhysicsMode, PhysicsRun, PhysicsState};
pub use pose::Pose;
pub use validate::{Issue, validate};

/// Parse and validate named runtime tracks without changing the document.
pub fn parse_layer_tracks(doc: &Document, json: &str) -> Result<Vec<LayerTrack>> {
    let layers = serde_json::from_str::<Vec<LayerTrack>>(json)?;
    let issues = validate::validate_layer_tracks(doc, &layers);
    if issues.is_empty() { Ok(layers) } else { Err(Error::Invalid(issues)) }
}

/// Parses a document and checks it. Bones are not reordered: a file with a
/// child before its parent is reported, not silently fixed.
pub fn from_str(json: &str) -> Result<Document> {
    let mut value: serde_json::Value = serde_json::from_str(json)?;
    migrate_expressions(&mut value);
    migrate_deform_skins(&mut value)?;
    migrate_skin_constraints(&mut value)?;
    let doc: Document = serde_json::from_value(value)?;
    let issues = validate(&doc);
    if issues.is_empty() { Ok(doc) } else { Err(Error::Invalid(issues)) }
}

fn migrate_skin_constraints(value: &mut serde_json::Value) -> Result<()> {
    use serde_json::Value;
    for skin in value.get_mut("skins").and_then(Value::as_array_mut).into_iter().flatten() {
        let Some(fields) = skin.as_object_mut() else { continue };
        if let Some(ik) = fields.remove("ik") {
            if fields.contains_key("constraints") { return Err(Error::op(0, "skin has both ik and constraints")); }
            fields.insert("constraints".into(), ik);
        }
    }
    Ok(())
}

/// Moves legacy slot deformation into the default skin without overwriting
/// a new-format timeline. Conflicting paths are rejected rather than lost.
fn migrate_deform_skins(value: &mut serde_json::Value) -> Result<()> {
    use serde_json::{Value, json};
    for animation in value.get_mut("animations").and_then(Value::as_object_mut).into_iter().flat_map(|a| a.values_mut()) {
        let mut legacy = Vec::new();
        for (slot, tl) in animation.get_mut("slots").and_then(Value::as_object_mut).into_iter().flatten() {
            if let Some(deform) = tl.as_object_mut().and_then(|t| t.remove("deform")) {
                legacy.push((slot.clone(), deform));
            }
        }
        for (slot, deform) in legacy {
            let target = animation.as_object_mut().unwrap().entry("deform").or_insert_with(|| json!({}));
            let object = target.as_object_mut().ok_or_else(|| Error::op(0, "deform must be an object"))?;
            let default = object.entry("default").or_insert_with(|| json!({}));
            let default = default.as_object_mut().ok_or_else(|| Error::op(0, "deform.default must be an object"))?;
            if default.contains_key(&slot) {
                return Err(Error::op(0, format!("conflicting legacy deform path for slot {slot:?}")));
            }
            default.insert(slot, deform);
        }
    }
    Ok(())
}

/// Older files had `expressions`: named slot overrides applied under every
/// animation. Nothing like it exists any more; a face pose is an animation keyed at
/// time 0 that plays on a lower track. Each expression becomes that
/// animation (same name, or `face/<name>` when the name is taken).
fn migrate_expressions(value: &mut serde_json::Value) {
    use serde_json::{Map, Value, json};
    let Some(root) = value.as_object_mut() else { return };
    let Some(Value::Object(expressions)) = root.remove("expressions") else { return };
    let animations = root.entry("animations").or_insert_with(|| Value::Object(Map::new()));
    let Some(animations) = animations.as_object_mut() else { return };
    for (name, e) in expressions {
        let mut slots = Map::new();
        for (slot, attachment) in e.get("slots").and_then(Value::as_object).into_iter().flatten() {
            slots.insert(slot.clone(), json!({ "attachment": [{ "time": 0.0, "name": attachment }] }));
        }
        for (slot, color) in e.get("colors").and_then(Value::as_object).into_iter().flatten() {
            let tl = slots.entry(slot.clone()).or_insert_with(|| json!({}));
            tl["color"] = json!([{ "time": 0.0, "value": color }]);
        }
        let key = if animations.contains_key(&name) { format!("face/{name}") } else { name };
        animations.insert(key, json!({ "slots": slots }));
    }
}

pub fn to_string(doc: &Document) -> String {
    format::to_string(doc).expect("document always serializes")
}

/// Reads a document for editing, giving older files the root bone.
pub fn load(path: impl AsRef<Path>) -> Result<Document> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.into(), source })?;
    let mut doc = from_str(&text)?;
    ops::ensure_root(&mut doc);
    Ok(doc)
}

/// Writes via a temporary file and rename so a crash never leaves a
/// half-written document.
pub fn save(path: impl AsRef<Path>, doc: &Document) -> Result<()> {
    let path = path.as_ref();
    let io = |source| Error::Io { path: path.into(), source };
    let tmp = path.with_extension("tmp~");
    std::fs::write(&tmp, to_string(doc)).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let ops = parse_ops(
            r##"[{"op":"add_bone","name":"root","length":30},
                {"op":"add_slot","name":"body","bone":"root","color":"#ff000080","attachment":"img",
                 "attachments":[{"type":"region","name":"img","image":"body.png","width":30,"height":40}]},
                {"op":"key_slot","animation":"blink","slot":"body","time":0.2,"attachment":null}]"##,
        )
        .unwrap();
        let doc = apply(&Document::new("rt"), &ops).unwrap();
        let text = to_string(&doc);
        assert_eq!(from_str(&text).unwrap(), doc);
        assert!(text.contains("\"#ff000080\""), "{text}");
    }
}
