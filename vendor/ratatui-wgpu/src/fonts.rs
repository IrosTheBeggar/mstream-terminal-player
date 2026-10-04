use std::hash::BuildHasher;
use std::hash::Hasher;
use std::hash::RandomState;

use ratatui_core::buffer::Cell;
use ratatui_core::style::Modifier;
use rustybuzz::ttf_parser::RasterImageFormat;
use rustybuzz::Face;
use unicode_properties::EmojiStatus;
use unicode_properties::UnicodeEmoji;

/// A Font which can be used for rendering.
#[derive(Clone)]
pub struct Font<'a> {
    font: Face<'a>,
    advance: f32,
    /// The face has an `m`, the glyph its width is read from. One without
    /// (a symbol or emoji face) only has `.notdef`'s advance to offer, which
    /// says nothing about a cell, so it does not narrow the grid.
    sets_width: bool,
    /// The face has colour glyphs the backend draws: COLR layers, or sbix or
    /// CBDT bitmaps. A text face has none, and is never asked which of its
    /// glyphs are in colour.
    colour: bool,
    id: u64,
}

impl<'a> Font<'a> {
    /// Create a new Font from data. Returns [`None`] if the font cannot
    /// be parsed. For a collection (`.ttc`) this is its first face; see
    /// [`Font::new_at`] for the others.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        Self::new_at(data, 0)
    }

    /// Create a new Font from the face at `index` of a font collection
    /// (`.ttc`/`.otc`); index 0 of a single font is the font itself. Returns
    /// [`None`] if there is no such face or it cannot be parsed.
    pub fn new_at(
        data: &'a [u8],
        index: u32,
    ) -> Option<Self> {
        let mut hasher = RandomState::new().build_hasher();
        // A bounded prefix and the length, not every byte: the bytes may be a
        // memory-mapped system collection of tens of megabytes, and reading
        // all of it here would fault the whole file into memory just to name
        // it. `RandomState::new` advances its keys on every call, so ids are
        // distinct per construction whatever is hashed.
        hasher.write(&data[..data.len().min(64 * 1024)]);
        hasher.write_usize(data.len());
        // The faces of one collection share its bytes; the index keeps their
        // ids, and so their cached glyphs, apart.
        hasher.write_u32(index);

        Face::from_slice(data, index).map(|font| {
            let m = font.glyph_index('m');
            let advance = font
                .glyph_hor_advance(m.unwrap_or_default())
                .unwrap_or_default() as f32;
            let tables = font.tables();
            let colour = tables.colr.is_some() || tables.sbix.is_some() || tables.cbdt.is_some();
            Self {
                font,
                advance,
                sets_width: m.is_some(),
                colour,
                id: hasher.finish(),
            }
        })
    }
}

impl Font<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn font(&'_ self) -> &'_ Face<'_> {
        &self.font
    }

    /// Whether this face has one picture for `text`: it shapes to a single glyph that advances,
    /// and that glyph is not `.notdef`. A grapheme the face has no ligature for shapes to several
    /// and the renderer draws only the first (VENDORED.md, change 16), so a caller that expects
    /// a joined picture asks this first: faces differ in which sequences they join (Segoe UI
    /// Emoji has no country flags at all), and that is the face's coverage, not a fault.
    pub fn joins(
        &self,
        text: &str,
    ) -> bool {
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&self.font, &[], buffer);
        let mut advancing = shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .filter(|(_, position)| position.x_advance != 0);
        matches!(
            (advancing.next(), advancing.next()),
            (Some((info, _)), None) if info.glyph_id != 0
        )
    }

    /// Whether this face composes `text` from several pictures that its positioning places,
    /// rather than joining it into one: the renderer then draws every picture of it into the
    /// cell's box (VENDORED.md, change 22). Windows 10's Segoe UI Emoji composes a family so,
    /// and [`Font::joins`] says no of it. Public so the player's tests know which to expect.
    pub fn composes(
        &self,
        text: &str,
    ) -> bool {
        if self.font.tables().colr.is_none() {
            return false;
        }
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&self.font, &[], buffer);
        let mut cell = Cell::EMPTY;
        cell.set_symbol(text);
        let rowmap = vec![0; text.len()];
        let runs = crate::backend::wgpu_backend::composed_runs(
            &self.font,
            shaped.glyph_infos(),
            shaped.glyph_positions(),
            &rowmap,
            std::slice::from_ref(&cell),
        );
        runs.into_iter().next().flatten().is_some()
    }

    /// How this face shapes `text` and where each glyph's ink lies, so a test can say what a
    /// composed cell must look like on whatever face it meets, and print what it met. Every
    /// shaped glyph is listed at its place, the pen plus its offset as `composed_runs` puts it,
    /// with the ink box of a COLR picture taken from the outlines painting it fills
    /// (`InkBounds`): a COLRv1 picture's own `glyf` entry may be empty, so its bounding box
    /// says nothing, while the layers it paints are outlines with boxes. Public so the player's
    /// tests can ask it (VENDORED.md, change 22).
    pub fn composition(
        &self,
        text: &str,
    ) -> Composition {
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&self.font, &[], buffer);
        let mut pen = 0;
        let mut glyphs = vec![];
        for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            let id = rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16);
            let (x, y) = (pen + position.x_offset, position.y_offset);
            let colour = self.font.is_color_glyph(id);
            let mut bounds = InkBounds::new(&self.font);
            if colour {
                let white = rustybuzz::ttf_parser::RgbaColor::new(255, 255, 255, 255);
                self.font.paint_color_glyph(id, 0, white, &mut bounds);
            }
            let ink = bounds.ink().map(|[x0, y0, x1, y1]| {
                [x0 + x as f32, y0 + y as f32, x1 + x as f32, y1 + y as f32]
            });
            glyphs.push(Placed {
                glyph: id.0,
                x_advance: position.x_advance,
                x_offset: position.x_offset,
                y_offset: position.y_offset,
                x,
                colour,
                v1: bounds.v1,
                ink,
            });
            pen += position.x_advance;
        }
        Composition {
            glyphs,
            advance: pen,
            units_per_em: self.font.units_per_em(),
            composes: self.composes(text),
        }
    }

    /// Whether this face's glyph for `ch` is one the backend draws in colour: COLR layers, or a
    /// colour bitmap (sbix or CBDT, a PNG or premultiplied BGRA). A face's monochrome bitmaps
    /// (EBDT, an old CJK face's hinted strikes) and a text face's outlines are not. Asked only
    /// of a cluster with emoji presentation, by [`Fonts::select_font`].
    fn colour_glyph(
        &self,
        ch: char,
    ) -> bool {
        if !self.colour {
            return false;
        }
        let Some(glyph) = self.font.glyph_index(ch) else {
            return false;
        };
        self.colour_glyph_id(glyph)
    }

    /// Whether this face draws all of `cluster` as one picture in colour: it shapes to a single
    /// advancing glyph ([`Font::joins`]) that is in colour as [`Font::colour_glyph`] judges a
    /// glyph. For a keycap (`1️⃣`, `#️⃣`), whose base, a digit or `#`, is a monochrome glyph
    /// even in the emoji face, the picture being the face's ligature for the whole sequence
    /// (VENDORED.md, change 24). Public so the player's tests know which to expect.
    pub fn colour_cluster(
        &self,
        cluster: &str,
    ) -> bool {
        if !self.colour {
            return false;
        }
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(cluster);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&self.font, &[], buffer);
        let mut advancing = shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .filter(|(_, position)| position.x_advance != 0);
        match (advancing.next(), advancing.next()) {
            (Some((info, _)), None) if info.glyph_id != 0 => u16::try_from(info.glyph_id)
                .is_ok_and(|id| self.colour_glyph_id(rustybuzz::ttf_parser::GlyphId(id))),
            _ => false,
        }
    }

    /// Whether `glyph` is one the backend draws in colour (see [`Font::colour_glyph`]).
    fn colour_glyph_id(
        &self,
        glyph: rustybuzz::ttf_parser::GlyphId,
    ) -> bool {
        self.font.is_color_glyph(glyph)
            || self.font.glyph_raster_image(glyph, u16::MAX).is_some_and(|raster| {
                matches!(
                    raster.format,
                    RasterImageFormat::PNG | RasterImageFormat::BitmapPremulBgra32
                )
            })
    }

    pub(crate) fn char_width(
        &self,
        height_px: u32,
    ) -> u32 {
        let scale = height_px as f32 / self.font.height() as f32;
        (self.advance * scale) as u32
    }

    /// [`Font::char_width`] for a fallback face: none (`u32::MAX`, which no
    /// `min` picks) for a face without an `m`. The last resort always
    /// counts, with or without one.
    fn fallback_width(
        &self,
        height_px: u32,
    ) -> u32 {
        if self.sets_width {
            self.char_width(height_px)
        } else {
            u32::MAX
        }
    }
}

/// A string as one face shapes it, from [`Font::composition`]: its glyphs in order and the
/// run's whole advance, in the face's units.
#[derive(Clone, Debug)]
pub struct Composition {
    pub glyphs: Vec<Placed>,
    /// The sum of the glyphs' advances: what change 22 fits into the cell's box.
    pub advance: i32,
    pub units_per_em: i32,
    /// [`Font::composes`] of the same string.
    pub composes: bool,
}

impl Composition {
    /// The ink of every COLR picture at its place, together: what a composed cell draws, as
    /// `[x_min, y_min, x_max, y_max]` in font units, y up. `None` if none has ink.
    pub fn ink(&self) -> Option<[f32; 4]> {
        self.glyphs
            .iter()
            .filter(|glyph| glyph.colour)
            .filter_map(|glyph| glyph.ink)
            .reduce(|[a0, b0, a1, b1], [x0, y0, x1, y1]| {
                [a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)]
            })
    }

    /// The first COLR picture's ink at its place: what was drawn of a composition before
    /// change 22, its first picture alone.
    pub fn first_ink(&self) -> Option<[f32; 4]> {
        self.glyphs.iter().find(|glyph| glyph.colour).and_then(|glyph| glyph.ink)
    }
}

/// One shaped glyph of a [`Composition`].
#[derive(Clone, Debug)]
pub struct Placed {
    pub glyph: u16,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    /// Where it is painted: the pen before it plus its `x_offset`.
    pub x: i32,
    /// A COLR picture (`Face::is_color_glyph`), as `composed_runs` counts one.
    pub colour: bool,
    /// Painted through a COLRv1 paint graph (a clip, a layer or a transform was pushed), not
    /// COLRv0's flat layers.
    pub v1: bool,
    /// The picture's ink at its place, `[x_min, y_min, x_max, y_max]` in font units, y up;
    /// `None` for a glyph that is not a picture or paints nothing.
    pub ink: Option<[f32; 4]>,
}

/// A COLR painter that paints nothing and keeps the box of what would be: the bounding box of
/// every outline a layer fills, through the transforms pushed around it, cut to the glyph's
/// clip box when the face gives one. A box, not the ink itself: an outline's box can hold
/// space its fill leaves clear, so it is an upper bound, tight for the faces' people.
struct InkBounds<'f, 'a> {
    face: &'f rustybuzz::Face<'a>,
    /// The transform in force and the ones it replaced, innermost last.
    transform: rustybuzz::ttf_parser::Transform,
    stack: Vec<rustybuzz::ttf_parser::Transform>,
    ink: Option<[f32; 4]>,
    /// The base glyph's clip box: the first pushed before any outline.
    clip: Option<[f32; 4]>,
    v1: bool,
}

impl<'f, 'a> InkBounds<'f, 'a> {
    fn new(face: &'f rustybuzz::Face<'a>) -> Self {
        Self {
            face,
            transform: Default::default(),
            stack: vec![],
            ink: None,
            clip: None,
            v1: false,
        }
    }

    /// `[x0, y0, x1, y1]` through the transform in force: the box of its four corners.
    fn transformed(
        &self,
        [x0, y0, x1, y1]: [f32; 4],
    ) -> [f32; 4] {
        let t = self.transform;
        let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
            .map(|(x, y)| (t.a * x + t.c * y + t.e, t.b * x + t.d * y + t.f));
        corners.iter().fold(
            [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
            |[a0, b0, a1, b1], &(x, y)| [a0.min(x), b0.min(y), a1.max(x), b1.max(y)],
        )
    }

    fn ink(&self) -> Option<[f32; 4]> {
        let [x0, y0, x1, y1] = self.ink?;
        let Some([c0, d0, c1, d1]) = self.clip else {
            return Some([x0, y0, x1, y1]);
        };
        let cut = [x0.max(c0), y0.max(d0), x1.min(c1), y1.min(d1)];
        (cut[0] < cut[2] && cut[1] < cut[3]).then_some(cut)
    }
}

impl<'a> rustybuzz::ttf_parser::colr::Painter<'a> for InkBounds<'_, 'a> {
    fn outline_glyph(
        &mut self,
        glyph_id: rustybuzz::ttf_parser::GlyphId,
    ) {
        let Some(rect) = self.face.glyph_bounding_box(glyph_id) else {
            return;
        };
        let rect = [rect.x_min, rect.y_min, rect.x_max, rect.y_max].map(f32::from);
        let [x0, y0, x1, y1] = self.transformed(rect);
        self.ink = Some(match self.ink {
            None => [x0, y0, x1, y1],
            Some([a0, b0, a1, b1]) => [a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)],
        });
    }

    fn paint(
        &mut self,
        _: rustybuzz::ttf_parser::colr::Paint<'a>,
    ) {
    }

    fn push_clip(&mut self) {
        self.v1 = true;
    }

    fn push_clip_box(
        &mut self,
        clipbox: rustybuzz::ttf_parser::colr::ClipBox,
    ) {
        self.v1 = true;
        if self.clip.is_none() && self.ink.is_none() {
            let rect = [clipbox.x_min, clipbox.y_min, clipbox.x_max, clipbox.y_max];
            self.clip = Some(self.transformed(rect));
        }
    }

    fn pop_clip(&mut self) {}

    fn push_layer(
        &mut self,
        _: rustybuzz::ttf_parser::colr::CompositeMode,
    ) {
        self.v1 = true;
    }

    fn pop_layer(&mut self) {}

    fn push_transform(
        &mut self,
        transform: rustybuzz::ttf_parser::Transform,
    ) {
        self.v1 = true;
        self.stack.push(self.transform);
        // The outer transform applies last, as the renderer's `Painter` composes them.
        self.transform = rustybuzz::ttf_parser::Transform::combine(self.transform, transform);
    }

    fn pop_transform(&mut self) {
        if let Some(outer) = self.stack.pop() {
            self.transform = outer;
        }
    }
}

/// How one face is drawn into a cell: font units to pixels, and the baseline
/// in the face's own units (its distance below the cell's top is
/// `ascender * scale`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FaceScale {
    pub(crate) scale: f32,
    pub(crate) ascender: f32,
    /// The face is drawn as the last resort is: the same em, line and
    /// ascender, so its own line fits the cell exactly as before.
    pub(crate) primary: bool,
}

/// A collection of fonts to use for rendering. Supports font fallback.
///
/// It is recommended, but not required, that all fonts have the same/very
/// similar aspect ratio, or you may get unexpected results during rendering due
/// to fallback.
pub struct Fonts<'a> {
    char_width: u32,
    char_height: u32,

    last_resort: Font<'a>,

    regular: Vec<Font<'a>>,
    bold: Vec<Font<'a>>,
    italic: Vec<Font<'a>>,
    bold_italic: Vec<Font<'a>>,
}

impl<'a> Fonts<'a> {
    /// Create a new, empty set of fonts. The provided font will be used as a
    /// last-resort fallback if no other fonts can render a particular
    /// character. Rendering will attempt to fake bold/italic styles using this
    /// font where appropriate.
    ///
    /// The provided size_px will be the rendered height in pixels of all fonts
    /// in this collection.
    pub fn new(
        font: Font<'a>,
        size_px: u32,
    ) -> Self {
        Self {
            char_width: font.char_width(size_px),
            char_height: size_px,
            last_resort: font,
            regular: vec![],
            bold: vec![],
            italic: vec![],
            bold_italic: vec![],
        }
    }

    /// The height (in pixels) of all fonts.
    #[inline]
    pub fn height_px(&self) -> u32 {
        self.char_height
    }

    /// Change the height of all fonts in this collection to the specified
    /// height in pixels.
    pub fn set_size_px(
        &mut self,
        height_px: u32,
    ) {
        self.char_height = height_px;

        let fallbacks = self
            .regular
            .iter()
            .chain(self.bold.iter())
            .chain(self.italic.iter())
            .chain(self.bold_italic.iter())
            .map(|font| font.fallback_width(height_px));
        self.char_width = std::iter::once(self.last_resort.char_width(height_px))
            .chain(fallbacks)
            .min()
            .unwrap_or_default();
    }

    /// Add a collection of fonts for various styles. They will automatically be
    /// added to the appropriate fallback font list based on the font's
    /// bold/italic properties. Note that this will automatically organize fonts
    /// by relative width in order to optimize fallback rendering quality. The
    /// ordering of already provided fonts will remain unchanged.
    pub fn add_fonts(
        &mut self,
        fonts: impl IntoIterator<Item = Font<'a>>,
    ) {
        let bold_italic_len = self.bold_italic.len();
        let italic_len = self.italic.len();
        let bold_len = self.bold.len();
        let regular_len = self.regular.len();

        for font in fonts {
            if !font.font().is_monospaced() {
                warn!("Non monospace font used in add_fonts, this may cause unexpected rendering.");
            }

            self.char_width = self.char_width.min(font.fallback_width(self.char_height));
            if font.font().is_italic() && font.font().is_bold() {
                self.bold_italic.push(font);
            } else if font.font().is_italic() {
                self.italic.push(font);
            } else if font.font().is_bold() {
                self.bold.push(font);
            } else {
                self.regular.push(font);
            }
        }

        self.bold_italic[bold_italic_len..].sort_by_key(|font| font.char_width(self.char_height));
        self.italic[italic_len..].sort_by_key(|font| font.char_width(self.char_height));
        self.bold[bold_len..].sort_by_key(|font| font.char_width(self.char_height));
        self.regular[regular_len..].sort_by_key(|font| font.char_width(self.char_height));
    }

    /// Add a new collection of fonts for regular styled text. These fonts will
    /// come _after_ previously provided fonts in the fallback order.
    pub fn add_regular_fonts(
        &mut self,
        fonts: impl IntoIterator<Item = Font<'a>>,
    ) {
        self.char_width = self.char_width.min(Self::add_fonts_internal(
            &mut self.regular,
            fonts,
            self.char_height,
        ));
    }

    /// Add a new collection of fonts for bold styled text. These fonts will
    /// come _after_ previously provided fonts in the fallback order.
    ///
    /// You do not have to provide these for bold text to be supported. If no
    /// bold fonts are supplied, rendering will fallback to the regular fonts
    /// with fake bolding.
    pub fn add_bold_fonts(
        &mut self,
        fonts: impl IntoIterator<Item = Font<'a>>,
    ) {
        self.char_width = self.char_width.min(Self::add_fonts_internal(
            &mut self.bold,
            fonts,
            self.char_height,
        ));
    }

    /// Add a new collection of fonts for italic styled text. These fonts will
    /// come _after_ previously provided fonts in the fallback order.
    ///
    /// It is recommended, but not required, that you provide italic fonts if
    /// your application intends to make use of italics. If no italic fonts
    /// are supplied, rendering will fallback to the regular fonts with fake
    /// italics.
    pub fn add_italic_fonts(
        &mut self,
        fonts: impl IntoIterator<Item = Font<'a>>,
    ) {
        self.char_width = self.char_width.min(Self::add_fonts_internal(
            &mut self.italic,
            fonts,
            self.char_height,
        ));
    }

    /// Add a new collection of fonts for bold italic styled text. These fonts
    /// will come _after_ previously provided fonts in the fallback order.
    ///
    /// You do not have to provide these for bold text to be supported. If no
    /// bold fonts are supplied, rendering will fallback to the italic fonts
    /// with fake bolding.
    pub fn add_bold_italic_fonts(
        &mut self,
        fonts: impl IntoIterator<Item = Font<'a>>,
    ) {
        self.char_width = self.char_width.min(Self::add_fonts_internal(
            &mut self.bold_italic,
            fonts,
            self.char_height,
        ));
    }
}

impl<'a> Fonts<'a> {
    /// The minimum width (in pixels) across all fonts.
    pub(crate) fn min_width_px(&self) -> u32 {
        self.char_width
    }

    /// How a face is scaled into the cell. The last resort's line (ascender
    /// to descender) is the cell's height, as upstream sized every face.
    /// Every other face is drawn at the last resort's pixels per em, with its
    /// baseline on the last resort's: upstream fitted each face's own line
    /// to the cell, so a face with a tall line drew small and one with a
    /// short line large (Apple SD Gothic Neo's hangul, on a 1200-unit line,
    /// at 23 px beside Hiragino's kana, on a 1000-unit line, at 27 to 30,
    /// in a 32 px cell), and a hangul syllable, a kana and a hanzi did not
    /// sit at one size. A face with the last resort's em, line and
    /// ascender (another copy of it, or one drawn on its metrics) is
    /// scaled exactly as before.
    pub(crate) fn face_scale(
        &self,
        face: &Face,
    ) -> FaceScale {
        let primary = self.last_resort.font();
        let height = self.char_height as f32;
        if face.units_per_em() == primary.units_per_em()
            && face.height() == primary.height()
            && face.ascender() == primary.ascender()
        {
            return FaceScale {
                scale: height / face.height() as f32,
                ascender: face.ascender() as f32,
                primary: true,
            };
        }
        let primary_scale = height / primary.height() as f32;
        let scale = primary_scale * primary.units_per_em() as f32 / face.units_per_em() as f32;
        FaceScale {
            scale,
            ascender: primary.ascender() as f32 * primary_scale / scale,
            primary: false,
        }
    }

    pub(crate) fn count(&self) -> usize {
        1 + self.bold.len() + self.italic.len() + self.bold_italic.len() + self.regular.len()
    }

    pub(crate) fn font_for_cell(
        &'_ self,
        cell: &Cell,
    ) -> (&'_ Font<'_>, bool, bool) {
        if cell.modifier.contains(Modifier::BOLD | Modifier::ITALIC) {
            self.select_font(
                cell.symbol(),
                self.bold_italic
                    .iter()
                    .map(|f| (f, false, false))
                    .chain(self.italic.iter().map(|f| (f, true, false)))
                    .chain(self.bold.iter().map(|f| (f, false, true)))
                    .chain(self.regular.iter().map(|f| (f, true, true))),
                true,
                true,
            )
        } else if cell.modifier.contains(Modifier::BOLD) {
            self.select_font(
                cell.symbol(),
                self.bold
                    .iter()
                    .map(|f| (f, false, false))
                    .chain(self.regular.iter().map(|f| (f, true, false))),
                true,
                false,
            )
        } else if cell.modifier.contains(Modifier::ITALIC) {
            self.select_font(
                cell.symbol(),
                self.italic
                    .iter()
                    .map(|f| (f, false, false))
                    .chain(self.regular.iter().map(|f| (f, false, true))),
                false,
                true,
            )
        } else {
            self.select_font(
                cell.symbol(),
                self.regular.iter().map(|f| (f, false, false)),
                false,
                false,
            )
        }
    }

    fn select_font<'fonts>(
        &'fonts self,
        cluster: &str,
        fonts: impl IntoIterator<Item = (&'fonts Font<'a>, bool, bool)>,
        last_resort_fake_bold: bool,
        last_resort_fake_italic: bool,
    ) -> (&'fonts Font<'a>, bool, bool) {
        // A face with the cluster's first character, its base, beats any face without it,
        // and then the face with the most of the cluster wins, as upstream chose. Upstream
        // counted characters alone, so a base followed by characters that only an emoji face
        // has (a stray run of tags, the tail of a flag whose 🏴 a text field clipped) was
        // given to the emoji face, which has no glyph for the base and drew `.notdef`, a box,
        // over it. With the base's face, the shaper hides the default-ignorable rest.
        //
        // Between those two, a cluster with emoji presentation (🎵, ❤️, a ZWJ family, a flag)
        // prefers a face whose glyph for its base is in colour. The first face with every
        // character used to win outright, and a symbol face that comes before the emoji face
        // so that ♥ and ✔ keep their text form has monochrome outlines for emoji too: Segoe
        // UI Symbol drew 🎵 as an outline and 👨‍👩‍👧 as one grey silhouette on Windows, where
        // a terminal draws both from Segoe UI Emoji. A cluster with text presentation (♥, ✔,
        // ★ without VS16, a digit) is chosen as before, and with no colour face for an emoji
        // the first face with the most of it still draws it.
        //
        // A keycap (a digit, `#` or `*`, VS16, then U+20E3) is the one emoji whose base is
        // text in every face: the emoji face's `1` is a monochrome digit like any other, and
        // its picture is the ligature it shapes the whole sequence to. Judged by its base it
        // was never in colour, so the emoji face drew it only by having the most of it, and a
        // text face ahead of it with all three characters (one that maps U+FE0F) drew it as a
        // plain digit beside an enclosing mark. For a keycap the face's shaping of the whole
        // cluster is asked as well ([`Font::colour_cluster`]): a face that joins it into one
        // colour picture is in colour. A digit or `#` alone is text and never asks.
        let emoji = emoji_presentation(cluster);
        let keycap = emoji && is_keycap(cluster);
        let mut max = (false, false, 0);
        let mut font = None;
        let base = cluster.chars().next();
        for (candidate, fake_bold, fake_italic) in fonts.into_iter().chain(std::iter::once((
            &self.last_resort,
            last_resort_fake_bold,
            last_resort_fake_italic,
        ))) {
            let (count, last_idx) =
                cluster
                    .chars()
                    .enumerate()
                    .fold((0, 0), |(mut count, _), (idx, ch)| {
                        count += usize::from(candidate.font().glyph_index(ch).is_some());
                        (count, idx)
                    });
            let has_base = base.is_some_and(|ch| candidate.font().glyph_index(ch).is_some());
            let colour = emoji
                && has_base
                && (base.is_some_and(|ch| candidate.colour_glyph(ch))
                    || (keycap && candidate.colour_cluster(cluster)));
            if (has_base, colour, count) > max {
                max = (has_base, colour, count);
                font = Some((candidate, fake_bold, fake_italic));
            }

            // A face with all of the cluster ends the search, unless the cluster is an emoji
            // and the face would draw it as an outline: a colour face may still come.
            if count == last_idx + 1 && (!emoji || colour) {
                break;
            }
        }

        *font.get_or_insert((
            &self.last_resort,
            last_resort_fake_bold,
            last_resort_fake_italic,
        ))
    }

    fn add_fonts_internal(
        target: &mut Vec<Font<'a>>,
        fonts: impl IntoIterator<Item = Font<'a>>,
        char_height: u32,
    ) -> u32 {
        let len = target.len();
        target.extend(fonts);

        target[len..]
            .iter()
            .map(|font| font.fallback_width(char_height))
            .min()
            .unwrap_or(u32::MAX)
    }
}

/// Whether a cluster (one cell's grapheme) asks to be drawn as an emoji, a picture, rather than
/// as text, by Unicode's rules (UTS #51): a character whose default is emoji presentation (🎵,
/// ⌚, a regional indicator) unless VS15 (U+FE0E) follows it; any character VS16 (U+FE0F)
/// follows; and an emoji followed by what only an emoji sequence has, a skin tone, a keycap's
/// U+20E3, tags (a subdivision flag) or a ZWJ and another emoji. A character whose default is
/// text (♥, ✔, ★, a digit, `#`) with none of those after it is text. Public so the player's
/// tests can hold it to known code points.
pub fn emoji_presentation(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    let Some(base) = chars.next() else {
        return false;
    };
    match chars.next() {
        Some('\u{FE0E}') => false,
        Some('\u{FE0F}') => true,
        None => default_emoji(base),
        Some(_) if default_emoji(base) => true,
        Some(_) if !base.is_emoji_char() => false,
        Some(_) => {
            // A skin tone, a keycap, a tag, or an emoji after a ZWJ.
            let mut after_zwj = false;
            cluster.chars().skip(1).any(|ch| {
                let joined = std::mem::replace(&mut after_zwj, ch == '\u{200D}');
                (joined && ch.is_emoji_char())
                    || matches!(
                        ch,
                        '\u{1F3FB}'..='\u{1F3FF}' | '\u{20E3}' | '\u{E0020}'..='\u{E007F}'
                    )
            })
        }
    }
}

/// Whether a cluster is a keycap: a digit, `#` or `*`, VS16 (or, unqualified, nothing), then
/// U+20E3, and nothing more (UTS #51's `emoji_keycap_sequence`). Its base is text in every
/// face, so [`Fonts::select_font`] judges a face's colour by the whole sequence instead.
fn is_keycap(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    let base = chars.next().is_some_and(|ch| ch.is_ascii_digit() || ch == '#' || ch == '*');
    let rest: Vec<char> = chars.collect();
    base && matches!(rest.as_slice(), ['\u{FE0F}', '\u{20E3}'] | ['\u{20E3}'])
}

/// `Emoji_Presentation=Yes`: the character is a picture unless asked otherwise. It is the
/// property, not `Emoji`, which every character with an emoji form has: ♥, ✔ and `#` are
/// emoji characters whose default is text. No ASCII character has it, so most cells are
/// answered without the table.
fn default_emoji(ch: char) -> bool {
    !ch.is_ascii()
        && matches!(
            ch.emoji_status(),
            EmojiStatus::EmojiPresentation
                | EmojiStatus::EmojiPresentationAndModifierBase
                | EmojiStatus::EmojiPresentationAndEmojiComponent
                | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
        )
}
