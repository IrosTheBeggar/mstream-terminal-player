//! Album art as textures: step 5 of the window-mode spike.
//!
//! A terminal gets its pictures through an escape-sequence protocol
//! (tui/graphics.rs); the window has no terminal to speak one to, so until
//! now every cover in it was the ▀-mosaic. Here the window draws them
//! itself, in two halves that meet at a [`Board`]:
//!
//! - the drawing path's half: the window installs `Graphics::hosted` in
//!   the App, so a cover the GUI draws — the bar's card, the wall, the
//!   queue rows, Now Playing — is recorded on the board (its cell rect and
//!   its art) and its cells are left blank. Everything that decides WHERE
//!   a picture goes, the mosaic under an overlay among it, is the drawing
//!   path the terminal walks, untouched.
//! - the GPU's half: [`CoverPost`], the backend's post-processor. It runs
//!   ratatui-wgpu's own text blit first, unchanged, then draws one
//!   textured quad per recorded cover over it in the same encoder.
//!
//! The quad's place is the cell rect as fractions of the grid, because the
//! default post-processor (without aspect preservation — the builder's
//! default, and the window's) samples the text texture at `pixel /
//! surface`: the grid is stretched over the whole surface, and a cell's
//! share of the grid is its share of the surface. Inside that box the
//! picture is fitted the way the terminal protocols fit it — whole, centred,
//! the slack left showing the blank ground round it — but to the pixel
//! rather than to the cell.
//!
//! Textures are cached by art id and bounded ([`KEEP`]). A cover arrives as
//! its 128 px thumbnail, which uploads in microseconds, and a box that
//! wants more pixels than that has the source decoded on a worker thread,
//! so the frame the keyboard waits on never decodes a jpeg; the sharper
//! texture replaces the thumbnail's when it lands.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};

use ratatui::layout::Rect;
use ratatui_wgpu::PostProcessor;
use ratatui_wgpu::shaders::DefaultPostProcessor;
use wgpu::{
    CommandEncoder, Device, Queue, RenderPipeline, SurfaceConfiguration, TextureFormat,
    TextureView,
};

use crate::tui::art::Art;
use crate::tui::graphics::PictureHost;

/// How many cover textures stay on the GPU. The wall's page and the queue
/// panel together are under twenty; this keeps a page turn back and forth
/// from uploading again, and bounds the memory to a few dozen covers at
/// most a few megabytes each.
const KEEP: usize = 48;

/// A box that wants this much more than the texture holds asks for the
/// source: a few percent is rounding, not blur.
const SHARPER: f32 = 1.05;

/// One cover this frame: which art, in which cells, on which grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placed {
    art: u64,
    rect: Rect,
    grid: (u16, u16),
}

/// Pixels on their way to a texture: RGBA rows, and the source bytes a
/// sharper texture could still be decoded from.
struct Pixels {
    art: u64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Empty when these ARE the source's pixels, or there is no source.
    source: Option<Arc<[u8]>>,
}

/// What the drawing path and the post-processor share. Both run on the
/// window's thread, one after the other within a frame; the decode worker
/// is the only other hand on it.
#[derive(Default)]
pub(super) struct Board {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// This frame's covers, in drawing order.
    placed: Vec<Placed>,
    /// The art the post-processor has a texture for, or is being handed
    /// one: the first sighting of anything else carries its pixels.
    known: HashSet<u64>,
    /// First sightings' thumbnails, for the next process to upload.
    arrivals: Vec<Pixels>,
    /// The worker's sharper decodes, likewise.
    decoded: Vec<Pixels>,
}

impl Board {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding this lock was a panic on the window's
        // thread, which ended the window; the data is still covers.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A frame is about to draw: last frame's covers are forgotten, so a
    /// cover that is no longer drawn is no longer painted.
    pub(super) fn begin_frame(&self) {
        self.lock().placed.clear();
    }
}

impl PictureHost for Board {
    fn place(&self, area: Rect, grid: Rect, art: &Art) {
        let mut inner = self.lock();
        if inner.known.insert(art.id()) {
            inner.arrivals.push(thumbnail(art));
        }
        inner.placed.push(Placed { art: art.id(), rect: area, grid: (grid.width, grid.height) });
    }
}

/// The art's thumbnail as RGBA, with its source for a sharper decode.
fn thumbnail(art: &Art) -> Pixels {
    let rgba = art.rgb().as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
    let source = (!art.source().is_empty()).then(|| Arc::from(art.source()));
    Pixels { art: art.id(), width: art.width(), height: art.height(), rgba, source }
}

/// One cover texture on the GPU.
struct Entry {
    bind: wgpu::BindGroup,
    size: (u32, u32),
    /// Where a sharper texture would come from; `None` once this is as
    /// sharp as the source gets.
    source: Option<Arc<[u8]>>,
    /// A sharper decode is on the worker.
    asked: bool,
    /// The process that last drew it, for eviction.
    used: u64,
}

/// What the decode worker is asked: this source, fitted to no more than
/// this many pixels.
struct Request {
    art: u64,
    source: Arc<[u8]>,
    want: (u32, u32),
}

/// ratatui-wgpu's text blit, then the covers over it.
pub(super) struct CoverPost {
    text: DefaultPostProcessor,
    board: Arc<Board>,
    /// `process` is handed a queue but no device, and textures and a
    /// growing vertex buffer need one: wgpu's handles are shared, so this
    /// is the backend's own device.
    device: Device,
    pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Unorm-sRGB on an sRGB surface, so the sample is linear and the
    /// surface's sRGB store writes the cover's own bytes back out; plain
    /// Unorm on a surface that stores what it is given.
    format: TextureFormat,
    textures: HashMap<u64, Entry>,
    vertices: wgpu::Buffer,
    /// The covers the vertex buffer has room for.
    room: usize,
    /// What the last process drew: the covers to compare the next frame's
    /// against, for `needs_update`.
    drawn: Vec<Placed>,
    processed: u64,
    uploads: u64,
    decodes: u64,
    worker: Option<Sender<Request>>,
}

/// Four floats a vertex: NDC position, then texture coordinate.
const VERTEX_BYTES: u64 = 16;
const VERTICES_PER_COVER: u64 = 6;

impl CoverPost {
    /// What the covers cost, for the stats lever.
    pub(super) fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "processed": self.processed,
            "textures": self.textures.len(),
            "uploads": self.uploads,
            "sharper_decodes_asked": self.decodes,
        })
    }

    fn vertex_buffer(device: &Device, room: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cover Vertices"),
            size: room.max(1) as u64 * VERTICES_PER_COVER * VERTEX_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// A texture for `pixels`, replacing any the art had.
    fn upload(&mut self, queue: &Queue, pixels: Pixels) {
        let size = wgpu::Extent3d {
            width: pixels.width,
            height: pixels.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cover"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
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
            &pixels.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * pixels.width),
                rows_per_image: Some(pixels.height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cover Bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let used = self.textures.get(&pixels.art).map_or(self.processed, |old| old.used);
        self.uploads += 1;
        self.textures.insert(
            pixels.art,
            Entry {
                bind,
                size: (pixels.width, pixels.height),
                source: pixels.source,
                asked: false,
                used,
            },
        );
    }

    /// Hand a source to the decode worker, starting it on first use. The
    /// worker lives as long as this post-processor's sender does.
    fn ask(&mut self, request: Request) {
        if self.worker.is_none() {
            let (tx, rx) = channel::<Request>();
            let board = self.board.clone();
            let spawned = std::thread::Builder::new().name("window-covers".into()).spawn(move || {
                while let Ok(request) = rx.recv() {
                    if let Some(pixels) = sharper(&request) {
                        board.lock().decoded.push(pixels);
                    }
                }
            });
            if spawned.is_err() {
                return;
            }
            self.worker = Some(tx);
        }
        if let Some(worker) = &self.worker {
            self.decodes += 1;
            let _ = worker.send(request);
        }
    }

    /// Drop the textures drawn longest ago past [`KEEP`], never one this
    /// frame drew, and tell the board so their next sighting brings pixels.
    fn evict(&mut self) {
        if self.textures.len() <= KEEP {
            return;
        }
        let mut ages: Vec<(u64, u64)> = self
            .textures
            .iter()
            .filter(|(_, entry)| entry.used < self.processed)
            .map(|(&art, entry)| (entry.used, art))
            .collect();
        ages.sort_unstable();
        let excess = self.textures.len() - KEEP;
        let mut inner = self.board.lock();
        for (_, art) in ages.into_iter().take(excess) {
            self.textures.remove(&art);
            inner.known.remove(&art);
        }
    }
}

/// The source decoded and, when it has more pixels than the box wants,
/// fitted down to the box — a texture sampled without mipmaps aliases when
/// it is shrunk much. Keeps the source only when it was shrunk, so a box
/// that grows later can still ask for more.
fn sharper(request: &Request) -> Option<Pixels> {
    let decoded = image::load_from_memory(&request.source).ok()?;
    let (w, h) = (decoded.width(), decoded.height());
    let (want_w, want_h) = (request.want.0.max(1), request.want.1.max(1));
    let (image, source) = if w > want_w || h > want_h {
        let fitted = decoded.resize(want_w, want_h, image::imageops::FilterType::Triangle);
        (fitted, Some(request.source.clone()))
    } else {
        (decoded, None)
    };
    let rgba = image.into_rgba8();
    Some(Pixels {
        art: request.art,
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        source,
    })
}

/// The cover's quad in surface pixels: the cell rect's share of the grid,
/// and the picture fitted whole inside it, centred, edges on whole pixels.
fn quad(placed: &Placed, surface: (f32, f32), picture: (u32, u32)) -> [f32; 4] {
    let (cols, rows) = (f32::from(placed.grid.0.max(1)), f32::from(placed.grid.1.max(1)));
    let r = placed.rect;
    let x0 = f32::from(r.x) / cols * surface.0;
    let x1 = f32::from(r.x + r.width) / cols * surface.0;
    let y0 = f32::from(r.y) / rows * surface.1;
    let y1 = f32::from(r.y + r.height) / rows * surface.1;
    let (bw, bh) = (x1 - x0, y1 - y0);
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let scale = (bw / pw).min(bh / ph);
    let (w, h) = (pw * scale, ph * scale);
    let left = (x0 + (bw - w) / 2.0).round();
    let top = (y0 + (bh - h) / 2.0).round();
    [left, top, (left + w).round().min(x1.round()), (top + h).round().min(y1.round())]
}

impl PostProcessor for CoverPost {
    type UserData = Arc<Board>;

    fn compile(
        device: &Device,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
        board: Arc<Board>,
    ) -> Self {
        let text = DefaultPostProcessor::compile(device, text_view, surface_config, ());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cover Bindings Layout"),
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
        // Linear both ways: a cover is a photograph, and nearest sampling
        // at a non-integer scale is the blockiness the picture is here to
        // be better than.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Cover Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cover Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cover Layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cover Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_BYTES,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let format = if surface_config.format.is_srgb() {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        };
        let room = 16;
        CoverPost {
            text,
            board,
            device: device.clone(),
            pipeline,
            layout,
            sampler,
            format,
            textures: HashMap::new(),
            vertices: CoverPost::vertex_buffer(device, room),
            room,
            drawn: Vec::new(),
            processed: 0,
            uploads: 0,
            decodes: 0,
            worker: None,
        }
    }

    fn resize(
        &mut self,
        device: &Device,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
    ) {
        // The covers need nothing: their places are fractions of the
        // grid, worked out again every process against the new surface.
        self.text.resize(device, text_view, surface_config);
    }

    fn process(
        &mut self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
        surface_view: &TextureView,
    ) {
        self.text.process(encoder, queue, text_view, surface_config, surface_view);
        self.processed += 1;

        let (placed, arrivals, decoded) = {
            let mut inner = self.board.lock();
            let arrivals = std::mem::take(&mut inner.arrivals);
            let decoded = std::mem::take(&mut inner.decoded);
            (inner.placed.clone(), arrivals, decoded)
        };
        for pixels in arrivals {
            self.upload(queue, pixels);
        }
        // A sharper decode for art evicted while it was on the worker
        // has nowhere to go; the next sighting starts over.
        for pixels in decoded {
            if self.textures.contains_key(&pixels.art) {
                self.upload(queue, pixels);
            }
        }

        let surface = (surface_config.width as f32, surface_config.height as f32);
        let mut floats: Vec<f32> = Vec::with_capacity(placed.len() * 24);
        let mut draws: Vec<u64> = Vec::with_capacity(placed.len());
        let mut asks = Vec::new();
        for cover in &placed {
            let Some(entry) = self.textures.get_mut(&cover.art) else { continue };
            entry.used = self.processed;
            let [left, top, right, bottom] = quad(cover, surface, entry.size);
            if right <= left || bottom <= top {
                continue;
            }
            let (w, h) = (right - left, bottom - top);
            if !entry.asked
                && let Some(source) = &entry.source
                && (w > entry.size.0 as f32 * SHARPER || h > entry.size.1 as f32 * SHARPER)
            {
                entry.asked = true;
                asks.push(Request {
                    art: cover.art,
                    source: source.clone(),
                    want: (w.ceil() as u32, h.ceil() as u32),
                });
            }
            let ndc = |x: f32, y: f32| [x / surface.0 * 2.0 - 1.0, 1.0 - y / surface.1 * 2.0];
            let (tl, tr) = (ndc(left, top), ndc(right, top));
            let (bl, br) = (ndc(left, bottom), ndc(right, bottom));
            for (corner, uv) in [
                (tl, [0.0, 0.0]),
                (tr, [1.0, 0.0]),
                (bl, [0.0, 1.0]),
                (bl, [0.0, 1.0]),
                (tr, [1.0, 0.0]),
                (br, [1.0, 1.0]),
            ] {
                floats.extend_from_slice(&[corner[0], corner[1], uv[0], uv[1]]);
            }
            draws.push(cover.art);
        }
        for request in asks {
            self.ask(request);
        }

        if !draws.is_empty() {
            if draws.len() > self.room {
                self.room = draws.len().next_power_of_two();
                self.vertices = CoverPost::vertex_buffer(&self.device, self.room);
            }
            let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
            queue.write_buffer(&self.vertices, 0, &bytes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cover Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    // Over the text the blit just drew, never instead of it.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            for (i, art) in draws.iter().enumerate() {
                let Some(entry) = self.textures.get(art) else { continue };
                let first = i as u32 * VERTICES_PER_COVER as u32;
                pass.set_bind_group(0, &entry.bind, &[]);
                pass.draw(first..first + VERTICES_PER_COVER as u32, 0..1);
            }
        }
        self.drawn = placed;
        self.evict();
    }

    /// The backend composites and presents only when a cell changed; a
    /// cover arriving on a still screen, one that moved without its cells
    /// changing, or a sharper texture landing must present too.
    fn needs_update(&self) -> bool {
        let inner = self.board.lock();
        inner.placed != self.drawn || !inner.decoded.is_empty()
    }
}

/// A quad per cover in NDC, sampled straight: the texture's format does
/// the colour space (see `CoverPost::format`), and a cover is opaque.
const SHADER: &str = r"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> Out {
    return Out(vec4<f32>(position, 0.0, 1.0), uv);
}

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(picture, picture_sampler, in.uv).rgb, 1.0);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(x: u16, y: u16, width: u16, height: u16) -> Placed {
        Placed { art: 1, rect: Rect { x, y, width, height }, grid: (100, 30) }
    }

    #[test]
    fn a_cell_rect_is_its_share_of_the_surface() {
        // 100×30 cells stretched over 2000×900 px: a cell is 20×30 px, and
        // a picture exactly the box's shape fills the box.
        let q = quad(&placed(10, 3, 6, 4), (2000.0, 900.0), (120, 120));
        assert_eq!(q, [200.0, 90.0, 320.0, 210.0]);
    }

    #[test]
    fn a_square_cover_in_a_wide_box_is_centred_not_stretched() {
        // 12×4 cells at 20×30 px is 240×120 px: a square picture is 120 px
        // a side, with 60 px of ground either side.
        let q = quad(&placed(0, 0, 12, 4), (2000.0, 900.0), (400, 400));
        assert_eq!(q, [60.0, 0.0, 180.0, 120.0]);
    }

    #[test]
    fn a_tall_box_letterboxes_above_and_below() {
        // 6×8 cells: 120×240 px; the square is 120 a side, 60 px each way.
        let q = quad(&placed(0, 0, 6, 8), (2000.0, 900.0), (128, 128));
        assert_eq!(q, [0.0, 60.0, 120.0, 180.0]);
    }

    #[test]
    fn the_board_carries_pixels_only_on_first_sight() {
        let board = Board::default();
        let art = Art::from_rgb(2, 1, vec![255, 0, 0, 0, 0, 255]).unwrap();
        let grid = Rect::new(0, 0, 100, 30);
        board.place(Rect::new(1, 1, 4, 2), grid, &art);
        board.place(Rect::new(9, 1, 4, 2), grid, &art);
        {
            let inner = board.lock();
            assert_eq!(inner.placed.len(), 2);
            assert_eq!(inner.arrivals.len(), 1, "one art, one upload");
            assert_eq!(inner.arrivals[0].rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
            assert!(inner.arrivals[0].source.is_none(), "an Art from pixels has no source");
        }
        board.begin_frame();
        assert!(board.lock().placed.is_empty(), "a frame starts with no covers");
    }
}
