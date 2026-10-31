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
}

const PNGS: [&[u8]; 29] = [
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
