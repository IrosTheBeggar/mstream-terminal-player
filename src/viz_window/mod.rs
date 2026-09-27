//! `viz-window`: the visualizer's window, a child process of the player
//! (PLAN.md, Phase 11.1; docs/ux-contracts/visualizer-window.md).
//!
//! A process of its own because AppKit wants the process's first thread for
//! the event loop and the player's belongs to the terminal — and because a
//! preset that panics the shader compiler must cost the window, not the
//! player. The parent computes the audio texture from its own playback and
//! writes it down this process's stdin ([`pipe`]); this end only draws:
//! winit for the window and its keys, the same wgpu [`Scene`] the probe
//! draws with, at the window's logical size, scaled onto the surface by one
//! blit. EOF on stdin is the parent gone, and the window closes with it.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::runtime::block_on;
use crate::shader::library::BUILTIN;
use crate::shader::preset::Preset;
use crate::shader::render::{Gpu, Offscreen, Scene};

pub mod pipe;
use pipe::Message;

const TITLE: &str = "mStream Visualizer";
/// The window as it opens: the presets' 16:9, at a size that sits beside a
/// terminal.
const OPENING: (u32, u32) = (960, 540);

#[derive(clap::Args)]
pub struct WindowArgs {
    /// The preset to open on, by its number in the library's order (1-based)
    #[arg(long, default_value_t = 1)]
    pub preset: usize,

    /// Open fullscreen
    #[arg(long)]
    pub fullscreen: bool,
}

pub fn run(args: WindowArgs) -> i32 {
    let event_loop = match EventLoop::<Message>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("viz-window: no display to open a window on ({e})");
            return 1;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    spawn_reader(event_loop.create_proxy());
    let mut app = App::new(args);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("viz-window: {e}");
        return 1;
    }
    app.exit_code
}

/// The parent's messages, read off stdin on a thread of their own and
/// handed to the event loop. EOF or a broken pipe is the parent gone.
fn spawn_reader(proxy: EventLoopProxy<Message>) {
    std::thread::Builder::new()
        .name("viz-window stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                match pipe::read(&mut stdin) {
                    Ok(Some(message)) => {
                        if proxy.send_event(message).is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = proxy.send_event(Message::Quit);
                        return;
                    }
                }
            }
        })
        .expect("a thread for stdin");
}

struct App {
    fullscreen_at_open: bool,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    /// The preset in front, by its index in `BUILTIN`.
    preset: usize,
    titles: Vec<String>,
    scenes: Vec<Option<Scene>>,
    /// A preset this GPU refused: skipped rather than retried.
    refused: Vec<bool>,
    audio: Vec<u8>,
    audio_fresh: bool,
    occluded: bool,
    started: Instant,
    last_frame: Instant,
    exit_code: i32,
}

struct Gfx {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    /// The preset draws here at logical size; the blit scales it up.
    target: Offscreen,
    blit: Blit,
}

impl App {
    fn new(args: WindowArgs) -> App {
        let count = BUILTIN.len();
        let titles = BUILTIN
            .iter()
            .map(|builtin| {
                let title = Preset::parse(builtin.source).ok().and_then(|p| p.title);
                format!("{} {}", &builtin.file[..2], title.unwrap_or_else(|| builtin.file.to_string()))
            })
            .collect();
        App {
            fullscreen_at_open: args.fullscreen,
            window: None,
            gfx: None,
            preset: args.preset.clamp(1, count) - 1,
            titles,
            scenes: (0..count).map(|_| None).collect(),
            refused: vec![false; count],
            audio: vec![0; pipe::AUDIO_LEN],
            audio_fresh: false,
            occluded: false,
            started: Instant::now(),
            last_frame: Instant::now(),
            exit_code: 0,
        }
    }

    /// The window, its surface, the GPU and the blit — contract clauses 4–6.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(OPENING.0, OPENING.1));
        let window = Arc::new(
            event_loop.create_window(attributes).map_err(|e| format!("the window would not open: {e}"))?,
        );
        // The display handle goes in with the instance: on Wayland and X11
        // the backend needs it before a surface can exist.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("the window has no drawing surface: {e}"))?;
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        };
        let adapter = match block_on(instance.request_adapter(&options)) {
            Ok(Ok(adapter)) => adapter,
            Ok(Err(e)) => return Err(format!("no GPU would draw the window: {e}")),
            Err(e) => return Err(e.to_string()),
        };
        let gpu = Gpu::new(&adapter)?;

        // The preset's colours are already what it wants on screen (see
        // `render::FORMAT`): a plain format shows them as they are, and an
        // sRGB-only surface gets the blit that undoes the second encoding.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("the surface offers no format to draw in")?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("the surface would take no configuration")?;
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&gpu.device, &config);

        let target = Offscreen::for_display(&gpu, logical_size(&window));
        let mut blit = Blit::new(&gpu, format);
        blit.bind(&gpu, &target.view);
        self.gfx = Some(Gfx { surface, config, gpu, target, blit });

        window.focus_window();
        if self.fullscreen_at_open {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        self.retitle(&window);
        window.request_redraw();
        self.window = Some(window);
        Ok(())
    }

    fn retitle(&self, window: &Window) {
        window.set_title(&format!("{TITLE} — {}", self.titles[self.preset]));
    }

    /// The window changed size or scale: the surface follows the pixels,
    /// the target the points (contract clause 5).
    fn resize(&mut self) {
        let (Some(window), Some(gfx)) = (&self.window, self.gfx.as_mut()) else { return };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            gfx.config.width = 0;
            gfx.config.height = 0;
            return;
        }
        if (gfx.config.width, gfx.config.height) != (size.width, size.height) {
            gfx.config.width = size.width;
            gfx.config.height = size.height;
            gfx.surface.configure(&gfx.gpu.device, &gfx.config);
        }
        let logical = logical_size(window);
        if gfx.target.size != logical {
            gfx.target = Offscreen::for_display(&gfx.gpu, logical);
            gfx.blit.bind(&gfx.gpu, &gfx.target.view);
            for scene in self.scenes.iter_mut().flatten() {
                scene.resize(&gfx.gpu, logical);
            }
        }
    }

    /// The preset in front, compiled on first sight. A preset the GPU
    /// refuses is skipped; refused all round, the window gives up.
    fn ensure_scene(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(gfx) = self.gfx.as_ref() else { return false };
        for _ in 0..BUILTIN.len() {
            if self.scenes[self.preset].is_some() {
                return true;
            }
            if !self.refused[self.preset] {
                let builtin = &BUILTIN[self.preset];
                let loaded = Preset::parse(builtin.source)
                    .map_err(|e| e.to_string())
                    .and_then(|preset| gfx.gpu.load(&preset, gfx.target.size));
                match loaded {
                    Ok(scene) => {
                        self.scenes[self.preset] = Some(scene);
                        return true;
                    }
                    Err(e) => {
                        eprintln!("viz-window: {} does not draw on this GPU: {e}", builtin.file);
                        self.refused[self.preset] = true;
                    }
                }
            }
            self.preset = (self.preset + 1) % BUILTIN.len();
        }
        eprintln!("viz-window: no preset draws on this GPU");
        self.exit_code = 1;
        event_loop.exit();
        false
    }

    /// `←` `→`: the next preset that draws (contract clause 7).
    fn step(&mut self, by: isize) {
        let count = BUILTIN.len() as isize;
        for _ in 0..count {
            self.preset = ((self.preset as isize + by).rem_euclid(count)) as usize;
            if !self.refused[self.preset] {
                break;
            }
        }
        if let Some(window) = &self.window {
            self.retitle(window);
            window.request_redraw();
        }
    }

    fn toggle_fullscreen(&self) {
        if let Some(window) = &self.window {
            let next = if window.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) };
            window.set_fullscreen(next);
        }
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop) {
        if !self.ensure_scene(event_loop) {
            return;
        }
        let Some(gfx) = self.gfx.as_mut() else { return };
        if gfx.config.width == 0 || gfx.config.height == 0 {
            return;
        }
        let now = Instant::now();
        let time = now.duration_since(self.started).as_secs_f32();
        let delta = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if self.audio_fresh {
            gfx.gpu.upload_audio(&self.audio);
            self.audio_fresh = false;
        }
        let scene = self.scenes[self.preset].as_mut().expect("ensured above");
        scene.draw(&gfx.gpu, &gfx.target.view, time, delta);

        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.gpu.device, &gfx.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let view = frame.texture.create_view(&Default::default());
        gfx.blit.draw(&gfx.gpu, &view);
        gfx.gpu.queue.present(frame);
    }
}

impl ApplicationHandler<Message> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.open(event_loop) {
            eprintln!("viz-window: {e}");
            self.exit_code = 1;
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: Message) {
        match message {
            Message::Audio(bytes) => {
                self.audio = bytes;
                self.audio_fresh = true;
            }
            Message::Preset(index) => {
                self.preset = (index as usize).min(BUILTIN.len() - 1);
                if let Some(window) = &self.window {
                    self.retitle(window);
                    window.request_redraw();
                }
            }
            Message::Raise => {
                if let Some(window) = &self.window {
                    window.focus_window();
                }
            }
            Message::Quit => event_loop.exit(),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resize(),
            // Behind another window or minimized: nothing is drawn until it
            // shows again (contract clause 6).
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded && let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) | Key::Character("q") => event_loop.exit(),
                    Key::Named(NamedKey::ArrowRight) => self.step(1),
                    Key::Named(NamedKey::ArrowLeft) => self.step(-1),
                    Key::Character("f") => self.toggle_fullscreen(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.draw(event_loop);
                // Paced by the display: Fifo's acquire waits for the next
                // vertical blank, so this is one frame per refresh, not a
                // spin.
                if !self.occluded && let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// The window's size in points — what the preset shades at.
fn logical_size(window: &Window) -> (u32, u32) {
    let size: LogicalSize<f64> = window.inner_size().to_logical(window.scale_factor());
    (size.width.round().max(1.0) as u32, size.height.round().max(1.0) as u32)
}

/// One triangle that copies the target onto the surface, upscaled by the
/// sampler, and undoes an sRGB surface's encoding when it has to.
const BLIT: &str = "
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Out {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: Out;
    out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return textureSample(picture, linear_sampler, in.uv);
}

@fragment
fn fs_srgb(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(picture, linear_sampler, in.uv);
    return vec4<f32>(pow(c.rgb, vec3<f32>(2.2)), c.a);
}
";

struct Blit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    group: Option<wgpu::BindGroup>,
}

impl Blit {
    fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Blit {
        let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT.into()),
        });
        let layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
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
        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if format.is_srgb() { "fs_srgb" } else { "fs" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Blit { pipeline, layout, sampler, group: None }
    }

    /// Point the blit at a (new) target.
    fn bind(&mut self, gpu: &Gpu, target: &wgpu::TextureView) {
        self.group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(target) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    fn draw(&self, gpu: &Gpu, surface_view: &wgpu::TextureView) {
        let Some(group) = &self.group else { return };
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
    }
}
