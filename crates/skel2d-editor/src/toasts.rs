//! Stacked toast notifications for transient messages (what `app.status`
//! carries). Identical repeats collapse into one toast with a count, so a
//! refused drag cannot flood the stack. Playback event toasts stay separate:
//! they are a 0.6 s flash in the viewport, not messages to read.

use eframe::egui::{self, Color32, RichText};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

impl Kind {
    fn color(self) -> Color32 {
        match self {
            Kind::Info => theme::ACCENT,
            Kind::Success => theme::KEY_NONE,
            Kind::Warning => theme::WARN,
            Kind::Error => theme::KEY_SET,
        }
    }

    /// A glyph per kind, so the kind does not rest on color alone.
    fn glyph(self) -> &'static str {
        match self {
            Kind::Info => "i",
            Kind::Success => "OK",
            Kind::Warning => "!",
            Kind::Error => "×",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Info => "Info",
            Kind::Success => "Done",
            Kind::Warning => "Warning",
            Kind::Error => "Error",
        }
    }

    fn seconds(self) -> f32 {
        match self {
            Kind::Info | Kind::Success => theme::TOAST_INFO_SECS,
            Kind::Warning => theme::TOAST_WARN_SECS,
            Kind::Error => theme::TOAST_ERROR_SECS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: Kind,
    pub text: String,
    /// How often this exact message arrived while it was showing.
    pub count: u32,
    pub remaining: f32,
}

#[derive(Debug, Default)]
pub struct Toasts {
    /// Oldest first; drawn with the newest at the bottom.
    pub items: Vec<Toast>,
}

impl Toasts {
    /// Shows `text`, or counts it on the identical toast already showing.
    pub fn push(&mut self, kind: Kind, text: impl Into<String>) {
        let text = text.into();
        if let Some(t) = self.items.iter_mut().find(|t| t.kind == kind && t.text == text) {
            t.count += 1;
            t.remaining = kind.seconds();
            return;
        }
        self.items.push(Toast { kind, text, count: 1, remaining: kind.seconds() });
        while self.items.len() > theme::TOAST_MAX {
            self.items.remove(0);
        }
    }

    /// Advances the timers by `dt`; the toast under the pointer (`paused`) keeps its time.
    pub fn tick(&mut self, dt: f32, paused: Option<usize>) {
        for (i, t) in self.items.iter_mut().enumerate() {
            if Some(i) != paused {
                t.remaining -= dt;
            }
        }
        self.items.retain(|t| t.remaining > 0.0);
    }
}

/// What kind a status message is: refusals warn, failures are errors, completed work succeeds.
pub fn classify(text: &str) -> Kind {
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["not applied", "refused", "cannot", "can't", "needs ", "pick or create", "nothing changed", "discarded", "tangled", "적용되지 않", "수 없", "먼저", "버렸"]) {
        Kind::Warning
    } else if has(&["error", "failed", "invalid", "does not exist", "missing", "오류", "실패", "올바르지 않", "누락", "없습니다"]) {
        Kind::Error
    } else if has(&["keyed", "copied", "applied", "saved", "opened", "added", "created", "imported", "찍었", "복사", "저장", "열었", "추가"]) {
        Kind::Success
    } else {
        Kind::Info
    }
}

/// Draws the stack in the bottom right corner of `rect` and advances its timers.
/// Returns true while it needs repaints.
pub fn show(ctx: &egui::Context, toasts: &mut Toasts, rect: egui::Rect) -> bool {
    if toasts.items.is_empty() {
        return false;
    }
    let mut hovered = None;
    let mut closed = None;
    egui::Area::new(egui::Id::unique("toast_stack"))
        .fixed_pos(rect.right_bottom() - egui::vec2(theme::TOAST_MARGIN, theme::TOAST_MARGIN))
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            ui.set_max_width(theme::TOAST_W.min((rect.width() - 2.0 * theme::TOAST_MARGIN).max(120.0)));
            ui.spacing_mut().item_spacing.y = 6.0;
            for (i, t) in toasts.items.iter().enumerate() {
                let color = t.kind.color();
                let frame = egui::Frame::new().fill(theme::OVERLAY_BG).stroke(egui::Stroke::new(1.0, color)).corner_radius(5).inner_margin(egui::Margin::symmetric(8, 6));
                let out = frame.show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(t.kind.glyph()).strong().color(color).size(theme::SIZE_BODY)).on_hover_text(crate::i18n::tr(t.kind.name()));
                        let text = if t.count > 1 { format!("{}  ×{}", t.text, t.count) } else { t.text.clone() };
                        ui.add(egui::Label::new(RichText::new(text).color(theme::TEXT).size(theme::SIZE_SMALL)).wrap());
                        if ui.small_button("×").on_hover_text(crate::i18n::tr("Dismiss")).clicked() {
                            closed = Some(i);
                        }
                    });
                });
                if out.response.hovered() {
                    hovered = Some(i);
                }
            }
        });
    if let Some(i) = closed {
        toasts.items.remove(i);
    }
    let dt = ctx.input(|i| i.stable_dt).min(0.25);
    toasts.tick(dt, hovered);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_messages_collapse_into_a_count_and_restart_the_timer() {
        let mut t = Toasts::default();
        t.push(Kind::Warning, "Not applied");
        t.tick(5.0, None);
        t.push(Kind::Warning, "Not applied");
        t.push(Kind::Warning, "Not applied");
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].count, 3);
        assert_eq!(t.items[0].remaining, theme::TOAST_WARN_SECS);
        // The same text as another kind is a different toast.
        t.push(Kind::Error, "Not applied");
        assert_eq!(t.items.len(), 2);
    }

    #[test]
    fn toasts_expire_by_kind_and_hovering_pauses_one() {
        let mut t = Toasts::default();
        t.push(Kind::Info, "a");
        t.push(Kind::Error, "b");
        t.tick(theme::TOAST_INFO_SECS + 0.1, Some(0));
        assert_eq!(t.items.len(), 2, "the hovered toast keeps its time");
        t.tick(theme::TOAST_INFO_SECS + 0.1, None);
        assert_eq!(t.items.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["b"], "info expired, the error stays");
        t.tick(theme::TOAST_ERROR_SECS, None);
        assert!(t.items.is_empty());
    }

    #[test]
    fn the_stack_keeps_the_newest_few() {
        let mut t = Toasts::default();
        for i in 0..theme::TOAST_MAX + 3 {
            t.push(Kind::Info, format!("m{i}"));
        }
        assert_eq!(t.items.len(), theme::TOAST_MAX);
        assert_eq!(t.items.last().unwrap().text, format!("m{}", theme::TOAST_MAX + 2));
    }

    #[test]
    fn kinds_are_told_apart() {
        assert_eq!(classify("Not applied (m): the mesh outline would cross itself."), Kind::Warning);
        assert_eq!(classify("failed to open file"), Kind::Error);
        assert_eq!(classify("Keyed 3 edited properties"), Kind::Success);
        assert_eq!(classify("Pasted"), Kind::Info);
    }
}
