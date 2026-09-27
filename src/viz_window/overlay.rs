//! egui in the visualizer window (contract clause 9): its input from
//! winit, its pass once a frame, and its paint on the window's own device —
//! after the blit, over the picture, in the same submission.

use std::path::PathBuf;
use std::sync::Arc;

use winit::event::WindowEvent;
use winit::window::Window;

use crate::shader::render::Gpu;

pub struct Overlay {
    pub ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
}

impl Overlay {
    /// `format` is the surface's: egui draws straight onto it.
    pub fn new(window: &Window, gpu: &Gpu, format: wgpu::TextureFormat) -> Overlay {
        let ctx = context();
        let max_texture = gpu.device.limits().max_texture_dimension_2d as usize;
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_texture),
        );
        Overlay { ctx, state, painter: Painter::new(gpu, format) }
    }

    /// A window event, egui's to see first. `true` when egui keeps it for
    /// itself — a key while one of its widgets has the keyboard — and the
    /// window's own keys must not act on it.
    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// One pass of the UI, on what winit has said since the last; its paint
    /// waits for [`Overlay::paint`].
    pub fn run(&mut self, window: &Window, gpu: &Gpu, ui: impl FnMut(&mut egui::Ui)) {
        let input = self.state.take_egui_input(window);
        let mut output = self.ctx.run_ui(input, ui);
        self.state.handle_platform_output(window, std::mem::take(&mut output.platform_output));
        self.painter.take(&self.ctx, gpu, output);
    }

    pub fn paint(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
    ) -> Vec<wgpu::CommandBuffer> {
        self.painter.paint(gpu, encoder, view, size_in_pixels)
    }

    pub fn release(&mut self) {
        self.painter.release();
    }
}

/// egui's context for the window: its fonts and its look. The tests lay
/// the controls out with the same, so what they click is where it is drawn.
pub(super) fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts(&ctx);
    super::controls::style(&ctx);
    ctx
}

/// egui's paint on a wgpu device: a pass's output taken in, and recorded
/// onto a view when a frame has one.
struct Painter {
    renderer: egui_wgpu::Renderer,
    /// The last pass's paint, for the frame it lands on.
    jobs: Vec<egui::ClippedPrimitive>,
    pixels_per_point: f32,
    /// Textures egui has finished with, let go once a frame is submitted.
    finished: Vec<egui::TextureId>,
}

impl Painter {
    fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Painter {
        let renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        Painter { renderer, jobs: Vec::new(), pixels_per_point: 1.0, finished: Vec::new() }
    }

    /// A pass's output. Its texture changes are uploaded now, shown or not:
    /// the font atlas grows by deltas, and one dropped is a glyph lost for
    /// good.
    fn take(&mut self, ctx: &egui::Context, gpu: &Gpu, mut output: egui::FullOutput) {
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in &deltas {
                self.renderer.update_texture(&gpu.device, &gpu.queue, id, delta);
            }
        }
        self.finished.extend(output.textures_delta.free.drain());
        self.pixels_per_point = output.pixels_per_point;
        self.jobs = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    }

    /// The last pass, recorded onto `view` over what is already there —
    /// nothing at all while the controls are away, not even the pass.
    /// Returns what egui's own callbacks want submitted first (none today).
    fn paint(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
    ) -> Vec<wgpu::CommandBuffer> {
        if self.jobs.is_empty() {
            return Vec::new();
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels, pixels_per_point: self.pixels_per_point };
        let first = self.renderer.update_buffers(&gpu.device, &gpu.queue, encoder, &self.jobs, &screen);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("controls"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.renderer.render(&mut pass.forget_lifetime(), &self.jobs, &screen);
        first
    }

    /// After a submission: the textures egui has finished with.
    fn release(&mut self) {
        for id in self.finished.drain(..) {
            self.renderer.free_texture(&id);
        }
    }
}

/// egui's two text faces carry Latin, Greek and Cyrillic — eight of the
/// ten languages — and Hack the arrows the tooltips name keys with, so each
/// family falls back on the other. Japanese and Chinese borrow the system's
/// CJK face, checked the way egui will read it (egui panics on a face it
/// cannot parse), set a touch lower to sit on the Latin baseline; where the
/// system has none, the controls speak English rather than boxes.
fn fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::empty();
    let own = [("Ubuntu-Light", epaint_default_fonts::UBUNTU_LIGHT), ("Hack", epaint_default_fonts::HACK_REGULAR)];
    for (name, face) in own {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(face)));
    }
    fonts.families.insert(FontFamily::Proportional, vec!["Ubuntu-Light".into(), "Hack".into()]);
    fonts.families.insert(FontFamily::Monospace, vec!["Hack".into(), "Ubuntu-Light".into()]);
    let lang = rust_i18n::locale().to_string();
    let faces = cjk_faces(&lang);
    if !faces.is_empty() {
        let borrowed = faces.into_iter().find_map(|(path, index)| {
            let bytes = std::fs::read(&path).ok()?;
            if skrifa::FontRef::from_index(&bytes, index).is_err() {
                eprintln!("viz-window: {} is not a face egui can read", path.display());
                return None;
            }
            let mut face = FontData::from_owned(bytes);
            face.index = index;
            face.tweak.y_offset_factor = 0.12;
            Some(face)
        });
        match borrowed {
            Some(face) => {
                fonts.font_data.insert("system-cjk".into(), Arc::new(face));
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.entry(family).or_default().push("system-cjk".into());
                }
            }
            None => {
                eprintln!("viz-window: no system font draws {lang}; the controls speak English");
                rust_i18n::set_locale("en");
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// A platform's faces for a language, best first: a path and the face's
/// index in its collection.
type Faces = Vec<(&'static str, u32)>;

/// Where each platform keeps a face for the language, best first, with the
/// face's index in a collection. Noto's CJK collection holds JP, KR, SC,
/// TC and HK in that order, so Chinese asks for the third.
fn cjk_faces(lang: &str) -> Vec<(PathBuf, u32)> {
    let (ja, zh): (Faces, Faces) = if cfg!(target_os = "macos") {
        (
            vec![
                ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
                ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
                ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
            ],
            vec![
                ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
                ("/System/Library/Fonts/STHeiti Light.ttc", 0),
                ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
            ],
        )
    } else if cfg!(windows) {
        (
            vec![("Fonts/YuGothR.ttc", 0), ("Fonts/meiryo.ttc", 0), ("Fonts/msgothic.ttc", 0)],
            vec![("Fonts/msyh.ttc", 0), ("Fonts/simsun.ttc", 0), ("Fonts/simhei.ttf", 0)],
        )
    } else {
        let noto = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        ];
        let fallback = [
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
            ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0),
            ("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf", 0),
        ];
        (
            noto.iter().map(|p| (*p, 0)).chain(fallback).collect(),
            noto.iter().map(|p| (*p, 2)).chain(fallback).collect(),
        )
    };
    let chosen = match lang {
        "ja" => ja,
        "zh" => zh,
        _ => return Vec::new(),
    };
    // Windows' font folder is under wherever Windows is.
    let windows =
        std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    chosen
        .into_iter()
        .map(|(path, index)| (if cfg!(windows) { windows.join(path) } else { PathBuf::from(path) }, index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, ViewportId, ViewportInfo, vec2};

    use crate::config::VisualizerPrefs;
    use crate::runtime::block_on;
    use crate::shader::audio::AudioTexture;
    use crate::shader::library::BUILTIN;
    use crate::shader::preset::Preset;
    use crate::shader::render::{FORMAT, Offscreen};
    use crate::viz_window::controls::{self, Controls, Tuning, View};

    #[derive(Clone, Copy)]
    enum Shown {
        /// The bar, a tooltip up.
        Bar,
        /// The dropdown open, a row under the pointer.
        Dropdown,
        /// The tuning panel.
        Tuning,
    }

    /// The controls drawn over a preset on this machine's GPU, in the states
    /// a screenshot of the real window cannot reach — synthetic clicks need
    /// the Accessibility permission — at a Retina scale: the bar, the open
    /// dropdown, the tuning panel, and the panel in Russian, Japanese and
    /// Chinese for their faces. PNGs go to `$VIZ_OVERLAY_PNG`, or the temp
    /// directory's `viz-overlay`.
    #[test]
    #[ignore = "needs a GPU; run with --ignored --nocapture and look at the PNGs"]
    fn the_controls_drawn_over_a_preset() {
        let _locale = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::var_os("VIZ_OVERLAY_PNG")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("viz-overlay"));
        std::fs::create_dir_all(&dir).unwrap();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        };
        let adapter = block_on(instance.request_adapter(&options)).unwrap().expect("a GPU");
        let gpu = Gpu::new(&adapter).unwrap();
        const SCALE: f32 = 2.0;
        let points = vec2(960.0, 540.0);
        let size = ((points.x * SCALE) as u32, (points.y * SCALE) as u32);

        let shots = [
            ("bar", "en", 1, Shown::Bar),
            ("dropdown", "en", 1, Shown::Dropdown),
            ("tuning", "en", 3, Shown::Tuning),
            ("tuning-ru", "ru", 0, Shown::Tuning),
            ("tuning-ja", "ja", 2, Shown::Tuning),
            ("tuning-zh", "zh", 6, Shown::Tuning),
        ];
        for (name, lang, current, shown) in shots {
            rust_i18n::set_locale(lang);
            let ctx = context();
            let entries = controls::entries();
            let mut tuning = Tuning::from_prefs(&VisualizerPrefs::default(), &entries);
            let mut state = Controls::default();
            state.key_hints = true;
            let mut painter = Painter::new(&gpu, FORMAT);
            let mut time = 0.0;
            let mut pass = |state: &mut Controls, tuning: &mut Tuning, events: Vec<Event>| {
                let mut viewports = egui::ViewportIdMap::default();
                viewports.insert(
                    ViewportId::ROOT,
                    ViewportInfo { native_pixels_per_point: Some(SCALE), ..ViewportInfo::default() },
                );
                let input = RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, points)),
                    viewports,
                    time: Some(time),
                    events,
                    ..RawInput::default()
                };
                let view = View { entries: &entries, current, fullscreen: false };
                let output = ctx.run_ui(input, |ui| {
                    controls::show(ui, state, &view, tuning);
                });
                painter.take(&ctx, &gpu, output);
                time += 1.0 / 60.0;
            };
            let press = |pos, pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            for _ in 0..10 {
                pass(&mut state, &mut tuning, Vec::new());
            }
            match shown {
                Shown::Bar => {
                    let next = state.seen.next.unwrap().center();
                    pass(&mut state, &mut tuning, vec![Event::PointerMoved(next)]);
                }
                Shown::Dropdown => {
                    let pick = state.seen.pick.unwrap().center();
                    pass(&mut state, &mut tuning, vec![Event::PointerMoved(pick)]);
                    pass(&mut state, &mut tuning, vec![press(pick, true)]);
                    pass(&mut state, &mut tuning, vec![press(pick, false)]);
                    pass(&mut state, &mut tuning, Vec::new());
                    let row = state.seen.rows[4].center();
                    pass(&mut state, &mut tuning, vec![Event::PointerMoved(row)]);
                }
                Shown::Tuning => {
                    state.toggle_tuning();
                    pass(&mut state, &mut tuning, vec![Event::PointerMoved(Pos2::new(40.0, 40.0))]);
                }
            }
            for _ in 0..50 {
                pass(&mut state, &mut tuning, Vec::new());
            }

            // The preset under it, a couple of seconds into a groove.
            let preset = Preset::parse(BUILTIN[current].source).unwrap();
            let mut scene = gpu.load(&preset, size).unwrap();
            scene.set_params(&tuning.knobs[current]);
            let target = Offscreen::new(&gpu, size);
            let mut texture = AudioTexture::new();
            for frame in 0..120 {
                let t = frame as f32 / 60.0;
                let samples: Vec<f32> = (0..1024)
                    .map(|i| {
                        let s = t + i as f32 / 44_100.0;
                        let kick = (-((t * 2.0).fract()) * 6.0).exp();
                        0.5 * kick * (s * 55.0 * std::f32::consts::TAU).sin()
                            + 0.1 * (s * 440.0 * std::f32::consts::TAU).sin()
                            + 0.05 * (s * 3_520.0 * std::f32::consts::TAU).sin()
                    })
                    .collect();
                gpu.upload_audio(texture.update(&samples, 1.0 / 60.0));
                scene.draw(&gpu, &target.view, t, 1.0 / 60.0);
            }
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let first = painter.paint(&gpu, &mut encoder, &target.view, [size.0, size.1]);
            gpu.queue.submit(first.into_iter().chain([encoder.finish()]));
            painter.release();
            let rgba = target.read(&gpu).unwrap();
            let path = dir.join(format!("{name}.png"));
            image::RgbaImage::from_raw(size.0, size.1, rgba).unwrap().save(&path).unwrap();
            println!("{}", path.display());
        }
        rust_i18n::set_locale("en");
    }

    #[test]
    fn only_the_two_languages_egui_cannot_draw_borrow_a_face() {
        for lang in ["en", "de", "es", "fr", "it", "pl", "pt", "ru"] {
            assert!(cjk_faces(lang).is_empty(), "{lang}");
        }
        assert!(!cjk_faces("ja").is_empty());
        assert!(!cjk_faces("zh").is_empty());
    }
}
