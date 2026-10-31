//! Skel2D desktop editor. Every edit goes through `skel2d_core::apply`, the
//! same op API the CLI, HTTP and MCP servers use, so the editor and AI agents
//! can work on one file side by side: external changes are reloaded live.

mod app;
mod dialogs;
mod blendpass;
mod errors;
mod export;
mod i18n;
mod icons;
mod ikbend;
mod import;
mod meshedit;
mod palette;
mod physicsview;
mod theme;
mod panels;
mod steps;
mod timeline;
mod toasts;
mod viewport;

use std::path::PathBuf;

const USAGE: &str = "usage: skel2d-editor [FILE] [--anim NAME] [--time SECONDS] [--select BONE] [--select-slot SLOT] [--select-ik NAME] [--select-physics NAME] [--dialog NAME] [--tracks A,B | --layers JSON] [--skin NAME]... [--tool NAME] [--step 1-5] [--expand] [--toast-demo] [--auto-key on|off] [--edit-rotate DEG] [--lang en|ko] [--size WxH] [--screenshot OUT.png [--wait SECONDS]]";

fn parse_args() -> Result<(app::StartOptions, [f32; 2], bool), String> {
    let mut opts = app::StartOptions::default();
    let mut size = [1280.0, 800.0];
    let mut size_given = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value\n{USAGE}"));
        match arg.to_str() {
            Some("--anim") => opts.anim = Some(value("--anim")?.to_string_lossy().into_owned()),
            Some("--time") => {
                let v = value("--time")?;
                opts.time = v.to_string_lossy().parse().map_err(|_| format!("--time: not a number: {v:?}"))?;
            }
            Some("--expand") => opts.expand = true,
            Some("--toast-demo") => opts.toast_demo = true,
            Some("--auto-key") => opts.auto_key = Some(value("--auto-key")?.to_string_lossy() != "off"),
            Some("--edit-rotate") => {
                let v = value("--edit-rotate")?;
                opts.edit_rotate = Some(v.to_string_lossy().parse().map_err(|_| format!("--edit-rotate: not a number: {v:?}"))?);
            }
            Some("--select") => opts.select = Some(value("--select")?.to_string_lossy().into_owned()),
            Some("--select-slot") => opts.select_slot = Some(value("--select-slot")?.to_string_lossy().into_owned()),
            Some("--select-ik") => opts.select_ik = Some(value("--select-ik")?.to_string_lossy().into_owned()),
            Some("--dialog") => opts.dialog = Some(value("--dialog")?.to_string_lossy().into_owned()),
            Some("--select-physics") => opts.select_physics = Some(value("--select-physics")?.to_string_lossy().into_owned()),
            Some("--skin") => opts.skins.push(value("--skin")?.to_string_lossy().into_owned()),
            Some("--tracks") => {
                opts.tracks = value("--tracks")?.to_string_lossy().split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect();
            }
            Some("--layers") => opts.layers = Some(value("--layers")?.to_string_lossy().into_owned()),
            Some("--tool") => {
                let v = value("--tool")?.to_string_lossy().to_lowercase();
                // Older names still work.
                let v = match v.as_str() {
                    "select" => "pose".to_string(),
                    "move" => "translate".to_string(),
                    "bone" => "create".to_string(),
                    _ => v,
                };
                opts.tool = Some(app::Tool::ALL.iter().map(|t| t.0).find(|t| t.name() == v).ok_or(format!("--tool: unknown {v:?} (pose, create, weights, mesh, rotate, translate, scale)"))?);
            }
            Some("--step") => {
                let v = value("--step")?;
                opts.step = Some(v.to_string_lossy().parse::<usize>().ok().filter(|n| (1..=5).contains(n)).ok_or(format!("--step: expected 1-5, got {v:?}"))?);
            }
            Some("--lang") => {
                let v = value("--lang")?.to_string_lossy().to_lowercase();
                opts.lang = Some(match v.as_str() {
                    "en" | "english" => i18n::Lang::English,
                    "ko" | "korean" => i18n::Lang::Korean,
                    _ => return Err(format!("--lang: expected en or ko, got {v:?}")),
                });
            }
            Some("--screenshot") => opts.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            Some("--wait") => {
                let v = value("--wait")?;
                opts.wait = v.to_string_lossy().parse().map_err(|_| format!("--wait: not a number: {v:?}"))?;
            }
            Some("--size") => {
                let v = value("--size")?.to_string_lossy().into_owned();
                let (w, h) = v.split_once('x').ok_or(format!("--size: expected WxH, got {v:?}"))?;
                let p = |s: &str| s.parse::<f32>().ok().filter(|n| *n >= 320.0).ok_or(format!("--size: bad value {v:?} (min 320)"));
                size = [p(w)?, p(h)?];
                size_given = true;
            }
            Some("-h" | "--help") => return Err(USAGE.to_string()),
            Some(s) if s.starts_with("--") => return Err(format!("unknown option {s}\n{USAGE}")),
            _ if opts.path.is_none() => opts.path = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.to_string()),
        }
    }
    if opts.layers.is_some() && !opts.tracks.is_empty() {
        return Err("use either --tracks or --layers".into());
    }
    Ok((opts, size, size_given))
}

fn main() -> eframe::Result {
    let (opts, size, size_given) = match parse_args() {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Skel2D")
            .with_inner_size(size)
            .with_min_inner_size([640.0, 400.0])
            .with_icon(icons::app_icon()),
        // An explicit --size must win over the remembered window geometry.
        persist_window: !size_given,
        ..Default::default()
    };
    eframe::run_native("Skel2D", options, Box::new(move |cc| Ok(Box::new(app::EditorApp::with_options(cc, opts)))))
}
