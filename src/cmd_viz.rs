//! `viz-probe`: the visualizer presets, drawn on this machine's GPU.
//!
//! The presets compile everywhere the tests can reach — naga's frontend
//! and all four of its writers run in CI (`shader::matrix`). What CI cannot
//! reach is a driver: the Metal compiler, a DX12 runtime, a Mesa build, a
//! phone-grade GL on a single-board computer. This asks the one in front of
//! it. It lists the GPU adapters, opens the best, compiles every built-in
//! preset into pipelines, draws each for a couple of seconds of a synthetic
//! groove through the real audio texture, and reports what it cost and
//! whether anything appeared. `--png` keeps the last frame of each, so one
//! glance settles whether "it drew" means what it should. The sibling of
//! `graphics-probe`: a diagnostic for the layer that only misbehaves on
//! somebody else's machine.
//!
//! `WGPU_BACKEND=gl` (or `vulkan`, `metal`, `dx12`) pins the backend, which
//! is how a machine with two is asked about each.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args;

use crate::runtime::block_on;
use crate::shader::audio::AudioTexture;
use crate::shader::library::BUILTIN;
use crate::shader::preset::Preset;
use crate::shader::render::{Gpu, Offscreen};

/// What each preset is drawn at: the shape the presets were composed for,
/// and small enough that a weak GPU answers in seconds.
const SIZE: (u32, u32) = (640, 360);

const RATE: f64 = 44_100.0;
const FPS: f32 = 60.0;

/// A channel value this bright counts a pixel as lit — past the dithering
/// and the faint vignettes that a preset showing nothing still has.
const LIT: u8 = 24;

#[derive(Args)]
pub struct VizProbeArgs {
    /// Also write each preset's last frame to this directory as a PNG
    #[arg(long, value_name = "DIR")]
    pub png: Option<PathBuf>,

    /// Frames to draw per preset, sixty to the simulated second
    #[arg(long, default_value_t = 120)]
    pub frames: u32,
}

pub fn run(args: VizProbeArgs) -> i32 {
    println!("viz-probe: the visualizer presets, on this machine's GPU\n");
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());

    let adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all())).unwrap_or_default();
    if adapters.is_empty() {
        println!("no GPU adapters. The window visualizer needs one; the terminal visualizer does not.");
        return 1;
    }
    println!("adapters:");
    for adapter in &adapters {
        println!("  {}", describe(&adapter.get_info()));
    }

    let options = wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    };
    let adapter = match block_on(instance.request_adapter(&options)) {
        Ok(Ok(adapter)) => adapter,
        Ok(Err(e)) => {
            println!("\nno adapter would serve: {e}");
            return 1;
        }
        Err(e) => {
            println!("\n{e}");
            return 1;
        }
    };
    let info = adapter.get_info();
    println!("\nusing {}; the presets are compiled to {}\n", info.name, language(info.backend));
    let gpu = match Gpu::new(&adapter) {
        Ok(gpu) => gpu,
        Err(e) => {
            println!("{e}");
            return 1;
        }
    };
    if let Some(dir) = &args.png
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        println!("can't make {}: {e}", dir.display());
        return 1;
    }

    let target = Offscreen::new(&gpu, SIZE);
    let frames = args.frames.max(1);
    println!(
        "  {:<34} {:>6} {:>9} {:>10} {:>5}   ({}×{}, {frames} frames each)",
        "preset", "passes", "compile", "per frame", "lit", SIZE.0, SIZE.1
    );
    let mut failed = 0;
    for builtin in &BUILTIN {
        let preset = match Preset::parse(builtin.source) {
            Ok(preset) => preset,
            Err(e) => {
                println!("  {:<34} {e}", builtin.file);
                failed += 1;
                continue;
            }
        };
        let name = format!("{} {}", &builtin.file[..2], preset.title.as_deref().unwrap_or(builtin.file));
        let name: String = name.chars().take(34).collect();

        let started = Instant::now();
        let loaded = gpu.load(&preset, SIZE);
        let compile = started.elapsed();
        let mut scene = match loaded {
            Ok(scene) => scene,
            Err(e) => {
                println!("  {name:<34} does not compile: {e}");
                failed += 1;
                continue;
            }
        };

        let mut groove = Groove::default();
        let mut texture = AudioTexture::new();
        let mut history: Vec<f32> = Vec::new();
        let mut drawing = Duration::ZERO;
        let mut stalled = None;
        for frame in 0..frames {
            for _ in 0..(RATE / f64::from(FPS)) as usize {
                history.push(groove.sample());
            }
            let keep = history.len().saturating_sub(4096);
            history.drain(..keep);
            gpu.upload_audio(texture.update(&history, 1.0 / FPS));

            let started = Instant::now();
            scene.draw(&gpu, &target.view, frame as f32 / FPS, 1.0 / FPS);
            if let Err(e) = Offscreen::finish(&gpu) {
                stalled = Some(e);
                break;
            }
            drawing += started.elapsed();
        }
        let pixels = match stalled.map_or_else(|| target.read(&gpu), Err) {
            Ok(pixels) => pixels,
            Err(e) => {
                println!("  {name:<34} compiled, then the GPU failed: {e}");
                failed += 1;
                continue;
            }
        };

        let lit = pixels.chunks_exact(4).filter(|p| p[0].max(p[1]).max(p[2]) >= LIT).count() as f64
            / f64::from(SIZE.0 * SIZE.1);
        let verdict = if lit < 0.005 {
            failed += 1;
            "drew nothing"
        } else {
            "ok"
        };
        println!(
            "  {name:<34} {:>6} {:>6} ms {:>7.2} ms {:>4.0}%   {verdict}",
            scene.passes(),
            compile.as_millis(),
            drawing.as_secs_f64() * 1000.0 / f64::from(frames),
            lit * 100.0,
        );

        if let Some(dir) = &args.png {
            let path = dir.join(builtin.file.replace(".glsl", ".png"));
            if let Err(e) = image::save_buffer(&path, &pixels, SIZE.0, SIZE.1, image::ExtendedColorType::Rgba8) {
                println!("    (can't write {}: {e})", path.display());
            }
        }
    }

    println!();
    if failed == 0 {
        println!("all {} built-in presets compiled and drew.", BUILTIN.len());
    } else {
        println!("{failed} of {} built-in presets did not draw.", BUILTIN.len());
    }
    println!("(04 Cyber Fuji is vendored but not built in while its license is settled.)");
    i32::from(failed != 0)
}

fn describe(info: &wgpu::AdapterInfo) -> String {
    let kind = match info.device_type {
        wgpu::DeviceType::IntegratedGpu => "integrated GPU",
        wgpu::DeviceType::DiscreteGpu => "discrete GPU",
        wgpu::DeviceType::VirtualGpu => "virtual GPU",
        wgpu::DeviceType::Cpu => "software, on the CPU",
        wgpu::DeviceType::Other => "other",
    };
    let driver = [info.driver.as_str(), info.driver_info.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let driver = if driver.is_empty() { String::new() } else { format!(", {driver}") };
    format!("{} — {}, {kind}{driver}", info.name, info.backend.to_str())
}

/// What naga turns the presets into for a backend's driver to finish.
fn language(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Metal => "Metal Shading Language",
        wgpu::Backend::Dx12 => "HLSL",
        wgpu::Backend::Vulkan => "SPIR-V",
        wgpu::Backend::Gl => "GLSL",
        _ => "whatever this backend reads",
    }
}

/// Something for the presets to react to: a kick every half second, a bass
/// line, a chord, and a hat between the kicks. Made up, but made up the
/// same way every run, so the probe's pictures are too.
#[derive(Default)]
struct Groove {
    n: u64,
    lcg: u32,
}

impl Groove {
    fn sample(&mut self) -> f32 {
        use std::f64::consts::TAU;
        let t = self.n as f64 / RATE;
        self.n += 1;
        self.lcg = self.lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = f64::from(self.lcg >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;

        let beat = t % 0.5;
        let kick = (TAU * 55.0 * beat).sin() * (-beat / 0.12).exp() * 0.8;
        let bass = (TAU * 110.0 * t).sin() * 0.15;
        let chord: f64 = [440.0, 554.37, 659.25].iter().map(|hz| (TAU * hz * t).sin()).sum::<f64>() * 0.06;
        let hat = noise * (-((t + 0.25) % 0.5) / 0.03).exp() * 0.15;
        (kick + bass + chord + hat).clamp(-1.0, 1.0) as f32
    }
}
