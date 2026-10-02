//! The band's waveform as pixels: the waveform spike (2026-10-02).
//!
//! The terminal draws the Now Playing band as eighth blocks, one bar a
//! column and eight heights a row, mirrored over two rows: about ninety
//! bars of the server's 800, at sixteen heights. The window has the
//! pixels for all 800, so it paints the shape itself, the way it paints
//! covers (covers.rs): the drawing path blanks the bar's cells and records
//! the wave on the [`Board`](super::covers::Board); [`WavePass`] paints it
//! after the covers, in the same encoder.
//!
//! The peaks go up once per track as an R8 texture, 800 texels by one; a
//! uniform per frame carries where the band is, how far the playhead has
//! gone, where the pointer is, the three colours and the terminal's
//! stretch for the band's width. One quad, and its fragment shader works
//! out two mirrored envelopes at its own pixel:
//!
//! - the body: the root mean square of the bars in a window one column
//!   wide, centred on the pixel, then stretched as `resample_bars` stretches
//!   a column — the terminal's shape, sliding smoothly instead of stepping
//!   a cell at a time (at a column's centre it IS the terminal's column);
//! - the peaks: each bar itself (linear between two bars wider than a
//!   pixel, the loudest under a pixel narrower than one), on the same
//!   stretch, drawn faint behind the body — the detail the columns average
//!   away.
//!
//! Each is inked to its edge with a pixel of analytic anti-aliasing
//! (nothing here multisamples) and blended over the blank ground the text
//! blit drew there.

use std::sync::Arc;

use ratatui::layout::Rect;
use wgpu::{CommandEncoder, Device, Queue, RenderPipeline, SurfaceConfiguration, TextureView};

/// How many tracks' peaks stay on the GPU: the one playing and the one
/// before it, so a skip back does not upload again. 800 bytes each.
const KEEP_TRACKS: usize = 2;

/// The uniform's size: seven `vec4<f32>`s (see `Wave` in [`SHADER`]).
const UNIFORM_BYTES: u64 = 112;

/// How strongly the bars' own peaks show behind the column-wide body: a
/// texture under the shape, not a second shape.
const PEAK_INK: f32 = 0.35;

/// One wave this frame, as the drawing path recorded it.
#[derive(Clone, Debug)]
pub(super) struct PlacedWave {
    pub key: u64,
    /// The bars scaled to the loudest (`tui::ui::wave_shape`).
    pub peaks: Arc<[u8]>,
    /// The terminal's stretch for this width, in fractions of the loudest
    /// bar: a column's root mean square `e` stands `(e - floor) / span`.
    pub floor: f32,
    pub span: f32,
    /// The bar's own cells, both rows of the band.
    pub rect: Rect,
    pub grid: (u16, u16),
    pub progress: f32,
    /// The bar's column under the pointer.
    pub hover: Option<u16>,
    /// Played, unplayed, marker: the theme's bytes, resolved through the
    /// window's palette (a name answers as the cells' text does).
    pub colours: [[u8; 3]; 3],
    /// An overlay registered after it touches it: not painted this frame.
    pub under: bool,
}

/// What a wave paints, to the pixel, on a surface of a given size: two
/// frames whose waves agree on this paint the same pixels, so the second
/// needs no present. Progress is kept as the playhead's pixel, not the
/// fraction, so a playing track presents when the split moves a pixel
/// rather than on every frame the clock moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    key: u64,
    rect: Rect,
    grid: (u16, u16),
    played: i64,
    hover: Option<u16>,
    colours: [[u8; 3]; 3],
}

impl PlacedWave {
    /// The band in surface pixels: the cells' share of the grid, as a
    /// cover's box is (covers.rs `quad`), on whole pixels.
    fn pixels(&self, surface: (f32, f32)) -> [f32; 4] {
        let (cols, rows) = (f32::from(self.grid.0.max(1)), f32::from(self.grid.1.max(1)));
        let r = self.rect;
        [
            (f32::from(r.x) / cols * surface.0).round(),
            (f32::from(r.y) / rows * surface.1).round(),
            (f32::from(r.x + r.width) / cols * surface.0).round(),
            (f32::from(r.y + r.height) / rows * surface.1).round(),
        ]
    }

    pub(super) fn seen(&self, surface: (u32, u32)) -> Seen {
        let [left, _, right, _] = self.pixels((surface.0 as f32, surface.1 as f32));
        Seen {
            key: self.key,
            rect: self.rect,
            grid: self.grid,
            played: (f64::from(self.progress) * f64::from(right - left)).round() as i64,
            hover: self.hover,
            colours: self.colours,
        }
    }
}

/// One track's peaks on the GPU.
struct Track {
    key: u64,
    bind: wgpu::BindGroup,
    used: u64,
}

/// The wave's pipeline and what it keeps between frames.
pub(super) struct WavePass {
    pipeline: RenderPipeline,
    peaks_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The surface stores sRGB: the colours go to the shader decoded to
    /// linear, which the store encodes back to the theme's own bytes, as
    /// the text blit's decode does for the cells (VENDORED.md change 3).
    srgb: bool,
    tracks: Vec<Track>,
    /// A uniform buffer and its bindings per wave drawn in one frame: each
    /// draw reads its own, since every write lands before the submit.
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// What the last process painted, for `changed`.
    drawn: Vec<Seen>,
    /// The surface the last process painted on, in pixels.
    surface: (u32, u32),
    processed: u64,
    uploads: u64,
    painted: u64,
}

impl WavePass {
    pub(super) fn compile(device: &Device, surface_config: &SurfaceConfiguration) -> WavePass {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Wave Uniform Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let peaks_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Wave Peaks Layout"),
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
        // Linear between bars: where a bar is wider than a pixel the
        // envelope slopes from one to the next instead of stepping.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Wave Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wave Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Wave Layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&peaks_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wave Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // Blended, unlike a cover: the envelope's edge is a pixel
                // of partial ink over the ground, and everything outside
                // the envelope is the ground itself.
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        WavePass {
            pipeline,
            peaks_layout,
            uniform_layout,
            sampler,
            srgb: surface_config.format.is_srgb(),
            tracks: Vec::new(),
            slots: Vec::new(),
            drawn: Vec::new(),
            surface: (0, 0),
            processed: 0,
            uploads: 0,
            painted: 0,
        }
    }

    /// What the waves cost, for the stats lever.
    pub(super) fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "tracks": self.tracks.len(),
            "uploads": self.uploads,
            "painted": self.painted,
        })
    }

    /// Whether `waves` would paint anything other than what the last
    /// process painted: a hover move, a seek or a new track on a frame
    /// whose cells did not change (the bar's cells are blank either way).
    pub(super) fn changed(&self, waves: &[PlacedWave]) -> bool {
        let seen = waves.iter().filter(|wave| !wave.under).map(|wave| wave.seen(self.surface));
        !seen.eq(self.drawn.iter().copied())
    }

    /// The track's peaks as a texture, uploaded on first sight; past
    /// [`KEEP_TRACKS`] the track drawn longest ago goes, never one this
    /// frame draws.
    fn track(&mut self, device: &Device, queue: &Queue, wave: &PlacedWave) {
        if let Some(track) = self.tracks.iter_mut().find(|track| track.key == wave.key) {
            track.used = self.processed;
            return;
        }
        let max = device.limits().max_texture_dimension_2d as usize;
        let peaks = &wave.peaks[..wave.peaks.len().min(max)];
        let size = wgpu::Extent3d {
            width: peaks.len() as u32,
            height: 1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Wave Peaks"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            peaks,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width),
                rows_per_image: Some(1),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Wave Peaks Bindings"),
            layout: &self.peaks_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.uploads += 1;
        let processed = self.processed;
        let stale = |at: &usize| self.tracks[*at].used < processed;
        if self.tracks.len() >= KEEP_TRACKS
            && let Some(oldest) =
                (0..self.tracks.len()).filter(stale).min_by_key(|&at| self.tracks[at].used)
        {
            self.tracks.remove(oldest);
        }
        self.tracks.push(Track { key: wave.key, bind, used: processed });
    }

    fn slot(&mut self, device: &Device, at: usize) {
        while self.slots.len() <= at {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Wave Uniform"),
                size: UNIFORM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Wave Uniform Bindings"),
                layout: &self.uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.slots.push((buffer, bind));
        }
    }

    /// A theme colour as the shader wants it for this surface.
    fn colour(&self, [r, g, b]: [u8; 3]) -> [f32; 4] {
        let channel = |byte: u8| {
            let c = f32::from(byte) / 255.0;
            if !self.srgb {
                c
            } else if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [channel(r), channel(g), channel(b), 1.0]
    }

    /// Paint this frame's waves over what the surface holds.
    pub(super) fn process(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        surface_config: &SurfaceConfiguration,
        surface_view: &TextureView,
        waves: &[PlacedWave],
    ) {
        self.processed += 1;
        self.surface = (surface_config.width, surface_config.height);
        let surface = (surface_config.width as f32, surface_config.height as f32);
        // Each wave's uniform slot and its track's key.
        let mut draws: Vec<(usize, u64)> = Vec::new();
        let mut drawn = Vec::new();
        for wave in waves.iter().filter(|wave| !wave.under && !wave.peaks.is_empty()) {
            let [left, top, right, bottom] = wave.pixels(surface);
            if right <= left || bottom <= top {
                continue;
            }
            self.track(device, queue, wave);
            let slot = draws.len();
            self.slot(device, slot);
            let ndc = |x: f32, y: f32| [x / surface.0 * 2.0 - 1.0, 1.0 - y / surface.1 * 2.0];
            let ([x0, y0], [x1, y1]) = (ndc(left, top), ndc(right, bottom));
            // The marker is the glyph band's caret, a left eighth of a
            // cell, at the left edge of the column a click there seeks to.
            let cell_w = surface.0 / f32::from(wave.grid.0.max(1));
            let hover = wave
                .hover
                .map_or(-1.0, |column| f32::from(column) / f32::from(wave.rect.width.max(1)));
            let [played, unplayed, marker] = wave.colours.map(|c| self.colour(c));
            let params = [
                wave.progress.clamp(0.0, 1.0),
                hover,
                (bottom - top) / 2.0,
                (cell_w / 8.0).max(1.0).round(),
            ];
            // A column's worth of bars: the window the body averages over.
            let window = wave.peaks.len() as f32 / f32::from(wave.rect.width.max(1));
            let shape = [wave.floor, wave.span.max(f32::EPSILON), window, PEAK_INK];
            let floats: Vec<f32> = [
                [x0, y0, x1, y1],
                [left, top, right, bottom],
                played,
                unplayed,
                marker,
                params,
                shape,
            ]
            .concat();
            let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
            queue.write_buffer(&self.slots[slot].0, 0, &bytes);
            draws.push((slot, wave.key));
            drawn.push(wave.seen(self.surface));
        }
        self.drawn = drawn;
        if draws.is_empty() {
            return;
        }
        self.painted += draws.len() as u64;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Wave Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        for (slot, key) in draws {
            let Some(track) = self.tracks.iter().find(|track| track.key == key) else { continue };
            pass.set_bind_group(0, &self.slots[slot].1, &[]);
            pass.set_bind_group(1, &track.bind, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// One quad over the band; the envelopes worked out at each pixel.
const SHADER: &str = r"
struct Wave {
    // The quad: left, top, right, bottom in NDC.
    ndc: vec4<f32>,
    // The same box in surface pixels.
    rect: vec4<f32>,
    played: vec4<f32>,
    unplayed: vec4<f32>,
    marker: vec4<f32>,
    // Progress 0..1, the hover's x 0..1 or -1, the envelopes' half height
    // in pixels, the marker's width in pixels.
    params: vec4<f32>,
    // The terminal's stretch (floor, span, in fractions of the loudest
    // bar), how many bars a column holds, and how strongly the peaks show.
    shape: vec4<f32>,
};

@group(0) @binding(0) var<uniform> wave: Wave;
@group(1) @binding(0) var peaks: texture_2d<f32>;
@group(1) @binding(1) var peaks_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    let x = mix(wave.ndc.x, wave.ndc.z, corner.x);
    let y = mix(wave.ndc.y, wave.ndc.w, corner.y);
    return vec4<f32>(x, y, 0.0, 1.0);
}

// A level (a fraction of the loudest bar) on the terminal's stretch: 0 at
// the floor, 1 at the loudest column.
fn stretched(level: f32) -> f32 {
    return clamp((level - wave.shape.x) / wave.shape.y, 0.0, 1.0);
}

// The root mean square of the bars in a column-wide window centred on bar
// position t, each bar weighted by how much of it the window covers: the
// terminal's column wherever a column's window lands, sliding between.
// A fixed count of loads keeps the loop's flow uniform; a band so narrow
// that a column holds more than 64 bars averages the middle 64.
fn body(t: f32) -> f32 {
    let texels = f32(textureDimensions(peaks).x);
    let half = min(wave.shape.z, 64.0) * 0.5;
    let a = max(t - half, 0.0);
    let b = min(t + half, texels);
    var sum = 0.0;
    var weight = 0.0;
    for (var k = 0; k < 66; k = k + 1) {
        let left = floor(a) + f32(k);
        let covered = max(min(left + 1.0, b) - max(left, a), 0.0);
        let at = clamp(i32(left), 0, i32(texels) - 1);
        let bar = textureLoad(peaks, vec2<i32>(at, 0), 0).r;
        sum = sum + bar * bar * covered;
        weight = weight + covered;
    }
    return sqrt(sum / max(weight, 0.0001));
}

// The bars themselves at u: between two bars wider than a pixel, the line
// from one to the next; where several share a pixel, the loudest of them.
fn peak(u: f32) -> f32 {
    let texels = f32(textureDimensions(peaks).x);
    let per_pixel = texels / max(wave.rect.z - wave.rect.x, 1.0);
    var loudest = textureSampleLevel(peaks, peaks_sampler, vec2<f32>(u, 0.5), 0.0).r;
    let first = u * texels - per_pixel * 0.5;
    for (var k = 0; k < 8; k = k + 1) {
        let offset = (f32(k) + 0.5) * per_pixel / 8.0;
        let at = clamp(i32(floor(first + offset)), 0, i32(texels) - 1);
        if per_pixel > 1.0 {
            loudest = max(loudest, textureLoad(peaks, vec2<i32>(at, 0), 0).r);
        }
    }
    return loudest;
}

// Ink for a mirrored envelope reaching `reach` pixels each side of the
// centre: the signed distance to its edge (negative inside) over its
// gradient's length, so a steep flank is as soft as a flat top, then a
// pixel-wide smoothstep.
fn ink(y: f32, centre: f32, reach: f32) -> f32 {
    let d = abs(y - centre) - reach;
    let soft = max(length(vec2<f32>(dpdx(d), dpdy(d))), 0.001);
    return 1.0 - smoothstep(-0.5 * soft, 0.5 * soft, d);
}

@fragment
fn fs_main(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let width = max(wave.rect.z - wave.rect.x, 1.0);
    let u = clamp((at.x - wave.rect.x) / width, 0.0, 1.0);
    let texels = f32(textureDimensions(peaks).x);
    let centre = (wave.rect.y + wave.rect.w) * 0.5;
    let half = wave.params.z;
    // Never thinner than an eighth of the half: the glyph band's .max(1),
    // a quiet passage drawn as a hairline rather than a hole.
    let shape = max(stretched(body(u * texels)), 0.125);
    let detail = max(stretched(peak(u)), shape);
    let solid = ink(at.y, centre, shape * half);
    let faint = ink(at.y, centre, detail * half) * wave.shape.w;
    var alpha = max(solid, faint);
    // Played left of the playhead, unplayed right, the split's pixel shared.
    let split = wave.rect.x + wave.params.x * width;
    var colour = mix(wave.unplayed.rgb, wave.played.rgb, clamp(split - at.x + 0.5, 0.0, 1.0));
    if wave.params.y >= 0.0 {
        // The marker is the glyph band's caret, at the left edge of the
        // column a click seeks to; how much of this pixel it covers.
        let left = wave.rect.x + wave.params.y * width;
        let mark = clamp(min(at.x + 0.5, left + wave.params.w) - max(at.x - 0.5, left), 0.0, 1.0);
        colour = mix(colour, wave.marker.rgb, mark);
        alpha = max(alpha, mark);
    }
    return vec4<f32>(colour, alpha);
}
";
