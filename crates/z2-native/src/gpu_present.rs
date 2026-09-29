//! Windowed blit of the `pixels` texture into a [`Viewport`].
//!
//! `pixels` 0.15's built-in `ScalingRenderer` scales only by whole numbers
//! and never below 1x: in fullscreen on a 2560x1600 or 1920x1080 display that
//! left thick black borders on every side (6x of 240 is 1440, not 1600), and a
//! texture larger than the window — any HD pack in a DPI-scaled window — was
//! cropped rather than shrunk. This renderer replaces it inside
//! [`pixels::Pixels::render_with`] and draws the texture into whatever
//! rectangle [`crate::app::compute_viewport`] chose, fractional scale
//! included.
//!
//! Sampling is "sharp bilinear": each texel is magnified by the whole-number
//! part of the scale with nearest-neighbour, and only the one-pixel seams
//! between texels are blended. Integer scales therefore stay perfectly crisp,
//! fractional ones get no uneven "fat pixel" columns, and a texture drawn
//! smaller than 1x falls back to plain bilinear.
//!
//! Everything is decided per fragment from `@builtin(position)`, so no wgpu
//! viewport or scissor rectangle is set: a surface that is momentarily a
//! different size than the viewport was computed for (a resize still in
//! flight) can never trip wgpu's viewport validation.

use pixels::wgpu;

use crate::app::Viewport;

const SHADER: &str = r"
struct Locals {
    // Drawn rectangle on the surface: x, y, w, h (surface pixels).
    dest: vec4<f32>,
    // Visible texture window: x, y, w, h (texels).
    src: vec4<f32>,
    // Texture size in texels (xy); zw unused.
    tex: vec4<f32>,
};

@group(0) @binding(0) var r_tex: texture_2d<f32>;
@group(0) @binding(1) var r_sampler: sampler;
@group(0) @binding(2) var<uniform> u: Locals;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole surface.
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = pos.xy - u.dest.xy;
    if (p.x < 0.0 || p.y < 0.0 || p.x >= u.dest.z || p.y >= u.dest.w) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let texels_per_px = u.src.zw / u.dest.zw;
    let texel = u.src.xy + p * texels_per_px;
    // Sharp bilinear: nearest within each texel's integer-scaled core, a
    // one-pixel blend at its edges.
    let prescale = max(floor(vec2<f32>(1.0) / texels_per_px), vec2<f32>(1.0));
    let base = floor(texel);
    let d = texel - base - vec2<f32>(0.5);
    let region = vec2<f32>(0.5) - vec2<f32>(0.5) / prescale;
    let f = (d - clamp(d, -region, region)) * prescale + vec2<f32>(0.5);
    let uv = (base + f) / u.tex.xy;
    return textureSampleLevel(r_tex, r_sampler, uv, 0.0);
}
";

/// Size of the `Locals` uniform (three `vec4<f32>`).
const LOCALS_BYTES: u64 = 48;

/// The uniform block for `vp` over a `tex`-sized texture, as the shader
/// reads it (native-endian `f32`s; every target z2rs ships is little-endian,
/// which is what wgpu expects).
#[must_use]
pub fn locals_bytes(vp: &Viewport, tex: (u32, u32)) -> [u8; LOCALS_BYTES as usize] {
    let vals: [f32; 12] = [
        vp.x as f32,
        vp.y as f32,
        vp.w.max(1) as f32,
        vp.h.max(1) as f32,
        vp.src_x as f32,
        0.0,
        vp.src_w.max(1e-3) as f32,
        tex.1.max(1) as f32,
        tex.0.max(1) as f32,
        tex.1.max(1) as f32,
        0.0,
        0.0,
    ];
    let mut out = [0u8; LOCALS_BYTES as usize];
    for (chunk, v) in out.chunks_exact_mut(4).zip(vals) {
        chunk.copy_from_slice(&v.to_ne_bytes());
    }
    out
}

/// GPU objects for the viewport blit. Build once per `pixels` instance; call
/// [`ViewportRenderer::rebind`] after every `Pixels::resize_buffer` (which
/// replaces the texture).
#[derive(Debug)]
pub struct ViewportRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl ViewportRenderer {
    /// Build the pipeline for `pixels`' device, texture and target format.
    #[must_use]
    pub fn new(pixels: &pixels::Pixels<'_>) -> Self {
        let device = pixels.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("z2_viewport_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("z2_viewport_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("z2_viewport_locals"),
            size: LOCALS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("z2_viewport_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(LOCALS_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("z2_viewport_pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("z2_viewport_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: "vs_main",
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: pixels.render_texture_format(),
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });
        let bind_group = Self::bind(device, &layout, &sampler, &uniform, pixels.texture());
        Self {
            pipeline,
            layout,
            sampler,
            uniform,
            bind_group,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        uniform: &wgpu::Buffer,
        texture: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("z2_viewport_bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        })
    }

    /// Re-point at `pixels`' current texture (call after `resize_buffer`).
    pub fn rebind(&mut self, pixels: &pixels::Pixels<'_>) {
        self.bind_group = Self::bind(
            pixels.device(),
            &self.layout,
            &self.sampler,
            &self.uniform,
            pixels.texture(),
        );
    }

    /// Draw the texture into `vp` on `target`, black everywhere else. Called
    /// from inside `Pixels::render_with`.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        queue: &wgpu::Queue,
        vp: &Viewport,
        tex: (u32, u32),
    ) {
        queue.write_buffer(&self.uniform, 0, &locals_bytes(vp, tex));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("z2_viewport_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{compute_viewport, ScaleMode};

    #[test]
    fn locals_carry_the_viewport_and_the_full_texture_height() {
        let vp = compute_viewport((1920, 1080), (432, 240), ScaleMode::Fit, 8);
        let b = locals_bytes(&vp, (432, 240));
        let f: Vec<f32> = b
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(&f[0..4], &[0.0, 0.0, 1920.0, 1080.0]);
        assert!((f[4] - 2.6667).abs() < 1e-3, "src_x {}", f[4]);
        assert!((f[6] - 426.6667).abs() < 1e-3, "src_w {}", f[6]);
        assert_eq!(f[5], 0.0, "never trims the top (HUD)");
        assert_eq!(f[7], 240.0, "full texture height");
        assert_eq!(&f[8..10], &[432.0, 240.0]);
    }

    /// The shader must parse and validate with the naga wgpu 0.19 uses, or
    /// the windowed app would panic creating the pipeline.
    #[test]
    fn shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("WGSL validates");
        let entries: Vec<&str> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            entries.contains(&"vs_main") && entries.contains(&"fs_main"),
            "{entries:?}"
        );
    }
}
