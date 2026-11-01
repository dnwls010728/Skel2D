use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use skel2d_core::pose::Track;
use skel2d_render::{GifJob, GifOptions, GifStats, gif_plan, render_gif};
use skel2d_core::{Document, MovePath, PhysicsRun, Pose};
use skel2d_render::{RenderOptions, Renderer};

use crate::{PhysicsArgs, RenderArgs, GifArgs};

pub fn error_json(err: &anyhow::Error) -> Value {
    match err.downcast_ref::<skel2d_core::Error>() {
        Some(skel2d_core::Error::Invalid(issues)) => json!({ "error": "document is invalid", "issues": issues }),
        Some(skel2d_core::Error::Op { index, message }) => json!({ "error": message, "op_index": index }),
        _ => json!({ "error": format!("{err:#}") }),
    }
}

pub fn base_dir(file: &Path) -> PathBuf {
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

pub fn new(file: &Path, name: Option<String>, force: bool) -> Result<Value> {
    if file.exists() && !force {
        bail!("{} already exists (use --force to overwrite)", file.display());
    }
    let name = name.unwrap_or_else(|| {
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        stem.split('.').next().unwrap_or_default().to_string()
    });
    skel2d_core::save(file, &Document::with_root(name))?;
    Ok(json!({ "created": file }))
}

pub fn validate(file: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(file).with_context(|| file.display().to_string())?;
    let doc = skel2d_core::from_str(&text)?;
    let issues = skel2d_core::validate(&doc);
    if issues.is_empty() {
        Ok(json!({ "valid": true }))
    } else {
        Err(skel2d_core::Error::Invalid(issues).into())
    }
}

pub fn info(file: &Path) -> Result<Value> {
    Ok(info_json(&skel2d_core::load(file)?))
}

pub fn info_json(doc: &Document) -> Value {
    let resolved_skins: Vec<_> = std::iter::once(("default".to_string(), Vec::new()))
        .chain(doc.skins.iter().map(|s| (s.name.clone(), vec![s.name.clone()])))
        .map(|(skin, active)| json!({
            "skin":skin,
            "slots":doc.slots.iter().enumerate().map(|(i, slot)| {
                let found = slot.attachment.as_deref().and_then(|name| doc.resolve_attachment_from(i, name, &active));
                json!({"slot":slot.name,"attachment":found.map(|(_,a)|a.name()),"source_skin":found.map(|(k,_)|k.unwrap_or("default"))})
            }).collect::<Vec<_>>()
        })).collect();
    json!({
        "name": doc.name,
        "bones": doc.bones.iter().map(|b| json!({ "name": b.name, "parent": b.parent })).collect::<Vec<_>>(),
        "slots": doc.slots.iter().map(|s| json!({
            "name": s.name,
            "bone": s.bone,
            "attachment": s.attachment,
            "attachments": s.attachments.iter().map(|a| a.name()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "animations": doc.animations.iter().map(|(name, a)| json!({
            "name": name,
            "duration": a.duration(),
            "bones": a.bones.keys().collect::<Vec<_>>(),
            "slots": a.slots.keys().collect::<Vec<_>>(),
            "draw_order_keys": a.draw_order.len(),
            "event_keys": a.events.len(),
        })).collect::<Vec<_>>(),
        "events": doc.events,
        "resolved_skins": resolved_skins,
    })
}

pub fn apply(file: &Path, ops_file: Option<PathBuf>, ops: Option<String>, dry_run: bool) -> Result<Value> {
    let text = match (ops, ops_file) {
        (Some(inline), _) => inline,
        (None, Some(p)) if p.as_os_str() == "-" => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
        (None, Some(p)) => std::fs::read_to_string(&p).with_context(|| p.display().to_string())?,
        (None, None) => bail!("give an ops file, \"-\" for stdin, or --ops '<json>'"),
    };
    let ops = skel2d_rig::parse_ops(&text)?;
    let doc = skel2d_core::load(file)?;
    let report = skel2d_rig::apply_any(&doc, &base_dir(file), &ops)?;
    if !dry_run {
        skel2d_core::save(file, &report.document)?;
    }
    Ok(json!({ "applied": ops.len(), "expanded":report.ops.len(), "meshes":report.meshes, "pruned_influences":report.pruned_influences, "saved": !dry_run }))
}

pub fn pose(file: &Path, anim: Option<&str>, time: f32, since: Option<f32>, layers: Option<&str>, skins: &[String], physics: &PhysicsArgs) -> Result<Value> {
    let doc = skel2d_core::load(file)?;
    pose_json(&doc, anim, time, since, layers, skins, physics)
}

/// The simulation to run, if physics is on and the document has any.
fn physics_run(doc: &Document, args: &PhysicsArgs) -> Result<Option<PhysicsRun>> {
    match args.physics.as_deref() {
        None | Some("simulate") => {}
        Some("none") => return Ok(None),
        Some(other) => bail!("unknown physics mode {other:?}, expected \"simulate\" or \"none\""),
    }
    let fps = args.fps.unwrap_or(60.0);
    if !(1.0..=1000.0).contains(&fps) { bail!("fps must be between 1 and 1000"); }
    let path = MovePath::parse(args.moves.as_deref().unwrap_or("")).map_err(|e| anyhow!(e))?;
    Ok((!doc.physics.is_empty()).then(|| PhysicsRun::new(fps, path)))
}

/// `since`: the clock of the previous look; the report's `events` are those
/// fired after it, up to `time`. Omitted: everything from the start.
pub fn pose_json(doc: &Document, anim: Option<&str>, time: f32, since: Option<f32>, layers: Option<&str>, skins: &[String], physics: &PhysicsArgs) -> Result<Value> {
    find_anim(doc, anim)?;
    if let Some(s) = skins.iter().find(|s| doc.skin(s).is_none()) { bail!("skin {s:?} does not exist"); }
    let layers = skel2d_core::parse_layer_tracks(doc, layers.unwrap_or("[]"))?;
    let tracks_at = |t: f32| skel2d_core::pose::preview_tracks(doc, anim, t, &layers);
    let pose = match physics_run(doc, physics)?.filter(|_| !tracks_at(time).is_empty()) {
        Some(mut run) => run.advance(doc, &tracks_at, skins, time),
        None => Pose::compute_skinned(doc, &tracks_at(time), skins),
    };
    let mut report = serde_json::to_value(pose.report(doc))?;
    report["active_skins"] = json!(skins);
    let before = since.map(tracks_at);
    let events: Vec<_> = tracks_at(time).iter().enumerate()
        .flat_map(|(i, track)| skel2d_core::pose::fired_events(doc, track.animation, before.as_ref().map(|b| b[i].time), track.time))
        .collect();
    if !events.is_empty() { report["events"] = json!(events); }
    Ok(report)
}

fn find_anim<'a>(doc: &'a Document, anim: Option<&str>) -> Result<Option<&'a skel2d_core::Animation>> {
    anim.map(|name| doc.animations.get(name).ok_or_else(|| anyhow!("animation {name:?} does not exist")))
        .transpose()
}

pub fn render_options(args: &RenderArgs) -> RenderOptions {
    RenderOptions {
        width: args.width.clamp(1, 4096),
        height: args.height.clamp(1, 4096),
        zoom: args.zoom,
        center: args.center,
        draw_bones: !args.no_bones,
        ..RenderOptions::default()
    }
}

/// Renders to PNG bytes. Returns the bytes and the images that failed to load.
pub fn render_bytes(doc: &Document, base: &Path, args: &RenderArgs) -> Result<(Vec<u8>, Vec<String>)> {
    let opts = render_options(args);
    let animation = find_anim(doc, args.anim.as_deref())?;
    let layers = skel2d_core::parse_layer_tracks(doc, args.layers.as_deref().unwrap_or("[]"))?;
    if let Some(s) = args.skins.iter().find(|s| doc.skin(s).is_none()) {
        return Err(anyhow!("skin {s:?} does not exist"));
    }
    // Extra tracks: (animation, fixed time or follow the frame clock).
    let extra = args
        .tracks
        .iter()
        .map(|t| {
            let (name, time) = match t.split_once('@') {
                Some((n, s)) => (n, Some(s.parse::<f32>().map_err(|_| anyhow!("--track {t:?}: bad time"))?)),
                None => (t.as_str(), None),
            };
            Ok((find_anim(doc, Some(name))?.expect("name given"), time))
        })
        .collect::<Result<Vec<_>>>()?;
    let tracks_at = |clock: f32| {
        let mut tracks: Vec<Track> = animation.map(|a| Track { animation: a, time: clock, alpha: 1.0 }).into_iter().collect();
        for &(a, fixed) in &extra {
            let d = a.duration();
            let time = fixed.unwrap_or(if d > 0.0 { clock % d } else { 0.0 });
            tracks.push(Track { animation: a, time, alpha: 1.0 });
        }
        let configured = skel2d_core::pose::preview_tracks(doc, args.anim.as_deref(), clock, &layers);
        tracks.extend(configured.into_iter().skip(usize::from(animation.is_some())));
        tracks
    };
    // Physics needs something animated; one run spans every frame in order.
    let mut run = physics_run(doc, &args.physics)?.filter(|_| !tracks_at(args.time).is_empty());
    let mut pose_at = |clock: f32| match &mut run {
        Some(run) => run.advance(doc, &tracks_at, &args.skins, clock),
        None => Pose::compute_skinned(doc, &tracks_at(clock), &args.skins),
    };
    let mut renderer = Renderer::new(base);
    let pixmap = match args.frames {
        Some(n) => {
            if n == 0 || n > 64 {
                bail!("--frames must be between 1 and 64");
            }
            // Frames span the main animation, or the longest extra track.
            let span = animation
                .map(|a| a.duration())
                .unwrap_or_else(|| extra.iter().map(|(a, _)| a.duration())
                    .chain(layers.iter().filter(|l| l.enabled && l.alpha > 0.0).filter_map(|l| doc.animations.get(&l.anim)).map(|a| a.duration()))
                    .fold(0.0, f32::max));
            if animation.is_none() && extra.is_empty() && !layers.iter().any(|l| l.enabled && l.alpha > 0.0) {
                bail!("--frames needs --anim, --track or an enabled --layers entry");
            }
            let poses: Vec<Pose> = (0..n)
                .map(|i| pose_at(if n == 1 { args.time } else { span * i as f32 / (n - 1) as f32 }))
                .collect();
            renderer.render_poses(doc, &poses, &opts)
        }
        None => renderer.render(doc, &pose_at(args.time), &opts, None),
    };
    let png = pixmap.encode_png().map_err(|e| anyhow!("PNG encoding failed: {e}"))?;
    Ok((png, renderer.missing_images))
}

pub fn render(file: &Path, out: &Path, args: &RenderArgs) -> Result<Value> {
    let doc = skel2d_core::load(file)?;
    let (png, missing) = render_bytes(&doc, &base_dir(file), args)?;
    std::fs::write(out, png).with_context(|| out.display().to_string())?;
    Ok(json!({ "out": out, "missing_images": missing }))
}

/// Writes one GIF per planned job: `out` is a file, or a folder for a batch.
pub fn gif_export(doc: &Document, base: &Path, job: &GifJob, opts: &GifOptions, out: &Path, all_animations: bool, all_skins: bool) -> Result<Value> {
    let plan = gif_plan(doc, job, all_animations, all_skins);
    let batch = all_animations || all_skins;
    if batch {
        std::fs::create_dir_all(out).with_context(|| out.display().to_string())?;
    }
    let mut files = Vec::new();
    for (stem, job) in &plan {
        let path = if batch { out.join(format!("{stem}.gif")) } else { out.to_path_buf() };
        let (bytes, stats) = gif_bytes(doc, base, job, opts)?;
        std::fs::write(&path, &bytes).with_context(|| path.display().to_string())?;
        files.push(json!({ "out": path, "bytes": bytes.len(), "stats": stats }));
    }
    Ok(json!({ "files": files }))
}

pub fn gif_bytes(doc: &Document, base: &Path, job: &GifJob, opts: &GifOptions) -> Result<(Vec<u8>, GifStats)> {
    let mut bytes = Vec::new();
    let stats = render_gif(doc, base, job, opts, &mut bytes, &mut |_, _| true).map_err(|e| anyhow!(e))?;
    Ok((bytes, stats))
}

fn gif_inputs(file: &Path, args: &GifArgs) -> Result<(Document, GifJob, GifOptions)> {
    let doc = skel2d_core::load(file)?;
    let job = GifJob {
        anim: args.anim.clone(),
        layers: args.tracks.iter().map(skel2d_core::LayerTrack::new).collect(),
        skins: args.skins.clone(),
    };
    if let Some(t) = job.layers.iter().find(|t| !doc.animations.contains_key(&t.anim)) { bail!("animation {:?} does not exist", t.anim); }
    let hex = skel2d_core::Color::parse_hex(&args.background).ok_or_else(|| anyhow!("--background {:?}: expected RRGGBB", args.background))?.to_rgba8();
    let opts = GifOptions {
        fps: args.gif_fps, colors: args.colors, dither: args.dither, quality: args.quality,
        alpha_threshold: args.alpha_threshold, transparent: args.transparent, background: [hex[0], hex[1], hex[2]],
        forever: !args.no_loop, include_last_frame: args.include_last_frame, plays: args.plays,
        pause_after: args.pause_after, warm_up: args.warm_up, bones: args.bones, physics: !args.no_physics,
        scale: args.scale, fit: args.fit, enlarge: args.enlarge, pad: args.pad, crop: args.crop, range: args.range,
    };
    Ok((doc, job, opts))
}

pub fn gif(file: &Path, out: &Path, args: &GifArgs) -> Result<Value> {
    let (doc, job, opts) = gif_inputs(file, args)?;
    gif_export(&doc, &base_dir(file), &job, &opts, out, args.all_animations, args.all_skins)
}

/// A sprite sheet per planned file: `<name>.png` and `<name>.json`.
pub fn sheet(file: &Path, out: &Path, columns: u32, padding: u32, args: &GifArgs) -> Result<Value> {
    let (doc, job, opts) = gif_inputs(file, args)?;
    let batch = args.all_animations || args.all_skins;
    if batch {
        std::fs::create_dir_all(out).with_context(|| out.display().to_string())?;
    }
    let base = base_dir(file);
    let mut files = Vec::new();
    for (stem, job) in gif_plan(&doc, &job, args.all_animations, args.all_skins) {
        let path = if batch { out.join(format!("{stem}.png")) } else { out.to_path_buf() };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let layout = skel2d_render::SheetOptions { columns, padding };
        let (png, json, stats) = skel2d_render::render_sheet(&doc, &base, &job, &opts, layout, &name, &mut |_, _| true).map_err(|e| anyhow!(e))?;
        let description = path.with_extension("json");
        std::fs::write(&path, &png).with_context(|| path.display().to_string())?;
        std::fs::write(&description, json).with_context(|| description.display().to_string())?;
        files.push(json!({ "out": path, "json": description, "bytes": png.len(), "stats": stats }));
    }
    Ok(json!({ "files": files }))
}

pub fn schema(kind: &str) -> Result<Value> {
    let schema = match kind {
        "document" => schemars::schema_for!(Document),
        "ops" => schemars::schema_for!(Vec<skel2d_rig::AnyOp>),
        "rig" => schemars::schema_for!(skel2d_rig::RigSpec),
        "layers" => schemars::schema_for!(skel2d_rig::layers::LayerManifest),
        "tracks" => schemars::schema_for!(Vec<skel2d_core::LayerTrack>),
        other => bail!("unknown schema {other:?}, expected \"document\", \"ops\", \"rig\", \"layers\" or \"tracks\""),
    };
    Ok(serde_json::to_value(schema)?)
}
