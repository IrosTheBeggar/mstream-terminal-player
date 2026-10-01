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
//! Textures are cached by art and size, bounded by count ([`KEEP`]) and by
//! bytes ([`KEEP_BYTES`]). A cover arrives as its 128 px thumbnail, which
//! uploads in microseconds, and a box that wants more pixels than that has
//! the source decoded on a worker thread, so the frame the keyboard waits
//! on never decodes a jpeg; the sharper texture supersedes the thumbnail's
//! when it lands. A box less than half a texture's size has the source
//! resampled down to it on the same worker, because the textures have no
//! mipmaps and a texture sampled much smaller than itself skips texels and
//! shimmers as the picture moves. One art drawn at two sizes at once (the
//! wall's tile and the queue row's) keeps a texture for each.
//!
//! A hard-edged picture ([`Art::is_crisp`], the pairing QR code) is drawn
//! through a nearest-neighbour sampler and resampled by nearest neighbour,
//! so its modules stay squares; it arrives as its source's own pixels
//! rather than the thumbnail, a small PNG decoded on first sight.

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
/// from uploading again.
const KEEP: usize = 48;

/// And how many bytes of them, beside [`KEEP`]: a texture is as large as
/// the box it was decoded for, so on a large display a page of big tiles
/// is several megabytes a cover (a 1000 px square is 4 MB), and a count
/// alone would let 48 of those hold 190 MB of GPU memory. 64 MB is a full
/// wall at that size plus the queue's small ones; past it the textures
/// drawn longest ago go first. A frame's own covers are never evicted, so
/// one frame that needs more than this keeps it until they leave.
const KEEP_BYTES: u64 = 64 * 1024 * 1024;

/// A box that wants this much more than the texture holds asks for the
/// source: a few percent is rounding, not blur.
const SHARPER: f32 = 1.05;

/// A texture more than this many times its box on both sides is resampled
/// down to the box: past 2:1 a sample without mipmaps skips texels.
const SMALLER: f32 = 2.0;

/// One cover this frame: which art, in which cells, on which grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placed {
    art: u64,
    rect: Rect,
    grid: (u16, u16),
}

/// Pixels on their way to a texture: RGBA rows, whether they are as
/// large as the picture gets, and, on an art's first sighting, what its
/// other sizes would be decoded from.
struct Pixels {
    art: u64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// These are the source's own pixels, or there is no source: no
    /// texture of this art can be larger.
    native: bool,
    /// The bytes other sizes are decoded from; only a first sighting
    /// carries them, and only when the art kept its source.
    source: Option<Arc<[u8]>>,
    /// Hard-edged ([`Art::is_crisp`]): sampled and resampled by nearest
    /// neighbour.
    crisp: bool,
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
    /// The worker's resamples, likewise.
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

    /// Where this frame's covers are painted, in cells: what the script's
    /// dump says, so a run can tell a picture from the text under a modal.
    pub(super) fn placed_rects(&self) -> Vec<Rect> {
        self.lock().placed.iter().map(|placed| placed.rect).collect()
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

/// The art's first texture as RGBA, with its source for other sizes: the
/// thumbnail, or for a hard-edged picture the source's own pixels — the
/// thumbnail is an area-averaged shrink that would blur a QR code's
/// modules, and the code's PNG is a few kilobytes that decode in well
/// under a millisecond, so this is the one decode the drawing thread does.
fn thumbnail(art: &Art) -> Pixels {
    let source = (!art.source().is_empty()).then(|| Arc::from(art.source()));
    let crisp = art.is_crisp();
    if crisp && let Some(image) = source.as_deref().and_then(|s| image::load_from_memory(s).ok()) {
        let rgba = image.into_rgba8();
        let (width, height) = rgba.dimensions();
        let rgba = rgba.into_raw();
        return Pixels { art: art.id(), width, height, rgba, native: true, source, crisp };
    }
    let rgba = art.rgb().as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
    let native = source.is_none();
    Pixels { art: art.id(), width: art.width(), height: art.height(), rgba, native, source, crisp }
}

/// One texture of a cover on the GPU.
struct Texture {
    bind: wgpu::BindGroup,
    size: (u32, u32),
    /// The process that last drew it, for eviction.
    used: u64,
}

impl Texture {
    fn bytes(&self) -> u64 {
        u64::from(self.size.0) * u64::from(self.size.1) * 4
    }
}

/// One art's textures, a size each, and where more sizes come from.
struct Cover {
    /// The bytes other sizes are decoded from; `None` when the art kept
    /// none, which leaves it at its thumbnail.
    source: Option<Arc<[u8]>>,
    crisp: bool,
    /// The source's own size, once a texture of it has landed: no larger
    /// one exists to ask for.
    native: Option<(u32, u32)>,
    textures: Vec<Texture>,
    /// A resample of this art is on the worker: one at a time, so a box
    /// being dragged larger asks again only once the last one lands.
    asked: bool,
}

/// What the decode worker is asked: this source, fitted to no more than
/// this many pixels.
struct Request {
    art: u64,
    source: Arc<[u8]>,
    want: (u32, u32),
    crisp: bool,
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
    /// Nearest-neighbour, for hard-edged pictures.
    crisp_sampler: wgpu::Sampler,
    /// Unorm-sRGB on an sRGB surface, so the sample is linear and the
    /// surface's sRGB store writes the cover's own bytes back out; plain
    /// Unorm on a surface that stores what it is given.
    format: TextureFormat,
    covers: HashMap<u64, Cover>,
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
        let textures = || self.covers.values().flat_map(|cover| &cover.textures);
        serde_json::json!({
            "processed": self.processed,
            "covers": self.covers.len(),
            "textures": textures().count(),
            "texture_bytes": textures().map(Texture::bytes).sum::<u64>(),
            "uploads": self.uploads,
            "resamples_asked": self.decodes,
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

    /// A texture for `pixels` beside the art's others. It supersedes the
    /// art's smaller ones down to half its size: any box those served, it
    /// serves without a resample, so a box grown in steps keeps one
    /// texture, not one a step.
    fn upload(&mut self, queue: &Queue, pixels: Pixels) {
        let Some(cover) = self.covers.get(&pixels.art) else { return };
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
        let sampler = if cover.crisp { &self.crisp_sampler } else { &self.sampler };
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cover Bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        self.uploads += 1;
        let size = (pixels.width, pixels.height);
        let Some(cover) = self.covers.get_mut(&pixels.art) else { return };
        if pixels.native {
            cover.native = Some(size);
        }
        let superseded = |t: &Texture| {
            t.size.0 <= size.0
                && t.size.1 <= size.1
                && t.size.0.saturating_mul(2) >= size.0
                && t.size.1.saturating_mul(2) >= size.1
        };
        cover.textures.retain(|t| !superseded(t));
        cover.textures.push(Texture { bind, size, used: self.processed });
    }

    /// Hand a source to the decode worker, starting it on first use. The
    /// worker lives as long as this post-processor's sender does.
    fn ask(&mut self, request: Request) {
        if self.worker.is_none() {
            let (tx, rx) = channel::<Request>();
            let board = self.board.clone();
            let spawned = std::thread::Builder::new().name("window-covers".into()).spawn(move || {
                while let Ok(request) = rx.recv() {
                    // A source that will not decode still answers, so the
                    // art is not left waiting on the worker for good.
                    let pixels = resample(&request).unwrap_or(Pixels {
                        art: request.art,
                        width: 0,
                        height: 0,
                        rgba: Vec::new(),
                        native: true,
                        source: None,
                        crisp: request.crisp,
                    });
                    board.lock().decoded.push(pixels);
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

    /// Drop the textures drawn longest ago until [`KEEP`] and
    /// [`KEEP_BYTES`] both hold, never one this frame drew; an art left
    /// with none is forgotten, and the board told, so its next sighting
    /// brings pixels.
    fn evict(&mut self) {
        let mut count = 0;
        let mut bytes = 0;
        let mut ages: Vec<(u64, u64, (u32, u32), u64)> = Vec::new();
        for (&art, cover) in &self.covers {
            for texture in &cover.textures {
                count += 1;
                bytes += texture.bytes();
                if texture.used < self.processed {
                    ages.push((texture.used, art, texture.size, texture.bytes()));
                }
            }
        }
        if count <= KEEP && bytes <= KEEP_BYTES {
            return;
        }
        ages.sort_unstable();
        let mut inner = self.board.lock();
        for (_, art, size, weight) in ages {
            if count <= KEEP && bytes <= KEEP_BYTES {
                break;
            }
            let Some(cover) = self.covers.get_mut(&art) else { continue };
            cover.textures.retain(|t| t.size != size);
            count -= 1;
            bytes -= weight;
            if cover.textures.is_empty() {
                self.covers.remove(&art);
                inner.known.remove(&art);
            }
        }
    }
}

/// The source decoded and, when it has more pixels than the box wants,
/// fitted down to the box — a texture sampled without mipmaps aliases when
/// it is shrunk much. One path for both directions: a box larger than the
/// texture it had gets the source's pixels up to the box, a box much
/// smaller gets them fitted down to it. A photograph is filtered (a
/// triangle filter, widened with the ratio, so a large shrink averages
/// every source pixel); a hard-edged picture is sampled nearest, so its
/// modules stay solid.
fn resample(request: &Request) -> Option<Pixels> {
    let decoded = image::load_from_memory(&request.source).ok()?;
    let (w, h) = (decoded.width(), decoded.height());
    let (want_w, want_h) = (request.want.0.max(1), request.want.1.max(1));
    let native = w <= want_w && h <= want_h;
    let image = if native {
        decoded
    } else {
        let filter = if request.crisp {
            image::imageops::FilterType::Nearest
        } else {
            image::imageops::FilterType::Triangle
        };
        decoded.resize(want_w, want_h, filter)
    };
    let rgba = image.into_rgba8();
    Some(Pixels {
        art: request.art,
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        native,
        source: None,
        crisp: request.crisp,
    })
}

/// Which of a cover's textures (by their `sizes`) draws a box of `want`
/// pixels, and what size to ask the worker for, if anything. The smallest
/// texture that fills the box (within [`SHARPER`]) draws it; failing one,
/// the largest there is, and a larger one is asked for unless that is the
/// source's own size (`native`). A texture more than [`SMALLER`] times the
/// box on both sides asks for one the box's size. Nothing is asked unless
/// `can_ask` (the art has a source and no resample out).
fn choose(
    sizes: &[(u32, u32)],
    native: Option<(u32, u32)>,
    can_ask: bool,
    want: (f32, f32),
) -> Option<(usize, Option<(u32, u32)>)> {
    let fills = |(w, h): (u32, u32)| w as f32 * SHARPER >= want.0 && h as f32 * SHARPER >= want.1;
    let area = |i: &usize| u64::from(sizes[*i].0) * u64::from(sizes[*i].1);
    let filling = (0..sizes.len()).filter(|&i| fills(sizes[i])).min_by_key(area);
    let chosen = filling.or_else(|| (0..sizes.len()).max_by_key(area))?;
    let (w, h) = sizes[chosen];
    let box_size = (want.0.ceil().max(1.0) as u32, want.1.ceil().max(1.0) as u32);
    let ask = if !can_ask {
        None
    } else if filling.is_none() {
        (native != Some((w, h))).then_some(box_size)
    } else if w as f32 > want.0 * SMALLER && h as f32 > want.1 * SMALLER {
        Some(box_size)
    } else {
        None
    };
    Some((chosen, ask))
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
        // Nearest both ways for a hard-edged picture (a QR code): a module
        // is a square of one colour, and a blend at its edge is a grey
        // fringe a scanner has to see through. Its uneven steps at a
        // non-integer scale are a pixel at most.
        let crisp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Crisp Cover Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
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
            crisp_sampler,
            format,
            covers: HashMap::new(),
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
        for mut pixels in arrivals {
            let cover = Cover {
                source: pixels.source.take(),
                crisp: pixels.crisp,
                native: None,
                textures: Vec::new(),
                asked: false,
            };
            self.covers.insert(pixels.art, cover);
            self.upload(queue, pixels);
        }
        // A resample for art evicted while it was on the worker has nowhere
        // to go; the next sighting starts over. One that failed (no pixels)
        // drops the art's source: the textures it has stay, and a source
        // that would not decode once is not handed to the worker again.
        for pixels in decoded {
            let Some(cover) = self.covers.get_mut(&pixels.art) else { continue };
            cover.asked = false;
            if pixels.width > 0 && pixels.height > 0 {
                self.upload(queue, pixels);
            } else {
                cover.source = None;
            }
        }

        let surface = (surface_config.width as f32, surface_config.height as f32);
        let mut floats: Vec<f32> = Vec::with_capacity(placed.len() * 24);
        let mut draws: Vec<wgpu::BindGroup> = Vec::with_capacity(placed.len());
        let mut asks = Vec::new();
        for placed_cover in &placed {
            let Some(cover) = self.covers.get_mut(&placed_cover.art) else { continue };
            // The box itself, before the picture is fitted in it, picks the
            // texture: the fit depends on the picture's shape, which every
            // size of it shares.
            let Some(first) = cover.textures.first() else { continue };
            let [left, top, right, bottom] = quad(placed_cover, surface, first.size);
            if right <= left || bottom <= top {
                continue;
            }
            let (w, h) = (right - left, bottom - top);
            let sizes: Vec<(u32, u32)> = cover.textures.iter().map(|t| t.size).collect();
            let can_ask = !cover.asked && cover.source.is_some();
            let Some((chosen, ask)) = choose(&sizes, cover.native, can_ask, (w, h)) else {
                continue;
            };
            if let (Some(want), Some(source)) = (ask, &cover.source) {
                cover.asked = true;
                asks.push(Request {
                    art: placed_cover.art,
                    source: source.clone(),
                    want,
                    crisp: cover.crisp,
                });
            }
            let texture = &mut cover.textures[chosen];
            texture.used = self.processed;
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
            draws.push(texture.bind.clone());
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
            for (i, bind) in draws.iter().enumerate() {
                let first = i as u32 * VERTICES_PER_COVER as u32;
                pass.set_bind_group(0, bind, &[]);
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
    fn the_smallest_texture_that_fills_the_box_draws_it() {
        let sizes = [(128, 128), (300, 300), (600, 600)];
        assert_eq!(choose(&sizes, None, true, (250.0, 250.0)), Some((1, None)));
        assert_eq!(choose(&sizes, None, true, (120.0, 120.0)), Some((0, None)));
        // Within a few percent is a fill, not a blur.
        assert_eq!(choose(&sizes, None, true, (310.0, 310.0)), Some((1, None)));
    }

    #[test]
    fn a_box_larger_than_every_texture_asks_for_the_source_once() {
        let sizes = [(128, 128)];
        assert_eq!(choose(&sizes, None, true, (400.0, 400.0)), Some((0, Some((400, 400)))));
        assert_eq!(choose(&sizes, None, false, (400.0, 400.0)), Some((0, None)), "one out");
        // The source's own size is as large as it gets.
        assert_eq!(choose(&sizes, Some((128, 128)), true, (400.0, 400.0)), Some((0, None)));
    }

    #[test]
    fn a_box_under_half_a_texture_asks_for_its_own_size() {
        let sizes = [(600, 600)];
        assert_eq!(choose(&sizes, None, true, (250.5, 250.5)), Some((0, Some((251, 251)))));
        // Half or more draws from the texture as it is.
        assert_eq!(choose(&sizes, None, true, (300.0, 300.0)), Some((0, None)));
        // Once the small one is there, it draws, and nothing more is asked.
        let sizes = [(600, 600), (251, 251)];
        assert_eq!(choose(&sizes, None, true, (250.5, 250.5)), Some((1, None)));
    }

    #[test]
    fn a_shrink_is_filtered_and_a_crisp_shrink_keeps_its_squares() {
        // Two-pixel stripes, 8 px a side: a filtered shrink to 2 px blends
        // them to grey; a crisp one keeps black and white.
        let stripe = |x: u32, _| image::Luma([if x % 4 < 2 { 0 } else { 255 }]);
        let image = image::GrayImage::from_fn(8, 8, stripe);
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let source: Arc<[u8]> = Arc::from(png);
        let request = |crisp| Request { art: 1, source: source.clone(), want: (2, 2), crisp };
        let soft = resample(&request(false)).unwrap();
        let hard = resample(&request(true)).unwrap();
        assert_eq!((soft.width, soft.height, soft.native), (2, 2, false));
        assert!(hard.rgba.chunks(4).all(|px| px[0] == 0 || px[0] == 255), "{:?}", hard.rgba);
        assert!(soft.rgba.chunks(4).any(|px| px[0] != 0 && px[0] != 255), "{:?}", soft.rgba);
        // A box larger than the source gets the source as it is.
        let whole = resample(&Request { want: (20, 20), ..request(false) }).unwrap();
        assert_eq!((whole.width, whole.height, whole.native), (8, 8, true));
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
