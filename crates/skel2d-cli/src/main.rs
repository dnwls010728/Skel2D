//! `skel2d` command line tool. Every command prints compact JSON on stdout
//! and, on failure, `{"error": ..., "issues": [...]}` on stderr with exit
//! code 1, so scripts and AI agents can drive it without parsing prose.

mod commands;
mod mcp;
mod rig;
mod serve;
mod session;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "skel2d", version, about = "Skel2D 2D skeletal animation tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Animation to sample; setup pose when omitted.
    #[arg(long)]
    pub anim: Option<String>,
    /// Time in seconds (with --anim).
    #[arg(long, default_value_t = 0.0)]
    pub time: f32,
    /// Render N evenly spaced frames side by side (with --anim).
    #[arg(long)]
    pub frames: Option<u32>,
    #[arg(long, default_value_t = 512)]
    pub width: u32,
    #[arg(long, default_value_t = 512)]
    pub height: u32,
    /// Pixels per world unit; fits the content when omitted.
    #[arg(long)]
    pub zoom: Option<f32>,
    /// Hide the bone overlay.
    #[arg(long)]
    pub no_bones: bool,
    /// World point at the image center, as X,Y (with --zoom for close-ups).
    #[arg(long, value_parser = parse_point, allow_hyphen_values = true)]
    pub center: Option<[f32; 2]>,
    /// Extra animation layered on top, as NAME or NAME@SECONDS (repeatable),
    /// in order (first is lowest), like animation tracks: e.g. a face pose, then a
    /// blink. Without @ it follows the frame time, looping over its own duration.
    #[arg(long = "track")]
    pub tracks: Vec<String>,
    /// Ordered preview tracks as JSON [{"anim":"blink","alpha":0.5,"enabled":true}].
    /// Applied after --track entries; first is lowest. The main anim is skipped.
    #[arg(long)]
    pub layers: Option<String>,
    /// Active skin (repeatable; later ones win where several fill a placeholder).
    #[arg(long = "skin")]
    pub skins: Vec<String>,
    #[command(flatten)]
    pub physics: PhysicsArgs,
}

/// Animated GIF settings; see `skel2d_render::GifOptions`.
#[derive(clap::Args)]
pub struct GifArgs {
    /// Animation to play; the setup pose when omitted.
    #[arg(long)]
    pub anim: Option<String>,
    /// Animation layered on top, following the clock (repeatable, first is lowest).
    #[arg(long = "track")]
    pub tracks: Vec<String>,
    /// Active skin (repeatable; later ones win).
    #[arg(long = "skin")]
    pub skins: Vec<String>,
    /// One file per animation; -o is then a folder.
    #[arg(long)]
    pub all_animations: bool,
    /// One file per skin; -o is then a folder.
    #[arg(long)]
    pub all_skins: bool,
    /// Frames per second (at most 50, the best GIF can time).
    #[arg(long, default_value_t = 50.0)]
    pub gif_fps: f32,
    /// Palette size, 2..=256.
    #[arg(long, default_value_t = 256)]
    pub colors: u16,
    #[arg(long)]
    pub dither: bool,
    /// 1..=10: better colors take longer.
    #[arg(long, default_value_t = 5)]
    pub quality: u8,
    /// See-through background.
    #[arg(long)]
    pub transparent: bool,
    /// With --transparent: pixels below this alpha (0..=255) become transparent.
    #[arg(long, default_value_t = 128)]
    pub alpha_threshold: u8,
    /// Background color as RRGGBB.
    #[arg(long, default_value = "ffffff")]
    pub background: String,
    /// Play once instead of looping.
    #[arg(long)]
    pub no_loop: bool,
    /// Keep the frame at the very end, which repeats the first one in a loop.
    #[arg(long)]
    pub include_last_frame: bool,
    /// Plays of the animation stored in the file.
    #[arg(long, default_value_t = 1)]
    pub plays: u32,
    /// Seconds to hold the last frame of each play.
    #[arg(long, default_value_t = 0.0)]
    pub pause_after: f32,
    /// Plays simulated first, so physics starts in motion.
    #[arg(long, default_value_t = 0)]
    pub warm_up: u32,
    /// Draw the bones.
    #[arg(long)]
    pub bones: bool,
    /// Plain animated pose, without simulating physics.
    #[arg(long)]
    pub no_physics: bool,
    /// Size in percent of world units; ignored with --fit.
    #[arg(long, default_value_t = 100.0)]
    pub scale: f32,
    /// Fit the content into this many pixels, as WxH.
    #[arg(long, value_parser = parse_size)]
    pub fit: Option<[u32; 2]>,
    /// With --fit: small content grows until it touches the limit.
    #[arg(long)]
    pub enlarge: bool,
    /// With --fit: the image is exactly that size, with space around the content.
    #[arg(long)]
    pub pad: bool,
    /// World rectangle X,Y,WIDTH,HEIGHT instead of the content's bounds.
    #[arg(long, value_parser = parse_rect, allow_hyphen_values = true)]
    pub crop: Option<[f32; 4]>,
    /// Frames to keep, as FIRST-LAST counted from 0.
    #[arg(long, value_parser = parse_range)]
    pub range: Option<[u32; 2]>,
}

/// Physics of a pose with animations: simulated from time 0, deterministically.
#[derive(clap::Args, Default)]
pub struct PhysicsArgs {
    /// "simulate" (default) or "none".
    #[arg(long)]
    pub physics: Option<String>,
    /// Simulation frames per second (1..=1000, default 60).
    #[arg(long)]
    pub fps: Option<f32>,
    /// World path of the whole skeleton for physics to react to, as "x,y@t;x,y@t".
    #[arg(long = "move", allow_hyphen_values = true)]
    pub moves: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Create an empty document.
    New {
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Check a document and list every problem found.
    Validate { file: PathBuf },
    /// Summarize bones, slots and animations.
    Info { file: PathBuf },
    /// Apply edit ops (a JSON op or array of ops) atomically and save.
    Apply {
        file: PathBuf,
        /// Ops file, or "-" for stdin. Use --ops to pass JSON inline instead.
        ops_file: Option<PathBuf>,
        /// Inline ops JSON.
        #[arg(long, conflicts_with = "ops_file")]
        ops: Option<String>,
        /// Validate and report without saving.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print world-space bone transforms and slot state.
    Pose {
        file: PathBuf,
        #[arg(long)]
        anim: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        time: f32,
        /// Report only the events fired after this clock time, up to --time.
        #[arg(long)]
        since: Option<f32>,
        /// Ordered preview tracks as JSON, same as render --layers.
        #[arg(long)]
        layers: Option<String>,
        /// Active skin (repeatable; later ones win).
        #[arg(long = "skin")]
        skins: Vec<String>,
        #[command(flatten)]
        physics: PhysicsArgs,
    },
    /// Render the setup pose or an animation frame to PNG.
    Render {
        file: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        args: RenderArgs,
    },
    /// Export animations as animated GIF.
    Gif {
        file: PathBuf,
        /// Output file, or folder with --all-animations / --all-skins.
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        args: GifArgs,
    },
    /// Export animations as a sprite sheet: one PNG plus a JSON file with each
    /// frame's position and duration.
    Sheet {
        file: PathBuf,
        /// Output PNG, or folder with --all-animations / --all-skins.
        #[arg(short, long)]
        out: PathBuf,
        /// Frames per row; 0 picks a nearly square sheet.
        #[arg(long, default_value_t = 0)]
        columns: u32,
        /// Empty pixels between frames.
        #[arg(long, default_value_t = 0)]
        padding: u32,
        #[command(flatten)]
        args: GifArgs,
    },
    /// Analyze a PNG (size, visible bounds, islands). With --grid, also write
    /// a copy with labeled pixel coordinates to read joint positions from.
    ImageInfo {
        image: PathBuf,
        #[arg(long)]
        grid: Option<PathBuf>,
        /// Grid spacing in pixels.
        #[arg(long, default_value_t = 50)]
        step: u32,
        /// Joint spec to mark on the grid image (checks a spec before rigging).
        #[arg(long)]
        spec: Option<PathBuf>,
    },
    /// Build a rigged document from a PNG and a rig spec (joints in image
    /// pixels). See `skel2d schema rig`.
    Rig {
        /// A PNG, or a layers.json manifest (see `skel2d schema layers`).
        image: PathBuf,
        /// Spec file, or use --spec-json.
        #[arg(long)]
        spec: Option<PathBuf>,
        #[arg(long, conflicts_with = "spec")]
        spec_json: Option<String>,
        #[arg(short, long)]
        out: PathBuf,
        /// Mesh density in pixels: grid cell size, or vertex spacing of outline meshes (default: long side / 16).
        #[arg(long)]
        cell: Option<usize>,
        /// Regular grid mesh instead of an adaptive outline mesh.
        #[arg(long)]
        grid_mesh: bool,
        /// Rigid image region instead of a weighted mesh.
        #[arg(long)]
        no_mesh: bool,
        #[arg(long, default_value_t = 3)]
        max_influences: usize,
        /// Overwrite an existing output file.
        #[arg(long)]
        force: bool,
    },
    /// Cut a parts sheet into one PNG per part (reading order). Pieces closer
    /// than --merge pixels stay together (e.g. a pair of eyes).
    Slice {
        sheet: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value = "part")]
        prefix: String,
        #[arg(long, default_value_t = 40)]
        merge: usize,
        #[arg(long, default_value_t = 4)]
        padding: u32,
        /// Ignore specks smaller than this many pixels.
        #[arg(long, default_value_t = 50)]
        min_pixels: usize,
        /// Remove a flat background color first (chroma key), e.g. 00ff00.
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value_t = 60)]
        key_tolerance: u8,
        /// Clean AI-generated noise first: alpha below this becomes transparent
        /// and specks under --min-pixels are erased (0 = off; try 32).
        #[arg(long, default_value_t = 0)]
        defringe: u8,
    },
    /// Cut named rectangles out of a sheet: --rects file with
    /// [{"name": "eyes_open", "rect": [x0, y0, x1, y1]}, ...] (pixels).
    Crop {
        sheet: PathBuf,
        #[arg(long)]
        rects: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value_t = 60)]
        key_tolerance: u8,
        #[arg(long, default_value_t = 0)]
        defringe: u8,
        #[arg(long, default_value_t = 50)]
        min_pixels: usize,
    },
    /// Split a layered PSD into one PNG per top-level group (or ungrouped
    /// layer; every layer with --layers), plus layers.json (back to front)
    /// and composite.png.
    Psd {
        file: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Skip this layer or group, e.g. a flattened reference (repeatable).
        #[arg(long)]
        exclude: Vec<String>,
        /// One part per layer; groups only organize them (layer-per-part PSD).
        #[arg(long)]
        layers: bool,
        /// One part per innermost group, for PSDs with nested groups.
        #[arg(long, conflicts_with = "layers")]
        inner_groups: bool,
        /// Leave out layers hidden in the PSD (default: import them).
        #[arg(long)]
        skip_hidden: bool,
    },
    /// Remove a flat background color (chroma key) and write a transparent PNG.
    Key {
        image: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value = "00ff00")]
        color: String,
        #[arg(long, default_value_t = 60)]
        tolerance: u8,
    },
    /// Print a JSON Schema: "document", "ops", "rig", "layers" (rig manifest), or "tracks" (preview).
    Schema {
        #[arg(default_value = "ops")]
        kind: String,
    },
    /// Serve a document over a local HTTP API (127.0.0.1 only).
    Serve {
        file: PathBuf,
        #[arg(long, default_value_t = 7878)]
        port: u16,
    },
    /// Serve a document as an MCP server over stdio (for AI agents).
    Mcp { file: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::New { file, name, force } => commands::new(&file, name, force),
        Command::Validate { file } => commands::validate(&file),
        Command::Info { file } => commands::info(&file),
        Command::Apply { file, ops_file, ops, dry_run } => commands::apply(&file, ops_file, ops, dry_run),
        Command::Pose { file, anim, time, since, layers, skins, physics } => commands::pose(&file, anim.as_deref(), time, since, layers.as_deref(), &skins, &physics),
        Command::Render { file, out, args } => commands::render(&file, &out, &args),
        Command::Gif { file, out, args } => commands::gif(&file, &out, &args),
        Command::Sheet { file, out, columns, padding, args } => commands::sheet(&file, &out, columns, padding, &args),
        Command::ImageInfo { image, grid, step, spec } => rig::image_info(&image, grid.as_deref(), step, spec.as_deref()),
        Command::Rig { image, spec, spec_json, out, cell, grid_mesh, no_mesh, max_influences, force } => {
            rig::rig(&image, spec.as_deref(), spec_json, &out, cell, grid_mesh, no_mesh, max_influences, force)
        }
        Command::Slice { sheet, out, prefix, merge, padding, min_pixels, key, key_tolerance, defringe } => {
            rig::KeyArgs::new(key.as_deref(), key_tolerance).and_then(|k| rig::slice(&sheet, &out, &prefix, merge, padding, min_pixels, k, defringe))
        }
        Command::Crop { sheet, rects, out, key, key_tolerance, defringe, min_pixels } => {
            rig::KeyArgs::new(key.as_deref(), key_tolerance).and_then(|k| rig::crop(&sheet, &rects, &out, k, defringe, min_pixels))
        }
        Command::Psd { file, out, exclude, layers, inner_groups, skip_hidden } => rig::psd(&file, &out, &exclude, if layers { skel2d_rig::psd::Split::Layers } else if inner_groups { skel2d_rig::psd::Split::Innermost } else { skel2d_rig::psd::Split::Groups }, if skip_hidden { skel2d_rig::psd::Hidden::Skip } else { skel2d_rig::psd::Hidden::Include }),
        Command::Key { image, out, color, tolerance } => rig::key(&image, &out, &color, tolerance),
        Command::Schema { kind } => commands::schema(&kind),
        Command::Serve { file, port } => serve::run(&file, port),
        Command::Mcp { file } => mcp::run(&file),
    };
    match result {
        Ok(value) => {
            if !value.is_null() {
                println!("{}", skel2d_core::format::to_compact(&value).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}", commands::error_json(&err));
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn parse_point(s: &str) -> Result<[f32; 2], String> {
    let (x, y) = s.split_once(',').ok_or("expected X,Y")?;
    Ok([x.trim().parse().map_err(|_| "bad X")?, y.trim().parse().map_err(|_| "bad Y")?])
}

fn parse_size(s: &str) -> Result<[u32; 2], String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH")?;
    Ok([w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?])
}

fn parse_rect(s: &str) -> Result<[f32; 4], String> {
    let v: Vec<f32> = s.split(',').map(str::parse).collect::<Result<_, _>>().map_err(|_| "expected X,Y,WIDTH,HEIGHT")?;
    <[f32; 4]>::try_from(v).map_err(|_| "expected X,Y,WIDTH,HEIGHT".to_string())
}

fn parse_range(s: &str) -> Result<[u32; 2], String> {
    let (a, b) = s.split_once('-').ok_or("expected FIRST-LAST")?;
    Ok([a.parse().map_err(|_| "bad first frame")?, b.parse().map_err(|_| "bad last frame")?])
}
