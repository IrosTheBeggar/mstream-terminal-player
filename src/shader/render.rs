//! Presets, drawn: the GPU half of the visualizer.
//!
//! A [`Gpu`] is a device plus everything every preset shares: the pipeline
//! layout the translation's bindings promise (`glsl::layout`), the one
//! sampler, the audio texture, and a black texture for a channel wired to
//! nothing. A [`Scene`] is one preset compiled on it — a pipeline per pass,
//! a ping-pong pair per buffer — and draws a frame into any RGBA8 view.
//!
//! Offscreen for now: `viz-probe` draws every preset into a texture and
//! reads it back. The window (Phase 10.1) hands the same [`Scene::draw`]
//! its surface's view instead.

use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::audio;
use super::glsl::{self, Uniforms, layout};
use super::preset::{Channel, MAX_PARAMS, PassName, Preset};
use crate::runtime::block_on;

/// What every pass draws into: plain RGBA8, as Android's buffers are — so a
/// feedback buffer holds exactly what it holds on a phone. Not an sRGB
/// format: a preset's colours are already what it wants on screen, and an
/// sRGB target would encode them a second time, washing everything out.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One triangle that covers the target. The fragment stage is the preset;
/// this only has to put a fragment under every pixel.
const VERTEX: &str = "
@vertex
fn main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}
";

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    uniform_layout: wgpu::BindGroupLayout,
    channel_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vertex: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    audio: wgpu::Texture,
    audio_view: wgpu::TextureView,
    black_view: wgpu::TextureView,
}

impl Gpu {
    pub fn new(adapter: &wgpu::Adapter) -> Result<Gpu, String> {
        // WebGL2's limits are the floor every backend clears — a GLES 3.0
        // driver included — and four textures and 176 bytes of uniforms sit
        // far inside them. Texture sizes follow the adapter's own ceiling.
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mstream visualizer"),
            required_limits: limits,
            ..Default::default()
        }))?
        .map_err(|e| format!("the GPU would not open a device: {e}"))?;

        let fragment = wgpu::ShaderStages::FRAGMENT;
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ShaderToy uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: layout::UNIFORM_BINDING,
                visibility: fragment,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(layout::SIZE as u64),
                },
                count: None,
            }],
        });
        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: fragment,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: layout::SAMPLER_BINDING,
            visibility: fragment,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        let channel_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iChannel0-3"),
            entries: &entries,
        });
        // Group order is the translation's: uniforms, then channels.
        debug_assert_eq!((layout::UNIFORM_GROUP, layout::CHANNEL_GROUP), (0, 1));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preset pass"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&channel_layout)],
            immediate_size: 0,
        });

        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen triangle"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(VERTEX)),
        });
        // Linear and clamped, for every channel: what Android sets on the
        // audio texture and on every buffer it renders.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("iChannel sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let audio = texture(
            &device,
            "audio",
            (audio::WIDTH as u32, audio::HEIGHT as u32),
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let audio_view = audio.create_view(&Default::default());
        let black = texture(
            &device,
            "unbound channel",
            (1, 1),
            FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        write(&queue, &black, (1, 1), 4, &[0, 0, 0, 255]);
        let black_view = black.create_view(&Default::default());

        Ok(Gpu {
            device,
            queue,
            uniform_layout,
            channel_layout,
            pipeline_layout,
            vertex,
            sampler,
            audio,
            audio_view,
            black_view,
        })
    }

    /// Hand the shaders this frame's audio texture — `AudioTexture`'s bytes.
    pub fn upload_audio(&self, bytes: &[u8]) {
        let size = (audio::WIDTH as u32, audio::HEIGHT as u32);
        write(&self.queue, &self.audio, size, 1, bytes);
    }

    /// Compile every pass of `preset` for this device, with its buffers
    /// sized for an output of `size`. A pass that will not compile fails the
    /// whole preset, with the pass named and the compiler's reason — which
    /// can be a panic inside naga, caught here so it costs one preset.
    pub fn load(&self, preset: &Preset, size: (u32, u32)) -> Result<Scene, String> {
        let mut passes = Vec::new();
        for pass in &preset.passes {
            let name = pass.name.as_str();
            let source = glsl::translate(preset, pass).map_err(|e| format!("{name}: {e}"))?;
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let built = catch_unwind(AssertUnwindSafe(|| self.pipeline(name, source)));
            let error = block_on(scope.pop())?;
            let pipeline = match (built, error) {
                (Err(_), _) => return Err(format!("{name}: the shader compiler panicked")),
                (Ok(_), Some(error)) => return Err(format!("{name}: {error}")),
                (Ok(pipeline), None) => pipeline,
            };
            let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(name),
                size: layout::SIZE as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout: &self.uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: layout::UNIFORM_BINDING,
                    resource: uniforms.as_entire_binding(),
                }],
            });
            passes.push(Compiled {
                name: pass.name,
                pipeline,
                uniforms,
                uniform_group,
                channels: pass.channels,
                fixed: pass.size,
            });
        }

        let mut scene = Scene {
            passes,
            buffers: Default::default(),
            params: preset.default_params(),
            frame: 0,
            size: (0, 0),
        };
        scene.resize(self, size);
        Ok(scene)
    }

    fn pipeline(&self, name: &str, source: String) -> wgpu::RenderPipeline {
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Glsl {
                shader: Cow::Owned(source),
                stage: wgpu::naga::ShaderStage::Fragment,
                defines: &[],
            },
        });
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

/// One preset, compiled, with its buffers.
pub struct Scene {
    passes: Vec<Compiled>,
    /// Buffers A to D, where the preset has them.
    buffers: [Option<PingPong>; 4],
    params: [f32; MAX_PARAMS],
    frame: i32,
    /// The output size the full-size buffers were made for.
    size: (u32, u32),
}

struct Compiled {
    name: PassName,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    channels: [Channel; 4],
    fixed: Option<(u32, u32)>,
}

/// A buffer pass's two textures: one written this frame, the other holding
/// the last, which is what a pass reading itself sees.
struct PingPong {
    views: [wgpu::TextureView; 2],
    size: (u32, u32),
    write: usize,
}

fn slot(pass: PassName) -> usize {
    match pass {
        PassName::BufferA => 0,
        PassName::BufferB => 1,
        PassName::BufferC => 2,
        PassName::BufferD => 3,
        PassName::Image => unreachable!("the image pass has no buffer"),
    }
}

impl Scene {
    /// Remake the full-size buffers for a new output size, blank. A fixed
    /// size buffer keeps its contents: a 1×1 state buffer does not care
    /// what the window did.
    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        for pass in self.passes.iter().filter(|p| p.name.is_buffer()) {
            let wanted = pass.fixed.unwrap_or(size);
            let current = &mut self.buffers[slot(pass.name)];
            if current.as_ref().is_some_and(|b| b.size == wanted) {
                continue;
            }
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            let views = std::array::from_fn(|_| {
                texture(&gpu.device, pass.name.as_str(), wanted, FORMAT, usage).create_view(&Default::default())
            });
            *current = Some(PingPong { views, size: wanted, write: 0 });
        }
        self.size = size;
    }

    pub fn passes(&self) -> usize {
        self.passes.len()
    }

    /// Draw one frame into `target`, which must be [`FORMAT`] and the size
    /// last given to [`Scene::resize`]. `time` is seconds since the preset
    /// started and `delta` since the last frame; the audio texture is
    /// whatever was last uploaded.
    pub fn draw(&mut self, gpu: &Gpu, target: &wgpu::TextureView, time: f32, delta: f32) {
        for buffer in self.buffers.iter_mut().flatten() {
            buffer.write ^= 1;
        }
        let output = [self.size.0 as f32, self.size.1 as f32];
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        for pass in &self.passes {
            let (view, size) = match pass.name {
                PassName::Image => (target, self.size),
                name => {
                    let buffer = self.buffers[slot(name)].as_ref().expect("resize made every buffer");
                    (&buffer.views[buffer.write], buffer.size)
                }
            };
            let uniforms = Uniforms {
                resolution: [size.0 as f32, size.1 as f32],
                time,
                time_delta: delta,
                frame: self.frame,
                params: self.params,
                channel_resolution: output,
            };
            gpu.queue.write_buffer(&pass.uniforms, 0, &uniforms.to_bytes());

            let channels: [&wgpu::TextureView; 4] =
                std::array::from_fn(|i| self.channel(gpu, pass.channels[i], pass.name));
            let mut entries: Vec<wgpu::BindGroupEntry> = channels
                .iter()
                .enumerate()
                .map(|(i, view)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry {
                binding: layout::SAMPLER_BINDING,
                resource: wgpu::BindingResource::Sampler(&gpu.sampler),
            });
            let channel_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(pass.name.as_str()),
                layout: &gpu.channel_layout,
                entries: &entries,
            });

            let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(pass.name.as_str()),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render.set_pipeline(&pass.pipeline);
            render.set_bind_group(layout::UNIFORM_GROUP, &pass.uniform_group, &[]);
            render.set_bind_group(layout::CHANNEL_GROUP, &channel_group, &[]);
            render.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
        self.frame += 1;
    }

    /// What a channel reads for the pass `reader`. A buffer that ran earlier
    /// this frame gives this frame's pixels; itself, or one still to run,
    /// gives last frame's. ShaderToy's rule, which is what an imported shader
    /// was written against. Android agrees on the first two, but hands a
    /// buffer still to run over as it was the frame before last — its write
    /// index flips only once the whole frame is drawn. No bundled preset
    /// reads a later buffer, so the two never show the difference.
    fn channel<'a>(&'a self, gpu: &'a Gpu, channel: Channel, reader: PassName) -> &'a wgpu::TextureView {
        match channel {
            Channel::Unbound => &gpu.black_view,
            Channel::Audio => &gpu.audio_view,
            Channel::Buffer(buffer) => match &self.buffers[slot(buffer)] {
                // Wired to a buffer the file does not have: black, as on
                // Android.
                None => &gpu.black_view,
                Some(ping) => &ping.views[if buffer < reader { ping.write } else { ping.write ^ 1 }],
            },
        }
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn write(queue: &wgpu::Queue, texture: &wgpu::Texture, (width, height): (u32, u32), texel: u32, bytes: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * texel),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
}

/// An offscreen [`FORMAT`] target the probe can read back.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
}

impl Offscreen {
    pub fn new(gpu: &Gpu, size: (u32, u32)) -> Offscreen {
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let texture = texture(&gpu.device, "offscreen", size, FORMAT, usage);
        let view = texture.create_view(&Default::default());
        Offscreen { texture, view, size }
    }

    /// Wait for the GPU, then copy the picture out as tightly packed RGBA.
    pub fn read(&self, gpu: &Gpu) -> Result<Vec<u8>, String> {
        let (width, height) = self.size;
        // Rows of a texture-to-buffer copy are padded to 256 bytes.
        let row = width * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("the GPU did not finish: {e}"))?;
        let mapped = slice.get_mapped_range().map_err(|e| format!("the readback would not map: {e}"))?;
        let mut pixels = Vec::with_capacity((row * height) as usize);
        for y in 0..height as usize {
            let start = y * padded as usize;
            pixels.extend_from_slice(&mapped[start..start + row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }

    /// Wait for everything submitted so far — for timing a frame.
    pub fn finish(gpu: &Gpu) -> Result<(), String> {
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map(drop).map_err(|e| e.to_string())
    }
}
