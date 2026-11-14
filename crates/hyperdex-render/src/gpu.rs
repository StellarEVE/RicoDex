//! The GPU compositor: the same transitions as the CPU oracle, as
//! WGSL shaders over wgpu, rendering offscreen and reading frames
//! back. This is the fast path the architecture calls for, and it
//! exists to be measured against the oracle: the test suite renders
//! identical inputs through both and reports the difference.
//!
//! Implemented on the GPU in this slice: Cross Dissolve, Dip to
//! Color and Crash Zoom, selected by a mode uniform in one shader.
//! [`GpuCompositor::supports`] is the honest list; the render job
//! falls back to the CPU oracle for everything else, and for
//! machines where no wgpu adapter exists at all.

use crate::error::RenderError;
use crate::frame::Frame;
use crate::transition::{TransitionKind, TransitionParams};

const SHADER: &str = r#"
struct Uniforms {
    progress: f32,
    mode: u32,
    pad0: f32,
    pad1: f32,
    dip: vec4<f32>,
}
@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: Uniforms;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    var out: VsOut;
    let x = f32((idx << 1u) & 2u);
    let y = f32(idx & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let p = u.progress;
    if (u.mode == 0u) {
        // Cross Dissolve.
        let a = textureSample(tex_a, samp, in.uv);
        let b = textureSample(tex_b, samp, in.uv);
        return mix(a, b, p);
    }
    if (u.mode == 1u) {
        // Dip to Color.
        let a = textureSample(tex_a, samp, in.uv);
        let b = textureSample(tex_b, samp, in.uv);
        if (p < 0.5) {
            return mix(a, u.dip, p * 2.0);
        }
        return mix(u.dip, b, (p - 0.5) * 2.0);
    }
    // Crash Zoom: the outgoing frame punches in towards the centre
    // as progress rises, the incoming frame settles back from the
    // same punch, and the cut over happens across the middle of the
    // move. The same maths as the CPU oracle, and the form the
    // compositor spike first proved.
    let centre = vec2<f32>(0.5, 0.5);
    let zoom_a = 1.0 + 3.0 * p * p;
    let zoom_b = 1.0 + 3.0 * (1.0 - p) * (1.0 - p);
    let uv_a = centre + (in.uv - centre) / zoom_a;
    let uv_b = centre + (in.uv - centre) / zoom_b;
    let cut = smoothstep(0.35, 0.65, p);
    let a = textureSample(tex_a, samp, uv_a);
    let b = textureSample(tex_b, samp, uv_b);
    return mix(a, b, cut);
}
"#;

fn mode_of(kind: TransitionKind) -> Option<u32> {
    match kind {
        TransitionKind::CrossDissolve => Some(0),
        TransitionKind::DipToColor => Some(1),
        TransitionKind::CrashZoom => Some(2),
        _ => None,
    }
}

/// A wgpu device and the transition pipeline, ready to composite.
/// Construction is the expensive part; one instance serves a whole
/// render job.
pub struct GpuCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adapter_name: String,
}

impl GpuCompositor {
    /// The transitions this compositor implements. Everything else
    /// belongs to the CPU oracle in this slice.
    pub fn supports(kind: TransitionKind) -> bool {
        mode_of(kind).is_some()
    }

    /// Initialise wgpu, preferring a fallback (software) adapter and
    /// then any adapter. Returns None when no adapter exists, which
    /// callers treat as "use the CPU oracle", never as an error.
    pub fn new() -> Option<GpuCompositor> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Option<GpuCompositor> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let mut adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            })
            .await;
        if adapter.is_none() {
            adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await;
        }
        let adapter = adapter?;
        let info = adapter.get_info();
        let adapter_name = format!(
            "{} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("hyperdex-render device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .ok()?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hyperdex-render transitions"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("hyperdex-render bind layout"),
                entries: &[
                    texture_entry(0),
                    texture_entry(1),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(
                            wgpu::SamplerBindingType::Filtering,
                        ),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hyperdex-render pipeline layout"),
                bind_group_layouts: &[&bind_layout],
                push_constant_ranges: &[],
            });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hyperdex-render transition pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hyperdex-render linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Some(GpuCompositor {
            device,
            queue,
            pipeline,
            bind_layout,
            sampler,
            adapter_name,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Composite one frame on the GPU. Errors for transition kinds
    /// the GPU path does not implement (check
    /// [`GpuCompositor::supports`] first) and for size mismatches.
    pub fn composite(
        &self,
        a: &Frame,
        b: &Frame,
        kind: TransitionKind,
        progress: f32,
        params: &TransitionParams,
    ) -> Result<Frame, RenderError> {
        let mode = mode_of(kind).ok_or_else(|| RenderError::Gpu {
            reason: format!("{} is not implemented on the GPU path", kind.name()),
        })?;
        if a.width != b.width || a.height != b.height {
            return Err(RenderError::SizeMismatch {
                a: (a.width, a.height),
                b: (b.width, b.height),
            });
        }
        let (width, height) = (a.width, a.height);
        let p = progress.clamp(0.0, 1.0);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let row_bytes = width as usize * 4;
        let padded_row = row_bytes.div_ceil(256) * 256;

        let upload = |data: &[u8], label: &str| {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes as u32),
                    rows_per_image: Some(height),
                },
                size,
            );
            tex
        };
        let tex_a = upload(&a.pixels, "hyperdex-render frame a");
        let tex_b = upload(&b.pixels, "hyperdex-render frame b");
        let view_a = tex_a.create_view(&Default::default());
        let view_b = tex_b.create_view(&Default::default());

        let mut uniform_bytes = [0u8; 32];
        uniform_bytes[0..4].copy_from_slice(&p.to_le_bytes());
        uniform_bytes[4..8].copy_from_slice(&mode.to_le_bytes());
        uniform_bytes[16..20]
            .copy_from_slice(&(params.dip_color[0] as f32 / 255.0).to_le_bytes());
        uniform_bytes[20..24]
            .copy_from_slice(&(params.dip_color[1] as f32 / 255.0).to_le_bytes());
        uniform_bytes[24..28]
            .copy_from_slice(&(params.dip_color[2] as f32 / 255.0).to_le_bytes());
        uniform_bytes[28..32].copy_from_slice(&1.0f32.to_le_bytes());
        let uniform_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hyperdex-render uniforms"),
            size: uniform_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform_buf, 0, &uniform_bytes);

        let out_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hyperdex-render output"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out_tex.create_view(&Default::default());
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hyperdex-render readback"),
            size: (padded_row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hyperdex-render bind group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view_a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view_b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("hyperdex-render frame encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hyperdex-render transition pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
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
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &out_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row as u32),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        self.device.poll(wgpu::Maintain::Wait);

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|_| RenderError::Gpu {
                reason: "readback channel closed".to_string(),
            })?
            .map_err(|e| RenderError::Gpu {
                reason: format!("readback map failed: {e:?}"),
            })?;
        let mapped = slice.get_mapped_range();
        let mut pixels = vec![0u8; row_bytes * height as usize];
        for y in 0..height as usize {
            let src = y * padded_row;
            let dst = y * row_bytes;
            pixels[dst..dst + row_bytes].copy_from_slice(&mapped[src..src + row_bytes]);
        }
        drop(mapped);
        readback.unmap();
        Frame::new(width, height, pixels)
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Compare two frames pixel by pixel: the maximum and the mean
/// absolute per channel difference. The harness the GPU versus CPU
/// agreement tests (and any caller) use to measure the fast path
/// against the oracle.
pub fn frame_difference(a: &Frame, b: &Frame) -> Result<(u8, f64), RenderError> {
    if a.width != b.width || a.height != b.height {
        return Err(RenderError::SizeMismatch {
            a: (a.width, a.height),
            b: (b.width, b.height),
        });
    }
    let mut max = 0u8;
    let mut total = 0u64;
    for (pa, pb) in a.pixels.iter().zip(b.pixels.iter()) {
        let diff = pa.abs_diff(*pb);
        if diff > max {
            max = diff;
        }
        total += diff as u64;
    }
    let mean = if a.pixels.is_empty() {
        0.0
    } else {
        total as f64 / a.pixels.len() as f64
    };
    Ok((max, mean))
}
