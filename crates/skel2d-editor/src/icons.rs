//! Toolbar and tree icons: white glyphs on transparency, tinted at draw time
//! (buttons tint them like their text). Generated art lives in
//! `assets/icons`, 64 px squares.

use std::sync::OnceLock;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The icon set is kept complete; not every layout uses every icon.
#[allow(dead_code)]
pub enum Icon {
    Select,
    Rotate,
    Move,
    Scale,
    Bone,
    Mesh,
    Weights,
    Snap,
    Onion,
    Names,
    Bones,
    Help,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Hierarchy,
    DrawOrder,
    Constraint,
    Animation,
    Slot,
    Image,
    Search,
    Add,
    Skeleton,
    Expand,
    CollapseAll,
    Skin,
    Physics,
    ToastSuccess,
    ToastInfo,
    ToastWarning,
    ToastError,
    Key,
    Shear,
    Clip,
    Play,
    Pause,
    Close,
    Delete,
    Rename,
    Duplicate,
    Undo,
    Redo,
    ArrowUp,
    ArrowDown,
    Reset,
    Save,
    OpenFolder,
    NewFile,
    Export,
    Import,
    Loop,
    Settings,
    Copy,
    Paste,
    FrameAll,
}

const PNGS: [&[u8]; 57] = [
    include_bytes!("../assets/icons/select.png"),
    include_bytes!("../assets/icons/rotate.png"),
    include_bytes!("../assets/icons/move.png"),
    include_bytes!("../assets/icons/scale.png"),
    include_bytes!("../assets/icons/bone.png"),
    include_bytes!("../assets/icons/mesh.png"),
    include_bytes!("../assets/icons/weights.png"),
    include_bytes!("../assets/icons/snap.png"),
    include_bytes!("../assets/icons/onion.png"),
    include_bytes!("../assets/icons/names.png"),
    include_bytes!("../assets/icons/bones.png"),
    include_bytes!("../assets/icons/help.png"),
    include_bytes!("../assets/icons/eye.png"),
    include_bytes!("../assets/icons/eye_off.png"),
    include_bytes!("../assets/icons/lock.png"),
    include_bytes!("../assets/icons/unlock.png"),
    include_bytes!("../assets/icons/hierarchy.png"),
    include_bytes!("../assets/icons/draw_order.png"),
    include_bytes!("../assets/icons/constraint.png"),
    include_bytes!("../assets/icons/animation.png"),
    include_bytes!("../assets/icons/slot.png"),
    include_bytes!("../assets/icons/image.png"),
    include_bytes!("../assets/icons/search.png"),
    include_bytes!("../assets/icons/add.png"),
    include_bytes!("../assets/icons/skeleton.png"),
    include_bytes!("../assets/icons/expand.png"),
    include_bytes!("../assets/icons/collapse_all.png"),
    include_bytes!("../assets/icons/skin.png"),
    include_bytes!("../assets/icons/physics.png"),
    include_bytes!("../assets/icons/toast_success.png"),
    include_bytes!("../assets/icons/toast_info.png"),
    include_bytes!("../assets/icons/toast_warning.png"),
    include_bytes!("../assets/icons/toast_error.png"),
    include_bytes!("../assets/icons/key.png"),
    include_bytes!("../assets/icons/shear.png"),
    include_bytes!("../assets/icons/clip.png"),
    include_bytes!("../assets/icons/play.png"),
    include_bytes!("../assets/icons/pause.png"),
    include_bytes!("../assets/icons/close.png"),
    include_bytes!("../assets/icons/delete.png"),
    include_bytes!("../assets/icons/rename.png"),
    include_bytes!("../assets/icons/duplicate.png"),
    include_bytes!("../assets/icons/undo.png"),
    include_bytes!("../assets/icons/redo.png"),
    include_bytes!("../assets/icons/arrow_up.png"),
    include_bytes!("../assets/icons/arrow_down.png"),
    include_bytes!("../assets/icons/reset.png"),
    include_bytes!("../assets/icons/save.png"),
    include_bytes!("../assets/icons/open_folder.png"),
    include_bytes!("../assets/icons/new_file.png"),
    include_bytes!("../assets/icons/export.png"),
    include_bytes!("../assets/icons/import.png"),
    include_bytes!("../assets/icons/loop.png"),
    include_bytes!("../assets/icons/settings.png"),
    include_bytes!("../assets/icons/copy.png"),
    include_bytes!("../assets/icons/paste.png"),
    include_bytes!("../assets/icons/frame_all.png"),
];

static TEXTURES: OnceLock<Vec<TextureHandle>> = OnceLock::new();

/// Uploads the icons; call once at startup.
pub fn load(ctx: &egui::Context) {
    TEXTURES.get_or_init(|| {
        PNGS.iter()
            .enumerate()
            .map(|(i, png)| ctx.load_texture(format!("icon{i}"), decode(png), TextureOptions::LINEAR))
            .collect()
    });
}

/// The icon at `size` points, white (tint it, or let a button do it).
/// A button with an icon before its label; the icon follows the text color.
pub fn labeled<'a>(icon: Icon, text: impl Into<egui::WidgetText>) -> egui::Button<'a> {
    egui::Button::new((image(icon, 16.0), text.into())).image_tint_follows_text_color(true)
}

/// A small button that is only an icon. Give it hover text.
pub fn small<'a>(icon: Icon) -> egui::Button<'a> {
    egui::Button::new(image(icon, 14.0)).image_tint_follows_text_color(true)
}

pub fn image(icon: Icon, size: f32) -> egui::Image<'static> {
    let tex = &TEXTURES.get().expect("icons::load runs at startup")[icon as usize];
    egui::Image::from_texture((tex.id(), egui::vec2(size, size))).fit_to_exact_size(egui::vec2(size, size))
}

/// The texture of an icon, for painters that draw it at a fixed screen size.
pub fn texture_id(icon: Icon) -> egui::TextureId {
    TEXTURES.get().expect("icons::load runs at startup")[icon as usize].id()
}

/// The window icon.
pub fn app_icon() -> egui::IconData {
    let pm = tiny_skia::Pixmap::decode_png(include_bytes!("../assets/app_icon.png")).expect("bundled icon is a valid PNG");
    let rgba = pm.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    egui::IconData { rgba: rgba.collect(), width: pm.width(), height: pm.height() }
}

fn decode(png: &[u8]) -> ColorImage {
    let pm = tiny_skia::Pixmap::decode_png(png).expect("bundled icon is a valid PNG");
    // tiny-skia keeps premultiplied alpha, which is what egui textures want.
    ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data())
}
