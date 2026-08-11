//! The cover as a picture, on terminals that can draw one.
//!
//! Everything else in the drawing path renders through characters, which is
//! why it works everywhere. Kitty, sixel and iTerm2 are the exception: they
//! carry real pixels, and an album cover is the one thing this player shows
//! that is worth them. [`Graphics`] holds the capability query's answer and
//! the encoded cover, and is the only place that knows any of those
//! protocols exist.
//!
//! Two rules shape the whole module.
//!
//! **No picker means no picture.** The query needs a real terminal to
//! answer it, so a test, a replay run, a pipe, or a `TERM` that cannot say
//! yes all end up here with nothing — and the caller draws what it drew
//! before. That single condition covers `TestBackend` (where the sixel
//! escape sequence would land *in the asserted buffer* as binary soup),
//! the wasm build, tmux without passthrough, and a terminal that simply
//! doesn't do graphics. It is not a fallback bolted on; it is the default.
//!
//! **Halfblocks are declined.** The crate offers them when nothing better
//! is available, and they are worse than what this repo already has:
//! `tui::canvas` box-averages through `art::cover_sample` and merges runs
//! into shared spans, where the generic fallback assumes a 4:8 cell and
//! draws every cell its own span. Taking it would be a downgrade wearing
//! the word "image", so a halfblocks-only answer is treated as a no.

#[cfg(not(target_arch = "wasm32"))]
pub use native::Graphics;
#[cfg(target_arch = "wasm32")]
pub use stub::Graphics;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use ratatui::Frame;
    use ratatui::layout::{Rect, Size};
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::protocol::Protocol;
    use ratatui_image::{Image, Resize};

    use crate::tui::art::Art;

    pub struct Graphics {
        /// `None` is the ordinary state, not the error state — see the
        /// module note.
        picker: Option<Picker>,
        /// The cover, encoded for the protocol in hand. Encoding is
        /// blocking and happens at render time, so it is done once per
        /// answer rather than once per frame: a new cover or a resized
        /// panel rebuilds it, and everything else reuses it. The same
        /// bargain `viz::CoverGrid` strikes, keyed the same way.
        cached: Option<Cached>,
        /// The last attempt that came to nothing, so it is not made again
        /// next frame. Failure here is as sticky as success: a cover that
        /// will not decode, or fits this box at zero size, would otherwise
        /// be fully re-decoded thirty times a second for as long as the
        /// view is up — the mosaic drawing quietly over the waste.
        refused: Option<Refusal>,
        /// How many render-time decodes have run, for the tests that pin
        /// the caching above — a cache that silently stopped caching would
        /// otherwise still pass every drawing assertion.
        #[cfg(test)]
        decodes: std::cell::Cell<u32>,
    }

    /// What failed, precisely enough not to over-refuse: a decode failure
    /// is the art's for good, a zero fit or a refused encoder is only this
    /// art in this box — a resize deserves a fresh try.
    struct Refusal {
        art: u64,
        /// `None` when the bytes would not decode at all.
        area: Option<(u16, u16)>,
    }

    struct Cached {
        art: u64,
        /// The source's own pixel dimensions. Kept so the question "would
        /// this area want a different picture?" can be answered without
        /// decoding the cover again to ask it.
        source: (u32, u32),
        /// How many cells the picture came out as. This, not the area, is
        /// what the cache turns on — see [`Graphics::draw`].
        size: (u16, u16),
        protocol: Protocol,
    }

    /// The answer only. A `Protocol` is a cover's worth of encoded pixels,
    /// which is not something any test failure wants printed at it.
    impl std::fmt::Debug for Graphics {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.picker {
                Some(picker) => write!(f, "Graphics({:?})", picker.protocol_type()),
                None => write!(f, "Graphics(off)"),
            }
        }
    }

    impl Graphics {
        /// Ask the terminal what it can draw. Writes escape sequences to
        /// stdio and reads the replies, so it belongs at startup with the
        /// terminal already in raw mode — never inside a draw. Bounded: the
        /// crate answers within two seconds even opposite a terminal that
        /// says nothing, and immediately when stdin is not a tty at all.
        ///
        /// `MSTREAM_NO_GRAPHICS=1` skips the whole question. The probe is
        /// as honest as the terminal's own answers, and a terminal that
        /// misdescribes itself — advertising a protocol it draws badly —
        /// would otherwise leave no way back to the mosaic.
        pub fn probe() -> Graphics {
            if std::env::var("MSTREAM_NO_GRAPHICS").is_ok_and(|v| !v.is_empty() && v != "0") {
                tracing::info!("terminal graphics: disabled by MSTREAM_NO_GRAPHICS");
                return Graphics::disabled();
            }
            // The probe is only as good as the terminal's answers, and
            // terminals lie in both directions — the kill switch above is
            // for one lie, this is for the other: a terminal (or an
            // intermediary like tmux) that draws a protocol it will not
            // admit to. Forcing skips the query entirely, so the font size
            // comes from the window-size ioctl, with the halfblocks
            // picker's 10x20 guess when the terminal reports no pixels.
            if let Ok(forced) = std::env::var("MSTREAM_GRAPHICS") {
                let protocol = match forced.to_ascii_lowercase().as_str() {
                    "kitty" => Some(ProtocolType::Kitty),
                    "sixel" => Some(ProtocolType::Sixel),
                    "iterm2" => Some(ProtocolType::Iterm2),
                    "off" | "0" => {
                        tracing::info!("terminal graphics: disabled by MSTREAM_GRAPHICS");
                        return Graphics::disabled();
                    }
                    other => {
                        tracing::info!("terminal graphics: MSTREAM_GRAPHICS={other:?} ignored");
                        None
                    }
                };
                if let Some(protocol) = protocol {
                    let font = ratatui::crossterm::terminal::window_size()
                        .ok()
                        .and_then(forced_font)
                        .unwrap_or(ratatui_image::FontSize::new(10, 20));
                    // Deprecated in favour of the query — which is the
                    // thing being overridden.
                    #[allow(deprecated)]
                    let mut picker = Picker::from_fontsize(font);
                    picker.set_protocol_type(protocol);
                    tracing::info!("terminal graphics: forced {forced:?}, cell {font:?}");
                    return Graphics { picker: Some(picker), ..Graphics::disabled() };
                }
            }

            let mut picker = Picker::from_query_stdio()
                .ok()
                .filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks);

            // iTerm2 answers both the kitty query and DA1's sixel bit, and
            // the query trusts capability answers over environment hints —
            // so left alone the probe lands on kitty there. Both answers
            // are worse than the terminal's own protocol, one fatally:
            // iTerm2 3.6 says yes to kitty but does not draw the
            // unicode-placeholder form this crate emits — smoke-tested on
            // 3.6.11: the placeholder cells render as nothing at all — and
            // its sixel is palette-bound and costs 165 ms a cover to
            // encode against iTerm2-protocol's 67 (both measured, 640px).
            // Demoted *after* the query rather than blacklisted before it,
            // though upstream reaches for the blacklist on WezTerm and
            // Konsole: iTerm2 answers `[16t` from its window layer a beat
            // later than `[5n`, which the crate treats as the end of the
            // replies — with kitty and sixel struck from the query there
            // was nothing left in front to wait behind, the cell-size
            // report lost that race, and the probe read "no answers at
            // all" (found on 3.6.11: the blacklisted probe came back
            // empty in 25 ms; the full query never does). Not under tmux:
            // these variables outlive the terminal that set them, and a
            // tmux session may be attached from somewhere that has never
            // heard of iTerm2.
            if let Some(picker) = picker.as_mut()
                && matches!(picker.protocol_type(), ProtocolType::Kitty | ProtocolType::Sixel)
                && std::env::var_os("TMUX").is_none()
                && ["TERM_PROGRAM", "LC_TERMINAL"]
                    .iter()
                    .any(|var| std::env::var(var).is_ok_and(|v| v.contains("iTerm")))
            {
                picker.set_protocol_type(ProtocolType::Iterm2);
            }

            let graphics = Graphics { picker, ..Graphics::disabled() };
            // One line in the flight recorder: which way this terminal
            // answered is the first fact every cover-looks-wrong report
            // needs, and it is unknowable after the fact.
            tracing::info!("terminal graphics: {graphics:?}");
            graphics
        }

        /// The probe's whole answer as one printable line, for the
        /// `graphics-probe` diagnostic. The capability list is upstream's
        /// raw finding, worth seeing when the conclusion looks wrong —
        /// "sixel" in the capabilities but half-blocks in the answer says
        /// the kill switch or a filter got involved; nothing in the
        /// capabilities says the terminal never answered.
        pub fn diagnostics(&self) -> String {
            match &self.picker {
                Some(picker) => {
                    let font = picker.font_size();
                    format!(
                        "protocol {}   cell {}x{} px   capabilities {:?}",
                        self.protocol().unwrap_or("none"),
                        font.width,
                        font.height,
                        picker.capabilities()
                    )
                }
                None => "no pixel protocol — covers render as half-block mosaic".into(),
            }
        }

        /// A `Graphics` that never draws. What every build that is not the
        /// real binary gets, and what `App` starts with.
        pub fn disabled() -> Graphics {
            Graphics {
                picker: None,
                cached: None,
                refused: None,
                #[cfg(test)]
                decodes: std::cell::Cell::new(0),
            }
        }

        /// Which protocol is carrying the picture, for `graphics-probe` to
        /// say out loud. `None` is the character rendering.
        ///
        /// Worth showing rather than keeping to ourselves: whether a
        /// terminal ended up on kitty, on sixel, or on neither is the first
        /// thing anyone asks when the cover looks wrong, and it is not
        /// otherwise discoverable from inside the program.
        pub fn protocol(&self) -> Option<&'static str> {
            match self.picker.as_ref()?.protocol_type() {
                ProtocolType::Kitty => Some("kitty"),
                ProtocolType::Sixel => Some("sixel"),
                ProtocolType::Iterm2 => Some("iterm2"),
                // Declined at probe time, so this is unreachable — named
                // rather than caught by a wildcard so that a new protocol
                // upstream shows up here as a compile error instead of
                // silently reporting itself as half-blocks.
                ProtocolType::Halfblocks => None,
            }
        }

        /// Draw `art` centred in `area`, reporting whether it managed to.
        /// `false` is the caller's cue to draw the mosaic instead, and is
        /// returned for every reason there might be: no picker, no source
        /// bytes, bytes that won't decode, an area too small to hold a
        /// picture, or an encoder that refused.
        ///
        /// What gets re-encoded, and when, is the whole performance story.
        /// Encoding a cover costs about 10 ms on kitty and about 165 ms on
        /// sixel, and it happens here, at render time, on the thread the
        /// keyboard is waiting on. So the cache turns on the *fitted size*
        /// rather than on the area: a square cover in a wide panel is
        /// bounded by the panel's height, and every column added or removed
        /// leaves the picture exactly the same size. Keyed on the area, a
        /// slow drag of a terminal edge would re-encode on every cell
        /// crossed; keyed on the size, most of those cost the arithmetic
        /// below and nothing else.
        pub fn draw(&mut self, frame: &mut Frame, area: Rect, art: &Art) -> bool {
            let Some(picker) = self.picker.as_ref() else {
                return false;
            };
            if area.width == 0 || area.height == 0 {
                return false;
            }
            // A failure already on record answers without the decode that
            // discovering it again would cost — see `Refusal`.
            if let Some(refusal) = &self.refused
                && refusal.art == art.id()
                && refusal.area.is_none_or(|a| a == (area.width, area.height))
            {
                return false;
            }
            let font = picker.font_size();
            let font = (font.width, font.height);

            let stale = match &self.cached {
                Some(held) => {
                    held.art != art.id()
                        || fit(area, font, held.source.0, held.source.1) != held.size
                }
                None => true,
            };

            if stale {
                // Decoded here rather than kept decoded: the cache holds
                // sixty-four covers, and at this size the pixels are an
                // order of magnitude more memory than the bytes.
                #[cfg(test)]
                self.decodes.set(self.decodes.get() + 1);
                let Ok(source) = image::load_from_memory(art.source()) else {
                    self.refused = Some(Refusal { art: art.id(), area: None });
                    return false;
                };
                let dimensions = (source.width(), source.height());
                let size = fit(area, font, dimensions.0, dimensions.1);
                if size.0 == 0 || size.1 == 0 {
                    self.refused =
                        Some(Refusal { art: art.id(), area: Some((area.width, area.height)) });
                    return false;
                }
                let fitted = Size::new(size.0, size.1);
                // Scale rather than Fit: Fit never enlarges, so a cover
                // smaller than its box kept its own size while `centre`
                // placed the box's — small art sat off-centre in blank
                // slack where the mosaic fills the panel. Scale meets the
                // box exactly; the box already has the cover's shape, so
                // nothing distorts. Triangle over the Nearest default
                // because enlargement is now a path a low-res cover
                // actually takes, and Nearest enlarges into mosaic — the
                // thing this rendering exists to be better than.
                let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
                let Ok(protocol) = picker.new_protocol(source, fitted, resize) else {
                    self.refused =
                        Some(Refusal { art: art.id(), area: Some((area.width, area.height)) });
                    return false;
                };
                self.cached = Some(Cached { art: art.id(), source: dimensions, size, protocol });
            }

            // Unwrap: the branch above either filled this or returned.
            let held = self.cached.as_ref().expect("just built");
            frame.render_widget(Image::new(&held.protocol), centre(area, held.size));
            true
        }
    }

    impl Graphics {
        /// A picker that pretends: the named protocol at a 10x20 cell, no
        /// terminal consulted. What lets the pixel path run under
        /// `TestBackend`, where its escape sequences land in the asserted
        /// buffer — exactly what a test of that path wants to look at.
        #[cfg(test)]
        pub(crate) fn forced(protocol: ProtocolType) -> Graphics {
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(10, 20));
            picker.set_protocol_type(protocol);
            Graphics { picker: Some(picker), ..Graphics::disabled() }
        }
    }

    /// The cell size a window-size report implies, or `None` for a report
    /// with a zero anywhere in it. Clamped to sanity in both directions:
    /// some terminals report pixel fields smaller than the cell grid — the
    /// quotient truncates to a zero-wide font that upstream divides by —
    /// and a transiently one-column window implies a font wider than any
    /// glyph ever drawn, which overflows upstream's pixel arithmetic
    /// instead.
    fn forced_font(
        size: ratatui::crossterm::terminal::WindowSize,
    ) -> Option<ratatui_image::FontSize> {
        if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
            return None;
        }
        Some(ratatui_image::FontSize::new(
            (size.width / size.columns).clamp(1, 128),
            (size.height / size.rows).clamp(1, 128),
        ))
    }

    /// How many cells a `w` × `h` pixel image takes when scaled to the
    /// largest size that fits inside `area` without changing shape.
    ///
    /// Cells are about twice as tall as they are wide, so a square cover
    /// laid out in cells is not a square of cells — the font size is what
    /// converts between the two, and getting it wrong is the difference
    /// between a cover and a cover stretched into a letterbox.
    fn fit(area: Rect, font: (u16, u16), w: u32, h: u32) -> (u16, u16) {
        if w == 0 || h == 0 {
            return (0, 0);
        }
        // u64 because the cross-products multiply a source dimension by a
        // pixel area: a panel maxes out around 2^32 pixels a side in the
        // worst u16 corner, and times a photograph's width that clears u32
        // — an overflow here would be a wrongly-sized cover once a decade,
        // which is the worst kind of bug to reproduce.
        let (fw, fh) = (u64::from(font.0.max(1)), u64::from(font.1.max(1)));
        let (aw, ah) = (u64::from(area.width) * fw, u64::from(area.height) * fh);
        let (w, h) = (u64::from(w), u64::from(h));
        // Whole-pixel scale, taken as a ratio rather than a float so the
        // two dimensions cannot round apart.
        let cells_w = (aw.min(w * ah / h) / fw) as u16;
        let cells_h = (ah.min(h * aw / w) / fh) as u16;
        (cells_w.min(area.width), cells_h.min(area.height))
    }

    /// A `size` rect in the middle of `area`, the odd row and column
    /// falling below and to the right. Saturating because a rect that
    /// overhangs its area is a panic in ratatui's own arithmetic, and a
    /// cover is not worth taking the program down over.
    fn centre(area: Rect, size: (u16, u16)) -> Rect {
        let (width, height) = (size.0.min(area.width), size.1.min(area.height));
        Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn area(width: u16, height: u16) -> Rect {
            Rect { x: 0, y: 0, width, height }
        }

        #[test]
        fn a_square_cover_fits_as_half_as_many_rows_as_columns() {
            // A 10x20 pixel cell, so a square picture across all 40 columns
            // is 400 pixels wide, wants 400 tall, and 400 pixels is 20 rows
            // — not 40. Treating cells as square is what puts a cover in a
            // letterbox, and this is the test that would catch it.
            assert_eq!(fit(area(40, 30), (10, 20), 500, 500), (40, 20));
        }

        #[test]
        fn the_short_dimension_is_what_limits_it() {
            // Same cover, a panel too short to give it 20 rows: the height
            // binds instead and the width comes down to match.
            assert_eq!(fit(area(40, 8), (10, 20), 500, 500), (16, 8));
        }

        #[test]
        fn what_fits_is_centred_in_what_it_was_given() {
            // 40x20 in 40x30: nothing spare across, ten rows spare down,
            // five of them above.
            let placed = centre(area(40, 30), fit(area(40, 30), (10, 20), 500, 500));
            assert_eq!((placed.x, placed.y), (0, 5), "{placed:?}");
            // Still inside, on both axes, which is the property that
            // matters more than the exact offset.
            assert!(placed.x + placed.width <= 40);
            assert!(placed.y + placed.height <= 30);
        }

        #[test]
        fn a_panel_with_no_room_asks_for_nothing_rather_than_overflowing() {
            let (width, height) = fit(area(1, 1), (10, 20), 500, 500);
            assert!(width <= 1 && height <= 1);
            assert_eq!(fit(area(40, 30), (10, 20), 0, 0), (0, 0), "a cover with no pixels");
            // A size that could not possibly fit is clamped rather than
            // wrapped into a rect that overhangs the screen.
            let placed = centre(area(4, 4), (99, 99));
            assert_eq!((placed.width, placed.height), (4, 4), "{placed:?}");
        }

        #[test]
        fn a_wide_cover_keeps_its_shape_too() {
            // Twice as wide as tall, on square-ish pixels: the row count
            // comes out half what the square one got.
            let square = fit(area(40, 30), (10, 20), 500, 500);
            let wide = fit(area(40, 30), (10, 20), 1000, 500);
            assert!(wide.1 < square.1, "{wide:?} vs {square:?}");
            assert!(wide.0 >= square.0);
        }

        #[test]
        fn widening_a_panel_a_column_at_a_time_does_not_keep_re_encoding() {
            // The reason the cache turns on the fitted size rather than on
            // the area. This square cover is bounded by the panel's height
            // at every one of these widths, so it is the same picture each
            // time — and encoding it again would be 165 ms of frozen
            // keyboard per column on a sixel terminal.
            let sizes: Vec<_> =
                (40..60).map(|width| fit(area(width, 20), (10, 20), 640, 640)).collect();
            let first = sizes[0];
            assert!(
                sizes.iter().all(|size| *size == first),
                "a height-bound cover changed size as the panel widened: {sizes:?}"
            );
            // And when the picture really would come out different it does
            // notice — the cache would be wrong, not merely cold. A row
            // taken away, since height is what binds here; a row added
            // would not, because at 40 columns the width binds instead.
            assert_ne!(fit(area(40, 19), (10, 20), 640, 640), first);
        }

        /// A cover as real encoded bytes, because the render-time decode is
        /// part of the path under test.
        fn a_cover(side: u32) -> crate::tui::art::Art {
            let mut pixels = image::RgbImage::new(side, side);
            for (x, y, pixel) in pixels.enumerate_pixels_mut() {
                // Something with detail in it: a flat fill quantises to one
                // palette entry and would flatter sixel enormously.
                let v = ((x * 7) ^ (y * 13)) as u8;
                *pixel = image::Rgb([v, v.wrapping_mul(3), 255 - v]);
            }
            let mut bytes = std::io::Cursor::new(Vec::new());
            pixels.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            crate::tui::art::decode(&bytes.into_inner()).unwrap()
        }

        /// Every protocol produces something, and what each one costs.
        ///
        /// No terminal ever answers a query in a test, so the protocols are
        /// forced rather than detected — which is the only way this path is
        /// exercised at all in CI, and it is worth exercising: an upstream
        /// change that stops one of them encoding would otherwise only show
        /// up on someone's actual terminal.
        ///
        /// The cover is deliberately small. At 640px this one test cost the
        /// debug suite twelve seconds, almost all of it sixel quantisation;
        /// the regression it guards — a protocol that stops encoding — is
        /// exactly as visible at 128. (The real costs, measured in release
        /// at 640px into 60x30: kitty 10.6 ms, sixel 165 ms, iTerm2 67 ms
        /// to encode; re-drawing the same cover, microseconds.) Run with
        /// `--nocapture` for this size's numbers.
        #[test]
        fn every_protocol_encodes_a_cover_into_something_the_terminal_can_read() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            let art = a_cover(128);
            for protocol in [ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
                let mut graphics = Graphics::forced(protocol);

                let mut terminal = Terminal::new(TestBackend::new(24, 12)).unwrap();
                let start = std::time::Instant::now();
                let mut drew = false;
                terminal
                    .draw(|frame| drew = graphics.draw(frame, frame.area(), &art))
                    .unwrap();
                let encode = start.elapsed();

                assert!(drew, "{protocol:?} declined to draw a 128px cover");
                let bytes: usize = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol().len())
                    .sum();
                eprintln!("{protocol:?}: {encode:?} to encode, {bytes} bytes to the terminal");
                // Not a threshold, a sanity floor: a protocol that "worked"
                // but wrote a screenful of spaces has not drawn a cover.
                assert!(bytes > 1_000, "{protocol:?} wrote only {bytes} bytes");

                // The second frame is the one that has to be free — it is
                // every frame after the first, and the reason encoding at
                // render time is survivable at all.
                let start = std::time::Instant::now();
                terminal
                    .draw(|frame| {
                        graphics.draw(frame, frame.area(), &art);
                    })
                    .unwrap();
                let cached = start.elapsed();
                eprintln!("  and {cached:?} for the same cover again");
                assert!(cached < encode, "{protocol:?}: the cache saved nothing");
            }
        }

        #[test]
        fn a_disabled_graphics_never_claims_to_have_drawn() {
            let graphics = Graphics::disabled();
            assert_eq!(graphics.protocol(), None);
            assert_eq!(format!("{graphics:?}"), "Graphics(off)");
        }

        #[test]
        fn a_small_cover_is_enlarged_to_fill_its_fitted_box() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // A 64px cover in a 400x200px area: Fit would have kept it at
            // its own 7x4 cells anchored where `centre` placed the 20x10
            // box the arithmetic promised — small art hanging in the box's
            // slack. Scale meets the promise. Kitty writes each row's
            // placeholders into the row's first cell as one string, so the
            // evidence is the char count and where the rows start: 20
            // placeholder chars per row, 10 rows, first cell at column 10
            // — the centred box exactly. Any of enlargement, centring or
            // the fit regressing moves at least one of the three.
            let art = a_cover(64);
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
            let mut drew = false;
            terminal.draw(|frame| drew = graphics.draw(frame, frame.area(), &art)).unwrap();
            assert!(drew, "a small cover still draws");

            let buffer = terminal.backend().buffer();
            let mut chars = 0;
            let (mut xs, mut ys) = ((u16::MAX, 0u16), (u16::MAX, 0u16));
            for y in 0..10 {
                for x in 0..40 {
                    let here =
                        buffer[(x, y)].symbol().matches('\u{10EEEE}').count();
                    if here > 0 {
                        chars += here;
                        xs = (xs.0.min(x), xs.1.max(x));
                        ys = (ys.0.min(y), ys.1.max(y));
                    }
                }
            }
            assert_eq!(chars, 20 * 10, "every cell of the fitted box is covered");
            assert_eq!(xs.0, 10, "the box starts at the centred column");
            assert_eq!(ys, (0, 9), "and spans the rows the fit promised");
        }

        /// Wide and short, so a small box fits it at zero rows.
        fn a_banner() -> crate::tui::art::Art {
            let mut pixels = image::RgbImage::new(560, 140);
            for (x, y, pixel) in pixels.enumerate_pixels_mut() {
                let v = ((x * 3) ^ (y * 11)) as u8;
                *pixel = image::Rgb([v, 255 - v, v.wrapping_mul(5)]);
            }
            let mut bytes = std::io::Cursor::new(Vec::new());
            pixels.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            crate::tui::art::decode(&bytes.into_inner()).unwrap()
        }

        #[test]
        fn a_cover_that_cannot_fit_is_refused_once_not_every_frame() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // The banner in a 60x60px box fits at zero rows: no picture.
            // Learning that costs one full decode — and only one, where it
            // used to cost one per frame, thirty a second, for as long as
            // the view stayed up with the mosaic drawing over the waste.
            let art = a_banner();
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let mut terminal = Terminal::new(TestBackend::new(6, 3)).unwrap();
            for _ in 0..3 {
                terminal
                    .draw(|frame| assert!(!graphics.draw(frame, frame.area(), &art)))
                    .unwrap();
            }
            assert_eq!(graphics.decodes.get(), 1, "one decode buys the whole answer");

            // A resize is a different question and earns a fresh attempt.
            let mut terminal = Terminal::new(TestBackend::new(24, 12)).unwrap();
            let mut drew = false;
            terminal.draw(|frame| drew = graphics.draw(frame, frame.area(), &art)).unwrap();
            assert!(drew, "the banner fits a real panel");
            assert_eq!(graphics.decodes.get(), 2);

            // Bytes that never decode are refused at every size for the
            // price of one failed attempt (a from_rgb art has no source).
            let pixels = crate::tui::art::Art::from_rgb(2, 2, vec![0; 12]).unwrap();
            for size in [(6, 3), (24, 12)] {
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                terminal
                    .draw(|frame| assert!(!graphics.draw(frame, frame.area(), &pixels)))
                    .unwrap();
            }
            assert_eq!(graphics.decodes.get(), 3, "undecodable bytes are asked exactly once");
        }

        #[test]
        fn a_degenerate_window_report_cannot_produce_a_zero_font() {
            use ratatui::crossterm::terminal::WindowSize;
            // Pixel fields smaller than the cell grid — some conpty and
            // embedded hosts report this — used to truncate to a zero-wide
            // font that upstream divides by.
            let font =
                forced_font(WindowSize { rows: 50, columns: 200, width: 100, height: 100 })
                    .unwrap();
            assert_eq!((font.width, font.height), (1, 2));
            // A transiently one-column window implies an absurd font.
            let font =
                forced_font(WindowSize { rows: 1, columns: 1, width: 3840, height: 2160 })
                    .unwrap();
            assert_eq!((font.width, font.height), (128, 128));
            // A zero anywhere means the report is unusable, not clampable.
            assert!(
                forced_font(WindowSize { rows: 0, columns: 80, width: 800, height: 600 })
                    .is_none()
            );
        }
    }
}

/// The browser build draws through ratzilla's DOM backend, where there is
/// no terminal to query and an escape sequence is just text in a `<div>`.
/// Same shape as the real one so the drawing path does not fork.
#[cfg(target_arch = "wasm32")]
mod stub {
    use ratatui::Frame;
    use ratatui::layout::Rect;

    use crate::tui::art::Art;

    #[derive(Debug)]
    pub struct Graphics;

    impl Graphics {
        pub fn disabled() -> Graphics {
            Graphics
        }

        pub fn protocol(&self) -> Option<&'static str> {
            None
        }

        pub fn draw(&mut self, _frame: &mut Frame, _area: Rect, _art: &Art) -> bool {
            false
        }
    }
}
