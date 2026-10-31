//! GIF export from the editor: the form's data and the background worker.
//! The document is copied, so editing goes on while it runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use skel2d_core::Document;
use skel2d_render::{GifJob, GifOptions, GifStats, MAX_GIF_FPS, gif_frame_count, gif_plan, render_gif};

use crate::app::EditorApp;
use crate::i18n::tr;
use crate::theme;

#[derive(Clone, Debug, PartialEq)]
pub struct GifRequest {
    pub opts: GifOptions,
    pub anim: Option<String>,
    pub all_animations: bool,
    pub all_skins: bool,
    pub file: String,
    pub folder: String,
}

impl GifRequest {
    pub fn new(app: &EditorApp) -> Self {
        let dir = app.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
        let anim = app.playback.anim.clone().or_else(|| app.doc.animations.keys().next().cloned());
        let mut request = Self {
            opts: app.settings.gif.clone(),
            anim,
            all_animations: false,
            all_skins: false,
            file: String::new(),
            folder: dir.join("gif").display().to_string(),
        };
        request.file = dir.join(request.default_name(&app.doc)).display().to_string();
        request
    }

    fn default_name(&self, doc: &Document) -> String {
        let base = if doc.name.is_empty() { "untitled" } else { &doc.name };
        let name = match &self.anim {
            Some(anim) => format!("{base}-{anim}"),
            None => base.to_string(),
        };
        format!("{}.gif", skel2d_render::safe_file_name(&name))
    }

    /// The animation may have been renamed or removed since the last export.
    pub fn still_valid(&self, doc: &Document) -> bool {
        self.anim.as_ref().is_none_or(|a| doc.animations.contains_key(a))
    }

    fn batch(&self) -> bool {
        self.all_animations || self.all_skins
    }

    fn job(&self, app: &EditorApp) -> GifJob {
        GifJob { anim: self.anim.clone(), layers: app.playback.layers.clone(), skins: app.skins.clone() }
    }

    /// Every output file with its job.
    fn plan(&self, app: &EditorApp) -> Vec<(PathBuf, GifJob)> {
        gif_plan(&app.doc, &self.job(app), self.all_animations, self.all_skins).into_iter().map(|(stem, job)| {
            let path = if self.batch() { Path::new(&self.folder).join(format!("{stem}.gif")) } else { PathBuf::from(&self.file) };
            (path, job)
        }).collect()
    }

    pub fn error(&self, app: &EditorApp) -> Option<String> {
        let target = if self.batch() { &self.folder } else { &self.file };
        if target.trim().is_empty() {
            return Some(tr("Choose where to save").to_string());
        }
        (!self.all_animations && gif_frame_count(&app.doc, &self.job(app), &self.opts_ready()) == 0).then(|| tr("The frame range is empty").to_string())
    }

    fn opts_ready(&self) -> GifOptions {
        GifOptions { fps: self.opts.fps.clamp(1.0, MAX_GIF_FPS), ..self.opts.clone() }
    }

    fn summary(&self, app: &EditorApp) -> String {
        if self.batch() {
            return crate::tf!("{} files", self.plan(app).len());
        }
        let job = self.job(app);
        let frames = gif_frame_count(&app.doc, &job, &self.opts);
        let fps = self.opts.fps.max(1.0);
        crate::tf!("{} frames, {} seconds", frames, format!("{:.2}", frames as f32 / fps))
    }

    pub fn ui(&mut self, app: &EditorApp, ui: &mut egui::Ui) {
        let doc = &app.doc;
        theme::field(ui, tr("What to export"), "", |ui| {
            let shown = if self.all_animations { tr("All animations (one file each)").to_string() } else { self.anim.clone().unwrap_or_else(|| tr("Setup pose").to_string()) };
            let before = (self.anim.clone(), self.all_animations);
            egui::ComboBox::from_id_salt("gif_anim").width(240.0).selected_text(shown).show_ui(ui, |ui| {
                if ui.selectable_label(self.all_animations, tr("All animations (one file each)")).clicked() {
                    self.all_animations = true;
                }
                if ui.selectable_label(!self.all_animations && self.anim.is_none(), tr("Setup pose")).clicked() {
                    (self.all_animations, self.anim) = (false, None);
                }
                for name in doc.animations.keys() {
                    if ui.selectable_label(!self.all_animations && self.anim.as_ref() == Some(name), name).clicked() {
                        (self.all_animations, self.anim) = (false, Some(name.clone()));
                    }
                }
            });
            // Follows the pick unless the file name was changed by hand.
            if before != (self.anim.clone(), self.all_animations) && !self.all_animations {
                let path = Path::new(&self.file);
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                self.file = dir.join(self.default_name(doc)).display().to_string();
            }
        });
        if !doc.skins.is_empty() {
            theme::field(ui, tr("Skins"), "", |ui| {
                theme::segmented(ui, &mut self.all_skins, &[(false, tr("Previewed")), (true, tr("One file per skin"))]);
            });
        }
        let batch = self.batch();
        theme::field(ui, tr(if batch { "Output folder" } else { "Output file" }), "", |ui| {
            let browse = ui.button(tr("Browse..."));
            let target = if batch { &mut self.folder } else { &mut self.file };
            ui.add(egui::TextEdit::singleline(target).desired_width(ui.available_width()));
            if browse.clicked() {
                if batch {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { *target = p.display().to_string(); }
                } else {
                    let name = Path::new(target.as_str()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if let Some(p) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name(name).save_file() { *target = p.display().to_string(); }
                }
            }
        });
        let summary = self.summary(app);
        theme::hint(ui, &summary);
        ui.add_space(6.0);

        let o = &mut self.opts;
        theme::group(ui, "gif_timing", tr("Timing"), "", true, |ui| {
            theme::field(ui, "FPS", tr("Frames per second. 50 is the smoothest a GIF can play."), |ui| { ui.add(egui::DragValue::new(&mut o.fps).range(1.0..=MAX_GIF_FPS).speed(0.5)); });
            ui.checkbox(&mut o.forever, tr("Loop forever"));
            ui.checkbox(&mut o.include_last_frame, tr("Include last frame")).on_hover_text(tr("Leave this off for loops: the last frame repeats the first"));
            theme::field(ui, tr("Plays"), tr("Times the animation is stored in the file"), |ui| { ui.add(egui::DragValue::new(&mut o.plays).range(1..=100)); });
            theme::field(ui, tr("Pause after (seconds)"), tr("Holds the last frame of each play"), |ui| { ui.add(egui::DragValue::new(&mut o.pause_after).range(0.0..=60.0).speed(0.05)); });
            theme::field(ui, tr("Warm up (plays)"), tr("Plays simulated first, so physics starts in motion"), |ui| { ui.add(egui::DragValue::new(&mut o.warm_up).range(0..=20)); });
            let mut some = o.range.is_some();
            if ui.checkbox(&mut some, tr("Only some frames")).changed() {
                o.range = some.then_some([0, 10]);
            }
            if let Some(range) = &mut o.range {
                theme::field(ui, tr("First frame"), "", |ui| { ui.add(egui::DragValue::new(&mut range[0])); });
                theme::field(ui, tr("Last frame"), "", |ui| { ui.add(egui::DragValue::new(&mut range[1])); });
            }
        });
        theme::group(ui, "gif_size", tr("Size"), "", true, |ui| {
            let mut fit = o.fit.is_some();
            if ui.checkbox(&mut fit, tr("Fit into pixels")).changed() {
                o.fit = fit.then_some([512, 512]);
            }
            match &mut o.fit {
                Some(limit) => {
                    theme::field(ui, tr("Width"), "", |ui| { ui.add(egui::DragValue::new(&mut limit[0]).range(1..=4096)); });
                    theme::field(ui, tr("Height"), "", |ui| { ui.add(egui::DragValue::new(&mut limit[1]).range(1..=4096)); });
                    ui.checkbox(&mut o.enlarge, tr("Enlarge small content"));
                    ui.checkbox(&mut o.pad, tr("Exact size with space around"));
                }
                None => {
                    theme::field(ui, tr("Scale (%)"), "", |ui| { ui.add(egui::DragValue::new(&mut o.scale).range(1.0..=1000.0)); });
                }
            }
            let mut crop = o.crop.is_some();
            if ui.checkbox(&mut crop, tr("Crop to a world rectangle")).changed() {
                o.crop = crop.then_some([-100.0, 0.0, 200.0, 200.0]);
            }
            if let Some(rect) = &mut o.crop {
                ui.horizontal(|ui| {
                    for (value, label) in rect.iter_mut().zip(["X", "Y", "W", "H"]) {
                        ui.label(label);
                        ui.add(egui::DragValue::new(value));
                    }
                });
            }
        });
        theme::group(ui, "gif_colors", tr("Colors"), "", true, |ui| {
            theme::field(ui, tr("Number of colors"), tr("Fewer colors make a smaller file"), |ui| { ui.add(egui::DragValue::new(&mut o.colors).range(2..=256)); });
            ui.checkbox(&mut o.dither, tr("Dither")).on_hover_text(tr("Smoother gradients, larger file"));
            theme::field(ui, tr("Quality"), tr("Higher quality has better colors and takes longer"), |ui| { ui.add(egui::Slider::new(&mut o.quality, 1..=10)); });
            theme::field(ui, tr("Background"), "", |ui| { ui.color_edit_button_srgb(&mut o.background); });
            ui.checkbox(&mut o.transparent, tr("Transparent background")).on_hover_text(tr("GIF pixels are either solid or see-through, so soft edges mix with the background color"));
            if o.transparent {
                theme::field(ui, tr("Alpha threshold"), tr("Pixels fainter than this become transparent"), |ui| { ui.add(egui::Slider::new(&mut o.alpha_threshold, 1..=255)); });
            }
        });
        theme::group(ui, "gif_show", tr("Show"), "", true, |ui| {
            ui.checkbox(&mut o.bones, tr("Draw bones"));
            ui.checkbox(&mut o.physics, tr("Simulate physics"));
        });
    }
}

enum Message {
    Progress { file: usize, done: u32, total: u32 },
    Finished(Result<Vec<(PathBuf, GifStats, u64)>, String>),
}

pub struct RunningExport {
    receiver: Receiver<Message>,
    cancel: Arc<AtomicBool>,
    names: Vec<String>,
    file: usize,
    fraction: f32,
}

/// Replaces the running export, whose thread stops at its next frame.
pub fn start(app: &mut EditorApp, request: GifRequest) {
    if let Some(old) = &app.gif_export {
        old.cancel.store(true, Ordering::Relaxed);
    }
    app.settings.gif = request.opts.clone();
    let plan = request.plan(app);
    let (doc, base, opts) = (app.doc.clone(), app.base_dir(), request.opts.clone());
    let batch = request.batch().then(|| PathBuf::from(&request.folder));
    app.last_gif = Some(request);
    let (sender, receiver) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let names = plan.iter().map(|(path, _)| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()).collect();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        let run = || -> Result<Vec<(PathBuf, GifStats, u64)>, String> {
            if let Some(folder) = &batch {
                std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            }
            let mut written = Vec::new();
            for (file, (path, job)) in plan.iter().enumerate() {
                let mut bytes = Vec::new();
                let stats = render_gif(&doc, &base, job, &opts, &mut bytes, &mut |done, total| {
                    let _ = sender.send(Message::Progress { file, done, total });
                    !stop.load(Ordering::Relaxed)
                })?;
                // Written only when complete, so a failure leaves any older file alone.
                std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                written.push((path.clone(), stats, bytes.len() as u64));
            }
            Ok(written)
        };
        let result = run();
        let _ = sender.send(Message::Finished(result));
    });
    app.gif_export = Some(RunningExport { receiver, cancel, names, file: 0, fraction: 0.0 });
}

/// A stable batch progress state for scripted screenshots.
pub fn preview_progress(app: &mut EditorApp) {
    let (_, receiver) = channel();
    app.gif_export = Some(RunningExport {
        receiver,
        cancel: Arc::new(AtomicBool::new(false)),
        names: (1..=5).map(|n| format!("animation-{n}.gif")).collect(),
        file: 1,
        fraction: 0.42,
    });
}

pub fn repeat_last(app: &mut EditorApp) {
    match app.last_gif.clone().filter(|r| r.still_valid(&app.doc)) {
        Some(request) => start(app, request),
        None => app.status = tr("Nothing exported yet. Ctrl+E opens the export window.").to_string(),
    }
}

fn finished_text(written: &[(PathBuf, GifStats, u64)], folder: Option<&Path>) -> String {
    let missing: Vec<&str> = written.iter().flat_map(|w| w.1.missing_images.iter().map(String::as_str)).collect();
    let text = match written {
        [(path, stats, bytes)] if folder.is_none() => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::tf!("Exported {}: {}x{}, {} frames, {} KB", name, stats.width, stats.height, stats.frames, bytes / 1024)
        }
        _ => crate::tf!("Exported {} GIF files to {}", written.len(), folder.map(|f| f.display().to_string()).unwrap_or_default()),
    };
    if missing.is_empty() { text } else { format!("{text} · {}", crate::tf!("Images not found: {}", missing.join(", "))) }
}

/// Follows the worker and shows its progress with a way to cancel.
pub fn show_progress(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(export) = &mut app.gif_export else { return };
    let mut finished = None;
    while let Ok(message) = export.receiver.try_recv() {
        match message {
            Message::Progress { file, done, total } => {
                export.file = file;
                export.fraction = done as f32 / total.max(1) as f32;
            }
            Message::Finished(result) => finished = Some(result),
        }
    }
    if let Some(result) = finished {
        let folder = app.last_gif.as_ref().filter(|r| r.batch()).map(|r| PathBuf::from(&r.folder));
        app.status = match result {
            Ok(written) => finished_text(&written, folder.as_deref()),
            Err(e) if e == "cancelled" => tr("GIF export cancelled").to_string(),
            Err(e) => crate::tf!("GIF export failed: {}", e),
        };
        app.gif_export = None;
        return;
    }
    egui::Window::new(tr("Exporting GIF"))
        .id(egui::Id::unique("gif_progress"))
        .title_bar(false)
        .frame(theme::popup_frame())
        .collapsible(false)
        .resizable(false)
        .default_width(theme::TOAST_W - theme::DIALOG_GAP * 2.0)
        .max_width(theme::TOAST_W - theme::DIALOG_GAP * 2.0)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-theme::DIALOG_GAP, 96.0))
        .show(ctx, |ui| {
            ui.set_min_width(260.0);
            let name = export.names.get(export.file).map(String::as_str).unwrap_or_default();
            theme::dialog_title(ui, tr("Exporting GIF"), name);
            if export.names.len() > 1 {
                ui.label(egui::RichText::new(crate::tf!("File {} of {}", export.file + 1, export.names.len())).color(theme::TEXT_DIM));
            }
            ui.label(egui::RichText::new(format!("{:.0}%", export.fraction * 100.0)).strong().color(theme::ACCENT));
            let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, 6.0, theme::HEADER_BG);
            let filled = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * export.fraction.clamp(0.0, 1.0), bar.height()));
            ui.painter().rect_filled(filled, 6.0, theme::ACCENT);
            ui.add_space(theme::DIALOG_GAP);
            let cancelling = export.cancel.load(Ordering::Relaxed);
            if cancelling {
                ui.label(egui::RichText::new(tr("Cancelling...")).color(theme::TEXT_DIM));
            }
            if ui.add_enabled(!cancelling, theme::dialog_button(tr("Cancel"), theme::Emphasis::Plain)).clicked() {
                export.cancel.store(true, Ordering::Relaxed);
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;
    use skel2d_core::{apply, parse_ops};

    #[test]
    fn background_export_writes_the_file_and_reports_it() {
        let dir = std::env::temp_dir().join(format!("skel2d-editor-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ops = parse_ops(r#"[{"op":"add_slot","name":"s","bone":"root","attachment":"r","attachments":[{"type":"region","name":"r","width":40,"height":20}]},
            {"op":"key_bone","animation":"spin","bone":"root","time":0,"rotate":0},
            {"op":"key_bone","animation":"spin","bone":"root","time":1,"rotate":360}]"#).unwrap();
        let doc = apply(&Document::with_root("t"), &ops).unwrap();
        let mut app = EditorApp::test_app(doc, dir.join("t.skel.json"));
        app.playback.anim = Some("spin".to_string());
        let mut request = GifRequest::new(&app);
        assert!(request.file.ends_with("t-spin.gif"), "{}", request.file);
        request.opts.fps = 5.0;
        request.opts.background = [0, 0, 0];
        let file = PathBuf::from(&request.file);
        assert_eq!(request.error(&app), None);
        start(&mut app, request);
        let export = app.gif_export.take().unwrap();
        let written = loop {
            if let Message::Finished(result) = export.receiver.recv().unwrap() { break result.unwrap(); }
        };
        assert_eq!((written.len(), written[0].1.frames), (1, 5));
        assert_eq!(written[0].2, std::fs::metadata(&file).unwrap().len());
        assert!(app.last_gif.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
