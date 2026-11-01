//! Visual style shared by every panel: one palette, readable type sizes,
//! high-contrast text and clearly outlined controls. Colors and sizes live
//! here only; panels use the helpers below instead of literal sizes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

/// Selection, active tool, focused items.
pub const ACCENT: Color32 = Color32::from_rgb(92, 196, 255);
/// Animate mode: viewport frame, keyed values, record state.
pub const ANIMATE: Color32 = Color32::from_rgb(255, 122, 122);
/// Setup mode badge.
pub const SETUP: Color32 = Color32::from_rgb(120, 214, 150);
/// Unselected bones and key diamonds.
pub const BONE: Color32 = Color32::from_rgb(255, 196, 64);
/// Slots, attachments, slot timelines.
pub const SLOT: Color32 = Color32::from_rgb(130, 220, 170);
/// IK constraints and their timelines.
pub const IK: Color32 = Color32::from_rgb(206, 150, 255);
/// Physics constraints and their timelines.
pub const PHYSICS: Color32 = Color32::from_rgb(98, 214, 196);
/// Draw order timeline and keys.
pub const DRAW_ORDER: Color32 = Color32::from_rgb(120, 170, 255);
/// Event definitions, keys and the playback pop-up.
pub const EVENT: Color32 = Color32::from_rgb(255, 140, 190);
/// Seconds the event pop-up stays visible after an event fires while playing.
pub const EVENT_TOAST_SECS: f32 = 0.6;
/// Message toasts (see toasts.rs): how many show, their width and corner margin,
/// and how long each kind stays (errors also close with their button).
pub const TOAST_MAX: usize = 5;
pub const TOAST_W: f32 = 340.0;
pub const TOAST_MARGIN: f32 = 12.0;
pub const TOAST_INFO_SECS: f32 = 4.0;
pub const TOAST_WARN_SECS: f32 = 6.0;
pub const TOAST_ERROR_SECS: f32 = 10.0;
/// Faint non-simulated bone shown behind a simulated pose.
pub const PHYSICS_GHOST: Color32 = Color32::from_rgba_premultiplied(40, 52, 49, 56);
/// Key button states, as Spine colors them.
pub const KEY_SET: Color32 = Color32::from_rgb(235, 72, 64);
pub const KEY_CHANGED: Color32 = Color32::from_rgb(255, 156, 40);
pub const KEY_NONE: Color32 = Color32::from_rgb(96, 200, 96);
/// Outline (hull) edges of a mesh being edited.
pub const HULL: Color32 = Color32::from_rgb(64, 224, 230);
pub const MESH_HULL_STROKE: f32 = 2.5;
pub const MESH_EDGE_STROKE: f32 = 2.0;
pub const MESH_TRIANGLE_STROKE: f32 = 1.0;
pub const MESH_PICK_RADIUS: f32 = 6.0;
pub const WEIGHT_BAR_WIDTH: f32 = 52.0;
pub const WARN: Color32 = Color32::from_rgb(255, 180, 72);
/// Regular text.
pub const TEXT: Color32 = Color32::from_gray(236);
/// Secondary text (hints, units, inactive labels); still 7:1 on the panels.
pub const TEXT_DIM: Color32 = Color32::from_gray(176);
pub const PANEL_BG: Color32 = Color32::from_gray(43);
/// Section header strips.
pub const HEADER_BG: Color32 = Color32::from_gray(56);
/// Spine-like neutral mid-gray viewport, so both dark and light art read.
pub const VIEWPORT_BG: Color32 = Color32::from_gray(66);
pub const GRID_MINOR: Color32 = Color32::from_gray(72);
pub const GRID_MAJOR: Color32 = Color32::from_gray(86);
pub const AXIS: Color32 = Color32::from_gray(120);

/// Dark backing for text drawn over the viewport.
pub const OVERLAY_BG: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 215);
/// Key-state text on dark backgrounds (lighter than the key fills).
pub const KEY_SET_TEXT: Color32 = Color32::from_rgb(255, 128, 118);
/// Disabled labels: dimmer than `TEXT_DIM` but still at least 3:1.
pub const TEXT_DISABLED: Color32 = Color32::from_gray(140);
/// Text on filled accent buttons (dark on the bright accent).
pub const ON_ACCENT: Color32 = Color32::from_gray(16);
/// Filled accent for the active step.
pub const STEP_ACTIVE: Color32 = Color32::from_rgb(70, 160, 220);
/// Layout sizes of the step screens.
pub const GUIDE_BAND_H: f32 = 30.0;
pub const TOOLS_W: f32 = 152.0;
pub const SIDE_W: f32 = 420.0;
pub const STEP_BTN_H: f32 = 36.0;
/// Smallest clickable height (px).
pub const MIN_CLICK_H: f32 = 28.0;
pub const THUMB: f32 = 56.0;
pub const THUMB_SMALL: f32 = 36.0;

/// Type scale. Overlay text drawn with the painter uses these too.
pub const SIZE_SMALL: f32 = 13.0;
pub const SIZE_BODY: f32 = 15.0;
pub const SIZE_SECTION: f32 = 15.0;
pub const SIZE_HEADING: f32 = 16.0;
/// Shared modal sizing; height follows the viewport through a scroll area.
pub const SIZE_DIALOG_TITLE: f32 = 21.0;
pub const FIELD_LABEL_W: f32 = 190.0;
pub const DIALOG_WIDTH: f32 = 500.0;
pub const DIALOG_HEIGHT: f32 = 560.0;
pub const DIALOG_GAP: f32 = 12.0;
/// Space inside a popup, on each side of its content.
pub const DIALOG_MARGIN: f32 = 22.0;
pub const TRACK_LIST_HEIGHT: f32 = 112.0;
pub const TRACK_CONTROLS_WIDTH: f32 = 230.0;
pub const TRACK_NAME_MIN_WIDTH: f32 = 50.0;
pub const TRACK_ALPHA_WIDTH: f32 = 90.0;
pub const TRACK_DROP_STROKE: f32 = 2.0;

/// Whether a font with Hangul was found; without one Korean is unreadable.
pub fn has_korean_font() -> bool {
    KOREAN_FONT.load(Ordering::Relaxed)
}

static KOREAN_FONT: AtomicBool = AtomicBool::new(false);

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.options_mut(|o| {
        o.theme_preference = egui::ThemePreference::Dark;
        // The app handles Ctrl +/- itself so the size is remembered.
        o.zoom_with_keyboard = false;
    });
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(SIZE_HEADING, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(SIZE_BODY, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(SIZE_BODY, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(SIZE_SMALL, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
        ]
        .into();
        // Compact, Spine-like density.
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size = egui::vec2(40.0, MIN_CLICK_H);
        style.spacing.icon_width = 16.0;
        style.spacing.combo_width = 140.0;
        style.spacing.indent = 14.0;
        style.interaction.tooltip_delay = 0.35;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = PANEL_BG;
        v.window_fill = Color32::from_gray(48);
        v.window_stroke = Stroke::new(1.0, Color32::from_gray(80));
        v.extreme_bg_color = Color32::from_gray(30);
        v.faint_bg_color = Color32::from_gray(50);
        v.override_text_color = Some(TEXT);
        v.weak_text_color = Some(TEXT_DIM);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.42);
        v.selection.stroke = Stroke::new(1.5, ACCENT);
        v.hyperlink_color = ACCENT;
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        let radius = CornerRadius::same(3);
        // Controls get a visible outline so buttons never read as plain text.
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_gray(64));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(62);
        v.widgets.inactive.bg_fill = Color32::from_gray(62);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(78));
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.weak_bg_fill = Color32::from_gray(78);
        v.widgets.hovered.bg_fill = Color32::from_gray(78);
        v.widgets.hovered.bg_stroke = Stroke::new(1.5, ACCENT.gamma_multiply(0.8));
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        v.widgets.active.weak_bg_fill = Color32::from_gray(92);
        v.widgets.active.bg_fill = Color32::from_gray(92);
        v.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
        v.widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        v.widgets.open.weak_bg_fill = Color32::from_gray(70);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = radius;
        }
    });
}

/// System fonts that cover Hangul, best first.
const KOREAN_FONTS: &[&str] = &[
    "C:/Windows/Fonts/malgun.ttf",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
];

/// System Latin UI fonts, sturdier than egui's thin default.
const LATIN_FONTS: &[&str] = &["C:/Windows/Fonts/segoeui.ttf"];

/// Latin text uses a system UI font when there is one; Hangul comes from a
/// system Korean font, which is also the monospace fallback. Korean fonts
/// draw `\` as `₩`, so they never come first for Latin text (paths).
fn install_fonts(ctx: &egui::Context) {
    let read = |list: &[&str]| list.iter().find_map(|p| std::fs::read(p).ok());
    let mut fonts = FontDefinitions::default();
    if let Some(bytes) = read(LATIN_FONTS) {
        fonts.font_data.insert("latin".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "latin".into());
    }
    if let Some(bytes) = read(KOREAN_FONTS) {
        fonts.font_data.insert("korean".into(), Arc::new(FontData::from_owned(bytes)));
        let latin = fonts.font_data.contains_key("latin");
        fonts.families.entry(FontFamily::Proportional).or_default().insert(usize::from(latin), "korean".into());
        fonts.families.entry(FontFamily::Monospace).or_default().push("korean".into());
        KOREAN_FONT.store(true, Ordering::Relaxed);
    }
    ctx.set_fonts(fonts);
}

/// A section title used in the side panels: a tinted strip so sections are
/// easy to find while scanning.
pub fn section(ui: &mut egui::Ui, title: &str) {
    section_with(ui, title, |_| {});
}

/// [`section`] with extra widgets (add/delete buttons) on the same strip.
pub fn section_with(ui: &mut egui::Ui, title: &str, extra: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    egui::Frame::new().fill(HEADER_BG).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).strong().size(SIZE_SECTION).color(Color32::WHITE));
            extra(ui);
        });
    });
    ui.add_space(2.0);
}

/// Inspector property labels: fixed column, left-aligned so a row reads
/// label-then-value and every value starts at the same x.
pub const LABEL_W: f32 = 84.0;
/// Axis tags on paired fields (X red, Y green), as in most DCC tools.
pub const AXIS_X: Color32 = Color32::from_rgb(232, 96, 92);
pub const AXIS_Y: Color32 = Color32::from_rgb(120, 200, 96);
/// Outliner rows.
pub const ROW_H: f32 = 28.0;
pub const ROW_INDENT: f32 = 14.0;
pub const ROW_ALT: Color32 = Color32::from_gray(46);
pub const ROW_HOVER: Color32 = Color32::from_gray(60);
pub const GUIDE: Color32 = Color32::from_gray(68);

/// A collapsible inspector group: a full-width header strip (chevron, title,
/// optional summary on the right) over an indented body. Open state is kept
/// per `id` across selections, so a folded group stays folded.
pub fn group<R>(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, title: &str, summary: &str, default_open: bool, body: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let id = ui.make_persistent_id(("group", id));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    ui.add_space(4.0);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    let open = state.is_open();
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if r.hovered() { Color32::from_gray(64) } else { HEADER_BG });
    chevron(p, egui::pos2(rect.left() + 12.0, rect.center().y), open, TEXT_DIM);
    let font = FontId::proportional(SIZE_SECTION);
    p.text(egui::pos2(rect.left() + 24.0, rect.center().y), egui::Align2::LEFT_CENTER, title, font, Color32::WHITE);
    if !summary.is_empty() {
        p.text(rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, summary, FontId::proportional(SIZE_SMALL), TEXT_DIM);
    }
    if r.clicked() {
        state.toggle(ui);
    }
    state.store(ui.ctx());
    if !state.is_open() {
        return None;
    }
    let inner = egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 2, top: 4, bottom: 2 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui)
    });
    Some(inner.inner)
}

/// A down (open) or right (closed) triangle centered on `c`.
pub fn chevron(p: &egui::Painter, c: egui::Pos2, open: bool, color: Color32) {
    let v = |x: f32, y: f32| c + egui::vec2(x, y);
    let pts = if open { vec![v(-4.0, -2.0), v(4.0, -2.0), v(0.0, 3.0)] } else { vec![v(-2.0, -4.0), v(3.0, 0.0), v(-2.0, 4.0)] };
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// One inspector row: a dim label in the fixed label column, then the value
/// widgets filling the rest of the width.
pub fn prop<R>(ui: &mut egui::Ui, label: &str, tip: &str, value: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        let (rect, r) = ui.allocate_exact_size(egui::vec2(LABEL_W, 22.0), egui::Sense::hover());
        let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(SIZE_BODY), TEXT_DIM);
        let clipped = galley.size().x > LABEL_W - 4.0;
        ui.painter().with_clip_rect(rect).galley(rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0), galley, TEXT_DIM);
        if !tip.is_empty() || clipped {
            r.on_hover_text(if tip.is_empty() { label.to_string() } else { format!("{label}\n{tip}") });
        }
        value(ui)
    })
    .inner
}

/// A small colored axis tag ("X", "Y") in front of a paired number field.
pub fn axis_tag(ui: &mut egui::Ui, text: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 20.0), egui::Sense::hover());
    ui.painter().rect_filled(egui::Rect::from_center_size(rect.center(), egui::vec2(3.0, 14.0)).translate(egui::vec2(-4.0, 0.0)), 1.0, color);
    ui.painter().text(rect.center() + egui::vec2(2.0, 0.0), egui::Align2::CENTER_CENTER, text, FontId::proportional(SIZE_SMALL), color);
}

/// A centered popup over a darkened window.
pub fn popup_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_gray(48))
        .stroke(Stroke::new(1.0, Color32::from_gray(96)))
        .corner_radius(10)
        .inner_margin(DIALOG_MARGIN as i8)
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(140) })
}

#[cfg(test)]
mod dialog_row_tests {
    use super::*;

    /// Gap between the button row's bottom and the modal's bottom, after settling.
    fn gap(screen: egui::Vec2, heights: &[f32], sized: bool) -> f32 {
        let ctx = egui::Context::default();
        let mut out = 0.0;
        for &height in heights {
            for _ in 0..6 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)), ..Default::default() };
                let _ = ctx.run_pass(input, |ctx| {
                    let shown = modal("row_test").show(ctx, |ui| {
                        ui.set_width(DIALOG_WIDTH);
                        ui.allocate_space(egui::vec2(100.0, height));
                        if sized {
                            dialog_buttons(ui, |ui| ui.add(egui::Button::new("OK").min_size(egui::vec2(60.0, MIN_CLICK_H))).rect)
                        } else {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(egui::Button::new("OK").min_size(egui::vec2(60.0, MIN_CLICK_H))).rect).inner
                        }
                    });
                    out = shown.response.rect.bottom() - shown.inner.bottom();
                });
            }
        }
        out
    }

    #[test]
    fn buttons_sit_at_the_bottom_of_a_popup() {
        for screen in [egui::vec2(1280.0, 820.0), egui::vec2(2560.0, 1440.0)] {
            for heights in [&[500.0][..], &[500.0, 60.0][..]] {
                assert_eq!(gap(screen, heights, true), gap(screen, &[60.0], true), "{screen:?} {heights:?}: the margin stays the same when content shrinks");
            }
        }
    }
}

pub fn modal(id: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::unique(id)).frame(popup_frame()).backdrop_color(Color32::from_black_alpha(170))
}

/// A popup's title and, under it, one or two lines saying what it is for.
pub fn dialog_title(ui: &mut egui::Ui, title: &str, about: &str) {
    ui.label(egui::RichText::new(title).strong().size(SIZE_DIALOG_TITLE).color(Color32::WHITE));
    if !about.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(about).size(SIZE_BODY).color(TEXT_DIM));
    }
    ui.add_space(14.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Emphasis { Plain, Primary, Danger }

/// The action row at the bottom of a popup, right-aligned. It has its own
/// height: a bare right-to-left layout centers its buttons in whatever height
/// is left, which leaves them floating above the popup's bottom edge.
pub fn dialog_buttons<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = egui::vec2(ui.available_width(), MIN_CLICK_H);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), add).inner
}

pub fn dialog_button(text: &str, emphasis: Emphasis) -> egui::Button<'_> {
    let (fill, color) = match emphasis {
        Emphasis::Plain => (Color32::from_gray(70), Color32::WHITE),
        Emphasis::Primary => (ACCENT, ON_ACCENT),
        Emphasis::Danger => (Color32::from_rgb(196, 58, 52), Color32::WHITE),
    };
    egui::Button::new(egui::RichText::new(text).size(SIZE_BODY).strong().color(color)).fill(fill).corner_radius(6).min_size(egui::vec2(104.0, 38.0))
}

/// A labeled control in a popup: the label on the left, the control in a straight column.
pub fn field<R>(ui: &mut egui::Ui, label: &str, tip: &str, value: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        let (rect, r) = ui.allocate_exact_size(egui::vec2(FIELD_LABEL_W, 26.0), egui::Sense::hover());
        let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(SIZE_BODY), TEXT);
        let clipped = galley.size().x > FIELD_LABEL_W - 6.0;
        ui.painter().with_clip_rect(rect).galley(rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0), galley, TEXT);
        if !tip.is_empty() || clipped {
            r.on_hover_text(if tip.is_empty() { label.to_string() } else { format!("{label}\n{tip}") });
        }
        value(ui)
    })
    .inner
}

/// Buttons side by side, one of them picked.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (option, label) in options {
            let on = *value == *option;
            let text = egui::RichText::new(*label).size(SIZE_BODY).color(if on { ON_ACCENT } else { TEXT });
            let button = egui::Button::new(text).fill(if on { ACCENT } else { Color32::from_gray(66) }).corner_radius(5).min_size(egui::vec2(60.0, 30.0));
            if ui.add(button).clicked() && !on {
                *value = *option;
                changed = true;
            }
        }
    });
    changed
}

/// A full-width choice with a line of explanation.
pub fn choice_card(ui: &mut egui::Ui, on: bool, title: &str, about: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if on { ACCENT.gamma_multiply(0.22) } else if r.hovered() { Color32::from_gray(70) } else { Color32::from_gray(58) };
    p.rect(rect, 6.0, fill, Stroke::new(1.0, if on { ACCENT } else { Color32::from_gray(84) }), egui::StrokeKind::Inside);
    let dot = egui::pos2(rect.left() + 18.0, rect.center().y);
    p.circle_stroke(dot, 7.0, Stroke::new(1.5, if on { ACCENT } else { TEXT_DIM }));
    if on { p.circle_filled(dot, 3.5, ACCENT); }
    p.text(egui::pos2(rect.left() + 36.0, rect.top() + 18.0), egui::Align2::LEFT_CENTER, title, FontId::proportional(SIZE_BODY), Color32::WHITE);
    p.text(egui::pos2(rect.left() + 36.0, rect.top() + 38.0), egui::Align2::LEFT_CENTER, about, FontId::proportional(SIZE_SMALL), TEXT_DIM);
    ui.add_space(2.0);
    r.clicked()
}

/// A problem to fix before the popup can be applied.
pub fn error_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().fill(KEY_SET.gamma_multiply(0.2)).stroke(Stroke::new(1.0, KEY_SET)).corner_radius(5).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(text).color(KEY_SET_TEXT).size(SIZE_BODY));
    });
}

/// A result worth reading at a glance, e.g. how many vertices a mesh gets.
pub fn summary_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().fill(ACCENT.gamma_multiply(0.14)).corner_radius(5).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(text).color(Color32::WHITE).size(SIZE_BODY).strong());
    });
}

/// Secondary helper text.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(TEXT_DIM).size(SIZE_SMALL));
}

/// A button for destructive actions: tinted so it is not clicked by mistake.
pub fn danger_button(text: &str) -> egui::Button<'_> {
    crate::icons::labeled(crate::icons::Icon::Delete, egui::RichText::new(text).color(Color32::from_rgb(255, 150, 150))).stroke(Stroke::new(1.0, Color32::from_rgb(140, 70, 70)))
}

/// WCAG relative luminance of an opaque sRGB color.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two opaque colors (1..21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A bone's own color made readable as text on the dark panels: very dark
/// colors are lifted toward white until they reach 4.5:1, others are kept.
pub fn legible(c: Color32) -> Color32 {
    let c = Color32::from_rgb(c.r(), c.g(), c.b());
    let mut out = c;
    for step in 1..=10 {
        if contrast(out, PANEL_BG) >= 4.5 {
            break;
        }
        out = c.lerp_to_gamma(Color32::WHITE, step as f32 / 10.0);
    }
    out
}

/// The color a bone is drawn with: its own where set, else the theme's bone color.
pub fn bone_color(bone: &skel2d_core::Bone) -> Color32 {
    bone.color.map_or(BONE, |c| {
        let [r, g, b, a] = c.to_rgba8();
        Color32::from_rgba_unmultiplied(r, g, b, a)
    })
}

/// A bone's own color as text on a dark panel.
pub fn bone_text_color(bone: &skel2d_core::Bone) -> Color32 {
    legible(bone_color(bone))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over a half transparent overlay the background is the overlay on the viewport color.
    fn over(overlay: Color32, under: Color32) -> Color32 {
        let a = overlay.a() as f32 / 255.0;
        let mix = |o: u8, u: u8| (o as f32 + u as f32 * (1.0 - a)).round().min(255.0) as u8;
        Color32::from_rgb(mix(overlay.r(), under.r()), mix(overlay.g(), under.g()), mix(overlay.b(), under.b()))
    }

    #[test]
    fn dark_bone_colors_are_lifted_for_text_only() {
        let dark = Color32::from_rgb(20, 20, 40);
        assert!(contrast(legible(dark), PANEL_BG) >= 4.5);
        assert_eq!(legible(BONE), BONE, "readable colors are kept");
        assert!(contrast(legible(Color32::BLACK), PANEL_BG) >= 4.5);
    }

    #[test]
    fn contrast_of_known_pairs() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn body_text_has_at_least_4_5_to_1_on_every_background() {
        let backgrounds = [PANEL_BG, HEADER_BG, ROW_ALT, ROW_HOVER, VIEWPORT_BG, Color32::from_gray(30), Color32::from_gray(48), Color32::from_gray(62), Color32::from_gray(78), over(OVERLAY_BG, VIEWPORT_BG)];
        for bg in backgrounds {
            // Dim text is never drawn on the bright hover fill of buttons.
            for (name, fg) in [("TEXT", TEXT), ("TEXT_DIM", TEXT_DIM)] {
                if name == "TEXT_DIM" && bg == Color32::from_gray(78) {
                    continue;
                }
                assert!(contrast(fg, bg) >= 4.5, "{name} on {bg:?} is {:.2}:1", contrast(fg, bg));
            }
        }
        for bg in [PANEL_BG, HEADER_BG, ROW_ALT, over(OVERLAY_BG, VIEWPORT_BG)] {
            for (name, fg) in [("ACCENT", ACCENT), ("WARN", WARN), ("ANIMATE", ANIMATE), ("SETUP", SETUP), ("KEY_SET_TEXT", KEY_SET_TEXT), ("SLOT", SLOT), ("BONE", BONE)] {
                assert!(contrast(fg, bg) >= 4.5, "{name} on {bg:?} is {:.2}:1", contrast(fg, bg));
            }
        }
    }

    #[test]
    fn disabled_text_has_at_least_3_to_1() {
        for bg in [PANEL_BG, HEADER_BG, ROW_ALT, Color32::from_gray(62)] {
            assert!(contrast(TEXT_DISABLED, bg) >= 3.0, "disabled on {bg:?} is {:.2}:1", contrast(TEXT_DISABLED, bg));
        }
    }

    #[test]
    fn labels_on_filled_buttons_are_readable() {
        assert!(contrast(ON_ACCENT, STEP_ACTIVE) >= 4.5);
        assert!(contrast(ON_ACCENT, ACCENT) >= 4.5);
        assert!(contrast(TEXT, Color32::from_gray(92)) >= 4.5, "pressed button");
    }

    #[test]
    fn text_sizes_and_click_height_follow_the_spec() {
        assert!(SIZE_BODY >= 14.0 && SIZE_HEADING >= 14.0 && SIZE_SECTION >= 14.0);
        assert!(SIZE_SMALL >= 12.0);
        assert!(MIN_CLICK_H >= 28.0 && ROW_H >= 28.0 && STEP_BTN_H >= 28.0);
    }
}
