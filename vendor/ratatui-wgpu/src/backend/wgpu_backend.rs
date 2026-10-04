use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::mem::size_of;
use std::num::NonZeroU64;

use bitvec::order::Lsb0;
use bitvec::slice::BitSlice;
use bitvec::vec::BitVec;
use indexmap::IndexMap;
use raqote::DrawOptions;
use raqote::DrawTarget;
use raqote::SolidSource;
use raqote::StrokeStyle;
use raqote::Transform;
use ratatui_core::backend::Backend;
use ratatui_core::backend::ClearType;
use ratatui_core::backend::WindowSize;
use ratatui_core::buffer::Cell;
use ratatui_core::layout::Position;
use ratatui_core::layout::Size;
use ratatui_core::style::Modifier;
use rustybuzz::shape_with_plan;
use rustybuzz::ttf_parser::GlyphId;
use rustybuzz::ttf_parser::RasterGlyphImage;
use rustybuzz::ttf_parser::RasterImageFormat;
use rustybuzz::ttf_parser::RgbaColor;
use rustybuzz::GlyphBuffer;
use rustybuzz::UnicodeBuffer;
use unicode_bidi::Level;
use unicode_bidi::ParagraphBidiInfo;
use unicode_properties::GeneralCategoryGroup;
use unicode_properties::UnicodeEmoji;
use unicode_properties::UnicodeGeneralCategory;
use unicode_width::UnicodeWidthStr;
use web_time::Duration;
use web_time::Instant;
use wgpu::util::BufferInitDescriptor;
use wgpu::util::DeviceExt;
use wgpu::Buffer;
use wgpu::BufferUsages;
use wgpu::CommandEncoderDescriptor;
use wgpu::Device;
use wgpu::Extent3d;
use wgpu::IndexFormat;
use wgpu::LoadOp;
use wgpu::Operations;
use wgpu::Origin3d;
use wgpu::Queue;
use wgpu::RenderPassColorAttachment;
use wgpu::RenderPassDescriptor;
use wgpu::StoreOp;
use wgpu::Surface;
use wgpu::SurfaceConfiguration;
use wgpu::Texture;
use wgpu::TextureAspect;

use crate::backend::build_wgpu_state;
use crate::backend::private::Token;
use crate::backend::PostProcessor;
use crate::backend::RenderSurface;
use crate::backend::RenderTexture;
use crate::backend::TextBgVertexMember;
use crate::backend::TextCacheBgPipeline;
use crate::backend::TextCacheFgPipeline;
use crate::backend::TextVertexMember;
use crate::backend::Viewport;
use crate::backend::WgpuState;
use crate::colors::ColorTable;
use crate::colors::Rgb;
use crate::fonts::FaceScale;
use crate::fonts::Font;
use crate::fonts::Fonts;
use crate::shaders::DefaultPostProcessor;
use crate::utils::plan_cache::PlanCache;
use crate::utils::text_atlas::Atlas;
use crate::utils::text_atlas::CacheRect;
use crate::utils::text_atlas::Entry;
use crate::utils::text_atlas::Key;
use crate::utils::Outline;
use crate::utils::Painter;
use crate::RandomState;

const NULL_CELL: Cell = Cell::new("");

pub(super) struct RenderInfo {
    cell: usize,
    cached: CacheRect,
    underline_pos_min: u16,
    underline_pos_max: u16,
    strikeout_pos_min: u16,
    strikeout_pos_max: u16,
}
/// Map from (x, y, glyph, cells wide) -> (cell index, cache entry).
/// We use an IndexMap because we want a consistent rendering order for
/// vertices. The width is in the key as it is in [`Sourced`]'s: one glyph
/// can stand at one place narrow and then wide (an emoji face's ❤ before
/// and after its VS16 is typed), and keyed without it the new placement and
/// the old were one entry, which the old one's removal took away — the
/// grown heart drew nothing and its second cell kept what it held. The key
/// holds no owner, and one place can be one cell's on a frame and another's
/// on the next (an unjoined flag's second letter is its flag's cell's), so a
/// removal takes only an entry its own cell put there (`flush`).
type Rendered = IndexMap<(i32, i32, GlyphId, u32), RenderInfo, RandomState>;

/// Set of (x, y, glyph, char width).
type Sourced = HashSet<(i32, i32, GlyphId, u32), RandomState>;

/// A ratatui backend leveraging wgpu for rendering.
///
/// Constructed using a [`Builder`](crate::Builder).
///
/// The first lifetime parameter is the lifetime of the data for referenced
/// [`Font`] objects. The second lifetime parameter is the lifetime of the
/// referenced [`Surface`] (typically the lifetime of your window object).
///
/// Limitations:
/// - The cursor is tracked but not rendered.
/// - No builtin accessibilty, although [`WgpuBackend::get_text`] is provided to
///   access the screen's contents.
pub struct WgpuBackend<
    'f,
    's,
    P: PostProcessor = DefaultPostProcessor,
    S: RenderSurface<'s> = Surface<'s>,
> {
    pub(super) post_process: P,

    pub(super) cells: Vec<Cell>,
    pub(super) dirty_rows: Vec<bool>,
    pub(super) dirty_cells: BitVec,
    pub(super) rendered: Vec<Rendered>,
    pub(super) sourced: Vec<Sourced>,
    pub(super) fast_blinking: BitVec,
    pub(super) slow_blinking: BitVec,

    pub(super) cursor: (u16, u16),

    pub(super) viewport: Viewport,

    pub(super) surface: S,
    pub(super) _surface: PhantomData<&'s S>,
    pub(super) surface_config: SurfaceConfiguration,
    /// A frame was composited but never presented, because the surface had
    /// no texture to give: the next flush presents even if nothing changed.
    pub(super) present_owed: bool,
    /// What each stage of the build took: the adapter, the device, the
    /// surface's configuration, and the textures, shaders and pipelines.
    pub(super) build_timings: Vec<(&'static str, Duration)>,
    /// The adapter the build drew with, given or requested.
    pub(super) adapter_info: wgpu::AdapterInfo,
    pub(super) device: Device,
    pub(super) queue: Queue,

    pub(super) plan_cache: PlanCache,
    pub(super) buffer: UnicodeBuffer,
    pub(super) row: String,
    pub(super) rowmap: Vec<u16>,

    pub(super) cached: Atlas,
    pub(super) text_cache: Texture,
    pub(super) text_mask: Texture,
    pub(super) bg_vertices: Vec<TextBgVertexMember>,
    pub(super) text_indices: Vec<[u32; 6]>,
    pub(super) text_vertices: Vec<TextVertexMember>,
    pub(super) text_bg_compositor: TextCacheBgPipeline,
    pub(super) text_fg_compositor: TextCacheFgPipeline,
    pub(super) text_screen_size_buffer: Buffer,

    pub(super) wgpu_state: WgpuState,

    pub(super) fonts: Fonts<'f>,
    pub(super) colors: ColorTable,
    pub(super) reset_fg: Rgb,
    pub(super) reset_bg: Rgb,

    pub(super) fast_duration: Duration,
    pub(super) last_fast_toggle: Instant,
    pub(super) show_fast: bool,
    pub(super) slow_duration: Duration,
    pub(super) last_slow_toggle: Instant,
    pub(super) show_slow: bool,
}

impl<'f, 's, P: PostProcessor, S: RenderSurface<'s>> WgpuBackend<'f, 's, P, S> {
    /// Get the [`PostProcessor`] associated with this backend.
    pub fn post_processor(&self) -> &P {
        &self.post_process
    }

    /// Get a mutable reference to the [`PostProcessor`] associated with this
    /// backend.
    pub fn post_processor_mut(&mut self) -> &mut P {
        &mut self.post_process
    }

    /// The texture format the surface was configured with: the linear twin
    /// of wgpu's default where the surface offers one, so colours pass
    /// through as given, else an sRGB format, which the post processor gets
    /// in its `surface_config` and must decode for.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// Resize the rendering surface. This should be called e.g. to keep the
    /// backend in sync with your window size.
    pub fn resize(
        &mut self,
        width: u32,
        height: u32,
    ) {
        let limits = self.device.limits();
        let width = width.min(limits.max_texture_dimension_2d);
        let height = height.min(limits.max_texture_dimension_2d);

        if width == self.surface_config.width && height == self.surface_config.height
            || width == 0
            || height == 0
        {
            return;
        }

        let (inset_width, inset_height) = match self.viewport {
            Viewport::Full => (0, 0),
            Viewport::Shrink { width, height } => (width, height),
        };

        let dims = self.size().unwrap();
        let current_width = dims.width;
        let current_height = dims.height;

        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface
            .configure(&self.device, &self.surface_config, Token);

        let width = width - inset_width;
        let height = height - inset_height;

        let chars_wide = width / self.fonts.min_width_px();
        let chars_high = height / self.fonts.height_px();

        if chars_wide != current_width as u32 || chars_high != current_height as u32 {
            self.cells.clear();
            self.rendered.clear();
            self.sourced.clear();
            self.fast_blinking.clear();
            self.slow_blinking.clear();
        }

        // This always needs to be cleared because the surface is cleared when it is
        // resized. If we don't re-render the rows, we end up with a blank surface when
        // the resize is less than a character dimension.
        self.dirty_rows.clear();

        self.wgpu_state = build_wgpu_state(
            &self.device,
            chars_wide * self.fonts.min_width_px(),
            chars_high * self.fonts.height_px(),
        );

        self.post_process.resize(
            &self.device,
            &self.wgpu_state.text_dest_view,
            &self.surface_config,
        );

        info!(
            "Resized from {}x{} to {}x{}",
            current_width, current_height, chars_wide, chars_high,
        );
    }

    /// Get the text currently displayed on the screen.
    pub fn get_text(&self) -> String {
        let bounds = self.size().unwrap();
        self.cells.chunks(bounds.width as usize).fold(
            String::with_capacity((bounds.width + 1) as usize * bounds.height as usize),
            |dest, row| {
                let mut dest = row.iter().fold(dest, |mut dest, s| {
                    dest.push_str(s.symbol());
                    dest
                });
                dest.push('\n');
                dest
            },
        )
    }

    /// Update the color-table used for rendering. This will cause a full
    /// repaint of the screen the next time [`WgpuBackend::flush`] is
    /// called.
    pub fn update_color_table(
        &mut self,
        new_colors: ColorTable,
    ) {
        self.dirty_rows.clear();
        self.colors = new_colors;
    }

    /// What each stage of the build took, in order: `adapter`, `device`,
    /// `configure` (the surface) and `pipelines` (the textures, the shaders
    /// and the pipelines, the post processor's included).
    pub fn build_timings(&self) -> &[(&'static str, Duration)] {
        &self.build_timings
    }

    /// The adapter the backend draws with: the one handed to
    /// [`Builder::with_device`](crate::Builder::with_device) when the build
    /// took it, else the one the build requested for itself.
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// A present is owed: the surface gave no texture the last time one was
    /// asked for (an occluded window), or [`WgpuBackend::owe_present`] was
    /// called, and the next flush presents whether anything changed or not.
    pub fn owes_present(&self) -> bool {
        self.present_owed
    }

    /// Owe a present: the next flush composites what the backend already
    /// holds onto a fresh surface texture and presents it, though no cell
    /// changed. For a window coming into view, whose last present may have
    /// gone to a surface the compositor never showed: the text pass's
    /// target holds every cell as last drawn, so this puts the whole screen
    /// up again for the cost of the post processor's pass, where a repaint
    /// of every cell would redraw them all first.
    pub fn owe_present(&mut self) {
        self.present_owed = true;
    }

    /// Update the fonts used for rendering. This will cause a full repaint of
    /// the screen the next time [`WgpuBackend::flush`] is called.
    pub fn update_fonts(
        &mut self,
        new_fonts: Fonts<'f>,
    ) {
        self.dirty_rows.clear();
        self.cached.match_fonts(&new_fonts);
        self.fonts = new_fonts;
    }

    fn render(&mut self) {
        let bounds = self.window_size().unwrap();

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Draw Encoder"),
            });

        if !self.text_vertices.is_empty() {
            {
                let mut uniforms = self
                    .queue
                    .write_buffer_with(
                        &self.text_screen_size_buffer,
                        0,
                        NonZeroU64::new(size_of::<[f32; 4]>() as u64).unwrap(),
                    )
                    .unwrap();
                uniforms.copy_from_slice(bytemuck::cast_slice(&[
                    bounds.columns_rows.width as f32 * self.fonts.min_width_px() as f32,
                    bounds.columns_rows.height as f32 * self.fonts.height_px() as f32,
                    0.0,
                    0.0,
                ]));
            }

            let bg_vertices = self.device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Text Bg Vertices"),
                contents: bytemuck::cast_slice(&self.bg_vertices),
                usage: BufferUsages::VERTEX,
            });

            let fg_vertices = self.device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Text Vertices"),
                contents: bytemuck::cast_slice(&self.text_vertices),
                usage: BufferUsages::VERTEX,
            });

            let indices = self.device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Text Indices"),
                contents: bytemuck::cast_slice(&self.text_indices),
                usage: BufferUsages::INDEX,
            });

            {
                let mut text_render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Text Render Pass"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &self.wgpu_state.text_dest_view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Load,
                            store: StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });

                text_render_pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);

                text_render_pass.set_pipeline(&self.text_bg_compositor.pipeline);
                text_render_pass.set_bind_group(0, &self.text_bg_compositor.fs_uniforms, &[]);
                text_render_pass.set_vertex_buffer(0, bg_vertices.slice(..));
                text_render_pass.draw_indexed(0..(self.bg_vertices.len() as u32 / 4) * 6, 0, 0..1);

                text_render_pass.set_pipeline(&self.text_fg_compositor.pipeline);
                text_render_pass.set_bind_group(0, &self.text_fg_compositor.fs_uniforms, &[]);
                text_render_pass.set_bind_group(1, &self.text_fg_compositor.atlas_bindings, &[]);

                text_render_pass.set_vertex_buffer(0, fg_vertices.slice(..));
                text_render_pass.draw_indexed(
                    0..(self.text_vertices.len() as u32 / 4) * 6,
                    0,
                    0..1,
                );
            }
        }

        let texture = match self.surface.get_current_texture(Token) {
            Ok(texture) => texture,
            Err(reason) => {
                // Said once per outage: the first failure at warn, the
                // retries every flush makes while the present is owed (an
                // occluded window: ten a second at a 100 ms poll) at debug,
                // until a present succeeds. Upstream logged every one at
                // error.
                if self.present_owed {
                    debug!("Still no surface texture to present into: {reason}");
                } else {
                    warn!(
                        "Failed to acquire surface texture: {reason}; the present is owed \
                         until the surface gives one"
                    );
                }
                // Submit the text pass anyway, so the composite holds this
                // frame's cells (their rows are no longer dirty and will not
                // be drawn again), and owe the present: upstream dropped
                // both, and a window whose content then stood still kept the
                // stale frame.
                self.queue.submit(Some(encoder.finish()));
                self.present_owed = true;
                return;
            }
        };

        self.post_process.process(
            &mut encoder,
            &self.queue,
            &self.wgpu_state.text_dest_view,
            &self.surface_config,
            texture.get_view(Token),
        );

        self.queue.submit(Some(encoder.finish()));
        texture.present(&self.queue, Token);
        if self.present_owed {
            debug!("The surface gave a texture again; the owed present is made");
        }
        self.present_owed = false;
    }
}

impl<'s, P: PostProcessor, S: RenderSurface<'s>> Backend for WgpuBackend<'_, 's, P, S> {
    type Error = std::io::Error;

    fn draw<'a, I>(
        &mut self,
        content: I,
    ) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let bounds = self.size()?;

        self.cells
            .resize(bounds.height as usize * bounds.width as usize, Cell::EMPTY);
        self.sourced.resize_with(
            bounds.height as usize * bounds.width as usize,
            Sourced::default,
        );
        self.rendered.resize_with(
            bounds.height as usize * bounds.width as usize,
            Rendered::default,
        );
        self.fast_blinking
            .resize(bounds.height as usize * bounds.width as usize, false);
        self.slow_blinking
            .resize(bounds.height as usize * bounds.width as usize, false);
        self.dirty_rows.resize(bounds.height as usize, true);

        for (x, y, cell) in content {
            let index = y as usize * bounds.width as usize + x as usize;

            // A blank for a cell a wide glyph still covers is ratatui's
            // clear of a VS16 emoji's second half (its diff sends one when
            // that cell's symbol changed, for terminals that leave the half
            // behind), not a cell of its own: the glyph covers it. Written,
            // it took the continuation's place and shaped as a cell, so
            // everything after the emoji on the row drew one cell right
            // (`❤▏` typed into `❤️▏` moved a field's border a cell out). A
            // continuation is only ever left covered: a narrower cell over
            // its glyph turns the continuations it uncovers into blanks
            // first, below.
            if self.cells[index] == NULL_CELL && cell.symbol() == " " {
                continue;
            }

            self.fast_blinking
                .set(index, cell.modifier.contains(Modifier::RAPID_BLINK));
            self.slow_blinking
                .set(index, cell.modifier.contains(Modifier::SLOW_BLINK));

            // A narrower cell over a wide one erases the whole of it, as a
            // terminal does when a character lands on a wide glyph's first
            // half: the continuation cells the new cell does not cover
            // become blanks. ratatui's diff counts on that (a blank there
            // in both frames is not sent), and upstream left them as the
            // empty continuation, which shapes to nothing, so every glyph
            // after them on the row drew a cell to the left of its own:
            // `│日本│` redrawn as `│ab  │` put the last `│` in cell 4.
            //
            // Every glyph the new cell lands on is erased so, not only the
            // one in its own cell: a wide cell's second half can land on
            // the first half of a wide glyph that began there, whose own
            // continuation lies past the new cell's reach (`a日x` redrawn
            // as `日 x`, the new 日's second cell the old one's first). Left
            // as the empty continuation, that cell turned away the blank
            // ratatui does send for it, by the rule above, and the row
            // lost a cell: the bar's `Heart ❤️ Song`, after a title with an
            // emoji one cell further on, drew as `Heart ❤️Song`.
            let width = cell.symbol().width().max(1);
            let end = (index + width).min(self.cells.len());
            let reach = (index..end)
                .filter(|&at| self.cells[at] != NULL_CELL)
                .map(|at| at + self.cells[at].symbol().width().max(1))
                .max()
                .unwrap_or(end)
                .clamp(end, self.cells.len());
            for covered in &mut self.cells[end..reach] {
                if *covered == NULL_CELL {
                    *covered = Cell::EMPTY;
                }
            }

            self.cells[index] = cell.clone();

            let start = (index + 1).min(self.cells.len());
            self.cells[start..end].fill(NULL_CELL);
            self.dirty_rows[y as usize] = true;
        }

        Ok(())
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        Ok(Position::new(self.cursor.0, self.cursor.1))
    }

    fn set_cursor_position<Pos: Into<Position>>(
        &mut self,
        position: Pos,
    ) -> std::io::Result<()> {
        let bounds = self.size()?;
        let pos: Position = position.into();
        self.cursor = (pos.x.min(bounds.width - 1), pos.y.min(bounds.height - 1));
        Ok(())
    }

    fn clear(&mut self) -> std::io::Result<()> {
        self.cells.clear();
        self.dirty_rows.clear();
        self.cursor = (0, 0);

        Ok(())
    }

    fn size(&self) -> std::io::Result<Size> {
        let (inset_width, inset_height) = match self.viewport {
            Viewport::Full => (0, 0),
            Viewport::Shrink { width, height } => (width, height),
        };
        let width = self.surface_config.width - inset_width;
        let height = self.surface_config.height - inset_height;

        Ok(Size {
            width: (width / self.fonts.min_width_px()) as u16,
            height: (height / self.fonts.height_px()) as u16,
        })
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        let (inset_width, inset_height) = match self.viewport {
            Viewport::Full => (0, 0),
            Viewport::Shrink { width, height } => (width, height),
        };
        let width = self.surface_config.width - inset_width;
        let height = self.surface_config.height - inset_height;

        Ok(WindowSize {
            columns_rows: Size {
                width: (width / self.fonts.min_width_px()) as u16,
                height: (height / self.fonts.height_px()) as u16,
            },
            pixels: Size {
                width: width as u16,
                height: height as u16,
            },
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let bounds = self.size()?;
        self.dirty_cells.clear();
        self.dirty_cells.resize(self.cells.len(), false);

        let fast_toggle_dirty = self.last_fast_toggle.elapsed() >= self.fast_duration;
        if fast_toggle_dirty {
            self.last_fast_toggle = Instant::now();
            self.show_fast = !self.show_fast;

            for index in self.fast_blinking.iter_ones() {
                self.dirty_cells.set(index, true);
            }
        }

        let slow_toggle_dirty = self.last_slow_toggle.elapsed() >= self.slow_duration;
        if slow_toggle_dirty {
            self.last_slow_toggle = Instant::now();
            self.show_slow = !self.show_slow;

            for index in self.slow_blinking.iter_ones() {
                self.dirty_cells.set(index, true);
            }
        }

        let mut pending_cache_updates = HashMap::<_, _, RandomState>::default();

        for (y, (row, sourced)) in self
            .cells
            .chunks(bounds.width as usize)
            .zip(self.sourced.chunks_mut(bounds.width as usize))
            .enumerate()
        {
            if !self.dirty_rows[y] {
                continue;
            }

            self.dirty_rows[y] = false;
            let mut new_sourced = vec![Sourced::default(); bounds.width as usize];

            // This block concatenates the strings for the row into one string for bidi
            // resolution, then maps bytes for the string to their associated cell index. It
            // also maps the row's cell index to the font that can source all glyphs for
            // that cell.
            self.row.clear();
            self.rowmap.clear();
            let mut fontmap = Vec::with_capacity(self.rowmap.capacity());
            for (idx, cell) in row.iter().enumerate() {
                self.row.push_str(cell.symbol());
                self.rowmap
                    .resize(self.rowmap.len() + cell.symbol().len(), idx as u16);
                fontmap.push(self.fonts.font_for_cell(cell));
            }

            let mut x = 0;
            // rustbuzz provides a non-zero x-advance for the first character in a cluster
            // with combining characters. The remainder of the cluster doesn't account for
            // this advance, so if we advance prior to rendering them, we end up with all of
            // the associated characters being offset by a cell. To combat this, we only
            // bump the x-advance after we've finished processing all of the characters in a
            // cell. This assumes that we 1) always get a non-zero advance at the beginning
            // of a cluster and 2) the next cluster in the sequence starts with a non-zero
            // advance.
            let mut next_advance = 0;
            // Which cells have had a glyph that advances placed in them. A cell is one
            // grapheme, so it draws as one picture over the cells its width claims: an
            // emoji sequence the face has a ligature for (a flag, a ZWJ family, a skin
            // tone) shapes to one glyph, but one it has none for shapes to several that
            // each advance, and upstream drew the second in the next cell and so on,
            // over whatever that cell held. Only the first is drawn now: the sequence
            // degrades to its base emoji (👨 for an unknown family, 👍 for a tone the
            // face lacks), in its own cells. What follows a glyph left out is left out
            // too: a glyph that does not advance after one is placed against it, and drawn
            // at the first glyph's place it lands wrong (a family Windows 10's Segoe UI Emoji
            // composes, below, drew its girl at the man's place, reaching into the cell
            // before, while that composition was cut to its man).
            //
            // A flag the face has no picture for is one exception: its two regional
            // indicators shape to two letters, and the first alone, a narrow letter in the
            // flag's two cells, said nothing (no face the window has on Windows 10 has country
            // flags: Segoe UI Symbol, ahead of Segoe UI Emoji, draws the pair as its letters,
            // and 🇯🇵 was a sliver of a `J`). Each letter is drawn in a cell of its own instead,
            // `J` then `P`, as Windows Terminal draws it. A face with the flag shapes the pair
            // to one glyph, which covers both cells as before.
            //
            // A sequence a colour face composes is the other: Windows 10's Segoe UI Emoji has
            // no picture for a family, and builds 👨‍👩‍👧 from its people by its positioning, the
            // man and the woman advancing and the girl placed back in front of them. All of
            // them are drawn, into the cell's one box, fitted to the run's advance as one
            // glyph's would be (`composed_runs`), as Windows Terminal draws it; the first
            // advancing glyph alone was a man where the family was meant.
            let mut placed = vec![0u8; bounds.width as usize];
            let mut shape = |font: &Font,
                             fake_bold,
                             fake_italic,
                             buffer: GlyphBuffer|
             -> UnicodeBuffer {
                let metrics = font.font();
                let face_scale = self.fonts.face_scale(metrics);
                let advance_scale = face_scale.scale;

                // How many glyphs that advance each cell shaped to, for the flags above.
                // `.notdef` is not counted: a pair the face has no letters for (the last
                // resort's boxes, on a system with no emoji face) keeps change 16's one glyph
                // over both cells, as every other emoji there does, not a box to a cell.
                let mut advancing = vec![0u8; bounds.width as usize];
                for (info, position) in buffer
                    .glyph_infos()
                    .iter()
                    .zip(buffer.glyph_positions().iter())
                {
                    let advances = (position.x_advance as f32 * advance_scale) as i32 != 0;
                    if info.glyph_id != 0 && advances {
                        let cell_idx = self.rowmap[info.cluster as usize] as usize;
                        advancing[cell_idx] = advancing[cell_idx].saturating_add(1);
                    }
                }
                // The cells this face composes from several pictures (above). Only a COLR
                // face composes: an sbix or CBDT face joins a sequence it knows into one glyph.
                let composed = if metrics.tables().colr.is_some() {
                    composed_runs(
                        metrics,
                        buffer.glyph_infos(),
                        buffer.glyph_positions(),
                        &self.rowmap,
                        row,
                    )
                } else {
                    Vec::new()
                };

                for (info, position) in buffer
                    .glyph_infos()
                    .iter()
                    .zip(buffer.glyph_positions().iter())
                {
                    let cell_idx = self.rowmap[info.cluster as usize] as usize;
                    let cell = &row[cell_idx];
                    let run = composed.get(cell_idx).and_then(Option::as_ref);
                    // An unjoined flag's letters stand one to a cell, each a cell wide.
                    let split = run.is_none()
                        && advancing[cell_idx] == 2
                        && regional_pair(cell.symbol());
                    let max_width = if split { 1 } else { cell.symbol().width() };
                    let sourced = &mut new_sourced[cell_idx];

                    let mut basey = y as i32 * self.fonts.height_px() as i32
                        + (position.y_offset as f32 * advance_scale) as i32;
                    let mut advance = (position.x_advance as f32 * advance_scale) as i32;
                    if run.is_some() {
                        // A composed cell is drawn once, as its first glyph, and advances as
                        // one cell; the run carries its pictures' own offsets.
                        if std::mem::replace(&mut placed[cell_idx], 1) != 0 {
                            continue;
                        }
                        basey = y as i32 * self.fonts.height_px() as i32;
                        advance = 1;
                    } else {
                        if advance != 0 {
                            placed[cell_idx] = placed[cell_idx].saturating_add(1);
                        }
                        // Past the glyphs the cell draws (one, or an unjoined flag's two), and
                        // whatever does not advance after them, which hangs on one left out.
                        if placed[cell_idx] > 1 + u8::from(split) {
                            continue;
                        }
                    }
                    if advance != 0 {
                        x += next_advance;
                        advance =
                            max_width as i32 * advance.signum() * self.fonts.min_width_px() as i32;
                        next_advance = advance;
                    }
                    let basex = if run.is_some() {
                        x
                    } else {
                        x + (position.x_offset as f32 * advance_scale) as i32
                    };

                    // This assumes that we only want to underline the first character in the
                    // cluster, and that the remaining characters are all combining characters
                    // which don't need an underline.
                    let set = if advance != 0 {
                        Modifier::BOLD
                            | Modifier::ITALIC
                            | Modifier::UNDERLINED
                            | Modifier::CROSSED_OUT
                    } else {
                        Modifier::BOLD | Modifier::ITALIC
                    };

                    let ch = self.row[info.cluster as usize..].chars().next().unwrap();
                    // A composed run is as wide as its pictures' advances together.
                    let width = match run {
                        Some(run) => run.advance as f32,
                        None => metrics
                            .glyph_hor_advance(GlyphId(info.glyph_id as _))
                            .unwrap_or_default() as f32,
                    };
                    let width = (width * advance_scale) as u32;
                    // The glyph's box is the cells its cell claims, which ratatui measured
                    // for the whole grapheme. Upstream measured the cluster's first
                    // character alone, which is narrower than the grapheme for an emoji
                    // with VS16 (❤ is one cell, ❤️ two) and a flag (each regional
                    // indicator is one cell, the pair two): the picture was squeezed into
                    // one cell of the two. For every other cell the two widths agree.
                    let chars_wide = (max_width as u32).max(1);

                    // The width is part of the key: one glyph can stand in a narrow cell
                    // and a wide one (an emoji face's ❤ with and without VS16), and its
                    // raster is drawn for its box.
                    let key = Key {
                        style: cell.modifier.intersection(set),
                        glyph: info.glyph_id,
                        font: font.id(),
                        cells: chars_wide,
                        run: run.map_or(0, |run| run.key),
                    };
                    let width = if width == 0 {
                        chars_wide * self.fonts.min_width_px()
                    } else {
                        width
                    };

                    let cached = self.cached.get(
                        &key,
                        chars_wide * self.fonts.min_width_px(),
                        self.fonts.height_px(),
                    );

                    let offset = (basey.max(0) as usize / self.fonts.height_px() as usize)
                        .min(bounds.height as usize - 1)
                        * bounds.width as usize
                        + (basex.max(0) as usize / self.fonts.min_width_px() as usize)
                            .min(bounds.width as usize - 1);

                    sourced.insert((basex, basey, GlyphId(info.glyph_id as _), chars_wide));

                    let mut underline_pos_min = 0;
                    let mut underline_pos_max = 0;
                    if key.style.contains(Modifier::UNDERLINED) {
                        let underline_position = face_scale.ascender
                            - metrics
                                .underline_metrics()
                                .map(|m| m.position as f32)
                                .unwrap_or(0.0);
                        let underline_position = (underline_position * advance_scale) as u16;

                        let underline_thickness = metrics
                            .underline_metrics()
                            .map(|m| m.thickness as f32)
                            .unwrap_or(100.0); // observed average
                                               // default underlines are a bit thin for larger font-sizes.
                        let underline_thickness =
                            (underline_thickness * 1.3 * advance_scale).max(1.0) as u16;

                        // might overflow the box
                        if underline_position + underline_thickness < cached.height as u16 {
                            underline_pos_min = underline_position;
                            underline_pos_max = underline_pos_min + underline_thickness;
                        } else {
                            underline_pos_min =
                                (cached.height as u16).saturating_sub(underline_thickness);
                            underline_pos_max = cached.height as u16;
                        }
                    }

                    let mut strikeout_pos_min = 0;
                    let mut strikeout_pos_max = 0;
                    if key.style.contains(Modifier::CROSSED_OUT) {
                        let strikeout_position = metrics
                            .strikeout_metrics()
                            .map(|m| m.position)
                            .unwrap_or_default();
                        let strikeout_position = if strikeout_position > 0 {
                            face_scale.ascender - strikeout_position as f32
                        } else {
                            face_scale.ascender * 0.7f32 // observed average
                        };
                        let strikeout_position = (strikeout_position * advance_scale) as u16;

                        let strikeout_thickness = metrics
                            .strikeout_metrics()
                            .map(|m| m.thickness as f32)
                            .unwrap_or(100.0); // observed average
                                               // default strikeout lines are a bit thin for larger font-sizes.
                        let strikeout_thickness =
                            (strikeout_thickness * 1.8 * advance_scale).max(1.0) as u16;

                        strikeout_pos_min = strikeout_position;
                        strikeout_pos_max = strikeout_pos_min + strikeout_thickness;
                    }

                    self.rendered[offset].insert(
                        (basex, basey, GlyphId(info.glyph_id as _), chars_wide),
                        RenderInfo {
                            cell: y * bounds.width as usize + cell_idx,
                            cached: *cached,
                            underline_pos_min,
                            underline_pos_max,
                            strikeout_pos_min,
                            strikeout_pos_max,
                        },
                    );
                    for x_offset in 0..chars_wide as usize {
                        self.dirty_cells.set(offset + x_offset, true);
                    }

                    if cached.cached() {
                        continue;
                    }

                    pending_cache_updates.entry(key).or_insert_with(|| {
                        let is_emoji = ch.is_emoji_char()
                            && !matches!(ch.general_category_group(), GeneralCategoryGroup::Number);

                        let (rect, image, colour) = rasterize_glyph(
                            cached,
                            metrics,
                            GlyphId(info.glyph_id as _),
                            run.map_or(&[][..], |run| &run.glyphs[..]),
                            fake_italic & !is_emoji,
                            fake_bold & !is_emoji,
                            face_scale,
                            width,
                        );
                        (rect, image, colour)
                    });
                }

                buffer.clear()
            };

            let bidi = ParagraphBidiInfo::new(&self.row, None);
            let (levels, runs) = bidi.visual_runs(0..bidi.levels.len());

            let (mut current_font, mut current_fake_bold, mut current_fake_italic) = fontmap[0];
            let mut current_level = Level::ltr();

            for (level, range) in runs.into_iter().map(|run| (levels[run.start], run)) {
                let chars = &self.row[range.clone()];
                let cells = &self.rowmap[range.clone()];
                for (idx, ch) in chars.char_indices() {
                    let cell_idx = cells[idx] as usize;
                    let (font, fake_bold, fake_italic) = fontmap[cell_idx];

                    if font.id() != current_font.id()
                        || current_fake_bold != fake_bold
                        || current_fake_italic != fake_italic
                        || current_level != level
                    {
                        let mut buffer = std::mem::take(&mut self.buffer);

                        self.buffer = shape(
                            current_font,
                            current_fake_bold,
                            current_fake_italic,
                            shape_with_plan(
                                current_font.font(),
                                self.plan_cache.get(current_font, &mut buffer),
                                buffer,
                            ),
                        );

                        current_font = font;
                        current_fake_bold = fake_bold;
                        current_fake_italic = fake_italic;
                        current_level = level;
                    }

                    self.buffer.add(ch, (range.start + idx) as u32);
                }
            }

            let mut buffer = std::mem::take(&mut self.buffer);
            self.buffer = shape(
                current_font,
                current_fake_bold,
                current_fake_italic,
                shape_with_plan(
                    current_font.font(),
                    self.plan_cache.get(current_font, &mut buffer),
                    buffer,
                ),
            );

            let cells = new_sourced.into_iter().zip(sourced.iter_mut());
            for (owner, (new, old)) in cells.enumerate() {
                let owner = y * bounds.width as usize + owner;
                if new != *old {
                    for (x, y, glyph, width) in old.difference(&new) {
                        let cell = ((*y).max(0) as usize / self.fonts.height_px() as usize)
                            .min(bounds.height as usize - 1)
                            * bounds.width as usize
                            + ((*x).max(0) as usize / self.fonts.min_width_px() as usize)
                                .min(bounds.width as usize - 1);

                        for offset_x in 0..*width as usize {
                            if cell >= self.dirty_cells.len() {
                                break;
                            }

                            self.dirty_cells.set(cell + offset_x, true);
                        }

                        // Only an entry this cell put there: another cell may have put the
                        // same glyph at the same place this frame, an unjoined flag's second
                        // letter handed from one flag to the next (`x🇵🇪` redrawn as `🇯🇵`),
                        // and taking that away left the letter as the frame before drew it.
                        let key = (*x, *y, *glyph, *width);
                        if self.rendered[cell].get(&key).is_some_and(|info| info.cell == owner) {
                            self.rendered[cell].shift_remove(&key);
                        }
                    }
                    *old = new;
                }
            }
        }

        for (_, (cached, image, mask)) in pending_cache_updates {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.text_cache,
                    mip_level: 0,
                    origin: Origin3d {
                        x: cached.x,
                        y: cached.y,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                bytemuck::cast_slice(&image),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(cached.width * size_of::<u32>() as u32),
                    rows_per_image: Some(cached.height),
                },
                Extent3d {
                    width: cached.width,
                    height: cached.height,
                    depth_or_array_layers: 1,
                },
            );

            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.text_mask,
                    mip_level: 0,
                    origin: Origin3d {
                        x: cached.x,
                        y: cached.y,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &vec![if mask { 255 } else { 0 }; (cached.width * cached.height) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(cached.width),
                    rows_per_image: Some(cached.height),
                },
                Extent3d {
                    width: cached.width,
                    height: cached.height,
                    depth_or_array_layers: 1,
                },
            )
        }

        if self.post_process.needs_update() || self.dirty_cells.any() || self.present_owed {
            self.bg_vertices.clear();
            self.text_vertices.clear();
            self.text_indices.clear();

            let mut index_offset = 0;
            for index in self.dirty_cells.iter_ones() {
                let cell = &self.cells[index];
                let to_render = &self.rendered[index];

                let reverse = cell.modifier.contains(Modifier::REVERSED);
                let bg_color = if reverse {
                    self.colors.c2c(cell.fg, self.reset_fg)
                } else {
                    self.colors.c2c(cell.bg, self.reset_bg)
                };

                let [r, g, b] = bg_color;
                let bg_color_u32: u32 = u32::from_be_bytes([r, g, b, 255]);
                // A glyph placed in a wide cell's continuation (an unjoined flag's second
                // letter, `flush`'s shaping) is its flag's: the continuation's own background
                // is the default one, which would show as a gap in a highlighted row.
                let continuation = cell.symbol().is_empty();

                for (
                    (x, y, _, _),
                    RenderInfo {
                        cell,
                        cached,
                        underline_pos_min,
                        underline_pos_max,
                        strikeout_pos_min,
                        strikeout_pos_max,
                    },
                ) in to_render.iter()
                {
                    let cell = &self.cells[*cell];
                    let reverse = cell.modifier.contains(Modifier::REVERSED);
                    let fg_color = if reverse {
                        self.colors.c2c(cell.bg, self.reset_bg)
                    } else {
                        self.colors.c2c(cell.fg, self.reset_fg)
                    };
                    let bg_color_u32 = if continuation {
                        let [r, g, b] = if reverse {
                            self.colors.c2c(cell.fg, self.reset_fg)
                        } else {
                            self.colors.c2c(cell.bg, self.reset_bg)
                        };
                        u32::from_be_bytes([r, g, b, 255])
                    } else {
                        bg_color_u32
                    };

                    let alpha = if cell.modifier.contains(Modifier::HIDDEN)
                        | (cell.modifier.contains(Modifier::RAPID_BLINK) & !self.show_fast)
                        | (cell.modifier.contains(Modifier::SLOW_BLINK) & !self.show_slow)
                    {
                        0
                    } else if cell.modifier.contains(Modifier::DIM) {
                        127
                    } else {
                        255
                    };

                    let underline_color = fg_color;
                    let [r, g, b] = fg_color;
                    let fg_color: u32 = u32::from_be_bytes([r, g, b, alpha]);

                    let [r, g, b] = underline_color;
                    let underline_color = u32::from_be_bytes([r, g, b, alpha]);
                    let strikeout_color = u32::from_be_bytes([r, g, b, alpha]);

                    for offset_x in (0..cached.width).step_by(self.fonts.min_width_px() as usize) {
                        self.text_indices.push([
                            index_offset,     // x, y
                            index_offset + 1, // x + w, y
                            index_offset + 2, // x, y + h
                            index_offset + 2, // x, y + h
                            index_offset + 3, // x + w, y + h
                            index_offset + 1, // x + w, y
                        ]);
                        index_offset += 4;

                        let x = *x as f32 + offset_x as f32;
                        let y = *y as f32;
                        let uvx = cached.x + offset_x;
                        let uvy = cached.y;

                        self.bg_vertices.push(TextBgVertexMember {
                            vertex: [x, y],
                            bg_color: bg_color_u32,
                        });
                        self.bg_vertices.push(TextBgVertexMember {
                            vertex: [x + self.fonts.min_width_px() as f32, y],
                            bg_color: bg_color_u32,
                        });
                        self.bg_vertices.push(TextBgVertexMember {
                            vertex: [x, y + self.fonts.height_px() as f32],
                            bg_color: bg_color_u32,
                        });
                        self.bg_vertices.push(TextBgVertexMember {
                            vertex: [
                                x + self.fonts.min_width_px() as f32,
                                y + self.fonts.height_px() as f32,
                            ],
                            bg_color: bg_color_u32,
                        });

                        let underline_pos = ((*underline_pos_min as u32 + uvy) << 16)
                            | (*underline_pos_max as u32 + uvy);
                        let strikeout_pos = ((*strikeout_pos_min as u32 + uvy) << 16)
                            | (*strikeout_pos_max as u32 + uvy);

                        self.text_vertices.push(TextVertexMember {
                            vertex: [x, y],
                            uv: [uvx as f32, uvy as f32],
                            fg_color,
                            underline_pos,
                            underline_color,
                            strikeout_pos,
                            strikeout_color,
                        });
                        self.text_vertices.push(TextVertexMember {
                            vertex: [x + self.fonts.min_width_px() as f32, y],
                            uv: [uvx as f32 + self.fonts.min_width_px() as f32, uvy as f32],
                            fg_color,
                            underline_pos,
                            underline_color,
                            strikeout_pos,
                            strikeout_color,
                        });
                        self.text_vertices.push(TextVertexMember {
                            vertex: [x, y + self.fonts.height_px() as f32],
                            uv: [uvx as f32, uvy as f32 + self.fonts.height_px() as f32],
                            fg_color,
                            underline_pos,
                            underline_color,
                            strikeout_pos,
                            strikeout_color,
                        });
                        self.text_vertices.push(TextVertexMember {
                            vertex: [
                                x + self.fonts.min_width_px() as f32,
                                y + self.fonts.height_px() as f32,
                            ],
                            uv: [
                                uvx as f32 + self.fonts.min_width_px() as f32,
                                uvy as f32 + self.fonts.height_px() as f32,
                            ],
                            fg_color,
                            underline_pos,
                            underline_color,
                            strikeout_pos,
                            strikeout_color,
                        });
                    }
                }
            }

            self.render();
        }

        Ok(())
    }

    fn clear_region(
        &mut self,
        clear_type: ClearType,
    ) -> std::io::Result<()> {
        let bounds = self.size()?;
        let line_start = self.cursor.1 as usize * bounds.width as usize;
        let idx = line_start + self.cursor.0 as usize;

        match clear_type {
            ClearType::All => self.clear(),
            ClearType::AfterCursor => {
                self.cells.truncate(idx + 1);
                Ok(())
            }
            ClearType::BeforeCursor => {
                self.cells[..idx].fill(Cell::EMPTY);
                Ok(())
            }
            ClearType::CurrentLine => {
                self.cells[line_start..line_start + bounds.width as usize].fill(Cell::EMPTY);
                Ok(())
            }
            ClearType::UntilNewLine => {
                let remain = (bounds.width - self.cursor.0) as usize;
                self.cells[idx..idx + remain].fill(Cell::EMPTY);
                Ok(())
            }
        }
    }
}

/// Whether a cell's grapheme is a flag of regional indicators: two of them, and nothing else.
fn regional_pair(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let regional = |ch: Option<char>| matches!(ch, Some('\u{1F1E6}'..='\u{1F1FF}'));
    regional(chars.next()) && regional(chars.next()) && chars.next().is_none()
}

/// A cell a COLR face composes from several pictures: the pictures, each with its place from
/// the cell's origin in font units (its pen position plus its offset, and its vertical
/// offset), the run's advance in font units, and the atlas key's hash of the two.
pub(crate) struct Composed {
    glyphs: Vec<(GlyphId, i32, i32)>,
    advance: i32,
    key: u64,
}

/// The cells of one shaped run that a COLR face composes rather than joins, by cell index;
/// empty where none is. Windows 10's Segoe UI Emoji has no picture for a family or a couple:
/// it shapes 👨‍👩‍👧 to a man and a woman that advance and a girl that does not, placed by its
/// positioning back in front of them, and 👩‍❤️‍👨 to a woman, a heart placed over her and a man.
/// A cell is composed when it has emoji presentation, every glyph of it with ink is a COLR
/// picture, and a picture after the first does not advance: the face placed it among the
/// others, which is what tells a composition from a sequence of separate emoji it cannot join
/// (👨‍🦖, a man and a dinosaur that both advance, which change 16 draws as its man). Blank
/// glyphs (a ZWJ the face draws as nothing) are passed over. [`Font::composes`] asks the same.
pub(crate) fn composed_runs(
    face: &rustybuzz::Face,
    infos: &[rustybuzz::GlyphInfo],
    positions: &[rustybuzz::GlyphPosition],
    rowmap: &[u16],
    row: &[Cell],
) -> Vec<Option<Composed>> {
    #[derive(Default)]
    struct Gathered {
        glyphs: Vec<(GlyphId, i32, i32)>,
        pen: i32,
        ink_not_colour: bool,
        placed: bool,
    }
    let mut gathered: Vec<Option<Gathered>> = (0..row.len()).map(|_| None).collect();
    for (info, position) in infos.iter().zip(positions) {
        let cell = rowmap[info.cluster as usize] as usize;
        let at = gathered[cell].get_or_insert_with(Gathered::default);
        let glyph = GlyphId(info.glyph_id as _);
        if face.is_color_glyph(glyph) {
            at.placed |= position.x_advance == 0 && !at.glyphs.is_empty();
            at.glyphs.push((glyph, at.pen + position.x_offset, position.y_offset));
        } else if face.glyph_bounding_box(glyph).is_some() {
            at.ink_not_colour = true;
        }
        at.pen += position.x_advance;
    }
    gathered
        .into_iter()
        .enumerate()
        .map(|(cell, at)| {
            let at = at?;
            let composed = !at.ink_not_colour
                && at.placed
                && crate::fonts::emoji_presentation(row[cell].symbol());
            composed.then(|| {
                let mut hasher = DefaultHasher::new();
                at.glyphs.hash(&mut hasher);
                at.pen.hash(&mut hasher);
                // Never 0, which is one glyph's.
                let key = hasher.finish() | 1;
                Composed { glyphs: at.glyphs, advance: at.pen, key }
            })
        })
        .collect()
}

/// A glyph's raster for its atlas box, and whether it is in colour: a colour glyph's own pixels
/// are drawn (the mask's 255), a monochrome one is coverage the cell's foreground colour fills.
/// Upstream set the mask from the cluster's first character being an emoji, so an emoji drawn
/// from a text face's outline (a symbol face's ✔) came out white whatever the cell's colour,
/// and a colour picture of a character that is not an emoji would have been tinted.
///
/// `run` is a composed cell's pictures and their places in font units (`composed_runs`), all
/// painted into the one box, `actual_width` then being the run's advance; empty for `glyph`
/// alone, which is the run's first picture otherwise.
fn rasterize_glyph(
    cached: Entry,
    metrics: &rustybuzz::Face,
    glyph: GlyphId,
    run: &[(GlyphId, i32, i32)],
    fake_italic: bool,
    fake_bold: bool,
    face: FaceScale,
    actual_width: u32,
) -> (CacheRect, Vec<u32>, bool) {
    let advance_scale = face.scale;
    let alone = [(glyph, 0, 0)];
    let members = if run.is_empty() { &alone[..] } else { run };
    // `advance_scale` sizes the face (`Fonts::face_scale`). A glyph whose advance is wider than
    // its box is shrunk to the box and centred in the height it no longer fills. One narrower
    // than its box is centred in it at that size, never enlarged: enlarging also grows the line
    // past the cell, and the raster is clipped to the box, so a face whose wide glyphs advance
    // less than two cells (Apple SD Gothic Neo's hangul, 865 of 1000 units) lost the top of
    // every syllable and its right-hand strokes. The offsets are in the 2x raster's pixels, as
    // the transform below is.
    let fit = (cached.width as f32 / actual_width as f32).min(1.0);
    let mut computed_offset_x = cached.width as f32 - actual_width as f32 * fit;
    let mut computed_offset_y = cached.height as f32 * (1.0 - fit);
    let mut scale = fit * advance_scale * 2.0;

    // A fallback face drawn at the last resort's em and on its baseline can reach past the
    // cell where its own line, fitted to the cell, did not (a face whose ink rides high or
    // low). Only then is it moved, and only as far as it must be: shifted back inside when
    // its ink is no taller than the box, shrunk to the box's height when it is.
    if !face.primary {
        // The ink's height is every picture's of a composed run, each raised by its offset.
        let ink = members
            .iter()
            .filter_map(|&(member, _, dy)| {
                let bounds = metrics.glyph_bounding_box(member)?;
                Some((f32::from(bounds.y_min) + dy as f32, f32::from(bounds.y_max) + dy as f32))
            })
            .reduce(|(low, high), (y_min, y_max)| (low.min(y_min), high.max(y_max)));
        if let Some((y_min, y_max)) = ink {
            let box_h = cached.height as f32 * 2.0;
            let baseline = face.ascender * scale + computed_offset_y;
            let top = baseline - y_max * scale;
            let bottom = baseline - y_min * scale;
            let ink = bottom - top;
            if ink > box_h {
                let shrink = box_h / ink;
                scale *= shrink;
                computed_offset_x = cached.width as f32 - actual_width as f32 * fit * shrink;
                computed_offset_y = (y_max - face.ascender) * scale;
            } else if top < 0.0 {
                computed_offset_y -= top;
            } else if bottom > box_h {
                computed_offset_y -= bottom - box_h;
            }
        }
    }
    let baseline = face.ascender * scale + computed_offset_y;

    let skew = if fake_italic {
        Transform::new(
            /* scale x */ 1.0,
            /* skew x */ 0.0,
            /* skew y */ -0.25,
            /* scale y */ 1.0,
            /* translate x */ -0.25 * cached.width as f32,
            /* translate y */ 0.0,
        )
    } else {
        Transform::default()
    };

    let mut image = vec![0u32; cached.width as usize * 2 * cached.height as usize * 2];
    let mut target = DrawTarget::from_backing(
        cached.width as i32 * 2,
        cached.height as i32 * 2,
        &mut image[..],
    );

    // Each picture at its place: a glyph alone at the box's origin, a composed run's members
    // where the face's positioning put them.
    let mut painted = false;
    for &(member, dx, dy) in members {
        let mut painter = Painter::new(
            metrics,
            &mut target,
            skew,
            scale,
            baseline - dy as f32 * scale,
            computed_offset_x + dx as f32 * scale,
        );
        painted |= metrics
            .paint_color_glyph(member, 0, RgbaColor::new(255, 255, 255, 255), &mut painter)
            .is_some();
    }
    if painted {
        let mut final_image = DrawTarget::new(cached.width as i32, cached.height as i32);
        final_image.draw_image_with_size_at(
            cached.width as f32,
            cached.height as f32,
            0.,
            0.,
            &raqote::Image {
                width: cached.width as i32 * 2,
                height: cached.height as i32 * 2,
                data: &image,
            },
            &DrawOptions {
                blend_mode: raqote::BlendMode::Src,
                antialias: raqote::AntialiasMode::None,
                ..Default::default()
            },
        );

        let mut final_image = final_image.into_vec();
        for argb in final_image.iter_mut() {
            let [a, r, g, b] = argb.to_be_bytes();
            *argb = u32::from_le_bytes([r, g, b, a]);
        }

        return (*cached, final_image, true);
    }

    // A colour bitmap (sbix, CBDT) from the smallest strike at least twice the box's height, so
    // the downscale has pixels to average; upstream asked for the largest (Apple Color Emoji's
    // is 160 px, for a box of 16 to 32). A strike can lack a glyph the face has elsewhere (Apple
    // Color Emoji's 40, 48 and 52 px strikes have no ZWJ family), so a larger strike, then the
    // largest, is asked next.
    let strike = u16::try_from(cached.height * 2).unwrap_or(u16::MAX);
    let em = advance_scale * metrics.units_per_em() as f32;
    let colour = [strike, strike.saturating_mul(2), u16::MAX]
        .into_iter()
        .filter_map(|ppem| metrics.glyph_raster_image(glyph, ppem))
        .find_map(|raster| colour_bitmap(raster, cached, em));
    if let Some(pixels) = colour {
        return (*cached, pixels, true);
    }

    let mut render = Outline::default();
    if let Some(bounds) = metrics.outline_glyph(glyph, &mut render) {
        let path = render.finish();

        // Some fonts return bounds that are entirely negative. I'm not sure why this
        // is, but it means the glyph won't render at all. We check for this here and
        // offset it if so. This seems to let those fonts render correctly.
        let x_off = if bounds.x_max < 0 {
            -bounds.x_min as f32
        } else {
            0.
        };
        let x_off = x_off * scale + computed_offset_x;
        let y_off = baseline;

        let mut target = DrawTarget::from_backing(
            cached.width as i32 * 2,
            cached.height as i32 * 2,
            &mut image[..],
        );
        target.set_transform(
            &Transform::scale(scale, -scale)
                .then(&skew)
                .then_translate((x_off, y_off).into()),
        );

        target.fill(
            &path,
            &raqote::Source::Solid(SolidSource::from_unpremultiplied_argb(255, 255, 255, 255)),
            &DrawOptions::default(),
        );

        if fake_bold {
            target.stroke(
                &path,
                &raqote::Source::Solid(SolidSource::from_unpremultiplied_argb(255, 255, 255, 255)),
                &StrokeStyle {
                    width: 1.5,
                    ..Default::default()
                },
                &DrawOptions::new(),
            );
        }

        let mut final_image = DrawTarget::new(cached.width as i32, cached.height as i32);
        final_image.draw_image_with_size_at(
            cached.width as f32,
            cached.height as f32,
            0.,
            0.,
            &raqote::Image {
                width: cached.width as i32 * 2,
                height: cached.height as i32 * 2,
                data: &image,
            },
            &DrawOptions {
                blend_mode: raqote::BlendMode::Src,
                antialias: raqote::AntialiasMode::None,
                ..Default::default()
            },
        );

        return (*cached, final_image.into_vec(), false);
    }

    if let Some(raster) = metrics.glyph_raster_image(glyph, u16::MAX) {
        if raster.width != 0 && raster.height != 0 {
            if let Some((rect, pixels)) =
                extract_bw_image(&mut image, raster, cached, advance_scale)
            {
                return (rect, pixels, false);
            }
        }
    }

    (
        *cached,
        vec![0u32; cached.width as usize * cached.height as usize],
        false,
    )
}

/// A colour bitmap glyph's pixels as straight (not premultiplied) RGBA, row by row: a PNG
/// (sbix, CBDT; decoded only with the `png` feature) or premultiplied BGRA (CBDT's format 32).
/// `None` for a format that is not colour, or a PNG that does not decode.
fn colour_pixels(raster: &RasterGlyphImage) -> Option<(usize, usize, Vec<[u8; 4]>)> {
    match raster.format {
        RasterImageFormat::PNG => {
            #[cfg(feature = "png")]
            {
                let mut decoder = png::Decoder::new(std::io::Cursor::new(raster.data));
                decoder.set_transformations(png::Transformations::normalize_to_color8());
                let mut reader = decoder.read_info().ok()?;
                let mut bytes = vec![0; reader.output_buffer_size()?];
                let frame = reader.next_frame(&mut bytes).ok()?;
                let (width, height) = (frame.width as usize, frame.height as usize);
                let bytes = &bytes[..frame.buffer_size()];
                let pixels: Vec<[u8; 4]> = match frame.color_type {
                    png::ColorType::Rgba => {
                        bytes.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
                    }
                    png::ColorType::Rgb => {
                        bytes.chunks_exact(3).map(|p| [p[0], p[1], p[2], 255]).collect()
                    }
                    png::ColorType::GrayscaleAlpha => {
                        bytes.chunks_exact(2).map(|p| [p[0], p[0], p[0], p[1]]).collect()
                    }
                    png::ColorType::Grayscale => bytes.iter().map(|&g| [g, g, g, 255]).collect(),
                    png::ColorType::Indexed => return None,
                };
                (pixels.len() == width * height).then_some((width, height, pixels))
            }
            #[cfg(not(feature = "png"))]
            None
        }
        RasterImageFormat::BitmapPremulBgra32 => {
            let (width, height) = (raster.width as usize, raster.height as usize);
            let pixels: Vec<[u8; 4]> = raster
                .data
                .chunks_exact(4)
                .map(|p| {
                    let [b, g, r, a] = [p[0], p[1], p[2], p[3]];
                    let straight = |c: u8| {
                        if a == 0 {
                            0
                        } else {
                            (u32::from(c) * 255 / u32::from(a)).min(255) as u8
                        }
                    };
                    [straight(r), straight(g), straight(b), a]
                })
                .collect();
            (pixels.len() == width * height).then_some((width, height, pixels))
        }
        _ => None,
    }
}

/// A colour bitmap glyph drawn into its box: its strike's em scaled to `em` pixels, the size
/// the face is drawn at (`Fonts::face_scale`: the last resort's em, as every fallback face's
/// outlines are), no larger than the box's shorter side, and smaller still if the bitmap
/// would be wider or taller than the box, centred both ways; each pixel the average of the
/// bitmap pixels it covers, sampled on a grid as fine as the downscale, in premultiplied
/// terms so a transparent neighbour does not darken an edge. Upstream stretched the bitmap
/// over the whole box, offset by the bitmap's bearings read as font units (they are the
/// strike's pixels), with one bilinear sample per pixel: a 160 px picture squeezed into a 32
/// px box read one pixel in five, and a straight-alpha PNG was treated as premultiplied.
/// The result is straight RGBA, as the text pipeline blends (`ALPHA_BLENDING`) and the atlas
/// holds.
fn colour_bitmap(
    raster: RasterGlyphImage,
    cached: Entry,
    em: f32,
) -> Option<Vec<u32>> {
    let (width, height, pixels) = colour_pixels(&raster)?;
    if width == 0 || height == 0 {
        return None;
    }
    let (box_w, box_h) = (cached.width as f32, cached.height as f32);
    let strike_em = f32::from(raster.pixels_per_em.max(1));
    let scale = (em.min(box_w).min(box_h) / strike_em)
        .min(box_w / width as f32)
        .min(box_h / height as f32);
    let (drawn_w, drawn_h) = (width as f32 * scale, height as f32 * scale);
    let (left, top) = ((box_w - drawn_w) / 2.0, (box_h - drawn_h) / 2.0);
    let samples = (1.0 / scale).ceil().clamp(1.0, 8.0) as usize;

    let mut out = vec![0u32; cached.width as usize * cached.height as usize];
    for y in 0..cached.height as usize {
        for x in 0..cached.width as usize {
            let mut sum = [0f32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = x as f32 + (sx as f32 + 0.5) / samples as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / samples as f32;
                    let (u, v) = ((px - left) / scale, (py - top) / scale);
                    if u < 0.0 || v < 0.0 || u >= width as f32 || v >= height as f32 {
                        continue;
                    }
                    let [r, g, b, a] = pixels[v as usize * width + u as usize];
                    let alpha = f32::from(a) / 255.0;
                    sum[0] += f32::from(r) * alpha;
                    sum[1] += f32::from(g) * alpha;
                    sum[2] += f32::from(b) * alpha;
                    sum[3] += alpha;
                }
            }
            if sum[3] == 0.0 {
                continue;
            }
            let count = (samples * samples) as f32;
            let straight = |c: f32| (c / sum[3]).round().clamp(0.0, 255.0) as u8;
            let alpha = (sum[3] / count * 255.0).round().clamp(0.0, 255.0) as u8;
            out[y * cached.width as usize + x] =
                u32::from_le_bytes([straight(sum[0]), straight(sum[1]), straight(sum[2]), alpha]);
        }
    }
    Some(out)
}

fn extract_bw_image(
    image: &mut Vec<u32>,
    raster: RasterGlyphImage,
    cached: Entry,
    scale: f32,
) -> Option<(CacheRect, Vec<u32>)> {
    image.resize(raster.width as usize * raster.height as usize, 0);

    match raster.format {
        RasterImageFormat::BitmapMono => {
            from_gray_unpacked::<1, 2>(image, raster, LUT_1);
        }
        RasterImageFormat::BitmapMonoPacked => {
            from_gray_packed::<1, 2>(image, raster, LUT_1);
        }
        RasterImageFormat::BitmapGray2 => {
            from_gray_unpacked::<2, 4>(image, raster, LUT_2);
        }
        RasterImageFormat::BitmapGray2Packed => {
            from_gray_packed::<2, 4>(image, raster, LUT_2);
        }
        RasterImageFormat::BitmapGray4 => {
            from_gray_unpacked::<4, 16>(image, raster, LUT_4);
        }
        RasterImageFormat::BitmapGray4Packed => {
            from_gray_packed::<4, 16>(image, raster, LUT_4);
        }
        RasterImageFormat::BitmapGray8 => {
            for (byte, dst) in raster.data.iter().zip(image.iter_mut()) {
                *dst = u32::from_be_bytes([*byte, 255, 255, 255]);
            }
        }
        _ => return None,
    }

    let mut final_image = DrawTarget::new(cached.width as i32, cached.height as i32);
    final_image.draw_image_with_size_at(
        cached.width as f32,
        cached.height as f32,
        raster.x as f32 * scale,
        raster.y as f32 * scale,
        &raqote::Image {
            width: raster.width as i32,
            height: raster.height as i32,
            data: &*image,
        },
        &DrawOptions {
            blend_mode: raqote::BlendMode::Src,
            antialias: raqote::AntialiasMode::None,
            ..Default::default()
        },
    );

    let mut final_image = final_image.into_vec();
    for argb in final_image.iter_mut() {
        let [a, r, g, b] = argb.to_be_bytes();
        *argb = u32::from_le_bytes([r, g, b, a]);
    }

    Some((*cached, final_image))
}

fn from_gray_unpacked<const BITS: usize, const ENTRIES: usize>(
    image: &mut [u32],
    raster: RasterGlyphImage,
    steps: [u8; ENTRIES],
) {
    for (bits, dst) in raster
        .data
        .chunks((raster.width as usize / (8 / BITS)) + 1)
        .zip(image.chunks_mut(raster.width as usize))
    {
        let bits = BitSlice::<_, Lsb0>::from_slice(bits);
        for (bits, dst) in bits.chunks(BITS).zip(dst.iter_mut()) {
            let mut index = 0;
            for idx in bits.iter_ones() {
                index |= 1 << (BITS - idx - 1);
            }
            let value = steps[index as usize];
            *dst = u32::from_be_bytes([value, 255, 255, 255]);
        }
    }
}

fn from_gray_packed<const BITS: usize, const ENTRIES: usize>(
    image: &mut [u32],
    raster: RasterGlyphImage,
    steps: [u8; ENTRIES],
) {
    let bits = BitSlice::<_, Lsb0>::from_slice(raster.data);
    for (bits, dst) in bits.chunks(BITS).zip(image.iter_mut()) {
        let mut index = 0;
        for idx in bits.iter_ones() {
            index |= 1 << (BITS - idx - 1);
        }
        let value = steps[index as usize];
        *dst = u32::from_be_bytes([value, 255, 255, 255]);
    }
}

const LUT_1: [u8; 2] = [0, 255];
const LUT_2: [u8; 4] = [0, 255 / 3, 2 * (255 / 3), 255];
const LUT_4: [u8; 16] = [
    0,
    (255 / 15),
    2 * (255 / 15),
    3 * (255 / 15),
    4 * (255 / 15),
    5 * (255 / 15),
    6 * (255 / 15),
    7 * (255 / 15),
    8 * (255 / 15),
    9 * (255 / 15),
    10 * (255 / 15),
    11 * (255 / 15),
    12 * (255 / 15),
    13 * (255 / 15),
    14 * (255 / 15),
    255,
];
