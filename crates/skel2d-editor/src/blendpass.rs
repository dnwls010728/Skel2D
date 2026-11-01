//! Draws slots with a blend mode egui's own pass cannot do. Runs inside egui's
//! render pass through a paint callback, so the slot keeps its place between
//! the other shapes.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Rect, TextureId};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use skel2d_core::BlendMode;

const SHADER: &str = r#"
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> }
@vertex fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> Out {
    return Out(vec4<f32>(position, 0.0, 1.0), uv, color);
}
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    return textureSample(image, linear, input.uv) * input.color;
}
"#;

/// The clipped slot: its image, cut by the alpha of a mask rendered for the viewport.
const CLIP_SHADER: &str = r#"
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) mask_uv: vec2<f32> }
@vertex fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> Out {
    return Out(vec4<f32>(position, 0.0, 1.0), uv, color, vec2<f32>(position.x * 0.5 + 0.5, 0.5 - position.y * 0.5));
}
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(1) @binding(0) var mask: texture_2d<f32>;
@group(1) @binding(1) var mask_linear: sampler;
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    return textureSample(image, linear, input.uv) * input.color * textureSample(mask, mask_linear, input.mask_uv).a;
}
"#;

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// x, y, u, v, r, g, b, a
const VERTEX_SIZE: u64 = 8 * 4;

fn blend_state(mode: BlendMode) -> wgpu::BlendState {
    use wgpu::BlendFactor::{Dst, One, OneMinusSrc, OneMinusSrcAlpha};
    let color = |src_factor, dst_factor| wgpu::BlendComponent { src_factor, dst_factor, operation: wgpu::BlendOperation::Add };
    wgpu::BlendState {
        color: match mode {
            BlendMode::Multiply => color(Dst, OneMinusSrcAlpha),
            BlendMode::Screen => color(One, OneMinusSrc),
            BlendMode::Additive => color(One, One),
            BlendMode::Normal => color(One, OneMinusSrcAlpha),
        },
        alpha: color(One, OneMinusSrcAlpha),
    }
}

struct BlendPass {
    pipelines: HashMap<&'static str, wgpu::RenderPipeline>,
    /// Per blend mode: draws a slot cut by a mask.
    clip_pipelines: HashMap<&'static str, wgpu::RenderPipeline>,
    /// Draws a clip base into its mask texture.
    mask_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<TextureId, wgpu::BindGroup>,
}

fn pipeline_key(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Additive => "additive",
        BlendMode::Normal => "normal",
    }
}

/// Sets the pass up. False when the target blends in linear light, where these
/// modes would not match the exported look: the slots then draw as normal.
pub fn install(state: &egui_wgpu::RenderState) -> bool {
    if state.target_format.is_srgb() { return false; }
    let device = &state.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("slot blend"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("slot blend"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("slot blend"), bind_group_layouts: &[Some(&layout)], ..Default::default() });
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
    let pipeline = |layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, format: wgpu::TextureFormat, mode: BlendMode| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slot blend"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend_state(mode)), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let pipelines = [BlendMode::Multiply, BlendMode::Screen, BlendMode::Additive].map(|mode| (pipeline_key(mode), pipeline(&pipeline_layout, &shader, state.target_format, mode)));
    let clip_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("slot clip"), source: wgpu::ShaderSource::Wgsl(CLIP_SHADER.into()) });
    let clip_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("slot clip"), bind_group_layouts: &[Some(&layout), Some(&layout)], ..Default::default() });
    let clip_pipelines = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Additive].map(|mode| (pipeline_key(mode), pipeline(&clip_layout, &clip_shader, state.target_format, mode)));
    let mask_pipeline = pipeline(&pipeline_layout, &shader, MASK_FORMAT, BlendMode::Normal);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("slot blend"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    state.renderer.write().callback_resources.insert(BlendPass { pipelines: pipelines.into_iter().collect(), clip_pipelines: clip_pipelines.into_iter().collect(), mask_pipeline, layout, sampler, textures: HashMap::new() });
    true
}

/// Premultiplied RGBA pixels of a slot's image.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

struct BlendDraw {
    mode: BlendMode,
    texture: TextureId,
    pixels: Arc<Pixels>,
    vertices: Vec<u8>,
    indices: Vec<u8>,
    buffers: OnceLock<(wgpu::Buffer, wgpu::Buffer)>,
    mask: Option<Arc<ClipMask>>,
}

/// A clip base for one frame: rendered once into a viewport-sized texture,
/// however many slots clip to it.
pub struct ClipMask {
    rect: Rect,
    texture: TextureId,
    pixels: Arc<Pixels>,
    vertices: Vec<u8>,
    indices: Vec<u8>,
    rendered: OnceLock<wgpu::BindGroup>,
}

pub fn clip_mask(rect: Rect, texture: TextureId, pixels: Arc<Pixels>, mesh: &egui::Mesh) -> Arc<ClipMask> {
    let (vertices, indices) = encode(rect, mesh);
    Arc::new(ClipMask { rect, texture, pixels, vertices, indices, rendered: OnceLock::new() })
}

/// The paint callback for one slot. `rect` is the viewport: egui maps clip
/// space onto it while the callback paints.
pub fn shape(rect: Rect, mode: BlendMode, texture: TextureId, pixels: Arc<Pixels>, mesh: &egui::Mesh, mask: Option<Arc<ClipMask>>) -> egui::Shape {
    let (vertices, indices) = encode(rect, mesh);
    egui_wgpu::Callback::new_paint_callback(rect, BlendDraw { mode, texture, pixels, vertices, indices, buffers: OnceLock::new(), mask }).into()
}

/// Vertex and index bytes, with positions in the clip space of `rect`.
fn encode(rect: Rect, mesh: &egui::Mesh) -> (Vec<u8>, Vec<u8>) {
    let mut vertices = Vec::with_capacity(mesh.vertices.len() * VERTEX_SIZE as usize);
    for v in &mesh.vertices {
        let x = (v.pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
        let y = 1.0 - (v.pos.y - rect.top()) / rect.height() * 2.0;
        let [r, g, b, a] = v.color.to_array().map(|c| c as f32 / 255.0);
        for value in [x, y, v.uv.x, v.uv.y, r, g, b, a] {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
    }
    (vertices, mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect())
}

fn gpu_buffer(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("slot blend"), size: bytes.len() as u64, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    queue.write_buffer(&buffer, 0, bytes);
    buffer
}

impl BlendPass {
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: TextureId, pixels: &Pixels) {
        if self.textures.contains_key(&id) { return; }
        let size = wgpu::Extent3d { width: pixels.width, height: pixels.height, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("slot blend"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Plain bytes: this pass blends in the same space as the rest of the frame.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &pixels.data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pixels.width * 4), rows_per_image: None },
            size,
        );
        let view = texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slot blend"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.textures.insert(id, group);
    }

    /// Renders a clip base into a new texture and returns it ready to sample.
    fn render_mask(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, screen: &ScreenDescriptor, mask: &ClipMask) -> Option<wgpu::BindGroup> {
        self.upload(device, queue, mask.texture, &mask.pixels);
        let size = wgpu::Extent3d {
            width: ((mask.rect.width() * screen.pixels_per_point).ceil() as u32).max(1),
            height: ((mask.rect.height() * screen.pixels_per_point).ceil() as u32).max(1),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("slot clip mask"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let vertices = gpu_buffer(device, queue, &mask.vertices, wgpu::BufferUsages::VERTEX);
        let indices = gpu_buffer(device, queue, &mask.indices, wgpu::BufferUsages::INDEX);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("slot clip mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mask_pipeline);
            pass.set_bind_group(0, self.textures.get(&mask.texture)?, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..(mask.indices.len() / 4) as u32, 0, 0..1);
        }
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slot clip mask"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }))
    }
}

impl CallbackTrait for BlendDraw {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, screen: &ScreenDescriptor, encoder: &mut wgpu::CommandEncoder, resources: &mut CallbackResources) -> Vec<wgpu::CommandBuffer> {
        if let Some(pass) = resources.get_mut::<BlendPass>() {
            pass.upload(device, queue, self.texture, &self.pixels);
            if let Some(mask) = &self.mask
                && mask.rendered.get().is_none()
                && let Some(group) = pass.render_mask(device, queue, encoder, screen, mask)
            {
                let _ = mask.rendered.set(group);
            }
        }
        self.buffers.get_or_init(|| (gpu_buffer(device, queue, &self.vertices, wgpu::BufferUsages::VERTEX), gpu_buffer(device, queue, &self.indices, wgpu::BufferUsages::INDEX)));
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(blend) = resources.get::<BlendPass>() else { return };
        let pipelines = if self.mask.is_some() { &blend.clip_pipelines } else { &blend.pipelines };
        let (Some(pipeline), Some(group), Some((vertices, indices))) = (pipelines.get(pipeline_key(self.mode)), blend.textures.get(&self.texture), self.buffers.get()) else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        if let Some(mask) = &self.mask {
            let Some(rendered) = mask.rendered.get() else { return };
            pass.set_bind_group(1, rendered, &[]);
        }
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..(self.indices.len() / 4) as u32, 0, 0..1);
    }
}
