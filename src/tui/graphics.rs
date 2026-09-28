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
pub use native::{Graphics, release_all, release_dropped};
#[cfg(target_arch = "wasm32")]
pub use stub::Graphics;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Write;
    use std::sync::Mutex;

    use ratatui::Frame;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Rect, Size};
    use ratatui::widgets::Widget;
    use ratatui_image::picker::cap_parser::Parser;
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::protocol::Protocol;
    use ratatui_image::protocol::kitty::Kitty;
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
        /// Whether the picker's cell size may be re-read from the
        /// window-size ioctl as the session runs. True for every picker a
        /// real terminal produced; false for the test constructor, whose
        /// 10x20 must not drift toward whatever terminal happens to be
        /// running the suite.
        adaptive: bool,
        /// Whether a kitty transmission outlives a resize here — see
        /// [`images_outlive_resize`]. When it does, a resize keeps the
        /// cached picture, and a wall of covers is not sent again for
        /// every cell a window edge is dragged across.
        images_outlive_resize: bool,
        /// Whether kitty's pixels go deflated — see [`deflate_kitty`].
        deflate: bool,
        /// Whether escapes this instance builds itself go through tmux's
        /// passthrough: the picker's own answer, read once for it
        /// ([`wrapped_for_tmux`]) rather than on every encode.
        tmux: bool,
        /// When the window-size ioctl was last consulted. One cover per
        /// frame was this module's design point; the GUI's album wall
        /// draws fifteen, and re-asking for every one of them was
        /// hundreds of syscalls a second for an answer that changes on
        /// the scale of someone adjusting their font.
        font_checked: std::time::Instant,
        /// The one kitty image this instance draws, for its whole life —
        /// see [`KittyId`].
        kitty_id: KittyId,
        /// How many render-time decodes have run, for the tests that pin
        /// the caching above — a cache that silently stopped caching would
        /// otherwise still pass every drawing assertion.
        #[cfg(test)]
        decodes: std::cell::Cell<u32>,
        /// And how many protocols have been built — every encode, from the
        /// thumbnail or the source — for the tests that pin what a scroll
        /// or a shared cover costs.
        #[cfg(test)]
        encodes: std::cell::Cell<u32>,
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
        /// The pixel dimensions the fit was taken from — the thumbnail's
        /// when it had every pixel the box wanted, the decoded source's
        /// otherwise. Kept so the question "would this area want a
        /// different picture?" can be answered without decoding the cover
        /// again to ask it.
        source: (u32, u32),
        /// How many cells the picture came out as. This, not the area, is
        /// what the cache turns on — see [`Graphics::draw`].
        size: (u16, u16),
        /// How many cells the protocol actually encoded — the fit floors
        /// its two dimensions independently, so for aspect ratios the
        /// rounding does not favour, the picture comes out a cell or so
        /// smaller than the box it was fitted to. Centring on the box
        /// left that slack hanging below and to the right; the picture's
        /// own size is what belongs in the middle.
        shown: (u16, u16),
        protocol: Protocol,
        /// Kitty's pixels, until the frame that carries them to the
        /// terminal — see [`kitty_picture`]. `None` once sent, and for
        /// the protocols that carry their pixels in the cells.
        transmit: Option<String>,
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
        /// stdio and reads the replies — the crate handles raw mode itself,
        /// so the caller owes it only a real terminal and a moment nothing
        /// else is talking: at startup, never inside a draw. Bounded: the
        /// crate answers within two seconds even opposite a terminal that
        /// says nothing — and on such a terminal its termios restore can
        /// be lost with its reader thread, so callers bracket the call in
        /// state they re-assert themselves (ratatui::init/restore in the
        /// player, a crossterm raw-mode snapshot in graphics-probe).
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
                    return Graphics {
                        tmux: wrapped_for_tmux(&picker),
                        picker: Some(picker),
                        adaptive: true,
                        images_outlive_resize: images_outlive_resize(protocol, |var| {
                            std::env::var(var).ok()
                        }),
                        deflate: deflate_kitty(|var| std::env::var(var).ok()),
                        ..Graphics::disabled()
                    };
                }
            }

            if skip_query_for_apple_terminal(|var| std::env::var(var).ok()) {
                tracing::info!("terminal graphics: Apple Terminal — no pixel protocol, query skipped");
                return Graphics { adaptive: true, ..Graphics::disabled() };
            }

            if crate::kit::theme::legacy_conhost() {
                tracing::info!("terminal graphics: classic conhost — queries go unanswered, query skipped");
                return Graphics { adaptive: true, ..Graphics::disabled() };
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
            // empty in 25 ms; the full query never does). Not under a
            // multiplexer — tmux, zellij, or screen: these variables
            // outlive the terminal that set them (iTerm2 forwards
            // LC_TERMINAL through ssh on purpose), and inside a
            // multiplexer the queries were answered by the multiplexer,
            // whose own protocol support is what actually matters.
            if let Some(picker) = picker.as_mut()
                && demote_for_iterm2(picker.protocol_type(), |var| std::env::var(var).ok())
            {
                picker.set_protocol_type(ProtocolType::Iterm2);
            }

            let outlive = picker.as_ref().is_some_and(|p| {
                images_outlive_resize(p.protocol_type(), |var| std::env::var(var).ok())
            });
            let graphics = Graphics {
                tmux: picker.as_ref().is_some_and(wrapped_for_tmux),
                picker,
                adaptive: true,
                images_outlive_resize: outlive,
                deflate: deflate_kitty(|var| std::env::var(var).ok()),
                ..Graphics::disabled()
            };
            // One line in the flight recorder: which way this terminal
            // answered is the first fact every cover-looks-wrong report
            // needs, and it is unknowable after the fact.
            tracing::info!("terminal graphics: {graphics:?}");
            graphics
        }

        /// The probe's whole answer as one printable line, for the
        /// `graphics-probe` diagnostic. The capability list is upstream's
        /// raw finding, worth seeing when the conclusion looks wrong —
        /// kitty in the capabilities with iterm2 as the protocol is the
        /// demotion doing its job. The mosaic verdict is one fixed line
        /// with no list at all: every road there discards the picker,
        /// answers and all, so it cannot say whether the terminal
        /// answered halfblocks-only or never answered anything.
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

        /// A second `Graphics` on the same probed answer, with its own
        /// empty cache. The cache holds ONE encoded picture, so two
        /// surfaces alternating on one instance would re-encode both
        /// every frame — a fork gives each its own slot.
        pub fn fork(&self) -> Graphics {
            Graphics {
                picker: self.picker.clone(),
                adaptive: self.adaptive,
                images_outlive_resize: self.images_outlive_resize,
                deflate: self.deflate,
                tmux: self.tmux,
                ..Graphics::disabled()
            }
        }

        /// A `Graphics` that never draws. What every build that is not the
        /// real binary gets, and what `App` starts with.
        pub fn disabled() -> Graphics {
            Graphics {
                picker: None,
                cached: None,
                refused: None,
                adaptive: false,
                images_outlive_resize: false,
                deflate: false,
                tmux: false,
                font_checked: std::time::Instant::now(),
                kitty_id: KittyId::default(),
                #[cfg(test)]
                decodes: std::cell::Cell::new(0),
                #[cfg(test)]
                encodes: std::cell::Cell::new(0),
            }
        }

        /// How many covers this instance has encoded — tests only.
        #[cfg(test)]
        pub(crate) fn encodes(&self) -> u32 {
            self.encodes.get()
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

        /// The terminal may have been replaced — a tmux reattach puts a
        /// fresh kitty instance behind the same tty, and the image store
        /// the cached transmission lives in went with the old one. Kitty is
        /// the one protocol that transmits pixels once and then draws by
        /// reference; sixel and iTerm2 carry their pixels in the cells and
        /// survive a full repaint on their own. Called on every terminal
        /// resize, which a reattach almost always delivers; a same-size
        /// reattach stays blank only until the next resize or track change.
        /// Where no reattach can happen — kitty itself, no multiplexer — the
        /// transmission stays: sending a wall of covers again on every
        /// resize was most of what a resize cost there.
        pub fn refresh(&mut self) {
            let kitty =
                self.picker.as_ref().is_some_and(|p| p.protocol_type() == ProtocolType::Kitty);
            if kitty && !self.images_outlive_resize {
                self.cached = None;
            }
            // A resize is what the half-second font check exists to catch:
            // the next draw re-reads the cell size rather than encoding
            // against the old one for up to half a second.
            self.font_checked = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_millis(500))
                .unwrap_or_else(std::time::Instant::now);
        }

        /// Keep the picker's cell size current. Cmd+minus mid-session
        /// changes the font; the window keeps its pixels; and a sixel
        /// image encoded against the old cell size paints over its
        /// neighbours (kitty and iTerm2 scale to cells and shrug). The
        /// window-size ioctl is the same well the forced path drinks from
        /// — no escape bytes, microseconds — and a report without pixels
        /// keeps the probed font. The picker is rebuilt rather than
        /// adjusted because upstream exposes no setter; its capability
        /// list goes with it, which only `graphics-probe` prints, and
        /// that prints before any draw.
        fn refresh_font(&mut self) {
            if !self.adaptive {
                return;
            }
            // Half a second is soon enough to catch a font change; asking
            // on every draw was fine with one cover a frame and is not
            // with a wall of them.
            if self.font_checked.elapsed() < std::time::Duration::from_millis(500) {
                return;
            }
            self.font_checked = std::time::Instant::now();
            let Some(picker) = self.picker.as_ref() else {
                return;
            };
            let Some(fresh) =
                ratatui::crossterm::terminal::window_size().ok().and_then(forced_font)
            else {
                return;
            };
            let held = picker.font_size();
            if (held.width, held.height) == (fresh.width, fresh.height) {
                return;
            }
            let protocol = picker.protocol_type();
            #[allow(deprecated)]
            let mut rebuilt = Picker::from_fontsize(fresh);
            rebuilt.set_protocol_type(protocol);
            self.tmux = wrapped_for_tmux(&rebuilt);
            self.picker = Some(rebuilt);
            // Everything remembered was measured against the old cells.
            self.cached = None;
            self.refused = None;
            tracing::info!(
                "terminal graphics: cell size now {}x{} px",
                fresh.width,
                fresh.height
            );
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
        ///
        /// Nor does the cache turn on the area's place: an encoded cover
        /// draws anywhere for free — kitty by reference to what it
        /// transmitted, sixel and iTerm2 by their bytes re-emitted — so a
        /// picture that merely moves (a scrolled queue row) costs nothing.
        ///
        /// And what an encode decodes turns on how many pixels the box
        /// wants. The thumbnail beside every `Art` shares the source's
        /// shape at 128 px a side, so fitting the box from it says whether
        /// the source is needed at all: a 6x3 cover at a 10x20 font is 60
        /// px a side and a 12x6 wall cell 120, both inside the thumbnail —
        /// no decode, where decoding a 400 px jpeg was tens of
        /// milliseconds in a debug build for every cover a scroll revealed
        /// (2026-09-20). Only a box that wants more pixels than the
        /// thumbnail holds decodes the source.
        pub fn draw(&mut self, frame: &mut Frame, area: Rect, art: &Art) -> bool {
            self.refresh_font();
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

            // Warm: the same cover at the same fitted size, wherever the
            // box now stands.
            let warm = self.cached.as_ref().is_some_and(|held| {
                held.art == art.id()
                    && fit(area, font, held.source.0, held.source.1) == held.size
            });
            if !warm {
                // A failure already on record for this box answers without
                // the work of discovering it again — see `Refusal`.
                if self.refused.as_ref().is_some_and(|refusal| {
                    refusal.art == art.id() && refusal.area == Some((area.width, area.height))
                }) {
                    return false;
                }
                let thumb = (art.width(), art.height());
                let size = fit(area, font, thumb.0, thumb.1);
                if size.0 == 0 || size.1 == 0 {
                    self.refused =
                        Some(Refusal { art: art.id(), area: Some((area.width, area.height)) });
                    return false;
                }
                let wanted =
                    (u32::from(size.0) * u32::from(font.0), u32::from(size.1) * u32::from(font.1));
                let (source, dimensions, size) = if wanted.0 <= thumb.0 && wanted.1 <= thumb.1 {
                    // The thumbnail has every pixel the box can show.
                    let pixels = image::RgbImage::from_raw(thumb.0, thumb.1, art.rgb().to_vec())
                        .expect("an Art's pixels match its dimensions");
                    (image::DynamicImage::ImageRgb8(pixels), thumb, size)
                } else {
                    // Bytes that would not decode will not decode now.
                    if self
                        .refused
                        .as_ref()
                        .is_some_and(|refusal| refusal.art == art.id() && refusal.area.is_none())
                    {
                        return false;
                    }
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
                    (source, dimensions, size)
                };
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
                // Kitty's picture is built here rather than by the picker,
                // to be sent under this instance's own id (`KittyId`).
                let built = match picker.protocol_type() {
                    ProtocolType::Kitty => {
                        let (tmux, deflate) = (self.tmux, self.deflate);
                        let id = self.kitty_id.get(tmux);
                        kitty_picture(picker, source, fitted, &resize, (id, tmux, deflate))
                            .map(|(protocol, transmit)| (protocol, Some(transmit)))
                    }
                    // A cover that arrived as a JPEG goes to iTerm2 as one;
                    // everything lossless stays lossless (`iterm2_jpeg`).
                    ProtocolType::Iterm2 if art.source().starts_with(&[0xFF, 0xD8, 0xFF]) => {
                        iterm2_jpeg(picker, source, fitted, &resize, self.tmux).map(|p| (p, None))
                    }
                    _ => picker.new_protocol(source, fitted, resize).ok().map(|p| (p, None)),
                };
                let Some((protocol, transmit)) = built else {
                    self.refused =
                        Some(Refusal { art: art.id(), area: Some((area.width, area.height)) });
                    return false;
                };
                #[cfg(test)]
                self.encodes.set(self.encodes.get() + 1);
                let shown = protocol.size();
                self.cached = Some(Cached {
                    art: art.id(),
                    source: dimensions,
                    size,
                    shown: (shown.width, shown.height),
                    protocol,
                    transmit,
                });
            }

            // Unwrap: the branch above either filled this or returned.
            let held = self.cached.as_mut().expect("just built");
            let placed = centre(area, held.shown);
            frame.render_widget(Image::new(&held.protocol), placed);
            // Kitty's pixels ride the first frame that draws the picture,
            // ahead of the placeholders in its first cell — where upstream
            // puts its own — and are let go once they have: after that
            // frame nothing can send them again, so keeping them was a
            // cover's worth of base64 held for nothing (performance audit
            // #95). A picture that drew nothing here (its first cell off
            // the buffer) keeps them for a frame that does.
            if held.transmit.is_some()
                && let Some(cell) = frame.buffer_mut().cell_mut((placed.x, placed.y))
                && cell.symbol().contains('\u{10EEEE}')
                && let Some(transmit) = held.transmit.take()
            {
                let symbol = transmit + cell.symbol();
                cell.set_symbol(&symbol);
            }
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
            let tmux = wrapped_for_tmux(&picker);
            Graphics { picker: Some(picker), tmux, ..Graphics::disabled() }
        }
    }

    /// Kitty keeps every picture it is sent until it is told to let go —
    /// and nothing told it. Upstream mints a random id for every encode and
    /// never deletes one, so each new cover, wall page, screen switch and
    /// resize left the last picture in the terminal's image store, as a GPU
    /// texture, under a virtual placement that its alternate-screen clear
    /// skips: up to kitty's 320 MiB quota, outliving the player until the
    /// window closed (performance audit #94).
    ///
    /// So each `Graphics` owns ONE id for its whole life, and every encode
    /// is sent under it. kitty takes a transmission to an id it already
    /// holds as a replacement — the old image, its texture and its
    /// placement freed in place (graphics.c, `handle_add_command`) — and
    /// the new pixels travel in the same frame as the placeholders that
    /// show them, so no frame shows a gap. A `Graphics` that goes away (a
    /// queue row's slot let go, a wall slot past the page) owes the
    /// terminal a delete, sent after the next frame has drawn over its
    /// cells ([`release_dropped`]); leaving the alternate screen deletes
    /// the rest ([`release_all`]).
    ///
    /// Ids are a random base plus a count, not a count from one: two
    /// players in panes of one tmux share the outer kitty's store, and a
    /// repeated id there would replace — and delete — the other's picture.
    #[derive(Default)]
    struct KittyId(Option<(u32, bool)>);

    impl KittyId {
        /// This instance's id, handed out on first use.
        fn get(&mut self, tmux: bool) -> u32 {
            if let Some((id, _)) = self.0 {
                return id;
            }
            let mut ids = KITTY_IDS.lock().unwrap_or_else(|poison| poison.into_inner());
            if ids.next == 0 {
                ids.next = fastrand::u32(1..);
            }
            let id = ids.next;
            // Zero is kitty's "no id"; the count steps over it when it wraps.
            ids.next = ids.next.wrapping_add(1).max(1);
            ids.live.push((id, tmux));
            self.0 = Some((id, tmux));
            id
        }
    }

    impl Drop for KittyId {
        fn drop(&mut self) {
            if let Some(entry) = self.0.take() {
                let mut ids = KITTY_IDS.lock().unwrap_or_else(|poison| poison.into_inner());
                ids.live.retain(|live| live.0 != entry.0);
                ids.gone.push(entry);
            }
        }
    }

    /// Every id this process has handed out and not yet deleted, with
    /// whether its escapes go through tmux: `live` still drawn by a
    /// `Graphics`, `gone` owed a delete.
    struct KittyIds {
        next: u32,
        live: Vec<(u32, bool)>,
        gone: Vec<(u32, bool)>,
    }

    static KITTY_IDS: Mutex<KittyIds> =
        Mutex::new(KittyIds { next: 0, live: Vec::new(), gone: Vec::new() });

    /// The deletes owed for the pictures whose `Graphics` went away — and,
    /// with `all`, for every picture this process sent.
    fn owed_deletes(all: bool) -> String {
        let mut ids = KITTY_IDS.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut owed = std::mem::take(&mut ids.gone);
        if all {
            owed.append(&mut ids.live);
        }
        owed.iter().map(|&(id, tmux)| kitty_delete(id, tmux)).collect()
    }

    /// Uppercase `I`: the image's data goes too, not only its placements —
    /// by id, because a delete-all skips the virtual placements the
    /// placeholders draw with. `q=2`: no reply either way; a terminal that
    /// never saw the id (a tmux reattach) has nothing to answer about.
    fn kitty_delete(id: u32, tmux: bool) -> String {
        let (start, escape, end) = Parser::tmux_start_escape_end(tmux);
        format!("{start}{escape}_Ga=d,d=I,i={id},q=2{escape}\\{end}")
    }

    /// Delete, in the terminal, the kitty pictures whose `Graphics` went
    /// away. Called after each frame is written: a picture let go while a
    /// frame was drawing — a queue slot pruned, a wall slot past the page
    /// — is off screen once that frame is out, so its delete never blanks
    /// a cell still showing it. Nothing owed is nothing written.
    pub fn release_dropped() {
        write_deletes(&owed_deletes(false));
    }

    /// Delete every kitty picture this process sent, for leaving the
    /// alternate screen: kitty's own clear on the way out keeps images with
    /// virtual placements, so without this they stay in the window's store
    /// after the player has gone. Before `ratatui::restore` — the images
    /// live in the alternate screen's store, and a delete sent after the
    /// switch asks the main screen's.
    pub fn release_all() {
        write_deletes(&owed_deletes(true));
    }

    fn write_deletes(deletes: &str) {
        if deletes.is_empty() {
            return;
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(deletes.as_bytes());
        let _ = out.flush();
    }

    /// Whether the picker frames its escapes for tmux's passthrough. The
    /// crate decides that when the picker is built and keeps the answer to
    /// itself; an iTerm2 protocol carries it in a public field, so a
    /// one-pixel one reads it back — the same answer sixel is framed by,
    /// without a second opinion that could disagree with it.
    fn wrapped_for_tmux(picker: &Picker) -> bool {
        let mut asking = picker.clone();
        asking.set_protocol_type(ProtocolType::Iterm2);
        let pixel = image::DynamicImage::new_rgb8(1, 1);
        matches!(
            asking.new_protocol(pixel, Size::new(1, 1), Resize::Fit(None)),
            Ok(Protocol::ITerm2(ratatui_image::protocol::iterm2::Iterm2 { is_tmux: true, .. }))
        )
    }

    /// The kitty picture, fitted and resized exactly as
    /// `Picker::new_protocol` does it for `Resize::Scale` — the same cells,
    /// the same pixels, padded the same way — but under `id`, and in two
    /// halves: the placeholders to draw every frame, and the transmission
    /// to send once.
    ///
    /// Upstream's picture keeps its transmission for as long as it lives,
    /// though it hands it out once: a wall page held a page of base64 that
    /// could never be written again — 5-13 MB at 200x60 (performance audit
    /// #95). So the placeholders here are upstream's,
    /// drawn for a one-pixel stand-in whose own transmission is spent on a
    /// scratch cell, and the real one is the caller's to send and drop.
    fn kitty_picture(
        picker: &Picker,
        source: image::DynamicImage,
        fitted: Size,
        resize: &Resize,
        (id, tmux, deflate): (u32, bool, bool),
    ) -> Option<(Protocol, String)> {
        let font = picker.font_size();
        let cells = resize.size_for(&source, font, fitted);
        // No background: the picker's is transparent unless set, and
        // nothing here sets it.
        let pixels = resize.resize(&source, font, cells, None);
        let transmit = kitty_transmit(&pixels, id, tmux, deflate);
        let stand_in = image::DynamicImage::new_rgb8(1, 1);
        let placeholders = Protocol::Kitty(Kitty::new(stand_in, cells, id, tmux).ok()?);
        let scratch = Rect::new(0, 0, 1, 1);
        Image::new(&placeholders).allow_clipping(true).render(scratch, &mut Buffer::empty(scratch));
        Some((placeholders, transmit))
    }

    /// Kitty's transmit-and-place command for `img` under `id`: its pixels
    /// in base64 chunks of 4096 characters, the protocol's ceiling, the
    /// first carrying the image's keys and a virtual placement (U=1) for
    /// the placeholders to draw — upstream's `transmit_virtual`, framed the
    /// same way for tmux, and ours to drop once sent.
    ///
    /// Upstream always sends RGBA, uncompressed: 5.3 bytes on the wire a
    /// pixel, a quarter of them an alpha byte that is constant for a cover
    /// (performance audit #93). An opaque picture goes as RGB (f=24) here —
    /// the pixels being sent decide, not the source's format: a fit that
    /// leaves slack is padded onto transparent, and f=24 would paint that
    /// strip black. And with `deflate` the pixels go through zlib (o=z).
    fn kitty_transmit(img: &image::DynamicImage, id: u32, tmux: bool, deflate: bool) -> String {
        use base64::Engine;
        use std::fmt::Write as _;

        let (w, h) = (img.width(), img.height());
        let (format, pixels) = if opaque(img) {
            (24, img.to_rgb8().into_raw())
        } else {
            (32, img.to_rgba8().into_raw())
        };
        // Level 1, zlib's fastest: over a slow link the bytes are the
        // cost, and past its first level deflate buys a few per cent more
        // for several times the time.
        let (payload, compression) = if deflate {
            (miniz_oxide::deflate::compress_to_vec_zlib(&pixels, 1), "o=z,")
        } else {
            (pixels, "")
        };
        let (start, escape, end) = Parser::tmux_start_escape_end(tmux);
        const CHUNK: usize = 4096 / 4 * 3;
        let chunks = payload.len().div_ceil(CHUNK);
        let per_chunk = start.len() + 2 * escape.len() + 12 + 4096 + end.len();
        let mut data = String::with_capacity(chunks * per_chunk + 48);
        for (i, chunk) in payload.chunks(CHUNK).enumerate() {
            data.push_str(start);
            let _ = write!(data, "{escape}_Gq=2,");
            if i == 0 {
                let _ = write!(data, "i={id},a=T,U=1,f={format},{compression}t=d,s={w},v={h},");
            }
            let _ = write!(data, "m={};", u8::from(i + 1 < chunks));
            base64::engine::general_purpose::STANDARD.encode_string(chunk, &mut data);
            let _ = write!(data, "{escape}\\");
            data.push_str(end);
        }
        data
    }

    /// How hard iTerm2's JPEG covers are compressed. The source was a JPEG
    /// already, typically saved at 80-90; at a cover's size in cells 85
    /// keeps a detailed photograph within 35 dB of upstream's lossless PNG
    /// of the same pixels, at a third to a sixth of the bytes.
    const JPEG_QUALITY: u8 = 85;

    /// The iTerm2 picture of a cover that arrived as a JPEG, sent as one.
    ///
    /// Upstream always sends PNG, and a photograph PNG-encodes to several
    /// times its JPEG: a wall page was ~2 MB of base64 at 200x60, written
    /// on every page turn and carried in the frame's cells — copied and
    /// compared — every frame it stood (performance audit #96). Only art
    /// that was already lossy goes this way: the QR code, the wordmark and
    /// PNG covers keep upstream's lossless encode. Fitted and scaled as
    /// upstream does it (`Resize::Scale`, Triangle), but not padded: JPEG
    /// has no transparent to pad with, and iTerm2 draws the picture at its
    /// own size over the cells cleared for the box — which is all the
    /// transparent strip showed. Framed as upstream frames it, tmux and
    /// the erased cells included.
    fn iterm2_jpeg(
        picker: &Picker,
        source: image::DynamicImage,
        fitted: Size,
        resize: &Resize,
        tmux: bool,
    ) -> Option<Protocol> {
        use base64::Engine;
        use std::fmt::Write as _;

        let font = picker.font_size();
        let cells = resize.size_for(&source, font, fitted);
        let (w, h) = (
            u32::from(cells.width) * u32::from(font.width),
            u32::from(cells.height) * u32::from(font.height),
        );
        let scaled = source.resize(w, h, image::imageops::FilterType::Triangle).into_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
            .encode_image(&scaled)
            .ok()?;
        let (start, escape, end) = Parser::tmux_start_escape_end(tmux);
        let rows = usize::from(cells.height);
        let mut data = String::with_capacity(jpeg.len() / 3 * 4 + 16 * rows + 128);
        data.push_str(start);
        // Upstream's `clear_area` (crate-private): the box's cells erased a
        // row at a time, then back to the top, so no stale text shows
        // through where the picture does not reach.
        if cells.height == 1 {
            let _ = write!(data, "{escape}[{}X", cells.width);
        } else {
            for _ in 0..cells.height {
                let _ = write!(data, "{escape}[{}X{escape}[1B", cells.width);
            }
            let _ = write!(data, "{escape}[{}A", cells.height);
        }
        let _ = write!(
            data,
            "{escape}]1337;File=inline=1;size={};width={}px;height={}px;doNotMoveCursor=1:",
            jpeg.len(),
            scaled.width(),
            scaled.height()
        );
        base64::engine::general_purpose::STANDARD.encode_string(&jpeg, &mut data);
        let _ = write!(data, "\x07{end}");
        Some(Protocol::ITerm2(ratatui_image::protocol::iterm2::Iterm2 {
            data,
            size: cells,
            is_tmux: tmux,
        }))
    }

    /// Whether every pixel of `img` is opaque: no alpha channel, or one at
    /// full everywhere. RGBA8 is the one alpha layout a resize here makes
    /// (the padding); any other is taken at its word.
    fn opaque(img: &image::DynamicImage) -> bool {
        match img {
            image::DynamicImage::ImageRgba8(rgba) => {
                rgba.pixels().all(|pixel| pixel.0[3] == u8::MAX)
            }
            other => !other.color().has_alpha(),
        }
    }

    /// Whether kitty's pixels go deflated (`o=z`). Over ssh a wall page is
    /// megabytes down a link counted in Mbit/s — seconds of frozen screen
    /// at 20 — and zlib's fastest level sends 40-60% of them; locally the
    /// pty moves that page in tens of milliseconds, less than deflating it
    /// would take out of the frame's encode budget (~0.5 ms a wall cover,
    /// ~12 ms a 640 px one — the audit's measurements, #93). And only to
    /// kitty and Ghostty, which inflate: the capability query asks about
    /// f=24, never o=z, and with q=2 a terminal that could not inflate
    /// would show nothing and say nothing. Both name themselves in TERM,
    /// which ssh carries; under tmux TERM is tmux's own, and the pixels go
    /// plain.
    fn deflate_kitty(env: impl Fn(&str) -> Option<String>) -> bool {
        let remote = env("SSH_CONNECTION").is_some() || env("SSH_TTY").is_some();
        let inflates =
            env("TERM").is_some_and(|term| term == "xterm-kitty" || term == "xterm-ghostty");
        remote && inflates
    }

    /// Whether a queried protocol should give way to iTerm2's own.
    ///
    /// True only when the answers evidently came from iTerm2 itself: one
    /// of the variables it sets names it, and no multiplexer sits in
    /// between. Under tmux, zellij or screen the queries were answered by
    /// the multiplexer — demoting on the outer terminal's leftover
    /// environment would pick a protocol the thing actually drawing may
    /// not speak (zellij answers DA1's sixel bit and draws no OSC 1337 at
    /// all).
    /// Apple's Terminal has no pixel protocol to find — probed live on
    /// macOS 26 (Terminal.app 470.2): the kitty query goes unanswered and
    /// is REFLECTED into the screen as literal text (`Gi=31,s=1,...`),
    /// DA1 carries no sixel bit, and there is no iTerm2 protocol — so
    /// asking buys nothing and paints junk into scrollback. TERM_PROGRAM
    /// is set locally by Terminal.app and not forwarded over ssh, and a
    /// multiplexer overwrites it with its own name, so this never masks a
    /// capable terminal on the far side of either. `MSTREAM_GRAPHICS`
    /// still forces a protocol past this, should a future Terminal.app
    /// learn to draw.
    fn skip_query_for_apple_terminal(env: impl Fn(&str) -> Option<String>) -> bool {
        env("TERM_PROGRAM").is_some_and(|v| v == "Apple_Terminal")
    }

    /// Whether a multiplexer — tmux, zellij, or screen — sits between us
    /// and the terminal: the queries were answered by it, the outer
    /// terminal's variables may be stale, and a reattach can put another
    /// terminal behind the same tty.
    fn multiplexed(env: &impl Fn(&str) -> Option<String>) -> bool {
        ["TMUX", "ZELLIJ", "STY"].iter().any(|var| env(var).is_some())
    }

    /// Whether a kitty transmission outlives a terminal resize. In kitty
    /// itself it does: a resize and the full-screen clear ratatui sends
    /// with one pass over the virtual placements the unicode-placeholder
    /// form draws with, and an image is freed only once its last placement
    /// goes (kitty's graphics.c, `clear_filter_func` and `grman_resize`).
    /// Under a multiplexer a reattach can swap the terminal out, and other
    /// terminals that speak the protocol clear by rules of their own — both
    /// keep having the picture sent again.
    ///
    /// kitty sets TERM itself, and ssh carries it. KITTY_WINDOW_ID alone is
    /// weaker: a terminal started from a kitty shell inherits it, so it
    /// counts only while nothing claims TERM_PROGRAM, which such a
    /// terminal sets to its own name. Guessed wrong, this way costs a
    /// resize what it cost before; the other way, blank covers.
    fn images_outlive_resize(
        protocol: ProtocolType,
        env: impl Fn(&str) -> Option<String>,
    ) -> bool {
        let kitty = env("TERM").is_some_and(|t| t == "xterm-kitty")
            || (env("KITTY_WINDOW_ID").is_some() && env("TERM_PROGRAM").is_none());
        protocol == ProtocolType::Kitty && kitty && !multiplexed(&env)
    }

    fn demote_for_iterm2(
        protocol: ProtocolType,
        env: impl Fn(&str) -> Option<String>,
    ) -> bool {
        matches!(protocol, ProtocolType::Kitty | ProtocolType::Sixel)
            && !multiplexed(&env)
            && ["TERM_PROGRAM", "LC_TERMINAL"]
                .iter()
                .any(|var| env(var).is_some_and(|v| v.contains("iTerm")))
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
        // two dimensions cannot round apart. The 65535 cap keeps the
        // fitted size inside upstream's u16 pixel arithmetic even at the
        // font clamp's ceiling — unreachable from today's panels, but a
        // wrapping multiply is not a bug worth leaving a door open for.
        let cells_w = (aw.min(w * ah / h) / fw).min(65535 / fw) as u16;
        let cells_h = (ah.min(h * aw / w) / fh).min(65535 / fh) as u16;
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
        fn the_diagnostics_name_the_protocol_and_the_cell_or_say_there_is_none() {
            // The line graphics-probe prints is the one artifact a
            // cover-looks-wrong report can carry; it must name the
            // protocol and the cell size it was decided with.
            let line = Graphics::forced(ProtocolType::Sixel).diagnostics();
            assert!(line.contains("protocol sixel"), "{line}");
            assert!(line.contains("cell 10x20 px"), "{line}");

            // And the mosaic verdict is the one fixed sentence — no
            // protocol, no cell size, nothing to misread as an answer.
            let line = Graphics::disabled().diagnostics();
            assert!(line.contains("no pixel protocol"), "{line}");
            assert!(!line.contains("cell"), "{line}");
        }

        #[test]
        fn only_kitty_itself_keeps_its_pictures_through_a_resize() {
            let kitty = |var: &str| match var {
                "KITTY_WINDOW_ID" => Some("1".to_string()),
                _ => None,
            };
            assert!(images_outlive_resize(ProtocolType::Kitty, kitty));
            let by_term = |var: &str| match var {
                "TERM" => Some("xterm-kitty".to_string()),
                _ => None,
            };
            assert!(
                images_outlive_resize(ProtocolType::Kitty, by_term),
                "over ssh, TERM is what says so"
            );
            // A reattach can put another terminal behind the tty.
            for mux in ["TMUX", "ZELLIJ", "STY"] {
                let inside = |var: &str| match var {
                    "KITTY_WINDOW_ID" => Some("1".to_string()),
                    v if v == mux => Some("1".to_string()),
                    _ => None,
                };
                assert!(!images_outlive_resize(ProtocolType::Kitty, inside), "{mux}");
            }
            // Another terminal speaking kitty's protocol clears by its own
            // rules; sixel and iTerm2 carry their pixels in the cells.
            let ghostty = |var: &str| match var {
                "TERM" => Some("xterm-ghostty".to_string()),
                "TERM_PROGRAM" => Some("ghostty".to_string()),
                _ => None,
            };
            assert!(!images_outlive_resize(ProtocolType::Kitty, ghostty));
            // One started from a kitty shell inherits KITTY_WINDOW_ID.
            let nested = |var: &str| match var {
                "KITTY_WINDOW_ID" => Some("1".to_string()),
                "TERM" => Some("wezterm".to_string()),
                "TERM_PROGRAM" => Some("WezTerm".to_string()),
                _ => None,
            };
            assert!(!images_outlive_resize(ProtocolType::Kitty, nested));
            assert!(!images_outlive_resize(ProtocolType::Sixel, kitty));
            assert!(!images_outlive_resize(ProtocolType::Iterm2, kitty));
        }

        #[test]
        fn a_resize_sends_kittys_picture_again_only_where_a_reattach_could_lose_it() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            let art = a_cover(64);
            for (outlive, encodes) in [(true, 1), (false, 2)] {
                let mut graphics = Graphics {
                    images_outlive_resize: outlive,
                    ..Graphics::forced(ProtocolType::Kitty)
                };
                let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
                graphics.refresh();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
                assert_eq!(graphics.encodes(), encodes, "outlives a resize: {outlive}");
            }
        }

        /// The image ids the frame's kitty transmissions went out under.
        fn transmitted_ids(
            terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        ) -> Vec<u32> {
            let mut ids = Vec::new();
            for cell in &terminal.backend().buffer().content {
                for (at, _) in cell.symbol().match_indices("_Gq=2,i=") {
                    let digits: String = cell.symbol()[at + 8..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    ids.push(digits.parse().unwrap());
                }
            }
            ids
        }

        #[test]
        fn one_graphics_sends_every_picture_it_ever_draws_under_one_id() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // A new cover, a new size, a resize's re-send: each is a new
            // transmission, and each goes to the id the last one used — so
            // kitty replaces the picture in place instead of keeping both
            // (performance audit #94).
            let (a, b) = (a_cover(64), a_cover(96));
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let mut ids = Vec::new();
            for (art, width) in [(&a, 40), (&b, 40), (&b, 12)] {
                let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), art))).unwrap();
                ids.extend(transmitted_ids(&terminal));
            }
            graphics.refresh();
            let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
            terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &b))).unwrap();
            ids.extend(transmitted_ids(&terminal));
            assert_eq!(graphics.encodes(), 4, "every draw above was a new picture");
            assert_eq!(ids.len(), 4, "{ids:?}");
            assert!(ids.iter().all(|id| *id == ids[0] && *id != 0), "{ids:?}");

            // A fork is another surface, with a picture of its own.
            let mut fork = graphics.fork();
            let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
            terminal.draw(|frame| assert!(fork.draw(frame, frame.area(), &b))).unwrap();
            let forked = transmitted_ids(&terminal);
            assert_eq!(forked.len(), 1);
            assert_ne!(forked[0], ids[0], "a fork shares no id");
        }

        #[test]
        fn a_graphics_let_go_owes_the_terminal_a_delete_and_leaving_owes_the_rest() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            let art = a_cover(64);
            let mut kept = Graphics::forced(ProtocolType::Kitty);
            let mut dropped = kept.fork();
            let id = |graphics: &mut Graphics| {
                let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
                transmitted_ids(&terminal)[0]
            };
            let (kept_id, dropped_id) = (id(&mut kept), id(&mut dropped));
            drop(dropped);

            // Other tests' pictures come and go through the same ledger, so
            // this asks only after its own two.
            let owed = owed_deletes(false);
            assert!(owed.contains(&format!("_Ga=d,d=I,i={dropped_id},q=2")), "{owed:?}");
            assert!(!owed.contains(&format!("i={kept_id},")), "a live picture is not deleted");
            assert!(!owed_deletes(false).contains(&format!("i={dropped_id},")), "owed once");
            // Leaving the alternate screen deletes what is still drawn.
            assert!(owed_deletes(true).contains(&format!("_Ga=d,d=I,i={kept_id},q=2")));
        }

        #[test]
        fn a_delete_is_framed_like_the_transmission_it_undoes() {
            assert_eq!(kitty_delete(7, false), "\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
            // Through tmux: the passthrough wrapper, every ESC inside doubled.
            assert_eq!(
                kitty_delete(7, true),
                "\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=7,q=2\x1b\x1b\\\x1b\\"
            );
        }

        /// A kitty transmission as the terminal reads it: the first chunk's
        /// keys, and the chunks' base64 joined and — for `o=z` — inflated.
        fn received(transmit: &str) -> (String, Vec<u8>) {
            use base64::Engine;
            let (mut keys, mut payload) = (String::new(), Vec::new());
            for command in transmit.split("\x1b_G").skip(1) {
                let command = command.split("\x1b\\").next().unwrap();
                let (control, data) = command.split_once(';').unwrap();
                assert!(data.len() <= 4096, "a chunk past the protocol's ceiling");
                if keys.is_empty() {
                    keys = control.to_string();
                }
                payload.extend(base64::engine::general_purpose::STANDARD.decode(data).unwrap());
            }
            if keys.contains("o=z") {
                payload = miniz_oxide::inflate::decompress_to_vec_zlib(&payload).unwrap();
            }
            (keys, payload)
        }

        /// Received pixels as RGBA, whichever format they came in.
        fn rgba(keys: &str, pixels: Vec<u8>) -> Vec<u8> {
            if !keys.contains("f=24") {
                return pixels;
            }
            pixels.chunks(3).flat_map(|p| [p[0], p[1], p[2], u8::MAX]).collect()
        }

        #[test]
        fn opaque_pixels_go_as_rgb_and_only_the_fits_padding_keeps_its_alpha() {
            // The alpha byte was a constant quarter of every opaque cover
            // on the wire (performance audit #93).
            let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
                image::Rgb([x as u8, y as u8, 7])
            }));
            let (keys, pixels) = received(&kitty_transmit(&rgb, 9, false, false));
            assert!(keys.contains("f=24,t=d,s=30,v=20,") && !keys.contains("o=z"), "{keys}");
            assert_eq!(pixels, rgb.to_rgb8().into_raw());
            // RGBA that is opaque everywhere is RGB on the wire too.
            let solid = image::DynamicImage::ImageRgba8(rgb.to_rgba8());
            let (keys, pixels) = received(&kitty_transmit(&solid, 9, false, false));
            assert!(keys.contains("f=24"), "{keys}");
            assert_eq!(pixels, rgb.to_rgb8().into_raw());
            // The fit's padding is transparent, and stays so: f=24 would
            // paint the strip black over the ground.
            let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
            let font = ratatui_image::FontSize::new(13, 27);
            let padded = resize.resize(&rgb, font, Size::new(12, 6), None);
            assert!(padded.to_rgba8().pixels().any(|pixel| pixel.0[3] == 0), "the test's premise");
            let (keys, pixels) = received(&kitty_transmit(&padded, 9, false, false));
            assert!(keys.contains("f=32"), "{keys}");
            assert_eq!(pixels, padded.to_rgba8().into_raw());
        }

        #[test]
        fn deflated_pixels_inflate_to_exactly_what_was_drawn_in_fewer_bytes() {
            // Something photographic enough to compress like a cover does.
            let cover = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(192, 192, |x, y| {
                let v = ((x * x + y * 3) / 7) as u8;
                image::Rgb([v, v.wrapping_add((y / 3) as u8), 255 - v])
            }));
            let plain = kitty_transmit(&cover, 5, false, false);
            let deflated = kitty_transmit(&cover, 5, false, true);
            let (keys, pixels) = received(&deflated);
            assert!(keys.starts_with("q=2,i=5,a=T,U=1,f=24,o=z,t=d,s=192,v=192,"), "{keys}");
            assert_eq!(pixels, cover.to_rgb8().into_raw());
            assert!(deflated.len() < plain.len() * 3 / 4, "{} vs {}", deflated.len(), plain.len());
            // Through tmux, every chunk is wrapped the way the rest are.
            let wrapped = kitty_transmit(&cover, 5, true, true);
            let chunks = wrapped.matches("\x1bPtmux;\x1b\x1b_Gq=2,").count();
            assert_eq!(chunks, deflated.matches("\x1b_Gq=2,").count());
            assert_eq!(wrapped.matches("\x1b\x1b\\\x1b\\").count(), chunks);
        }

        #[test]
        fn pixels_go_deflated_only_over_ssh_to_a_terminal_known_to_inflate() {
            type Env = &'static [(&'static str, &'static str)];
            fn env(pairs: Env) -> impl Fn(&str) -> Option<String> {
                move |var| pairs.iter().find(|(name, _)| *name == var).map(|(_, v)| v.to_string())
            }
            const SSH: (&str, &str) = ("SSH_CONNECTION", "10.0.0.2 50000 10.0.0.1 22");
            const TTY: (&str, &str) = ("SSH_TTY", "/dev/ttys003");
            assert!(deflate_kitty(env(&[SSH, ("TERM", "xterm-kitty")])));
            assert!(deflate_kitty(env(&[TTY, ("TERM", "xterm-ghostty")])));
            // Locally the pty moves a page faster than zlib compresses it.
            assert!(!deflate_kitty(env(&[("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1")])));
            // Nobody known to inflate: under tmux TERM is tmux's own.
            assert!(!deflate_kitty(env(&[TTY, ("TERM", "tmux-256color")])));
            assert!(!deflate_kitty(env(&[TTY, ("TERM", "xterm-256color")])));

            // The answer rides with the Graphics, forks included.
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;
            let art = a_cover(64);
            let remote = Graphics { deflate: true, ..Graphics::forced(ProtocolType::Kitty) };
            let local = Graphics::forced(ProtocolType::Kitty);
            for (mut graphics, deflated) in [(remote.fork(), true), (local, false)] {
                let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
                let sent: String =
                    terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
                assert_eq!(sent.contains(",o=z,"), deflated, "deflated: {deflated}");
            }
        }

        #[test]
        fn the_kitty_picture_is_the_one_the_picker_would_have_built() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            /// Every cell of one frame drawing `protocol`, in order.
            fn frame_of(terminal: &mut Terminal<TestBackend>, protocol: &Protocol) -> String {
                terminal
                    .draw(|frame| frame.render_widget(Image::new(protocol), frame.area()))
                    .unwrap();
                terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
            }

            // Same cells, same pixels: only the id and the wire format
            // differ. A square cover that fits its box exactly, a banner,
            // and a cover the fit leaves transparent padding beside.
            let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
            let cases = [
                ((10, 20), (128, 128), (12, 6)),
                ((10, 20), (400, 201), (30, 9)),
                ((13, 27), (128, 65), (12, 6)),
            ];
            let mut formats = Vec::new();
            for (font, (side, tall), fitted) in cases {
                #[allow(deprecated)]
                let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(font.0, font.1));
                picker.set_protocol_type(ProtocolType::Kitty);
                let pixels = image::RgbImage::from_fn(side, tall, |x, y| {
                    image::Rgb([(x * 7) as u8, (y * 13) as u8, (x ^ y) as u8])
                });
                let source = image::DynamicImage::ImageRgb8(pixels);
                let fitted = Size::new(fitted.0, fitted.1);
                let (ours, transmit) =
                    kitty_picture(&picker, source.clone(), fitted, &resize, (42, false, false))
                        .unwrap();
                let theirs = picker.new_protocol(source.clone(), fitted, resize.clone()).unwrap();
                assert_eq!(ours.size(), theirs.size(), "{font:?} {side}");

                // The picker's picture sends its transmission ahead of the
                // first row's placeholders; ours is the same picture, pixel
                // for pixel, whichever format carries it.
                let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
                let first = frame_of(&mut terminal, &theirs);
                let (their_keys, their_pixels) = received(first.split("\x1b[s").next().unwrap());
                let (our_keys, our_pixels) = received(&transmit);
                assert_eq!(rgba(&our_keys, our_pixels), their_pixels, "{font:?} {side}");
                let size = |keys: &str| keys.split(",t=d,").nth(1).unwrap().to_string();
                assert_eq!(size(&our_keys), size(&their_keys), "{font:?} {side}");
                // RGB where every pixel is opaque, RGBA where the fit padded.
                let padded = their_pixels.chunks(4).any(|pixel| pixel[3] != u8::MAX);
                assert_eq!(our_keys.contains("f=32"), padded, "{font:?} {side}: {our_keys}");
                assert_eq!(our_keys.contains("f=24"), !padded, "{font:?} {side}: {our_keys}");
                formats.push(padded);

                // And the placeholders ours draws every frame are the ones
                // upstream's own draws once its transmission is spent.
                let cells = ours.size();
                let pixels = resize.resize(&source, picker.font_size(), cells, None);
                let upstream = Protocol::Kitty(Kitty::new(pixels, cells, 42, false).unwrap());
                let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
                frame_of(&mut terminal, &upstream);
                let mut again = Terminal::new(TestBackend::new(40, 12)).unwrap();
                let mut spent = Terminal::new(TestBackend::new(40, 12)).unwrap();
                assert_eq!(frame_of(&mut spent, &ours), frame_of(&mut again, &upstream));
                assert!(!frame_of(&mut spent, &ours).contains("a=T"), "the stand-in sends nothing");
            }
            assert!(formats.contains(&true) && formats.contains(&false), "{formats:?}");
        }

        #[test]
        fn a_kitty_picture_lets_go_of_its_pixels_once_the_frame_carries_them() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            let art = a_cover(64);
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let symbols = |terminal: &Terminal<TestBackend>| -> String {
                terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
            };
            // The first frame carries them, once, ahead of the placeholders.
            let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
            terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
            let first = symbols(&terminal);
            assert_eq!(first.matches("a=T").count(), 1, "one transmission");
            assert!(first.find("a=T") < first.find('\u{10EEEE}'), "ahead of what shows it");
            let held = graphics.cached.as_ref().unwrap();
            assert!(held.transmit.is_none(), "and not kept once sent");

            // Every later frame is the placeholders alone, drawn warm.
            let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
            terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), &art))).unwrap();
            let later = symbols(&terminal);
            assert!(!later.contains("_G"), "nothing sent again");
            assert_eq!(later.matches('\u{10EEEE}').count(), first.matches('\u{10EEEE}').count());
            assert_eq!(graphics.encodes(), 1);
        }

        /// A cover that arrived as a JPEG, as most do.
        fn a_jpeg_cover(width: u32, height: u32) -> crate::tui::art::Art {
            let pixels = image::RgbImage::from_fn(width, height, |x, y| {
                let v = ((x * x + y * 3) / 7) as u8;
                image::Rgb([v, v.wrapping_add((y / 3) as u8), 255 - v])
            });
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
                .encode_image(&pixels)
                .unwrap();
            crate::tui::art::decode(&bytes.into_inner()).unwrap()
        }

        /// The iTerm2 escape a frame carries: everything before the image
        /// data, the image's bytes, and what follows them.
        fn iterm2_sent(data: &str) -> (String, Vec<u8>, String) {
            use base64::Engine;
            let (head, rest) = data.split_once("doNotMoveCursor=1:").unwrap();
            let (payload, tail) = rest.split_once('\x07').unwrap();
            let bytes = base64::engine::general_purpose::STANDARD.decode(payload).unwrap();
            (head.to_string(), bytes, tail.to_string())
        }

        #[test]
        fn a_jpeg_cover_goes_to_iterm2_as_a_jpeg_and_lossless_art_stays_png() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // A photograph PNG-encodes to several times its JPEG, and the
            // payload is written on every page turn and carried in the
            // frame's cells (performance audit #96).
            let frame_sent = |art: &crate::tui::art::Art| {
                let mut graphics = Graphics::forced(ProtocolType::Iterm2);
                let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
                terminal.draw(|frame| assert!(graphics.draw(frame, frame.area(), art))).unwrap();
                let sent: String =
                    terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
                iterm2_sent(&sent)
            };
            let (head, jpeg, _) = frame_sent(&a_jpeg_cover(300, 300));
            assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) && jpeg.ends_with(&[0xFF, 0xD9]));
            assert!(head.contains(&format!("size={};", jpeg.len())), "{head:?}");
            let decoded = image::load_from_memory(&jpeg).unwrap();
            let (w, h) = (decoded.width(), decoded.height());
            assert!(head.contains(&format!("width={w}px;height={h}px;")), "{head:?}");
            // The QR code, the wordmark and PNG covers stay lossless.
            let (_, png, _) = frame_sent(&a_cover(300));
            assert!(png.starts_with(b"\x89PNG"), "lossless art stays PNG");
        }

        #[test]
        fn the_iterm2_jpeg_stands_where_upstreams_png_stood() {
            // The same cells, the same erased box ahead of it, the same
            // framing — the tmux wrapper included. Not padded: the picture
            // is its own size, over the cells the box cleared, which is all
            // upstream's transparent strip showed.
            let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(10, 20));
            picker.set_protocol_type(ProtocolType::Iterm2);
            for (width, height) in [(300, 300), (300, 150), (150, 300)] {
                let art = a_jpeg_cover(width, height);
                let source = image::load_from_memory(art.source()).unwrap();
                let fitted = Size::new(24, 8);
                let ours = iterm2_jpeg(&picker, source.clone(), fitted, &resize, false).unwrap();
                let theirs = picker.new_protocol(source, fitted, resize.clone()).unwrap();
                assert_eq!(ours.size(), theirs.size(), "{width}x{height}");
                let (Protocol::ITerm2(ours), Protocol::ITerm2(theirs)) = (ours, theirs) else {
                    panic!("iTerm2 both")
                };
                let (our_head, jpeg, our_tail) = iterm2_sent(&ours.data);
                let (their_head, png, their_tail) = iterm2_sent(&theirs.data);
                let erased = |head: &str| head.split("\x1b]1337;").next().unwrap().to_string();
                assert_eq!(erased(&our_head), erased(&their_head), "{width}x{height}");
                assert_eq!(our_tail, their_tail);
                assert!(jpeg.len() * 2 < png.len(), "{} vs {}", jpeg.len(), png.len());
                // The picture itself is the unpadded scale of the same fit.
                let shown = image::load_from_memory(&jpeg).unwrap();
                let padded = image::load_from_memory(&png).unwrap().to_rgba8();
                let (w, h) = (shown.width(), shown.height());
                assert!(w <= padded.width() && h <= padded.height());
                assert!(w == padded.width() || h == padded.height(), "one side meets the box");
                let beyond = padded.enumerate_pixels().filter(|(x, y, _)| *x >= w || *y >= h);
                assert!(beyond.clone().all(|(_, _, pixel)| pixel.0[3] == 0), "padding past it");
            }
            let art = a_jpeg_cover(300, 300);
            let source = image::load_from_memory(art.source()).unwrap();
            let Some(Protocol::ITerm2(wrapped)) =
                iterm2_jpeg(&picker, source, Size::new(24, 8), &resize, true)
            else {
                panic!("iTerm2")
            };
            assert!(wrapped.data.starts_with("\x1bPtmux;\x1b\x1b["), "{:?}", &wrapped.data[..20]);
            assert!(wrapped.data.ends_with("\x07\x1b\\"));
            assert!(wrapped.is_tmux);
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
            // The thumbnail's shape says so before any decode — and the
            // refusal is remembered, where it used to be rediscovered
            // every frame, thirty a second, for as long as the view stayed
            // up with the mosaic drawing over the waste.
            let art = a_banner();
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let mut terminal = Terminal::new(TestBackend::new(6, 3)).unwrap();
            for _ in 0..3 {
                terminal
                    .draw(|frame| assert!(!graphics.draw(frame, frame.area(), &art)))
                    .unwrap();
            }
            assert_eq!(graphics.decodes.get(), 0, "the thumbnail's shape answers without a decode");
            assert_eq!(graphics.encodes.get(), 0);

            // A resize is a different question and earns a fresh attempt —
            // a 240px-wide box wants more than the thumbnail's 128, so
            // this one decodes the source.
            let mut terminal = Terminal::new(TestBackend::new(24, 12)).unwrap();
            let mut drew = false;
            terminal.draw(|frame| drew = graphics.draw(frame, frame.area(), &art)).unwrap();
            assert!(drew, "the banner fits a real panel");
            assert_eq!(graphics.decodes.get(), 1);
            assert_eq!(graphics.encodes.get(), 1);

            // Bytes that never decode are refused at every size that needs
            // them for the price of one failed attempt (a from_rgb art has
            // no source, and two pixels fill no box).
            let pixels = crate::tui::art::Art::from_rgb(2, 2, vec![0; 12]).unwrap();
            for size in [(6, 3), (24, 12)] {
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                terminal
                    .draw(|frame| assert!(!graphics.draw(frame, frame.area(), &pixels)))
                    .unwrap();
            }
            assert_eq!(graphics.decodes.get(), 2, "undecodable bytes are asked exactly once");
        }

        #[test]
        fn a_small_box_draws_from_the_thumbnail_and_a_moved_box_re_encodes_nothing() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // A 400px cover into a 6x3 box: 60px a side, well inside the
            // thumbnail — no decode, one encode.
            let art = a_cover(400);
            let mut graphics = Graphics::forced(ProtocolType::Kitty);
            let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
            let mut drew = false;
            terminal
                .draw(|frame| {
                    drew = graphics.draw(frame, Rect { x: 0, y: 0, width: 6, height: 3 }, &art)
                })
                .unwrap();
            assert!(drew);
            assert_eq!((graphics.decodes.get(), graphics.encodes.get()), (0, 1));

            // The same box elsewhere on the screen — a scrolled row — is
            // the warm cache drawn at the new place.
            terminal
                .draw(|frame| {
                    drew = graphics.draw(frame, Rect { x: 20, y: 6, width: 6, height: 3 }, &art)
                })
                .unwrap();
            assert!(drew);
            assert_eq!(graphics.encodes.get(), 1, "a move costs no encode");
            let placed = terminal.backend().buffer()[(20, 6)].symbol().contains('\u{10EEEE}');
            assert!(placed, "the placeholders stand at the new place");

            // A box the thumbnail cannot fill decodes the source.
            terminal
                .draw(|frame| drew = graphics.draw(frame, frame.area(), &art))
                .unwrap();
            assert!(drew);
            assert_eq!((graphics.decodes.get(), graphics.encodes.get()), (1, 2));
        }

        /// What a queue row's cover costs to encode, by protocol, against
        /// what the source decode alone used to cost every encode. Run
        /// with `cargo test cover_encode_costs -- --ignored --nocapture`,
        /// and again with `--release` for the shipped numbers.
        #[test]
        #[ignore]
        fn cover_encode_costs() {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;

            // A 400x400 jpeg with detail in it, as covers actually ship.
            let mut pixels = image::RgbImage::new(400, 400);
            for (x, y, pixel) in pixels.enumerate_pixels_mut() {
                let v = ((x * 7) ^ (y * 13)) as u8;
                *pixel = image::Rgb([v, v.wrapping_mul(3), 255 - v]);
            }
            let mut jpeg = std::io::Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .encode_image(&pixels)
                .unwrap();
            let jpeg = jpeg.into_inner();
            let art = crate::tui::art::decode(&jpeg).unwrap();

            let start = std::time::Instant::now();
            for _ in 0..10 {
                let _ = image::load_from_memory(&jpeg).unwrap();
            }
            let decode = start.elapsed() / 10;
            eprintln!(
                "decoding the {} KB source jpeg: {decode:?} — what every encode paid before",
                jpeg.len() / 1024
            );

            for protocol in [ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
                let mut graphics = Graphics::forced(protocol);
                let mut terminal = Terminal::new(TestBackend::new(6, 3)).unwrap();
                let start = std::time::Instant::now();
                terminal
                    .draw(|frame| assert!(graphics.draw(frame, frame.area(), &art)))
                    .unwrap();
                let encode = start.elapsed();
                let start = std::time::Instant::now();
                terminal
                    .draw(|frame| assert!(graphics.draw(frame, frame.area(), &art)))
                    .unwrap();
                let again = start.elapsed();
                eprintln!(
                    "{protocol:?}: {encode:?} to encode a 6x3 cover from the thumbnail ({} decodes), {again:?} to draw it again",
                    graphics.decodes.get()
                );
            }

            // What deflating a kitty transmission adds, and saves, at a
            // wall cover's size and at a large Now Playing cover's.
            let source = image::load_from_memory(&jpeg).unwrap();
            for side in [120, 192, 400] {
                let pixels = source.resize_exact(side, side, image::imageops::FilterType::Triangle);
                let start = std::time::Instant::now();
                let plain = kitty_transmit(&pixels, 1, false, false);
                let fast = start.elapsed();
                let start = std::time::Instant::now();
                let deflated = kitty_transmit(&pixels, 1, false, true);
                let slow = start.elapsed();
                eprintln!(
                    "kitty {side}px: {} bytes in {fast:?}, deflated {} bytes in {slow:?}",
                    plain.len(),
                    deflated.len()
                );
            }

            // iTerm2: upstream's PNG against the JPEG a JPEG cover now goes
            // as, a wall cell's box and a Now Playing cover's.
            let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(10, 20));
            picker.set_protocol_type(ProtocolType::Iterm2);
            for cells in [Size::new(12, 6), Size::new(42, 21)] {
                let start = std::time::Instant::now();
                let png = picker.new_protocol(source.clone(), cells, resize.clone()).unwrap();
                let slow = start.elapsed();
                let start = std::time::Instant::now();
                let jpeg = iterm2_jpeg(&picker, source.clone(), cells, &resize, false).unwrap();
                let fast = start.elapsed();
                let bytes = |protocol: &Protocol| match protocol {
                    Protocol::ITerm2(iterm2) => iterm2.data.len(),
                    _ => 0,
                };
                eprintln!(
                    "iTerm2 {}x{} cells: PNG {} bytes in {slow:?}, JPEG {} bytes in {fast:?}",
                    cells.width,
                    cells.height,
                    bytes(&png),
                    bytes(&jpeg)
                );
            }
        }

        #[test]
        fn the_query_is_skipped_only_for_a_local_apple_terminal() {
            let apple = |var: &str| match var {
                "TERM_PROGRAM" => Some("Apple_Terminal".to_string()),
                _ => None,
            };
            assert!(skip_query_for_apple_terminal(apple));
            // iTerm2 and tmux both claim TERM_PROGRAM as their own name,
            // and over ssh the variable is simply absent — the skip fires
            // for none of them.
            for name in ["iTerm.app", "tmux", "WezTerm", "ghostty"] {
                let other = |var: &str| match var {
                    "TERM_PROGRAM" => Some(name.to_string()),
                    _ => None,
                };
                assert!(!skip_query_for_apple_terminal(other), "{name}");
            }
            assert!(!skip_query_for_apple_terminal(|_| None), "ssh: no TERM_PROGRAM");
        }


        #[test]
        fn the_demotion_knows_the_real_iterm2_from_its_leftover_environment() {
            let iterm = |var: &str| match var {
                "LC_TERMINAL" => Some("iTerm2".to_string()),
                _ => None,
            };
            assert!(demote_for_iterm2(ProtocolType::Kitty, iterm));
            assert!(demote_for_iterm2(ProtocolType::Sixel, iterm));
            // Its own protocol needs no demoting; halfblocks never got in.
            assert!(!demote_for_iterm2(ProtocolType::Iterm2, iterm));
            assert!(!demote_for_iterm2(ProtocolType::Halfblocks, iterm));

            // Under any multiplexer the queries were answered by the
            // multiplexer, and the outer terminal's leftover environment
            // must not outvote what it said — zellij answers DA1's sixel
            // bit and draws no OSC 1337 at all.
            for mux in ["TMUX", "ZELLIJ", "STY"] {
                let inside = |var: &str| match var {
                    "LC_TERMINAL" => Some("iTerm2".to_string()),
                    v if v == mux => Some("1".to_string()),
                    _ => None,
                };
                assert!(!demote_for_iterm2(ProtocolType::Kitty, inside), "{mux}");
            }

            // A terminal that never said iTerm keeps what it answered.
            assert!(!demote_for_iterm2(ProtocolType::Kitty, |_| None));
            let wezterm = |var: &str| match var {
                "TERM_PROGRAM" => Some("WezTerm".to_string()),
                _ => None,
            };
            assert!(!demote_for_iterm2(ProtocolType::Kitty, wezterm));
        }

        #[test]
        fn a_fit_never_escapes_upstreams_pixel_arithmetic() {
            // A thousand cells at the font clamp's 128px ceiling would be
            // 128000 pixels a side — far past the u16 upstream multiplies
            // in. The cap trades cells no panel has for a multiply that
            // cannot wrap.
            let (w, h) = fit(area(1000, 1000), (128, 128), 4000, 4000);
            assert_eq!((w, h), (511, 511));
            assert!(u32::from(w) * 128 <= 65535 && u32::from(h) * 128 <= 65535);
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
