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
    // Display effects: x = scanline strength (0 = off), y = game lines in
    // the texture (240); zw unused.
    fx: vec4<f32>,
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
    let c = textureSampleLevel(r_tex, r_sampler, uv, 0.0);
    if (u.fx.x <= 0.0) {
        return c;
    }
    // Scanlines at output resolution: darken the lower part of each game
    // line, faded out where a line is under ~2 output pixels (aliasing).
    let line = texel.y * u.fx.y / u.tex.y;
    let px_per_line = u.dest.w / u.src.w * u.tex.y / u.fx.y;
    let fade = clamp(px_per_line - 1.5, 0.0, 1.0);
    let dark = smoothstep(0.45, 0.85, fract(line)) * u.fx.x * 0.6 * fade;
    return vec4<f32>(c.rgb * (1.0 - dark), c.a);
}
";

/// Size of the `Locals` uniform (four `vec4<f32>`).
const LOCALS_BYTES: u64 = 64;

/// The uniform block for `vp` over a `tex`-sized texture, as the shader
/// reads it (native-endian `f32`s; every target z2rs ships is little-endian,
/// which is what wgpu expects).
#[must_use]
pub fn locals_bytes(vp: &Viewport, tex: (u32, u32)) -> [u8; LOCALS_BYTES as usize] {
    locals_bytes_fx(vp, tex, 0.0)
}

/// [`locals_bytes`] with a scanline strength (`0..=1`, display
/// enhancement; see [`crate::display_enh::scanline_strength`]).
#[must_use]
pub fn locals_bytes_fx(
    vp: &Viewport,
    tex: (u32, u32),
    scanlines: f32,
) -> [u8; LOCALS_BYTES as usize] {
    let vals: [f32; 16] = [
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
        scanlines.clamp(0.0, 1.0),
        z2_core::game::FRAME_H as f32,
        0.0,
        0.0,
    ];
    let mut out = [0u8; LOCALS_BYTES as usize];
    for (chunk, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(vals) {
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
    /// Scanline strength for the next [`Self::render`].
    scanlines: std::cell::Cell<f32>,
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
            scanlines: std::cell::Cell::new(0.0),
        }
    }

    /// Scanline strength (`0` = off) for the following renders.
    pub fn set_scanlines(&self, strength: f32) {
        self.scanlines.set(strength);
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
        queue.write_buffer(
            &self.uniform,
            0,
            &locals_bytes_fx(vp, tex, self.scanlines.get()),
        );
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

/// The graphics backends to try, in order, each as `(label, backends)`.
///
/// `env` (the `WGPU_BACKEND` variable, already parsed) wins outright. Then
/// the config's `gpu_backend`: a named backend is tried alone; `auto` (or
/// anything unknown) on Windows tries DirectX 12, then Vulkan, one at a
/// time, and elsewhere lets wgpu pick from everything.
///
/// Why one at a time on Windows: `pixels` creates its wgpu instance with
/// `Backends::all()` unless told otherwise, and wgpu 0.19 then initializes
/// *every* requested backend in `Instance::new` (wgpu-core `instance.rs`
/// calls `hal::Instance::init` for each backend in the mask) — the Vulkan
/// loader (with whatever implicit layers overlays and capture tools have
/// registered) and, for GL, `LoadLibraryA("opengl32.dll")` plus
/// `wglCreateContext` (wgpu-hal `gles/wgl.rs`), which loads the vendor's
/// OpenGL ICD (`atio6axx.dll` on AMD, `nvoglv64.dll` on NVIDIA) — even when
/// DX12 ends up drawing. A crash inside one of those drivers takes the whole
/// process down with 0xC0000005 before a window appears. DX12 alone touches
/// only `d3d12.dll` / `dxgi.dll` (wgpu-hal `dx12/instance.rs`).
///
/// Why OpenGL is not in the automatic Windows list: it is wgpu's
/// best-effort backend there, and the machines on which DX12 *and* Vulkan
/// both fail cleanly have no usable GPU driver at all — DXGI still offers
/// DX12 the WARP software adapter there, and Windows' generic OpenGL 1.1 is
/// far below what wgpu's GL backend needs. What the GL path does bring is
/// the vendor GL ICD, the module in the v0.4.0 AMD crash. It stays
/// reachable as `gpu_backend = "gl"` or `WGPU_BACKEND=gl`.
#[must_use]
pub fn backend_attempts(
    pref: &str,
    env: Option<wgpu::Backends>,
    windows: bool,
) -> Vec<(&'static str, wgpu::Backends)> {
    use wgpu::Backends as B;
    if let Some(b) = env {
        return vec![("WGPU_BACKEND", b)];
    }
    match named_backend(pref) {
        Some(named) => vec![named],
        None if windows => vec![("DX12", B::DX12), ("Vulkan", B::VULKAN)],
        None => vec![("auto", B::all())],
    }
}

/// `true` when [`backend_attempts`] picks the list itself (no
/// `WGPU_BACKEND`, and `gpu_backend` names no backend). Only such a list
/// skips backends that crashed before ([`crate::gpu_guard`]).
#[must_use]
pub fn backend_choice_is_automatic(pref: &str, env: Option<wgpu::Backends>) -> bool {
    env.is_none() && named_backend(pref).is_none()
}

fn named_backend(pref: &str) -> Option<(&'static str, wgpu::Backends)> {
    use wgpu::Backends as B;
    Some(match pref.trim().to_ascii_lowercase().as_str() {
        "dx12" | "d3d12" | "directx" => ("DX12", B::DX12),
        "vulkan" | "vk" => ("Vulkan", B::VULKAN),
        "gl" | "opengl" | "gles" => ("GL", B::GL),
        "metal" => ("Metal", B::METAL),
        _ => return None,
    })
}

/// Build the `pixels` context for `window`, trying [`backend_attempts`] in
/// order. A backend that errors (no adapter, device request refused) or
/// panics inside wgpu moves on to the next one; every attempt and the chosen
/// adapter are logged through [`crate::diag::breadcrumb`].
///
/// A native crash inside a driver cannot be caught here, so each attempt is
/// first recorded in the [`crate::gpu_guard`] file, cleared only once the
/// first frame is presented ([`crate::gpu_guard::confirm`]); the next launch
/// skips a backend the previous one died in.
///
/// The present mode is pixels' default, `AutoVsync`: wgpu-core resolves it
/// to `FifoRelaxed` when the surface offers it and otherwise to `Fifo`,
/// which every surface must support, so no refresh rate can make the
/// surface configure fail. Emulation speed never follows the refresh rate:
/// the loop steps from wall time (`FrameTimer`); vsync only decides how
/// often a finished frame is shown.
pub fn create_pixels(
    tex: (u32, u32),
    surface: (u32, u32),
    window: std::sync::Arc<winit::window::Window>,
    pref: &str,
) -> Result<pixels::Pixels<'static>, String> {
    use crate::diag::breadcrumb;
    let env = wgpu::util::backend_bits_from_env();
    let all = backend_attempts(pref, env, cfg!(target_os = "windows"));
    let automatic = backend_choice_is_automatic(pref, env);
    let attempts = crate::gpu_guard::with_guard(|guard, crashed_last| {
        if let Some(c) = &crashed_last {
            breadcrumb(format_args!(
                "gpu: the last run died while starting graphics backend {c} \
                 (a crash inside the graphics driver)"
            ));
        }
        let (kept, skipped) = crate::gpu_guard::filter_attempts(&all, guard.crashed(), automatic);
        if !skipped.is_empty() {
            let next: Vec<&str> = kept.iter().map(|(l, _)| *l).collect();
            breadcrumb(format_args!(
                "gpu: graphics backend {} crashed last time; trying {} \
                 (name it in \"gpu_backend\", or delete {} in the data folder, \
                 to try it again)",
                skipped.join(", "),
                next.join(", then "),
                crate::gpu_guard::GUARD_FILE_NAME,
            ));
        } else if let Some(c) = crashed_last.filter(|c| kept.iter().any(|(l, _)| l == c)) {
            breadcrumb(format_args!(
                "gpu: trying {c} again although it crashed last time (chosen \
                 explicitly, or every backend has crashed before)"
            ));
        }
        kept
    });
    let mut errors = Vec::new();
    for (label, backends) in attempts {
        breadcrumb(format_args!(
            "gpu: trying backend {label} ({backends:?}), surface {}x{}, texture {}x{}",
            surface.0, surface.1, tex.0, tex.1
        ));
        crate::gpu_guard::with_guard(|g, _| g.begin(label));
        let st = pixels::SurfaceTexture::new(surface.0, surface.1, std::sync::Arc::clone(&window));
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pixels::PixelsBuilder::new(tex.0, tex.1, st)
                .wgpu_backend(backends)
                .build()
        }));
        match built {
            Ok(Ok(p)) => {
                let info = p.adapter().get_info();
                breadcrumb(format_args!(
                    "gpu: using '{}' via {:?} (driver '{} {}', type {:?}, vendor 0x{:04x} device 0x{:04x}), surface format {:?}",
                    info.name,
                    info.backend,
                    info.driver,
                    info.driver_info,
                    info.device_type,
                    info.vendor,
                    info.device,
                    p.surface_texture_format(),
                ));
                return Ok(p);
            }
            Ok(Err(e)) => {
                crate::gpu_guard::with_guard(|g, _| g.failed());
                breadcrumb(format_args!("gpu: backend {label} failed: {e}"));
                errors.push(format!("{label}: {e}"));
            }
            Err(_) => {
                crate::gpu_guard::with_guard(|g, _| g.failed());
                breadcrumb(format_args!("gpu: backend {label} panicked (see above)"));
                errors.push(format!("{label}: panicked"));
            }
        }
    }
    Err(format!(
        "no usable graphics backend ({}); try setting \"gpu_backend\" in z2-native.json \
         or WGPU_BACKEND=dx12|vulkan|gl",
        errors.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{compute_viewport, ScaleMode};

    #[test]
    fn backend_order_prefers_dx12_alone_on_windows() {
        use wgpu::Backends as B;
        let order: Vec<B> = backend_attempts("auto", None, true)
            .into_iter()
            .map(|(_, b)| b)
            .collect();
        // OpenGL is never tried automatically on Windows (it loads the
        // vendor GL ICD, the v0.4.0 AMD crash), only when named.
        assert_eq!(order, vec![B::DX12, B::VULKAN]);
        assert!(order.iter().all(|b| !b.contains(B::GL)));
        assert_eq!(backend_attempts("opengl", None, true), vec![("GL", B::GL)]);
        // Garbage means auto.
        assert_eq!(backend_attempts("??", None, true).len(), 2);
        // Elsewhere wgpu picks (Metal on macOS, Vulkan/GL on Linux).
        assert_eq!(
            backend_attempts("auto", None, false),
            vec![("auto", B::all())]
        );
        // A named backend is tried alone, any case.
        assert_eq!(
            backend_attempts(" Vulkan ", None, true),
            vec![("Vulkan", B::VULKAN)]
        );
        assert_eq!(backend_attempts("gl", None, false), vec![("GL", B::GL)]);
        // WGPU_BACKEND beats the config.
        assert_eq!(
            backend_attempts("dx12", Some(B::GL), true),
            vec![("WGPU_BACKEND", B::GL)]
        );
    }

    #[test]
    fn only_automatic_choices_skip_crashed_backends() {
        use wgpu::Backends as B;
        assert!(backend_choice_is_automatic("auto", None));
        assert!(backend_choice_is_automatic("", None));
        assert!(backend_choice_is_automatic("??", None));
        assert!(!backend_choice_is_automatic("dx12", None));
        assert!(!backend_choice_is_automatic(" GL ", None));
        assert!(!backend_choice_is_automatic("auto", Some(B::GL)));
        // A DX12 crash on Windows falls through to Vulkan.
        let all = backend_attempts("auto", None, true);
        let (kept, skipped) = crate::gpu_guard::filter_attempts(&all, &["DX12".to_string()], true);
        assert_eq!(kept, vec![("Vulkan", B::VULKAN)]);
        assert_eq!(skipped, vec!["DX12"]);
    }

    #[test]
    fn locals_carry_the_viewport_and_the_full_texture_height() {
        let vp = compute_viewport((1920, 1080), (432, 240), ScaleMode::Fit, 8);
        let b = locals_bytes(&vp, (432, 240));
        let f: Vec<f32> = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_ne_bytes(*c))
            .collect();
        assert_eq!(&f[0..4], &[0.0, 0.0, 1920.0, 1080.0]);
        assert!((f[4] - 2.6667).abs() < 1e-3, "src_x {}", f[4]);
        assert!((f[6] - 426.6667).abs() < 1e-3, "src_w {}", f[6]);
        assert_eq!(f[5], 0.0, "never trims the top (HUD)");
        assert_eq!(f[7], 240.0, "full texture height");
        assert_eq!(&f[8..10], &[432.0, 240.0]);
        assert_eq!(&f[12..14], &[0.0, 240.0], "no scanlines by default");
        let fx: Vec<f32> = locals_bytes_fx(&vp, (432, 240), 0.5)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_ne_bytes(*c))
            .collect();
        assert_eq!(&fx[..12], &f[..12], "the viewport part is unchanged");
        assert_eq!(fx[12], 0.5);
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
