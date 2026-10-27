//! Exports rendered on the GPU compositor: the frame a monitor would show, read back as sRGB
//! RGBA8 (`filmcraft_export::FrameRenderer`). The app registers one on its wgpu device; the bench
//! on its own. Export settings that need the CPU's picture (HDR output, overlays, a different
//! output geometry) never ask it, and a frame it cannot render (device error, too large, a panic
//! in the GPU stack) falls back to the CPU for that frame or for good.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use eframe::wgpu;
use filmcraft_project::{ItemId, Project};
use filmcraft_render::plan::FramePlan;
use filmcraft_render::{RenderOptions, SourceProvider};
use filmcraft_time::Tick;

/// The GPU compositor exports render with (one at a time: frames queue on the lock).
pub struct GpuExportRenderer {
    compositor: Mutex<Option<filmcraft_gpu::GpuCompositor>>,
    max_texture: u32,
    /// Set by the device's error handler: the GPU is unusable, the CPU renders from then on.
    broken: Arc<AtomicBool>,
    rendered: AtomicU64,
}

impl GpuExportRenderer {
    /// A renderer on `device` (None when the compositor cannot be created there).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, broken: Arc<AtomicBool>) -> Option<Self> {
        let c = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filmcraft_gpu::GpuCompositor::new(device, queue))).ok()?;
        Some(Self { compositor: Mutex::new(Some(c)), max_texture: device.limits().max_texture_dimension_2d, broken, rendered: AtomicU64::new(0) })
    }

    /// Frames rendered so far (diagnostics, tests).
    pub fn frames_rendered(&self) -> u64 {
        self.rendered.load(Ordering::Relaxed)
    }
}

fn plan_side(plan: &FramePlan) -> usize {
    match plan {
        FramePlan::Layers { width, height, layers, .. } => {
            layers.iter().map(|l| l.frame.width.max(l.frame.height) as usize).fold((*width).max(*height), usize::max)
        }
        FramePlan::Image(img) => img.w.max(img.h),
    }
}

impl filmcraft_engine::export::FrameRenderer for GpuExportRenderer {
    fn render_rgba8(&self, project: &Project, seq: ItemId, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider) -> Option<(u32, u32, Vec<u8>)> {
        if self.broken.load(Ordering::Relaxed) {
            return None;
        }
        let plan = filmcraft_render::plan::plan_frame(project, seq, t, opts, sources);
        if plan_side(&plan) > self.max_texture as usize {
            return None;
        }
        let mut guard = self.compositor.lock().unwrap_or_else(PoisonError::into_inner);
        let c = guard.as_mut()?;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.composite(&plan);
            c.read_output()
        })) {
            Ok(out) if !self.broken.load(Ordering::Relaxed) => {
                if out.is_some() {
                    self.rendered.fetch_add(1, Ordering::Relaxed);
                }
                out
            }
            Ok(_) => None,
            Err(_) => {
                log::error!("GPU export rendering failed; rendering exports on the CPU");
                *guard = None;
                None
            }
        }
    }
}
