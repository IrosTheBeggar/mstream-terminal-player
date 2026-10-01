pub(crate) mod builder;
pub(crate) mod wgpu_backend;

use std::num::NonZeroU32;

use wgpu::Adapter;
use wgpu::BindGroup;
use wgpu::Buffer;
use wgpu::BufferDescriptor;
use wgpu::BufferUsages;
use wgpu::CommandEncoder;
use wgpu::CommandEncoderDescriptor;
use wgpu::CurrentSurfaceTexture;
use wgpu::Device;
use wgpu::Extent3d;
use wgpu::Queue;
use wgpu::RenderPipeline;
use wgpu::Surface;
use wgpu::SurfaceColorSpace;
use wgpu::SurfaceConfiguration;
use wgpu::SurfaceTexture;
use wgpu::Texture;
use wgpu::TextureDescriptor;
use wgpu::TextureDimension;
use wgpu::TextureFormat;
use wgpu::TextureUsages;
use wgpu::TextureView;
use wgpu::TextureViewDescriptor;

/// A pipeline for post-processing rendered text.
pub trait PostProcessor {
    /// Custom user data which will be supplied during creation of the post
    /// processor. Use this to pass in any external state your processor
    /// requires.
    type UserData;

    /// Called during initialization of the backend. This should fully
    /// initialize the post processor for rendering. Note that you are expected
    /// to render to the final surface during [`PostProcessor::process`].
    fn compile(
        device: &Device,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
        user_data: Self::UserData,
    ) -> Self;

    /// Called after the drawing dimensions have changed (e.g. the surface was
    /// resized).
    fn resize(
        &mut self,
        device: &Device,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
    );

    /// Called after text has finished compositing. The provided `text_view` is
    /// the composited text. The final output of your implementation should
    /// render to the provided `surface_view`.
    ///
    /// <div class="warning">
    ///
    /// Retaining a reference to the provided surface view will cause a panic if
    /// the swapchain is recreated.
    ///
    /// </div>
    fn process(
        &mut self,
        encoder: &mut CommandEncoder,
        queue: &Queue,
        text_view: &TextureView,
        surface_config: &SurfaceConfiguration,
        surface_view: &TextureView,
    );

    /// Called to see if this post processor wants to update the screen. By
    /// default, the backend only runs the compositor and post processor when
    /// the text changes. Returning true from this will override that behavior
    /// and cause the processor to be invoked after a call to flush, even if no
    /// text changes occurred.
    fn needs_update(&self) -> bool {
        false
    }
}

/// The surface dimensions of the backend in pixels.
pub struct Dimensions {
    pub width: NonZeroU32,
    pub height: NonZeroU32,
}

impl From<(NonZeroU32, NonZeroU32)> for Dimensions {
    fn from((width, height): (NonZeroU32, NonZeroU32)) -> Self {
        Self { width, height }
    }
}

/// Controls the area the text is rendered to relative to the presentation
/// surface.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub enum Viewport {
    /// Render to the entire surface.
    #[default]
    Full,
    /// Render to a reduced area starting at the top right and rendering up to
    /// the bottom left - (width, height).
    Shrink { width: u32, height: u32 },
}

mod private {
    use wgpu::Surface;

    use crate::backend::HeadlessSurface;
    use crate::backend::HeadlessTarget;
    use crate::backend::RenderTarget;

    pub trait Sealed {}

    pub struct Token;

    impl Sealed for Surface<'_> {}
    impl Sealed for RenderTarget {}
    impl Sealed for HeadlessTarget {}
    impl Sealed for HeadlessSurface {}
}

/// A Texture target that can be rendered to.
pub trait RenderTexture: private::Sealed + Sized {
    /// Gets a [`wgpu::TextureView`] that can be used for rendering.
    fn get_view(
        &self,
        _token: private::Token,
    ) -> &TextureView;
    /// Presents the rendered result if applicable.
    fn present(
        self,
        _queue: &Queue,
        _token: private::Token,
    ) {
    }
}

impl RenderTexture for RenderTarget {
    fn get_view(
        &self,
        _token: private::Token,
    ) -> &TextureView {
        &self.view
    }

    fn present(
        self,
        queue: &Queue,
        _token: private::Token,
    ) {
        queue.present(self.texture);
    }
}

impl RenderTexture for HeadlessTarget {
    fn get_view(
        &self,
        _token: private::Token,
    ) -> &TextureView {
        &self.view
    }
}

/// A surface that can be rendered to.
pub trait RenderSurface<'s>: private::Sealed {
    type Target: RenderTexture;

    fn wgpu_surface(
        &self,
        _token: private::Token,
    ) -> Option<&Surface<'s>>;

    fn get_default_config(
        &self,
        adapter: &Adapter,
        width: u32,
        height: u32,
        _token: private::Token,
    ) -> Option<SurfaceConfiguration>;

    fn configure(
        &mut self,
        device: &Device,
        config: &SurfaceConfiguration,
        _token: private::Token,
    );

    fn get_current_texture(
        &self,
        _token: private::Token,
    ) -> Option<Self::Target>;
}

pub struct RenderTarget {
    texture: SurfaceTexture,
    view: TextureView,
}

impl<'s> RenderSurface<'s> for Surface<'s> {
    type Target = RenderTarget;

    fn wgpu_surface(
        &self,
        _token: private::Token,
    ) -> Option<&Surface<'s>> {
        Some(self)
    }

    fn get_default_config(
        &self,
        adapter: &Adapter,
        width: u32,
        height: u32,
        _token: private::Token,
    ) -> Option<SurfaceConfiguration> {
        self.get_default_config(adapter, width, height)
    }

    fn configure(
        &mut self,
        device: &Device,
        config: &SurfaceConfiguration,
        _token: private::Token,
    ) {
        Surface::configure(self, device, config);
    }

    fn get_current_texture(
        &self,
        _token: private::Token,
    ) -> Option<Self::Target> {
        let output = match self.get_current_texture() {
            CurrentSurfaceTexture::Success(output) | CurrentSurfaceTexture::Suboptimal(output) => {
                output
            }
            unavailable => {
                error!("Failed to acquire surface texture: {unavailable:?}");
                return None;
            }
        };

        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());

        Some(RenderTarget {
            texture: output,
            view,
        })
    }
}

/// The target a [`HeadlessSurface`] hands the post processor each frame.
pub struct HeadlessTarget {
    view: TextureView,
}

/// An offscreen surface: frames render into a texture that
/// [`WgpuBackend::read_pixels`](crate::WgpuBackend::read_pixels) reads back,
/// so a backend can be drawn and checked without a window. Build one with
/// [`Builder::build_headless`](crate::Builder::build_headless).
pub struct HeadlessSurface {
    pub(crate) texture: Option<Texture>,
    pub(crate) buffer: Option<Buffer>,
    /// Bytes per row of `buffer`: a row of pixels padded up to the
    /// alignment a texture-to-buffer copy demands.
    pub(crate) buffer_width: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: TextureFormat,
}

impl HeadlessSurface {
    /// A surface of the given format. It must be a four-byte colour format
    /// that can be rendered to and copied from: the `Rgba8` and `Bgra8`
    /// families, linear or sRGB.
    pub fn new(format: TextureFormat) -> Self {
        Self {
            format,
            ..Default::default()
        }
    }
}

impl Default for HeadlessSurface {
    fn default() -> Self {
        Self {
            texture: Default::default(),
            buffer: Default::default(),
            buffer_width: Default::default(),
            width: Default::default(),
            height: Default::default(),
            format: TextureFormat::Rgba8Unorm,
        }
    }
}

impl RenderSurface<'static> for HeadlessSurface {
    type Target = HeadlessTarget;

    fn wgpu_surface(
        &self,
        _token: private::Token,
    ) -> Option<&Surface<'static>> {
        None
    }

    fn get_default_config(
        &self,
        _adapter: &Adapter,
        width: u32,
        height: u32,
        _token: private::Token,
    ) -> Option<SurfaceConfiguration> {
        Some(SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            color_space: SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::Immediate,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        })
    }

    fn configure(
        &mut self,
        device: &Device,
        config: &SurfaceConfiguration,
        _token: private::Token,
    ) {
        self.texture = Some(device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: self.format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        }));

        // A copy into a buffer writes rows at a multiple of 256 bytes; upstream
        // used the bare row, which only worked for widths that are a multiple
        // of 64 pixels.
        self.buffer_width = (config.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        self.buffer = Some(device.create_buffer(&BufferDescriptor {
            label: None,
            size: (self.buffer_width * config.height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        self.width = config.width;
        self.height = config.height;
    }

    fn get_current_texture(
        &self,
        _token: private::Token,
    ) -> Option<Self::Target> {
        self.texture.as_ref().map(|t| HeadlessTarget {
            view: t.create_view(&TextureViewDescriptor::default()),
        })
    }
}

impl<P: PostProcessor> crate::WgpuBackend<'_, 'static, P, HeadlessSurface> {
    /// The last presented frame as RGBA bytes, four per pixel, row by row
    /// from the top left: the bytes the surface holds, so an sRGB surface
    /// reads back encoded values, as a screen would show them. `None` when
    /// nothing has been configured or the copy could not be mapped.
    pub fn read_pixels(&self) -> Option<Vec<u8>> {
        let surface = &self.surface;
        let texture = surface.texture.as_ref()?;
        let buffer = surface.buffer.as_ref()?;

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(surface.buffer_width),
                    rows_per_image: Some(surface.height),
                },
            },
            Extent3d {
                width: surface.width,
                height: surface.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (send, recv) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |mapped| {
            let _ = send.send(mapped);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok()?;
        recv.recv().ok()?.ok()?;

        let row_bytes = surface.width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * surface.height as usize);
        {
            let mapped = slice.get_mapped_range().ok()?;
            for row in mapped.chunks(surface.buffer_width as usize) {
                pixels.extend_from_slice(&row[..row_bytes]);
            }
        }
        buffer.unmap();

        if matches!(
            surface.format,
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb
        ) {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Some(pixels)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TextBgVertexMember {
    vertex: [f32; 2],
    bg_color: u32,
}

// Vertex + UVCoord + Color
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TextVertexMember {
    vertex: [f32; 2],
    uv: [f32; 2],
    fg_color: u32,
    underline_pos: u32,
    underline_color: u32,
    strikeout_pos: u32,
    strikeout_color: u32,
}

struct TextCacheBgPipeline {
    pipeline: RenderPipeline,
    fs_uniforms: BindGroup,
}

struct TextCacheFgPipeline {
    pipeline: RenderPipeline,
    fs_uniforms: BindGroup,
    atlas_bindings: BindGroup,
}

struct WgpuState {
    text_dest_view: TextureView,
}

fn build_wgpu_state(
    device: &Device,
    drawable_width: u32,
    drawable_height: u32,
) -> WgpuState {
    let text_dest = device.create_texture(&TextureDescriptor {
        label: Some("Text Compositor Out"),
        size: Extent3d {
            width: drawable_width.max(1),
            height: drawable_height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let text_dest_view = text_dest.create_view(&TextureViewDescriptor::default());

    WgpuState { text_dest_view }
}
