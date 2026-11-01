//! Step 5 groups for what is not a bone or slot value: the draw order and the
//! events of the open animation. Both only build ops, so the CLI can do the
//! same; the command palette reuses the helpers at the bottom.

use super::*;

// ------------------------------------------------------------ draw order

/// Slot names back to front as the pose shows them at the playhead.
pub(crate) fn current_order(app: &EditorApp) -> Vec<String> {
    let pose = app.pose();
    if pose.draw_order.len() == app.doc.slots.len() {
        pose.draw_order.iter().map(|&i| app.doc.slots[i].name.clone()).collect()
    } else {
        app.doc.slots.iter().map(|s| s.name.clone()).collect()
    }
}

/// `order` with `name` one place nearer the front (or the back); `None` at the end of the list.
pub(crate) fn moved(order: &[String], name: &str, forward: bool) -> Option<Vec<String>> {
    let i = order.iter().position(|n| n == name)?;
    let j = if forward { i + 1 } else { i.checked_sub(1)? };
    if j >= order.len() {
        return None;
    }
    let mut out = order.to_vec();
    out.swap(i, j);
    Some(out)
}

pub(crate) fn draw_order_key_here(app: &EditorApp) -> bool {
    let t = app.playback.time;
    app.animation().is_some_and(|a| a.draw_order.iter().any(|k| (k.time - t).abs() < TIME_EPS))
}

/// Keys `order` (`None` = the setup order) at the playhead.
pub(crate) fn key_draw_order(app: &mut EditorApp, order: Option<Vec<String>>) -> bool {
    let Some(animation) = app.playback.anim.clone() else { return false };
    let time = app.playback.time;
    app.edit(vec![Op::KeyDrawOrder { animation, time, order }])
}

pub(crate) fn remove_draw_order_key(app: &mut EditorApp) -> bool {
    let Some(animation) = app.playback.anim.clone() else { return false };
    let time = app.playback.time;
    app.edit(vec![Op::RemoveDrawOrderKey { animation, time }])
}

/// Moves `slot` one place in the draw order at the playhead and keys the result.
pub(crate) fn move_slot(app: &mut EditorApp, slot: &str, forward: bool) -> bool {
    match moved(&current_order(app), slot, forward) {
        Some(order) => key_draw_order(app, Some(order)),
        None => {
            app.status = tr(if forward { "That slot is already in front" } else { "That slot is already at the back" }).into();
            false
        }
    }
}

pub(super) fn draw_order_group(app: &mut EditorApp, ui: &mut egui::Ui, open: bool) {
    let Some(anim) = app.playback.anim.clone() else { return };
    let t = app.playback.time;
    let keys: Vec<f32> = app.animation().map(|a| a.draw_order.iter().map(|k| k.time).collect()).unwrap_or_default();
    let here = draw_order_key_here(app);
    let summary = if here { tr("keyed here").to_string() } else if keys.is_empty() { tr("setup order").to_string() } else { crate::tf!("{} keys", keys.len()) };
    let mut set: Option<Option<Vec<String>>> = None;
    let mut remove = false;
    theme::group(ui, "anim_draw_order", tr("Draw order"), &summary, open, |ui| {
        note_label(ui, &crate::tf!("Front on top, as at {} s in \"{}\". The arrows key the whole order at the playhead.", format!("{t:.2}"), anim));
        let order = current_order(app);
        let state = if here { KeyState::Keyed } else if keys.is_empty() { KeyState::None } else { KeyState::Interpolated };
        ui.horizontal(|ui| {
            if key_button(ui, state) {
                set = Some(Some(order.clone()));
            }
            if ui.button(tr("Setup order")).on_hover_text(tr("Key the setup order here: the slots go back to where step 2 put them")).clicked() {
                set = Some(None);
            }
            let remove_button = ui.add_enabled(here, egui::Button::new(tr("Remove key")));
            if remove_button.on_hover_text(tr("Delete the draw order key at the playhead")).on_disabled_hover_text(tr("There is no draw order key at the playhead")).clicked() {
                remove = true;
            }
        });
        egui::ScrollArea::vertical().id_salt("draw_order_rows").max_height(230.0).auto_shrink([false, true]).show(ui, |ui| {
            let last = order.len().saturating_sub(1);
            for (rank, name) in order.iter().rev().enumerate() {
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::MIN_CLICK_H - 4.0);
                    let selected = app.selection.slot.as_deref() == Some(name.as_str());
                    let w = ui.available_width() - 70.0;
                    ui.allocate_ui_with_layout(egui::vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(name).color(if selected { TEXT } else { theme::SLOT }).size(theme::SIZE_BODY)).truncate());
                    });
                    let up = ui.add_enabled(rank > 0, crate::icons::small(crate::icons::Icon::ArrowUp));
                    if up.on_hover_text(tr("Bring one place forward (keys the order here)")).on_disabled_hover_text(tr("Already in front")).clicked() {
                        set = moved(&order, name, true).map(Some);
                    }
                    let down = ui.add_enabled(rank < last, crate::icons::small(crate::icons::Icon::ArrowDown));
                    if down.on_hover_text(tr("Send one place back (keys the order here)")).on_disabled_hover_text(tr("Already at the back")).clicked() {
                        set = moved(&order, name, false).map(Some);
                    }
                });
            }
        });
        note_label(ui, tr("Keys hold until the next one. The setup order itself is changed in step 2 (Images): drag slots in the Draw order list."));
    });
    if let Some(order) = set {
        key_draw_order(app, order);
    }
    if remove {
        remove_draw_order_key(app);
    }
}

// ------------------------------------------------------------ events

fn defaults_text(def: &skel2d_core::EventDef) -> String {
    let mut parts = vec![def.int.to_string(), short(def.float)];
    if !def.string.is_empty() {
        parts.push(format!("\"{}\"", def.string));
    }
    parts.join(" · ")
}

fn short(v: f32) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The key of event `name` at the playhead, if any.
pub(crate) fn event_key_here(app: &EditorApp, name: &str) -> Option<skel2d_core::EventKey> {
    let t = app.playback.time;
    app.animation()?.events.iter().find(|k| k.name == name && (k.time - t).abs() < TIME_EPS).cloned()
}

/// Fires `name` at the playhead. A key already there keeps its overrides.
pub(crate) fn key_event_now(app: &mut EditorApp, name: &str) -> bool {
    let Some(animation) = app.playback.anim.clone() else { return false };
    let time = app.playback.time;
    let old = event_key_here(app, name);
    app.edit(vec![Op::KeyEvent {
        animation, name: name.to_string(), time,
        int: old.as_ref().and_then(|k| k.int), float: old.as_ref().and_then(|k| k.float), string: old.and_then(|k| k.string),
    }])
}

pub(crate) fn remove_event_key(app: &mut EditorApp, name: &str) -> bool {
    let Some(animation) = app.playback.anim.clone() else { return false };
    let time = app.playback.time;
    app.edit(vec![Op::RemoveEventKey { animation, name: name.to_string(), time }])
}

/// What the event widgets asked for this frame.
#[derive(Default)]
struct EventEdit {
    /// New overrides for the key at the playhead (all three, the changed one included).
    key: Option<(Option<i32>, Option<f32>, Option<String>)>,
    /// A default changed, with the coalescing field name.
    defaults: Option<(&'static str, Op)>,
    fire: bool,
    remove: bool,
}

pub(super) fn events_group(app: &mut EditorApp, ui: &mut egui::Ui, open: bool) {
    let Some(anim) = app.playback.anim.clone() else { return };
    let t = app.playback.time;
    let names: Vec<String> = app.doc.events.keys().cloned().collect();
    // A deleted or renamed-away selection falls back to nothing.
    if app.timeline.event.as_ref().is_some_and(|e| !app.doc.events.contains_key(e)) {
        app.timeline.event = None;
    }
    let selected = app.timeline.event.clone();
    let count = app.animation().map_or(0, |a| a.events.len());
    let summary = crate::tf!("{} defined · {} keys here", names.len(), count);
    let mut edit = EventEdit::default();
    let mut pick: Option<String> = None;
    let (mut new, mut rename, mut delete) = (false, false, false);
    theme::group(ui, "anim_events", tr("Events"), &summary, open, |ui| {
        note_label(ui, tr("An event is a named moment, like a footstep, that the game hears when playback passes its key."));
        if names.is_empty() {
            note_label(ui, tr("No events yet. Add one, then key it where it should fire."));
        }
        for name in &names {
            let def = &app.doc.events[name];
            let label = format!("{name}   {}", defaults_text(def));
            let on = selected.as_deref() == Some(name.as_str());
            if ui.selectable_label(on, RichText::new(label).color(if on { TEXT } else { theme::EVENT })).on_hover_text(tr("Pick this event to key it, change its defaults, rename or delete it")).clicked() {
                pick = Some(name.clone());
            }
        }
        ui.horizontal(|ui| {
            new = ui.button(tr("Add event…")).on_hover_text(tr("Define a new event with default values")).clicked();
            rename = ui.add_enabled(selected.is_some(), egui::Button::new(tr("Rename…"))).on_hover_text(tr("Rename the picked event; its keys follow")).on_disabled_hover_text(tr("Pick an event in the list first")).clicked();
            delete = ui.add_enabled(selected.is_some(), theme::danger_button(tr("Delete"))).on_hover_text(tr("Delete the picked event and all its keys")).on_disabled_hover_text(tr("Pick an event in the list first")).clicked();
        });
        let Some(name) = selected.as_deref() else {
            theme::hint(ui, tr("Pick an event to key it at the playhead."));
            return;
        };
        let Some(def) = app.doc.events.get(name).cloned() else { return };
        let key = event_key_here(app, name);

        theme::section(ui, &crate::tf!("{} at {} s", name, format!("{t:.2}")));
        ui.horizontal(|ui| {
            let fire = ui.add_enabled(key.is_none(), egui::Button::new(tr("Key at playhead")));
            if fire.on_hover_text(tr("Fires this event when playback passes this time")).on_disabled_hover_text(tr("This event already has a key at the playhead")).clicked() {
                edit.fire = true;
            }
            let remove = ui.add_enabled(key.is_some(), egui::Button::new(tr("Remove key")));
            if remove.on_hover_text(tr("Delete this event's key at the playhead")).on_disabled_hover_text(tr("This event has no key at the playhead")).clicked() {
                edit.remove = true;
            }
        });
        match &key {
            None => theme::hint(ui, tr("No key at the playhead. Key it to set values for this firing only.")),
            Some(k) => {
                theme::hint(ui, tr("Values for this firing. Reset returns one to the event's default."));
                let (mut int, mut float, mut string) = (k.int, k.float, k.string.clone());
                let mut changed = false;
                theme::prop(ui, tr("Whole number"), tr("Whole-number value passed to the game"), |ui| {
                    let mut v = int.unwrap_or(def.int);
                    if ui.add(DragValue::new(&mut v)).on_hover_text(tr("Whole-number value passed to the game")).changed() { int = Some(v); changed = true; }
                    if ui.add_enabled(int.is_some(), crate::icons::small(crate::icons::Icon::Reset)).on_hover_text(tr("Follow the event's default again")).on_disabled_hover_text(tr("Already following the event's default")).clicked() { int = None; changed = true; }
                });
                theme::prop(ui, tr("Decimal number"), tr("Decimal value passed to the game"), |ui| {
                    let mut v = float.unwrap_or(def.float);
                    if ui.add(DragValue::new(&mut v).speed(0.01).max_decimals(3)).on_hover_text(tr("Decimal value passed to the game")).changed() && v.is_finite() { float = Some(v); changed = true; }
                    if ui.add_enabled(float.is_some(), crate::icons::small(crate::icons::Icon::Reset)).on_hover_text(tr("Follow the event's default again")).on_disabled_hover_text(tr("Already following the event's default")).clicked() { float = None; changed = true; }
                });
                theme::prop(ui, tr("Text"), tr("Text value passed to the game"), |ui| {
                    let mut v = string.clone().unwrap_or_else(|| def.string.clone());
                    let w = (ui.available_width() - 40.0).max(40.0);
                    if ui.add(egui::TextEdit::singleline(&mut v).desired_width(w)).on_hover_text(tr("Text value passed to the game")).changed() { string = Some(v); changed = true; }
                    if ui.add_enabled(string.is_some(), crate::icons::small(crate::icons::Icon::Reset)).on_hover_text(tr("Follow the event's default again")).on_disabled_hover_text(tr("Already following the event's default")).clicked() { string = None; changed = true; }
                });
                if changed {
                    edit.key = Some((int, float, string));
                }
            }
        }

        theme::section(ui, tr("Defaults"));
        theme::hint(ui, tr("Used by every key that does not set its own value, in all animations."));
        let mut d = def.clone();
        let mut field = "";
        theme::prop(ui, tr("Whole number"), tr("Default whole-number value"), |ui| {
            if ui.add(DragValue::new(&mut d.int)).on_hover_text(tr("Default whole-number value")).changed() { field = "int"; }
        });
        theme::prop(ui, tr("Decimal number"), tr("Default decimal value"), |ui| {
            if ui.add(DragValue::new(&mut d.float).speed(0.01).max_decimals(3)).on_hover_text(tr("Default decimal value")).changed() && d.float.is_finite() { field = "float"; }
        });
        theme::prop(ui, tr("Text"), tr("Default text value"), |ui| {
            let w = ui.available_width();
            if ui.add(egui::TextEdit::singleline(&mut d.string).desired_width(w)).on_hover_text(tr("Default text value")).changed() { field = "string"; }
        });
        if !field.is_empty() {
            let op = Op::SetEvent { name: name.to_string(), rename: None, int: Some(d.int), float: Some(d.float), string: Some(d.string) };
            edit.defaults = Some((field, op));
        }
    });

    if let Some(n) = pick {
        app.timeline.event = Some(n);
    }
    if new {
        crate::dialogs::new_event(app);
    }
    if let Some(name) = &selected {
        if rename {
            crate::dialogs::rename(app, crate::dialogs::Item::Event(name.clone()));
        }
        if delete {
            crate::dialogs::delete(app, crate::dialogs::Item::Event(name.clone()));
        }
        if edit.fire {
            key_event_now(app, name);
        }
        if edit.remove {
            remove_event_key(app, name);
        }
        if let Some((int, float, string)) = edit.key {
            let op = Op::KeyEvent { animation: anim.clone(), name: name.clone(), time: t, int, float, string };
            app.edit_coalesced(format!("event:{name}:{anim}:{}", (t * 10_000.0).round() as i64), vec![op]);
        }
        if let Some((field, op)) = edit.defaults {
            app.edit_coalesced(format!("event:{name}:default:{field}"), vec![op]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn moving_swaps_neighbours_and_stops_at_the_ends() {
        let order = names(&["a", "b", "c"]);
        assert_eq!(moved(&order, "a", true), Some(names(&["b", "a", "c"])));
        assert_eq!(moved(&order, "c", false), Some(names(&["a", "c", "b"])));
        assert_eq!(moved(&order, "c", true), None);
        assert_eq!(moved(&order, "a", false), None);
        assert_eq!(moved(&order, "missing", true), None);
    }

    fn app() -> EditorApp {
        let doc = skel2d_core::apply(&skel2d_core::Document::with_root("t"), &skel2d_core::parse_ops(r#"[
            {"op":"add_slot","name":"s1","bone":"root"},
            {"op":"add_slot","name":"s2","bone":"root"},
            {"op":"add_slot","name":"s3","bone":"root"},
            {"op":"add_animation","name":"a","duration":2}
        ]"#).unwrap()).unwrap();
        EditorApp::test_app(doc, std::path::PathBuf::from("extra.skel.json"))
    }

    #[test]
    fn draw_order_buttons_key_the_full_order_and_the_pose_follows() {
        let mut app = app();
        app.playback.anim = Some("a".into());
        app.playback.time = 1.0;
        let setup = app.doc.slots.clone();
        assert_eq!(current_order(&app), names(&["s1", "s2", "s3"]));
        assert!(move_slot(&mut app, "s3", false));
        assert_eq!(app.doc.slots, setup, "animate mode never edits the setup order");
        let key = &app.doc.animations["a"].draw_order[0];
        assert_eq!((key.time, key.order.clone()), (1.0, Some(names(&["s1", "s3", "s2"]))));
        assert_eq!(current_order(&app), names(&["s1", "s3", "s2"]));
        // Before the key the setup order shows.
        app.playback.time = 0.5;
        assert_eq!(current_order(&app), names(&["s1", "s2", "s3"]));
        // The front slot cannot move further forward.
        app.playback.time = 1.0;
        assert!(!move_slot(&mut app, "s2", true));
        assert!(draw_order_key_here(&app));
        assert!(key_draw_order(&mut app, None));
        assert_eq!(app.doc.animations["a"].draw_order[0].order, None);
        assert!(remove_draw_order_key(&mut app));
        assert!(!draw_order_key_here(&app));
    }

    #[test]
    fn event_key_keeps_overrides_when_fired_again() {
        let mut app = app();
        app.playback.anim = Some("a".into());
        app.playback.time = 0.5;
        assert!(app.edit(vec![Op::AddEvent { name: "step".into(), int: 1, float: 0.0, string: String::new() }]));
        assert!(key_event_now(&mut app, "step"));
        assert!(app.edit(vec![Op::KeyEvent { animation: "a".into(), name: "step".into(), time: 0.5, int: Some(9), float: None, string: None }]));
        assert!(key_event_now(&mut app, "step"));
        let k = event_key_here(&app, "step").unwrap();
        assert_eq!((k.int, k.float, k.string), (Some(9), None, None));
        assert!(remove_event_key(&mut app, "step"));
        assert!(event_key_here(&app, "step").is_none());
    }

    #[test]
    fn groups_render_and_change_nothing_by_themselves() {
        let ctx = egui::Context::default();
        crate::icons::load(&ctx);
        let mut app = app();
        assert!(app.edit(vec![Op::AddEvent { name: "step".into(), int: 1, float: 0.5, string: "dust".into() }]));
        app.playback.anim = Some("a".into());
        app.timeline.event = Some("step".into());
        let before = app.doc.clone();
        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    draw_order_group(&mut app, ui, true);
                    events_group(&mut app, ui, true);
                });
            });
        }
        assert_eq!(app.doc, before);
    }
}
