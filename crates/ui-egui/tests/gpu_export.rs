//! Exports rendered on the GPU compositor (`gpu_export::GpuExportRenderer`, as the app registers
//! it) give the CPU's frames, and exports that need the CPU's picture (overlays, a different
//! output geometry) or turn GPU rendering off never ask the GPU. Skips without a GPU adapter.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eframe::wgpu;
use filmcraft_engine::Session;
use filmcraft_media::FrameRequest;
use filmcraft_time::Tick;
use filmcraft_ui_egui::gpu_export::GpuExportRenderer;
use serde_json::{Value, json};

/// A GPU; software adapters (CI runners) are skipped, as in the golden GPU parity tests.
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    if adapter.get_info().device_type == wgpu::DeviceType::Cpu && std::env::var_os("FILMCRAFT_GPU_TESTS_ON_CPU").is_none() {
        eprintln!("SKIPPED (gpu): software adapter {} (FILMCRAFT_GPU_TESTS_ON_CPU=1 runs these anyway)", adapter.get_info().name);
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / n as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

fn frames(path: &std::path::Path, n: usize, rate: filmcraft_time::FrameRate) -> Vec<Vec<u8>> {
    let bytes: Arc<[u8]> = std::fs::read(path).unwrap().into();
    let src = filmcraft_codecs::open_bytes(&path.to_string_lossy(), bytes).unwrap();
    (0..n).map(|i| src.video_frame(FrameRequest::full(rate.tick_of(i as i64) + Tick(1000))).unwrap().to_rgba8()).collect()
}

#[test]
fn gpu_rendered_exports_match_the_cpu() {
    let Some((d, q)) = device() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let renderer = Arc::new(GpuExportRenderer::new(&d, &q, Arc::new(AtomicBool::new(false))).expect("GPU export renderer"));
    filmcraft_engine::export::register_renderer(Some(renderer.clone()));

    // the demo project's sequence (clips, Lumetri, titles), its first second
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let seq = s.state.active_sequence.expect("demo sequence");
    let rate = s.project.sequence(seq).unwrap().settings.frame_rate;
    let n = (rate.num as f64 / rate.den as f64).round() as usize;
    s.edit("range", |p, _| {
        let q = p.sequence_mut(seq).unwrap();
        q.mark_in = Some(Tick::ZERO);
        q.mark_out = Some(rate.tick_of(n as i64 - 1));
        Ok(())
    })
    .unwrap();
    let dir = std::env::temp_dir().join(format!("fc-gpu-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let export = |s: &mut Session, name: &str, settings: Value| {
        let path = dir.join(name);
        s.execute("file.exportMedia", json!({"path": path, "format": "prores", "wait": true, "settings": settings})).unwrap();
        path
    };

    let before = renderer.frames_rendered();
    let gpu = export(&mut s, "gpu.mov", json!({}));
    assert_eq!(renderer.frames_rendered() - before, n as u64, "every frame rendered on the GPU");
    let cpu = export(&mut s, "cpu.mov", json!({"gpuRendering": false}));
    assert_eq!(renderer.frames_rendered() - before, n as u64, "GPU rendering off: the CPU renders");
    let (g, c) = (frames(&gpu, n, rate), frames(&cpu, n, rate));
    for (i, (a, b)) in g.iter().zip(&c).enumerate() {
        let q = psnr(a, b);
        assert!(q > 40.0, "frame {i}: GPU vs CPU export {q:.1} dB");
    }

    // a timecode burn-in is drawn on the CPU's picture: the CPU renders
    let at = renderer.frames_rendered();
    export(&mut s, "tc.mov", json!({"effects": {"timecode_overlay": {"enabled": true}}}));
    assert_eq!(renderer.frames_rendered(), at, "overlays render on the CPU");
    // another frame size (scaled to fit): the CPU renders
    export(&mut s, "small.mov", json!({"frameSize": [640, 360]}));
    assert_eq!(renderer.frames_rendered(), at, "other geometry renders on the CPU");

    filmcraft_engine::export::register_renderer(None);
    let _ = std::fs::remove_dir_all(&dir);
}
