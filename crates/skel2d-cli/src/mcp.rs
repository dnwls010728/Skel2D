//! MCP server over stdio (newline-delimited JSON-RPC 2.0) for one document.
//! stdout carries only protocol messages; logs go to stderr. State is shared
//! with `serve` through [`Session`], so external edits are picked up per call.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::RenderArgs;
use crate::PhysicsArgs;
use crate::commands::{self, base_dir, error_json, info_json, pose_json, render_bytes};
use crate::session::Session;

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = "Edit a 2D skeletal animation document. Refer to bones, slots and animations by name, \
never by index. Angles are degrees, y-up, counter-clockwise. Batch related ops into one skel2d_apply_ops call \
(atomic: all or nothing). Verify with skel2d_pose numbers first, then skel2d_render frames. \
Call skel2d_schema for op details. To rig a PNG: skel2d_image_info (look at the grid), then skel2d_rig with \
joint pixel coords (replaces the document; skel2d_undo reverts). Layered character: skel2d_slice a parts sheet, \
then skel2d_rig with a layers.json manifest as image (format: docs/AI_GUIDE.md).";

pub fn run(file: &Path) -> Result<Value> {
    let mut session = Session::open(file)?;
    eprintln!("{}", json!({ "mcp": "stdio", "file": file }));
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&mut session, &msg),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(Value::Null)
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Handles one JSON-RPC message; `None` for notifications.
fn handle(session: &mut Session, msg: &Value) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return msg.get("id").map(|id| rpc_error(id.clone(), -32600, "invalid request"));
    };
    let id = msg.get("id")?.clone();
    let params = &msg["params"];
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "skel2d", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(rpc_error(id, -32602, "missing tool name"));
            };
            match call_tool(session, name, &params["arguments"]) {
                Ok(content) => json!({ "content": content }),
                Err(e) => json!({ "content": [text(&error_json(&e))], "isError": true }),
            }
        }
        other => return Some(rpc_error(id, -32601, &format!("method not found: {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn text(v: &Value) -> Value {
    json!({ "type": "text", "text": skel2d_core::format::to_compact(&v).unwrap_or_default() })
}

fn physics_args(args: &Value) -> Result<PhysicsArgs> {
    Ok(PhysicsArgs {
        physics: opt_str(args, "physics")?.map(str::to_string),
        fps: opt_num(args, "fps")?.map(|v| v as f32),
        moves: opt_str(args, "move")?.map(str::to_string),
    })
}

fn tools() -> Value {
    let anim = json!({ "type": "string", "description": "Animation name; setup pose if omitted" });
    let time = json!({ "type": "number", "description": "Seconds" });
    let none = json!({ "type": "object", "properties": {} });
    let layers = json!({ "type": "array", "description": "Ordered preview tracks, lowest first. Applied after tracks; the main anim is skipped.",
                "items": { "type": "object", "additionalProperties": false, "required": ["anim"], "properties": {
                  "anim": { "type": "string" }, "alpha": { "type": "number", "minimum": 0, "maximum": 1, "default": 1 },
                  "enabled": { "type": "boolean", "default": true } } } });
    let physics = json!({ "enum": ["simulate", "none"], "description": "With animations physics simulates from time 0 (default), or stays off" });
    let fps = json!({ "type": "number", "description": "Simulation frames per second, default 60" });
    let moves = json!({ "type": "string", "description": "World path of the skeleton for physics to react to: \"x,y@t;x,y@t\"" });
    json!([
        { "name": "skel2d_info", "description": "Summary of bones, slots and animations.", "inputSchema": none },
        { "name": "skel2d_get_document", "description": "Full document JSON.", "inputSchema": none },
        { "name": "skel2d_apply_ops",
          "description": "Apply edit ops atomically and save. Call skel2d_schema kind=ops for the op format.",
          "inputSchema": { "type": "object", "properties": {
              "ops": { "type": ["array", "object"], "description": "One op or an array of ops" },
              "dry_run": { "type": "boolean", "description": "Validate without saving" } },
            "required": ["ops"] } },
        { "name": "skel2d_pose", "description": "World-space bone transforms, slot state, changed draw order, fired events and physics offsets (numbers).",
          "inputSchema": { "type": "object", "properties": { "anim": anim, "time": time, "layers": layers,
            "since": { "type": "number", "description": "Report only events fired after this clock time, up to time" },
            "physics": physics, "fps": fps, "move": moves,
            "skins": { "type": "array", "items": { "type": "string" } } } } },
        { "name": "skel2d_render", "description": "Render a PNG of the setup pose or animation frames.",
          "inputSchema": { "type": "object", "properties": {
              "anim": anim, "time": time,
              "frames": { "type": "integer", "description": "1..64 frames side by side (needs anim)" },
              "width": { "type": "integer", "description": "Default 384" },
              "height": { "type": "integer", "description": "Default 384" },
              "zoom": { "type": "number", "description": "Pixels per unit; auto-fit if omitted" },
              "bones": { "type": "boolean", "description": "Bone overlay, default true" },
              "center": { "type": "array", "items": { "type": "number" }, "description": "[x, y] world point to center on (close-ups with zoom)" },
              "tracks": { "type": "array", "items": { "type": "string" }, "description": "Extra anims NAME or NAME@SECONDS layered on top" },
              "layers": layers,
              "physics": physics, "fps": fps, "move": moves,
              "skins": { "type": "array", "items": { "type": "string" }, "description": "Active skins, later ones win" } } } },
        { "name": "skel2d_gif", "description": "Export animations as animated GIF files. out is a file, or a folder with all_animations / all_skins.",
          "inputSchema": { "type": "object", "properties": {
              "out": { "type": "string" }, "anim": anim, "layers": layers,
              "skins": { "type": "array", "items": { "type": "string" } },
              "all_animations": { "type": "boolean" }, "all_skins": { "type": "boolean" },
              "options": { "type": "object", "description": "fps (max 50), colors, dither, quality, transparent, alpha_threshold, background [r,g,b], forever, include_last_frame, plays, pause_after, warm_up, bones, physics, scale, fit [w,h], enlarge, pad, crop [x,y,w,h], range [first,last]" } },
            "required": ["out"] } },
        { "name": "skel2d_schema", "description": "JSON Schema of the ops or the document.",
          "inputSchema": { "type": "object", "properties": { "kind": { "enum": ["ops", "document", "tracks"] } }, "required": ["kind"] } },
        { "name": "skel2d_image_info",
          "description": "PNG size, visible bounds and a labeled pixel-grid image (labels = original pixels).",
          "inputSchema": { "type": "object", "properties": {
              "image": { "type": "string", "description": "PNG path (relative to the document)" },
              "grid": { "type": "boolean", "description": "Default true" },
              "step": { "type": "number", "description": "Grid spacing in px, default 50" },
              "spec": { "type": "object", "description": "Rig spec whose joints to mark, to check them before rigging" } },
            "required": ["image"] } },
        { "name": "skel2d_rig",
          "description": "Replace the document with an auto-rigged PNG or layers.json manifest. Spec: skel2d schema rig.",
          "inputSchema": { "type": "object", "properties": {
              "image": { "type": "string", "description": "PNG or layers.json path (relative to the document)" },
              "spec": { "type": "object", "description": "Rig spec: template humanoid|chain|custom" },
              "cell": { "type": "number", "description": "Mesh density px: grid cell size, or vertex spacing of outline meshes" },
              "grid_mesh": { "type": "boolean", "description": "Regular grid mesh instead of adaptive outline mesh" },
              "no_mesh": { "type": "boolean" },
              "max_influences": { "type": "number", "description": "Default 3" } },
            "required": ["image", "spec"] } },
        { "name": "skel2d_slice",
          "description": "Cut a parts sheet PNG into one PNG per part (reading order). Pieces closer than merge px stay together.",
          "inputSchema": { "type": "object", "properties": {
              "sheet": { "type": "string", "description": "PNG path (relative to the document)" },
              "out_dir": { "type": "string", "description": "Output folder (relative to the document)" },
              "prefix": { "type": "string", "description": "File prefix, default part" },
              "merge": { "type": "number", "description": "Default 40" },
              "padding": { "type": "number", "description": "Default 4" },
              "min_pixels": { "type": "number", "description": "Ignore specks below this, default 50" },
              "defringe": { "type": "number", "description": "Erase alpha below this + specks first (AI images: 32)" },
              "key": { "type": "string", "description": "Chroma-key background color RRGGBB, e.g. 00ff00" } },
            "required": ["sheet", "out_dir"] } },
        { "name": "skel2d_undo", "description": "Revert the last successful apply in this session.", "inputSchema": none },
    ])
}

fn opt_num(args: &Value, key: &str) -> Result<Option<f64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| anyhow!("{key} must be a number")),
    }
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or_else(|| anyhow!("{key} must be a string")),
    }
}

fn opt_strings(args: &Value, key: &str) -> Result<Vec<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("{key} must contain strings"))).collect(),
        _ => Err(anyhow!("{key} must be an array")),
    }
}

fn call_tool(session: &mut Session, name: &str, args: &Value) -> Result<Vec<Value>> {
    session.reload_if_changed()?;
    let value = match name {
        "skel2d_info" => info_json(&session.doc),
        "skel2d_get_document" => serde_json::to_value(&session.doc)?,
        "skel2d_apply_ops" => {
            let ops = args
                .get("ops")
                .filter(|o| o.is_array() || o.is_object())
                .ok_or_else(|| anyhow!("ops must be an op object or an array of ops"))?;
            let ops = skel2d_rig::parse_ops(&ops.to_string())?;
            let report = skel2d_rig::apply_any(&session.doc, &base_dir(&session.file), &ops)?;
            let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            if !dry_run {
                session.commit(report.document)?;
            }
            json!({ "applied": ops.len(), "expanded":report.ops.len(), "meshes":report.meshes, "pruned_influences":report.pruned_influences, "saved": !dry_run })
        }
        "skel2d_pose" => pose_json(&session.doc, opt_str(args, "anim")?, opt_num(args, "time")?.unwrap_or(0.0) as f32, opt_num(args, "since")?.map(|v| v as f32), args.get("layers").map(serde_json::to_string).transpose()?.as_deref(), &opt_strings(args, "skins")?, &physics_args(args)?)?,
        "skel2d_render" => {
            let dim = |key| opt_num(args, key).map(|v| v.map_or(384, |v| v as u32));
            let render = RenderArgs {
                anim: opt_str(args, "anim")?.map(str::to_string),
                time: opt_num(args, "time")?.unwrap_or(0.0) as f32,
                frames: opt_num(args, "frames")?.map(|v| v as u32),
                width: dim("width")?,
                height: dim("height")?,
                zoom: opt_num(args, "zoom")?.map(|v| v as f32),
                no_bones: args.get("bones").and_then(Value::as_bool) == Some(false),
                center: args.get("center").and_then(Value::as_array).and_then(|a| Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])),
                tracks: args.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                layers: args.get("layers").map(serde_json::to_string).transpose()?,
                skins: args.get("skins").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                physics: physics_args(args)?,
            };
            let (png, missing) = render_bytes(&session.doc, &base_dir(&session.file), &render)?;
            return Ok(vec![
                json!({ "type": "image", "data": base64(&png), "mimeType": "image/png" }),
                text(&json!({ "missing_images": missing })),
            ]);
        }
        "skel2d_gif" => {
            let out = opt_str(args, "out")?.ok_or_else(|| anyhow!("out is required"))?;
            let layers = skel2d_core::parse_layer_tracks(&session.doc, &args.get("layers").map(serde_json::to_string).transpose()?.unwrap_or_else(|| "[]".to_string()))?;
            let job = skel2d_render::GifJob { anim: opt_str(args, "anim")?.map(str::to_string), layers, skins: opt_strings(args, "skins")? };
            let opts: skel2d_render::GifOptions = args.get("options").map(|o| serde_json::from_value(o.clone())).transpose()?.unwrap_or_default();
            let flag = |key| args.get(key).and_then(Value::as_bool) == Some(true);
            commands::gif_export(&session.doc, &base_dir(&session.file), &job, &opts, std::path::Path::new(out), flag("all_animations"), flag("all_skins"))?
        }
        "skel2d_image_info" => return image_info(session, args),
        "skel2d_rig" => rig(session, args)?,
        "skel2d_slice" => slice(session, args)?,
        "skel2d_schema" => commands::schema(opt_str(args, "kind")?.unwrap_or("ops"))?,
        "skel2d_undo" => match session.undo()? {
            Some(remaining) => json!({ "undone": true, "remaining": remaining }),
            None => return Err(anyhow!("nothing to undo")),
        },
        other => return Err(anyhow!("unknown tool {other:?}")),
    };
    Ok(vec![text(&value)])
}

fn resolve_image(session: &Session, args: &Value) -> Result<std::path::PathBuf> {
    let image = opt_str(args, "image")?.ok_or_else(|| anyhow!("image is required"))?;
    Ok(base_dir(&session.file).join(image))
}

fn image_info(session: &Session, args: &Value) -> Result<Vec<Value>> {
    let path = resolve_image(session, args)?;
    let img = crate::rig::load_png(&path)?;
    let mut info = serde_json::to_value(skel2d_rig::image_info(&img))?;
    let mut out = Vec::new();
    if args.get("grid").and_then(Value::as_bool) != Some(false) {
        let step = opt_num(args, "step")?.map_or(50, |v| v.max(1.0) as u32);
        let points = match args.get("spec").filter(|s| !s.is_null()) {
            Some(spec) => {
                let spec: skel2d_rig::RigSpec =
                    serde_json::from_value(spec.clone()).map_err(|e| anyhow!("invalid rig spec: {e}"))?;
                crate::rig::spec_points(&spec)
            }
            None => Vec::new(),
        };
        let grid = skel2d_rig::grid::overlay(&img, step, &points);
        let long = grid.width().max(grid.height());
        let grid = if long > 1024 { downscale(&grid, 1024.0 / long as f32) } else { grid };
        let png = grid.encode_png().map_err(|e| anyhow!("png encode: {e}"))?;
        info["grid_note"] = json!("Grid labels are in ORIGINAL image pixels (x right, y down), even if the overlay is downscaled.");
        out.push(json!({ "type": "image", "data": base64(&png), "mimeType": "image/png" }));
    }
    out.insert(0, text(&info));
    Ok(out)
}

fn downscale(src: &tiny_skia::Pixmap, s: f32) -> tiny_skia::Pixmap {
    let w = ((src.width() as f32 * s).round() as u32).max(1);
    let h = ((src.height() as f32 * s).round() as u32).max(1);
    let mut dst = tiny_skia::Pixmap::new(w, h).expect("non-empty");
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    dst.draw_pixmap(0, 0, src.as_ref(), &paint, tiny_skia::Transform::from_scale(s, s), None);
    dst
}

fn slice(session: &Session, args: &Value) -> Result<Value> {
    let base = base_dir(&session.file);
    let sheet = opt_str(args, "sheet")?.ok_or_else(|| anyhow!("sheet is required"))?;
    let out_dir = opt_str(args, "out_dir")?.ok_or_else(|| anyhow!("out_dir is required"))?;
    let num = |key, default: f64| opt_num(args, key).map(|v| v.unwrap_or(default).max(0.0));
    crate::rig::slice(
        &base.join(sheet),
        &base.join(out_dir),
        opt_str(args, "prefix")?.unwrap_or("part"),
        num("merge", 40.0)? as usize,
        num("padding", 4.0)? as u32,
        num("min_pixels", 50.0)? as usize,
        crate::rig::KeyArgs::new(opt_str(args, "key")?, num("key_tolerance", 60.0)?.min(255.0) as u8)?,
        num("defringe", 0.0)?.min(255.0) as u8,
    )
}

fn rig(session: &mut Session, args: &Value) -> Result<Value> {
    let path = resolve_image(session, args)?;
    let spec = args.get("spec").filter(|s| s.is_object()).ok_or_else(|| anyhow!("spec must be an object"))?;
    let spec: skel2d_rig::RigSpec = serde_json::from_value(spec.clone()).map_err(|e| anyhow!("invalid rig spec: {e}"))?;
    let base = base_dir(&session.file);
    let mut opts = skel2d_rig::RigOptions {
        cell: opt_num(args, "cell")?.map(|v| v.max(1.0) as usize),
        grid_mesh: args.get("grid_mesh").and_then(Value::as_bool).unwrap_or(false),
        no_mesh: args.get("no_mesh").and_then(Value::as_bool).unwrap_or(false),
        max_influences: opt_num(args, "max_influences")?.map_or(3, |v| v.max(1.0) as usize),
        name: if session.doc.name.is_empty() { "character".into() } else { session.doc.name.clone() },
        ..skel2d_rig::RigOptions::default()
    };
    let is_manifest = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let (doc, report) = if is_manifest {
        let (layers, canvas) = crate::rig::load_layers(&path, &base)?;
        skel2d_rig::layers::rig_layers(&layers, canvas, &spec, &opts)
    } else {
        opts.image_path = crate::rig::relative_to(&path, &base);
        skel2d_rig::rig(&crate::rig::load_png(&path)?, &spec, &opts)
    }
    .map_err(|e| anyhow!(e))?;
    session.commit(doc)?;
    Ok(serde_json::to_value(report)?)
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(tag: &str) -> Session {
        let dir = std::env::temp_dir().join(format!("skel2d-mcp-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.skel.json");
        skel2d_core::save(&file, &skel2d_core::Document::new("t")).unwrap();
        Session::open(&file).unwrap()
    }

    fn rpc(s: &mut Session, method: &str, params: Value) -> Value {
        handle(s, &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })).unwrap()
    }

    fn call(s: &mut Session, tool: &str, args: Value) -> Value {
        rpc(s, "tools/call", json!({ "name": tool, "arguments": args }))["result"].clone()
    }

    #[test]
    fn initialize_negotiates_version() {
        let mut s = session("init");
        let r = rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "skel2d");
        let r = rpc(&mut s, "initialize", json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn lists_tools() {
        let mut s = session("list");
        let r = rpc(&mut s, "tools/list", json!({}));
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 11);
    }

    #[test]
    fn apply_ops_success_and_error() {
        let mut s = session("apply");
        let ok = call(&mut s, "skel2d_apply_ops", json!({ "ops": { "op": "add_bone", "name": "root" } }));
        assert!(ok.get("isError").is_none());
        assert!(ok["content"][0]["text"].as_str().unwrap().contains("\"applied\":1"));
        assert_eq!(skel2d_core::load(&s.file).unwrap().bones.len(), 1);

        let bad = call(&mut s, "skel2d_apply_ops", json!({ "ops": [{ "op": "add_bone", "name": "a", "parent": "nope" }] }));
        assert_eq!(bad["isError"], true);
        assert!(bad["content"][0]["text"].as_str().unwrap().contains("op_index"));

        assert!(call(&mut s, "skel2d_undo", json!({})).get("isError").is_none());
        assert_eq!(call(&mut s, "skel2d_undo", json!({}))["isError"], true);
    }

    #[test]
    fn render_returns_image() {
        let mut s = session("render");
        call(&mut s, "skel2d_apply_ops", json!({ "ops": [{ "op": "add_bone", "name": "root", "length": 50 }] }));
        let r = call(&mut s, "skel2d_render", json!({ "width": 64, "height": 64 }));
        assert_eq!(r["content"][0]["type"], "image");
        assert_eq!(r["content"][0]["mimeType"], "image/png");
        assert!(r["content"][0]["data"].as_str().unwrap().starts_with("iVBOR"));
        assert_eq!(call(&mut s, "skel2d_render", json!({ "anim": "nope" }))["isError"], true);
    }

    #[test]
    fn render_accepts_ordered_weighted_layers_and_rejects_invalid_configs() {
        let mut s = session("preview_layers");
        let ops = json!([
            {"op":"add_bone","name":"arm","length":40},
            {"op":"key_bone","animation":"z","bone":"arm","time":0,"rotate":20},
            {"op":"key_bone","animation":"a","bone":"arm","time":0,"rotate":80}
        ]);
        assert!(call(&mut s, "skel2d_apply_ops", json!({"ops":ops})).get("isError").is_none());
        let render = |s: &mut Session, layers: Value| call(s, "skel2d_render", json!({"width":64,"height":64,"zoom":1,"center":[0,0],"layers":layers}));
        let base = render(&mut s, json!([]));
        let forward = render(&mut s, json!([{"anim":"z"},{"anim":"a","alpha":0.5}]));
        let reverse = render(&mut s, json!([{"anim":"a","alpha":0.5},{"anim":"z"}]));
        assert!(forward.get("isError").is_none());
        assert_ne!(forward["content"][0]["data"], reverse["content"][0]["data"]);
        let pose = call(&mut s, "skel2d_pose", json!({"layers":[{"anim":"z"},{"anim":"a","alpha":0.5}]}));
        assert!(pose.get("isError").is_none());
        let numbers: Value = serde_json::from_str(pose["content"][0]["text"].as_str().unwrap()).unwrap();
        // Bone 0 is the root every new document starts with; "arm" is bone 1.
        assert_eq!(numbers["bones"][1]["rotation"], 50.0);
        assert_eq!(base["content"][0]["data"], render(&mut s, json!([{"anim":"z","enabled":false},{"anim":"a","alpha":0}]))["content"][0]["data"]);
        for config in [json!([{"anim":"missing"}]), json!([{"anim":"z","alpha":2}]), json!([{"anim":"z"},{"anim":"z"}]), Value::Null] {
            assert_eq!(render(&mut s, config)["isError"], true);
        }
    }

    #[test]
    fn shear_and_inheritance_ops_are_visible_in_pose_and_render() {
        let mut s = session("shear_inherit");
        let result = call(&mut s, "skel2d_apply_ops", json!({"ops":[
            {"op":"add_bone","name":"parent","rotation":90,"scale_x":2,"scale_y":3},
            {"op":"add_bone","name":"child","parent":"parent","x":5,"length":10,"inherit":"only_translation"},
            {"op":"key_bone","animation":"demo","bone":"child","time":0,"shear":[0,30]}
        ]}));
        assert!(result.get("isError").is_none());
        let result = call(&mut s, "skel2d_pose", json!({"anim":"demo"}));
        let report: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        // Bones: root (every new document has it), parent, child.
        assert_eq!(report["bones"][2]["inherit"], "only_translation");
        assert_eq!(report["bones"][2]["shear_y"], 30.0);
        assert_eq!(report["bones"][2]["rotation"], 0.0);
        assert!((report["bones"][2]["world"]["c"].as_f64().unwrap() + 0.5).abs() < 0.0001);
        assert_eq!(call(&mut s, "skel2d_render", json!({"anim":"demo","width":64,"height":64}))["content"][0]["type"], "image");
    }

    fn write_png(dir: &Path, w: u32, h: u32) {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(200, 60, 60, 255);
        let r = tiny_skia::Rect::from_xywh(w as f32 * 0.4, 10.0, w as f32 * 0.2, h as f32 - 20.0).unwrap();
        pm.fill_rect(r, &p, tiny_skia::Transform::identity(), None);
        pm.save_png(dir.join("c.png")).unwrap();
    }

    #[test]
    fn image_info_returns_image() {
        let mut s = session("imginfo");
        write_png(&base_dir(&s.file), 2000, 100);
        let r = call(&mut s, "skel2d_image_info", json!({ "image": "c.png" }));
        assert!(r.get("isError").is_none(), "{r}");
        assert!(r["content"][0]["text"].as_str().unwrap().contains("\"width\":2000"));
        assert_eq!(r["content"][1]["type"], "image");
        let r = call(&mut s, "skel2d_image_info", json!({ "image": "c.png", "grid": false }));
        assert_eq!(r["content"].as_array().unwrap().len(), 1);
        assert_eq!(call(&mut s, "skel2d_image_info", json!({ "image": "nope.png" }))["isError"], true);
    }

    #[test]
    fn rig_replaces_document_and_undoes() {
        let mut s = session("rig");
        write_png(&base_dir(&s.file), 100, 200);
        let spec = json!({ "template": "chain", "segments": 3, "root": "bottom" });
        let r = call(&mut s, "skel2d_rig", json!({ "image": "c.png", "spec": spec }));
        assert!(r.get("isError").is_none(), "{r}");
        assert!(r["content"][0]["text"].as_str().unwrap().contains("\"bones\":4"), "{r}");
        assert_eq!(skel2d_core::load(&s.file).unwrap().bones.len(), 4);
        assert!(call(&mut s, "skel2d_undo", json!({})).get("isError").is_none());
        // Undo restores the new document: just the root bone.
        assert_eq!(skel2d_core::load(&s.file).unwrap().bones.len(), 1);
        assert_eq!(call(&mut s, "skel2d_rig", json!({ "image": "c.png", "spec": { "template": "x" } }))["isError"], true);
    }


    fn write_rect_png(path: &Path, w: u32, h: u32) {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(30, 120, 200, 255));
        pm.save_png(path).unwrap();
    }

    #[test]
    fn slice_cuts_sheet() {
        let mut s = session("slice");
        let dir = base_dir(&s.file);
        let mut sheet = tiny_skia::Pixmap::new(300, 100).unwrap();
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(0, 0, 0, 255);
        for x in [20.0, 60.0, 220.0] {
            sheet.fill_rect(tiny_skia::Rect::from_xywh(x, 30.0, 20.0, 20.0).unwrap(), &p, tiny_skia::Transform::identity(), None);
        }
        sheet.save_png(dir.join("sheet.png")).unwrap();
        let r = call(&mut s, "skel2d_slice", json!({ "sheet": "sheet.png", "out_dir": "cut", "prefix": "p", "merge": 30 }));
        assert!(r.get("isError").is_none(), "{r}");
        let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["pieces"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["pieces"][0]["size"], json!([68, 28]));
        assert!(dir.join("cut").join("p_01.png").exists() && dir.join("cut").join("p_02.png").exists());
        // Existing files are not overwritten.
        assert_eq!(call(&mut s, "skel2d_slice", json!({ "sheet": "sheet.png", "out_dir": "cut", "prefix": "p", "merge": 30 }))["isError"], true);
        assert_eq!(call(&mut s, "skel2d_slice", json!({ "sheet": "sheet.png" }))["isError"], true);
    }

    #[test]
    fn rig_accepts_layer_manifest() {
        let mut s = session("layers");
        let parts = base_dir(&s.file).join("parts");
        std::fs::create_dir_all(&parts).unwrap();
        write_rect_png(&parts.join("body.png"), 100, 200);
        write_rect_png(&parts.join("eye.png"), 10, 10);
        write_rect_png(&parts.join("eye_closed.png"), 10, 4);
        let manifest = json!({ "layers": [
            { "name": "body", "image": "body.png", "bone": "root" },
            { "name": "eye", "image": "eye.png", "center": [50, 60], "bone": "root",
              "variants": [{ "name": "eye_closed", "image": "eye_closed.png" }] } ] });
        std::fs::write(parts.join("layers.json"), manifest.to_string()).unwrap();
        let spec = json!({ "template": "custom", "bones": [{ "name": "root", "start": [50, 190], "end": [50, 100] }] });
        let r = call(&mut s, "skel2d_rig", json!({ "image": "parts/layers.json", "spec": spec }));
        assert!(r.get("isError").is_none(), "{r}");
        let doc = skel2d_core::load(&s.file).unwrap();
        assert_eq!(doc.slots.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["body", "eye"]);
        let eye = doc.slot("eye").unwrap();
        assert!(eye.attachment("eye_closed").is_some());
        let skel2d_core::Attachment::Region(region) = eye.attachment("eye").unwrap() else { panic!("eye is a region") };
        assert_eq!(region.image.as_deref(), Some("parts/eye.png"));
        let bad = call(&mut s, "skel2d_rig", json!({ "image": "parts/missing.json", "spec": spec }));
        assert_eq!(bad["isError"], true);
    }

    #[test]
    fn render_supports_tracks_center() {
        let mut s = session("expr");
        write_rect_png(&base_dir(&s.file).join("a.png"), 20, 20);
        write_rect_png(&base_dir(&s.file).join("b.png"), 20, 20);
        let ops = json!([
            { "op": "add_bone", "name": "root", "length": 10 },
            { "op": "add_slot", "name": "face", "bone": "root", "attachment": "a", "attachments": [
                { "type": "region", "name": "a", "image": "a.png", "width": 20, "height": 20 },
                { "type": "region", "name": "b", "image": "b.png", "width": 20, "height": 20 }] },
            { "op": "add_animation", "name": "hide", "duration": 1 },
            { "op": "key_slot", "animation": "hide", "slot": "face", "time": 0, "attachment": null }
        ]);
        let ok = call(&mut s, "skel2d_apply_ops", json!({ "ops": ops }));
        assert!(ok.get("isError").is_none(), "{ok}");
        let data = |s: &mut Session, extra: Value| {
            let mut args = json!({ "width": 64, "height": 64, "zoom": 2, "bones": false });
            args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let r = call(s, "skel2d_render", args);
            assert!(r.get("isError").is_none(), "{r}");
            r["content"][0]["data"].as_str().unwrap().to_string()
        };
        let base = data(&mut s, json!({}));
        assert_ne!(base, data(&mut s, json!({ "tracks": ["hide"] })));
        assert_ne!(base, data(&mut s, json!({ "center": [30, 0] })));
        assert_eq!(call(&mut s, "skel2d_render", json!({ "tracks": ["nope"] }))["isError"], true);
    }
    #[test]
    fn protocol_errors() {
        let mut s = session("proto");
        assert_eq!(rpc(&mut s, "bogus", json!({}))["error"]["code"], -32601);
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&mut s, &note).is_none());
        assert_eq!(handle(&mut s, &json!({ "id": 3 })).unwrap()["error"]["code"], -32600);
    }

    #[test]
    fn base64_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }
}
